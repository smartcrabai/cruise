//! Durable, repository-scoped Manager conversation and approval records.
//!
//! A proposal is data, never an instruction to execute provider output. The
//! scheduler must only consume an approved, validated snapshot.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use crate::error::{CruiseError, Result};

static WRITER: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Criterion {
    pub id: String,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub id: String,
    pub criterion_ids: Vec<String>,
    pub argv: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub instructions: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

/// A reviewed change to an approved Goal, bound to the exact tip and contract revision it
/// was reviewed against. Merely saving this record never authorizes anything.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectivePreview {
    pub id: String,
    pub revision: u64,
    pub goal_id: String,
    pub integrated_tip: String,
    /// Number of approved contract amendments when this preview was saved.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub contract_revision: u64,
    #[serde(flatten)]
    pub correction: Correction,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if callbacks receive a field reference"
)]
pub(crate) fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Correction {
    /// An additive task after a definite failed verification.
    Task(Task),
    /// Narrow wording changes, kept apart from the original proposal.
    Amendment(Vec<ContractEdit>),
    /// Instructions for resuming one failed attempt in its own worktree.
    Resume(AttemptCorrection),
    /// A task whose requirement the integrated tip may already satisfy.
    Evidence(EvidenceRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractTarget {
    Criterion(String),
    Task(String),
}

impl ContractTarget {
    /// Kind and ID as shown to the child and in PR evidence.
    pub(crate) fn label(&self) -> (&'static str, &str) {
        match self {
            Self::Criterion(id) => ("criterion", id),
            Self::Task(id) => ("task", id),
        }
    }
}

/// Replace one passage of a criterion description or of task instructions. Checks, IDs
/// and dependencies are not amendable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractEdit {
    #[serde(flatten)]
    pub target: ContractTarget,
    /// SHA-256 of the whole target text before this amendment. Set by the daemon.
    #[serde(default)]
    pub old_sha256: String,
    pub replace: String,
    pub with: String,
}

/// Everything the owner reviewed about the attempt, checked again until it launches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptCorrection {
    pub task_id: String,
    pub attempt: u32,
    pub session_id: String,
    pub base_sha: String,
    /// Step the saved checkpoint resumes from.
    pub step: String,
    pub head: String,
    /// SHA-256 over the worktree's uncommitted and untracked content.
    pub state: String,
    pub instructions: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRequest {
    pub task_id: String,
    pub attempt: u32,
    pub session_id: String,
    pub criteria: Vec<String>,
    /// Every approved check of those criteria. The requester cannot narrow this set.
    pub checks: Vec<String>,
    pub blobs: Vec<ReviewedBlob>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewedBlob {
    pub path: String,
    pub blob: String,
}

/// One execution of an approved evidence request. Only a complete passing result at the
/// approved tip resolves the task; it is never an integration and never replayed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRun {
    /// Approved preview that fixes the tip, contract, checks and reviewed blobs.
    pub revision: u64,
    pub task_id: String,
    pub worktree: PathBuf,
    /// Saved before the first effect, so a started run found again was interrupted.
    pub started: bool,
    pub checks: Vec<crate::manager_git::CheckEvidence>,
    pub outcome: Option<EvidenceOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceOutcome {
    Resolved,
    Unresolved(String),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub revision: u64,
    pub objective: String,
    pub target_branch: String,
    pub target_sha: String,
    #[serde(default)]
    pub exclusions: Vec<String>,
    pub criteria: Vec<Criterion>,
    pub checks: Vec<Check>,
    pub tasks: Vec<Task>,
    pub workflow: String,
    #[serde(default)]
    pub contract: Option<FrozenContract>,
}

/// The exact local authority shown to the human before approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrozenContract {
    pub anchor: PathBuf,
    pub origin: String,
    pub forge_kind: crate::forge::ForgeKind,
    pub forge_host: String,
    pub forge_repository: String,
    pub workflow: crate::config::WorkflowConfig,
}
impl Proposal {
    /// Validate the proposal's identifiers, criteria, task graph, and contract.
    ///
    /// # Errors
    /// Returns an error if the proposal is incomplete or internally inconsistent.
    pub fn validate(&self) -> Result<()> {
        fn visit<'a>(
            id: &'a String,
            tasks: &HashMap<&'a String, &'a Task>,
            visiting: &mut HashSet<&'a String>,
            done: &mut HashSet<&'a String>,
        ) -> bool {
            if done.contains(id) {
                return true;
            }
            if !visiting.insert(id) {
                return false;
            }
            let Some(task) = tasks.get(id) else {
                return false;
            };
            for dep in &task.dependencies {
                if !visit(dep, tasks, visiting, done) {
                    return false;
                }
            }
            visiting.remove(id);
            done.insert(id);
            true
        }
        let invalid =
            |message: &str| CruiseError::Other(format!("invalid manager proposal: {message}"));
        if self.id.trim().is_empty()
            || self.objective.trim().is_empty()
            || self.target_branch.trim().is_empty()
            || self.target_sha.trim().is_empty()
            || self.workflow.trim().is_empty()
        {
            return Err(invalid("missing identity, objective, target, or workflow"));
        }
        if self.criteria.is_empty() || self.checks.is_empty() || self.tasks.is_empty() {
            return Err(invalid("criteria, checks, and tasks must not be empty"));
        }
        let mut criteria = HashSet::new();
        for c in &self.criteria {
            if c.id.trim().is_empty() || c.description.trim().is_empty() || !criteria.insert(&c.id)
            {
                return Err(invalid("empty or duplicate criterion"));
            }
        }
        let mut checks = HashSet::new();
        for c in &self.checks {
            if c.id.trim().is_empty()
                || !checks.insert(&c.id)
                || c.argv.is_empty()
                || c.argv.first().is_some_and(String::is_empty)
                || c.criterion_ids.is_empty()
                || c.criterion_ids.iter().any(|id| !criteria.contains(id))
            {
                return Err(invalid("invalid check or criterion reference"));
            }
        }
        let mut tasks = HashMap::new();
        for t in &self.tasks {
            if t.id.trim().is_empty()
                || t.instructions.trim().is_empty()
                || tasks.insert(&t.id, t).is_some()
            {
                return Err(invalid("empty or duplicate task"));
            }
        }
        let mut visiting = HashSet::new();
        let mut done = HashSet::new();
        for id in tasks.keys() {
            if !visit(id, &tasks, &mut visiting, &mut done) {
                return Err(invalid("unknown dependency or cycle"));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub id: String,
    pub role: String,
    pub text: String,
    pub status: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    Verifying,
    AwaitingMerge,
    Completed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub proposal: Proposal,
    pub status: GoalStatus,
    pub reason: Option<String>,
    /// Coordinator-owned records, persisted with the approval under the same lock.
    #[serde(default)]
    pub execution: GoalExecution,
}

/// Durable execution intent. Missing fields on older records never authorize work.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoalExecution {
    pub anchor: Option<PathBuf>,
    pub origin: Option<String>,
    pub workflow: Option<crate::config::WorkflowConfig>,
    pub integrated_tip: Option<String>,
    #[serde(default)]
    pub attempts: Vec<TaskAttempt>,
    pub paused_from: Option<GoalStatus>,
    pub paused_reason: Option<String>,
    #[serde(default)]
    pub publication: Option<crate::manager_publication::Publication>,
    #[serde(default)]
    pub publication_history: Vec<crate::manager_publication::Publication>,
    #[serde(default)]
    pub corrective_preview: Option<CorrectivePreview>,
    /// Immutable audit trail of explicitly approved corrections, in approval order.
    #[serde(default)]
    pub corrective_approvals: Vec<CorrectivePreview>,
    /// Audit trail of unapproved previews discarded by an explicit verification retry.
    #[serde(default)]
    pub corrective_discarded: Vec<CorrectivePreview>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_runs: Vec<EvidenceRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAttempt {
    pub task_id: String,
    pub attempt: u32,
    pub session_id: String,
    pub base_sha: String,
    /// None denotes an older attempt whose launch state cannot be proven.
    #[serde(default)]
    pub launch_started: Option<bool>,
    /// An explicit resume has been accepted but not yet dispatched.
    #[serde(default)]
    pub resume_pending: bool,
    /// Dispatch began, but its outcome has not yet been recorded.
    #[serde(default)]
    pub resume_starting: bool,
    pub result_sha: Option<String>,
    #[serde(default)]
    pub finalization_intent: Option<ChildFinalizationIntent>,
    #[serde(default)]
    pub integrated: bool,
    pub integration_intent: Option<crate::manager_git::IntegrationIntent>,
    /// Approved contract amendments this attempt's plan was built with.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub contract_revision: u64,
    /// Approved corrections that are part of this attempt's plan, by preview revision.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corrections: Vec<u64>,
    /// Approved correction that the pending resume must deliver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correction_pending: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChildFinalizationIntent {
    pub expected_head: String,
    pub expected_tree: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub repository: PathBuf,
    pub transcript: Vec<Turn>,
    pub proposal: Option<Proposal>,
    pub goals: Vec<Goal>,
}

pub(crate) fn repository(cwd: &Path) -> Result<PathBuf> {
    let cwd = cwd.canonicalize()?;
    let output = Command::new("git")
        .arg("-C")
        .arg(&cwd)
        .args(["rev-parse", "--git-common-dir"])
        .output()?;
    if !output.status.success() {
        return Err(CruiseError::Other(
            "manager requires a local Git repository".into(),
        ));
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|_| CruiseError::Other("Git common directory is not UTF-8".into()))?;
    let path = PathBuf::from(value.trim());
    Ok(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
    .canonicalize()?)
}

fn git_text(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").arg("-C").arg(cwd).args(args).output()?;
    if !output.status.success() {
        return Err(CruiseError::Other(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(|e| CruiseError::Other(e.to_string()))
}

fn permanent_anchor(cwd: &Path, repo: &Path) -> Result<PathBuf> {
    let listing = git_text(cwd, &["worktree", "list", "--porcelain", "-z"])?;
    let first = listing
        .split('\0')
        .next()
        .and_then(|line| line.strip_prefix("worktree "))
        .ok_or_else(|| CruiseError::Other("cannot identify permanent checkout".into()))?;
    let anchor = PathBuf::from(first).canonicalize()?;
    if git_text(&anchor, &["rev-parse", "--is-bare-repository"])? != "false"
        || PathBuf::from(git_text(&anchor, &["rev-parse", "--show-toplevel"])?).canonicalize()?
            != anchor
        || repository(&anchor)? != repo
    {
        return Err(CruiseError::Other(
            "permanent checkout is unavailable or changed".into(),
        ));
    }
    Ok(anchor)
}

pub(crate) fn checkout_for(repository: &Path) -> Result<PathBuf> {
    permanent_anchor(repository, repository)
}

fn freeze_contract(cwd: &Path, repo: &Path) -> Result<FrozenContract> {
    let anchor = permanent_anchor(cwd, repo)?;
    let origin = git_text(&anchor, &["remote", "get-url", "origin"])?;
    if git_text(&anchor, &["remote", "get-url", "--push", "--all", "origin"])? != origin {
        return Err(CruiseError::Other(
            "origin push destination differs from fetch origin".into(),
        ));
    }
    let forge = crate::forge::resolve_remote(&origin, crate::forge::forge_override_from_env()?)?;
    let (yaml, source) =
        crate::resolver::resolve_config_in_client_context(Some("builtin:tdd"), &anchor)?;
    let mut workflow = crate::resolver::resolve_workflow_config_with_env(
        &yaml,
        &source,
        &anchor,
        Some(&anchor),
        crate::config::ConfigEnv::Process,
    )?;
    workflow.create_pr = false;
    workflow.cleanup_after_pr = false;
    workflow.force_exec = false;
    workflow.after_pr.clear();
    workflow.flags.shift_remove("after_pr");
    workflow.flags.shift_remove("merge");
    crate::config::validate_config(&workflow)?;
    Ok(FrozenContract {
        anchor,
        origin,
        forge_kind: forge.kind,
        forge_host: forge.host,
        forge_repository: forge.repository,
        workflow,
    })
}

fn verify_contract(cwd: &Path, repo: &Path, proposal: &Proposal) -> Result<()> {
    let contract = proposal.contract.as_ref().ok_or_else(|| {
        CruiseError::Other("proposal has no frozen execution contract; create a new preview".into())
    })?;
    let anchor = permanent_anchor(cwd, repo)?;
    if anchor != contract.anchor
        || git_text(&anchor, &["remote", "get-url", "origin"])? != contract.origin
        || git_text(&anchor, &["remote", "get-url", "--push", "--all", "origin"])?
            != contract.origin
    {
        return Err(CruiseError::Other(
            "checkout anchor or origin changed since preview; create a new preview".into(),
        ));
    }
    let forge =
        crate::forge::resolve_remote(&contract.origin, crate::forge::forge_override_from_env()?)?;
    if forge.kind != contract.forge_kind
        || forge.host != contract.forge_host
        || forge.repository != contract.forge_repository
    {
        return Err(CruiseError::Other(
            "forge identity changed since preview; create a new preview".into(),
        ));
    }
    let reference = format!("refs/remotes/origin/{}^{{commit}}", proposal.target_branch);
    if git_text(
        &anchor,
        &["rev-parse", "--verify", "--end-of-options", &reference],
    )? != proposal.target_sha
    {
        return Err(CruiseError::Other(
            "target branch changed since preview; create a new preview".into(),
        ));
    }
    crate::config::validate_config(&contract.workflow)?;
    Ok(())
}
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}
fn hex_digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
fn record_path(repo: &Path) -> Result<PathBuf> {
    let key = hex_digest(repo.to_string_lossy().as_bytes());
    Ok(crate::paths::data_dir()?
        .join("manager")
        .join(format!("{key}.json")))
}

impl Goal {
    fn amendments(&self) -> impl Iterator<Item = &Vec<ContractEdit>> {
        self.execution
            .corrective_approvals
            .iter()
            .filter_map(|approved| match &approved.correction {
                Correction::Amendment(edits) => Some(edits),
                _ => None,
            })
    }

    /// Number of approved contract amendments. Revision 0 is the original proposal.
    pub(crate) fn contract_revision(&self) -> u64 {
        self.amendments().count() as u64
    }

    /// Text of one criterion after the first `revision` approved amendments. The proposal
    /// itself is never rewritten, so every earlier revision stays readable.
    pub(crate) fn criterion_text(&self, id: &str, revision: u64) -> Result<String> {
        let original = self
            .proposal
            .criteria
            .iter()
            .find(|criterion| criterion.id == id)
            .ok_or_else(|| CruiseError::Other("unknown contract criterion".into()))?;
        self.amended(
            &original.description,
            revision,
            |target| matches!(target, ContractTarget::Criterion(criterion) if criterion == id),
        )
    }

    /// Instructions of one task after the first `revision` approved amendments.
    pub(crate) fn task_text(&self, id: &str, revision: u64) -> Result<String> {
        let original = self
            .proposal
            .tasks
            .iter()
            .find(|task| task.id == id)
            .ok_or_else(|| CruiseError::Other("unknown contract task".into()))?;
        self.amended(
            &original.instructions,
            revision,
            |target| matches!(target, ContractTarget::Task(task) if task == id),
        )
    }

    fn contract_text(&self, target: &ContractTarget, revision: u64) -> Result<String> {
        match target {
            ContractTarget::Criterion(id) => self.criterion_text(id, revision),
            ContractTarget::Task(id) => self.task_text(id, revision),
        }
    }

    /// Every target amended after revision `from` up to `to`, with its text at `to`.
    pub(crate) fn amended_between(
        &self,
        from: u64,
        to: u64,
    ) -> Result<Vec<(&ContractTarget, String)>> {
        let skip = usize::try_from(from).unwrap_or(usize::MAX);
        let take = usize::try_from(to.saturating_sub(from)).unwrap_or(usize::MAX);
        let mut changed: Vec<(&ContractTarget, String)> = Vec::new();
        for edit in self.amendments().skip(skip).take(take).flatten() {
            if !changed.iter().any(|(target, _)| *target == &edit.target) {
                changed.push((&edit.target, self.contract_text(&edit.target, to)?));
            }
        }
        Ok(changed)
    }

    fn amended(
        &self,
        original: &str,
        revision: u64,
        is_target: impl Fn(&ContractTarget) -> bool,
    ) -> Result<String> {
        let revision = usize::try_from(revision).unwrap_or(usize::MAX);
        let mut text = original.to_owned();
        for edits in self.amendments().take(revision) {
            let mut edits = edits
                .iter()
                .filter(|edit| is_target(&edit.target))
                .peekable();
            if edits.peek().is_some() {
                text = amend(&text, edits)?;
            }
        }
        Ok(text)
    }

    /// An approved evidence run resolved this task without a child integration.
    pub(crate) fn task_resolved(&self, task_id: &str) -> bool {
        self.execution
            .evidence_runs
            .iter()
            .any(|run| run.task_id == task_id && run.outcome == Some(EvidenceOutcome::Resolved))
    }

    /// Dependents may start: a child was integrated, or evidence resolved the task.
    pub(crate) fn task_done(&self, task_id: &str) -> bool {
        self.task_resolved(task_id)
            || self
                .execution
                .attempts
                .iter()
                .any(|attempt| attempt.task_id == task_id && attempt.integrated)
    }

    pub(crate) fn approved(&self, revision: u64) -> Option<&CorrectivePreview> {
        self.execution
            .corrective_approvals
            .iter()
            .find(|approved| approved.revision == revision)
    }

    /// The pending preview, only when it is exactly the one the human reviewed.
    pub(crate) fn reviewed_preview(
        &self,
        preview_id: &str,
        revision: u64,
    ) -> Result<&CorrectivePreview> {
        self.execution
            .corrective_preview
            .as_ref()
            .filter(|preview| {
                preview.id == preview_id
                    && preview.revision == revision
                    && preview.goal_id == self.id
            })
            .ok_or_else(|| CruiseError::Busy("stale or unknown corrective preview".into()))
    }
}

/// Apply one amendment's edits to the text they target. Each edit names the digest of the
/// text it was reviewed against and replaces a passage that occurs exactly once.
fn amend<'a>(text: &str, edits: impl Iterator<Item = &'a ContractEdit>) -> Result<String> {
    let reviewed = hex_digest(text.as_bytes());
    let mut amended = text.to_owned();
    for edit in edits {
        if edit.old_sha256 != reviewed
            || edit.replace.is_empty()
            || edit.replace == edit.with
            || amended.matches(&edit.replace).count() != 1
        {
            return Err(CruiseError::Other(
                "contract amendment does not match the reviewed text".into(),
            ));
        }
        amended = amended.replacen(&edit.replace, &edit.with, 1);
    }
    if amended.trim().is_empty() {
        return Err(CruiseError::Other(
            "contract amendment leaves an empty criterion or task".into(),
        ));
    }
    Ok(amended)
}

/// Bind requested edits to the exact text they amend at the current contract revision. A
/// requester may name the digest of the text it reviewed; any other text is refused.
pub(crate) fn amendment(goal: &Goal, mut edits: Vec<ContractEdit>) -> Result<Correction> {
    if edits.is_empty() {
        return Err(CruiseError::Other("contract amendment is empty".into()));
    }
    let revision = goal.contract_revision();
    for edit in &mut edits {
        if edit.old_sha256.is_empty() {
            edit.old_sha256 = hex_digest(goal.contract_text(&edit.target, revision)?.as_bytes());
        }
    }
    for edit in &edits {
        amend(
            &goal.contract_text(&edit.target, revision)?,
            edits.iter().filter(|other| other.target == edit.target),
        )?;
    }
    Ok(Correction::Amendment(edits))
}
fn load(repo: PathBuf) -> Result<Snapshot> {
    let path = record_path(&repo)?;
    match std::fs::read(&path) {
        Ok(bytes) => {
            let snapshot: Snapshot = serde_json::from_slice(&bytes).map_err(|e| {
                CruiseError::Other(format!("corrupt manager record {}: {e}", path.display()))
            })?;
            if snapshot.repository != repo {
                return Err(CruiseError::Other(
                    "manager record belongs to a different repository".into(),
                ));
            }
            Ok(snapshot)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Snapshot {
            repository: repo,
            transcript: vec![],
            proposal: None,
            goals: vec![],
        }),
        Err(e) => Err(e.into()),
    }
}
fn save(snapshot: &Snapshot) -> Result<()> {
    let path = record_path(&snapshot.repository)?;
    let dir = path
        .parent()
        .ok_or_else(|| CruiseError::Other("invalid manager storage path".into()))?;
    std::fs::create_dir_all(dir)?;
    let temp = dir.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(
            &serde_json::to_vec_pretty(snapshot).map_err(|e| CruiseError::Other(e.to_string()))?,
        )?;
        file.sync_all()?;
        std::fs::rename(&temp, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}
/// Load the Manager state for the repository containing `cwd`.
///
/// # Errors
/// Returns an error if the repository cannot be resolved or its state cannot be read.
pub fn snapshot(cwd: &Path) -> Result<Snapshot> {
    snapshot_for_repository(repository(cwd)?)
}

pub(crate) fn snapshot_for_repository(repo: PathBuf) -> Result<Snapshot> {
    let _guard = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    load(repo)
}

/// Read every record as one consistent scheduling inventory. A single corrupt
/// record makes the inventory unusable: otherwise its owned children could be
/// omitted from the daemon-wide concurrency count.
pub(crate) fn snapshots() -> Result<Vec<Snapshot>> {
    inventory(false)
}

/// Read valid persisted records without letting an unrelated record hide them.
pub(crate) fn readable_snapshots() -> Result<Vec<Snapshot>> {
    inventory(true)
}

fn inventory(skip_invalid: bool) -> Result<Vec<Snapshot>> {
    let _guard = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = crate::paths::data_dir()?.join("manager");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(error) if skip_invalid => {
                eprintln!(
                    "warning: cannot read Manager entry in {}: {error}",
                    directory.display()
                );
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    paths.sort();
    let mut snapshots = Vec::with_capacity(paths.len());
    for path in paths {
        match read_inventory_record(&path) {
            Ok(snapshot) => snapshots.push(snapshot),
            Err(error) if skip_invalid => {
                eprintln!(
                    "warning: skipping Manager record {}: {error}",
                    path.display()
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(snapshots)
}

fn read_inventory_record(path: &Path) -> Result<Snapshot> {
    let bytes = std::fs::read(path)?;
    let snapshot: Snapshot = serde_json::from_slice(&bytes).map_err(|error| {
        CruiseError::Other(format!(
            "corrupt manager record {}: {error}",
            path.display()
        ))
    })?;
    if record_path(&snapshot.repository)? != path {
        return Err(CruiseError::Other(format!(
            "Manager record identity does not match {}",
            path.display()
        )));
    }
    Ok(snapshot)
}

/// Validate execution states before the coordinator can continue them.
/// Discover records by their storage directory rather
/// than by the daemon's current working directory, which is not a checkout.
/// A corrupt or misnamed record is reported and is never replaced.
pub(crate) fn reconcile_startup() -> Result<Vec<String>> {
    let _guard = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = crate::paths::data_dir()?.join("manager");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut errors = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(error.to_string());
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let result = (|| -> Result<()> {
            let bytes = std::fs::read(&path)?;
            let mut snapshot: Snapshot = serde_json::from_slice(&bytes)?;
            if record_path(&snapshot.repository)? != path {
                return Err(CruiseError::Other(
                    "Manager record identity does not match its path".into(),
                ));
            }
            let mut changed = false;
            for goal in &mut snapshot.goals {
                let effective = if goal.status == GoalStatus::Paused {
                    goal.execution.paused_from.as_ref()
                } else {
                    Some(&goal.status)
                };
                if !matches!(
                    effective,
                    Some(GoalStatus::Active | GoalStatus::Verifying | GoalStatus::AwaitingMerge)
                ) {
                    continue;
                }
                let mut validation = goal.clone();
                validation.status = GoalStatus::Active;
                let problem = crate::manager_coordinator::ready_tasks(&validation, 1)
                    .err()
                    .map(|error| error.to_string())
                    .or_else(|| {
                        goal.execution.attempts.iter().find_map(|attempt| {
                            if !attempt.integrated
                                && attempt.finalization_intent.is_some()
                                && attempt.result_sha.is_none()
                            {
                                Some(format!(
                                    "task {} has an unresolved effect intent",
                                    attempt.task_id
                                ))
                            } else {
                                None
                            }
                        })
                    });
                if let Some(problem) = problem {
                    let reason = format!("Manager startup requires review: {problem}");
                    if goal.status == GoalStatus::Paused {
                        goal.execution.paused_from = Some(GoalStatus::Blocked);
                        goal.execution.paused_reason = Some(reason);
                    } else {
                        goal.status = GoalStatus::Blocked;
                        goal.reason = Some(reason);
                    }
                    changed = true;
                }
            }
            if changed {
                save(&snapshot)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            errors.push(format!("{}: {error}", path.display()));
        }
    }
    Ok(errors)
}

/// Modify a single Goal while holding the record's writer lock. The closure must
/// only prepare durable intent, never perform external side effects.
pub(crate) fn update_goal<T>(
    cwd: &Path,
    goal_id: &str,
    update: impl FnOnce(&mut Goal) -> Result<T>,
) -> Result<T> {
    let _guard = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut snapshot = load(repository(cwd)?)?;
    let goal = snapshot
        .goals
        .iter_mut()
        .find(|goal| goal.id == goal_id)
        .ok_or_else(|| CruiseError::Other("unknown Manager goal for this repository".into()))?;
    let result = update(goal)?;
    save(&snapshot)?;
    Ok(result)
}

/// Block one Goal after recovery fails, even if its checkout anchor is unavailable.
pub(crate) fn block_startup_goal(repository: &Path, goal_id: &str, problem: &str) -> Result<()> {
    let _guard = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut snapshot = load(repository.to_path_buf())?;
    let goal = snapshot
        .goals
        .iter_mut()
        .find(|goal| goal.id == goal_id)
        .ok_or_else(|| CruiseError::Other("Manager goal disappeared during startup".into()))?;
    let reason = format!("Manager startup requires review: {problem}");
    if goal.status == GoalStatus::Paused {
        goal.execution.paused_from = Some(GoalStatus::Blocked);
        goal.execution.paused_reason = Some(reason);
    } else if goal.status == GoalStatus::Active {
        goal.status = GoalStatus::Blocked;
        goal.reason = Some(reason);
    }
    save(&snapshot)
}

/// Persist pause before signalling owned operations. A completed Goal cannot be paused.
pub(crate) fn pause_goal(cwd: &Path, goal_id: &str) -> Result<Goal> {
    update_goal(cwd, goal_id, |goal| {
        if goal.status == GoalStatus::Completed {
            return Err(CruiseError::Other(
                "completed Manager goal cannot be paused".into(),
            ));
        }
        if goal.status != GoalStatus::Paused {
            goal.execution.paused_from = Some(goal.status.clone());
            goal.execution.paused_reason = goal.reason.take();
            goal.status = GoalStatus::Paused;
            goal.reason = Some("Paused by user".into());
        }
        Ok(goal.clone())
    })
}

/// Resume restores the exact prior state, not an implicit retry of a blocked task.
pub(crate) fn resume_goal(cwd: &Path, goal_id: &str) -> Result<Goal> {
    update_goal(cwd, goal_id, |goal| {
        if goal.status != GoalStatus::Paused {
            return Err(CruiseError::Other("Manager goal is not paused".into()));
        }
        goal.status = goal
            .execution
            .paused_from
            .take()
            .unwrap_or(GoalStatus::Blocked);
        goal.reason = goal.execution.paused_reason.take();
        if goal.status == GoalStatus::Blocked && goal.reason.is_none() {
            goal.reason = Some("Goal requires review before execution".into());
        }
        Ok(goal.clone())
    })
}
/// Persist a user message in the repository's Manager conversation.
///
/// # Errors
/// Returns an error for an empty message or if repository state cannot be updated.
pub fn message(cwd: &Path, text: String) -> Result<Turn> {
    if text.trim().is_empty() {
        return Err(CruiseError::Other("message must not be empty".into()));
    }
    let _guard = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut snapshot = load(repository(cwd)?)?;
    let turn = Turn {
        id: uuid::Uuid::new_v4().to_string(),
        role: "user".into(),
        text,
        status: "interrupted".into(),
    };
    snapshot.transcript.push(turn.clone());
    save(&snapshot)?;
    Ok(turn)
}

/// Complete a previously persisted user turn and append the provider response.
/// Interrupted turns remain visible if the daemon stops before this transition.
pub(crate) fn finish_message(cwd: &Path, id: &str, text: String) -> Result<Turn> {
    let _guard = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut snapshot = load(repository(cwd)?)?;
    let turn = snapshot
        .transcript
        .iter_mut()
        .find(|turn| turn.id == id && turn.role == "user")
        .ok_or_else(|| CruiseError::Other("manager turn no longer exists".into()))?;
    turn.status = "completed".into();
    let reply = Turn {
        id: uuid::Uuid::new_v4().to_string(),
        role: "assistant".into(),
        text,
        status: "completed".into(),
    };
    snapshot.transcript.push(reply.clone());
    save(&snapshot)?;
    Ok(reply)
}

pub(crate) fn fail_turn(cwd: &Path, id: &str, error: &str) -> Result<()> {
    let _guard = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut snapshot = load(repository(cwd)?)?;
    if !snapshot
        .transcript
        .iter()
        .any(|turn| turn.id == id && turn.role == "user")
    {
        return Err(CruiseError::Other("manager turn no longer exists".into()));
    }
    snapshot.transcript.push(Turn {
        id: uuid::Uuid::new_v4().to_string(),
        role: "assistant".into(),
        text: format!("Turn interrupted: {error}. Resubmit explicitly to retry."),
        status: "interrupted".into(),
    });
    save(&snapshot)
}

fn stage_preview(goal: &mut Goal, tip: &str, correction: Correction) -> Result<CorrectivePreview> {
    let revision = goal
        .execution
        .corrective_preview
        .iter()
        .chain(&goal.execution.corrective_approvals)
        .chain(&goal.execution.corrective_discarded)
        .map(|preview| preview.revision)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| CruiseError::Other("corrective preview revision exhausted".into()))?;
    let preview = CorrectivePreview {
        id: uuid::Uuid::new_v4().to_string(),
        revision,
        goal_id: goal.id.clone(),
        integrated_tip: tip.to_owned(),
        contract_revision: goal.contract_revision(),
        correction,
    };
    goal.execution.corrective_preview = Some(preview.clone());
    Ok(preview)
}

fn reviewed_state_unchanged(goal: &Goal, tip: &str, contract_revision: u64) -> Result<()> {
    if goal.execution.integrated_tip.as_deref() != Some(tip)
        || goal.contract_revision() != contract_revision
    {
        return Err(CruiseError::Busy(
            "Goal tip or contract changed; refresh before corrective review".into(),
        ));
    }
    Ok(())
}

/// Persist a corrective task preview without authorizing execution.
pub(crate) fn preview_corrective(
    cwd: &Path,
    goal_id: &str,
    expected_tip: &str,
    task: Task,
) -> Result<CorrectivePreview> {
    update_goal(cwd, goal_id, |goal| {
        let tip = crate::manager_publication::corrective_tip(goal)?;
        if tip != expected_tip {
            return Err(CruiseError::Busy(
                "Goal tip changed; refresh before corrective review".into(),
            ));
        }
        if task.id.trim().is_empty()
            || task.instructions.trim().is_empty()
            || goal
                .proposal
                .tasks
                .iter()
                .any(|existing| existing.id == task.id)
            || task.dependencies.iter().any(|id| {
                !goal
                    .proposal
                    .tasks
                    .iter()
                    .any(|existing| &existing.id == id)
            })
        {
            return Err(CruiseError::Other(
                "invalid corrective task or dependencies".into(),
            ));
        }
        stage_preview(goal, expected_tip, Correction::Task(task))
    })
}

/// Persist a correction built from the Goal as it was at `expected_tip` and
/// `contract_revision`. Like every preview, it authorizes nothing.
pub(crate) fn preview_correction(
    cwd: &Path,
    goal_id: &str,
    expected_tip: &str,
    contract_revision: u64,
    correction: Correction,
) -> Result<CorrectivePreview> {
    update_goal(cwd, goal_id, |goal| {
        reviewed_state_unchanged(goal, expected_tip, contract_revision)?;
        stage_preview(goal, expected_tip, correction)
    })
}

/// Move exactly the reviewed preview into the approval trail. `authorize` records what the
/// approval allows in the same write, so neither can exist without the other.
pub(crate) fn approve_correction(
    cwd: &Path,
    goal_id: &str,
    preview_id: &str,
    revision: u64,
    authorize: impl FnOnce(&mut Goal, &CorrectivePreview) -> Result<()>,
) -> Result<Goal> {
    update_goal(cwd, goal_id, |goal| {
        let preview = goal.reviewed_preview(preview_id, revision)?.clone();
        reviewed_state_unchanged(goal, &preview.integrated_tip, preview.contract_revision)?;
        authorize(goal, &preview)?;
        goal.execution.corrective_approvals.push(preview);
        goal.execution.corrective_preview = None;
        Ok(goal.clone())
    })
}

/// Approve exactly the corrective task displayed to the human, without changing the
/// approved checks or discarding any prior execution and verification evidence.
pub(crate) fn approve_corrective(
    cwd: &Path,
    goal_id: &str,
    preview_id: &str,
    revision: u64,
) -> Result<Goal> {
    update_goal(cwd, goal_id, |goal| {
        if goal
            .execution
            .corrective_approvals
            .iter()
            .any(|approved| approved.id == preview_id && approved.revision == revision)
        {
            return Ok(goal.clone());
        }
        let preview = goal.reviewed_preview(preview_id, revision)?.clone();
        let Correction::Task(task) = &preview.correction else {
            return Err(CruiseError::Other(
                "corrective preview is not an additive task".into(),
            ));
        };
        if crate::manager_publication::corrective_tip(goal)? != preview.integrated_tip.as_str() {
            return Err(CruiseError::Busy(
                "Goal tip changed; refresh before corrective review".into(),
            ));
        }
        let mut proposal = goal.proposal.clone();
        proposal.tasks.push(task.clone());
        proposal.validate()?;
        let publication = goal
            .execution
            .publication
            .take()
            .ok_or_else(|| CruiseError::Other("verification attempt disappeared".into()))?;
        goal.execution.publication_history.push(publication);
        goal.execution.corrective_approvals.push(preview);
        goal.execution.corrective_preview = None;
        goal.proposal = proposal;
        goal.status = GoalStatus::Active;
        goal.reason = None;
        Ok(goal.clone())
    })
}

/// Persist a validated preview. Identity and revision are assigned by the daemon,
/// not by the provider. This does not authorize any execution.
///
/// # Errors
/// Returns an error if the proposal is invalid or repository state cannot be updated.
pub fn save_proposal(cwd: &Path, proposal: Proposal) -> Result<Proposal> {
    save_proposal_for_turn(cwd, proposal, None)
}

pub(crate) fn save_proposal_for_turn(
    cwd: &Path,
    mut proposal: Proposal,
    turn_id: Option<&str>,
) -> Result<Proposal> {
    let _guard = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut snapshot = load(repository(cwd)?)?;
    proposal.workflow = "builtin:tdd".into();
    proposal.contract = Some(freeze_contract(cwd, &snapshot.repository)?);
    proposal.id = turn_id.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    proposal.revision = snapshot
        .proposal
        .iter()
        .chain(snapshot.goals.iter().map(|goal| &goal.proposal))
        .map(|proposal| proposal.revision)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| CruiseError::Other("proposal revision exhausted".into()))?;
    proposal.validate()?;
    verify_contract(cwd, &snapshot.repository, &proposal)?;
    snapshot.proposal = Some(proposal.clone());
    if let Some(id) = turn_id {
        let turn = snapshot
            .transcript
            .iter_mut()
            .find(|turn| turn.id == id && turn.role == "user")
            .ok_or_else(|| CruiseError::Other("manager turn no longer exists".into()))?;
        turn.status = "completed".into();
    }
    save(&snapshot)?;
    Ok(proposal)
}

/// Approve the reviewed proposal and durably register its initial Goal.
/// Re-approval of the same proposal returns the existing Goal.
///
/// # Errors
/// Returns an error if the proposal is stale, its reviewed contract changed,
/// or state cannot be persisted.
pub fn approve(cwd: &Path, proposal_id: &str, revision: u64) -> Result<Goal> {
    let _guard = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut snapshot = load(repository(cwd)?)?;
    if let Some(goal) = snapshot
        .goals
        .iter()
        .find(|goal| goal.proposal.id == proposal_id && goal.proposal.revision == revision)
    {
        return Ok(goal.clone());
    }

    let proposal = snapshot
        .proposal
        .as_ref()
        .filter(|proposal| proposal.id == proposal_id && proposal.revision == revision)
        .ok_or_else(|| CruiseError::Busy("stale or unknown manager proposal".into()))?
        .clone();
    proposal.validate()?;
    verify_contract(cwd, &snapshot.repository, &proposal)?;
    let contract = proposal
        .contract
        .as_ref()
        .ok_or_else(|| CruiseError::Other("proposal has no frozen execution contract".into()))?;
    let execution = GoalExecution {
        anchor: Some(contract.anchor.clone()),
        origin: Some(contract.origin.clone()),
        workflow: Some(contract.workflow.clone()),
        integrated_tip: Some(proposal.target_sha.clone()),
        ..GoalExecution::default()
    };
    let goal = Goal {
        id: uuid::Uuid::new_v4().to_string(),
        proposal,
        status: GoalStatus::Active,
        reason: None,
        execution,
    };
    snapshot.proposal = None;
    snapshot.goals.push(goal.clone());
    save(&snapshot)?;
    Ok(goal)
}
