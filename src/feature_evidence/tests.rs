use super::*;

struct Session {
    _temp: tempfile::TempDir,
    _data: crate::test_support::EnvGuard,
    repo: PathBuf,
    plan: PathBuf,
    root: PathBuf,
}

fn git(repo: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(status.success(), "git {args:?}");
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap_or(path)).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(path, text).unwrap_or_else(|e| panic!("{e}"));
}

/// A committed repository with a verification skill mapping two features.
fn session() -> Session {
    let temp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let data = crate::test_support::EnvGuard::set("XDG_DATA_HOME", temp.path().join("data"));
    let repo = temp.path().join("repo");
    let skill = repo.join(".agents/skills/verify-app");
    write(&skill.join("SKILL.md"), "---\nname: verify-app\n---\n");
    write(&skill.join("features/README.md"), "# map\n");
    write(
        &skill.join("features/cli-run.md"),
        "# Run\n\nCovers: `src/cli.rs`, `src/engine/`\n",
    );
    write(
        &skill.join("features/webui.md"),
        "# WebUI\n\nCovers: `webui/**/*.tsx`\n",
    );
    write(&repo.join("src/cli.rs"), "fn main() {}\n");
    write(&repo.join(".gitignore"), "target/\n");
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "-A"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-qm",
            "i",
        ],
    );
    let session = crate::paths::data_dir()
        .unwrap_or_else(|e| panic!("{e}"))
        .join("sessions/test-session");
    write(&session.join("plan.md"), "plan");
    Session {
        root: session.join(crate::artifacts::ARTIFACTS_DIR),
        plan: session.join("plan.md"),
        _temp: temp,
        _data: data,
        repo,
    }
}

fn report(s: &Session, json: &str) {
    crate::artifacts::write(&s.root, REPORT, json).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn prepare_selects_only_features_whose_covers_changed() {
    let _lock = crate::test_support::lock_process();
    let s = session();
    assert!(!prepare(&s.repo, &s.plan).unwrap_or_else(|e| panic!("{e}")));
    write(&s.repo.join("src/engine/step.rs"), "x");
    assert!(prepare(&s.repo, &s.plan).unwrap_or_else(|e| panic!("{e}")));
    assert_eq!(touched_ids(&s.root), ["cli-run"]);
    let plan = crate::artifacts::read(&s.root, PLAN).unwrap_or_default();
    assert!(plan.contains("`cli-run`"), "{plan}");
}

#[test]
fn missing_skill_stops_before_any_drive() {
    let _lock = crate::test_support::lock_process();
    let s = session();
    std::fs::remove_dir_all(s.repo.join(".agents")).unwrap_or_else(|e| panic!("{e}"));
    let error = prepare(&s.repo, &s.plan)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(error.contains("no verification skill"), "{error}");
}

#[test]
fn proved_feature_is_saved_and_bound_to_the_tree() {
    let _lock = crate::test_support::lock_process();
    let s = session();
    write(&s.repo.join("src/cli.rs"), "fn main() { run() }\n");
    assert!(prepare(&s.repo, &s.plan).unwrap_or_else(|e| panic!("{e}")));
    write(
        &s.repo.join("target/verify-cruise/run/cli-run/01-final.txt"),
        "workflow complete\n",
    );
    report(
        &s,
        r#"{"features":[{"id":"cli-run","status":"proved","summary":"exit 0","files":["target/verify-cruise/run/cli-run/01-final.txt"]}]}"#,
    );
    assert_eq!(
        validate(&s.repo, &s.plan).unwrap_or_else(|e| panic!("{e}")),
        "Feature evidence proved 1 feature(s)."
    );
    let section = publication(
        &s.root,
        &s.repo,
        "test-session",
        crate::forge::ForgeKind::GitLab,
    )
    .unwrap_or_else(|e| panic!("{e}"))
    .unwrap_or_default();
    assert!(section.contains("- **cli-run**: exit 0"), "{section}");
    assert!(section.contains("`01-final.txt`"), "{section}");
    // A later edit makes the evidence stale: publication refuses it.
    write(&s.repo.join("src/cli.rs"), "fn main() {}\n");
    assert!(
        publication(
            &s.root,
            &s.repo,
            "test-session",
            crate::forge::ForgeKind::GitLab
        )
        .is_err()
    );
}

#[test]
fn regression_blocked_and_unproved_features_stop_before_the_pr() {
    let _lock = crate::test_support::lock_process();
    let s = session();
    write(&s.repo.join("src/cli.rs"), "changed\n");
    assert!(prepare(&s.repo, &s.plan).unwrap_or_else(|e| panic!("{e}")));
    for (json, expected) in [
        (
            r#"{"features":[{"id":"cli-run","status":"failed","summary":"exit 1","files":[]}]}"#,
            "feature regression",
        ),
        (
            r#"{"features":[{"id":"cli-run","status":"blocked","summary":"no PTY","files":[]}]}"#,
            "evidence blocked",
        ),
        (
            r#"{"features":[{"id":"cli-run","status":"map drift","summary":"menu renamed","files":[]}]}"#,
            "evidence blocked",
        ),
        (
            r#"{"features":[{"id":"cli-run","status":"proved","summary":"ok","files":[]}]}"#,
            "without saved files",
        ),
        (r#"{"features":[]}"#, "no result for feature"),
        (
            r#"{"features":[{"id":"cli-run","status":"proved","summary":"ok","files":["src/cli.rs"]}]}"#,
            "must be under",
        ),
    ] {
        report(&s, json);
        let error = validate(&s.repo, &s.plan)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(error.contains(expected), "{json}: {error}");
    }
}

#[test]
fn edits_outside_the_skill_and_credential_text_are_refused() {
    let _lock = crate::test_support::lock_process();
    let s = session();
    write(&s.repo.join("src/cli.rs"), "changed\n");
    assert!(prepare(&s.repo, &s.plan).unwrap_or_else(|e| panic!("{e}")));
    write(
        &s.repo.join("target/verify-cruise/run/log.txt"),
        "login: http://x/login?nonce=abc\n",
    );
    report(
        &s,
        r#"{"features":[{"id":"cli-run","status":"proved","summary":"ok","files":["target/verify-cruise/run/log.txt"]}]}"#,
    );
    let error = validate(&s.repo, &s.plan)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(error.contains("credential-like"), "{error}");
    write(
        &s.repo.join(".agents/skills/verify-app/features/cli-run.md"),
        "# Run\n\nCovers: `src/cli.rs`\n",
    );
    write(&s.repo.join("target/verify-cruise/run/log.txt"), "ok\n");
    validate(&s.repo, &s.plan).unwrap_or_else(|e| panic!("map fix in the skill is allowed: {e}"));
    write(&s.repo.join("src/other.rs"), "x");
    let error = validate(&s.repo, &s.plan)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(error.contains("may edit only"), "{error}");
}

#[test]
fn section_replacement_keeps_the_rest_of_the_body() {
    let body = "## Change\n\nx\n\n## Implementation evidence\n\n- Quint\n\n### Feature journeys (tree `abc`)\n\n- old\n\n## Notes\n\ny\n";
    let updated = replace_section(body, "### Feature journeys (tree `def`)\n\n- new\n");
    assert!(updated.contains("- Quint"), "{updated}");
    assert!(updated.contains("- new"), "{updated}");
    assert!(!updated.contains("- old"), "{updated}");
    assert!(updated.contains("## Notes\n\ny"), "{updated}");
    let appended = replace_section("## Change\n", "### Feature journeys\n\n- new\n");
    assert!(appended.ends_with("- new\n"), "{appended}");
}

#[test]
fn github_publication_pushes_files_to_the_evidence_branch_and_links_them() {
    let _lock = crate::test_support::lock_process();
    let s = session();
    // The GitHub origin is rewritten to a local bare repository, so the push stays local.
    let bare = s.repo.parent().unwrap_or(&s.repo).join("origin.git");
    git(
        s.repo.parent().unwrap_or(&s.repo),
        &["init", "-q", "--bare", "origin.git"],
    );
    git(
        &s.repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/app.git",
        ],
    );
    git(
        &s.repo,
        &[
            "config",
            &format!("url.{}.insteadOf", bare.display()),
            "https://github.com/owner/app.git",
        ],
    );
    write(&s.repo.join("src/cli.rs"), "fn main() { run() }\n");
    assert!(prepare(&s.repo, &s.plan).unwrap_or_else(|e| panic!("{e}")));
    write(
        &s.repo.join("target/verify-cruise/run/cli-run/01-final.png"),
        "\u{89}PNG fake",
    );
    write(
        &s.repo.join("target/verify-cruise/run/cli-run/result.json"),
        "{\"exit_code\":0}\n",
    );
    report(
        &s,
        r#"{"features":[{"id":"cli-run","status":"proved","summary":"exit 0","files":["target/verify-cruise/run/cli-run/01-final.png","target/verify-cruise/run/cli-run/result.json"]}]}"#,
    );
    validate(&s.repo, &s.plan).unwrap_or_else(|e| panic!("{e}"));
    let section = publication(
        &s.root,
        &s.repo,
        "test-session",
        crate::forge::ForgeKind::GitHub,
    )
    .unwrap_or_else(|e| panic!("{e}"))
    .unwrap_or_default();
    assert!(
        section.contains("![01-final.png](https://github.com/owner/app/raw/"),
        "{section}"
    );
    assert!(
        section.contains("[result.json](https://github.com/owner/app/blob/"),
        "{section}"
    );
    let listing = std::process::Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "cruise-evidence"])
        .current_dir(&bare)
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    let listing = String::from_utf8_lossy(&listing.stdout);
    assert!(listing.contains("sessions/test-session/"), "{listing}");
    assert!(listing.contains("cli-run/00-01-final.png"), "{listing}");
    // The checkout's branch and index are untouched.
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&s.repo)
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(String::from_utf8_lossy(&status.stdout), " M src/cli.rs\n");
}
