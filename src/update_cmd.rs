use std::fs;

use crate::cli::UpdateArgs;
use crate::error::{CruiseError, Result};

const INSTALLATIONS_DIR: &str = "installations";
const TRANSACTION_FILE: &str = "update-transaction.json";

pub(crate) fn run(args: UpdateArgs) -> Result<()> {
    if args.status {
        return print_status();
    }

    Err(CruiseError::Other(
        "native application updates are not available in this build".to_string(),
    ))
}

fn print_status() -> Result<()> {
    let installations_dir = crate::paths::state_dir()?.join(INSTALLATIONS_DIR);
    let installations = match fs::read_dir(&installations_dir) {
        Ok(installations) => installations,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("No update transaction is in progress.");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };

    let mut installation_dirs = installations
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    installation_dirs.sort_unstable();

    let mut found_transaction = false;
    for installation_dir in installation_dirs {
        match fs::symlink_metadata(&installation_dir) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        }

        let transaction_path = installation_dir.join(TRANSACTION_FILE);
        let transaction: serde_json::Value = match fs::read_to_string(&transaction_path) {
            Ok(transaction) => serde_json::from_str(&transaction)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let installation_id = installation_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let transaction = serde_json::to_string_pretty(&transaction)?;

        println!("Update transaction for installation {installation_id}:\n{transaction}");
        found_transaction = true;
    }

    if !found_transaction {
        println!("No update transaction is in progress.");
    }

    Ok(())
}
