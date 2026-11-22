use super::release::{Archive, Digest, Target};
use crate::{
    error::{CruiseError, Result},
    http_download::{
        MAX_ARCHIVE_ENTRIES, UrlTrust, checked_content_length, normalize_archive_path,
        open_response,
    },
};
use sha2::{Digest as _, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use url::Url;
const LIMIT: u64 = 512 * 1024 * 1024;
fn err(s: impl Into<String>) -> CruiseError {
    CruiseError::Other(s.into())
}

pub(crate) fn fetch_and_verify(
    target: &Target,
    dest: &Path,
    hosts: &[&str],
    #[cfg(test)] source_base: Option<&Url>,
) -> Result<()> {
    target.digest.validate()?;
    let original = Url::parse(&target.url).map_err(|e| err(e.to_string()))?;
    #[cfg(test)]
    let url = if let Some(base) = source_base {
        let loopback = match base.host() {
            Some(url::Host::Domain(host)) => host == "localhost",
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
        if !loopback
            || !matches!(base.scheme(), "http" | "https")
            || !base.username().is_empty()
            || base.password().is_some()
        {
            return Err(err("test provider source must be a loopback origin"));
        }
        Url::parse(&format!(
            "{}{}",
            base.origin().ascii_serialization(),
            &original[url::Position::BeforePath..]
        ))
        .map_err(|e| err(e.to_string()))?
    } else {
        original
    };
    #[cfg(not(test))]
    let url = original;
    #[cfg(test)]
    let trust = if let Some(base) = source_base {
        UrlTrust::Loopback(base)
    } else {
        UrlTrust::Hosts(hosts)
    };
    #[cfg(not(test))]
    let trust = UrlTrust::Hosts(hosts);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(300)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_send_request(Some(Duration::from_secs(30)))
        .timeout_send_body(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(30)))
        .build()
        .into();
    let response = open_response(&agent, &url, trust, "application/octet-stream")?;
    let expected_len = checked_content_length(&response, LIMIT)?;
    let archive_path = dest.with_extension("download");
    let mut archive = fs::File::create(&archive_path)?;
    let mut reader = response.into_body().into_reader();
    let mut sha256 = Sha256::new();
    let mut size = 0u64;
    let mut buf = vec![0u8; 65536];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size = size
            .checked_add(n as u64)
            .filter(|n| *n <= LIMIT)
            .ok_or_else(|| err("provider download exceeds size limit"))?;
        sha256.update(&buf[..n]);
        archive.write_all(&buf[..n])?;
    }
    if expected_len.is_some_and(|n| n != size) {
        return Err(err("provider download Content-Length mismatch"));
    }
    drop(archive);
    let Digest::Sha256(expected) = &target.digest;
    let actual = crate::http_download::hex_digest(&sha256.finalize());
    if actual.as_str() != expected.as_str() {
        return Err(err(format!(
            "provider SHA256 mismatch: expected {expected}, actual {actual}"
        )));
    }
    extract_archive(target, dest, &archive_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dest, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn extract_archive(target: &Target, dest: &Path, archive_path: &Path) -> Result<()> {
    match target.archive {
        Archive::Raw => {
            fs::rename(archive_path, dest)?;
        }
        Archive::TarGz => {
            let decoder = flate2::read::GzDecoder::new(fs::File::open(archive_path)?);
            let mut tar = tar::Archive::new(decoder);
            let mut found = false;
            let mut count = 0usize;
            let mut total = 0u64;
            for entry in tar.entries()? {
                let mut entry = entry?;
                count += 1;
                if count > MAX_ARCHIVE_ENTRIES {
                    return Err(err("too many archive entries"));
                }
                let ty = entry.header().entry_type();
                if !ty.is_file() && !ty.is_dir() {
                    return Err(err("unsafe archive entry type"));
                }
                let raw_path = entry.path_bytes();
                let raw_path =
                    std::str::from_utf8(&raw_path).map_err(|_| err("archive path is not UTF-8"))?;
                let normalized = normalize_archive_path(raw_path, ty.is_dir())?;
                let path = Path::new(&normalized);
                let length = entry.size();
                total = total
                    .checked_add(length)
                    .filter(|n| *n <= LIMIT)
                    .ok_or_else(|| err("archive exceeds size limit"))?;
                let expected = Path::new(&target.entry);
                if ty.is_file() && path != expected {
                    return Err(err("unexpected archive entry"));
                }
                if ty.is_dir() && !expected.starts_with(path) {
                    return Err(err("unexpected archive directory"));
                }
                if path == expected && ty.is_file() {
                    if found {
                        return Err(err("duplicate archive entry"));
                    }
                    let mut out = fs::File::create(dest)?;
                    std::io::copy(&mut entry, &mut out)?;
                    found = true;
                }
            }
            if !found {
                return Err(err("expected binary missing from archive"));
            }
            fs::remove_file(archive_path)?;
        }
    }
    Ok(())
}
pub(crate) fn probe(binary: &Path, expected: &str) -> Result<()> {
    probe_with_timeout(binary, expected, Duration::from_secs(30))
}

#[cfg(target_os = "linux")]
fn executable_busy(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::ETXTBSY)
}

#[cfg(not(target_os = "linux"))]
fn executable_busy(_error: &std::io::Error) -> bool {
    false
}

fn probe_with_timeout(binary: &Path, expected: &str, timeout: Duration) -> Result<()> {
    let mut cmd = Command::new(binary);
    cmd.args(["version", "--json"])
        .env("JCODE_NO_AUTO_UPDATE", "1");
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    let deadline = Instant::now() + timeout;
    let mut child = loop {
        match cmd.spawn() {
            Ok(child) => break child,
            // A concurrent fork can temporarily inherit the just-closed writable
            // download descriptor until exec closes it. Linux rejects execution
            // during that window, even though this process closed its handle.
            Err(e) if executable_busy(&e) && Instant::now() < deadline => {
                std::thread::sleep(
                    Duration::from_millis(10)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Err(e) => return Err(e.into()),
        }
    };
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| err("provider probe stdout unavailable"))?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = (|| {
            let mut output = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stdout.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                if output.len() < 65536 {
                    let keep = n.min(65536 - output.len());
                    output.extend_from_slice(&buf[..keep]);
                }
            }
            Ok::<_, std::io::Error>(output)
        })();
        let _ = sender.send(result);
    });
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Err(err("provider version probe timed out"));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(err("provider version probe failed"));
    }
    let output = receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|e| match e {
            std::sync::mpsc::RecvTimeoutError::Timeout => err("provider version probe timed out"),
            std::sync::mpsc::RecvTimeoutError::Disconnected => {
                err("provider probe output reader failed")
            }
        })??;
    let text = String::from_utf8_lossy(&output);
    let version = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .get("semver")
                .and_then(|version| version.as_str())
                .map(str::to_owned)
        });
    if version.as_deref() != Some(expected) {
        return Err(err(format!(
            "provider version mismatch: expected {expected}, got {}",
            version.as_deref().unwrap_or("invalid output")
        )));
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[expect(clippy::unwrap_used, reason = "test fixtures must succeed")]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[cfg(target_os = "linux")]
    #[test]
    fn probe_retries_text_file_busy_until_writer_closes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("tool");
        fs::write(&binary, b"#!/bin/sh\nprintf '{\"semver\":\"1.0.0\"}\\n'\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let writer = fs::OpenOptions::new().write(true).open(&binary).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(writer);
        });
        let result = probe_with_timeout(&binary, "1.0.0", Duration::from_secs(2));
        release.join().unwrap();
        result.unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn probe_does_not_retry_busy_file_indefinitely() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("tool");
        fs::write(&binary, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let _writer = fs::OpenOptions::new().write(true).open(&binary).unwrap();
        let start = Instant::now();
        assert!(
            probe_with_timeout(
                Provider::Jcode,
                &binary,
                "1.0.0",
                Duration::from_millis(100)
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn archive_rejects_extra_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("fixture.tar.gz");
        let file = fs::File::create(&archive).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(encoder);
        for name in ["jcode-linux-x86_64", "extra"] {
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, name, &b"x"[..]).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        let target = Target {
            key: "test",
            url: "https://example.com/test".into(),
            archive: Archive::TarGz,
            entry: "jcode-linux-x86_64".into(),
            digest: Digest::Sha256("unused".into()),
        };
        assert!(
            extract_archive(&target, &dir.path().join("binary"), &archive)
                .unwrap_err()
                .to_string()
                .contains("unexpected archive entry")
        );
    }

    #[test]
    fn truncated_content_length_rejects_download() {
        let target = Target {
            key: "test",
            url: "https://example.com/test".into(),
            archive: Archive::Raw,
            entry: "test".into(),
            digest: Digest::Sha256("0".repeat(64)),
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\nshort",
                )
                .unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        assert!(fetch_and_verify(&target, &dir.path().join("binary"), &[], Some(&base)).is_err());
        server.join().unwrap();
    }

    #[test]
    fn probe_times_out_when_descendant_retains_stdout() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("probe");
        fs::write(
            &binary,
            b"#!/bin/sh\nsleep 2 &\nprintf '{\"semver\":\"1.0.0\"}\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let start = Instant::now();
        let result = probe_with_timeout(&binary, "1.0.0", Duration::from_millis(100));
        assert!(result.unwrap_err().to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
