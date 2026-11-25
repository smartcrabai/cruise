//! Owner-approved Manager corrections against a real daemon and CLI in an isolated HOME.
//! Children and checks are shell scripts, the forge is a scripted `gh`, and the remote is a
//! local bare repository. Nothing here reaches a provider or the network.
#![cfg(unix)]

use cruise::manager::{self, Proposal};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const WAIT: Duration = Duration::from_secs(90);
const ORIGIN: &str = "https://github.com/example/project.git";
const FIX: [&str; 4] = ["t1", "--resume", "--instructions", "WRITE: fixed"];
const EVIDENCE: [&str; 6] = ["t1", "--evidence", "--criterion", "c1", "--path", "file"];

/// Stands in for the agent: it does only what the plan it is given says.
const CHILD: &str = r#"#!/bin/sh
plan=$1
task=$(sed -n 's/.*"task_id":"\([^"]*\)".*/\1/p' "$HOME/data/cruise/sessions/${PWD##*/}/manager-owner.json")
echo "$task" >> "$HOME/child-runs"
cp "$plan" "$HOME/plan-$task"
while [ -f "$HOME/hold-$task" ]; do sleep 0.1; done
sed -n 's/^WRITE: //p' "$plan" | while read -r name; do echo "$task" > "$name"; done
"#;

/// An approved check: its result is whatever the fixture arranged for its name.
const CHECK: &str = r#"#!/bin/sh
echo "$1 $(git rev-parse HEAD)" >> "$HOME/check-runs"
while [ -f "$HOME/hold-check-$1" ]; do sleep 0.1; done
[ ! -f "$HOME/fail-check-$1" ]
"#;

/// A forge that records calls and reports the pushed Goal branch as one open PR.
const GH: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$HOME/gh-calls"
id=$(cat "$HOME/goal-id")
sha=$(git --git-dir "$HOME/remote.git" rev-parse "refs/heads/cruise/goal/$id" 2>/dev/null)
pr="{\"number\":7,\"html_url\":\"https://github.com/example/project/pull/7\",\"state\":\"open\",\"merged_at\":null,\"head\":{\"ref\":\"cruise/goal/$id\",\"sha\":\"$sha\",\"repo\":{\"full_name\":\"example/project\"}},\"base\":{\"ref\":\"main\",\"repo\":{\"full_name\":\"example/project\"}}}"
case "$1 $2" in
  'pr create') touch "$HOME/pr-created" ;;
  'api --hostname') case "$*" in
      *'state=all'*) if [ -f "$HOME/pr-created" ]; then printf '[%s]\n' "$pr"; else echo '[]'; fi ;;
      *) printf '%s\n' "$pr" ;;
    esac ;;
  *) echo unexpected-command >&2; exit 1 ;;
esac
"#;

fn git(dir: &Path, args: &[&str]) -> TestResult<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()?;
    if !out.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(String::from_utf8(out.stdout)?.trim().into())
}

fn script(path: &Path, body: &str) -> TestResult {
    fs::write(path, body)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn sha256(text: &str) -> String {
    use std::fmt::Write as _;
    Sha256::digest(text.as_bytes())
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

fn isolated(command: &mut Command, home: &Path) -> TestResult {
    command
        .env_clear()
        .env(
            "PATH",
            format!("{}:{}", home.join("bin").display(), std::env::var("PATH")?),
        )
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("LANG", "en_US.UTF-8")
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    Ok(())
}

// This subprocess only saves a preview with the real proposal validator and contract
// freezer. Approval and all execution belong to the real daemon.
#[test]
fn save_corrections_proposal_fixture() -> TestResult {
    let Some(proposal) = std::env::var_os("CORRECTIONS_PROPOSAL") else {
        return Ok(());
    };
    let proposal: Proposal = serde_json::from_slice(&fs::read(proposal)?)?;
    manager::save_proposal(&std::env::current_dir()?, proposal)?;
    Ok(())
}

struct Fixture {
    _root: TempDir,
    home: PathBuf,
    repo: PathBuf,
    goal: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.cruise(&["daemon", "stop", "--force"]);
    }
}

fn failing(id: &str) -> Value {
    json!({"id":id, "instructions":format!("Nothing is left to change for {id}"), "dependencies":[]})
}

fn writing(id: &str, dependencies: &[&str]) -> Value {
    json!({"id":id, "instructions":format!("Do the work of {id}\nWRITE: {id}-result"), "dependencies":dependencies})
}

/// The latest attempt of a task in a Goal snapshot.
fn attempt(goal: &Value, task: &str) -> Value {
    goal["execution"]["attempts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|attempt| attempt["task_id"] == task)
        .max_by_key(|attempt| attempt["attempt"].as_u64())
        .cloned()
        .unwrap_or(Value::Null)
}

fn integrated(task: &'static str) -> impl Fn(&Value) -> bool {
    move |goal| attempt(goal, task)["integrated"] == true
}

/// What approving a resume correction writes, as if the daemon stopped right after it.
fn approve_in_record(state: &mut Value, task: &str, started: bool) {
    let execution = &mut state["goals"][0]["execution"];
    let preview = execution["corrective_preview"].take();
    let revision = preview["revision"].clone();
    if let Some(approvals) = execution["corrective_approvals"].as_array_mut() {
        approvals.push(preview);
    }
    for attempt in execution["attempts"].as_array_mut().into_iter().flatten() {
        if attempt["task_id"] == task && started {
            attempt["corrections"] = json!([revision]);
            attempt["resume_starting"] = json!(true);
        } else if attempt["task_id"] == task {
            attempt["correction_pending"] = revision.clone();
            attempt["resume_pending"] = json!(true);
        }
    }
}

impl Fixture {
    /// Two criteria, three checks, and a Goal of `tasks` approved through the CLI. The
    /// children of `held` tasks wait until their hold file is removed.
    fn new(tasks: &Value, held: &[&str]) -> TestResult<Self> {
        let root = TempDir::new()?;
        let home = root.path().canonicalize()?;
        let (repo, remote) = (home.join("repo"), home.join("remote.git"));
        fs::create_dir_all(home.join("config/cruise"))?;
        fs::write(
            home.join("config/cruise/default.yaml"),
            "check_base_ci: false\n",
        )?;
        fs::create_dir_all(home.join("bin"))?;
        let real_git = String::from_utf8(Command::new("which").arg("git").output()?.stdout)?;
        // Only push and ls-remote are redirected, to the local bare remote.
        script(
            &home.join("bin/git"),
            &format!(
                "#!/bin/sh\ncase \"$1\" in push|ls-remote) exec '{git}' -c 'url.{}.insteadOf={ORIGIN}' \"$@\" ;; *) exec '{git}' \"$@\" ;; esac\n",
                remote.display(),
                git = real_git.trim(),
            ),
        )?;
        script(&home.join("bin/gh"), GH)?;
        script(&home.join("child.sh"), CHILD)?;
        script(&home.join("check.sh"), CHECK)?;
        for task in held {
            fs::write(home.join(format!("hold-{task}")), "")?;
        }
        fs::create_dir(&repo)?;
        let remote = remote.display().to_string();
        git(&home, &["init", "--bare", "-q", &remote])?;
        git(&repo, &["init", "-q", "-b", "main"])?;
        git(&repo, &["config", "user.name", "Test"])?;
        git(&repo, &["config", "user.email", "test@example.test"])?;
        fs::write(repo.join("file"), "base\n")?;
        git(&repo, &["add", "file"])?;
        git(&repo, &["commit", "-qm", "base"])?;
        git(&repo, &["remote", "add", "origin", ORIGIN])?;
        git(&repo, &["push", "-q", &remote, "main"])?;
        git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"])?;
        let mut fixture = Self {
            _root: root,
            home,
            repo,
            goal: String::new(),
        };
        fixture.seed(tasks)?;
        fixture.start()?;
        let revision = fixture.snapshot()?["proposal"]["revision"].to_string();
        fixture.ok(&["approve", "--revision", &revision, "--yes"])?;
        fixture.goal = fixture.snapshot()?["goals"][0]["id"]
            .as_str()
            .ok_or("approved Goal has no ID")?
            .into();
        fs::write(fixture.home.join("goal-id"), &fixture.goal)?;
        Ok(fixture)
    }

    fn edit_record(&self, edit: impl FnOnce(&mut Value)) -> TestResult {
        let record = fs::read_dir(self.home.join("data/cruise/manager"))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "json"))
            .ok_or("Manager record missing")?;
        let mut state: Value = serde_json::from_slice(&fs::read(&record)?)?;
        edit(&mut state);
        fs::write(record, serde_json::to_vec_pretty(&state)?)?;
        Ok(())
    }

    fn seed(&self, tasks: &Value) -> TestResult {
        let check = |id: &str, criteria: &[&str]| json!({"id":id, "criterion_ids":criteria, "argv":["sh", self.home.join("check.sh"), id]});
        let proposal = json!({
            "id":"", "revision":0, "objective":"Deliver the reviewed goal", "target_branch":"main",
            "target_sha":git(&self.repo, &["rev-parse", "HEAD"])?, "exclusions":["Do not change the public API"],
            "criteria":[{"id":"c1","description":"The first requirement holds"},{"id":"c2","description":"The second requirement holds"}],
            "checks":[check("k1", &["c1"]), check("k2", &["c1", "c2"]), check("k3", &["c2"])],
            "tasks":tasks, "workflow":"builtin:tdd", "contract":null,
        });
        fs::write(
            self.home.join("proposal.json"),
            serde_json::to_vec(&proposal)?,
        )?;
        let mut helper = Command::new(std::env::current_exe()?);
        isolated(&mut helper, &self.home)?;
        let out = helper
            .current_dir(&self.repo)
            .args(["--exact", "save_corrections_proposal_fixture"])
            .env("CORRECTIONS_PROPOSAL", self.home.join("proposal.json"))
            .output()?;
        if !out.status.success() {
            return Err(format!("seed: {out:?}").into());
        }
        // Children run a deterministic command instead of the built-in agent workflow.
        let workflow: cruise::config::WorkflowConfig = serde_json::from_value(json!({
            "create_pr":false, "cleanup_after_pr":false,
            "steps":{"implement":{
                "command":format!("sh '{}' {{plan}}", self.home.join("child.sh").display()),
                "if":{"no-file-changes":"failed"},
            }}
        }))?;
        self.edit_record(|state| {
            state["proposal"]["contract"]["workflow"] = json!(workflow);
        })
    }

    fn cruise(&self, args: &[&str]) -> TestResult<Output> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        isolated(&mut command, &self.home)?;
        Ok(command.current_dir(&self.repo).args(args).output()?)
    }

    fn start(&self) -> TestResult {
        let out = self.cruise(&["daemon", "start", "--port", "0"])?;
        if !out.status.success() {
            return Err(format!("daemon start: {out:?}").into());
        }
        Ok(())
    }

    fn stop(&self) -> TestResult {
        let out = self.cruise(&["daemon", "stop", "--force"])?;
        if !out.status.success() {
            return Err(format!("daemon stop: {out:?}").into());
        }
        Ok(())
    }

    /// Kill the daemon without letting it record anything, as a crash would.
    fn kill(&self) -> TestResult {
        let instance: Value = serde_json::from_slice(&fs::read(
            self.home.join("state/cruise/daemon/instance.json"),
        )?)?;
        let pid = instance["pid"].to_string();
        if !Command::new("kill").args(["-9", &pid]).status()?.success() {
            return Err("could not kill the daemon".into());
        }
        let deadline = Instant::now() + WAIT;
        while Command::new("kill")
            .args(["-0", &pid])
            .output()?
            .status
            .success()
        {
            if Instant::now() > deadline {
                return Err("the killed daemon is still running".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    fn manager(&self, args: &[&str]) -> TestResult<Output> {
        let mut all = vec!["manager"];
        all.extend_from_slice(args);
        self.cruise(&all)
    }

    /// A Manager command that must succeed; its stdout.
    fn ok(&self, args: &[&str]) -> TestResult<String> {
        let out = self.manager(args)?;
        if !out.status.success() {
            return Err(format!(
                "manager {args:?} failed: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
            .into());
        }
        Ok(String::from_utf8(out.stdout)?)
    }

    /// A Manager command that must be refused with `reason`.
    fn refused(&self, args: &[&str], reason: &str) -> TestResult {
        let out = self.manager(args)?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        if out.status.success() || !stderr.contains(reason) {
            return Err(
                format!("manager {args:?} was not refused with {reason:?}: {out:?}").into(),
            );
        }
        Ok(())
    }

    fn snapshot(&self) -> TestResult<Value> {
        Ok(serde_json::from_str(&self.ok(&["status", "--json"])?)?)
    }

    fn goal(&self) -> TestResult<Value> {
        Ok(self.snapshot()?["goals"][0].clone())
    }

    /// Poll the Goal until `done`, reporting the daemon's view when it never is.
    fn wait(&self, what: &str, done: impl Fn(&Value) -> bool) -> TestResult<Value> {
        let deadline = Instant::now() + WAIT;
        loop {
            let goal = self.goal()?;
            if done(&goal) {
                return Ok(goal);
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "timed out waiting for {what}: goal={goal:#} runs={} checks={} daemon={}",
                    text(&self.home.join("child-runs")),
                    text(&self.home.join("check-runs")),
                    text(&self.home.join("state/cruise/daemon/daemon.log"))
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Wait until the latest attempt of `task` has failed and the daemon has reported it.
    fn wait_failed(&self, task: &str, number: u64) -> TestResult<Value> {
        self.wait(&format!("{task} #{number} to fail"), |goal| {
            let attempt = attempt(goal, task);
            attempt["attempt"] == number
                && attempt["child_state"]["phase"] == "Failed"
                && !attempt["resume_pending"].as_bool().unwrap_or(false)
                && !attempt["resume_starting"].as_bool().unwrap_or(false)
                && goal["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains(&format!("failure for task {task} ")))
        })
    }

    fn release(&self, hold: &str) -> TestResult {
        Ok(fs::remove_file(self.home.join(hold))?)
    }

    fn lines(&self, file: &str, start: &str) -> usize {
        text(&self.home.join(file))
            .lines()
            .filter(|line| line.split(' ').next() == Some(start))
            .count()
    }

    fn worktree(&self, goal: &Value, task: &str) -> PathBuf {
        let session = attempt(goal, task)["session_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        self.home.join("data/cruise/worktrees").join(session)
    }

    /// Save a corrective preview through the CLI and return its revision.
    fn preview(&self, args: &[&str]) -> TestResult<String> {
        let mut all = vec!["corrective", self.goal.as_str()];
        all.extend_from_slice(args);
        self.ok(&all)?;
        Ok(self.goal()?["execution"]["corrective_preview"]["revision"].to_string())
    }

    fn approval<'a>(&'a self, revision: &'a str) -> [&'a str; 5] {
        [
            "approve-corrective",
            &self.goal,
            "--revision",
            revision,
            "--yes",
        ]
    }

    fn amendment(&self, name: &str, edits: &Value) -> TestResult<String> {
        let file = self.home.join(name);
        fs::write(&file, serde_json::to_vec(edits)?)?;
        Ok(file.display().to_string())
    }

    /// Send one Manager action straight to the daemon API.
    async fn action(&self, action: &Value) -> TestResult<reqwest::Response> {
        let instance: Value = serde_json::from_slice(&fs::read(
            self.home.join("state/cruise/daemon/instance.json"),
        )?)?;
        let origin = instance["origin"].as_str().ok_or("daemon origin")?;
        Ok(reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()?
            .post(format!("{origin}api/manager/action"))
            .bearer_auth(instance["token"].as_str().ok_or("daemon token")?)
            .json(&json!({"cwd":self.repo, "action":action}))
            .send()
            .await?)
    }
}

#[tokio::test]
async fn approved_correction_resumes_the_failed_attempt_in_place_exactly_once() -> TestResult {
    let fixture = Fixture::new(&json!([failing("t1"), writing("t2", &["t1"])]), &[])?;
    let failed = fixture.wait_failed("t1", 1)?;
    assert!(
        failed["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("made no workspace file changes")),
        "{failed}"
    );
    let worktree = fixture.worktree(&failed, "t1");
    fs::write(worktree.join("kept"), "uncommitted work\n")?;

    // A preview authorizes nothing, and a plain resume never delivers it.
    let first = fixture.preview(&FIX)?;
    fixture.ok(&["resume", &fixture.goal, "t1"])?;
    fixture.wait("the plain resume to run", |_| {
        fixture.lines("child-runs", "t1") == 2
    })?;
    fixture.wait_failed("t1", 1)?;
    assert!(!text(&fixture.home.join("plan-t1")).contains("Approved correction"));
    assert!(!worktree.join("fixed").exists());

    // The worktree is part of what was reviewed: changing it voids the preview.
    fs::write(worktree.join("kept"), "changed after review\n")?;
    fixture.refused(
        &fixture.approval(&first),
        "changed since the correction was reviewed",
    )?;
    assert_eq!(fixture.lines("child-runs", "t1"), 2);

    // Two racing approvals of the reviewed preview resume the same attempt once.
    let second = fixture.preview(&FIX)?;
    let reviewed = fixture.goal()?["execution"]["corrective_preview"].clone();
    assert_eq!(reviewed["resume"]["step"], "implement", "{reviewed}");
    assert_eq!(
        reviewed["resume"]["session_id"],
        attempt(&failed, "t1")["session_id"]
    );
    let approve = json!({"type":"approve_corrective", "goal_id":fixture.goal, "preview_id":reviewed["id"], "revision":reviewed["revision"]});
    let (one, two) = tokio::join!(fixture.action(&approve), fixture.action(&approve));
    assert!(one?.status().is_success() && two?.status().is_success());
    let done = fixture.wait("t1 to integrate", integrated("t1"))?;
    let resumed = attempt(&done, "t1");
    assert_eq!(resumed["attempt"], 1, "{done}");
    assert_eq!(resumed["session_id"], attempt(&failed, "t1")["session_id"]);
    assert_eq!(resumed["corrections"], json!([second.parse::<u64>()?]));
    let plan = text(&fixture.home.join("plan-t1"));
    assert!(
        plan.contains(&format!("# Approved correction {second}\n\nWRITE: fixed\n")),
        "{plan}"
    );
    let tip = done["execution"]["integrated_tip"].as_str().ok_or("tip")?;
    assert_eq!(
        git(&fixture.repo, &["show", &format!("{tip}:kept")])?,
        "changed after review"
    );
    assert_eq!(
        git(&fixture.repo, &["show", &format!("{tip}:fixed")])?,
        "t1"
    );

    // The approval is spent: nothing is left to approve and nothing ran twice.
    fixture.refused(&fixture.approval(&second), "no corrective preview")?;
    fixture.wait("t2 to integrate", integrated("t2"))?;
    assert_eq!(fixture.lines("child-runs", "t1"), 3);
    assert_eq!(
        fixture.goal()?["execution"]["corrective_approvals"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    Ok(())
}

#[test]
fn correction_reviewed_at_another_tip_or_for_another_attempt_is_refused() -> TestResult {
    let fixture = Fixture::new(&json!([failing("t1"), writing("tx", &[])]), &["tx"])?;
    fixture.wait_failed("t1", 1)?;

    let before_tip_moved = fixture.preview(&FIX)?;
    fixture.release("hold-tx")?;
    fixture.wait("tx to integrate", integrated("tx"))?;
    fixture.refused(
        &fixture.approval(&before_tip_moved),
        "changed since the correction was reviewed",
    )?;

    let for_first_attempt = fixture.preview(&FIX)?;
    fixture.ok(&["retry", &fixture.goal, "t1"])?;
    fixture.wait_failed("t1", 2)?;
    fixture.refused(
        &fixture.approval(&for_first_attempt),
        "changed since the correction was reviewed",
    )?;

    assert_eq!(fixture.lines("child-runs", "t1"), 2);
    assert!(!text(&fixture.home.join("plan-t1")).contains("Approved correction"));
    let goal = fixture.goal()?;
    assert_eq!(goal["execution"]["corrective_approvals"], json!([]));
    assert_eq!(attempt(&goal, "t1")["corrections"], Value::Null);
    Ok(())
}

#[test]
fn armed_correction_survives_a_restart_but_not_a_changed_worktree() -> TestResult {
    let fixture = Fixture::new(&json!([failing("t1"), failing("t9")]), &[])?;
    fixture.wait_failed("t1", 1)?;
    let failed = fixture.wait_failed("t9", 1)?;

    // The daemon stops after the approval is saved and before the resume is dispatched.
    let armed = fixture.preview(&FIX)?;
    fixture.stop()?;
    fixture.edit_record(|state| approve_in_record(state, "t1", false))?;
    fixture.start()?;
    let done = fixture.wait("t1 to integrate", integrated("t1"))?;
    assert_eq!(
        attempt(&done, "t1")["corrections"],
        json!([armed.parse::<u64>()?])
    );
    assert_eq!(fixture.lines("child-runs", "t1"), 2);

    // Same boundary, but the reviewed worktree changed while the daemon was down.
    let other = ["t9", "--resume", "--instructions", "WRITE: other"];
    let stale = fixture.preview(&other)?;
    fixture.stop()?;
    fixture.edit_record(|state| approve_in_record(state, "t9", false))?;
    fs::write(fixture.worktree(&failed, "t9").join("late"), "unreviewed\n")?;
    fixture.start()?;
    let refused = fixture.wait("the armed correction to be refused", |goal| {
        attempt(goal, "t9")["resume_pending"] == false
            && goal["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("approved correction was not started"))
    })?;
    assert_eq!(
        attempt(&refused, "t9")["correction_pending"].to_string(),
        stale
    );
    assert_eq!(attempt(&refused, "t9")["corrections"], Value::Null);
    assert_eq!(fixture.lines("child-runs", "t9"), 1);
    fixture.refused(
        &["resume", &fixture.goal, "t9"],
        "an approved correction was not started",
    )?;

    // A fresh review of the changed worktree is what starts it.
    let fresh = fixture.preview(&other)?;
    fixture.ok(&fixture.approval(&fresh))?;
    let done = fixture.wait("t9 to integrate", integrated("t9"))?;
    assert_eq!(
        attempt(&done, "t9")["corrections"],
        json!([fresh.parse::<u64>()?])
    );
    assert_eq!(attempt(&done, "t9")["correction_pending"], Value::Null);
    assert_eq!(fixture.lines("child-runs", "t9"), 2);
    Ok(())
}

#[test]
fn correction_that_started_before_a_crash_is_never_replayed() -> TestResult {
    let fixture = Fixture::new(&json!([failing("t1")]), &[])?;
    fixture.wait_failed("t1", 1)?;
    fixture.preview(&FIX)?;
    fixture.stop()?;
    fixture.edit_record(|state| approve_in_record(state, "t1", true))?;
    fixture.start()?;
    let blocked = fixture.wait("the interrupted launch to block", |goal| {
        goal["status"] == "blocked"
    })?;
    assert!(
        blocked["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("interrupted resume dispatch")),
        "{blocked}"
    );
    assert_eq!(fixture.lines("child-runs", "t1"), 1);
    assert!(!fixture.worktree(&blocked, "t1").join("fixed").exists());
    Ok(())
}

/// Requests that would weaken or misapply the contract never become a preview.
fn refuse_unsafe_amendments(fixture: &Fixture) -> TestResult {
    let edit = |target: Value, extra: Value| {
        let mut edit = json!({"replace":"first requirement", "with":"first narrowed requirement"});
        for source in [target, extra] {
            for (key, value) in source.as_object().into_iter().flatten() {
                edit[key] = value.clone();
            }
        }
        json!([edit])
    };
    for (name, edits, reason) in [
        (
            "wrong-digest.json",
            edit(
                json!({"criterion":"c1"}),
                json!({"old_sha256":sha256("other text")}),
            ),
            "does not match the reviewed text",
        ),
        (
            "unknown.json",
            edit(json!({"criterion":"c9"}), json!({})),
            "unknown contract criterion",
        ),
        ("check.json", edit(json!({"check":"k1"}), json!({})), ""),
        (
            "absent.json",
            json!([{"criterion":"c2", "replace":"first requirement", "with":"x"}]),
            "does not match the reviewed text",
        ),
    ] {
        let file = fixture.amendment(name, &edits)?;
        fixture.refused(&["corrective", &fixture.goal, "--amend", &file], reason)?;
    }
    assert_eq!(
        fixture.goal()?["execution"]["corrective_preview"],
        Value::Null
    );
    Ok(())
}

#[test]
fn amendment_is_kept_apart_and_reaches_later_children_verification_and_the_pr() -> TestResult {
    let tasks = json!([failing("t1"), {"id":"t2", "instructions":"Do the work of t2", "dependencies":["t1"]}]);
    let fixture = Fixture::new(&tasks, &[])?;
    let before = fixture.wait_failed("t1", 1)?;
    let session = attempt(&before, "t1")["session_id"]
        .as_str()
        .ok_or("session")?
        .to_owned();
    let plan_file = fixture
        .home
        .join("data/cruise/sessions")
        .join(session)
        .join("plan.md");
    let plan_before = text(&plan_file);
    refuse_unsafe_amendments(&fixture)?;

    let edits = json!([
        {"criterion":"c1", "replace":"first requirement", "with":"first narrowed requirement"},
        {"task":"t2", "replace":"Do the work of t2", "with":"Do the work of t2\nWRITE: second"},
    ]);
    let file = fixture.amendment("amendment.json", &edits)?;
    let amended = fixture.preview(&["--amend", &file])?;
    let reviewed = fixture.goal()?["execution"]["corrective_preview"].clone();
    assert_eq!(
        reviewed["amendment"][0]["old_sha256"],
        sha256("The first requirement holds"),
        "{reviewed}"
    );
    fixture.ok(&fixture.approval(&amended))?;

    // The original proposal and the existing attempt are exactly what they were.
    let after = fixture.goal()?;
    assert_eq!(after["proposal"], before["proposal"]);
    assert_eq!(text(&plan_file), plan_before);
    assert_eq!(attempt(&after, "t1")["contract_revision"], Value::Null);
    // An amendment reviewed against the text it has just replaced is stale.
    let stale = fixture.amendment(
        "stale.json",
        &json!([{"criterion":"c1", "old_sha256":sha256("The first requirement holds"), "replace":"first", "with":"1st"}]),
    )?;
    fixture.refused(
        &["corrective", &fixture.goal, "--amend", &stale],
        "does not match the reviewed text",
    )?;

    // A corrected resume of the older attempt is told the texts amended since.
    let fix = fixture.preview(&FIX)?;
    fixture.ok(&fixture.approval(&fix))?;
    fixture.wait("t1 to integrate", integrated("t1"))?;
    let plan = text(&fixture.home.join("plan-t1"));
    assert!(plan.starts_with(&plan_before), "{plan}");
    assert!(
        plan.contains("- criterion c1: The first narrowed requirement holds\n- task t2: Do the work of t2\nWRITE: second\n"),
        "{plan}"
    );

    // A child reserved after the amendment, the final verification and the PR use it.
    let done = fixture.wait("the PR", |goal| goal["status"] == "awaiting_merge")?;
    assert_eq!(attempt(&done, "t2")["contract_revision"], 1, "{done}");
    let plan = text(&fixture.home.join("plan-t2"));
    assert!(
        plan.contains("- c1: The first narrowed requirement holds\n"),
        "{plan}"
    );
    let publication = &done["execution"]["publication"];
    assert_eq!(publication["contract_revision"], 1, "{publication}");
    let checked: Vec<_> = publication["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|check| (&check["check_id"], &check["verified_tip"]))
        .collect();
    let tip = &done["execution"]["integrated_tip"];
    assert_eq!(
        checked,
        [
            (&json!("k1"), tip),
            (&json!("k2"), tip),
            (&json!("k3"), tip)
        ]
    );
    let pr = text(&fixture.home.join("gh-calls"));
    for evidence in [
        "## Approved contract amendments",
        &format!(
            "criterion `c1`, previous text sha256 `{}`",
            sha256("The first requirement holds")
        ),
        "> first narrowed requirement",
        "task `t2`",
    ] {
        assert!(pr.contains(evidence), "{evidence}: {pr}");
    }
    Ok(())
}

#[tokio::test]
async fn evidence_resolves_a_satisfied_task_without_a_commit_and_the_final_tip_is_verified()
-> TestResult {
    let fixture = Fixture::new(&json!([failing("t1"), writing("t2", &["t1"])]), &[])?;
    let failed = fixture.wait_failed("t1", 1)?;
    let base = failed["execution"]["integrated_tip"].clone();
    for (criterion, path, reason) in [
        ("c9", "file", "distinct approved criteria"),
        ("c1", "missing", "not a file at the Goal tip"),
        ("c1", "../file", "not a file at the Goal tip"),
    ] {
        let request = [
            "corrective",
            &fixture.goal,
            "t1",
            "--evidence",
            "--criterion",
            criterion,
            "--path",
            path,
        ];
        fixture.refused(&request, reason)?;
    }

    // The requester names requirements; the daemon derives every check of them.
    let narrowed = json!({"type":"preview_corrective", "goal_id":fixture.goal, "expected_tip":base,
        "evidence":{"task_id":"t1", "criteria":["c1"], "paths":["file"], "checks":["k1"]}});
    let preview: Value = fixture.action(&narrowed).await?.json().await?;
    assert_eq!(
        preview["evidence"]["checks"],
        json!(["k1", "k2"]),
        "{preview}"
    );
    assert_eq!(
        preview["evidence"]["blobs"],
        json!([{"path":"file", "blob":git(&fixture.repo, &["rev-parse", "HEAD:file"])?}])
    );
    assert_eq!(fixture.lines("check-runs", "k1"), 0);
    fixture.ok(&fixture.approval(&preview["revision"].to_string()))?;

    let resolved = fixture.wait("the evidence run", |goal| {
        !goal["execution"]["evidence_runs"][0]["outcome"].is_null()
    })?;
    let run = &resolved["execution"]["evidence_runs"][0];
    assert_eq!(run["outcome"], "resolved", "{run}");
    assert_eq!(resolved["execution"]["integrated_tip"], base);
    let unchanged = attempt(&resolved, "t1");
    assert_eq!(unchanged["integrated"], false, "{unchanged}");
    assert_eq!(unchanged["result_sha"], Value::Null);
    let tip = base.as_str().ok_or("tip")?;
    assert_eq!(
        text(&fixture.home.join("check-runs")),
        format!("k1 {tip}\nk2 {tip}\n")
    );
    for command in ["resume", "retry"] {
        assert!(
            !fixture
                .manager(&[command, &fixture.goal, "t1"])?
                .status
                .success()
        );
    }

    // The dependent starts from the unchanged tip, and every check runs at the final one.
    let done = fixture.wait("the PR", |goal| goal["status"] == "awaiting_merge")?;
    assert_eq!(attempt(&done, "t2")["base_sha"], base, "{done}");
    let last = done["execution"]["integrated_tip"].as_str().ok_or("tip")?;
    assert_ne!(last, tip);
    assert_eq!(
        text(&fixture.home.join("check-runs")),
        format!("k1 {tip}\nk2 {tip}\nk1 {last}\nk2 {last}\nk3 {last}\n")
    );
    let pr = text(&fixture.home.join("gh-calls"));
    assert!(pr.contains("## Tasks resolved by evidence"), "{pr}");
    assert!(
        pr.contains(&format!("- `t1` at `{tip}` (corrective revision {}): criteria c1; checks k1, k2; reviewed `file` blob", preview["revision"])),
        "{pr}"
    );
    Ok(())
}

#[test]
fn evidence_reviewed_at_another_tip_or_for_another_attempt_is_refused() -> TestResult {
    let tasks = json!([failing("t1"), writing("t2", &["t1"]), writing("tx", &[])]);
    let fixture = Fixture::new(&tasks, &["tx"])?;
    fixture.wait_failed("t1", 1)?;

    let before_tip_moved = fixture.preview(&EVIDENCE)?;
    fixture.release("hold-tx")?;
    fixture.wait("tx to integrate", integrated("tx"))?;
    fixture.refused(
        &fixture.approval(&before_tip_moved),
        "Goal tip or contract changed",
    )?;

    let for_first_attempt = fixture.preview(&EVIDENCE)?;
    fixture.ok(&["retry", &fixture.goal, "t1"])?;
    fixture.wait_failed("t1", 2)?;
    fixture.refused(
        &fixture.approval(&for_first_attempt),
        "changed since the evidence preview",
    )?;

    let goal = fixture.goal()?;
    assert_eq!(goal["execution"]["evidence_runs"], Value::Null, "{goal}");
    assert_eq!(attempt(&goal, "t2"), Value::Null);
    assert_eq!(fixture.lines("check-runs", "k1"), 0);
    Ok(())
}

#[test]
fn red_interrupted_or_overtaken_evidence_leaves_the_task_failed() -> TestResult {
    let tasks = json!([failing("t1"), writing("t2", &["t1"]), writing("tx", &[])]);
    let fixture = Fixture::new(&tasks, &["tx"])?;
    fixture.wait_failed("t1", 1)?;
    let outcome = |index: usize| {
        fixture.wait("the evidence run", |goal| {
            !goal["execution"]["evidence_runs"][index]["outcome"].is_null()
        })
    };
    let started = |runs: usize| {
        fixture.wait("the evidence check to start", |_| {
            fixture.lines("check-runs", "k1") == runs
        })
    };

    // A red check: the run and its result stay as evidence, the task stays failed.
    fs::write(fixture.home.join("fail-check-k2"), "")?;
    fixture.ok(&fixture.approval(&fixture.preview(&EVIDENCE)?))?;
    let red = outcome(0)?;
    let run = &red["execution"]["evidence_runs"][0];
    assert_eq!(
        run["outcome"],
        json!({"unresolved":"check k2 failed or was interrupted"})
    );
    assert_eq!(run["checks"][0]["exit_code"], 0, "{run}");
    assert_eq!(run["checks"][1]["exit_code"], 1, "{run}");
    fixture.release("fail-check-k2")?;

    // Another task integrates during a check: its green result is for an older tip.
    fs::write(fixture.home.join("hold-check-k1"), "")?;
    fixture.ok(&fixture.approval(&fixture.preview(&EVIDENCE)?))?;
    started(2)?;
    fixture.release("hold-tx")?;
    fixture.wait("tx to integrate", integrated("tx"))?;
    fixture.release("hold-check-k1")?;
    let overtaken = outcome(1)?;
    assert_eq!(
        overtaken["execution"]["evidence_runs"][1]["outcome"],
        json!({"unresolved":"check k1 failed or was interrupted"})
    );

    // The daemon dies during a check: the outcome is unknown and is never replayed.
    fs::write(fixture.home.join("hold-check-k1"), "")?;
    fixture.ok(&fixture.approval(&fixture.preview(&EVIDENCE)?))?;
    started(3)?;
    fixture.kill()?;
    fixture.release("hold-check-k1")?;
    fixture.start()?;
    let interrupted = outcome(2)?;
    assert_eq!(
        interrupted["execution"]["evidence_runs"][2]["outcome"],
        json!({"unresolved":"evidence run was interrupted before its outcome was saved"})
    );
    assert_eq!(fixture.lines("check-runs", "k1"), 3);

    let goal = fixture.wait_failed("t1", 1)?;
    assert_eq!(attempt(&goal, "t1")["integrated"], false);
    assert_eq!(attempt(&goal, "t2"), Value::Null, "{goal}");

    // None of that prevents a complete green run at the current tip from resolving it.
    fixture.ok(&fixture.approval(&fixture.preview(&EVIDENCE)?))?;
    assert_eq!(
        outcome(3)?["execution"]["evidence_runs"][3]["outcome"],
        "resolved"
    );
    fixture.wait("t2 to integrate", integrated("t2"))?;
    Ok(())
}
