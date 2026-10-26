//! The pre-tool hook is an executable contract: jcode treats only exit 2 as a deny.
use std::io::Write;
use std::process::{Command, Output, Stdio};

use cruise::tool_policy::{RuleSource, ShellRule};
use serde_json::json;

fn rules(prefix: &str, source: RuleSource) -> String {
    serde_json::to_string(&vec![ShellRule {
        prefix: prefix.to_string(),
        source,
    }])
    .unwrap_or_else(|error| panic!("serialize gate rules: {error}"))
}

fn gate(tool: &str, policy: Option<&str>, input: &str) -> Output {
    let home = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    command
        .env_clear()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("JCODE_HOOK_TOOL_NAME", tool)
        .args(["internal", "jcode-pre-tool"])
        .current_dir(home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(policy) = policy {
        command.env("CRUISE_JCODE_SHELL_DENY", policy);
    }
    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("spawn pre-tool gate: {error}"));
    child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("piped stdin"))
        .write_all(input.as_bytes())
        .unwrap_or_else(|error| panic!("write hook input: {error}"));
    child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("wait for pre-tool gate: {error}"))
}

fn assert_code(output: &Output, expected: i32) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn pre_tool_gate_allows_unmatched_bash_command() {
    let policy = rules("git commit", RuleSource::AllowCommit);
    assert_code(
        &gate("bash", Some(&policy), r#"{"command":"git status"}"#),
        0,
    );
}

#[test]
fn pre_tool_gate_reports_the_allow_commit_source_when_blocking() {
    let policy = rules("git commit", RuleSource::AllowCommit);
    let output = gate("bash", Some(&policy), r#"{"command":"git commit -m x"}"#);
    assert_code(&output, 2);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("git commit") && stderr.contains("allow_commit"),
        "{stderr}"
    );
}

#[test]
fn pre_tool_gate_reports_explicit_disallowed_tools_when_blocking() {
    let policy = rules("npm publish", RuleSource::DisallowedTools);
    let output = gate(
        "bash",
        Some(&policy),
        r#"{"command":"npm publish --access public"}"#,
    );
    assert_code(&output, 2);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("npm publish") && stderr.contains("disallowed_tools"),
        "{stderr}"
    );
}

#[test]
fn pre_tool_gate_ignores_non_bash_tools_even_with_invalid_policy_and_input() {
    assert_code(&gate("write", Some("invalid JSON"), "not JSON"), 0);
}

#[test]
fn pre_tool_gate_allows_bash_without_a_policy_even_with_invalid_input() {
    assert_code(&gate("bash", None, "not JSON"), 0);
}

#[test]
fn pre_tool_gate_fails_closed_for_malformed_policy() {
    // Prove that exit 2 is from policy validation, not a missing CLI command.
    assert_code(&gate("bash", Some("[]"), r#"{"command":"git status"}"#), 0);
    assert_code(
        &gate("bash", Some("invalid JSON"), r#"{"command":"git status"}"#),
        2,
    );
}

#[test]
fn pre_tool_gate_fails_closed_when_command_cannot_be_read() {
    let policy = rules("git push", RuleSource::AllowCommit);
    assert_code(
        &gate("bash", Some(&policy), r#"{"command":"git status"}"#),
        0,
    );
    for input in ["not JSON", "{}", r#"{"command":42}"#] {
        assert_code(&gate("bash", Some(&policy), input), 2);
    }
}

#[test]
fn pre_tool_gate_applies_matching_inside_wrapped_compound_commands() {
    let policy = rules("git push", RuleSource::AllowCommit);
    let input =
        json!({"command": "echo ready && timeout 5 sh -c '/usr/bin/git -C . push origin HEAD'"});
    let output = gate("bash", Some(&policy), &input.to_string());
    assert_code(&output, 2);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("git push"), "{stderr}");
}

#[test]
fn pre_tool_gate_denies_explicit_wrapper_prefixes_with_allow_commit() {
    use cruise::config::PermissionMode;
    use cruise::tool_policy::{ToolPolicy, ToolRestriction};

    for wrapper in ["env", "command"] {
        let policy = ToolPolicy::for_prompt_step(
            &[ToolRestriction::ShellPrefix(wrapper.into())],
            &[],
            true,
            PermissionMode::Full,
        );
        let policy =
            serde_json::to_string(&policy.shell_rules).unwrap_or_else(|error| panic!("{error}"));
        for command in [
            format!("KEY=value {wrapper} printf POLICY_BYPASS"),
            wrapper.into(),
        ] {
            let output = gate(
                "bash",
                Some(&policy),
                &json!({"command":command}).to_string(),
            );
            assert_code(&output, 2);
            assert!(String::from_utf8_lossy(&output.stderr).contains("disallowed_tools"));
        }
        assert_code(
            &gate("bash", Some(&policy), r#"{"command":"printf ok"}"#),
            0,
        );
    }
}

#[test]
fn pre_tool_gate_inspects_env_search_path_for_both_rule_sources() {
    for source in [RuleSource::DisallowedTools, RuleSource::AllowCommit] {
        let reason = if source == RuleSource::DisallowedTools {
            "disallowed_tools"
        } else {
            "allow_commit"
        };
        let policy = rules("git push", source);
        let output = gate(
            "bash",
            Some(&policy),
            r#"{"command":"env -P /usr/bin git push"}"#,
        );
        assert_code(&output, 2);
        assert!(String::from_utf8_lossy(&output.stderr).contains(reason));
        assert_code(
            &gate(
                "bash",
                Some(&policy),
                r#"{"command":"env -P /usr/bin git status"}"#,
            ),
            0,
        );
    }
}

#[test]
fn pre_tool_gate_retains_git_config_operands_for_both_policy_sources() {
    use cruise::config::PermissionMode;
    use cruise::tool_policy::{ToolPolicy, ToolRestriction};

    for explicit in [false, true] {
        let restrictions = if explicit {
            vec![ToolRestriction::ShellPrefix(
                "git -c core.hooksPath=".into(),
            )]
        } else {
            vec![]
        };
        let policy =
            ToolPolicy::for_prompt_step(&restrictions, &[], explicit, PermissionMode::Full);
        let policy =
            serde_json::to_string(&policy.shell_rules).unwrap_or_else(|error| panic!("{error}"));
        let reason = if explicit {
            "disallowed_tools"
        } else {
            "allow_commit is false"
        };
        for command in [
            "git -C . -c core.hooksPath=/dev/null status",
            "git -c color.ui=false -c core.hooksPath=/dev/null status",
        ] {
            let output = gate(
                "bash",
                Some(&policy),
                &json!({"command":command}).to_string(),
            );
            assert_code(&output, 2);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains("`git -c core.hooksPath=`") && stderr.contains(reason),
                "{stderr}"
            );
        }
        for command in [
            "git -C . -c color.ui=false status",
            "git -c color.ui=false -c advice.statusHints=false status",
        ] {
            assert_code(
                &gate(
                    "bash",
                    Some(&policy),
                    &json!({"command":command}).to_string(),
                ),
                0,
            );
        }
    }
}
