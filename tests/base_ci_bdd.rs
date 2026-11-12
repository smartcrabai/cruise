//! Executable base-CI contracts through the production CLI, with no forge network.
#![expect(clippy::unwrap_used, reason = "isolated executable contract fixtures")]

#[cfg(unix)]
mod contracts {
    use cucumber::{World as _, given, then, when};
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;
    use std::path::PathBuf;
    use std::process::{Command, Output, Stdio};

    const SESSION: &str = "20261112000000001";

    fn script(path: &Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[derive(Debug, Default, cucumber::World)]
    struct BaseCiWorld {
        root: Option<tempfile::TempDir>,
        output: Option<Output>,
        original_state: Vec<u8>,
        rollup: String,
        forge: String,
        dry: bool,
        configured_branch: bool,
    }
    impl BaseCiWorld {
        fn path(&self, path: &str) -> PathBuf {
            self.root.as_ref().unwrap().path().join(path)
        }
        fn state(&self) -> PathBuf {
            self.path(&format!("data/cruise/sessions/{SESSION}/state.json"))
        }
        fn terminal(&self) -> String {
            let output = self.output.as_ref().unwrap();
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        }
        fn run(&mut self, dry: bool) {
            self.dry = dry;
            let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
            command
                .env_clear()
                .current_dir(self.path("repo"))
                .env(
                    "PATH",
                    std::env::join_paths(std::iter::once(self.path("bin")).chain(
                        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
                    ))
                    .unwrap(),
                )
                .env("HOME", self.path("home"))
                .env("XDG_CONFIG_HOME", self.path("config"))
                .env("XDG_DATA_HOME", self.path("data"))
                .env("XDG_STATE_HOME", self.path("state"))
                .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
                .env("CRUISE_DISABLE_HERDR", "1")
                .env("CRUISE_SKIP_AGENT_AUTH_CHECK", "1")
                .env("CRUISE_UI_LANGUAGE", "en")
                .env("NO_COLOR", "1")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env(
                    "REAL_GIT",
                    std::env::var("BASE_CI_REAL_GIT").unwrap_or_else(|_| "/usr/bin/git".to_owned()),
                )
                .env("GH_LOG", self.path("gh.log"))
                .env("GLAB_LOG", self.path("glab.log"))
                .stdin(Stdio::null())
                .arg("run")
                .arg(SESSION);
            if dry {
                command.arg("--dry-run");
            }
            self.output = Some(command.output().unwrap());
        }
    }

    #[given(expr = "an isolated {word} session with a {word} rollup")]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Cucumber captures owned strings"
    )]
    fn setup(w: &mut BaseCiWorld, forge: String, state: String) {
        w.rollup.clone_from(&state);
        w.forge.clone_from(&forge);
        w.root = Some(tempfile::tempdir().unwrap());
        for dir in ["bin", "repo", "home", "config"] {
            std::fs::create_dir_all(w.path(dir)).unwrap();
        }
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(w.path("repo"))
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(w.path("repo"))
                .args([
                    "remote",
                    "add",
                    "origin",
                    if forge == "GitLab" {
                        "https://gitlab.com/owner/repo.git"
                    } else {
                        "https://github.com/owner/repo.git"
                    }
                ])
                .status()
                .unwrap()
                .success()
        );
        script(
            &w.path("bin/git"),
            "#!/bin/sh\nfor arg in \"$@\"; do case \"$arg\" in fetch|pull|push|clone|ls-remote|remote-https|remote-http|remote-ssh) exit 91;; esac; done\nexec \"$REAL_GIT\" \"$@\"\n",
        );
        let response = if state == "NONE" {
            "null".to_owned()
        } else {
            json!({"state": state}).to_string()
        };
        let result = if state == "ERROR" {
            "echo unavailable >&2; exit 1".to_owned()
        } else {
            format!(
                "echo '{}'",
                json!({"data":{"repository":{"defaultBranchRef":{"name":"main","target":{"oid":"abcdef1234567890abcdef1234567890abcdef12","statusCheckRollup":serde_json::from_str::<serde_json::Value>(&response).unwrap()}},"ref":{"name":"develop","target":{"oid":"abcdef1234567890abcdef1234567890abcdef12","statusCheckRollup":serde_json::from_str::<serde_json::Value>(&response).unwrap()}}}}})
            )
        };
        script(
            &w.path("bin/gh"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$GH_LOG\"\ncase \"$*\" in --version) echo 'gh version 2.0.0';; *graphql*) {result};; *) exit 92;; esac\n"
            ),
        );
        let pipeline = if state == "NONE" {
            json!([])
        } else {
            json!([{"status": state}])
        };
        let commit = json!({"id":"abcdef1234567890abcdef1234567890abcdef12"});
        script(
            &w.path("bin/glab"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$GLAB_LOG\"\ncase \"$*\" in --version) echo 'glab version 1.0.0';; *repository/commits/*) echo '{commit}';; *pipelines\\?*) echo '{pipeline}';; *projects/owner%2Frepo) echo '{{\"default_branch\":\"main\"}}';; *) exit 93;; esac\n"
            ),
        );
        let session_dir = w.state().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&session_dir).unwrap();
        let state = json!({"id":SESSION,"base_dir":w.path("repo"),"phase":"Planned","config":{"kind":"builtin_snapshot"},"input":"","current_step":null,"created_at":"2026-11-12T00:00:00Z","completed_at":null,"worktree_path":null,"worktree_branch":null,"input_as_plan":true});
        w.original_state = serde_json::to_vec(&state).unwrap();
        std::fs::write(w.state(), &w.original_state).unwrap();
        std::fs::write(
            session_dir.join("config.yaml"),
            "command: [cat]\nsteps:\n  test:\n    prompt: do not execute\n",
        )
        .unwrap();
        std::fs::write(session_dir.join("plan.md"), "# Test plan\n").unwrap();
    }
    #[given("the configured base branch is develop")]
    fn configured_branch(w: &mut BaseCiWorld) {
        w.configured_branch = true;
        assert!(
            Command::new("git")
                .current_dir(w.path("repo"))
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.com",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "initial"
                ])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(w.path("repo"))
                .args(["branch", "develop"])
                .status()
                .unwrap()
                .success()
        );
        std::fs::create_dir_all(w.path("repo/.cruise")).unwrap();
        std::fs::write(
            w.path("repo/.cruise/default.yaml"),
            "base_branch: develop\n",
        )
        .unwrap();
    }
    #[then("the configured branch is queried")]
    fn configured_query(w: &mut BaseCiWorld) {
        let log = std::fs::read_to_string(w.path("gh.log")).unwrap();
        assert!(log.contains("ref=refs/heads/develop"), "{log}");
        assert!(log.contains("ref(qualifiedName:$ref)"), "{log}");
    }
    #[given("base CI checking is disabled")]
    fn disable(w: &mut BaseCiWorld) {
        std::fs::create_dir_all(w.path("repo/.cruise")).unwrap();
        std::fs::write(
            w.path("repo/.cruise/default.yaml"),
            "check_base_ci: false\n",
        )
        .unwrap();
    }
    #[when("I start the workflow")]
    fn start(w: &mut BaseCiWorld) {
        w.run(false);
    }
    #[when("I dry run the workflow")]
    fn dry(w: &mut BaseCiWorld) {
        w.run(true);
    }
    #[then("the base CI query is made")]
    fn queried(w: &mut BaseCiWorld) {
        assert!(
            std::fs::read_to_string(w.path(if w.forge == "GitLab" {
                "glab.log"
            } else {
                "gh.log"
            }))
            .unwrap()
            .contains(if w.forge == "GitLab" {
                "projects/owner%2Frepo/repository/commits/main"
            } else {
                "graphql"
            }),
            "{}",
            w.terminal()
        );
    }
    #[then("no base CI query is made")]
    fn not_queried(w: &mut BaseCiWorld) {
        assert!(
            !std::fs::read_to_string(w.path("gh.log"))
                .unwrap_or_default()
                .contains("graphql"),
            "{}",
            w.terminal()
        );
    }
    #[then(expr = "the base CI outcome is {word}")]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Cucumber captures owned strings"
    )]
    fn outcome(w: &mut BaseCiWorld, expected: String) {
        let text = w.terminal();
        if expected == "rejected" {
            assert!(!w.output.as_ref().unwrap().status.success(), "{text}");
            assert!(text.contains("check_base_ci: false"), "{text}");
            assert_eq!(std::fs::read(w.state()).unwrap(), w.original_state);
            assert!(!w.path(&format!("data/cruise/worktrees/{SESSION}")).exists());
            assert!(!w.path(&format!("data/cruise/clones/{SESSION}")).exists());
        } else {
            assert!(!text.contains("check_base_ci: false"), "{text}");
            if w.dry {
                assert!(w.output.as_ref().unwrap().status.success(), "{text}");
                assert!(
                    text.contains("Dry Run: Workflow Flow") && text.contains("test [prompt]"),
                    "{text}"
                );
                assert_eq!(std::fs::read(w.state()).unwrap(), w.original_state);
            } else {
                assert!(text.contains("-> worktree:"), "{text}");
                assert!(text.contains("> test"), "{text}");
                let branch = if w.configured_branch {
                    "develop"
                } else {
                    "main"
                };
                let message = match w.rollup.as_str() {
                    "SUCCESS" | "success" => Some(format!("base CI: {branch} @ abcdef1 passing")),
                    "PENDING" | "running" => Some(format!(
                        "warning: base CI: {branch} @ abcdef1 pending; continuing"
                    )),
                    "NONE" => Some(format!(
                        "warning: base CI: {branch} @ abcdef1 no checks; continuing"
                    )),
                    _ => None,
                };
                if let Some(message) = message {
                    assert!(text.contains(&message), "{text}");
                }
            }
        }
    }
    pub async fn run() {
        use cucumber::writer::Stats as _;
        let writer = BaseCiWorld::cucumber()
            .fail_on_skipped()
            .run("tests/features/base_ci.feature")
            .await;
        assert!(!writer.execution_has_failed(), "BDD execution failed");
        assert!(writer.passed_steps() > 0, "no scenarios executed");
    }
}
#[cfg(unix)]
#[tokio::main]
async fn main() {
    contracts::run().await;
}
#[cfg(not(unix))]
fn main() {
    panic!("base CI BDD specifications require Unix subprocess fixtures");
}
