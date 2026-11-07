#![cfg(unix)]

//! Observable contracts for `workflow doctor`. Each invocation uses isolated XDG roots.
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

const OK: &str =
    "command: [echo]\nsteps:\n  build:\n    command: echo build\n  test:\n    command: echo test\n";

struct Fixture {
    root: TempDir,
}
impl Fixture {
    fn new() -> Self {
        let root =
            tempfile::tempdir().unwrap_or_else(|e| panic!("create temporary directory: {e}"));
        for name in ["cwd", "home", "config", "data", "state"] {
            fs::create_dir(root.path().join(name))
                .unwrap_or_else(|e| panic!("create fixture directory: {e}"));
        }
        Self { root }
    }
    fn write(&self, path: &str, text: &str) {
        let path = self.root.path().join(path);
        fs::create_dir_all(
            path.parent()
                .unwrap_or_else(|| panic!("fixture path has no parent")),
        )
        .unwrap_or_else(|e| panic!("create fixture parent: {e}"));
        fs::write(path, text).unwrap_or_else(|e| panic!("write fixture: {e}"));
    }
    fn run(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("CRUISE_") {
                cmd.env_remove(key);
            }
        }
        cmd.current_dir(self.root.path().join("cwd"))
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("NO_COLOR", "1")
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("run doctor command: {e}"))
    }
    fn doctor(&self, args: &[&str], code: i32) -> Value {
        let mut all = vec!["workflow", "doctor", "--json"];
        all.extend_from_slice(args);
        let out = self.run(&all);
        assert_eq!(
            out.status.code(),
            Some(code),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.stderr.is_empty(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let report: Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("parse doctor report: {e}"));
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["exit_code"], code);
        assert_eq!(report["ok"], code == 0);
        report
    }
}
fn targets(report: &Value) -> &[Value] {
    report["targets"]
        .as_array()
        .unwrap_or_else(|| panic!("expected value missing from doctor report"))
}
fn has_code(target: &Value, code: &str) -> bool {
    target["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("expected value missing from doctor report"))
        .iter()
        .any(|d| d["code"] == code)
}

#[test]
fn inactive_file_changed_target_is_not_checked() {
    let f = Fixture::new();
    let yaml =
        "command: [echo]\nsteps:\n  a:\n    command: echo\n    if:\n      file-changed: missing\n";
    f.write("cwd/active.yaml", yaml);
    f.write(
        "cwd/inactive.yaml",
        &format!("{yaml}      no-file-changes: failed\n"),
    );
    assert!(has_code(
        &targets(&f.doctor(&["active.yaml"], 1))[0],
        "step_not_found"
    ));
    assert!(!has_code(
        &targets(&f.doctor(&["inactive.yaml"], 0))[0],
        "step_not_found"
    ));
}

#[test]
fn reports_sdk_and_retry_errors_in_validation_order() {
    let f = Fixture::new();
    f.write(
        "cwd/invalid.yaml",
        "sdk: invalid\nretry:\n  base_delay_ms: 0\nsteps:\n  a:\n    command: echo\n",
    );
    let report = f.doctor(&["invalid.yaml"], 1);
    let diagnostics = targets(&report)[0]["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("expected value missing from doctor report"));
    let messages: Vec<_> = diagnostics
        .iter()
        .map(|d| {
            d["message"]
                .as_str()
                .unwrap_or_else(|| panic!("expected value missing from doctor report"))
        })
        .collect();
    let sdk = messages
        .iter()
        .position(|m| m.contains("sdk"))
        .unwrap_or_else(|| panic!("expected value missing from doctor report"));
    let retry = messages
        .iter()
        .position(|m| m.contains("retry.base_delay_ms"))
        .unwrap_or_else(|| panic!("expected value missing from doctor report"));
    assert!(sdk < retry, "{messages:?}");
}

#[test]
fn reports_sdk_and_parallel_errors_in_validation_order() {
    let f = Fixture::new();
    f.write(
        "cwd/invalid-parallel.yaml",
        "sdk: invalid\nsteps:\n  a:\n    parallel: {}\n",
    );
    let report = f.doctor(&["invalid-parallel.yaml"], 1);
    let diagnostics = targets(&report)[0]["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("expected value missing from doctor report"));
    let messages: Vec<_> = diagnostics
        .iter()
        .map(|d| {
            d["message"]
                .as_str()
                .unwrap_or_else(|| panic!("expected value missing from doctor report"))
        })
        .collect();
    let sdk = messages
        .iter()
        .position(|m| m.contains("sdk"))
        .unwrap_or_else(|| panic!("expected value missing from doctor report"));
    let parallel = messages
        .iter()
        .position(|m| m.contains("parallel step 'a'"))
        .unwrap_or_else(|| panic!("expected value missing from doctor report"));
    assert!(sdk < parallel, "{messages:?}");
}

#[test]
fn symlinked_workflow_resolves_references_beside_canonical_file() {
    let f = Fixture::new();
    f.write(
        "real/workflow.yaml",
        "command: [echo]\nsteps:\n  a:\n    prompt_file: prompt.md\n",
    );
    f.write("real/prompt.md", "hello");
    std::os::unix::fs::symlink(
        f.root.path().join("real/workflow.yaml"),
        f.root.path().join("cwd/link.yaml"),
    )
    .unwrap_or_else(|e| panic!("create workflow symlink: {e}"));
    assert_eq!(targets(&f.doctor(&["link.yaml"], 0))[0]["status"], "ok");
}

#[test]
fn default_discovers_local_workflows_and_reports_routing_errors() {
    let f = Fixture::new();
    f.write("cwd/.cruise/ok.yaml", OK);
    f.write(
        "cwd/.cruise/bad.yaml",
        "command: [echo]\nsteps:\n  build:\n    command: echo build\n    next: nowhere\n",
    );
    f.write("cwd/.cruise/default.yaml", "{}\n");
    let r = f.doctor(&[], 1);
    assert_eq!(r["summary"]["checked"], 2);
    assert_eq!(r["summary"]["passed"], 1);
    assert_eq!(r["summary"]["failed"], 1);
    let ok = targets(&r)
        .iter()
        .find(|t| {
            t["target"]
                .as_str()
                .unwrap_or_else(|| panic!("expected value missing from doctor report"))
                .ends_with("ok.yaml")
        })
        .unwrap_or_else(|| panic!("expected value missing from doctor report"));
    let bad = targets(&r)
        .iter()
        .find(|t| {
            t["target"]
                .as_str()
                .unwrap_or_else(|| panic!("expected value missing from doctor report"))
                .ends_with("bad.yaml")
        })
        .unwrap_or_else(|| panic!("expected value missing from doctor report"));
    assert_eq!(ok["kind"], "project");
    assert_eq!(ok["status"], "ok");
    assert_eq!(bad["kind"], "project");
    assert_eq!(bad["status"], "error");
    assert!(has_code(bad, "step_not_found"));
}

#[test]
fn unreadable_project_directory_is_unresolved_with_valid_target() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.write("cwd/cruise.yaml", OK);
    let dir = f.root.path().join("cwd/.cruise");
    fs::create_dir(&dir).unwrap_or_else(|e| panic!("create project directory: {e}"));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o0))
        .unwrap_or_else(|e| panic!("remove directory permissions: {e}"));
    if fs::read_dir(&dir).is_ok() {
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|e| panic!("restore directory permissions: {e}"));
        eprintln!("skipping unreadable directory assertion: permissions are not enforced");
        return;
    }
    let output = f.run(&["workflow", "doctor", "--json"]);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|e| panic!("restore directory permissions: {e}"));
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("parse doctor report: {e}"));
    // The valid workflow is discovered, but loading project defaults also
    // encounters the unreadable directory and can report a separate error.
    assert!(
        targets(&report)
            .iter()
            .any(|t| t["target"] == "./cruise.yaml" && t["status"] != "unresolved")
    );
    assert!(targets(&report).iter().any(|t| {
        t["status"] == "unresolved"
            && has_code(t, "invalid_target")
            && t["target"]
                .as_str()
                .unwrap_or_else(|| panic!("expected value missing from doctor report"))
                .ends_with(".cruise")
    }));
}

#[test]
fn all_includes_all_builtins_without_diagnostics() {
    let f = Fixture::new();
    let r = f.doctor(&["--all"], 0);
    let builtins: Vec<_> = targets(&r)
        .iter()
        .filter(|t| t["kind"] == "builtin")
        .collect();
    let expected: Vec<_> = cruise::builtin_workflows::BUILTIN_WORKFLOWS
        .iter()
        .map(|workflow| format!("builtin:{}", workflow.name))
        .collect();
    let actual: Vec<_> = builtins
        .iter()
        .map(|target| target["target"].as_str())
        .collect();
    assert_eq!(
        actual,
        expected
            .iter()
            .map(|name| Some(name.as_str()))
            .collect::<Vec<_>>()
    );
    for t in builtins {
        assert_eq!(t["status"], "ok");
        assert_eq!(
            t["diagnostics"]
                .as_array()
                .unwrap_or_else(|| panic!("expected value missing from doctor report"))
                .len(),
            0
        );
    }
}

#[test]
fn all_reports_user_workflow_directory_read_failure() {
    let f = Fixture::new();
    f.write("config/cruise/workflows", "not a directory");
    let report = f.doctor(&["--all"], 2);
    let failure = targets(&report)
        .iter()
        .find(|t| t["status"] == "unresolved")
        .unwrap_or_else(|| panic!("expected value missing from doctor report"));
    assert!(has_code(failure, "invalid_target"));
    assert!(
        failure["target"]
            .as_str()
            .unwrap_or_else(|| panic!("expected value missing from doctor report"))
            .ends_with("cruise/workflows")
    );
    assert!(
        failure["diagnostics"][0]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("expected value missing from doctor report"))
            .contains("cannot read")
    );
}

#[test]
fn missing_and_empty_targets_are_usage_errors() {
    let f = Fixture::new();
    let missing = f.doctor(&["nope"], 2);
    assert_eq!(targets(&missing)[0]["status"], "unresolved");
    assert!(has_code(&targets(&missing)[0], "target_not_found"));
    let empty = f.doctor(&[], 2);
    assert_eq!(targets(&empty)[0]["status"], "unresolved");
    assert!(has_code(&targets(&empty)[0], "no_targets"));
}

#[test]
fn unresolved_takes_precedence_and_preserves_input_order() {
    let f = Fixture::new();
    f.write(
        "cwd/bad.yaml",
        "command: [echo]\nsteps:\n  a:\n    command: echo a\n    next: absent\n",
    );
    let r = f.doctor(&["bad.yaml", "missing.yaml"], 2);
    assert_eq!(targets(&r)[0]["target"], "bad.yaml");
    assert_eq!(targets(&r)[0]["status"], "error");
    assert_eq!(targets(&r)[1]["status"], "unresolved");
}

#[test]
fn flags_must_be_declared_per_target() {
    let f = Fixture::new();
    f.write("cwd/ok.yaml", OK);
    for flag in ["unknown", "after_pr"] {
        let r = f.doctor(&["-f", flag, "ok.yaml"], 1);
        assert!(has_code(&targets(&r)[0], "unknown_flag"));
    }
}

#[test]
fn selected_flags_and_duplicate_assignments_are_checked() {
    let f = Fixture::new();
    f.write("cwd/flags.yaml", "command: [echo]\nflags:\n  gate: {}\nsteps:\n  a: {command: echo}\n  b: {command: echo, flag: gate}\n");
    f.doctor(&["-f", "gate", "flags.yaml"], 0);
    let report = f.doctor(&["-f", "gate", "-f", "gate", "flags.yaml"], 1);
    assert!(
        targets(&report)[0]["diagnostics"]
            .as_array()
            .unwrap_or_else(|| panic!("expected value missing from doctor report"))
            .iter()
            .any(|d| d["message"]
                .as_str()
                .unwrap_or_else(|| panic!("expected value missing from doctor report"))
                .contains("more than once"))
    );
}

#[test]
fn all_conflicts_with_targets_and_flags() {
    let f = Fixture::new();
    for args in [
        &["workflow", "doctor", "--all", "ok.yaml"][..],
        &["workflow", "doctor", "--all", "-f", "x"][..],
    ] {
        let out = f.run(args);
        assert_eq!(out.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("--all") && stderr.contains("cannot be used"),
            "{stderr}"
        );
    }
}

#[test]
fn modified_package_is_warning_and_strict_makes_it_fail() {
    let f = Fixture::new();
    f.write("config/cruise/workflows/pkg.yaml", OK);
    f.write("config/cruise/workflows/pkg.cruise-package.json", r#"{"name":"pkg","owner":"owner","repo":"repo","workflow_path":"cruise.yaml","requested_ref":"main","resolved_commit":"1111111111111111111111111111111111111111","workflow_sha256":"0000000000000000000000000000000000000000000000000000000000000000"}"#);
    for (args, code) in [(&["pkg"][..], 0), (&["--strict", "pkg"][..], 1)] {
        let r = f.doctor(args, code);
        let t = &targets(&r)[0];
        assert_eq!(t["kind"], "user");
        assert_eq!(t["package"], true);
        assert_eq!(t["status"], "warning");
        assert!(has_code(t, "package_modified"));
    }
}

#[test]
fn builtin_name_ambiguity_is_not_silently_resolved() {
    let f = Fixture::new();
    f.write("config/cruise/workflows/tdd.yaml", OK);
    let r = f.doctor(&["tdd"], 2);
    assert!(has_code(&targets(&r)[0], "ambiguous_target"));
    let r = f.doctor(&["builtin:tdd"], 0);
    assert_eq!(targets(&r)[0]["kind"], "builtin");
}

#[test]
fn references_are_collected_before_resolution() {
    let f = Fixture::new();
    f.write("cwd/refs.yaml", "command: [echo]\nsteps:\n  a:\n    prompt_file: missing1.md\n  b:\n    prompt_file: missing2.md\n  c:\n    workflow_call: ./missing.yaml\n");
    let r = f.doctor(&["refs.yaml"], 1);
    let ds = targets(&r)[0]["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("expected value missing from doctor report"));
    for step in ["a", "b", "c"] {
        assert!(
            ds.iter().any(|d| d["step"] == step && d["stage"] == "refs"),
            "{ds:?}"
        );
    }
}

#[test]
fn unreachable_is_a_warning_and_strict_promotes_exit_status() {
    let f = Fixture::new();
    f.write("cwd/branch.yaml", "command: [echo]\nsteps:\n  a:\n    command: echo a\n    next: c\n  b:\n    command: echo b\n  c:\n    command: echo c\n");
    for (args, code) in [
        (&["branch.yaml"][..], 0),
        (&["--strict", "branch.yaml"][..], 1),
    ] {
        let r = f.doctor(args, code);
        let d = targets(&r)[0]["diagnostics"]
            .as_array()
            .unwrap_or_else(|| panic!("expected value missing from doctor report"))
            .iter()
            .find(|d| d["code"] == "unreachable_step")
            .unwrap_or_else(|| panic!("expected value missing from doctor report"));
        assert_eq!(d["level"], "warning");
        assert_eq!(d["step"], "b");
        assert!(
            d["message"]
                .as_str()
                .unwrap_or_else(|| panic!("expected value missing from doctor report"))
                .contains("under flags: none")
        );
        for key in ["level", "code", "stage", "step", "message"] {
            assert!(d.get(key).is_some());
        }
    }
}

#[test]
fn main_warnings_survive_after_pr_cycle_error() {
    let f = Fixture::new();
    f.write("cwd/cycle.yaml", "command: [echo]\nsteps:\n  a: {command: echo, next: c}\n  b: {command: echo}\n  c: {command: echo}\nafter-pr:\n  loop: {command: echo, next: loop}\n");
    let report = f.doctor(&["cycle.yaml"], 1);
    let diagnostics = targets(&report)[0]["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("expected value missing from doctor report"));
    assert!(
        diagnostics
            .iter()
            .any(|d| d["code"] == "unreachable_step" && d["step"] == "b")
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d["code"] == "no_normal_exit" && d["level"] == "error")
    );
}

#[test]
fn doctor_does_not_create_runtime_state_or_modify_project() {
    let f = Fixture::new();
    f.write("cwd/cruise.yaml", OK);
    let before = fs::read(f.root.path().join("cwd/cruise.yaml"))
        .unwrap_or_else(|e| panic!("read project workflow: {e}"));
    f.doctor(&[], 0);
    assert_eq!(
        fs::read(f.root.path().join("cwd/cruise.yaml"))
            .unwrap_or_else(|e| panic!("read project workflow: {e}")),
        before
    );
    assert_eq!(
        fs::read_dir(f.root.path().join("cwd"))
            .unwrap_or_else(|e| panic!("read project directory: {e}"))
            .count(),
        1
    );
    for path in [
        "data/cruise",
        "state/cruise",
        "config/cruise/default.yaml",
        "cwd/.cruise",
    ] {
        assert!(
            !Path::new(&f.root.path().join(path)).exists(),
            "created {path}"
        );
    }
}
