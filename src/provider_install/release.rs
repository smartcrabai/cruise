use super::Provider;
use crate::error::{CruiseError, Result};
use crate::http_download::{UrlTrust, get_bounded, validate_url};
use semver::Version as SemverVersion;
use serde::Deserialize;
use std::time::Duration;
use url::Url;

// Keep in sync with action/scripts/provision-jcode.sh's MIN_JCODE_VERSION.
pub const JCODE_MIN_VERSION: &str = "0.88.0";

const MAX_METADATA_SIZE: u64 = 1024 * 1024;
const JCODE_API: &str = "https://api.github.com/repos/1jehuang/jcode/releases/latest";
const JCODE_API_HOSTS: &[&str] = &["api.github.com"];
const JCODE_DOWNLOAD_HOSTS: &[&str] = &[
    "github.com",
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
    "github-cloud.s3.amazonaws.com",
];

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub target: Target,
}

#[derive(Clone, Debug)]
pub struct Target {
    pub key: &'static str,
    pub url: String,
    pub archive: Archive,
    pub entry: String,
    pub digest: Digest,
}

#[derive(Clone, Copy, Debug)]
pub enum Archive {
    Raw,
    TarGz,
}

#[derive(Clone, Debug)]
pub enum Digest {
    Sha256(String),
}

impl Digest {
    pub(crate) fn validate(&self) -> Result<()> {
        let Self::Sha256(value) = self;
        if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            Ok(())
        } else {
            Err(invalid("invalid SHA256 digest"))
        }
    }
}
#[cfg(test)]
pub(super) fn latest_from(provider: Provider, key: &'static str, base: &Url) -> Result<Release> {
    validate_loopback_base(base)?;
    resolve(provider, key, Some(base))
}
pub(crate) fn latest_jcode(key: &'static str) -> Result<Release> {
    resolve(Provider::Jcode, key, None)
}
fn resolve(provider: Provider, key: &'static str, source_base: Option<&Url>) -> Result<Release> {
    validate_target_key(provider, key)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(45)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_per_read(Some(Duration::from_secs(15)))
        .timeout_per_write(Some(Duration::from_secs(15)))
        .build()
        .into();
    fetch_latest_jcode(&agent, key, source_base)
}

fn fetch_latest_jcode(
    agent: &ureq::Agent,
    key: &'static str,
    source_base: Option<&Url>,
) -> Result<Release> {
    let api = Url::parse(JCODE_API).map_err(|error| invalid(&error.to_string()))?;
    let bytes = metadata(
        agent,
        &api,
        JCODE_API_HOSTS,
        "application/vnd.github+json",
        source_base,
    )?;
    let release: GithubRelease = serde_json::from_slice(&bytes)?;
    if release.draft || release.prerelease {
        return Err(invalid(
            "jcode latest release is not a stable published release",
        ));
    }
    let tag = release
        .tag_name
        .strip_prefix('v')
        .ok_or_else(|| invalid("jcode release tag must use a v-prefixed SemVer version"))?;
    let version = stable_version(tag)?;
    if release.tag_name != format!("v{version}") {
        return Err(invalid("jcode release tag is not canonical SemVer"));
    }

    let name = format!("jcode-{key}.tar.gz");
    let mut assets = release
        .assets
        .into_iter()
        .filter(|asset| asset.name == name);
    let asset = assets
        .next()
        .ok_or_else(|| invalid("jcode release is missing the requested platform asset"))?;
    if assets.next().is_some() {
        return Err(invalid("jcode release has duplicate platform assets"));
    }
    let digest_text = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .ok_or_else(|| invalid("jcode release asset is missing a SHA256 digest"))?;
    let digest = Digest::Sha256(digest_text.to_ascii_lowercase());
    digest.validate()?;

    let url = Url::parse(&asset.browser_download_url)
        .map_err(|error| invalid(&format!("invalid jcode asset URL: {error}")))?;
    validate_url(&url, UrlTrust::Hosts(JCODE_DOWNLOAD_HOSTS))?;
    let expected_path = format!("/1jehuang/jcode/releases/download/v{version}/{name}");
    if url.path() != expected_path {
        return Err(invalid(
            "jcode asset URL does not match its release version and platform",
        ));
    }
    Ok(Release {
        version,
        target: Target {
            key,
            url: url.to_string(),
            archive: Archive::TarGz,
            entry: if key.starts_with("windows-") {
                format!("jcode-{key}.exe")
            } else {
                format!("jcode-{key}")
            },
            digest,
        },
    })
}

fn metadata(
    agent: &ureq::Agent,
    upstream: &Url,
    hosts: &[&str],
    accept: &str,
    source_base: Option<&Url>,
) -> Result<Vec<u8>> {
    let url = if let Some(base) = source_base {
        loopback_url(base, upstream)?
    } else {
        upstream.clone()
    };
    let trust = source_base.map_or(UrlTrust::Hosts(hosts), UrlTrust::Loopback);
    get_bounded(agent, &url, trust, accept, MAX_METADATA_SIZE)
}

fn loopback_url(base: &Url, upstream: &Url) -> Result<Url> {
    let joined = format!(
        "{}{}",
        base.origin().ascii_serialization(),
        &upstream[url::Position::BeforePath..]
    );
    Url::parse(&joined).map_err(|error| invalid(&format!("invalid loopback metadata URL: {error}")))
}

#[cfg(test)]
fn validate_loopback_base(base: &Url) -> Result<()> {
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
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err(invalid(
            "test provider metadata source must be a loopback origin",
        ));
    }
    Ok(())
}

fn validate_target_key(_provider: Provider, key: &str) -> Result<()> {
    if matches!(
        key,
        "linux-x86_64"
            | "linux-aarch64"
            | "macos-aarch64"
            | "macos-x86_64"
            | "windows-x86_64"
            | "windows-aarch64"
    ) {
        Ok(())
    } else {
        Err(invalid("unsupported provider platform key"))
    }
}

fn stable_version(text: &str) -> Result<String> {
    let version = SemverVersion::parse(text)
        .map_err(|error| invalid(&format!("invalid stable release version: {error}")))?;
    if !version.pre.is_empty() || version.to_string() != text {
        return Err(invalid("release version is not canonical stable SemVer"));
    }
    Ok(text.to_string())
}

fn invalid(message: &str) -> CruiseError {
    CruiseError::Other(message.to_string())
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    digest: Option<String>,
    browser_download_url: String,
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "local release metadata fixtures")]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::{TcpListener, TcpStream},
        sync::{Arc, Mutex},
        thread,
    };

    const SHA256: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const VERSION: &str = "1.2.3";

    struct Reply {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl Reply {
        fn ok(body: impl Into<Vec<u8>>) -> Self {
            Self {
                status: 200,
                headers: vec![("Content-Type".into(), "application/json".into())],
                body: body.into(),
            }
        }

        fn json(body: &serde_json::Value) -> Self {
            Self::ok(serde_json::to_vec(body).unwrap())
        }
    }

    struct Server {
        base: Url,
        requests: Arc<Mutex<Vec<String>>>,
        join: thread::JoinHandle<()>,
    }

    impl Server {
        fn new(replies: Vec<Reply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let base = Url::parse(&format!("http://{address}/")).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let recorded = Arc::clone(&requests);
            let join = thread::spawn(move || {
                for reply in replies {
                    let (mut stream, _) = listener.accept().unwrap();
                    let request = read_request(&mut stream);
                    recorded.lock().unwrap().push(request);
                    write_reply(&mut stream, reply);
                }
            });
            Self {
                base,
                requests,
                join,
            }
        }

        fn finish(self) -> Vec<String> {
            self.join.join().unwrap();
            Arc::try_unwrap(self.requests)
                .unwrap()
                .into_inner()
                .unwrap()
        }
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
        }
        first
    }

    fn write_reply(stream: &mut TcpStream, reply: Reply) {
        let reason = match reply.status {
            200 => "OK",
            302 => "Found",
            500 => "Internal Server Error",
            _ => "Bad Request",
        };
        write!(stream, "HTTP/1.1 {} {reason}\r\n", reply.status).unwrap();
        let content_length = reply
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("Content-Length"));
        for (name, value) in reply.headers {
            write!(stream, "{name}: {value}\r\n").unwrap();
        }
        if !content_length {
            write!(stream, "Content-Length: {}\r\n", reply.body.len()).unwrap();
        }
        write!(stream, "Connection: close\r\n\r\n").unwrap();
        stream.write_all(&reply.body).unwrap();
    }

    fn jcode_metadata(key: &str) -> serde_json::Value {
        serde_json::json!({
            "tag_name": format!("v{VERSION}"),
            "draft": false,
            "prerelease": false,
            "assets": [{
                "name": format!("jcode-{key}.tar.gz"),
                "digest": format!("sha256:{SHA256}"),
                "browser_download_url": format!("https://github.com/1jehuang/jcode/releases/download/v{VERSION}/jcode-{key}.tar.gz")
            }]
        })
    }

    #[test]
    fn resolves_jcode_target_layouts_from_provider_metadata() {
        let jcode_keys = [
            "linux-x86_64",
            "linux-aarch64",
            "macos-aarch64",
            "macos-x86_64",
            "windows-x86_64",
            "windows-aarch64",
        ];
        for key in jcode_keys {
            let server = Server::new(vec![Reply::json(&jcode_metadata(key))]);
            let release = latest_from(Provider::Jcode, key, &server.base).unwrap();
            assert_eq!(release.version, VERSION);
            assert_eq!(release.target.key, key);
            assert!(matches!(release.target.archive, Archive::TarGz));
            assert_eq!(
                release.target.entry,
                if key.starts_with("windows-") {
                    format!("jcode-{key}.exe")
                } else {
                    format!("jcode-{key}")
                }
            );
            assert_eq!(
                release.target.url,
                format!(
                    "https://github.com/1jehuang/jcode/releases/download/v{VERSION}/jcode-{key}.tar.gz"
                )
            );
            assert!(matches!(release.target.digest, Digest::Sha256(_)));
            let requests = server.finish();
            assert!(requests[0].starts_with("GET /repos/1jehuang/jcode/releases/latest "));
        }
    }

    #[test]
    fn rejects_unstable_or_inconsistent_jcode_metadata() {
        let server = Server::new(vec![Reply::ok(b"{".to_vec())]);
        assert!(latest_from(Provider::Jcode, "linux-x86_64", &server.base).is_err());
        server.finish();

        let mut bad = jcode_metadata("linux-x86_64");
        bad["draft"] = true.into();
        assert_jcode_error(&bad);
        let mut bad = jcode_metadata("linux-x86_64");
        bad["prerelease"] = true.into();
        assert_jcode_error(&bad);
        let mut bad = jcode_metadata("linux-x86_64");
        bad["tag_name"] = "v1.2.3-rc.1".into();
        assert_jcode_error(&bad);
        let mut bad = jcode_metadata("linux-x86_64");
        bad["assets"][0]["digest"] = serde_json::Value::Null;
        assert_jcode_error(&bad);
        let mut bad = jcode_metadata("linux-x86_64");
        bad["assets"][0]["digest"] = "sha256:not-a-hash".into();
        assert_jcode_error(&bad);
        let mut bad = jcode_metadata("linux-x86_64");
        bad["assets"][0]["browser_download_url"] = "https://evil.example/file.tgz".into();
        assert_jcode_error(&bad);
        let mut bad = jcode_metadata("linux-x86_64");
        bad["assets"][0]["browser_download_url"] =
            "https://github.com/1jehuang/jcode/releases/download/v9.9.9/jcode-linux-x86_64.tar.gz"
                .into();
        assert_jcode_error(&bad);
    }

    fn assert_jcode_error(metadata: &serde_json::Value) {
        let server = Server::new(vec![Reply::json(metadata)]);
        assert!(latest_from(Provider::Jcode, "linux-x86_64", &server.base).is_err());
        server.finish();
    }

    #[test]
    fn rejects_http_redirects_oversized_metadata_and_non_loopback_source() {
        for reply in [
            Reply {
                status: 500,
                headers: vec![],
                body: b"failure".to_vec(),
            },
            Reply {
                status: 302,
                headers: vec![("Location".into(), "https://evil.example/metadata".into())],
                body: vec![],
            },
            Reply {
                status: 200,
                headers: vec![("Content-Length".into(), (MAX_METADATA_SIZE + 1).to_string())],
                body: vec![],
            },
        ] {
            let server = Server::new(vec![reply]);
            assert!(latest_from(Provider::Jcode, "linux-x86_64", &server.base).is_err());
            let requests = server.finish();
            assert_eq!(requests.len(), 1);
        }
        let remote = Url::parse("https://evil.example/").unwrap();
        assert!(latest_from(Provider::Jcode, "linux-x86_64", &remote).is_err());
    }

    #[test]
    fn sha256_digest_validation_rejects_invalid_encodings() {
        for digest in [String::new(), "0".into(), "g".repeat(64), "0".repeat(63)] {
            assert!(Digest::Sha256(digest).validate().is_err());
        }
        assert!(Digest::Sha256(SHA256.into()).validate().is_ok());
    }
}
