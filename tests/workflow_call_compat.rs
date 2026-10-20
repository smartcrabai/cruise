use std::path::PathBuf;

use cruise::config::WorkflowConfig;
use cruise::error::CruiseError;
use cruise::workflow_call::{resolve_workflow_calls, resolve_workflow_calls_from_path};
use cruise::workflow_defaults::{WorkflowConfigResolution, WorkflowDefaults};
use tempfile::TempDir;

#[test]
fn workflow_config_retains_legacy_direct_field_types() {
    let config = WorkflowConfig::from_yaml("steps: {}\n").unwrap_or_else(|error| panic!("{error}"));
    let _: bool = config.interactive_planning;
    let _: bool = config.cleanup_after_pr;
    let _: bool = config.force_exec;
    let _: bool = config.computer_use;
    let _: cruise::config::PermissionMode = config.permission;
}

#[test]
fn public_defaults_application_uses_workflow_yaml_field_presence() {
    let project = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let cruise_dir = project.path().join(".cruise");
    std::fs::create_dir_all(&cruise_dir)
        .unwrap_or_else(|error| panic!("create defaults dir: {error}"));
    std::fs::write(
        cruise_dir.join("default.yaml"),
        "interactive_planning: true\n",
    )
    .unwrap_or_else(|error| panic!("write defaults: {error}"));
    let defaults = WorkflowDefaults::load(Some(project.path()))
        .unwrap_or_else(|error| panic!("load defaults: {error}"));

    let mut absent = WorkflowConfigResolution::from_yaml("steps: {}\n")
        .unwrap_or_else(|error| panic!("parse workflow: {error}"));
    defaults
        .apply_to(&mut absent)
        .unwrap_or_else(|error| panic!("apply defaults: {error}"));
    assert!(absent.config.interactive_planning);

    let mut explicit_false =
        WorkflowConfigResolution::from_yaml("interactive_planning: false\nsteps: {}\n")
            .unwrap_or_else(|error| panic!("parse workflow: {error}"));
    defaults
        .apply_to(&mut explicit_false)
        .unwrap_or_else(|error| panic!("apply defaults: {error}"));
    assert!(!explicit_false.config.interactive_planning);
}

#[test]
fn legacy_resolver_call_arities_remain_public() {
    let _: fn(WorkflowConfig, PathBuf) -> Result<WorkflowConfig, CruiseError> =
        resolve_workflow_calls;
    let _: fn(PathBuf) -> Result<WorkflowConfig, CruiseError> = resolve_workflow_calls_from_path;
}
