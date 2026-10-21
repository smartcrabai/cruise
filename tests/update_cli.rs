//! Public entry-point contract for the native application updater.

use std::fs;
use std::path::Path;
use std::process::Command;

fn run_status(state_home: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cruise"))
        .args(["update", "--status"])
        .env("XDG_STATE_HOME", state_home)
        .output()
        .unwrap_or_else(|error| panic!("failed to run cruise update --status: {error}"))
}

#[track_caller]
fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn update_help_exposes_read_only_status_option() {
    let output = Command::new(env!("CARGO_BIN_EXE_cruise"))
        .args(["update", "--help"])
        .output()
        .unwrap_or_else(|error| panic!("failed to run cruise update --help: {error}"));

    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{text}");
    assert!(
        text.contains("--status"),
        "update help omits --status: {text}"
    );
}

#[test]
fn update_status_without_transactions_is_read_only() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let output = run_status(temp.path());

    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "No update transaction is in progress.\n"
    );
    assert!(!temp.path().join("cruise").exists());
}

#[test]
fn update_status_lists_transactions_across_installations_without_changing_them() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let installations = temp.path().join("cruise").join("installations");
    let alpha = installations.join("alpha").join("update-transaction.json");
    let beta = installations.join("beta").join("update-transaction.json");
    fs::create_dir_all(
        alpha
            .parent()
            .unwrap_or_else(|| panic!("alpha parent missing")),
    )
    .unwrap_or_else(|error| panic!("failed to create alpha fixture: {error}"));
    fs::create_dir_all(
        beta.parent()
            .unwrap_or_else(|| panic!("beta parent missing")),
    )
    .unwrap_or_else(|error| panic!("failed to create beta fixture: {error}"));
    fs::write(&alpha, br#"{"phase":"staging","version":"1.2.3"}"#)
        .unwrap_or_else(|error| panic!("failed to write alpha fixture: {error}"));
    fs::write(&beta, br#"{"phase":"activating","version":"1.2.4"}"#)
        .unwrap_or_else(|error| panic!("failed to write beta fixture: {error}"));
    let alpha_before =
        fs::read(&alpha).unwrap_or_else(|error| panic!("failed to read alpha: {error}"));
    let beta_before =
        fs::read(&beta).unwrap_or_else(|error| panic!("failed to read beta: {error}"));

    let output = run_status(temp.path());
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_success(&output);
    assert_eq!(
        stdout,
        concat!(
            "Update transaction for installation alpha:\n",
            "{\n  \"phase\": \"staging\",\n  \"version\": \"1.2.3\"\n}\n",
            "Update transaction for installation beta:\n",
            "{\n  \"phase\": \"activating\",\n  \"version\": \"1.2.4\"\n}\n",
        )
    );
    assert_eq!(
        fs::read(&alpha).unwrap_or_else(|error| panic!("failed to reread alpha: {error}")),
        alpha_before
    );
    assert_eq!(
        fs::read(&beta).unwrap_or_else(|error| panic!("failed to reread beta: {error}")),
        beta_before
    );
}

#[test]
fn update_status_skips_non_directory_installation_entries() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let installations = temp.path().join("cruise").join("installations");
    fs::create_dir_all(&installations)
        .unwrap_or_else(|error| panic!("failed to create installations directory: {error}"));
    fs::write(installations.join("not-a-directory"), b"not a directory")
        .unwrap_or_else(|error| panic!("failed to write non-directory fixture: {error}"));

    let output = run_status(temp.path());

    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "No update transaction is in progress.\n"
    );
}

#[cfg(unix)]
#[test]
fn update_status_skips_symlinked_installation_directories() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let installations = temp.path().join("cruise").join("installations");
    fs::create_dir_all(&installations)
        .unwrap_or_else(|error| panic!("failed to create installations directory: {error}"));
    let external_installation = temp.path().join("external-installation");
    fs::create_dir_all(&external_installation)
        .unwrap_or_else(|error| panic!("failed to create external installation: {error}"));
    fs::write(
        external_installation.join("update-transaction.json"),
        br#"{"phase":"staging"}"#,
    )
    .unwrap_or_else(|error| panic!("failed to write external transaction: {error}"));
    symlink(
        &external_installation,
        installations.join("linked-installation"),
    )
    .unwrap_or_else(|error| panic!("failed to create installation symlink: {error}"));

    let output = run_status(temp.path());

    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "No update transaction is in progress.\n"
    );
}

#[test]
fn update_status_skips_missing_transaction_files() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let installation = temp
        .path()
        .join("cruise")
        .join("installations")
        .join("installed");
    fs::create_dir_all(&installation)
        .unwrap_or_else(|error| panic!("failed to create installation fixture: {error}"));

    let output = run_status(temp.path());

    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "No update transaction is in progress.\n"
    );
}

#[test]
fn update_status_reports_malformed_transaction_json() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let installation = temp
        .path()
        .join("cruise")
        .join("installations")
        .join("malformed");
    fs::create_dir_all(&installation)
        .unwrap_or_else(|error| panic!("failed to create installation fixture: {error}"));
    fs::write(installation.join("update-transaction.json"), b"{")
        .unwrap_or_else(|error| panic!("failed to write malformed transaction: {error}"));

    let output = run_status(temp.path());

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).starts_with("Error: "),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn update_status_propagates_transaction_read_errors() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let transaction = temp
        .path()
        .join("cruise")
        .join("installations")
        .join("unreadable")
        .join("update-transaction.json");
    fs::create_dir_all(&transaction)
        .unwrap_or_else(|error| panic!("failed to create transaction read-error fixture: {error}"));

    let output = run_status(temp.path());

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).starts_with("Error: "),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
