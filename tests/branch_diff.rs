//! Branch diff contract tests. Each fixture uses a real, isolated git repository.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use cruise::{
    branch_diff::{
        DiffSource, MAX_PATCH_BYTES, UNTRACKED_FILE_LIMIT, session_base_commit, session_branch_diff,
    },
    session::SessionState,
    session_config::SessionConfigRef,
};
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

struct Repo {
    _tmp: TempDir,
    base: PathBuf,
    wt: PathBuf,
    state: SessionState,
}
impl Repo {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().join("base");
        let wt = tmp.path().join("worktree");
        fs::create_dir(&base).unwrap();
        git(&base, &["init", "-b", "main"]);
        git(&base, &["config", "user.email", "test@example.com"]);
        git(&base, &["config", "user.name", "Test User"]);
        fs::write(base.join("README.md"), "initial\n").unwrap();
        git(&base, &["add", "README.md"]);
        git(&base, &["commit", "-m", "initial"]);
        git(
            &base,
            &["worktree", "add", "-b", "session", wt.to_str().unwrap()],
        );
        let mut state = SessionState::new(
            "test-session".into(),
            base.clone(),
            SessionConfigRef::File {
                path: base.join("test.yaml"),
            },
            String::new(),
        );
        state.worktree_branch = Some("session".into());
        state.worktree_path = Some(wt.clone());
        Self {
            _tmp: tmp,
            base,
            wt,
            state,
        }
    }
    fn commit(&self, path: &str, contents: &str) {
        fs::write(self.wt.join(path), contents).unwrap();
        git(&self.wt, &["add", path]);
        git(&self.wt, &["commit", "-m", "session change"]);
    }
}

#[test]
fn t1_no_branch_is_unavailable() {
    let mut r = Repo::new();
    r.state.worktree_branch = None;
    assert!(
        session_branch_diff(&r.state, false)
            .unwrap_err()
            .to_string()
            .contains("has no worktree branch")
    );
}

#[test]
fn t2_deleted_branch_is_reported() {
    let mut r = Repo::new();
    git(&r.base, &["worktree", "remove", r.wt.to_str().unwrap()]);
    git(&r.base, &["branch", "-D", "session"]);
    r.state.worktree_path = None;
    assert!(
        session_branch_diff(&r.state, false)
            .unwrap_err()
            .to_string()
            .contains("no longer exists")
    );
}

#[test]
fn t3_committed_only_includes_patch_stat_and_full_merge_base() {
    let mut r = Repo::new();
    let base = git(&r.base, &["rev-parse", "HEAD"]);
    r.commit("a.txt", "hello\n");
    r.state.worktree_path = None;
    let diff = session_branch_diff(&r.state, false).unwrap();
    assert_eq!(diff.source, DiffSource::CommittedOnly);
    assert_eq!(diff.base_commit, base);
    assert!(diff.patch.contains("+++ b/a.txt"), "{}", diff.patch);
    assert!(diff.stat.contains("a.txt | 1 +"), "{}", diff.stat);
    assert!(!diff.truncated);
}

#[test]
fn t4_worktree_includes_tracked_and_untracked_changes() {
    let r = Repo::new();
    fs::write(r.wt.join("README.md"), "changed\n").unwrap();
    fs::write(r.wt.join("new.txt"), "new\n").unwrap();
    let diff = session_branch_diff(&r.state, false).unwrap();
    assert_eq!(diff.source, DiffSource::Worktree);
    for expected in [
        "+changed",
        "diff --git a/new.txt b/new.txt",
        "new file mode",
        "+++ b/new.txt",
    ] {
        assert!(
            diff.patch.contains(expected),
            "missing {expected}: {}",
            diff.patch
        );
    }
    assert!(diff.stat.contains(" new.txt | untracked"), "{}", diff.stat);
}

#[test]
fn t5_base_advancement_does_not_appear_in_diff() {
    let r = Repo::new();
    r.commit("session.txt", "session\n");
    fs::write(r.base.join("base-only.txt"), "base\n").unwrap();
    git(&r.base, &["add", "base-only.txt"]);
    git(&r.base, &["commit", "-m", "advance base"]);
    let diff = session_branch_diff(&r.state, false).unwrap();
    assert!(diff.patch.contains("session.txt"));
    assert!(!diff.patch.contains("base-only.txt"));
}

#[test]
fn t6_external_diff_and_textconv_are_never_executed() {
    let r = Repo::new();
    fs::write(r.wt.join(".gitattributes"), "*.txt diff=evil\n").unwrap();
    fs::write(r.wt.join("evil.txt"), "before\n").unwrap();
    git(&r.wt, &["add", ".gitattributes", "evil.txt"]);
    git(&r.wt, &["commit", "-m", "attributes"]);
    fs::write(r.wt.join("evil.txt"), "after\n").unwrap();
    let marker = r._tmp.path().join("MARKER");
    let command = format!("touch {}", marker.display());
    git(&r.base, &["config", "diff.evil.textconv", &command]);
    git(&r.base, &["config", "diff.evil.command", &command]);
    let diff = session_branch_diff(&r.state, false).unwrap();
    assert!(diff.patch.contains("evil.txt"));
    assert!(!marker.exists());
}

#[test]
fn t7_clean_branch_has_empty_diff() {
    let r = Repo::new();
    let diff = session_branch_diff(&r.state, false).unwrap();
    assert_eq!(diff.patch, "");
    assert_eq!(diff.stat, "");
    assert!(!diff.truncated);
}

#[test]
fn t8_large_untracked_patch_is_bounded_at_newline() {
    let r = Repo::new();
    fs::write(
        r.wt.join("large.txt"),
        ("a".repeat(127) + "\n").repeat(16384),
    )
    .unwrap();
    let diff = session_branch_diff(&r.state, false).unwrap();
    assert!(diff.truncated);
    assert!(diff.patch.len() <= MAX_PATCH_BYTES);
    assert!(diff.patch.ends_with('\n'));
}

#[test]
fn large_tracked_patch_is_bounded_and_git_finishes() {
    let r = Repo::new();
    fs::write(r.wt.join("README.md"), ("large line\n").repeat(600_000)).unwrap();
    let diff = session_branch_diff(&r.state, false).unwrap();
    assert!(diff.truncated);
    assert!(diff.patch.len() <= MAX_PATCH_BYTES);
    assert!(diff.patch.ends_with('\n'));
    assert!(diff.patch.contains("+large line"));
    assert!(session_branch_diff(&r.state, false).is_ok());
}

#[cfg(unix)]
#[test]
fn untracked_non_utf8_path_is_passed_to_git_unchanged() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    let r = Repo::new();
    fs::write(r.wt.join(OsStr::from_bytes(b"invalid-\xff.txt")), "native path content\n").unwrap();
    let diff = session_branch_diff(&r.state, false).unwrap();
    assert!(diff.patch.contains("+native path content"));
    assert!(!diff.truncated);
}

#[test]
fn t9_untracked_count_is_bounded() {
    let r = Repo::new();
    for n in 0..=UNTRACKED_FILE_LIMIT {
        fs::write(r.wt.join(format!("file-{n:04}.txt")), "hello\n").unwrap();
    }
    let diff = session_branch_diff(&r.state, false).unwrap();
    assert!(diff.truncated);
    assert_eq!(
        diff.patch.matches("diff --git ").count(),
        UNTRACKED_FILE_LIMIT
    );
}

#[test]
fn t10_color_is_opt_in() {
    let r = Repo::new();
    fs::write(r.wt.join("README.md"), "changed\n").unwrap();
    assert!(
        session_branch_diff(&r.state, true)
            .unwrap()
            .patch
            .contains("\u{1b}[")
    );
    assert!(
        !session_branch_diff(&r.state, false)
            .unwrap()
            .patch
            .contains("\u{1b}[")
    );
}

#[test]
fn t11_missing_worktree_falls_back_to_committed_only() {
    let mut r = Repo::new();
    r.commit("a.txt", "hello\n");
    r.state.worktree_path = Some(r._tmp.path().join("missing"));
    let diff = session_branch_diff(&r.state, false).unwrap();
    assert_eq!(diff.source, DiffSource::CommittedOnly);
    assert!(diff.patch.contains("+++ b/a.txt"));
}

#[test]
fn t12_worktreeinclude_files_and_directories_are_excluded() {
    let r = Repo::new();
    fs::write(r.base.join(".worktreeinclude"), "secret.env\nvendor/\nREADME.md\n").unwrap();
    fs::write(r.wt.join("README.md"), "tracked change\n").unwrap();
    fs::write(r.wt.join("secret.env"), "secret\n").unwrap();
    fs::create_dir(r.wt.join("vendor")).unwrap();
    fs::write(r.wt.join("vendor/copied.txt"), "copied\n").unwrap();
    fs::write(r.wt.join("visible.txt"), "visible\n").unwrap();
    let diff = session_branch_diff(&r.state, false).unwrap();
    assert!(diff.patch.contains("+tracked change"), "{}", diff.patch);
    assert!(diff.stat.contains("README.md"), "{}", diff.stat);
    assert!(diff.patch.contains("visible.txt"));
    assert!(!diff.patch.contains("secret.env"));
    assert!(!diff.patch.contains("vendor/"));
    assert!(!diff.stat.contains("secret.env"));
    assert!(!diff.stat.contains("vendor/"));
}

#[test]
fn t13_non_repository_reports_git_error_not_unborn_head() {
    let mut r = Repo::new();
    r.state.base_dir = r._tmp.path().join("not-a-repository");
    fs::create_dir(&r.state.base_dir).unwrap();
    let error = session_branch_diff(&r.state, false)
        .unwrap_err()
        .to_string();
    assert!(!error.contains("has no HEAD commit"), "{error}");
    assert!(
        error.contains("git") || error.contains("failed to read"),
        "{error}"
    );
}

#[test]
fn t14a_unborn_base_has_no_head_commit() {
    let mut r = Repo::new();
    let unborn = r._tmp.path().join("unborn");
    fs::create_dir(&unborn).unwrap();
    git(&unborn, &["init", "-b", "main"]);
    r.state.base_dir = unborn;
    let error = session_base_commit(&r.state).unwrap_err().to_string();
    assert!(error.contains("has no HEAD commit"), "{error}");
}

#[test]
#[ignore = "requires the sibling local-merge feature for explicit merge-target precedence"]
fn t14b_explicit_merge_target_takes_precedence_over_head() {
    let mut r = Repo::new();
    // Keep HEAD on an older diverged branch while main advances to the session commit.
    r.commit("a.txt", "hello\n");
    git(&r.base, &["checkout", "-b", "other"]);
    fs::write(r.base.join("other.txt"), "other\n").unwrap();
    git(&r.base, &["add", "other.txt"]);
    git(&r.base, &["commit", "-m", "other"]);
    git(&r.base, &["branch", "-f", "main", "session"]);
    // The merge-target field is supplied by the sibling local-merge change. Using
    // serde keeps this contract test buildable before that field is introduced.
    let mut value = serde_json::to_value(&r.state).unwrap();
    value["forked_from_branch"] = serde_json::json!("main");
    r.state = serde_json::from_value(value).unwrap();
    let expected = git(&r.base, &["merge-base", "main", "refs/heads/session"]);
    let actual = session_base_commit(&r.state).unwrap();
    assert_eq!(actual, expected);
    assert_ne!(actual, git(&r.base, &["rev-parse", "HEAD"]));
}
