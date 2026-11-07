#![cfg(unix)]

//! Public CLI contracts for planned provider installation. No downloads are attempted.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn run(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CRUISE_") {
            command.env_remove(key);
        }
    }
    command
        .args(args)
        .current_dir(root)
        .env("HOME", root.join("home"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("PATH", root.join("bin"))
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .output()
        .unwrap_or_else(|error| panic!("failed to run cruise: {error}"))
}

#[test]
fn status_lists_all_pins_without_creating_data_or_executing_providers() -> std::io::Result<()> {
    let temp = TempDir::new()?;
    let root = temp.path();
    fs::create_dir(root.join("bin"))?;
    for provider in ["jcode", "claude", "opencode"] {
        let path = root.join("bin").join(provider);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\necho invoked >> '{}'\n",
                root.join("executed").display()
            ),
        )?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    let output = run(root, &["install", "--status"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for value in ["jcode", "claude", "opencode", "0.99.1", "2.1.250", "2.1.3"] {
        assert!(stdout.contains(value), "missing {value}: {stdout}");
    }
    for column in [
        "PROVIDER",
        "PINNED",
        "MANAGED",
        "PATH",
        "OVERRIDE",
        "EFFECTIVE",
    ] {
        assert!(stdout.contains(column), "missing {column}: {stdout}");
    }
    assert!(!root.join("data").exists());
    assert!(!root.join("executed").exists());
    Ok(())
}

#[test]
fn status_preserves_existing_data() -> std::io::Result<()> {
    let temp = TempDir::new()?;
    let dir = temp.path().join("data/cruise/providers/jcode");
    fs::create_dir_all(&dir)?;
    let record = dir.join("install.json");
    fs::write(&record, b"invalid record\n")?;
    let before = fs::metadata(&record)?.modified()?;
    let output = run(temp.path(), &["install", "--status"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(&record)?, b"invalid record\n");
    assert_eq!(fs::metadata(&record)?.modified()?, before);
    assert_eq!(fs::read_dir(&dir)?.count(), 1);
    Ok(())
}

#[test]
fn install_rejects_invalid_argument_combinations_before_doing_work() -> std::io::Result<()> {
    for args in [
        vec!["install"],
        vec!["install", "--force"],
        vec!["install", "jcode", "--status"],
        vec!["install", "--status", "--force"],
        vec!["install", "unknown"],
        vec!["install", "jcode", "claude"],
    ] {
        let temp = TempDir::new()?;
        let output = run(temp.path(), &args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "unexpected success for {args:?}");
        if args == ["install"] {
            assert!(
                stderr.to_ascii_lowercase().contains("usage:"),
                "{args:?}: {stderr}"
            );
        }
        assert!(!temp.path().join("data").exists(), "{args:?} created data");
    }
    Ok(())
}
