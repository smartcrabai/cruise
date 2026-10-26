//! Public runtime contracts. Fake CLIs never contact a model or a real `OpenCode` server.
//! Run with `cargo test --all-features --test opencode_runtime_contract`.
#![cfg(unix)]

use std::collections::HashMap;
use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cruise::cancellation::CancellationToken;
use cruise::config::PermissionMode;
use cruise::error::CruiseError;
use cruise::executor::{Executor, PromptRun};
use cruise::test_support::{EnvGuard, lock_process, prepend_to_path, write_executable_script};
use tempfile::TempDir;

const RUN_LIMIT: Duration = Duration::from_secs(12);

struct Fixture {
    root: TempDir,
    pid: PathBuf,
}

impl Fixture {
    fn new(body: &str) -> Self {
        let root =
            TempDir::new().unwrap_or_else(|error| panic!("temporary runtime fixture: {error}"));
        let pid = root.path().join("launched.pid");
        // Paths are passed in environment variables, not interpolated into shell code.
        write_executable_script(
            &root.path().join("opencode"),
            &format!(
                "#!/bin/sh\ncase \"${{1:-}}\" in --version) printf 'opencode v2.1.3\\n'; exit 0 ;; esac\nprintf '%s\\n' \"$$\" >> \"$CONTRACT_PID\"\n{body}\n"
            ),
        );
        // A backend-selection regression must not invoke an installed real agent.
        for name in ["jcode", "claude"] {
            write_executable_script(&root.path().join(name), "#!/bin/sh\nexit 97\n");
        }
        Self { root, pid }
    }

    fn pids(&self) -> Vec<i32> {
        fs::read_to_string(&self.pid)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                line.parse()
                    .unwrap_or_else(|error| panic!("fake CLI PID: {error}"))
            })
            .collect()
    }

    fn assert_stopped(&self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.pids().iter().any(|&pid| alive(pid)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            self.pids().iter().all(|&pid| !alive(pid)),
            "Executor returned before its fake OpenCode CLI was reaped"
        );
    }

    fn run(&self, cancelled: bool) -> Result<cruise::executor::PromptOutcome, CruiseError> {
        let env = HashMap::new();
        let mcp = cruise::config::McpServers::default();
        let token = CancellationToken::new();
        if cancelled {
            token.cancel();
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("test runtime: {error}"));
        let result = runtime.block_on(async {
            tokio::time::timeout(
                RUN_LIMIT,
                // An empty command is intentional: this must select the SDK, not a
                // fake fallback command that could make missing support look correct.
                Executor::new(Some("opencode"), &[]).run(PromptRun {
                    prompt: "Do not contact a model: startup contract only",
                    model: Some(&cruise::config::ModelSpec::Single(
                        "contract/model".to_owned(),
                    )),
                    subagent_model: None,
                    max_retries: 0,
                    env: &env,
                    mcp_servers: &mcp,
                    on_notice: None,
                    cancel_token: Some(&token),
                    working_dir: Some(self.root.path()),
                    stream: None,
                    tools: vec![],
                    on_session_id: None,
                    resume: None,
                    computer_use: false,
                    permission: PermissionMode::Full,
                    tool_policy: cruise::tool_policy::ToolPolicy::default(),
                }),
            )
            .await
        });
        // A broken backend's blocking task must not make runtime destruction
        // wait forever before the fixture's emergency PID cleanup can run.
        runtime.shutdown_timeout(Duration::from_secs(1));
        result.unwrap_or_else(|error| {
            panic!("OpenCode startup rejection exceeded bounded deadline: {error}")
        })
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal zero checks existence without delivering a signal.
    unsafe { libc::kill(pid, 0) == 0 }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Last-resort cleanup also runs on assertion failure or timeout. Scripts
        // below have no child processes, so there is no shell child to orphan.
        for pid in self.pids() {
            if alive(pid) {
                // SAFETY: this PID was recorded by our private fake executable.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
}

fn with_fixture(body: &str, check: impl FnOnce(&Fixture)) {
    let _lock = lock_process();
    let fixture = Fixture::new(body);
    let _path = prepend_to_path(fixture.root.path());
    let _pid = EnvGuard::set("CONTRACT_PID", &fixture.pid);
    let _home = cruise::test_support::set_fake_home(fixture.root.path());
    let _config = EnvGuard::remove("OPENCODE_CONFIG_CONTENT");
    let _password = EnvGuard::remove("OPENCODE_SERVER_PASSWORD");
    // Even a buggy client accepting 0.0.0.0 must hit only our local tripwire,
    // never a proxy inherited from the developer's environment.
    let _proxy_guards: Vec<_> = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ]
    .into_iter()
    .map(EnvGuard::remove)
    .collect();
    let _no_proxy = EnvGuard::set("NO_PROXY", "*");
    let _no_proxy_lower = EnvGuard::set("no_proxy", "*");
    let _data = EnvGuard::set("XDG_DATA_HOME", fixture.root.path().join("data"));
    let _state = EnvGuard::set("XDG_STATE_HOME", fixture.root.path().join("state"));
    let _configuration = EnvGuard::set("XDG_CONFIG_HOME", fixture.root.path().join("config"));
    check(&fixture);
}

fn startup_error(fixture: &Fixture, classes: &[&str]) -> String {
    let result = fixture.run(false);
    fixture.assert_stopped();
    assert!(
        !fixture.pids().is_empty(),
        "fake OpenCode CLI never launched, so this is not a runtime contract result: {result:?}"
    );
    let error = match result {
        Err(error) => error.to_string(),
        Ok(outcome) => panic!("invalid startup must fail: {outcome:?}"),
    };
    let lower = error.to_ascii_lowercase();
    assert!(
        lower.contains("opencode"),
        "missing backend context: {error}"
    );
    assert!(
        classes.iter().any(|class| lower.contains(class)),
        "expected meaningful runtime error class {classes:?}: {error}"
    );
    error
}

#[test]
fn malformed_startup_json_is_rejected_and_cli_reaped() {
    with_fixture(
        "printf '%s\\n' '{\"url\":'\nexec /bin/cat >/dev/null",
        |fixture| {
            startup_error(fixture, &["json", "malformed", "protocol", "startup"]);
        },
    );
}

#[test]
fn v1_listening_log_is_not_accepted_as_v2_startup() {
    with_fixture(
        "printf '%s\\n' 'opencode server listening on http://127.0.0.1:1'\nexit 0",
        |fixture| {
            startup_error(fixture, &["v2", "startup", "protocol", "json"]);
        },
    );
}

#[test]
fn nonloopback_startup_url_is_refused_before_any_connection() {
    // 0.0.0.0 is not loopback, but an erroneous connection to it reaches
    // this local listener. This makes the no-connect assertion observable
    // without sending traffic to an actual external machine.
    let listener =
        TcpListener::bind("0.0.0.0:0").unwrap_or_else(|error| panic!("tripwire listener: {error}"));
    listener
        .set_nonblocking(true)
        .unwrap_or_else(|error| panic!("nonblocking tripwire: {error}"));
    let port = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("listener address: {error}"))
        .port();
    with_fixture(
        &format!(
            "printf '%s\\n' '{{\"url\":\"http://0.0.0.0:{port}\"}}'\nexec /bin/cat >/dev/null"
        ),
        |fixture| {
            let result = fixture.run(false);
            fixture.assert_stopped();
            assert!(
                matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
                "runtime connected to a refused nonloopback startup address"
            );
            assert!(
                !fixture.pids().is_empty(),
                "fake CLI did not launch: {result:?}"
            );
            let error = match result {
                Err(error) => error.to_string().to_ascii_lowercase(),
                Ok(outcome) => panic!("nonloopback URL must fail: {outcome:?}"),
            };
            assert!(error.contains("opencode"), "{error}");
            assert!(
                ["loopback", "url", "address", "local"]
                    .iter()
                    .any(|class| error.contains(class)),
                "missing unsafe-address diagnostic: {error}"
            );
        },
    );
}

#[test]
fn failed_cli_redacts_server_password_and_preserves_stderr() {
    with_fixture(
        "printf '%s' \"$OPENCODE_SERVER_PASSWORD\" > \"$CONTRACT_PID.secret\"\nprintf 'contract-startup-failure password=%s\\n' \"$OPENCODE_SERVER_PASSWORD\" >&2\ni=0\nwhile [ \"$i\" -lt 8192 ]; do\n printf '0123456789abcdef0123456789abcdef\\n' >&2\n i=$((i + 1))\ndone\nprintf 'contract-startup-failure password=%s\\n' \"$OPENCODE_SERVER_PASSWORD\" >&2\nexit 23",
        |fixture| {
            let error = startup_error(fixture, &["exit", "failed", "startup"]);
            assert!(
                error.contains("contract-startup-failure"),
                "failed CLI diagnostic discarded useful stderr context"
            );
            let secret = fs::read_to_string(fixture.pid.with_extension("pid.secret"))
                .unwrap_or_else(|error| {
                    panic!("fake CLI captured generated server password: {error}")
                });
            assert!(!secret.is_empty(), "runtime must supply a server password");
            assert!(
                !error.contains(&secret),
                "server password leaked in diagnostics"
            );
        },
    );
}

#[test]
fn precancelled_request_launches_no_cli() {
    with_fixture("exit 99", |fixture| {
        let result = fixture.run(true);
        fixture.assert_stopped();
        assert!(
            fixture.pids().is_empty(),
            "pre-cancelled request launched CLI"
        );
        assert!(
            matches!(result, Err(CruiseError::Interrupted)),
            "pre-cancellation must be interruption, not unsupported SDK or command fallback: {result:?}"
        );
    });
}
