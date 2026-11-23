#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    fs,
    io::{BufReader, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::Path,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

use jcode_sdk::api::{
    API_VERSION_MAJOR, ApiEvent, ApiRequest, ClientFrame, ServerFrame, read_frame, write_frame,
};

const FAKE_JCODE: &str = "CRUISE_TEST_MANAGER_FAKE_JCODE";

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn cruise(home: &Path, sdk: bool) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cruise"));
    c.env_clear()
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", home.join("bin").display()),
        )
        .env("JCODE_HOME", home.join("jcode"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .current_dir(home)
        .stdin(Stdio::null());
    if sdk {
        c.env("CRUISE_SDK", "jcode")
            .env("CRUISE_PLAN_MODEL", "fixture/model");
    } else {
        c.env(
            "CRUISE_COMMAND",
            serde_json::to_string(&[home.join("forbidden-command")])
                .unwrap_or_else(|error| panic!("test operation failed: {error}")),
        );
    }
    c
}

fn command(home: &Path, sdk: bool, args: &[&str]) -> Output {
    cruise(home, sdk)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))
}

struct ManagerCli(Option<Child>);

impl ManagerCli {
    fn spawn(home: &Path, repo: &Path, args: &[&str], stdin: Option<&str>) -> Self {
        let mut child = Self(Some(
            cruise(home, true)
                .current_dir(repo)
                .arg("manager")
                .args(args)
                .stdin(if stdin.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap_or_else(|error| panic!("spawn manager CLI: {error}")),
        ));
        if let Some(text) = stdin {
            child
                .child()
                .stdin
                .take()
                .unwrap_or_else(|| panic!("missing piped stdin"))
                .write_all(text.as_bytes())
                .unwrap_or_else(|error| panic!("write manager CLI stdin: {error}"));
        }
        child
    }

    fn child(&mut self) -> &mut Child {
        self.0
            .as_mut()
            .unwrap_or_else(|| panic!("missing manager CLI child"))
    }

    async fn finish(mut self) -> Output {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self
            .child()
            .try_wait()
            .unwrap_or_else(|error| panic!("poll manager CLI: {error}"))
            .is_none()
        {
            assert!(Instant::now() < deadline, "manager CLI did not exit");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.0
            .take()
            .unwrap_or_else(|| panic!("missing manager CLI child"))
            .wait_with_output()
            .unwrap_or_else(|error| panic!("read manager CLI output: {error}"))
    }
}

impl Drop for ManagerCli {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn cli_stdout(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "manager CLI exited {}: {stdout}{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    stdout.into_owned()
}

fn transcript(snapshot: &Value) -> &[Value] {
    snapshot["transcript"]
        .as_array()
        .unwrap_or_else(|| panic!("missing transcript array"))
}

fn daemon_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|error| panic!("build daemon client: {error}"))
}

struct Stop<'a>(&'a Path);
impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = command(self.0, true, &["daemon", "stop", "--force"]);
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))
        .trim()
        .into()
}

fn start(home: &Path, sdk: bool) -> (String, String) {
    let out = command(home, sdk, &["daemon", "start", "--port", "0"]);
    assert!(
        out.status.success(),
        "start: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(
        &fs::read(home.join("state/cruise/daemon/instance.json"))
            .unwrap_or_else(|error| panic!("test operation failed: {error}")),
    )
    .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    (
        v["origin"]
            .as_str()
            .unwrap_or_else(|| panic!("expected string"))
            .into(),
        v["token"]
            .as_str()
            .unwrap_or_else(|| panic!("expected string"))
            .into(),
    )
}

async fn snapshot(client: &reqwest::Client, origin: &str, token: &str, repo: &Path) -> Value {
    let mut url = reqwest::Url::parse(&format!("{origin}api/manager/snapshot"))
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    url.query_pairs_mut().append_pair(
        "cwd",
        repo.to_str()
            .unwrap_or_else(|| panic!("expected UTF-8 path")),
    );
    client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))
        .error_for_status()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))
        .json()
        .await
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))
}

async fn action(
    client: &reqwest::Client,
    origin: &str,
    token: &str,
    repo: &Path,
    action: Value,
) -> Value {
    let response = client
        .post(format!("{origin}api/manager/action"))
        .bearer_auth(token)
        .json(&json!({"cwd":repo,"action":action}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "action: {body}");
    serde_json::from_str(&body).unwrap_or_else(|error| panic!("test operation failed: {error}"))
}

async fn wait_for(
    client: &reqwest::Client,
    origin: &str,
    token: &str,
    repo: &Path,
    mut predicate: impl FnMut(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = snapshot(client, origin, token, repo).await;
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(Instant::now() < deadline, "snapshot timeout: {snapshot}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn no_children(home: &Path, snapshot: &Value) {
    assert!(
        snapshot["goals"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .is_empty(),
        "unexpected goals: {snapshot}"
    );
    let dir = home.join("data/cruise/sessions");
    assert!(
        !dir.exists()
            || fs::read_dir(dir)
                .unwrap_or_else(|error| panic!("test operation failed: {error}"))
                .next()
                .is_none(),
        "unauthorized child session"
    );
}

fn install_jcode(home: &Path) {
    fs::create_dir(home.join("bin"))
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let jcode = home.join("bin/jcode");
    let executable = std::env::current_exe().unwrap_or_else(|error| panic!("{error}"));
    let log = home.join("fake-jcode.log");
    fs::write(
        &jcode,
        format!(
            "#!/bin/sh\n[ \"$1\" = --no-update ] && [ \"$2\" = api-bridge ] && [ \"$3\" = --api-socket ] || exit 91\nexport {FAKE_JCODE}=1\nexport CRUISE_TEST_SOCKET=\"$4\"\nexport CRUISE_TEST_RELEASE_FILE={}\nexec {} --ignored --exact manager_fake_jcode_runtime --nocapture > {} 2>&1\n",
            shell_quote(&home.join("release-jcode")),
            shell_quote(&executable),
            shell_quote(&log)
        )
    )
    .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    fs::set_permissions(&jcode, fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
}

fn setup_repo(home: &Path) -> std::path::PathBuf {
    let repo = home.join("repo");
    fs::create_dir(&repo).unwrap_or_else(|error| panic!("test operation failed: {error}"));
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.test"]);
    fs::write(repo.join("file"), "base\n")
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    git(&repo, &["add", "file"]);
    git(&repo, &["commit", "-qm", "base"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["update-ref", "refs/remotes/origin/main", &sha]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/project.git",
        ],
    );
    repo
}

fn event(value: Value) -> ApiEvent {
    serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
}

fn cli_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::Builder::new()
        .prefix("cm-")
        .tempdir()
        .unwrap_or_else(|error| panic!("create isolated manager CLI home: {error}"));
    install_jcode(root.path());
    let repo = setup_repo(root.path());
    (root, repo)
}

#[tokio::test]
async fn say_no_wait_prints_the_running_turn_id_and_rejects_busy() {
    let (root, repo) = cli_fixture();
    let home = root.path();
    let _stop = Stop(home);
    let (origin, token) = start(home, true);
    let client = daemon_client();

    let accepted = ManagerCli::spawn(home, &repo, &["say", "First goal", "--no-wait"], None)
        .finish()
        .await;
    let stdout = cli_stdout(&accepted);
    let running = snapshot(&client, &origin, &token, &repo).await;
    let [turn] = transcript(&running) else {
        panic!("expected exactly one running turn: {running}");
    };
    assert_eq!(turn["role"], "user");
    assert_eq!(turn["text"], "First goal");
    assert_eq!(turn["status"], "running");
    let id = turn["id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing turn ID"));
    assert!(
        !id.is_empty() && stdout.contains(id),
        "missing {id} in {stdout}"
    );

    let busy = ManagerCli::spawn(home, &repo, &["say", "Second goal", "--no-wait"], None)
        .finish()
        .await;
    let stderr = String::from_utf8_lossy(&busy.stderr);
    assert!(
        !busy.status.success() && stderr.contains("a manager conversation turn is already running"),
        "expected Busy failure, got {}: {stderr}",
        busy.status
    );
    let still_running = snapshot(&client, &origin, &token, &repo).await;
    assert_eq!(transcript(&still_running), transcript(&running));
    no_children(home, &still_running);

    fs::write(home.join("release-jcode"), "release")
        .unwrap_or_else(|error| panic!("release fake provider: {error}"));
    wait_for(&client, &origin, &token, &repo, |state| {
        transcript(state)
            .iter()
            .any(|turn| turn["id"] == id && turn["status"] == "completed")
    })
    .await;
}

#[tokio::test]
async fn say_reads_stdin_and_waits_for_the_assistant_reply() {
    let (root, repo) = cli_fixture();
    let home = root.path();
    let _stop = Stop(home);
    let (origin, token) = start(home, true);
    let client = daemon_client();
    let mut say = ManagerCli::spawn(home, &repo, &["say"], Some("hello\n"));
    let pending = wait_for(&client, &origin, &token, &repo, |state| {
        !transcript(state).is_empty()
            || say
                .child()
                .try_wait()
                .unwrap_or_else(|error| panic!("poll say: {error}"))
                .is_some()
    })
    .await;
    if transcript(&pending).is_empty() {
        let output = say.finish().await;
        panic!("say exited before submitting stdin: {output:?}");
    }
    let [turn] = transcript(&pending) else {
        panic!("expected just the pending user turn: {pending}");
    };
    assert_eq!(turn["role"], "user");
    assert_eq!(turn["text"], "hello");
    assert_eq!(turn["status"], "running");

    // Keep the provider blocked across the planned one-second snapshot polling interval.
    tokio::time::sleep(Duration::from_millis(1250)).await;
    assert!(
        say.child()
            .try_wait()
            .unwrap_or_else(|error| panic!("poll say: {error}"))
            .is_none(),
        "say exited while its turn was running"
    );
    fs::write(home.join("release-jcode"), "release")
        .unwrap_or_else(|error| panic!("release fake provider: {error}"));
    let stdout = cli_stdout(&say.finish().await);
    assert!(
        stdout.contains("Discussed goal"),
        "missing assistant reply: {stdout}"
    );
    let completed = snapshot(&client, &origin, &token, &repo).await;
    let [user, assistant] = transcript(&completed) else {
        panic!("expected user and assistant turns: {completed}");
    };
    assert_eq!(user["id"], turn["id"]);
    assert_eq!(user["status"], "completed");
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["text"], "Discussed goal");
    no_children(home, &completed);
}

#[tokio::test]
async fn say_ctrl_c_stops_waiting_but_the_daemon_completes_the_same_turn() {
    let (root, repo) = cli_fixture();
    let home = root.path();
    let _stop = Stop(home);
    let (origin, token) = start(home, true);
    let client = daemon_client();
    let mut say = ManagerCli::spawn(home, &repo, &["say", "Keep working"], None);
    let pending = wait_for(&client, &origin, &token, &repo, |state| {
        !transcript(state).is_empty()
            || say
                .child()
                .try_wait()
                .unwrap_or_else(|error| panic!("poll say: {error}"))
                .is_some()
    })
    .await;
    if transcript(&pending).is_empty() {
        let output = say.finish().await;
        panic!(
            "say exited before submitting the turn: {}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let [turn] = transcript(&pending) else {
        panic!("expected one pending turn before SIGINT: {pending}");
    };
    assert_eq!(turn["role"], "user");
    assert_eq!(turn["text"], "Keep working");
    assert_eq!(turn["status"], "running");

    // Let say enter its snapshot wait loop before sending the signal.
    tokio::time::sleep(Duration::from_millis(1250)).await;
    assert!(
        say.child()
            .try_wait()
            .unwrap_or_else(|error| panic!("poll say: {error}"))
            .is_none(),
        "say exited before SIGINT"
    );
    let signal = Command::new("/bin/kill")
        .args(["-s", "INT", &say.child().id().to_string()])
        .status()
        .unwrap_or_else(|error| panic!("send SIGINT to say: {error}"));
    assert!(signal.success(), "failed to signal say: {signal}");
    let output = say.finish().await;
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.code().is_some(),
        "say died from SIGINT: {output:?}"
    );
    let diagnostic = diagnostic.to_ascii_lowercase();
    assert!(
        diagnostic.lines().any(|line| line.contains("turn")
            && ["continu", "running", "not cancel"]
                .iter()
                .any(|fragment| line.contains(fragment))),
        "say did not report that the turn continues: {output:?}"
    );

    let running = snapshot(&client, &origin, &token, &repo).await;
    let [user] = transcript(&running) else {
        panic!("expected one running turn after SIGINT: {running}");
    };
    assert_eq!(user["id"], turn["id"]);
    assert_eq!(user["status"], "running");

    fs::write(home.join("release-jcode"), "release")
        .unwrap_or_else(|error| panic!("release fake provider: {error}"));
    let completed = wait_for(&client, &origin, &token, &repo, |state| {
        transcript(state).len() >= 2
    })
    .await;
    let [user, assistant] = transcript(&completed) else {
        panic!("expected user and assistant turns: {completed}");
    };
    assert_eq!(user["id"], turn["id"]);
    assert_eq!(user["status"], "completed");
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["text"], "Discussed goal");
}

#[tokio::test]
async fn say_stops_waiting_when_the_daemon_marks_the_turn_interrupted() {
    let (root, repo) = cli_fixture();
    let home = root.path();
    let _stop = Stop(home);
    // The daemon rejects command providers before invoking them, after accepting the turn.
    let (origin, token) = start(home, false);
    let client = daemon_client();

    let output = ManagerCli::spawn(home, &repo, &["say", "Interrupted goal"], None)
        .finish()
        .await;
    let interrupted = snapshot(&client, &origin, &token, &repo).await;
    let [user, assistant] = transcript(&interrupted) else {
        panic!("expected interrupted user and assistant turns: {interrupted}; CLI: {output:?}");
    };
    assert_eq!(user["text"], "Interrupted goal");
    assert_eq!(user["status"], "interrupted");
    assert_eq!(assistant["status"], "interrupted");
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // An interrupted turn must fail normally and show the assistant diagnostic.
    assert!(
        output.status.code().is_some_and(|code| code != 0) && diagnostic.contains("not command"),
        "missing interruption diagnostic: {output:?}"
    );
    no_children(home, &interrupted);
}

#[tokio::test]
async fn propose_prints_preview_and_revision_bound_approval_command_without_approving() {
    let (root, repo) = cli_fixture();
    let home = root.path();
    let _stop = Stop(home);
    let (origin, token) = start(home, true);
    let client = daemon_client();
    fs::write(home.join("release-jcode"), "release")
        .unwrap_or_else(|error| panic!("release fake provider: {error}"));

    let stdout = cli_stdout(
        &ManagerCli::spawn(home, &repo, &["propose"], None)
            .finish()
            .await,
    );
    let proposed = snapshot(&client, &origin, &token, &repo).await;
    let proposal = &proposed["proposal"];
    assert_eq!(proposal["objective"], "Deliver goal");
    assert_eq!(proposal["id"], transcript(&proposed)[0]["id"]);
    assert!(stdout.contains("Non-interactive approval requires both --yes and --revision."));
    let revision = proposal["revision"]
        .as_u64()
        .unwrap_or_else(|| panic!("missing proposal revision"));
    assert!(
        stdout.contains("Deliver goal"),
        "missing proposal preview: {stdout}"
    );
    assert!(
        stdout.contains(&format!("cruise manager approve --revision {revision}")),
        "missing revision-bound approval command: {stdout}"
    );
    no_children(home, &proposed);
    cli_stdout(
        &ManagerCli::spawn(home, &repo, &["say", "/make-goal"], None)
            .finish()
            .await,
    );
    let after_say = snapshot(&client, &origin, &token, &repo).await;
    assert_eq!(&after_say["proposal"], proposal);
}

#[test]
#[ignore = "subprocess-only fake jcode runtime"]
fn manager_fake_jcode_runtime() {
    if std::env::var(FAKE_JCODE).as_deref() != Ok("1") {
        return;
    }
    let listener = UnixListener::bind(
        std::env::var_os("CRUISE_TEST_SOCKET").unwrap_or_else(|| panic!("missing API socket")),
    )
    .unwrap_or_else(|error| panic!("bind fake Jcode socket: {error}"));
    listener
        .set_nonblocking(false)
        .unwrap_or_else(|error| panic!("configure fake Jcode socket: {error}"));
    let (mut socket, _) = listener
        .accept()
        .unwrap_or_else(|error| panic!("accept fake Jcode connection: {error}"));
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap_or_else(|error| panic!("set fake Jcode read timeout: {error}"));
    socket
        .set_write_timeout(Some(Duration::from_secs(15)))
        .unwrap_or_else(|error| panic!("set fake Jcode write timeout: {error}"));
    let mut reader = BufReader::new(
        socket
            .try_clone()
            .unwrap_or_else(|error| panic!("clone fake Jcode socket: {error}")),
    );
    while let Ok(frame) = read_frame::<_, ClientFrame>(&mut reader) {
        let serialized = serde_json::to_string(&frame.request)
            .unwrap_or_else(|error| panic!("serialize fake Jcode request: {error}"));
        if let ApiRequest::SendMessage { session_id, .. } = &frame.request {
            let release_file = std::env::var_os("CRUISE_TEST_RELEASE_FILE")
                .unwrap_or_else(|| panic!("missing fake Jcode release file"));
            let release_path = Path::new(&release_file);
            let deadline = Instant::now() + Duration::from_secs(15);
            while !release_path.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(
                release_path.exists(),
                "timed out waiting to release fake Jcode"
            );
            let output = if serialized.contains("Create a proposal for human review") {
                r#"{"objective":"Deliver goal","target_branch":"main","exclusions":[],"criteria":[{"id":"c","description":"Works"}],"checks":[{"id":"check","criterion_ids":["c"],"argv":["true"]}],"tasks":[{"id":"task","instructions":"Implement","dependencies":[]}]}"#
            } else {
                "Discussed goal"
            };
            for value in [
                json!({"ev":"message_accepted","session_id":session_id}),
                json!({"ev":"text_delta","session_id":session_id,"text":output}),
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
                .unwrap_or_else(|error| panic!("write fake Jcode event: {error}"));
            }
            continue;
        }
        let reply = match &frame.request {
            ApiRequest::Ping => ApiEvent::Pong,
            ApiRequest::Hello { .. } => event(json!({
                "ev":"hello_ok",
                "version":API_VERSION_MAJOR,
                "server":"fake-jcode/0.99.0",
                "capabilities":["sessions","session_tools","runtime_commands"]
            })),
            ApiRequest::CreateSession { .. } => event(json!({
                "ev":"attached",
                "session":{"session_id":"manager-fixture","status":"idle","saved":false,"user_connected":false,"archived":false}
            })),
            ApiRequest::GetRuntimeInfo { session_id } => event(json!({
                "ev":"runtime_info",
                "session_id":session_id,
                "routes":[{"model":"model","provider":"fixture","api_method":"openai-compatible:fixture-route","available":true,"detail":"test route"}]
            })),
            ApiRequest::ConfigureTools { .. }
            | ApiRequest::SetModel { .. }
            | ApiRequest::SetReasoningEffort { .. } => ApiEvent::Ok,
            other => panic!("unexpected fake Jcode request: {other:?}"),
        };
        write_frame(
            &mut socket,
            &ServerFrame {
                v: API_VERSION_MAJOR,
                reply_to: Some(frame.id),
                event: reply,
            },
        )
        .unwrap_or_else(|error| panic!("write fake Jcode response: {error}"));
    }
}

#[tokio::test]
async fn conversation_and_preview_survive_disconnect_and_restart_and_command_is_rejected() {
    let root = tempfile::Builder::new()
        .prefix("cm-")
        .tempdir()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let home = root.path();
    install_jcode(home);
    let repo = setup_repo(home);
    let forbidden = home.join("forbidden-command");
    fs::write(&forbidden, "#!/bin/sh\ntouch \"$HOME/command-ran\"\n")
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    fs::set_permissions(&forbidden, fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let _stop = Stop(home);
    let (origin, token) = start(home, true);
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let turn = action(
        &client,
        &origin,
        &token,
        &repo,
        json!({"type":"message","text":"Plan a goal"}),
    )
    .await;
    drop(client);
    fs::write(home.join("release-jcode"), "release")
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let completed_snapshot = wait_for(&client, &origin, &token, &repo, |snapshot| {
        snapshot["transcript"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .iter()
            .any(|turn| turn["text"] == "Discussed goal")
    })
    .await;
    assert!(
        completed_snapshot["transcript"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .iter()
            .any(|entry| entry["id"] == turn["id"] && entry["status"] == "completed")
    );
    no_children(home, &completed_snapshot);
    action(&client, &origin, &token, &repo, json!({"type":"propose"})).await;
    let proposed_snapshot = wait_for(&client, &origin, &token, &repo, |snapshot| {
        snapshot["proposal"].is_object()
    })
    .await;
    let proposal = proposed_snapshot["proposal"].clone();
    no_children(home, &proposed_snapshot);
    let out = command(home, true, &["daemon", "stop", "--force"]);
    assert!(
        out.status.success(),
        "stop: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (origin, token) = start(home, false);
    let snapshot = snapshot(&client, &origin, &token, &repo).await;
    assert_eq!(snapshot["proposal"], proposal);
    assert!(
        snapshot["transcript"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .iter()
            .any(|entry| entry["id"] == turn["id"] && entry["status"] == "completed")
    );
    no_children(home, &snapshot);
    action(
        &client,
        &origin,
        &token,
        &repo,
        json!({"type":"message","text":"Must not execute command"}),
    )
    .await;
    let snapshot = wait_for(&client, &origin, &token, &repo, |snapshot| {
        snapshot["transcript"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .iter()
            .any(|turn| turn["text"].as_str().unwrap_or("").contains("not command"))
    })
    .await;
    assert_eq!(snapshot["proposal"], proposal);
    no_children(home, &snapshot);
    assert!(!home.join("command-ran").exists());
}
