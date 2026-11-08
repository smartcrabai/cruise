use crate::error::{CruiseError, Result};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::Path;
use url::Url;

pub(crate) const MAX_ARCHIVE_ENTRIES: usize = 32;
const MAX_ARCHIVE_SIZE: u64 = 128 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const COPY_BUFFER_SIZE: usize = 16 * 1024;

#[derive(Clone, Copy)]
pub(crate) enum UrlTrust<'a> {
    GitHubApi,
    GitHubDownload,
    Loopback(&'a Url),
    Hosts(&'a [&'a str]),
}

pub(crate) fn get_bounded(
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

pub(crate) fn download_archive(
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

pub(crate) fn copy_limited<R: Read, W: Write>(
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

pub(crate) fn content_length(response: &ureq::http::Response<ureq::Body>) -> Result<Option<u64>> {
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

pub(crate) fn checked_content_length(
    response: &ureq::http::Response<ureq::Body>,
    limit: u64,
) -> Result<Option<u64>> {
    let length = content_length(response)?;
    if length.is_some_and(|length| length > limit) {
        return Err(invalid("release response exceeds its size limit"));
    }
    Ok(length)
}

pub(crate) fn open_response(
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

pub(crate) fn validate_url(url: &Url, trust: UrlTrust<'_>) -> Result<()> {
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
        UrlTrust::Hosts(hosts) => {
            if url.scheme() != "https"
                || url.port_or_known_default() != Some(443)
                || !url.host_str().is_some_and(|host| hosts.contains(&host))
            {
                return Err(invalid("download URL is not allowed for this provider"));
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

pub(crate) fn normalize_archive_path(path: &str, is_dir: bool) -> Result<String> {
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

fn invalid(message: &str) -> CruiseError {
    CruiseError::Other(message.to_string())
}
pub(crate) fn hex_digest(digest: &[u8]) -> String {
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test fixture URLs are valid")]
mod tests {
    use super::*;

    #[test]
    fn provider_hosts_require_https_and_exact_host() {
        let trust = UrlTrust::Hosts(&["downloads.claude.ai"]);
        for (url, expected) in [
            ("https://downloads.claude.ai/file", true),
            ("http://downloads.claude.ai/file", false),
            ("https://downloads.claude.ai:444/file", false),
            ("https://downloads.claude.ai.evil.test/file", false),
            ("https://user@downloads.claude.ai/file", false),
        ] {
            assert_eq!(
                validate_url(&Url::parse(url).unwrap(), trust).is_ok(),
                expected,
                "{url}"
            );
        }
    }
}
