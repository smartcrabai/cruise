//! Attack-the-premise gate for built-in repair loops (`tests/formal/premise_gate.qnt`).
//!
//! A gate is one host-identified check: a failing CI check name from the host
//! CI inventory, or a findings-ledger ID that an apply round attempted. The host
//! counts fixes that failed against each gate. Once two fixes failed and no
//! premise has been tested since, `premise-check` routes the repair loop through
//! a read-only premise step before the next fix. `premise-record` validates that
//! step's report and the unchanged workspace, then records the verdict for the
//! next fix. The step neither consumes nor adds fix budget.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CruiseError, Result};
use crate::trusted_git::{session_artifacts_root, worktree_tree};

const STATE: &str = "premise/state.json";
const VERSION: u32 = 1;
/// Failed fixes against one gate before the next fix needs a tested premise.
const THRESHOLD: u32 = 2;
const VERDICTS: [&str; 3] = ["held", "refuted", "no shared premise"];
const FIELDS: [&str; 4] = ["premise", "probe", "observed", "verdict"];
const WORKLIST_NAME: &str = "analysis/worklist.md";
const MAX_GATE_NAME: usize = 200;

/// Gate exit status that stops the premise step without retry (`EX_TEMPFAIL`).
pub(crate) const STOP_EXIT_CODE: i32 = 75;

/// Which repair loop a premise command serves.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepairLoop {
    /// After-PR CI repair (`fix-ci-error`); gates are failing CI check names.
    Ci,
    /// Built-in TDD review apply; gates are findings-ledger IDs.
    Review,
}

impl RepairLoop {
    fn prefix(self) -> &'static str {
        match self {
            Self::Ci => "ci:",
            Self::Review => "review:",
        }
    }

    fn report(self) -> &'static str {
        match self {
            Self::Ci => "premise/ci.md",
            Self::Review => "premise/review.md",
        }
    }

    fn owed(self) -> &'static str {
        match self {
            Self::Ci => "premise/ci-owed.md",
            Self::Review => "premise/review-owed.md",
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    version: u32,
    gates: BTreeMap<String, Gate>,
    /// CI gates the last host-committed CI fix targeted, judged at the next evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ci_fix: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending: Option<Pending>,
}

#[derive(Default, Serialize, Deserialize)]
struct Gate {
    /// Fixes that failed against this gate since it last passed or since its
    /// last tested premise.
    failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verdict: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Pending {
    repair: String,
    gates: Vec<String>,
    /// Working-tree tree when the premise step was owed; the step must not change it.
    tree: String,
}

/// Outcome of validating a premise report.
pub(crate) enum Recorded {
    Accepted,
    /// The report is incomplete; the message is retry feedback for the step.
    Incomplete(String),
    /// The step changed the workspace; the session must stop.
    Stop(String),
}

fn load(root: &Path) -> Result<State> {
    let Ok(text) = crate::artifacts::read(root, STATE) else {
        return Ok(State {
            version: VERSION,
            ..State::default()
        });
    };
    let state: State = serde_json::from_str(&text)
        .map_err(|error| premise_error(&format!("invalid premise state: {error}")))?;
    if state.version != VERSION {
        return Err(premise_error("premise state has an unknown version"));
    }
    Ok(state)
}

fn save(root: &Path, state: &State) -> Result<()> {
    let text = serde_json::to_string_pretty(state)
        .map_err(|error| premise_error(&format!("cannot encode premise state: {error}")))?;
    crate::artifacts::write(root, STATE, &format!("{text}\n"))
}

impl State {
    /// A fix against `gate` was observed failing.
    fn failed(&mut self, gate: &str) {
        let entry = self.gates.entry(gate.to_owned()).or_default();
        // A tested premise starts a new epoch: this fix is the first of it.
        entry.failures = if entry.verdict.take().is_some() {
            1
        } else {
            entry.failures.saturating_add(1)
        };
    }

    fn passed(&mut self, gate: &str) {
        self.gates.remove(gate);
    }

    fn owed(&self, gate: &str) -> bool {
        self.gates
            .get(gate)
            .is_some_and(|entry| entry.failures >= THRESHOLD && entry.verdict.is_none())
    }
}

/// Decide whether the next fix in `repair` needs a premise step.
///
/// Writes the loop's premise report (a no-op notice when nothing is owed) so
/// the fix prompt can always read it. Returns `true` when a premise is owed.
pub(crate) fn check(cwd: &Path, plan: &Path, repair: RepairLoop) -> Result<bool> {
    let root = session_artifacts_root(plan)?;
    let mut state = load(&root)?;
    let current: BTreeSet<String> = match repair {
        // Unavailable CI evidence owes nothing; the repair prompt reports it and waits.
        RepairLoop::Ci => crate::ci_followup::failing_checks(&root).unwrap_or_default(),
        RepairLoop::Review => worklist_ids(&crate::artifacts::read(&root, WORKLIST_NAME)?),
    }
    .into_iter()
    .map(|name| gate_name(repair, &name))
    .collect();
    let owed: Vec<String> = current.into_iter().filter(|g| state.owed(g)).collect();
    if owed.is_empty() {
        state.pending = None;
        save(&root, &state)?;
        let _ = crate::artifacts::remove(&root, repair.owed());
        crate::artifacts::write(&root, repair.report(), "No premise is owed for this fix.\n")?;
        return Ok(false);
    }
    let mut text = String::from(
        "Each gate below failed two or more fixes. Write the premise those fixes shared and test it before the next fix.\n\n",
    );
    for gate in &owed {
        let failures = state.gates.get(gate).map_or(0, |entry| entry.failures);
        let _ = writeln!(text, "- `{gate}`: {failures} failed fixes");
    }
    crate::artifacts::write(&root, repair.owed(), &text)?;
    crate::artifacts::write(
        &root,
        repair.report(),
        "Premise step pending; no verdict recorded.\n",
    )?;
    state.pending = Some(Pending {
        repair: repair.prefix().to_owned(),
        gates: owed,
        tree: worktree_tree(cwd)?,
    });
    save(&root, &state)?;
    Ok(true)
}

/// Validate the premise step's report and record each verdict.
pub(crate) fn record(cwd: &Path, plan: &Path, repair: RepairLoop) -> Result<Recorded> {
    let root = session_artifacts_root(plan)?;
    let mut state = load(&root)?;
    let pending = state
        .pending
        .take()
        .filter(|pending| pending.repair == repair.prefix())
        .ok_or_else(|| premise_error("no premise step is pending for this loop"))?;
    if worktree_tree(cwd)? != pending.tree {
        return Ok(Recorded::Stop(
            "The premise step changed the workspace. It must only read files and run probes; restore the workspace and resume, or reset the run.".into(),
        ));
    }
    let report = crate::artifacts::read(&root, repair.report())?;
    let verdicts = match parse_report(&report, &pending.gates) {
        Ok(verdicts) => verdicts,
        Err(message) => return Ok(Recorded::Incomplete(message)),
    };
    for (gate, verdict) in verdicts {
        state.gates.entry(gate).or_default().verdict = Some(verdict);
    }
    save(&root, &state)?;
    Ok(Recorded::Accepted)
}

/// Every CI check passed for the current head: reset all CI gates.
pub(crate) fn ci_passed(plan: &Path) -> Result<()> {
    let root = session_artifacts_root(plan)?;
    let mut state = load(&root)?;
    state
        .gates
        .retain(|gate, _| !gate.starts_with(RepairLoop::Ci.prefix()));
    state.ci_fix = None;
    save(&root, &state)
}

/// The host committed a CI repair for these failing checks.
pub(crate) fn ci_fix_committed(root: &Path, checks: &BTreeSet<String>) -> Result<()> {
    let mut state = load(root)?;
    state.ci_fix = Some(
        checks
            .iter()
            .map(|name| gate_name(RepairLoop::Ci, name))
            .collect(),
    );
    save(root, &state)
}

/// Host CI evidence for a new head: judge the last committed fix.
pub(crate) fn ci_observed(root: &Path, failing: &BTreeSet<String>) -> Result<()> {
    let mut state = load(root)?;
    let Some(fixed) = state.ci_fix.take() else {
        return Ok(());
    };
    let failing: BTreeSet<String> = failing
        .iter()
        .map(|name| gate_name(RepairLoop::Ci, name))
        .collect();
    for gate in fixed {
        if failing.contains(&gate) {
            state.failed(&gate);
        } else {
            state.passed(&gate);
        }
    }
    save(root, &state)
}

/// After the ledger gate accepted an apply round: judge every attempted ID.
pub(crate) fn review_observed(root: &Path, ledger_body: &str) -> Result<()> {
    let attempted = worklist_ids(&crate::artifacts::read(root, WORKLIST_NAME).unwrap_or_default());
    if attempted.is_empty() {
        return Ok(());
    }
    let mut state = load(root)?;
    for line in ledger_body.lines() {
        let Some(header) = line.strip_prefix("### ") else {
            continue;
        };
        let mut fields = header.split(" | ");
        let (Some(id), Some(status)) = (fields.next(), fields.next()) else {
            continue;
        };
        if !attempted.contains(id) {
            continue;
        }
        let gate = gate_name(RepairLoop::Review, id);
        match status {
            "persists" => state.failed(&gate),
            "resolved" | "wont-fix" => state.passed(&gate),
            _ => {}
        }
    }
    save(root, &state)
}

fn gate_name(repair: RepairLoop, name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_control() || c == '`' { ' ' } else { c })
        .take(MAX_GATE_NAME)
        .collect();
    format!("{}{}", repair.prefix(), clean.trim())
}

/// Source IDs listed as contributing findings in a consolidated worklist.
fn worklist_ids(worklist: &str) -> BTreeSet<String> {
    worklist
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- contributing findings:"))
        .flat_map(|ids| ids.split(','))
        .map(|id| id.trim().trim_matches('`').to_owned())
        .filter(|id| valid_finding_id(id))
        .collect()
}

fn valid_finding_id(value: &str) -> bool {
    value.split_once('-').is_some_and(|(prefix, number)| {
        matches!(prefix, "PLAN" | "WIRE" | "SIMP" | "REV" | "DOC")
            && !number.is_empty()
            && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// Parse `### <gate>` sections with `- premise:`, `- probe:`, `- observed:` and
/// `- verdict:` lines; every owed gate needs a complete section.
fn parse_report(
    report: &str,
    owed: &[String],
) -> std::result::Result<Vec<(String, String)>, String> {
    let mut sections: BTreeMap<&str, BTreeMap<&str, &str>> = BTreeMap::new();
    let mut current = None;
    for line in report.lines() {
        if let Some(heading) = line.strip_prefix("### ") {
            let heading = heading.trim().trim_matches('`');
            current = Some(heading);
            sections.entry(heading).or_default();
            continue;
        }
        let Some(section) = current else { continue };
        for field in FIELDS {
            if let Some(value) = line
                .trim_start()
                .strip_prefix("- ")
                .and_then(|rest| rest.strip_prefix(field))
                .and_then(|rest| rest.strip_prefix(':'))
            {
                sections
                    .entry(section)
                    .or_default()
                    .insert(field, value.trim());
            }
        }
    }
    let mut problems = String::new();
    let mut verdicts = Vec::new();
    for gate in owed {
        let Some(fields) = sections.get(gate.as_str()) else {
            let _ = writeln!(problems, "- missing section `### {gate}`");
            continue;
        };
        for field in FIELDS {
            if fields.get(field).is_none_or(|value| value.is_empty()) {
                let _ = writeln!(
                    problems,
                    "- `### {gate}` needs a nonempty `- {field}:` line"
                );
            }
        }
        match fields.get("verdict").map(|value| value.trim_matches('`')) {
            Some(verdict) if VERDICTS.contains(&verdict) => {
                verdicts.push((gate.clone(), verdict.to_owned()));
            }
            Some(other) if !other.is_empty() => {
                let _ = writeln!(
                    problems,
                    "- `### {gate}` verdict `{other}` is not one of: held, refuted, no shared premise"
                );
            }
            _ => {}
        }
    }
    if problems.is_empty() {
        Ok(verdicts)
    } else {
        Err(format!(
            "Premise report incomplete. Rewrite it with one section per owed gate:\n{problems}"
        ))
    }
}

fn premise_error(message: &str) -> CruiseError {
    CruiseError::Other(format!("premise gate failed: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            version: VERSION,
            ..State::default()
        }
    }

    #[test]
    fn third_fix_needs_premise_and_verdict_starts_new_epoch() {
        let mut s = state();
        s.failed("ci:test");
        assert!(!s.owed("ci:test"));
        s.failed("ci:test");
        assert!(s.owed("ci:test"));
        if let Some(gate) = s.gates.get_mut("ci:test") {
            gate.verdict = Some("refuted".into());
        }
        assert!(!s.owed("ci:test"));
        s.failed("ci:test");
        assert_eq!(s.gates["ci:test"].failures, 1);
        assert!(s.gates["ci:test"].verdict.is_none());
        s.failed("ci:test");
        assert!(s.owed("ci:test"));
    }

    #[test]
    fn other_gates_and_passes_reset_independently() {
        let mut s = state();
        s.failed("ci:test");
        s.failed("ci:lint");
        s.failed("ci:test");
        s.passed("ci:test");
        assert!(!s.owed("ci:test"));
        assert_eq!(s.gates["ci:lint"].failures, 1);
    }

    #[test]
    fn worklist_ids_reads_contributing_findings() {
        let worklist = "### W1 | major | a.rs:1\n- contributing findings: REV-1, `PLAN-2`\n- exact minimal change: x\n### W2 | minor | b.rs:2\n- contributing findings: SIMP-3\n";
        let ids: Vec<_> = worklist_ids(worklist).into_iter().collect();
        assert_eq!(ids, ["PLAN-2", "REV-1", "SIMP-3"]);
        assert!(worklist_ids("EMPTY WORKLIST\n").is_empty());
    }

    fn with_session(test: impl FnOnce(&Path, &Path, &Path)) {
        let _lock = crate::test_support::lock_process();
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let _data = crate::test_support::EnvGuard::set("XDG_DATA_HOME", temp.path().join("data"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap_or_else(|error| panic!("repo: {error}"));
        std::fs::write(repo.join("README"), "x").unwrap_or_else(|error| panic!("{error}"));
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
                "init",
            ],
        ] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .status()
                .unwrap_or_else(|error| panic!("git {args:?}: {error}"));
            assert!(status.success(), "git {args:?}");
        }
        let session = crate::paths::data_dir()
            .unwrap_or_else(|error| panic!("data dir: {error}"))
            .join("sessions/test-session");
        std::fs::create_dir_all(&session).unwrap_or_else(|error| panic!("session: {error}"));
        let plan = session.join("plan.md");
        std::fs::write(&plan, "plan").unwrap_or_else(|error| panic!("plan: {error}"));
        test(&repo, &plan, &session.join(crate::artifacts::ARTIFACTS_DIR));
    }

    fn write(root: &Path, name: &str, text: &str) {
        crate::artifacts::write(root, name, text).unwrap_or_else(|error| panic!("{error}"));
    }

    const WORKLIST: &str = "### W1 | major | a.rs:1\n- contributing findings: REV-1\n";
    const PERSISTS: &str =
        "iteration: 1\n\n### REV-1 | persists | major | a.rs:1\n- summary: s\n- evidence: e\n";
    const REPORT: &str = "### review:REV-1\n- premise: the fixture registers Ask\n- probe: `cargo test ask`\n- observed: prompts: []\n- verdict: refuted\n";

    fn two_failed_rounds(root: &Path) {
        write(root, WORKLIST_NAME, WORKLIST);
        for _ in 0..2 {
            review_observed(root, PERSISTS).unwrap_or_else(|error| panic!("{error}"));
        }
    }

    #[test]
    fn review_loop_owes_premise_after_two_failed_rounds_and_records_it() {
        with_session(|repo, plan, root| {
            write(root, WORKLIST_NAME, WORKLIST);
            review_observed(root, PERSISTS).unwrap_or_else(|error| panic!("{error}"));
            assert!(!check(repo, plan, RepairLoop::Review).unwrap_or_else(|e| panic!("{e}")));
            assert_eq!(
                crate::artifacts::read(root, RepairLoop::Review.report())
                    .ok()
                    .as_deref(),
                Some("No premise is owed for this fix.\n")
            );
            review_observed(root, PERSISTS).unwrap_or_else(|error| panic!("{error}"));
            assert!(check(repo, plan, RepairLoop::Review).unwrap_or_else(|e| panic!("{e}")));
            let owed = crate::artifacts::read(root, RepairLoop::Review.owed())
                .unwrap_or_else(|error| panic!("{error}"));
            assert!(owed.contains("`review:REV-1`: 2 failed fixes"), "{owed}");
            write(root, RepairLoop::Review.report(), REPORT);
            assert!(matches!(
                record(repo, plan, RepairLoop::Review).unwrap_or_else(|e| panic!("{e}")),
                Recorded::Accepted
            ));
            // The verdict is input for the next fix; no second premise is owed now.
            assert!(!check(repo, plan, RepairLoop::Review).unwrap_or_else(|e| panic!("{e}")));
        });
    }

    #[test]
    fn premise_step_that_changes_the_workspace_stops() {
        with_session(|repo, plan, root| {
            two_failed_rounds(root);
            assert!(check(repo, plan, RepairLoop::Review).unwrap_or_else(|e| panic!("{e}")));
            std::fs::write(repo.join("edit.txt"), "x").unwrap_or_else(|error| panic!("{error}"));
            write(root, RepairLoop::Review.report(), REPORT);
            assert!(matches!(
                record(repo, plan, RepairLoop::Review).unwrap_or_else(|e| panic!("{e}")),
                Recorded::Stop(_)
            ));
        });
    }

    #[test]
    fn incomplete_premise_report_is_returned_as_feedback() {
        with_session(|repo, plan, root| {
            two_failed_rounds(root);
            assert!(check(repo, plan, RepairLoop::Review).unwrap_or_else(|e| panic!("{e}")));
            write(
                root,
                RepairLoop::Review.report(),
                "### review:REV-1\n- premise: p\n",
            );
            assert!(matches!(
                record(repo, plan, RepairLoop::Review).unwrap_or_else(|e| panic!("{e}")),
                Recorded::Incomplete(message) if message.contains("`- probe:`")
            ));
        });
    }

    #[test]
    fn ci_fix_counts_only_checks_still_failing_and_pass_resets() {
        with_session(|_repo, plan, root| {
            let checks = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect();
            for _ in 0..2 {
                ci_fix_committed(root, &checks(&["test", "lint"]))
                    .unwrap_or_else(|e| panic!("{e}"));
                ci_observed(root, &checks(&["test"])).unwrap_or_else(|e| panic!("{e}"));
            }
            let state = load(root).unwrap_or_else(|error| panic!("{error}"));
            assert!(state.owed("ci:test"));
            assert!(!state.gates.contains_key("ci:lint"));
            ci_passed(plan).unwrap_or_else(|error| panic!("{error}"));
            assert!(
                load(root)
                    .unwrap_or_else(|e| panic!("{e}"))
                    .gates
                    .is_empty()
            );
        });
    }

    #[test]
    fn report_needs_every_field_and_a_known_verdict() {
        let owed = vec!["ci:test".to_owned(), "ci:lint".to_owned()];
        let good = "### ci:test\n- premise: the fixture starts the daemon\n- probe: `cargo test x`\n- observed: no daemon\n- verdict: refuted\n\n### `ci:lint`\n- premise: p\n- probe: q\n- observed: r\n- verdict: held\n";
        let verdicts = parse_report(good, &owed).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(verdicts.len(), 2);
        let missing = parse_report("### ci:test\n- premise: p\n- verdict: maybe\n", &owed)
            .err()
            .unwrap_or_default();
        assert!(
            missing.contains("missing section `### ci:lint`"),
            "{missing}"
        );
        assert!(missing.contains("`- probe:`"), "{missing}");
        assert!(missing.contains("verdict `maybe`"), "{missing}");
    }
}
