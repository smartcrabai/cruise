//! Runtime contracts intentionally bypass command-backend preflight, as the
//! command stubs exercise the real prompt, timeout, and workspace guard paths.
use super::*;
use crate::flags::{FlagValues, NO_FLAGS};
use crate::instruction_inbox::{self, AcceptRequest, Origin, RunInbox};
use crate::option_handler::NoOpOptionHandler;
use crate::session::{SessionManager, SessionPhase, SessionState};
use crate::session_config::SessionConfigRef;
use crate::test_support::lock_process;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::TempDir;

const ADVICE: &str = "advisor/advice.md";
const DIFF: &str = "advisor/advice.md.diff";
const ROUTED_STEPS: &str = r"
  advise:
    prompt: review
    advisor: true
    output_file: advisor/advice.md
    if:
      fail: fallback
  apply:
    command: 'true'
  fallback:
    command: 'true'
";

struct Fixture {
    root: TempDir,
    repo: PathBuf,
    artifacts: PathBuf,
}

impl Fixture {
    fn new(git: bool) -> Result<Self> {
        let root = TempDir::new()?;
        let repo = root.path().join("repo");
        let artifacts = root.path().join("artifacts");
        fs::create_dir(&repo)?;
        fs::create_dir(&artifacts)?;
        if git {
            crate::test_support::init_git_repo(&repo);
        }
        crate::artifacts::write(&artifacts, ADVICE, "STALE ADVICE")?;
        Ok(Self {
            root,
            repo,
            artifacts,
        })
    }

    fn assert_no_advice(&self) {
        assert!(
            !self.artifacts.join(ADVICE).exists(),
            "stale advice survived"
        );
    }

    async fn run(
        &self,
        backend: &str,
        steps: &str,
        flags: &FlagValues,
        skipped_steps: &[String],
        instructions: Option<&RunInbox>,
    ) -> (Result<ExecutionResult>, Vec<String>) {
        let backend =
            serde_json::to_string(&["sh", "-c", backend]).unwrap_or_else(|error| panic!("{error}"));
        let yaml = format!(
            "command: {backend}\npermission: full\nflags:\n  advisor:\n    description: Enable advisor\nsteps:\n{steps}"
        );
        let config = WorkflowConfig::from_yaml(&yaml).unwrap_or_else(|error| panic!("{error}"));
        let compiled = crate::workflow::compile(config).unwrap_or_else(|error| panic!("{error}"));
        let mut dag = crate::dag::build_dag(&compiled, 3).unwrap_or_else(|error| panic!("{error}"));
        let start = dag.start.clone();
        let mut vars = VariableStore::new(String::new());
        vars.set_artifacts_root(self.artifacts.clone());
        let mut tracker = FileTracker::with_root(self.repo.clone());
        let started = Mutex::new(Vec::new());
        let ctx = ExecutionContext {
            instructions,
            usage_session_id: None,
            compiled: &compiled,
            max_retries: 3,
            rate_limit_retries: 0,
            on_step_start: &|step| {
                started
                    .lock()
                    .unwrap_or_else(|error| panic!("{error}"))
                    .push(step.to_owned());
                Ok(())
            },
            cancel_token: None,
            option_handler: &NoOpOptionHandler,
            ask_handler: None,
            config_reloader: None,
            working_dir: Some(&self.repo),
            skipped_steps,
            flags,
            on_step_log: None,
        };
        let result =
            execute_steps_with_dag(&ctx, &mut vars, &mut tracker, &mut dag, &start, &|_, _| {
                Ok(())
            })
            .await;
        (
            result,
            started
                .into_inner()
                .unwrap_or_else(|error| panic!("{error}")),
        )
    }
}

#[tokio::test]
async fn advisor_receives_host_diff_and_replaces_stale_advice() -> Result<()> {
    let _lock = lock_process();
    let f = Fixture::new(true)?;
    fs::write(f.repo.join(".git/info/exclude"), "ignored.txt\n")?;
    fs::write(f.repo.join("README.md"), "tracked edit\n")?;
    let stage = std::process::Command::new("git")
        .args(["add", "README.md"])
        .current_dir(&f.repo)
        .output()?;
    assert!(stage.status.success(), "fixture stage: {stage:?}");
    fs::write(f.repo.join("untracked.txt"), "untracked edit\n")?;
    fs::write(f.repo.join("-"), "literal dash file content\n")?;
    fs::write(f.repo.join("ignored.txt"), "ignored edit\n")?;
    fs::write(f.repo.join(".worktreeinclude"), "local-only.txt\n")?;
    fs::write(f.repo.join("local-only.txt"), "excluded worktree content\n")?;
    let steps = ROUTED_STEPS.replace("prompt: review", "prompt: '{file:advisor/advice.md.diff}'");
    let (result, started) = f.run("cat", &steps, &NO_FLAGS, &[], None).await;
    assert_eq!(result?.failed, 0);
    assert_eq!(started, ["advise", "apply", "fallback"]);

    let diff = crate::artifacts::read(&f.artifacts, DIFF)?;
    let (stat, patch) = diff
        .split_once("diff --git")
        .unwrap_or_else(|| panic!("missing patch: {diff}"));
    assert!(stat.contains("README.md") && stat.contains('|'), "{stat}");
    assert!(stat.lines().any(|line| line == " - | untracked"), "{stat}");
    assert!(patch.contains("+tracked edit"), "{patch}");
    assert!(patch.contains("+untracked edit"), "{patch}");
    assert!(patch.lines().any(|line| line == "+literal dash file content"), "{patch}");
    assert!(!diff.contains("ignored.txt"), "{diff}");
    assert!(!diff.contains("excluded worktree content"), "{diff}");
    let advice = crate::artifacts::read(&f.artifacts, ADVICE)?;
    assert!(
        advice.contains(diff.trim()),
        "backend did not receive the diff artifact"
    );
    assert!(!advice.contains("STALE ADVICE"));
    Ok(())
}

#[tokio::test]
async fn advisor_invalidates_before_resolving_step_environment() -> Result<()> {
    let _lock = lock_process();
    let f = Fixture::new(true)?;
    let dispatched = f.root.path().join("dispatched");
    let backend = format!("touch '{}'", dispatched.display());
    let steps = ROUTED_STEPS.replace("    if:", "    env:\n      CHECK: '{undefined_advisor_env}'\n    if:");
    let (result, _) = f.run(&backend, &steps, &NO_FLAGS, &[], None).await;
    assert!(matches!(result, Err(CruiseError::UndefinedVariable(_))), "{result:?}");
    assert!(!dispatched.exists());
    f.assert_no_advice();
    Ok(())
}

#[tokio::test]
async fn advisor_publishes_only_after_visit_acceptance() -> Result<()> {
    let _lock = lock_process();
    for (condition, routed, error) in [
        ("    if:\n      no-file-changes: failed\n", false, "step_no_file_changes"),
        ("    quality_gates:\n      - command: 'exit 9'\n        stop_exit_codes: [9]\n", false, "quality_gate_stopped"),
        ("    quality_gates:\n      - command: 'exit 9'\n    if:\n      fail: fallback\n", true, ""),
    ] {
        let f = Fixture::new(true)?;
        let steps = format!("  advise:\n    prompt: review\n    advisor: true\n    output_file: {ADVICE}\n{condition}  fallback:\n    prompt: '{{file:{ADVICE}}}'\n");
        // Check absence on every dispatch, including retries, and emit a marker that advisors ignore.
        let backend = format!("test ! -e '{}' || exit 99; echo 'NO_CHANGES_INTENTIONAL: read-only'", f.artifacts.join(ADVICE).display());
        let (result, started) = f.run(&backend, &steps, &NO_FLAGS, &[], None).await;
        let Err(result) = result else { panic!("advisor rejection unexpectedly succeeded"); };
        if routed {
            assert!(result.to_string().contains("failed to read artifact"), "{result}");
            assert_eq!(started, ["advise", "advise", "advise", "advise", "fallback"]);
        } else {
            assert_eq!(result.telemetry_type(), error, "{result}");
            assert_eq!(started, ["advise"]);
        }
        f.assert_no_advice();
    }
    Ok(())
}

#[tokio::test]
async fn accepted_advisor_preserves_prev_and_publishes_before_successor() -> Result<()> {
    let _lock = lock_process();
    let f = Fixture::new(true)?;
    let steps = r"
  ordinary:
    prompt: ordinary-before
  advise:
    prompt: review-only
    advisor: true
    output_file: advisor/advice.md
  apply:
    prompt: 'previous={prev.output} advice={file:advisor/advice.md}'
    output_file: consumed
";
    let backend = r#"p=$(cat); case "$p" in *ordinary-before*) echo prior;; *review-only*) echo fresh;; *) printf '%s' "$p";; esac"#;
    let (result, started) = f.run(backend, steps, &NO_FLAGS, &[], None).await;
    assert_eq!(result?.failed, 0);
    assert_eq!(started, ["ordinary", "advise", "apply"]);
    let consumed = crate::artifacts::read(&f.artifacts, "consumed")?;
    assert!(consumed.contains("previous=prior") && consumed.contains("advice=fresh"), "{consumed}");
    Ok(())
}

#[tokio::test]
async fn advisor_diff_includes_the_201st_eligible_untracked_file() -> Result<()> {
    let _lock = lock_process();
    let f = Fixture::new(true)?;
    for index in 0..201 {
        fs::write(f.repo.join(format!("advisor-untracked-{index:03}.txt")), "tiny\n")?;
    }
    let (result, _) = f.run("echo fresh", ROUTED_STEPS, &NO_FLAGS, &[], None).await;
    assert_eq!(result?.failed, 0);
    let diff = crate::artifacts::read(&f.artifacts, DIFF)?;
    let (stat, patch) = diff.split_once("diff --git").unwrap_or_else(|| panic!("no patch"));
    assert!(stat.contains("advisor-untracked-200.txt"));
    assert!(patch.contains("+++ b/advisor-untracked-200.txt"));
    assert!(!diff.contains("[Diff truncated]"));
    Ok(())
}

#[tokio::test]
async fn advisor_truncates_oversized_diff_at_line_boundary() -> Result<()> {
    const CAP: usize = 256 * 1024;
    let _lock = lock_process();
    let f = Fixture::new(true)?;
    let line = "x".repeat(1000);
    fs::write(f.repo.join("README.md"), format!("{line}\n").repeat(350))?;
    let (result, _) = f
        .run(
            "cat >/dev/null; echo fresh",
            ROUTED_STEPS,
            &NO_FLAGS,
            &[],
            None,
        )
        .await;
    assert_eq!(result?.failed, 0);
    let diff = crate::artifacts::read(&f.artifacts, DIFF)?;
    let diff = diff
        .strip_suffix('\n')
        .unwrap_or_else(|| panic!("diff must end in a newline"));
    let (patch, notice) = diff
        .rsplit_once('\n')
        .unwrap_or_else(|| panic!("missing notice"));
    assert!(notice.to_ascii_lowercase().contains("truncat"), "{notice}");
    assert!(patch.len() < CAP);
    assert!(patch.len() >= CAP - line.len() - 2);
    assert_eq!(
        patch.trim_end().lines().last(),
        Some(format!("+{line}").as_str())
    );
    Ok(())
}

#[tokio::test]
async fn advisor_without_git_routes_before_starting_backend_and_removes_stale_advice() -> Result<()>
{
    let _lock = lock_process();
    let f = Fixture::new(false)?;
    let (result, started) = f
        .run(
            "cat >/dev/null; touch backend-started",
            ROUTED_STEPS,
            &NO_FLAGS,
            &[],
            None,
        )
        .await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(started, ["advise", "fallback"]);
    assert!(!f.repo.join("backend-started").exists());
    f.assert_no_advice();
    Ok(())
}

#[tokio::test]
async fn advisor_diff_access_error_routes_before_artifact_or_dispatch() -> Result<()> {
    let _lock = lock_process();
    let f = Fixture::new(true)?;
    let vanished = f.repo.join("vanishing.txt");
    fs::write(&vanished, "untracked before enumeration\n")?;
    let bin = f.root.path().join("bin");
    fs::create_dir(&bin)?;
    crate::test_support::write_executable_script(
        &bin.join("git"),
        r#"#!/bin/sh
PATH=${PATH#*:}
export PATH
previous=
for arg do
    if [ "$previous" = /dev/null ] && [ "$arg" = vanishing.txt ]; then
        rm -- "$2/vanishing.txt" || exit 99
        break
    fi
    previous=$arg
done
exec git "$@"
"#,
    );
    let _path = crate::test_support::prepend_to_path(&bin);
    let (result, started) = f
        .run("touch backend-started", ROUTED_STEPS, &NO_FLAGS, &[], None)
        .await;
    assert!(!vanished.exists(), "no-index fixture did not run");
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(started, ["advise", "fallback"]);
    assert!(!f.repo.join("backend-started").exists());
    assert!(!f.artifacts.join(DIFF).exists());
    f.assert_no_advice();
    Ok(())
}

#[tokio::test]
async fn advisor_backend_failure_routes_without_leaving_stale_advice() -> Result<()> {
    let _lock = lock_process();
    let f = Fixture::new(true)?;
    let (result, started) = f
        .run(
            "cat >/dev/null; exit 17",
            ROUTED_STEPS,
            &NO_FLAGS,
            &[],
            None,
        )
        .await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(started, ["advise", "fallback"]);
    assert!(
        f.artifacts.join(DIFF).is_file(),
        "host preparation did not run"
    );
    f.assert_no_advice();
    Ok(())
}

#[tokio::test]
async fn advisor_revisit_regenerates_diff_and_invalidates_previous_success() -> Result<()> {
    let _lock = lock_process();
    let f = Fixture::new(true)?;
    fs::write(f.repo.join("README.md"), "first visit\n")?;
    let steps = r"
  advise:
    advisor: true
    prompt: '{file:advisor/advice.md.diff}'
    output_file: advisor/advice.md
    if:
      fail: fallback
  apply:
    command: 'echo second-visit > README.md'
    next: advise
  fallback:
    command: 'true'
";
    let (result, started) = f
        .run(
            "if grep -q '^+second-visit$'; then exit 17; fi; echo 'first advice'",
            steps,
            &NO_FLAGS,
            &[],
            None,
        )
        .await;
    assert_eq!(result?.failed, 0);
    assert_eq!(started, ["advise", "apply", "advise", "fallback"]);
    assert!(crate::artifacts::read(&f.artifacts, DIFF)?.contains("+second-visit"));
    f.assert_no_advice();
    Ok(())
}

#[tokio::test]
async fn advisor_flag_and_user_skips_both_remove_stale_advice() -> Result<()> {
    let _lock = lock_process();
    let enabled = FlagValues::from([("advisor".to_owned(), true)]);
    let user_skip = ["advise".to_owned()];
    let steps = ROUTED_STEPS.replace("advisor: true", "advisor: true\n    flag: advisor");
    for (flags, skipped_steps) in [(&NO_FLAGS, &[][..]), (&enabled, &user_skip[..])] {
        let f = Fixture::new(false)?;
        let (result, started) = f.run("exit 99", &steps, flags, skipped_steps, None).await;
        assert_eq!(result?.skipped, 1);
        assert_eq!(started, ["apply", "fallback"]);
        f.assert_no_advice();
    }
    Ok(())
}

#[tokio::test]
async fn advice_consumer_fails_on_missing_artifact_after_skip() -> Result<()> {
    let _lock = lock_process();
    let f = Fixture::new(false)?;
    let steps = r"
  advise:
    advisor: true
    flag: advisor
    prompt: review
    output_file: advisor/advice.md
  consume:
    prompt: '{file:advisor/advice.md}'
";
    let (result, started) = f
        .run(
            "cat >/dev/null; touch backend-started",
            steps,
            &NO_FLAGS,
            &[],
            None,
        )
        .await;
    let error = match result {
        Err(error) => error.to_string(),
        Ok(_) => panic!("consumer unexpectedly used stale advice"),
    };
    assert!(
        error.contains("failed to read artifact") && error.contains(ADVICE),
        "{error}"
    );
    assert_eq!(started, ["consume"]);
    assert!(!f.repo.join("backend-started").exists());
    f.assert_no_advice();
    Ok(())
}

#[tokio::test]
async fn advisor_forces_read_only_and_does_not_route_a_workspace_change() -> Result<()> {
    let _lock = lock_process();
    let f = Fixture::new(true)?;
    let (result, started) = f
        .run(
            "cat >/dev/null; touch workspace-write",
            ROUTED_STEPS,
            &NO_FLAGS,
            &[],
            None,
        )
        .await;
    assert!(
        matches!(result, Err(CruiseError::ReadOnlyWorkspaceChanged)),
        "{result:?}"
    );
    assert_eq!(started, ["advise"]);
    assert!(f.repo.join("workspace-write").is_file());
    f.assert_no_advice();
    Ok(())
}

#[tokio::test]
async fn non_advisor_read_only_workspace_change_still_routes() -> Result<()> {
    let _lock = lock_process();
    let f = Fixture::new(true)?;
    let steps = ROUTED_STEPS.replace("advisor: true", "permission: read-only");
    let (result, started) = f
        .run(
            "cat >/dev/null; touch workspace-write",
            &steps,
            &NO_FLAGS,
            &[],
            None,
        )
        .await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(started, ["advise", "fallback"]);
    assert!(f.repo.join("workspace-write").is_file());
    Ok(())
}

#[tokio::test]
async fn advisor_timeout_preserves_guard_error_but_routes_a_clean_timeout() -> Result<()> {
    let _lock = lock_process();
    let steps = ROUTED_STEPS.replace("advisor: true", "advisor: true\n    timeout: '1'");
    for (backend, mutated) in [
        ("cat >/dev/null; touch workspace-write; exec sleep 30", true),
        ("cat >/dev/null; exec sleep 30", false),
    ] {
        let f = Fixture::new(true)?;
        let (result, started) = tokio::time::timeout(
            Duration::from_secs(10),
            f.run(backend, &steps, &NO_FLAGS, &[], None),
        )
        .await
        .unwrap_or_else(|error| panic!("prompt timeout did not cancel backend: {error}"));
        if mutated {
            assert!(
                matches!(result, Err(CruiseError::ReadOnlyWorkspaceChanged)),
                "{result:?}"
            );
            assert_eq!(started, ["advise"]);
        } else {
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(started, ["advise", "fallback"]);
        }
        assert_eq!(f.repo.join("workspace-write").is_file(), mutated);
        f.assert_no_advice();
    }
    Ok(())
}

#[tokio::test]
async fn advisor_snapshot_failure_is_fatal_even_with_failure_route() -> Result<()> {
    let _lock = lock_process();
    let f = Fixture::new(true)?;
    let (result, started) = f
        .run(
            "cat >/dev/null; mv ../repo ../moved-repo; exit 1",
            ROUTED_STEPS,
            &NO_FLAGS,
            &[],
            None,
        )
        .await;
    assert!(!f.repo.exists());
    assert!(f.root.path().join("moved-repo/README.md").is_file());
    let Err(error) = result else {
        panic!("an unverifiable advisor step must stop even with if.fail: {result:?}");
    };
    assert_eq!(
        error.telemetry_type(),
        "read_only_verification_failed",
        "{error:?}"
    );
    assert!(!crate::auto_requeue::is_eligible_error(&error));
    assert_eq!(started, ["advise"]);
    f.assert_no_advice();
    Ok(())
}

#[tokio::test]
async fn advisor_leaves_pending_instruction_for_the_next_prompt() -> Result<()> {
    const ID: &str = "20261124000000";
    const INSTRUCTION: &str = "instruction sentinel for the implementing agent";
    let _lock = lock_process();
    let f = Fixture::new(true)?;
    let manager = SessionManager::new(f.root.path().join("cruise"));
    let mut state = SessionState::new(
        ID.into(),
        f.repo.clone(),
        SessionConfigRef::BuiltinSnapshot { name: None },
        "advisor inbox fixture".into(),
    );
    state.phase = SessionPhase::Running;
    state.set_runner_to_current_process();
    manager.create(&state)?;
    let sessions = manager.sessions_dir();
    let inbox = RunInbox::open(&sessions, ID)?;
    instruction_inbox::accept(
        &sessions,
        ID,
        AcceptRequest {
            content: INSTRUCTION.to_owned(),
            origin: Origin::Cli,
            dedupe_key: None,
        },
    )?;
    let steps = r"
  advise:
    advisor: true
    prompt: review
    output_file: advisor/advice.md
  apply:
    prompt: apply
    output_file: applied.txt
";
    let (result, started) = f.run("cat", steps, &NO_FLAGS, &[], Some(&inbox)).await;
    assert_eq!(result?.failed, 0);
    assert_eq!(started, ["advise", "apply"]);
    assert!(!crate::artifacts::read(&f.artifacts, ADVICE)?.contains(INSTRUCTION));
    assert!(crate::artifacts::read(&f.artifacts, "applied.txt")?.contains(INSTRUCTION));
    let summary = instruction_inbox::summary(&sessions, ID)?;
    assert!(summary.pending.is_empty());
    assert_eq!(summary.history.len(), 1);
    assert_eq!(summary.history[0].status, "delivered@apply");
    Ok(())
}
