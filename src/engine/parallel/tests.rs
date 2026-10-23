use std::sync::Mutex;

use super::*;
use crate::dag::ExecutionDag;
use crate::engine::{ExecutionResult, execute_steps_with_dag};
use crate::file_tracker::FileTracker;
use crate::option_handler::NoOpOptionHandler;
use crate::workflow::CompiledWorkflow;
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    compiled: CompiledWorkflow,
    dag: ExecutionDag,
    vars: VariableStore,
    starts: Mutex<Vec<String>>,
    logs: Mutex<Vec<String>>,
    checkpoint: Mutex<Option<(String, String)>>,
    before_steps: Mutex<std::collections::HashMap<String, crate::graph::NodeRuntime>>,
}

impl Fixture {
    fn new(yaml: &str) -> Self {
        let config = crate::config::WorkflowConfig::from_yaml(yaml)
            .unwrap_or_else(|error| panic!("{error}"));
        crate::config::validate_config(&config).unwrap_or_else(|error| panic!("{error}"));
        let compiled = crate::workflow::compile(config).unwrap_or_else(|error| panic!("{error}"));
        let dag = crate::dag::build_dag(&compiled, 3).unwrap_or_else(|error| panic!("{error}"));
        Self {
            root: TempDir::new().unwrap_or_else(|error| panic!("{error}")),
            compiled,
            dag,
            vars: VariableStore::new("input".to_string()),
            starts: Mutex::new(Vec::new()),
            logs: Mutex::new(Vec::new()),
            checkpoint: Mutex::new(None),
            before_steps: Mutex::new(std::collections::HashMap::new()),
        }
    }

    async fn run(
        &mut self,
        cancel: Option<&CancellationToken>,
        start: Option<String>,
    ) -> Result<ExecutionResult> {
        let start = start.unwrap_or_else(|| self.dag.start.clone());
        let mut tracker = FileTracker::with_root(self.root.path().to_path_buf());
        let ctx = ExecutionContext {
            compiled: &self.compiled,
            max_retries: 3,
            rate_limit_retries: 0,
            on_step_start: &|name| {
                self.starts
                    .lock()
                    .unwrap_or_else(|e| panic!("{e}"))
                    .push(name.to_string());
                Ok(())
            },
            cancel_token: cancel,
            option_handler: &NoOpOptionHandler,
            config_reloader: None,
            working_dir: Some(self.root.path()),
            skipped_steps: &[],
            flags: &crate::flags::NO_FLAGS,
            on_step_log: Some(&|stream, line| {
                self.logs
                    .lock()
                    .unwrap_or_else(|e| panic!("{e}"))
                    .push(format!("{stream}: {line}"));
            }),
        };
        execute_steps_with_dag(
            &ctx,
            &mut self.vars,
            &mut tracker,
            &mut self.dag,
            &start,
            &|checkpoint, dag| {
                if !dag.state.completed {
                    self.before_steps
                        .lock()
                        .unwrap_or_else(|e| panic!("{e}"))
                        .insert(checkpoint.step_name.to_string(), dag.state.runtime.clone());
                }
                *self.checkpoint.lock().unwrap_or_else(|e| panic!("{e}")) =
                    Some((checkpoint.node_id.clone(), serde_json::to_string(dag)?));
                Ok(())
            },
        )
        .await
    }

    fn outputs(&self) -> serde_json::Value {
        serde_json::from_str(self.vars.prev_output().unwrap_or_default())
            .unwrap_or_else(|error| panic!("{error}"))
    }
}

// Commands use POSIX shell syntax (`;`, `[ -f ]`, `>&2`) that cmd.exe cannot run.
#[cfg(unix)]
#[tokio::test]
async fn parallel_children_overlap_and_join_before_successor() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r"
steps:
  checks:
    parallel:
      first:
        timeout: '5'
        command: 'touch first.started; while [ ! -f second.started ]; do sleep 0.01; done; echo one; touch first.done'
      second:
        timeout: '5'
        command: 'touch second.started; while [ ! -f first.started ]; do sleep 0.01; done; echo two >&2; touch second.done'
  joined:
    command: test -f first.done && test -f second.done && touch joined
",
    );
    let result = fixture
        .run(None, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.failed, 0);
    assert_eq!(result.run, 2);
    assert!(fixture.root.path().join("joined").exists());
    let before = fixture
        .before_steps
        .lock()
        .unwrap_or_else(|e| panic!("{e}"));
    let runtime = &before["joined"];
    let saved_outputs: serde_json::Value =
        serde_json::from_str(runtime.prev_output.as_deref().unwrap_or_default())
            .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(runtime.prev_success, Some(true));
    assert_eq!(saved_outputs["first"]["success"], true);
    assert_eq!(saved_outputs["second"]["stderr"], "two\n");
    assert_eq!(
        *fixture.starts.lock().unwrap_or_else(|e| panic!("{e}")),
        ["checks", "joined"]
    );
    let logs = fixture.logs.lock().unwrap_or_else(|e| panic!("{e}"));
    assert!(logs.iter().any(|line| line == "stdout: [checks/first] one"));
    assert!(
        logs.iter()
            .any(|line| line == "stderr: [checks/second] two")
    );
}

#[tokio::test]
async fn parallel_results_publish_in_order_with_env_isolation_and_skips() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r#"
command: [sh, -c, 'sleep "$DELAY"; cat >/dev/null; printf result']
env:
  DELAY: '0'
steps:
  review:
    env:
      LITERAL: '{prev.output}'
    parallel:
      slow:
        env:
          DELAY: '0.1'
        prompt: 'first:{prev.output}'
      fast:
        prompt: 'second:{prev.output}'
      check_env:
        command: 'test "$LITERAL" = "{prev.output}"'
      skipped:
        skip: true
        command: 'touch unexpected'
      absent:
        when: { exists: '*.absent' }
        command: 'touch unexpected'
"#,
    );
    fixture
        .vars
        .set_prev_output(Some("literal{braces}".to_string()));
    fixture.vars.set_prev_success(Some(false));
    let result = fixture
        .run(None, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.failed, 0);
    let outputs = fixture.outputs();
    assert_eq!(outputs["slow"]["success"], true);
    assert_eq!(outputs["check_env"]["success"], true);
    assert_eq!(outputs["skipped"]["skipped"], true);
    assert_eq!(outputs["absent"]["skipped"], true);
    assert_eq!(fixture.vars.prev_success(), Some(true));
    assert!(
        fixture
            .vars
            .prev_output()
            .unwrap_or_default()
            .starts_with("{\"slow\":")
    );
    assert!(!fixture.root.path().join("unexpected").exists());
}

#[tokio::test]
async fn missing_jcode_model_is_fatal_for_parallel_blocks_with_or_without_if_fail() {
    let _process = crate::test_support::lock_process();
    let home = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
    let mut guards = crate::test_support::set_fake_home(home.path());
    #[cfg(unix)]
    {
        let bin = home.path().join("bin");
        guards.push(crate::test_support::install_exiting_jcode_stub(&bin));
    }
    guards.push(crate::test_support::EnvGuard::set(
        "JCODE_HOME",
        home.path().join("jcode-home"),
    ));
    for key in ["JCODE_MODEL", "CRUISE_MODEL", "CRUISE_PLAN_MODEL"] {
        guards.push(crate::test_support::EnvGuard::remove(key));
    }

    for parent_if_fail in ["", "    if:\n      fail: recover\n"] {
        let yaml = format!(
            "sdk: jcode\nsteps:\n  checks:\n    parallel:\n      missing-model:\n        prompt: inspect the change\n{parent_if_fail}  recover:\n    command: echo recovery-ran\n"
        );
        let mut fixture = Fixture::new(&yaml);

        let result = fixture.run(None, None).await;

        assert!(
            matches!(result, Err(crate::error::CruiseError::MissingJcodeModel(_))),
            "parallel missing-model errors must remain fatal (parent if.fail={parent_if_fail:?}): {result:?}"
        );
        assert_eq!(
            *fixture
                .starts
                .lock()
                .unwrap_or_else(|error| panic!("{error}")),
            ["checks"],
            "the if.fail recovery step must not run"
        );
    }
    assert!(home.path().join(".config/cruise/default.yaml").is_file());
}

// Commands use POSIX shell syntax that cmd.exe cannot run.
#[cfg(unix)]
#[tokio::test]
async fn parallel_parent_retries_all_children_and_handles_failure_once() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r"
steps:
  checks:
    parallel:
      flaky:
        command: 'if [ -f attempted ]; then exit 0; else touch attempted; exit 1; fi'
      sibling:
        command: 'echo run >> sibling.runs'
    if:
      fail: { retry: true }
  done:
    command: touch done
",
    );
    let result = fixture
        .run(None, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.failed, 0);
    assert_eq!(result.run, 3);
    assert_eq!(
        std::fs::read_to_string(fixture.root.path().join("sibling.runs")).unwrap_or_default(),
        "run\nrun\n"
    );
    assert!(fixture.root.path().join("done").exists());
}

// Commands use POSIX shell syntax that cmd.exe cannot run.
#[cfg(unix)]
#[tokio::test]
async fn parallel_fatal_prompt_error_waits_for_sibling_before_returning() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r"
command: [sh, -c, 'exit 7']
steps:
  checks:
    parallel:
      broken: { prompt: fail }
      sibling: { command: 'sleep 0.1; touch completed' }
  unexpected:
    command: touch unexpected
",
    );
    assert!(fixture.run(None, None).await.is_err());
    assert!(fixture.root.path().join("completed").exists());
    assert!(!fixture.root.path().join("unexpected").exists());
}

// Commands use POSIX shell syntax that cmd.exe cannot run.
#[cfg(unix)]
#[tokio::test]
async fn parallel_multiple_command_failures_count_as_one_failed_step() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r"
steps:
  checks:
    parallel:
      first: { command: 'echo first >&2; exit 1' }
      second: { command: 'echo second >&2; exit 2' }
",
    );
    let result = fixture
        .run(None, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.run, 1);
    assert_eq!(result.failed, 1);
    assert_eq!(fixture.vars.prev_success(), Some(false));
    assert_eq!(
        fixture.vars.prev_stderr(),
        Some("[checks/first] first\n\n[checks/second] second\n")
    );
    assert_eq!(fixture.outputs()["first"]["success"], false);
    assert_eq!(fixture.outputs()["second"]["success"], false);
}

// Commands use POSIX shell syntax that cmd.exe cannot run.
#[cfg(unix)]
#[tokio::test]
async fn parallel_child_timeout_does_not_cancel_sibling() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r"
steps:
  checks:
    parallel:
      slow: { command: 'sleep 30', timeout: '1' }
      sibling: { command: 'sleep 1.1; touch completed', timeout: '5' }
",
    );
    let result = fixture
        .run(None, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.failed, 1);
    assert_eq!(fixture.vars.prev_success(), Some(false));
    assert_eq!(fixture.outputs()["sibling"]["success"], true);
    assert!(
        fixture.outputs()["slow"]["stderr"]
            .as_str()
            .unwrap_or_default()
            .contains("timed out")
    );
    assert!(fixture.root.path().join("completed").exists());
}

#[tokio::test]
async fn parallel_block_timeout_drains_processes_and_follows_failure_edge() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r"
steps:
  checks:
    timeout: '1'
    parallel:
      first: { command: 'sleep 2; touch leaked' }
      second: { command: 'sleep 2; touch leaked' }
    if: { fail: recovered }
    next: unexpected
  recovered:
    command: touch recovered
    next: done
  unexpected:
    command: touch unexpected
  done:
    command: 'true'
",
    );
    let result = fixture
        .run(None, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.failed, 0);
    assert!(fixture.root.path().join("recovered").exists());
    assert!(!fixture.root.path().join("unexpected").exists());
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert!(!fixture.root.path().join("leaked").exists());
}

// Commands use POSIX shell syntax that cmd.exe cannot run.
#[cfg(unix)]
#[tokio::test]
async fn parallel_cancellation_keeps_parent_checkpoint_and_resume_reruns_block() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r"
steps:
  checks:
    parallel:
      quick:
        command: 'echo run >> quick.runs'
      slow:
        command: 'touch started; if [ ! -f resume ]; then sleep 30; fi; touch completed'
  done:
    command: touch done
",
    );
    let token = CancellationToken::new();
    let root = fixture.root.path().to_path_buf();
    let cancel = async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !root.join("started").exists() || !root.join("quick.runs").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|e| panic!("children failed to start: {e}"));
        token.cancel();
    };
    let (result, ()) = tokio::join!(fixture.run(Some(&token), None), cancel);
    assert!(matches!(result, Err(CruiseError::Interrupted)));
    assert!(!root.join("done").exists());
    assert!(!root.join("completed").exists());
    let (node, saved) = fixture
        .checkpoint
        .lock()
        .unwrap_or_else(|e| panic!("{e}"))
        .clone()
        .unwrap_or_else(|| panic!("missing checkpoint"));
    fixture.dag = serde_json::from_str(&saved).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(fixture.dag.step_name_for_node(&node), Some("checks"));
    std::fs::write(root.join("resume"), "").unwrap_or_else(|e| panic!("{e}"));
    let result = fixture
        .run(None, Some(node))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.failed, 0);
    assert_eq!(
        std::fs::read_to_string(root.join("quick.runs")).unwrap_or_default(),
        "run\nrun\n"
    );
    assert!(root.join("done").exists());
}

#[tokio::test]
async fn parallel_skip_uses_sequential_fallback() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r"
steps:
  checks:
    skip: true
    next: unexpected
    parallel:
      child: { command: touch unexpected }
  done:
    command: touch done
    next: end
  unexpected:
    command: touch unexpected
  end:
    command: 'true'
",
    );
    let result = fixture
        .run(None, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.skipped, 1);
    assert!(fixture.root.path().join("done").exists());
    assert!(!fixture.root.path().join("unexpected").exists());
}

#[tokio::test]
async fn parallel_blocks_run_inside_groups_and_after_pr() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r"
groups:
  review:
    steps:
      checks:
        parallel:
          first: { command: touch first }
          second: { command: touch second }
steps:
  review: { group: review }
after-pr:
  publish:
    parallel:
      docs: { command: touch docs }
      notify: { command: touch notify }
",
    );
    fixture
        .run(None, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(fixture.root.path().join("first").exists());
    assert!(fixture.root.path().join("second").exists());
    fixture.compiled = fixture.compiled.to_after_pr_compiled();
    fixture.dag = crate::dag::build_dag(&fixture.compiled, 3).unwrap_or_else(|e| panic!("{e}"));
    fixture
        .run(None, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(fixture.root.path().join("docs").exists());
    assert!(fixture.root.path().join("notify").exists());
}

// Commands use POSIX shell syntax that cmd.exe cannot run.
#[cfg(unix)]
#[tokio::test]
async fn parallel_env_model_arrays_and_stdin_keep_child_boundaries() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r#"
command: [sh, -c, 'cat >/dev/null; printf "%s:%s:%s:%s" "$1" "$SHARED" "$WORKFLOW_ONLY" "$PARENT_ONLY"', agent, '{model}']
model: workflow-model
env:
  SHARED: workflow
  WORKFLOW_ONLY: inherited
steps:
  checks:
    env:
      SHARED: parent
      PARENT_ONLY: '{input}'
    parallel:
      override:
        model: child-model
        env: { SHARED: child }
        prompt: '{input}'
      inherited:
        prompt: '{input}'
      commands:
        command:
          - 'if read line; then exit 1; fi; echo first > order'
          - 'test "$(cat order)" = first; echo second >> order; exit 7'
          - 'touch unexpected'
"#,
    );
    fixture.vars.set_prev_input(Some("old choice".to_string()));
    let result = fixture
        .run(None, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.failed, 1);
    let outputs = fixture.outputs();
    assert_eq!(
        outputs["override"]["output"],
        "child-model:child:inherited:input"
    );
    assert_eq!(
        outputs["inherited"]["output"],
        "workflow-model:parent:inherited:input"
    );
    assert_eq!(outputs["commands"]["output"], serde_json::Value::Null);
    assert_eq!(outputs["commands"]["success"], false);
    assert_eq!(fixture.vars.prev_input(), None);
    assert_eq!(
        std::fs::read_to_string(fixture.root.path().join("order")).unwrap_or_default(),
        "first\nsecond\n"
    );
    assert!(!fixture.root.path().join("unexpected").exists());
}

// Commands use POSIX shell syntax that cmd.exe cannot run.
#[cfg(unix)]
#[tokio::test]
async fn parallel_prompt_failure_recovery_waits_for_siblings() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r"
command: [sh, -c, 'exit 7']
steps:
  checks:
    parallel:
      broken: { prompt: fail }
      sibling: { command: 'sleep 0.1; touch completed' }
    if: { fail: recovered }
    next: unexpected
  recovered:
    command: test -f completed && touch recovered
    next: done
  unexpected:
    command: touch unexpected
  done:
    command: 'true'
",
    );
    let result = fixture
        .run(None, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.failed, 0);
    assert!(fixture.root.path().join("recovered").exists());
    assert!(!fixture.root.path().join("unexpected").exists());
}

#[tokio::test]
async fn parallel_parent_timeout_stops_descendants_after_shell_exit() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r"
steps:
  checks:
    timeout: '1'
    parallel:
      background: { command: '(sleep 2; touch leaked) &' }
      sibling: { command: 'sleep 30' }
    if: { fail: recovered }
  recovered:
    command: touch recovered
",
    );
    let result = fixture
        .run(None, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.failed, 0);
    assert!(fixture.root.path().join("recovered").exists());
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert!(
        !fixture.root.path().join("leaked").exists(),
        "descendant outlived the parallel timeout"
    );
}

#[cfg(unix)]
fn install_claude_stub(root: &std::path::Path) {
    crate::test_support::write_executable_script(
        &root.join("claude"),
        "#!/bin/sh\ncat >/dev/null\ntouch \"$CHILD.started\"\nsleep 2\ntouch \"$CHILD.stopped\"\n",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn parallel_sdk_timeouts_wait_for_backend_shutdown_before_successor() {
    let _lock = crate::test_support::lock_process();
    for timeout_owner in ["parent", "child"] {
        let parent_timeout = if timeout_owner == "parent" {
            "    timeout: '1'\n"
        } else {
            ""
        };
        let child_timeout = if timeout_owner == "child" {
            "        timeout: '1'\n"
        } else {
            ""
        };
        let mut fixture = Fixture::new(&format!(
            "sdk: claude\nsteps:\n  checks:\n{parent_timeout}    parallel:\n      agent:\n        prompt: wait\n        env: {{ CHILD: agent }}\n{child_timeout}      sibling:\n        command: 'sleep 1.2; touch sibling.done'\n    if: {{ fail: recovered }}\n  recovered:\n    command: 'test -f agent.stopped && test -f agent.started && touch recovered'\n"
        ));
        install_claude_stub(fixture.root.path());
        let _path = crate::test_support::prepend_to_path(fixture.root.path());
        let result = fixture
            .run(None, None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let stopped_before_successor = fixture.root.path().join("recovered").exists();
        // Keep the fixture alive while the old implementation's detached worker exits.
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(
            stopped_before_successor,
            "{timeout_owner} timeout returned before SDK shutdown: {result:?}"
        );
        assert_eq!(result.failed, 0);
        if timeout_owner == "child" {
            assert!(fixture.root.path().join("sibling.done").exists());
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn parallel_sdk_cancellation_waits_for_every_backend() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r"
sdk: claude
steps:
  checks:
    parallel:
      first: { prompt: wait, env: { CHILD: first } }
      second: { prompt: wait, env: { CHILD: second } }
  unexpected:
    command: touch unexpected
",
    );
    install_claude_stub(fixture.root.path());
    let _path = crate::test_support::prepend_to_path(fixture.root.path());
    let token = CancellationToken::new();
    let root = fixture.root.path().to_path_buf();
    let cancel = async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !root.join("first.started").exists() || !root.join("second.started").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|e| panic!("SDK children failed to start: {e}"));
        token.cancel();
    };
    let (result, ()) = tokio::join!(fixture.run(Some(&token), None), cancel);
    let stopped_at_return =
        root.join("first.stopped").exists() && root.join("second.stopped").exists();
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert!(matches!(result, Err(CruiseError::Interrupted)));
    assert!(
        stopped_at_return,
        "cancel returned while SDK children were running"
    );
    assert!(!root.join("unexpected").exists());
}

#[tokio::test]
async fn parallel_classic_prompt_timeout_stops_descendants_after_shell_exit() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r"
command: [sh, -c, 'cat >/dev/null; (sleep 2; touch leaked) &']
steps:
  checks:
    timeout: '1'
    parallel:
      agent: { prompt: review }
    if: { fail: recovered }
  recovered:
    command: touch recovered
",
    );
    let result = fixture
        .run(None, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.failed, 0);
    assert!(fixture.root.path().join("recovered").exists());
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert!(
        !fixture.root.path().join("leaked").exists(),
        "prompt descendant outlived the timeout"
    );
}

// The fake executable owns a real process lifetime while this HTTP fixture
// deliberately leaves each V2 turn pending until the engine cancels it.
#[cfg(unix)]
struct PendingOpenCode {
    task: tokio::task::JoinHandle<()>,
    _path: crate::test_support::EnvGuard,
}

#[cfg(unix)]
impl PendingOpenCode {
    async fn install(root: &std::path::Path) -> Self {
        use axum::response::IntoResponse as _;
        use futures::StreamExt as _;
        use serde_json::json;
        let directory = root.to_path_buf();
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let directory = directory.clone();
            async move {
                let path = request.uri().path().to_owned();
                let method = request.method().clone();
                let bytes = axum::body::to_bytes(request.into_body(), 1_048_576)
                    .await
                    .unwrap_or_default();
                let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
                if path == "/api/info" {
                    return axum::Json(json!({"version":"2.1.3", "pid":0, "urls":[], "paths":{"tmp":directory}})).into_response();
                }
                if path == "/api/plugin" {
                    let overlays: Vec<serde_json::Value> = std::fs::read_dir(&directory)
                        .unwrap_or_else(|e| panic!("{e}"))
                        .filter_map(std::result::Result::ok)
                        .filter(|entry| entry.file_name().to_string_lossy().starts_with("overlay."))
                        .filter_map(|entry| std::fs::read(entry.path()).ok())
                        .filter_map(|bytes| serde_json::from_slice(&bytes).ok()).collect();
                    let client = reqwest::Client::builder().no_proxy().build().unwrap_or_else(|e| panic!("{e}"));
                    for overlay in overlays {
                        let plugin = overlay["plugins"].as_array().and_then(|plugins| plugins.last()).and_then(serde_json::Value::as_str).unwrap_or_else(|| panic!("native plugin overlay missing"));
                        let bridge: serde_json::Value = serde_json::from_slice(&std::fs::read(std::path::Path::new(plugin).join("bridge.json")).unwrap_or_else(|e| panic!("{e}"))).unwrap_or_else(|e| panic!("{e}"));
                        let ready = client.post(format!("{}/ready", bridge["url"].as_str().unwrap_or_default()))
                            .header("authorization", bridge["token"].as_str().unwrap_or_default())
                            .json(&json!({"sessionID":bridge["sessionID"]})).send().await.unwrap_or_else(|e| panic!("{e}"));
                        assert!(ready.status().is_success(), "authenticated readiness failed");
                    }
                    return axum::Json(json!({"location":{"directory":directory},"data": []})).into_response();
                }
                if path == "/api/event" {
                    let stream = futures::stream::once(async {
                        Ok::<_, std::convert::Infallible>("data: {\"id\":\"evt_connected\",\"type\":\"server.connected\",\"data\":{}}\n\n")
                    }).chain(futures::stream::pending());
                    return ([("content-type", "text/event-stream")], axum::body::Body::from_stream(stream)).into_response();
                }
                if path == "/api/session" {
                    return axum::Json(json!({"data":{"id":body["id"], "location":{"directory":directory}}})).into_response();
                }
                if path.ends_with("/prompt") {
                    std::fs::write(directory.join(format!("{}.prompt", body["id"].as_str().unwrap_or("input"))), "accepted").unwrap_or_else(|e| panic!("{e}"));
                    return axum::Json(json!({"data":{"id":body["id"]}})).into_response();
                }
                if path.ends_with("/inbox") { return axum::Json(json!({"data":[]})).into_response(); }
                if path.ends_with("/log") {
                    let session = path.split('/').nth(4).unwrap_or_default();
                    let synced = json!({"type":"log.synced", "aggregateID":session, "seq":0});
                    return (
                        [("content-type", "text/event-stream")],
                        format!("data: {synced}\n\n"),
                    ).into_response();
                }
                if path.ends_with("/interrupt") { return axum::Json(json!({"interrupted":true})).into_response(); }
                if method == axum::http::Method::GET {
                    return axum::Json(json!({"data":{"id":path.rsplit('/').next(), "location":{"directory":directory}}})).into_response();
                }
                axum::http::StatusCode::NO_CONTENT.into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let address = listener.local_addr().unwrap_or_else(|e| panic!("{e}"));
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .unwrap_or_else(|e| panic!("{e}"));
        });
        crate::test_support::write_executable_script(
            &root.join("opencode"),
            &format!(
                "#!/bin/sh\ncase \"$1\" in --version) echo 'opencode v2.1.3'; exit 0;; esac\numask 077\necho $$ >> '{}'/started.pids\nprintf '%s' \"$OPENCODE_CONFIG_CONTENT\" > '{}/overlay.'$$\ntrap 'touch \"{}/stopped.$$\"; exit 0' INT TERM\nprintf '%s\\n' '{{\"url\":\"http://{address}\"}}'\ncat >/dev/null\ntouch '{}/stopped.'$$\n",
                root.display(),
                root.display(),
                root.display(),
                root.display()
            ),
        );
        Self {
            task,
            _path: crate::test_support::prepend_to_path(root),
        }
    }
}

#[cfg(unix)]
impl Drop for PendingOpenCode {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(unix)]
fn assert_opencode_children_reaped(root: &std::path::Path, count: usize) {
    let pids = std::fs::read_to_string(root.join("started.pids"))
        .unwrap_or_else(|e| panic!("no OpenCode started: {e}"));
    assert_eq!(pids.lines().count(), count);
    for pid in pids.lines() {
        let pid: i32 = pid.parse().unwrap_or_else(|e| panic!("{e}"));
        // SAFETY: signal zero only checks whether this fixture's child still exists.
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "OpenCode child {pid} still alive at engine return"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn parallel_opencode_parent_and_child_timeouts_reap_before_successor() {
    let _lock = crate::test_support::lock_process();
    let _config = crate::test_support::EnvGuard::remove("OPENCODE_CONFIG_CONTENT");
    let home = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let _home = crate::test_support::set_fake_home(home.path());
    for owner in ["parent", "child"] {
        let parent = if owner == "parent" {
            "    timeout: '1'\n"
        } else {
            ""
        };
        let child = if owner == "child" {
            "        timeout: '1'\n"
        } else {
            ""
        };
        let mut fixture = Fixture::new(&format!(
            "sdk: opencode\nmodel: fixture/pending\nsteps:\n  checks:\n{parent}    parallel:\n      agent:\n        prompt: wait\n{child}      sibling:\n        command: 'sleep 1.2; touch sibling.done'\n    if: {{ fail: recovered }}\n  recovered:\n    command: 'while read pid; do if kill -0 \"$pid\" 2>/dev/null; then exit 1; fi; done < started.pids; touch recovered'\n"
        ));
        let _server = PendingOpenCode::install(fixture.root.path()).await;
        let result = fixture
            .run(None, None)
            .await
            .unwrap_or_else(|e| panic!("{owner}: {e}"));
        assert_eq!(result.failed, 0, "{owner} timeout recovery failed");
        assert!(fixture.root.path().join("recovered").exists());
        assert!(
            std::fs::read_dir(fixture.root.path())
                .unwrap_or_else(|e| panic!("{e}"))
                .filter_map(std::result::Result::ok)
                .any(|entry| entry.path().extension().is_some_and(|ext| ext == "prompt")),
            "{owner} timeout must exercise an accepted V2 turn, not a startup failure"
        );
        assert_opencode_children_reaped(fixture.root.path(), 1);
        if owner == "child" {
            assert!(fixture.root.path().join("sibling.done").exists());
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn parallel_opencode_cancellation_reaps_all_children_before_return() {
    let _lock = crate::test_support::lock_process();
    let _config = crate::test_support::EnvGuard::remove("OPENCODE_CONFIG_CONTENT");
    let home = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let _home = crate::test_support::set_fake_home(home.path());
    let mut fixture = Fixture::new(
        "sdk: opencode\nmodel: fixture/pending\nsteps:\n  checks:\n    parallel:\n      first: { prompt: wait }\n      second: { prompt: wait }\n  unexpected:\n    command: touch unexpected\n",
    );
    let _server = PendingOpenCode::install(fixture.root.path()).await;
    let root = fixture.root.path().to_path_buf();
    let token = CancellationToken::new();
    let cancel = async {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let accepted = std::fs::read_dir(&root)
                    .unwrap_or_else(|e| panic!("{e}"))
                    .filter_map(std::result::Result::ok)
                    .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "prompt"))
                    .count();
                if accepted == 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|e| panic!("both OpenCode turns must be accepted before cancelling: {e}"));
        token.cancel();
    };
    let (result, ()) = tokio::join!(fixture.run(Some(&token), None), cancel);
    assert!(
        matches!(result, Err(CruiseError::Interrupted)),
        "{result:?}"
    );
    assert_opencode_children_reaped(&root, 2);
    assert!(!root.join("unexpected").exists());
}
