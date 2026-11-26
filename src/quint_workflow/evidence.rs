use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Agent, Files, MODEL_LIMIT, Preview, Run, Stage, Tools, error, sha256, tools};
use crate::artifacts;
use crate::error::Result;
use crate::session::SessionState;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Receipt {
    workspace_sha256: String,
    report_sha256: String,
    tests: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    model: String,
    ci: String,
    blockers: Vec<String>,
    checks: Vec<Check>,
    measurements: Vec<Measurement>,
    recordings: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Check {
    command: String,
    exit: i32,
    excerpt: String,
    earlier: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Measurement {
    label: String,
    before: Option<f64>,
    after: f64,
}

pub(super) async fn finish(agent: &Agent<'_>, files: &Files, preview: &Preview) -> Result<Receipt> {
    let model = files.approved(preview)?;
    let source = files.read_model(&format!("{}.qnt", preview.model_sha256))?;
    let mut vars = crate::variable::VariableStore::default();
    vars.set_named_file(super::APPROVED_VAR, model.clone());
    let mut prompt = vars.resolve(include_str!("../../prompts/quint-final.md"))?;
    prompt.push_str("\nRead-only earlier reports, when present:");
    let names: Vec<_> = [
        "test-first.md",
        "implementation.md",
        "refactor/baseline.md",
        "e2e/report.md",
        "refactor/e2e-verification.md",
        "refactor/final-verification.md",
    ]
    .into_iter()
    .map(|name| {
        if name.contains('/') {
            name.to_owned()
        } else {
            files.name(name)
        }
    })
    .map(|name| files.root.join(name))
    .filter(|path| path.is_file())
    .collect();
    for file in &names {
        let _ = write!(prompt, "\n- {}", file.display());
    }
    let mut inputs = names;
    inputs.push(model);
    let output = agent.run(&prompt, "quint-final", false, inputs).await?;
    let report = parse_report(&output)?;
    if !report.blockers.is_empty()
        || report
            .checks
            .iter()
            .any(|check| !check.earlier && check.exit != 0)
    {
        files.write("final-agent.json", output.as_bytes(), MODEL_LIMIT)?;
        return Err(error(format!(
            "Final Quint verification blocked: {}",
            crate::telemetry::stderr::sanitize(&report.blockers.join("; ")).body
        )));
    }
    files.approved(preview)?;
    let workspace = agent.workspace.canonicalize()?;
    if Path::new(&report.model)
        .extension()
        .is_none_or(|extension| extension != "qnt")
    {
        return Err(error(
            "Final Quint report must name the repository's .qnt model",
        ));
    }
    if artifacts::read_bytes(&workspace, &report.model, MODEL_LIMIT)? != source {
        return Err(error(
            "Repository Quint model differs from the approved revision",
        ));
    }
    let ci = artifacts::read(&workspace, &report.ci)?;
    if !ci.contains(&report.model) {
        return Err(error(
            "Reported CI command or script does not reference the approved repository model",
        ));
    }
    let workspace_sha256 = crate::file_tracker::fingerprint(&workspace)?;
    let tools = Tools::load()?;
    let checked = tools::check(&tools, &files.root, source, false, agent.token).await?;
    if workspace_sha256 != crate::file_tracker::fingerprint(&workspace)? {
        return Err(error(
            "Workspace changed during final Quint verification; retry it",
        ));
    }
    files.write("final-agent.json", output.as_bytes(), MODEL_LIMIT)?;
    files.write("final.log", checked.log.as_bytes(), MODEL_LIMIT)?;
    Ok(Receipt {
        workspace_sha256,
        report_sha256: sha256(output.as_bytes()),
        tests: checked.tests,
    })
}

fn parse_report(output: &str) -> Result<Report> {
    if output.len() > MODEL_LIMIT {
        return Err(error("Oversized final Quint report"));
    }
    serde_json::from_str(output).map_err(|err| error(format!("Invalid final Quint report: {err}")))
}

pub(crate) fn publication_evidence(
    session: &SessionState,
    root: Option<&Path>,
    workspace: &Path,
) -> Result<Option<String>> {
    let Some(Run::On { stage }) = &session.quint_run else {
        return Ok(None);
    };
    let Stage::Verified { preview, receipt } = stage else {
        return Err(error(
            "The approved Quint model has not passed final verification",
        ));
    };
    let root = root.ok_or_else(|| error("Missing Quint evidence directory"))?;
    let files = Files::new(root, session.execution_id.as_deref())?;
    files.approved(preview)?;
    if crate::file_tracker::fingerprint(&workspace.canonicalize()?)? != receipt.workspace_sha256 {
        return Err(error(
            "Workspace changed after final Quint verification; restore the verified files or reset the run",
        ));
    }
    let data = artifacts::read(root, &files.name("final-agent.json"))?;
    if sha256(data.as_bytes()) != receipt.report_sha256 {
        return Err(error(
            "Final Quint evidence no longer matches the verification receipt",
        ));
    }
    let report = parse_report(&data)?;
    let mut body = format!(
        "## Implementation evidence\n\n- Approved Quint model SHA-256: `{}`\n- Host: `quint typecheck model.qnt` exit 0; `quint test model.qnt --backend typescript --seed 57` exit 0, {} passing\n",
        preview.model_sha256, receipt.tests,
    );
    if let Some(maintained) = super::maintain::report(root) {
        body.push_str("- Repository Quint models brought in line with this change:\n");
        for line in maintained.lines() {
            let _ = writeln!(body, "  {line}");
        }
    }
    append_observations(&mut body, &report, session, workspace);
    Ok(Some(body))
}

fn append_observations(
    body: &mut String,
    report: &Report,
    session: &SessionState,
    workspace: &Path,
) {
    for check in report.checks.iter().take(8) {
        if let Some(command) = publishable(&check.command) {
            let provenance = if check.earlier {
                "Earlier step report"
            } else {
                "Final agent observation"
            };
            let _ = write!(body, "- {provenance}: `{command}` exit {}", check.exit);
            if let Some(excerpt) = publishable(&check.excerpt) {
                let _ = write!(body, ": `{excerpt}`");
            }
            body.push('\n');
        }
    }
    for measurement in report.measurements.iter().take(8) {
        if let Some(label) = publishable(&measurement.label) {
            if let Some(before) = measurement.before {
                let _ = writeln!(
                    body,
                    "- Final agent observation: `{label}` {before} → {}",
                    measurement.after
                );
            } else {
                let _ = writeln!(
                    body,
                    "- Final agent observation: saved `{label}` = {}",
                    measurement.after
                );
            }
        }
    }
    if !report.recordings.is_empty()
        && let Ok(forge) = crate::forge::repository_context(session.repo.as_deref(), workspace)
    {
        let prefix = format!("/{}/", forge.repository);
        for candidate in report.recordings.iter().take(4) {
            if let Ok(url) = url::Url::parse(candidate)
                && url.scheme() == "https"
                && url.host_str() == Some(forge.host.as_str())
                && url.path().starts_with(&prefix)
                && url.query().is_none()
                && url.fragment().is_none()
                && url.username().is_empty()
                && url.password().is_none()
                && publishable(url.as_str()).is_some()
            {
                let _ = writeln!(body, "- Recording (final agent observation): <{url}>");
            }
        }
    }
}

fn publishable(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 240 || value.chars().any(char::is_control) {
        return None;
    }
    if crate::telemetry::stderr::sanitize(value).body.trim() != value {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    if [
        "token",
        "password",
        "secret",
        "authorization",
        "api_key",
        "apikey",
        "credential",
    ]
    .iter()
    .any(|word| lower.contains(word))
    {
        return None;
    }
    if std::env::vars_os().any(|(name, secret)| {
        let name = name.to_string_lossy().to_ascii_lowercase();
        secret
            .to_str()
            .is_some_and(|secret| secret.len() >= 6 && value.contains(secret))
            && ["token", "password", "secret", "key", "credential"]
                .iter()
                .any(|word| name.contains(word))
    }) {
        return None;
    }
    Some(value.replace('`', "'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_config::SessionConfigRef;

    #[test]
    fn publication_uses_only_verified_matching_evidence()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace)?;
        std::fs::write(workspace.join("saved"), "7")?;
        let workspace = workspace.canonicalize()?;
        let root = temp.path().join("artifacts");
        let mut session = SessionState::new(
            "test".into(),
            workspace.clone(),
            SessionConfigRef::InlineSnapshot,
            "task".into(),
        );
        let files = Files::new(&root, session.execution_id.as_deref())?;
        session.quint_run = Some(Run::On {
            stage: Stage::Draft { feedback: None },
        });
        assert!(publication_evidence(&session, Some(&root), &workspace).is_err());
        let model = b"module model {}";
        let model_sha256 = sha256(model);
        files.write(&format!("{model_sha256}.qnt"), model, MODEL_LIMIT)?;
        let data = br#"{"model":"model.qnt","ci":"ci","blockers":[],"checks":[{"command":"cat saved","exit":0,"excerpt":"7","earlier":false},{"command":"echo private-token","exit":0,"excerpt":"private-token","earlier":false}],"measurements":[],"recordings":[]}"#;
        files.write("final-agent.json", data, MODEL_LIMIT)?;
        session.quint_run = Some(Run::On {
            stage: Stage::Verified {
                preview: Preview {
                    model_sha256,
                    image: super::super::Image {
                        name: "unused.png".into(),
                        sha256: String::new(),
                    },
                },
                receipt: Receipt {
                    workspace_sha256: crate::file_tracker::fingerprint(&workspace)?,
                    report_sha256: sha256(data),
                    tests: 1,
                },
            },
        });
        let body =
            publication_evidence(&session, Some(&root), &workspace)?.ok_or("missing evidence")?;
        assert!(body.contains("`cat saved` exit 0: `7`"));
        assert!(!body.contains("private-token"));
        std::fs::write(workspace.join("saved"), "8")?;
        assert!(publication_evidence(&session, Some(&root), &workspace).is_err());
        session.quint_run = Some(Run::Off);
        assert!(publication_evidence(&session, None, &workspace)?.is_none());
        Ok(())
    }
}
