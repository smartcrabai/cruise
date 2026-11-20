#![cfg(all(target_os = "linux", feature = "jcode-bubblewrap"))]

//! Opt-in end-to-end coverage for the real Linux jcode Bubblewrap boundary.
//! Run with `JCODE_REAL_CLI=/absolute/path/to/jcode-linux-x86_64 cargo test --features jcode-bubblewrap --test jcode_bwrap_real -- --ignored --nocapture`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse as _, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tempfile::TempDir;

const PROFILE: &str = "test-profile";
const MODEL: &str = "test-model";
const TASK_MARKER: &str = "JCODE_BWRAP_REAL_E2E";
const MODEL_DONE: &str = "JCODE_BWRAP_REAL_E2E_OK";
const DUMMY_SECRET: &str = "JCODE_BWRAP_DUMMY_SECRET_MUST_NOT_BE_READ";
const PLAN_MARKER: &str = "JCODE_BWRAP_PLAN_READONLY_REAL";
const TDD_MARKER: &str = "JCODE_BWRAP_TDD_FULL_GRAPH_REAL";
const PLAN_WRITE_PROBE: &str = "bwrap-plan-write-probe";
const PLAN_BODY: &str = "# Bubblewrap plan fixture\n\nSaved through submit_plan.\n";
const TDD_TEST_MARKER: &str = "bwrap-tdd-test-first.txt";
const TDD_IMPLEMENTATION_MARKER: &str = "bwrap-tdd-implementation.txt";
const PLAN_READ_ONLY_COMMAND: &str = r#"python3 - <<'PY'
import errno
from pathlib import Path

assert Path("tracked.txt").read_text(encoding="utf-8") == "staged\nunstaged\n"
print("READ_ONLY_WORKSPACE_READ_OK")
try:
    with open("bwrap-plan-write-probe", "w", encoding="utf-8") as probe:
        probe.write("unexpected")
except OSError as error:
    if error.errno not in (errno.EACCES, errno.EROFS):
        raise
    print(f"READ_ONLY_WRITE_DENIED errno={error.errno}")
else:
    raise SystemExit("READ_ONLY_WRITE_ALLOWED")
PY"#;
const TDD_APPLY_OUTPUT: &str = r"## Applied work items
- none - EMPTY WORKLIST; no workspace files changed

## Reverted work items
- none

## Verification
- command: none - no workspace files changed
- result: skipped because the worklist was empty
- tools and versions: none
- evidence or blocker: EMPTY WORKLIST; no workspace files changed

## Ledger Records
```text
iteration: 1

### REV-1 | persists | major | src/main.rs:1
- summary: reproduced issue remains
- evidence: current behavior still demonstrates it
```";

#[derive(Clone, Copy)]
enum TddStage {
    WriteTests,
    Implement,
    Analyze,
    Consolidate,
    Apply,
}

fn tdd_stage(context: &str) -> Option<TddStage> {
    if context.contains("write tests only; production implementation belongs to the next step") {
        Some(TddStage::WriteTests)
    } else if context.contains("implement production code that satisfies the contract tests.") {
        Some(TddStage::Implement)
    } else if context.contains("## Ledger Records") {
        Some(TddStage::Apply)
    } else if context
        .contains("Consolidate the joined reports above into one ordered worklist for apply.")
    {
        Some(TddStage::Consolidate)
    } else if [
        "Verify every item in",
        "task-changed runtime symbols",
        "Analyze task changes for behavior-preserving cuts",
        "Review task changes for correctness",
        "Analyze the task change set for stale repository documentation",
    ]
    .iter()
    .any(|marker| context.contains(marker))
    {
        Some(TddStage::Analyze)
    } else {
        None
    }
}

fn tdd_stage_command(stage: TddStage) -> Option<String> {
    match stage {
        TddStage::WriteTests => Some(format!(
            "printf 'test-first output\\n' > {TDD_TEST_MARKER}; printf 'TDD_TEST_FIRST_WRITE_OK\\n'"
        )),
        TddStage::Implement => Some(format!(
            "printf 'production output\\n' > {TDD_IMPLEMENTATION_MARKER}; printf 'TDD_IMPLEMENTATION_WRITE_OK\\n'"
        )),
        TddStage::Analyze | TddStage::Consolidate | TddStage::Apply => None,
    }
}

fn tdd_stage_output(stage: TddStage) -> &'static str {
    match stage {
        TddStage::WriteTests => {
            "## Test Contract Map\n- Fixture task -> bwrap-tdd-test-first.txt\n\n## Changes\n- Wrote the test-first fixture output\n\n## Build Result\n- Not applicable to the fixture response\n\n## Expected Red Result\n- Fixture output recorded"
        }
        TddStage::Implement => {
            "## Summary\n- Fixture implementation output recorded\n\n## Changes\n- Wrote bwrap-tdd-implementation.txt\n\n## Build Result\n- Not applicable to the fixture response\n\n## Test Result\n- Fixture output recorded"
        }
        TddStage::Analyze => "NO FINDINGS",
        TddStage::Consolidate => {
            "# Consolidated Findings\n\n## Worklist\n\nEMPTY WORKLIST\n\n## Dropped\n- none\n\n## Unavailable sources\n- none"
        }
        TddStage::Apply => TDD_APPLY_OUTPUT,
    }
}

fn request_tool_name(request: &Value, expected: &str) -> Option<String> {
    request["tools"]
        .as_array()?
        .iter()
        .find(|tool| tool["function"]["name"] == expected)
        .and_then(|tool| tool["function"]["name"].as_str())
        .map(str::to_owned)
}

fn current_prompt_context(request: &Value) -> String {
    request["messages"]
        .as_array()
        .and_then(|messages| {
            messages
                .iter()
                .rev()
                .find(|message| message["role"] == "user")
        })
        .map_or_else(
            || request["messages"].to_string(),
            |message| message["content"].to_string(),
        )
}

fn current_turn_tool_results(request: &Value) -> usize {
    let Some(messages) = request["messages"].as_array() else {
        return 0;
    };
    let start = messages
        .iter()
        .rposition(|message| message["role"] == "user")
        .unwrap_or(0);
    messages[start..]
        .iter()
        .filter(|message| message["role"] == "tool")
        .count()
}

fn session_dirs(fixture: &Fixture) -> Vec<PathBuf> {
    let sessions = fixture.xdg_data.join("cruise/sessions");
    let entries = fs::read_dir(&sessions)
        .unwrap_or_else(|error| panic!("read session directory {sessions:?}: {error}"));
    entries
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("read session entry in {sessions:?}: {error}"))
                .path()
        })
        .filter(|path| path.is_dir())
        .collect()
}

fn new_session_dir(fixture: &Fixture, before: &[PathBuf]) -> PathBuf {
    let mut created = session_dirs(fixture)
        .into_iter()
        .filter(|path| !before.contains(path))
        .collect::<Vec<_>>();
    assert_eq!(
        created.len(),
        1,
        "expected one new session directory, got {created:?}"
    );
    created
        .pop()
        .unwrap_or_else(|| panic!("new session directory disappeared"))
}
const COMMAND_TIMEOUT: Duration = Duration::from_secs(180);

type MetadataSnapshot = BTreeMap<String, Vec<u8>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires installed jcode v1.0.1 and /usr/bin/bwrap v0.11.1"]
async fn installed_jcode_runs_in_real_bubblewrap_with_only_model_network_access() {
    let jcode = real_jcode_binary();
    assert_version(&jcode, "1.0.1");
    let bwrap = PathBuf::from("/usr/bin/bwrap");
    assert!(
        bwrap.is_file(),
        "the launcher selects /usr/bin/bwrap on Linux"
    );
    assert_version(&bwrap, "0.11.1");

    let fixture = Fixture::new(&jcode);
    fixture.install_cruise_capture_shim();
    fixture.create_linked_repositories();
    let secret = fixture.write_dummy_secret();
    let metadata = fixture.git_metadata();
    let before = metadata.snapshot();
    let sentinel = HostSentinel::start();

    let first_command = workspace_edit_command();
    let second_command = denied_effects_command(&secret, sentinel.address.port(), &metadata);
    let model = Arc::new(ModelFixture::new(
        [first_command, second_command],
        metadata.files.clone(),
    ));
    let model_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("bind local model fixture: {error}"));
    let model_address = model_listener
        .local_addr()
        .unwrap_or_else(|error| panic!("read model fixture address: {error}"));
    assert_ne!(model_address.port(), sentinel.address.port());
    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(models))
        .with_state(Arc::clone(&model));
    let model_server = tokio::spawn(async move {
        axum::serve(model_listener, app)
            .await
            .unwrap_or_else(|error| panic!("serve local model fixture: {error}"));
    });

    let address = model_address.to_string();
    let result = tokio::task::spawn_blocking(move || {
        run_acceptance(&fixture, &sentinel, &model, &before, &address);
    })
    .await;
    model_server.abort();
    result.unwrap_or_else(|error| panic!("real Bubblewrap acceptance worker failed: {error}"));
}

struct Fixture {
    root: TempDir,
    home: PathBuf,
    xdg_config: PathBuf,
    xdg_data: PathBuf,
    xdg_state: PathBuf,
    xdg_cache: PathBuf,
    host_bin: PathBuf,
    jcode_home: PathBuf,
    main_source: PathBuf,
    repo: PathBuf,
    nested_source: PathBuf,
    nested_repo: PathBuf,
    jcode: PathBuf,
}

impl Fixture {
    fn new(jcode: &Path) -> Self {
        let root = TempDir::new().unwrap_or_else(|error| panic!("create acceptance root: {error}"));
        let path = root
            .path()
            .canonicalize()
            .unwrap_or_else(|error| panic!("canonicalize acceptance root: {error}"));
        let home = path.join("home");
        let xdg_config = path.join("xdg/config");
        let xdg_data = path.join("xdg/data");
        let xdg_state = path.join("xdg/state");
        let xdg_cache = path.join("xdg/cache");
        let jcode_home = home.join("jcode");
        let main_source = path.join("main-source");
        let host_bin = path.join("host-bin");
        let repo = path.join("linked-main");
        let nested_source = path.join("nested-source");
        let nested_repo = repo.join("nested");
        for directory in [
            &home,
            &xdg_config,
            &xdg_data,
            &xdg_state,
            &xdg_cache,
            &jcode_home,
            &main_source,
            &host_bin,
            &nested_source,
        ] {
            fs::create_dir_all(directory)
                .unwrap_or_else(|error| panic!("create fixture directory {directory:?}: {error}"));
        }
        Self {
            root,
            home,
            xdg_config,
            host_bin,
            xdg_data,
            xdg_state,
            xdg_cache,
            jcode_home,
            main_source,
            repo,
            nested_source,
            nested_repo,
            jcode: jcode.to_path_buf(),
        }
    }

    fn command(&self, cwd: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        self.isolated_environment(&mut command);
        command.current_dir(cwd);
        command
    }

    fn isolated_environment(&self, command: &mut Command) {
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env("XDG_DATA_HOME", &self.xdg_data)
            .env("XDG_STATE_HOME", &self.xdg_state)
            .env("XDG_CACHE_HOME", &self.xdg_cache)
            .env("JCODE_HOME", &self.jcode_home)
            .env("JCODE_HOOKS_DISABLED", "1")
            .env("JCODE_NO_TELEMETRY", "1")
            .env("JCODE_CHECK_UPDATES", "0")
            .env("CRUISE_JCODE_BUBBLEWRAP", "1")
            .env("CRUISE_JCODE_CLI_PATH", &self.jcode)
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_COUNT", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("NO_COLOR", "1");
    }

    fn path_with_host_tools(&self) -> std::ffi::OsString {
        let inherited_path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.host_bin.clone()];
        paths.extend(std::env::split_paths(&inherited_path));
        std::env::join_paths(paths).unwrap_or_else(|error| panic!("build host-tool PATH: {error}"))
    }

    fn install_cruise_capture_shim(&self) {
        let shim = self.host_bin.join("cruise");
        let capture = self.root.path().join("tdd-capture");
        let real_cruise = shell_quote(Path::new(env!("CARGO_BIN_EXE_cruise")));
        let capture_analysis = shell_quote(&capture.join("analysis"));
        let capture_invocation = shell_quote(&capture.join("invocation.txt"));
        let capture_ledger = shell_quote(&capture.join("cruise-findings-ledger.md"));
        let capture_apply_output = shell_quote(&capture.join("analysis/apply-output.md"));
        let script = format!(
            r#"#!/bin/sh
set -u
{real_cruise} "$@"
status=$?
if [ "$status" -ne 0 ]; then
  exit "$status"
fi
if [ "$#" -ge 2 ] && [ "$1" = "internal" ] && [ "$2" = "tdd-ledger-snapshot" ]; then
  plan=${{CRUISE_TDD_LEDGER_PLAN:?missing TDD ledger plan}}
  session_dir=$(dirname "$plan")
  mkdir -p {capture_analysis} || exit $?
  printf '%s\n' "$@" >> {capture_invocation} || exit $?
  cp "$session_dir/artifacts/cruise-findings-ledger.md" {capture_ledger} || exit $?
  cp "$session_dir/artifacts/analysis/apply-output.md" {capture_apply_output} || exit $?
fi
exit "$status"
"#
        );
        fs::write(&shim, script)
            .unwrap_or_else(|error| panic!("write host Cruise capture shim: {error}"));
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("make host Cruise capture shim executable: {error}"));
    }

    fn create_linked_repositories(&self) {
        git_ok(
            &self.main_source,
            &["init", "--quiet", "--initial-branch=main"],
            &self.home,
        );
        fs::write(self.main_source.join("tracked.txt"), "original\n")
            .unwrap_or_else(|error| panic!("write main fixture file: {error}"));
        commit_fixture(&self.main_source, "main fixture", &self.home);
        git_ok(
            &self.main_source,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "bubblewrap-probe",
                path_arg(&self.repo),
                "HEAD",
            ],
            &self.home,
        );

        git_ok(
            &self.nested_source,
            &["init", "--quiet", "--initial-branch=nested-main"],
            &self.home,
        );
        fs::write(
            self.nested_source.join("nested-tracked.txt"),
            "nested-original\n",
        )
        .unwrap_or_else(|error| panic!("write nested fixture file: {error}"));
        commit_fixture(&self.nested_source, "nested fixture", &self.home);
        git_ok(
            &self.nested_source,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                path_arg(&self.nested_repo),
                "HEAD",
            ],
            &self.home,
        );
        assert!(
            self.repo.join(".git").is_file(),
            "main repo must be a linked worktree"
        );
        assert!(
            self.nested_repo.join(".git").is_file(),
            "nested repo must be a linked worktree"
        );
        assert_eq!(git_output(&self.repo, &["remote"], &self.home), "");
        assert_eq!(git_output(&self.nested_repo, &["remote"], &self.home), "");
        fs::write(self.repo.join("tracked.txt"), "staged\n")
            .unwrap_or_else(|error| panic!("seed staged main change: {error}"));
        git_ok(&self.repo, &["add", "--", "tracked.txt"], &self.home);
        assert_eq!(
            git_output(&self.repo, &["diff", "--cached", "--numstat"], &self.home),
            "1\t1\ttracked.txt"
        );
    }

    fn write_dummy_secret(&self) -> PathBuf {
        let secret = self.home.join("unavailable/dummy-secret.txt");
        fs::create_dir_all(secret.parent().unwrap_or_else(|| panic!("secret parent")))
            .unwrap_or_else(|error| panic!("create secret directory: {error}"));
        fs::write(&secret, DUMMY_SECRET)
            .unwrap_or_else(|error| panic!("write dummy secret: {error}"));
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| panic!("protect dummy secret: {error}"));
        secret
    }

    fn git_metadata(&self) -> GitMetadata {
        let mut files = Vec::new();
        for (prefix, repo, branch) in [
            ("main", &self.repo, "main"),
            ("nested", &self.nested_repo, "nested-main"),
        ] {
            files.push(MetadataFile {
                name: format!("{prefix}_gitfile"),
                path: repo.join(".git"),
            });
            let refs_path = format!("refs/heads/{branch}");
            for metadata_name in ["config", "index", refs_path.as_str()] {
                let path = git_path(repo, metadata_name, &self.home);
                files.push(MetadataFile {
                    name: format!("{prefix}_{metadata_name}").replace('/', "_"),
                    path,
                });
            }
        }
        let metadata = GitMetadata { files };
        for entry in &metadata.files {
            assert!(
                entry.path.is_file(),
                "missing Git metadata target {} at {:?}",
                entry.name,
                entry.path
            );
        }
        metadata
    }

    fn record_path(&self) -> PathBuf {
        self.xdg_state.join("cruise/daemon/instance.json")
    }
}

#[derive(Clone)]
struct MetadataFile {
    name: String,
    path: PathBuf,
}

struct GitMetadata {
    files: Vec<MetadataFile>,
}

impl GitMetadata {
    fn snapshot(&self) -> MetadataSnapshot {
        self.files
            .iter()
            .map(|entry| {
                let contents = fs::read(&entry.path).unwrap_or_else(|error| {
                    panic!("read original {} at {:?}: {error}", entry.name, entry.path)
                });
                (entry.name.clone(), contents)
            })
            .collect()
    }
}

struct ModelFixture {
    commands: [String; 2],
    metadata_files: Vec<MetadataFile>,
    requests: Mutex<Vec<Value>>,
    metadata_after_workspace_edit: Mutex<Option<MetadataSnapshot>>,
    next_tool_call: AtomicUsize,
}

impl ModelFixture {
    fn new(commands: [String; 2], metadata_files: Vec<MetadataFile>) -> Self {
        Self {
            commands,
            metadata_files,
            requests: Mutex::new(Vec::new()),
            metadata_after_workspace_edit: Mutex::new(None),
            next_tool_call: AtomicUsize::new(0),
        }
    }

    fn snapshot_metadata(&self) -> Result<MetadataSnapshot, String> {
        self.metadata_files
            .iter()
            .map(|entry| {
                fs::read(&entry.path)
                    .map(|contents| (entry.name.clone(), contents))
                    .map_err(|error| {
                        format!("snapshot {} at {:?}: {error}", entry.name, entry.path)
                    })
            })
            .collect()
    }
}

fn model_response(
    model: &ModelFixture,
    request: &Value,
    context: &str,
    tool_results: usize,
) -> (Value, &'static str) {
    let bash_available = request_tool_name(request, "bash").is_some();
    let call_index = model.next_tool_call.fetch_add(1, Ordering::SeqCst);
    let tool_call = |name: String, input: Value| {
        json!({
            "role": "assistant",
            "tool_calls": [{
                "index": 0,
                "id": format!("call_bwrap_{call_index}"),
                "type": "function",
                "function": {"name": name, "arguments": input.to_string()}
            }]
        })
    };
    if let Some(name) = request_tool_name(request, "generate_title")
        && tool_results == 0
    {
        (
            tool_call(name, json!({"title": "Bubblewrap acceptance plan"})),
            "tool_calls",
        )
    } else if context.contains(PLAN_MARKER) && tool_results == 0 && bash_available {
        (
            tool_call(
                "bash".to_owned(),
                json!({
                    "command": PLAN_READ_ONLY_COMMAND,
                    "intent": "Prove planning can read the workspace but cannot write through Bubblewrap"
                }),
            ),
            "tool_calls",
        )
    } else if context.contains(PLAN_MARKER) && tool_results == 1 {
        if let Some(name) = request_tool_name(request, "submit_plan") {
            (tool_call(name, json!({"content": PLAN_BODY})), "tool_calls")
        } else {
            (json!({"role": "assistant", "content": MODEL_DONE}), "stop")
        }
    } else if let Some(stage) = tdd_stage(context)
        && tool_results == 0
        && bash_available
        && let Some(command) = tdd_stage_command(stage)
    {
        (
            tool_call(
                "bash".to_owned(),
                json!({
                    "command": command,
                    "intent": "Produce deterministic fixture output for the real builtin:tdd graph"
                }),
            ),
            "tool_calls",
        )
    } else if let Some(stage) = tdd_stage(context) {
        (
            json!({"role": "assistant", "content": tdd_stage_output(stage)}),
            "stop",
        )
    } else if context.contains(TASK_MARKER) && tool_results < model.commands.len() && bash_available
    {
        let input = json!({
            "command": model.commands[tool_results],
            "intent": "Verify the real Linux Bubblewrap boundary"
        });
        (tool_call("bash".to_owned(), input), "tool_calls")
    } else {
        (json!({"role": "assistant", "content": MODEL_DONE}), "stop")
    }
}

async fn chat_completions(
    State(model): State<Arc<ModelFixture>>,
    Json(request): Json<Value>,
) -> Response {
    let tool_results = request["messages"].as_array().map_or(0, |messages| {
        messages
            .iter()
            .filter(|message| message["role"] == "tool")
            .count()
    });
    let context = current_prompt_context(&request);
    model
        .requests
        .lock()
        .unwrap_or_else(|error| panic!("model request lock: {error}"))
        .push(request.clone());

    if tool_results == 1 {
        let mut snapshot = model
            .metadata_after_workspace_edit
            .lock()
            .unwrap_or_else(|error| panic!("metadata snapshot lock: {error}"));
        if snapshot.is_none() {
            match model.snapshot_metadata() {
                Ok(metadata) => *snapshot = Some(metadata),
                Err(message) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, message).into_response();
                }
            }
        }
    }

    let turn_tool_results = current_turn_tool_results(&request);
    let (delta, finish) = model_response(&model, &request, &context, turn_tool_results);
    let chunk = |delta: Value, finish: Value| {
        json!({
            "id": "chatcmpl_jcode_bwrap_real",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": MODEL,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]
        })
        .to_string()
    };
    (
        [("content-type", "text/event-stream")],
        format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            chunk(delta, Value::Null),
            chunk(json!({}), json!(finish))
        ),
    )
        .into_response()
}

async fn models() -> Json<Value> {
    Json(json!({
        "object": "list",
        "data": [{"id": MODEL, "object": "model", "created": 1, "owned_by": "local"}]
    }))
}

struct HostSentinel {
    address: std::net::SocketAddr,
    accepted: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl HostSentinel {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|error| panic!("bind host loopback sentinel: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("read host sentinel address: {error}"));
        listener
            .set_nonblocking(true)
            .unwrap_or_else(|error| panic!("set sentinel nonblocking: {error}"));
        let accepted = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_accepted = Arc::clone(&accepted);
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        worker_accepted.fetch_add(1, Ordering::SeqCst);
                        drop(stream);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if worker_stop.load(Ordering::SeqCst) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept host sentinel connection: {error}"),
                }
            }
        });
        Self {
            address,
            accepted,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for HostSentinel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct CruiseDaemon<'a> {
    fixture: &'a Fixture,
    identity: Value,
    stopped: bool,
}

impl<'a> CruiseDaemon<'a> {
    fn start(fixture: &'a Fixture) -> Self {
        Self::start_with_jcode_home(fixture, &fixture.jcode_home)
    }

    fn start_with_jcode_home(fixture: &'a Fixture, jcode_home: &Path) -> Self {
        let mut command = fixture.command(&fixture.repo);
        command
            .env("JCODE_HOME", jcode_home)
            .env("PATH", fixture.path_with_host_tools())
            .args(["daemon", "start", "--host", "127.0.0.1", "--port", "0"]);
        let output = run_command(
            &mut command,
            "start isolated Cruise daemon",
            Duration::from_secs(30),
        );
        assert_output_success("start isolated Cruise daemon", &output);
        let identity: serde_json::Value = serde_json::from_slice(
            &fs::read(fixture.record_path())
                .unwrap_or_else(|error| panic!("read isolated daemon identity record: {error}")),
        )
        .unwrap_or_else(|error| panic!("parse isolated daemon identity: {error}"));
        assert_eq!(identity["host"], "127.0.0.1");
        assert_eq!(identity["port_explicit"].as_bool(), Some(true));
        Self {
            fixture,
            identity,
            stopped: false,
        }
    }

    fn stop(&mut self) {
        self.stop_if_owned(true);
        self.stopped = true;
        assert!(
            !self.fixture.record_path().exists(),
            "owned daemon record remained after stop"
        );
    }

    fn stop_if_owned(&self, report_errors: bool) {
        let Ok(contents) = fs::read(self.fixture.record_path()) else {
            return;
        };
        let Ok(current) = serde_json::from_slice::<Value>(&contents) else {
            assert!(
                !report_errors,
                "refusing to stop an unrecognized daemon record"
            );
            return;
        };
        for field in [
            "instance_id",
            "pid",
            "process_started_at",
            "origin",
            "port",
            "token",
        ] {
            if current[field] != self.identity[field] {
                assert!(
                    !report_errors,
                    "refusing to stop daemon: fixture identity changed at {field}"
                );
                return;
            }
        }
        let mut command = self.fixture.command(&self.fixture.repo);
        command.args(["daemon", "stop", "--force"]);
        if report_errors {
            let output = run_command(
                &mut command,
                "stop owned Cruise daemon",
                Duration::from_secs(30),
            );
            assert_output_success("stop owned Cruise daemon", &output);
        } else {
            let _ = command.output();
        }
    }
}

impl Drop for CruiseDaemon<'_> {
    fn drop(&mut self) {
        if !self.stopped {
            self.stop_if_owned(false);
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "keeps the linked-worktree, loopback, read-only planning, TDD, and rejected-home boundary proofs together"
)]
fn run_acceptance(
    fixture: &Fixture,
    sentinel: &HostSentinel,
    model: &Arc<ModelFixture>,
    initial_metadata: &MetadataSnapshot,
    model_address: &str,
) {
    configure_provider(fixture, model_address);
    assert!(
        model
            .requests
            .lock()
            .unwrap_or_else(|error| panic!("requests lock: {error}"))
            .is_empty(),
        "provider setup unexpectedly contacted the model fixture"
    );

    let mut daemon = CruiseDaemon::start(fixture);
    let start_status = daemon_status(&daemon.identity);
    assert_eq!(start_status["pid"], daemon.identity["pid"]);
    assert_eq!(start_status["activeOperations"], 0);

    let prompt = format!(
        "{TASK_MARKER}: verify the host-staged edit, leave an additional unstaged edit, create an untracked file, edit the nested linked repository, then verify direct Git metadata writes and filesystem/network isolation. Finish only after the scripted checks complete."
    );
    let model_ref = format!("{PROFILE}/{MODEL}");
    let mut command = fixture.command(&fixture.repo);
    command.args([
        "exec",
        "--instant",
        "--sdk",
        "jcode",
        "--model",
        model_ref.as_str(),
        prompt.as_str(),
    ]);
    let output = run_command(
        &mut command,
        "run real Cruise instant execution",
        COMMAND_TIMEOUT,
    );
    assert_output_success("real Cruise instant execution", &output);
    let terminal = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        terminal.contains(MODEL_DONE),
        "model turn did not complete successfully: {terminal}"
    );

    let requests = model
        .requests
        .lock()
        .unwrap_or_else(|error| panic!("requests lock: {error}"));
    assert!(
        requests.len() >= 3,
        "expected two real tool turns and a final model turn: {requests:?}"
    );
    let first = &requests[0];
    assert!(
        first["stream"].as_bool() == Some(true),
        "jcode did not use the fixture's streaming model API: {first}"
    );
    assert!(
        first["model"]
            .as_str()
            .is_some_and(|value| value.ends_with(MODEL)),
        "unexpected provider model route: {first}"
    );
    assert!(
        first["tools"]
            .as_array()
            .is_some_and(|tools| { tools.iter().any(|tool| tool["function"]["name"] == "bash") }),
        "real jcode request did not expose the bash tool"
    );
    assert!(
        first["messages"].as_array().is_some_and(|messages| {
            messages.iter().any(|message| {
                message["role"] == "user" && message["content"].to_string().contains(TASK_MARKER)
            })
        }),
        "real jcode request did not carry the acceptance task"
    );
    let tool_result_count = requests
        .iter()
        .filter_map(|request| request["messages"].as_array())
        .map(|messages| {
            messages
                .iter()
                .filter(|message| message["role"] == "tool")
                .count()
        })
        .max()
        .unwrap_or(0);
    assert_eq!(
        tool_result_count, 2,
        "the model did not receive both real bash tool results"
    );
    let tool_outputs = requests
        .iter()
        .filter_map(|request| request["messages"].as_array())
        .flatten()
        .filter(|message| message["role"] == "tool")
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>();
    assert!(
        tool_outputs
            .iter()
            .any(|output| output.contains("INDEX_ADD_DENIED")),
        "the guest did not report the forbidden Git index update as denied: {tool_outputs:?}"
    );
    assert!(
        !tool_outputs
            .iter()
            .any(|output| output.contains("INDEX_ADD_ALLOWED")),
        "the guest unexpectedly modified the Git index"
    );
    let evidence = fs::read_to_string(fixture.repo.join("bwrap-probe.json"))
        .unwrap_or_else(|error| panic!("read in-sandbox boundary evidence: {error}"));
    let evidence: Value = serde_json::from_str(&evidence)
        .unwrap_or_else(|error| panic!("parse in-sandbox boundary evidence: {error}"));
    assert!(
        evidence["metadata_writes"]
            .as_object()
            .is_some_and(|writes| { writes.values().all(|result| result["blocked"] == true) }),
        "direct Git metadata write was not blocked: {evidence}"
    );
    assert!(
        !evidence["secret_readable"].as_bool().unwrap_or(false),
        "dummy secret was readable: {evidence}"
    );
    assert!(
        !evidence["host_loopback_connected"]
            .as_bool()
            .unwrap_or(false),
        "host loopback was reachable: {evidence}"
    );
    assert!(
        evidence["sandbox_loopback_ok"].as_bool().unwrap_or(false),
        "sandbox could not use its own loopback: {evidence}"
    );
    assert_eq!(
        sentinel.accepted.load(Ordering::SeqCst),
        0,
        "host sentinel accepted a sandbox connection"
    );
    drop(requests);

    assert_eq!(
        fs::read_to_string(fixture.repo.join("tracked.txt"))
            .unwrap_or_else(|error| panic!("read edited tracked file: {error}")),
        "staged\nunstaged\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.nested_repo.join("nested-tracked.txt"))
            .unwrap_or_else(|error| panic!("read nested tracked file: {error}")),
        "nested-edited\n"
    );
    assert_eq!(
        git_output(
            &fixture.repo,
            &["status", "--porcelain=v1", "--untracked-files=all"],
            &fixture.home,
        ),
        "MM tracked.txt\n?? bwrap-probe.json\n?? nested/\n?? untracked.txt"
    );
    assert_eq!(
        git_output(
            &fixture.nested_repo,
            &["status", "--porcelain=v1", "--untracked-files=all"],
            &fixture.home,
        ),
        " M nested-tracked.txt\n?? nested-untracked.txt"
    );
    assert_eq!(
        git_output(
            &fixture.repo,
            &["diff", "--cached", "--numstat"],
            &fixture.home
        ),
        "1\t1\ttracked.txt"
    );
    assert_eq!(
        git_output(&fixture.repo, &["diff", "--numstat"], &fixture.home),
        "1\t0\ttracked.txt"
    );
    assert_eq!(
        git_output(&fixture.nested_repo, &["diff", "--numstat"], &fixture.home),
        "1\t1\tnested-tracked.txt"
    );

    let after_edit = model
        .metadata_after_workspace_edit
        .lock()
        .unwrap_or_else(|error| panic!("metadata snapshot lock: {error}"))
        .clone()
        .unwrap_or_else(|| panic!("model server did not snapshot metadata between tool calls"));
    for entry in &fixture.git_metadata().files {
        let actual = fs::read(&entry.path)
            .unwrap_or_else(|error| panic!("read Git metadata {}: {error}", entry.name));
        if entry.name.ends_with("_index") {
            assert_eq!(
                actual, after_edit[&entry.name],
                "direct write changed {}",
                entry.name
            );
        } else {
            assert_eq!(
                actual, initial_metadata[&entry.name],
                "protected metadata changed: {}",
                entry.name
            );
        }
    }

    run_read_only_plan(fixture, model);
    run_builtin_tdd(fixture, model);

    let final_status = daemon_status(&daemon.identity);
    assert_eq!(final_status["pid"], daemon.identity["pid"]);
    assert_eq!(final_status["activeOperations"], 0);
    assert!(!final_status["batchActive"].as_bool().unwrap_or(true));
    daemon.stop();
    let rejected_home = fixture.repo.join(".jcode-inside-workspace");
    fs::create_dir_all(&rejected_home)
        .unwrap_or_else(|error| panic!("create intentionally rejected jcode home: {error}"));
    let provider_calls_before_rejection = model
        .requests
        .lock()
        .unwrap_or_else(|error| panic!("requests lock: {error}"))
        .len();
    let mut rejected_daemon = CruiseDaemon::start_with_jcode_home(fixture, &rejected_home);
    let model_ref = format!("{PROFILE}/{MODEL}");
    let mut rejected_command = fixture.command(&fixture.repo);
    rejected_command.env("JCODE_HOME", &rejected_home).args([
        "exec",
        "--instant",
        "--sdk",
        "jcode",
        "--model",
        model_ref.as_str(),
        "JCODE_BWRAP_REJECT_SOURCE_HOME",
    ]);
    let rejected = run_command(
        &mut rejected_command,
        "run rejected Bubblewrap setup",
        Duration::from_secs(60),
    );
    assert!(
        !rejected.status.success(),
        "unsafe jcode home unexpectedly succeeded"
    );
    let rejected_output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(
        rejected_output
            .contains("jcode workspace must not contain the operator or source jcode home"),
        "the real CLI did not surface the sandbox refusal: {rejected_output}"
    );
    assert_eq!(
        model
            .requests
            .lock()
            .unwrap_or_else(|error| panic!("requests lock: {error}"))
            .len(),
        provider_calls_before_rejection,
        "rejected sandbox setup contacted the model provider"
    );
    rejected_daemon.stop();
}

fn run_read_only_plan(fixture: &Fixture, model: &Arc<ModelFixture>) {
    let config_path = fixture.root.path().join("readonly-plan.yaml");
    let model_ref = format!("{PROFILE}/{MODEL}");
    fs::write(
        &config_path,
        format!(
            "sdk: jcode\nmodel: {model_ref}\ninteractive_planning: true\nsteps:\n  implement:\n    prompt: '{{input}}'\n"
        ),
    )
    .unwrap_or_else(|error| panic!("write plan fixture workflow: {error}"));

    let before = session_dirs(fixture);
    let request_offset = model
        .requests
        .lock()
        .unwrap_or_else(|error| panic!("requests lock: {error}"))
        .len();
    let mut command = fixture.command(&fixture.repo);
    command.args(["plan", "--config", path_arg(&config_path), PLAN_MARKER]);
    let output = run_command(
        &mut command,
        "run real read-only Jcode planning",
        COMMAND_TIMEOUT,
    );
    assert_output_success("real read-only Jcode planning", &output);

    let session_dir = new_session_dir(fixture, &before);
    let plan_path = session_dir.join("plan.md");
    assert_eq!(
        fs::read_to_string(&plan_path)
            .unwrap_or_else(|error| panic!("read host-submitted plan {plan_path:?}: {error}")),
        PLAN_BODY,
        "submit_plan did not persist the plan in the Cruise session"
    );
    assert!(
        !fixture.repo.join(PLAN_WRITE_PROBE).exists(),
        "the Bubblewrap planning agent wrote into the read-only workspace"
    );

    let requests = model
        .requests
        .lock()
        .unwrap_or_else(|error| panic!("requests lock: {error}"));
    let plan_requests = &requests[request_offset..];
    assert!(
        plan_requests.iter().all(|request| {
            request["model"]
                .as_str()
                .is_some_and(|model| model.ends_with(MODEL))
        }),
        "planning requests did not use the loopback synthetic provider"
    );
    let context = plan_requests
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        context.contains(PLAN_MARKER),
        "plan prompt lacked its unique task marker"
    );
    assert!(
        context.contains("READ_ONLY_WORKSPACE_READ_OK"),
        "Bash did not read the workspace"
    );
    assert!(
        context.contains("READ_ONLY_WRITE_DENIED errno="),
        "Bash did not observe the kernel's read-only denial"
    );
    assert!(
        plan_requests
            .iter()
            .any(|request| request_tool_name(request, "submit_plan").is_some()),
        "the real planning request did not expose submit_plan"
    );
    assert!(
        context.contains("Plan saved."),
        "submit_plan did not return its host-tool result to Jcode"
    );
}

fn run_builtin_tdd(fixture: &Fixture, model: &Arc<ModelFixture>) {
    let config_dir = fixture.xdg_config.join("cruise");
    fs::create_dir_all(&config_dir)
        .unwrap_or_else(|error| panic!("create isolated Cruise config directory: {error}"));
    let config_path = config_dir.join("config.json");
    fs::write(
        &config_path,
        json!({"model": format!("{PROFILE}/{MODEL}")}).to_string(),
    )
    .unwrap_or_else(|error| panic!("write isolated Cruise model config: {error}"));

    let capture_dir = fixture.root.path().join("tdd-capture");
    let head_before = git_output(&fixture.repo, &["rev-parse", "HEAD"], &fixture.home);
    let request_offset = model
        .requests
        .lock()
        .unwrap_or_else(|error| panic!("requests lock: {error}"))
        .len();
    let mut command = fixture.command(&fixture.repo);
    command.args([
        "exec",
        "-c",
        "builtin:tdd",
        "--flag",
        "formal_methods=off",
        "--flag",
        "e2e=off",
        "--flag",
        "after_pr=off",
        "--flag",
        "merge=off",
        TDD_MARKER,
    ]);
    let output = run_command(
        &mut command,
        "run complete builtin:tdd graph",
        COMMAND_TIMEOUT,
    );
    assert_output_success("complete builtin:tdd graph", &output);

    assert_eq!(
        fs::read_to_string(fixture.repo.join(TDD_TEST_MARKER))
            .unwrap_or_else(|error| panic!("read test-first graph output: {error}")),
        "test-first output\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join(TDD_IMPLEMENTATION_MARKER))
            .unwrap_or_else(|error| panic!("read implementation graph output: {error}")),
        "production output\n"
    );

    let requests = model
        .requests
        .lock()
        .unwrap_or_else(|error| panic!("requests lock: {error}"));
    assert_tdd_full_graph(&requests[request_offset..]);
    drop(requests);
    assert_tdd_host_artifacts(fixture, &capture_dir, &head_before);
}

fn assert_tdd_full_graph(requests: &[Value]) {
    assert!(
        requests.iter().all(|request| {
            request["model"]
                .as_str()
                .is_some_and(|model| model.ends_with(MODEL))
        }),
        "builtin:tdd requests did not use the loopback synthetic provider"
    );
    let context = requests
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    for prompt in [
        "write tests only; production implementation belongs to the next step",
        "implement production code that satisfies the contract tests.",
        "Verify every item in",
        "task-changed runtime symbols",
        "Analyze task changes for behavior-preserving cuts",
        "Review task changes for correctness",
        "Analyze the task change set for stale repository documentation",
        "Consolidate the joined reports above",
        "Apply only the consolidated worklist",
        "## Ledger Records",
    ] {
        assert!(
            context.contains(prompt),
            "builtin:tdd did not reach prompt context {prompt:?}"
        );
    }
}

fn assert_tdd_host_artifacts(fixture: &Fixture, capture_dir: &Path, head_before: &str) {
    let invocation_path = capture_dir.join("invocation.txt");
    assert_eq!(
        fs::read_to_string(&invocation_path)
            .unwrap_or_else(|error| panic!("read real TDD host gate invocation: {error}")),
        "internal\ntdd-ledger-snapshot\n",
        "the host capture shim did not forward the exact TDD snapshot command"
    );
    let apply_output_path = capture_dir.join("analysis/apply-output.md");
    let apply_output = fs::read_to_string(&apply_output_path)
        .unwrap_or_else(|error| panic!("read TDD apply output {apply_output_path:?}: {error}"));
    assert!(
        apply_output.contains(TDD_APPLY_OUTPUT),
        "TDD apply did not emit its fenced ledger records body: {apply_output}"
    );
    assert!(
        !apply_output
            .lines()
            .any(|line| line.starts_with("head: ") || line.starts_with("snapshot: ")),
        "model output supplied host-derived ledger identifiers"
    );

    let ledger_path = capture_dir.join("cruise-findings-ledger.md");
    let ledger = fs::read_to_string(&ledger_path)
        .unwrap_or_else(|error| panic!("read host TDD ledger {ledger_path:?}: {error}"));
    let head_after = git_output(&fixture.repo, &["rev-parse", "HEAD"], &fixture.home);
    assert_eq!(
        head_after, head_before,
        "builtin:tdd unexpectedly moved HEAD"
    );
    let head_line = format!("head: {head_before}");
    assert!(
        ledger.lines().any(|line| line == head_line.as_str()),
        "TDD ledger head was not derived from the fixture repository: {ledger}"
    );
    assert!(
        ledger.contains("### REV-1 | persists | major | src/main.rs:1"),
        "TDD ledger did not retain the model's record body: {ledger}"
    );
    let snapshot = ledger
        .lines()
        .find_map(|line| line.strip_prefix("snapshot: "))
        .unwrap_or_else(|| panic!("TDD ledger did not include a host snapshot: {ledger}"));
    let snapshot_commit = format!("{snapshot}^{{commit}}");
    git_ok(
        &fixture.repo,
        &["cat-file", "-e", snapshot_commit.as_str()],
        &fixture.home,
    );
}

fn configure_provider(fixture: &Fixture, model_address: &str) {
    let base_url = format!("http://{model_address}/v1");
    let mut command = Command::new(&fixture.jcode);
    fixture.isolated_environment(&mut command);
    command.current_dir(&fixture.repo).args([
        "--no-update",
        "--no-selfdev",
        "provider",
        "add",
        PROFILE,
        "--base-url",
        &base_url,
        "--model",
        MODEL,
        "--no-api-key",
        "--set-default",
    ]);
    let output = run_command(
        &mut command,
        "configure fixture jcode provider",
        Duration::from_secs(30),
    );
    assert_output_success("configure fixture jcode provider", &output);
    let config = fixture.jcode_home.join("config.toml");
    let text = fs::read_to_string(&config)
        .unwrap_or_else(|error| panic!("read fixture jcode config {config:?}: {error}"));
    for expected in [PROFILE, MODEL, model_address] {
        assert!(
            text.contains(expected),
            "fixture jcode config missing {expected}: {text}"
        );
    }
}

fn workspace_edit_command() -> String {
    [
        "set -eu",
        "test \"$(git diff --cached --numstat)\" = \"$(printf '1\\t1\\ttracked.txt')\"",
        "printf 'unstaged\\n' >> tracked.txt",
        "printf 'main untracked\\n' > untracked.txt",
        "printf 'nested-edited\\n' > nested/nested-tracked.txt",
        "printf 'nested untracked\\n' > nested/nested-untracked.txt",
        "test \"$(git -C nested status --porcelain=v1 --untracked-files=all | wc -l)\" -eq 2",
        "if git add -- tracked.txt >/dev/null 2>&1; then printf 'INDEX_ADD_ALLOWED\\n'; else printf 'INDEX_ADD_DENIED\\n'; fi",
        "printf 'WORKSPACE_EDIT_OK\\n'",
    ]
    .join("\n")
}

fn denied_effects_command(secret: &Path, sentinel_port: u16, metadata: &GitMetadata) -> String {
    let mut command = format!("python3 - {} {sentinel_port}", shell_quote(secret));
    for entry in &metadata.files {
        command.push(' ');
        command.push_str(&shell_quote(Path::new(&entry.name)));
        command.push(' ');
        command.push_str(&shell_quote(&entry.path));
    }
    command.push_str(" <<'PY'\n");
    command.push_str(
        r#"import json, os, socket, sys
secret_path = sys.argv[1]
host_port = int(sys.argv[2])
metadata_writes = {}
for name, path in zip(sys.argv[3::2], sys.argv[4::2]):
    fd = None
    try:
        fd = os.open(path, os.O_WRONLY | os.O_APPEND)
        os.write(fd, b"\nBWRAP_DIRECT_WRITE_PROBE\n")
    except OSError as error:
        metadata_writes[name] = {"blocked": True, "errno": error.errno}
    else:
        metadata_writes[name] = {"blocked": False, "errno": None}
    finally:
        if fd is not None:
            os.close(fd)
try:
    with open(secret_path, "rb") as secret:
        secret.read(1)
    secret_readable = True
except OSError:
    secret_readable = False
host = socket.socket()
host.settimeout(1.0)
try:
    host.connect(("127.0.0.1", host_port))
    host_loopback_connected = True
except OSError:
    host_loopback_connected = False
finally:
    host.close()
server = socket.socket()
server.settimeout(1.0)
server.bind(("127.0.0.1", 0))
server.listen(1)
client = socket.create_connection(server.getsockname(), timeout=1.0)
connection, _ = server.accept()
client.sendall(b"loopback")
sandbox_loopback_ok = connection.recv(8) == b"loopback"
connection.close()
client.close()
server.close()
evidence = {"metadata_writes": metadata_writes, "secret_readable": secret_readable,
            "host_loopback_connected": host_loopback_connected,
            "sandbox_loopback_ok": sandbox_loopback_ok}
with open("bwrap-probe.json", "w", encoding="utf-8") as result:
    json.dump(evidence, result, sort_keys=True)
print("BWRAP_PROBE_OK")
PY"#,
    );
    command
}

fn run_command(command: &mut Command, what: &str, timeout: Duration) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("{what}: could not start command: {error}"));
    let mut stdout = child
        .stdout
        .take()
        .unwrap_or_else(|| panic!("{what}: missing stdout pipe"));
    let mut stderr = child
        .stderr
        .take()
        .unwrap_or_else(|| panic!("{what}: missing stderr pipe"));
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                timed_out = true;
                let _ = child.kill();
                break child.wait().unwrap_or_else(|error| {
                    panic!("{what}: collect timed-out child status: {error}")
                });
            }
            Err(error) => panic!("{what}: wait for child: {error}"),
        }
    };
    let output = Output {
        status,
        stdout: stdout_reader
            .join()
            .unwrap_or_else(|_| panic!("{what}: stdout reader panicked"))
            .unwrap_or_else(|error| panic!("{what}: read stdout: {error}")),
        stderr: stderr_reader
            .join()
            .unwrap_or_else(|_| panic!("{what}: stderr reader panicked"))
            .unwrap_or_else(|error| panic!("{what}: read stderr: {error}")),
    };
    assert!(
        !timed_out,
        "{what} timed out after {timeout:?}:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn assert_output_success(what: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{what} failed ({}):\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn real_jcode_binary() -> PathBuf {
    let value = std::env::var_os("JCODE_REAL_CLI")
        .unwrap_or_else(|| panic!("set JCODE_REAL_CLI to the installed jcode v1.0.1 executable"));
    let binary = PathBuf::from(value);
    assert!(
        binary.is_absolute() && binary.is_file(),
        "JCODE_REAL_CLI must be an absolute executable path: {binary:?}"
    );
    binary
}

fn assert_version(binary: &Path, expected: &str) {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .unwrap_or_else(|error| panic!("run {binary:?} --version: {error}"));
    assert_output_success("read installed runtime version", &output);
    let version = format!(
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let exact = version
        .split(|character: char| {
            !character.is_ascii_alphanumeric()
                && character != '.'
                && character != '-'
                && character != '+'
        })
        .any(|token| {
            semver::Version::parse(token.strip_prefix('v').unwrap_or(token))
                .is_ok_and(|parsed| parsed.to_string() == expected)
        });
    assert!(
        exact,
        "expected {binary:?} version {expected}, got {version:?}"
    );
}

fn daemon_status(identity: &Value) -> Value {
    let port = identity["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .unwrap_or_else(|| panic!("invalid daemon port in identity: {identity}"));
    let token = identity["token"]
        .as_str()
        .unwrap_or_else(|| panic!("missing daemon token: {identity}"));
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .unwrap_or_else(|error| panic!("connect to isolated daemon health endpoint: {error}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap_or_else(|error| panic!("set daemon health timeout: {error}"));
    write!(
        stream,
        "GET /api/v1/status HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    )
    .unwrap_or_else(|error| panic!("request daemon health endpoint: {error}"));
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .unwrap_or_else(|error| panic!("read daemon health endpoint: {error}"));
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("invalid daemon health response: {response:?}"));
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or_else(|| panic!("invalid daemon health status: {headers:?}"));
    assert_eq!(
        status, "200",
        "daemon health endpoint returned {response:?}"
    );
    serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("parse daemon health response: {error}"))
}

fn commit_fixture(repo: &Path, message: &str, home: &Path) {
    git_ok(
        repo,
        &[
            "-c",
            "user.name=Bubblewrap acceptance",
            "-c",
            "user.email=bwrap-acceptance@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "add",
            "--",
            ".",
        ],
        home,
    );
    git_ok(
        repo,
        &[
            "-c",
            "user.name=Bubblewrap acceptance",
            "-c",
            "user.email=bwrap-acceptance@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            message,
        ],
        home,
    );
}

fn git_ok(cwd: &Path, args: &[&str], home: &Path) {
    let output = git_command(cwd, args, home);
    assert_output_success("Git fixture operation", &output);
}

fn git_output(cwd: &Path, args: &[&str], home: &Path) -> String {
    let output = git_command(cwd, args, home);
    assert_output_success("Git fixture inspection", &output);
    String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_owned()
}

fn git_path(repo: &Path, path: &str, home: &Path) -> PathBuf {
    let output = git_output(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-path", path],
        home,
    );
    let path = PathBuf::from(output);
    path.canonicalize()
        .unwrap_or_else(|error| panic!("canonicalize Git path {path:?}: {error}"))
}

fn git_command(cwd: &Path, args: &[&str], home: &Path) -> Output {
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(cwd)
        .args(args);
    run_command(&mut command, "Git fixture command", Duration::from_secs(15))
}

fn path_arg(path: &Path) -> &str {
    path.to_str()
        .unwrap_or_else(|| panic!("temporary fixture path is not UTF-8: {path:?}"))
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path_arg(path).replace('\'', "'\\''"))
}
