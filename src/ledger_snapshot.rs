//! Trusted Git snapshots for the built-in TDD findings ledger.

use std::collections::HashSet;
use std::path::Path;

use crate::error::{CruiseError, Result};
use crate::trusted_git::{
    ensure_no_repository_filters, git_output, git_output_with_index, parse_oid,
    session_artifacts_root, temporary_index_copy, valid_oid,
};

const LEDGER_NAME: &str = "cruise-findings-ledger.md";
const APPLY_OUTPUT_NAME: &str = "analysis/apply-output.md";
const INVALID_LEDGER: &str = "head: invalid\nsnapshot: invalid\niteration: 0\n";

struct LedgerRecords<'a> {
    body: &'a str,
    iteration: u64,
}

struct PreviousLedger<'a> {
    head: &'a str,
    snapshot: &'a str,
    iteration: u64,
}

/// Add trusted HEAD/snapshot fields to the apply step's session artifact.
///
/// The plan path identifies the host-owned session; artifact names and Git
/// commands remain fixed here. The previous ledger is invalidated before any
/// operation that can fail, so it cannot remain fresh after a failed apply.
pub(crate) fn refresh_after_apply(cwd: &Path, plan_path: &Path) -> Result<()> {
    let artifacts_root = session_artifacts_root(plan_path)?;
    let previous = crate::artifacts::read(&artifacts_root, LEDGER_NAME).ok();
    crate::artifacts::write(&artifacts_root, LEDGER_NAME, INVALID_LEDGER)?;

    let output = crate::artifacts::read(&artifacts_root, APPLY_OUTPUT_NAME)?;
    let records = ledger_records(&output)?;
    let head = git_oid(cwd, &["rev-parse", "--verify", "HEAD"])?;
    let expected_iteration = match previous.as_deref().and_then(previous_ledger) {
        Some(previous)
            if previous.head == head && verify_commit(cwd, previous.snapshot).is_ok() =>
        {
            previous
                .iteration
                .checked_add(1)
                .ok_or_else(|| snapshot_error("TDD ledger iteration overflowed"))?
        }
        _ => 1,
    };
    if records.iteration != expected_iteration {
        return Err(snapshot_error(
            "apply output iteration does not match the current TDD ledger",
        ));
    }

    ensure_no_repository_filters(cwd)?;
    let temporary_index = temporary_index_copy(cwd)?;
    let snapshot_output =
        git_output_with_index(cwd, &["stash", "create"], Some(&temporary_index.path))?;
    let snapshot = if snapshot_output.stdout.is_empty() {
        None
    } else {
        Some(parse_oid("git stash create", &snapshot_output.stdout)?)
    };
    let head_after = git_oid(cwd, &["rev-parse", "--verify", "HEAD"])?;
    if head != head_after {
        return Err(snapshot_error(
            "HEAD changed while creating the TDD ledger snapshot",
        ));
    }
    let snapshot = snapshot.as_deref().unwrap_or(&head);
    verify_commit(cwd, snapshot)?;

    let contents = format!("head: {head}\nsnapshot: {snapshot}\n{}\n", records.body);
    crate::artifacts::write(&artifacts_root, LEDGER_NAME, &contents)
}

fn ledger_records(output: &str) -> Result<LedgerRecords<'_>> {
    let mut marker_end = None;
    let mut offset = 0;
    for line in output.split_inclusive('\n') {
        let text = line.strip_suffix('\n').unwrap_or(line);
        let text = text.strip_suffix('\r').unwrap_or(text);
        if text == "## Ledger Records" {
            if marker_end.is_some() {
                return Err(snapshot_error(
                    "apply output has multiple ledger-record sections",
                ));
            }
            marker_end = Some(offset + line.len());
        }
        offset += line.len();
    }

    let mut section = marker_end
        .and_then(|end| output.get(end..))
        .ok_or_else(|| snapshot_error("apply output is missing its ledger-record section"))?;
    if let Some(rest) = section.strip_prefix("\r\n") {
        section = rest;
    } else if let Some(rest) = section.strip_prefix('\n') {
        section = rest;
    }
    let (fence, remainder) = section
        .split_once('\n')
        .ok_or_else(|| snapshot_error("ledger records must be in a text code block"))?;
    if fence.strip_suffix('\r').unwrap_or(fence) != "```text" {
        return Err(snapshot_error(
            "ledger records must be in a text code block",
        ));
    }
    let close_start = remainder
        .find("\n```")
        .ok_or_else(|| snapshot_error("ledger-record code block is not closed"))?;
    let body = &remainder[..close_start];
    let after_close = &remainder[close_start + 4..];
    if !after_close.is_empty() && after_close != "\n" && after_close != "\r\n" {
        return Err(snapshot_error(
            "ledger records must be the final apply-output section",
        ));
    }

    let (iteration_line, entries) = if let Some((header, entries)) = body.split_once("\n\n") {
        (header, entries)
    } else if let Some(header) = body.strip_suffix('\n') {
        (header, "")
    } else {
        return Err(snapshot_error(
            "ledger records need an iteration and a blank line",
        ));
    };
    let value = iteration_line
        .strip_prefix("iteration: ")
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| snapshot_error("ledger iteration is missing or malformed"))?;
    let iteration = value
        .parse::<u64>()
        .ok()
        .filter(|iteration| *iteration > 0)
        .ok_or_else(|| snapshot_error("ledger iteration is missing or malformed"))?;
    validate_ledger_entries(entries)?;

    Ok(LedgerRecords { body, iteration })
}

fn validate_ledger_entries(entries: &str) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }

    let mut ids = HashSet::new();
    for record in entries.split("\n\n") {
        let mut lines = record.lines();
        let Some(header) = lines.next().and_then(|line| line.strip_prefix("### ")) else {
            return Err(snapshot_error(
                "ledger record must start with a finding heading",
            ));
        };
        let mut fields = header.split(" | ");
        let (Some(id), Some(state), Some(severity), Some(location)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(snapshot_error("ledger finding heading is malformed"));
        };
        if fields.next().is_some()
            || !valid_record_id(id)
            || !ids.insert(id)
            || !matches!(state, "new" | "persists" | "resolved" | "wont-fix")
            || !matches!(severity, "critical" | "major" | "minor")
            || !valid_file_line(location)
        {
            return Err(snapshot_error("ledger finding heading is malformed"));
        }
        let Some(summary) = lines
            .next()
            .and_then(|line| line.strip_prefix("- summary: "))
        else {
            return Err(snapshot_error("ledger finding summary is missing"));
        };
        let Some(evidence) = lines
            .next()
            .and_then(|line| line.strip_prefix("- evidence: "))
        else {
            return Err(snapshot_error("ledger finding evidence is missing"));
        };
        if summary.trim().is_empty() || evidence.trim().is_empty() {
            return Err(snapshot_error(
                "ledger finding summary and evidence must not be blank",
            ));
        }
        if let Some(protects) = lines.next() {
            let Some(value) = protects.strip_prefix("- protects: ") else {
                return Err(snapshot_error("ledger record contains an unexpected field"));
            };
            let Some((construct, location)) = value.rsplit_once(" at ") else {
                return Err(snapshot_error("ledger protects field is malformed"));
            };
            if construct.trim().is_empty() || !valid_file_line(location) {
                return Err(snapshot_error("ledger protects field is malformed"));
            }
        }
        if lines.next().is_some() {
            return Err(snapshot_error("ledger record contains unexpected text"));
        }
    }
    Ok(())
}

fn valid_record_id(value: &str) -> bool {
    let Some((prefix, number)) = value.split_once('-') else {
        return false;
    };
    matches!(prefix, "PLAN" | "WIRE" | "SIMP" | "REV" | "DOC")
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && number.parse::<u64>().is_ok_and(|number| number > 0)
}

fn valid_file_line(value: &str) -> bool {
    let Some((path, line)) = value.rsplit_once(':') else {
        return false;
    };
    !path.trim().is_empty()
        && line.bytes().all(|byte| byte.is_ascii_digit())
        && line.parse::<u64>().is_ok_and(|line| line > 0)
}

fn previous_ledger(ledger: &str) -> Option<PreviousLedger<'_>> {
    let mut fields = ledger.splitn(4, '\n');
    let head = fields.next()?.strip_prefix("head: ")?;
    let snapshot = fields.next()?.strip_prefix("snapshot: ")?;
    let iteration = fields
        .next()?
        .strip_prefix("iteration: ")?
        .parse::<u64>()
        .ok()?;
    let mut entries = fields.next()?.strip_prefix('\n')?;
    if let Some(without_final_newline) = entries.strip_suffix('\n') {
        entries = without_final_newline;
    }
    if !valid_oid(head) || !valid_oid(snapshot) || iteration == 0 {
        return None;
    }
    validate_ledger_entries(entries).ok()?;
    Some(PreviousLedger {
        head,
        snapshot,
        iteration,
    })
}

fn git_oid(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = git_output(cwd, args)?;
    parse_oid(&format!("git {}", args.join(" ")), &output.stdout)
}

fn verify_commit(cwd: &Path, oid: &str) -> Result<()> {
    if !valid_oid(oid) {
        return Err(snapshot_error("TDD ledger snapshot is malformed"));
    }
    let object = format!("{oid}^{{commit}}");
    git_output(cwd, &["cat-file", "-e", &object]).map(|_| ())
}

fn snapshot_error(message: &str) -> CruiseError {
    CruiseError::Other(format!("TDD ledger snapshot failed: {message}"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::*;
    use crate::trusted_git::{DISABLED_GLOBAL_CONFIG, DISABLED_HOOKS_CONFIG, parse_git_path};

    fn with_session(test: impl FnOnce(&Path, &Path, &Path)) {
        let _lock = crate::test_support::lock_process();
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let _data_home =
            crate::test_support::EnvGuard::set("XDG_DATA_HOME", temp.path().join("data"));
        let workspace = temp.path().join("repo");
        fs::create_dir_all(&workspace)
            .unwrap_or_else(|error| panic!("failed to create test repository: {error}"));
        let session_dir = crate::paths::data_dir()
            .unwrap_or_else(|error| panic!("data dir failed: {error}"))
            .join("sessions")
            .join("test-session");
        fs::create_dir_all(&session_dir)
            .unwrap_or_else(|error| panic!("failed to create test session: {error}"));
        let plan = session_dir.join("plan.md");
        fs::write(&plan, "plan")
            .unwrap_or_else(|error| panic!("failed to create test plan: {error}"));
        let artifacts = session_dir.join(crate::artifacts::ARTIFACTS_DIR);
        crate::artifacts::write(&artifacts, APPLY_OUTPUT_NAME, &apply_output(1, ""))
            .unwrap_or_else(|error| panic!("failed to create test output artifact: {error}"));
        test(&workspace, &plan, &artifacts);
    }

    fn apply_output(iteration: u64, entries: &str) -> String {
        format!(
            "## Applied work items\n- none\n\n## Ledger Records\n```text\niteration: {iteration}\n\n{entries}```\n"
        )
    }

    fn git(repo: &Path, args: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .args([
                "--no-optional-locks",
                "-c",
                DISABLED_HOOKS_CONFIG,
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
            ])
            .args(args)
            .current_dir(repo)
            .env("GIT_CONFIG_COUNT", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", DISABLED_GLOBAL_CONFIG)
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .output()
            .unwrap_or_else(|error| panic!("git command failed to start: {error}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn repository_index(repo: &Path) -> PathBuf {
        parse_git_path(repo, &git(repo, &["rev-parse", "--git-path", "index"]))
            .unwrap_or_else(|error| panic!("failed to resolve repository index: {error}"))
    }

    fn init_repo(repo: &Path) {
        git(repo, &["init"]);
        git(repo, &["config", "user.name", "Cruise Test"]);
        git(repo, &["config", "user.email", "cruise@example.invalid"]);
        fs::write(repo.join("tracked.txt"), "before\n")
            .unwrap_or_else(|error| panic!("failed to write tracked file: {error}"));
        git(repo, &["add", "tracked.txt"]);
        git(repo, &["commit", "-m", "initial"]);
    }

    fn ledger_snapshot(artifacts: &Path) -> String {
        crate::artifacts::read(artifacts, LEDGER_NAME)
            .unwrap_or_else(|error| panic!("failed to read ledger: {error}"))
            .lines()
            .nth(1)
            .and_then(|line| line.strip_prefix("snapshot: "))
            .unwrap_or_else(|| panic!("ledger snapshot missing"))
            .to_string()
    }

    fn ledger_content(artifacts: &Path) -> String {
        crate::artifacts::read(artifacts, LEDGER_NAME)
            .unwrap_or_else(|error| panic!("failed to read ledger: {error}"))
    }

    #[test]
    fn captures_real_worktree_snapshot_without_changing_index_or_refs() {
        with_session(|repo, plan, artifacts| {
            init_repo(repo);
            crate::artifacts::write(
                artifacts,
                APPLY_OUTPUT_NAME,
                &apply_output(
                    1,
                    "### REV-1 | persists | major | src/lib.rs:1\n- summary: preserves behavior\n- evidence: reproduced on the final snapshot\n",
                ),
            )
            .unwrap_or_else(|error| panic!("failed to update test output artifact: {error}"));
            fs::write(repo.join("staged.txt"), "staged\n")
                .unwrap_or_else(|error| panic!("failed to write staged file: {error}"));
            git(repo, &["add", "staged.txt"]);
            fs::write(repo.join("tracked.txt"), "after\n")
                .unwrap_or_else(|error| panic!("failed to modify tracked file: {error}"));
            git(repo, &["config", "--local", "--unset-all", "user.name"]);
            git(repo, &["config", "--local", "--unset-all", "user.email"]);
            let index_path = repository_index(repo);
            let index_before = fs::read(&index_path)
                .unwrap_or_else(|error| panic!("failed to read index: {error}"));
            let refs_before = git(repo, &["for-each-ref", "--format=%(refname) %(objectname)"]);
            let head = String::from_utf8(git(repo, &["rev-parse", "--verify", "HEAD"]))
                .unwrap_or_else(|error| panic!("HEAD was not UTF-8: {error}"))
                .trim()
                .to_string();

            refresh_after_apply(repo, plan)
                .unwrap_or_else(|error| panic!("snapshot failed: {error}"));

            let ledger = ledger_content(artifacts);
            assert!(ledger.contains("iteration: 1\n"));
            assert!(ledger.contains("REV-1"));
            assert!(ledger.contains("reproduced on the final snapshot"));

            let snapshot = ledger_snapshot(artifacts);
            assert_ne!(snapshot, head);
            assert_eq!(
                git(repo, &["show", &format!("{snapshot}:tracked.txt")]),
                b"after\n"
            );
            assert_eq!(
                git(repo, &["show", &format!("{snapshot}:staged.txt")]),
                b"staged\n"
            );
            assert_eq!(
                fs::read(&index_path)
                    .unwrap_or_else(|error| panic!("failed to read index after snapshot: {error}")),
                index_before
            );
            assert_eq!(
                git(repo, &["for-each-ref", "--format=%(refname) %(objectname)"]),
                refs_before
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn rejects_repository_clean_filters_before_snapshotting() {
        use std::os::unix::fs::PermissionsExt as _;

        with_session(|repo, plan, artifacts| {
            init_repo(repo);
            fs::write(repo.join(".gitattributes"), "tracked.txt filter=blocked\n")
                .unwrap_or_else(|error| panic!("failed to create attributes: {error}"));
            git(repo, &["add", ".gitattributes"]);
            git(repo, &["commit", "-m", "add filter attribute"]);

            let filter = repo.join("clean-filter");
            fs::write(&filter, "#!/bin/sh\n: > filter-ran\ncat\n")
                .unwrap_or_else(|error| panic!("failed to write clean filter: {error}"));
            let mut permissions = fs::metadata(&filter)
                .unwrap_or_else(|error| panic!("failed to stat clean filter: {error}"))
                .permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&filter, permissions)
                .unwrap_or_else(|error| panic!("failed to make clean filter executable: {error}"));
            git(repo, &["config", "filter.blocked.clean", "./clean-filter"]);
            git(repo, &["config", "filter.blocked.smudge", "cat"]);
            fs::write(repo.join("tracked.txt"), "changed\n")
                .unwrap_or_else(|error| panic!("failed to modify filtered file: {error}"));

            assert!(refresh_after_apply(repo, plan).is_err());
            assert!(!repo.join("filter-ran").exists());
            assert_eq!(ledger_content(artifacts), INVALID_LEDGER);
        });
    }

    #[test]
    fn uses_head_for_clean_tree_and_increments_a_fresh_ledger() {
        with_session(|repo, plan, artifacts| {
            init_repo(repo);
            let head = String::from_utf8(git(repo, &["rev-parse", "--verify", "HEAD"]))
                .unwrap_or_else(|error| panic!("HEAD was not UTF-8: {error}"))
                .trim()
                .to_string();
            crate::artifacts::write(
                artifacts,
                LEDGER_NAME,
                &format!(
                    "head: {head}\nsnapshot: {head}\niteration: 7\n\n### REV-1 | persists | major | src/lib.rs:1\n- summary: prior evidence\n- evidence: still applies\n"
                ),
            )
            .unwrap_or_else(|error| panic!("failed to seed previous ledger: {error}"));
            crate::artifacts::write(
                artifacts,
                APPLY_OUTPUT_NAME,
                &apply_output(
                    8,
                    "### REV-1 | persists | major | src/lib.rs:1\n- summary: prior evidence\n- evidence: still applies\n",
                ),
            )
            .unwrap_or_else(|error| panic!("failed to update apply output: {error}"));

            refresh_after_apply(repo, plan)
                .unwrap_or_else(|error| panic!("snapshot failed: {error}"));

            assert_eq!(ledger_snapshot(artifacts), head);
            let ledger = ledger_content(artifacts);
            assert!(ledger.contains("iteration: 8\n"));
            assert!(ledger.contains("prior evidence"));
            assert_eq!(git(repo, &["cat-file", "-e", &head]), b"");
        });
    }

    #[test]
    fn stale_head_restarts_iteration_at_one() {
        with_session(|repo, plan, artifacts| {
            init_repo(repo);
            let old_head = String::from_utf8(git(repo, &["rev-parse", "--verify", "HEAD"]))
                .unwrap_or_else(|error| panic!("HEAD was not UTF-8: {error}"))
                .trim()
                .to_string();
            crate::artifacts::write(
                artifacts,
                LEDGER_NAME,
                &format!(
                    "head: {old_head}\nsnapshot: {old_head}\niteration: 9\n\n### REV-1 | persists | major | src/lib.rs:1\n- summary: stale evidence\n- evidence: belongs to the prior HEAD\n"
                ),
            )
            .unwrap_or_else(|error| panic!("failed to seed stale ledger: {error}"));
            fs::write(repo.join("tracked.txt"), "new head\n")
                .unwrap_or_else(|error| panic!("failed to update tracked file: {error}"));
            git(repo, &["add", "tracked.txt"]);
            git(repo, &["commit", "-m", "advance HEAD"]);

            refresh_after_apply(repo, plan)
                .unwrap_or_else(|error| panic!("snapshot failed: {error}"));

            assert!(ledger_content(artifacts).contains("iteration: 1\n"));
        });
    }

    #[test]
    fn snapshot_failure_invalidates_previous_ledger() {
        with_session(|repo, plan, artifacts| {
            crate::artifacts::write(
                artifacts,
                LEDGER_NAME,
                "head: old\nsnapshot: old\niteration: 8\n\n### REV-1 | persists | major | src/lib.rs:1\n- summary: stale evidence\n- evidence: must not remain fresh\n",
            )
            .unwrap_or_else(|error| panic!("failed to seed previous ledger: {error}"));

            assert!(refresh_after_apply(repo, plan).is_err());
            assert_eq!(ledger_content(artifacts), INVALID_LEDGER);
        });
    }

    #[test]
    fn malformed_apply_output_invalidates_previous_ledger_before_failure() {
        with_session(|repo, plan, artifacts| {
            init_repo(repo);
            let head = String::from_utf8(git(repo, &["rev-parse", "--verify", "HEAD"]))
                .unwrap_or_else(|error| panic!("HEAD was not UTF-8: {error}"))
                .trim()
                .to_string();
            crate::artifacts::write(
                artifacts,
                LEDGER_NAME,
                &format!("head: {head}\nsnapshot: {head}\niteration: 4\n\n"),
            )
            .unwrap_or_else(|error| panic!("failed to seed previous ledger: {error}"));
            crate::artifacts::write(
                artifacts,
                APPLY_OUTPUT_NAME,
                "## Ledger Records\n```text\nhead: model\nsnapshot: model\n```\n",
            )
            .unwrap_or_else(|error| panic!("failed to write malformed output: {error}"));

            assert!(refresh_after_apply(repo, plan).is_err());
            assert_eq!(ledger_content(artifacts), INVALID_LEDGER);
        });
    }

    #[test]
    fn rejects_malformed_records_and_nonfinal_ledger_blocks() {
        assert!(ledger_records("## Ledger Records\n```text\niteration: 1\n\n```\n").is_ok());
        assert!(
            ledger_records("## Ledger Records\n```text\nhead: model\nsnapshot: model\n```\n")
                .is_err()
        );
        assert!(ledger_records("## Ledger Records\n```text\niteration: zero\n\n```\n").is_err());
        assert!(
            ledger_records("## Ledger Records\n```text\niteration: 1\n\nnot a record\n```\n")
                .is_err()
        );
        assert!(
            ledger_records("## Ledger Records\n```text\niteration: 1\n\n```\nafter\n").is_err()
        );
    }
}
