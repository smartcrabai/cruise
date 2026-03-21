use std::path::Path;

use crate::error::Result;
use crate::session::{SessionManager, SessionState};

const MAX_SESSION_TITLE_CHARS: usize = 80;

pub fn refresh_session_title_from_plan(session: &mut SessionState, plan_markdown: &str) {
    session.title = Some(derive_session_title(plan_markdown));
}

/// Recomputes a session title from the saved session state and plan file.
///
/// # Errors
///
/// Returns an error if the saved plan file cannot be read as non-empty markdown.
pub fn refresh_session_title_from_session(
    manager: &SessionManager,
    session: &mut SessionState,
) -> Result<()> {
    let plan_path = session.plan_path(&manager.sessions_dir());
    let plan_markdown = read_plan_markdown(&plan_path)?;
    refresh_session_title_from_plan(session, &plan_markdown);
    Ok(())
}

#[must_use]
pub(crate) fn derive_session_title(plan_markdown: &str) -> String {
    let candidate = first_markdown_heading(plan_markdown)
        .or_else(|| first_non_empty_plan_line(plan_markdown))
        .unwrap_or("Session");
    truncate_title(candidate, MAX_SESSION_TITLE_CHARS)
}

pub(crate) fn read_plan_markdown(path: &Path) -> Result<String> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        crate::error::CruiseError::Other(format!(
            "failed to read generated plan {}: {e}",
            path.display()
        ))
    })?;
    if content.trim().is_empty() {
        return Err(crate::error::CruiseError::Other(format!(
            "generated plan {} is empty",
            path.display()
        )));
    }
    Ok(content)
}

fn first_markdown_heading(plan_markdown: &str) -> Option<&str> {
    plan_markdown.lines().find_map(|line| {
        let trimmed = line.trim();
        if !trimmed.starts_with('#') {
            return None;
        }
        let heading = trimmed.trim_start_matches('#').trim();
        if heading.is_empty() {
            None
        } else {
            Some(heading)
        }
    })
}

fn first_non_empty_plan_line(plan_markdown: &str) -> Option<&str> {
    plan_markdown
        .lines()
        .map(strip_plan_prefix)
        .find(|line| !line.is_empty())
}

fn strip_plan_prefix(line: &str) -> &str {
    let trimmed = line.trim();
    let trimmed = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
        .unwrap_or(trimmed);
    strip_ordered_list_prefix(trimmed).unwrap_or(trimmed).trim()
}

fn strip_ordered_list_prefix(line: &str) -> Option<&str> {
    let digit_count = line.chars().take_while(char::is_ascii_digit).count();
    if digit_count == 0 {
        return None;
    }
    let rest = &line[digit_count..];
    rest.strip_prefix(". ").map(str::trim)
}

fn truncate_title(title: &str, max_chars: usize) -> String {
    let truncated: String = title.chars().take(max_chars).collect();
    truncated.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    // ── read_plan_markdown ────────────────────────────────────────────────────

    #[test]
    fn test_read_plan_markdown_returns_err_when_file_missing() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan_path = tmp.path().join("plan.md");
        assert!(
            read_plan_markdown(&plan_path).is_err(),
            "expected Err for missing file, got Ok"
        );
    }

    #[test]
    fn test_read_plan_markdown_returns_err_when_file_is_empty() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan_path = tmp.path().join("plan.md");
        std::fs::write(&plan_path, "").unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            read_plan_markdown(&plan_path).is_err(),
            "expected Err for empty file, got Ok"
        );
    }

    #[test]
    fn test_read_plan_markdown_returns_err_when_file_is_whitespace_only() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan_path = tmp.path().join("plan.md");
        std::fs::write(&plan_path, "   \n\t\n  ").unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            read_plan_markdown(&plan_path).is_err(),
            "expected Err for whitespace-only file, got Ok"
        );
    }

    #[test]
    fn test_read_plan_markdown_returns_content_when_file_has_real_content() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan_path = tmp.path().join("plan.md");
        let content = "# Implementation Plan\n\nStep 1: do something\n";
        std::fs::write(&plan_path, content).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            read_plan_markdown(&plan_path).unwrap_or_else(|e| panic!("{e:?}")),
            content
        );
    }

    fn test_session() -> SessionState {
        SessionState::new(
            "20260321130000".to_string(),
            PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "raw task input".to_string(),
        )
    }

    #[test]
    fn test_refresh_session_title_from_plan_sets_title_from_heading() {
        let mut session = test_session();
        refresh_session_title_from_plan(&mut session, "# Add session titles\n\n- Update CLI\n");
        assert_eq!(session.title.as_deref(), Some("Add session titles"));
    }

    #[test]
    fn test_refresh_session_title_from_plan_overwrites_existing_title() {
        let mut session = test_session();
        session.title = Some("Old title".to_string());
        refresh_session_title_from_plan(&mut session, "# New plan heading\n");
        assert_eq!(session.title.as_deref(), Some("New plan heading"));
    }

    #[test]
    fn test_derive_session_title_prefers_heading() {
        let title = derive_session_title(
            r"
# Add session titles

- Update CLI list
- Update GUI sidebar
",
        );

        assert_eq!(title, "Add session titles");
    }

    #[test]
    fn test_derive_session_title_strips_all_heading_hashes() {
        // H2 heading: strip_prefix('#') alone would leave "# H2 title" with a spurious #
        let title = derive_session_title("## H2 section title\n\n- step one\n");
        assert_eq!(title, "H2 section title");

        let title = derive_session_title("### H3 section title\n");
        assert_eq!(title, "H3 section title");
    }

    #[test]
    fn test_derive_session_title_falls_back_to_first_non_empty_line() {
        let title = derive_session_title(
            r"
1. Generate session titles after approval
2. Display them in the sidebar
",
        );

        assert_eq!(title, "Generate session titles after approval");
    }
}
