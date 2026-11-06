use std::io::{IsTerminal, Read};

use console::style;

use crate::cli::DraftArgs;
use crate::error::{CruiseError, Result};
use crate::multiline_input::{InputResult, prompt_multiline};
use crate::session::{SessionManager, SessionPhase, SessionState};
use crate::session_config::SessionConfigRef;

pub fn run(args: DraftArgs) -> Result<()> {
    let (yaml, source) = crate::resolver::resolve_config(args.config.as_deref())?;
    eprintln!("{}", style(source.display_string()).dim());

    let noninteractive = !std::io::stdin().is_terminal();
    let input = if args.issue.is_some() && args.input.is_none() {
        String::new()
    } else {
        read_draft_input(args.input, noninteractive)?
    };

    crate::resolver::load_config_from_source(&yaml, &source)?;

    let manager = SessionManager::new(crate::paths::data_dir()?);
    let session_id = SessionManager::new_session_id();
    let base_dir = std::env::current_dir()?;
    let task_input = crate::issue_input::resolve_task_input(
        &input,
        args.issue,
        None,
        &base_dir,
        &crate::issue_input::GhGithub,
    )?;
    let config = crate::resolver::load_config_from_source(&yaml, &source)?;
    let config_ref = SessionConfigRef::from_source(&source, None)?;
    let mut session = SessionState::new(session_id.clone(), base_dir, config_ref, task_input.text);
    session.phase = SessionPhase::Draft;
    session.issue_number = task_input.issue_number;
    manager.create_with_config(&session, &config)?;
    let result = (|| {
        session.attachments = crate::attachments::store_downloaded_images(
            &manager.sessions_dir().join(&session.id),
            &task_input.images,
        )?;
        manager.save(&session)
    })();
    if let Err(error) = result {
        if let Err(cleanup_error) = manager.delete(&session.id) {
            eprintln!("warning: failed to clean up session: {cleanup_error}");
        }
        return Err(error);
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
