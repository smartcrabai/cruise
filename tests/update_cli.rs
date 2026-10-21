//! Public entry-point contract for the native application updater.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

fn run_status(state_home: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cruise"))
        .args(["update", "--status"])
        .env("XDG_STATE_HOME", state_home)
        .env_remove("CRUISE_UPDATE_TEST_CAPABILITIES")
        .output()
        .unwrap_or_else(|error| panic!("failed to run cruise update --status: {error}"))
}

fn runtime_path() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_cruise"))
        .parent()
        .unwrap_or_else(|| panic!("launcher has no parent directory"))
        .join(if cfg!(windows) {
            "cruise-runtime.exe"
        } else {
            "cruise-runtime"
        })
}

fn launcher_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cruise"))
}

fn copy_pair(bin: &Path) -> (PathBuf, PathBuf) {
    fs::create_dir_all(bin).unwrap_or_else(|error| panic!("failed to create install bin: {error}"));
    let launcher = bin.join(if cfg!(windows) {
        "cruise.exe"
    } else {
        "cruise"
    });
    let runtime = bin.join(if cfg!(windows) {
        "cruise-runtime.exe"
    } else {
        "cruise-runtime"
    });
    fs::copy(launcher_path(), &launcher)
        .unwrap_or_else(|error| panic!("failed to copy test launcher: {error}"));
    fs::copy(runtime_path(), &runtime)
        .unwrap_or_else(|error| panic!("failed to copy test runtime: {error}"));
    (launcher, runtime)
}

fn file_stamp(path: &Path) -> (u64, SystemTime, [u8; 32]) {
    let metadata = fs::metadata(path)
        .unwrap_or_else(|error| panic!("failed to stat {}: {error}", path.display()));
    let modified = metadata.modified().unwrap_or_else(|error| {
        panic!(
            "failed to get modified time for {}: {error}",
            path.display()
        )
    });
    let mut file = File::open(path)
        .unwrap_or_else(|error| panic!("failed to open {}: {error}", path.display()));
    let mut digest = Sha256::new();
    let mut buffer = [0; 16 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    (metadata.len(), modified, digest.finalize().into())
}

fn modified_time(path: &Path) -> SystemTime {
    fs::metadata(path)
        .unwrap_or_else(|error| panic!("failed to stat {}: {error}", path.display()))
        .modified()
        .unwrap_or_else(|error| {
            panic!(
                "failed to get modified time for {}: {error}",
                path.display()
            )
        })
}

fn update_command(
    launcher: &Path,
    cargo_home: &Path,
    state_home: &Path,
    home: &Path,
    runtime_override: Option<&Path>,
) -> Output {
    let mut command = Command::new(launcher);
    command
        .arg("update")
        .env("CARGO_HOME", cargo_home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_STATE_HOME", state_home)
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .env("http_proxy", "http://127.0.0.1:1")
        .env("https_proxy", "http://127.0.0.1:1")
        .env("NO_PROXY", "localhost,127.0.0.1,::1")
        .env("no_proxy", "localhost,127.0.0.1,::1");
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("CRUISE_")
        {
            command.env_remove(key);
        }
    }
    command.env("CRUISE_UPDATE_TEST_BASE_URL", "http://127.0.0.1:1");
    if let Some(runtime) = runtime_override {
        command.env("CRUISE_RUNTIME", runtime);
    }
    command
        .output()
        .unwrap_or_else(|error| panic!("failed to run {} update: {error}", launcher.display()))
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

#[cfg(all(feature = "test-utils", debug_assertions))]
#[test]
fn update_test_capability_query_is_read_only_and_does_not_contact_release_server() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let state_home = temp.path().join("state-home");
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap_or_else(|error| panic!("failed to bind local release fixture: {error}"));
    listener
        .set_nonblocking(true)
        .unwrap_or_else(|error| panic!("failed to configure local release fixture: {error}"));
    let base_url = format!(
        "http://{}",
        listener.local_addr().unwrap_or_else(|error| panic!(
            "failed to read local release fixture address: {error}"
        ))
    );

    let output = Command::new(env!("CARGO_BIN_EXE_cruise"))
        .args(["update", "--status"])
        .env("XDG_STATE_HOME", &state_home)
        .env("CRUISE_UPDATE_TEST_CAPABILITIES", "1")
        .env("CRUISE_UPDATE_TEST_BASE_URL", base_url)
        .output()
        .unwrap_or_else(|error| panic!("failed to query update test capabilities: {error}"));

    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "cruise-update-test-utils-v1\n"
    );
    assert!(
        !state_home.exists(),
        "capability query read or created the updater state directory"
    );
    assert!(
        matches!(
            listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ),
        "capability query contacted the release fixture"
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

#[test]
fn update_rejects_homebrew_and_external_installs_before_writing_state() {
    for (kind, install_suffix, expected_diagnostic) in [
        (
            "external",
            PathBuf::from("external/bin"),
            "original distribution",
        ),
        (
            "homebrew",
            PathBuf::from("Cellar/cruise/0.2.16/bin"),
            "brew upgrade smartcrabai/tap/cruise",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let install_bin = temp.path().join(install_suffix);
        let (launcher, runtime) = copy_pair(&install_bin);
        let cargo_home = temp.path().join("cargo-home");
        let state_home = temp.path().join("state-home");
        let home = temp.path().join("home");
        let launcher_before = file_stamp(&launcher);
        let runtime_before = file_stamp(&runtime);
        let install_bin_modified_before = modified_time(&install_bin);

        let output = update_command(&launcher, &cargo_home, &state_home, &home, None);
        assert!(
            !output.status.success(),
            "{kind} update unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let diagnostic = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            diagnostic
                .to_lowercase()
                .contains(&expected_diagnostic.to_lowercase()),
            "{kind} update did not explain the required update path: {diagnostic}"
        );
        assert_eq!(
            file_stamp(&launcher),
            launcher_before,
            "{kind} launcher changed"
        );
        assert_eq!(
            file_stamp(&runtime),
            runtime_before,
            "{kind} sibling runtime changed"
        );
        assert_eq!(
            fs::read_dir(&install_bin)
                .unwrap_or_else(|error| panic!("failed to read {kind} install bin: {error}"))
                .count(),
            2,
            "{kind} gate changed the install directory"
        );
        assert_eq!(
            modified_time(&install_bin),
            install_bin_modified_before,
            "{kind} gate modified the install directory"
        );
        assert!(
            !install_bin.join(".cruise-update.lock").exists(),
            "{kind} gate created the install lock"
        );
        assert!(
            !cargo_home.exists(),
            "{kind} gate created or changed the separate Cargo Home"
        );
        assert!(
            !state_home.exists(),
            "{kind} gate created the updater state directory"
        );
    }
}

#[test]
fn update_rejects_runtime_override_without_mutating_managed_install() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let cargo_home = temp.path().join("cargo-home");
    let install_bin = cargo_home.join("bin");
    let (launcher, runtime) = copy_pair(&install_bin);
    let state_home = temp.path().join("state-home");
    let home = temp.path().join("home");
    let launcher_before = file_stamp(&launcher);
    let runtime_before = file_stamp(&runtime);
    let install_bin_modified_before = modified_time(&install_bin);

    let output = update_command(&launcher, &cargo_home, &state_home, &home, Some(&runtime));

    assert!(
        !output.status.success(),
        "update with CRUISE_RUNTIME unexpectedly succeeded"
    );
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        diagnostic.contains("CRUISE_RUNTIME"),
        "override gate did not explain how to restore managed update behavior: {diagnostic}"
    );
    assert_eq!(file_stamp(&launcher), launcher_before, "launcher changed");
    assert_eq!(
        file_stamp(&runtime),
        runtime_before,
        "sibling runtime changed"
    );
    assert_eq!(
        fs::read_dir(&install_bin)
            .unwrap_or_else(|error| panic!("failed to read managed install bin: {error}"))
            .count(),
        2,
        "override gate changed the install directory"
    );
    assert_eq!(
        modified_time(&install_bin),
        install_bin_modified_before,
        "override gate modified the install directory"
    );
    assert!(
        !install_bin.join(".cruise-update.lock").exists(),
        "override gate created the install lock"
    );
    assert_eq!(
        fs::read_dir(&cargo_home)
            .unwrap_or_else(|error| panic!("failed to read separate Cargo Home: {error}"))
            .count(),
        1,
        "override gate changed Cargo Home receipts or directories"
    );
    assert!(
        !state_home.exists(),
        "override gate created the updater state directory"
    );
}

#[test]
fn update_status_is_read_only_with_runtime_override_and_existing_transaction() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let state_home = temp.path().join("state-home");
    let installations_dir = state_home.join("cruise").join("installations");
    let transaction = installations_dir
        .join("kept")
        .join("update-transaction.json");
    fs::create_dir_all(
        transaction
            .parent()
            .unwrap_or_else(|| panic!("transaction parent missing")),
    )
    .unwrap_or_else(|error| panic!("failed to create transaction fixture: {error}"));
    let transaction_contents = br#"{"phase":"staging","version":"1.2.3"}"#;
    fs::write(&transaction, transaction_contents)
        .unwrap_or_else(|error| panic!("failed to write transaction fixture: {error}"));
    let transaction_stamp_before = file_stamp(&transaction);
    let installations_modified_before = modified_time(&installations_dir);
    let cargo_home = temp.path().join("cargo-home");
    let install_bin = cargo_home.join("bin");
    fs::create_dir_all(&install_bin)
        .unwrap_or_else(|error| panic!("failed to create managed bin fixture: {error}"));
    let lock = install_bin.join(".cruise-update.lock");
    let home = temp.path().join("home");

    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    command
        .args(["update", "--status"])
        .env("CARGO_HOME", &cargo_home)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_STATE_HOME", &state_home);
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("CRUISE_")
        {
            command.env_remove(key);
        }
    }
    command.env("CRUISE_UPDATE_TEST_BASE_URL", "http://127.0.0.1:1");
    command.env("CRUISE_RUNTIME", runtime_path());
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to run status with runtime override: {error}"));

    assert_success(&output);
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("Update transaction for installation kept:"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        fs::read(&transaction)
            .unwrap_or_else(|error| panic!("failed to reread transaction: {error}")),
        transaction_contents,
        "status changed existing transaction contents"
    );
    assert_eq!(
        file_stamp(&transaction),
        transaction_stamp_before,
        "status changed transaction metadata"
    );
    assert_eq!(
        modified_time(&installations_dir),
        installations_modified_before,
        "status modified the installation state directory"
    );
    assert!(
        !lock.exists(),
        "read-only status created the managed installation lock"
    );
    assert_eq!(
        fs::read_dir(state_home.join("cruise").join("installations"))
            .unwrap_or_else(|error| panic!("failed to read installation state: {error}"))
            .count(),
        1,
        "status changed the state tree"
    );
}
