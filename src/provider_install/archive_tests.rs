#![expect(
    clippy::unwrap_used,
    reason = "isolated loopback and archive test fixtures"
)]

use super::{
    fetch::fetch_and_verify,
    release::{Archive, Digest, Target},
};
use crate::http_download::MAX_ARCHIVE_ENTRIES;
use flate2::{Compression, write::GzEncoder};
use sha2::{Digest as _, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};
use url::Url;

fn target(digest: Digest, archive: Archive) -> Target {
    Target {
        key: "fixture",
        url: "https://downloads.claude.ai/fixture".into(),
        archive,
        entry: "package/bin/tool".into(),
        digest,
    }
}

fn archive(entries: &[(&str, tar::EntryType)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    for (path, kind) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o755);
        header.set_entry_type(*kind);
        if kind.is_symlink() || kind.is_hard_link() {
            header.set_link_name("target").unwrap();
        }
        // Write the name directly so malicious traversal paths are not rejected by
        // tar's safe path-building helpers before reaching the code under test.
        let name = &mut header.as_mut_bytes()[..100];
        name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_cksum();
        builder.append(&header, &[][..]).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn opencode_archive() -> Vec<u8> {
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    for path in ["package/", "package/bin/"] {
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o755);
        header.set_entry_type(tar::EntryType::Directory);
        builder.append_data(&mut header, path, &[][..]).unwrap();
    }
    for (path, contents) in [
        (
            "package/package.json",
            br#"{"name":"@opencode/cli-darwin-arm64","version":"2.3.0","license":"MIT","repository":{"type":"git","url":"git+https://github.com/anomalyco/opencode.git"},"os":["darwin"],"cpu":["arm64"]}"#.as_slice(),
        ),
        ("package/bin/opencode", b"fixture executable".as_slice()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o755);
        header.set_entry_type(tar::EntryType::Regular);
        builder.append_data(&mut header, path, contents).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn rejected_archive(bytes: Vec<u8>, expected: &str) {
    let digest = crate::http_download::hex_digest(&Sha256::digest(&bytes));
    let artifact = target(
        Digest::Sha256(digest),
        Archive::TarGz {
            root: Some("package"),
        },
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 2048];
        assert!(stream.read(&mut request).unwrap() > 0);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .unwrap();
        stream.write_all(&bytes).unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("tool");
    let error = fetch_and_verify(&artifact, &dest, &[], Some(&base)).unwrap_err();
    server.join().unwrap();
    assert!(
        error.to_string().contains(expected),
        "unexpected error: {error}"
    );
}

#[test]
fn malformed_sri_is_rejected_before_network() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let dir = tempfile::tempdir().unwrap();
    for sri in [
        "sha256-invalid",
        "sha512-short",
        "sha512-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB==",
    ] {
        let artifact = target(Digest::Sha512Sri(sri.into()), Archive::Raw);
        let error =
            fetch_and_verify(&artifact, &dir.path().join("tool"), &[], Some(&base)).unwrap_err();
        assert!(error.to_string().contains("invalid SHA512 SRI"), "{error}");
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn traversal_and_links_are_rejected() {
    rejected_archive(
        archive(&[("package/../escape", tar::EntryType::Regular)]),
        "archive",
    );
    for kind in [tar::EntryType::Symlink, tar::EntryType::Link] {
        rejected_archive(
            archive(&[("package/bin/tool", kind)]),
            "unsafe archive entry type",
        );
    }
}

#[test]
fn npm_opencode_package_metadata_is_accepted() {
    let bytes = opencode_archive();
    let artifact = Target {
        key: "darwin-arm64",
        url: "https://registry.npmjs.org/@opencode/cli-darwin-arm64/latest".into(),
        archive: Archive::TarGz {
            root: Some("package"),
        },
        entry: "package/bin/opencode".into(),
        digest: Digest::Sha256(crate::http_download::hex_digest(&Sha256::digest(&bytes))),
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 2048];
        assert!(stream.read(&mut request).unwrap() > 0);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .unwrap();
        stream.write_all(&bytes).unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("opencode");
    fetch_and_verify(&artifact, &dest, &[], Some(&base)).unwrap();
    server.join().unwrap();
    assert_eq!(
        std::fs::read(dest).unwrap(),
        b"fixture executable",
        "the expected OpenCode executable should be extracted"
    );
}

#[test]
fn unrelated_archive_files_remain_rejected() {
    rejected_archive(
        archive(&[
            ("package/bin/tool", tar::EntryType::Regular),
            ("package/README.md", tar::EntryType::Regular),
        ]),
        "unexpected archive entry",
    );
    rejected_archive(
        archive(&[
            ("package/bin/tool", tar::EntryType::Regular),
            ("package/package.json", tar::EntryType::Regular),
        ]),
        "unexpected archive entry",
    );
}

#[test]
fn duplicate_required_binary_is_rejected() {
    rejected_archive(
        archive(&[
            ("package/bin/tool", tar::EntryType::Regular),
            ("package/bin/tool", tar::EntryType::Regular),
        ]),
        "duplicate archive entry",
    );
}

#[test]
fn excessive_entries_and_missing_binary_are_rejected() {
    let entries = vec![("package", tar::EntryType::Directory); MAX_ARCHIVE_ENTRIES + 1];
    rejected_archive(archive(&entries), "too many archive entries");
    rejected_archive(
        archive(&[("package", tar::EntryType::Directory)]),
        "expected binary missing",
    );
}

#[test]
fn redirect_to_untrusted_host_is_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 2048];
        assert!(stream.read(&mut request).unwrap() > 0);
        stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://example.invalid/fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("tool");
    let artifact = target(Digest::Sha256("0".repeat(64)), Archive::Raw);
    let error = fetch_and_verify(&artifact, &dest, &[], Some(&base)).unwrap_err();
    server.join().unwrap();
    assert!(
        error
            .to_string()
            .contains("left the configured loopback server"),
        "{error}"
    );
    assert!(!dest.exists());
}
