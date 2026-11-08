use super::*;
use pins::{Archive, Digest, Pin, TargetPin};
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

const SCRIPT: &[u8] = b"#!/bin/sh\nprintf '2.1.250 (Claude Code)\\n'\n";

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

fn fixture_pin(body: &[u8]) -> Pin {
    let digest = crate::http_download::hex_digest(&Sha256::digest(body));
    let target = TargetPin {
        key: target::detect_target_key(Provider::Claude).unwrap(),
        url: "https://downloads.claude.ai/fixture",
        archive: Archive::Raw,
        entry: "claude",
        digest: Digest::Sha256(Box::leak(digest.into_boxed_str())),
    };
    Pin {
        version: "2.1.250",
        targets: Box::leak(Box::new([target])),
    }
}
fn install_fixture(root: &Path, pin: &Pin, server: &Server, force: bool) -> Result<InstallOutcome> {
    install_in(Provider::Claude, force, root, pin, &[], Some(&server.base))
}
fn paths(root: &Path) -> (PathBuf, PathBuf) {
    let dir = root.join("providers/claude");
    let record = dir.join("install.json");
    let relative = record::read_record(&record).unwrap().relative_path;
    (dir.join(relative), record)
}
fn assert_no_staging(root: &Path) {
    let dir = root.join("providers/claude");
    assert!(!fs::read_dir(dir).unwrap().any(|e| {
        e.unwrap()
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
    let server = Server::new(SCRIPT);
    install_fixture(temp.path(), &fixture_pin(SCRIPT), &server, false).unwrap();
    assert!(!dir.exists());
    assert_eq!(fs::read(paths(temp.path()).0).unwrap(), SCRIPT);
}

#[test]
fn install_noop_force_and_integrity_repair() {
    let temp = tempfile::tempdir().unwrap();
    let server = Server::new(SCRIPT);
    let pin = fixture_pin(SCRIPT);
    let root = temp.path();
    assert!(matches!(
        install_fixture(root, &pin, &server, false).unwrap(),
        InstallOutcome::Installed { .. }
    ));
    let (binary, record) = paths(root);
    let initial_record = fs::read(&record).unwrap();
    assert_eq!(fs::read(&binary).unwrap(), SCRIPT);
    assert!(matches!(
        install_fixture(root, &pin, &server, false).unwrap(),
        InstallOutcome::AlreadyInstalled { .. }
    ));
    assert_eq!(server.count(), 1);
    assert_eq!(fs::read(&record).unwrap(), initial_record);
    assert!(
        matches!(install_fixture(root, &pin, &server, true).unwrap(), InstallOutcome::Reinstalled { reason, .. } if reason == "force")
    );
    assert_eq!(server.count(), 2);
    let replacement = paths(root).0;
    assert_ne!(binary, replacement);
    assert_eq!(fs::read(&binary).unwrap(), SCRIPT);
    let binary = replacement;
    fs::write(&binary, b"corrupt").unwrap();
    assert!(
        matches!(install_fixture(root, &pin, &server, false).unwrap(), InstallOutcome::Reinstalled { reason, .. } if reason == "integrity")
    );
    assert_eq!(server.count(), 3);
    assert_eq!(fs::read(paths(root).0).unwrap(), SCRIPT);
    assert_no_staging(root);
}

#[test]
fn failed_digest_and_probe_preserve_previous_install() {
    let temp = tempfile::tempdir().unwrap();
    let good = Server::new(SCRIPT);
    let pin = fixture_pin(SCRIPT);
    let root = temp.path();
    install_fixture(root, &pin, &good, false).unwrap();
    let (binary, record) = paths(root);
    let original_record = fs::read(&record).unwrap();
    let wrong = Server::new(b"not the pinned payload");
    assert!(install_fixture(root, &pin, &wrong, true).is_err());
    assert_eq!(fs::read(&binary).unwrap(), SCRIPT);
    assert_eq!(fs::read(&record).unwrap(), original_record);
    assert_no_staging(root);
    let bad_script = b"#!/bin/sh\necho '9.9.9 (Claude Code)'\n";
    let bad = Server::new(bad_script);
    let bad_pin = fixture_pin(bad_script);
    assert!(install_fixture(root, &bad_pin, &bad, true).is_err());
    assert_eq!(fs::read(&binary).unwrap(), SCRIPT);
    assert_eq!(fs::read(&record).unwrap(), original_record);
    assert_no_staging(root);
}

#[test]
fn concurrent_installs_share_lock_and_download_once() {
    let temp = tempfile::tempdir().unwrap();
    let server = Server::new(SCRIPT);
    let pin = fixture_pin(SCRIPT);
    let root = temp.path();
    let (first, second) = thread::scope(|scope| {
        let a = scope.spawn(|| install_fixture(root, &pin, &server, false).unwrap());
        let b = scope.spawn(|| install_fixture(root, &pin, &server, false).unwrap());
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
    assert_eq!(server.count(), 1);
    assert_eq!(fs::read(paths(root).0).unwrap(), SCRIPT);
    assert_no_staging(root);
}
