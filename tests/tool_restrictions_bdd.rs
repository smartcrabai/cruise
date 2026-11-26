//! Task-grounded contracts against the public policy API and the real pre-tool binary.
#![expect(
    clippy::unwrap_used,
    reason = "BDD assertions and isolated child processes"
)]
use std::io::{ErrorKind, Write};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use cruise::config::{McpServers, PermissionMode};
use cruise::tool_policy::{RuleSource, ToolPolicy, ToolRestriction};
use cucumber::{World as _, given, then, when};

#[derive(Debug, Default, cucumber::World)]
struct PolicyWorld {
    workflow: Vec<ToolRestriction>,
    step: Vec<ToolRestriction>,
    policy: ToolPolicy,
    output: Option<Output>,
}
fn restrictions(text: &str) -> Vec<ToolRestriction> {
    if text == "none" {
        return Vec::new();
    }
    text.split(',')
        .map(|s| ToolRestriction::try_from(s.trim().to_owned()).unwrap())
        .collect()
}
#[given(expr = "workflow restrictions {string} and step restrictions {string}")]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Cucumber captures owned strings"
)]
fn inputs(w: &mut PolicyWorld, workflow: String, step_restrictions: String) {
    w.workflow = restrictions(&workflow);
    w.step = restrictions(&step_restrictions);
}
#[when(expr = "the public policy is computed with permission {word} and allow_commit {word}")]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Cucumber captures owned strings"
)]
fn compute(w: &mut PolicyWorld, permission: String, allow: String) {
    let permission = match permission.as_str() {
        "full" => PermissionMode::Full,
        "edit" => PermissionMode::Edit,
        "read-only" => PermissionMode::ReadOnly,
        _ => panic!("unknown permission {permission}"),
    };
    assert!(matches!(allow.as_str(), "true" | "false"));
    w.policy = ToolPolicy::for_prompt_step(&w.workflow, &w.step, allow == "true", permission);
}
#[then("workflow and step restrictions form a deduplicated union")]
fn union(w: &mut PolicyWorld) {
    assert!(w.policy.deny_web && w.policy.deny_subagent);
    assert_eq!(
        w.policy
            .denied_mcp_servers
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        ["github"]
    );
    assert_eq!(w.policy.shell_rules.len(), 1);
    assert_eq!(w.policy.shell_rules[0].prefix, "npm publish");
    assert_eq!(w.policy.shell_rules[0].source, RuleSource::DisallowedTools);
    assert!(w.policy.has_explicit());
    let notice = w.policy.prompt_notice().unwrap();
    for label in ["web", "subagent", "mcp:github", "shell:npm publish"] {
        assert!(notice.contains(label), "{notice}");
    }
}
#[then("all implicit git rules have allow_commit provenance and no explicit notice")]
fn implicit(w: &mut PolicyWorld) {
    assert_eq!(
        w.policy
            .shell_rules
            .iter()
            .map(|r| r.prefix.as_str())
            .collect::<Vec<_>>(),
        // Normative plan section 2.3, deliberately independent of production constants.
        [
            "git commit",
            "git merge",
            "git rebase",
            "git cherry-pick",
            "git revert",
            "git am",
            "git pull",
            "git push",
            "git reset",
            "git switch",
            "git checkout -b",
            "git checkout -B",
            "git update-ref",
            "git config core.hooksPath",
            "git -c core.hooksPath=",
        ]
    );
    assert!(
        w.policy
            .shell_rules
            .iter()
            .all(|r| r.source == RuleSource::AllowCommit)
    );
    assert!(!w.policy.has_explicit());
    assert_eq!(w.policy.prompt_notice(), None);
}
#[then(expr = "the policy has {int} shell rules")]
fn shell_rule_count(w: &mut PolicyWorld, count: usize) {
    assert_eq!(w.policy.shell_rules.len(), count);
}
#[then("web and subagent restrictions remain active")]
fn non_shell(w: &mut PolicyWorld) {
    assert!(w.policy.deny_web && w.policy.deny_subagent);
}
#[then(expr = "MCP filtering retains {string} without mutating the input")]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Cucumber captures owned strings"
)]
fn mcp(w: &mut PolicyWorld, expected: String) {
    let servers: McpServers = serde_yaml::from_str(
        "github:\n  command: github-mcp\n  args: [serve]\ndocs:\n  type: http\n  url: https://example.invalid/mcp\n  headers:\n    X-Test: retained\n",
    ).unwrap();
    let original = servers.clone();
    let filtered = w.policy.filter_mcp_servers(&servers);
    let names = if expected == "none" {
        Vec::new()
    } else {
        expected.split(',').map(str::to_owned).collect()
    };
    assert_eq!(filtered.keys().cloned().collect::<Vec<_>>(), names);
    for (name, config) in filtered {
        assert_eq!(config, original[&name]);
    }
    assert_eq!(servers, original);
}
#[derive(Clone, Copy)]
enum GateInput {
    Required,
    MayExitEarly,
}

fn gate(
    w: &mut PolicyWorld,
    tool: &str,
    policy: Option<String>,
    payload: &str,
    input: GateInput,
    wait_before_write: bool,
) {
    // The hook must not see developer credentials, telemetry or repository state.
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    command
        .args(["internal", "jcode-pre-tool"])
        .env_clear()
        .env("CRUISE_UI_LANGUAGE", "en")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .current_dir(root.path())
        .env("JCODE_HOOK_TOOL_NAME", tool)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(policy) = policy {
        command.env("CRUISE_JCODE_SHELL_DENY", policy);
    }
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let may_exit_early = matches!(input, GateInput::MayExitEarly);
    if wait_before_write {
        // Keep the writer open; bound the wait in case the hook starts reading stdin.
        assert!(
            may_exit_early,
            "cannot force exit for an input-reading fixture"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                drop(stdin);
                let _ = child.kill();
                let output = child.wait_with_output().unwrap();
                panic!("gate did not exit before stdin was written: {output:?}");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let write = stdin.write_all(payload.as_bytes());
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    let broken_pipe = write.as_ref().err().map(std::io::Error::kind) == Some(ErrorKind::BrokenPipe);
    if wait_before_write {
        assert!(
            broken_pipe,
            "expected BrokenPipe after child exit: {write:?}"
        );
    }
    if !(may_exit_early && broken_pipe) {
        write.unwrap();
    }
    w.output = Some(output);
}
#[when(expr = "the real bash gate inspects {string}")]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Cucumber captures owned strings"
)]
fn inspect(w: &mut PolicyWorld, script: String) {
    gate(
        w,
        "bash",
        Some(serde_json::to_string(&w.policy.shell_rules).unwrap()),
        &serde_json::json!({"command":script}).to_string(),
        GateInput::Required,
        false,
    );
}
#[when(expr = "the real gate receives malformed case {word}")]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Cucumber captures owned strings"
)]
fn malformed(w: &mut PolicyWorld, case: String) {
    malformed_gate(w, &case, false);
}

#[when(expr = "the real gate exits before stdin is written for malformed case {word}")]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Cucumber captures owned strings"
)]
fn malformed_exit_first(w: &mut PolicyWorld, case: String) {
    malformed_gate(w, &case, true);
}

fn malformed_gate(w: &mut PolicyWorld, case: &str, wait_before_write: bool) {
    use GateInput::{MayExitEarly, Required};

    let valid = serde_json::to_string(&w.policy.shell_rules).unwrap();
    let (tool, policy, payload, input) = match case {
        "policy-json" => ("bash", Some("{".to_owned()), "{}", MayExitEarly),
        "policy-prefix" => (
            "bash",
            Some(r#"[{"prefix":"git;push","source":"AllowCommit"}]"#.to_owned()),
            "{}",
            MayExitEarly,
        ),
        "input-json" => ("bash", Some(valid), "{", Required),
        "missing-command" => ("bash", Some(valid), "{}", Required),
        "nonstring-command" => ("bash", Some(valid), r#"{"command":42}"#, Required),
        "nonbash" => ("read", Some("{".to_owned()), "{", MayExitEarly),
        "absent-policy" => ("bash", None, "{", MayExitEarly),
        _ => panic!("unknown malformed fixture {case}"),
    };
    gate(w, tool, policy, payload, input, wait_before_write);
}
#[then(expr = "the gate exits {int} with diagnostic {string}")]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Cucumber captures owned strings"
)]
fn result(w: &mut PolicyWorld, code: i32, diagnostic: String) {
    let output = w.output.as_ref().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(code), "{stderr}");
    assert_eq!(output.stdout, Vec::<u8>::new());
    if diagnostic == "none" {
        assert!(stderr.is_empty(), "{stderr}");
    } else {
        assert!(stderr.contains(&diagnostic), "{stderr}");
    }
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    use cucumber::writer::Stats as _;
    // cucumber 0.23 has no separate pending/undefined switch. Undefined steps
    // are skipped, and fail_on_skipped promotes them to failed steps.
    let writer = PolicyWorld::cucumber()
        .fail_on_skipped()
        .run(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/features/tool_restrictions.feature"
        ))
        .await;
    assert!(
        !writer.execution_has_failed(),
        "{} step failed, {} parsing errors, {} hook errors",
        writer.failed_steps(),
        writer.parsing_errors(),
        writer.hook_errors()
    );
    assert!(
        writer.passed_steps() + writer.failed_steps() > 0,
        "no tool restriction scenarios executed"
    );
}
