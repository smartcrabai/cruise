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

fn record_path(home: &Path, common: &Path) -> Result<std::path::PathBuf> {
    let mut key = String::with_capacity(64);
    for byte in Sha256::digest(common.to_string_lossy().as_bytes()) {
        write!(key, "{byte:02x}")?;
    }
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
  push|fetch) exit 2;;
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
    echo '{"id":1}'
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
}
