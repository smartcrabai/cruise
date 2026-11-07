//! Contracts for load-time workflow call return routing.
use std::path::{Path, PathBuf};

use cruise::config::{WorkflowConfig, validate_config};
use cruise::error::CruiseError;
use cruise::workflow_call::resolve_workflow_calls_from_path_with_defaults;
use cruise::workflow_defaults::WorkflowDefaults;
use tempfile::TempDir;

fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    path
}

fn load(path: &Path) -> cruise::error::Result<WorkflowConfig> {
    let config =
        resolve_workflow_calls_from_path_with_defaults(path, &WorkflowDefaults::default())?;
    validate_config(&config)?;
    Ok(config)
}

fn fixture(callee: &str, call_fields: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    write(dir.path(), "callee.yaml", callee);
    let root = write(
        dir.path(),
        "caller.yaml",
        &format!(
            "model: fixture-model\nplan_model: fixture-model\ncommand: [unused-agent]\ncreate_pr: false\nsteps:\n  gate:\n    workflow_call: ./callee.yaml\n{call_fields}  fix:\n    command: 'true'\n  publish:\n    command: 'true'\n"
        ),
    );
    (dir, root)
}

fn invalid(path: &Path, fragments: &[&str]) {
    match load(path) {
        Err(CruiseError::InvalidStepConfig(message)) => {
            for fragment in fragments {
                assert!(
                    message.contains(fragment),
                    "missing {fragment:?}: {message}"
                );
            }
        }
        other => panic!("expected InvalidStepConfig, got {other:?}"),
    }
}

#[test]
fn option_routes_are_parent_scoped_and_metadata_is_removed() {
    let (_dir, root) = fixture(
        "returns: [approved, needs_fix]\nsteps:\n  review:\n    command: 'true'\n  decide:\n    option:\n      - {selector: Approve, return: approved}\n      - {selector: Fix, return: needs_fix}\n",
        "    on_return: {approved: publish, needs_fix: fix}\n",
    );
    let config = load(&root).unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    assert_eq!(
        config.steps.keys().map(String::as_str).collect::<Vec<_>>(),
        ["gate/review", "gate/decide", "fix", "publish"]
    );
    let json = serde_json::to_value(&config).unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    assert!(json.get("returns").is_none());
    assert!(config.steps["gate/review"].next.is_none());
    let options = json["steps"]["gate/decide"]["option"]
        .as_array()
        .unwrap_or_else(|| panic!("expected options array"));
    assert_eq!(options[0]["next"], "publish");
    assert_eq!(options[1]["next"], "fix");
    assert!(options.iter().all(|o| o.get("return").is_none()));
    assert!(
        json["steps"]
            .as_object()
            .unwrap_or_else(|| panic!("expected steps object"))
            .values()
            .all(|s| s.get("return").is_none() && s.get("on_return").is_none())
    );
}

#[test]
fn step_return_and_normal_fallthrough_preserve_distinct_routes() {
    let (_dir, root) = fixture(
        "returns: [ok]\nsteps:\n  finish: {command: 'true', return: ok}\n",
        "    on_return: {ok: publish}\n",
    );
    assert_eq!(
        load(&root)
            .unwrap_or_else(|e| panic!("unexpected error: {e:?}"))
            .steps["gate/finish"]
            .next
            .as_deref(),
        Some("publish")
    );
    let (_dir, root) = fixture(
        "returns: [ok]\nsteps:\n  early: {command: 'true', return: ok}\n  last: {command: 'true'}\n",
        "    on_return: {ok: fix}\n    next: publish\n",
    );
    let config = load(&root).unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    assert_eq!(config.steps["gate/early"].next.as_deref(), Some("fix"));
    assert_eq!(config.steps["gate/last"].next.as_deref(), Some("publish"));
}

#[test]
fn nested_returns_are_mapped_at_each_scope() {
    let (dir, root) = fixture(
        "returns: [done]\nsteps:\n  inner:\n    workflow_call: ./leaf.yaml\n    on_return: {ok: finish}\n  finish: {command: 'true', return: done}\n",
        "    on_return: {done: publish}\n",
    );
    write(
        dir.path(),
        "leaf.yaml",
        "returns: [ok]\nsteps:\n  finish: {command: 'true', return: ok}\n",
    );
    let config = load(&root).unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    assert_eq!(
        config.steps["gate/inner/finish"].next.as_deref(),
        Some("gate/finish")
    );
    assert_eq!(config.steps["gate/finish"].next.as_deref(), Some("publish"));
    let wrong = write(
        dir.path(),
        "wrong.yaml",
        "model: fixture-model\nplan_model: fixture-model\ncommand: [unused-agent]\nsteps:\n  gate:\n    workflow_call: ./callee.yaml\n    on_return: {ok: publish}\n  publish: {command: 'true'}\n",
    );
    invalid(&wrong, &["undeclared return label", "ok"]);
}

#[test]
fn declaration_and_mapping_validation() {
    let cases = [
        (
            "steps:\n  finish: {command: 'true', return: foo}\n",
            "    on_return: {foo: fix}\n",
            "undeclared return label",
            "foo",
        ),
        (
            "returns: [approved, needs_fix]\nsteps:\n  finish: {command: 'true', return: approved}\n",
            "    on_return: {approved: fix, needs_fix: publish}\n",
            "declared but never returned",
            "needs_fix",
        ),
        (
            "returns: [a, a]\nsteps:\n  finish: {command: 'true', return: a}\n",
            "    on_return: {a: fix}\n",
            "invalid or duplicate return label",
            "a",
        ),
        (
            "returns: ['bad label']\nsteps:\n  finish: {command: 'true', return: 'bad label'}\n",
            "    on_return: {'bad label': fix}\n",
            "invalid or duplicate return label",
            "bad label",
        ),
        (
            "returns: [approved]\nsteps:\n  finish: {command: 'true', return: approved}\n",
            "",
            "unmapped return label",
            "approved",
        ),
        (
            "returns: [approved]\nsteps:\n  finish: {command: 'true', return: approved}\n",
            "    on_return: {approved: fix, extra: publish}\n",
            "undeclared return label",
            "extra",
        ),
        (
            "returns: [approved]\nsteps:\n  finish: {command: 'true', return: approved}\n",
            "    on_return: {approved: nowhere}\n",
            "on_return target",
            "nowhere",
        ),
    ];
    for (callee, fields, first, second) in cases {
        let (_dir, root) = fixture(callee, fields);
        invalid(&root, &[first, second]);
    }
}

#[test]
fn conflicting_and_misplaced_returns_are_rejected() {
    let cases = [
        (
            "returns: [ok]\nsteps:\n  finish: {command: 'true', return: ok, next: finish}\n",
            "    on_return: {ok: fix}\n",
            "both next and return",
        ),
        (
            "returns: [ok]\nsteps:\n  finish:\n    return: ok\n    option:\n      - {selector: Yes, next: finish}\n",
            "    on_return: {ok: fix}\n",
            "return must be set on option items",
        ),
        (
            "returns: [ok]\nsteps:\n  finish: {command: 'true', return: ok}\n",
            "    on_return: {ok: fix}\n    next: publish\n",
            "call-site next is unreachable",
        ),
    ];
    for (callee, fields, message) in cases {
        let (_dir, root) = fixture(callee, fields);
        invalid(&root, &[message]);
    }
    let dir = TempDir::new().unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    for (name, yaml) in [
        (
            "returns.yaml",
            "returns: [ok]\nsteps:\n  run: {command: 'true'}\n",
        ),
        (
            "return.yaml",
            "steps:\n  run: {command: 'true', return: ok}\n",
        ),
    ] {
        let root = write(dir.path(), name, yaml);
        invalid(
            &root,
            &["return routes are only valid in a workflow_call target"],
        );
    }
}

#[test]
fn loops_with_and_without_normal_exit_route_independently() {
    let (_dir, root) = fixture(
        "returns: [ok]\nsteps:\n  decide:\n    option:\n      - {selector: Retry, next: decide}\n      - {selector: Done, return: ok}\n  normal: {command: 'true'}\n",
        "    on_return: {ok: fix}\n    next: publish\n",
    );
    let config = load(&root).unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    let json = serde_json::to_value(&config).unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    assert_eq!(
        json["steps"]["gate/decide"]["option"][0]["next"],
        "gate/decide"
    );
    assert_eq!(json["steps"]["gate/decide"]["option"][1]["next"], "fix");
    assert_eq!(config.steps["gate/normal"].next.as_deref(), Some("publish"));
    let compiled =
        cruise::workflow::compile(config).unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    let graph = cruise::graph::build_graph(&compiled, 3)
        .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    cruise::graph::validation::validate_workflow(&compiled, &graph, &graph.start, &[])
        .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));

    let (_dir, root) = fixture(
        "returns: [ok]\nsteps:\n  loop: {command: 'true', next: loop}\n  finish: {command: 'true', return: ok}\n",
        "    on_return: {ok: fix}\n    next: publish\n",
    );
    invalid(&root, &["call-site next is unreachable"]);

    let (_dir, root) = fixture(
        "returns: [ok]\nsteps:\n  loop: {command: 'true', next: loop}\n  finish: {command: 'true', return: ok}\n",
        "    on_return: {ok: fix}\n",
    );
    let compiled = cruise::workflow::compile(
        load(&root).unwrap_or_else(|e| panic!("unexpected error: {e:?}")),
    )
    .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    let graph = cruise::graph::build_graph(&compiled, 3)
        .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    assert!(
        cruise::graph::validation::validate_workflow(&compiled, &graph, &graph.start, &[]).is_err()
    );
}

#[test]
fn additional_return_placement_and_conflicts_are_rejected() {
    for (callee, fields, message) in [
        (
            "returns: [ok]\nsteps:\n  decide:\n    option:\n      - {selector: Done, next: decide, return: ok}\n",
            "    on_return: {ok: fix}\n",
            "both next and return",
        ),
        (
            "returns: [ok]\nsteps:\n  finish:\n    command: 'true'\n    return: ok\n    if: {file-changed: '*.rs'}\n",
            "    on_return: {ok: fix}\n",
            "return-bearing step cannot declare if.file-changed",
        ),
        (
            "returns: [ok]\ngroups:\n  g:\n    steps:\n      finish: {command: 'true', return: ok}\nsteps:\n  run: {command: 'true'}\n",
            "    on_return: {ok: fix}\n",
            "return is only valid on top-level callee steps",
        ),
        (
            "returns: [ok]\nsteps:\n  run: {command: 'true'}\nafter-pr:\n  finish: {command: 'true', return: ok}\n",
            "    on_return: {ok: fix}\n",
            "return is only valid on top-level callee steps",
        ),
        (
            "steps:\n  finish: {command: 'true'}\n",
            "    on_return: {ok: fix}\n",
            "undeclared return label",
        ),
    ] {
        let (_dir, root) = fixture(callee, fields);
        invalid(&root, &[message]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn return_and_skips_follow_the_executed_path() {
    use cruise::engine::{ExecutionContext, execute_steps_with_graph};
    use cruise::file_tracker::FileTracker;
    use cruise::graph::build_graph;
    use cruise::option_handler::OptionHandler;
    use cruise::variable::VariableStore;
    use cruise::workflow::compile;

    struct NoOptions;
    impl OptionHandler for NoOptions {
        fn select_option(
            &self,
            _: &[cruise::step::OptionChoice],
            _: Option<&str>,
        ) -> cruise::error::Result<cruise::step::option::OptionResult> {
            panic!("unexpected option")
        }
    }

    for (step_skip, call_skip, expected) in [
        (false, false, "return\npublish\n"),
        (true, false, "normal\nfix\npublish\n"),
        (false, true, "normal\nfix\npublish\n"),
    ] {
        let callee = format!(
            "returns: [ok]\nsteps:\n  finish: {{command: 'echo return >> visits', return: ok, skip: {step_skip}}}\n  normal: {{command: 'echo normal >> visits'}}\n"
        );
        let fields = format!("    on_return: {{ok: publish}}\n    skip: {call_skip}\n");
        let (dir, root) = fixture(&callee, &fields);
        let root_text = std::fs::read_to_string(&root)
            .unwrap_or_else(|e| panic!("unexpected error: {e:?}"))
            .replace("command: 'true'", "command: 'echo fix >> visits'")
            .replace(
                "  publish:\n    command: 'echo fix >> visits'",
                "  publish:\n    command: 'echo publish >> visits'",
            );
        std::fs::write(&root, root_text).unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
        let compiled = compile(load(&root).unwrap_or_else(|e| panic!("unexpected error: {e:?}")))
            .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
        let mut graph =
            build_graph(&compiled, 3).unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
        let ctx = ExecutionContext {
            compiled: &compiled,
            max_retries: 3,
            rate_limit_retries: 0,
            on_step_start: &|_| Ok(()),
            cancel_token: None,
            option_handler: &NoOptions,
            config_reloader: None,
            working_dir: Some(dir.path()),
            skipped_steps: &[],
            flags: &cruise::flags::NO_FLAGS,
            on_step_log: None,
            instructions: None,
            usage_session_id: None,
        };
        execute_steps_with_graph(
            &ctx,
            &mut VariableStore::new(String::new()),
            &mut FileTracker::with_root(dir.path().into()),
            &mut graph,
            &|_, _| Ok(()),
        )
        .await
        .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("visits"))
                .unwrap_or_else(|e| panic!("unexpected error: {e:?}")),
            expected
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn option_return_follows_the_executed_path() {
    use cruise::engine::{ExecutionContext, execute_steps_with_graph};
    use cruise::file_tracker::FileTracker;
    use cruise::graph::build_graph;
    use cruise::option_handler::OptionHandler;
    use cruise::variable::VariableStore;
    use cruise::workflow::compile;

    struct ChooseFirst;
    impl OptionHandler for ChooseFirst {
        fn select_option(
            &self,
            choices: &[cruise::step::OptionChoice],
            _: Option<&str>,
        ) -> cruise::error::Result<cruise::step::option::OptionResult> {
            match &choices[0] {
                cruise::step::OptionChoice::Selector { next, .. } => {
                    Ok(cruise::step::option::OptionResult {
                        next_step: next.clone(),
                        text_input: None,
                    })
                }
                cruise::step::OptionChoice::TextInput { .. } => panic!("expected selector"),
            }
        }
    }
    let (dir, root) = fixture(
        "returns: [ok]\nsteps:\n  decide:\n    option:\n      - {selector: Done, return: ok}\n",
        "    on_return: {ok: publish}\n",
    );
    let root_text = std::fs::read_to_string(&root)
        .unwrap_or_else(|e| panic!("unexpected error: {e:?}"))
        .replace("command: 'true'", "command: 'echo fix >> visits'")
        .replace(
            "  publish:\n    command: 'echo fix >> visits'",
            "  publish:\n    command: 'echo publish >> visits'",
        );
    std::fs::write(&root, root_text).unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    let compiled = compile(load(&root).unwrap_or_else(|e| panic!("unexpected error: {e:?}")))
        .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    let mut graph = build_graph(&compiled, 3).unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    let ctx = ExecutionContext {
        compiled: &compiled,
        max_retries: 3,
        rate_limit_retries: 0,
        on_step_start: &|_| Ok(()),
        cancel_token: None,
        option_handler: &ChooseFirst,
        config_reloader: None,
        working_dir: Some(dir.path()),
        skipped_steps: &[],
        flags: &cruise::flags::NO_FLAGS,
        on_step_log: None,
        instructions: None,
        usage_session_id: None,
    };
    execute_steps_with_graph(
        &ctx,
        &mut VariableStore::new(String::new()),
        &mut FileTracker::with_root(dir.path().into()),
        &mut graph,
        &|_, _| Ok(()),
    )
    .await
    .unwrap_or_else(|e| panic!("unexpected error: {e:?}"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("visits"))
            .unwrap_or_else(|e| panic!("unexpected error: {e:?}")),
        "publish\n"
    );
}
