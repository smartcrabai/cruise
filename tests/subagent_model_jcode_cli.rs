#![cfg(unix)]

//! CLI-to-SDK wire contracts. The only jcode on PATH is this test executable.
//! YAML deliberately avoids referencing the new Rust configuration fields.
//! A main model is required by Cruise's existing CLI validation, so the
//! subagent-only backend case must be covered below the CLI boundary.
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cruise::test_support::{init_git_repo, write_executable_script};
use jcode_sdk::api::{
    API_VERSION_MAJOR, ApiEvent, ApiRequest, ClientFrame, RuntimeCommand, ServerFrame, read_frame,
    write_frame,
};
use serde_json::{Value, json};
use tempfile::TempDir;

const FLAG: &str = "CRUISE_TEST_JCODE_TRANSPORT";
const ROUTE: &str = "fixture-route:worker-model";

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

struct Fixture {
    root: TempDir,
    repo: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        // A short root also keeps SDK socket aliases below Unix path limits.
        let root = tempfile::Builder::new()
            .prefix("cj-")
            .tempdir()
            .unwrap_or_else(|error| panic!("{error}"));
        let repo = root.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|error| panic!("{error}"));
        init_git_repo(&repo);
        let this = Self { root, repo };
        fs::create_dir_all(this.path("bin")).unwrap_or_else(|error| panic!("{error}"));
        for backend in ["claude", "opencode"] {
            write_executable_script(
                &this.path(&format!("bin/{backend}")),
                "#!/bin/sh\nexit 91\n",
            );
        }
        write_executable_script(
            &this.path("bin/jcode"),
            &format!(
                "#!/bin/sh\n[ \"$1\" = --no-update ] && [ \"$2\" = api-bridge ] && [ \"$3\" = --api-socket ] || exit 91\nexport {FLAG}=1\nexport CRUISE_TEST_SOCKET=\"$4\"\nexec {} --ignored --exact fixture_jcode_runtime --nocapture > {} 2>&1\n",
                quote(&std::env::current_exe().unwrap_or_else(|error| panic!("{error}"))),
                quote(&this.path("runtime-log"))
            ),
        );
        this
    }
    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }
    fn run(&self, fields: &str, fail: bool) -> (bool, String, Vec<Value>) {
        fs::write(
            self.repo.join("workflow.yaml"),
            format!("sdk: jcode\n{fields}steps:\n  answer:\n    prompt: TRANSPORT_REQUEST\n"),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if ["CRUISE_", "JCODE_", "CLAUDE", "ANTHROPIC_", "OPENAI_"]
                .iter()
                .any(|p| name.starts_with(p))
            {
                cmd.env_remove(key);
            }
        }
        let mut paths = vec![self.path("bin")];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let stdout = File::create(self.path("stdout")).unwrap_or_else(|error| panic!("{error}"));
        let stderr = File::create(self.path("stderr")).unwrap_or_else(|error| panic!("{error}"));
        let mut child = cmd
            .current_dir(&self.repo)
            .env(
                "PATH",
                std::env::join_paths(paths).unwrap_or_else(|error| panic!("{error}")),
            )
            .env("HOME", self.path("home"))
            .env("USERPROFILE", self.path("home"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("JCODE_HOME", self.path("jcode"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("GIT_CONFIG_COUNT", "0")
            .env("CRUISE_TEST_RECORD", self.path("frames"))
            .env("CRUISE_TEST_FAIL", if fail { "1" } else { "0" })
            .env(
                "CRUISE_TEST_EXPECT_OVERRIDE",
                if fields.contains("subagent_model:") {
                    "1"
                } else {
                    "0"
                },
            )
            .args([
                "exec",
                "--rate-limit-retries",
                "0",
                "--config",
                "workflow.yaml",
                "TRANSPORT_REQUEST",
            ])
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .unwrap_or_else(|error| panic!("{error}"));
        let deadline = Instant::now() + Duration::from_secs(25);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap_or_else(|error| panic!("{error}")) {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "CLI timed out: {}",
                    fs::read_to_string(self.path("stderr"))
                        .unwrap_or_else(|error| panic!("{error}"))
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let output = format!(
            "{}{}{}",
            fs::read_to_string(self.path("runtime-log")).unwrap_or_default(),
            fs::read_to_string(self.path("stdout")).unwrap_or_else(|error| panic!("{error}")),
            fs::read_to_string(self.path("stderr")).unwrap_or_else(|error| panic!("{error}"))
        );
        let frames = fs::read_to_string(self.path("frames"))
            .unwrap_or_default()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap_or_else(|error| panic!("{error}")))
            .collect();
        (status.success(), output, frames)
    }
}

fn event(value: Value) -> ApiEvent {
    serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
}

#[test]
#[ignore = "subprocess-only fake jcode runtime"]
#[expect(
    clippy::too_many_lines,
    reason = "one bounded wire loop keeps the subprocess fixture self-contained"
)]
fn fixture_jcode_runtime() {
    if std::env::var(FLAG).as_deref() != Ok("1") {
        return;
    }
    let listener = UnixListener::bind(
        std::env::var_os("CRUISE_TEST_SOCKET").unwrap_or_else(|| panic!("missing fixture socket")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    listener
        .set_nonblocking(true)
        .unwrap_or_else(|error| panic!("{error}"));
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut record = OpenOptions::new()
        .create(true)
        .append(true)
        .open(
            std::env::var_os("CRUISE_TEST_RECORD")
                .unwrap_or_else(|| panic!("missing fixture record")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
    while Instant::now() < deadline {
        let (mut socket, _) = match listener.accept() {
            Ok(pair) => pair,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(e) => panic!("accept: {e}"),
        };
        socket
            .set_nonblocking(false)
            .unwrap_or_else(|error| panic!("{error}"));
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap_or_else(|error| panic!("{error}"));
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap_or_else(|error| panic!("{error}"));
        let mut reader =
            BufReader::new(socket.try_clone().unwrap_or_else(|error| panic!("{error}")));
        let mut applied = false;
        loop {
            let frame = match read_frame::<_, ClientFrame>(&mut reader) {
                Ok(frame) => frame,
                Err(error) => {
                    eprintln!("fixture read ended: {error}");
                    break;
                }
            };
            writeln!(
                record,
                "{}",
                serde_json::to_string(&frame.request).unwrap_or_else(|error| panic!("{error}"))
            )
            .unwrap_or_else(|error| panic!("{error}"));
            record.flush().unwrap_or_else(|error| panic!("{error}"));
            let reply = match &frame.request {
                ApiRequest::Ping => ApiEvent::Pong,
                ApiRequest::Hello { .. } => event(
                    json!({"ev":"hello_ok","version":API_VERSION_MAJOR,"server":"fake-jcode/0.99.0","capabilities":["sessions","session_tools","runtime_commands"]}),
                ),
                ApiRequest::CreateSession { .. } => event(
                    json!({"ev":"attached","session":{"session_id":"fixture-session","status":"idle","saved":false,"user_connected":false,"archived":false}}),
                ),
                ApiRequest::GetRuntimeInfo { session_id } => event(
                    json!({"ev":"runtime_info","session_id":session_id,"routes":[{"model":"worker-model","provider":"fixture-provider","api_method":"openai-compatible:fixture-route","available":true,"detail":"test route"}]}),
                ),
                ApiRequest::RuntimeCommand {
                    session_id,
                    command,
                    args,
                } => {
                    assert_eq!(*command, RuntimeCommand::Agents);
                    assert_eq!(args, &["swarm", ROUTE]);
                    if std::env::var("CRUISE_TEST_FAIL").as_deref() == Ok("1") {
                        event(
                            json!({"ev":"error","code":"internal","message":"FIXTURE_SWARM_REJECTED"}),
                        )
                    } else {
                        applied = true;
                        event(
                            json!({"ev":"runtime_command_result","session_id":session_id,"command":command,"data":{},"message":"swarm configured"}),
                        )
                    }
                }
                ApiRequest::SendMessage { session_id, .. } => {
                    let expected =
                        std::env::var("CRUISE_TEST_EXPECT_OVERRIDE").as_deref() == Ok("1");
                    // Observable result depends on the setting already being active at dispatch.
                    let text = if applied == expected {
                        "TRANSPORT_SWARM_READY"
                    } else {
                        "TRANSPORT_WRONG_SWARM"
                    };
                    for value in [
                        json!({"ev":"message_accepted","session_id":session_id}),
                        json!({"ev":"text_delta","session_id":session_id,"text":text}),
                        json!({"ev":"text_done","session_id":session_id}),
                        json!({"ev":"turn_done","session_id":session_id}),
                    ] {
                        write_frame(
                            &mut socket,
                            &ServerFrame {
                                v: API_VERSION_MAJOR,
                                reply_to: None,
                                event: event(value),
                            },
                        )
                        .unwrap_or_else(|error| panic!("{error}"));
                    }
                    continue;
                }
                ApiRequest::ConfigureTools { .. }
                | ApiRequest::SetModel { .. }
                | ApiRequest::SetReasoningEffort { .. } => ApiEvent::Ok,
                other => panic!("unexpected request: {other:?}"),
            };
            write_frame(
                &mut socket,
                &ServerFrame {
                    v: API_VERSION_MAJOR,
                    reply_to: Some(frame.id),
                    event: reply,
                },
            )
            .unwrap_or_else(|error| panic!("{error}"));
        }
    }
}

fn count(frames: &[Value], request: &str) -> usize {
    frames.iter().filter(|f| f["req"] == request).count()
}

#[test]
fn subagent_route_is_active_at_prompt_dispatch_with_one_shared_runtime_query() {
    for main in ["model: openai/main-model\n", "model: main-model\n"] {
        let fixture = Fixture::new();
        let (ok, output, frames) = fixture.run(
            &format!("{main}subagent_model: fixture-provider/worker-model\n"),
            false,
        );
        assert!(ok, "{output}\n{frames:?}");
        assert!(output.contains("TRANSPORT_SWARM_READY"), "{output}");
        assert_eq!(count(&frames, "get_runtime_info"), 1, "{frames:?}");
        assert_eq!(count(&frames, "runtime_command"), 1, "{frames:?}");
        assert_eq!(count(&frames, "send_message"), 1, "{frames:?}");
        assert_eq!(count(&frames, "set_model"), 1, "{frames:?}");
    }
}

#[test]
fn absent_subagent_override_sends_no_runtime_command() {
    let fixture = Fixture::new();
    let (ok, output, frames) = fixture.run("model: openai/main-model\n", false);
    assert!(
        ok && output.contains("TRANSPORT_SWARM_READY"),
        "{output}\n{frames:?}"
    );
    assert_eq!(count(&frames, "runtime_command"), 0, "{frames:?}");
    assert_eq!(count(&frames, "get_runtime_info"), 1, "{frames:?}");
    assert_eq!(count(&frames, "send_message"), 1, "{frames:?}");
}

#[test]
fn rejected_swarm_override_has_context_and_never_sends_prompt() {
    let fixture = Fixture::new();
    let (ok, output, frames) = fixture.run(
        "model: openai/main-model\nsubagent_model: fixture-provider/worker-model\n",
        true,
    );
    assert!(!ok, "{output}");
    assert!(
        output.contains("failed to set the jcode subagent model")
            && output.contains("FIXTURE_SWARM_REJECTED"),
        "{output}\n{frames:?}"
    );
    assert!(count(&frames, "runtime_command") > 0, "{frames:?}");
    assert_eq!(count(&frames, "send_message"), 0, "{frames:?}");
}
