//! Host-side preparation of advisor artifacts.

use std::path::Path;

use crate::{
    artifacts, branch_diff,
    error::{CruiseError, Result},
};

const DIFF_CAP: usize = 256 * 1024;
const TRUNCATION_NOTICE: &str = "[Diff truncated]\n";

/// Invalidate advice at the start of an advisor visit.
pub(crate) fn invalidate(artifacts_root: Option<&Path>, output_file: &str) -> Result<()> {
    if let Some(root) = artifacts_root {
        artifacts::remove(root, output_file)?;
    }
    Ok(())
}

/// Prepare the new review input.
pub(crate) fn prepare(
    artifacts_root: Option<&Path>,
    output_file: &str,
    working_dir: Option<&Path>,
) -> Result<String> {
    let root = artifacts_root.ok_or_else(|| {
        CruiseError::Other("cannot prepare advisor diff without a session artifacts root".into())
    })?;
    let working_dir = working_dir.unwrap_or(Path::new("."));
    let name = format!("{output_file}.diff");
    let diff = branch_diff::worktree_diff(working_dir, "HEAD", DIFF_CAP)?;
    let mut text = diff.stat;
    text.push_str(&diff.patch);
    let truncated = branch_diff::truncate_at_line(&mut text, DIFF_CAP) || diff.truncated;
    if truncated {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(TRUNCATION_NOTICE);
    }
    artifacts::write(root, &name, &text)?;
    Ok(format!(
        "Prepared advisor diff '{name}' ({} bytes, truncated: {truncated})",
        text.len()
    ))
}
