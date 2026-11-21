//! Publication recovery integration with a local bare Git remote and a scripted
//! forge CLI, not a live forge. The post-approval child integration is a seeded
//! recovery checkpoint, so this does not exercise child scheduling or SDKs.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn git(dir: &Path, args: &[&str]) -> TestResult<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()?;
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim().into())
}
fn cli(home: &Path, args: &[&str]) -> TestResult<std::process::Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_cruise"))
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env(
            "PATH",
            format!("{}:{}", home.join("bin").display(), std::env::var("PATH")?),
        )
        .env("FORGE_FIXTURE", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("CRUISE_CONFIG")
        .env_remove("CRUISE_DAEMON_PORT")
        .stdin(Stdio::null())
        .args(args)
        .output()?)
}
struct Stop<'a>(&'a Path);
impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = cli(self.0, &["daemon", "stop", "--force"]);
    }
}
fn start(home: &Path) -> TestResult<Value> {
    let out = cli(home, &["daemon", "start", "--port", "0"])?;
    assert!(
        out.status.success(),
        "start: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(serde_json::from_slice(&fs::read(
        home.join("state/cruise/daemon/instance.json"),
    )?)?)
}
async fn snapshot(client: &reqwest::Client, instance: &Value, repo: &Path) -> TestResult<Value> {
    let mut url = reqwest::Url::parse(&format!(
        "{}api/manager/snapshot",
        instance["origin"].as_str().ok_or("missing string field")?
    ))?;
    url.query_pairs_mut()
        .append_pair("cwd", repo.to_str().ok_or("path is not UTF-8")?);
    let response = client
        .get(url)
        .bearer_auth(instance["token"].as_str().ok_or("missing string field")?)
        .send()
        .await?;
    assert!(
        response.status().is_success(),
        "snapshot: {}",
        response.status()
    );
    Ok(response.json().await?)
}
async fn wait_status(
    client: &reqwest::Client,
    instance: &Value,
    repo: &Path,
    wanted: &str,
) -> TestResult<Value> {
    let until = Instant::now() + Duration::from_secs(25);
    loop {
        let snap = snapshot(client, instance, repo).await?;
        let goal = &snap["goals"][0];
        if goal["status"] == wanted {
            return Ok(goal.clone());
        }
        assert!(goal["status"] != "blocked", "blocked: {goal}");
        assert!(
            Instant::now() < until,
            "timed out waiting for {wanted}: {goal}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// The helper uses the actual proposal validation and approval functions before
// the parent writes a simulated already-integrated child recovery checkpoint.
#[test]
fn seed_approved_proposal() -> TestResult {
    let Some(root) = std::env::var_os("MANAGER_FORGE_SEED") else {
        return Ok(());
    };
    let root = std::path::PathBuf::from(root);
    let repo = root.join("repo");
    let sha = git(&repo, &["rev-parse", "HEAD"])?;
    let proposal = cruise::manager::Proposal {
        id: "provider".into(),
        revision: 1,
        objective: "Verified goal".into(),
        target_branch: "main".into(),
        target_sha: sha,
        exclusions: vec![],
        criteria: vec![cruise::manager::Criterion {
            id: "c".into(),
            description: "file exists".into(),
        }],
        checks: vec![cruise::manager::Check {
            id: "check".into(),
            criterion_ids: vec!["c".into()],
            argv: vec!["test".into(), "-f".into(), "change".into()],
        }],
        tasks: vec![cruise::manager::Task {
            id: "task".into(),
            instructions: "add change".into(),
            dependencies: vec![],
        }],
        workflow: "builtin:tdd".into(),
        contract: None,
    };
    let proposal = cruise::manager::save_proposal(&repo, proposal)?;
    let goal = cruise::manager::approve(&repo, &proposal.id, proposal.revision)?;
    fs::write(root.join("goal-id"), goal.id)?;
    Ok(())
}

#[tokio::test]
async fn verified_publication_and_merge_observation_survive_restart() -> TestResult {
    for forge in ["github", "gitlab"] {
        publication_and_restart(forge).await?;
    }
    Ok(())
}

fn seed_fixture(home: &Path, forge: &str) -> TestResult<(String, String)> {
    let repo = home.join("repo");
    let remote = home.join("remote.git");
    fs::create_dir(&repo)?;
    git(
        home,
        &[
            "init",
            "--bare",
            "-q",
            remote.to_str().ok_or("path is not UTF-8")?,
        ],
    )?;
    git(&repo, &["init", "-q", "-b", "main"])?;
    git(&repo, &["config", "user.name", "Test"])?;
    git(&repo, &["config", "user.email", "test@example.test"])?;
    fs::write(repo.join("base"), "base\n")?;
    git(&repo, &["add", "base"])?;
    git(&repo, &["commit", "-qm", "base"])?;
    let base = git(&repo, &["rev-parse", "HEAD"])?;
    let url = if forge == "github" {
        "https://github.com/example/project.git"
    } else {
        "https://gitlab.com/example/project.git"
    };
    git(&repo, &["remote", "add", "origin", url])?;
    // Initial transport is direct. During daemon execution, a narrow git wrapper
    // redirects only push and ls-remote to this local bare repository.
    git(
        &repo,
        &[
            "push",
            "-q",
            remote.to_str().ok_or("path is not UTF-8")?,
            "main",
        ],
    )?;
    git(&repo, &["update-ref", "refs/remotes/origin/main", &base])?;
    fs::create_dir_all(home.join("config/cruise"))?;
    fs::write(
        home.join("config/cruise/default.yaml"),
        "check_base_ci: false\n",
    )?;
    let helper = Command::new(std::env::current_exe()?)
        .args(["--exact", "seed_approved_proposal", "--nocapture"])
        .env("MANAGER_FORGE_SEED", home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("CRUISE_CONFIG")
        .output()?;
    assert!(
        helper.status.success(),
        "seed: {} {}",
        String::from_utf8_lossy(&helper.stdout),
        String::from_utf8_lossy(&helper.stderr)
    );
    let id = fs::read_to_string(home.join("goal-id"))?;
    fs::write(repo.join("change"), "approved result\n")?;
    git(&repo, &["add", "change"])?;
    git(&repo, &["commit", "-qm", "child result"])?;
    let tip = git(&repo, &["rev-parse", "HEAD"])?;
    git(
        &repo,
        &["update-ref", &format!("refs/heads/cruise/goal/{id}"), &tip],
    )?;
    git(&repo, &["reset", "--hard", &base])?;
    let dir = home.join("data/cruise/manager");
    let record = fs::read_dir(&dir)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .ok_or("manager record missing")?;
    let mut state: Value = serde_json::from_slice(&fs::read(&record)?)?;
    let goal = &mut state["goals"][0];
    goal["status"] = json!("active");
    goal["reason"] = Value::Null;
    goal["execution"]["integrated_tip"] = json!(tip);
    goal["execution"]["attempts"] = json!([{"task_id":"task","attempt":1,"session_id":"fixture-child","base_sha":base,"launch_started":true,"result_sha":tip,"integrated":true,"integration_intent":null}]);
    fs::write(&record, serde_json::to_vec_pretty(&state)?)?;
    Ok((id, tip))
}

async fn publication_and_restart(forge: &str) -> TestResult {
    let tmp = TempDir::new()?;
    let home = tmp.path();
    let repo = home.join("repo");
    let remote = home.join("remote.git");
    let url = if forge == "github" {
        "https://github.com/example/project.git"
    } else {
        "https://gitlab.com/example/project.git"
    };
    let (id, tip) = seed_fixture(home, forge)?;
    fs::create_dir(home.join("bin"))?;
    let script = home
        .join("bin")
        .join(if forge == "github" { "gh" } else { "glab" });
    let real_git = String::from_utf8(Command::new("which").arg("git").output()?.stdout)?;
    let git_wrapper = home.join("bin/git");
    fs::write(
        &git_wrapper,
        format!(
            "#!/bin/sh\ncase \"$1\" in push|ls-remote) exec '{}' -c 'url.{}.insteadOf={}' \"$@\" ;; *) exec '{}' \"$@\" ;; esac\n",
            real_git.trim(),
            remote.display(),
            url,
            real_git.trim()
        ),
    )?;
    fs::set_permissions(&git_wrapper, fs::Permissions::from_mode(0o755))?;
    fs::write(
        &script,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$FORGE_FIXTURE/calls\"\ncase \"$1 $2\" in\n  'pr create'|'mr create') touch \"$FORGE_FIXTURE/created\" ;;\n  'api --hostname') case \"$*\" in *'state=all'*) if [ -f \"$FORGE_FIXTURE/created\" ]; then printf '['; cat \"$FORGE_FIXTURE/response\"; printf ']\\n'; else echo '[]'; fi ;; *) cat \"$FORGE_FIXTURE/response\" ;; esac ;;\n  *) echo unexpected-command >&2; exit 1 ;;\nesac\n",
    )?;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
    let response = |state: &str, merged: Value| {
        if forge == "github" {
            json!({"number":7,"html_url":"https://github.com/example/project/pull/7","state":state,"merged_at":merged,"head":{"ref":format!("cruise/goal/{id}"),"sha":tip,"repo":{"full_name":"example/project"}},"base":{"ref":"main","repo":{"full_name":"example/project"}}})
        } else {
            json!({"iid":7,"web_url":"https://gitlab.com/example/project/-/merge_requests/7","state":if state == "open" { "opened" } else if merged.is_null() { "closed" } else { "merged" },"source_project_id":1,"target_project_id":1,"source_branch":format!("cruise/goal/{id}"),"target_branch":"main","sha":tip})
        }
    };
    fs::write(
        home.join("response"),
        serde_json::to_vec(&response("open", Value::Null))?,
    )?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let instance = start(home)?;
    let _stop = Stop(home);
    let goal = wait_status(&client, &instance, &repo, "awaiting_merge").await?;
    assert_eq!(
        goal["execution"]["publication"]["checks"][0]["verified_tip"],
        tip
    );
    assert_eq!(
        git(
            home,
            &[
                "--git-dir",
                remote.to_str().ok_or("path is not UTF-8")?,
                "rev-parse",
                &format!("refs/heads/cruise/goal/{id}")
            ]
        )?,
        tip
    );
    let stopped = cli(home, &["daemon", "stop", "--force"])?;
    assert!(stopped.status.success());
    fs::write(
        home.join("response"),
        serde_json::to_vec(&response("closed", json!("2026-01-01T00:00:00Z")))?,
    )?;
    let instance = start(home)?;
    let completed = wait_status(&client, &instance, &repo, "completed").await?;
    assert_eq!(
        completed["execution"]["publication"]["request"]["head_sha"],
        tip
    );
    let calls = fs::read_to_string(home.join("calls"))?;
    assert_forge_calls(&calls, forge, &id)?;
    Ok(())
}

fn assert_forge_calls(calls: &str, forge: &str, id: &str) -> TestResult {
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("pr create ") || line.starts_with("mr create "))
            .count(),
        1,
        "{calls}"
    );
    let create = calls
        .lines()
        .find(|line| line.starts_with("pr create ") || line.starts_with("mr create "))
        .ok_or("forge create call missing")?;
    let (head_flag, base_flag) = if forge == "github" {
        ("--head", "--base")
    } else {
        ("--source-branch", "--target-branch")
    };
    assert!(
        create.contains(&format!("{head_flag} cruise/goal/{id}")),
        "{create}"
    );
    assert!(create.contains(&format!("{base_flag} main")), "{create}");
    assert!(
        !calls.lines().any(|line| line.contains(" merge")),
        "{calls}"
    );
    Ok(())
}
