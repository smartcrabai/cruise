#![cfg(unix)]
//! End-to-end contract for `cruise exec --flag`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

const WORKFLOW: &str = "command: [echo]
flags:
  e2e:
    description: Run E2E
  docs: {}
steps:
  build:
    command: touch build.txt
  e2e-test:
    flag: e2e
    command: touch e2e.txt
  docs-test:
    flag: docs
    command: touch docs.txt
  any-test:
    flag: [e2e, docs]
    command: touch any.txt
";

struct Fixture {
    root: TempDir,
    repo: PathBuf,
    config: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git command: {error}"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap_or_else(|error| {
            panic!("failed to create temporary fixture directory: {error}")
        });
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap_or_else(|error| {
            panic!("failed to create fixture repository directory: {error}")
        });
        git(&repo, &["init", "-q", "-b", "main"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ],
        );
        let config = root.path().join("workflow.yaml");
        std::fs::write(&config, WORKFLOW)
            .unwrap_or_else(|error| panic!("failed to write fixture workflow config: {error}"));
        Self { root, repo, config }
    }

    fn exec(&self, extra: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(name);
            }
        }
        command
            .arg("exec")
            .arg("--config")
            .arg(&self.config)
            .args(extra)
            .current_dir(&self.repo)
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env_remove("HERDR_ENV")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
            .output()
            .unwrap_or_else(|error| panic!("failed to run cruise exec command: {error}"))
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn exec_without_flags_runs_unflagged_steps_and_skips_flagged_side_effects() {
    let f = Fixture::new();
    let out = f.exec(&[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("build.txt").exists());
    assert!(!f.repo.join("e2e.txt").exists());
    assert!(!f.repo.join("docs.txt").exists());
    assert!(!f.repo.join("any.txt").exists());
}

#[test]
fn exec_accepts_a_bare_flag_name() {
    let f = Fixture::new();
    let out = f.exec(&["--flag", "e2e"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("e2e.txt").exists());
    assert!(f.repo.join("any.txt").exists());
    assert!(!f.repo.join("docs.txt").exists());
}

#[test]
fn exec_enables_any_flag_list_member_without_enabling_others() {
    let f = Fixture::new();
    let out = f.exec(&["-f", "docs"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("docs.txt").exists());
    assert!(f.repo.join("any.txt").exists());
    assert!(!f.repo.join("e2e.txt").exists());
}

#[test]
fn exec_accepts_a_comma_separated_flag_list() {
    let f = Fixture::new();
    let out = f.exec(&["-f", "e2e,docs"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("e2e.txt").exists());
    assert!(f.repo.join("docs.txt").exists());
    assert!(f.repo.join("any.txt").exists());
}

#[test]
fn exec_accepts_repeated_flag_arguments() {
    let f = Fixture::new();
    let out = f.exec(&["--flag", "e2e", "-f", "docs"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("e2e.txt").exists());
    assert!(f.repo.join("docs.txt").exists());
    assert!(f.repo.join("any.txt").exists());
}

#[test]
fn exec_accepts_explicit_off_in_a_comma_separated_list() {
    let f = Fixture::new();
    let out = f.exec(&["-f", "e2e,docs=off"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("e2e.txt").exists());
    assert!(f.repo.join("any.txt").exists());
    assert!(!f.repo.join("docs.txt").exists());
}

#[test]
fn exec_explicit_off_skips_the_flagged_step() {
    let f = Fixture::new();
    let out = f.exec(&["--flag", "e2e=off"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("build.txt").exists());
    assert!(!f.repo.join("e2e.txt").exists());
    assert!(!f.repo.join("any.txt").exists());
}

#[test]
fn exec_rejects_malformed_and_empty_flag_values() {
    for invalid in ["e2e=maybe", "e2e=", "e2e,,docs", ",e2e", "e2e,", ""] {
        let f = Fixture::new();
        let out = f.exec(&["--flag", invalid]);
        let message = text(&out);
        assert!(!out.status.success(), "{invalid:?}: {message}");
        assert!(!f.repo.join("build.txt").exists(), "{invalid:?}: {message}");
    }
}

#[test]
fn exec_rejects_duplicate_flag_names() {
    let f = Fixture::new();
    let out = f.exec(&["--flag", "e2e,e2e"]);
    let message = text(&out);
    assert!(!out.status.success(), "{message}");
    assert!(message.contains("more than once"), "{message}");
    assert!(!f.repo.join("build.txt").exists());
}

#[test]
fn exec_rejects_an_undeclared_flag_name() {
    let f = Fixture::new();
    let out = f.exec(&["--flag", "e2e,ghost"]);
    let message = text(&out);
    assert!(!out.status.success(), "{message}");
    assert!(message.contains("ghost"), "{message}");
    assert!(!f.repo.join("build.txt").exists());
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod interactive {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::process::CommandExt;
    use std::process::Child;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const FLAG_PROMPT: &str = "Which optional workflow flags should be enabled?";

    const GATED_EXIT_WORKFLOW: &str = "command: [echo]
flags:
  e2e:
    description: Run E2E
steps:
  gate:
    flag: e2e
    command: touch gate.txt
    next: finish
  loop:
    command: touch loop.txt
    next: gate
  finish:
    command: touch finish.txt
";

    #[test]
    fn exec_selects_flags_before_validating_a_gated_cycle_exit() {
        assert_gated_exit_selection(false);
    }

    #[test]
    fn force_exec_plan_selects_flags_before_validating_a_gated_cycle_exit() {
        assert_gated_exit_selection(true);
    }

    fn assert_gated_exit_selection(force_exec: bool) {
        for enabled in [true, false] {
            let f = Fixture::new();
            let yaml = format!("force_exec: {force_exec}\n{GATED_EXIT_WORKFLOW}");
            std::fs::write(&f.config, &yaml)
                .unwrap_or_else(|error| panic!("write gated-exit config: {error}"));
            let keys: &[u8] = if enabled { b" \r" } else { b"\r" };
            let (success, text) = if force_exec {
                std::fs::write(f.repo.join("cruise.yaml"), &yaml)
                    .unwrap_or_else(|error| panic!("write force-exec config: {error}"));
                cli_in_pty(
                    &f,
                    &["plan", "task"],
                    &[("./cruise.yaml", b"\r"), ("Run E2E", keys)],
                )
            } else {
                exec_in_pty(&f, &[], Some(keys))
            };
            assert!(text.contains(FLAG_PROMPT), "{text}");
            assert_eq!(success, enabled, "{text}");
            assert_eq!(f.repo.join("gate.txt").exists(), enabled, "{text}");
            assert_eq!(f.repo.join("finish.txt").exists(), enabled, "{text}");
            assert!(!f.repo.join("loop.txt").exists(), "{text}");
            if !enabled {
                assert!(
                    text.contains("has no normal exit and reaches a cycle"),
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn exec_gated_cycle_dry_run_rejects_without_prompting() {
        let f = Fixture::new();
        std::fs::write(&f.config, GATED_EXIT_WORKFLOW)
            .unwrap_or_else(|error| panic!("write gated-exit config: {error}"));
        let (success, text) = exec_in_pty(&f, &["--dry-run"], None);
        assert!(!success, "{text}");
        assert!(
            text.contains("has no normal exit and reaches a cycle"),
            "{text}"
        );
        assert!(!text.contains(FLAG_PROMPT), "{text}");
        for file in ["gate.txt", "loop.txt", "finish.txt"] {
            assert!(!f.repo.join(file).exists(), "{text}");
        }
    }

    // Kill the whole script process group on failure, including a cruise process
    // still waiting for terminal input. Never join a potentially blocked reader.
    struct PtyChild(Child);

    impl Drop for PtyChild {
        fn drop(&mut self) {
            // forkpty may put cruise in its own session, outside script's
            // process group. Terminate direct children before reaping script.
            let _ = Command::new("pkill")
                .args(["-KILL", "-P", &self.0.id().to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", self.0.id())])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn read_stream(mut stream: impl Read + Send + 'static, tx: mpsc::SyncSender<Vec<u8>>) {
        std::thread::spawn(move || {
            let mut bytes = [0; 4096];
            loop {
                match stream.read(&mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(bytes[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }

    fn exec_in_pty(f: &Fixture, extra: &[&str], response: Option<&[u8]>) -> (bool, String) {
        let config = f.config.to_string_lossy();
        let mut args = vec!["exec", "--config", &config];
        args.extend_from_slice(extra);
        let responses: Vec<_> = response.into_iter().map(|keys| ("Run E2E", keys)).collect();
        cli_in_pty(f, &args, &responses)
    }

    fn pty_command(args: &[&str]) -> Command {
        let args = std::iter::once(env!("CARGO_BIN_EXE_cruise")).chain(args.iter().copied());
        let mut command = Command::new("script");
        #[cfg(target_os = "macos")]
        command.args(["-q", "/dev/null"]).args(args);
        #[cfg(target_os = "linux")]
        {
            let line = args
                .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
                .collect::<Vec<_>>()
                .join(" ");
            command.args(["-q", "-e", "-c", &line, "/dev/null"]);
        }
        command
    }

    fn cli_in_pty(f: &Fixture, args: &[&str], responses: &[(&str, &[u8])]) -> (bool, String) {
        let mut command = pty_command(args);
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(name);
            }
        }
        let mut child = PtyChild(
            command
                .current_dir(&f.repo)
                .env("XDG_CONFIG_HOME", f.root.path().join("config"))
                .env("XDG_DATA_HOME", f.root.path().join("data"))
                .env("XDG_STATE_HOME", f.root.path().join("state"))
                .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
                .env_remove("HERDR_ENV")
                .env("TERM", "xterm-256color")
                .env("NO_COLOR", "1")
                .process_group(0)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap_or_else(|error| panic!("failed to spawn PTY: {error}")),
        );
        let mut input = child
            .0
            .stdin
            .take()
            .unwrap_or_else(|| panic!("missing input"));
        let (tx, rx) = mpsc::sync_channel(32);
        read_stream(
            child
                .0
                .stdout
                .take()
                .unwrap_or_else(|| panic!("missing stdout")),
            tx.clone(),
        );
        read_stream(
            child
                .0
                .stderr
                .take()
                .unwrap_or_else(|| panic!("missing stderr")),
            tx,
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut transcript = Vec::new();
        let mut response_index = 0;
        let mut response_start = 0;
        let mut cursor_queries = 0;
        let mut status = None;
        loop {
            assert!(
                Instant::now() < deadline,
                "PTY timed out: {}",
                String::from_utf8_lossy(&transcript)
            );
            match rx.recv_timeout(Duration::from_millis(25)) {
                Ok(bytes) => transcript.extend(bytes),
                Err(mpsc::RecvTimeoutError::Disconnected) if status.is_some() => break,
                Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {}
            }
            assert!(
                transcript.len() < 2 * 1024 * 1024,
                "PTY transcript exceeded 2 MiB"
            );
            let text = String::from_utf8_lossy(&transcript);
            // script is not a terminal emulator. Answer cursor-position requests
            // made by the prompt renderer rather than waiting for its timeout.
            let queries = text.matches("\x1b[6n").count();
            for _ in cursor_queries..queries {
                input
                    .write_all(b"\x1b[1;1R")
                    .unwrap_or_else(|error| panic!("cursor response: {error}"));
            }
            cursor_queries = queries;
            if let Some((marker, response)) = responses.get(response_index)
                && transcript[response_start..]
                    .windows(marker.len())
                    .any(|bytes| bytes == marker.as_bytes())
            {
                input
                    .write_all(response)
                    .unwrap_or_else(|error| panic!("prompt response: {error}"));
                input
                    .flush()
                    .unwrap_or_else(|error| panic!("flush prompt response: {error}"));
                response_index += 1;
                response_start = transcript.len();
            }
            if status.is_none() {
                status = child
                    .0
                    .try_wait()
                    .unwrap_or_else(|error| panic!("poll PTY: {error}"));
            }
        }
        (
            status.is_some_and(|status| status.success()),
            String::from_utf8_lossy(&transcript).into_owned(),
        )
    }

    #[test]
    fn exec_interactively_selects_e2e_and_runs_only_selected_flag() {
        let f = Fixture::new();
        // Flags retain declaration order: e2e, then docs.
        let (success, text) = exec_in_pty(&f, &[], Some(b" \r"));
        assert!(success, "{text}");
        assert!(text.contains(FLAG_PROMPT), "{text}");
        assert!(f.repo.join("build.txt").exists(), "{text}");
        assert!(f.repo.join("e2e.txt").exists(), "{text}");
        assert!(f.repo.join("any.txt").exists(), "{text}");
        assert!(!f.repo.join("docs.txt").exists(), "{text}");
    }

    #[test]
    fn exec_explicit_docs_flag_suppresses_interactive_selection() {
        let f = Fixture::new();
        // Supply no input, so a regression cannot accidentally accept defaults.
        let (success, text) = exec_in_pty(&f, &["-f", "docs"], None);
        assert!(success, "{text}");
        assert!(!text.contains(FLAG_PROMPT), "{text}");
        assert!(f.repo.join("docs.txt").exists(), "{text}");
        assert!(f.repo.join("any.txt").exists(), "{text}");
        assert!(!f.repo.join("e2e.txt").exists(), "{text}");
    }

    fn planned_session(f: &Fixture, with_plan: bool) -> (cruise::session::SessionManager, String) {
        use cruise::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode};
        use cruise::session_config::SessionConfigRef;

        let manager = SessionManager::new(f.root.path().join("data/cruise"));
        let id = "20261024000000".to_owned();
        let mut state = SessionState::new(
            id.clone(),
            f.repo.clone(),
            SessionConfigRef::File {
                path: f.config.clone(),
            },
            "saved plan".to_owned(),
        );
        state.phase = SessionPhase::Planned;
        state.workspace_mode = WorkspaceMode::CurrentBranch;
        state.flags.insert("docs".to_owned(), true);
        manager
            .create(&state)
            .unwrap_or_else(|error| panic!("create session: {error}"));
        if with_plan {
            std::fs::write(state.plan_path(&manager.sessions_dir()), "# Saved plan\n")
                .unwrap_or_else(|error| panic!("write plan: {error}"));
        }
        (manager, id)
    }

    #[test]
    fn run_enter_keeps_saved_flag_selection_and_persists_explicit_off_values() {
        let f = Fixture::new();
        let (manager, id) = planned_session(&f, true);
        let (success, text) = cli_in_pty(&f, &["run", &id], &[("Run E2E", b"\r")]);
        assert!(success, "{text}");
        assert!(text.contains(FLAG_PROMPT), "{text}");
        assert!(
            f.repo.join("docs.txt").exists(),
            "saved docs should start selected: {text}"
        );
        assert!(!f.repo.join("e2e.txt").exists(), "{text}");
        let state = manager
            .load(&id)
            .unwrap_or_else(|error| panic!("load session: {error}"));
        assert_eq!(
            state.flags,
            cruise::flags::FlagValues::from([("docs".into(), true), ("e2e".into(), false)])
        );
    }

    #[test]
    fn run_cancelling_flag_selection_keeps_the_planned_session_unchanged() {
        let f = Fixture::new();
        let (manager, id) = planned_session(&f, true);
        let before = manager
            .load(&id)
            .unwrap_or_else(|error| panic!("load session: {error}"));
        let (success, text) = cli_in_pty(&f, &["run", &id], &[("Run E2E", b"\x1b")]);
        assert!(!success, "{text}");
        assert!(text.contains("workflow flag selection cancelled"), "{text}");
        let state = manager
            .load(&id)
            .unwrap_or_else(|error| panic!("load session: {error}"));
        assert_eq!(state.phase, cruise::session::SessionPhase::Planned);
        assert_eq!(state.flags, before.flags);
        assert!(state.current_step.is_none());
        assert!(!f.repo.join("build.txt").exists(), "{text}");
    }

    #[test]
    fn run_without_usable_plan_fails_before_flag_selection() {
        let f = Fixture::new();
        let (manager, id) = planned_session(&f, false);
        let (success, text) = cli_in_pty(&f, &["run", &id], &[]);
        assert!(!success, "{text}");
        assert!(text.contains("no usable plan"), "{text}");
        assert!(!text.contains(FLAG_PROMPT), "{text}");
        assert_eq!(
            manager
                .load(&id)
                .unwrap_or_else(|error| panic!("load session: {error}"))
                .phase,
            cruise::session::SessionPhase::Planned
        );
        assert!(!f.repo.join("build.txt").exists(), "{text}");
    }

    #[test]
    fn plan_cancelling_skip_selection_does_not_retain_pending_flags() {
        let mut f = Fixture::new();
        f.repo = f.root.path().join("plain");
        std::fs::create_dir(&f.repo).unwrap_or_else(|error| panic!("create directory: {error}"));
        let config = f.config.to_string_lossy();
        let (success, text) = cli_in_pty(
            &f,
            &["plan", "--skip-planning", "--config", &config, "test plan"],
            &[
                ("Publish as Issue", b"\r"),
                ("Run E2E", b" \r"),
                ("Steps to skip (", b"\x1b"),
                ("Publish as Issue", b"\r"),
                ("Run E2E", b"\r"),
                ("Steps to skip (", b"\r"),
            ],
        );
        assert!(success, "{text}");
        let manager = cruise::session::SessionManager::new(f.root.path().join("data/cruise"));
        let sessions = manager
            .list()
            .unwrap_or_else(|error| panic!("list sessions: {error}"));
        assert_eq!(sessions.len(), 1, "{text}");
        // The second approval accepts untouched defaults. The first attempt's
        // e2e=true must not survive cancelling its skip selection.
        assert_eq!(
            sessions[0].flags,
            cruise::flags::FlagValues::from([("docs".into(), false), ("e2e".into(), false)]),
            "{text}"
        );
        assert!(sessions[0].skipped_steps.is_empty(), "{text}");
        assert!(!f.repo.join("build.txt").exists(), "{text}");
    }

    #[test]
    fn plan_execute_now_selects_flags_before_skip_steps() {
        let mut f = Fixture::new();
        // Stop at workspace selection without creating branches or refs.
        f.repo = f.root.path().join("plain");
        std::fs::create_dir(&f.repo).unwrap_or_else(|error| panic!("create directory: {error}"));
        let config = f.config.to_string_lossy();
        let (success, text) = cli_in_pty(
            &f,
            &["plan", "--skip-planning", "--config", &config, "test plan"],
            &[
                ("Publish as Issue", b"\x1b[B\x1b[B\x1b[B\r"),
                ("Run E2E", b" \r"),
                ("Steps to skip (", b"\r"),
                ("Use current branch", b"\x1b"),
            ],
        );
        assert!(!success, "{text}");
        assert!(
            text.contains("workspace") && text.contains("cancel"),
            "{text}"
        );
        let flag_prompt = text.find(FLAG_PROMPT).unwrap_or_else(|| panic!("{text}"));
        let skip_prompt = text
            .find("Steps to skip (")
            .unwrap_or_else(|| panic!("{text}"));
        assert!(flag_prompt < skip_prompt, "{text}");
        assert!(!text[skip_prompt..].contains(FLAG_PROMPT), "{text}");
        let workspace_prompt = text[skip_prompt..]
            .find("Use current branch")
            .map_or_else(|| panic!("{text}"), |offset| skip_prompt + offset);
        let skip_start = text
            .find("Steps to skip (Space to toggle, Enter to confirm)")
            .unwrap_or_else(|| panic!("{text}"));
        let skip_output = &text[skip_start..workspace_prompt];
        assert!(skip_output.contains("e2e-test"), "{text}");
        assert!(!skip_output.contains("docs-test"), "{text}");
        let manager = cruise::session::SessionManager::new(f.root.path().join("data/cruise"));
        let sessions = manager
            .list()
            .unwrap_or_else(|error| panic!("list sessions: {error}"));
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].phase, cruise::session::SessionPhase::Planned);
        assert_eq!(
            sessions[0].flags,
            cruise::flags::FlagValues::from([("docs".into(), false), ("e2e".into(), true)])
        );
        assert!(!f.repo.join("build.txt").exists(), "{text}");
    }

    #[test]
    fn plan_execute_now_forwards_explicit_flags_without_an_extra_flag_prompt() {
        let mut f = Fixture::new();
        // Planning supports nongit directories. Cancel at workspace selection,
        // after flag selection would occur, without creating branches or refs.
        f.repo = f.root.path().join("plain");
        std::fs::create_dir(&f.repo).unwrap_or_else(|error| panic!("create directory: {error}"));
        let config = f.config.to_string_lossy();
        let (success, text) = cli_in_pty(
            &f,
            &[
                "plan",
                "--skip-planning",
                "--config",
                &config,
                "-f",
                "docs",
                "test plan",
            ],
            &[
                ("Publish as Issue", b"\x1b[B\x1b[B\x1b[B\r"),
                ("Steps to skip (", b"\r"),
                ("Use current branch", b"\x1b"),
            ],
        );
        assert!(!success, "{text}");
        assert!(
            text.contains("workspace") && text.contains("cancel"),
            "{text}"
        );
        assert!(!text.contains(FLAG_PROMPT), "{text}");
        let manager = cruise::session::SessionManager::new(f.root.path().join("data/cruise"));
        let sessions = manager
            .list()
            .unwrap_or_else(|error| panic!("list sessions: {error}"));
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].phase, cruise::session::SessionPhase::Planned);
        assert_eq!(sessions[0].flags.get("docs"), Some(&true));
        assert!(!f.repo.join("build.txt").exists(), "{text}");
    }

    #[test]
    fn force_exec_plan_selects_workflow_flags() {
        assert_force_exec_selection(&["plan", "task"]);
    }

    #[test]
    fn force_exec_root_positional_selects_workflow_flags() {
        assert_force_exec_selection(&["task"]);
    }

    #[test]
    fn force_exec_background_plan_selects_flags_in_the_foreground() {
        assert_force_exec_selection(&["--plan", "task"]);
    }

    fn force_exec_fixture() -> Fixture {
        let f = Fixture::new();
        std::fs::write(
            f.repo.join("cruise.yaml"),
            format!("force_exec: true\n{WORKFLOW}"),
        )
        .unwrap_or_else(|error| panic!("write force-exec config: {error}"));
        f
    }

    fn assert_force_exec_selection(args: &[&str]) {
        let f = force_exec_fixture();
        let (success, text) =
            cli_in_pty(&f, args, &[("./cruise.yaml", b"\r"), ("Run E2E", b" \r")]);
        assert!(success, "{text}");
        assert!(text.contains(FLAG_PROMPT), "{text}");
        assert!(f.repo.join("e2e.txt").exists(), "{text}");
        assert!(!f.repo.join("docs.txt").exists(), "{text}");
    }

    #[test]
    fn force_exec_background_plan_forwards_explicit_flags_without_prompting() {
        let f = force_exec_fixture();
        let (success, text) = cli_in_pty(
            &f,
            &["--plan", "task", "-f", "docs"],
            &[("./cruise.yaml", b"\r")],
        );
        assert!(success, "{text}");
        assert!(!text.contains(FLAG_PROMPT), "{text}");
        assert!(f.repo.join("docs.txt").exists(), "{text}");
        assert!(!f.repo.join("e2e.txt").exists(), "{text}");
    }

    #[test]
    fn exec_flag_selection_cancellation_has_no_side_effects_and_cleans_session() {
        assert_exec_cancelled(b"\x1b");
    }

    #[test]
    fn exec_ctrl_c_during_flag_selection_cleans_session() {
        assert_exec_cancelled(b"\x03");
    }

    fn assert_exec_cancelled(keys: &[u8]) {
        fn assert_no_session_state(path: &Path) {
            if !path.exists() {
                return;
            }
            for entry in
                std::fs::read_dir(path).unwrap_or_else(|error| panic!("read sessions: {error}"))
            {
                let path = entry
                    .unwrap_or_else(|error| panic!("session entry: {error}"))
                    .path();
                if path.file_name().is_some_and(|name| name == "sessions") && path.is_dir() {
                    assert!(
                        std::fs::read_dir(&path)
                            .unwrap_or_else(|error| panic!("read session directory: {error}"))
                            .next()
                            .is_none(),
                        "temporary exec session directory remains: {}",
                        path.display()
                    );
                }
                assert!(
                    path.file_name().is_none_or(|name| name != "state.json"),
                    "temporary exec session remains: {}",
                    path.display()
                );
                if path.is_dir() {
                    assert_no_session_state(&path);
                }
            }
        }
        let f = Fixture::new();
        let (success, text) = exec_in_pty(&f, &[], Some(keys));
        assert!(!success, "{text}");
        assert!(text.contains("workflow flag selection cancelled"), "{text}");
        for file in ["build.txt", "e2e.txt", "docs.txt", "any.txt"] {
            assert!(!f.repo.join(file).exists(), "unexpected {file}: {text}");
        }
        assert_no_session_state(&f.root.path().join("data"));
        assert_no_session_state(&f.root.path().join("state"));
    }
}
