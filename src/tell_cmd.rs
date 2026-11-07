use crate::{
    cli::TellArgs,
    error::CruiseError,
    instruction_inbox::{self, AcceptRequest, Origin},
    session::SessionManager,
};
use std::io::{IsTerminal, Read};

pub fn run(args: TellArgs) -> Result<(), CruiseError> {
    let manager = SessionManager::new(crate::paths::data_dir()?);
    let sessions = manager.sessions_dir();
    if args.log {
        manager.load(&args.session_id)?;
        for item in instruction_inbox::summary(&sessions, &args.session_id)?.history {
            let first = item.content.lines().next().unwrap_or("");
            let preview: String = first.chars().take(80).collect();
            println!(
                "#{}  {}  {}  {}  {}",
                item.id, item.at, item.origin, item.status, preview
            );
        }
        return Ok(());
    }
    let content = match args.instruction {
        Some(text) if text != "-" => text,
        value => {
            if value.is_none() && std::io::stdin().is_terminal() {
                return Err(CruiseError::SessionError(
                    "instruction text is required".into(),
                ));
            }
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            text
        }
    };
    let receipt = instruction_inbox::accept(
        &sessions,
        &args.session_id,
        AcceptRequest {
            content,
            origin: if args.origin == "github" {
                Origin::Github
            } else {
                Origin::Cli
            },
            dedupe_key: args.dedupe_key,
        },
    )?;
    if receipt.duplicate {
        println!(
            "instruction #{} was already accepted for {}.",
            receipt.id, args.session_id
        );
    } else {
        print!(
            "instruction #{} accepted for {}; queued for the next agent prompt, if one runs.",
            receipt.id, args.session_id
        );
        if let Some(step) = receipt.running_step {
            print!(" The running step \"{step}\" already started and will not see it.");
        }
        println!();
    }
    Ok(())
}
