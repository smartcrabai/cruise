//! End-to-end resolution checks for `install --status`, without running provider CLIs.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};
use tempfile::TempDir;

const VERSION: &str = "0.99.1";
const OVERRIDE: &str = "CRUISE_JCODE_CLI_PATH";

struct Fixture {
    root: TempDir,
    data: PathBuf,
    path: PathBuf,
    marker: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap_or_else(|e| panic!("create fixture directory: {e}"));
        let data = root.path().join("data");
        let path = root.path().join("bin");
        fs::create_dir_all(&path).unwrap_or_else(|e| panic!("create fixture bin directory: {e}"));
        let marker = root.path().join("executed");
        let fixture = Self {
            root,
            data,
            path,
            marker,
        };
        fixture.stub(&fixture.path.join("jcode"));
        fixture
    }

    fn stub(&self, path: &Path) {
        let parent = path
            .parent()
            .unwrap_or_else(|| panic!("stub has no parent: {}", path.display()));
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("create stub directory: {e}"));
        fs::write(
            path,
            format!("#!/bin/sh\necho executed >> '{}'\n", self.marker.display()),
        )
        .unwrap_or_else(|e| panic!("write stub: {e}"));
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|e| panic!("make stub executable: {e}"));
    }

    fn record(&self, target: &str) -> PathBuf {
        let dir = self.data.join("cruise/providers/jcode");
        let binary = dir.join(VERSION).join("jcode");
        self.stub(&binary);
        let metadata = fs::metadata(&binary).unwrap_or_else(|e| panic!("read stub metadata: {e}"));
        let nanos = metadata
            .modified()
            .unwrap_or_else(|e| panic!("read stub modification time: {e}"))
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|e| panic!("stub modification time predates epoch: {e}"))
            .as_nanos();
        let mut digest = String::new();
        for byte in Sha256::digest(fs::read(&binary).unwrap_or_else(|e| panic!("read stub: {e}"))) {
            use std::fmt::Write as _;
            write!(&mut digest, "{byte:02x}").unwrap_or_else(|e| panic!("format digest: {e}"));
        }
        let record = serde_json::json!({
            "schema": 1, "provider": "jcode", "version": VERSION,
            "target": target, "relative_path": format!("{VERSION}/jcode"),
            "sha256": digest, "len": metadata.len(), "mtime_unix_nanos": nanos,
            "source_url": "https://example.invalid/jcode"
        });
        fs::write(
            dir.join("install.json"),
            serde_json::to_vec(&record).unwrap_or_else(|e| panic!("serialize record: {e}")),
        )
        .unwrap_or_else(|e| panic!("write record: {e}"));
        binary
    }

    fn status(&self, value: Option<&Path>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        command
            .args(["install", "--status"])
            .env("HOME", self.root.path())
            .env("XDG_DATA_HOME", &self.data)
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("PATH", &self.path)
            .env_remove(OVERRIDE)
            .env_remove("CRUISE_CLAUDE_CLI_PATH")
            .env_remove("CRUISE_OPENCODE_CLI_PATH");
        if let Some(value) = value {
            command.env(OVERRIDE, value);
        }
        command
            .output()
            .unwrap_or_else(|e| panic!("run status command: {e}"))
    }
}

fn target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("macos-aarch64"),
        ("macos", "x86_64") => Some("macos-x86_64"),
        ("linux", "aarch64") if !cfg!(target_env = "musl") => Some("linux-aarch64"),
        ("linux", "x86_64") if !cfg!(target_env = "musl") => Some("linux-x86_64"),
        _ => None,
    }
}

fn row(output: &Output) -> String {
    assert!(
        output.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find(|line| line.split_whitespace().next() == Some("jcode"))
        .unwrap_or_else(|| panic!("missing jcode row: {stdout}"))
        .to_owned()
}

fn effective(line: &str, expected: &str) {
    // The final status column is EFFECTIVE. Compare its source rather than
    // accidentally matching the MANAGED, PATH, or OVERRIDE column headings.
    let last = line
        .split_whitespace()
        .last()
        .unwrap_or_else(|| panic!("empty status row: {line}"))
        .to_ascii_lowercase();
    assert!(
        last.contains(expected),
        "expected effective {expected}, got {line}"
    );
}

#[test]
fn override_wins_over_ready_managed_and_path_and_empty_override_is_ignored() {
    let Some(target) = target() else { return };
    let f = Fixture::new();
    let managed = f.record(target);
    let override_path = f.root.path().join("override-jcode");
    f.stub(&override_path);
    let line = row(&f.status(Some(&override_path)));
    assert!(
        line.contains(
            override_path.to_str().unwrap_or_else(|| panic!(
                "override path is not UTF-8: {}",
                override_path.display()
            ))
        ),
        "{line}"
    );
    effective(&line, "override");
    let line = row(&f.status(Some(Path::new(""))));
    assert!(
        line.contains(
            managed
                .to_str()
                .unwrap_or_else(|| panic!("managed path is not UTF-8: {}", managed.display()))
        ) || line.to_ascii_lowercase().contains("ready"),
        "{line}"
    );
    effective(&line, "managed");
    assert!(!f.marker.exists(), "status executed a provider binary");
}

#[test]
fn invalid_override_never_falls_back_to_ready_managed_or_path() {
    let Some(target) = target() else { return };
    let f = Fixture::new();
    f.record(target);
    let missing = f.root.path().join("missing-jcode");
    let output = f.status(Some(&missing));
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_ascii_lowercase();
    assert!(
        combined.contains("jcode") && combined.contains("error"),
        "{combined}"
    );
    assert!(
        combined.contains(&missing.to_string_lossy().to_ascii_lowercase()),
        "{combined}"
    );
    assert!(!f.marker.exists());
}

#[test]
fn stale_and_damaged_records_fall_back_to_path_without_modification() {
    let Some(target) = target() else { return };
    for damage in ["stale", "stamp", "json", "traversal"] {
        let f = Fixture::new();
        let binary = f.record(target);
        let record = f.data.join("cruise/providers/jcode/install.json");
        if damage == "json" {
            fs::write(&record, b"{broken").unwrap_or_else(|e| panic!("damage record JSON: {e}"));
        } else if damage == "stamp" {
            fs::write(&binary, b"changed length")
                .unwrap_or_else(|e| panic!("damage binary stamp: {e}"));
        } else {
            let mut value: serde_json::Value = serde_json::from_slice(
                &fs::read(&record).unwrap_or_else(|e| panic!("read record: {e}")),
            )
            .unwrap_or_else(|e| panic!("parse record: {e}"));
            if damage == "stale" {
                value["version"] = "0.1.0".into();
            } else {
                value["relative_path"] = "../outside".into();
            }
            fs::write(
                &record,
                serde_json::to_vec(&value)
                    .unwrap_or_else(|e| panic!("serialize damaged record: {e}")),
            )
            .unwrap_or_else(|e| panic!("write damaged record: {e}"));
        }
        let before = snapshot(f.root.path());
        let output = f.status(None);
        let line = row(&output);
        effective(&line, "path");
        assert!(
            line.to_ascii_lowercase().contains(if damage == "stale" {
                "stale"
            } else {
                "damaged"
            }),
            "{damage}: {line}"
        );
        assert!(
            output.stderr.is_empty(),
            "status must not emit resolver warnings: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            before,
            snapshot(f.root.path()),
            "status changed files for {damage}"
        );
        assert!(!f.marker.exists(), "status executed a provider binary");
    }
}

fn snapshot(root: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime, Option<Vec<u8>>)> {
    fn visit(
        root: &Path,
        path: &Path,
        entries: &mut Vec<(PathBuf, u64, std::time::SystemTime, Option<Vec<u8>>)>,
    ) {
        let metadata = fs::symlink_metadata(path)
            .unwrap_or_else(|e| panic!("snapshot metadata for {}: {e}", path.display()));
        entries.push((
            path.strip_prefix(root)
                .unwrap_or_else(|e| panic!("snapshot path outside root: {e}"))
                .to_owned(),
            metadata.len(),
            metadata.modified().unwrap_or_else(|e| {
                panic!("snapshot modification time for {}: {e}", path.display())
            }),
            if metadata.is_file() {
                Some(
                    fs::read(path)
                        .unwrap_or_else(|e| panic!("snapshot read {}: {e}", path.display())),
                )
            } else {
                None
            },
        ));
        if metadata.is_dir() {
            for entry in fs::read_dir(path)
                .unwrap_or_else(|e| panic!("snapshot read directory {}: {e}", path.display()))
            {
                visit(
                    root,
                    &entry
                        .unwrap_or_else(|e| panic!("snapshot directory entry: {e}"))
                        .path(),
                    entries,
                );
            }
        }
    }
    let mut entries = Vec::new();
    visit(root, root, &mut entries);
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}
