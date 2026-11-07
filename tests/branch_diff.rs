//! Branch diff contract tests. Fixtures use isolated git repositories.
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

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()?;
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

struct Repo {
    tmp: TempDir,
    base: PathBuf,
    wt: PathBuf,
    state: SessionState,
}
impl Repo {
    fn new() -> Result<Self> {
        let tmp = TempDir::new()?;
        let base = tmp.path().join("base");
        let wt = tmp.path().join("worktree");
        fs::create_dir(&base)?;
        git(&base, &["init", "-b", "main"])?;
        git(&base, &["config", "user.email", "test@example.com"])?;
        git(&base, &["config", "user.name", "Test User"])?;
        fs::write(base.join("README.md"), "initial\n")?;
        git(&base, &["add", "README.md"])?;
        git(&base, &["commit", "-m", "initial"])?;
        git(
            &base,
            &[
                "worktree",
                "add",
                "-b",
                "session",
                wt.to_str().ok_or("non-UTF-8 worktree path")?,
            ],
        )?;
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
        Ok(Self {
            tmp,
            base,
            wt,
            state,
        })
    }
    fn commit(&self, path: &str, contents: &str) -> Result<()> {
        fs::write(self.wt.join(path), contents)?;
        git(&self.wt, &["add", path])?;
        git(&self.wt, &["commit", "-m", "session change"])?;
        Ok(())
    }
}

#[test]
fn t1_no_branch_is_unavailable() -> Result<()> {
    let mut r = Repo::new()?;
    r.state.worktree_branch = None;
    assert!(
        session_branch_diff(&r.state, false)
            .err()
            .ok_or("expected an error")?
            .to_string()
            .contains("has no worktree branch")
    );
    Ok(())
}

#[test]
fn t2_deleted_branch_is_reported() -> Result<()> {
    let mut r = Repo::new()?;
    git(
        &r.base,
        &[
            "worktree",
            "remove",
            r.wt.to_str().ok_or("non-UTF-8 worktree path")?,
        ],
    )?;
    git(&r.base, &["branch", "-D", "session"])?;
    r.state.worktree_path = None;
    assert!(
        session_branch_diff(&r.state, false)
            .err()
            .ok_or("expected an error")?
            .to_string()
            .contains("no longer exists")
    );
    Ok(())
}

#[test]
fn t3_committed_only_includes_patch_stat_and_full_merge_base() -> Result<()> {
    let mut r = Repo::new()?;
    let base = git(&r.base, &["rev-parse", "HEAD"])?;
    r.commit("a.txt", "hello\n")?;
    r.state.worktree_path = None;
    let diff = session_branch_diff(&r.state, false)?;
    assert_eq!(diff.source, DiffSource::CommittedOnly);
    assert_eq!(diff.base_commit, base);
    assert!(diff.patch.contains("+++ b/a.txt"), "{}", diff.patch);
    assert!(diff.stat.contains("a.txt | 1 +"), "{}", diff.stat);
    assert!(!diff.truncated);
    Ok(())
}

#[test]
fn t4_worktree_includes_tracked_and_untracked_changes() -> Result<()> {
    let r = Repo::new()?;
    fs::write(r.wt.join("README.md"), "changed\n")?;
    fs::write(r.wt.join("new.txt"), "new\n")?;
    let diff = session_branch_diff(&r.state, false)?;
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
    Ok(())
}

#[test]
fn t5_base_advancement_does_not_appear_in_diff() -> Result<()> {
    let r = Repo::new()?;
    r.commit("session.txt", "session\n")?;
    fs::write(r.base.join("base-only.txt"), "base\n")?;
    git(&r.base, &["add", "base-only.txt"])?;
    git(&r.base, &["commit", "-m", "advance base"])?;
    let diff = session_branch_diff(&r.state, false)?;
    assert!(diff.patch.contains("session.txt"));
    assert!(!diff.patch.contains("base-only.txt"));
    Ok(())
}

#[test]
fn t6_external_diff_and_textconv_are_never_executed() -> Result<()> {
    let r = Repo::new()?;
    fs::write(r.wt.join(".gitattributes"), "*.txt diff=evil\n")?;
    fs::write(r.wt.join("evil.txt"), "before\n")?;
    git(&r.wt, &["add", ".gitattributes", "evil.txt"])?;
    git(&r.wt, &["commit", "-m", "attributes"])?;
    fs::write(r.wt.join("evil.txt"), "after\n")?;
    let marker = r.tmp.path().join("MARKER");
    let command = format!("touch {}", marker.display());
    git(&r.base, &["config", "diff.evil.textconv", &command])?;
    git(&r.base, &["config", "diff.evil.command", &command])?;
    let diff = session_branch_diff(&r.state, false)?;
    assert!(diff.patch.contains("evil.txt"));
    assert!(!marker.exists());
    Ok(())
}

#[test]
fn t7_clean_branch_has_empty_diff() -> Result<()> {
    let r = Repo::new()?;
    let diff = session_branch_diff(&r.state, false)?;
    assert_eq!(diff.patch, "");
    assert_eq!(diff.stat, "");
    assert!(!diff.truncated);
    Ok(())
}

#[test]
fn t8_large_untracked_patch_is_bounded_at_newline() -> Result<()> {
    let r = Repo::new()?;
    fs::write(
        r.wt.join("large.txt"),
        ("a".repeat(127) + "\n").repeat(16384),
    )?;
    let diff = session_branch_diff(&r.state, false)?;
    assert!(diff.truncated);
    assert!(diff.patch.len() <= MAX_PATCH_BYTES);
    assert!(diff.patch.ends_with('\n'));
    Ok(())
}

#[test]
fn large_tracked_patch_is_bounded_and_git_finishes() -> Result<()> {
    let r = Repo::new()?;
    fs::write(r.wt.join("README.md"), ("large line\n").repeat(600_000))?;
    let diff = session_branch_diff(&r.state, false)?;
    assert!(diff.truncated);
    assert!(diff.patch.len() <= MAX_PATCH_BYTES);
    assert!(diff.patch.ends_with('\n'));
    assert!(diff.patch.contains("+large line"));
    assert!(session_branch_diff(&r.state, false).is_ok());
    Ok(())
}

#[cfg(unix)]
#[test]
fn untracked_non_utf8_path_is_passed_to_git_unchanged() -> Result<()> {
    use std::{
        ffi::OsStr,
        os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    };

    // Run the mock in a separate process so its PATH cannot affect parallel tests.
    // It also exercises the fallback on filesystems that accept non-UTF-8 names.
    if std::env::var_os("CRUISE_TEST_NON_UTF8_GIT").is_none() {
        let mock = TempDir::new()?;
        let real_git = Command::new("which").arg("git").output()?;
        assert!(real_git.status.success());
        let real_git = String::from_utf8(real_git.stdout)?.trim().to_owned();
        let script = mock.path().join("git");
        fs::write(
            &script,
            r#"#!/bin/sh
if [ "$3" = ls-files ]; then
    printf 'invalid-\377.txt\000'
    exit 0
fi
for arg do
    if [ "$arg" = --no-index ]; then
        no_index=1
    fi
    last=$arg
done
if [ "$no_index" = 1 ]; then
    expected=$(printf 'invalid-\377.txt')
    if [ "$last" != "$expected" ]; then
        echo 'non-UTF-8 path was not passed unchanged' >&2
        exit 2
    fi
    printf '+native path content\n'
    exit 1
fi
exec "$CRUISE_TEST_REAL_GIT" "$@"
"#,
        )?;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
        let path = std::env::join_paths(std::iter::once(mock.path().to_path_buf()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))?;
        let output = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "untracked_non_utf8_path_is_passed_to_git_unchanged",
                "--nocapture",
            ])
            .env("CRUISE_TEST_NON_UTF8_GIT", "1")
            .env("CRUISE_TEST_REAL_GIT", real_git)
            .env("PATH", path)
            .output()?;
        assert!(
            output.status.success(),
            "mock git subprocess failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "mock git subprocess did not run the test: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    let r = Repo::new()?;
    if std::env::var_os("CRUISE_TEST_NON_UTF8_GIT").is_none() {
        match fs::write(
            r.wt.join(OsStr::from_bytes(b"invalid-\xff.txt")),
            "native path content\n",
        ) {
            Ok(()) => {}
            // The subprocess above already checked the complete Git interaction.
            Err(error) if error.raw_os_error() == Some(libc::EILSEQ) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    let diff = session_branch_diff(&r.state, false)?;
    assert!(diff.patch.contains("+native path content"));
    assert!(!diff.truncated);
    Ok(())
}

#[test]
fn t9_untracked_count_is_bounded() -> Result<()> {
    let r = Repo::new()?;
    for n in 0..=UNTRACKED_FILE_LIMIT {
        fs::write(r.wt.join(format!("file-{n:04}.txt")), "hello\n")?;
    }
    let diff = session_branch_diff(&r.state, false)?;
    assert!(diff.truncated);
    assert_eq!(
        diff.patch.matches("diff --git ").count(),
        UNTRACKED_FILE_LIMIT
    );
    Ok(())
}

#[test]
fn t10_color_is_opt_in() -> Result<()> {
    let r = Repo::new()?;
    fs::write(r.wt.join("README.md"), "changed\n")?;
    assert!(
        session_branch_diff(&r.state, true)?
            .patch
            .contains("\u{1b}[")
    );
    assert!(
        !session_branch_diff(&r.state, false)?
            .patch
            .contains("\u{1b}[")
    );
    Ok(())
}

#[test]
fn t11_missing_worktree_falls_back_to_committed_only() -> Result<()> {
    let mut r = Repo::new()?;
    r.commit("a.txt", "hello\n")?;
    r.state.worktree_path = Some(r.tmp.path().join("missing"));
    let diff = session_branch_diff(&r.state, false)?;
    assert_eq!(diff.source, DiffSource::CommittedOnly);
    assert!(diff.patch.contains("+++ b/a.txt"));
    Ok(())
}

#[test]
fn t12_worktreeinclude_files_and_directories_are_excluded() -> Result<()> {
    let r = Repo::new()?;
    fs::write(
        r.base.join(".worktreeinclude"),
        "secret.env\nvendor/\nREADME.md\n",
    )?;
    fs::write(r.wt.join("README.md"), "tracked change\n")?;
    fs::write(r.wt.join("secret.env"), "secret\n")?;
    fs::create_dir(r.wt.join("vendor"))?;
    fs::write(r.wt.join("vendor/copied.txt"), "copied\n")?;
    fs::write(r.wt.join("visible.txt"), "visible\n")?;
    let diff = session_branch_diff(&r.state, false)?;
    assert!(diff.patch.contains("+tracked change"), "{}", diff.patch);
    assert!(diff.stat.contains("README.md"), "{}", diff.stat);
    assert!(diff.patch.contains("visible.txt"));
    assert!(!diff.patch.contains("secret.env"));
    assert!(!diff.patch.contains("vendor/"));
    assert!(!diff.stat.contains("secret.env"));
    assert!(!diff.stat.contains("vendor/"));
    Ok(())
}

#[test]
fn t13_non_repository_reports_git_error_not_unborn_head() -> Result<()> {
    let mut r = Repo::new()?;
    r.state.base_dir = r.tmp.path().join("not-a-repository");
    fs::create_dir(&r.state.base_dir)?;
    let error = session_branch_diff(&r.state, false)
        .err()
        .ok_or("expected an error")?
        .to_string();
    assert!(!error.contains("has no HEAD commit"), "{error}");
    assert!(
        error.contains("git") || error.contains("failed to read"),
        "{error}"
    );
    Ok(())
}

#[test]
fn t14a_unborn_base_has_no_head_commit() -> Result<()> {
    let mut r = Repo::new()?;
    let unborn = r.tmp.path().join("unborn");
    fs::create_dir(&unborn)?;
    git(&unborn, &["init", "-b", "main"])?;
    r.state.base_dir = unborn;
    let error = session_base_commit(&r.state)
        .err()
        .ok_or("expected an error")?
        .to_string();
    assert!(error.contains("has no HEAD commit"), "{error}");
    Ok(())
}

#[test]
#[ignore = "requires the sibling local-merge feature for explicit merge-target precedence"]
fn t14b_explicit_merge_target_takes_precedence_over_head() -> Result<()> {
    let mut r = Repo::new()?;
    // Keep HEAD on an older diverged branch while main advances to the session commit.
    r.commit("a.txt", "hello\n")?;
    git(&r.base, &["checkout", "-b", "other"])?;
    fs::write(r.base.join("other.txt"), "other\n")?;
    git(&r.base, &["add", "other.txt"])?;
    git(&r.base, &["commit", "-m", "other"])?;
    git(&r.base, &["branch", "-f", "main", "session"])?;
    // The merge-target field is supplied by the sibling local-merge change. Using
    // serde keeps this contract test buildable before that field is introduced.
    let mut value = serde_json::to_value(&r.state)?;
    value["forked_from_branch"] = serde_json::json!("main");
    r.state = serde_json::from_value(value)?;
    let expected = git(&r.base, &["merge-base", "main", "refs/heads/session"])?;
    let actual = session_base_commit(&r.state)?;
    assert_eq!(actual, expected);
    assert_ne!(actual, git(&r.base, &["rev-parse", "HEAD"])?);
    Ok(())
}
