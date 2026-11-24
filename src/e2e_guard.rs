//! Deterministic policy guard for built-in UI E2E steps.
//!
//! `e2e-baseline` records the working tree before a flagged E2E step;
//! `e2e-guard` runs as that step's quality gate. It rejects dependency,
//! lockfile, toolchain, and CI changes and newly introduced E2E frameworks,
//! then stops the session when the step reports blocked capabilities.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;

use crate::error::{CruiseError, Result};
use crate::trusted_git::{
    ensure_no_repository_filters, git_output, git_output_with_index, parse_oid,
    session_artifacts_root, temporary_index_copy, valid_oid,
};

/// Gate exit status that stops the step without a retry (`EX_TEMPFAIL`).
pub(crate) const BLOCKED_EXIT_CODE: i32 = 75;

const STATUS_PREFIX: &str = "E2E_STATUS:";
const BLOCKERS_HEADING: &str = "## Blockers";
const MAX_BLOCKERS_BYTES: usize = 4096;

/// Exact file names of dependency manifests, lockfiles, and toolchain pins.
const DEPENDENCY_FILES: &[&str] = &[
    ".node-version",
    ".npmrc",
    ".nvmrc",
    ".python-version",
    ".ruby-version",
    ".tool-versions",
    ".yarnrc.yml",
    "Cargo.lock",
    "Cargo.toml",
    "Cartfile",
    "Cartfile.resolved",
    "Directory.Packages.props",
    "Gemfile",
    "Gemfile.lock",
    "Package.resolved",
    "Package.swift",
    "Pipfile",
    "Pipfile.lock",
    "Podfile",
    "Podfile.lock",
    "build.gradle",
    "build.gradle.kts",
    "bun.lock",
    "bun.lockb",
    "composer.json",
    "composer.lock",
    "deno.json",
    "deno.jsonc",
    "deno.lock",
    "flake.lock",
    "flake.nix",
    "global.json",
    "go.mod",
    "go.sum",
    "go.work",
    "go.work.sum",
    "gradle-wrapper.properties",
    "libs.versions.toml",
    "mise.toml",
    "mix.exs",
    "mix.lock",
    "npm-shrinkwrap.json",
    "package-lock.json",
    "package.json",
    "packages.lock.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "poetry.lock",
    "pom.xml",
    "pubspec.lock",
    "pubspec.yaml",
    "pyproject.toml",
    "rust-toolchain",
    "rust-toolchain.toml",
    "settings.gradle",
    "settings.gradle.kts",
    "setup.cfg",
    "setup.py",
    "uv.lock",
    "yarn.lock",
];

/// Path prefixes and exact paths of provider-native CI configuration.
const CI_PREFIXES: &[&str] = &[
    ".buildkite/",
    ".circleci/",
    ".forgejo/workflows/",
    ".gitea/workflows/",
    ".github/workflows/",
    ".gitlab/ci/",
    ".woodpecker/",
];
const CI_FILES: &[&str] = &[
    ".drone.yml",
    ".gitlab-ci.yml",
    ".travis.yml",
    ".woodpecker.yml",
    "Jenkinsfile",
    "appveyor.yml",
    "azure-pipelines.yml",
    "bitbucket-pipelines.yml",
    "bitrise.yml",
    "codemagic.yaml",
];

/// Outcome of one guard evaluation.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// No policy violation and the report (when required) is complete.
    Pass,
    /// Retryable: the step must undo forbidden changes or fix its report.
    Violation(String),
    /// Not retryable: required prerequisites are missing.
    Blocked(String),
}

/// Record the working tree before a flagged UI E2E step.
pub(crate) fn record_baseline(cwd: &Path, plan_path: &Path, step: &str) -> Result<()> {
    let artifacts = session_artifacts_root(plan_path)?;
    let tree = worktree_tree(cwd)?;
    crate::artifacts::write(&artifacts, &baseline_name(step)?, &format!("{tree}\n"))
}

/// Compare the working tree with the step's baseline and validate its report.
pub(crate) fn check(
    cwd: &Path,
    plan_path: &Path,
    step: &str,
    report: Option<&str>,
) -> Result<Verdict> {
    let artifacts = session_artifacts_root(plan_path)?;
    let baseline = crate::artifacts::read(&artifacts, &baseline_name(step)?).map_err(|_| {
        guard_error(&format!(
            "no baseline for '{step}'; the e2e-baseline step must run before it"
        ))
    })?;
    let baseline = baseline.trim();
    if !valid_oid(baseline) {
        return Err(guard_error(&format!("baseline for '{step}' is malformed")));
    }
    let current = worktree_tree(cwd)?;
    let violations = violations(cwd, baseline, &current)?;
    if !violations.is_empty() {
        let mut message = String::from(
            "UI E2E policy violation: this step may use only tooling already in the repository. \
             Restore these paths to their state when the step started. Do not add, upgrade, or \
             downgrade dependencies, toolchains, or CI, and do not introduce a new E2E framework. \
             Report each missing capability with its exact required preparation under \
             `## Blockers` and finish with `E2E_STATUS: blocked`.\n",
        );
        for violation in &violations {
            let _ = writeln!(message, "- {violation}");
        }
        return Ok(Verdict::Violation(message));
    }
    let Some(report) = report else {
        return Ok(Verdict::Pass);
    };
    let text = crate::artifacts::read(&artifacts, report)
        .map_err(|error| guard_error(&format!("cannot read report '{report}': {error}")))?;
    Ok(report_verdict(&text, &artifacts.join(report)))
}

fn report_verdict(text: &str, path: &Path) -> Verdict {
    let statuses = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix(STATUS_PREFIX))
        .map(str::trim)
        .collect::<Vec<_>>();
    let status = match statuses.as_slice() {
        [status @ ("passed" | "blocked" | "not-applicable")] => *status,
        _ => {
            return Verdict::Violation(format!(
                "The UI E2E report must contain exactly one line `{STATUS_PREFIX} passed`, \
                 `{STATUS_PREFIX} blocked`, or `{STATUS_PREFIX} not-applicable`; found {}.\n",
                statuses.len()
            ));
        }
    };
    if status != "blocked" {
        return Verdict::Pass;
    }
    let blockers = blockers_section(text);
    if blockers.is_empty() {
        return Verdict::Violation(format!(
            "`{STATUS_PREFIX} blocked` requires a nonempty `{BLOCKERS_HEADING}` section listing \
             each missing capability, why it is missing, and the exact preparation it needs.\n"
        ));
    }
    let mut blockers = blockers.to_string();
    if blockers.len() > MAX_BLOCKERS_BYTES {
        let mut end = MAX_BLOCKERS_BYTES;
        while !blockers.is_char_boundary(end) {
            end -= 1;
        }
        blockers.truncate(end);
        blockers.push_str("\n...");
    }
    Verdict::Blocked(format!(
        "UI E2E blocked: prepare the prerequisites below, then resume the session; to continue \
         without UI E2E, resume with `-f e2e=off`.\nReport: {}\n{blockers}\n",
        path.display()
    ))
}

fn blockers_section(text: &str) -> &str {
    let mut start = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if let Some(begin) = start {
            if trimmed.starts_with("## ")
                || trimmed.starts_with("# ")
                || trimmed.trim_start().starts_with(STATUS_PREFIX)
            {
                return text[begin..offset].trim();
            }
        } else if trimmed == BLOCKERS_HEADING {
            start = Some(offset + line.len());
        }
        offset += line.len();
    }
    start.map_or("", |begin| text[begin..].trim())
}

fn violations(cwd: &Path, baseline: &str, current: &str) -> Result<Vec<String>> {
    let diff = git_output(
        cwd,
        &[
            "diff-tree",
            "-r",
            "-z",
            "--no-renames",
            "--name-status",
            baseline,
            current,
        ],
    )?;
    let mut fields = diff
        .stdout
        .split(|byte| *byte == 0)
        .filter(|f| !f.is_empty());
    let mut changes = Vec::new();
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        changes.push((
            String::from_utf8_lossy(status).into_owned(),
            String::from_utf8_lossy(path).into_owned(),
        ));
    }
    if changes.is_empty() {
        return Ok(Vec::new());
    }
    let existing = git_output(cwd, &["ls-tree", "-r", "-z", "--name-only", baseline])?;
    let existing_frameworks = existing
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|path| framework(&String::from_utf8_lossy(path)))
        .collect::<HashSet<_>>();
    let mut found = Vec::new();
    for (status, path) in changes {
        if is_dependency_file(&path) {
            found.push(format!(
                "{status} {path} (dependency manifest, lockfile, or toolchain pin)"
            ));
        } else if is_ci_config(&path) {
            found.push(format!("{status} {path} (CI configuration)"));
        } else if status == "A"
            && let Some(name) = framework(&path)
            && !existing_frameworks.contains(name)
        {
            found.push(format!("{status} {path} (new E2E framework: {name})"));
        }
    }
    Ok(found)
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_dependency_file(path: &str) -> bool {
    let name = file_name(path);
    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    DEPENDENCY_FILES.contains(&name)
        || ["csproj", "fsproj", "gemspec"].contains(&extension)
        || (name.starts_with("requirements") && extension == "txt")
}

fn is_ci_config(path: &str) -> bool {
    CI_FILES.contains(&path) || CI_PREFIXES.iter().any(|prefix| path.starts_with(prefix))
}

/// Identify configuration that introduces a UI E2E framework.
fn framework(path: &str) -> Option<&'static str> {
    if path.split('/').any(|component| component == ".maestro") {
        return Some("Maestro");
    }
    let name = file_name(path);
    let families: &[(&str, &[&str])] = &[
        (
            "Playwright",
            &["playwright.config.", "playwright-ct.config."],
        ),
        ("Cypress", &["cypress.config.", "cypress.json"]),
        ("Detox", &[".detoxrc", "detox.config."]),
        ("WebdriverIO", &["wdio.conf.", "wdio."]),
        ("Nightwatch", &["nightwatch.conf.", "nightwatch.json"]),
        ("TestCafe", &[".testcaferc"]),
        ("CodeceptJS", &["codecept.conf."]),
        ("Puppeteer", &[".puppeteerrc"]),
    ];
    families.iter().find_map(|(family, prefixes)| {
        prefixes
            .iter()
            .any(|prefix| name.starts_with(prefix))
            .then_some(*family)
    })
}

fn worktree_tree(cwd: &Path) -> Result<String> {
    ensure_no_repository_filters(cwd)?;
    let index = temporary_index_copy(cwd)?;
    git_output_with_index(cwd, &["add", "--all"], Some(&index.path))?;
    let output = git_output_with_index(cwd, &["write-tree"], Some(&index.path))?;
    parse_oid("git write-tree", &output.stdout)
}

fn baseline_name(step: &str) -> Result<String> {
    if step.is_empty()
        || !step
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(guard_error(&format!("invalid step name '{step}'")));
    }
    Ok(format!("e2e/{step}.baseline"))
}

fn guard_error(message: &str) -> CruiseError {
    CruiseError::Other(format!("UI E2E guard failed: {message}"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::*;

    struct Session {
        // Fields drop in order: restore the environment before releasing the lock.
        _data_home: crate::test_support::EnvGuard,
        _lock: crate::test_support::ProcessLock,
        _temp: tempfile::TempDir,
        repo: PathBuf,
        plan: PathBuf,
        artifacts: PathBuf,
    }

    fn session() -> Session {
        let lock = crate::test_support::lock_process();
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let data_home =
            crate::test_support::EnvGuard::set("XDG_DATA_HOME", temp.path().join("data"));
        let session_dir = crate::paths::data_dir()
            .unwrap_or_else(|error| panic!("data dir: {error}"))
            .join("sessions")
            .join("e2e-session");
        fs::create_dir_all(&session_dir).unwrap_or_else(|error| panic!("session: {error}"));
        let plan = session_dir.join("plan.md");
        fs::write(&plan, "plan").unwrap_or_else(|error| panic!("plan: {error}"));
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|error| panic!("repo: {error}"));
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.name", "Cruise Test"]);
        git(&repo, &["config", "user.email", "cruise@example.invalid"]);
        write(&repo, "package.json", "{\"devDependencies\":{}}\n");
        write(&repo, "src/app.ts", "export const app = 1;\n");
        git(&repo, &["add", "--all"]);
        git(&repo, &["commit", "-q", "-m", "initial"]);
        Session {
            _lock: lock,
            _data_home: data_home,
            _temp: temp,
            artifacts: session_dir.join(crate::artifacts::ARTIFACTS_DIR),
            repo,
            plan,
        }
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env(
                "GIT_CONFIG_GLOBAL",
                crate::trusted_git::DISABLED_GLOBAL_CONFIG,
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap_or_else(|error| panic!("git: {error}"));
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write(repo: &Path, path: &str, content: &str) {
        let path = repo.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|error| panic!("mkdir: {error}"));
        }
        fs::write(path, content).unwrap_or_else(|error| panic!("write: {error}"));
    }

    fn verdict(s: &Session, report: Option<&str>) -> Verdict {
        check(&s.repo, &s.plan, "e2e", report).unwrap_or_else(|error| panic!("check: {error}"))
    }

    fn violation_lines(verdict: Verdict) -> Vec<String> {
        let Verdict::Violation(message) = verdict else {
            panic!("expected a violation, got {verdict:?}");
        };
        message
            .lines()
            .filter_map(|line| line.strip_prefix("- ").map(str::to_string))
            .collect()
    }

    #[test]
    fn rejects_dependency_lockfile_ci_and_new_framework_changes() {
        let s = session();
        record_baseline(&s.repo, &s.plan, "e2e").unwrap_or_else(|error| panic!("{error}"));
        write(
            &s.repo,
            "package.json",
            "{\"devDependencies\":{\"x\":\"1\"}}\n",
        );
        write(&s.repo, "apps/mobile/bun.lock", "lock\n");
        write(&s.repo, ".github/workflows/e2e.yml", "on: push\n");
        write(
            &s.repo,
            "apps/mobile/e2e/playwright.config.ts",
            "export default {};\n",
        );
        write(
            &s.repo,
            "apps/mobile/e2e/reply.spec.ts",
            "test('x', () => {});\n",
        );

        let lines = violation_lines(verdict(&s, None));
        assert_eq!(
            lines,
            [
                "A .github/workflows/e2e.yml (CI configuration)",
                "A apps/mobile/bun.lock (dependency manifest, lockfile, or toolchain pin)",
                "A apps/mobile/e2e/playwright.config.ts (new E2E framework: Playwright)",
                "M package.json (dependency manifest, lockfile, or toolchain pin)",
            ]
        );
    }

    #[test]
    fn ignores_changes_before_the_baseline_and_existing_framework_files() {
        let s = session();
        write(&s.repo, ".maestro/flow.yaml", "appId: x\n");
        write(
            &s.repo,
            "package.json",
            "{\"devDependencies\":{\"y\":\"2\"}}\n",
        );
        record_baseline(&s.repo, &s.plan, "e2e").unwrap_or_else(|error| panic!("{error}"));
        write(
            &s.repo,
            ".maestro/flow.yaml",
            "appId: x\n---\n- launchApp\n",
        );
        write(&s.repo, ".maestro/reply.yaml", "appId: x\n");
        write(&s.repo, "src/app.ts", "export const app = 2;\n");

        assert_eq!(verdict(&s, None), Verdict::Pass);
    }

    #[test]
    fn rejects_a_new_maestro_directory() {
        let s = session();
        record_baseline(&s.repo, &s.plan, "e2e").unwrap_or_else(|error| panic!("{error}"));
        write(&s.repo, "apps/mobile/.maestro/flow.yaml", "appId: x\n");

        assert_eq!(
            violation_lines(verdict(&s, None)),
            ["A apps/mobile/.maestro/flow.yaml (new E2E framework: Maestro)"]
        );
    }

    #[test]
    fn blocked_report_stops_with_its_blockers() {
        let s = session();
        record_baseline(&s.repo, &s.plan, "e2e").unwrap_or_else(|error| panic!("{error}"));
        crate::artifacts::write(
            &s.artifacts,
            "e2e/report.md",
            "## Targets\n- iOS\n\n## Blockers\n- iOS visual: Maestro is not installed; install Maestro 2.x.\n\n## Commands\n- none\n\nE2E_STATUS: blocked\n",
        )
        .unwrap_or_else(|error| panic!("{error}"));

        let Verdict::Blocked(message) = verdict(&s, Some("e2e/report.md")) else {
            panic!("expected blocked verdict");
        };
        assert!(message.contains("Maestro is not installed; install Maestro 2.x."));
        assert!(!message.contains("## Commands"));
        assert!(message.contains(&s.artifacts.join("e2e/report.md").display().to_string()));
    }

    #[test]
    fn report_requires_one_status_and_blockers_when_blocked() {
        let s = session();
        record_baseline(&s.repo, &s.plan, "e2e").unwrap_or_else(|error| panic!("{error}"));
        for (report, expected_pass) in [
            ("## Targets\n- web\n\nE2E_STATUS: passed\n", true),
            ("No UI target.\nE2E_STATUS: not-applicable\n", true),
            ("## Targets\n- web\n", false),
            ("E2E_STATUS: passed\nE2E_STATUS: blocked\n", false),
            ("E2E_STATUS: skipped\n", false),
            ("## Blockers\n\nE2E_STATUS: blocked\n", false),
        ] {
            crate::artifacts::write(&s.artifacts, "e2e/report.md", report)
                .unwrap_or_else(|error| panic!("{error}"));
            let verdict = verdict(&s, Some("e2e/report.md"));
            assert_eq!(verdict == Verdict::Pass, expected_pass, "{report:?}");
            if !expected_pass {
                assert!(matches!(verdict, Verdict::Violation(_)), "{report:?}");
            }
        }
    }

    #[test]
    fn missing_baseline_is_an_error() {
        let s = session();
        assert!(check(&s.repo, &s.plan, "e2e", None).is_err());
        assert!(record_baseline(&s.repo, &s.plan, "../escape").is_err());
    }
}
