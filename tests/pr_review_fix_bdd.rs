//! Production existing-PR after-PR loop contracts with fake GitHub and real Git.
#[cfg(unix)]
mod contracts {
    use cruise::config::{WorkflowConfig, validate_config};
    use cruise::error::CruiseError;
    use cruise::file_tracker::FileTracker;
    use cruise::flags::FlagValues;
    use cruise::option_handler::OptionHandler;
    use cruise::pr_review_fix::PrTarget;
    use cruise::session::SessionState;
    use cruise::session_config::SessionConfigRef;
    use cruise::variable::VariableStore;
    use cruise::worktree::WorktreeContext;
    use cucumber::writer::Stats as _;
    use cucumber::{World as _, given, then, when};
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

    struct NoOpOptionHandler;

    impl OptionHandler for NoOpOptionHandler {
        fn select_option(
            &self,
            _choices: &[cruise::step::OptionChoice],
            _plan: Option<&str>,
        ) -> cruise::error::Result<cruise::step::option::OptionResult> {
            panic!("unexpected option step")
        }
    }

    #[derive(Debug, Default, cucumber::World)]
    struct ReviewWorld {
        root: Option<tempfile::TempDir>,
        limit: usize,
        result: Option<Result<(), CruiseError>>,
    }

    fn executable(path: &std::path::Path, script: &str) {
        std::fs::write(path, script).expect("write fake executable");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    fn thread(id: &str) -> serde_json::Value {
        json!({"id":id,"isResolved":false,"viewerCanReply":true,"viewerCanResolve":true,
            "path":"src/lib.rs","line":1,"comments":{"pageInfo":{"hasNextPage":false,"endCursor":null},
            "nodes":[{"author":{"login":"writer"},"body":"please fix"}]}})
    }
    fn response(threads: Vec<serde_json::Value>) -> String {
        json!({"data":{"repository":{"pullRequest":{"reviewThreads":{
            "pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":threads}}}}})
        .to_string()
    }

    #[given(expr = "the all-reviewer fixture has {string} threads and a limit of {int}")]
    fn fixture(world: &mut ReviewWorld, mode: String, limit: usize) {
        let root = tempfile::tempdir().expect("fixture");
        let path = root.path();
        let bin = path.join("bin");
        std::fs::create_dir(&bin).expect("bin");
        let repo = path.join("repo");
        std::fs::create_dir(&repo).expect("repo");
        for args in [
            &["init", "-q", "-b", "feature"][..],
            &[
                "-c",
                "user.name=BDD",
                "-c",
                "user.email=bdd@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "fixture",
            ][..],
            &["remote", "add", "origin", "https://github.com/acme/repo.git"][..],
            &["update-ref", "refs/remotes/origin/feature", "HEAD"][..],
        ] {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .expect("run git fixture");
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let initial = if mode == "empty" {
            vec![]
        } else if mode == "excluded" {
            let mut excluded = thread("excluded");
            excluded["comments"]["nodes"][0]["author"]["login"] = json!("reader");
            vec![excluded]
        } else {
            vec![thread("t1"), thread("t2")]
        };
        let after = match mode.as_str() {
            "empty" | "complete" | "excluded" => vec![],
            "unchanged" => initial.clone(),
            "partial" | "two iterations" => vec![thread("t2")],
            _ => panic!("unknown mode {mode}"),
        };
        std::fs::write(path.join("initial.json"), response(initial)).expect("initial response");
        std::fs::write(path.join("after.json"), response(after)).expect("after response");
        let gh = format!(
            r##"#!/bin/sh
printf '%s\n' "$*" >> '{0}/gh.log'
case "$*" in
  --version) echo 'gh version test' ;;
  *collaborators/reader/permission*) echo read ;;
  *collaborators/*/permission*) echo write ;;
  *reviewThreads*)
    if [ ! -e '{0}/seen' ]; then touch '{0}/seen'; cat '{0}/initial.json'
    elif [ '{1}' = 'two iterations' ] && [ -f '{0}/prompt' ] && [ "$(wc -l < '{0}/prompt')" -ge 2 ]; then echo '{2}'
    else touch '{0}/seen-again'; cat '{0}/after.json'; fi ;;
  *headRefOid*) echo head ;;
  *) echo "unexpected gh invocation: $*" >&2; exit 1 ;;
esac
"##,
            path.display(),
            mode,
            response(vec![])
        );
        executable(&bin.join("gh"), &gh);
        executable(
            &bin.join("prompt"),
            &format!(
                "#!/bin/sh\ngit status --porcelain --untracked-files=all >> '{0}/prompt-status'\necho run >> '{0}/prompt'\necho '{{\"actions\":[]}}'\n",
                path.display()
            ),
        );
        let old_path = std::env::var_os("PATH").unwrap_or_default();
        let path_value =
            std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&old_path)))
                .expect("path");
        // One test process, one current-thread runtime, and serialized scenarios.
        unsafe { std::env::set_var("PATH", path_value) };
        world.limit = limit;
        world.root = Some(root);
    }

    #[when("the existing PR workflow runs")]
    async fn execute(world: &mut ReviewWorld) {
        let root = world.root.as_ref().expect("fixture").path();
        let repo = root.join("repo");
        let yaml = format!(
            "steps:\n  main:\n    prompt: unused\nafter-pr:\n  review:\n    github-review:\n      reviewers: all\n      max-iterations: {}\n    prompt: address feedback\n    timeout: 30\n",
            world.limit
        );
        let mut config = WorkflowConfig::from_yaml(&yaml).expect("workflow YAML");
        config.command = vec![root.join("bin/prompt").to_string_lossy().into_owned()];
        validate_config(&config).expect("validate workflow");
        let compiled = cruise::workflow::compile(config).expect("compile workflow");
        let mut vars = VariableStore::new("fixture".into());
        vars.set_artifacts_root(root.join("artifacts"));
        let mut tracker = FileTracker::with_root(repo.clone());
        let mut state = SessionState::new(
            "20261106000000".into(),
            repo.clone(),
            SessionConfigRef::inline_snapshot(),
            "fixture".into(),
        );
        state.pr_target = Some(PrTarget {
            number: 12,
            title: "fixture".into(),
            url: "https://github.com/acme/repo/pull/12".into(),
            head_branch: "feature".into(),
            base_branch: "main".into(),
        });
        let ctx = WorktreeContext {
            path: repo.clone(),
            original_dir: repo,
            branch: "feature".into(),
            owns_branch: false,
        };
        world.result = Some(
            cruise::worktree_pr::handle_worktree_pr_with_persistence(
                &ctx,
                &compiled,
                &mut vars,
                &mut tracker,
                &mut state,
                0,
                0,
                &[],
                &FlagValues::new(),
                None,
                &NoOpOptionHandler,
                None,
                None,
                None,
            )
            .await,
        );
    }

    #[then("the review loop succeeds without a prompt")]
    fn empty(world: &mut ReviewWorld) {
        assert!(
            world.result.as_ref().expect("result").is_ok(),
            "{:?}",
            world.result
        );
        assert!(
            !world
                .root
                .as_ref()
                .expect("fixture")
                .path()
                .join("prompt")
                .exists()
        );
    }
    #[then("the review loop succeeds after the prompt")]
    fn complete(world: &mut ReviewWorld) {
        assert!(
            world.result.as_ref().expect("result").is_ok(),
            "{:?}",
            world.result
        );
        assert!(world.root.as_ref().unwrap().path().join("prompt").exists());
    }
    #[then(expr = "the review loop fails for {string}")]
    fn failure(world: &mut ReviewWorld, expected: String) {
        match world.result.as_ref().expect("result") {
            Err(CruiseError::GitHubReviewFailed { detail, .. }) => {
                assert!(detail.contains(&expected), "{detail}")
            }
            result => panic!("expected GitHubReviewFailed, got {result:?}"),
        }
    }
    #[then("the prompt ran in a clean git repository")]
    fn invoked(world: &mut ReviewWorld) {
        let root = world.root.as_ref().expect("fixture").path();
        assert!(root.join("prompt").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("prompt-status")).unwrap(),
            ""
        );
        let git = std::process::Command::new("git")
            .args(["status", "--porcelain", "--untracked-files=all"])
            .current_dir(root.join("repo"))
            .output()
            .expect("git status");
        assert!(
            git.status.success() && git.stdout.is_empty(),
            "{}",
            String::from_utf8_lossy(&git.stdout)
        );
        let gh = std::fs::read_to_string(root.join("gh.log")).expect("gh log");
        assert!(
            gh.contains("reviewThreads") && gh.contains("collaborators/writer/permission"),
            "{gh}"
        );
    }

    #[then(expr = "the prompt ran {int} times in a clean git repository")]
    fn invoked_times(world: &mut ReviewWorld, count: usize) {
        invoked(world);
        let root = world.root.as_ref().unwrap().path();
        assert_eq!(
            std::fs::read_to_string(root.join("prompt"))
                .unwrap()
                .lines()
                .count(),
            count
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pr_review_fix_bdd() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/features/pr_review_fix.feature"
        );
        let feature =
            cucumber::gherkin::Feature::parse_path(path, cucumber::gherkin::GherkinEnv::default())
                .expect("parse feature");
        assert_eq!(feature.scenarios.len(), 6);
        let writer = ReviewWorld::cucumber()
            .max_concurrent_scenarios(1)
            .fail_on_skipped()
            .run(path)
            .await;
        assert!(writer.passed_steps() + writer.failed_steps() > 0);
        assert!(
            !writer.execution_has_failed(),
            "{} failed steps, {} parsing errors, {} hook errors",
            writer.failed_steps(),
            writer.parsing_errors(),
            writer.hook_errors()
        );
    }
}
