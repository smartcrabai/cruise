use super::process::run_bounded;
use crate::cancellation::CancellationToken;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};
const URL: &str = "https://github.com/AlloyTools/org.alloytools.alloy/releases/download/v6.2.0/org.alloytools.alloy.dist.jar";
const HASH: &str = "6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d";
pub fn java_major(s: &str) -> Result<u32, String> {
    let version = s.split('"').nth(1).ok_or("cannot parse Java version")?;
    let mut parts = version.split('.');
    let first = parts
        .next()
        .and_then(|v| v.parse::<u32>().ok())
        .ok_or("cannot parse Java version")?;
    if first == 1 {
        parts
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| "cannot parse Java version".into())
    } else {
        Ok(first)
    }
}
pub fn check_quint(
    dir: &std::path::Path,
    cancel: Option<&CancellationToken>,
) -> Result<Option<String>, String> {
    let mut c = Command::new("quint");
    c.arg("--version");
    let out = run_bounded(c, Duration::from_secs(60), cancel, dir, "quint-version");
    if out.cancelled {
        return Err("cancelled".into());
    }
    if out.exit != Some(0) {
        return Err(format!(
            "quint unavailable: {} {}",
            out.message,
            out.output()
        ));
    }
    let version = out.output();
    Ok(
        if version
            .split_whitespace()
            .any(|v| v.trim_start_matches('v') == "0.33.0")
        {
            None
        } else {
            Some(format!(
                "Quint version differs from pinned 0.33.0: {}",
                version.trim()
            ))
        },
    )
}
pub fn check_java(dir: &std::path::Path, cancel: Option<&CancellationToken>) -> Result<(), String> {
    let mut c = Command::new("java");
    c.arg("-version");
    let out = run_bounded(c, Duration::from_secs(60), cancel, dir, "java-version");
    if out.exit != Some(0) {
        return Err(format!(
            "java unavailable: {} {}",
            out.message,
            out.output()
        ));
    }
    let major = java_major(&out.output())?;
    if major < 17 {
        return Err(format!("Java 17 or newer required, found {major}"));
    }
    Ok(())
}
fn digest(path: &std::path::Path) -> Result<String, String> {
    let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
pub fn alloy_jar(timeout: Duration, cancel: Option<&CancellationToken>) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("CRUISE_ALLOY_JAR") {
        let path = PathBuf::from(path);
        return if path.is_absolute() {
            Ok(path)
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .map_err(|e| e.to_string())
        };
    }
    let dir = crate::paths::data_dir()
        .map_err(|e| e.to_string())?
        .join("formal-tools");
    alloy_jar_in(&dir, HASH, |file| download(URL, file, timeout, cancel))
}

// Keep the file on the caller thread so cancellation can remove it immediately,
// even if a socket operation is still waiting for the HTTP timeout.
fn download(url: &str, file: &mut fs::File, timeout: Duration, cancel: Option<&CancellationToken>) -> Result<(), String> {
    let started = Instant::now();
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        return Err("cancelled".into());
    }
    if timeout.is_zero() {
        return Err("Alloy download timed out".into());
    }
    let url = url.to_owned();
    let (tx, rx) = mpsc::sync_channel::<Result<Vec<u8>, String>>(2);
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(timeout))
                .build().into();
            let mut response = agent.get(&url).call().map_err(|e| e.to_string())?;
            let mut reader = response.body_mut().as_reader();
            loop {
                let mut buf = vec![0; 16384];
                let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
                buf.truncate(n);
                if tx.send(Ok(buf)).is_err() || n == 0 {
                    return Ok(());
                }
            }
        })();
        if let Err(error) = result {
            let _ = tx.send(Err(error));
        }
    });
    loop {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err("cancelled".into());
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err("Alloy download timed out".into());
        }
        match rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
            Ok(Ok(bytes)) if bytes.is_empty() => return Ok(()),
            Ok(Ok(bytes)) => file.write_all(&bytes).map_err(|e| e.to_string())?,
            Ok(Err(error)) => return Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("Alloy download worker stopped".into()),
        }
    }
}

fn alloy_jar_in(
    dir: &std::path::Path,
    expected_hash: &str,
    fetch: impl FnOnce(&mut fs::File) -> Result<(), String>,
) -> Result<PathBuf, String> {
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let jar = dir.join("alloy-6.2.0.jar");
    if digest(&jar).is_ok_and(|d| d == expected_hash) {
        return Ok(jar);
    }
    let part = dir.join(format!("alloy-{}.part", uuid::Uuid::new_v4()));
    let result = (|| -> Result<(), String> {
        let mut file = fs::File::create(&part).map_err(|e| e.to_string())?;
        fetch(&mut file)?;
        file.flush().map_err(|e| e.to_string())?;
        if digest(&part)? != expected_hash {
            return Err("Alloy JAR SHA-256 mismatch".into());
        }
        fs::rename(&part, &jar).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result.map(|()| jar)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stalled_download(cancel_download: bool) {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/alloy", listener.local_addr().unwrap());
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\npartial").unwrap();
            let _ = ready_tx.send(());
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
        });
        let dir = tempfile::tempdir().unwrap();
        let token = CancellationToken::new();
        let canceller = if cancel_download {
            let token = token.clone();
            Some(std::thread::spawn(move || {
                ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                std::thread::sleep(Duration::from_millis(100));
                token.cancel();
            }))
        } else {
            None
        };
        let start = Instant::now();
        let timeout = if cancel_download { Duration::from_secs(3) } else { Duration::from_millis(300) };
        let error = alloy_jar_in(dir.path(), HASH, |file| download(&url, file, timeout, Some(&token))).unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(2), "download did not stop promptly");
        if cancel_download {
            assert_eq!(error, "cancelled");
        } else {
            assert!(error.to_lowercase().contains("timed out") || error.to_lowercase().contains("timeout"), "{error}");
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        let _ = release_tx.send(());
        server.join().unwrap();
        if let Some(canceller) = canceller { canceller.join().unwrap(); }
    }

    #[test]
    fn stalled_download_times_out_and_removes_partial() {
        stalled_download(false);
    }

    #[test]
    fn stalled_download_cancels_and_removes_partial() {
        stalled_download(true);
    }

    #[test]
    fn alloy_download_validates_before_rename_and_removes_mismatched_part() {
        let dir = tempfile::tempdir().unwrap();
        let jar = dir.path().join("alloy-6.2.0.jar");
        let correct = b"valid Alloy bytes";
        let expected = "8c85ac41c7c94f3d79a12b53dafca4968820d1cef033aacdb5579dba78e20e1b";
        fs::write(&jar, b"existing invalid bytes").unwrap();

        let error = alloy_jar_in(dir.path(), expected, |file| {
            file.write_all(b"incorrect download")
                .map_err(|e| e.to_string())
        })
        .unwrap_err();
        assert_eq!(error, "Alloy JAR SHA-256 mismatch");
        assert_eq!(fs::read(&jar).unwrap(), b"existing invalid bytes");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);

        let result = alloy_jar_in(dir.path(), expected, |file| {
            file.write_all(correct).map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(result, jar);
        assert_eq!(fs::read(&jar).unwrap(), correct);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn parses_java_major_versions() {
        assert_eq!(java_major("java version \"1.8.0_402\""), Ok(8));
        assert_eq!(java_major("openjdk version \"17.0.10\""), Ok(17));
        assert_eq!(java_major("openjdk version \"21\""), Ok(21));
        for invalid in [
            "",
            "openjdk 17",
            "java version \"x.0\"",
            "java version \"1.x\"",
        ] {
            assert!(java_major(invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn alloy_override_is_resolved_in_child_environment() {
        const CHILD: &str = "CRUISE_TEST_ALLOY_OVERRIDE_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let expected = std::env::current_dir()
                .unwrap()
                .join("relative-test-alloy.jar");
            assert_eq!(alloy_jar(Duration::from_secs(1), None).unwrap(), expected);
            return;
        }
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "formal_verify::tools::tests::alloy_override_is_resolved_in_child_environment",
            ])
            .env(CHILD, "1")
            .env("CRUISE_ALLOY_JAR", "relative-test-alloy.jar")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[cfg(unix)]
    #[test]
    fn quint_version_warning_in_child_environment() {
        use std::os::unix::fs::PermissionsExt;
        const CHILD: &str = "CRUISE_TEST_QUINT_VERSION_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let dir = tempfile::tempdir().unwrap();
            let warning = check_quint(dir.path(), None).unwrap().unwrap();
            assert!(warning.contains("0.33.0"));
            assert!(warning.contains("0.34.0"));
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let quint = dir.path().join("quint");
        fs::write(&quint, "#!/bin/sh\necho 'quint 0.34.0'\n").unwrap();
        fs::set_permissions(&quint, fs::Permissions::from_mode(0o755)).unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "formal_verify::tools::tests::quint_version_warning_in_child_environment",
            ])
            .env(CHILD, "1")
            .env("PATH", dir.path())
            .status()
            .unwrap();
        assert!(status.success());
    }
}
