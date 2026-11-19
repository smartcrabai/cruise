use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use console::style;

use crate::application::{NewSessionImage, NewSessionRequest};
use crate::cli::DraftArgs;
use crate::error::{CruiseError, Result};
use crate::multiline_input::{InputResult, prompt_multiline};

pub async fn run(args: DraftArgs) -> Result<()> {
    let (yaml, source) = crate::resolver::resolve_config(args.config.as_deref())?;
    eprintln!("{}", style(source.display_string()).dim());

    let noninteractive = !std::io::stdin().is_terminal();
    let input = if args.issue.is_some() && args.input.is_none() {
        String::new()
    } else {
        read_draft_input(args.input, noninteractive)?
    };

    crate::resolver::load_config_from_source(&yaml, &source)?;
    let context = crate::request_context::RequestContext::capture()?;
    let base_dir = context.cwd.clone();
    let task_input = crate::issue_input::resolve_task_input(
        &input,
        args.issue,
        None,
        &base_dir,
        &crate::issue_input::GhGithub,
    )?;
    let (config_path, config_yaml) = config_request(&source, &yaml, &base_dir);
    let issue_images = task_input
        .images
        .into_iter()
        .map(|image| NewSessionImage {
            extension: image.extension.to_string(),
            bytes: image.bytes,
        })
        .collect();
    let request = NewSessionRequest {
        input: task_input.text,
        base_dir,
        config_path,
        config_yaml,
        issue_number: task_input.issue_number,
        issue_images,
        context: Some(context),
        ..NewSessionRequest::default()
    };
    let client = crate::daemon::client::DaemonClient::connect().await?;
    let session = client.create_session(&request).await?;

    eprintln!(
        "\n{} Session {} saved as draft.",
        style("✓").green().bold(),
        session.id
    );
    eprintln!(
        "  Generate plan later with: {}",
        style("cruise list").cyan()
    );
    Ok(())
}

fn config_request(
    source: &crate::resolver::ConfigSource,
    yaml: &str,
    cwd: &Path,
) -> (Option<PathBuf>, Option<String>) {
    if let Some(path) = source.path() {
        let path = crate::new_session_history::expand_tilde(&path.to_string_lossy());
        let path = PathBuf::from(path);
        return (
            Some(if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            }),
            None,
        );
    }
    match source {
        crate::resolver::ConfigSource::Builtin => (
            Some(PathBuf::from(
                crate::new_session_history::BUILTIN_CONFIG_KEY,
            )),
            None,
        ),
        crate::resolver::ConfigSource::NamedBuiltin(name) => (
            Some(PathBuf::from(format!(
                "{}{name}",
                crate::resolver::BUILTIN_SELECTOR_PREFIX
            ))),
            None,
        ),
        crate::resolver::ConfigSource::Inline => (None, Some(yaml.to_string())),
        crate::resolver::ConfigSource::Explicit(_)
        | crate::resolver::ConfigSource::EnvVar(_)
        | crate::resolver::ConfigSource::Local(_)
        | crate::resolver::ConfigSource::UserDir(_) => unreachable!(),
    }
}

fn read_draft_input(input: Option<String>, noninteractive: bool) -> Result<String> {
    if let Some(text) = input {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return Err(CruiseError::Other("input cannot be empty".to_string()));
        }
        return Ok(trimmed);
    }

    if noninteractive {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(CruiseError::IoError)?;
        let trimmed = s.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
        return Err(CruiseError::Other(
            "no input provided: stdin is not a terminal and no input argument was given"
                .to_string(),
        ));
    }

    match prompt_multiline("What would you like to implement?")? {
        InputResult::Submitted(text) => {
            if text.trim().is_empty() {
                return Err(CruiseError::Other("input cannot be empty".to_string()));
            }
            Ok(text)
        }
        InputResult::Cancelled => Err(CruiseError::StepPaused),
    }
}
