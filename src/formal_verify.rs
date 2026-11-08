pub mod alloy;
mod process;
pub mod quint;
mod tools;
use crate::{
    cancellation::CancellationToken,
    error::{CruiseError, Result},
};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Quint,
    Alloy,
}
impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Quint => "quint",
                Self::Alloy => "alloy",
            }
        )
    }
}
#[derive(Clone, Debug)]
pub struct SpecBlock {
    pub language: Language,
    pub source: String,
    pub number: usize,
}
#[derive(Clone, Debug, Default)]
pub struct ExtractedBlocks {
    pub blocks: Vec<SpecBlock>,
    pub over_limit: Vec<Language>,
}
// CommonMark's parser accepts unclosed fences. Check the source range for an explicit closing fence.
#[must_use]
pub fn extract_blocks(input: &str) -> ExtractedBlocks {
    // pulldown-cmark does not treat a lone CR as a line ending. Normalize it
    // without changing byte offsets, while leaving CRLF pairs intact.
    let normalized;
    let input = if input
        .as_bytes()
        .iter()
        .enumerate()
        .any(|(i, &byte)| byte == b'\r' && input.as_bytes().get(i + 1) != Some(&b'\n'))
    {
        let mut chars = input.chars().peekable();
        normalized = std::iter::from_fn(|| {
            chars.next().map(|ch| {
                if ch == '\r' && chars.peek() != Some(&'\n') {
                    '\n'
                } else {
                    ch
                }
            })
        })
        .collect::<String>();
        normalized.as_str()
    } else {
        input
    };
    let mut result = ExtractedBlocks::default();
    // The parser's block range starts at the opening fence, even in containers.
    // Track the end of emitted text separately because an implicit end at a
    // container boundary or EOF must not be mistaken for a closing fence.
    let mut current: Option<(Language, u8, usize, usize, String)> = None;
    let mut counts = [0usize; 2];
    for (event, range) in Parser::new(input).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                let language = match info.as_ref() {
                    "quint" => Some(Language::Quint),
                    "alloy" => Some(Language::Alloy),
                    _ => None,
                };
                if let Some(lang) = language {
                    let bytes = input.as_bytes();
                    let fence = bytes[range.start];
                    let n = bytes[range.start..]
                        .iter()
                        .take_while(|&&byte| byte == fence)
                        .count();
                    let content_start = bytes[range.start..range.end]
                        .iter()
                        .position(|&byte| byte == b'\r' || byte == b'\n')
                        .map_or(range.end, |offset| {
                            let end = range.start + offset + 1;
                            if bytes[end - 1] == b'\r' && bytes.get(end) == Some(&b'\n') {
                                end + 1
                            } else {
                                end
                            }
                        });
                    current = Some((lang, fence, n, content_start.min(range.end), String::new()));
                }
            }
            Event::Text(text) => {
                if let Some((_, _, _, content_end, source)) = &mut current {
                    source.push_str(&text);
                    *content_end = (*content_end).max(range.end);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((language, fence, n, content_end, source)) = current.take() {
                    // Only a closing fence is outside the parser's text events.
                    // Container prefixes may precede it, so examine its suffix.
                    let tail = &input.as_bytes()[content_end.min(range.end)..range.end];
                    let end = tail.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
                    let closed = tail[..end]
                        .iter()
                        .rev()
                        .take_while(|&&b| b == fence)
                        .count()
                        >= n;
                    if closed {
                        let idx = usize::from(language != Language::Quint);
                        counts[idx] += 1;
                        if counts[idx] <= 4 {
                            result.blocks.push(SpecBlock {
                                language,
                                source,
                                number: counts[idx],
                            });
                        } else {
                            result.over_limit.push(language);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    result
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Passed,
    Failed,
    Error,
}
impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Passed => "passed",
                Self::Failed => "failed",
                Self::Error => "error",
            }
        )
    }
}
#[must_use]
pub fn aggregate(verdicts: &[Verdict]) -> Verdict {
    if verdicts.contains(&Verdict::Failed) {
        Verdict::Failed
    } else if verdicts.is_empty() || verdicts.contains(&Verdict::Error) {
        Verdict::Error
    } else {
        Verdict::Passed
    }
}
#[derive(Clone, Debug)]
pub struct StageReport {
    pub name: String,
    pub verdict: Verdict,
    pub message: String,
    pub timed_out: bool,
    pub log: String,
}
#[derive(Clone, Debug)]
pub struct BlockReport {
    pub language: Language,
    pub number: usize,
    pub stages: Vec<StageReport>,
    pub artifact: PathBuf,
}
impl BlockReport {
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        aggregate(&self.stages.iter().map(|s| s.verdict).collect::<Vec<_>>())
    }
    pub fn error(&mut self, name: &str, msg: impl Into<String>) {
        self.stages.push(StageReport {
            name: name.into(),
            verdict: Verdict::Error,
            message: msg.into(),
            timed_out: false,
            log: String::new(),
        });
    }
}
#[derive(Clone, Debug)]
pub struct VerifyReport {
    pub blocks: Vec<BlockReport>,
    pub artifact: PathBuf,
    pub warnings: Vec<String>,
}
impl VerifyReport {
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        aggregate(
            &self
                .blocks
                .iter()
                .map(BlockReport::verdict)
                .collect::<Vec<_>>(),
        )
    }
    #[must_use]
    pub fn render_markdown(&self) -> String {
        let mut s = format!(
            "## Formal verification: {}\n\nArtifacts: `{}`\n",
            self.verdict(),
            self.artifact.display()
        );
        for warning in &self.warnings {
            let _ = writeln!(s, "\nWarning: {warning}");
        }
        if self.blocks.is_empty() {
            s.push_str("\nNo quint/alloy blocks were found, so nothing was verified.\n");
        }
        for b in &self.blocks {
            let _ = writeln!(
                s,
                "\n### {} block {}: {}\nArtifacts: `{}`",
                b.language,
                b.number,
                b.verdict(),
                b.artifact.display()
            );
            for stage in &b.stages {
                let _ = writeln!(
                    s,
                    "\n- {}: {} (timedOut: {}) {}",
                    stage.name, stage.verdict, stage.timed_out, stage.message
                );
                if !stage.log.is_empty() {
                    let _ = writeln!(s, "\n```text\n{}\n```", stage.log.replace("```", "'''"));
                }
            }
        }
        s
    }
}
/// Verify extracted specifications and save their report in the session directory.
///
/// # Errors
/// Returns an error if artifact creation fails or verification is interrupted.
pub fn verify_blocks(
    blocks: ExtractedBlocks,
    session_dir: &Path,
    timeout: Duration,
    cancel: Option<&CancellationToken>,
) -> Result<VerifyReport> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let dir = session_dir
        .join("formal")
        .join(format!("{}-{}", timestamp, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;
    let mut report = VerifyReport {
        blocks: Vec::new(),
        artifact: dir.clone(),
        warnings: Vec::new(),
    };
    for block in blocks.blocks {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(CruiseError::Interrupted);
        }
        let path = dir.join(format!("{}-{}", block.language, block.number));
        std::fs::create_dir_all(&path)?;
        let b = match block.language {
            Language::Quint => quint::verify(&block, &path, timeout, cancel, &mut report.warnings),
            Language::Alloy => alloy::verify(&block, &path, timeout, cancel),
        };
        report.blocks.push(b);
    }
    let mut excess_counts = [4usize; 2];
    for language in blocks.over_limit {
        let index = usize::from(language != Language::Quint);
        excess_counts[index] += 1;
        let mut b = BlockReport {
            language,
            number: excess_counts[index],
            stages: vec![],
            artifact: dir.clone(),
        };
        b.error(
            "limit",
            "Not verified: maximum of four blocks per language exceeded",
        );
        report.blocks.push(b);
    }
    let markdown = report.render_markdown();
    std::fs::write(dir.join("report.md"), markdown)?;
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        return Err(CruiseError::Interrupted);
    }
    Ok(report)
}
