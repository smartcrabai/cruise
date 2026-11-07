//! Read-only workflow authoring diagnostics.
use crate::cli::WorkflowDoctorArgs;
use crate::config::{FailAction, StepConfig, WorkflowConfig};
use crate::error::CruiseError;
use crate::flags::FlagValues;
use crate::workflow::CompiledWorkflow;
use crate::workflow_doctor_targets::{ResolvedTarget, resolve_targets};
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
struct Diagnostic {
    level: &'static str,
    code: &'static str,
    stage: &'static str,
    step: Option<String>,
    message: String,
    #[serde(skip)]
    original_error: Option<CruiseError>,
}
impl Diagnostic {
    fn error(stage: &'static str, error: CruiseError) -> Self {
        Self {
            level: "error",
            code: error.telemetry_type(),
            stage,
            step: None,
            message: error.to_string(),
            original_error: Some(error),
        }
    }
    fn new(
        level: &'static str,
        code: &'static str,
        stage: &'static str,
        step: Option<String>,
        message: String,
    ) -> Self {
        Self {
            level,
            code,
            stage,
            step,
            message,
            original_error: None,
        }
    }
}
#[derive(Serialize)]
struct TargetReport {
    target: String,
    kind: &'static str,
    package: bool,
    path: Option<std::path::PathBuf>,
    status: &'static str,
    steps: usize,
    after_pr_steps: usize,
    max_retries: usize,
    flags: Vec<String>,
    diagnostics: Vec<Diagnostic>,
}
#[derive(Serialize)]
struct Summary {
    checked: usize,
    passed: usize,
    failed: usize,
    errors: usize,
    warnings: usize,
}
#[derive(Serialize)]
struct Report {
    schema_version: u8,
    ok: bool,
    exit_code: i32,
    summary: Summary,
    targets: Vec<TargetReport>,
}

fn is_remote(value: &str) -> bool {
    value.starts_with("https://github.com/")
        || value.starts_with("https://raw.githubusercontent.com/")
}
fn scan_step(name: &str, step: &StepConfig, base: &Path, diagnostics: &mut Vec<Diagnostic>) {
    for (field, value) in [
        ("prompt_file", &step.prompt_file),
        ("workflow_call", &step.workflow_call),
    ] {
        if let Some(value) = value
            && !is_remote(value)
        {
            let path = if field == "prompt_file" {
                crate::workflow_call::resolve_local_prompt_path(value, base)
            } else {
                base.join(value)
            };
            if !path.is_file() {
                diagnostics.push(Diagnostic::new(
                    "error",
                    "invalid_step_config",
                    "refs",
                    Some(name.to_owned()),
                    format!("step '{name}': {field} not found: {}", path.display()),
                ));
            }
        }
    }
    if let Some(children) = &step.parallel {
        for (name, child) in children {
            scan_step(name, child, base, diagnostics);
        }
    }
}
fn prescan(yaml: &str, base: &Path) -> Vec<Diagnostic> {
    let config = match WorkflowConfig::from_yaml(yaml) {
        Ok(config) => config,
        Err(e) => {
            return vec![Diagnostic::error(
                "refs",
                CruiseError::ConfigParseError(e.to_string()),
            )];
        }
    };
    let mut diagnostics = Vec::new();
    for (name, step) in config
        .steps
        .iter()
        .chain(&config.after_pr)
        .chain(config.groups.values().flat_map(|group| &group.steps))
    {
        scan_step(name, step, base, &mut diagnostics);
    }
    diagnostics
}
fn routing(compiled: &CompiledWorkflow) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut check = |name: &str, field: &str, target: &str| {
        if let Err(error) = crate::graph::resolve_target(compiled, target) {
            let mut diagnostic = Diagnostic::new(
                "error",
                "step_not_found",
                "routing",
                Some(name.to_owned()),
                format!("step '{name}': {field} target '{target}' is not a step"),
            );
            diagnostic.original_error = Some(error);
            diagnostics.push(diagnostic);
        }
    };
    for (name, step) in &compiled.steps {
        if let Some(target) = &step.next {
            check(name, "next", target);
        }
        if let Some(options) = &step.option {
            for option in options {
                if let Some(target) = &option.next {
                    check(name, "option next", target);
                }
            }
        }
        if let Some(cond) = &step.if_condition {
            if cond.no_file_changes.is_none()
                && let Some(target) = &cond.file_changed
            {
                check(name, "if.file-changed", target);
            }
            if let Some(FailAction::Goto(target)) = &cond.fail {
                check(name, "if.fail", target);
            }
        }
    }
    let mut invocations: Vec<_> = compiled.invocations.iter().collect();
    invocations.sort_by_key(|(name, _)| *name);
    for (name, meta) in invocations {
        if let Some(target) = meta
            .if_condition
            .as_ref()
            .and_then(|c| c.file_changed.as_ref())
        {
            check(name, "if.file-changed", target);
        }
    }
    diagnostics
}
struct CheckOutcome {
    diagnostics: Vec<Diagnostic>,
    steps: usize,
    after_pr_steps: usize,
    max_retries: usize,
    flags: Vec<String>,
}
fn check_resolved(
    config: WorkflowConfig,
    values: &FlagValues,
    flag_error: Option<&CruiseError>,
) -> CheckOutcome {
    let max_retries = crate::config::resolve_effective_max_retries(None, &config);
    let mut result = CheckOutcome {
        diagnostics: Vec::new(),
        steps: config.steps.len(),
        after_pr_steps: config.after_pr.len(),
        max_retries,
        flags: config.flags.keys().cloned().collect(),
    };
    result.diagnostics.extend(
        crate::config::validate_config_all(&config)
            .into_iter()
            .map(|e| Diagnostic::error("validate", e)),
    );
    if let Err(e) = crate::config::validate_group_retry_budget(&config, max_retries) {
        result.diagnostics.push(Diagnostic::error("validate", e));
    }
    if let Err(e) = crate::flags::validate_names(&config, values) {
        result.diagnostics.push(Diagnostic::new(
            "error",
            "unknown_flag",
            "validate",
            None,
            e.to_string(),
        ));
    }
    if let Some(error) = flag_error {
        result.diagnostics.push(Diagnostic::new(
            "error",
            "unknown_flag",
            "validate",
            None,
            error.to_string(),
        ));
    }
    if !result.diagnostics.is_empty() {
        return result;
    }
    let compiled = match crate::workflow::compile(config) {
        Ok(c) => c,
        Err(e) => {
            result.diagnostics.push(Diagnostic::error("compile", e));
            return result;
        }
    };
    result.steps = compiled.steps.len();
    result.after_pr_steps = compiled.after_pr.len();
    result.diagnostics.extend(routing(&compiled));
    if !compiled.after_pr.is_empty() {
        result
            .diagnostics
            .extend(routing(&compiled.to_after_pr_compiled()));
    }
    if !result.diagnostics.is_empty() {
        return result;
    }
    add_graph_diagnostics(&compiled, max_retries, values, &mut result.diagnostics);
    result
}

fn add_graph_diagnostics(
    compiled: &CompiledWorkflow,
    max_retries: usize,
    values: &FlagValues,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let graph = match crate::graph::build_graph(compiled, max_retries) {
        Ok(g) => g,
        Err(e) => {
            diagnostics.push(Diagnostic::error("graph", e));
            return;
        }
    };
    let skips = crate::flags::preflight_skips(compiled, values, &[]);
    let mut phases = if compiled.after_pr.is_empty() {
        match crate::graph::validation::analyze_workflow(compiled, &graph, &graph.start, &skips) {
            Ok(phases) => phases.into_iter().map(Ok).collect(),
            Err(error) => vec![Err(error)],
        }
    } else {
        vec![crate::graph::validation::analyze_phase(
            &graph,
            compiled,
            &graph.start,
            &skips,
            "main",
        )]
    };
    if phases[0].is_ok() && !compiled.after_pr.is_empty() {
        let after = compiled.to_after_pr_compiled();
        phases.push(
            crate::graph::build_graph(&after, max_retries).and_then(|after_graph| {
                crate::graph::validation::analyze_phase(
                    &after_graph,
                    &after,
                    &after_graph.start,
                    &skips,
                    "after-pr",
                )
            }),
        );
    }
    for phase in phases {
        match phase {
            Ok(phase) => {
                let enabled = values
                    .iter()
                    .filter(|(_, v)| **v)
                    .map(|(k, _)| k.as_str())
                    .collect::<Vec<_>>();
                let flags = if enabled.is_empty() {
                    "none".to_owned()
                } else {
                    enabled.join(", ")
                };
                for step in phase.unreachable_steps {
                    let message = format!(
                        "{} step '{step}' is unreachable under flags: {flags}",
                        phase.phase
                    );
                    diagnostics.push(Diagnostic::new(
                        "warning",
                        "unreachable_step",
                        "graph",
                        Some(step),
                        message,
                    ));
                }
                if let Some(message) = phase.cycle_without_exit {
                    diagnostics.push(Diagnostic::new(
                        "warning",
                        "cycle_may_not_exit",
                        "graph",
                        None,
                        message,
                    ));
                }
            }
            Err(e) => {
                let mut diagnostic = Diagnostic::error("graph", e);
                if matches!(
                    diagnostic.original_error.as_ref(),
                    Some(CruiseError::InvalidStepConfig(_))
                ) {
                    diagnostic.code = "no_normal_exit";
                }
                diagnostics.push(diagnostic);
            }
        }
    }
}
fn check_target(
    target: ResolvedTarget,
    cwd: &Path,
    values: &FlagValues,
    flag_error: Option<&CruiseError>,
) -> TargetReport {
    let canonical = target
        .path
        .as_ref()
        .and_then(|path| path.canonicalize().ok());
    let base = canonical.as_deref().and_then(Path::parent).unwrap_or(cwd);
    let mut report = TargetReport {
        target: target.target,
        kind: target.kind,
        package: target.package,
        path: target.path,
        status: "ok",
        steps: 0,
        after_pr_steps: 0,
        max_retries: 0,
        flags: Vec::new(),
        diagnostics: Vec::new(),
    };
    if target.kind != "builtin" {
        report.diagnostics = prescan(&target.yaml, base);
    }
    if report.diagnostics.is_empty() {
        let _quiet = crate::console_mode::quiet_guard();
        match crate::resolver::resolve_workflow_config_with_mode(
            &target.yaml,
            &target.source,
            base,
            Some(cwd),
            true,
        ) {
            Ok(config) => {
                for message in config.deprecated_language_warnings() {
                    report.diagnostics.push(Diagnostic::new(
                        "warning",
                        "deprecated_setting",
                        "resolve",
                        None,
                        message,
                    ));
                }
                let outcome = check_resolved(config, values, flag_error);
                report.steps = outcome.steps;
                report.after_pr_steps = outcome.after_pr_steps;
                report.max_retries = outcome.max_retries;
                report.flags = outcome.flags;
                report.diagnostics.extend(outcome.diagnostics);
            }
            Err(e) => report.diagnostics.push(Diagnostic::error("resolve", e)),
        }
    }
    if let Some(name) = target.package_name
        && let Err(e) = crate::workflow_packages::workflows_dir()
            .and_then(|dir| crate::workflow_packages::load_unmodified(&dir, &name))
    {
        let (level, code) = if matches!(
            e,
            crate::workflow_packages::WorkflowPackageError::Modified(_)
        ) {
            ("warning", "package_modified")
        } else {
            ("error", "io_error")
        };
        report
            .diagnostics
            .push(Diagnostic::new(level, code, "package", None, e.to_string()));
    }
    report.status = if report.diagnostics.iter().any(|d| d.level == "error") {
        "error"
    } else if report.diagnostics.is_empty() {
        "ok"
    } else {
        "warning"
    };
    report
}
/// Validate generated YAML with isolated defaults and the shared static pipeline.
pub(crate) fn check_generated_yaml(yaml: &str, base_dir: &Path) -> crate::error::Result<()> {
    let config = WorkflowConfig::from_yaml(yaml)
        .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?;
    let config = crate::workflow_call::resolve_workflow_calls_without_app_model_defaults(
        config,
        base_dir,
        &crate::workflow_defaults::WorkflowDefaults::default(),
    )?;
    let result = check_resolved(config, &crate::flags::NO_FLAGS, None);
    if let Some(error) = result.diagnostics.into_iter().find(|d| d.level == "error") {
        return Err(error
            .original_error
            .unwrap_or(CruiseError::InvalidStepConfig(error.message)));
    }
    Ok(())
}
pub(crate) fn run(args: &WorkflowDoctorArgs) -> i32 {
    let _quiet = crate::console_mode::quiet_guard();
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let flag_result = crate::flags::from_cli(&args.flags);
    let values = flag_result.as_ref().unwrap_or(&crate::flags::NO_FLAGS);
    let targets: Vec<_> = resolve_targets(&args.targets, args.all, &cwd)
        .into_iter()
        .map(|t| match t {
            Ok(target) => check_target(target, &cwd, values, flag_result.as_ref().err()),
            Err(e) => TargetReport {
                target: e.target,
                kind: "file",
                package: false,
                path: None,
                status: "unresolved",
                steps: 0,
                after_pr_steps: 0,
                max_retries: 0,
                flags: vec![],
                diagnostics: vec![Diagnostic::new("error", e.code, "target", None, e.message)],
            },
        })
        .collect();
    let passed = targets
        .iter()
        .filter(|t| t.status == "ok" || t.status == "warning")
        .count();
    let errors = targets
        .iter()
        .flat_map(|t| &t.diagnostics)
        .filter(|d| d.level == "error")
        .count();
    let warnings = targets
        .iter()
        .flat_map(|t| &t.diagnostics)
        .filter(|d| d.level == "warning")
        .count();
    let exit_code = if targets.iter().any(|t| t.status == "unresolved") {
        2
    } else {
        i32::from(errors > 0 || args.strict && warnings > 0)
    };
    let report = Report {
        schema_version: 1,
        ok: exit_code == 0,
        exit_code,
        summary: Summary {
            checked: targets.len(),
            passed,
            failed: targets.len() - passed,
            errors,
            warnings,
        },
        targets,
    };
    if args.json {
        if let Err(e) = serde_json::to_writer_pretty(std::io::stdout().lock(), &report) {
            eprintln!("{e}");
            return 1;
        }
        println!();
    } else {
        print_report(&report);
    }
    exit_code
}

fn print_report(report: &Report) {
    for target in &report.targets {
        let detail = if target.status == "ok" {
            format!(
                "{} steps, {} after-pr steps",
                target.steps, target.after_pr_steps
            )
        } else if target.status == "unresolved" {
            String::new()
        } else {
            let errors = target
                .diagnostics
                .iter()
                .filter(|d| d.level == "error")
                .count();
            let warnings = target
                .diagnostics
                .iter()
                .filter(|d| d.level == "warning")
                .count();
            format!("{errors} errors, {warnings} warnings")
        };
        println!(
            "{:<10} {}  {}",
            target.status,
            crate::batch_dashboard::sanitize_terminal_text(&target.target),
            detail
        );
        for d in &target.diagnostics {
            println!(
                "             {:<7} {:<20} {}",
                d.level,
                d.code,
                crate::batch_dashboard::sanitize_terminal_text(&d.message)
            );
        }
    }
    let unresolved = report
        .targets
        .iter()
        .filter(|t| t.status == "unresolved")
        .count();
    println!(
        "\nChecked {} workflows: {} passed, {} failed ({} unresolved); {} errors, {} warnings",
        report.summary.checked,
        report.summary.passed,
        report.summary.failed,
        unresolved,
        report.summary.errors,
        report.summary.warnings
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prescan_collects_missing_references_across_all_step_kinds()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let yaml = "steps:\n  a:\n    prompt_file: missing-a\n  d:\n    workflow_call: ./missing.yaml\n  remote:\n    workflow_call: https://github.com/a/b/blob/main/w.yaml\ngroups:\n  g:\n    steps:\n      b:\n        prompt_file: missing-b\nafter-pr:\n  c:\n    prompt_file: missing-c\n";
        let diagnostics = prescan(yaml, dir.path());
        assert_eq!(diagnostics.len(), 4);
        for name in ["a", "b", "c", "d"] {
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.step.as_deref() == Some(name) && d.stage == "refs")
            );
        }
        Ok(())
    }

    #[test]
    fn routing_collects_independent_missing_destinations() -> Result<(), Box<dyn std::error::Error>>
    {
        let yaml = "steps:\n  a:\n    command: echo a\n    next: missing1\n  b:\n    option:\n      - selector: x\n        next: missing2\n  c:\n    command: echo c\n    if:\n      fail: missing3\n";
        let config = WorkflowConfig::from_yaml(yaml)?;
        let compiled = crate::workflow::compile(config)?;
        let diagnostics = routing(&compiled);
        assert_eq!(diagnostics.len(), 3);
        for name in ["a", "b", "c"] {
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.step.as_deref() == Some(name) && d.code == "step_not_found")
            );
        }
        Ok(())
    }
}
