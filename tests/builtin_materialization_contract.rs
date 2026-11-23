//! Observable built-in materialization contracts, with host settings isolated in subprocesses.
#![expect(clippy::unwrap_used, reason = "isolated contract fixtures")]

use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::sync::Mutex;

#[cfg(unix)]
use cruise::config::StringOrVec;
#[cfg(unix)]
use cruise::engine::{ExecutionContext, execute_steps_with_graph};
#[cfg(unix)]
use cruise::file_tracker::FileTracker;
#[cfg(unix)]
use cruise::graph::ExecutionGraph;
#[cfg(unix)]
use cruise::option_handler::OptionHandler;
#[cfg(unix)]
use cruise::variable::VariableStore;

use cruise::builtin_workflows::BUILTIN_WORKFLOWS;
use cruise::config::{
    FailAction, PermissionMode, ReviewerScope, SkipCondition, WorkflowConfig, validate_config,
};
use cruise::error::CruiseError;
use cruise::flags::{self, FlagValues};
use cruise::graph::{TransitionReason, build_graph};
use cruise::resolver::{ConfigSource, resolve_workflow_config};
use cruise::workflow::{CompiledWorkflow, compile};
use cruise::workflow_call::resolve_workflow_calls_from_path_with_defaults;
use cruise::workflow_cmd::eject_builtin;
use cruise::workflow_defaults::WorkflowDefaults;

const WORKER_PHASE: &str = "BUILTIN_MATERIALIZATION_TEST_PHASE";
const AFTER_PR_IDS: [&str; 8] = [
    "follow-up/pr-ready",
    "follow-up/sync-base",
    "follow-up/resolve-conflict",
    "follow-up/push-after-sync",
    "follow-up/finish-ci-fix",
    "follow-up/wait-ci",
    "follow-up/fix-ci-error",
    "squash-merge",
];
const MAX_RETRIES: usize = 3;

/// Built-ins with `force_exec: true` run only through exec and expose no after-PR graph.
const FORCE_EXEC_BUILTINS: [&str; 4] = ["refactor", "add-tests", "add-formal-specs", "security"];

fn is_force_exec_builtin(name: &str) -> bool {
    FORCE_EXEC_BUILTINS.contains(&name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The native merge step routes confirmed behind/conflicting outcomes back to
/// `follow-up/sync-base` over a backward edge bounded by `max_retries`.
fn assert_native_merge_resyncs_to_sync_base(compiled: &CompiledWorkflow) {
    let native_merge_steps: Vec<_> = compiled
        .after_pr
        .iter()
        .filter(|(_, step)| step.squash_merge == Some(true))
        .collect();
    assert!(!native_merge_steps.is_empty());
    let graph = build_graph(&compiled.to_after_pr_compiled(), MAX_RETRIES).unwrap();
    assert_eq!(graph.max_retries, MAX_RETRIES);
    for (name, step) in native_merge_steps {
        let target = match step
            .if_condition
            .as_ref()
            .and_then(|condition| condition.fail.as_ref())
        {
            Some(FailAction::Goto(target)) => target,
            other => panic!("{name} must route merge failures to sync-base, got {other:?}"),
        };
        assert_eq!(target, "follow-up/sync-base", "{name}");
        let node = graph
            .nodes
            .get(name)
            .unwrap_or_else(|| panic!("missing graph node for {name}"));
        let edge = node
            .successors
            .iter()
            .find(|edge| {
                matches!(
                    &edge.reason,
                    TransitionReason::IfFailGoto { target: edge_target }
                        if edge_target == target
                )
            })
            .unwrap_or_else(|| panic!("missing if.fail edge from {name} to {target}"));
        assert_eq!(edge.target.as_deref(), Some(target.as_str()), "{name}");
        assert!(
            graph.nodes.get_index_of(target).unwrap() < graph.nodes.get_index_of(name).unwrap(),
            "{name} -> {target} must be a backward edge governed by max_retries"
        );
    }
}

fn assert_after_pr_contract(config: WorkflowConfig) {
    validate_config(&config).unwrap();
    let compiled = compile(config).unwrap();
    assert_native_merge_resyncs_to_sync_base(&compiled);
    let ids: Vec<_> = compiled.after_pr.keys().map(String::as_str).collect();
    assert_eq!(ids, AFTER_PR_IDS);
    assert!(
        compiled
            .steps
            .values()
            .all(|step| step.squash_merge.is_none())
    );
    for (index, step) in compiled.after_pr.values().enumerate() {
        let expected: &[&str] = if index == 7 {
            &["merge"]
        } else {
            &["after_pr", "merge"]
        };
        let actual: Vec<_> = step
            .flag
            .as_ref()
            .unwrap()
            .as_slice()
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(actual, expected);
        assert_eq!(
            step.squash_merge,
            if index == 7 { Some(true) } else { None }
        );
    }
    let wait = &compiled.after_pr["follow-up/wait-ci"];
    assert_eq!(wait.accepted_exit_codes.as_deref(), Some(&[0, 75][..]));
    let fix = &compiled.after_pr["follow-up/fix-ci-error"];
    assert_eq!(fix.next.as_deref(), Some("follow-up/finish-ci-fix"));
    assert_eq!(fix.output_file.as_deref(), Some("ci-followup/result.json"));
    let finish = &compiled.after_pr["follow-up/finish-ci-fix"];
    assert_eq!(finish.next.as_deref(), Some("follow-up/wait-ci"));
    assert_eq!(finish.accepted_exit_codes.as_deref(), Some(&[0][..]));
    assert_eq!(
        compiled.after_pr["follow-up/sync-base"]
            .accepted_exit_codes
            .as_deref(),
        Some(&[0, 75][..])
    );
    assert_eq!(
        compiled.after_pr["follow-up/push-after-sync"]
            .next
            .as_deref(),
        Some("follow-up/wait-ci")
    );
    let enabled_ids = |values: FlagValues| {
        let skipped = flags::preflight_skips(&compiled, &values, &[]);
        compiled
            .after_pr
            .keys()
            .filter(|id| !skipped.contains(id))
            .map(String::as_str)
            .collect::<Vec<_>>()
    };
    assert_eq!(enabled_ids(FlagValues::new()), [] as [&str; 0]);
    assert_eq!(
        enabled_ids([("after_pr".to_owned(), true)].into()),
        AFTER_PR_IDS[..7]
    );
    assert_eq!(
        enabled_ids([("merge".to_owned(), true)].into()),
        AFTER_PR_IDS
    );
    assert_eq!(
        enabled_ids([("after_pr".to_owned(), true), ("merge".to_owned(), true)].into()),
        AFTER_PR_IDS
    );
}

fn assert_pr_review_contract(config: WorkflowConfig) {
    validate_config(&config).unwrap();
    let compiled = compile(config).unwrap();
    assert_eq!(
        compiled
            .after_pr
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["address-review"]
    );
    let step = &compiled.after_pr["address-review"];
    let review = step.github_review.as_ref().unwrap();
    assert_eq!(review.reviewers, ReviewerScope::All);
    assert_eq!(review.max_iterations, 3);
    assert_eq!(step.timeout.as_deref(), Some("60m"));
    assert!(step.allow_commit);
    assert!(step.require_bubblewrap);
    assert_eq!(step.output_file.as_deref(), Some("review-report.json"));
    assert!(step.squash_merge.is_none());
}

fn assert_force_exec_without_after_pr(name: &str, config: WorkflowConfig) {
    assert!(config.force_exec(), "{name} must stay force_exec");
    if name == "refactor" {
        assert!(
            config.flags.contains_key("formal_methods"),
            "refactor lost formal_methods"
        );
    }
    for flag in ["after_pr", "merge"] {
        let values = [(flag.to_owned(), true)]
            .into_iter()
            .collect::<FlagValues>();
        let error = flags::validate_names(&config, &values).unwrap_err();
        assert!(
            error.to_string().contains(flag),
            "{name} rejected {flag} with an unexpected error: {error}"
        );
    }
    validate_config(&config).unwrap();
    let compiled = compile(config).unwrap();
    assert!(
        compiled.after_pr.is_empty(),
        "{name} still declares after-PR steps"
    );
}

fn eject_with_host_settings(root: &Path, project: &Path, config_dir: &Path) {
    // These settings must not become part of a portable workflow artifact.
    write(&config_dir.join("config.yaml"), "model: app/host-model\n");
    write(
        &config_dir.join("default.yaml"),
        "model: user/host-model\ninteractive_planning: false\nforce_exec: true\nenv:\n  HOST_ONLY: user-host\n",
    );
    write(
        &project.join(".cruise/default.yaml"),
        "model: project/host-model\nenv:\n  HOST_ONLY: project-host\n",
    );
    for builtin in BUILTIN_WORKFLOWS {
        eject_builtin(builtin.name, &root.join("export").join(builtin.name)).unwrap();
    }
    // Remove every source of machine settings before a fresh process loads artifacts.
    std::fs::remove_dir_all(config_dir).unwrap();
    std::fs::remove_file(project.join(".cruise/default.yaml")).unwrap();
}

fn artifact(root: &Path, name: &str) -> PathBuf {
    root.join("export").join(name).join(format!("{name}.yaml"))
}

fn verify_local_builtin_resources(root: &Path) {
    let local = root.join("local-builtin");
    write(
        &local.join("_shared/after-pr.yaml"),
        "inputs: [resolve_conflict_prompt, fix_ci_error_scope]\nsteps:\n  local-only:\n    command: 'true'\n",
    );
    for builtin in BUILTIN_WORKFLOWS {
        let path = local.join(format!("{}.yaml", builtin.name));
        write(&path, builtin.yaml);
        let config =
            resolve_workflow_calls_from_path_with_defaults(path, &WorkflowDefaults::default())
                .unwrap();
        if builtin.name == "pr-review-fix" {
            assert_pr_review_contract(config);
        } else if is_force_exec_builtin(builtin.name) {
            // The local after-PR resource must not reach a built-in without an after-PR root.
            assert_force_exec_without_after_pr(builtin.name, config);
        } else {
            validate_config(&config).unwrap();
            assert!(config.after_pr.contains_key("follow-up/local-only"));
            assert!(!config.after_pr.contains_key("follow-up/wait-ci"));
            assert_eq!(config.after_pr.last().unwrap().0, "squash-merge");
        }
    }
}

fn verify_builtin_repair_policy(project: &Path) {
    for builtin in BUILTIN_WORKFLOWS {
        let config = resolve_workflow_config(
            builtin.yaml,
            &ConfigSource::NamedBuiltin(builtin.name),
            project,
            Some(project),
        )
        .unwrap();
        assert_eq!(config.create_pr, builtin.name != "research");
        if builtin.name == "pr-review-fix" {
            assert_pr_review_contract(config);
            continue;
        }
        if is_force_exec_builtin(builtin.name) {
            assert!(
                config.after_pr.is_empty(),
                "{} kept repair steps",
                builtin.name
            );
            continue;
        }
        for id in ["follow-up/resolve-conflict", "follow-up/fix-ci-error"] {
            let step = config
                .after_pr
                .get(id)
                .unwrap_or_else(|| panic!("{} did not materialize {id}", builtin.name));
            assert!(
                !step.allow_commit,
                "{} {id} must not commit from the prompt",
                builtin.name
            );
            assert_eq!(step.permission, Some(PermissionMode::Full));
            assert!(step.require_bubblewrap);
            assert!(
                matches!(&step.skip, Some(SkipCondition::Variable(name)) if name == "prev.success")
            );
        }
        assert_eq!(
            config.after_pr["follow-up/fix-ci-error"]
                .output_file
                .as_deref(),
            Some("ci-followup/result.json")
        );
        assert!(
            config.after_pr["follow-up/fix-ci-error"]
                .prompt
                .as_deref()
                .unwrap()
                .contains("{file:ci-followup/evidence.md}"),
            "{} repair prompt lost the captured cross-run CI evidence",
            builtin.name
        );
        let repair_prompt = config.after_pr["follow-up/fix-ci-error"]
            .prompt
            .as_deref()
            .unwrap();
        assert!(
            repair_prompt.contains("Do not stage, commit, push, call the forge"),
            "{} repair prompt must leave publication to the host",
            builtin.name
        );
        let finish_command = config.after_pr["follow-up/finish-ci-fix"]
            .command
            .as_ref()
            .unwrap()
            .as_slice()
            .join("\n");
        assert_eq!(finish_command, "{cruise.bin} internal finish-ci-followup");
        assert!(config.after_pr.contains_key("follow-up/finish-ci-fix"));
        assert_eq!(
            config.after_pr["follow-up/wait-ci"]
                .accepted_exit_codes
                .as_deref(),
            Some(&[0, 75][..])
        );
        assert_eq!(
            config.after_pr["follow-up/wait-ci"].timeout.as_deref(),
            Some("60m")
        );
        assert_eq!(
            config.after_pr["squash-merge"].timeout.as_deref(),
            Some("10m")
        );
    }
}

fn assert_builtin_e2e_contract(project: &Path) {
    for name in ["tdd", "refactor"] {
        let builtin = BUILTIN_WORKFLOWS
            .iter()
            .find(|builtin| builtin.name == name)
            .unwrap_or_else(|| panic!("missing builtin workflow {name}"));
        let config = resolve_workflow_config(
            builtin.yaml,
            &ConfigSource::NamedBuiltin(name),
            project,
            Some(project),
        )
        .unwrap();

        assert!(config.flags.contains_key("e2e"), "{name} must declare e2e");
        // Rejection by name validation also proves the legacy flag is no longer declared.
        for legacy in ["browser_e2e", "app_e2e"] {
            let values = [(legacy.to_string(), true)]
                .into_iter()
                .collect::<FlagValues>();
            let error = flags::validate_names(&config, &values).unwrap_err();
            assert!(
                error.to_string().contains(legacy),
                "{name} accepted legacy flag {legacy}"
            );
        }

        let compiled = compile(config).unwrap();
        let enabled = [("e2e".to_string(), true)]
            .into_iter()
            .collect::<FlagValues>();
        let off = flags::preflight_skips(&compiled, &FlagValues::new(), &[]);
        let on = flags::preflight_skips(&compiled, &enabled, &[]);
        let expected_e2e_steps: &[&str] = if name == "tdd" {
            &["e2e"]
        } else {
            &["prepare-e2e", "e2e-final-check"]
        };
        let actual_e2e_steps = compiled
            .steps
            .iter()
            .filter_map(|(step, config)| {
                config
                    .flag
                    .as_ref()
                    .is_some_and(|flag| flag.as_slice().iter().any(|name| name == "e2e"))
                    .then_some(step.as_str())
            })
            .collect::<Vec<_>>();
        assert_eq!(actual_e2e_steps, expected_e2e_steps, "{name}");
        for step in expected_e2e_steps {
            assert!(
                off.contains(&step.to_string()),
                "{name} did not gate {step} off"
            );
            assert!(!on.contains(&step.to_string()), "{name} gated {step} on");
        }

        let step_ids = compiled
            .steps
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let review_prefix = if name == "tdd" {
            "verify-review-pass/"
        } else {
            "verify-refactor-pass/"
        };
        let review_positions = step_ids
            .iter()
            .enumerate()
            .filter_map(|(index, step)| step.starts_with(review_prefix).then_some(index))
            .collect::<Vec<_>>();
        assert!(
            !review_positions.is_empty(),
            "{name} review phase is missing"
        );
        let position = |id: &str| {
            step_ids
                .iter()
                .position(|step| *step == id)
                .unwrap_or_else(|| panic!("{name} {id} step is missing"))
        };
        let first_e2e = position(expected_e2e_steps[0]);
        let last_e2e = position(expected_e2e_steps[expected_e2e_steps.len() - 1]);
        let last_review = review_positions[review_positions.len() - 1];
        assert!(
            first_e2e < review_positions[0],
            "{name} E2E must precede review"
        );
        // refactor runs its final E2E check after review; tdd has no post-review E2E.
        assert_eq!(
            last_review < last_e2e,
            name == "refactor",
            "{name} E2E order"
        );
        for &index in &review_positions {
            let id = step_ids[index].to_string();
            assert!(
                !off.contains(&id) && !on.contains(&id),
                "{name} review step {id} was gated"
            );
        }
    }
}

fn verify_ejected_builtins(root: &Path, project: &Path, config_dir: &Path) {
    // A cwd-local dependency is deliberately invalid. Embedded resolution must ignore it.
    write(&project.join("_shared/after-pr.yaml"), "not: [valid\n");
    for builtin in BUILTIN_WORKFLOWS {
        let path = artifact(root, builtin.name);
        let config =
            resolve_workflow_calls_from_path_with_defaults(&path, &WorkflowDefaults::default())
                .unwrap();
        assert!(
            config.model.is_none(),
            "{} captured a host model",
            builtin.name
        );
        assert!(!config.env.contains_key("HOST_ONLY"));
        assert!(
            config.interactive_planning,
            "{} captured host defaults",
            builtin.name
        );
        assert_eq!(config.create_pr, builtin.name != "research");
        validate_config(&config).unwrap();
    }
    // User defaults apply to omitted fields, project defaults win over user defaults,
    // and explicit root values (notably research's no-PR) remain authoritative.
    for project_override in [false, true] {
        write(
            &config_dir.join("default.yaml"),
            "model: user/portable\ninteractive_planning: false\nforce_exec: true\nenv:\n  LAYER: user\n  USER_ONLY: retained\n",
        );
        if project_override {
            write(
                &project.join(".cruise/default.yaml"),
                "model: project/portable\ninteractive_planning: true\nforce_exec: false\nenv:\n  LAYER: project\n",
            );
        }
        for builtin in BUILTIN_WORKFLOWS {
            let embedded = resolve_workflow_config(
                builtin.yaml,
                &ConfigSource::NamedBuiltin(builtin.name),
                project,
                Some(project),
            )
            .unwrap();
            let path = artifact(root, builtin.name);
            let defaults = WorkflowDefaults::load(Some(project)).unwrap();
            let ejected = resolve_workflow_calls_from_path_with_defaults(&path, &defaults).unwrap();
            assert_eq!(ejected.interactive_planning, project_override);
            assert_eq!(
                ejected.env.get("LAYER").map(String::as_str),
                Some(if project_override { "project" } else { "user" })
            );
            assert_eq!(
                ejected.env.get("USER_ONLY").map(String::as_str),
                Some("retained")
            );
            assert_eq!(
                ejected.model.as_ref().and_then(|model| model.primary()),
                Some(if project_override {
                    "project/portable"
                } else {
                    "user/portable"
                })
            );
            // Compare parsed execution semantics, not YAML bytes or serializer spelling.
            assert_eq!(
                serde_yaml::to_value(&ejected).unwrap(),
                serde_yaml::to_value(&embedded).unwrap(),
                "{} changed defaults precedence",
                builtin.name
            );
            assert_eq!(
                ejected.force_exec,
                !project_override || !matches!(builtin.name, "tdd" | "research" | "pr-review-fix"),
                "{} force_exec changed defaults precedence",
                builtin.name
            );
            validate_config(&embedded).unwrap();
        }
    }
}

fn worker(root: &Path, phase: &str) {
    let project = root.join("project");
    let config_dir = root.join("config/cruise");
    match phase {
        "eject" => eject_with_host_settings(root, &project, &config_dir),
        "local" => verify_local_builtin_resources(root),
        "repair-policy" => verify_builtin_repair_policy(&project),
        "e2e-contract" => assert_builtin_e2e_contract(&project),
        "missing-resource" => {
            write(
                &project.join("_shared/local-only.yaml"),
                "steps:\n  valid-local-step:\n    command: 'true'\n",
            );
            let error = resolve_workflow_config(
                "steps:\n  missing:\n    workflow_call: _shared/local-only.yaml\n",
                &ConfigSource::NamedBuiltin("tdd"),
                &project,
                Some(&project),
            )
            .unwrap_err();
            assert!(
                matches!(error, CruiseError::InvalidStepConfig(ref message)
                    if message.contains("missing embedded workflow resource")
                        && message.contains("_shared/local-only.yaml")),
                "embedded lookup must reject the valid local fallback: {error}"
            );
        }
        "embedded-cycle" => {
            let error = resolve_workflow_config(
                "steps:\n  recurse:\n    workflow_call: tdd.yaml\n",
                &ConfigSource::NamedBuiltin("tdd"),
                &project,
                Some(&project),
            )
            .unwrap_err();
            assert!(
                matches!(error, CruiseError::InvalidStepConfig(ref message)
                    if message.contains("cycle")
                        && message.contains("embedded resource 'tdd.yaml'")),
                "the active embedded root must reach cycle detection: {error}"
            );
        }
        "contracts" => {
            write(&project.join("_shared/after-pr.yaml"), "not: [valid\n");
            for builtin in BUILTIN_WORKFLOWS {
                let config = resolve_workflow_config(
                    builtin.yaml,
                    &ConfigSource::NamedBuiltin(builtin.name),
                    &project,
                    Some(&project),
                )
                .unwrap();
                if builtin.name == "pr-review-fix" {
                    assert_pr_review_contract(config);
                } else if is_force_exec_builtin(builtin.name) {
                    assert_force_exec_without_after_pr(builtin.name, config);
                } else {
                    assert_after_pr_contract(config);
                }
            }
        }
        "verify" => verify_ejected_builtins(root, &project, &config_dir),
        #[cfg(unix)]
        "after-pr-behavior" => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(verify_builtin_after_pr_behavior(&project)),
        _ => panic!("unknown worker phase"),
    }
}

#[test]
fn materialization_worker() {
    let Ok(phase) = std::env::var(WORKER_PHASE) else {
        return;
    };
    worker(std::env::current_dir().unwrap().parent().unwrap(), &phase);
}

fn run_isolated(phases: &[&str]) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("project")).unwrap();
    for &phase in phases {
        let mut command = Command::new(std::env::current_exe().unwrap());
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(key);
            }
        }
        command
            .args(["--exact", "materialization_worker", "--nocapture"])
            .current_dir(root.path().join("project"))
            .env(WORKER_PHASE, phase)
            .env("HOME", root.path().join("home"))
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_DATA_HOME", root.path().join("data"))
            .env("XDG_STATE_HOME", root.path().join("state"))
            .env_remove("HERDR_ENV");
        if phase == "eject" {
            command.env("CRUISE_MODEL", "env/host-model");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{phase} failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn builtin_after_pr_is_scoped_flagged_and_ignores_cwd_dependencies() {
    run_isolated(&["contracts"]);
}

#[test]
fn builtin_e2e_phases_are_gated_and_ordered_around_review() {
    run_isolated(&["e2e-contract"]);
}

#[test]
fn ejected_builtins_load_independently_without_capturing_host_defaults() {
    run_isolated(&["eject", "verify"]);
}

#[test]
fn locally_loaded_builtin_yaml_uses_local_dependencies_not_embedded_assets() {
    run_isolated(&["local"]);
}

#[test]
fn builtin_repair_steps_preserve_commit_permissions_skip_and_timeouts() {
    run_isolated(&["repair-policy"]);
}

#[test]
fn missing_embedded_resource_does_not_fall_back_to_valid_local_file() {
    run_isolated(&["missing-resource"]);
}

#[test]
fn builtin_call_back_to_active_embedded_root_is_a_cycle() {
    run_isolated(&["embedded-cycle"]);
}
#[cfg(unix)]
struct NoOpOptionHandler;

#[cfg(unix)]
impl OptionHandler for NoOpOptionHandler {
    fn select_option(
        &self,
        _: &[cruise::step::OptionChoice],
        _: Option<&str>,
    ) -> cruise::error::Result<cruise::step::option::OptionResult> {
        panic!("unexpected option interaction");
    }
}

#[cfg(unix)]
fn init_after_pr_repo(project: &Path) {
    let commands: &[&[&str]] = &[
        &["init", "-q", "-b", "main"],
        &["config", "user.name", "Cruise Test"],
        &["config", "user.email", "cruise@example.invalid"],
        &["config", "commit.gpgsign", "false"],
        &["config", "core.hooksPath", "/dev/null"],
        &["commit", "--allow-empty", "-qm", "initial"],
    ];
    for args in commands {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(project)
                .args(*args)
                .status()
                .unwrap()
                .success()
        );
    }
}

#[cfg(unix)]
fn builtin_after_pr_fixture(
    project: &Path,
    wait_command: &str,
    repair_command: Option<&str>,
    finish_command: &str,
) -> CompiledWorkflow {
    let builtin = BUILTIN_WORKFLOWS
        .iter()
        .find(|builtin| builtin.name == "tdd")
        .unwrap_or_else(|| panic!("missing tdd built-in"));
    let config = resolve_workflow_config(
        builtin.yaml,
        &ConfigSource::NamedBuiltin(builtin.name),
        project,
        Some(project),
    )
    .unwrap();
    let mut compiled = compile(config).unwrap().to_after_pr_compiled();
    compiled.sdk = None;
    compiled.model = None;
    compiled.command = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        concat!(
            "grep -F 'captured failure evidence' >/dev/null || exit 97; ",
            "printf '%s\\n' '{{\"action\":\"wait\"}}'"
        )
        .to_owned(),
    ];
    for (name, step) in &mut compiled.steps {
        if step.squash_merge == Some(true) {
            continue;
        }
        step.require_bubblewrap = false;
        let prompt_repair = name == "follow-up/fix-ci-error" && repair_command.is_none();
        if prompt_repair {
            step.prompt_file = None;
            step.command = None;
        } else {
            let command = match name.as_str() {
                "follow-up/wait-ci" => wait_command,
                "follow-up/fix-ci-error" => repair_command.unwrap(),
                "follow-up/finish-ci-fix" => finish_command,
                _ => "true",
            };
            step.prompt = None;
            step.prompt_file = None;
            step.permission = None;
            step.allow_commit = false;
            step.command = Some(StringOrVec::Single(command.to_owned()));
            step.output_file = None;
        }
        step.quality_gates = None;
        step.option = None;
        step.parallel = None;
        step.group = None;
        step.workflow_call = None;
        step.github_review = None;
    }
    compiled
}

#[cfg(unix)]
async fn run_builtin_after_pr(
    project: &Path,
    wait_command: &str,
    repair_command: Option<&str>,
    finish_command: &str,
) -> (
    cruise::error::Result<cruise::engine::ExecutionResult>,
    ExecutionGraph,
    Vec<String>,
) {
    let compiled = builtin_after_pr_fixture(project, wait_command, repair_command, finish_command);
    let mut graph = build_graph(&compiled, MAX_RETRIES).unwrap();
    let started = Mutex::new(Vec::new());
    let on_step_start = |step: &str| {
        started.lock().unwrap().push(step.to_owned());
        if step == "squash-merge" {
            Err(CruiseError::Interrupted)
        } else {
            Ok(())
        }
    };
    let flags = [("after_pr".to_owned(), true), ("merge".to_owned(), true)].into();
    let option_handler = NoOpOptionHandler;
    let mut vars = VariableStore::new(String::new());
    vars.set_artifacts_root(project.to_path_buf());
    vars.set_named_value("plan", String::new());
    vars.set_named_value(
        "inputs.fix_ci_error_scope",
        "in-scope test changes".to_owned(),
    );
    vars.set_named_value("pr.number", "42".to_owned());
    vars.set_named_value("pr.url", "https://github.com/owner/repo/pull/42".to_owned());
    let mut tracker = FileTracker::with_root(project.to_path_buf());
    let result = {
        let context = ExecutionContext {
            usage_session_id: None,
            compiled: &compiled,
            max_retries: MAX_RETRIES,
            rate_limit_retries: 0,
            on_step_start: &on_step_start,
            cancel_token: None,
            option_handler: &option_handler,
            ask_handler: None,
            config_reloader: None,
            working_dir: Some(project),
            skipped_steps: &[],
            flags: &flags,
            on_step_log: None,
            instructions: None,
        };
        execute_steps_with_graph(
            &context,
            &mut vars,
            &mut tracker,
            &mut graph,
            &|_, _| Ok(()),
        )
        .await
    };
    (result, graph, started.into_inner().unwrap())
}

#[cfg(unix)]
async fn verify_builtin_after_pr_behavior(root: &Path) {
    verify_after_pr_green_ci(root).await;
    verify_after_pr_hard_failure_after_prompt(root).await;
    verify_after_pr_no_commit_repair(root).await;
}

#[cfg(unix)]
async fn verify_after_pr_green_ci(root: &Path) {
    let green = root.join("green");
    std::fs::create_dir(&green).unwrap();
    init_after_pr_repo(&green);
    let (result, graph, started) = run_builtin_after_pr(
        &green,
        "printf 'CI passed\\n' >&2; exit 0",
        Some("printf 'unexpected repair' > repair-ran"),
        "printf 'unexpected finish' > finish-ran",
    )
    .await;
    assert!(matches!(result, Err(CruiseError::Interrupted)));
    assert!(started.iter().any(|step| step == "squash-merge"));
    assert!(!started.iter().any(|step| step == "follow-up/fix-ci-error"));
    assert!(!started.iter().any(|step| step == "follow-up/finish-ci-fix"));
    assert!(!green.join("finish-ran").exists());
    assert!(!green.join("repair-ran").exists());
    assert_eq!(graph.state.current.as_deref(), Some("squash-merge"));
}

#[cfg(unix)]
async fn verify_after_pr_hard_failure_after_prompt(root: &Path) {
    let hard_failure = root.join("hard-failure");
    std::fs::create_dir(&hard_failure).unwrap();
    init_after_pr_repo(&hard_failure);
    write(
        &hard_failure.join("ci-followup/evidence.md"),
        "captured failure evidence",
    );
    let (result, _graph, started) = run_builtin_after_pr(
        &hard_failure,
        "printf 'CI_FAILURE: unit-test job failed\\n' >&2; exit 75",
        None,
        "printf 'host rejected checkpoint\\n' >&2; exit 1",
    )
    .await;
    assert!(
        matches!(
            result,
            Err(CruiseError::RequiredCommandFailed {
                ref step,
                exit_code: Some(1),
                ..
            }) if step == "follow-up/finish-ci-fix"
        ),
        "unexpected terminal result: {result:?}"
    );
    assert!(
        started.iter().any(|step| step == "follow-up/fix-ci-error"),
        "the CI repair prompt did not run: {started:?}"
    );
    assert!(
        started.iter().any(|step| step == "follow-up/finish-ci-fix"),
        "the host finalizer did not run after the prompt: {started:?}"
    );
    assert!(
        !started.iter().any(|step| step == "squash-merge"),
        "a hard finalizer failure reached merge: {started:?}"
    );
    let checkpoint = std::fs::read_to_string(hard_failure.join("ci-followup/result.json")).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&checkpoint).unwrap(),
        serde_json::json!({"action": "wait"})
    );
    assert!(!hard_failure.join("README.md").exists());
}

#[cfg(unix)]
async fn verify_after_pr_no_commit_repair(root: &Path) {
    let no_op = root.join("no-op");
    std::fs::create_dir(&no_op).unwrap();
    init_after_pr_repo(&no_op);
    let (result, graph, started) = run_builtin_after_pr(
        &no_op,
        "printf 'CI_FAILURE: unit-test job failed\\n' >&2; exit 75",
        Some("printf 'attempt\\n' >> repair-attempts; printf 'repair made no commit\\n' >&2"),
        "true",
    )
    .await;
    assert!(matches!(
        result,
        Err(CruiseError::LoopProtection {
            max_retries: MAX_RETRIES,
            ..
        })
    ));
    assert!(!started.iter().any(|step| step == "squash-merge"));
    assert_eq!(
        started
            .iter()
            .filter(|step| step.as_str() == "follow-up/fix-ci-error")
            .count(),
        MAX_RETRIES
    );
    assert_eq!(
        started
            .iter()
            .filter(|step| step.as_str() == "follow-up/finish-ci-fix")
            .count(),
        MAX_RETRIES
    );
    assert_eq!(
        std::fs::read_to_string(no_op.join("repair-attempts"))
            .unwrap()
            .lines()
            .count(),
        MAX_RETRIES
    );
    assert_eq!(
        graph.state.edge_counts[&(
            "follow-up/finish-ci-fix".to_owned(),
            "follow-up/wait-ci".to_owned()
        )]
            .budgeted_traversals,
        MAX_RETRIES
    );
    let checkpoint = serde_json::to_string(&graph).unwrap();
    let restored: ExecutionGraph = serde_json::from_str(&checkpoint).unwrap();
    assert_eq!(restored, graph);
}

#[cfg(unix)]
#[test]
fn builtin_after_pr_routes_ci_repair_and_stops_noop_loops() {
    run_isolated(&["after-pr-behavior"]);
}
