use super::{
    Provider,
    pins::{Archive, Digest, TargetPin},
};
use crate::{
    error::{CruiseError, Result},
    http_download::{
        MAX_ARCHIVE_ENTRIES, UrlTrust, checked_content_length, normalize_archive_path,
        open_response,
    },
};
use sha2::{Digest as _, Sha256, Sha512};
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
fn b64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        s.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        s.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        s.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        s.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    s
}
fn validate_digest(digest: Digest) -> Result<()> {
    if let Digest::Sha512Sri(expected) = digest {
        let s = expected
            .strip_prefix("sha512-")
            .ok_or_else(|| err("invalid SHA512 SRI"))?;
        if s.len() != 88
            || !s.ends_with("==")
            || !matches!(s.as_bytes()[85], b'A' | b'Q' | b'g' | b'w')
            || !s.as_bytes()[..86]
                .iter()
                .copied()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
        {
            return Err(err("invalid SHA512 SRI"));
        }
    }
    Ok(())
}

pub(crate) fn fetch_and_verify(
    target: &TargetPin,
    dest: &Path,
    hosts: &[&str],
    #[cfg(test)] source_base: Option<&Url>,
) -> Result<()> {
    validate_digest(target.digest)?;
    let original = Url::parse(target.url).map_err(|e| err(e.to_string()))?;
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
    let mut sha512 = Sha512::new();
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
        sha512.update(&buf[..n]);
        archive.write_all(&buf[..n])?;
    }
    if expected_len.is_some_and(|n| n != size) {
        return Err(err("provider download Content-Length mismatch"));
    }
    drop(archive);
    let actual = match target.digest {
        Digest::Sha256(expected) => {
            let actual = crate::http_download::hex_digest(&sha256.finalize());
            if actual != expected {
                return Err(err(format!(
                    "provider SHA256 mismatch: expected {expected}, actual {actual}"
                )));
            }
            actual
        }
        Digest::Sha512Sri(expected) => {
            let actual = format!("sha512-{}", b64(&sha512.finalize()));
            if actual != expected {
                return Err(err(format!(
                    "provider SHA512 mismatch: expected {expected}, actual {actual}"
                )));
            }
            actual
        }
    };
    let _ = actual;
    extract_archive(target, dest, &archive_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dest, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn extract_archive(target: &TargetPin, dest: &Path, archive_path: &Path) -> Result<()> {
    match target.archive {
        Archive::Raw => {
            fs::rename(archive_path, dest)?;
        }
        Archive::TarGz { root } => {
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
                if let Some(root) = root
                    && !path.starts_with(root)
                {
                    return Err(err("archive entry outside root"));
                }
                let length = entry.size();
                total = total
                    .checked_add(length)
                    .filter(|n| *n <= LIMIT)
                    .ok_or_else(|| err("archive exceeds size limit"))?;
                let expected = Path::new(target.entry);
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
pub(crate) fn probe(provider: Provider, binary: &Path, expected: &str) -> Result<()> {
    probe_with_timeout(provider, binary, expected, Duration::from_secs(30))
}

fn probe_with_timeout(provider: Provider, binary: &Path, expected: &str, timeout: Duration) -> Result<()> {
    let mut cmd = Command::new(binary);
    match provider {
        Provider::Jcode => {
            cmd.args(["version", "--json"])
                .env("JCODE_NO_AUTO_UPDATE", "1");
        }
        Provider::Claude => {
            cmd.arg("--version").env("DISABLE_AUTOUPDATER", "1");
        }
        Provider::OpenCode => {
            cmd.arg("--version").env("OPENCODE_DISABLE_AUTOUPDATE", "1");
        }
    }
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
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
                if n == 0 { break; }
                if output.len() < 65536 {
                    let keep = n.min(65536 - output.len());
                    output.extend_from_slice(&buf[..keep]);
                }
            }
            Ok::<_, std::io::Error>(output)
        })();
        let _ = sender.send(result);
    });
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            std::thread::spawn(move || { let _ = child.wait(); });
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
            std::sync::mpsc::RecvTimeoutError::Disconnected => err("provider probe output reader failed"),
        })??;
    let text = String::from_utf8_lossy(&output);
    let version = match provider {
        Provider::Jcode => serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("semver").and_then(|v| v.as_str()).map(str::to_owned)),
        Provider::Claude => text.split_whitespace().next().map(str::to_owned),
        Provider::OpenCode => text
            .split_whitespace()
            .last()
            .map(|v| v.trim_start_matches('v').to_owned()),
    };
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

    #[test]
    fn archive_rejects_extra_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("fixture.tar.gz");
        let file = fs::File::create(&archive).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(encoder);
        for name in ["package/bin/opencode", "package/extra"] {
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, name, &b"x"[..]).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        let target = TargetPin { key: "test", url: "https://example.com/test", archive: Archive::TarGz { root: Some("package") }, entry: "package/bin/opencode", digest: Digest::Sha256("unused") };
        assert!(extract_archive(&target, &dir.path().join("binary"), &archive).unwrap_err().to_string().contains("unexpected archive entry"));
    }

    #[test]
    fn sha512_mismatch_rejects_download() {
        let body = b"payload";
        let sri: &'static str = Box::leak(format!("sha512-{}", b64(&Sha512::digest(b"other"))).into_boxed_str());
        let target = TargetPin { key: "test", url: "https://example.com/test", archive: Archive::Raw, entry: "test", digest: Digest::Sha512Sri(sri) };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            stream.write_all(body).unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let error = fetch_and_verify(&target, &dir.path().join("binary"), &[], Some(&base)).unwrap_err();
        assert!(error.to_string().contains("SHA512 mismatch"));
        server.join().unwrap();
    }

    #[test]
    fn truncated_content_length_rejects_download() {
        let target = TargetPin { key: "test", url: "https://example.com/test", archive: Archive::Raw, entry: "test", digest: Digest::Sha256("unused") };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\nshort").unwrap();
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
        fs::write(&binary, b"#!/bin/sh\nsleep 2 &\nprintf '1.0.0\\n'\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let start = Instant::now();
        let result = probe_with_timeout(Provider::Claude, &binary, "1.0.0", Duration::from_millis(100));
        assert!(result.unwrap_err().to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn verified_raw_download_and_probe() {
        let body = b"#!/bin/sh\nprintf '2.1.250 (Claude Code)\\n'\n";
        let hash = crate::http_download::hex_digest(&Sha256::digest(body));
        let hash: &'static str = Box::leak(hash.into_boxed_str());
        let target = TargetPin {
            key: "test",
            url: "https://downloads.claude.ai/test?token=fixture",
            archive: Archive::Raw,
            entry: "claude",
            digest: Digest::Sha256(hash),
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!(
            "http://{}/ignored/prefix",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let n = stream.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET /test?token=fixture "));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("claude");
        fetch_and_verify(&target, &dest, &[], Some(&base)).unwrap();
        probe(Provider::Claude, &dest, "2.1.250").unwrap();
        assert!(probe(Provider::Claude, &dest, "wrong").is_err());
        server.join().unwrap();
    }
}
