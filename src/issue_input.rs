//! Resolve issue bindings into planning input.
use crate::{
    error::{CruiseError, Result},
    forge::{self, ForgeContext, ForgeKind},
    issue_images::{self, DownloadedImage},
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::Path,
    process::{Command, Stdio},
};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IssueBinding {
    pub number: u64,
    pub additional: String,
}
#[derive(Debug, Clone)]
pub(crate) struct IssueComment {
    pub author: String,
    pub body: String,
}
#[derive(Debug, Clone)]
pub(crate) struct IssueRecord {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub url: String,
    pub labels: Vec<String>,
    pub comments: Vec<IssueComment>,
}
#[derive(Debug, Clone)]
pub(crate) struct ResolvedTaskInput {
    pub text: String,
    pub issue_number: Option<u64>,
    pub images: Vec<DownloadedImage>,
}
pub(crate) trait GithubIssueSource {
    fn fetch_issue(&self, ctx: &ForgeContext, number: u64) -> Result<IssueRecord>;
    fn fetch_image(&self, url: &Url, token: Option<&str>) -> Result<DownloadedImage>;
    fn auth_token(&self, host: &str) -> Option<String>;
}
pub(crate) struct GhGithub;

pub(crate) fn parse_binding(input: &str) -> Result<Option<IssueBinding>> {
    let (first, rest) = input.split_once('\n').unwrap_or((input, ""));
    let first = first.trim();
    let Some(number) = first.strip_prefix('#') else {
        return Ok(None);
    };
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(None);
    }
    let n = number
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| {
            CruiseError::Other(format!("issue number must be a positive integer: {first}"))
        })?;
    Ok(Some(IssueBinding {
        number: n,
        additional: rest.trim().to_string(),
    }))
}
#[derive(Deserialize)]
struct RawUser {
    login: String,
}
#[derive(Deserialize)]
struct RawComment {
    user: Option<RawUser>,
    body: Option<String>,
}
#[derive(Deserialize)]
struct RawLabel {
    name: String,
}
#[derive(Deserialize)]
struct RawIssue {
    number: u64,
    title: String,
    body: Option<String>,
    html_url: String,
    labels: Vec<RawLabel>,
}
fn parse_comment_pages(output: &str) -> Result<Vec<IssueComment>> {
    let mut comments = Vec::new();
    for page in serde_json::Deserializer::from_str(output).into_iter::<Vec<RawComment>>() {
        comments.extend(page?.into_iter().map(|c| IssueComment {
            author: c.user.map_or_else(|| "ghost".to_string(), |u| u.login),
            body: c.body.unwrap_or_default(),
        }));
    }
    Ok(comments)
}
fn gh_api(ctx: &ForgeContext, endpoint: &str, paginate: bool) -> Result<String> {
    let mut command = Command::new("gh");
    command.arg("api");
    if ctx.host != "github.com" {
        command.args(["--hostname", &ctx.host]);
    }
    if paginate {
        command.arg("--paginate");
    }
    let output = command
        .arg(endpoint)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run gh: {e}")))?;
    if !output.status.success() {
        return Err(CruiseError::Other(format!(
            "gh api failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
impl GithubIssueSource for GhGithub {
    fn fetch_issue(&self, ctx: &ForgeContext, number: u64) -> Result<IssueRecord> {
        crate::worktree_pr::ensure_gh_available()?;
        let endpoint = format!("repos/{}/issues/{number}", ctx.repository);
        let value: serde_json::Value = serde_json::from_str(&gh_api(ctx, &endpoint, false)?)?;
        if value.get("pull_request").is_some() {
            return Err(CruiseError::Other(format!(
                "#{number} is a pull request; cruise accepts issues only"
            )));
        }
        let raw: RawIssue = serde_json::from_value(value)?;
        let comments = parse_comment_pages(&gh_api(
            ctx,
            &format!("{endpoint}/comments?per_page=100"),
            true,
        )?)?;
        Ok(IssueRecord {
            number: raw.number,
            title: raw.title,
            body: raw.body.unwrap_or_default(),
            url: raw.html_url,
            labels: raw.labels.into_iter().map(|l| l.name).collect(),
            comments,
        })
    }
    fn fetch_image(&self, url: &Url, token: Option<&str>) -> Result<DownloadedImage> {
        issue_images::fetch_github_image(url, token)
    }
    fn auth_token(&self, host: &str) -> Option<String> {
        let out = Command::new("gh")
            .args(["auth", "token", "--hostname", host])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let token = String::from_utf8(out.stdout).ok()?.trim().to_string();
        if token.is_empty() { None } else { Some(token) }
    }
}
pub(crate) fn neutralize_cruise_mentions(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(i) = rest.as_bytes().windows(7).position(|w| w.eq_ignore_ascii_case(b"@cruise")) {
        result.push_str(&rest[..i]);
        let after = &rest[i + 7..];
        result.push_str("@ cruise");
        rest = after;
    }
    result.push_str(rest);
    result
}
pub(crate) fn format_issue_task(issue: &IssueRecord, additional: &str) -> String {
    let mut sections = vec![
        format!("# {}", issue.title),
        format!("Issue #{}: {}", issue.number, issue.url),
    ];
    if !issue.body.trim().is_empty() {
        sections.push(issue.body.clone());
    }
    if !issue.labels.is_empty() {
        sections.push(format!("### Labels\n\n{}", issue.labels.join(", ")));
    }
    if !issue.comments.is_empty() {
        sections.push(format!(
            "### Comments\n\n{}",
            issue
                .comments
                .iter()
                .map(|c| format!("**{}**: {}", c.author, c.body))
                .collect::<Vec<_>>()
                .join("\n\n")
        ));
    }
    let mut task = neutralize_cruise_mentions(&sections.join("\n\n"));
    if !additional.trim().is_empty() {
        task.push_str(&format!("\n\n## Additional instructions\n\n{}", additional.trim()));
    }
    task
}
pub(crate) fn resolve_task_input(
    input: &str,
    issue: Option<u64>,
    repo: Option<&str>,
    dir: &Path,
    source: &dyn GithubIssueSource,
) -> Result<ResolvedTaskInput> {
    let parsed = parse_binding(input)?;
    if issue.is_some() && parsed.is_some() {
        return Err(CruiseError::Other(
            "pass either #N or --issue, not both".into(),
        ));
    }
    let binding = match (issue, parsed) {
        (Some(0), _) => {
            return Err(CruiseError::Other(
                "issue number must be a positive integer: #0".into(),
            ));
        }
        (Some(number), _) => IssueBinding {
            number,
            additional: input.trim().to_string(),
        },
        (None, Some(binding)) => binding,
        (None, None) => {
            return Ok(ResolvedTaskInput {
                text: input.trim().to_string(),
                issue_number: None,
                images: vec![],
            });
        }
    };
    let ctx = forge::repository_context(repo, dir)?;
    if ctx.kind != ForgeKind::GitHub {
        return Err(CruiseError::Other(
            "issue input is supported for GitHub repositories only".into(),
        ));
    }
    let mut record = source.fetch_issue(&ctx, binding.number)?;
    let token = source.auth_token(&ctx.host);
    let mut images = Vec::new();
    let mut seen: HashMap<Url, Option<usize>> = HashMap::new();
    for body in
        std::iter::once(&mut record.body).chain(record.comments.iter_mut().map(|c| &mut c.body))
    {
        let mut spans = Vec::new();
        for image in issue_images::extract_image_refs(body) {
            if !issue_images::is_github_image_url(&image.url) {
                continue;
            }
            let number = if let Some(n) = seen.get(&image.url) {
                *n
            } else {
                let n = if seen.len() >= issue_images::MAX_ISSUE_IMAGES {
                    None
                } else {
                    match source.fetch_image(&image.url, token.as_deref()) {
                        Ok(data) => {
                            images.push(data);
                            Some(images.len())
                        }
                        Err(_) => None,
                    }
                };
                if n.is_none() {
                    crate::status_eprintln!(
                        "warning: failed to download GitHub image {}{}; leaving it as text",
                        image.url.origin().ascii_serialization(),
                        image.url.path()
                    );
                }
                seen.insert(image.url, n);
                n
            };
            if let Some(n) = number {
                spans.push((image.span, format!(" [Image #{n}]")));
            }
        }
        if !spans.is_empty() {
            *body = issue_images::annotate_spans(body, &spans);
        }
    }
    Ok(ResolvedTaskInput {
        text: format_issue_task(&record, &binding.additional),
        issue_number: Some(binding.number),
        images,
    })
}
pub(crate) fn has_closing_reference(body: &str, number: u64) -> bool {
    let code_blocks: Vec<_> = pulldown_cmark::Parser::new(body)
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::CodeBlock(_)) => Some(range),
            _ => None,
        })
        .collect();
    let mut offset = 0;
    body.split_inclusive('\n').any(|line| {
        let start = offset;
        offset += line.len();
        if code_blocks.iter().any(|range| range.contains(&start)) {
            return false;
        }
        let mut words = line.split_whitespace();
        let Some(keyword) = words.next() else {
            return false;
        };
        if ![
            "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
        ]
        .iter()
        .any(|k| keyword.eq_ignore_ascii_case(k))
        {
            return false;
        }
        words
            .next()
            .and_then(|s| s.strip_prefix('#'))
            .and_then(|s| s.parse::<u64>().ok())
            == Some(number)
    })
}

#[cfg(test)]
mod closing_tests {
    use super::*;

    #[test]
    fn issue_images_deduplicate_and_only_annotate_successes() {
        struct Source(std::cell::RefCell<Vec<String>>);
        impl GithubIssueSource for Source {
            fn fetch_issue(&self, _: &ForgeContext, _: u64) -> Result<IssueRecord> {
                Ok(IssueRecord {
                    number: 12, title: "title".into(), url: "url".into(), labels: vec![],
                    body: "Plain text".into(),
                    comments: vec![IssueComment {
                        author: "user".into(),
                        body: "![ok](https://user-images.githubusercontent.com/ok.png) ![again](https://user-images.githubusercontent.com/ok.png) ![fail](https://user-images.githubusercontent.com/fail.png) ![fail again](https://user-images.githubusercontent.com/fail.png) ![other](https://example.com/other.png)".into(),
                    }],
                })
            }
            fn fetch_image(&self, url: &Url, _: Option<&str>) -> Result<DownloadedImage> {
                self.0.borrow_mut().push(url.path().to_string());
                if url.path().contains("fail") {
                    Err(CruiseError::Other("download failed".into()))
                } else {
                    Ok(DownloadedImage { extension: "png", bytes: vec![1] })
                }
            }
            fn auth_token(&self, _: &str) -> Option<String> { None }
        }
        let source = Source(std::cell::RefCell::new(Vec::new()));
        let result = resolve_task_input("#12", None, Some("https://github.com/example/project"), Path::new("."), &source).unwrap();
        assert!(result.text.contains("Plain text"));
        assert_eq!(result.text.matches(" [Image #1]").count(), 2);
        assert!(result.text.contains("![fail](https://user-images.githubusercontent.com/fail.png) ![fail again](https://user-images.githubusercontent.com/fail.png) ![other](https://example.com/other.png)"));
        assert_eq!(result.images.len(), 1);
        assert_eq!(*source.0.borrow(), vec!["/ok.png", "/fail.png"]);
    }

    #[test]
    fn repeated_mixed_case_mentions_preserve_unicode_and_substrings() {
        assert_eq!(neutralize_cruise_mentions("é@CrUiSe🙂@CRUISEx"), "é@ cruise🙂@ cruisex");
    }

    #[test]
    fn issue_mentions_are_neutralized_but_instructions_are_preserved() {
        let issue = IssueRecord {
            number: 12,
            title: "@Cruise title".into(),
            body: "@cruise body".into(),
            url: "url".into(),
            labels: vec![],
            comments: vec![IssueComment { author: "user".into(), body: "@CRUISE comment".into() }],
        };
        let task = format_issue_task(&issue, "Keep @cruise intact");
        assert!(task.contains("@ cruise title"));
        assert!(task.contains("@ cruise body"));
        assert!(task.contains("@ cruise comment"));
        assert!(task.ends_with("Keep @cruise intact"));
    }

    #[test]
    fn fenced_references_do_not_close_issues() {
        for body in ["```text\nCloses #12\n```", "~~~\nfixes #12\n~~~"] {
            assert!(!has_closing_reference(body, 12));
            assert_eq!(with_closing_reference(body, Some(12)), format!("{body}\n\nCloses #12"));
        }
        let body = "```\nCloses #12\n```\n\nFixes #12";
        assert_eq!(with_closing_reference(body, Some(12)), body);
    }

    #[test]
    fn indented_reference_does_not_close_issue() {
        let body = "    Closes #12";
        assert!(!has_closing_reference(body, 12));
        assert_eq!(with_closing_reference(body, Some(12)), "    Closes #12\n\nCloses #12");
    }
}
pub(crate) fn with_closing_reference(body: &str, issue_number: Option<u64>) -> String {
    let Some(n) = issue_number else {
        return body.to_string();
    };
    if has_closing_reference(body, n) {
        return body.to_string();
    }
    let trimmed = body.trim_end();
    if trimmed.is_empty() {
        format!("Closes #{n}")
    } else {
        format!("{trimmed}\n\nCloses #{n}")
    }
}
