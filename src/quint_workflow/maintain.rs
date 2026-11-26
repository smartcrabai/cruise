//! Keep covered repository Quint models in step with the code before publication
//! (`tests/formal/formal_drift.qnt`).
//!
//! `.cruise/formal-coverage.toml` maps each self-contained repository model to the
//! code it describes and the commit where it was last confirmed. When a
//! `formal_methods` run changes covered code, the host runs the model as it is, an
//! agent classifies every covered change and edits only verification files, and the
//! host reruns it. A pass must also fail on the agent's scratch break and needs
//! owner approval of the new diagram; a failure is judged as model drift (repair, at
//! most twice) or a code bug (stop). Only in-sync or approved models move their
//! sync commit, and nothing else lets the run publish.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use serde::Deserialize;

use super::{Agent, Image, MAX_PNG, MODEL_LIMIT, Tools, error, sha256, tools};
use crate::artifacts;
use crate::error::{CruiseError, Result};
use crate::trusted_git::{git_output, worktree_tree};

pub(crate) const COVERAGE: &str = ".cruise/formal-coverage.toml";
const REPORT: &str = "formal-drift/report.md";

/// Owner decision on an updated model's diagram: `true` approves.
pub(crate) type Approve<'a> = dyn Fn(Image) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool>> + Send + 'a>>
    + Send
    + Sync
    + 'a;
const DONE: &str = "formal-drift/done";
const MAX_REPAIRS: usize = 2;
const CLASSES: [&str; 4] = [
    "no effect",
    "shape changed",
    "promise changed",
    "promise maybe broken",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Coverage {
    #[serde(default, rename = "model")]
    models: Vec<Entry>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: String,
    covers: Vec<String>,
    sync: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Maintained {
    classes: Vec<String>,
    property_changed: bool,
    evidence: String,
    mutation: Option<Mutation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Mutation {
    find: String,
    replace: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Judged {
    every_step_possible: bool,
    counterexample: String,
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    InSync,
    Updated,
}

impl Outcome {
    fn label(&self) -> &'static str {
        match self {
            Self::InSync => "in sync",
            Self::Updated => "updated",
        }
    }
}

fn load(workspace: &Path) -> Result<Option<Coverage>> {
    let path = workspace.join(COVERAGE);
    if !path.is_file() {
        return Ok(None);
    }
    let text = artifacts::read(workspace, COVERAGE)?;
    let coverage: Coverage =
        toml::from_str(&text).map_err(|err| error(format!("Invalid {COVERAGE}: {err}")))?;
    for entry in &coverage.models {
        let relative = Path::new(&entry.path);
        if relative.is_absolute()
            || entry
                .path
                .split('/')
                .any(|part| part == ".." || part.is_empty())
            || relative.extension().is_none_or(|ext| ext != "qnt")
            || entry.covers.is_empty()
            || entry
                .covers
                .iter()
                .any(|cover| cover.is_empty() || glob::Pattern::new(cover).is_err())
            || !crate::trusted_git::valid_oid(&entry.sync)
        {
            return Err(error(format!(
                "Invalid {COVERAGE} entry for `{}`: use a relative .qnt path, nonempty glob covers, and a full sync commit",
                entry.path
            )));
        }
    }
    Ok(Some(coverage))
}

/// Paths the task changed relative to `base` (tracked and untracked).
fn changed_paths(workspace: &Path, base: &str) -> Result<BTreeSet<String>> {
    let mut paths = BTreeSet::new();
    for args in [
        &["diff", "--name-only", "-z", "--no-renames", base, "--"][..],
        &["ls-files", "--others", "--exclude-standard", "-z"],
    ] {
        let output = git_output(workspace, args)?;
        paths.extend(
            output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
                .map(|path| String::from_utf8_lossy(path).into_owned()),
        );
    }
    Ok(paths)
}

/// Whether a code change (not the model or coverage record itself) hits `entry`.
fn covered(entry: &Entry, changed: &BTreeSet<String>) -> Vec<String> {
    let options = glob::MatchOptions {
        require_literal_separator: true,
        ..glob::MatchOptions::default()
    };
    changed
        .iter()
        .filter(|path| path.as_str() != entry.path && path.as_str() != COVERAGE)
        .filter(|path| {
            entry
                .covers
                .iter()
                .any(|cover| glob::Pattern::new(cover).is_ok_and(|p| p.matches_with(path, options)))
        })
        .cloned()
        .collect()
}

fn diff_paths(workspace: &Path, before: &str, after: &str) -> Result<Vec<String>> {
    let output = git_output(
        workspace,
        &["diff", "--name-only", "-z", "--no-renames", before, after],
    )?;
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect())
}

fn validate(update: &Maintained) -> std::result::Result<(), String> {
    if update.classes.is_empty()
        || update
            .classes
            .iter()
            .any(|c| !CLASSES.contains(&c.as_str()))
    {
        return Err(format!(
            "classes must be a nonempty subset of: {}",
            CLASSES.join(", ")
        ));
    }
    if update.property_changed
        && (!update.classes.iter().any(|c| c == "promise changed")
            || update.evidence.trim().is_empty())
    {
        return Err(
            "a property may change only for a `promise changed` class with cited evidence".into(),
        );
    }
    Ok(())
}

fn mutate(source: &[u8], mutation: Option<&Mutation>) -> Result<Vec<u8>> {
    let mutation = mutation.ok_or_else(|| {
        error("Formal maintenance blocked: an updated model needs a scratch break to prove it can fail")
    })?;
    let text = std::str::from_utf8(source).map_err(|_| error("Quint model is not UTF-8"))?;
    if mutation.find.is_empty() || text.matches(&mutation.find).count() != 1 {
        return Err(error(
            "Formal maintenance blocked: the scratch break must replace exactly one occurrence in the model",
        ));
    }
    Ok(text
        .replacen(&mutation.find, &mutation.replace, 1)
        .into_bytes())
}

fn parse<T: for<'de> Deserialize<'de>>(output: &str, what: &str) -> Result<T> {
    if output.len() > MODEL_LIMIT {
        return Err(error(format!("Oversized {what}")));
    }
    serde_json::from_str(output.trim()).map_err(|err| error(format!("Invalid {what}: {err}")))
}

/// Move `entry`'s sync commit to `head` in the coverage text.
fn with_sync(text: &str, entry: &Entry, head: &str) -> Result<String> {
    let path_line = format!("path = \"{}\"", entry.path);
    let sync_line = format!("sync = \"{}\"", entry.sync);
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let start = lines
        .iter()
        .position(|line| line.trim() == path_line)
        .ok_or_else(|| error(format!("Cannot find `{path_line}` in {COVERAGE}")))?;
    let sync = (start + 1..lines.len())
        .take_while(|&index| lines[index].trim() != "[[model]]")
        .find(|&index| lines[index].trim() == sync_line)
        .ok_or_else(|| error(format!("Cannot find the sync commit for `{}`", entry.path)))?;
    lines[sync] = format!("sync = \"{head}\"");
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

fn maintain_prompt(
    entry: &Entry,
    changed: &[String],
    as_is: &str,
    feedback: Option<&str>,
) -> String {
    let mut prompt = format!(
        "Bring the repository Quint model `{model}` back in line with the code this task changed.\n\n\
Coverage record ({COVERAGE}): covers {covers}; last confirmed at commit `{sync}`.\n\
Covered paths this task changed: {changed}\n\
The host ran the model as it is (typecheck and test): {as_is}\n\n\
Read `git diff {sync}` for every covered path and each commit message; also look for moved or renamed covered code and new code that writes the state the model guards. Classify each change as `no effect`, `shape changed` (an atomic step split, a branch added or removed, a check moved, a new path to guarded state, a new value range), `promise changed` (intended, with evidence in a doc comment, test, changelog, or commit message), or `promise maybe broken`.\n\n\
Edit only `{model}` and `{COVERAGE}`. Never edit product code, tests, or other files; the host rejects any other change. Update the model from the code, not from what makes it pass: keep one indivisible code step as one action, and keep the properties and bounds. Change a property only for `promise changed`, and quote the evidence. Do not weaken a property or the model to pass.\n\n\
If you changed the model, also give a scratch break: one exact text replacement in the model that removes the guard a property depends on, so the model's tests must fail. Do not apply it.\n\n\
Return only one JSON object (no fences):\n\
{{\"classes\":[\"shape changed\"],\"property_changed\":false,\"evidence\":\"\",\"mutation\":{{\"find\":\"exact model text\",\"replace\":\"broken text\"}}}}\n\
Use `\"mutation\":null` only when the model is unchanged.",
        model = entry.path,
        covers = entry.covers.join(", "),
        sync = entry.sync,
        changed = changed.join(", "),
    );
    if let Some(feedback) = feedback {
        let _ = write!(
            prompt,
            "\n\nThe previous update was model drift. Fix the verification, not the code:\n{feedback}"
        );
    }
    prompt
}

fn judge_prompt(entry: &Entry, failure: &str) -> String {
    format!(
        "The updated repository Quint model `{}` fails:\n\n{failure}\n\n\
Replay the failing test or counterexample against the real code, step by step. If some step cannot happen in the code, the model is wrong (drift). If every step can happen, the code breaks the property (bug). Read only; do not edit anything.\n\n\
Return only one JSON object (no fences): {{\"every_step_possible\":true,\"counterexample\":\"the steps in the code's own words, the harm in one sentence, and the commit that introduced it\"}}",
        entry.path
    )
}

/// Bring every model the task's covered changes reach back in line, or stop.
pub(crate) async fn maintain(
    agent: &Agent<'_>,
    tools: &Tools,
    root: &Path,
    base: &str,
    approve: &Approve<'_>,
    log: &crate::step::command::StepLogCallback<'_>,
) -> Result<()> {
    let workspace = agent.workspace;
    let Some(coverage) = load(workspace)? else {
        return Ok(());
    };
    let fingerprint = crate::file_tracker::fingerprint(&workspace.canonicalize()?)?;
    if artifacts::read(root, DONE).is_ok_and(|done| done == fingerprint) {
        return Ok(());
    }
    let changed = changed_paths(workspace, base)?;
    let mut report = String::new();
    for entry in &coverage.models {
        let hits = covered(entry, &changed);
        if hits.is_empty() {
            continue;
        }
        log(
            "info",
            &format!(
                "Formal maintenance: `{}` covers changed {}",
                entry.path,
                hits.join(", ")
            ),
        );
        let outcome = maintain_one(agent, tools, root, entry, &hits, approve).await?;
        let head = String::from_utf8_lossy(
            &git_output(workspace, &["rev-parse", "--verify", "HEAD"])?.stdout,
        )
        .trim()
        .to_owned();
        let text = artifacts::read(workspace, COVERAGE)?;
        let current: Coverage =
            toml::from_str(&text).map_err(|err| error(format!("Invalid {COVERAGE}: {err}")))?;
        let entry = current
            .models
            .iter()
            .find(|candidate| candidate.path == entry.path)
            .ok_or_else(|| error(format!("{COVERAGE} lost the entry for `{}`", entry.path)))?;
        artifacts::write(workspace, COVERAGE, &with_sync(&text, entry, &head)?)?;
        let _ = writeln!(
            report,
            "- `{}`: {}; sync `{head}`",
            entry.path,
            outcome.label()
        );
        log(
            "info",
            &format!("Formal maintenance: `{}` {}", entry.path, outcome.label()),
        );
    }
    if !report.is_empty() {
        artifacts::write(root, REPORT, &report)?;
    }
    artifacts::write(
        root,
        DONE,
        &crate::file_tracker::fingerprint(&workspace.canonicalize()?)?,
    )
}

async fn maintain_one(
    agent: &Agent<'_>,
    tools: &Tools,
    root: &Path,
    entry: &Entry,
    hits: &[String],
    approve: &Approve<'_>,
) -> Result<Outcome> {
    let workspace = agent.workspace;
    let original = artifacts::read_bytes(workspace, &entry.path, MODEL_LIMIT)?;
    let as_is = check(tools, root, original.clone(), agent).await?;
    let as_is_text = match &as_is {
        Ok(tests) => format!("passed ({tests} tests)"),
        Err(failure) => format!("failed: {failure}"),
    };
    let mut feedback = None;
    let mut repairs = 0;
    loop {
        let before = worktree_tree(workspace)?;
        let output = agent
            .run(
                &maintain_prompt(entry, hits, &as_is_text, feedback.as_deref()),
                "formal-maintain",
                false,
                Vec::new(),
            )
            .await?;
        let after = worktree_tree(workspace)?;
        let edited = diff_paths(workspace, &before, &after)?;
        if let Some(path) = edited
            .iter()
            .find(|path| path.as_str() != entry.path && path.as_str() != COVERAGE)
        {
            return Err(error(format!(
                "Formal maintenance blocked: it may edit only `{}` and {COVERAGE}, but changed `{path}`",
                entry.path
            )));
        }
        let update: Maintained = parse(&output, "formal maintenance report")?;
        validate(&update)
            .map_err(|reason| error(format!("Formal maintenance blocked: {reason}")))?;
        let source = artifacts::read_bytes(workspace, &entry.path, MODEL_LIMIT)?;
        if update.classes == ["no effect"] && as_is.is_ok() && source == original {
            return Ok(Outcome::InSync);
        }
        match check(tools, root, source.clone(), agent).await? {
            Ok(_) => {
                let broken = mutate(&source, update.mutation.as_ref())?;
                if check(tools, root, broken, agent).await?.is_ok() {
                    return Err(error(format!(
                        "Formal maintenance blocked: `{}` still passes with its scratch break, so it cannot fail",
                        entry.path
                    )));
                }
                let png = tools::check(tools, root, source, true, agent.token)
                    .await?
                    .png
                    .ok_or_else(|| error("Missing Quint review PNG"))?;
                let sha256 = sha256(&png);
                let name = format!("formal-drift/{sha256}.png");
                artifacts::write_bytes(root, &name, &png, MAX_PNG)?;
                if !approve(Image { name, sha256 }).await? {
                    return Err(error(format!(
                        "Updated Quint model `{}` rejected; pull-request creation stopped",
                        entry.path
                    )));
                }
                return Ok(Outcome::Updated);
            }
            Err(failure) => {
                let judged: Judged = parse(
                    &agent
                        .run(
                            &judge_prompt(entry, &failure),
                            "formal-judge",
                            true,
                            Vec::new(),
                        )
                        .await?,
                    "formal judgement",
                )?;
                if judged.every_step_possible {
                    return Err(error(format!(
                        "Code breaks the property in `{}`: {}",
                        entry.path,
                        crate::telemetry::stderr::sanitize(&judged.counterexample).body
                    )));
                }
                repairs += 1;
                if repairs > MAX_REPAIRS {
                    return Err(error(format!(
                        "Formal maintenance blocked: `{}` still drifts after {MAX_REPAIRS} repairs",
                        entry.path
                    )));
                }
                feedback = Some(format!(
                    "{failure}\nWhy it is drift: {}",
                    judged.counterexample
                ));
            }
        }
    }
}

/// Typecheck and test a model; a failing model is `Ok(Err(log))`, a host error is `Err`.
async fn check(
    tools: &Tools,
    root: &Path,
    source: Vec<u8>,
    agent: &Agent<'_>,
) -> Result<std::result::Result<usize, String>> {
    if !tools::self_contained(&source) {
        return Err(error(
            "Formal maintenance blocked: covered models must be self-contained (no imports)",
        ));
    }
    match tools::check(tools, root, source, false, agent.token).await {
        Ok(checked) => Ok(Ok(checked.tests)),
        Err(CruiseError::Interrupted) => Err(CruiseError::Interrupted),
        Err(failure) => Ok(Err(crate::telemetry::stderr::sanitize(
            &failure.to_string(),
        )
        .body)),
    }
}

/// Formal maintenance outcomes for the PR/MR body, if any ran.
pub(crate) fn report(root: &Path) -> Option<String> {
    artifacts::read(root, REPORT).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry {
        Entry {
            path: "tests/formal/gate.qnt".into(),
            covers: vec!["src/gate.rs".into(), "builtin/**/*.yaml".into()],
            sync: "a".repeat(40),
        }
    }

    #[test]
    fn covered_ignores_the_model_and_record_and_matches_globs() {
        let changed: BTreeSet<String> = [
            "tests/formal/gate.qnt",
            COVERAGE,
            "src/gate.rs",
            "builtin/_shared/after-pr.yaml",
            "src/other.rs",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(
            covered(&entry(), &changed),
            ["builtin/_shared/after-pr.yaml", "src/gate.rs"]
        );
        let none: BTreeSet<String> = ["tests/formal/gate.qnt".to_owned()].into();
        assert_eq!(covered(&entry(), &none), Vec::<String>::new());
    }

    #[test]
    fn property_change_needs_intended_promise_and_evidence() {
        let update = |classes: &[&str], property_changed, evidence: &str| Maintained {
            classes: classes.iter().map(|c| (*c).to_owned()).collect(),
            property_changed,
            evidence: evidence.into(),
            mutation: None,
        };
        assert!(validate(&update(&["shape changed"], false, "")).is_ok());
        assert!(validate(&update(&["promise maybe broken"], true, "x")).is_err());
        assert!(validate(&update(&["promise changed"], true, "")).is_err());
        assert!(validate(&update(&["promise changed"], true, "CHANGELOG says so")).is_ok());
        assert!(validate(&update(&[], false, "")).is_err());
        assert!(validate(&update(&["typo"], false, "")).is_err());
    }

    #[test]
    fn scratch_break_must_replace_exactly_one_occurrence() {
        let model = b"guard and guard";
        let once = Mutation {
            find: "and guard".into(),
            replace: "and true".into(),
        };
        assert_eq!(
            mutate(model, Some(&once)).ok().as_deref(),
            Some(&b"guard and true"[..])
        );
        let twice = Mutation {
            find: "guard".into(),
            replace: "true".into(),
        };
        assert!(mutate(model, Some(&twice)).is_err());
        assert!(mutate(model, None).is_err());
    }

    #[test]
    fn sync_moves_only_for_the_named_entry() {
        let other = "b".repeat(40);
        let text = format!(
            "[[model]]\npath = \"tests/formal/other.qnt\"\ncovers = [\"src/o.rs\"]\nsync = \"{}\"\n\n[[model]]\npath = \"tests/formal/gate.qnt\"\ncovers = [\"src/gate.rs\"]\nsync = \"{}\"\n",
            "a".repeat(40),
            "a".repeat(40)
        );
        let updated = with_sync(&text, &entry(), &other).unwrap_or_else(|e| panic!("{e}"));
        let coverage: Coverage = toml::from_str(&updated).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(coverage.models[0].sync, "a".repeat(40));
        assert_eq!(coverage.models[1].sync, other);
    }

    #[test]
    fn repository_coverage_record_names_existing_self_contained_models() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        let coverage = load(repo)
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("missing {COVERAGE}"));
        assert!(!coverage.models.is_empty());
        for entry in &coverage.models {
            let source = std::fs::read(repo.join(&entry.path))
                .unwrap_or_else(|e| panic!("{}: {e}", entry.path));
            assert!(
                super::tools::self_contained(&source),
                "{} imports",
                entry.path
            );
            for cover in &entry.covers {
                assert!(
                    glob::glob(&repo.join(cover).to_string_lossy())
                        .is_ok_and(|mut paths| paths.next().is_some()),
                    "{} covers nothing at {cover}",
                    entry.path
                );
            }
        }
    }

    #[test]
    fn coverage_rejects_unsafe_entries() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        assert!(load(dir.path()).unwrap_or_else(|e| panic!("{e}")).is_none());
        std::fs::create_dir_all(dir.path().join(".cruise")).unwrap_or_else(|e| panic!("{e}"));
        for bad in [
            "[[model]]\npath = \"../x.qnt\"\ncovers = [\"src/a.rs\"]\nsync = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n",
            "[[model]]\npath = \"m.qnt\"\ncovers = []\nsync = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n",
            "[[model]]\npath = \"m.qnt\"\ncovers = [\"src/a.rs\"]\nsync = \"main\"\n",
        ] {
            std::fs::write(dir.path().join(COVERAGE), bad).unwrap_or_else(|e| panic!("{e}"));
            assert!(load(dir.path()).is_err(), "{bad}");
        }
    }
}
