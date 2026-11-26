use std::fmt::Write;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn command(home: &Path, args: &[&str]) -> Result<std::process::Output> {
    command_at(home, home, args)
}

fn command_at(home: &Path, cwd: &Path, args: &[&str]) -> Result<std::process::Output> {
    let path = std::env::join_paths(std::iter::once(home.join("bin")).chain(
        std::env::split_paths(&std::env::var_os("PATH").ok_or("missing PATH")?),
    ))?;
    Ok(Command::new(env!("CARGO_BIN_EXE_cruise"))
        .current_dir(cwd)
        .env("HOME", home)
        .env("PATH", path)
        .env("CRUISE_RUNTIME", env!("CARGO_BIN_EXE_cruise-runtime"))
        .env_remove("CRUISE_FORGE")
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("CRUISE_CONFIG")
        .env_remove("CRUISE_DAEMON_PORT")
        .stdin(Stdio::null())
        .args(args)
        .output()?)
}
struct Stop<'a>(&'a Path);
impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = command(self.0, &["daemon", "stop"]);
    }
}
fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()?;
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}

fn sha256(bytes: &[u8]) -> Result<String> {
    let mut key = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(key, "{byte:02x}")?;
    }
    Ok(key)
}

fn record_path(home: &Path, common: &Path) -> Result<std::path::PathBuf> {
    let key = sha256(common.to_string_lossy().as_bytes())?;
    Ok(home.join("data/cruise/manager").join(format!("{key}.json")))
}

// These are persisted recovery fixtures, exercised through the authenticated production API.
#[tokio::test]
async fn retry_refuses_unsafe_recovery_states_without_mutating_them() -> Result<()> {
    let root = TempDir::new()?;
    let home = root.path();
    let repo = home.join("repo");
    fs::create_dir(&repo)?;
    git(&repo, &["init", "-q"])?;
    git(&repo, &["config", "user.name", "Test"])?;
    git(&repo, &["config", "user.email", "test@example.test"])?;
    fs::write(repo.join("file"), "base\n")?;
    git(&repo, &["add", "file"])?;
    git(&repo, &["commit", "-qm", "base"])?;
    let sha = git(&repo, &["rev-parse", "HEAD"])?;
    let common = repo.join(".git").canonicalize()?;
    let anchor = repo.canonicalize()?;
    let origin = "https://github.com/example/project.git";
    let workflow: cruise::config::WorkflowConfig = serde_json::from_value(json!({"steps":{}}))?;
    let workflow = serde_json::to_value(workflow)?;
    let proposal = json!({"id":"p", "revision":1, "objective":"goal", "target_branch":"main", "target_sha":sha,
        "exclusions":[], "criteria":[{"id":"c","description":"works"}],
        "checks":[{"id":"check","criterion_ids":["c"],"argv":["true"]}],
        "tasks":[{"id":"task","instructions":"work","dependencies":[]}], "workflow":"builtin:tdd",
        "contract":{"anchor":anchor,"origin":origin,"forge_kind":"GitHub","forge_host":"github.com","forge_repository":"example/project","workflow":workflow}});
    let publication = json!({"tip":sha,"worktree":home.join("verification"),"branch":"cruise/goal/g", "phase":"CheckIntent", "checks":[], "pending_check":null,"request":null,"forge_kind":"GitHub","forge_host":"github.com","forge_repository":"example/project"});
    let attempt = json!({"task_id":"task","attempt":1,"session_id":"s","base_sha":sha,"launch_started":true,"result_sha":sha,"integrated":true,"integration_intent":null});
    let make_goal = |id: &str| {
        json!({"id":id,"proposal":proposal,"status":"blocked","reason":"blocked",
        "execution":{"anchor":anchor,"origin":origin,"workflow":workflow,"integrated_tip":sha,"attempts":[attempt],"paused_from":null,"paused_reason":null,"publication":publication,"publication_history":[]}})
    };
    let mut goals = vec![];
    let mut g = make_goal("missing-attempt");
    g["execution"]["attempts"] = json!([]);
    goals.push(g);
    let mut g = make_goal("missing-publication");
    g["execution"]["publication"] = Value::Null;
    goals.push(g);
    let mut g = make_goal("tip-mismatch");
    g["execution"]["publication"]["tip"] = json!("different");
    goals.push(g);
    let mut g = make_goal("changed-contract");
    g["execution"]["origin"] = json!("different");
    goals.push(g);
    for phase in [
        "Verified",
        "PushIntent",
        "Pushed",
        "CreateIntent",
        "Published",
    ] {
        let mut g = make_goal(phase);
        g["execution"]["publication"]["phase"] = json!(phase);
        goals.push(g);
    }
    goals.push(make_goal("eligible"));
    let record = record_path(home, &common)?;
    fs::create_dir_all(record.parent().ok_or("record has no parent")?)?;
    fs::write(
        &record,
        serde_json::to_vec_pretty(
            &json!({"repository":common,"transcript":[],"proposal":null,"goals":goals}),
        )?,
    )?;
    let (_stop, instance, client) = start_daemon(home)?;
    let before: Value = serde_json::from_slice(&fs::read(&record)?)?;
    assert_unsafe_retries_rejected(&client, &instance, &repo, &before).await?;
    let after: Value = serde_json::from_slice(&fs::read(&record)?)?;
    assert_eq!(after, before);
    assert_eq!(git(&repo, &["rev-parse", "HEAD"])?, sha);
    assert_eligible_retry(&client, &instance, &repo, &publication).await?;
    Ok(())
}

fn start_daemon(home: &Path) -> Result<(Stop<'_>, Value, reqwest::Client)> {
    let started = command(home, &["daemon", "start", "--port", "0"])?;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let stop = Stop(home);
    let instance =
        serde_json::from_slice(&fs::read(home.join("state/cruise/daemon/instance.json"))?)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    Ok((stop, instance, client))
}

async fn assert_eligible_retry(
    client: &reqwest::Client,
    instance: &Value,
    repo: &Path,
    publication: &Value,
) -> Result<()> {
    let response = client
        .post(format!(
            "{}api/manager/action",
            instance["origin"]
                .as_str()
                .ok_or("missing daemon credential")?
        ))
        .bearer_auth(
            instance["token"]
                .as_str()
                .ok_or("missing daemon credential")?,
        )
        .json(&json!({"cwd":repo,"action":{"type":"retry","goal_id":"eligible"}}))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let goal: Value = response.json().await?;
    assert_eq!(goal["status"], "active");
    assert!(goal["execution"]["publication"].is_null());
    assert_eq!(
        goal["execution"]["publication_history"],
        json!([publication])
    );
    Ok(())
}

async fn assert_unsafe_retries_rejected(
    client: &reqwest::Client,
    instance: &Value,
    repo: &Path,
    before: &Value,
) -> Result<()> {
    for goal in before["goals"].as_array().ok_or("goals is not an array")? {
        if goal["id"] == "eligible" {
            continue;
        }
        for _ in 0..2 {
            let response = client
                .post(format!(
                    "{}api/manager/action",
                    instance["origin"]
                        .as_str()
                        .ok_or("missing daemon credential")?
                ))
                .bearer_auth(
                    instance["token"]
                        .as_str()
                        .ok_or("missing daemon credential")?,
                )
                .json(&json!({"cwd":repo,"action":{"type":"retry","goal_id":goal["id"]}}))
                .send()
                .await?;
            assert!(
                !response.status().is_success(),
                "unsafe retry accepted for {}",
                goal["id"]
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
mod github_publication {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const REJECTED: &str = r#"PR create outcome uncertain (Err("gh failed: pull request create failed: GraphQL: Title is too long (maximum is 256 characters) (createPullRequest)"), lookup Ok(None)). Human resolution required"#;
    const UNWRAPPED_REJECTED: &str = r#"PR create outcome uncertain (Err("gh failed: GraphQL: Title is too long (maximum is 256 characters) (createPullRequest)"), lookup Ok(None)). Human resolution required"#;
    const AMBIGUOUS_REJECTION: &str = r#"PR create outcome uncertain (Err("gh failed: pull request create failed: GraphQL: Title is too long (maximum is 256 characters) (createPullRequest); request timed out"), lookup Ok(None)). Human resolution required"#;

    fn shim(home: &Path, name: &str, script: &str) -> Result<()> {
        let path = home.join("bin").join(name);
        fs::write(&path, script)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    fn forge_fixtures(home: &Path) -> Result<()> {
        fs::create_dir(home.join("bin"))?;
        shim(
            home,
            "git",
            r#"#!/bin/sh
export PATH="${PATH#*:}"
case "$1" in
  ls-remote) test "$3" = origin || exit 2; exec git ls-remote --heads "$HOME/remote" "$4";;
  fetch)
    test -f "$HOME/retarget-fixture" || exit 2
    test "$#" = 10 && test "$9" = origin || exit 2
    exec git fetch "$2" "$3" "$4" "$5" "$6" "$7" "$8" "$HOME/remote" "${10}"
    ;;
  push)
    test -f "$HOME/retarget-fixture" && test "$#" = 3 && test "$2" = origin || exit 2
    printf '%s\n' "$3" >> "$HOME/pushes"
    git push "$HOME/remote" "$3" || exit $?
    if [ -f "$HOME/after-push-drift" ]; then
      git --git-dir="$HOME/remote" update-ref refs/heads/main "$(cat "$HOME/after-push-drift")" || exit $?
    fi
    touch "$HOME/pushed"
    if [ -f "$HOME/hold-push" ]; then while :; do sleep 1; done; fi
    exit 0
    ;;
esac
exec git "$@"
"#,
        )?;
        shim(
            home,
            "gh",
            r#"#!/bin/sh
set -eu
if [ "$1 $2 $3" = 'api --hostname github.com' ]; then
  if [ "$4" = user ]; then
    if [ -f "$HOME/auth-error" ]; then echo 'HTTP 401' >&2; exit 1; fi
    if [ -f "$HOME/other-user" ]; then echo '{"id":2}'; exit; fi
    echo '{"id":1}'
    exit
  fi
  if [ -f "$HOME/retarget-fixture" ] && [ "$4" = repos/example/project/pulls/1 ]; then
    pr=$(cat "$HOME/listed-legacy")
    pr=${pr#\[}
    printf '%s\n' "${pr%\]}"
    exit
  fi
  [ "$4 $5" = '--paginate repos/example/project/pulls?state=all&per_page=100' ]
  if [ -f "$HOME/lookup-error" ]; then echo 'lookup timed out' >&2; exit 1; fi
  for pr in "$HOME"/listed-*; do if [ -f "$pr" ]; then cat "$pr"; fi; done
  echo '[]'
else
  [ "$1 $2 $3 $4 $5 $7 $9 ${11}" = 'pr create --repo github.com/example/project --head --base --title --body' ]
  id=${6##*/}
  printf 'create\n' >> "$HOME/attempt-$id"
  printf '%s' "${10}" > "$HOME/title-$id"
  printf '%s' "${12}" > "$HOME/body-$id"
  case "$id" in
    timeout) echo 'request timed out' >&2; exit 1;;
    rejected) echo 'pull request create failed: GraphQL: Title is too long (maximum is 256 characters) (createPullRequest)' >&2; exit 1;;
  esac
  if [ -f "$HOME/retarget-fixture" ]; then
    if [ -f "$HOME/create-timeout" ]; then echo 'request timed out' >&2; exit 1; fi
    sha=$(git --git-dir="$HOME/remote" rev-parse "refs/heads/$6")
    printf '[{"number":1,"html_url":"https://github.com/example/project/pull/1","state":"open","merged_at":null,"head":{"ref":"%s","sha":"%s","repo":{"full_name":"example/project"}},"base":{"ref":"%s","repo":{"full_name":"example/project"}}}]\n' \
      "$6" "$sha" "$8" > "$HOME/staged-$id"
    mv "$HOME/staged-$id" "$HOME/listed-$id"
    if [ -f "$HOME/hold-create" ]; then while :; do sleep 1; done; fi
    exit 0
  fi
  cp "$HOME/pr-$id" "$HOME/staged-$id"
  mv "$HOME/staged-$id" "$HOME/listed-$id"
fi
"#,
        )
    }

    fn goal_in(record: &Path, id: &str) -> Result<Value> {
        let snapshot: Value = serde_json::from_slice(&fs::read(record)?)?;
        snapshot["goals"]
            .as_array()
            .ok_or("missing goals")?
            .iter()
            .find(|goal| goal["id"] == id)
            .cloned()
            .ok_or_else(|| format!("missing goal {id}").into())
    }

    const ORIGIN: &str = "https://github.com/example/project.git";

    struct LocalGit {
        repo: std::path::PathBuf,
        remote: std::path::PathBuf,
        base: String,
        tip: String,
        worktree: std::path::PathBuf,
    }

    fn setup_git(home: &Path) -> Result<LocalGit> {
        let repo = home.join("repo");
        fs::create_dir(&repo)?;
        git(&repo, &["init", "-q"])?;
        let remote = home.join("remote");
        git(&repo, &["init", "-q", "--bare", &remote.to_string_lossy()])?;
        git(&repo, &["config", "user.name", "Test"])?;
        git(&repo, &["config", "user.email", "test@example.test"])?;
        git(&repo, &["remote", "add", "origin", ORIGIN])?;
        fs::write(repo.join("file"), "base\n")?;
        git(&repo, &["add", "file"])?;
        git(&repo, &["commit", "-qm", "base"])?;
        let base = git(&repo, &["rev-parse", "HEAD"])?;
        fs::write(repo.join("file"), "integrated\n")?;
        git(&repo, &["commit", "-qam", "integrated"])?;
        let tip = git(&repo, &["rev-parse", "HEAD"])?;
        let manager = home.join("data/cruise/manager");
        let worktree = manager.join("verification/work");
        fs::create_dir_all(worktree.parent().ok_or("missing verification parent")?)?;
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--detach",
                &worktree.to_string_lossy(),
                &tip,
            ],
        )?;
        git(
            &repo,
            &[
                "push",
                &remote.to_string_lossy(),
                &format!("{base}:refs/heads/main"),
                &format!("{tip}:refs/heads/moved"),
            ],
        )?;
        Ok(LocalGit {
            repo,
            remote,
            base,
            tip,
            worktree,
        })
    }

    fn seed_goals(
        home: &Path,
        git_repo: &LocalGit,
        titles: &[String; 3],
    ) -> Result<(std::path::PathBuf, Value)> {
        let LocalGit {
            repo,
            remote,
            base,
            tip,
            worktree,
        } = git_repo;
        let manager = home.join("data/cruise/manager");
        let anchor = repo.canonicalize()?;
        let common = repo.join(".git").canonicalize()?;
        let workflow: cruise::config::WorkflowConfig = serde_json::from_value(json!({"steps":{}}))?;
        let workflow = serde_json::to_value(workflow)?;
        let argv = json!(["true", "unmodified approved argument"]);
        let checks = json!([{"check_id":"check", "verified_tip":tip, "argv":argv, "exit_code":0,
            "cancelled":false,"state_error":null,"stdout_log":manager.join("stdout"),"stderr_log":manager.join("stderr")}]);
        fs::write(manager.join("stdout"), "")?;
        fs::write(manager.join("stderr"), "")?;
        let [plain, ascii, unicode] = titles;
        let mut goals = Vec::new();
        for (index, id) in [
            "plain",
            "ascii",
            "unicode",
            "timeout",
            "rejected",
            "legacy",
            "existing",
            "wrong-base",
            "wrong-head",
            "unproven",
            "moved-head",
            "moved-base",
            "missing-checks",
        ]
        .into_iter()
        .enumerate()
        {
            let branch = format!("cruise/goal/{id}");
            let target = if id == "moved-base" { "moved" } else { "main" };
            let objective = match id {
                "plain" => plain,
                "ascii" => ascii,
                _ => unicode,
            };
            let reference = format!("refs/heads/{branch}");
            git(repo, &["update-ref", &reference, tip])?;
            let remote_tip = if id == "moved-head" { base } else { tip };
            git(remote, &["update-ref", &reference, remote_tip])?;
            let proposal = json!({"id":"p","revision":1,"objective":objective,"target_branch":target,"target_sha":base,
                "criteria":[{"id":"c","description":"unchanged contract"}],
                "checks":[{"id":"check","criterion_ids":["c"],"argv":argv}],
                "tasks":[{"id":"task","instructions":"work","dependencies":[]}],"workflow":"builtin:tdd",
                "contract":{"anchor":anchor,"origin":ORIGIN,"forge_kind":"GitHub","forge_host":"github.com","forge_repository":"example/project","workflow":workflow}});
            let (status, phase, reason) =
                if matches!(id, "plain" | "ascii" | "unicode" | "rejected") {
                    ("verifying", "Pushed", None)
                } else {
                    let reason = match id {
                        "existing" => UNWRAPPED_REJECTED,
                        "unproven" => AMBIGUOUS_REJECTION,
                        _ => REJECTED,
                    };
                    ("blocked", "CreateIntent", Some(reason))
                };
            let publication_checks = if id == "missing-checks" {
                json!([])
            } else {
                checks.clone()
            };
            goals.push(json!({"id":id,"proposal":proposal,"status":status,"reason":reason,
                "execution":{"anchor":anchor,"origin":ORIGIN,"workflow":workflow,"integrated_tip":tip,
                "attempts":[{"task_id":"task","attempt":1,"session_id":"s","base_sha":base,"launch_started":true,"result_sha":tip,"integrated":true}],
                "publication":{"tip":tip,"worktree":worktree,"branch":branch,"phase":phase,"checks":publication_checks,
                "pending_check":null,"request":null,"forge_kind":"GitHub","forge_host":"github.com","forge_repository":"example/project"}}}));
            let number = index + 1;
            let pr = json!([{"number":number,"html_url":format!("https://github.com/example/project/pull/{number}"),
                "state":"closed","merged_at":"2026-11-20T00:00:00Z",
                "head":{"ref":branch,"sha":if id == "wrong-head" {base} else {tip},"repo":{"full_name":"example/project"}},
                "base":{"ref":if id == "wrong-base" {"other"} else {target},"repo":{"full_name":"example/project"}}}]);
            let name = if matches!(id, "existing" | "wrong-base" | "wrong-head") {
                "listed"
            } else {
                "pr"
            };
            fs::write(home.join(format!("{name}-{id}")), serde_json::to_vec(&pr)?)?;
        }
        let record = record_path(home, &common)?;
        fs::write(
            &record,
            serde_json::to_vec(&json!({"repository":common,"transcript":[],"goals":goals}))?,
        )?;
        Ok((record, checks))
    }

    #[tokio::test]
    async fn github_title_limit_and_explicit_reconciliation() -> Result<()> {
        let root = TempDir::new()?;
        let home = root.path();
        let git_repo = setup_git(home)?;
        let titles = [
            "\u{5883}".repeat(256),
            "a".repeat(300),
            "\u{1f980}\u{5883}".repeat(150),
        ];
        let (record, checks) = seed_goals(home, &git_repo, &titles)?;
        forge_fixtures(home)?;
        let (_stop, _instance, _client) = start_daemon(home)?;
        for (id, status) in [
            ("plain", "completed"),
            ("ascii", "completed"),
            ("unicode", "completed"),
            ("rejected", "blocked"),
        ] {
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            while goal_in(&record, id)?["status"] != status {
                assert!(
                    std::time::Instant::now() < deadline,
                    "{id} did not reach {status}: {}",
                    goal_in(&record, id)?
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        assert_retries(home, &git_repo.repo, &record, &checks)?;
        assert_titles(home, &record, &titles)
    }

    fn assert_retries(home: &Path, repo: &Path, record: &Path, checks: &Value) -> Result<()> {
        let retry = |id| command_at(home, repo, &["manager", "retry", id]);
        let out = retry("timeout")?;
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let timed_out = goal_in(record, "timeout")?;
        assert!(timed_out["reason"].as_str().is_some_and(|reason| {
            reason.starts_with("PR create outcome uncertain")
                && reason.contains("request timed out")
        }));
        assert!(
            goal_in(record, "rejected")?["reason"]
                .as_str()
                .is_some_and(|reason| reason.starts_with("GitHub rejected the submitted PR title"))
        );
        let refuses = |id| -> Result<()> {
            let before = goal_in(record, id)?;
            let marker = home.join(format!("attempt-{id}"));
            let attempts = fs::read(&marker).ok();
            let out = retry(id)?;
            assert!(!out.status.success(), "unsafe retry accepted for {id}");
            if id == "moved-base" {
                let stderr = String::from_utf8_lossy(&out.stderr);
                assert!(
                    stderr.contains("Remote ref moved differs from verified publication"),
                    "{stderr}"
                );
            }
            assert_eq!(goal_in(record, id)?, before);
            assert_eq!(fs::read(marker).ok(), attempts);
            Ok(())
        };
        for id in [
            "timeout",
            "rejected",
            "wrong-base",
            "wrong-head",
            "unproven",
            "moved-head",
            "moved-base",
            "missing-checks",
        ] {
            refuses(id)?;
        }
        for failure in ["auth-error", "lookup-error"] {
            fs::write(home.join(failure), "")?;
            refuses("legacy")?;
            fs::remove_file(home.join(failure))?;
        }
        for id in ["legacy", "existing"] {
            assert!(!home.join(format!("attempt-{id}")).exists());
            let out = retry(id)?;
            assert!(
                out.status.success(),
                "{id}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(goal_in(record, id)?["status"], "completed");
            assert_eq!(
                &goal_in(record, id)?["execution"]["publication"]["checks"],
                checks
            );
        }
        assert!(!home.join("attempt-existing").exists());
        for id in ["timeout", "rejected"] {
            assert_eq!(
                fs::read_to_string(home.join(format!("attempt-{id}")))?,
                "create\n"
            );
        }
        Ok(())
    }

    fn assert_titles(home: &Path, record: &Path, titles: &[String; 3]) -> Result<()> {
        let [plain, ascii, unicode] = titles;
        for (id, original) in [
            ("plain", plain),
            ("ascii", ascii),
            ("unicode", unicode),
            ("legacy", unicode),
        ] {
            assert_eq!(
                fs::read_to_string(home.join(format!("attempt-{id}")))?,
                "create\n"
            );
            let title = fs::read_to_string(home.join(format!("title-{id}")))?;
            let body = fs::read_to_string(home.join(format!("body-{id}")))?;
            let original_body = "Created by Cruise Manager. Merge manually after review.";
            if id == "plain" {
                assert_eq!(&title, original);
                assert_eq!(body, original_body);
            } else {
                let prefix: String = original.chars().take(255).collect();
                assert_eq!(title, format!("{prefix}…"));
                assert_eq!(
                    body,
                    format!("{original_body}\n\n## Goal objective\n\n{original}")
                );
            }
            assert_eq!(goal_in(record, id)?["proposal"]["objective"], *original);
        }
        Ok(())
    }

    const HEAD: &str = "refs/heads/cruise/goal/legacy";
    const MAIN: &str = "refs/heads/main";
    const TRACKING: &str = "refs/remotes/origin/main";

    struct RetargetFixture {
        git: LocalGit,
        record: std::path::PathBuf,
        writer: std::path::PathBuf,
        new_base: String,
        objective: String,
    }

    impl RetargetFixture {
        fn new(home: &Path) -> Result<Self> {
            let git_repo = setup_git(home)?;
            let objective = "Long objective. ".repeat(20);
            let (record, _) = seed_goals(
                home,
                &git_repo,
                &["Short".into(), objective.clone(), objective.clone()],
            )?;
            let mut snapshot: Value = serde_json::from_slice(&fs::read(&record)?)?;
            let mut goal = goal_in(&record, "legacy")?;
            let script = r#"
                tip=$(git rev-parse HEAD) || exit
                printf '%s:%s:%s\n' "$1" "$2" "$tip" >> "$HOME/checks"
                if [ "$1" = second ]; then
                    if [ -f "$HOME/drift" ]; then
                        git --git-dir="$HOME/remote" update-ref refs/heads/main "$(cat "$HOME/drift")" || exit
                    fi
                    if [ -f "$HOME/check-error" ]; then exit 7; fi
                fi
            "#;
            let mut checks = Vec::new();
            let mut evidence = Vec::new();
            for check_id in ["first", "second"] {
                let argv = json!(["sh", "-c", script, "check", check_id, "approved argument"]);
                checks.push(json!({"id":check_id,"criterion_ids":["c"],"argv":argv}));
                let mut passed = goal["execution"]["publication"]["checks"][0].clone();
                passed["check_id"] = json!(check_id);
                passed["argv"] = argv;
                evidence.push(passed);
            }
            goal["proposal"]["checks"] = json!(checks);
            goal["execution"]["publication"]["checks"] = json!(evidence);
            goal["execution"]["publication"]["contract_revision"] = json!(1);
            let amendment = json!({"id":"amendment","revision":1,"goal_id":"legacy",
                "integrated_tip":git_repo.tip,"amendment":[{"criterion":"c",
                "old_sha256":sha256(b"unchanged contract")?,
                "replace":"unchanged contract","with":"reviewed contract"}]});
            goal["execution"]["corrective_approvals"] = json!([amendment]);
            let mut failed_attempt = goal["execution"]["attempts"][0].clone();
            failed_attempt["session_id"] = json!("earlier-attempt");
            failed_attempt["integrated"] = json!(false);
            failed_attempt["result_sha"] = Value::Null;
            goal["execution"]["attempts"][0]["attempt"] = json!(2);
            goal["execution"]["attempts"] =
                json!([failed_attempt, goal["execution"]["attempts"][0]]);
            snapshot["goals"] = json!([
                goal,
                goal_in(&record, "existing")?,
                goal_in(&record, "unproven")?,
                goal_in(&record, "moved-head")?,
                goal_in(&record, "missing-checks")?
            ]);
            let snapshot: cruise::manager::Snapshot = serde_json::from_value(snapshot)?;
            fs::write(&record, serde_json::to_vec(&snapshot)?)?;
            let writer = home.join("writer");
            git(
                home,
                &[
                    "clone",
                    "-qb",
                    "main",
                    &git_repo.remote.to_string_lossy(),
                    &writer.to_string_lossy(),
                ],
            )?;
            git(&writer, &["config", "user.name", "Test"])?;
            git(&writer, &["config", "user.email", "test@example.test"])?;
            fs::write(writer.join("upstream"), "new upstream content\n")?;
            git(&writer, &["add", "upstream"])?;
            git(&writer, &["commit", "-qm", "Advance target"])?;
            git(&writer, &["push", "origin", MAIN])?;
            let new_base = git(&writer, &["rev-parse", "HEAD"])?;
            git(&git_repo.repo, &["update-ref", TRACKING, &git_repo.base])?;
            forge_fixtures(home)?;
            fs::write(home.join("retarget-fixture"), "")?;
            Ok(Self {
                git: git_repo,
                record,
                writer,
                new_base,
                objective,
            })
        }

        fn preview(&self, home: &Path, id: &str) -> Result<std::process::Output> {
            command_at(
                home,
                &self.git.repo,
                &["manager", "corrective", id, "--retarget"],
            )
        }

        fn approve(&self, home: &Path, revision: u64) -> Result<std::process::Output> {
            command_at(
                home,
                &self.git.repo,
                &[
                    "manager",
                    "approve-corrective",
                    "legacy",
                    "--yes",
                    "--revision",
                    &revision.to_string(),
                ],
            )
        }

        fn goal(&self) -> Result<Value> {
            goal_in(&self.record, "legacy")
        }

        fn refs_unchanged(&self) -> Result<()> {
            assert_eq!(git(&self.git.repo, &["rev-parse", HEAD])?, self.git.tip);
            assert_eq!(
                git(&self.git.repo, &["rev-parse", TRACKING])?,
                self.git.base
            );
            assert_eq!(git(&self.git.remote, &["rev-parse", HEAD])?, self.git.tip);
            Ok(())
        }
    }

    async fn wait_until(mut complete: impl FnMut() -> Result<bool>) -> Result<()> {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !complete()? {
            assert!(
                std::time::Instant::now() < deadline,
                "retarget transition timed out"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Ok(())
    }

    fn succeeded(out: std::process::Output) -> String {
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap_or_else(|e| panic!("invalid CLI output: {e}"))
    }

    #[tokio::test]
    async fn approved_retarget_preserves_history_and_recovers_remote_intents_once() -> Result<()> {
        let root = TempDir::new()?;
        let home = root.path();
        let f = RetargetFixture::new(home)?;
        let before = f.goal()?;
        fs::write(home.join("hold-push"), "")?;
        fs::write(home.join("hold-create"), "")?;
        let (stop, instance, client) = start_daemon(home)?;

        let text = succeeded(f.preview(home, "legacy")?);
        assert!(text.contains("Advance target") && text.contains("upstream"));
        let staged = f.goal()?;
        let preview = &staged["execution"]["corrective_preview"];
        assert_eq!(preview["retarget"]["new_base"], f.new_base);
        assert_eq!(preview["retarget"]["old_base"], f.git.base);
        assert_eq!(preview["integrated_tip"], f.git.tip);
        assert_eq!(preview["contract_revision"], 1);
        let mut unchanged = staged.clone();
        unchanged["execution"]["corrective_preview"] = Value::Null;
        assert_eq!(unchanged, before);
        f.refs_unchanged()?;
        assert!(!home.join("checks").exists());
        assert!(!home.join("pushes").exists());
        let revision = preview["revision"].as_u64().ok_or("missing revision")?;
        succeeded(f.approve(home, revision)?);
        wait_until(|| Ok(home.join("pushed").exists())).await?;
        assert_eq!(f.goal()?["execution"]["publication"]["phase"], "PushIntent");
        let duplicate = client
            .post(format!(
                "{}api/manager/action",
                instance["origin"].as_str().ok_or("missing origin")?
            ))
            .bearer_auth(instance["token"].as_str().ok_or("missing token")?)
            .json(
                &json!({"cwd":f.git.repo,"action":{"type":"approve_corrective","goal_id":"legacy",
                "preview_id":preview["id"],"revision":revision}}),
            )
            .send()
            .await?;
        assert_eq!(duplicate.status(), StatusCode::ACCEPTED);
        drop(stop);

        let (stop, _, _) = start_daemon(home)?;
        wait_until(|| Ok(home.join("listed-legacy").exists())).await?;
        assert_eq!(
            f.goal()?["execution"]["publication"]["phase"],
            "CreateIntent"
        );
        drop(stop);
        let (_stop, _, _) = start_daemon(home)?;
        wait_until(|| Ok(f.goal()?["status"] == "awaiting_merge")).await?;
        let after = f.goal()?;
        assert_retarget_publication(home, &f, &before, preview, &after)
    }

    fn assert_retarget_publication(
        home: &Path,
        f: &RetargetFixture,
        before: &Value,
        preview: &Value,
        after: &Value,
    ) -> Result<()> {
        let tip = after["execution"]["integrated_tip"]
            .as_str()
            .ok_or("missing tip")?;
        assert_eq!(
            git(&f.git.repo, &["show", "-s", "--format=%P", tip])?,
            format!("{} {}", f.git.tip, f.new_base)
        );
        assert_eq!(git(&f.git.remote, &["rev-parse", HEAD])?, tip);
        assert_eq!(git(&f.git.remote, &["rev-parse", MAIN])?, f.new_base);
        assert_eq!(git(&f.git.repo, &["rev-parse", TRACKING])?, f.git.base);
        assert_eq!(after["proposal"], before["proposal"]);
        assert_eq!(
            after["execution"]["attempts"],
            before["execution"]["attempts"]
        );
        assert_eq!(
            after["execution"]["corrective_approvals"],
            json!([before["execution"]["corrective_approvals"][0], preview])
        );
        assert_eq!(
            after["execution"]["publication_history"],
            json!([before["execution"]["publication"]])
        );
        let current = &after["execution"]["publication"];
        assert_eq!(current["request"]["head_sha"], tip);
        for (fresh, prior) in current["checks"]
            .as_array()
            .ok_or("missing checks")?
            .iter()
            .zip(
                before["execution"]["publication"]["checks"]
                    .as_array()
                    .ok_or("missing prior evidence")?,
            )
        {
            assert_eq!(fresh["verified_tip"], tip);
            assert_eq!(fresh["argv"], prior["argv"]);
            assert_eq!(fresh["exit_code"], 0);
            assert_ne!(fresh["stdout_log"], prior["stdout_log"]);
        }
        assert_eq!(
            fs::read_to_string(home.join("checks"))?,
            format!("first:approved argument:{tip}\nsecond:approved argument:{tip}\n")
        );
        assert_eq!(
            fs::read_to_string(home.join("pushes"))?,
            format!("{tip}:{HEAD}\n")
        );
        assert_eq!(fs::read_to_string(home.join("attempt-legacy"))?, "create\n");
        assert_eq!(
            fs::read_to_string(home.join("title-legacy"))?
                .chars()
                .count(),
            256
        );
        assert!(fs::read_to_string(home.join("body-legacy"))?.contains(&f.objective));
        Ok(())
    }

    #[test]
    fn retarget_refuses_conflict_stale_refs_prs_and_unproven_absence() -> Result<()> {
        let root = TempDir::new()?;
        let home = root.path();
        let f = RetargetFixture::new(home)?;
        let (_stop, _, _) = start_daemon(home)?;
        for id in ["existing", "unproven", "moved-head", "missing-checks"] {
            let saved = goal_in(&f.record, id)?;
            assert!(!f.preview(home, id)?.status.success(), "{id} accepted");
            assert_eq!(goal_in(&f.record, id)?, saved);
        }
        succeeded(f.preview(home, "legacy")?);
        let reviewed = f.goal()?;
        let revision = reviewed["execution"]["corrective_preview"]["revision"]
            .as_u64()
            .ok_or("missing revision")?;
        for fault in ["lookup-error", "auth-error", "other-user", "listed-legacy"] {
            let contents = if fault == "listed-legacy" {
                fs::read(home.join("pr-legacy"))?
            } else {
                Vec::new()
            };
            fs::write(home.join(fault), contents)?;
            assert!(
                !f.approve(home, revision)?.status.success(),
                "{fault} accepted"
            );
            assert_eq!(f.goal()?, reviewed);
            fs::remove_file(home.join(fault))?;
        }
        fs::write(f.writer.join("file"), "conflicting upstream edit\n")?;
        git(&f.writer, &["commit", "-qam", "Conflicting edit"])?;
        git(&f.writer, &["push", "origin", MAIN])?;
        assert!(!f.approve(home, revision)?.status.success());
        assert_eq!(f.goal()?, reviewed);
        let text = succeeded(f.preview(home, "legacy")?);
        assert!(text.contains("CONFLICT") && text.contains("Conflicting edit"));
        let conflict = f.goal()?;
        assert_eq!(
            conflict["execution"]["corrective_discarded"],
            json!([reviewed["execution"]["corrective_preview"]])
        );
        let current = conflict["execution"]["corrective_preview"]["revision"]
            .as_u64()
            .ok_or("missing revision")?;
        assert!(!f.approve(home, revision)?.status.success());
        assert!(!f.approve(home, current)?.status.success());
        assert_eq!(f.goal()?, conflict);
        let tree = git(&f.writer, &["rev-parse", "HEAD^{tree}"])?;
        let unrelated = git(&f.writer, &["commit-tree", &tree, "-m", "Unrelated root"])?;
        git(
            &f.writer,
            &["push", "origin", &format!("+{unrelated}:{MAIN}")],
        )?;
        let output = f.preview(home, "legacy")?;
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("not a fast-forward"));
        assert_eq!(f.goal()?, conflict);
        f.refs_unchanged()?;
        assert!(!home.join("checks").exists());
        assert!(!home.join("attempt-legacy").exists());
        Ok(())
    }

    #[tokio::test]
    async fn retarget_stops_on_red_checks_remote_drift_or_uncertain_create() -> Result<()> {
        for (fault, phase, reason) in [
            ("check-error", "Ready", "second failed"),
            ("drift", "Verified", "Remote ref main differs"),
            ("after-push-drift", "Pushed", "Remote ref main differs"),
            ("create-timeout", "CreateIntent", "outcome uncertain"),
        ] {
            let root = TempDir::new()?;
            let home = root.path();
            let f = RetargetFixture::new(home)?;
            fs::write(home.join(fault), &f.git.base)?;
            let (stop, _, _) = start_daemon(home)?;
            succeeded(f.preview(home, "legacy")?);
            let revision = f.goal()?["execution"]["corrective_preview"]["revision"]
                .as_u64()
                .ok_or("missing revision")?;
            succeeded(f.approve(home, revision)?);
            wait_until(|| Ok(f.goal()?["status"] == "blocked")).await?;
            let blocked = f.goal()?;
            assert_eq!(
                blocked["execution"]["publication"]["phase"], phase,
                "{blocked}"
            );
            assert!(
                blocked["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(reason),
                "{blocked}"
            );
            drop(stop);
            let (_stop, _, _) = start_daemon(home)?;
            assert_eq!(f.goal()?, blocked);
            if fault == "create-timeout" {
                let retry = command_at(home, &f.git.repo, &["manager", "retry", "legacy"])?;
                assert!(!retry.status.success());
                assert_eq!(fs::read_to_string(home.join("attempt-legacy"))?, "create\n");
            } else if fault == "after-push-drift" {
                let tip = blocked["execution"]["integrated_tip"]
                    .as_str()
                    .ok_or("missing tip")?;
                assert_eq!(git(&f.git.remote, &["rev-parse", HEAD])?, tip);
                assert_eq!(
                    fs::read_to_string(home.join("pushes"))?,
                    format!("{tip}:{HEAD}\n")
                );
                assert!(!home.join("attempt-legacy").exists());
            } else {
                assert!(!home.join("pushes").exists());
                assert!(!home.join("attempt-legacy").exists());
                assert_eq!(git(&f.git.remote, &["rev-parse", HEAD])?, f.git.tip);
            }
        }
        Ok(())
    }
}
