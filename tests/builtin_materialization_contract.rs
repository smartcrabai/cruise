//! Observable built-in materialization contracts, with host settings isolated in subprocesses.
#![expect(clippy::unwrap_used, reason = "isolated contract fixtures")]

use std::path::{Path, PathBuf};
use std::process::Command;

use cruise::builtin_workflows::BUILTIN_WORKFLOWS;
use cruise::config::{PermissionMode, SkipCondition, WorkflowConfig, validate_config};
use cruise::error::CruiseError;
use cruise::flags::{self, FlagValues};
use cruise::resolver::{ConfigSource, resolve_workflow_config};
use cruise::workflow::compile;
use cruise::workflow_call::resolve_workflow_calls_from_path_with_defaults;
use cruise::workflow_cmd::eject_builtin;
use cruise::workflow_defaults::WorkflowDefaults;

const WORKER_PHASE: &str = "BUILTIN_MATERIALIZATION_TEST_PHASE";
const AFTER_PR_IDS: [&str; 7] = [
    "follow-up/pr-ready",
    "follow-up/sync-base",
    "follow-up/resolve-conflict",
    "follow-up/push-after-sync",
    "follow-up/wait-ci",
    "follow-up/fix-ci-error",
    "squash-merge",
];

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn assert_after_pr_contract(config: WorkflowConfig) {
    validate_config(&config).unwrap();
    let compiled = compile(config).unwrap();
    let ids: Vec<_> = compiled.after_pr.keys().map(String::as_str).collect();
    assert_eq!(ids, AFTER_PR_IDS);
    assert!(
        compiled
            .steps
            .values()
            .all(|step| step.squash_merge.is_none())
    );
    for (index, step) in compiled.after_pr.values().enumerate() {
        let expected: &[&str] = if index == 6 {
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
            if index == 6 { Some(true) } else { None }
        );
    }
    let fix = &compiled.after_pr["follow-up/fix-ci-error"];
    assert_eq!(fix.next.as_deref(), Some("follow-up/wait-ci"));
    assert_eq!(
        fix.if_condition.as_ref().unwrap().file_changed.as_deref(),
        Some("follow-up/sync-base")
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
        AFTER_PR_IDS[..6]
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
        "inputs: [resolve_conflict_prompt, fix_ci_error_prompt]\nsteps:\n  local-only:\n    command: 'true'\n",
    );
    for builtin in BUILTIN_WORKFLOWS {
        let path = local.join(format!("{}.yaml", builtin.name));
        write(&path, builtin.yaml);
        let config =
            resolve_workflow_calls_from_path_with_defaults(path, &WorkflowDefaults::default())
                .unwrap();
        validate_config(&config).unwrap();
        assert!(config.after_pr.contains_key("follow-up/local-only"));
        assert!(!config.after_pr.contains_key("follow-up/wait-ci"));
        assert_eq!(config.after_pr.last().unwrap().0, "squash-merge");
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
        for id in ["follow-up/resolve-conflict", "follow-up/fix-ci-error"] {
            let step = config
                .after_pr
                .get(id)
                .unwrap_or_else(|| panic!("{} did not materialize {id}", builtin.name));
            assert!(
                step.allow_commit,
                "{} {id} lost commit permission",
                builtin.name
            );
            assert_eq!(step.permission, Some(PermissionMode::Full));
            assert!(
                matches!(&step.skip, Some(SkipCondition::Variable(name)) if name == "prev.success")
            );
        }
        assert_eq!(
            config.after_pr["follow-up/wait-ci"].timeout.as_deref(),
            Some("30m")
        );
        assert_eq!(
            config.after_pr["squash-merge"].timeout.as_deref(),
            Some("10m")
        );
        assert_eq!(config.create_pr, builtin.name != "research");
    }
}

fn worker(root: &Path, phase: &str) {
    let project = root.join("project");
    let config_dir = root.join("config/cruise");
    match phase {
        "eject" => eject_with_host_settings(root, &project, &config_dir),
        "local" => verify_local_builtin_resources(root),
        "repair-policy" => verify_builtin_repair_policy(&project),
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
                assert_after_pr_contract(config);
            }
        }
        "verify" => {
            // A cwd-local dependency is deliberately invalid. Embedded resolution must ignore it.
            write(&project.join("_shared/after-pr.yaml"), "not: [valid\n");
            for builtin in BUILTIN_WORKFLOWS {
                let path = artifact(root, builtin.name);
                let config = resolve_workflow_calls_from_path_with_defaults(
                    &path,
                    &WorkflowDefaults::default(),
                )
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
                        &project,
                        Some(&project),
                    )
                    .unwrap();
                    let path = artifact(root, builtin.name);
                    let defaults = WorkflowDefaults::load(Some(&project)).unwrap();
                    let ejected =
                        resolve_workflow_calls_from_path_with_defaults(&path, &defaults).unwrap();
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
                        !project_override || !matches!(builtin.name, "tdd" | "research")
                    );
                    validate_config(&embedded).unwrap();
                }
            }
        }
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
