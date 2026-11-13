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
const CLAUDE_STABLE: &str = "https://downloads.claude.ai/claude-code-releases/stable";
const CLAUDE_RELEASE_ROOT: &str = "https://downloads.claude.ai/claude-code-releases";
const OPENCODE_REGISTRY: &str = "https://registry.npmjs.org";
const JCODE_API_HOSTS: &[&str] = &["api.github.com"];
const JCODE_DOWNLOAD_HOSTS: &[&str] = &[
    "github.com",
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
    "github-cloud.s3.amazonaws.com",
];
const CLAUDE_HOSTS: &[&str] = &["downloads.claude.ai"];
const OPENCODE_HOSTS: &[&str] = &["registry.npmjs.org"];

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
    TarGz { root: Option<&'static str> },
}

#[derive(Clone, Debug)]
pub enum Digest {
    Sha256(String),
    Sha512Sri(String),
}

impl Digest {
    pub(crate) fn validate(&self) -> Result<()> {
        let valid = match self {
            Self::Sha256(value) => {
                value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            }
            Self::Sha512Sri(value) => {
                let Some(encoded) = value.strip_prefix("sha512-") else {
                    return Err(invalid("invalid SHA512 SRI digest"));
                };
                encoded.len() == 88
                    && encoded.ends_with("==")
                    && matches!(encoded.as_bytes()[85], b'A' | b'Q' | b'g' | b'w')
                    && encoded.as_bytes()[..86]
                        .iter()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
            }
        };
        if valid {
            Ok(())
        } else {
            Err(invalid(match self {
                Self::Sha256(_) => "invalid SHA256 digest",
                Self::Sha512Sri(_) => "invalid SHA512 SRI digest",
            }))
        }
    }
}

pub(crate) fn latest(provider: Provider, key: &'static str) -> Result<Release> {
    resolve(provider, key, None)
}

#[cfg(test)]
pub(super) fn latest_from(provider: Provider, key: &'static str, base: &Url) -> Result<Release> {
    validate_loopback_base(base)?;
    resolve(provider, key, Some(base))
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
    match provider {
        Provider::Jcode => latest_jcode(&agent, key, source_base),
        Provider::Claude => latest_claude(&agent, key, source_base),
        Provider::OpenCode => latest_opencode(&agent, key, source_base),
    }
}

fn latest_jcode(
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
            archive: Archive::TarGz { root: None },
            entry: if key.starts_with("windows-") {
                format!("jcode-{key}.exe")
            } else {
                format!("jcode-{key}")
            },
            digest,
        },
    })
}

fn latest_claude(
    agent: &ureq::Agent,
    key: &'static str,
    source_base: Option<&Url>,
) -> Result<Release> {
    let stable = Url::parse(CLAUDE_STABLE).map_err(|error| invalid(&error.to_string()))?;
    let bytes = metadata(agent, &stable, CLAUDE_HOSTS, "text/plain", source_base)?;
    let version_text = std::str::from_utf8(&bytes)
        .map_err(|error| invalid(&format!("invalid Claude stable version: {error}")))?
        .trim();
    let version = stable_version(version_text)?;

    let manifest_url = Url::parse(&format!("{CLAUDE_RELEASE_ROOT}/{version}/manifest.json"))
        .map_err(|error| invalid(&error.to_string()))?;
    let bytes = metadata(
        agent,
        &manifest_url,
        CLAUDE_HOSTS,
        "application/json",
        source_base,
    )?;
    let manifest: ClaudeManifest = serde_json::from_slice(&bytes)?;
    let manifest_version = stable_version(&manifest.version)?;
    if manifest_version != version {
        return Err(invalid(
            "Claude manifest version does not match the stable channel",
        ));
    }
    let platform = manifest
        .platforms
        .get(key)
        .ok_or_else(|| invalid("Claude manifest is missing the requested platform"))?;
    let expected_binary = if key.starts_with("win32-") {
        "claude.exe"
    } else {
        "claude"
    };
    if platform.binary != expected_binary {
        return Err(invalid(
            "Claude manifest binary does not match the requested platform",
        ));
    }
    if platform.size == 0 {
        return Err(invalid("Claude manifest has an invalid binary size"));
    }
    let digest = Digest::Sha256(platform.checksum.to_ascii_lowercase());
    digest.validate()?;
    let url = Url::parse(&format!(
        "{CLAUDE_RELEASE_ROOT}/{version}/{key}/{expected_binary}"
    ))
    .map_err(|error| invalid(&error.to_string()))?;
    validate_url(&url, UrlTrust::Hosts(CLAUDE_HOSTS))?;
    Ok(Release {
        version,
        target: Target {
            key,
            url: url.to_string(),
            archive: Archive::Raw,
            entry: expected_binary.to_string(),
            digest,
        },
    })
}

fn latest_opencode(
    agent: &ureq::Agent,
    key: &'static str,
    source_base: Option<&Url>,
) -> Result<Release> {
    let package = format!("@opencode/cli-{key}");
    let api = Url::parse(&format!("{OPENCODE_REGISTRY}/{package}/latest"))
        .map_err(|error| invalid(&error.to_string()))?;
    let bytes = metadata(agent, &api, OPENCODE_HOSTS, "application/json", source_base)?;
    let metadata: NpmRelease = serde_json::from_slice(&bytes)?;
    if metadata.name != package {
        return Err(invalid(
            "OpenCode package metadata does not match the requested platform",
        ));
    }
    let version = stable_version(&metadata.version)?;
    let expected_path = format!("/{package}/-/cli-{key}-{version}.tgz");
    let url = Url::parse(&metadata.dist.tarball)
        .map_err(|error| invalid(&format!("invalid OpenCode tarball URL: {error}")))?;
    validate_url(&url, UrlTrust::Hosts(OPENCODE_HOSTS))?;
    if url.path() != expected_path || url.query().is_some() {
        return Err(invalid(
            "OpenCode tarball URL does not match its package and version",
        ));
    }
    let digest = Digest::Sha512Sri(metadata.dist.integrity);
    digest.validate()?;
    Ok(Release {
        version,
        target: Target {
            key,
            url: url.to_string(),
            archive: Archive::TarGz {
                root: Some("package"),
            },
            entry: if key.starts_with("windows-") {
                "package/bin/opencode.exe".to_string()
            } else {
                "package/bin/opencode".to_string()
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

fn validate_target_key(provider: Provider, key: &str) -> Result<()> {
    let valid = match provider {
        Provider::Jcode => matches!(
            key,
            "linux-x86_64"
                | "linux-aarch64"
                | "macos-aarch64"
                | "macos-x86_64"
                | "windows-x86_64"
                | "windows-aarch64"
        ),
        Provider::Claude => matches!(
            key,
            "darwin-arm64"
                | "darwin-x64"
                | "linux-x64"
                | "linux-arm64"
                | "linux-x64-musl"
                | "linux-arm64-musl"
                | "win32-x64"
                | "win32-arm64"
        ),
        Provider::OpenCode => matches!(
            key,
            "darwin-arm64"
                | "darwin-x64"
                | "darwin-x64-baseline"
                | "linux-x64"
                | "linux-x64-baseline"
                | "linux-arm64"
                | "linux-x64-musl"
                | "linux-x64-baseline-musl"
                | "linux-arm64-musl"
                | "windows-x64"
                | "windows-x64-baseline"
                | "windows-arm64"
        ),
    };
    if valid {
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

#[derive(Deserialize)]
struct ClaudeManifest {
    version: String,
    platforms: std::collections::HashMap<String, ClaudePlatform>,
}

#[derive(Deserialize)]
struct ClaudePlatform {
    binary: String,
    checksum: String,
    size: u64,
}

#[derive(Deserialize)]
struct NpmRelease {
    name: String,
    version: String,
    dist: NpmDist,
}

#[derive(Deserialize)]
struct NpmDist {
    tarball: String,
    integrity: String,
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
    const SRI: &str = "sha512-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==";
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

        fn json(body: serde_json::Value) -> Self {
            Self::ok(serde_json::to_vec(&body).unwrap())
        }

        fn text(body: &str) -> Self {
            let mut reply = Self::ok(body.as_bytes().to_vec());
            reply.headers[0].1 = "text/plain".into();
            reply
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

    fn claude_manifest(key: &str) -> serde_json::Value {
        serde_json::json!({
            "version": VERSION,
            "platforms": {
                key: {
                    "binary": if key.starts_with("win32-") { "claude.exe" } else { "claude" },
                    "checksum": SHA256,
                    "size": 123
                }
            }
        })
    }

    fn npm_metadata(key: &str) -> serde_json::Value {
        serde_json::json!({
            "name": format!("@opencode/cli-{key}"),
            "version": VERSION,
            "dist": {
                "tarball": format!("https://registry.npmjs.org/@opencode/cli-{key}/-/cli-{key}-{VERSION}.tgz"),
                "integrity": SRI
            }
        })
    }

    #[test]
    fn resolves_all_supported_target_layouts_from_provider_metadata() {
        let jcode_keys = [
            "linux-x86_64",
            "linux-aarch64",
            "macos-aarch64",
            "macos-x86_64",
            "windows-x86_64",
            "windows-aarch64",
        ];
        for key in jcode_keys {
            let server = Server::new(vec![Reply::json(jcode_metadata(key))]);
            let release = latest_from(Provider::Jcode, key, &server.base).unwrap();
            assert_eq!(release.version, VERSION);
            assert_eq!(release.target.key, key);
            assert!(matches!(
                release.target.archive,
                Archive::TarGz { root: None }
            ));
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

        let claude_keys = [
            "darwin-arm64",
            "darwin-x64",
            "linux-x64",
            "linux-arm64",
            "linux-x64-musl",
            "linux-arm64-musl",
            "win32-x64",
            "win32-arm64",
        ];
        for key in claude_keys {
            let server = Server::new(vec![
                Reply::text(VERSION),
                Reply::json(claude_manifest(key)),
            ]);
            let release = latest_from(Provider::Claude, key, &server.base).unwrap();
            let binary = if key.starts_with("win32-") {
                "claude.exe"
            } else {
                "claude"
            };
            assert_eq!(release.version, VERSION);
            assert_eq!(release.target.entry, binary);
            assert_eq!(
                release.target.url,
                format!(
                    "https://downloads.claude.ai/claude-code-releases/{VERSION}/{key}/{binary}"
                )
            );
            assert!(matches!(release.target.archive, Archive::Raw));
            assert!(matches!(release.target.digest, Digest::Sha256(_)));
            let requests = server.finish();
            assert!(requests[0].starts_with("GET /claude-code-releases/stable "));
            assert!(requests[1].starts_with(&format!(
                "GET /claude-code-releases/{VERSION}/manifest.json "
            )));
        }

        let opencode_keys = [
            "darwin-arm64",
            "darwin-x64",
            "darwin-x64-baseline",
            "linux-x64",
            "linux-x64-baseline",
            "linux-arm64",
            "linux-x64-musl",
            "linux-x64-baseline-musl",
            "linux-arm64-musl",
            "windows-x64",
            "windows-x64-baseline",
            "windows-arm64",
        ];
        for key in opencode_keys {
            let server = Server::new(vec![Reply::json(npm_metadata(key))]);
            let release = latest_from(Provider::OpenCode, key, &server.base).unwrap();
            assert_eq!(release.version, VERSION);
            assert_eq!(
                release.target.entry,
                if key.starts_with("windows-") {
                    "package/bin/opencode.exe"
                } else {
                    "package/bin/opencode"
                }
            );
            assert_eq!(
                release.target.url,
                format!("https://registry.npmjs.org/@opencode/cli-{key}/-/cli-{key}-{VERSION}.tgz")
            );
            assert!(matches!(
                release.target.archive,
                Archive::TarGz {
                    root: Some("package")
                }
            ));
            assert!(matches!(release.target.digest, Digest::Sha512Sri(_)));
            let requests = server.finish();
            assert!(requests[0].starts_with(&format!("GET /@opencode/cli-{key}/latest ")));
        }
    }

    #[test]
    fn rejects_unstable_or_inconsistent_release_metadata() {
        let server = Server::new(vec![Reply::ok(b"{".to_vec())]);
        assert!(latest_from(Provider::Jcode, "linux-x86_64", &server.base).is_err());
        server.finish();

        let mut bad = jcode_metadata("linux-x86_64");
        bad["draft"] = true.into();
        assert_jcode_error(bad);
        let mut bad = jcode_metadata("linux-x86_64");
        bad["prerelease"] = true.into();
        assert_jcode_error(bad);
        let mut bad = jcode_metadata("linux-x86_64");
        bad["tag_name"] = "v1.2.3-rc.1".into();
        assert_jcode_error(bad);
        let mut bad = jcode_metadata("linux-x86_64");
        bad["assets"][0]["digest"] = serde_json::Value::Null;
        assert_jcode_error(bad);
        let mut bad = jcode_metadata("linux-x86_64");
        bad["assets"][0]["digest"] = "sha256:not-a-hash".into();
        assert_jcode_error(bad);
        let mut bad = jcode_metadata("linux-x86_64");
        bad["assets"][0]["browser_download_url"] = "https://evil.example/file.tgz".into();
        assert_jcode_error(bad);
        let mut bad = jcode_metadata("linux-x86_64");
        bad["assets"][0]["browser_download_url"] = format!(
            "https://github.com/1jehuang/jcode/releases/download/v9.9.9/jcode-linux-x86_64.tar.gz"
        )
        .into();
        assert_jcode_error(bad);

        let server = Server::new(vec![Reply::text("1.2.3-rc.1")]);
        assert!(latest_from(Provider::Claude, "darwin-arm64", &server.base).is_err());
        server.finish();

        for manifest in [
            serde_json::json!({"version":"9.9.9","platforms":{"darwin-arm64":{"binary":"claude","checksum":SHA256,"size":123}}}),
            serde_json::json!({"version":VERSION,"platforms":{}}),
            serde_json::json!({"version":VERSION,"platforms":{"darwin-arm64":{"binary":"../evil","checksum":SHA256,"size":123}}}),
            serde_json::json!({"version":VERSION,"platforms":{"darwin-arm64":{"binary":"claude","checksum":"bad","size":123}}}),
        ] {
            let server = Server::new(vec![Reply::text(VERSION), Reply::json(manifest)]);
            assert!(latest_from(Provider::Claude, "darwin-arm64", &server.base).is_err());
            server.finish();
        }

        for metadata in [
            serde_json::json!({"name":"@opencode/cli-linux-arm64","version":VERSION,"dist":{"tarball":"https://registry.npmjs.org/@opencode/cli-linux-x64/-/cli-linux-x64-1.2.3.tgz","integrity":SRI}}),
            serde_json::json!({"name":"@opencode/cli-linux-x64","version":"1.2.3-beta.1","dist":{"tarball":"https://registry.npmjs.org/@opencode/cli-linux-x64/-/cli-linux-x64-1.2.3-beta.1.tgz","integrity":SRI}}),
            serde_json::json!({"name":"@opencode/cli-linux-x64","version":VERSION,"dist":{"tarball":"https://evil.example/@opencode/cli-linux-x64/-/cli-linux-x64-1.2.3.tgz","integrity":SRI}}),
            serde_json::json!({"name":"@opencode/cli-linux-x64","version":VERSION,"dist":{"tarball":"https://registry.npmjs.org/@opencode/cli-linux-x64/-/cli-linux-x64-9.9.9.tgz","integrity":SRI}}),
            serde_json::json!({"name":"@opencode/cli-linux-x64","version":VERSION,"dist":{"tarball":"https://registry.npmjs.org/@opencode/cli-linux-x64/-/cli-linux-x64-1.2.3.tgz","integrity":"sha512-invalid"}}),
            serde_json::json!({"name":"@opencode/cli-linux-x64","version":VERSION,"dist":{"tarball":"https://registry.npmjs.org/@opencode/cli-linux-x64/-/cli-linux-x64-1.2.3.tgz"}}),
        ] {
            let server = Server::new(vec![Reply::json(metadata)]);
            assert!(latest_from(Provider::OpenCode, "linux-x64", &server.base).is_err());
            server.finish();
        }
    }

    fn assert_jcode_error(metadata: serde_json::Value) {
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
        let remote = Url::parse("https://downloads.claude.ai/").unwrap();
        assert!(latest_from(Provider::Claude, "darwin-arm64", &remote).is_err());
    }

    #[test]
    fn digest_validation_rejects_invalid_encodings() {
        for digest in ["".to_string(), "0".into(), "g".repeat(64), "0".repeat(63)] {
            assert!(Digest::Sha256(digest).validate().is_err());
        }
        for digest in [
            "".to_string(),
            "sha512-not-base64".into(),
            format!("sha512-{}", "A".repeat(88)),
        ] {
            assert!(Digest::Sha512Sri(digest).validate().is_err());
        }
        assert!(Digest::Sha256(SHA256.into()).validate().is_ok());
        assert!(Digest::Sha512Sri(SRI.into()).validate().is_ok());
    }
}
