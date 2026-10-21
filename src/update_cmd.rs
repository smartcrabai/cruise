use std::fs;

use crate::cli::UpdateArgs;
use crate::error::{CruiseError, Result};
use crate::install_identity::{self, InstallState};

mod release;
mod transaction;

const INSTALLATIONS_DIR: &str = "installations";
const TRANSACTION_FILE: &str = "update-transaction.json";

pub(crate) fn run(args: &UpdateArgs) -> Result<()> {
    if args.status {
        #[cfg(all(feature = "test-utils", debug_assertions))]
        if std::env::var_os("CRUISE_UPDATE_TEST_CAPABILITIES").as_deref()
            == Some(std::ffi::OsStr::new("1"))
        {
            println!("cruise-update-test-utils-v1");
            return Ok(());
        }
        return print_status();
    }

    if std::env::var_os("CRUISE_RUNTIME").is_some() {
        return Err(CruiseError::Other(
            "native update is unavailable while CRUISE_RUNTIME overrides runtime selection; unset it and run update from the managed launcher".to_owned(),
        ));
    }

    let running = std::env::current_exe()?;
    let bin = match install_identity::inspect(&running) {
        InstallState::Managed(bin) => bin,
        InstallState::Homebrew => {
            return Err(CruiseError::Other(
                "Homebrew manages this installation; update it with `brew upgrade smartcrabai/tap/cruise`".to_owned(),
            ));
        }
        InstallState::External => {
            return Err(CruiseError::Other(format!(
                "native update is supported only for a managed CARGO_HOME/bin installation; update this external installation using its original distribution method ({})",
                running.display()
            )));
        }
    };
    let launcher_path = bin.join(install_identity::launcher_name());
    let launcher = fs::canonicalize(&launcher_path).map_err(|error| {
        CruiseError::Other(format!(
            "managed installation is missing its fixed launcher at {}: {error}",
            launcher_path.display()
        ))
    })?;
    if launcher.parent() != Some(bin.as_path()) {
        return Err(CruiseError::Other(
            "the fixed launcher does not resolve inside the managed install bin".to_owned(),
        ));
    }
    transaction::verify_current_selection(&bin, &launcher, &running)?;
    install_identity::install_baseline(&bin)?;

    let install = transaction::Install::acquire(&launcher, &bin, &crate::paths::state_dir()?)?;
    install.check_current_selection(&running)?;
    let current_version = env!("CARGO_PKG_VERSION");
    if install.recover()? {
        println!(
            "Recovered an interrupted Cruise update; run Cruise again to use the selected runtime."
        );
        return Ok(());
    }

    install.check_current_selection(&running)?;
    let (mut transaction, staging) = install.begin(current_version)?;
    let release = match release::acquire(current_version, release::BUILD_TARGET, &staging) {
        Ok(Some(release)) => release,
        Ok(None) => {
            install.no_update(&transaction)?;
            println!(
                "Latest stable release is not newer than Cruise {current_version}; no update was applied."
            );
            return Ok(());
        }
        Err(error) => {
            return Err(record_failure(&install, &mut transaction, error));
        }
    };

    if let Err(error) = install.apply(&mut transaction, &staging, release) {
        return Err(record_failure(&install, &mut transaction, error));
    }
    Ok(())
}

fn record_failure(
    install: &transaction::Install,
    transaction: &mut transaction::Transaction,
    error: CruiseError,
) -> CruiseError {
    match install.fail(transaction, &error.to_string()) {
        Ok(()) => error,
        Err(state_error) => CruiseError::Other(format!(
            "{error}; additionally failed to record the update failure: {state_error}"
        )),
    }
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
