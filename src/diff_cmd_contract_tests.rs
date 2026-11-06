//! Contract tests for exact diff output, included by `diff_cmd`.

use crate::branch_diff::{BranchDiff, DiffSource};
use crate::diff_cmd::write_session_diff;

fn example_diff() -> BranchDiff {
    BranchDiff {
        branch: "session-branch".into(),
        base_commit: "0123456789abcdef0123456789abcdef01234567".into(),
        source: DiffSource::Worktree,
        stat: " file.txt | 1 +\n".into(),
        patch: "diff --git a/file.txt b/file.txt\n+line without trailing newline".into(),
        truncated: false,
    }
}

#[test]
fn writes_selected_patch_or_stat_without_modification() {
    let diff = example_diff();
    for (stat, expected) in [(false, &diff.patch), (true, &diff.stat)] {
        let mut output = Vec::new();
        write_session_diff(&mut output, &diff, stat)
            .unwrap_or_else(|error| panic!("write diff: {error}"));
        assert_eq!(output, expected.as_bytes());
    }
}

#[test]
fn empty_patch_writes_nothing() {
    let mut diff = example_diff();
    diff.patch.clear();
    let mut output = Vec::new();
    write_session_diff(&mut output, &diff, false)
        .unwrap_or_else(|error| panic!("write empty diff: {error}"));
    assert_eq!(output, [] as [u8; 0]);
}

#[test]
fn preserves_trailing_newline_exactly() {
    let mut diff = example_diff();
    for patch in ["+line\n", "+line", "+line\n\n"] {
        diff.patch = patch.into();
        let mut output = Vec::new();
        write_session_diff(&mut output, &diff, false)
            .unwrap_or_else(|error| panic!("write diff: {error}"));
        assert_eq!(output, patch.as_bytes());
    }
}
