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
        mode: String,
        limit: usize,
        result: Option<Result<(), CruiseError>>,
    }

    fn executable(path: &std::path::Path, script: &str) {
        std::fs::write(path, script)
            .unwrap_or_else(|error| panic!("write fake executable: {error}"));
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("chmod fake executable: {error}"));
    }
    static HOST_GIT: std::sync::LazyLock<std::path::PathBuf> = std::sync::LazyLock::new(|| {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|directory| directory.join("git"))
            .find(|path| path.is_file())
            .unwrap_or_else(|| panic!("find git executable in PATH"))
    });

    fn git(cwd: &std::path::Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new(&*HOST_GIT)
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap_or_else(|error| panic!("run git {args:?}: {error}"))
    }

    fn git_ok(cwd: &std::path::Path, args: &[&str]) -> String {
        let output = git(cwd, args);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn configure_repository_controls(
        path: &std::path::Path,
        repo: &std::path::Path,
        origin: &std::path::Path,
        mode: &str,
        initial: &str,
    ) {
        git_ok(
            repo,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/acme/repo.git",
            ],
        );
        git_ok(origin, &["config", "receive.advertisePushOptions", "true"]);
        git_ok(repo, &["config", "push.followTags", "true"]);
        git_ok(repo, &["config", "push.pushOption", "fixture-option"]);
        git_ok(
            repo,
            &["tag", "-a", "review-only-tag", "-m", "fixture tag", "HEAD"],
        );
        executable(
            &origin.join("hooks/pre-receive"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"${{GIT_PUSH_OPTION_COUNT-unset}}\" > '{}'\n",
                path.join("push-options.txt").display()
            ),
        );
        if mode == "external-filter" {
            let clean = format!(
                "echo invoked > '{}'; cat",
                path.join("filter-ran").display()
            );
            git_ok(repo, &["config", "filter.fixture.clean", clean.as_str()]);
        }
        if mode == "in-progress-merge" {
            std::fs::write(repo.join(".git/MERGE_HEAD"), initial)
                .unwrap_or_else(|error| panic!("write merge marker: {error}"));
        }
        executable(
            &repo.join(".git/hooks/pre-commit"),
            &format!(
                "#!/bin/sh\necho called > '{}'\n",
                path.join("hook-ran").display()
            ),
        );
        executable(
            &repo.join(".git/hooks/pre-push"),
            &format!(
                "#!/bin/sh\necho called > '{}'\n",
                path.join("hook-ran").display()
            ),
        );
    }

    fn initialize_repository(
        path: &std::path::Path,
        repo: &std::path::Path,
        mode: &str,
    ) -> (std::path::PathBuf, String) {
        let origin = path.join("origin.git");
        git_ok(
            path,
            &["init", "--bare", "-q", origin.to_str().unwrap_or("")],
        );
        std::fs::write(repo.join("tracked.txt"), "baseline\n")
            .unwrap_or_else(|error| panic!("write tracked baseline: {error}"));
        if mode == "external-filter" {
            std::fs::write(repo.join(".gitattributes"), "tracked.txt filter=fixture\n")
                .unwrap_or_else(|error| panic!("write fixture attributes: {error}"));
        }
        git_ok(repo, &["init", "-q", "-b", "feature"]);
        git_ok(repo, &["add", "--all"]);
        git_ok(repo, &["config", "user.name", "BDD"]);
        git_ok(repo, &["config", "user.email", "bdd@example.invalid"]);
        git_ok(
            repo,
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "fixture",
            ],
        );
        let origin_text = origin.to_string_lossy().into_owned();
        git_ok(repo, &["remote", "add", "origin", origin_text.as_str()]);
        git_ok(repo, &["push", "-q", "-u", "origin", "feature"]);
        let initial = git_ok(repo, &["rev-parse", "HEAD"]);
        std::fs::write(path.join("initial-head.txt"), &initial)
            .unwrap_or_else(|error| panic!("save baseline HEAD: {error}"));

        if matches!(mode, "wrong-head" | "remote-advanced" | "reply-race") {
            git_ok(repo, &["checkout", "-q", "-b", "race"]);
            std::fs::write(repo.join("race.txt"), "concurrent update\n")
                .unwrap_or_else(|error| panic!("write concurrent change: {error}"));
            git_ok(repo, &["add", "--all"]);
            git_ok(repo, &["commit", "-q", "-m", "concurrent update"]);
            let race_head = git_ok(repo, &["rev-parse", "HEAD"]);
            std::fs::write(path.join("race-head.txt"), &race_head)
                .unwrap_or_else(|error| panic!("save concurrent HEAD: {error}"));
            git_ok(repo, &["push", "-q", "origin", "race"]);
            git_ok(repo, &["checkout", "-q", "feature"]);
            if mode == "wrong-head" {
                git_ok(
                    &origin,
                    &["update-ref", "refs/heads/feature", race_head.as_str()],
                );
            }
        }
        configure_repository_controls(path, repo, &origin, mode, &initial);
        if mode == "dirty-baseline" {
            std::fs::write(repo.join("dirty.txt"), "already dirty\n")
                .unwrap_or_else(|error| panic!("make baseline dirty: {error}"));
        } else if mode == "detached-head" {
            git_ok(repo, &["checkout", "-q", "--detach", "HEAD"]);
        } else if mode == "wrong-branch" {
            git_ok(repo, &["checkout", "-q", "-b", "other"]);
            git_ok(
                repo,
                &["branch", "--set-upstream-to=origin/feature", "other"],
            );
        } else if mode == "wrong-tracking" {
            git_ok(
                repo,
                &["update-ref", "refs/remotes/origin/other", initial.as_str()],
            );
            git_ok(
                repo,
                &["branch", "--set-upstream-to=origin/other", "feature"],
            );
        }
        (origin, initial)
    }

    fn thread(id: &str) -> serde_json::Value {
        json!({"id":id,"isResolved":false,"viewerCanReply":true,"viewerCanResolve":true,
            "path":"src/lib.rs","line":1,"comments":{"pageInfo":{"hasNextPage":false,"endCursor":null},
            "nodes":[{"author":{"login":"writer"},"body":"please fix"}]}})
    }
    fn response(threads: &[serde_json::Value]) -> String {
        json!({"data":{"repository":{"pullRequest":{"reviewThreads":{
            "pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":threads}}}}})
        .to_string()
    }

    #[given(expr = "the all-reviewer fixture has {string} threads and a limit of {int}")]
    fn fixture(world: &mut ReviewWorld, mode: String, limit: usize) {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("fixture: {error}"));
        let path = root.path();
        let bin = path.join("bin");
        std::fs::create_dir(&bin).unwrap_or_else(|error| panic!("create bin directory: {error}"));
        let repo = path.join("repo");
        std::fs::create_dir(&repo).unwrap_or_else(|error| panic!("create repo directory: {error}"));
        let (origin, _) = initialize_repository(path, &repo, &mode);
        let initial = if mode == "empty" {
            vec![]
        } else if mode == "excluded" {
            let mut excluded = thread("excluded");
            excluded["comments"]["nodes"][0]["author"]["login"] = json!("reader");
            vec![excluded]
        } else if mode == "ineligible-response" {
            let mut ineligible = thread("t1");
            ineligible["viewerCanReply"] = json!(false);
            vec![ineligible, thread("t2")]
        } else {
            vec![thread("t1"), thread("t2")]
        };
        let after = match mode.as_str() {
            "empty"
            | "complete"
            | "excluded"
            | "host-commit"
            | "host-no-changes"
            | "dirty-baseline"
            | "detached-head"
            | "wrong-branch"
            | "wrong-tracking"
            | "wrong-head"
            | "remote-advanced"
            | "reply-race"
            | "invalid-response"
            | "ineligible-response"
            | "custom-edit"
            | "external-filter"
            | "in-progress-merge"
            | "commit-cas-race" => vec![],
            "permission-revoked" => {
                let mut revoked = thread("t1");
                revoked["viewerCanReply"] = json!(false);
                vec![revoked, thread("t2")]
            }
            "write-race" => vec![thread("t1")],
            "unchanged" => initial.clone(),
            "partial" | "two iterations" => vec![thread("t2")],
            _ => panic!("unknown mode {mode}"),
        };
        std::fs::write(path.join("initial.json"), response(&initial))
            .unwrap_or_else(|error| panic!("write initial response: {error}"));
        std::fs::write(path.join("after.json"), response(&after))
            .unwrap_or_else(|error| panic!("write after response: {error}"));
        let gh = format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> '{0}/gh.log'
case "$*" in
  --version) echo 'gh version test' ;;
  *collaborators/reader/permission*) echo read ;;
  *collaborators/*/permission*) echo write ;;
  *addPullRequestReviewThreadReply*)
    echo "$*" >> '{0}/api-writes.log'; touch '{0}/actions-applied'
    echo '{{"data":{{"addPullRequestReviewThreadReply":{{"comment":{{"id":"c1"}}}}}}}}' ;;
  *reviewThreads*)
    if [ ! -e '{0}/seen' ]; then touch '{0}/seen'; cat '{0}/initial.json'
    elif [ -f '{0}/actions-applied' ]; then cat '{0}/after.json'
    elif [ '{1}' = 'write-race' ] && [ -f '{0}/host-pushed' ]; then cat '{0}/after.json'
    elif [ '{1}' = 'write-race' ]; then cat '{0}/initial.json'
    elif [ '{1}' = 'permission-revoked' ] && [ -f '{0}/prompt' ]; then cat '{0}/after.json'
    elif [ '{1}' = 'host-commit' ] || [ '{1}' = 'remote-advanced' ]; then cat '{0}/initial.json'
    elif [ '{1}' = 'two iterations' ] && [ -f '{0}/prompt' ] && [ "$(wc -l < '{0}/prompt')" -ge 2 ]; then echo '{2}'
    else touch '{0}/seen-again'; cat '{0}/after.json'; fi ;;
  *headRefName*)
    oid=$("$CRUISE_FIXTURE_REAL_GIT" --git-dir "$CRUISE_FIXTURE_ORIGIN" rev-parse refs/heads/feature) || exit
    printf '{{"state":"OPEN","number":12,"title":"fixture","url":"https://github.com/acme/repo/pull/12","headRefName":"feature","baseRefName":"main","headRefOid":"%s","isCrossRepository":false}}' "$oid" ;;
  *--jq*) "$CRUISE_FIXTURE_REAL_GIT" --git-dir "$CRUISE_FIXTURE_ORIGIN" rev-parse refs/heads/feature ;;
  *) echo "unexpected gh invocation: $*" >&2; exit 1 ;;
esac
"#,
            path.display(),
            mode,
            response(&[])
        );
        executable(&bin.join("gh"), &gh);
        let prompt = match mode.as_str() {
            "host-commit" => format!(
                "#!/bin/sh\ngit status --porcelain --untracked-files=all >> '{0}/prompt-status'\necho run >> '{0}/prompt'\nprintf 'host edit\\n' > tracked.txt\nprintf 'new file\\n' > 'review file.txt'\ngit add --all\nif git commit -m 'agent commit' > '{0}/prompt-agent-commit.log' 2>&1; then echo succeeded > '{0}/agent-commit-state'; else echo blocked > '{0}/agent-commit-state'; fi\necho '{{\"actions\":[{{\"thread_id\":\"t1\",\"reply\":\"fixed\",\"resolve\":false}}]}}'\n",
                path.display()
            ),
            "remote-advanced" => format!(
                "#!/bin/sh\necho run >> '{0}/prompt'\nprintf 'host edit\\n' > tracked.txt\necho '{{\"actions\":[{{\"thread_id\":\"t1\",\"reply\":\"fixed\",\"resolve\":false}}]}}'\n",
                path.display()
            ),
            "reply-race" => format!(
                "#!/bin/sh\necho run >> '{0}/prompt'\ngit --git-dir '{1}' update-ref refs/heads/feature \"$(cat '{0}/race-head.txt')\"\necho '{{\"actions\":[{{\"thread_id\":\"t1\",\"reply\":\"fixed\",\"resolve\":false}}]}}'\n",
                path.display(),
                origin.display()
            ),
            "permission-revoked" => format!(
                "#!/bin/sh\necho run >> '{0}/prompt'\nprintf 'edit\\n' > tracked.txt\necho '{{\"actions\":[{{\"thread_id\":\"t1\",\"reply\":\"fixed\",\"resolve\":false}}]}}'\n",
                path.display()
            ),
            "write-race" => format!(
                "#!/bin/sh\necho run >> '{0}/prompt'\nprintf 'host edit\\n' > tracked.txt\nprintf 'new file\\n' > 'review file.txt'\necho '{{\"actions\":[{{\"thread_id\":\"t1\",\"reply\":\"one\",\"resolve\":false}},{{\"thread_id\":\"t2\",\"reply\":\"two\",\"resolve\":false}}]}}'\n",
                path.display()
            ),
            "commit-cas-race" => format!(
                "#!/bin/sh\necho run >> '{0}/prompt'\nprintf 'host edit\\n' > tracked.txt\necho '{{\"actions\":[]}}'\n",
                path.display()
            ),
            "ineligible-response" => format!(
                "#!/bin/sh\necho run >> '{0}/prompt'\nprintf 'edit\\n' > tracked.txt\necho '{{\"actions\":[{{\"thread_id\":\"t1\",\"reply\":\"fixed\",\"resolve\":false}}]}}'\n",
                path.display()
            ),
            "invalid-response" | "custom-edit" => format!(
                "#!/bin/sh\ngit status --porcelain --untracked-files=all >> '{0}/prompt-status'\necho run >> '{0}/prompt'\nprintf 'edit\\n' > tracked.txt\n{1}\n",
                path.display(),
                if mode == "invalid-response" {
                    "echo 'not json'"
                } else {
                    "echo '{\"actions\":[]}'"
                }
            ),
            _ => format!(
                "#!/bin/sh\ngit status --porcelain --untracked-files=all >> '{0}/prompt-status'\necho run >> '{0}/prompt'\necho '{{\"actions\":[]}}'\n",
                path.display()
            ),
        };
        executable(&bin.join("prompt"), &prompt);
        executable(
            &bin.join("git"),
            r#"#!/bin/bash
set -u
real_git="${CRUISE_FIXTURE_REAL_GIT:?}"
git_prefix=()
while (($#)); do
  case "$1" in
    --no-optional-locks)
      git_prefix+=("$1"); shift ;;
    -c|-C|--git-dir|--work-tree|--namespace|--super-prefix|--exec-path|--config-env)
      if (($# < 2)); then echo "missing Git global option value: $1" >&2; exit 70; fi
      git_prefix+=("$1" "$2"); shift 2 ;;
    --git-dir=*|--work-tree=*|--namespace=*|--super-prefix=*|--exec-path=*)
      git_prefix+=("$1"); shift ;;
    *)
      break ;;
  esac
done
if (($# == 0)); then
  echo "missing Git subcommand" >&2
  exit 70
fi
subcommand=$1
shift

if [[ "$subcommand" == "push" ]]; then
  push_flags=()
  while (($#)) && [[ "$1" != "--" ]]; do
    push_flags+=("$1")
    shift
  done
  if (($# == 0)); then echo "host push has no separator" >&2; exit 70; fi
  shift
  if (($# != 2)); then echo "host push must have exactly one refspec" >&2; exit 70; fi
  endpoint=$1
  refspec=$2
  if [[ "$endpoint" != "$CRUISE_FIXTURE_PUSH_URL" ]]; then
    echo "refusing unexpected push endpoint: $endpoint" >&2
    exit 70
  fi
  if ((${#push_flags[@]} != 4)) \
    || [[ "${push_flags[0]}" != "--no-follow-tags" ]] \
    || [[ "${push_flags[1]}" != "--recurse-submodules=no" ]] \
    || [[ "${push_flags[2]}" != "--no-verify" ]] \
    || [[ "${push_flags[3]}" != --force-with-lease=refs/heads/feature:* ]]; then
    echo "refusing unexpected host push flags: ${push_flags[*]}" >&2
    exit 70
  fi
  if [[ "${refspec#*:}" != "refs/heads/feature" ]] \
    || [[ ! "${refspec%%:*}" =~ ^[[:xdigit:]]{40,64}$ ]]; then
    echo "refusing unexpected host refspec: $refspec" >&2
    exit 70
  fi
  if [[ "$CRUISE_FIXTURE_MODE" == "remote-advanced" ]]; then
    "$real_git" --git-dir "$CRUISE_FIXTURE_ORIGIN" \
      update-ref refs/heads/feature "$(cat "$CRUISE_FIXTURE_ROOT/race-head.txt")" || exit
  fi
  "$real_git" "${git_prefix[@]}" push "${push_flags[@]}" \
    -- "$CRUISE_FIXTURE_ORIGIN" "$refspec"
  result=$?
  if [ "$result" -eq 0 ]; then touch "$CRUISE_FIXTURE_ROOT/host-pushed"; fi
  exit "$result"
fi

if [[ "$subcommand" == "update-ref" \
  && "$CRUISE_FIXTURE_MODE" == "commit-cas-race" \
  && ! -e "$CRUISE_FIXTURE_ROOT/cas-raced" \
  && "${1-}" == "--no-deref" && "${2-}" == "-m" \
  && "${4-}" == "refs/heads/feature" && "$#" == 6 ]]; then
  parent=$6
  tree=$("$real_git" "${git_prefix[@]}" write-tree) || exit
  sibling=$("$real_git" "${git_prefix[@]}" -c user.name=BDD \
    -c user.email=bdd@example.invalid commit-tree "$tree" -p "$parent" \
    -m 'concurrent sibling') || exit
  "$real_git" "${git_prefix[@]}" update-ref --no-deref \
    refs/heads/feature "$sibling" "$parent" || exit
  touch "$CRUISE_FIXTURE_ROOT/cas-raced"
fi

exec "$real_git" "${git_prefix[@]}" "$subcommand" "$@"
"#,
        );
        let old_path = std::env::var_os("PATH").unwrap_or_default();
        let path_value =
            std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&old_path)))
                .unwrap_or_else(|error| panic!("join PATH: {error}"));
        unsafe {
            std::env::set_var("CRUISE_FIXTURE_REAL_GIT", &*HOST_GIT);
            std::env::set_var("CRUISE_FIXTURE_ORIGIN", &origin);
            std::env::set_var(
                "CRUISE_FIXTURE_PUSH_URL",
                "https://github.com/acme/repo.git",
            );
            std::env::set_var("CRUISE_FIXTURE_ROOT", path);
            std::env::set_var("CRUISE_FIXTURE_MODE", &mode);
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
            std::env::set_var("PATH", path_value);
        };
        world.mode = mode;
        world.limit = limit;
        world.root = Some(root);
    }

    #[when("the production after-PR review handler runs")]
    async fn execute(world: &mut ReviewWorld) {
        let root = world
            .root
            .as_ref()
            .unwrap_or_else(|| panic!("fixture"))
            .path();
        let repo = root.join("repo");
        let allow_commit = matches!(
            world.mode.as_str(),
            "host-commit"
                | "host-no-changes"
                | "remote-advanced"
                | "dirty-baseline"
                | "detached-head"
                | "wrong-branch"
                | "wrong-tracking"
                | "wrong-head"
                | "invalid-response"
                | "ineligible-response"
                | "external-filter"
                | "in-progress-merge"
                | "permission-revoked"
                | "write-race"
                | "commit-cas-race"
        );
        let yaml = format!(
            "steps:\n  main:\n    prompt: unused\nafter-pr:\n  review:\n    github-review:\n      reviewers: all\n      max-iterations: {}\n    prompt: address feedback\n    timeout: 30\n    allow_commit: {}\n",
            world.limit, allow_commit
        );
        let mut config = WorkflowConfig::from_yaml(&yaml)
            .unwrap_or_else(|error| panic!("workflow YAML: {error}"));
        config.command = vec![root.join("bin/prompt").to_string_lossy().into_owned()];
        validate_config(&config).unwrap_or_else(|error| panic!("validate workflow: {error}"));
        let compiled = cruise::workflow::compile(config)
            .unwrap_or_else(|error| panic!("compile workflow: {error}"));
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
            cruise::worktree_pr::handle_worktree_pr_with_checkpoint(
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
                None,
                None,
                None,
                &|_, _| Ok(()),
            )
            .await,
        );
    }

    #[then("the review loop succeeds without a prompt")]
    fn empty(world: &mut ReviewWorld) {
        assert!(
            world
                .result
                .as_ref()
                .unwrap_or_else(|| panic!("result"))
                .is_ok(),
            "{:?}",
            world.result
        );
        assert!(
            !world
                .root
                .as_ref()
                .unwrap_or_else(|| panic!("fixture"))
                .path()
                .join("prompt")
                .exists()
        );
    }
    #[then("the review loop succeeds after the prompt")]
    fn complete(world: &mut ReviewWorld) {
        assert!(
            world
                .result
                .as_ref()
                .unwrap_or_else(|| panic!("result"))
                .is_ok(),
            "{:?}",
            world.result
        );
        assert!(
            world
                .root
                .as_ref()
                .unwrap_or_else(|| panic!("fixture"))
                .path()
                .join("prompt")
                .exists()
        );
    }
    #[then(expr = "the review loop fails for {string}")]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Cucumber captures owned strings"
    )]
    fn failure(world: &mut ReviewWorld, expected: String) {
        match world.result.as_ref().unwrap_or_else(|| panic!("result")) {
            Err(CruiseError::GitHubReviewFailed { detail, .. }) => {
                assert!(detail.contains(&expected), "{detail}");
            }
            result => panic!("expected GitHubReviewFailed, got {result:?}"),
        }
    }
    #[then("the prompt ran in a clean git repository")]
    fn invoked(world: &mut ReviewWorld) {
        let root = world
            .root
            .as_ref()
            .unwrap_or_else(|| panic!("fixture"))
            .path();
        assert!(root.join("prompt").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("prompt-status"))
                .unwrap_or_else(|error| panic!("read prompt status: {error}")),
            ""
        );
        let git = std::process::Command::new("git")
            .args(["status", "--porcelain", "--untracked-files=all"])
            .current_dir(root.join("repo"))
            .output()
            .unwrap_or_else(|error| panic!("git status: {error}"));
        assert!(
            git.status.success() && git.stdout.is_empty(),
            "{}",
            String::from_utf8_lossy(&git.stdout)
        );
        let gh = std::fs::read_to_string(root.join("gh.log"))
            .unwrap_or_else(|error| panic!("read gh log: {error}"));
        assert!(
            gh.contains("reviewThreads") && gh.contains("collaborators/writer/permission"),
            "{gh}"
        );
    }

    #[then("the host committed and pushed only the validated prompt edits without hooks")]
    fn host_committed(world: &mut ReviewWorld) {
        let root = world
            .root
            .as_ref()
            .unwrap_or_else(|| panic!("fixture"))
            .path();
        let repo = root.join("repo");
        let initial = std::fs::read_to_string(root.join("initial-head.txt"))
            .unwrap_or_else(|error| panic!("read initial HEAD: {error}"));
        let head = git_ok(&repo, &["rev-parse", "HEAD"]);
        assert_ne!(head, initial.trim());
        assert_eq!(
            git_ok(&repo, &["log", "-1", "--format=%s"]),
            "fix: address pull request review feedback"
        );
        assert_eq!(git_ok(&repo, &["rev-list", "--count", "HEAD"]), "2");
        assert_eq!(
            git_ok(&repo, &["status", "--porcelain", "--untracked-files=all"]),
            ""
        );
        assert_eq!(
            git_ok(
                &root.join("origin.git"),
                &["rev-parse", "refs/heads/feature"]
            ),
            head
        );
        assert!(!root.join("hook-ran").exists());
        assert_eq!(
            std::fs::read_to_string(repo.join("tracked.txt"))
                .unwrap_or_else(|error| panic!("read edited tracked file: {error}")),
            "host edit\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("review file.txt"))
                .unwrap_or_else(|error| panic!("read committed new file: {error}")),
            "new file\n"
        );
        assert!(
            std::fs::read_to_string(root.join("api-writes.log"))
                .unwrap_or_else(|error| panic!("read applied review writes: {error}"))
                .contains("addPullRequestReviewThreadReply")
        );
        assert_eq!(
            std::fs::read_to_string(root.join("agent-commit-state"))
                .unwrap_or_else(|error| panic!("read prompt commit attempt: {error}"))
                .trim(),
            "blocked"
        );
        assert!(
            std::fs::read_to_string(root.join("prompt-agent-commit.log"))
                .unwrap_or_else(|error| panic!("read prompt commit guard output: {error}"))
                .contains("cruise commit guard")
        );
        assert_eq!(
            std::fs::read_to_string(root.join("push-options.txt"))
                .unwrap_or_else(|error| panic!("read push options: {error}"))
                .trim(),
            "0"
        );
        assert!(
            !git(
                &root.join("origin.git"),
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    "refs/tags/review-only-tag",
                ],
            )
            .status
            .success(),
            "host push must not follow tags"
        );
    }

    #[then("the host completed a no-change turn without a commit")]
    fn host_no_change(world: &mut ReviewWorld) {
        let root = world
            .root
            .as_ref()
            .unwrap_or_else(|| panic!("fixture"))
            .path();
        let initial = std::fs::read_to_string(root.join("initial-head.txt"))
            .unwrap_or_else(|error| panic!("read initial HEAD: {error}"));
        let initial = initial.trim();
        assert_eq!(git_ok(&root.join("repo"), &["rev-parse", "HEAD"]), initial);
        assert_eq!(
            git_ok(
                &root.join("origin.git"),
                &["rev-parse", "refs/heads/feature"]
            ),
            initial
        );
        assert_eq!(
            git_ok(&root.join("repo"), &["rev-list", "--count", "HEAD"]),
            "1"
        );
    }

    #[then("the host did not overwrite the advanced PR head")]
    fn host_preserved_advanced_head(world: &mut ReviewWorld) {
        let root = world
            .root
            .as_ref()
            .unwrap_or_else(|| panic!("fixture"))
            .path();
        let repo = root.join("repo");
        let initial = std::fs::read_to_string(root.join("initial-head.txt"))
            .unwrap_or_else(|error| panic!("read initial HEAD: {error}"));
        let advanced = std::fs::read_to_string(root.join("race-head.txt"))
            .unwrap_or_else(|error| panic!("read concurrent HEAD: {error}"));
        let head = git_ok(&repo, &["rev-parse", "HEAD"]);
        assert_ne!(head, initial.trim());
        assert_eq!(
            git_ok(
                &root.join("origin.git"),
                &["rev-parse", "refs/heads/feature"]
            ),
            advanced.trim()
        );
        assert_eq!(
            git_ok(&repo, &["log", "-1", "--format=%s"]),
            "fix: address pull request review feedback"
        );
        assert_eq!(
            git_ok(&repo, &["status", "--porcelain", "--untracked-files=all"]),
            ""
        );
        assert!(!root.join("api-writes.log").exists());
        assert!(!root.join("hook-ran").exists());
    }

    #[then("the host refused the edit turn before running the prompt")]
    fn refused_before_prompt(world: &mut ReviewWorld) {
        assert!(
            matches!(
                world.result.as_ref().unwrap_or_else(|| panic!("result")),
                Err(CruiseError::GitHubReviewFailed { .. })
            ),
            "expected GitHubReviewFailed, got {:?}",
            world.result
        );
        let root = world
            .root
            .as_ref()
            .unwrap_or_else(|| panic!("fixture"))
            .path();
        let initial = std::fs::read_to_string(root.join("initial-head.txt"))
            .unwrap_or_else(|error| panic!("read initial HEAD: {error}"));
        assert!(!root.join("prompt").exists());
        assert_eq!(
            git_ok(&root.join("repo"), &["rev-parse", "HEAD"]),
            initial.trim()
        );
        let expected_remote = if world.mode == "wrong-head" {
            std::fs::read_to_string(root.join("race-head.txt"))
                .unwrap_or_else(|error| panic!("read advanced HEAD: {error}"))
        } else {
            initial
        };
        assert_eq!(
            git_ok(
                &root.join("origin.git"),
                &["rev-parse", "refs/heads/feature"]
            ),
            expected_remote.trim()
        );
        if world.mode == "dirty-baseline" {
            assert_eq!(
                std::fs::read_to_string(root.join("repo/dirty.txt"))
                    .unwrap_or_else(|error| panic!("read dirty baseline: {error}")),
                "already dirty\n"
            );
        }
    }
    #[then("the host published edits but withheld the incomplete review batch")]
    fn published_without_review_writes(world: &mut ReviewWorld) {
        let root = world
            .root
            .as_ref()
            .unwrap_or_else(|| panic!("fixture"))
            .path();
        let repo = root.join("repo");
        let head = git_ok(&repo, &["rev-parse", "HEAD"]);
        assert_ne!(
            head,
            std::fs::read_to_string(root.join("initial-head.txt"))
                .unwrap_or_else(|error| panic!("read initial HEAD: {error}"))
                .trim()
        );
        assert_eq!(
            git_ok(
                &root.join("origin.git"),
                &["rev-parse", "refs/heads/feature"]
            ),
            head
        );
        assert_eq!(
            git_ok(&repo, &["log", "-1", "--format=%s"]),
            "fix: address pull request review feedback"
        );
        assert_eq!(
            git_ok(&repo, &["status", "--porcelain", "--untracked-files=all"]),
            ""
        );
        assert!(!root.join("api-writes.log").exists());
    }

    #[then("the reply-only turn did not write after the PR head changed")]
    fn reply_race_withheld(world: &mut ReviewWorld) {
        let root = world
            .root
            .as_ref()
            .unwrap_or_else(|| panic!("fixture"))
            .path();
        let initial = std::fs::read_to_string(root.join("initial-head.txt"))
            .unwrap_or_else(|error| panic!("read initial HEAD: {error}"));
        let advanced = std::fs::read_to_string(root.join("race-head.txt"))
            .unwrap_or_else(|error| panic!("read advanced HEAD: {error}"));
        assert!(root.join("prompt").exists());
        assert_eq!(
            git_ok(&root.join("repo"), &["rev-parse", "HEAD"]),
            initial.trim()
        );
        assert_eq!(
            git_ok(
                &root.join("origin.git"),
                &["rev-parse", "refs/heads/feature"]
            ),
            advanced.trim()
        );
        assert!(!root.join("api-writes.log").exists());
    }

    #[then("the concurrent local branch update was never published")]
    fn cas_race_withheld(world: &mut ReviewWorld) {
        let root = world
            .root
            .as_ref()
            .unwrap_or_else(|| panic!("fixture"))
            .path();
        let initial = std::fs::read_to_string(root.join("initial-head.txt"))
            .unwrap_or_else(|error| panic!("read initial HEAD: {error}"));
        let repo = root.join("repo");
        assert!(root.join("cas-raced").exists());
        assert!(!root.join("host-pushed").exists());
        assert_eq!(
            git_ok(&repo, &["log", "-1", "--format=%s"]),
            "concurrent sibling"
        );
        assert_eq!(
            git_ok(
                &root.join("origin.git"),
                &["rev-parse", "refs/heads/feature"]
            ),
            initial.trim()
        );
        assert!(!root.join("api-writes.log").exists());
    }

    #[then("the configured filter was not executed")]
    fn filter_not_executed(world: &mut ReviewWorld) {
        let root = world
            .root
            .as_ref()
            .unwrap_or_else(|| panic!("fixture"))
            .path();
        assert!(!root.join("prompt").exists());
        assert!(!root.join("filter-ran").exists());
    }

    #[then("the prompt edits remain uncommitted")]
    fn edits_remain_uncommitted(world: &mut ReviewWorld) {
        let root = world
            .root
            .as_ref()
            .unwrap_or_else(|| panic!("fixture"))
            .path();
        let initial = std::fs::read_to_string(root.join("initial-head.txt"))
            .unwrap_or_else(|error| panic!("read initial HEAD: {error}"));
        let repo = root.join("repo");
        assert!(root.join("prompt").exists());
        assert_eq!(git_ok(&repo, &["rev-parse", "HEAD"]), initial.trim());
        assert_ne!(
            git_ok(&repo, &["status", "--porcelain", "--untracked-files=all"]),
            ""
        );
        assert_eq!(
            git_ok(
                &root.join("origin.git"),
                &["rev-parse", "refs/heads/feature"]
            ),
            initial.trim()
        );
        assert!(!root.join("api-writes.log").exists());
    }

    #[then(expr = "the prompt ran {int} times in a clean git repository")]
    fn invoked_times(world: &mut ReviewWorld, count: usize) {
        invoked(world);
        let root = world
            .root
            .as_ref()
            .unwrap_or_else(|| panic!("fixture"))
            .path();
        assert_eq!(
            std::fs::read_to_string(root.join("prompt"))
                .unwrap_or_else(|error| panic!("read prompt: {error}"))
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
                .unwrap_or_else(|error| panic!("parse feature: {error}"));
        assert_eq!(feature.scenarios.len(), 23);
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
