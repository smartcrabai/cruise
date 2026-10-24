//! Task-grounded OAuth preflight contracts through real production CLI entrypoints.
#![expect(clippy::unwrap_used, reason = "isolated executable contract fixtures")]

#[cfg(unix)]
mod contracts {
    use cucumber::{World as _, given, then, when};
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::PathBuf;
    use std::process::{Command, Output, Stdio};

    const ACCESS: &str = "AUTH_BDD_ACCESS_SECRET_DO_NOT_PRINT";
    const REFRESH: &str = "AUTH_BDD_REFRESH_SECRET_DO_NOT_PRINT";

    #[derive(Default, cucumber::World)]
    pub struct AuthWorld {
        root: Option<tempfile::TempDir>,
        output: Option<Output>,
        credentials: Vec<(PathBuf, Vec<u8>)>,
        optout: bool,
        api: bool,
        dry: bool,
        workspace_baseline: Vec<u8>,
    }
    impl std::fmt::Debug for AuthWorld {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("AuthWorld")
                .field("root", &self.root)
                .finish_non_exhaustive()
        }
    }
    impl AuthWorld {
        fn path(&self, name: &str) -> PathBuf {
            self.root.as_ref().unwrap().path().join(name)
        }
        fn terminal(&self) -> String {
            let o = self.output.as_ref().unwrap();
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )
        }
        fn command(&self) -> Command {
            let mut c = Command::new(env!("CARGO_BIN_EXE_cruise"));
            // Clear the entire inherited environment: no developer credentials, SDK
            // routing, telemetry endpoints or workflow defaults may affect a child.
            c.env_clear()
                .current_dir(self.path("repo"))
                .env(
                    "PATH",
                    std::env::join_paths(std::iter::once(self.path("bin")).chain(
                        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
                    ))
                    .unwrap(),
                )
                .env("HOME", self.path("home"))
                .env("USERPROFILE", self.path("home"))
                .env("XDG_CONFIG_HOME", self.path("config"))
                .env("XDG_DATA_HOME", self.path("data"))
                .env("XDG_STATE_HOME", self.path("state"))
                .env("JCODE_HOME", self.path("jh"))
                .env("CLAUDE_CONFIG_DIR", self.path("claude"))
                .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
                .env("CRUISE_DISABLE_HERDR", "1")
                .env("NO_COLOR", "1")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .stdin(Stdio::null());
            if self.optout {
                c.env("CRUISE_SKIP_AGENT_AUTH_CHECK", "1");
            }
            if self.api {
                c.env("ANTHROPIC_API_KEY", "AUTH_BDD_API_SECRET_DO_NOT_PRINT");
            }
            c
        }
    }
    #[given(expr = "isolated {word} credentials and a {word} workflow")]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Cucumber captures owned strings"
    )]
    fn setup(w: &mut AuthWorld, credentials: String, selection: String) {
        w.root = Some(tempfile::tempdir().unwrap());
        for dir in ["bin", "repo", "home", "jh", "claude"] {
            std::fs::create_dir_all(w.path(dir)).unwrap();
        }
        cruise::test_support::init_git_repo(&w.path("repo"));
        for sdk in ["jcode", "claude"] {
            cruise::test_support::write_executable_script(
                &w.path(&format!("bin/{sdk}")),
                &format!(
                    "#!/bin/sh\nprintf '%s\\n' '{sdk}' >> '{}'\nprintf 'AUTH_BDD_SDK_REFUSAL: API Error: 404 model not found\\n' >&2\nexit 73\n",
                    w.path("dispatch").display()
                ),
            );
        }
        let refresh = if matches!(credentials.as_str(), "refreshable" | "expired-refresh") {
            json!(REFRESH)
        } else {
            serde_json::Value::Null
        };
        let mut jcode = json!({"anthropic_accounts":[{"access":ACCESS,"expires":1,"refresh":refresh}],"openai_accounts":[{"access_token":ACCESS,"expires_at":1,"refresh_token":refresh}]});
        if matches!(credentials.as_str(), "multi-renewable" | "multi-unknown") {
            // The first account is conclusively dead. A second renewable or
            // unknown account must prevent an all-accounts-expired rejection.
            let (anthropic, openai) = if credentials == "multi-renewable" {
                (
                    json!({"access":ACCESS,"expires":1,"refresh":REFRESH}),
                    json!({"access_token":ACCESS,"expires_at":1,"refresh_token":REFRESH}),
                )
            } else {
                (json!({"access":ACCESS}), json!({"access_token":ACCESS}))
            };
            jcode["anthropic_accounts"]
                .as_array_mut()
                .unwrap()
                .push(anthropic);
            jcode["openai_accounts"]
                .as_array_mut()
                .unwrap()
                .push(openai);
        }
        let mut claude =
            json!({"claudeAiOauth":{"accessToken":ACCESS,"expiresAt":1,"refreshToken":refresh}});
        if credentials == "expired-refresh" {
            claude["claudeAiOauth"]["refreshTokenExpiresAt"] = json!(1);
        }
        if credentials == "future-access" {
            claude["claudeAiOauth"]["expiresAt"] = json!(4_102_444_800_000_i64);
            claude["claudeAiOauth"]["refreshToken"] = json!(REFRESH);
            claude["claudeAiOauth"]["refreshTokenExpiresAt"] = json!(1);
        }
        for (name, value) in [
            ("jh/auth.json", &jcode),
            ("jh/openai-auth.json", &jcode),
            ("claude/.credentials.json", &claude),
        ] {
            if credentials == "missing" {
                continue;
            }
            let bytes = if credentials == "malformed" {
                b"not-json".to_vec()
            } else {
                serde_json::to_vec(value).unwrap()
            };
            let path = w.path(name);
            std::fs::write(&path, &bytes).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
            w.credentials.push((path, bytes));
        }
        let config = match selection.as_str() {
            "claude" => "sdk: claude\nmodel: bdd-model\n".to_owned(),
            "planning-only" => "sdk: jcode\nmodel: openai-api/bdd-model\nplan_model: claude/bdd-planner\n".to_owned(),
            "execution-only" => "sdk: jcode\nmodel: claude/bdd-model\nplan_model: openai-api/bdd-planner\n".to_owned(),
            "mixed" => "sdk: jcode\nmodel:\n  - openai-api/bdd-model\n  - sdk: claude\n    model: bdd-fallback\n".to_owned(),
            "both-expired" => "sdk: jcode\nmodel:\n  - claude/bdd-main\n  - sdk: claude\n    model: bdd-fallback\n  - claude/bdd-main-duplicate-target\n  - sdk: claude\n    model: bdd-fallback-duplicate-target\n".to_owned(),
            "openai" => "sdk: jcode\nmodel: openai/bdd-model\n".to_owned(),
            "api-env" => "sdk: claude\nmodel: bdd-model\nenv:\n  ANTHROPIC_API_KEY: AUTH_BDD_API_SECRET_DO_NOT_PRINT\n".to_owned(),
            _ => "sdk: jcode\nmodel: claude/bdd-model\n".to_owned(),
        };
        let yaml = match selection.as_str() {
            "parallel" => "sdk: jcode\nmodel: openai-api/bdd-main\nsteps:\n  batch:\n    parallel:\n      child:\n        prompt: AUTH_BDD_REQUEST\n        model: claude/bdd-nested\n".to_owned(),
            "group" => "sdk: jcode\nmodel: openai-api/bdd-main\ngroups:\n  nested:\n    steps:\n      child:\n        prompt: AUTH_BDD_REQUEST\n        model: claude/bdd-nested\nsteps:\n  answer:\n    group: nested\n".to_owned(),
            "after-pr" => "sdk: jcode\nmodel: openai-api/bdd-main\nafter-pr:\n  followup:\n    prompt: AUTH_BDD_REQUEST\n    model: claude/bdd-after\nsteps:\n  answer:\n    prompt: AUTH_BDD_REQUEST\n".to_owned(),
            "fallback" => "sdk: jcode\nmodel: openai-api/bdd-main\nretry:\n  fallback_chains:\n    default:\n      - claude/bdd-fallback\nsteps:\n  answer:\n    prompt: AUTH_BDD_REQUEST\n".to_owned(),
            _ => format!("{config}retry:\n  base_delay_ms: 1\n  max_delay_ms: 1\nsteps:\n  answer:\n    prompt: AUTH_BDD_REQUEST\n"),
        };
        std::fs::write(w.path("repo/workflow.yaml"), yaml).unwrap();
    }
    #[given("the auth preflight optout is enabled")]
    fn optout(w: &mut AuthWorld) {
        w.optout = true;
    }
    #[given("Claude API authentication is present in the child environment")]
    fn api(w: &mut AuthWorld) {
        w.api = true;
    }
    #[given("dry run is requested")]
    fn dry(w: &mut AuthWorld) {
        w.dry = true;
    }
    #[when(expr = "the production CLI enters {word}")]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Cucumber captures owned strings"
    )]
    fn run(w: &mut AuthWorld, entry: String) {
        if entry == "run" {
            // Execution contracts begin with an already approved session, not
            // a planning call that would itself require usable title auth.
            let manager = cruise::session::SessionManager::new(w.path("data/cruise"));
            let mut session = cruise::session::SessionState::new(
                "20261024000005".to_string(),
                w.path("repo"),
                cruise::session_config::SessionConfigRef::BuiltinSnapshot { name: None },
                "AUTH_BDD_REQUEST".to_string(),
            );
            session.phase = cruise::session::SessionPhase::Planned;
            manager.create(&session).unwrap();
            let dir = manager.sessions_dir().join(&session.id);
            std::fs::copy(w.path("repo/workflow.yaml"), dir.join("config.yaml")).unwrap();
            std::fs::write(dir.join("plan.md"), "# AUTH_BDD_REQUEST\n").unwrap();
        }
        w.workspace_baseline = Command::new("git")
            .current_dir(w.path("repo"))
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap()
            .stdout;
        let mut c = w.command();
        c.arg(&entry);
        if entry == "run" {
            let sessions = std::fs::read_dir(w.path("data/cruise/sessions")).unwrap();
            let ids = sessions
                .map(|e| e.unwrap())
                .filter(|e| e.path().is_dir())
                .collect::<Vec<_>>();
            assert_eq!(ids.len(), 1, "expected one planned session");
            c.arg(ids[0].file_name());
        } else {
            c.args(["--config", "workflow.yaml", "AUTH_BDD_REQUEST"]);
            if entry == "plan" {
                c.arg("--no-interactive-planning");
            }
        }
        if w.dry {
            c.arg("--dry-run");
        }
        c.args(["--rate-limit-retries", "0"]);
        w.output = Some(c.output().unwrap());
    }
    #[then("expired authentication stops before SDK dispatch and workspace creation")]
    fn rejected(w: &mut AuthWorld) {
        let text = w.terminal();
        assert!(!w.output.as_ref().unwrap().status.success(), "{text}");
        assert!(
            text.contains("agent authentication expired") && text.contains("login"),
            "{text}"
        );
        assert!(!w.path("dispatch").exists(), "SDK launched: {text}");
        let list = Command::new("git")
            .current_dir(w.path("repo"))
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        assert_eq!(
            list.stdout, w.workspace_baseline,
            "execution workspace changed"
        );
    }
    #[then("both expired providers have exactly one actionable login message")]
    fn both_expired(w: &mut AuthWorld) {
        let text = w.terminal();
        for advice in ["run `jcode login claude`", "run `claude auth login`"] {
            assert_eq!(text.matches(advice).count(), 1, "{text}");
        }
        for target in ["jcode provider `claude`", "claude CLI"] {
            assert_eq!(text.matches(target).count(), 1, "{text}");
        }
    }
    #[then("preflight passes to the observable refusing SDK boundary")]
    fn passed(w: &mut AuthWorld) {
        let text = w.terminal();
        assert!(
            !w.output.as_ref().unwrap().status.success(),
            "fixture must refuse"
        );
        assert!(w.path("dispatch").exists(), "SDK never reached: {text}");
        assert!(text.contains("AUTH_BDD_SDK_REFUSAL"), "{text}");
        assert!(!text.contains("agent authentication expired"), "{text}");
    }
    #[then("dry run succeeds without dispatch or authentication rejection")]
    fn dry_passed(w: &mut AuthWorld) {
        assert!(
            w.output.as_ref().unwrap().status.success(),
            "{}",
            w.terminal()
        );
        assert!(!w.path("dispatch").exists());
        assert!(!w.terminal().contains("agent authentication expired"));
    }
    #[then("credentials remain byte identical without locks and secrets stay private")]
    fn private(w: &mut AuthWorld) {
        for (path, bytes) in &w.credentials {
            assert!(
                std::fs::read(path).unwrap() == *bytes,
                "credential file changed"
            );
        }
        for dir in ["jh", "claude"] {
            let files = std::fs::read_dir(w.path(dir))
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect::<Vec<_>>();
            assert!(
                files
                    .iter()
                    .all(|p| w.credentials.iter().any(|(known, _)| known == p)
                        || (p.is_dir() && p.file_name().unwrap() == ".cruise-sdk-sessions")),
                "credentials directory changed: {files:?}"
            );
        }
        let text = w.terminal();
        for secret in [ACCESS, REFRESH, "AUTH_BDD_API_SECRET_DO_NOT_PRINT"] {
            assert!(!text.contains(secret), "secret exposed");
        }
    }
    #[then("the deliberately pending auth contract is implemented")]
    fn pending(_: &mut AuthWorld) {
        panic!("AUTH_BDD_PENDING: contract deliberately unimplemented");
    }

    pub async fn main() {
        use cucumber::writer::Stats as _;
        let writer = AuthWorld::cucumber()
            .max_concurrent_scenarios(1)
            .fail_on_skipped()
            .run(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/features/agent_auth.feature"
            ))
            .await;
        assert!(
            !writer.execution_has_failed(),
            "{} step failed, {} parsing errors, {} hook errors",
            writer.failed_steps(),
            writer.parsing_errors(),
            writer.hook_errors()
        );
        assert!(
            writer.passed_steps() + writer.failed_steps() > 0,
            "no agent auth scenarios executed"
        );
    }
}
#[cfg(unix)]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    contracts::main().await;
}
#[cfg(not(unix))]
fn main() {
    panic!("agent_auth_bdd requires Unix subprocess fixtures");
}
