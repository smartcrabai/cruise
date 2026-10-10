//! Pull request status inspection and merge command construction.

use crate::error::{CruiseError, Result};

/// Merge method chosen by the human for one merge operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrMergeMethod {
    Merge,
    Squash,
    Rebase,
}

/// One CI check or status context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrCheckStatus {
    pub name: String,
    pub status: String,
}

/// Snapshot of PR state used to preview a merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrMergeStatus {
    /// Upper-case GitHub state: `OPEN`, `CLOSED` or `MERGED`.
    pub state: String,
    pub mergeable: String,
    pub review_decision: String,
    pub checks: Vec<PrCheckStatus>,
}

impl PrMergeMethod {
    fn flag(self) -> &'static str {
        match self {
            Self::Merge => "--merge",
            Self::Squash => "--squash",
            Self::Rebase => "--rebase",
        }
    }
}

fn text(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn parse_check(value: &serde_json::Value) -> PrCheckStatus {
    let is_context =
        value.get("__typename").and_then(serde_json::Value::as_str) == Some("StatusContext");
    if is_context {
        return PrCheckStatus {
            name: text(value, "context"),
            status: text(value, "state"),
        };
    }
    let conclusion = text(value, "conclusion");
    let status = if conclusion.is_empty() {
        text(value, "status")
    } else {
        conclusion
    };
    PrCheckStatus {
        name: text(value, "name"),
        status,
    }
}

/// Run `gh` and return stdout, mapping spawn and exit failures.
fn run_gh(verb: &str, args: &[&str]) -> Result<Vec<u8>> {
    let output = std::process::Command::new("gh")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run gh: {e}")))?;
    if !output.status.success() {
        return Err(CruiseError::Other(format!(
            "gh pr {verb} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

/// Query the PR through `gh pr view`.
///
/// # Errors
///
/// Returns [`CruiseError::Other`] when gh fails or its output is unusable.
pub fn inspect(pr_url: &str) -> Result<PrMergeStatus> {
    let stdout = run_gh(
        "view",
        &[
            "pr",
            "view",
            pr_url,
            "--json",
            "state,mergeable,reviewDecision,statusCheckRollup",
        ],
    )?;
    let value: serde_json::Value = serde_json::from_slice(&stdout)
        .map_err(|e| CruiseError::Other(format!("invalid gh pr view output: {e}")))?;
    let state = text(&value, "state").to_uppercase();
    if !matches!(state.as_str(), "OPEN" | "CLOSED" | "MERGED") {
        return Err(CruiseError::Other(format!(
            "unexpected pull request state: '{state}'"
        )));
    }
    let checks = value
        .get("statusCheckRollup")
        .and_then(serde_json::Value::as_array)
        .map(|items| items.iter().map(parse_check).collect())
        .unwrap_or_default();
    Ok(PrMergeStatus {
        state,
        mergeable: text(&value, "mergeable"),
        review_decision: text(&value, "reviewDecision"),
        checks,
    })
}

/// Merge the PR through `gh pr merge` with the selected method.
///
/// # Errors
///
/// Returns [`CruiseError::Other`] when gh fails.
pub fn merge(pr_url: &str, method: PrMergeMethod) -> Result<()> {
    run_gh("merge", &["pr", "merge", pr_url, method.flag()])?;
    Ok(())
}

/// Fake `gh` installer shared by tests that exercise PR merge flows.
///
/// `gh pr view` prints `view.out` and exits with `view.exit` (default 0).
/// `gh pr merge` exits with `merge.exit` (default 0); on success, when
/// `view_after.out` exists it replaces `view.out`, simulating GitHub's new
/// state. Every invocation is appended to `gh.log`.
#[cfg(all(test, unix))]
pub(crate) mod fake_gh {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    pub(crate) struct FakeGh {
        pub dir: PathBuf,
    }

    impl FakeGh {
        pub(crate) fn install(root: &Path) -> Self {
            let dir = root.join("fake-gh");
            let bin = dir.join("bin");
            std::fs::create_dir_all(&bin).unwrap_or_else(|e| panic!("{e}"));
            let script = format!(
                "#!/bin/sh\n\
printf '%s\\n' \"$*\" >> '{d}/gh.log'\n\
if [ \"$1 $2\" = 'pr view' ]; then\n\
  if [ -f '{d}/view.exit' ] && [ \"$(cat '{d}/view.exit')\" != '0' ]; then echo 'view failed' >&2; exit \"$(cat '{d}/view.exit')\"; fi\n\
  cat '{d}/view.out'\n\
  exit 0\n\
fi\n\
if [ \"$1 $2\" = 'pr merge' ]; then\n\
  if [ -f '{d}/merge.exit' ] && [ \"$(cat '{d}/merge.exit')\" != '0' ]; then echo 'merge failed' >&2; exit \"$(cat '{d}/merge.exit')\"; fi\n\
  if [ -f '{d}/view_after.out' ]; then cp '{d}/view_after.out' '{d}/view.out'; fi\n\
  exit 0\n\
fi\n\
exit 2\n",
                d = dir.display()
            );
            let path = bin.join("gh");
            std::fs::write(&path, script).unwrap_or_else(|e| panic!("{e}"));
            let mut perms = std::fs::metadata(&path)
                .unwrap_or_else(|e| panic!("{e}"))
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap_or_else(|e| panic!("{e}"));
            Self { dir }
        }

        pub(crate) fn bin(&self) -> PathBuf {
            self.dir.join("bin")
        }

        pub(crate) fn set(&self, file: &str, content: &str) {
            std::fs::write(self.dir.join(file), content).unwrap_or_else(|e| panic!("{e}"));
        }

        pub(crate) fn log(&self) -> String {
            std::fs::read_to_string(self.dir.join("gh.log")).unwrap_or_default()
        }

        pub(crate) fn merge_calls(&self) -> Vec<String> {
            self.log()
                .lines()
                .filter(|l| l.starts_with("pr merge"))
                .map(str::to_string)
                .collect()
        }
    }

    pub(crate) fn view_json(state: &str) -> String {
        format!(
            r#"{{"state":"{state}","mergeable":"MERGEABLE","reviewDecision":"APPROVED","statusCheckRollup":[]}}"#
        )
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::fake_gh::{FakeGh, view_json};
    use super::*;

    fn path_with(
        gh: &FakeGh,
    ) -> (
        crate::test_support::EnvGuard,
        crate::test_support::ProcessLock,
    ) {
        let lock = crate::test_support::lock_process();
        let guard = crate::test_support::prepend_to_path(&gh.bin());
        (guard, lock)
    }

    const URL: &str = "https://github.com/owner/repo/pull/7";

    fn setup() -> (tempfile::TempDir, FakeGh) {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let gh = FakeGh::install(tmp.path());
        (tmp, gh)
    }

    #[test]
    fn inspect_pr_parses_state_mergeability_review_and_check_rollup() {
        let (_tmp, gh) = setup();
        gh.set(
            "view.out",
            r#"{"state":"OPEN","mergeable":"CONFLICTING","reviewDecision":"CHANGES_REQUESTED","statusCheckRollup":[
              {"__typename":"CheckRun","name":"build","status":"COMPLETED","conclusion":"FAILURE"},
              {"__typename":"StatusContext","context":"ci/lint","state":"PENDING"}]}"#,
        );
        let _path = path_with(&gh);

        let status = inspect(URL).unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(status.state, "OPEN");
        assert_eq!(status.mergeable, "CONFLICTING");
        assert_eq!(status.review_decision, "CHANGES_REQUESTED");
        assert_eq!(status.checks.len(), 2);
        assert_eq!(status.checks[0].name, "build");
        assert!(status.checks[0].status.to_uppercase().contains("FAILURE"));
        assert_eq!(status.checks[1].name, "ci/lint");
        assert!(status.checks[1].status.to_uppercase().contains("PENDING"));
        let log = gh.log();
        assert!(
            log.contains(&format!(
                "pr view {URL} --json state,mergeable,reviewDecision,statusCheckRollup"
            )),
            "{log}"
        );
    }

    #[test]
    fn inspect_pr_accepts_null_and_empty_check_rollup() {
        let (_tmp, gh) = setup();
        let _path = path_with(&gh);
        for rollup in ["null", "[]"] {
            gh.set(
                "view.out",
                &format!(
                    r#"{{"state":"MERGED","mergeable":"UNKNOWN","reviewDecision":"","statusCheckRollup":{rollup}}}"#
                ),
            );
            let status = inspect(URL).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(status.state, "MERGED");
            assert!(status.checks.is_empty(), "rollup {rollup}");
        }
    }

    #[test]
    fn inspect_pr_accepts_null_review_decision() {
        let (_tmp, gh) = setup();
        gh.set(
            "view.out",
            r#"{"state":"OPEN","mergeable":"MERGEABLE","reviewDecision":null,"statusCheckRollup":null}"#,
        );
        let _path = path_with(&gh);
        assert_eq!(inspect(URL).unwrap_or_else(|e| panic!("{e}")).state, "OPEN");
    }

    #[test]
    fn inspect_pr_fails_closed_on_spawn_exit_and_invalid_json() {
        let (tmp, gh) = setup();
        {
            let _path = path_with(&gh);
            gh.set("view.out", "not json");
            assert!(matches!(inspect(URL), Err(CruiseError::Other(_))));

            gh.set("view.out", r#"{"mergeable":"MERGEABLE"}"#);
            assert!(matches!(inspect(URL), Err(CruiseError::Other(_))));

            gh.set("view.out", r#"{"state":"WEIRD","mergeable":"MERGEABLE"}"#);
            assert!(matches!(inspect(URL), Err(CruiseError::Other(_))));

            gh.set("view.out", &view_json("OPEN"));
            gh.set("view.exit", "1");
            assert!(matches!(inspect(URL), Err(CruiseError::Other(_))));
        }
        let empty = tmp.path().join("empty-bin");
        std::fs::create_dir_all(&empty).unwrap_or_else(|e| panic!("{e}"));
        let _lock = crate::test_support::lock_process();
        let _path = crate::test_support::EnvGuard::set("PATH", empty.as_os_str());
        assert!(matches!(inspect(URL), Err(CruiseError::Other(_))));
    }

    #[test]
    fn merge_uses_selected_method_without_auto_or_admin() {
        for (method, flag) in [
            (PrMergeMethod::Merge, "--merge"),
            (PrMergeMethod::Squash, "--squash"),
            (PrMergeMethod::Rebase, "--rebase"),
        ] {
            let (_tmp, gh) = setup();
            let _path = path_with(&gh);

            merge(URL, method).unwrap_or_else(|e| panic!("{e}"));

            assert_eq!(gh.merge_calls(), vec![format!("pr merge {URL} {flag}")]);
        }
    }

    #[test]
    fn merge_failure_is_other_error() {
        let (_tmp, gh) = setup();
        gh.set("merge.exit", "1");
        let _path = path_with(&gh);
        assert!(matches!(
            merge(URL, PrMergeMethod::Squash),
            Err(CruiseError::Other(_))
        ));
    }
}
