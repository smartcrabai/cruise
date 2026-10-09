//! `cruise workflow` command adapter: add / remove / update / list GitHub workflow packages.

use std::io::{BufRead as _, IsTerminal as _, Write as _};

use crate::cli::{WorkflowArgs, WorkflowSubcommand};
use crate::error::{CruiseError, Result};
use crate::workflow_packages::{self as packages, PreparedPackage, WorkflowPackageError};

#[expect(clippy::needless_pass_by_value, reason = "used directly with map_err")]
fn to_cruise(e: WorkflowPackageError) -> CruiseError {
    CruiseError::Other(e.to_string())
}

/// Print the preview, then require confirmation (`--yes` or an interactive `y`).
fn confirm(prepared: &PreparedPackage, yes: bool) -> Result<()> {
    println!("{}", prepared.preview());
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        return Err(to_cruise(WorkflowPackageError::ConfirmationRequired));
    }
    print!("Install this workflow? [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(to_cruise(WorkflowPackageError::ConfirmationRequired))
    }
}

fn install(prepared: PreparedPackage, yes: bool, verb: &str) -> Result<()> {
    confirm(&prepared, yes)?;
    let installed = packages::commit_install(prepared).map_err(to_cruise)?;
    println!(
        "{verb} {} ({})",
        installed.name,
        installed.config_path.display()
    );
    Ok(())
}

pub fn run(args: WorkflowArgs) -> Result<()> {
    match args.command {
        WorkflowSubcommand::Add(a) => {
            let spec = packages::parse_package_spec(&a.spec).map_err(to_cruise)?;
            let name = a.name.unwrap_or_else(|| packages::default_name(&spec));
            let prepared = packages::prepare_install(&spec, &name).map_err(to_cruise)?;
            install(prepared, a.yes, "Installed")
        }
        WorkflowSubcommand::Update(a) => {
            let prepared = packages::prepare_update(&a.name).map_err(to_cruise)?;
            install(prepared, a.yes, "Updated")
        }
        WorkflowSubcommand::Remove(a) => {
            packages::remove(&a.name).map_err(to_cruise)?;
            println!("Removed {}", a.name);
            Ok(())
        }
        WorkflowSubcommand::List => {
            let installed = packages::list_installed().map_err(to_cruise)?;
            if installed.is_empty() {
                println!("No installed workflow packages.");
            }
            for p in installed {
                println!(
                    "{}\t{}/{}/{}\t{}\t{}\t{}",
                    p.name,
                    p.owner,
                    p.repo,
                    p.workflow_path,
                    p.requested_ref,
                    p.resolved_commit,
                    p.config_path.display()
                );
            }
            Ok(())
        }
    }
}
