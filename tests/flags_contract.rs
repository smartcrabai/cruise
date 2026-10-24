#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "isolated contract fixtures")]
//! Contract tests for workflow flags (`flags:` / `flag:`).

use std::sync::Mutex;

use cruise::cancellation::CancellationToken;
use cruise::config::{StringOrVec, WorkflowConfig, validate_config};
use cruise::engine::{ExecutionContext, ReloadedWorkflow, execute_steps_with_graph};
use cruise::file_tracker::FileTracker;
use cruise::flags::{self, FlagValues};
use cruise::graph::build_graph;
use cruise::graph::validation::validate_workflow;
use cruise::option_handler::OptionHandler;
use cruise::session::{SessionManager, SessionState};
use cruise::session_config::SessionConfigRef;
use cruise::variable::VariableStore;
use cruise::workflow::{CompiledWorkflow, compile};

struct NoOpOptionHandler;
impl OptionHandler for NoOpOptionHandler {
    fn select_option(
        &self,
        _: &[cruise::step::OptionChoice],
        _: Option<&str>,
    ) -> cruise::error::Result<cruise::step::option::OptionResult> {
        panic!("unexpected option interaction");
    }
}

fn parse(yaml: &str) -> WorkflowConfig {
    WorkflowConfig::from_yaml(yaml).unwrap()
}

fn values(pairs: &[(&str, bool)]) -> FlagValues {
    pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
}

fn invalid(yaml: &str) -> String {
    match WorkflowConfig::from_yaml(yaml) {
        Err(error) => error.to_string(),
        Ok(config) => match validate_config(&config) {
            Err(error) => error.to_string(),
            Ok(()) => panic!("expected invalid config:\n{yaml}"),
        },
    }
}

const TWO_FLAGS: &str = "command: [echo]
flags:
  e2e:
    description: Run E2E
  docs: {}
steps:
  build:
    command: 'true'
  test:
    flag: e2e
    command: 'true'
  publish:
    flag: docs
    command: 'true'
";

// ---------------------------------------------------------------- config

#[test]
fn config_parses_flags_in_declaration_order_and_step_flag() {
    let config = parse(TWO_FLAGS);
    let names: Vec<&str> = config.flags.keys().map(String::as_str).collect();
    assert_eq!(names, ["e2e", "docs"]);
    assert_eq!(config.flags["e2e"].description.as_deref(), Some("Run E2E"));
    let flag = config.steps["test"].flag.as_ref().unwrap();
    assert_eq!(flag.as_slice().len(), 1);
    assert_eq!(flag.as_slice()[0], "e2e");
    let serialized = serde_yaml::to_value(&config).unwrap();
    assert_eq!(serialized["steps"]["test"]["flag"].as_str(), Some("e2e"));
    assert_eq!(config.steps["build"].flag, None);
    validate_config(&config).unwrap();
}

#[test]
fn flag_declaration_cannot_carry_a_default_value() {
    let result = WorkflowConfig::from_yaml(
        "command: [echo]\nflags:\n  e2e:\n    default: true\nsteps:\n  a:\n    flag: e2e\n    command: 'true'\n",
    );
    assert!(result.is_err(), "flags must not accept a default");
}

#[test]
fn config_without_flags_does_not_serialize_flag_keys() {
    let config = parse("command: [echo]\nsteps:\n  a:\n    command: 'true'\n");
    let yaml = serde_yaml::to_string(&config).unwrap();
    assert!(!yaml.contains("flags"), "{yaml}");
    assert!(!yaml.contains("flag:"), "{yaml}");
}

#[test]
fn validation_rejects_reference_to_undeclared_flag() {
    let message = invalid(
        "command: [echo]\nflags:\n  e2e: {}\nsteps:\n  a:\n    flag: e2e\n    command: 'true'\n  b:\n    flag: nope\n    command: 'true'\n",
    );
    assert!(message.contains("nope"), "{message}");
}

#[test]
fn validation_rejects_declared_but_unused_flag() {
    let message = invalid(
        "command: [echo]\nflags:\n  e2e: {}\n  unused: {}\nsteps:\n  a:\n    flag: e2e\n    command: 'true'\n",
    );
    assert!(message.contains("unused"), "{message}");
}

#[test]
fn validation_rejects_flag_names_with_invalid_characters() {
    let message = invalid(
        "command: [echo]\nflags:\n  \"bad name\": {}\nsteps:\n  a:\n    flag: \"bad name\"\n    command: 'true'\n",
    );
    assert!(message.contains("bad name"), "{message}");
}

#[test]
fn validation_accepts_all_declared_names_in_a_flag_list() {
    let config = parse(
        "command: [echo]\nflags:\n  e2e: {}\n  docs: {}\nsteps:\n  a:\n    flag: [e2e, docs]\n    command: 'true'\n",
    );
    validate_config(&config).unwrap();
}

#[test]
fn validation_rejects_empty_malformed_and_unknown_flag_lists() {
    for flag in ["[]", "[e2e, '']", "[e2e, true]", "[e2e, ghost]"] {
        let yaml = format!(
            "command: [echo]\nflags:\n  e2e: {{}}\nsteps:\n  a:\n    flag: {flag}\n    command: 'true'\n"
        );
        let message = invalid(&yaml);
        assert!(!message.is_empty(), "{flag}");
        if flag.contains("ghost") {
            assert!(message.contains("ghost"), "{message}");
        }
    }
}

#[test]
fn native_squash_merge_is_last_after_pr_only_and_keeps_its_phase() {
    let config = parse(
        "command: [echo]\nflags:\n  merge: {}\nsteps:\n  main:\n    command: 'true'\nafter-pr:\n  merge:\n    squash-merge: true\n    flag: merge\n",
    );
    validate_config(&config).unwrap();
    let skippable = cruise::workflow::list_skippable_after_pr_steps(&config).unwrap();
    let ids: Vec<&str> = skippable.iter().map(|step| step.id.as_str()).collect();
    assert_eq!(ids, ["merge"]);
    let serialized = serde_yaml::to_value(&config).unwrap();
    assert_eq!(
        serialized["after-pr"]["merge"]["squash-merge"].as_bool(),
        Some(true)
    );

    let compiled = compile(config).unwrap();
    assert!(
        !compiled
            .steps
            .values()
            .any(|step| step.squash_merge.is_some())
    );
    assert_eq!(compiled.after_pr.len(), 1);
    let after_pr = compiled.to_after_pr_compiled();
    assert!(matches!(
        cruise::step::StepKind::try_from(after_pr.steps["merge"].clone()),
        Ok(cruise::step::StepKind::SquashMerge)
    ));
}

#[test]
fn native_squash_merge_rejects_wrong_phase_order_and_gates() {
    let wrong_phase = "command: [echo]\nflags:\n  merge: {}\nsteps:\n  merge:\n    squash-merge: true\n    flag: merge\n";
    assert!(invalid(wrong_phase).contains("after-pr"));

    let not_last = "command: [echo]\nflags:\n  merge: {}\nsteps:\n  main:\n    command: 'true'\nafter-pr:\n  merge:\n    squash-merge: true\n    flag: merge\n  later:\n    command: 'true'\n";
    assert!(invalid(not_last).contains("last after-pr step"));

    let no_merge_opt_in = "command: [echo]\nflags:\n  after_pr: {}\nsteps:\n  main:\n    command: 'true'\nafter-pr:\n  regular:\n    flag: after_pr\n    command: 'true'\n  merge:\n    squash-merge: true\n    flag: after_pr\n";
    assert!(invalid(no_merge_opt_in).contains("merge' flag"));
    let missing_declaration = parse(
        "command: [echo]\nsteps:\n  main:\n    command: 'true'\nafter-pr:\n  merge:\n    squash-merge: true\n    flag: merge\n",
    );
    assert!(compile(missing_declaration).is_err());

    let extra_gate = "command: [echo]\nflags:\n  merge: {}\n  after_pr: {}\nsteps:\n  main:\n    command: 'true'\nafter-pr:\n  regular:\n    flag: after_pr\n    command: 'true'\n  merge:\n    squash-merge: true\n    flag: [merge, after_pr]\n";
    assert!(invalid(extra_gate).contains("exactly the 'merge' flag"));
}

#[test]
fn native_squash_merge_rejects_group_and_parallel_placement() {
    let in_group = "flags:\n  merge: {}\ngroups:\n  g:\n    steps:\n      native:\n        squash-merge: true\n        flag: merge\nsteps:\n  main:\n    command: 'true'\nafter-pr:\n  use:\n    flag: merge\n    command: 'true'\n";
    assert!(invalid(in_group).contains("squash-merge"));

    let in_parallel = "flags:\n  merge: {}\nsteps:\n  main:\n    command: 'true'\nafter-pr:\n  fan:\n    parallel:\n      native:\n        squash-merge: true\n        flag: merge\n  use:\n    flag: merge\n    command: 'true'\n";
    assert!(invalid(in_parallel).contains("squash-merge"));
}

#[test]
fn native_squash_merge_rejects_executable_payloads_and_false_values() {
    for field in [
        "command: 'gh pr merge'",
        "output_file: result.md",
        "model: claude-opus",
        "computer_use: true",
        "permission: full",
        "allow_commit: true",
    ] {
        let yaml = format!(
            "command: [echo]\nflags:\n  merge: {{}}\nsteps:\n  main:\n    command: 'true'\nafter-pr:\n  merge:\n    squash-merge: true\n    flag: merge\n    {field}\n"
        );
        let message = invalid(&yaml);
        assert!(!message.is_empty(), "{field}: {message}");
    }

    let false_value = invalid(
        "command: [echo]\nflags:\n  merge: {}\nsteps:\n  main:\n    command: 'true'\nafter-pr:\n  merge:\n    squash-merge: false\n    flag: merge\n",
    );
    assert!(false_value.contains("true"), "{false_value}");
    let malformed = WorkflowConfig::from_yaml(
        "command: [echo]\nflags:\n  merge: {}\nsteps:\n  main:\n    command: 'true'\nafter-pr:\n  merge:\n    squash-merge: 'true'\n    flag: merge\n",
    );
    assert!(malformed.is_err());
}

#[test]
fn validation_rejects_flag_on_group_inner_step_in_config_and_compile() {
    let yaml = "command: [echo]
flags:
  e2e: {}
groups:
  review:
    steps:
      inner:
        flag: e2e
        command: 'true'
steps:
  run:
    group: review
";
    let message = invalid(yaml);
    assert!(message.contains("flag"), "{message}");
    // compile() is reachable without validate_config and must reject it too.
    assert!(compile(parse(yaml)).is_err());
}

#[test]
fn validation_rejects_flag_on_parallel_child() {
    let message = invalid(
        "command: [echo]\nflags:\n  e2e: {}\nsteps:\n  fan:\n    flag: e2e\n    parallel:\n      one:\n        flag: e2e\n        command: 'true'\n      two:\n        command: 'true'\n",
    );
    assert!(message.contains("flag"), "{message}");
}

#[test]
fn validation_allows_flag_on_parallel_parent_group_call_and_after_pr() {
    let config = parse(
        "command: [echo]
flags:
  a: {}
  b: {}
  c: {}
groups:
  review:
    steps:
      inner:
        command: 'true'
steps:
  fan:
    flag: a
    parallel:
      one:
        command: 'true'
      two:
        command: 'true'
  call:
    flag: b
    group: review
after-pr:
  publish:
    flag: c
    command: 'true'
",
    );
    validate_config(&config).unwrap();
}

// -------------------------------------------------------------- workflow

#[test]
fn group_call_flag_applies_to_every_expanded_substep() {
    let compiled = compile(parse(
        "command: [echo]
flags:
  e2e: {}
  docs: {}
groups:
  review:
    steps:
      one:
        command: 'true'
      two:
        command: 'true'
steps:
  call:
    group: review
    flag: [e2e, docs]
  other:
    command: 'true'
",
    ))
    .unwrap();
    for (name, step) in &compiled.steps {
        let actual = step.flag.as_ref().map(|flag| {
            flag.as_slice()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        });
        let expected = if name.starts_with("call/") {
            Some(vec!["e2e", "docs"])
        } else {
            None
        };
        assert_eq!(actual, expected, "step {name}");
    }
    assert!(compiled.steps.keys().any(|n| n.starts_with("call/")));
}

#[test]
fn after_pr_group_call_flag_is_propagated() {
    let compiled = compile(parse(
        "command: [echo]
flags:
  e2e: {}
groups:
  g:
    steps:
      one:
        command: 'true'
steps:
  main:
    command: 'true'
after-pr:
  call:
    group: g
    flag: [e2e]
",
    ))
    .unwrap();
    assert!(!compiled.after_pr.is_empty());
    assert!(compiled.after_pr.values().all(|step| {
        step.flag
            .as_ref()
            .is_some_and(|flag| flag.as_slice()[0] == "e2e")
    }));
}

fn write(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn workflow_call_flag_reaches_all_expanded_steps_not_only_the_first() {
    // Regression: validate_call_site is a denylist, so a missing propagation
    // would let the flag silently disappear.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "callee.yaml",
        "command: [x]\nsteps:\n  one:\n    command: 'true'\n  two:\n    command: 'true'\n  three:\n    command: 'true'\n",
    );
    let parent = write(
        dir.path(),
        "parent.yaml",
        "command: [echo]\nflags:\n  docs: {}\n  extra: {}\nsteps:\n  before:\n    command: 'true'\n  shared:\n    workflow_call: ./callee.yaml\n    flag: [docs, extra]\n  after:\n    command: 'true'\n",
    );
    let config = cruise::workflow_call::resolve_workflow_calls_from_path(parent).unwrap();
    let flagged: Vec<&str> = config
        .steps
        .iter()
        .filter(|(_, step)| {
            step.flag
                .as_ref()
                .is_some_and(|flag| flag.as_slice().len() == 2)
        })
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(flagged, ["shared/one", "shared/two", "shared/three"]);
    for name in &flagged {
        let names: Vec<&str> = config.steps[*name]
            .flag
            .as_ref()
            .unwrap()
            .as_slice()
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(names, ["docs", "extra"]);
    }
    assert_eq!(config.steps["before"].flag, None);
    assert_eq!(config.steps["after"].flag, None);
}

#[test]
fn workflow_call_rejects_callee_steps_that_carry_a_flag() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "callee.yaml",
        "command: [x]\nflags:\n  inner: {}\nsteps:\n  one:\n    flag: inner\n    command: 'true'\n",
    );
    let parent = write(
        dir.path(),
        "parent.yaml",
        "command: [echo]\nsteps:\n  shared:\n    workflow_call: ./callee.yaml\n",
    );
    assert!(cruise::workflow_call::resolve_workflow_calls_from_path(parent).is_err());
}

// ------------------------------------------------------- flags module API

#[test]
fn apply_enabled_turns_selected_declared_flags_on() {
    let choices = flags::choices(&parse(TWO_FLAGS));
    let result = flags::apply_enabled(&values(&[("e2e", false)]), &choices, &["e2e".to_string()]);
    assert_eq!(result.get("e2e"), Some(&true));
}

#[test]
fn apply_enabled_explicitly_disables_every_unselected_declaration() {
    let choices = flags::choices(&parse(TWO_FLAGS));
    let result = flags::apply_enabled(&values(&[("e2e", true)]), &choices, &[]);
    assert_eq!(result, values(&[("e2e", false), ("docs", false)]));
}

#[test]
fn apply_enabled_preserves_undeclared_keys_without_adding_unknown_selections() {
    let choices = flags::choices(&parse(TWO_FLAGS));
    let current = values(&[("legacy_on", true), ("legacy_off", false)]);
    let result = flags::apply_enabled(&current, &choices, &["unknown".to_string()]);
    assert_eq!(result.get("legacy_on"), Some(&true));
    assert_eq!(result.get("legacy_off"), Some(&false));
    assert!(!result.contains_key("unknown"));
    assert_eq!(
        current,
        values(&[("legacy_on", true), ("legacy_off", false)])
    );
}

#[test]
fn apply_enabled_without_declarations_preserves_current_values() {
    let current = values(&[("legacy", true)]);
    assert_eq!(flags::apply_enabled(&current, &[], &[]), current);
}

#[test]
fn parse_assignment_accepts_bare_enable_and_explicit_on_off_values() {
    assert_eq!(flags::parse_assignment("e2e"), Ok(("e2e".into(), true)));
    assert_eq!(flags::parse_assignment("e2e=on"), Ok(("e2e".into(), true)));
    assert_eq!(
        flags::parse_assignment("e2e=off"),
        Ok(("e2e".into(), false))
    );
    assert_eq!(flags::parse_assignment("a=true"), Ok(("a".into(), true)));
    assert_eq!(flags::parse_assignment("a=false"), Ok(("a".into(), false)));
    for bad in ["", "=on", "e2e=", "e2e=maybe"] {
        assert!(flags::parse_assignment(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn from_cli_rejects_duplicate_names() {
    let ok = flags::from_cli(&[("a".into(), true), ("b".into(), false)]).unwrap();
    assert_eq!(ok, values(&[("a", true), ("b", false)]));
    assert!(flags::from_cli(&[("a".into(), true), ("a".into(), false)]).is_err());
}

#[test]
fn validate_names_rejects_undeclared_and_lists_declared_names() {
    let config = parse(TWO_FLAGS);
    flags::validate_names(&config, &values(&[("e2e", true)])).unwrap();
    let error = flags::validate_names(&config, &values(&[("ghost", true)]))
        .unwrap_err()
        .to_string();
    assert!(error.contains("ghost"), "{error}");
    assert!(error.contains("e2e") && error.contains("docs"), "{error}");
}

#[test]
fn is_disabled_uses_any_enabled_flag_from_a_list() {
    let v = values(&[("on", true), ("off", false)]);
    let single_on = StringOrVec::from("on");
    let single_off = StringOrVec::from("off");
    let one_enabled = StringOrVec::Multiple(vec!["off".to_string(), "on".to_string()]);
    let none_enabled = StringOrVec::Multiple(vec!["off".to_string(), "unset".to_string()]);

    assert!(!flags::is_disabled(None, &v));
    assert!(!flags::is_disabled(Some(&single_on), &v));
    assert!(flags::is_disabled(Some(&single_off), &v));
    assert!(flags::is_disabled(Some(&none_enabled), &v));
    assert!(!flags::is_disabled(Some(&one_enabled), &v));
}

#[test]
fn choices_expose_declared_flags_in_order() {
    let choices = flags::choices(&parse(TWO_FLAGS));
    let names: Vec<&str> = choices.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["e2e", "docs"]);
    assert_eq!(choices[0].description.as_deref(), Some("Run E2E"));
    let json = serde_json::to_value(&choices[0]).unwrap();
    assert_eq!(json["name"], "e2e");
    assert_eq!(json["description"], "Run E2E");
}

#[test]
fn preflight_skips_merges_user_skips_with_flag_off_steps_in_main_and_after_pr() {
    let compiled = compile(parse(
        "command: [echo]
flags:
  e2e: {}
  docs: {}
steps:
  build:
    command: 'true'
  test:
    flag: [e2e, docs]
    command: 'true'
  lint:
    command: 'true'
after-pr:
  publish:
    flag: docs
    command: 'true'
  notify:
    flag: [e2e, docs]
    command: 'true'
",
    ))
    .unwrap();
    let mut skips =
        flags::preflight_skips(&compiled, &values(&[("e2e", true)]), &["lint".to_string()]);
    skips.sort();
    assert_eq!(skips, ["lint", "publish"]);

    let mut skips = flags::preflight_skips(&compiled, &values(&[("docs", true)]), &[]);
    skips.sort();
    assert_eq!(skips, Vec::<String>::new());

    let mut skips = flags::preflight_skips(&compiled, &FlagValues::new(), &[]);
    skips.sort();
    assert_eq!(skips, ["notify", "publish", "test"]);
}

// ------------------------------------------------------- graph validation

#[test]
fn flag_off_step_counts_as_always_skipped_in_preflight() {
    // `a` loops on itself forever unless it is skipped.
    let compiled = compile(parse(
        "command: [echo]\nflags:\n  loop: {}\nsteps:\n  a:\n    flag: loop\n    command: 'true'\n    next: a\n",
    ))
    .unwrap();
    let graph = build_graph(&compiled, 3).unwrap();
    let absent = flags::preflight_skips(&compiled, &FlagValues::new(), &[]);
    validate_workflow(&compiled, &graph, &graph.start, &absent).unwrap();
    let on = flags::preflight_skips(&compiled, &values(&[("loop", true)]), &[]);
    assert!(validate_workflow(&compiled, &graph, &graph.start, &on).is_err());
    let off = flags::preflight_skips(&compiled, &values(&[("loop", false)]), &[]);
    validate_workflow(&compiled, &graph, &graph.start, &off).unwrap();
}

// ----------------------------------------------------------------- engine

type Logs = Mutex<Vec<(String, String)>>;

struct Run {
    result: cruise::error::Result<cruise::engine::ExecutionResult>,
    logs: Vec<(String, String)>,
}

async fn run(
    dir: &std::path::Path,
    yaml: &str,
    flag_values: &FlagValues,
    reloader: Option<&(dyn Fn() -> cruise::error::Result<Option<ReloadedWorkflow>> + Send + Sync)>,
) -> Run {
    let compiled: CompiledWorkflow = compile(parse(yaml)).unwrap();
    let mut graph = build_graph(&compiled, 3).unwrap();
    let logs: Logs = Mutex::new(Vec::new());
    let on_log = |stream: &str, line: &str| {
        logs.lock().unwrap().push((stream.into(), line.into()));
    };
    let _token = CancellationToken::new();
    let ctx = ExecutionContext {
        compiled: &compiled,
        max_retries: 3,
        rate_limit_retries: 0,
        on_step_start: &|_| Ok(()),
        cancel_token: None,
        option_handler: &NoOpOptionHandler,
        config_reloader: reloader,
        working_dir: Some(dir),
        skipped_steps: &[],
        flags: flag_values,
        on_step_log: Some(&on_log),
    };
    let result = execute_steps_with_graph(
        &ctx,
        &mut VariableStore::new(String::new()),
        &mut FileTracker::with_root(dir.into()),
        &mut graph,
        &|_, _| Ok(()),
    )
    .await;
    let logs = logs.lock().unwrap().clone();
    Run { result, logs }
}

const ENGINE_YAML: &str = "command: [echo]
flags:
  e2e: {}
steps:
  build:
    command: touch build.txt
  e2e-test:
    flag: e2e
    command: touch e2e.txt
  finish:
    command: touch finish.txt
";

#[tokio::test]
async fn engine_skips_flag_off_steps_and_logs_the_flag() {
    let dir = tempfile::tempdir().unwrap();
    let r = run(dir.path(), ENGINE_YAML, &values(&[("e2e", false)]), None).await;
    let result = r.result.unwrap();
    assert_eq!((result.run, result.skipped), (2, 1));
    assert!(dir.path().join("build.txt").exists());
    assert!(!dir.path().join("e2e.txt").exists());
    assert!(dir.path().join("finish.txt").exists());
    assert!(
        r.logs
            .iter()
            .any(|(s, l)| s == "info" && l.contains("e2e-test") && l.contains("e2e")),
        "{:?}",
        r.logs
    );
}

#[tokio::test]
async fn engine_runs_flag_on_steps() {
    let dir = tempfile::tempdir().unwrap();
    let r = run(dir.path(), ENGINE_YAML, &values(&[("e2e", true)]), None).await;
    let result = r.result.unwrap();
    assert_eq!((result.run, result.skipped), (3, 0));
    assert!(dir.path().join("e2e.txt").exists());
}

#[tokio::test]
async fn engine_treats_absent_flag_values_as_off() {
    let dir = tempfile::tempdir().unwrap();
    let r = run(dir.path(), ENGINE_YAML, &FlagValues::new(), None).await;
    let result = r.result.unwrap();
    assert_eq!((result.run, result.skipped), (2, 1));
    assert!(dir.path().join("build.txt").exists());
    assert!(!dir.path().join("e2e.txt").exists());
    assert!(dir.path().join("finish.txt").exists());
}

#[tokio::test]
async fn flag_off_step_does_not_evaluate_its_skip_variable() {
    // `prev.success` is undefined for the first step; evaluating it would error.
    let dir = tempfile::tempdir().unwrap();
    let yaml = "command: [echo]
flags:
  e2e: {}
steps:
  first:
    flag: e2e
    skip: prev.success
    command: touch first.txt
  second:
    command: touch second.txt
";
    let r = run(dir.path(), yaml, &values(&[("e2e", false)]), None).await;
    let result = r.result.unwrap();
    assert_eq!((result.run, result.skipped), (1, 1));
    assert!(!dir.path().join("first.txt").exists());
}

#[tokio::test]
async fn flag_off_group_call_skips_every_substep() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = "command: [echo]
flags:
  e2e: {}
groups:
  review:
    steps:
      one:
        command: touch one.txt
      two:
        command: touch two.txt
steps:
  call:
    group: review
    flag: e2e
  finish:
    command: touch finish.txt
";
    let r = run(dir.path(), yaml, &values(&[("e2e", false)]), None).await;
    let result = r.result.unwrap();
    assert_eq!((result.run, result.skipped), (1, 2));
    assert!(!dir.path().join("one.txt").exists());
    assert!(!dir.path().join("two.txt").exists());
    assert!(dir.path().join("finish.txt").exists());
}

#[tokio::test]
async fn hot_reload_with_absent_new_flag_runs_with_it_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let initial = "command: [echo]\nsteps:\n  a:\n    command: touch original.txt\n";
    let reloaded = compile(parse(
        "command: [echo]\nflags:\n  new: {}\nsteps:\n  a:\n    flag: new\n    command: touch reloaded.txt\n  b:\n    command: touch continued.txt\n",
    ))
    .unwrap();
    let reload_callback = move || {
        Ok(Some(ReloadedWorkflow {
            compiled: reloaded.clone(),
            retry_policy: None,
        }))
    };
    let r = run(
        dir.path(),
        initial,
        &FlagValues::new(),
        Some(&reload_callback),
    )
    .await;
    r.result.unwrap();
    assert!(!dir.path().join("original.txt").exists());
    assert!(!dir.path().join("reloaded.txt").exists());
    assert!(dir.path().join("continued.txt").exists());
}

// ---------------------------------------------------------------- session

fn new_state() -> SessionState {
    SessionState::new(
        "20261019000000".into(),
        "/repo".into(),
        SessionConfigRef::InlineSnapshot,
        "input".into(),
    )
}

#[test]
fn session_flags_default_to_empty_and_are_omitted_from_state_json() {
    let state = new_state();
    assert!(state.flags.is_empty());
    let json = serde_json::to_value(&state).unwrap();
    assert!(json.get("flags").is_none(), "{json}");
    let back: SessionState = serde_json::from_value(json).unwrap();
    assert!(back.flags.is_empty());
}

#[test]
fn session_flags_round_trip_through_the_session_store() {
    let dir = tempfile::tempdir().unwrap();
    let manager = SessionManager::new(dir.path().to_path_buf());
    let mut state = new_state();
    state.flags = values(&[("e2e", false), ("docs", true)]);
    manager.create(&state).unwrap();
    let loaded = manager.load(&state.id).unwrap();
    assert_eq!(loaded.flags, values(&[("docs", true), ("e2e", false)]));
}
