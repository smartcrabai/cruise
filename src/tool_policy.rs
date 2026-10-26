//! SDK-neutral, per-prompt tool restriction contracts.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::config::{McpServers, PermissionMode};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum ToolRestriction {
    Web,
    Subagent,
    Mcp,
    McpServer(String),
    ShellPrefix(String),
}

impl TryFrom<String> for ToolRestriction {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let error = || {
            "expected web, subagent, mcp, mcp:<name>, or shell:<prefix> (ASCII command words only)"
                .to_owned()
        };
        match value.as_str() {
            "web" => Ok(Self::Web),
            "subagent" => Ok(Self::Subagent),
            "mcp" => Ok(Self::Mcp),
            _ => {
                if let Some(name) = value.strip_prefix("mcp:")
                    && !name.is_empty()
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                {
                    return Ok(Self::McpServer(name.into()));
                }
                if let Some(prefix) = value.strip_prefix("shell:")
                    && prefix
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b" ._/:=@+%-".contains(&b))
                {
                    let prefix = prefix.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !prefix.is_empty() {
                        return Ok(Self::ShellPrefix(prefix));
                    }
                }
                Err(error())
            }
        }
    }
}

impl From<ToolRestriction> for String {
    fn from(value: ToolRestriction) -> Self {
        match value {
            ToolRestriction::Web => "web".into(),
            ToolRestriction::Subagent => "subagent".into(),
            ToolRestriction::Mcp => "mcp".into(),
            ToolRestriction::McpServer(name) => format!("mcp:{name}"),
            ToolRestriction::ShellPrefix(prefix) => format!("shell:{prefix}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleSource {
    DisallowedTools,
    AllowCommit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellRule {
    pub prefix: String,
    pub source: RuleSource,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolPolicy {
    pub deny_web: bool,
    pub deny_subagent: bool,
    pub deny_all_mcp: bool,
    pub denied_mcp_servers: BTreeSet<String>,
    pub shell_rules: Vec<ShellRule>,
}

impl ToolPolicy {
    #[must_use]
    pub fn for_prompt_step(
        workflow: &[ToolRestriction],
        step: &[ToolRestriction],
        allow_commit: bool,
        permission: PermissionMode,
    ) -> Self {
        let mut policy = Self::default();
        for restriction in workflow.iter().chain(step) {
            match restriction {
                ToolRestriction::Web => policy.deny_web = true,
                ToolRestriction::Subagent => policy.deny_subagent = true,
                ToolRestriction::Mcp => policy.deny_all_mcp = true,
                ToolRestriction::McpServer(name) => {
                    policy.denied_mcp_servers.insert(name.clone());
                }
                ToolRestriction::ShellPrefix(prefix) if permission == PermissionMode::Full => {
                    if !policy.shell_rules.iter().any(|r| r.prefix == *prefix) {
                        policy.shell_rules.push(ShellRule {
                            prefix: prefix.clone(),
                            source: RuleSource::DisallowedTools,
                        });
                    }
                }
                ToolRestriction::ShellPrefix(_) => {}
            }
        }
        if !allow_commit && permission == PermissionMode::Full {
            for prefix in IMPLICIT_ALLOW_COMMIT_SHELL_DENIES {
                if !policy.shell_rules.iter().any(|r| r.prefix == *prefix) {
                    policy.shell_rules.push(ShellRule {
                        prefix: (*prefix).into(),
                        source: RuleSource::AllowCommit,
                    });
                }
            }
        }
        policy
    }

    #[must_use]
    pub fn has_explicit(&self) -> bool {
        self.deny_web
            || self.deny_subagent
            || self.deny_all_mcp
            || !self.denied_mcp_servers.is_empty()
            || self
                .shell_rules
                .iter()
                .any(|r| r.source == RuleSource::DisallowedTools)
    }

    #[must_use]
    pub fn filter_mcp_servers(&self, servers: &McpServers) -> McpServers {
        servers
            .iter()
            .filter(|(name, _)| !self.deny_all_mcp && !self.denied_mcp_servers.contains(*name))
            .map(|(name, config)| (name.clone(), config.clone()))
            .collect()
    }

    #[must_use]
    pub fn prompt_notice(&self) -> Option<String> {
        let mut labels = Vec::new();
        if self.deny_web {
            labels.push("web".to_owned());
        }
        if self.deny_subagent {
            labels.push("subagent".to_owned());
        }
        if self.deny_all_mcp {
            labels.push("mcp".to_owned());
        }
        labels.extend(self.denied_mcp_servers.iter().map(|n| format!("mcp:{n}")));
        labels.extend(
            self.shell_rules
                .iter()
                .filter(|r| r.source == RuleSource::DisallowedTools)
                .map(|r| format!("shell:{}", r.prefix)),
        );
        (!labels.is_empty()).then(|| {
            format!(
                "Tool restrictions for this step (disallowed_tools): {}.",
                labels.join(", ")
            )
        })
    }
}

pub const IMPLICIT_ALLOW_COMMIT_SHELL_DENIES: &[&str] = &[
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
];

/// Consume the contents of a command substitution, respecting nested quoting.
fn substitution_body(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    backtick: bool,
) -> String {
    let mut inner = String::new();
    let mut depth = 1;
    let mut inner_quote = None;
    while let Some(next) = chars.next() {
        if next == '\\' {
            inner.push(next);
            if let Some(escaped) = chars.next() {
                inner.push(escaped);
            }
            continue;
        }
        if backtick && next == '`' {
            break;
        }
        if !backtick {
            if inner_quote == Some(next) {
                inner_quote = None;
            } else if inner_quote.is_none() && matches!(next, '\'' | '"') {
                inner_quote = Some(next);
            } else if inner_quote.is_none() {
                if next == '(' {
                    depth += 1;
                }
                if next == ')' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
        }
        inner.push(next);
    }
    inner
}

/// Lex shell words, retaining quoted arguments but separating executable command lists.
/// This is deliberately not a shell interpreter (in particular, heredocs may overmatch).
fn shell_commands(script: &str) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote = None;
    let mut chars = script.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && quote != Some('\'') {
            if let Some(next) = chars.next()
                && next != '\n'
            {
                word.push(next);
                started = true;
            }
            continue;
        }
        if quote == Some('\'') {
            if c == '\'' {
                quote = None;
            } else {
                word.push(c);
            }
            continue;
        }
        if (c == '$' && chars.peek() == Some(&'(')) || c == '`' {
            let backtick = c == '`';
            if !backtick {
                chars.next();
            }
            let inner = substitution_body(&mut chars, backtick);
            commands.extend(shell_commands(&inner));
            started = true;
            continue;
        }
        if c == '"' {
            quote = if quote == Some('"') { None } else { Some('"') };
            started = true;
        } else if quote == Some('"') {
            word.push(c);
        } else if c == '\'' {
            quote = Some(c);
            started = true;
        } else if c == '#' && !started {
            for next in chars.by_ref() {
                if next == '\n' {
                    break;
                }
            }
            if !words.is_empty() {
                commands.push(std::mem::take(&mut words));
            }
        } else if c.is_whitespace() || "&|;(){}".contains(c) {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
            if (c == '\n' || "&|;(){}".contains(c)) && !words.is_empty() {
                commands.push(std::mem::take(&mut words));
            }
        } else {
            word.push(c);
            started = true;
        }
    }
    if started {
        words.push(word);
    }
    if !words.is_empty() {
        commands.push(words);
    }
    commands
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name
                .bytes()
                .enumerate()
                .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
    })
}

/// Whether a wrapper option consumes a separate argument (attached values are already included).
fn wrapper_option_takes_value(wrapper: &str, option: &str) -> bool {
    match wrapper {
        "env" => matches!(
            option,
            "-u" | "--unset" | "-C" | "--chdir" | "-S" | "--split-string" | "-P"
        ),
        "exec" => option == "-a",
        "nice" => matches!(option, "-n" | "--adjustment"),
        "time" => matches!(option, "-f" | "--format" | "-o" | "--output"),
        "timeout" => matches!(option, "-s" | "--signal" | "-k" | "--kill-after"),
        "stdbuf" => matches!(
            option,
            "-i" | "-o" | "-e" | "--input" | "--output" | "--error"
        ),
        "xargs" => {
            matches!(
                option,
                "-a" | "--arg-file"
                    | "-d"
                    | "--delimiter"
                    | "-E"
                    | "-I"
                    | "-J"
                    | "-L"
                    | "-l"
                    | "-n"
                    | "-P"
                    | "-R"
                    | "-S"
                    | "-s"
                    | "--eof"
                    | "--replace"
                    | "--max-lines"
                    | "--max-args"
                    | "--max-procs"
                    | "--max-chars"
                    | "--process-slot-var"
            ) || (!option.starts_with("--")
                && option[1..]
                    .chars()
                    .last()
                    .is_some_and(|c| "adEIJLnPRSs".contains(c)))
        }
        _ => false,
    }
}

/// Return the wrapped executable, consuming option arguments rather than mistaking them for commands.
fn unwrap_command(mut words: &[String]) -> Vec<String> {
    loop {
        while words.first().is_some_and(|w| assignment(w)) {
            words = &words[1..];
        }
        let Some(first) = words.first() else {
            return words.to_vec();
        };
        let wrapper = basename(first);
        if !matches!(
            wrapper,
            "env"
                | "command"
                | "builtin"
                | "exec"
                | "nohup"
                | "nice"
                | "time"
                | "timeout"
                | "stdbuf"
                | "xargs"
        ) {
            return words.to_vec();
        }
        words = &words[1..];
        while let Some(option) = words.first() {
            if option == "--" {
                words = &words[1..];
                break;
            }
            if wrapper == "env" && (assignment(option) || option == "-") {
                words = &words[1..];
                continue;
            }
            if wrapper == "env"
                && (option == "-S"
                    || option == "--split-string"
                    || option.starts_with("--split-string=")
                    || (option.starts_with("-S") && option.len() > 2))
            {
                let (script, rest) = if let Some(script) = option
                    .strip_prefix("--split-string=")
                    .or_else(|| option.strip_prefix("-S").filter(|s| !s.is_empty()))
                {
                    (script, &words[1..])
                } else if let Some(script) = words.get(1) {
                    (script.as_str(), &words[2..])
                } else {
                    return Vec::new();
                };
                let mut expanded: Vec<String> =
                    shell_commands(script).into_iter().flatten().collect();
                expanded.extend_from_slice(rest);
                return unwrap_command(&expanded);
            }
            if !option.starts_with('-') || option == "-" {
                break;
            }
            let takes_value = wrapper_option_takes_value(wrapper, option);
            words = &words[1..];
            if takes_value && !words.is_empty() {
                words = &words[1..];
            }
        }
        if wrapper == "timeout" && !words.is_empty() {
            words = &words[1..];
        }
    }
}

fn prefix_matches(words: &[&str], prefix: &str) -> bool {
    let prefix = prefix.split_whitespace();
    let prefix_len = prefix.clone().count();
    prefix_len != 0
        && words.len() >= prefix_len
        && prefix.enumerate().all(|(i, p)| {
            let word = if i == 0 { basename(words[i]) } else { words[i] };
            let p = if i == 0 { basename(p) } else { p };
            if i + 1 == prefix_len && p.ends_with('=') {
                word.starts_with(p)
            } else {
                word == p
            }
        })
}

pub(crate) fn shell_command_violation<'a>(
    command: &str,
    rules: &'a [ShellRule],
) -> Option<&'a ShellRule> {
    let mut scripts = vec![command.to_owned()];
    while let Some(script) = scripts.pop() {
        for command in shell_commands(&script) {
            let original: Vec<_> = command
                .iter()
                .skip_while(|word| assignment(word))
                .map(String::as_str)
                .collect();
            let words = unwrap_command(&command);
            let raw: Vec<_> = words.iter().map(String::as_str).collect();
            let mut normalized = Vec::new();
            let mut git_config_candidates = Vec::new();
            if words.first().is_some_and(|first| basename(first) == "git") {
                let mut i = 1;
                while i < raw.len() && raw[i].starts_with('-') {
                    if raw[i] == "-c" && i + 1 < raw.len() {
                        let mut candidate = vec![raw[0]];
                        candidate.extend_from_slice(&raw[i..]);
                        git_config_candidates.push(candidate);
                    }
                    let takes_value = matches!(
                        raw[i],
                        "-C" | "-c"
                            | "--git-dir"
                            | "--work-tree"
                            | "--namespace"
                            | "--config-env"
                            | "--super-prefix"
                            | "--exec-path"
                    );
                    i += 1;
                    if takes_value && i < raw.len() {
                        i += 1;
                    }
                }
                normalized = vec![raw[0]];
                normalized.extend_from_slice(&raw[i..]);
            }
            if let Some(rule) = rules.iter().find(|rule| {
                (rule.source == RuleSource::DisallowedTools
                    && prefix_matches(&original, &rule.prefix))
                    || prefix_matches(&raw, &rule.prefix)
                    || prefix_matches(&normalized, &rule.prefix)
                    || git_config_candidates
                        .iter()
                        .any(|candidate| prefix_matches(candidate, &rule.prefix))
            }) {
                return Some(rule);
            }
            let Some(first) = words.first() else {
                continue;
            };
            if matches!(basename(first), "sh" | "bash" | "zsh" | "dash") {
                let mut i = 1;
                while let Some(option) = words.get(i) {
                    if option == "--" || (!option.starts_with('-') && !option.starts_with('+')) {
                        break;
                    }
                    if option.starts_with('-') && !option.starts_with("--") && option.contains('c')
                    {
                        if let Some(script) = words.get(i + 1) {
                            scripts.push(script.clone());
                        }
                        break;
                    }
                    if matches!(
                        option.as_str(),
                        "-o" | "+o" | "-O" | "+O" | "--rcfile" | "--init-file"
                    ) {
                        i += 1;
                    }
                    i += 1;
                }
            }
        }
    }
    None
}

/// jcode permits exit 0, blocks exit 2, and fails open for any other exit status.
pub(crate) fn run_jcode_pre_tool_gate() -> i32 {
    if std::env::var("JCODE_HOOK_TOOL_NAME").as_deref() != Ok("bash") {
        return 0;
    }
    let policy = match std::env::var("CRUISE_JCODE_SHELL_DENY") {
        Ok(policy) => policy,
        Err(std::env::VarError::NotPresent) => return 0,
        Err(error) => {
            eprintln!("cruise: invalid shell deny policy: {error}");
            return 2;
        }
    };
    let rules: Vec<ShellRule> = match serde_json::from_str(&policy) {
        Ok(rules) => rules,
        Err(error) => {
            eprintln!("cruise: invalid shell deny policy: {error}");
            return 2;
        }
    };
    if rules
        .iter()
        .any(|r| ToolRestriction::try_from(format!("shell:{}", r.prefix)).is_err())
    {
        eprintln!("cruise: invalid shell deny prefix");
        return 2;
    }
    let input: serde_json::Value = match serde_json::from_reader(std::io::stdin().lock()) {
        Ok(input) => input,
        Err(error) => {
            eprintln!("cruise: cannot read bash command: {error}");
            return 2;
        }
    };
    let Some(command) = input.get("command").and_then(serde_json::Value::as_str) else {
        eprintln!("cruise: cannot read bash command from hook input");
        return 2;
    };
    if let Some(rule) = shell_command_violation(command, &rules) {
        let reason = match rule.source {
            RuleSource::AllowCommit => "(allow_commit is false)",
            RuleSource::DisallowedTools => "by disallowed_tools",
        };
        eprintln!("cruise: `{}` is denied in this step {reason}", rule.prefix);
        return 2;
    }
    0
}

#[cfg(test)]
mod wrapper_regressions {
    use super::*;

    #[test]
    fn env_search_path_preserves_explicit_and_implicit_git_denials() {
        for explicit in [false, true] {
            let restrictions = if explicit {
                vec![ToolRestriction::ShellPrefix("git push".into())]
            } else {
                vec![]
            };
            let policy =
                ToolPolicy::for_prompt_step(&restrictions, &[], explicit, PermissionMode::Full);
            let rule = shell_command_violation("env -P /usr/bin git push", &policy.shell_rules)
                .unwrap_or_else(|| panic!("env -P bypassed policy"));
            assert_eq!(
                rule.source,
                if explicit {
                    RuleSource::DisallowedTools
                } else {
                    RuleSource::AllowCommit
                }
            );
            assert!(
                shell_command_violation("env -P /usr/bin git status", &policy.shell_rules)
                    .is_none()
            );
        }
    }

    #[test]
    fn explicit_wrapper_prefixes_survive_unwrapping_in_rule_order() {
        for wrapper in ["env", "command"] {
            for allow_commit in [true, false] {
                let policy = ToolPolicy::for_prompt_step(
                    &[ToolRestriction::ShellPrefix(wrapper.into())],
                    &[],
                    allow_commit,
                    PermissionMode::Full,
                );
                for command in [
                    format!("KEY=value {wrapper} printf POLICY_BYPASS"),
                    format!("{wrapper} git push"),
                    wrapper.to_owned(),
                ] {
                    let rule = shell_command_violation(&command, &policy.shell_rules)
                        .unwrap_or_else(|| panic!("allowed {command}"));
                    assert_eq!(rule.prefix, wrapper);
                    assert_eq!(rule.source, RuleSource::DisallowedTools);
                }
                assert!(shell_command_violation("printf ok", &policy.shell_rules).is_none());
            }
        }
    }

    #[test]
    fn wrapper_option_arguments_and_nested_substitutions_are_inspected() {
        let rules = vec![ShellRule {
            prefix: "git push".into(),
            source: RuleSource::AllowCommit,
        }];
        for command in [
            "env --unset HOME --chdir /repo KEY=value git push",
            "env - git push",
            "env -S 'git push'",
            "env --split-string='git push' origin",
            "env -Sgit push",
            "exec -a ignored git push",
            "time -f ignored -o /tmp/timing git push",
            "timeout -s TERM -k 2 5 git push",
            "stdbuf -o L -e 0 git push",
            "xargs -r -n 1 -P 2 git push",
            "xargs -rn 1 git push",
            "xargs -J '{}' -R 2 git push",
            "bash -lc 'git push'",
            "bash -o pipefail -c 'git push'",
            "echo \"$(git push)\"",
            "echo $(echo $(git push))",
            "echo \"`git push`\"",
            "git pu\\sh",
            "git 'push'",
        ] {
            assert!(
                shell_command_violation(command, &rules).is_some(),
                "{command}"
            );
        }
        for command in [
            "echo 'env -S git push'",
            "echo ignored # git push",
            "echo \\$(git status)",
            "echo \"\\$(git push)\"",
            "git log --grep='git push'",
        ] {
            assert!(
                shell_command_violation(command, &rules).is_none(),
                "{command}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::McpServerConfig;

    fn parse(value: &str) -> ToolRestriction {
        ToolRestriction::try_from(value.to_owned())
            .unwrap_or_else(|error| panic!("{value:?}: {error}"))
    }

    fn rule(prefix: &str, source: RuleSource) -> ShellRule {
        ShellRule {
            prefix: prefix.into(),
            source,
        }
    }

    // Keep the expected list independent of production constants.
    fn implicit_prefixes() -> Vec<&'static str> {
        vec![
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
    }

    #[test]
    fn parsing_normalizes_and_round_trips_supported_vocabulary() {
        let cases = [
            ("web", ToolRestriction::Web, "web"),
            ("subagent", ToolRestriction::Subagent, "subagent"),
            ("mcp", ToolRestriction::Mcp, "mcp"),
            (
                "mcp:github",
                ToolRestriction::McpServer("github".into()),
                "mcp:github",
            ),
            (
                "shell:  gh   pr  merge  ",
                ToolRestriction::ShellPrefix("gh pr merge".into()),
                "shell:gh pr merge",
            ),
            (
                "shell:git -c core.hooksPath=",
                ToolRestriction::ShellPrefix("git -c core.hooksPath=".into()),
                "shell:git -c core.hooksPath=",
            ),
            (
                "shell:/usr/bin/npm publish --tag=next @scope/pkg+test_1.0%20",
                ToolRestriction::ShellPrefix(
                    "/usr/bin/npm publish --tag=next @scope/pkg+test_1.0%20".into(),
                ),
                "shell:/usr/bin/npm publish --tag=next @scope/pkg+test_1.0%20",
            ),
        ];
        for (input, expected, canonical) in cases {
            assert_eq!(parse(input), expected);
            assert_eq!(String::from(expected.clone()), canonical);
            let json = serde_json::to_string(&expected).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(json, format!("\"{canonical}\""));
            assert_eq!(
                serde_json::from_str::<ToolRestriction>(&json).unwrap_or_else(|e| panic!("{e}")),
                expected
            );
        }
    }

    #[test]
    fn parsing_rejects_unknown_empty_and_injection_values_with_vocabulary_help() {
        for input in [
            "",
            "Web",
            "shell",
            "edit",
            "bash:git",
            "unknown",
            "mcp:",
            "shell:",
            "shell:   ",
        ] {
            let error = match ToolRestriction::try_from(input.to_owned()) {
                Err(error) => error,
                Ok(value) => panic!("accepted {input:?} as {value:?}"),
            };
            for vocabulary in ["web", "subagent", "mcp", "shell:"] {
                assert!(
                    error.contains(vocabulary),
                    "{input:?}: missing {vocabulary:?} in {error:?}"
                );
            }
        }
        for forbidden in [
            '*', '?', '(', ')', ',', '"', '\'', '`', '$', ';', '|', '&', '<', '>', '\\', '\n',
            '\r', '\t', '\0', '\u{7f}', 'é',
        ] {
            let input = format!("shell:git {forbidden}commit");
            assert!(
                ToolRestriction::try_from(input.clone()).is_err(),
                "accepted {input:?}"
            );
            assert!(serde_json::from_value::<ToolRestriction>(serde_json::json!(input)).is_err());
        }
        for value in [
            serde_json::json!(null),
            serde_json::json!(1),
            serde_json::json!({"web":true}),
        ] {
            assert!(serde_json::from_value::<ToolRestriction>(value).is_err());
        }
    }

    #[test]
    fn effective_policy_unions_workflow_and_step_and_deduplicates() {
        use ToolRestriction::{McpServer, ShellPrefix, Subagent, Web};
        let workflow = vec![
            Web,
            McpServer("github".into()),
            ShellPrefix("npm publish".into()),
        ];
        let step = vec![
            Web,
            Subagent,
            McpServer("github".into()),
            McpServer("docs".into()),
            ShellPrefix("npm publish".into()),
        ];
        let policy = ToolPolicy::for_prompt_step(&workflow, &step, true, PermissionMode::Full);
        assert!(policy.deny_web && policy.deny_subagent);
        assert!(!policy.deny_all_mcp);
        assert_eq!(
            policy.denied_mcp_servers,
            BTreeSet::from(["docs".into(), "github".into()])
        );
        assert_eq!(
            policy.shell_rules,
            vec![rule("npm publish", RuleSource::DisallowedTools)]
        );
        assert!(policy.has_explicit());
        let inherited = ToolPolicy::for_prompt_step(&workflow, &[], true, PermissionMode::Full);
        assert!(inherited.deny_web);
        assert_eq!(inherited.shell_rules, policy.shell_rules);
    }

    #[test]
    fn allow_commit_controls_only_implicit_rules_and_preserves_explicit_denials() {
        let implicit = ToolPolicy::for_prompt_step(&[], &[], false, PermissionMode::Full);
        let actual: BTreeSet<_> = implicit
            .shell_rules
            .iter()
            .map(|r| r.prefix.as_str())
            .collect();
        assert_eq!(actual, implicit_prefixes().into_iter().collect());
        assert_eq!(implicit.shell_rules.len(), actual.len());
        assert!(
            implicit
                .shell_rules
                .iter()
                .all(|r| r.source == RuleSource::AllowCommit)
        );
        assert!(!implicit.has_explicit());
        assert_eq!(
            ToolPolicy::for_prompt_step(&[], &[], true, PermissionMode::Full),
            ToolPolicy::default()
        );
        let explicit = [ToolRestriction::ShellPrefix("git commit".into())];
        for allow_commit in [true, false] {
            let policy =
                ToolPolicy::for_prompt_step(&[], &explicit, allow_commit, PermissionMode::Full);
            let matching: Vec<_> = policy
                .shell_rules
                .iter()
                .filter(|r| r.prefix == "git commit")
                .collect();
            assert_eq!(matching.len(), 1);
            assert_eq!(matching[0].source, RuleSource::DisallowedTools);
            assert!(policy.has_explicit());
        }
    }

    #[test]
    fn restricted_permissions_drop_all_shell_rules_without_widening_permission() {
        let restrictions = [
            ToolRestriction::ShellPrefix("git commit".into()),
            ToolRestriction::Web,
        ];
        for permission in [PermissionMode::ReadOnly, PermissionMode::Edit] {
            for allow_commit in [true, false] {
                let policy =
                    ToolPolicy::for_prompt_step(&restrictions, &[], allow_commit, permission);
                assert_eq!(policy.shell_rules, []);
                assert!(policy.deny_web);
                assert!(policy.has_explicit());
                assert_eq!(
                    ToolPolicy::for_prompt_step(&[], &[], allow_commit, permission).shell_rules,
                    []
                );
            }
        }
    }

    #[test]
    fn mcp_filtering_removes_only_selected_workflow_servers_and_preserves_values() {
        let servers: McpServers = ["github", "docs", "local"]
            .into_iter()
            .map(|name| {
                (
                    name.into(),
                    McpServerConfig {
                        command: Some(format!("serve-{name}")),
                        ..Default::default()
                    },
                )
            })
            .collect();
        assert_eq!(ToolPolicy::default().filter_mcp_servers(&servers), servers);
        let policy = ToolPolicy::for_prompt_step(
            &[ToolRestriction::McpServer("docs".into())],
            &[],
            true,
            PermissionMode::Full,
        );
        let filtered = policy.filter_mcp_servers(&servers);
        assert_eq!(
            filtered.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["github", "local"])
        );
        assert_eq!(filtered.get("github"), servers.get("github"));
        assert_eq!(filtered.get("local"), servers.get("local"));
        assert_eq!(servers.len(), 3);
        let all =
            ToolPolicy::for_prompt_step(&[], &[ToolRestriction::Mcp], true, PermissionMode::Full);
        assert!(all.deny_all_mcp);
        assert!(all.filter_mcp_servers(&servers).is_empty());
    }

    #[test]
    fn notices_and_explicit_detection_exclude_implicit_rules() {
        assert_eq!(ToolPolicy::default().prompt_notice(), None);
        let implicit = ToolPolicy::for_prompt_step(&[], &[], false, PermissionMode::Full);
        assert_eq!(implicit.prompt_notice(), None);
        assert!(!implicit.has_explicit());
        for restriction in [
            ToolRestriction::Web,
            ToolRestriction::Subagent,
            ToolRestriction::Mcp,
            ToolRestriction::McpServer("github".into()),
            ToolRestriction::ShellPrefix("npm publish".into()),
        ] {
            let label = match &restriction {
                ToolRestriction::Web => "web",
                ToolRestriction::Subagent => "subagent",
                ToolRestriction::Mcp => "mcp",
                ToolRestriction::McpServer(_) => "github",
                ToolRestriction::ShellPrefix(_) => "npm publish",
            };
            let policy =
                ToolPolicy::for_prompt_step(&[], &[restriction], false, PermissionMode::Full);
            assert!(policy.has_explicit());
            let notice = policy
                .prompt_notice()
                .unwrap_or_else(|| panic!("missing notice"));
            assert!(notice.contains(label), "{notice}");
            assert!(
                !notice.contains("git commit"),
                "implicit rule leaked: {notice}"
            );
        }
    }

    fn assert_implicit_match(command: &str, expected: Option<&str>) {
        let rules: Vec<_> = implicit_prefixes()
            .into_iter()
            .map(|p| rule(p, RuleSource::AllowCommit))
            .collect();
        let matched = shell_command_violation(command, &rules);
        assert_eq!(
            matched.map(|r| r.prefix.as_str()),
            expected,
            "command: {command}"
        );
    }

    #[test]
    fn matcher_named_positive_cases() {
        for (command, prefix) in [
            ("a && git commit -m x", "git commit"),
            ("FOO=1 git push", "git push"),
            ("timeout 5 git pull", "git pull"),
            ("git -C . commit", "git commit"),
            ("git --no-optional-locks commit", "git commit"),
            (
                "git -c core.hooksPath=/dev/null status",
                "git -c core.hooksPath=",
            ),
            ("sh -c 'git push'", "git push"),
            ("/usr/bin/git reset --hard", "git reset"),
            ("git checkout -b x", "git checkout -b"),
            ("git checkout -B x", "git checkout -B"),
            ("git reset HEAD file", "git reset"),
            (
                "git config core.hooksPath /dev/null",
                "git config core.hooksPath",
            ),
            ("git update-ref refs/heads/x HEAD", "git update-ref"),
        ] {
            assert_implicit_match(command, Some(prefix));
        }
        for prefix in implicit_prefixes() {
            let command = if prefix.ends_with('=') {
                format!("{prefix}/dev/null status")
            } else {
                format!("{prefix} argument")
            };
            assert_implicit_match(&command, Some(prefix));
        }
    }

    #[test]
    fn matcher_named_negative_cases_and_word_boundaries() {
        for command in [
            "git checkout -- f",
            "git stash create",
            "echo \"git commit\"",
            "git log --grep=\"git push\"",
            "git commit-tree",
            "git status",
            "git diff",
            "git log",
            "git branch",
            "git checkout existing",
            "git restore --staged f",
            "echo 'git push; git commit'",
            "printf '%s' 'git push'",
            "git config core.hooksPathSuffix x",
        ] {
            assert_implicit_match(command, None);
        }
        assert!(shell_command_violation("git commit", &[]).is_none());
        assert_implicit_match("", None);
    }

    #[test]
    fn matcher_unwraps_assignments_wrappers_and_recursive_shells() {
        for command in [
            "env FOO=1 git push",
            "env -i -u HOME FOO=1 git push",
            "command -- git push",
            "builtin git push",
            "exec git push",
            "nohup git push",
            "nice -n 10 git push",
            "time git push",
            "timeout --signal=TERM 5 git push",
            "stdbuf -oL git push",
            "xargs -n 1 git push",
            "FOO=1 env BAR=2 command nohup nice -n 3 git push",
            "bash -c 'git push'",
            "zsh -c 'git push'",
            "dash -c 'git push'",
            "sh -c \"bash -c 'git push'\"",
        ] {
            assert_implicit_match(command, Some("git push"));
        }
    }

    #[test]
    fn matcher_splits_control_operators_and_substitutions_but_respects_quotes() {
        for command in [
            "true || git push",
            "echo ok; git push",
            "echo ok | git push",
            "true & git push",
            "echo ok\ngit push",
            "(git push)",
            "{ git push; }",
            "echo $(git push)",
            "echo `git push`",
        ] {
            assert_implicit_match(command, Some("git push"));
        }
        for command in [
            "echo 'a && git push'",
            "echo \"x | git push\"",
            "echo '$(git push)'",
            "echo '`git push`'",
        ] {
            assert_implicit_match(command, None);
        }
    }

    #[test]
    fn matcher_normalizes_git_global_options_and_retains_raw_hooks_path_match() {
        for command in [
            "git -C /repo -c color.ui=false push",
            "git --git-dir=.git push",
            "git --git-dir .git push",
            "git --work-tree=/repo push",
            "git --work-tree /repo push",
            "git --namespace=ns push",
            "git --namespace ns push",
            "git --no-pager push",
            "git -P push",
            "git --no-optional-locks push",
        ] {
            assert_implicit_match(command, Some("git push"));
        }
        assert_implicit_match(
            "git -c core.hooksPath=/tmp/hooks status",
            Some("git -c core.hooksPath="),
        );
        assert_implicit_match("git -c color.ui=false status", None);
    }

    #[test]
    fn matcher_retains_each_git_config_operand_for_both_policy_sources() {
        for explicit in [false, true] {
            let restrictions = if explicit {
                vec![parse("shell:git -c core.hooksPath=")]
            } else {
                vec![]
            };
            let policy =
                ToolPolicy::for_prompt_step(&restrictions, &[], explicit, PermissionMode::Full);
            for command in [
                "git -C . -c core.hooksPath=/dev/null status",
                "git -c color.ui=false -c core.hooksPath=/dev/null status",
                "env /usr/bin/git -C . -c color.ui=false -c core.hooksPath=/dev/null status",
            ] {
                let matched = shell_command_violation(command, &policy.shell_rules)
                    .unwrap_or_else(|| panic!("allowed {command}"));
                assert_eq!(matched.prefix, "git -c core.hooksPath=");
                assert_eq!(
                    matched.source,
                    if explicit {
                        RuleSource::DisallowedTools
                    } else {
                        RuleSource::AllowCommit
                    }
                );
            }
            for command in [
                "git -C . -c color.ui=false status",
                "git -c color.ui=false -c advice.statusHints=false status",
                "git -C . -c core.hooksPathSuffix=/dev/null status",
                "git status -c core.hooksPath=/dev/null",
            ] {
                assert!(
                    shell_command_violation(command, &policy.shell_rules).is_none(),
                    "{command}"
                );
            }
        }
    }

    #[test]
    fn git_config_candidates_preserve_ordered_rule_provenance() {
        let hooks = rule("git -c core.hooksPath=", RuleSource::AllowCommit);
        for other in [
            rule("env", RuleSource::DisallowedTools),
            rule("git -C", RuleSource::DisallowedTools),
            rule("git push", RuleSource::DisallowedTools),
            rule("git -c core.hooksPath=", RuleSource::DisallowedTools),
        ] {
            for rules in [[hooks.clone(), other.clone()], [other, hooks.clone()]] {
                assert_eq!(
                    shell_command_violation(
                        "env git -C . -c color.ui=false -c core.hooksPath=/dev/null push",
                        &rules,
                    ),
                    Some(&rules[0]),
                );
            }
        }
    }

    #[test]
    fn matcher_arbitrary_prefixes_and_equals_suffix_only() {
        let rules = [
            rule("gh pr merge", RuleSource::DisallowedTools),
            rule("npm publish", RuleSource::DisallowedTools),
            rule("deploy target=", RuleSource::DisallowedTools),
        ];
        for (command, expected) in [
            ("gh pr merge 123", Some(0)),
            ("/usr/bin/npm publish --tag next", Some(1)),
            ("deploy target=prod", Some(2)),
            ("gh pr merger", None),
            ("npm publisher", None),
            ("echo npm publish", None),
            ("deploy target", None),
        ] {
            assert_eq!(
                shell_command_violation(command, &rules),
                expected.map(|index| &rules[index]),
                "{command}"
            );
        }
    }

    #[test]
    fn prefix_matching_retains_empty_short_basename_and_equals_boundaries() {
        assert!(!prefix_matches(&[], ""));
        assert!(!prefix_matches(&["git"], " \t "));
        assert!(!prefix_matches(&[], "git"));
        assert!(!prefix_matches(&["git"], "git push"));
        assert!(prefix_matches(&["/usr/bin/git", "push"], "/bin/git push"));
        assert!(prefix_matches(
            &["git", "-c", "core.hooksPath=/dev/null"],
            "git -c core.hooksPath="
        ));
        assert!(!prefix_matches(
            &["git", "-c", "core.hooksPath"],
            "git -c core.hooksPath="
        ));
        assert!(!prefix_matches(
            &["deploy", "target=prod", "now"],
            "deploy target= now"
        ));
    }
}
