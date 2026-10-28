use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

use crate::error::{CruiseError, Result};

pub(super) const BUILD_TARGET: &str = env!("CRUISE_BUILD_TARGET");

const API_RELEASE_URL: &str = "https://api.github.com/repos/smartcrabai/cruise/releases/latest";
const DOWNLOAD_ROOT: &str = "https://github.com/smartcrabai/cruise/releases/download/";
const MAX_API_SIZE: u64 = 1024 * 1024;
const MAX_MANIFEST_SIZE: u64 = 2 * 1024 * 1024;
const MAX_ARCHIVE_SIZE: u64 = 128 * 1024 * 1024;
const MAX_EXPANDED_SIZE: u64 = 512 * 1024 * 1024;
const MAX_XZ_MEMORY: u64 = 128 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 32;
const MAX_REDIRECTS: usize = 5;
const COPY_BUFFER_SIZE: usize = 16 * 1024;
const TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
];

pub(super) struct ReleaseRuntime {
    pub version: String,
    pub target: String,
    pub artifact: String,
    pub archive_sha256: String,
    pub runtime: PathBuf,
    pub runtime_sha256: String,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    prerelease: bool,
    draft: bool,
}

#[derive(Deserialize)]
struct DistManifest {
    announcement_tag: String,
    announcement_is_prerelease: bool,
    releases: Vec<ManifestRelease>,
    artifacts: std::collections::BTreeMap<String, ManifestArtifact>,
}

#[derive(Deserialize)]
struct ManifestRelease {
    app_name: String,
    app_version: String,
    artifacts: Vec<String>,
}

#[derive(Deserialize)]
struct ManifestArtifact {
    name: String,
    kind: String,
    #[serde(default)]
    target_triples: Vec<String>,
    assets: Option<Vec<ManifestAsset>>,
    checksum: Option<String>,
    checksums: Option<ManifestChecksums>,
}

#[derive(Deserialize)]
struct ManifestAsset {
    name: String,
    path: String,
    kind: String,
    id: Option<String>,
}

#[derive(Deserialize)]
struct ManifestChecksums {
    sha256: String,
}

struct ServiceUrls {
    api_release: Url,
    download_root: Url,
    loopback: Option<Url>,
}

#[derive(Clone, Copy)]
enum UrlTrust<'a> {
    GitHubApi,
    GitHubDownload,
    Loopback(&'a Url),
}

pub(super) fn acquire(
    current_version: &str,
    target: &str,
    staging: &Path,
) -> Result<Option<ReleaseRuntime>> {
    if !TARGETS.contains(&target) || target != BUILD_TARGET {
        return Err(CruiseError::Other(format!(
            "native update target {target:?} does not match this build target {BUILD_TARGET:?}"
        )));
    }
    validate_staging_dir(staging)?;
    let current = parse_stable_version(current_version)?;
    let services = service_urls()?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(180)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_per_read(Some(Duration::from_secs(30)))
        .timeout_per_write(Some(Duration::from_secs(30)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    let (tag, version, manifest) = latest_release_manifest(&agent, &services)?;
    let Some(selected) = select_update_archive(&manifest, &tag, &version, &current, target)? else {
        return Ok(None);
    };
    download_selected_runtime(&agent, &services, &tag, &version, target, staging, selected)
        .map(Some)
}

struct SelectedArchive {
    name: String,
    sha256: String,
}

fn latest_release_manifest(
    agent: &ureq::Agent,
    services: &ServiceUrls,
) -> Result<(String, Version, DistManifest)> {
    let api_trust = services
        .loopback
        .as_ref()
        .map_or(UrlTrust::GitHubApi, UrlTrust::Loopback);
    let release_bytes = get_bounded(
        agent,
        &services.api_release,
        api_trust,
        "application/vnd.github+json",
        MAX_API_SIZE,
    )?;
    let release: GitHubRelease = serde_json::from_slice(&release_bytes)?;
    if release.draft || release.prerelease {
        return Err(CruiseError::Other(
            "GitHub latest release is not a stable published release".to_string(),
        ));
    }
    let version_text = release
        .tag_name
        .strip_prefix('v')
        .ok_or_else(|| invalid("release tag must use a v-prefixed SemVer version"))?;
    let version = parse_stable_version(version_text)?;
    if release.tag_name != format!("v{version}") {
        return Err(invalid("release tag is not canonical SemVer"));
    }
    let manifest_url = release_url(
        &services.download_root,
        &release.tag_name,
        "dist-manifest.json",
    );
    let download_trust = services
        .loopback
        .as_ref()
        .map_or(UrlTrust::GitHubDownload, UrlTrust::Loopback);
    let manifest_bytes = get_bounded(
        agent,
        &manifest_url,
        download_trust,
        "application/json",
        MAX_MANIFEST_SIZE,
    )?;
    let manifest: DistManifest = serde_json::from_slice(&manifest_bytes)?;
    Ok((release.tag_name, version, manifest))
}

fn select_update_archive(
    manifest: &DistManifest,
    tag: &str,
    version: &Version,
    current: &Version,
    target: &str,
) -> Result<Option<SelectedArchive>> {
    let release_info = unique_app_release(manifest)?;
    if manifest.announcement_tag != tag
        || manifest.announcement_is_prerelease
        || release_info.app_version != version.to_string()
    {
        return Err(invalid(
            "release API tag and dist-manifest release metadata do not agree",
        ));
    }
    if !is_newer(version, current) {
        return Ok(None);
    }

    let archive_name = archive_name_for_target(target)?;
    let artifact = select_archive(manifest, release_info, target, &archive_name)?;
    // SHA-256 from the same HTTPS release detects corruption, not a compromised publisher.
    let archive_sha256 = parse_sha256(
        artifact
            .checksums
            .as_ref()
            .ok_or_else(|| invalid("release archive is missing its SHA256 checksum"))?
            .sha256
            .as_str(),
    )?;
    let checksum_name = format!("{archive_name}.sha256");
    let checksum_artifact = manifest
        .artifacts
        .get(&checksum_name)
        .ok_or_else(|| invalid("release manifest has no matching SHA256 artifact"))?;
    if artifact.checksum.as_deref() != Some(checksum_name.as_str())
        || checksum_artifact.name != checksum_name
        || checksum_artifact.kind != "checksum"
        || !checksum_artifact
            .target_triples
            .iter()
            .any(|candidate| candidate == target)
        || !release_info
            .artifacts
            .iter()
            .any(|item| item == &checksum_name)
    {
        return Err(invalid("release manifest has no matching SHA256 artifact"));
    }
    let runtime_asset = runtime_asset(artifact, target)?;
    if runtime_asset.path != runtime_filename(target) {
        return Err(invalid("manifest runtime path does not match its target"));
    }
    Ok(Some(SelectedArchive {
        name: archive_name,
        sha256: archive_sha256,
    }))
}

fn download_selected_runtime(
    agent: &ureq::Agent,
    services: &ServiceUrls,
    tag: &str,
    version: &Version,
    target: &str,
    staging: &Path,
    selected: SelectedArchive,
) -> Result<ReleaseRuntime> {
    let download_trust = services
        .loopback
        .as_ref()
        .map_or(UrlTrust::GitHubDownload, UrlTrust::Loopback);
    let archive_path = staging.join(".cruise-update-archive");
    let runtime_path = staging.join(runtime_filename(target));
    let mut archive_created = false;
    let result = (|| {
        let (downloaded_size, downloaded_sha256) = download_archive(
            agent,
            &release_url(&services.download_root, tag, &selected.name),
            download_trust,
            &archive_path,
            &mut archive_created,
        )?;
        if downloaded_size == 0 {
            return Err(invalid("downloaded release archive is empty"));
        }
        verify_archive_checksum(&downloaded_sha256, &selected.sha256)?;
        let runtime_sha256 = extract_runtime(
            &archive_path,
            &selected.name,
            target,
            runtime_filename(target),
            &runtime_path,
        )?;
        Ok(ReleaseRuntime {
            version: version.to_string(),
            target: target.to_string(),
            artifact: selected.name.clone(),
            archive_sha256: selected.sha256,
            runtime: runtime_path,
            runtime_sha256,
        })
    })();
    if archive_created {
        let _ = fs::remove_file(&archive_path);
    }
    result
}

fn service_urls() -> Result<ServiceUrls> {
    #[cfg(all(feature = "test-utils", debug_assertions))]
    if let Some(base) = std::env::var_os("CRUISE_UPDATE_TEST_BASE_URL") {
        let base = Url::parse(
            base.to_str()
                .ok_or_else(|| invalid("test release base URL is not valid UTF-8"))?,
        )
        .map_err(|error| invalid(&format!("invalid test release base URL: {error}")))?;
        validate_loopback_base(&base)?;
        let mut api_release = base.clone();
        api_release.set_path("/repos/smartcrabai/cruise/releases/latest");
        let mut download_root = base.clone();
        download_root.set_path("/smartcrabai/cruise/releases/download/");
        return Ok(ServiceUrls {
            api_release,
            download_root,
            loopback: Some(base),
        });
    }

    Ok(ServiceUrls {
        api_release: Url::parse(API_RELEASE_URL)
            .map_err(|error| invalid(&format!("invalid built-in release API URL: {error}")))?,
        download_root: Url::parse(DOWNLOAD_ROOT)
            .map_err(|error| invalid(&format!("invalid built-in release download URL: {error}")))?,
        loopback: None,
    })
}

#[cfg(any(test, all(feature = "test-utils", debug_assertions)))]
fn validate_loopback_base(base: &Url) -> Result<()> {
    let loopback_host = base.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if base.scheme() != "http"
        || !loopback_host
        || base.port().is_none()
        || base.path() != "/"
        || base.query().is_some()
        || base.fragment().is_some()
        || !base.username().is_empty()
        || base.password().is_some()
    {
        return Err(invalid(
            "test release base URL must be an HTTP loopback root with an explicit port",
        ));
    }
    Ok(())
}

fn validate_staging_dir(staging: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(staging)?;
    if !metadata.file_type().is_dir() {
        return Err(invalid("release staging path is not a real directory"));
    }
    if fs::read_dir(staging)?.next().transpose()?.is_some() {
        return Err(invalid("release staging directory is not empty"));
    }
    Ok(())
}

fn parse_stable_version(value: &str) -> Result<Version> {
    let version = Version::parse(value).map_err(|error| {
        invalid(&format!(
            "invalid stable release version {value:?}: {error}"
        ))
    })?;
    if !version.pre.is_empty() {
        return Err(invalid(
            "prerelease versions cannot be used for native updates",
        ));
    }
    Ok(version)
}

fn is_newer(candidate: &Version, current: &Version) -> bool {
    candidate.cmp_precedence(current).is_gt()
}

fn unique_app_release(manifest: &DistManifest) -> Result<&ManifestRelease> {
    let mut releases = manifest
        .releases
        .iter()
        .filter(|release| release.app_name == "cruise");
    let release = releases
        .next()
        .ok_or_else(|| invalid("dist-manifest has no cruise release"))?;
    if releases.next().is_some() {
        return Err(invalid("dist-manifest has multiple cruise releases"));
    }
    Ok(release)
}

fn archive_name_for_target(target: &str) -> Result<String> {
    if !TARGETS.contains(&target) {
        return Err(invalid("unsupported native update target"));
    }
    let extension = if target == "x86_64-pc-windows-msvc" {
        "zip"
    } else {
        "tar.xz"
    };
    Ok(format!("cruise-{target}.{extension}"))
}

fn select_archive<'a>(
    manifest: &'a DistManifest,
    release: &ManifestRelease,
    target: &str,
    expected_name: &str,
) -> Result<&'a ManifestArtifact> {
    let mut matches = manifest.artifacts.iter().filter(|(_, artifact)| {
        artifact.kind == "executable-zip"
            && artifact
                .target_triples
                .iter()
                .any(|candidate| candidate == target)
    });
    let (name, artifact) = matches
        .next()
        .ok_or_else(|| invalid("dist-manifest has no matching target archive"))?;
    if matches.next().is_some()
        || name.as_str() != expected_name
        || artifact.name != expected_name
        || !release.artifacts.iter().any(|item| item == name)
    {
        return Err(invalid(
            "dist-manifest target archive is ambiguous or unpublished",
        ));
    }
    Ok(artifact)
}

fn runtime_asset<'a>(artifact: &'a ManifestArtifact, target: &str) -> Result<&'a ManifestAsset> {
    let assets = artifact
        .assets
        .as_ref()
        .ok_or_else(|| invalid("release archive has no manifest asset list"))?;
    let id = format!("cruise-{target}-exe-cruise-runtime");
    let mut matches = assets.iter().filter(|asset| {
        asset.kind == "executable"
            && asset.name == "cruise-runtime"
            && asset.id.as_deref() == Some(id.as_str())
    });
    let asset = matches
        .next()
        .ok_or_else(|| invalid("release archive has no cruise-runtime executable"))?;
    if matches.next().is_some() {
        return Err(invalid(
            "release archive has duplicate cruise-runtime assets",
        ));
    }
    Ok(asset)
}

fn runtime_filename(target: &str) -> &'static str {
    if target == "x86_64-pc-windows-msvc" {
        "cruise-runtime.exe"
    } else {
        "cruise-runtime"
    }
}

fn release_url(root: &Url, tag: &str, asset: &str) -> Url {
    let mut url = root.clone();
    url.set_path(&format!(
        "/smartcrabai/cruise/releases/download/{tag}/{asset}"
    ));
    url
}

fn parse_sha256(value: &str) -> Result<String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid(
            "release SHA256 must contain exactly 64 hexadecimal digits",
        ));
    }
    Ok(value.to_ascii_lowercase())
}

fn get_bounded(
    agent: &ureq::Agent,
    url: &Url,
    trust: UrlTrust<'_>,
    accept: &str,
    limit: u64,
) -> Result<Vec<u8>> {
    let response = open_response(agent, url, trust, accept)?;
    let expected_length = checked_content_length(&response, limit)?;
    let mut bytes = Vec::new();
    let size = copy_limited(response.into_body().into_reader(), &mut bytes, limit, None)?;
    if expected_length.is_some_and(|expected| expected != size) {
        return Err(invalid("release response ended before its Content-Length"));
    }
    Ok(bytes)
}

fn download_archive(
    agent: &ureq::Agent,
    url: &Url,
    trust: UrlTrust<'_>,
    path: &Path,
    created: &mut bool,
) -> Result<(u64, String)> {
    let response = open_response(agent, url, trust, "application/octet-stream")?;
    let expected_length = checked_content_length(&response, MAX_ARCHIVE_SIZE)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    *created = true;
    let mut digest = Sha256::new();
    let size = copy_limited(
        response.into_body().into_reader(),
        &mut file,
        MAX_ARCHIVE_SIZE,
        Some(&mut digest),
    )?;
    let sha256 = hex_digest(&digest.finalize());
    if expected_length.is_some_and(|expected| expected != size) {
        return Err(invalid("release archive ended before its Content-Length"));
    }
    file.flush()?;
    Ok((size, sha256))
}

fn verify_archive_checksum(actual: &str, expected: &str) -> Result<()> {
    if actual != expected {
        return Err(invalid(
            "downloaded release archive failed its SHA256 check",
        ));
    }
    Ok(())
}

fn copy_limited<R: Read, W: Write>(
    mut input: R,
    output: &mut W,
    limit: u64,
    mut digest: Option<&mut Sha256>,
) -> Result<u64> {
    let mut buffer = [0; COPY_BUFFER_SIZE];
    let mut size = 0_u64;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .filter(|size| *size <= limit)
            .ok_or_else(|| invalid("release response exceeded its size limit"))?;
        output.write_all(&buffer[..count])?;
        if let Some(digest) = digest.as_mut() {
            digest.update(&buffer[..count]);
        }
    }
    Ok(size)
}

fn content_length(response: &ureq::http::Response<ureq::Body>) -> Result<Option<u64>> {
    response
        .headers()
        .get(ureq::http::header::CONTENT_LENGTH)
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| invalid("release response has an invalid Content-Length"))
        })
        .transpose()
}

fn checked_content_length(
    response: &ureq::http::Response<ureq::Body>,
    limit: u64,
) -> Result<Option<u64>> {
    let length = content_length(response)?;
    if length.is_some_and(|length| length > limit) {
        return Err(invalid("release response exceeds its size limit"));
    }
    Ok(length)
}

fn open_response(
    agent: &ureq::Agent,
    url: &Url,
    trust: UrlTrust<'_>,
    accept: &str,
) -> Result<ureq::http::Response<ureq::Body>> {
    let mut current = url.clone();
    for redirects in 0..=MAX_REDIRECTS {
        validate_url(&current, trust)?;
        let response = agent
            .get(current.as_str())
            .header("Accept", accept)
            .header("User-Agent", "cruise-native-updater")
            .call()
            .map_err(|error| invalid(&format!("release request failed: {error}")))?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            if redirects == MAX_REDIRECTS {
                return Err(invalid("release server exceeded the redirect limit"));
            }
            let location = response
                .headers()
                .get(ureq::http::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| invalid("release redirect has no Location header"))?;
            current = current
                .join(location)
                .map_err(|error| invalid(&format!("invalid release redirect URL: {error}")))?;
            validate_url(&current, trust)?;
            continue;
        }
        if status != 200 {
            return Err(invalid(&format!("release server returned HTTP {status}")));
        }
        return Ok(response);
    }
    Err(invalid("release server exceeded the redirect limit"))
}

fn validate_url(url: &Url, trust: UrlTrust<'_>) -> Result<()> {
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(invalid(
            "release URL contains forbidden credentials or fragment",
        ));
    }
    match trust {
        UrlTrust::GitHubApi => {
            if url.scheme() != "https"
                || url.host_str() != Some("api.github.com")
                || url.port_or_known_default() != Some(443)
            {
                return Err(invalid(
                    "release API URL is not an official HTTPS GitHub URL",
                ));
            }
        }
        UrlTrust::GitHubDownload => {
            let official_host = matches!(
                url.host_str(),
                Some(
                    "github.com"
                        | "release-assets.githubusercontent.com"
                        | "objects.githubusercontent.com"
                        | "github-cloud.s3.amazonaws.com"
                )
            );
            if url.scheme() != "https" || !official_host || url.port_or_known_default() != Some(443)
            {
                return Err(invalid(
                    "release download URL is not an official HTTPS GitHub URL",
                ));
            }
        }
        UrlTrust::Loopback(base) => {
            if url.origin() != base.origin() {
                return Err(invalid(
                    "test release redirect left the configured loopback server",
                ));
            }
        }
    }
    Ok(())
}

fn extract_runtime(
    archive_path: &Path,
    archive_name: &str,
    target: &str,
    runtime_name: &str,
    destination: &Path,
) -> Result<String> {
    let mut output_created = false;
    let result = if archive_name.ends_with(".tar.xz") {
        extract_tar_xz(
            archive_path,
            target,
            runtime_name,
            destination,
            &mut output_created,
        )
    } else if Path::new(archive_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        extract_zip(
            archive_path,
            target,
            runtime_name,
            destination,
            &mut output_created,
        )
    } else {
        Err(invalid("unsupported release archive format"))
    };
    if result.is_err() && output_created {
        let _ = fs::remove_file(destination);
    }
    result
}

fn extract_tar_xz(
    archive_path: &Path,
    target: &str,
    runtime_name: &str,
    destination: &Path,
    output_created: &mut bool,
) -> Result<String> {
    let file = File::open(archive_path)?;
    let decoder = xz_decoder(file, MAX_XZ_MEMORY)?;
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| invalid(&format!("invalid release tar archive: {error}")))?
        .raw(true);
    let root = format!("cruise-{target}");
    let mut seen = HashSet::new();
    let mut total_size = 0_u64;
    let mut count = 0_usize;
    let mut root_seen = false;
    let mut runtime_hash = None;
    for entry in entries {
        let mut entry =
            entry.map_err(|error| invalid(&format!("invalid release tar entry: {error}")))?;
        count += 1;
        if count > MAX_ARCHIVE_ENTRIES {
            return Err(invalid("release archive contains too many entries"));
        }
        let entry_type = entry.header().entry_type();
        let is_dir = entry_type.is_dir();
        if !is_dir && !entry_type.is_file() {
            return Err(invalid("release archive contains a link or special file"));
        }
        let raw_path = entry.path_bytes();
        let raw_path = std::str::from_utf8(raw_path.as_ref())
            .map_err(|_| invalid("release archive path is not UTF-8"))?;
        let normalized = normalize_archive_path(raw_path, is_dir)?;
        let mut components = normalized.split('/');
        let archive_root = components.next().unwrap_or("");
        let filename = components.next();
        if archive_root != root || components.next().is_some() {
            return Err(invalid("release archive contains an unexpected path"));
        }
        if !seen.insert(normalized.clone()) {
            return Err(invalid("release archive contains duplicate entries"));
        }
        let entry_size = entry
            .header()
            .size()
            .map_err(|error| invalid(&format!("invalid release tar entry size: {error}")))?;
        total_size = total_size
            .checked_add(entry_size)
            .filter(|size| *size <= MAX_EXPANDED_SIZE)
            .ok_or_else(|| invalid("release archive expands beyond its size limit"))?;
        if is_dir {
            if filename.is_some() || normalized != root {
                return Err(invalid("release archive contains an unexpected directory"));
            }
            root_seen = true;
            continue;
        }
        let Some(filename) = filename else {
            return Err(invalid("release archive contains a file outside its root"));
        };
        if !allowed_archive_file(filename, runtime_name) {
            return Err(invalid("release archive contains an unexpected file"));
        }
        if filename == runtime_name {
            runtime_hash = Some(write_runtime(
                &mut entry,
                entry_size,
                destination,
                output_created,
            )?);
        }
    }
    if !root_seen {
        return Err(invalid(
            "release archive is missing its top-level directory",
        ));
    }
    runtime_hash.ok_or_else(|| invalid("release archive is missing cruise-runtime"))
}
fn xz_decoder<R: Read>(reader: R, memory_limit: u64) -> Result<xz2::read::XzDecoder<R>> {
    let stream = xz2::stream::Stream::new_stream_decoder(memory_limit, 0)
        .map_err(|error| invalid(&format!("cannot initialize bounded XZ decoder: {error}")))?;
    Ok(xz2::read::XzDecoder::new_stream(reader, stream))
}

fn extract_zip(
    archive_path: &Path,
    target: &str,
    runtime_name: &str,
    destination: &Path,
    output_created: &mut bool,
) -> Result<String> {
    if target != "x86_64-pc-windows-msvc" {
        return Err(invalid("ZIP release archive has a non-Windows target"));
    }
    let file = File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| invalid(&format!("invalid release ZIP archive: {error}")))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(invalid("release archive contains too many entries"));
    }
    let mut seen = HashSet::new();
    let mut total_size = 0_u64;
    let mut runtime_hash = None;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| invalid(&format!("invalid release ZIP entry: {error}")))?;
        if entry.is_dir() {
            return Err(invalid(
                "Windows release ZIP must contain root-level files only",
            ));
        }
        validate_zip_entry_type(&entry)?;
        if entry.encrypted()
            || !matches!(
                entry.compression(),
                zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
            )
        {
            return Err(invalid(
                "release ZIP uses encryption or unsupported compression",
            ));
        }
        let normalized = normalize_archive_path(entry.name(), false)?;
        if normalized.contains('/') || !seen.insert(normalized.clone()) {
            return Err(invalid("release ZIP contains a nested or duplicate path"));
        }
        let filename = normalized.as_str();
        let entry_size = entry.size();
        total_size = total_size
            .checked_add(entry_size)
            .filter(|size| *size <= MAX_EXPANDED_SIZE)
            .ok_or_else(|| invalid("release archive expands beyond its size limit"))?;
        if !allowed_archive_file(filename, runtime_name) {
            return Err(invalid("release archive contains an unexpected file"));
        }
        if filename == runtime_name {
            runtime_hash = Some(write_runtime(
                &mut entry,
                entry_size,
                destination,
                output_created,
            )?);
        }
    }
    runtime_hash.ok_or_else(|| invalid("release archive is missing cruise-runtime"))
}

fn normalize_archive_path(path: &str, is_dir: bool) -> Result<String> {
    if path.len() > 1024
        || path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains('\0')
    {
        return Err(invalid("release archive contains an unsafe path"));
    }
    let path = if is_dir {
        path.strip_suffix('/').unwrap_or(path)
    } else {
        path
    };
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(invalid("release archive contains an unsafe path"));
    }
    Ok(path.to_string())
}

fn allowed_archive_file(filename: &str, runtime_name: &str) -> bool {
    matches!(filename, "LICENSE" | "README.md" | "cruise" | "cruise.exe")
        || filename == runtime_name
}

fn validate_zip_entry_type<R: Read>(entry: &zip::read::ZipFile<'_, R>) -> Result<()> {
    if entry.is_symlink() || !entry.is_file() {
        return Err(invalid("release ZIP contains a link or special file"));
    }
    if let Some(mode) = entry.unix_mode() {
        let file_type = mode & 0o170_000;
        if file_type != 0 && file_type != 0o100_000 {
            return Err(invalid("release ZIP contains a link or special file"));
        }
    }
    Ok(())
}

fn write_runtime<R: Read>(
    input: &mut R,
    expected_size: u64,
    destination: &Path,
    output_created: &mut bool,
) -> Result<String> {
    if expected_size == 0 || expected_size > MAX_EXPANDED_SIZE {
        return Err(invalid("release runtime has an invalid size"));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    *output_created = true;
    let mut digest = Sha256::new();
    let mut buffer = [0; COPY_BUFFER_SIZE];
    let mut remaining = expected_size;
    while remaining != 0 {
        let wanted = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| invalid("release runtime has an invalid size"))?;
        let count = input.read(&mut buffer[..wanted])?;
        if count == 0 {
            return Err(invalid("release runtime ended before its declared size"));
        }
        file.write_all(&buffer[..count])?;
        digest.update(&buffer[..count]);
        remaining -= count as u64;
    }
    let mut extra = [0; 1];
    if input.read(&mut extra)? != 0 {
        return Err(invalid("release runtime exceeds its declared size"));
    }
    file.flush()?;
    set_runtime_permissions(&file)?;
    Ok(hex_digest(&digest.finalize()))
}

#[cfg(unix)]
fn set_runtime_permissions(file: &File) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_runtime_permissions(_file: &File) -> Result<()> {
    Ok(())
}

fn invalid(message: &str) -> CruiseError {
    CruiseError::Other(message.to_string())
}

fn hex_digest(digest: &[u8]) -> String {
    let mut output = String::with_capacity(digest.len() * 2);
    for &byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn write_zip_file(path: &Path, name: &str, options: zip::write::SimpleFileOptions) {
        let file = std::fs::File::create(path).unwrap_or_else(|error| panic!("{error}"));
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file(name, options)
            .unwrap_or_else(|error| panic!("{error}"));
        archive
            .write_all(b"runtime bytes")
            .unwrap_or_else(|error| panic!("{error}"));
        archive.finish().unwrap_or_else(|error| panic!("{error}"));
    }

    fn write_zip_symlink(path: &Path, name: &str, target: &str) {
        let file = std::fs::File::create(path).unwrap_or_else(|error| panic!("{error}"));
        let mut archive = zip::ZipWriter::new(file);
        archive
            .add_symlink(name, target, zip::write::SimpleFileOptions::default())
            .unwrap_or_else(|error| panic!("{error}"));
        archive.finish().unwrap_or_else(|error| panic!("{error}"));
    }

    fn write_tar_xz_file(path: &Path, target: &str) {
        let file = std::fs::File::create(path).unwrap_or_else(|error| panic!("{error}"));
        let encoder = xz2::write::XzEncoder::new(file, 6);
        let mut archive = tar::Builder::new(encoder);
        let root = format!("cruise-{target}");
        let root_directory = format!("{root}/");
        let mut root_header = tar::Header::new_gnu();
        root_header.set_entry_type(tar::EntryType::Directory);
        root_header.set_size(0);
        root_header.set_mode(0o755);
        root_header.set_cksum();
        archive
            .append_data(&mut root_header, root_directory.as_str(), std::io::empty())
            .unwrap_or_else(|error| panic!("{error}"));
        let mut runtime_header = tar::Header::new_gnu();
        runtime_header.set_size(b"runtime bytes".len() as u64);
        runtime_header.set_mode(0o755);
        runtime_header.set_cksum();
        archive
            .append_data(
                &mut runtime_header,
                format!("{root}/cruise-runtime").as_str(),
                &b"runtime bytes"[..],
            )
            .unwrap_or_else(|error| panic!("{error}"));
        archive.finish().unwrap_or_else(|error| panic!("{error}"));
        archive
            .into_inner()
            .unwrap_or_else(|error| panic!("{error}"))
            .finish()
            .unwrap_or_else(|error| panic!("{error}"));
    }

    fn write_tar_xz_with_pax_runtime(path: &Path, target: &str) {
        let file = std::fs::File::create(path).unwrap_or_else(|error| panic!("{error}"));
        let encoder = xz2::write::XzEncoder::new(file, 6);
        let mut archive = tar::Builder::new(encoder);
        let root = format!("cruise-{target}");
        let mut root_header = tar::Header::new_gnu();
        root_header.set_entry_type(tar::EntryType::Directory);
        root_header.set_size(0);
        root_header.set_mode(0o755);
        root_header.set_cksum();
        archive
            .append_data(&mut root_header, format!("{root}/"), std::io::empty())
            .unwrap_or_else(|error| panic!("{error}"));
        let runtime_path = format!("{root}/cruise-runtime");
        archive
            .append_pax_extensions([("path", runtime_path.as_bytes())])
            .unwrap_or_else(|error| panic!("{error}"));
        let mut runtime_header = tar::Header::new_gnu();
        runtime_header.set_size(b"runtime bytes".len() as u64);
        runtime_header.set_mode(0o755);
        runtime_header.set_cksum();
        archive
            .append_data(&mut runtime_header, "ignored-path", &b"runtime bytes"[..])
            .unwrap_or_else(|error| panic!("{error}"));
        archive.finish().unwrap_or_else(|error| panic!("{error}"));
        archive
            .into_inner()
            .unwrap_or_else(|error| panic!("{error}"))
            .finish()
            .unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn parses_cargo_dist_manifest_release_schema() {
        let mut manifest: DistManifest = serde_json::from_str(
            r#"{
                "announcement_tag":"v0.2.17",
                "announcement_is_prerelease":false,
                "releases":[{"app_name":"cruise","app_version":"0.2.17","artifacts":["cruise-x86_64-apple-darwin.tar.xz","cruise-x86_64-apple-darwin.tar.xz.sha256"]}],
                "artifacts":{
                    "cruise-x86_64-apple-darwin.tar.xz":{
                        "name":"cruise-x86_64-apple-darwin.tar.xz",
                        "kind":"executable-zip",
                        "target_triples":["x86_64-apple-darwin"],
                        "assets":[{"id":"cruise-x86_64-apple-darwin-exe-cruise-runtime","name":"cruise-runtime","path":"cruise-runtime","kind":"executable"}],
                        "checksum":"cruise-x86_64-apple-darwin.tar.xz.sha256",
                        "checksums":{"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
                    },
                    "cruise-x86_64-apple-darwin.tar.xz.sha256":{"name":"cruise-x86_64-apple-darwin.tar.xz.sha256","kind":"checksum","target_triples":["x86_64-apple-darwin"]},
                    "sha256.sum":{"name":"sha256.sum","kind":"unified-checksum"},
                    "source.tar.gz":{"name":"source.tar.gz","kind":"source-tarball","checksum":"source.tar.gz.sha256","checksums":{"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}},
                    "source.tar.gz.sha256":{"name":"source.tar.gz.sha256","kind":"checksum"},
                    "cruise.rb":{"name":"cruise.rb","kind":"installer","target_triples":["aarch64-apple-darwin","aarch64-unknown-linux-gnu","x86_64-apple-darwin","x86_64-pc-windows-gnu","x86_64-unknown-linux-gnu"]}
                }
            }"#,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            manifest.artifacts["sha256.sum"].target_triples,
            Vec::<String>::new()
        );
        assert_eq!(
            manifest.artifacts["source.tar.gz"].target_triples,
            Vec::<String>::new()
        );
        let archive_name = "cruise-x86_64-apple-darwin.tar.xz";
        let target_triples = std::mem::take(
            &mut manifest
                .artifacts
                .get_mut(archive_name)
                .unwrap_or_else(|| panic!("missing archive artifact"))
                .target_triples,
        );
        let release = unique_app_release(&manifest).unwrap_or_else(|error| panic!("{error}"));
        assert!(select_archive(&manifest, release, "x86_64-apple-darwin", archive_name).is_err());
        manifest
            .artifacts
            .get_mut(archive_name)
            .unwrap_or_else(|| panic!("missing archive artifact"))
            .target_triples = target_triples;
        let release = unique_app_release(&manifest).unwrap_or_else(|error| panic!("{error}"));
        let artifact = select_archive(
            &manifest,
            release,
            "x86_64-apple-darwin",
            "cruise-x86_64-apple-darwin.tar.xz",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            runtime_asset(artifact, "x86_64-apple-darwin")
                .unwrap_or_else(|error| panic!("{error}"))
                .path,
            "cruise-runtime"
        );
    }

    #[test]
    fn stable_version_comparison_rejects_equal_older_and_prerelease_versions() {
        let current = parse_stable_version("0.2.16").unwrap_or_else(|error| panic!("{error}"));
        assert!(!is_newer(
            &parse_stable_version("0.2.15").unwrap_or_else(|error| panic!("{error}")),
            &current
        ));
        assert!(!is_newer(
            &parse_stable_version("0.2.16").unwrap_or_else(|error| panic!("{error}")),
            &current
        ));
        assert!(is_newer(
            &parse_stable_version("0.2.17").unwrap_or_else(|error| panic!("{error}")),
            &current
        ));
        assert!(!is_newer(
            &parse_stable_version("0.2.16+new-build").unwrap_or_else(|error| panic!("{error}")),
            &parse_stable_version("0.2.16+old-build").unwrap_or_else(|error| panic!("{error}"))
        ));
        assert!(parse_stable_version("0.2.17-rc.1").is_err());
        assert_eq!(TARGETS.len(), 5);
        for target in TARGETS {
            assert!(archive_name_for_target(target).is_ok());
        }
        assert!(archive_name_for_target("unknown-target").is_err());
    }

    #[test]
    fn checksum_and_download_size_boundaries_are_enforced() {
        let checksum = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(
            parse_sha256(checksum).unwrap_or_else(|error| panic!("{error}")),
            checksum
        );
        assert!(parse_sha256("short").is_err());
        assert!(parse_sha256(&"g".repeat(64)).is_err());
        assert!(verify_archive_checksum(checksum, checksum).is_ok());
        assert!(verify_archive_checksum("different", checksum).is_err());
        let mut digest = Sha256::new();
        assert_eq!(
            copy_limited(Cursor::new(b"abc"), &mut Vec::new(), 3, Some(&mut digest))
                .unwrap_or_else(|error| panic!("{error}")),
            3
        );
        assert_eq!(
            hex_digest(&digest.finalize()),
            hex_digest(&Sha256::digest(b"abc"))
        );
        assert!(copy_limited(Cursor::new(b"abcd"), &mut Vec::new(), 3, None).is_err());
    }

    #[test]
    fn archive_path_validation_rejects_traversal_and_unsafe_forms() {
        assert_eq!(
            normalize_archive_path("cruise-x86_64-apple-darwin/cruise-runtime", false)
                .unwrap_or_else(|error| panic!("{error}")),
            "cruise-x86_64-apple-darwin/cruise-runtime"
        );
        for path in [
            "../cruise-runtime",
            "cruise-root/../cruise-runtime",
            "/cruise-root/cruise-runtime",
            "cruise-root\\\\cruise-runtime",
            "cruise-root//cruise-runtime",
            "cruise-root/./cruise-runtime",
        ] {
            assert!(normalize_archive_path(path, false).is_err(), "{path}");
        }
        assert!(allowed_archive_file("cruise-runtime", "cruise-runtime"));
        assert!(!allowed_archive_file("evil-link", "cruise-runtime"));
    }

    #[test]
    fn windows_zip_extracts_root_runtime_and_rejects_traversal_and_symlink() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let valid_archive = directory.path().join("valid.zip");
        write_zip_file(
            &valid_archive,
            "cruise-runtime.exe",
            zip::write::SimpleFileOptions::default(),
        );
        let runtime = directory.path().join("cruise-runtime.exe");
        let runtime_hash = extract_runtime(
            &valid_archive,
            "cruise-x86_64-pc-windows-msvc.zip",
            "x86_64-pc-windows-msvc",
            "cruise-runtime.exe",
            &runtime,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            fs::read(&runtime).unwrap_or_else(|error| panic!("{error}")),
            b"runtime bytes"
        );
        assert_eq!(runtime_hash, hex_digest(&Sha256::digest(b"runtime bytes")));

        let traversal_archive = directory.path().join("traversal.zip");
        write_zip_file(
            &traversal_archive,
            "../cruise-runtime.exe",
            zip::write::SimpleFileOptions::default(),
        );
        let traversal_output = directory.path().join("traversal-runtime.exe");
        assert!(
            extract_runtime(
                &traversal_archive,
                "cruise-x86_64-pc-windows-msvc.zip",
                "x86_64-pc-windows-msvc",
                "cruise-runtime.exe",
                &traversal_output
            )
            .is_err()
        );
        assert!(!traversal_output.exists());

        let symlink_archive = directory.path().join("symlink.zip");
        write_zip_symlink(&symlink_archive, "cruise-runtime.exe", "../target");
        let symlink_output = directory.path().join("symlink-runtime.exe");
        assert!(
            extract_runtime(
                &symlink_archive,
                "cruise-x86_64-pc-windows-msvc.zip",
                "x86_64-pc-windows-msvc",
                "cruise-runtime.exe",
                &symlink_output
            )
            .is_err()
        );
        assert!(!symlink_output.exists());
    }

    #[test]
    fn acquire_requires_an_existing_empty_staging_directory() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        assert!(validate_staging_dir(directory.path()).is_ok());
        fs::write(directory.path().join("unexpected"), b"keep")
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(validate_staging_dir(directory.path()).is_err());
    }

    #[test]
    fn macos_tar_xz_extracts_only_runtime_from_target_root() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let archive = directory.path().join("release.tar.xz");
        write_tar_xz_file(&archive, "aarch64-apple-darwin");
        let runtime = directory.path().join("cruise-runtime");
        let runtime_hash = extract_runtime(
            &archive,
            "cruise-aarch64-apple-darwin.tar.xz",
            "aarch64-apple-darwin",
            "cruise-runtime",
            &runtime,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            fs::read(&runtime).unwrap_or_else(|error| panic!("{error}")),
            b"runtime bytes"
        );
        assert_eq!(runtime_hash, hex_digest(&Sha256::digest(b"runtime bytes")));
    }

    #[test]
    fn xz_decoder_rejects_dictionary_over_its_memory_limit() {
        let mut encoder = xz2::write::XzEncoder::new(Vec::new(), 6);
        encoder
            .write_all(b"bounded xz decoder fixture")
            .unwrap_or_else(|error| panic!("{error}"));
        let encoded_stream = encoder.finish().unwrap_or_else(|error| panic!("{error}"));
        let mut decoder = xz_decoder(Cursor::new(encoded_stream), 1024 * 1024)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(decoder.read_to_end(&mut Vec::new()).is_err());
    }

    #[test]
    fn raw_tar_rejects_pax_path_override_before_runtime_extraction() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let archive = directory.path().join("pax.tar.xz");
        write_tar_xz_with_pax_runtime(&archive, "aarch64-apple-darwin");
        let runtime = directory.path().join("cruise-runtime");
        assert!(
            extract_runtime(
                &archive,
                "cruise-aarch64-apple-darwin.tar.xz",
                "aarch64-apple-darwin",
                "cruise-runtime",
                &runtime
            )
            .is_err()
        );
        assert!(!runtime.exists());
    }

    #[test]
    fn loopback_test_base_requires_local_http_origin() {
        let local = Url::parse("http://127.0.0.1:8123/").unwrap_or_else(|error| panic!("{error}"));
        validate_loopback_base(&local).unwrap_or_else(|error| panic!("{error}"));
        let remote =
            Url::parse("http://example.com:8123/").unwrap_or_else(|error| panic!("{error}"));
        assert!(validate_loopback_base(&remote).is_err());
        let path =
            Url::parse("http://127.0.0.1:8123/nested/").unwrap_or_else(|error| panic!("{error}"));
        assert!(validate_loopback_base(&path).is_err());
    }
}
