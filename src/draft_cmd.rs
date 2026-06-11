use std::io::{IsTerminal, Read};

use console::style;

use crate::cli::DraftArgs;
use crate::config::validate_config;
use crate::error::{CruiseError, Result};
use crate::multiline_input::{InputResult, prompt_multiline};
use crate::session::{SessionManager, SessionPhase, SessionState};

pub fn run(args: DraftArgs) -> Result<()> {
    let (yaml, source) = crate::resolver::resolve_config(args.config.as_deref())?;
    eprintln!("{}", style(source.display_string()).dim());

    let noninteractive = !std::io::stdin().is_terminal();
    let input = read_draft_input(args.input, noninteractive)?;

    let config = match source.path() {
        Some(path) => crate::workflow_call::resolve_workflow_calls_from_path(path)?,
        None => crate::workflow_call::resolve_workflow_calls(
            crate::config::WorkflowConfig::from_yaml(&yaml)
                .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?,
            std::env::current_dir()?,
        )?,
    };
    validate_config(&config)?;

    let manager = SessionManager::new(crate::paths::data_dir()?);
    let session_id = SessionManager::new_session_id();
    let base_dir = std::env::current_dir()?;
    let mut session = SessionState::new(
        session_id.clone(),
        base_dir,
        source.display_string(),
        input.trim().to_string(),
    );
    session.config_path = source.path().cloned();
    session.phase = SessionPhase::Draft;
    manager.create(&session)?;

    if session.config_path.is_none() {
        let session_dir = manager.sessions_dir().join(&session_id);
        if let Err(e) = std::fs::write(session_dir.join("config.yaml"), &yaml) {
            let _ = manager.delete(&session_id);
            return Err(CruiseError::IoError(e));
        }
    }

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
