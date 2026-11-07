//! Helpers for live evaluation reporting.
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};

pub(super) fn output_root(repo: &Path, configured: Option<&str>) -> PathBuf {
    configured
        .filter(|value| !value.is_empty())
        .map_or_else(|| repo.join("target/prompt-eval"), |value| repo.join(value))
}

/// Summarize all trials using all-of semantics. Errors never count as passes.
pub(super) fn summary(
    counts: &BTreeMap<String, BTreeMap<String, (usize, usize, bool)>>,
) -> (String, Vec<String>) {
    let mut text = String::from(
        "# Prompt evaluation\n\n| Row | Candidate | Base | Change |\n| --- | --- | --- | --- |\n",
    );
    let mut failed = Vec::new();
    let mut regressions = Vec::new();
    let mut improvements = Vec::new();
    for (row, sides) in counts {
        let candidate = sides.get("candidate").copied().unwrap_or_default();
        let candidate_pass = candidate.1 > 0 && candidate.0 == candidate.1;
        let base = sides.get("base").copied();
        if !candidate_pass {
            failed.push(row.clone());
        }
        let change = match base {
            Some((_, _, true)) => "base error",
            Some((passed, total, false)) if total > 0 && passed == total && !candidate_pass => {
                regressions.push(row.clone());
                "regression"
            }
            Some((passed, total, false)) if total > 0 && passed < total && candidate_pass => {
                improvements.push(row.clone());
                "improvement"
            }
            Some(_) => "unchanged",
            None => "-",
        };
        let base_text = base.map_or_else(
            || "-".to_owned(),
            |(p, n, error)| {
                if error {
                    format!("{p}/{n} (error)")
                } else {
                    format!("{p}/{n}")
                }
            },
        );
        let candidate_text = if candidate.2 {
            format!("{}/{} (error)", candidate.0, candidate.1)
        } else {
            format!("{}/{}", candidate.0, candidate.1)
        };
        let _ = writeln!(
            text,
            "| {row} | {candidate_text} | {base_text} | {change} |"
        );
    }
    for (label, rows) in [("Regressions", regressions), ("Improvements", improvements)] {
        let _ = writeln!(
            text,
            "\n{label}: {}",
            if rows.is_empty() {
                "none".to_owned()
            } else {
                rows.join(", ")
            }
        );
    }
    (text, failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reports_all_of_regressions_and_improvements() {
        let counts = BTreeMap::from([
            (
                "regression".into(),
                BTreeMap::from([
                    ("candidate".into(), (2, 3, false)),
                    ("base".into(), (3, 3, false)),
                ]),
            ),
            (
                "improvement".into(),
                BTreeMap::from([
                    ("candidate".into(), (3, 3, false)),
                    ("base".into(), (2, 3, false)),
                ]),
            ),
        ]);
        let (text, failed) = summary(&counts);
        assert_eq!(failed, ["regression"]);
        assert!(text.contains("Regressions: regression"));
        assert!(text.contains("Improvements: improvement"));
    }
    #[test]
    fn base_error_is_not_reported_as_improvement() {
        let counts = BTreeMap::from([(
            "row".into(),
            BTreeMap::from([
                ("candidate".into(), (1, 1, false)),
                ("base".into(), (0, 1, true)),
            ]),
        )]);
        let (text, failed) = summary(&counts);
        assert_eq!(failed, [] as [String; 0]);
        assert!(text.contains("base error"));
        assert!(text.contains("Improvements: none"));
    }
    #[test]
    fn resolves_configured_output_root() {
        let repo = Path::new("repo");
        assert_eq!(output_root(repo, None), repo.join("target/prompt-eval"));
        assert_eq!(output_root(repo, Some("reports")), repo.join("reports"));
        assert_eq!(output_root(repo, Some("")), repo.join("target/prompt-eval"));
    }
}
