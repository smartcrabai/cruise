use super::*;
use release::{Archive, Digest, Release, Target};
use sha2::{Digest as _, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

const V1: &[u8] = b"#!/bin/sh\nprintf '1.2.3 (Claude Code)\\n'\n";
const V2: &[u8] = b"#!/bin/sh\nprintf '1.2.4 (Claude Code)\\n'\n";
const WRONG_VERSION: &[u8] = b"#!/bin/sh\nprintf '9.9.9 (Claude Code)\\n'\n";

struct Server {
    base: Url,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(body: &'static [u8]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let count = requests.clone();
        let shutdown = stop.clone();
        let thread = thread::spawn(move || {
            while !shutdown.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut request = [0; 4096];
                        let _ = stream.read(&mut request);
                        count.fetch_add(1, Ordering::SeqCst);
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(body);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("fixture server: {e}"),
                }
            }
        });
        Self {
            base,
            requests,
            stop,
            thread: Some(thread),
        }
    }
    fn count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn fixture_release(version: &str, key: &'static str, body: &[u8]) -> Release {
    Release {
        version: version.into(),
        target: Target {
            key,
            url: format!("https://downloads.claude.ai/claude-code-releases/{version}/{key}/claude"),
            archive: Archive::Raw,
            entry: "claude".into(),
            digest: Digest::Sha256(crate::http_download::hex_digest(&Sha256::digest(body))),
        },
    }
}

fn install_fixture(
    root: &Path,
    release: Release,
    server: &Server,
    force: bool,
    selections: &AtomicUsize,
) -> Result<InstallOutcome> {
    install_in(
        Provider::Claude,
        force,
        root,
        move |key| {
            selections.fetch_add(1, Ordering::SeqCst);
            assert_eq!(key, release.target.key);
            Ok(release)
        },
        &[],
        Some(&server.base),
    )
}

fn paths(root: &Path) -> (PathBuf, PathBuf) {
    let dir = root.join("providers/claude");
    let record = dir.join("install.json");
    let relative = record::read_record(&record).unwrap().relative_path;
    (dir.join(relative), record)
}

fn assert_no_staging(root: &Path) {
    let dir = root.join("providers/claude");
    assert!(!fs::read_dir(dir).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".staging-")
    }));
}

#[test]
fn interrupted_staging_is_removed_on_next_install() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("providers/claude/.staging-interrupted");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("partial"), b"incomplete").unwrap();
    let server = Server::new(V1);
    let selections = AtomicUsize::new(0);
    install_fixture(
        temp.path(),
        fixture_release(
            "1.2.3",
            target::detect_target_key(Provider::Claude).unwrap(),
            V1,
        ),
        &server,
        false,
        &selections,
    )
    .unwrap();
    assert!(!dir.exists());
    assert_eq!(fs::read(paths(temp.path()).0).unwrap(), V1);
}

#[test]
fn latest_is_discovered_each_time_and_upgrade_force_and_repair_are_atomic() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let key = target::detect_target_key(Provider::Claude).unwrap();
    let first_server = Server::new(V1);
    let second_server = Server::new(V2);
    let selections = AtomicUsize::new(0);

    assert!(matches!(
        install_fixture(
            root,
            fixture_release("1.2.3", key, V1),
            &first_server,
            false,
            &selections,
        )
        .unwrap(),
        InstallOutcome::Installed { version } if version == "1.2.3"
    ));
    let (first_binary, record) = paths(root);
    let initial_record = fs::read(&record).unwrap();
    assert_eq!(fs::read(&first_binary).unwrap(), V1);

    assert!(matches!(
        install_fixture(
            root,
            fixture_release("1.2.3", key, V1),
            &first_server,
            false,
            &selections,
        )
        .unwrap(),
        InstallOutcome::AlreadyInstalled { version } if version == "1.2.3"
    ));
    assert_eq!(selections.load(Ordering::SeqCst), 2);
    assert_eq!(
        first_server.count(),
        1,
        "same-version install fetched an artifact"
    );
    assert_eq!(fs::read(&record).unwrap(), initial_record);

    assert!(matches!(
        install_fixture(
            root,
            fixture_release("1.2.3", key, V1),
            &first_server,
            true,
            &selections,
        )
        .unwrap(),
        InstallOutcome::Reinstalled { reason, .. } if reason == "force"
    ));
    let (forced_binary, _) = paths(root);
    assert_ne!(first_binary, forced_binary);
    assert_eq!(fs::read(&first_binary).unwrap(), V1);
    assert_eq!(first_server.count(), 2);

    fs::write(&forced_binary, b"damaged").unwrap();
    assert!(matches!(
        install_fixture(
            root,
            fixture_release("1.2.3", key, V1),
            &first_server,
            false,
            &selections,
        )
        .unwrap(),
        InstallOutcome::Reinstalled { reason, .. } if reason == "integrity"
    ));
    let (repaired_binary, _) = paths(root);
    assert_ne!(forced_binary, repaired_binary);
    assert_eq!(fs::read(&repaired_binary).unwrap(), V1);
    assert_eq!(fs::read(&first_binary).unwrap(), V1);

    assert!(matches!(
        install_fixture(
            root,
            fixture_release("1.2.4", key, V2),
            &second_server,
            false,
            &selections,
        )
        .unwrap(),
        InstallOutcome::Reinstalled { version, reason }
            if version == "1.2.4" && reason == "update"
    ));
    let (updated_binary, updated_record) = paths(root);
    assert_ne!(repaired_binary, updated_binary);
    assert_eq!(fs::read(&updated_binary).unwrap(), V2);
    assert_eq!(fs::read(&repaired_binary).unwrap(), V1);
    assert_eq!(
        record::read_record(&updated_record).unwrap().version,
        "1.2.4"
    );
    assert_eq!(selections.load(Ordering::SeqCst), 5);
    assert_eq!(second_server.count(), 1);
    assert_no_staging(root);
}

#[test]
fn metadata_errors_and_malformed_releases_leave_active_install_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let key = target::detect_target_key(Provider::Claude).unwrap();
    let server = Server::new(V1);
    let selections = AtomicUsize::new(0);
    install_fixture(
        root,
        fixture_release("1.2.3", key, V1),
        &server,
        false,
        &selections,
    )
    .unwrap();
    let (binary, record) = paths(root);
    let old_binary = fs::read(&binary).unwrap();
    let old_record = fs::read(&record).unwrap();

    let selections = &selections;
    let metadata_error = install_in(
        Provider::Claude,
        false,
        root,
        move |_| {
            selections.fetch_add(1, Ordering::SeqCst);
            Err(CruiseError::Other("metadata unavailable".into()))
        },
        &[],
        Some(&server.base),
    );
    assert!(metadata_error.is_err());

    let invalid_release = Release {
        version: "1.2.4-rc.1".into(),
        target: fixture_release("1.2.4", key, V1).target,
    };
    let malformed = install_in(
        Provider::Claude,
        false,
        root,
        move |_| {
            selections.fetch_add(1, Ordering::SeqCst);
            Ok(invalid_release)
        },
        &[],
        Some(&server.base),
    );
    assert!(malformed.is_err());
    assert_eq!(selections.load(Ordering::SeqCst), 3);
    assert_eq!(fs::read(&binary).unwrap(), old_binary);
    assert_eq!(fs::read(&record).unwrap(), old_record);
    assert_no_staging(root);
}

#[test]
fn bad_digest_and_probe_preserve_previous_generation_and_record() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let key = target::detect_target_key(Provider::Claude).unwrap();
    let good_server = Server::new(V1);
    let selections = AtomicUsize::new(0);
    install_fixture(
        root,
        fixture_release("1.2.3", key, V1),
        &good_server,
        false,
        &selections,
    )
    .unwrap();
    let (old_binary, record) = paths(root);
    let original_record = fs::read(&record).unwrap();

    let wrong_body = b"not the selected payload";
    let wrong_digest_release = fixture_release("1.2.4", key, V2);
    let wrong_digest_server = Server::new(wrong_body);
    assert!(
        install_fixture(
            root,
            wrong_digest_release,
            &wrong_digest_server,
            true,
            &selections,
        )
        .is_err()
    );
    assert_eq!(fs::read(&old_binary).unwrap(), V1);
    assert_eq!(fs::read(&record).unwrap(), original_record);

    let bad_probe_server = Server::new(WRONG_VERSION);
    assert!(
        install_fixture(
            root,
            fixture_release("1.2.4", key, WRONG_VERSION),
            &bad_probe_server,
            true,
            &selections,
        )
        .is_err()
    );
    assert_eq!(fs::read(&old_binary).unwrap(), V1);
    assert_eq!(fs::read(&record).unwrap(), original_record);
    assert_no_staging(root);
}

#[test]
fn concurrent_installs_serialize_release_selection_and_publication() {
    use fs2::FileExt;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let key = target::detect_target_key(Provider::Claude).unwrap();
    let release = fixture_release("1.2.3", key, V1);
    let server = Server::new(V1);
    let selections = AtomicUsize::new(0);
    let install = || {
        install_in(
            Provider::Claude,
            false,
            root,
            |key| {
                selections.fetch_add(1, Ordering::SeqCst);
                assert_eq!(key, release.target.key);
                let lock = fs::OpenOptions::new()
                    .write(true)
                    .open(root.join("providers/claude/.install.lock"))
                    .unwrap();
                assert!(
                    lock.try_lock_exclusive().is_err(),
                    "selection ran outside lock"
                );
                Ok(release.clone())
            },
            &[],
            Some(&server.base),
        )
        .unwrap()
    };
    let (first, second) = thread::scope(|scope| {
        let a = scope.spawn(install);
        let b = scope.spawn(install);
        (a.join().unwrap(), b.join().unwrap())
    });
    assert!(matches!(
        (&first, &second),
        (
            InstallOutcome::Installed { .. },
            InstallOutcome::AlreadyInstalled { .. }
        ) | (
            InstallOutcome::AlreadyInstalled { .. },
            InstallOutcome::Installed { .. }
        )
    ));
    assert_eq!(selections.load(Ordering::SeqCst), 2);
    assert_eq!(server.count(), 1);
    assert_eq!(fs::read(paths(root).0).unwrap(), V1);
    assert_no_staging(root);
}

#[test]
fn production_claude_metadata_resolution_installs_through_loopback() {
    let version = "1.2.3";
    let key = target::detect_target_key(Provider::Claude).unwrap();
    let checksum = crate::http_download::hex_digest(&Sha256::digest(V1));
    let platform = serde_json::json!({
        "binary": "claude",
        "checksum": checksum,
        "size": V1.len(),
    });
    let mut platforms = serde_json::Map::new();
    platforms.insert(key.into(), platform);
    let manifest = serde_json::to_vec(&serde_json::json!({
        "version": version,
        "platforms": platforms,
    }))
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let manifest_path = format!("/claude-code-releases/{version}/manifest.json");
    let binary_path = format!("/claude-code-releases/{version}/{key}/claude");
    let thread = thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 4096];
            let n = stream.read(&mut request).unwrap();
            let path = String::from_utf8_lossy(&request[..n])
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_owned();
            let body = if path == "/claude-code-releases/stable" {
                version.as_bytes()
            } else if path == manifest_path {
                manifest.as_slice()
            } else if path == binary_path {
                V1
            } else {
                panic!("unexpected metadata fixture route: {path}");
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let outcome = install_in(
        Provider::Claude,
        false,
        temp.path(),
        |key| release::latest_from(Provider::Claude, key, &base),
        &[],
        Some(&base),
    )
    .unwrap();
    thread.join().unwrap();
    assert!(matches!(
        outcome,
        InstallOutcome::Installed { version: installed } if installed == version
    ));
    assert_eq!(fs::read(paths(temp.path()).0).unwrap(), V1);
}
