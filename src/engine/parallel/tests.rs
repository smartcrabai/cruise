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
    let joined_node = fixture
        .dag
        .first_node_for_step("joined")
        .unwrap_or_else(|| panic!("missing joined node"));
    let runtime = &fixture.dag.nodes[joined_node].runtime;
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
async fn parallel_prompts_use_same_input_and_publish_ordered_results() {
    let _lock = crate::test_support::lock_process();
    let mut fixture = Fixture::new(
        r#"
command: [sh, -c, 'sleep "$DELAY"; cat']
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
    assert_eq!(outputs["slow"]["output"], "first:literal{braces}");
    assert_eq!(outputs["fast"]["output"], "second:literal{braces}");
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
