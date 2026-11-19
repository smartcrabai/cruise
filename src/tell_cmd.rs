use std::io::{IsTerminal as _, Read};

use crate::{cli::TellArgs, error::CruiseError, request_context::RequestContext};

pub async fn run(args: TellArgs) -> Result<(), CruiseError> {
    let context = RequestContext::capture()?;
    if args.log {
        let client = crate::daemon::client::DaemonClient::connect().await?;
        for item in client.instructions(&args.session_id, &context).await? {
            let first = item.content.lines().next().unwrap_or("");
            let preview: String = first.chars().take(80).collect();
            println!(
                "#{}  {}  {}  {}  {}",
                item.id, item.at, item.origin, item.status, preview
            );
        }
        return Ok(());
    }
    let instruction_text = match args.instruction {
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
    let client = crate::daemon::client::DaemonClient::connect().await?;
    let receipt = client
        .tell(
            &args.session_id,
            &crate::application::TellRequest {
                content: instruction_text,
                origin: args.origin,
                dedupe_key: args.dedupe_key,
                context: Some(context),
            },
        )
        .await?;
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
