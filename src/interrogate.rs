//! Reproduce-before-apply gate for the built-in TDD review loop with flag
//! `interrogate` (`tests/formal/review_repro_gate.qnt`).
//!
//! The existing consolidate step writes the static worklist. `interrogate-baseline`
//! records the working tree, a read-only `reproduce` step reruns each behavioral
//! item's command, and `interrogate-filter` keeps only reproduced behavioral items
//! (plus every SIMP/DOC-only item) in the worklist apply reads. Not-reproduced
//! items become `Dropped` with reason `not reproduced`; blocked items are listed for
//! apply to keep `persists` without editing. A workspace change stops the run.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::error::{CruiseError, Result};
use crate::trusted_git::{session_artifacts_root, worktree_tree};

const WORKLIST: &str = "analysis/worklist.md";
const STATIC_WORKLIST: &str = "analysis/worklist-static.md";
const REPRODUCTION: &str = "analysis/reproduction.md";
const BASELINE: &str = "interrogate/baseline";
const BEHAVIORAL: [&str; 3] = ["REV", "PLAN", "WIRE"];

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Verdict {
    Reproduced,
    NotReproduced,
    Blocked,
}

struct Item {
    id: String,
    text: String,
    sources: Vec<String>,
}

impl Item {
    fn behavioral(&self) -> bool {
        self.sources.iter().any(|source| {
            source
                .split_once('-')
                .is_some_and(|(prefix, _)| BEHAVIORAL.contains(&prefix))
        })
    }
}

struct Worklist {
    items: Vec<Item>,
    /// `## Dropped` and every later section, verbatim.
    rest: String,
}

/// Record the working tree before the reproduce step.
pub(crate) fn baseline(cwd: &Path, plan: &Path) -> Result<()> {
    let root = session_artifacts_root(plan)?;
    crate::artifacts::write(&root, BASELINE, &format!("{}\n", worktree_tree(cwd)?))
}

/// Keep reproduced behavioral items and rewrite the worklist apply reads.
pub(crate) fn filter(cwd: &Path, plan: &Path) -> Result<String> {
    let root = session_artifacts_root(plan)?;
    let baseline = crate::artifacts::read(&root, BASELINE)
        .map_err(|_| gate_error("no reproduction baseline; interrogate-baseline must run first"))?;
    if worktree_tree(cwd)? != baseline.trim() {
        return Err(gate_error(
            "the reproduce step changed the workspace; it may only run commands. Restore the workspace and resume, or reset the run",
        ));
    }
    let text = crate::artifacts::read(&root, WORKLIST)?;
    let worklist = parse_worklist(&text)?;
    let behavioral: Vec<&Item> = worklist
        .items
        .iter()
        .filter(|item| item.behavioral())
        .collect();
    let verdicts = if behavioral.is_empty() {
        BTreeMap::new()
    } else {
        parse_verdicts(&crate::artifacts::read(&root, REPRODUCTION)?)
    };
    let missing: Vec<&str> = behavioral
        .iter()
        .filter(|item| !verdicts.contains_key(&item.id))
        .map(|item| item.id.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(gate_error(&format!(
            "the reproduction report has no verdict for {}",
            missing.join(", ")
        )));
    }
    crate::artifacts::write(&root, STATIC_WORKLIST, &text)?;
    let (filtered, summary) = render(&worklist, &verdicts);
    crate::artifacts::write(&root, WORKLIST, &filtered)?;
    Ok(summary)
}

fn render(worklist: &Worklist, verdicts: &BTreeMap<String, (Verdict, String)>) -> (String, String) {
    let verdict = |item: &Item| {
        if item.behavioral() {
            verdicts
                .get(&item.id)
                .map_or(Verdict::NotReproduced, |(v, _)| *v)
        } else {
            Verdict::Reproduced
        }
    };
    let kept: Vec<&Item> = worklist
        .items
        .iter()
        .filter(|item| verdict(item) == Verdict::Reproduced)
        .collect();
    let mut out = String::from("# Consolidated Findings\n\n## Worklist\n\n");
    if kept.is_empty() {
        out.push_str("EMPTY WORKLIST: no consolidated item was reproduced; apply must change no workspace files.\n\n");
    }
    for item in &kept {
        out.push_str(item.text.trim_end());
        out.push_str("\n\n");
    }
    let mut dropped = String::new();
    let mut blocked = String::new();
    let (mut not_reproduced, mut blocked_items) = (0, 0);
    for item in &worklist.items {
        let observed = verdicts
            .get(&item.id)
            .map_or("", |(_, observed)| observed.as_str());
        match verdict(item) {
            Verdict::Reproduced => {}
            Verdict::NotReproduced => {
                not_reproduced += 1;
                let detail = if observed.is_empty() {
                    "the command did not show the failure"
                } else {
                    observed
                };
                for source in &item.sources {
                    let _ = writeln!(
                        dropped,
                        "- `{source}` - reason: not reproduced - {} ({detail})",
                        item.id
                    );
                }
            }
            Verdict::Blocked => {
                blocked_items += 1;
                let _ = writeln!(
                    blocked,
                    "- {} ({}) - {observed}",
                    item.id,
                    item.sources.join(", ")
                );
            }
        }
    }
    let rest = worklist.rest.trim_end();
    if dropped.is_empty() {
        out.push_str(rest);
        out.push('\n');
    } else if let Some(after) = rest.strip_prefix("## Dropped") {
        // A `- none` placeholder no longer holds once items were dropped here.
        let (existing, tail) = after
            .split_once("\n## ")
            .map_or((after, ""), |(a, t)| (a, t));
        let existing: String = existing
            .lines()
            .filter(|line| !line.trim().is_empty() && line.trim() != "- none")
            .map(|line| format!("{line}\n"))
            .collect();
        let _ = write!(out, "## Dropped\n\n{existing}{dropped}");
        if !tail.is_empty() {
            let _ = write!(out, "\n## {tail}\n");
        }
    } else {
        let _ = write!(out, "## Dropped\n\n{dropped}\n{rest}\n");
    }
    if !blocked.is_empty() {
        let _ = write!(
            out,
            "\n## Blocked by reproduction\n\nDo not edit these; record every contributing ID as `persists` with the blocker:\n{blocked}"
        );
    }
    let summary = format!(
        "Reproduction kept {} of {} worklist items ({not_reproduced} not reproduced, {blocked_items} blocked).",
        kept.len(),
        worklist.items.len(),
    );
    (out, summary)
}

fn parse_worklist(text: &str) -> Result<Worklist> {
    let start = text
        .find("## Worklist")
        .ok_or_else(|| gate_error("the worklist has no `## Worklist` section"))?;
    let body = &text[start + "## Worklist".len()..];
    let (items_text, rest) = match body.find("\n## ") {
        Some(end) => (&body[..end], body[end + 1..].to_owned()),
        None => (body, String::new()),
    };
    let mut items = Vec::new();
    for block in items_text.split("\n### ").skip(1) {
        let heading = block.lines().next().unwrap_or_default();
        let id = heading
            .split(" | ")
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        if !id.starts_with('W') || id.len() < 2 || !id[1..].bytes().all(|b| b.is_ascii_digit()) {
            return Err(gate_error(&format!(
                "malformed worklist item heading `{heading}`"
            )));
        }
        let sources = block
            .lines()
            .find_map(|line| line.trim().strip_prefix("- contributing findings:"))
            .map(|ids| {
                ids.split(',')
                    .map(|id| id.trim().trim_matches('`').to_owned())
                    .filter(|id| !id.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        items.push(Item {
            id,
            text: format!("### {block}"),
            sources,
        });
    }
    Ok(Worklist { items, rest })
}

/// `### W<n>` sections with a `- verdict:` and optional `- observed:` line.
fn parse_verdicts(report: &str) -> BTreeMap<String, (Verdict, String)> {
    let mut verdicts = BTreeMap::new();
    for block in report.split("### ").skip(1) {
        let id = block
            .lines()
            .next()
            .unwrap_or_default()
            .split(|c: char| c.is_whitespace() || c == '|')
            .next()
            .unwrap_or_default()
            .to_owned();
        let field = |name: &str| {
            block.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("- ")
                    .and_then(|rest| rest.strip_prefix(name))
                    .and_then(|rest| rest.strip_prefix(':'))
                    .map(|value| value.trim().trim_matches('`').to_owned())
            })
        };
        let verdict = match field("verdict").as_deref() {
            Some("reproduced") => Verdict::Reproduced,
            Some("not-reproduced" | "not reproduced") => Verdict::NotReproduced,
            Some("blocked") => Verdict::Blocked,
            _ => continue,
        };
        let observed: String = field("observed")
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect();
        verdicts.insert(id, (verdict, observed));
    }
    verdicts
}

fn gate_error(message: &str) -> CruiseError {
    CruiseError::Other(format!("interrogate gate failed: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKLIST_TEXT: &str = "# Consolidated Findings\n\n## Worklist\n\n### W1 | major | a.rs:1\n- contributing findings: REV-1, PLAN-2\n- repro: `cargo test a`\n- exact minimal change: fix a\n- verification owed: cargo test a\n\n### W2 | minor | b.rs:2\n- contributing findings: REV-3\n- repro: `cargo test b`\n- exact minimal change: fix b\n- verification owed: cargo test b\n\n### W3 | minor | c.rs:3\n- contributing findings: SIMP-1\n- repro: none\n- exact minimal change: shrink c\n- verification owed: cargo build\n\n### W4 | major | d.rs:4\n- contributing findings: WIRE-1\n- repro: `docker compose up`\n- exact minimal change: wire d\n- verification owed: hurl\n\n## Dropped\n\n- `DOC-1` - reason: below bar - no mismatch\n\n## Unavailable sources\n\n- none\n";
    const REPORT: &str = "### W1\n- command: `cargo test a`\n- exit: 101\n- observed: assertion failed at a.rs:1\n- verdict: reproduced\n\n### W2\n- command: `cargo test b`\n- exit: 0\n- observed: test passed\n- verdict: not-reproduced\n\n### W4\n- command: `docker compose up`\n- exit: 127\n- observed: docker not installed\n- verdict: blocked\n";

    fn filtered() -> (String, String) {
        let worklist = parse_worklist(WORKLIST_TEXT).unwrap_or_else(|e| panic!("{e}"));
        render(&worklist, &parse_verdicts(REPORT))
    }

    #[test]
    fn keeps_reproduced_behavioral_and_every_simp_doc_item() {
        let (out, summary) = filtered();
        assert!(out.contains("### W1 | major"), "{out}");
        assert!(out.contains("### W3 | minor"), "{out}");
        assert!(!out.contains("### W2 |"), "{out}");
        assert!(!out.contains("### W4 |"), "{out}");
        assert_eq!(
            summary,
            "Reproduction kept 2 of 4 worklist items (1 not reproduced, 1 blocked)."
        );
    }

    #[test]
    fn not_reproduced_is_dropped_with_reason_and_blocked_is_listed_for_persists() {
        let (out, _) = filtered();
        assert!(
            out.contains("- `REV-3` - reason: not reproduced - W2 (test passed)"),
            "{out}"
        );
        assert!(out.contains("- `DOC-1` - reason: below bar"), "{out}");
        assert!(out.contains("## Blocked by reproduction"), "{out}");
        assert!(
            out.contains("- W4 (WIRE-1) - docker not installed"),
            "{out}"
        );
        assert!(out.contains("## Unavailable sources"), "{out}");
    }

    #[test]
    fn nothing_reproduced_empties_the_worklist() {
        let worklist = parse_worklist(WORKLIST_TEXT).unwrap_or_else(|e| panic!("{e}"));
        let only_behavioral = Worklist {
            items: worklist
                .items
                .into_iter()
                .filter(Item::behavioral)
                .collect(),
            rest: worklist.rest,
        };
        let report = "### W1\n- verdict: not-reproduced\n### W2\n- verdict: not-reproduced\n### W4\n- verdict: blocked\n";
        let (out, _) = render(&only_behavioral, &parse_verdicts(report));
        assert!(out.contains("EMPTY WORKLIST"), "{out}");
    }

    #[test]
    fn missing_verdicts_and_workspace_changes_stop() {
        let _lock = crate::test_support::lock_process();
        let temp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let _data = crate::test_support::EnvGuard::set("XDG_DATA_HOME", temp.path().join("data"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(repo.join("README"), "x").unwrap_or_else(|e| panic!("{e}"));
        for args in [
            &["init", "-q"][..],
            &["add", "README"],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-qm",
                "i",
            ],
        ] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .status()
                .unwrap_or_else(|e| panic!("{e}"));
            assert!(status.success());
        }
        let session = crate::paths::data_dir()
            .unwrap_or_else(|e| panic!("{e}"))
            .join("sessions/test-session");
        std::fs::create_dir_all(&session).unwrap_or_else(|e| panic!("{e}"));
        let plan = session.join("plan.md");
        std::fs::write(&plan, "plan").unwrap_or_else(|e| panic!("{e}"));
        let root = session.join(crate::artifacts::ARTIFACTS_DIR);
        let write = |name: &str, text: &str| {
            crate::artifacts::write(&root, name, text).unwrap_or_else(|e| panic!("{e}"));
        };
        write(WORKLIST, WORKLIST_TEXT);
        baseline(&repo, &plan).unwrap_or_else(|e| panic!("{e}"));
        write(REPRODUCTION, "### W1\n- verdict: reproduced\n");
        let missing = filter(&repo, &plan)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(missing.contains("no verdict for W2, W4"), "{missing}");
        write(REPRODUCTION, REPORT);
        std::fs::write(repo.join("edit.txt"), "x").unwrap_or_else(|e| panic!("{e}"));
        let changed = filter(&repo, &plan)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(changed.contains("changed the workspace"), "{changed}");
        std::fs::remove_file(repo.join("edit.txt")).unwrap_or_else(|e| panic!("{e}"));
        filter(&repo, &plan).unwrap_or_else(|e| panic!("{e}"));
        let applied = crate::artifacts::read(&root, WORKLIST).unwrap_or_else(|e| panic!("{e}"));
        assert!(!applied.contains("### W2 |"), "{applied}");
        assert_eq!(
            crate::artifacts::read(&root, STATIC_WORKLIST)
                .ok()
                .as_deref(),
            Some(WORKLIST_TEXT)
        );
    }
}
