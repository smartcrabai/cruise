//! Extraction and safe retrieval of GitHub issue images.
use crate::error::{CruiseError, Result};
use pulldown_cmark::{Event, LinkType, Parser, Tag};
use std::ops::Range;
use std::time::Duration;
use url::Url;

pub(crate) const MAX_ISSUE_IMAGES: usize = 20;
pub(crate) const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
#[derive(Debug, Clone)]
pub(crate) struct DownloadedImage {
    pub extension: &'static str,
    pub bytes: Vec<u8>,
}
#[derive(Debug, Clone)]
pub(crate) struct ImageRef {
    pub url: Url,
    pub span: Range<usize>,
}

fn safe_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
}
pub(crate) fn is_github_image_url(url: &Url) -> bool {
    if !safe_url(url) {
        return false;
    }
    match url.host_str() {
        Some("github.com") => {
            let parts: Vec<_> = url
                .path()
                .strip_prefix('/')
                .unwrap_or("")
                .split('/')
                .collect();
            parts.iter().all(|p| !p.is_empty())
                && ((parts.len() >= 3 && parts[0] == "user-attachments" && parts[1] == "assets")
                    || (parts.len() >= 5 && parts[2] == "assets"))
        }
        Some("user-images.githubusercontent.com" | "private-user-images.githubusercontent.com") => {
            url.path() != "/" && !url.path().is_empty()
        }
        _ => false,
    }
}
pub(crate) fn is_allowed_redirect(url: &Url) -> bool {
    if !safe_url(url) {
        return false;
    }
    if is_github_image_url(url) {
        return true;
    }
    let host = url.host_str().unwrap_or("");
    host == "github-cloud.s3.amazonaws.com"
        || (host.ends_with(".amazonaws.com")
            && host.contains(".s3")
            && [
                "github-production-user-asset-",
                "github-production-repository-file-",
                "github-production-release-asset-",
            ]
            .iter()
            .any(|p| host.starts_with(p)))
}
pub(crate) fn authorization_for(
    original: &Url,
    current: &Url,
    token: Option<&str>,
) -> Option<String> {
    if original.host_str() == Some("github.com") && original.origin() == current.origin() {
        token.map(|t| format!("token {t}"))
    } else {
        None
    }
}
pub(crate) fn validate_image(content_type: &str, bytes: &[u8]) -> Result<&'static str> {
    let kind = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let ext = match kind.as_str() {
        "image/png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => Some("png"),
        "image/jpeg" if bytes.starts_with(b"\xff\xd8\xff") => Some("jpg"),
        "image/gif" if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") => Some("gif"),
        "image/webp" if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" => {
            Some("webp")
        }
        _ => None,
    };
    ext.ok_or_else(|| CruiseError::Other("unsupported or invalid image".into()))
}
pub(crate) fn fetch_github_image(url: &Url, token: Option<&str>) -> Result<DownloadedImage> {
    if !is_github_image_url(url) {
        return Err(CruiseError::Other("unsupported image URL".into()));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    let mut current = url.clone();
    for hop in 0..=5 {
        let mut request = agent.get(current.as_str());
        if let Some(auth) = authorization_for(url, &current, token) {
            request = request.header("Authorization", &auth);
        }
        let mut response = request
            .call()
            .map_err(|e| CruiseError::Other(format!("image request failed: {e}")))?;
        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            if hop == 5 {
                return Err(CruiseError::Other("too many redirects".into()));
            }
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| CruiseError::Other("missing redirect location".into()))?;
            let next = current
                .join(location)
                .map_err(|e| CruiseError::Other(format!("invalid redirect: {e}")))?;
            if !is_allowed_redirect(&next) {
                return Err(CruiseError::Other("disallowed redirect".into()));
            }
            current = next;
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(CruiseError::Other(format!("HTTP {status}")));
        }
        if response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|n| n > MAX_IMAGE_BYTES as u64)
        {
            return Err(CruiseError::Other("image too large".into()));
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let bytes = response
            .body_mut()
            .with_config()
            .limit(MAX_IMAGE_BYTES as u64)
            .read_to_vec()
            .map_err(|e| CruiseError::Other(format!("image read failed: {e}")))?;
        let extension = validate_image(&content_type, &bytes)?;
        return Ok(DownloadedImage { extension, bytes });
    }
    Err(CruiseError::Other("too many redirects".into()))
}

fn html_images(html: &str, offset: usize, refs: &mut Vec<ImageRef>) {
    let lower = html.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(rel) = lower[pos..].find("<img") {
        let start = pos + rel;
        let after = start + 4;
        if !lower
            .as_bytes()
            .get(after)
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b'>' || *b == b'/')
        {
            pos = after;
            continue;
        }
        let mut i = after;
        let bytes = html.as_bytes();
        let mut src = None;
        while i < bytes.len() && bytes[i] != b'>' {
            while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
                i += 1;
            }
            let name = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'-' | b'_' | b':'))
            {
                i += 1;
            }
            if i == name {
                if i < bytes.len() && bytes[i] != b'>' {
                    i += 1;
                }
                continue;
            }
            let is_src = html[name..i].eq_ignore_ascii_case("src");
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if bytes.get(i) != Some(&b'=') {
                continue;
            }
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let quote = bytes.get(i).copied().filter(|b| *b == b'\'' || *b == b'"');
            if quote.is_some() {
                i += 1;
            }
            let value = i;
            while i < bytes.len()
                && match quote {
                    Some(q) => bytes[i] != q,
                    None => !bytes[i].is_ascii_whitespace() && bytes[i] != b'>',
                }
            {
                i += 1;
            }
            if is_src {
                src = Some(html[value..i].replace("&amp;", "&"));
            }
            if quote.is_some() && i < bytes.len() {
                i += 1;
            }
        }
        if i >= bytes.len() {
            break;
        }
        pos = i + 1;
        if let Some(url) = src.and_then(|s| Url::parse(&s).ok()) {
            refs.push(ImageRef {
                url,
                span: offset + start..offset + pos,
            });
        }
    }
}
pub(crate) fn extract_image_refs(body: &str) -> Vec<ImageRef> {
    let mut refs = Vec::new();
    for (event, span) in Parser::new(body).into_offset_iter() {
        match event {
            Event::Start(Tag::Image {
                link_type: LinkType::Inline,
                dest_url,
                ..
            }) => {
                if let Ok(url) = Url::parse(&dest_url) {
                    refs.push(ImageRef { url, span });
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                html_images(&html, span.start, &mut refs);
            }
            _ => {}
        }
    }
    refs.sort_by_key(|r| r.span.start);
    refs
}
pub(crate) fn annotate_spans(body: &str, spans: &[(Range<usize>, String)]) -> String {
    let mut output = body.to_owned();
    let mut sorted: Vec<_> = spans.iter().collect();
    sorted.sort_by_key(|(span, _)| std::cmp::Reverse(span.end));
    for (span, annotation) in sorted {
        output.insert_str(span.end, annotation);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotations_keep_descending_and_equal_offset_order() {
        let spans = vec![(0..1, "A".into()), (1..2, "B".into()), (0..1, "C".into())];
        assert_eq!(annotate_spans("xy", &spans), "xCAyB");
    }

    #[test]
    fn image_hosts_and_download_boundary() {
        for url in [
            "https://github.com/user-attachments/assets/abc",
            "https://user-images.githubusercontent.com/a.png",
        ] {
            assert!(is_github_image_url(
                &Url::parse(url).unwrap_or_else(|error| panic!("invalid test URL: {error}"))
            ));
        }
        for url in [
            "https://example.com/a.png",
            "http://github.com/user-attachments/assets/a",
            "https://github.com/user-attachments/assets/",
        ] {
            let url = Url::parse(url).unwrap_or_else(|error| panic!("invalid test URL: {error}"));
            assert!(!is_github_image_url(&url));
            assert!(fetch_github_image(&url, None).is_err());
        }
    }
}
