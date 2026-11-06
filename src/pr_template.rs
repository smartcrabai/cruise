use crate::config::WorkflowConfig;
use crate::error::{CruiseError, Result};
use crate::session::SessionState;
use crate::variable::resolve_template_with_lookup;
use crate::workflow::CompiledWorkflow;

const COMMIT_VARIABLES: &[&str] = &["title", "issue", "session_id", "branch"];
const BODY_VARIABLES: &[&str] = &["title", "body", "issue", "session_id", "branch"];

pub(crate) fn validate_config(config: &WorkflowConfig) -> Result<()> {
    for (key, template, allowed) in [
        (
            "commit_message_template",
            config.commit_message_template.as_deref(),
            COMMIT_VARIABLES,
        ),
        (
            "pr_body_template",
            config.pr_body_template.as_deref(),
            BODY_VARIABLES,
        ),
    ] {
        if let Some(template) = template {
            let mut names = Vec::new();
            resolve_template_with_lookup(template, |name| {
                names.push(name.to_owned());
                Ok(String::new())
            })?;
            for name in names {
                if !allowed.contains(&name.as_str()) {
                    return Err(CruiseError::ConfigParseError(format!(
                        "{key}: unknown variable `{{{name}}}`; allowed: {}",
                        allowed.join(", ")
                    )));
                }
            }
        }
    }
    Ok(())
}

pub(crate) struct PrTexts {
    pub title: String,
    pub body: String,
    pub commit_message: Option<String>,
}

fn issue_variable(_session: &SessionState) -> String {
    String::new()
}

pub(crate) fn render_pr_texts(
    compiled: &CompiledWorkflow,
    session: &SessionState,
    branch: &str,
    generated_title: &str,
    generated_body: &str,
) -> Result<PrTexts> {
    let generated_title = generated_title.trim();
    if compiled.commit_message_template.is_none() && compiled.pr_body_template.is_none() {
        return Ok(PrTexts {
            title: generated_title.to_owned(),
            body: generated_body.to_owned(),
            commit_message: None,
        });
    }
    let title = if generated_title.is_empty() {
        session
            .input_or_title()
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("")
    } else {
        generated_title
    };
    if compiled.pr_body_template.is_some() && title.is_empty() {
        return Err(CruiseError::Other(
            "pr_body_template requires a nonempty generated or session title".to_owned(),
        ));
    }
    let issue = issue_variable(session);
    let render = |template: &str, allowed: &[&str]| {
        resolve_template_with_lookup(template, |name| {
            if !allowed.contains(&name) {
                return Err(CruiseError::UndefinedVariable(name.to_owned()));
            }
            let value = match name {
                "title" => title,
                "body" => generated_body.trim(),
                "issue" => &issue,
                "session_id" => &session.id,
                "branch" => branch,
                _ => {
                    return Err(CruiseError::UndefinedVariable(name.to_owned()));
                }
            };
            Ok(value.to_owned())
        })
    };
    let commit_message = compiled
        .commit_message_template
        .as_deref()
        .map(|template| {
            let rendered = render(template, COMMIT_VARIABLES)?.trim().to_owned();
            if rendered.is_empty() {
                return Err(CruiseError::Other(
                    "commit_message_template rendered an empty commit message".to_owned(),
                ));
            }
            Ok(rendered)
        })
        .transpose()?;
    let (output_title, body) = if let Some(template) = compiled.pr_body_template.as_deref() {
        (
            title.to_owned(),
            render(template, BODY_VARIABLES)?.trim().to_owned(),
        )
    } else {
        (generated_title.to_owned(), generated_body.to_owned())
    };
    Ok(PrTexts {
        title: output_title,
        body,
        commit_message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiled(extra: &str) -> CompiledWorkflow {
        let yaml = format!("{extra}\nsteps:\n  main:\n    command: echo ok\n");
        crate::workflow::compile(WorkflowConfig::from_yaml(&yaml).unwrap()).unwrap()
    }

    fn session(input: &str) -> SessionState {
        SessionState::new(
            "session-1".to_owned(),
            std::path::PathBuf::from("."),
            crate::session_config::SessionConfigRef::BuiltinSnapshot { name: None },
            input.to_owned(),
        )
    }

    #[test]
    fn preserves_legacy_text_and_fill_behavior() {
        let texts =
            render_pr_texts(&compiled(""), &session("input"), "branch", "  ", " body \n").unwrap();
        assert_eq!(texts.title, "");
        assert_eq!(texts.body, " body \n");
        assert_eq!(texts.commit_message, None);
    }

    #[test]
    fn renders_templates_without_rescanning_values() {
        let workflow = compiled(
            "commit_message_template: 'feat: {title} ({branch})'\npr_body_template: '{{x}} {body} {session_id} {issue}'",
        );
        let texts = render_pr_texts(
            &workflow,
            &session("\n task first \nsecond"),
            "feature",
            "",
            " body {plan} ",
        )
        .unwrap();
        assert_eq!(texts.title, "task first");
        assert_eq!(texts.body, "{x} body {plan} session-1");
        assert_eq!(
            texts.commit_message.as_deref(),
            Some("feat: task first (feature)")
        );
    }

    #[test]
    fn commit_only_preserves_pr_fill_and_rejects_empty_message() {
        let workflow = compiled("commit_message_template: '{title}'");
        let texts =
            render_pr_texts(&workflow, &session("task"), "feature", " ", " original ").unwrap();
        assert_eq!(texts.title, "");
        assert_eq!(texts.body, " original ");
        assert_eq!(texts.commit_message.as_deref(), Some("task"));
        let workflow = compiled("commit_message_template: '{issue}'");
        assert!(render_pr_texts(&workflow, &session("task"), "feature", "title", "").is_err());
    }
}
