#![cfg(unix)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use cruise::application::{
    ApplicationEvent, ApplicationEventSink, CruiseApplication, NewSessionRequest, PlanRequest,
    RunRequest,
};
use cruise::session::{SessionManager, SessionPhase, WorkspaceMode};
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::metrics::v1::{metric, number_data_point};
use opentelemetry_proto::tonic::trace::v1::status::StatusCode;
use prost::Message;
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    home: PathBuf,
    project: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new(yaml: &str) -> Self {
        let root = TempDir::new().unwrap_or_else(|error| panic!("create fixture: {error}"));
        let home = root.path().join("home");
        let project = root.path().join("project");
        std::fs::create_dir_all(&home).unwrap_or_else(|error| panic!("create home: {error}"));
        std::fs::create_dir_all(&project).unwrap_or_else(|error| panic!("create project: {error}"));
        cruise::test_support::init_git_repo(&project);
        let config = root.path().join("workflow.yaml");
        std::fs::write(&config, yaml)
            .unwrap_or_else(|error| panic!("write workflow {}: {error}", config.display()));
        Self {
            root,
            home,
            project,
            config,
        }
    }

    fn command_base(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if (name.starts_with("CRUISE_") && name != "CRUISE_GUARD_COMMON_DIR")
                || name.starts_with("OTEL_")
                || name.starts_with("JCODE_")
            {
                command.env_remove(name.as_ref());
            }
        }
        command
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("GIT_CONFIG_COUNT", "0")
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("NO_COLOR", "1")
            .env_remove("HERDR_ENV")
            .stdin(Stdio::null());
        command
    }

    fn command(&self, extra_env: &[(&str, &str)], input: &str) -> Command {
        self.command_with_args(&[], extra_env, input)
    }

    fn add_env(command: &mut Command, extra_env: &[(&str, &str)]) {
        for (name, value) in extra_env {
            command.env(name, value);
        }
    }

    fn cli_command(&self, args: &[&str], extra_env: &[(&str, &str)]) -> Command {
        let mut command = self.command_base();
        command.args(args);
        Self::add_env(&mut command, extra_env);
        command
    }

    fn command_with_args(
        &self,
        extra_args: &[&str],
        extra_env: &[(&str, &str)],
        input: &str,
    ) -> Command {
        let mut command = self.command_base();
        command
            .args(["exec", "--config"])
            .arg(&self.config)
            .arg(input)
            .args(extra_args);
        Self::add_env(&mut command, extra_env);
        command
    }

    fn run(&self, extra_env: &[(&str, &str)], input: &str) -> Output {
        self.command(extra_env, input)
            .output()
            .unwrap_or_else(|error| panic!("run cruise exec: {error}"))
    }

    fn run_with_args(
        &self,
        extra_args: &[&str],
        extra_env: &[(&str, &str)],
        input: &str,
    ) -> Output {
        self.command_with_args(extra_args, extra_env, input)
            .output()
            .unwrap_or_else(|error| panic!("run cruise exec: {error}"))
    }
}
fn create_application_session(
    app: &CruiseApplication,
    base_dir: &std::path::Path,
    input: &str,
    model: &str,
) -> String {
    app.create_session(NewSessionRequest {
        input: input.to_owned(),
        base_dir: base_dir.to_path_buf(),
        config_path: None,
        config_yaml: Some(format!(
            "command: [sh, -c, 'cat']\nmodel: ['{model}']\nsteps:\n  s1:\n    prompt: planning smoke\n"
        )),
        repo: None,
        workspace_mode: WorkspaceMode::Worktree,
        allow_dirty_working_tree: false,
        attachments: vec![],
        skipped_steps: vec![],
        flags: std::collections::BTreeMap::new(),
    })
    .unwrap_or_else(|error| panic!("create application planning session: {error}"))
    .id
}

#[derive(Clone, Debug)]
struct CapturedRequest {
    path: String,
    headers: String,
    body: Vec<u8>,
}

struct Collector {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Collector {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .unwrap_or_else(|error| panic!("bind loopback collector: {error}"));
        listener
            .set_nonblocking(true)
            .unwrap_or_else(|error| panic!("set collector nonblocking: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("get collector address: {error}"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_requests = Arc::clone(&requests);
        let thread_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if let Some(request) = read_request(stream) {
                            thread_requests
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .push(request);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            address,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    fn requests(&self) -> Vec<CapturedRequest> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn wait_for_paths(&self, expected: &[&str]) -> Vec<CapturedRequest> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let requests = self.requests();
            if expected
                .iter()
                .all(|path| requests.iter().any(|request| request.path == *path))
            {
                return requests;
            }
            assert!(
                Instant::now() < deadline,
                "collector did not receive {expected:?}; received {:?}",
                requests
                    .iter()
                    .map(|request| &request.path)
                    .collect::<Vec<_>>()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .unwrap_or_else(|_| panic!("collector thread panicked"));
        }
    }
}

fn read_request(mut stream: TcpStream) -> Option<CapturedRequest> {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap_or_else(|error| panic!("set collector read timeout: {error}"));
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap_or_else(|error| panic!("set collector write timeout: {error}"));
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    let header_end = loop {
        if let Some(index) = find_subslice(&bytes, b"\r\n\r\n") {
            break index + 4;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(count) => bytes.extend_from_slice(&chunk[..count]),
        }
        if bytes.len() > 64 * 1024 {
            return None;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]).into_owned();
    let request_line = headers.lines().next()?;
    let path = request_line.split_whitespace().nth(1)?.to_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    let expected_length = header_end.saturating_add(content_length);
    while bytes.len() < expected_length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(count) => bytes.extend_from_slice(&chunk[..count]),
        }
    }
    let body = bytes[header_end..expected_length].to_vec();
    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
    Some(CapturedRequest {
        path,
        headers,
        body,
    })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[derive(Clone, Debug)]
struct EventRecord {
    name: String,
    attributes: HashMap<String, String>,
    int_attributes: HashMap<String, i64>,
}

#[derive(Clone, Debug)]
struct SpanRecord {
    name: String,
    trace_id: Vec<u8>,
    span_id: Vec<u8>,
    parent_span_id: Vec<u8>,
    start_time_unix_nano: u64,
    end_time_unix_nano: u64,
    attributes: HashMap<String, String>,
    int_attributes: HashMap<String, i64>,
    bool_attributes: HashMap<String, bool>,
    resource_attributes: HashMap<String, String>,
    events: Vec<EventRecord>,
    status_code: i32,
}

fn spans(requests: &[CapturedRequest]) -> Vec<SpanRecord> {
    let mut records = Vec::new();
    for request in requests
        .iter()
        .filter(|request| request.path == "/v1/traces")
    {
        let exported = ExportTraceServiceRequest::decode(request.body.as_slice())
            .unwrap_or_else(|error| panic!("decode OTLP trace request: {error}"));
        for resource_spans in exported.resource_spans {
            let resource_attributes = resource_spans
                .resource
                .as_ref()
                .map(|resource| string_attributes(&resource.attributes))
                .unwrap_or_default();
            for scope_spans in resource_spans.scope_spans {
                for span in scope_spans.spans {
                    records.push(SpanRecord {
                        name: span.name,
                        trace_id: span.trace_id,
                        span_id: span.span_id,
                        parent_span_id: span.parent_span_id,
                        start_time_unix_nano: span.start_time_unix_nano,
                        end_time_unix_nano: span.end_time_unix_nano,
                        attributes: string_attributes(&span.attributes),
                        bool_attributes: bool_attributes(&span.attributes),
                        int_attributes: int_attributes(&span.attributes),
                        resource_attributes: resource_attributes.clone(),
                        events: span
                            .events
                            .into_iter()
                            .map(|event| EventRecord {
                                name: event.name,
                                attributes: string_attributes(&event.attributes),
                                int_attributes: int_attributes(&event.attributes),
                            })
                            .collect(),
                        status_code: span.status.map_or(0, |status| status.code),
                    });
                }
            }
        }
    }
    records
}

fn string_attributes(
    attributes: &[opentelemetry_proto::tonic::common::v1::KeyValue],
) -> HashMap<String, String> {
    attributes
        .iter()
        .filter_map(|attribute| {
            let value = attribute.value.as_ref()?.value.as_ref()?;
            match value {
                opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(value) => {
                    Some((attribute.key.clone(), value.clone()))
                }
                _ => None,
            }
        })
        .collect()
}
fn int_attributes(
    attributes: &[opentelemetry_proto::tonic::common::v1::KeyValue],
) -> HashMap<String, i64> {
    attributes
        .iter()
        .filter_map(|attribute| {
            let value = attribute.value.as_ref()?.value.as_ref()?;
            match value {
                opentelemetry_proto::tonic::common::v1::any_value::Value::IntValue(value) => {
                    Some((attribute.key.clone(), *value))
                }
                _ => None,
            }
        })
        .collect()
}

fn bool_attributes(
    attributes: &[opentelemetry_proto::tonic::common::v1::KeyValue],
) -> HashMap<String, bool> {
    attributes
        .iter()
        .filter_map(|attribute| {
            let value = attribute.value.as_ref()?.value.as_ref()?;
            match value {
                opentelemetry_proto::tonic::common::v1::any_value::Value::BoolValue(value) => {
                    Some((attribute.key.clone(), *value))
                }
                _ => None,
            }
        })
        .collect()
}

#[derive(Debug)]
struct MetricPoint {
    attributes: HashMap<String, String>,
    value: i64,
}

#[derive(Debug)]
struct DurationPoint {
    attributes: HashMap<String, String>,
    count: u64,
    sum_seconds: f64,
}

fn metrics(requests: &[CapturedRequest]) -> (Vec<MetricPoint>, Vec<DurationPoint>) {
    let mut counts = Vec::new();
    let mut durations = Vec::new();
    for request in requests
        .iter()
        .filter(|request| request.path == "/v1/metrics")
    {
        let exported = ExportMetricsServiceRequest::decode(request.body.as_slice())
            .unwrap_or_else(|error| panic!("decode OTLP metric request: {error}"));
        for resource_metrics in exported.resource_metrics {
            for scope_metrics in resource_metrics.scope_metrics {
                for exported_metric in scope_metrics.metrics {
                    match exported_metric.data {
                        Some(metric::Data::Sum(sum))
                            if exported_metric.name == "cruise.operation.count" =>
                        {
                            for point in sum.data_points {
                                let value = match point.value {
                                    Some(number_data_point::Value::AsInt(value)) => value,
                                    other => {
                                        panic!("expected integer operation counter, got {other:?}")
                                    }
                                };
                                counts.push(MetricPoint {
                                    attributes: string_attributes(&point.attributes),
                                    value,
                                });
                            }
                        }
                        Some(metric::Data::Histogram(histogram))
                            if exported_metric.name == "cruise.operation.duration" =>
                        {
                            for point in histogram.data_points {
                                durations.push(DurationPoint {
                                    attributes: string_attributes(&point.attributes),
                                    count: point.count,
                                    sum_seconds: point.sum.unwrap_or_else(|| {
                                        panic!("duration histogram omitted sum")
                                    }),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    (counts, durations)
}
fn retry_count_points(requests: &[CapturedRequest]) -> Vec<MetricPoint> {
    let mut points = Vec::new();
    for request in requests
        .iter()
        .filter(|request| request.path == "/v1/metrics")
    {
        let exported = ExportMetricsServiceRequest::decode(request.body.as_slice())
            .unwrap_or_else(|error| panic!("decode OTLP metric request: {error}"));
        for resource_metrics in exported.resource_metrics {
            for scope_metrics in resource_metrics.scope_metrics {
                for exported_metric in scope_metrics.metrics {
                    if exported_metric.name != "cruise.retry.count" {
                        continue;
                    }
                    let Some(metric::Data::Sum(sum)) = exported_metric.data else {
                        panic!("retry counter is not a sum");
                    };
                    for point in sum.data_points {
                        let value = match point.value {
                            Some(number_data_point::Value::AsInt(value)) => value,
                            other => panic!("expected integer retry counter, got {other:?}"),
                        };
                        points.push(MetricPoint {
                            attributes: string_attributes(&point.attributes),
                            value,
                        });
                    }
                }
            }
        }
    }
    points
}
fn assert_payload_omits(requests: &[CapturedRequest], secrets: &[&str]) {
    let body = requests
        .iter()
        .flat_map(|request| request.body.iter().copied())
        .collect::<Vec<_>>();
    let body_text = String::from_utf8_lossy(&body);
    for secret in secrets {
        assert!(
            !body_text.contains(secret),
            "telemetry payload leaked {secret:?}"
        );
    }
}

fn otlp_env(endpoint: &str) -> Vec<(String, String)> {
    vec![
        ("CRUISE_OBSERVABILITY_ENABLED".into(), "true".into()),
        ("OTEL_EXPORTER_OTLP_ENDPOINT".into(), endpoint.into()),
        ("OTEL_EXPORTER_OTLP_PROTOCOL".into(), "http/protobuf".into()),
        ("OTEL_EXPORTER_OTLP_TIMEOUT".into(), "3000".into()),
    ]
}

fn borrowed_env(env: &[(String, String)]) -> Vec<(&str, &str)> {
    env.iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn builtin_wait_ci_command(name: &str) -> String {
    let workflow = cruise::builtin_workflows::find_builtin(name)
        .unwrap_or_else(|| panic!("missing builtin workflow {name}"));
    let source_dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let config = cruise::resolver::resolve_workflow_config(
        workflow.yaml,
        &cruise::resolver::ConfigSource::NamedBuiltin(workflow.name),
        source_dir.path(),
        None,
    )
    .unwrap_or_else(|error| panic!("load builtin workflow {name}: {error}"));
    let command = config
        .after_pr
        .get("follow-up/wait-ci")
        .and_then(|step| step.command.as_ref())
        .unwrap_or_else(|| panic!("{name} has no wait-ci command"));
    let mut variables = cruise::variable::VariableStore::new(String::new());
    variables.set_named_value("pr.number", "42".to_owned());
    variables.set_named_value("pr.url", "https://github.com/owner/repo/pull/42".to_owned());
    variables
        .resolve(&command.as_slice().join("\n"))
        .unwrap_or_else(|error| panic!("resolve {name} wait-ci command: {error}"))
}

fn named_span<'a>(
    spans: &'a [SpanRecord],
    name: &str,
    attribute: &str,
    value: &str,
) -> &'a SpanRecord {
    spans
        .iter()
        .find(|span| {
            span.name == name
                && span
                    .attributes
                    .get(attribute)
                    .is_some_and(|actual| actual == value)
        })
        .unwrap_or_else(|| panic!("missing {name} span with {attribute}={value:?}: {spans:#?}"))
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", output_text(output));
}
fn wait_for_path(child: &mut Child, path: &std::path::Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        match child.try_wait() {
            Ok(Some(status)) => panic!("cruise exited before readiness marker: {status}"),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("wait for cruise readiness: {error}");
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not create readiness marker {}", path.display());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child
            .try_wait()
            .unwrap_or_else(|error| panic!("wait for cruise child: {error}"))
        {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("cruise child did not exit before its deadline");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

struct ExecPty {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Option<Box<dyn Write + Send>>,
    master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    reader: Option<JoinHandle<()>>,
    output: Arc<Mutex<Vec<u8>>>,
}

impl Drop for ExecPty {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        drop(self.writer.take());
        drop(self.master.take());
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn start_exec_pty(fixture: &Fixture, env: &[(&str, &str)], input: &str) -> ExecPty {
    start_command_pty(&fixture.command(env, input))
}

fn start_cli_pty(fixture: &Fixture, args: &[&str], env: &[(&str, &str)]) -> ExecPty {
    start_command_pty(&fixture.cli_command(args, env))
}

fn start_command_pty(std_command: &Command) -> ExecPty {
    let mut command = CommandBuilder::new(std_command.get_program());
    command.args(std_command.get_args());
    if let Some(cwd) = std_command.get_current_dir() {
        command.cwd(cwd);
    }
    for (key, value) in std_command.get_envs() {
        if let Some(value) = value {
            command.env(key, value);
        } else {
            command.env_remove(key);
        }
    }
    command.env("TERM", "xterm-256color");
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap_or_else(|error| panic!("open exec PTY: {error}"));
    let writer = pair
        .master
        .take_writer()
        .unwrap_or_else(|error| panic!("open exec PTY writer: {error}"));
    let reader = pair
        .master
        .try_clone_reader()
        .unwrap_or_else(|error| panic!("open exec PTY reader: {error}"));
    let child = pair
        .slave
        .spawn_command(command)
        .unwrap_or_else(|error| panic!("spawn cruise exec PTY: {error}"));
    drop(pair.slave);
    let output = Arc::new(Mutex::new(Vec::new()));
    let mut pty = ExecPty {
        child,
        writer: Some(writer),
        master: Some(pair.master),
        reader: None,
        output: Arc::clone(&output),
    };
    pty.reader = Some(thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        let mut reader = reader;
        while let Ok(n) = reader.read(&mut buffer) {
            if n == 0 {
                break;
            }
            output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(&buffer[..n]);
        }
    }));
    pty
}

fn send_pty(pty: &mut ExecPty, bytes: &[u8], purpose: &str) {
    let writer = pty
        .writer
        .as_mut()
        .unwrap_or_else(|| panic!("PTY writer already closed"));
    writer
        .write_all(bytes)
        .and_then(|()| writer.flush())
        .unwrap_or_else(|error| panic!("{purpose}: {error}"));
}
fn pty_transcript(pty: &ExecPty) -> String {
    let output = pty
        .output
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    String::from_utf8_lossy(&output).into_owned()
}

fn answer_cursor_queries(pty: &mut ExecPty, transcript: &str, answered: &mut usize) {
    let queries = transcript.matches("\x1b[6n").count();
    while *answered < queries {
        send_pty(pty, b"\x1b[1;1R", "answer PTY cursor query");
        *answered += 1;
    }
}

fn wait_for_pty_text(pty: &mut ExecPty, text: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut cursor_queries_answered = 0;
    loop {
        let transcript = pty_transcript(pty);
        answer_cursor_queries(pty, &transcript, &mut cursor_queries_answered);
        if transcript.contains(text) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "PTY did not reach the expected interaction point: {transcript}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_pty_exit(pty: &mut ExecPty, timeout: Duration) -> (portable_pty::ExitStatus, String) {
    let deadline = Instant::now() + timeout;
    let mut cursor_queries_answered = 0;
    loop {
        if let Some(status) = pty
            .child
            .try_wait()
            .unwrap_or_else(|error| panic!("wait for PTY cruise CLI: {error}"))
        {
            return (status, pty_transcript(pty));
        }
        let transcript = pty_transcript(pty);
        answer_cursor_queries(pty, &transcript, &mut cursor_queries_answered);
        assert!(
            Instant::now() < deadline,
            "PTY cruise CLI did not exit before the deadline: {transcript}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn escape_option_menu(pty: &mut ExecPty) -> (portable_pty::ExitStatus, String) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut cursor_queries_answered = 0;
    loop {
        if let Some(status) = pty
            .child
            .try_wait()
            .unwrap_or_else(|error| panic!("wait for PTY cruise exec: {error}"))
        {
            let output = pty
                .output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            return (status, String::from_utf8_lossy(&output).into_owned());
        }
        let output = pty
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let transcript = String::from_utf8_lossy(&output);
        let queries = transcript.matches("\x1b[6n").count();
        while cursor_queries_answered < queries {
            send_pty(pty, b"\x1b[1;1R", "answer PTY cursor query");
            cursor_queries_answered += 1;
        }
        if transcript.contains("Continue") {
            send_pty(pty, b"\x1b", "send PTY Escape");
        }
        assert!(
            Instant::now() < deadline,
            "cruise exec did not exit after the option menu Escape: {transcript}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn paused_exec_exports_paused_root_and_keeps_session_resumable() {
    let collector = Collector::start();
    let fixture = Fixture::new(
        "steps:\n  choose:\n    option:\n      - selector: Continue\n        next: done\n  done:\n    command: 'true'\n",
    );
    let pty_env = otlp_env(&collector.endpoint());
    let mut pty = start_exec_pty(&fixture, &borrowed_env(&pty_env), "paused-input");
    let (status, transcript) = escape_option_menu(&mut pty);
    drop(pty);
    assert!(status.success(), "cruise exec failed: {transcript}");

    let requests = collector.wait_for_paths(&["/v1/traces"]);
    let exported_spans = spans(&requests);
    let execution = named_span(
        &exported_spans,
        "cruise.execution",
        "cruise.execution.kind",
        "exec",
    );
    let workflow_phase = named_span(
        &exported_spans,
        "cruise.phase",
        "cruise.phase.name",
        "workflow",
    );
    let workflow = named_span(
        &exported_spans,
        "cruise.workflow",
        "cruise.operation",
        "workflow",
    );
    assert!(
        execution.parent_span_id.is_empty(),
        "execution is not a trace root"
    );
    assert_eq!(workflow_phase.parent_span_id, execution.span_id);
    assert_eq!(workflow.parent_span_id, workflow_phase.span_id);
    assert_eq!(
        execution
            .attributes
            .get("cruise.outcome")
            .map(String::as_str),
        Some("paused"),
        "execution outcome: {execution:#?}"
    );
    assert_eq!(
        execution.attributes.get("error.type").map(String::as_str),
        Some("paused"),
        "execution error classification: {execution:#?}"
    );

    let session_id = execution
        .attributes
        .get("cruise.session.id")
        .unwrap_or_else(|| panic!("execution span omitted session id: {execution:#?}"));
    let sessions = SessionManager::new(fixture.root.path().join("data/cruise"));
    let session = sessions
        .load(session_id)
        .unwrap_or_else(|error| panic!("paused exec session is not persisted: {error}"));
    assert!(
        session.phase.is_runnable(),
        "session is not resumable: {session:#?}"
    );
    assert!(
        sessions.dag_path(session_id).is_file(),
        "paused exec checkpoint is missing"
    );
}

fn assert_successful_trace_spans(exported_spans: &[SpanRecord], execution: &SpanRecord) {
    for span in exported_spans {
        assert_eq!(
            span.trace_id, execution.trace_id,
            "span escaped execution trace: {span:#?}"
        );
        assert_eq!(
            span.status_code,
            StatusCode::Ok as i32,
            "unexpected status: {span:#?}"
        );
        assert!(
            !span.attributes.contains_key("error.type"),
            "successful operation unexpectedly has error.type: {span:#?}"
        );
    }
}

fn assert_exec_run_isolation(fixture: &Fixture, session_id: &str, trace_id: &[u8]) {
    let collector = Collector::start();
    let mut env = otlp_env(&collector.endpoint());
    env.push((
        "OTEL_SERVICE_NAME".into(),
        "cruise-telemetry-contract".into(),
    ));
    let output = fixture.run(&borrowed_env(&env), "second-workflow-input-secret");
    assert_success(&output);
    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let (second_session_id, second_trace_id) = assert_parallel_trace(&requests);
    assert_ne!(
        session_id, second_session_id,
        "separate executions reused a session id"
    );
    assert_ne!(
        trace_id, second_trace_id,
        "separate executions reused a trace"
    );
}

fn assert_parallel_trace(requests: &[CapturedRequest]) -> (String, Vec<u8>) {
    let exported_spans = spans(requests);
    let execution = named_span(
        &exported_spans,
        "cruise.execution",
        "cruise.execution.kind",
        "exec",
    );
    let prepare = named_span(
        &exported_spans,
        "cruise.phase",
        "cruise.phase.name",
        "prepare",
    );
    let workflow_phase = named_span(
        &exported_spans,
        "cruise.phase",
        "cruise.phase.name",
        "workflow",
    );
    let workflow = named_span(
        &exported_spans,
        "cruise.workflow",
        "cruise.operation",
        "workflow",
    );
    assert_eq!(
        execution
            .resource_attributes
            .get("service.name")
            .map(String::as_str),
        Some("cruise-telemetry-contract")
    );
    let session_id = execution
        .attributes
        .get("cruise.session.id")
        .unwrap_or_else(|| panic!("execution missing session correlation: {execution:#?}"));
    assert_eq!(
        execution
            .attributes
            .get("cruise.workflow.kind")
            .map(String::as_str),
        Some("file")
    );
    assert_eq!(
        execution
            .attributes
            .get("cruise.workflow.name")
            .map(String::as_str),
        Some("workflow.yaml")
    );
    let step_spans = exported_spans
        .iter()
        .filter(|span| span.name == "cruise.step")
        .collect::<Vec<_>>();
    assert_eq!(
        step_spans.len(),
        3,
        "only the parallel parent and two executable children should have step spans: {step_spans:#?}"
    );
    assert!(
        step_spans.iter().all(|span| {
            span.attributes.get("cruise.step.name").is_none_or(|name| {
                !name.ends_with("skipped_child") && !name.ends_with("missing_child")
            })
        }),
        "non-executable children must not have spans: {step_spans:#?}"
    );
    let fan = named_span(&exported_spans, "cruise.step", "cruise.step.name", "fan");
    let prompt_child = named_span(&exported_spans, "cruise.step", "cruise.step.type", "prompt");
    let shell_child = named_span(
        &exported_spans,
        "cruise.step",
        "cruise.step.type",
        "command",
    );
    let agent = named_span(&exported_spans, "cruise.agent", "cruise.operation", "agent");
    assert!(
        execution.parent_span_id.is_empty(),
        "execution must be the trace root"
    );
    assert_successful_trace_spans(&exported_spans, execution);
    assert_eq!(
        fan.attributes.get("cruise.step.type").map(String::as_str),
        Some("parallel")
    );
    assert_eq!(
        agent
            .attributes
            .get("cruise.agent.backend")
            .map(String::as_str),
        Some("command")
    );
    assert_eq!(prepare.parent_span_id, execution.span_id);
    assert_eq!(workflow_phase.parent_span_id, execution.span_id);
    assert_eq!(workflow.parent_span_id, workflow_phase.span_id);
    assert_eq!(fan.parent_span_id, workflow.span_id);
    assert_eq!(prompt_child.parent_span_id, fan.span_id);
    assert_eq!(shell_child.parent_span_id, fan.span_id);
    assert_eq!(agent.parent_span_id, prompt_child.span_id);
    (session_id.clone(), execution.trace_id.clone())
}

fn run_application_planning_helper() {
    let _process = cruise::test_support::lock_process();
    let temp = TempDir::new().unwrap_or_else(|error| panic!("application fixture: {error}"));
    let mut environment = cruise::test_support::set_fake_home(temp.path());
    environment.extend([
        cruise::test_support::EnvGuard::remove("CRUISE_MODEL"),
        cruise::test_support::EnvGuard::remove("CRUISE_PLAN_MODEL"),
    ]);
    let telemetry = cruise::telemetry::init()
        .unwrap_or_else(|error| panic!("initialize application telemetry: {error}"));
    let app = CruiseApplication::new(SessionManager::new(temp.path().join("sessions")));
    let first_id = create_application_session(
        &app,
        temp.path(),
        "application-first-input-secret",
        "provider/application-first",
    );
    let second_id = create_application_session(
        &app,
        temp.path(),
        "application-second-input-secret",
        "provider/application-second",
    );
    let sink: Arc<dyn ApplicationEventSink> = Arc::new(|_event: ApplicationEvent| Ok(()));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("build application test runtime: {error}"));
    let (first_result, second_result) = runtime.block_on(async {
        tokio::join!(
            app.generate(&first_id, PlanRequest::default(), Arc::clone(&sink)),
            app.generate(&second_id, PlanRequest::default(), Arc::clone(&sink)),
        )
    });
    first_result.unwrap_or_else(|error| panic!("first application plan: {error}"));
    second_result.unwrap_or_else(|error| panic!("second application plan: {error}"));
    drop(runtime);
    telemetry.shutdown();
    let result_path = std::env::var_os("CRUISE_TELEMETRY_APPLICATION_RESULT")
        .unwrap_or_else(|| panic!("missing application telemetry result path"));
    std::fs::write(result_path, format!("{first_id}\n{second_id}\n"))
        .unwrap_or_else(|error| panic!("write application session ids: {error}"));
}

fn assert_application_planning_traces(collector: &Collector, result_path: &std::path::Path) {
    let session_ids = std::fs::read_to_string(result_path)
        .unwrap_or_else(|error| panic!("read application session ids: {error}"));
    let mut session_ids = session_ids.lines();
    let first_id = session_ids
        .next()
        .unwrap_or_else(|| panic!("application helper omitted first session id"));
    let second_id = session_ids
        .next()
        .unwrap_or_else(|| panic!("application helper omitted second session id"));
    assert_ne!(first_id, second_id, "parallel sessions reused an id");

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let exported_spans = spans(&requests);
    let executions = exported_spans
        .iter()
        .filter(|span| span.name == "cruise.execution")
        .collect::<Vec<_>>();
    assert_eq!(
        executions.len(),
        2,
        "expected one root execution per session"
    );
    let first_root = executions
        .iter()
        .find(|span| {
            span.attributes
                .get("cruise.session.id")
                .is_some_and(|id| id == first_id)
        })
        .unwrap_or_else(|| panic!("first session trace missing: {executions:#?}"));
    let second_root = executions
        .iter()
        .find(|span| {
            span.attributes
                .get("cruise.session.id")
                .is_some_and(|id| id == second_id)
        })
        .unwrap_or_else(|| panic!("second session trace missing: {executions:#?}"));
    assert_ne!(first_root.trace_id, second_root.trace_id);
    for root in [*first_root, *second_root] {
        assert!(
            root.parent_span_id.is_empty(),
            "execution should be a trace root"
        );
        assert_eq!(
            root.attributes
                .get("cruise.execution.kind")
                .map(String::as_str),
            Some("generate")
        );
        assert!(root.attributes.contains_key("cruise.workflow.kind"));
        assert!(root.attributes.contains_key("cruise.workflow.name"));
        let planning = exported_spans
            .iter()
            .find(|span| {
                span.name == "cruise.phase"
                    && span.attributes.get("cruise.phase.name").map(String::as_str)
                        == Some("planning")
                    && span.trace_id == root.trace_id
            })
            .unwrap_or_else(|| panic!("planning phase missing for {root:#?}"));
        assert_eq!(planning.parent_span_id, root.span_id);
        let agent = exported_spans
            .iter()
            .find(|span| span.name == "cruise.agent" && span.trace_id == root.trace_id)
            .unwrap_or_else(|| panic!("planning agent missing for {root:#?}"));
        assert_eq!(agent.parent_span_id, planning.span_id);
    }
    let result_path = result_path.to_string_lossy().into_owned();
    assert_payload_omits(
        &requests,
        &[
            "application-first-input-secret",
            "application-second-input-secret",
            "application-credential-secret",
            result_path.as_str(),
        ],
    );
}

fn run_application_failed_step_helper() {
    let _process = cruise::test_support::lock_process();
    let temp = TempDir::new().unwrap_or_else(|error| panic!("application run fixture: {error}"));
    let _environment = cruise::test_support::set_fake_home(temp.path());
    let telemetry = cruise::telemetry::init()
        .unwrap_or_else(|error| panic!("initialize application telemetry: {error}"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project)
        .unwrap_or_else(|error| panic!("create application project: {error}"));
    cruise::test_support::init_git_repo(&project);

    let app = CruiseApplication::new(SessionManager::new(temp.path().join("sessions")));
    let session = app
        .create_session(NewSessionRequest {
            input: "application-run-input-secret".to_owned(),
            base_dir: project,
            config_path: None,
            config_yaml: Some(
                "command: [sh, -c, 'cat >/dev/null']\ncreate_pr: false\nsteps:\n  failing:\n    command: 'exit 7'\n"
                    .to_owned(),
            ),
            repo: None,
            workspace_mode: WorkspaceMode::Worktree,
            allow_dirty_working_tree: false,
            attachments: vec![],
            skipped_steps: vec![],
            flags: std::collections::BTreeMap::new(),
        })
        .unwrap_or_else(|error| panic!("create application run session: {error}"));
    let sink: Arc<dyn ApplicationEventSink> = Arc::new(|_event: ApplicationEvent| Ok(()));
    app.use_input_as_plan(&session.id, &*sink)
        .unwrap_or_else(|error| panic!("prepare application run plan: {error}"));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("build application run runtime: {error}"));
    let completed = runtime
        .block_on(app.run_with_log_sink(
            &session.id,
            RunRequest::default(),
            Arc::clone(&sink),
            None,
        ))
        .unwrap_or_else(|error| panic!("application run unexpectedly returned an error: {error}"));
    assert_eq!(
        completed.phase,
        SessionPhase::Completed,
        "unresolved step failures must preserve the domain Completed state"
    );
    assert_eq!(
        app.read_session(&session.id)
            .unwrap_or_else(|error| panic!("read completed application session: {error}"))
            .phase,
        SessionPhase::Completed,
        "completed domain state must be persisted"
    );
    drop(runtime);
    telemetry.shutdown();

    let result_path = std::env::var_os("CRUISE_TELEMETRY_APPLICATION_RUN_RESULT")
        .unwrap_or_else(|| panic!("missing application run result path"));
    std::fs::write(
        result_path,
        format!("{}\n{}", session.id, temp.path().display()),
    )
    .unwrap_or_else(|error| panic!("write application run result: {error}"));
}

fn assert_application_failed_run_trace(collector: &Collector, result_path: &std::path::Path) {
    let helper_result = std::fs::read_to_string(result_path)
        .unwrap_or_else(|error| panic!("read application run result: {error}"));
    let mut result = helper_result.lines();
    let session_id = result
        .next()
        .unwrap_or_else(|| panic!("application run helper omitted session id"));
    let helper_path = result
        .next()
        .unwrap_or_else(|| panic!("application run helper omitted fixture path"));

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let exported_spans = spans(&requests);
    let execution = named_span(
        &exported_spans,
        "cruise.execution",
        "cruise.execution.kind",
        "run",
    );
    let failing = named_span(
        &exported_spans,
        "cruise.step",
        "cruise.step.name",
        "failing",
    );
    assert_eq!(execution.parent_span_id, [0_u8; 0]);
    assert_eq!(
        execution
            .attributes
            .get("cruise.session.id")
            .map(String::as_str),
        Some(session_id)
    );
    assert_eq!(execution.status_code, StatusCode::Error as i32);
    assert_eq!(
        execution
            .attributes
            .get("cruise.outcome")
            .map(String::as_str),
        Some("failed")
    );
    assert_eq!(
        execution.attributes.get("error.type").map(String::as_str),
        Some("step_failed")
    );
    assert_eq!(failing.status_code, StatusCode::Error as i32);
    assert_eq!(
        failing.attributes.get("error.type").map(String::as_str),
        Some("command_exit")
    );
    let result_path = result_path.to_string_lossy().into_owned();
    assert_payload_omits(
        &requests,
        &[
            "application-run-input-secret",
            "exit 7",
            helper_path,
            result_path.as_str(),
        ],
    );
}

#[test]
fn application_run_keeps_completed_domain_state_but_errors_root_for_failed_steps() {
    if std::env::var_os("CRUISE_TELEMETRY_APPLICATION_RUN_HELPER").is_some() {
        run_application_failed_step_helper();
        return;
    }

    let collector = Collector::start();
    let result_dir =
        TempDir::new().unwrap_or_else(|error| panic!("create application run result dir: {error}"));
    let result_path = result_dir.path().join("run-result.txt");
    let mut child = Command::new(
        std::env::current_exe().unwrap_or_else(|error| panic!("test executable path: {error}")),
    );
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy();
        if (name.starts_with("CRUISE_") && name != "CRUISE_GUARD_COMMON_DIR")
            || name.starts_with("OTEL_")
            || name.starts_with("JCODE_")
        {
            child.env_remove(name.as_ref());
        }
    }
    let output = child
        .args([
            "--exact",
            "application_run_keeps_completed_domain_state_but_errors_root_for_failed_steps",
            "--nocapture",
        ])
        .env("CRUISE_TELEMETRY_APPLICATION_RUN_HELPER", "1")
        .env("CRUISE_TELEMETRY_APPLICATION_RUN_RESULT", &result_path)
        .env("CRUISE_OBSERVABILITY_ENABLED", "true")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", collector.endpoint())
        .env("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf")
        .env("OTEL_EXPORTER_OTLP_TIMEOUT", "3000")
        .output()
        .unwrap_or_else(|error| panic!("run application telemetry subprocess: {error}"));
    assert_success(&output);
    assert_application_failed_run_trace(&collector, &result_path);
}

#[test]
fn application_api_parallel_planning_keeps_session_traces_isolated() {
    if std::env::var_os("CRUISE_TELEMETRY_APPLICATION_HELPER").is_some() {
        run_application_planning_helper();
        return;
    }

    let collector = Collector::start();
    let endpoint = collector.endpoint();
    let result_dir =
        TempDir::new().unwrap_or_else(|error| panic!("create application result dir: {error}"));
    let result_path = result_dir.path().join("sessions.txt");
    let mut child = Command::new(
        std::env::current_exe().unwrap_or_else(|error| panic!("test executable path: {error}")),
    );
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy();
        if (name.starts_with("CRUISE_") && name != "CRUISE_GUARD_COMMON_DIR")
            || name.starts_with("OTEL_")
            || name.starts_with("JCODE_")
        {
            child.env_remove(name.as_ref());
        }
    }
    let output = child
        .args([
            "--exact",
            "application_api_parallel_planning_keeps_session_traces_isolated",
            "--nocapture",
        ])
        .env("CRUISE_TELEMETRY_APPLICATION_HELPER", "1")
        .env("CRUISE_TELEMETRY_APPLICATION_RESULT", &result_path)
        .env("CRUISE_OBSERVABILITY_ENABLED", "true")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint)
        .env("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf")
        .env("OTEL_EXPORTER_OTLP_TIMEOUT", "3000")
        .env("OPENAI_API_KEY", "application-credential-secret")
        .output()
        .unwrap_or_else(|error| panic!("run application telemetry subprocess: {error}"));
    assert_success(&output);
    assert_application_planning_traces(&collector, &result_path);
}

fn assert_success_operation_metrics(
    count_points: &[MetricPoint],
    duration_points: &[DurationPoint],
    session_id: &str,
) {
    assert!(
        !count_points.is_empty(),
        "operation counter was not exported"
    );
    assert!(
        !duration_points.is_empty(),
        "operation duration histogram was not exported"
    );
    assert!(
        count_points.iter().all(|point| point.value > 0),
        "operation counter has a non-positive value: {count_points:?}"
    );
    assert!(
        duration_points
            .iter()
            .all(|point| point.count > 0 && point.sum_seconds > 0.0),
        "operation duration has an empty or non-positive observation: {duration_points:?}"
    );
    for attributes in count_points
        .iter()
        .map(|point| &point.attributes)
        .chain(duration_points.iter().map(|point| &point.attributes))
    {
        assert_eq!(
            attributes.len(),
            2,
            "unexpected metric labels: {attributes:?}"
        );
        assert!(attributes.contains_key("cruise.operation"));
        assert_eq!(
            attributes.get("cruise.outcome").map(String::as_str),
            Some("success")
        );
        assert!(
            attributes
                .values()
                .all(|value| value != session_id && !value.contains("workflow.yaml")),
            "metric labels contain execution identity: {attributes:?}"
        );
    }
}

fn assert_rate_limit_retry_spans(exported_spans: &[SpanRecord]) {
    let retried = named_span(exported_spans, "cruise.step", "cruise.step.name", "retried");
    let attempts = exported_spans
        .iter()
        .filter(|span| span.name == "cruise.attempt")
        .collect::<Vec<_>>();
    assert_eq!(
        attempts.len(),
        2,
        "expected one initial attempt and one retry"
    );
    for (index, attempt) in attempts.iter().enumerate() {
        assert_eq!(
            attempt.int_attributes.get("cruise.retry.attempt"),
            Some(&i64::try_from(index + 1).unwrap_or_else(|error| panic!("{error}"))),
            "attempt numbering must be 1-based: {attempt:#?}"
        );
        assert_eq!(
            attempt
                .attributes
                .get("cruise.agent.backend")
                .map(String::as_str),
            Some("command")
        );
        assert_eq!(attempt.parent_span_id, retried.span_id);
        if index == 0 {
            assert_eq!(
                attempt.attributes.get("error.type").map(String::as_str),
                Some("rate_limit"),
                "rate-limited attempt must retain the retry classifier: {attempt:#?}"
            );
        }
    }
    let retry_events = exported_spans
        .iter()
        .flat_map(|span| span.events.iter())
        .filter(|event| event.name == "cruise.retry")
        .collect::<Vec<_>>();
    assert_eq!(retry_events.len(), 1, "expected one retry event");
    let retry = retry_events[0];
    assert_eq!(
        retry
            .attributes
            .get("cruise.retry.action")
            .map(String::as_str),
        Some("backoff")
    );
    assert_eq!(
        retry
            .attributes
            .get("cruise.retry.reason")
            .map(String::as_str),
        Some("rate_limit")
    );
    assert_eq!(retry.int_attributes.get("cruise.retry.attempt"), Some(&1));
    assert!(!retry.attributes.contains_key("cruise.retry.from_model"));
    assert!(!retry.attributes.contains_key("cruise.retry.to_model"));
}

#[test]
fn enabled_exec_exports_safe_parented_traces_and_operation_metrics() {
    let collector = Collector::start();
    let yaml = r#"
command:
  - sh
  - -c
  - 'cat >/dev/null; printf "%s" "backend-output-secret"'
steps:
  fan:
    parallel:
      prompt_child:
        prompt: "prompt-body-secret"
      shell_child:
        command: "printf '%s' command-source-secret; sleep 0.02"
      skipped_child:
        command: "printf '%s' skipped-child-command-secret"
        skip: true
      missing_child:
        command: "printf '%s' missing-child-command-secret"
        when:
          exists: ".telemetry-child-must-not-exist"
"#;
    let fixture = Fixture::new(yaml);
    let mut env = otlp_env(&collector.endpoint());
    env.push((
        "OTEL_EXPORTER_OTLP_HEADERS".into(),
        "Authorization=Bearer header-value-secret".into(),
    ));
    env.push((
        "OTEL_SERVICE_NAME".into(),
        "cruise-telemetry-contract".into(),
    ));
    env.push(("OPENAI_API_KEY".into(), "credential-value-secret".into()));
    let output = fixture.run(&borrowed_env(&env), "workflow-input-secret");
    assert_success(&output);

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    assert!(
        requests.iter().any(|request| {
            request
                .headers
                .to_ascii_lowercase()
                .contains("authorization: bearer header-value-secret")
        }),
        "OTLP auth header was not sent as an HTTP header"
    );

    let root_path = fixture.root.path().to_string_lossy().into_owned();
    assert_payload_omits(
        &requests,
        &[
            "workflow-input-secret",
            "prompt-body-secret",
            "backend-output-secret",
            "command-source-secret",
            "skipped-child-command-secret",
            "missing-child-command-secret",
            "header-value-secret",
            "credential-value-secret",
            root_path.as_str(),
        ],
    );

    let (session_id, trace_id) = assert_parallel_trace(&requests);

    let (count_points, duration_points) = metrics(&requests);
    assert_success_operation_metrics(&count_points, &duration_points, &session_id);
    let successful_step_counts = count_points
        .iter()
        .filter(|point| {
            point.attributes.get("cruise.operation").map(String::as_str) == Some("step")
                && point.attributes.get("cruise.outcome").map(String::as_str) == Some("success")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        successful_step_counts.len(),
        1,
        "successful step metrics should aggregate only executable steps: {successful_step_counts:#?}"
    );
    assert_eq!(successful_step_counts[0].value, 3);
    let successful_step_durations = duration_points
        .iter()
        .filter(|point| {
            point.attributes.get("cruise.operation").map(String::as_str) == Some("step")
                && point.attributes.get("cruise.outcome").map(String::as_str) == Some("success")
        })
        .collect::<Vec<_>>();
    assert_eq!(successful_step_durations.len(), 1);
    assert_eq!(successful_step_durations[0].count, 3);
    assert_exec_run_isolation(&fixture, &session_id, &trace_id);
}

#[test]
fn no_file_changes_retry_attempts_keep_failed_step_telemetry() {
    let collector = Collector::start();
    let fixture = Fixture::new(
        "steps:\n  retrying:\n    command: \"if [ ! -f \\\"$HOME/attempt\\\" ]; then touch \\\"$HOME/attempt\\\"; else touch changed.txt; fi\"\n    if:\n      no-file-changes: retry\n",
    );
    let output = fixture.run(
        &borrowed_env(&otlp_env(&collector.endpoint())),
        "retry-input-secret",
    );
    assert_success(&output);

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let exported_spans = spans(&requests);
    let execution = named_span(
        &exported_spans,
        "cruise.execution",
        "cruise.execution.kind",
        "exec",
    );
    let workflow = named_span(
        &exported_spans,
        "cruise.workflow",
        "cruise.operation",
        "workflow",
    );
    let retry_attempts = exported_spans
        .iter()
        .filter(|span| {
            span.name == "cruise.step"
                && span.attributes.get("cruise.step.name").map(String::as_str) == Some("retrying")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        retry_attempts.len(),
        2,
        "no-file-changes retry must execute twice: {retry_attempts:#?}"
    );
    let failed = retry_attempts
        .iter()
        .filter(|span| span.status_code == StatusCode::Error as i32)
        .collect::<Vec<_>>();
    assert_eq!(
        failed.len(),
        1,
        "first no-change attempt must fail: {retry_attempts:#?}"
    );
    assert_eq!(
        failed[0].attributes.get("error.type").map(String::as_str),
        Some("step_no_file_changes"),
        "no-file-changes failure classification: {:#?}",
        failed[0]
    );
    assert_eq!(execution.status_code, StatusCode::Ok as i32);
    assert_eq!(workflow.status_code, StatusCode::Ok as i32);
    let (count_points, duration_points) = metrics(&requests);
    let failed_step_counts = count_points
        .iter()
        .filter(|point| {
            point.attributes.get("cruise.operation").map(String::as_str) == Some("step")
                && matches!(
                    point.attributes.get("cruise.outcome").map(String::as_str),
                    Some("error" | "failed")
                )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        failed_step_counts.len(),
        1,
        "one postcondition failure must be counted: {failed_step_counts:#?}"
    );
    assert_eq!(failed_step_counts[0].value, 1);
    assert_eq!(failed_step_counts[0].attributes.len(), 2);
    assert!(
        duration_points.iter().any(|point| {
            point.attributes.get("cruise.operation").map(String::as_str) == Some("step")
                && matches!(
                    point.attributes.get("cruise.outcome").map(String::as_str),
                    Some("error" | "failed")
                )
                && point.count == 1
        }),
        "failed postcondition duration metric missing or unsafe: {duration_points:#?}"
    );

    let project_path = fixture.project.to_string_lossy().into_owned();
    let home_path = fixture.home.to_string_lossy().into_owned();
    assert_payload_omits(
        &requests,
        &[
            "retry-input-secret",
            project_path.as_str(),
            home_path.as_str(),
            "touch changed.txt",
        ],
    );
}

#[test]
fn no_file_changes_failure_is_recorded_before_recovery() {
    let collector = Collector::start();
    let fixture = Fixture::new(
        "steps:\n  unchanged:\n    command: 'true'\n    if:\n      no-file-changes: failed\n      fail: recover\n  recover:\n    command: 'touch recovered.txt'\n",
    );
    let output = fixture.run(
        &borrowed_env(&otlp_env(&collector.endpoint())),
        "no-change-recovery-input-secret",
    );
    assert_success(&output);

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let exported_spans = spans(&requests);
    let execution = named_span(
        &exported_spans,
        "cruise.execution",
        "cruise.execution.kind",
        "exec",
    );
    let workflow = named_span(
        &exported_spans,
        "cruise.workflow",
        "cruise.operation",
        "workflow",
    );
    let unchanged = named_span(
        &exported_spans,
        "cruise.step",
        "cruise.step.name",
        "unchanged",
    );
    let recovered = named_span(
        &exported_spans,
        "cruise.step",
        "cruise.step.name",
        "recover",
    );
    assert_eq!(execution.status_code, StatusCode::Ok as i32);
    assert_eq!(workflow.status_code, StatusCode::Ok as i32);
    assert_eq!(
        unchanged.status_code,
        StatusCode::Error as i32,
        "postcondition failure must stay visible after recovery: {unchanged:#?}"
    );
    assert_eq!(
        unchanged.attributes.get("error.type").map(String::as_str),
        Some("step_no_file_changes")
    );
    assert_eq!(recovered.status_code, StatusCode::Ok as i32);
    assert_eq!(unchanged.parent_span_id, workflow.span_id);
    assert_eq!(recovered.parent_span_id, workflow.span_id);
    let project_path = fixture.project.to_string_lossy().into_owned();
    assert_payload_omits(
        &requests,
        &[
            "no-change-recovery-input-secret",
            project_path.as_str(),
            "touch recovered.txt",
        ],
    );
}

#[test]
fn skip_planning_foreground_and_background_omit_planning_telemetry() {
    let collector = Collector::start();
    let fixture = Fixture::new(
        "command: [sh, -c, 'true']\nsteps:\n  local:\n    command: 'touch local.txt'\n",
    );
    let env = otlp_env(&collector.endpoint());
    let borrowed = borrowed_env(&env);
    let config = fixture.config.to_string_lossy().into_owned();

    let foreground = fixture
        .cli_command(
            &[
                "plan",
                "--skip-planning",
                "--config",
                config.as_str(),
                "foreground-plan-input-secret",
            ],
            &borrowed,
        )
        .output()
        .unwrap_or_else(|error| panic!("run foreground skip-planning: {error}"));
    assert_success(&foreground);

    let mut background_env = borrowed.clone();
    background_env.push(("CRUISE_CONFIG", config.as_str()));
    let mut background =
        fixture.cli_command(&["--plan", "stdin", "--skip-planning"], &background_env);
    let mut child = background
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn background skip-planning: {error}"));
    child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("background plan stdin"))
        .write_all(b"background-plan-input-secret")
        .unwrap_or_else(|error| panic!("write background plan input: {error}"));
    let background = child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("wait for background skip-planning: {error}"));
    assert_success(&background);

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let exported_spans = spans(&requests);
    let roots = exported_spans
        .iter()
        .filter(|span| {
            span.name == "cruise.execution"
                && span
                    .attributes
                    .get("cruise.execution.kind")
                    .map(String::as_str)
                    == Some("plan")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        roots.len(),
        2,
        "both planning entry points need roots: {roots:#?}"
    );
    assert_ne!(roots[0].trace_id, roots[1].trace_id);
    assert!(roots.iter().all(|root| root.parent_span_id.is_empty()));
    assert!(
        roots
            .iter()
            .all(|root| root.attributes.contains_key("cruise.session.id"))
    );
    assert!(
        exported_spans.iter().all(|span| {
            span.name != "cruise.agent"
                && !(span.name == "cruise.phase"
                    && span.attributes.get("cruise.phase.name").map(String::as_str)
                        == Some("planning"))
        }),
        "skip-planning must not emit agent/planning spans: {exported_spans:#?}"
    );
    let fixture_path = fixture.root.path().to_string_lossy().into_owned();
    assert_payload_omits(
        &requests,
        &[
            "foreground-plan-input-secret",
            "background-plan-input-secret",
            fixture_path.as_str(),
        ],
    );
}

#[test]
fn failed_repo_clone_keeps_plan_root_and_preallocated_session_id() {
    let collector = Collector::start();
    let fixture = Fixture::new("steps:\n  local:\n    command: 'true'\n");
    let fake_bin = fixture.root.path().join("fake-gh/bin");
    std::fs::create_dir_all(&fake_bin)
        .unwrap_or_else(|error| panic!("create fake forge bin: {error}"));
    cruise::test_support::write_executable_script(
        &fake_bin.join("gh"),
        "#!/bin/sh\nif [ \"$1\" = '--version' ]; then\n  printf 'gh version 2.0.0\\n'\n  exit 0\nfi\nprintf 'repository clone failed\\n' >&2\nexit 2\n",
    );
    let search_path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let env = otlp_env(&collector.endpoint());
    let mut borrowed = borrowed_env(&env);
    borrowed.push(("PATH", search_path.as_str()));
    let output = fixture
        .cli_command(
            &[
                "plan",
                "--repo",
                "owner/repository",
                "clone-failure-input-secret",
            ],
            &borrowed,
        )
        .output()
        .unwrap_or_else(|error| panic!("run failed repository planning: {error}"));
    assert!(
        !output.status.success(),
        "fake forge clone failure unexpectedly succeeded: {}",
        output_text(&output)
    );

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let exported_spans = spans(&requests);
    let root = named_span(
        &exported_spans,
        "cruise.execution",
        "cruise.execution.kind",
        "plan",
    );
    assert_eq!(root.parent_span_id, [0_u8; 0]);
    assert_eq!(root.status_code, StatusCode::Error as i32);
    assert!(root.attributes.contains_key("error.type"));
    assert!(
        root.attributes
            .get("cruise.session.id")
            .is_some_and(|id| !id.is_empty())
    );
    assert!(
        !fixture
            .root
            .path()
            .join("data/cruise/sessions")
            .join(&root.attributes["cruise.session.id"])
            .exists(),
        "failed clone must not leave a partial session"
    );
    let fixture_path = fixture.root.path().to_string_lossy().into_owned();
    assert_payload_omits(
        &requests,
        &[
            "clone-failure-input-secret",
            "owner/repository",
            fixture_path.as_str(),
        ],
    );
}

fn approval_fixture() -> Fixture {
    Fixture::new(
        r##"
command:
  - sh
  - -c
  - 'cat >/dev/null; printf "%s" "$MOCK_PLAN_CONTENT" > "$PLAN_FILE"'
env:
  PLAN_FILE: "{plan}"
  MOCK_PLAN_CONTENT: "# Plan\n\nStep 1.\n"
create_pr: false
steps:
  planning:
    prompt: "plan-prompt-secret"
  execute:
    command: "touch execution-marker-secret"
 "##,
    )
}

#[test]
fn execute_now_finishes_plan_root_before_separate_run_root() {
    let collector = Collector::start();
    let fixture = approval_fixture();
    let env = otlp_env(&collector.endpoint());
    let borrowed = borrowed_env(&env);
    let config = fixture.config.to_string_lossy().into_owned();
    let mut pty = start_cli_pty(
        &fixture,
        &[
            "plan",
            "--config",
            config.as_str(),
            "execute-now-input-secret",
        ],
        &borrowed,
    );
    wait_for_pty_text(&mut pty, "Action:", Duration::from_secs(30));
    send_pty(&mut pty, b"\x1b[B\x1b[B\x1b[B\r", "select Execute now");
    wait_for_pty_text(&mut pty, "Steps to skip", Duration::from_secs(15));
    send_pty(&mut pty, b"\r", "confirm empty skip selection");
    wait_for_pty_text(
        &mut pty,
        "Where should cruise execute?",
        Duration::from_secs(15),
    );
    send_pty(&mut pty, b"\x1b[B\r", "select current-branch execution");
    let (status, _transcript) = wait_for_pty_exit(&mut pty, Duration::from_secs(60));
    assert_eq!(status.exit_code(), 0, "Execute now CLI command failed");
    drop(pty);

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let exported_spans = spans(&requests);
    let plan = named_span(
        &exported_spans,
        "cruise.execution",
        "cruise.execution.kind",
        "plan",
    );
    let run = named_span(
        &exported_spans,
        "cruise.execution",
        "cruise.execution.kind",
        "run",
    );
    assert_eq!(plan.parent_span_id, [0_u8; 0]);
    assert_eq!(run.parent_span_id, [0_u8; 0]);
    assert_ne!(
        plan.trace_id, run.trace_id,
        "Execute now must start run outside the plan trace"
    );
    assert!(
        plan.end_time_unix_nano <= run.start_time_unix_nano,
        "plan root overlapped the run root: {plan:#?} {run:#?}"
    );
    let session_id = plan
        .attributes
        .get("cruise.session.id")
        .unwrap_or_else(|| panic!("plan root omitted session id: {plan:#?}"));
    assert_eq!(
        run.attributes.get("cruise.session.id"),
        Some(session_id),
        "the run root must belong to the approved plan session"
    );
    assert_eq!(plan.status_code, StatusCode::Ok as i32);
    assert_eq!(run.status_code, StatusCode::Ok as i32);
    assert_eq!(
        plan.attributes.get("cruise.outcome").map(String::as_str),
        Some("success")
    );
    assert_eq!(
        run.attributes.get("cruise.outcome").map(String::as_str),
        Some("success")
    );
    let fixture_path = fixture.root.path().to_string_lossy().into_owned();
    assert_payload_omits(
        &requests,
        &[
            "execute-now-input-secret",
            "plan-prompt-secret",
            "execution-marker-secret",
            fixture_path.as_str(),
        ],
    );
}

#[test]
fn cancelling_plan_approval_exits_successfully_deletes_session_and_cancels_root() {
    let collector = Collector::start();
    let fixture = approval_fixture();
    let env = otlp_env(&collector.endpoint());
    let borrowed = borrowed_env(&env);
    let config = fixture.config.to_string_lossy().into_owned();
    let mut pty = start_cli_pty(
        &fixture,
        &[
            "plan",
            "--config",
            config.as_str(),
            "cancel-plan-input-secret",
        ],
        &borrowed,
    );
    wait_for_pty_text(&mut pty, "Action:", Duration::from_secs(30));
    send_pty(&mut pty, b"\x1b", "cancel plan approval");
    let (status, _transcript) = wait_for_pty_exit(&mut pty, Duration::from_secs(30));
    assert_eq!(
        status.exit_code(),
        0,
        "cancelled plan did not exit successfully"
    );
    drop(pty);

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let exported_spans = spans(&requests);
    let plan = named_span(
        &exported_spans,
        "cruise.execution",
        "cruise.execution.kind",
        "plan",
    );
    assert_eq!(plan.parent_span_id, [0_u8; 0]);
    assert_eq!(plan.status_code, StatusCode::Error as i32);
    assert_eq!(
        plan.attributes.get("cruise.outcome").map(String::as_str),
        Some("cancelled")
    );
    assert_eq!(
        plan.attributes.get("error.type").map(String::as_str),
        Some("cancelled")
    );
    let session_id = plan
        .attributes
        .get("cruise.session.id")
        .unwrap_or_else(|| panic!("cancelled plan root omitted session id: {plan:#?}"));
    assert!(
        !fixture
            .root
            .path()
            .join("data/cruise/sessions")
            .join(session_id)
            .exists(),
        "cancelled plan session must be deleted"
    );
    assert!(
        exported_spans.iter().all(|span| {
            span.name != "cruise.execution"
                || span
                    .attributes
                    .get("cruise.execution.kind")
                    .map(String::as_str)
                    != Some("run")
        }),
        "cancelled approval must not launch workflow execution: {exported_spans:#?}"
    );
    let fixture_path = fixture.root.path().to_string_lossy().into_owned();
    assert_payload_omits(
        &requests,
        &[
            "cancel-plan-input-secret",
            "plan-prompt-secret",
            fixture_path.as_str(),
        ],
    );
}

#[test]
fn failed_exec_exports_error_status_without_error_text() {
    let collector = Collector::start();
    let fixture = Fixture::new(
        "command: [sh, -c, 'cat >/dev/null; exit 0']\nsteps:\n  failing:\n    command: \"printf '%s' failure-output-secret >&2; exit 7\"\n",
    );
    let mut env = otlp_env(&collector.endpoint());
    env.push((
        "OTEL_EXPORTER_OTLP_HEADERS".into(),
        "Authorization=Bearer failure-header-secret".into(),
    ));
    let output = fixture.run(&borrowed_env(&env), "failure-input-secret");
    assert!(
        !output.status.success(),
        "failure workflow unexpectedly succeeded"
    );
    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let exported_spans = spans(&requests);
    let workflow = named_span(
        &exported_spans,
        "cruise.workflow",
        "cruise.operation",
        "workflow",
    );
    let failing = named_span(
        &exported_spans,
        "cruise.step",
        "cruise.step.name",
        "failing",
    );
    assert_eq!(
        workflow.status_code,
        StatusCode::Error as i32,
        "workflow status: {workflow:#?}"
    );
    assert_eq!(
        failing.status_code,
        StatusCode::Error as i32,
        "step status: {failing:#?}"
    );
    assert_eq!(
        failing.attributes.get("error.type").map(String::as_str),
        Some("command_exit"),
        "nonzero command must use a fixed classification: {failing:#?}"
    );
    assert_eq!(
        failing.int_attributes.get("cruise.command.exit_code"),
        Some(&7),
        "failed process exit code must remain numeric: {failing:#?}"
    );
    for span in [workflow, failing] {
        assert!(
            matches!(
                span.attributes.get("cruise.outcome").map(String::as_str),
                Some("error" | "failed")
            ),
            "unexpected failure outcome: {span:#?}"
        );
    }
    let (count_points, duration_points) = metrics(&requests);
    let metric_points = count_points
        .iter()
        .map(|point| &point.attributes)
        .chain(duration_points.iter().map(|point| &point.attributes))
        .collect::<Vec<_>>();
    assert!(
        !metric_points.is_empty(),
        "failure operation metrics missing"
    );
    assert!(
        metric_points.iter().any(|attributes| {
            matches!(
                attributes.get("cruise.outcome").map(String::as_str),
                Some("error" | "failed")
            )
        }),
        "failure metric outcome missing: {metric_points:?}"
    );
    for attributes in metric_points {
        assert_eq!(
            attributes.len(),
            2,
            "unsafe failure metric labels: {attributes:?}"
        );
        assert!(attributes.contains_key("cruise.operation"));
        assert!(attributes.contains_key("cruise.outcome"));
    }
    assert_payload_omits(
        &requests,
        &[
            "failure-output-secret",
            "failure-input-secret",
            "failure-header-secret",
            "exit 7",
        ],
    );
}

#[test]
fn scheduled_startup_fallback_exports_time_window_reason_not_cooldown() {
    let collector = Collector::start();
    let fixture = Fixture::new(
        "sdk: claude\nmodel:\n  - model: telemetry-scheduled-primary\n    disabled_during: ['00:00-24:00']\n  - telemetry-available-spare\nsteps:\n  answer:\n    prompt: SCHEDULE_TELEMETRY\n",
    );
    let bin = fixture.root.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("create bin: {error}"));
    cruise::test_support::write_executable_script(
        &bin.join("claude"),
        r#"#!/bin/sh
read -r user_frame
printf '%s\n' '{"type":"system","subtype":"init","session_id":"schedule-telemetry"}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"schedule-telemetry","result":"done"}'
"#,
    );
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let mut env = otlp_env(&collector.endpoint());
    env.push((
        "PATH".into(),
        std::env::join_paths(paths)
            .unwrap_or_else(|error| panic!("join PATH: {error}"))
            .to_string_lossy()
            .into_owned(),
    ));
    let output = fixture.run(&borrowed_env(&env), "schedule telemetry input");
    assert_success(&output);

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let exported = spans(&requests);
    let events: Vec<_> = exported
        .iter()
        .flat_map(|span| &span.events)
        .filter(|event| event.name == "cruise.retry")
        .collect();
    assert_eq!(
        events.len(),
        1,
        "one startup fallback event expected: {events:?}"
    );
    let event = events[0];
    for (key, value) in [
        ("cruise.retry.action", "fallback"),
        ("cruise.retry.reason", "model_time_window"),
        ("cruise.retry.from_model", "telemetry-scheduled-primary"),
        ("cruise.retry.to_model", "telemetry-available-spare"),
    ] {
        assert_eq!(
            event.attributes.get(key).map(String::as_str),
            Some(value),
            "{event:?}"
        );
    }
    let counts = retry_count_points(&requests);
    assert_eq!(counts.iter().map(|point| point.value).sum::<i64>(), 1);
    for point in counts {
        assert_eq!(
            point.attributes,
            HashMap::from([
                ("cruise.retry.action".to_owned(), "fallback".to_owned()),
                (
                    "cruise.retry.reason".to_owned(),
                    "model_time_window".to_owned()
                ),
            ]),
            "time windows must not be exported as cooldowns"
        );
    }
}

#[test]
fn command_rate_limit_retry_exports_attempts_and_bounded_retry_telemetry() {
    let collector = Collector::start();
    let fixture = Fixture::new(
        r"
command: [sh, -c, 'cat >/dev/null; exit 0']
steps:
  retried:
    command: >-
      if [ ! -e .telemetry-retried ]; then
        touch .telemetry-retried;
        echo 'rate limit retry-error-secret' >&2;
        exit 1;
      fi;
      echo retry-output-secret
",
    );
    let mut env = otlp_env(&collector.endpoint());
    env.push((
        "OTEL_EXPORTER_OTLP_HEADERS".into(),
        "Authorization=Bearer retry-header-secret".into(),
    ));
    env.push(("OPENAI_API_KEY".into(), "retry-credential-secret".into()));
    let output = fixture.run_with_args(
        &["--rate-limit-retries", "1"],
        &borrowed_env(&env),
        "retry-input-secret",
    );
    assert_success(&output);

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let exported_spans = spans(&requests);
    assert_rate_limit_retry_spans(&exported_spans);

    let retry_counts = retry_count_points(&requests);
    assert!(!retry_counts.is_empty(), "retry counter was not exported");
    assert_eq!(retry_counts.iter().map(|point| point.value).sum::<i64>(), 1);
    for point in &retry_counts {
        assert_eq!(
            point.attributes,
            HashMap::from([
                ("cruise.retry.action".to_owned(), "backoff".to_owned()),
                ("cruise.retry.reason".to_owned(), "rate_limit".to_owned()),
            ]),
            "retry counter labels must be bounded: {point:?}"
        );
    }
    assert_payload_omits(
        &requests,
        &[
            "retry-input-secret",
            "retry-error-secret",
            "retry-output-secret",
            "retry-header-secret",
            "retry-credential-secret",
        ],
    );
    let root_path = fixture.root.path().to_string_lossy().into_owned();
    assert_payload_omits(&requests, &[root_path.as_str()]);
}

#[test]
fn invalid_ci_markers_export_no_untrusted_fields() {
    let collector = Collector::start();
    let fixture = Fixture::new(
        r"steps:
  invalid-ci:
    command: |
      printf '%s\n' 'CRUISE_CI_DIAGNOSTIC_V1 github private-reason-secret FAILURE false 3' >&2
      printf '%s\n' 'CRUISE_CI_DIAGNOSTIC_V1 github ci_failed private-state-secret false 3' >&2
      printf '%s\n' 'CRUISE_CI_DIAGNOSTIC_V1 github ci_failed FAILURE false 3 leaked-field-secret' >&2
      printf '%s\n' 'invalid-marker-stderr-secret' >&2
      exit 9
",
    );
    let output = fixture.run(
        &borrowed_env(&otlp_env(&collector.endpoint())),
        "invalid-marker-input-secret",
    );
    assert!(
        !output.status.success(),
        "invalid marker fixture unexpectedly succeeded"
    );

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let spans = spans(&requests);
    let failed = named_span(&spans, "cruise.step", "cruise.step.name", "invalid-ci");
    assert_eq!(
        failed.int_attributes.get("cruise.command.exit_code"),
        Some(&9)
    );
    assert!(
        failed
            .attributes
            .keys()
            .chain(failed.int_attributes.keys())
            .chain(failed.bool_attributes.keys())
            .all(|key| !key.starts_with("cruise.ci.")),
        "malformed marker added CI attributes: {failed:#?}"
    );
    assert_payload_omits(
        &requests,
        &[
            "invalid-marker-input-secret",
            "private-reason-secret",
            "private-state-secret",
            "leaked-field-secret",
            "invalid-marker-stderr-secret",
        ],
    );
}

#[test]
fn builtin_wait_ci_exports_graphql_failure_reason_without_raw_response() {
    let collector = Collector::start();
    let command = builtin_wait_ci_command("tdd")
        .replace('{', "{{")
        .replace('}', "}}");
    let line_count = command.lines().count();
    let mut config = String::with_capacity(
        "steps:\n  wait-ci:\n    command: |\n".len() + command.len() + line_count * 7,
    );
    config.push_str("steps:\n  wait-ci:\n    command: |\n");
    for line in command.lines() {
        config.push_str("      ");
        config.push_str(line);
        config.push('\n');
    }
    let fixture = Fixture::new(&config);
    let fake_bin = fixture.root.path().join("fake-gh/bin");
    std::fs::create_dir_all(&fake_bin)
        .unwrap_or_else(|error| panic!("create fake gh directory: {error}"));
    cruise::test_support::write_executable_script(
        &fake_bin.join("gh"),
        r#"#!/bin/sh
if [ "$1" = repo ]; then
  printf '123/true\n'
  exit 0
fi
if [ "$1" = api ] && [ "$2" = graphql ]; then
  printf '{"errors":[{"message":"private-provider-response-secret"}]}\n'
  exit 0
fi
printf 'unexpected fake gh invocation with secret details\n' >&2
exit 97
"#,
    );
    let mut paths = vec![fake_bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let mut env = otlp_env(&collector.endpoint());
    env.push((
        "PATH".into(),
        std::env::join_paths(paths)
            .unwrap_or_else(|error| panic!("join fake gh PATH: {error}"))
            .to_string_lossy()
            .into_owned(),
    ));
    let output = fixture.run(&borrowed_env(&env), "builtin-wait-ci-input-secret");
    assert!(
        !output.status.success(),
        "GraphQL failure fixture unexpectedly succeeded"
    );

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let spans = spans(&requests);
    let wait_ci = named_span(&spans, "cruise.step", "cruise.step.name", "wait-ci");
    let attempt = spans
        .iter()
        .find(|span| span.name == "cruise.attempt" && span.parent_span_id == wait_ci.span_id)
        .unwrap_or_else(|| panic!("wait-ci command attempt was not exported: {spans:#?}"));
    for span in [wait_ci, attempt] {
        assert_eq!(
            span.attributes
                .get("cruise.ci.provider")
                .map(String::as_str),
            Some("github"),
            "{span:#?}"
        );
        assert_eq!(
            span.attributes
                .get("cruise.ci.failure_reason")
                .map(String::as_str),
            Some("graphql_error"),
            "{span:#?}"
        );
    }
    assert_eq!(
        wait_ci.attributes.get("error.type").map(String::as_str),
        Some("command_exit")
    );
    let exit_code = wait_ci
        .int_attributes
        .get("cruise.command.exit_code")
        .copied()
        .unwrap_or_else(|| panic!("failed built-in wait-ci did not export its exit code"));
    assert_eq!(
        attempt.int_attributes.get("cruise.command.exit_code"),
        Some(&exit_code),
        "failed built-in wait-ci attempt had a different numeric exit code: {attempt:#?}"
    );
    assert_payload_omits(
        &requests,
        &[
            "builtin-wait-ci-input-secret",
            "private-provider-response-secret",
            "unexpected fake gh invocation with secret details",
        ],
    );
}

#[test]
fn failed_signal_exports_numeric_signal_status() {
    let collector = Collector::start();
    let fixture = Fixture::new("steps:\n  terminated:\n    command: 'kill -TERM $$'\n");
    let output = fixture.run(
        &borrowed_env(&otlp_env(&collector.endpoint())),
        "signal-input-secret",
    );
    assert!(
        !output.status.success(),
        "signal fixture unexpectedly succeeded"
    );

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let spans = spans(&requests);
    let terminated = named_span(&spans, "cruise.step", "cruise.step.name", "terminated");
    assert_eq!(
        terminated.int_attributes.get("cruise.command.signal"),
        Some(&i64::from(libc::SIGTERM))
    );
    assert!(
        !terminated
            .int_attributes
            .contains_key("cruise.command.exit_code"),
        "signal termination should not have an exit code: {terminated:#?}"
    );
    assert_payload_omits(&requests, &["signal-input-secret", "kill -TERM $$"]);
}

#[test]
fn loop_protection_exports_bounded_traversal_diagnostics() {
    let collector = Collector::start();
    let fixture = Fixture::new(
        r"max_retries: 2
steps:
  repeat:
    command: 'printf loop-output-secret >/dev/null'
    if:
      no-file-changes: retry
",
    );
    let output = fixture.run(
        &borrowed_env(&otlp_env(&collector.endpoint())),
        "loop-input-secret",
    );
    assert!(
        !output.status.success(),
        "loop fixture unexpectedly succeeded"
    );

    let requests = collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let spans = spans(&requests);
    let loop_span = spans
        .iter()
        .find(|span| span.attributes.contains_key("cruise.loop.from"))
        .unwrap_or_else(|| panic!("no loop-protection span exported: {spans:#?}"));
    assert_eq!(
        loop_span.attributes.get("error.type").map(String::as_str),
        Some("loop_protection")
    );
    assert_eq!(
        loop_span
            .attributes
            .get("cruise.loop.from")
            .map(String::as_str),
        Some("repeat")
    );
    assert_eq!(
        loop_span
            .attributes
            .get("cruise.loop.to")
            .map(String::as_str),
        Some("repeat")
    );
    assert_eq!(
        loop_span.int_attributes.get("cruise.loop.max_retries"),
        Some(&2)
    );
    assert_eq!(
        loop_span.int_attributes.get("cruise.loop.traversals"),
        Some(&2)
    );
    assert_eq!(
        loop_span
            .int_attributes
            .get("cruise.loop.skipped_step_count"),
        Some(&0)
    );
    assert_payload_omits(
        &requests,
        &[
            "loop-input-secret",
            "loop-output-secret",
            "loop protection: edge repeat -> repeat exceeded max retries 2",
        ],
    );
}

#[test]
fn timeout_and_sigint_exports_fixed_error_classifications() {
    let timeout_collector = Collector::start();
    let timeout_fixture = Fixture::new(
        "command: [sh, -c, 'cat >/dev/null; exit 0']\nsteps:\n  slow:\n    command: 'sleep 30'\n    timeout: '1'\n",
    );
    let timeout_env = otlp_env(&timeout_collector.endpoint());
    let timeout_output = timeout_fixture.run(&borrowed_env(&timeout_env), "timeout-input-secret");
    assert!(
        !timeout_output.status.success(),
        "timeout workflow unexpectedly succeeded"
    );
    let timeout_requests = timeout_collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let timeout_spans = spans(&timeout_requests);
    let timeout_step = named_span(&timeout_spans, "cruise.step", "cruise.step.name", "slow");
    assert_eq!(
        timeout_step
            .attributes
            .get("error.type")
            .map(String::as_str),
        Some("timeout"),
        "timeout must have a fixed classification: {timeout_step:#?}"
    );
    assert_payload_omits(&timeout_requests, &["timeout-input-secret", "sleep 30"]);

    let cancel_collector = Collector::start();
    let cancel_fixture = Fixture::new(
        "command: [sh, -c, 'cat >/dev/null; exit 0']\nsteps:\n  slow:\n    command: 'touch .telemetry-cancel-started; sleep 30'\n",
    );
    let cancel_env = otlp_env(&cancel_collector.endpoint());
    let mut child = cancel_fixture
        .command(&borrowed_env(&cancel_env), "cancel-input-secret")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn cancellable cruise run: {error}"));
    wait_for_path(
        &mut child,
        &cancel_fixture.project.join(".telemetry-cancel-started"),
        Duration::from_secs(10),
    );
    let pid =
        i32::try_from(child.id()).unwrap_or_else(|error| panic!("invalid process id: {error}"));
    // SAFETY: `pid` is the live cruise subprocess created by this test.
    let signal_result = unsafe { libc::kill(pid, libc::SIGINT) };
    if signal_result != 0 {
        let _ = child.kill();
        let _ = child.wait();
        panic!("failed to send SIGINT to cruise subprocess");
    }
    let status = wait_for_child_exit(&mut child, Duration::from_secs(10));
    assert!(!status.success(), "SIGINT workflow unexpectedly succeeded");
    let cancel_requests = cancel_collector.wait_for_paths(&["/v1/traces", "/v1/metrics"]);
    let cancel_spans = spans(&cancel_requests);
    let cancel_step = named_span(&cancel_spans, "cruise.step", "cruise.step.name", "slow");
    assert_eq!(
        cancel_step.attributes.get("error.type").map(String::as_str),
        Some("cancelled"),
        "cancellation must have a fixed classification: {cancel_step:#?}"
    );
    assert_payload_omits(&cancel_requests, &["cancel-input-secret", "sleep 30"]);
}

#[test]
fn telemetry_is_off_by_default_and_sdk_disabled_takes_precedence() {
    let collector = Collector::start();
    let fixture = Fixture::new("steps:\n  local:\n    command: 'printf local-only >/dev/null'\n");

    let endpoint = collector.endpoint();
    let default_off = fixture.run(
        &[("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint.as_str())],
        "default-off-input",
    );
    assert_success(&default_off);
    thread::sleep(Duration::from_millis(100));
    assert!(
        collector.requests().is_empty(),
        "telemetry exported without opt-in"
    );

    let mut disabled = otlp_env(&collector.endpoint());
    disabled.push(("OTEL_SDK_DISABLED".into(), "true".into()));
    let disabled = fixture.run(&borrowed_env(&disabled), "sdk-disabled-input");
    assert_success(&disabled);
    thread::sleep(Duration::from_millis(100));
    assert!(
        collector.requests().is_empty(),
        "OTEL_SDK_DISABLED did not suppress exports"
    );
}

#[test]
fn unreachable_collector_does_not_fail_local_workflow() {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .unwrap_or_else(|error| panic!("reserve unreachable endpoint: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("read unreachable endpoint: {error}"));
    drop(listener);
    let endpoint = format!("http://{address}");
    let fixture = Fixture::new("steps:\n  local:\n    command: 'printf local-ok >/dev/null'\n");
    let output = fixture.run(&borrowed_env(&otlp_env(&endpoint)), "unreachable-input");
    assert_success(&output);
}

struct MergeCliCase {
    cli: Fixture,
    env: Vec<(String, String)>,
    gh_log: PathBuf,
    collector: Option<Collector>,
}

fn merge_cli_case(telemetry_enabled: bool, workflow_yaml: &str, gh_script: &str) -> MergeCliCase {
    let cli = Fixture::new(workflow_yaml);
    let bare = cli.root.path().join("origin.git");
    cruise::test_support::run_git_ok(cli.root.path(), &["init", "--bare", "origin.git"]);
    cruise::test_support::run_git_ok(
        &cli.project,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/service.git",
        ],
    );
    let push_url = bare.to_string_lossy().into_owned();
    cruise::test_support::run_git_ok(
        &cli.project,
        &["config", "remote.origin.pushurl", push_url.as_str()],
    );

    let collector = telemetry_enabled.then(Collector::start);
    let fake_bin = cli.root.path().join("fake-gh/bin");
    std::fs::create_dir_all(&fake_bin)
        .unwrap_or_else(|error| panic!("create fake gh bin: {error}"));
    let gh_log = cli.root.path().join("fake-gh.log");
    cruise::test_support::write_executable_script(&fake_bin.join("gh"), gh_script);
    let search_path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let env = vec![
        ("PATH".to_string(), search_path),
        (
            "FAKE_GH_LOG".to_string(),
            gh_log.to_string_lossy().into_owned(),
        ),
        (
            "FAKE_GH_STATUS_COUNT".to_string(),
            cli.root
                .path()
                .join("fake-gh-status-count")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "FAKE_GH_STATUS_SEQUENCE".to_string(),
            cli.root
                .path()
                .join("fake-gh-status-sequence")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "FAKE_GH_WRITE_ATTEMPTS".to_string(),
            cli.root
                .path()
                .join("fake-gh-write-attempts")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "FAKE_GH_SUCCESSFUL_WRITES".to_string(),
            cli.root
                .path()
                .join("fake-gh-successful-writes")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "FAKE_GH_READY_HEAD".to_string(),
            cli.root
                .path()
                .join("fake-gh-ready-head")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "FAKE_GH_MERGED".to_string(),
            cli.root
                .path()
                .join("fake-gh-merged")
                .to_string_lossy()
                .into_owned(),
        ),
    ];
    let mut env = env;
    if let Some(collector) = &collector {
        env.extend(otlp_env(&collector.endpoint()));
    }
    MergeCliCase {
        cli,
        env,
        gh_log,
        collector,
    }
}

fn merge_diagnostics_case(telemetry_enabled: bool) -> MergeCliCase {
    merge_cli_case(
        telemetry_enabled,
        "command: [cat]\ncreate_pr: true\nflags:\n  merge: {}\nsteps:\n  work:\n    command: \"printf changed > merge-diagnostic.txt\"\nafter-pr:\n  merge:\n    squash-merge: true\n    flag: merge\n",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_GH_LOG"
if [ "$1" = "--version" ]; then
  printf 'gh version 2.0.0\n'
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "create" ]; then
  printf 'https://github.com/acme/service/pull/7\n'
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "merge" ]; then
  printf 'refusing unsafe PR merge\n' >&2
  exit 97
fi
if [ "$1" = "api" ]; then
  case "$*" in
    *"/merge"*)
      printf 'refusing unsafe GitHub merge write\n' >&2
      exit 97
      ;;
  esac
  head=$(git rev-parse HEAD) || exit 97
  case "$*" in
    *mergeable*)
      printf '{"data":{"repository":{"squashMergeAllowed":true,"pullRequest":{"state":"OPEN","isDraft":false,"headRefOid":"%s","baseRefName":"main","mergeable":"CONFLICTING","mergeStateStatus":"BEHIND","reviewDecision":"APPROVED","mergeQueueEntry":null,"autoMergeRequest":null,"statusCheckRollup":{"state":"SUCCESS","commit":{"oid":"%s"},"contexts":{"totalCount":0,"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[]}}}}}}\n' "$head" "$head"
      exit 0
      ;;
    *pullRequest*)
      printf '{"data":{"repository":{"pullRequest":{"state":"OPEN","headRefOid":"%s"}}}}\n' "$head"
      exit 0
      ;;
  esac
fi
printf 'refusing unexpected fake gh command\n' >&2
exit 97
"#,
    )
}

fn merge_readiness_race_case() -> MergeCliCase {
    merge_cli_case(
        false,
        "command: [cat]\ncreate_pr: true\nflags:\n  merge: {}\nsteps:\n  work:\n    command: \"printf changed > merge-readiness-race.txt\"\nafter-pr:\n  wait-ci:\n    command: \"gh pr checks {pr.number} --watch\"\n    timeout: \"10m\"\n  merge:\n    squash-merge: true\n    flag: merge\n",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_GH_LOG"
if [ "$1" = "--version" ]; then
  printf 'gh version 2.0.0\n'
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "create" ]; then
  printf 'https://github.com/acme/service/pull/7\n'
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "checks" ] && [ "$4" = "--watch" ]; then
  printf 'All checks passed for old head\n'
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "merge" ]; then
  printf 'refusing unsafe PR merge\n' >&2
  exit 97
fi
if [ "$1" = "api" ]; then
  case "$*" in
    *"/merge"*)
      attempts=$(cat "$FAKE_GH_WRITE_ATTEMPTS" 2>/dev/null || printf '0')
      attempts=$((attempts + 1))
      printf '%s\n' "$attempts" > "$FAKE_GH_WRITE_ATTEMPTS" || exit 97
      head=$(git rev-parse HEAD) || exit 97
      ready_head=$(cat "$FAKE_GH_READY_HEAD" 2>/dev/null || true)
      status_count=$(cat "$FAKE_GH_STATUS_COUNT" 2>/dev/null || printf '0')
      if [ "$ready_head" != "$head" ] || [ "$attempts" -ne 1 ] || [ "$status_count" -lt 3 ]; then
        printf 'rejected early or duplicate merge write\n' >&2
        exit 97
      fi
      case " $* " in
        *"sha=$head"*) ;;
        *)
          printf 'merge write did not pin the current HEAD\n' >&2
          exit 97
          ;;
      esac
      printf '1\n' > "$FAKE_GH_SUCCESSFUL_WRITES" || exit 97
      printf 'MERGED\n' > "$FAKE_GH_MERGED" || exit 97
      printf '{"merged":true}\n'
      exit 0
      ;;
    *"/rules/branches/"*)
      printf '[]\n'
      exit 0
      ;;
    *statusCheckRollup*)
      head=$(git rev-parse HEAD) || exit 97
      count=$(cat "$FAKE_GH_STATUS_COUNT" 2>/dev/null || printf '0')
      count=$((count + 1))
      printf '%s\n' "$count" > "$FAKE_GH_STATUS_COUNT" || exit 97
      if [ "$count" -eq 1 ]; then
        rollup_head=0000000000000000000000000000000000000001
        printf 'UNKNOWN %s %s\n' "$head" "$rollup_head" >> "$FAKE_GH_STATUS_SEQUENCE"
        printf '{"data":{"repository":{"squashMergeAllowed":true,"pullRequest":{"state":"OPEN","isDraft":false,"headRefOid":"%s","baseRefName":"main","mergeable":"UNKNOWN","mergeStateStatus":"UNKNOWN","reviewDecision":"APPROVED","mergeQueueEntry":null,"autoMergeRequest":null,"statusCheckRollup":{"state":"SUCCESS","commit":{"oid":"%s"},"contexts":{"totalCount":1,"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[{"__typename":"CheckRun","name":"old-head-ci","status":"COMPLETED","conclusion":"SUCCESS"}]}}}}}}\n' "$head" "$rollup_head"
        exit 0
      fi
      if [ "$count" -eq 2 ]; then
        printf 'MERGEABLE UNSTABLE %s %s\n' "$head" "$head" >> "$FAKE_GH_STATUS_SEQUENCE"
        printf '{"data":{"repository":{"squashMergeAllowed":true,"pullRequest":{"state":"OPEN","isDraft":false,"headRefOid":"%s","baseRefName":"main","mergeable":"MERGEABLE","mergeStateStatus":"UNSTABLE","reviewDecision":"APPROVED","mergeQueueEntry":null,"autoMergeRequest":null,"statusCheckRollup":{"state":"PENDING","commit":{"oid":"%s"},"contexts":{"totalCount":1,"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[{"__typename":"CheckRun","name":"new-head-ci","status":"IN_PROGRESS","conclusion":null}]}}}}}}\n' "$head" "$head"
        exit 0
      fi
      printf '%s\n' "$head" > "$FAKE_GH_READY_HEAD" || exit 97
      printf 'MERGEABLE CLEAN %s %s\n' "$head" "$head" >> "$FAKE_GH_STATUS_SEQUENCE"
      printf '{"data":{"repository":{"squashMergeAllowed":true,"pullRequest":{"state":"OPEN","isDraft":false,"headRefOid":"%s","baseRefName":"main","mergeable":"MERGEABLE","mergeStateStatus":"CLEAN","reviewDecision":"APPROVED","mergeQueueEntry":null,"autoMergeRequest":null,"statusCheckRollup":{"state":"SUCCESS","commit":{"oid":"%s"},"contexts":{"totalCount":1,"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[{"__typename":"CheckRun","name":"new-head-ci","status":"COMPLETED","conclusion":"SUCCESS"}]}}}}}}\n' "$head" "$head"
      exit 0
      ;;
    *graphql*)
      head=$(git rev-parse HEAD) || exit 97
      state=$(cat "$FAKE_GH_MERGED" 2>/dev/null || printf 'OPEN')
      printf '{"data":{"repository":{"pullRequest":{"state":"%s","headRefOid":"%s"}}}}\n' "$state" "$head"
      exit 0
      ;;
  esac
fi
printf 'refusing unexpected fake gh command\n' >&2
exit 97
"#,
    )
}

fn merge_write_refusal_case(telemetry_enabled: bool, message: &str) -> MergeCliCase {
    let script = r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_GH_LOG"
if [ "$1" = "--version" ]; then echo 'gh version 2.0.0'; exit 0; fi
if [ "$1" = "pr" ] && [ "$2" = "create" ]; then echo 'https://github.com/acme/service/pull/7'; exit 0; fi
if [ "$1" = "api" ]; then
  case "$*" in
    *"/merge"*)
      echo attempt >> "$FAKE_GH_WRITE_ATTEMPTS"
      printf 'HTTP/1.1 405 Method Not Allowed\r\nX-Secret: header-secret-8241\r\n\r\n{"message":"%s"}\n' "$FAKE_REFUSAL_MESSAGE"
      echo 'stderr-secret-8241' >&2
      exit 1 ;;
    *"/rules/branches/"*) echo '[]'; exit 0 ;;
    *statusCheckRollup*)
      head=$(git rev-parse HEAD) || exit 97
      printf '{"data":{"repository":{"squashMergeAllowed":true,"pullRequest":{"state":"OPEN","isDraft":false,"headRefOid":"%s","baseRefName":"main","mergeable":"MERGEABLE","mergeStateStatus":"CLEAN","reviewDecision":"APPROVED","mergeQueueEntry":null,"autoMergeRequest":null,"statusCheckRollup":{"state":"SUCCESS","commit":{"oid":"%s"},"contexts":{"totalCount":1,"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[{"__typename":"CheckRun","name":"ci","status":"COMPLETED","conclusion":"SUCCESS"}]}}}}}}\n' "$head" "$head"
      exit 0 ;;
    *graphql*)
      head=$(git rev-parse HEAD) || exit 97
      printf '{"data":{"repository":{"pullRequest":{"state":"OPEN","headRefOid":"%s"}}}}\n' "$head"
      exit 0 ;;
  esac
fi
echo 'unexpected fake gh command' >&2
exit 97
"#;
    let mut scenario = merge_cli_case(
        telemetry_enabled,
        "command: [cat]\ncreate_pr: true\nflags:\n  merge: {}\nsteps:\n  work:\n    command: \"printf changed > merge-refusal.txt\"\nafter-pr:\n  merge:\n    squash-merge: true\n    flag: merge\n",
        script,
    );
    scenario
        .env
        .push(("FAKE_REFUSAL_MESSAGE".into(), message.into()));
    scenario
        .env
        .push(("FAKE_SECRET_SENTINEL".into(), "env-secret-8241".into()));
    scenario
}

#[test]
fn github_merge_write_refusal_is_safe_and_observable() {
    for (telemetry, message, reason) in [
        (
            false,
            "Pull Request is not mergeable",
            "merge_not_performed",
        ),
        (true, "Pull Request is not mergeable", "merge_not_performed"),
        (
            true,
            "Pull Request is not mergeable provider-secret-8241",
            "unclassified",
        ),
    ] {
        let scenario = merge_write_refusal_case(telemetry, message);
        let env = borrowed_env(&scenario.env);
        let session_id = create_merge_session(&scenario.cli, &env);
        let run = scenario
            .cli
            .cli_command(&["run", &session_id], &env)
            .output()
            .unwrap_or_else(|error| panic!("run refusal scenario: {error}"));
        assert!(
            !run.status.success(),
            "refused merge unexpectedly succeeded: {}",
            output_text(&run)
        );
        let manager = SessionManager::new(scenario.cli.root.path().join("data/cruise"));
        let session = manager
            .load(&session_id)
            .unwrap_or_else(|error| panic!("load failed merge session: {error}"));
        let SessionPhase::Failed(error) = &session.phase else {
            panic!("expected failed session: {session:#?}")
        };
        for expected in [reason, "405", "1"] {
            assert!(
                error.contains(expected),
                "missing {expected} in saved failure: {error}"
            );
        }
        let calls = std::fs::read_to_string(&scenario.gh_log)
            .unwrap_or_else(|error| panic!("read fake gh call log: {error}"));
        let writes: Vec<_> = calls
            .lines()
            .filter(|line| line.starts_with("api ") && line.contains("/merge"))
            .collect();
        assert_eq!(writes.len(), 1, "expected one merge write: {calls}");
        assert!(
            writes[0].contains("--include"),
            "missing response status: {calls}"
        );
        let write_attempts_path = scenario.cli.root.path().join("fake-gh-write-attempts");
        let write_attempts = std::fs::read_to_string(&write_attempts_path)
            .unwrap_or_else(|error| panic!("read fake gh write attempts: {error}"));
        assert_eq!(write_attempts.lines().count(), 1);
        assert!(!scenario.cli.root.path().join("fake-gh-merged").exists());
        assert!(
            !calls.lines().any(|line| line.starts_with("pr merge")),
            "unexpected queued merge: {calls}"
        );
        let mut outputs = vec![error.as_bytes().to_vec(), output_text(&run).into_bytes()];
        assert_merge_refusal_telemetry(scenario.collector.as_ref(), reason, &mut outputs);
        for output in outputs {
            for secret in [
                "header-secret-8241",
                "stderr-secret-8241",
                "env-secret-8241",
                "provider-secret-8241",
            ] {
                assert!(
                    !output
                        .windows(secret.len())
                        .any(|window| window == secret.as_bytes()),
                    "secret {secret} leaked"
                );
            }
        }
    }
}

fn assert_merge_refusal_telemetry(
    collector: Option<&Collector>,
    reason: &str,
    outputs: &mut Vec<Vec<u8>>,
) {
    let Some(collector) = collector else {
        return;
    };
    let requests = collector.wait_for_paths(&["/v1/traces"]);
    let exported = spans(&requests);
    let merge = named_span(&exported, "cruise.step", "cruise.step.name", "merge");
    assert_eq!(merge.status_code, StatusCode::Error as i32);
    assert_eq!(
        merge.attributes.get("error.type").map(String::as_str),
        Some("squash_merge_failed")
    );
    assert_eq!(
        merge
            .attributes
            .get("cruise.github.merge_refusal_reason")
            .map(String::as_str),
        Some(reason)
    );
    assert_eq!(
        merge.int_attributes.get("cruise.github.merge_http_status"),
        Some(&405)
    );
    assert_eq!(
        merge.int_attributes.get("cruise.github.merge_exit_code"),
        Some(&1)
    );
    assert_eq!(
        merge
            .attributes
            .get("cruise.github.mergeable")
            .map(String::as_str),
        Some("MERGEABLE")
    );
    assert_eq!(
        merge
            .attributes
            .get("cruise.github.merge_state_status")
            .map(String::as_str),
        Some("CLEAN")
    );
    outputs.extend(requests.into_iter().map(|request| request.body));
}

fn create_merge_session(fixture: &Fixture, env: &[(&str, &str)]) -> String {
    let config = fixture.config.to_string_lossy().into_owned();
    let plan = fixture
        .cli_command(
            &[
                "plan",
                "--config",
                config.as_str(),
                "--skip-planning",
                "--flag",
                "merge",
                "merge CLI input",
            ],
            env,
        )
        .output()
        .unwrap_or_else(|error| panic!("create merge CLI session: {error}"));
    assert!(plan.status.success(), "plan failed: {}", output_text(&plan));

    let manager = SessionManager::new(fixture.root.path().join("data/cruise"));
    let sessions = manager
        .list()
        .unwrap_or_else(|error| panic!("list merge CLI sessions: {error}"));
    assert_eq!(sessions.len(), 1, "unexpected sessions: {sessions:#?}");
    sessions[0].id.clone()
}

fn pin_inline_config_snapshot(fixture: &Fixture, session_id: &str) -> PathBuf {
    let manager = SessionManager::new(fixture.root.path().join("data/cruise"));
    let mut session = manager
        .load(session_id)
        .unwrap_or_else(|error| panic!("load session before snapshotting config: {error}"));
    let yaml = std::fs::read_to_string(&fixture.config)
        .unwrap_or_else(|error| panic!("read workflow before snapshotting: {error}"));
    let snapshot_path = manager.sessions_dir().join(session_id).join("config.yaml");
    std::fs::write(&snapshot_path, yaml)
        .unwrap_or_else(|error| panic!("write legacy inline config snapshot: {error}"));
    session.config = cruise::session_config::SessionConfigRef::inline_snapshot();
    manager
        .save(&session)
        .unwrap_or_else(|error| panic!("save inline config snapshot reference: {error}"));
    std::fs::remove_file(&fixture.config)
        .unwrap_or_else(|error| panic!("remove live workflow after snapshotting: {error}"));
    snapshot_path
}

#[test]
fn rejected_squash_merge_persists_github_mergeability_and_exports_it() {
    for telemetry_enabled in [false, true] {
        let scenario = merge_diagnostics_case(telemetry_enabled);
        let borrowed = borrowed_env(&scenario.env);
        let fixture = &scenario.cli;
        let gh_log = &scenario.gh_log;
        let collector = scenario.collector.as_ref();

        let session_id = create_merge_session(fixture, &borrowed);
        let manager = SessionManager::new(fixture.root.path().join("data/cruise"));
        let run = fixture
            .cli_command(&["run", session_id.as_str()], &borrowed)
            .output()
            .unwrap_or_else(|error| panic!("run rejected squash merge: {error}"));
        assert!(
            !run.status.success(),
            "non-mergeable pull request unexpectedly completed: {}",
            output_text(&run)
        );

        let session = manager
            .load(&session_id)
            .unwrap_or_else(|error| panic!("load failed merge diagnostics session: {error}"));
        let SessionPhase::Failed(phase_error) = &session.phase else {
            panic!("expected failed session after merge readiness rejection: {session:#?}");
        };
        assert!(
            phase_error.contains("mergeable=CONFLICTING"),
            "saved phase error omitted actual mergeable value: {phase_error}"
        );
        assert!(
            phase_error.contains("mergeStateStatus=BEHIND"),
            "saved phase error omitted actual merge-state value: {phase_error}"
        );
        let gh_calls = std::fs::read_to_string(gh_log)
            .unwrap_or_else(|error| panic!("read fake gh log: {error}"));
        assert!(
            gh_calls.contains("pr create"),
            "fake gh was not used: {gh_calls}"
        );
        let unsafe_merge_calls = gh_calls
            .lines()
            .filter(|call| {
                call.starts_with("pr merge")
                    || (call.starts_with("api ")
                        && (call.contains("/merge") || call.contains("--method PUT")))
            })
            .collect::<Vec<_>>();
        assert!(
            unsafe_merge_calls.is_empty(),
            "fake gh received unsafe merge write call: {unsafe_merge_calls:?}"
        );

        if let Some(collector) = collector {
            let requests = collector.wait_for_paths(&["/v1/traces"]);
            let exported_spans = spans(&requests);
            let merge = named_span(&exported_spans, "cruise.step", "cruise.step.name", "merge");
            assert_eq!(
                merge.status_code,
                StatusCode::Error as i32,
                "rejected merge step must be recorded as failed: {merge:#?}"
            );
            assert_eq!(
                merge
                    .attributes
                    .get("cruise.github.mergeable")
                    .map(String::as_str),
                Some("CONFLICTING"),
                "mergeable attribute must match the rejected response: {merge:#?}"
            );
            assert_eq!(
                merge
                    .attributes
                    .get("cruise.github.merge_state_status")
                    .map(String::as_str),
                Some("BEHIND"),
                "merge-state attribute must match the rejected response: {merge:#?}"
            );
        }
    }
}

fn assert_expected_head_readiness(root: &std::path::Path) -> String {
    const OLD_HEAD: &str = "0000000000000000000000000000000000000001";

    let status_count = std::fs::read_to_string(root.join("fake-gh-status-count"))
        .unwrap_or_else(|error| panic!("read fake status count: {error}"))
        .trim()
        .parse::<usize>()
        .unwrap_or_else(|error| panic!("parse fake status count: {error}"));
    assert_eq!(
        status_count, 3,
        "native readiness should observe stale CI, current-head pending CI, then green CI"
    );
    let status_sequence = std::fs::read_to_string(root.join("fake-gh-status-sequence"))
        .unwrap_or_else(|error| panic!("read fake status sequence: {error}"));
    let status_sequence: Vec<_> = status_sequence.lines().collect();
    assert_eq!(
        status_sequence.len(),
        3,
        "fake provider should observe exactly the three readiness phases: {status_sequence:?}"
    );
    assert!(
        status_sequence[0].starts_with("UNKNOWN ") && status_sequence[0].ends_with(OLD_HEAD),
        "the first native response should be UNKNOWN with stale old-head SUCCESS: {status_sequence:?}"
    );
    let expected_head = std::fs::read_to_string(root.join("fake-gh-ready-head"))
        .unwrap_or_else(|error| panic!("read ready HEAD: {error}"))
        .trim()
        .to_string();
    let pending_status = format!("MERGEABLE UNSTABLE {expected_head} {expected_head}");
    assert_eq!(
        status_sequence[1],
        pending_status.as_str(),
        "native readiness must wait through current-head pending CI: {status_sequence:?}"
    );
    let expected_status = format!("MERGEABLE CLEAN {expected_head} {expected_head}");
    assert_eq!(
        status_sequence[2],
        expected_status.as_str(),
        "native readiness should merge only after current-head green CI: {status_sequence:?}"
    );
    expected_head
}

fn assert_single_guarded_merge(scenario: &MergeCliCase, expected_head: &str) {
    let root = scenario.cli.root.path();
    assert_eq!(
        std::fs::read_to_string(root.join("fake-gh-write-attempts"))
            .unwrap_or_else(|error| panic!("read merge write attempts: {error}"))
            .trim(),
        "1",
        "exactly one merge write should be attempted"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("fake-gh-successful-writes"))
            .unwrap_or_else(|error| panic!("read successful merge writes: {error}"))
            .trim(),
        "1",
        "the fake provider should accept exactly one guarded merge write"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("fake-gh-merged"))
            .unwrap_or_else(|error| panic!("read fake merged state: {error}"))
            .trim(),
        "MERGED",
        "the fake provider should confirm MERGED only after the guarded write"
    );

    let gh_calls = std::fs::read_to_string(&scenario.gh_log)
        .unwrap_or_else(|error| panic!("read fake gh call log: {error}"));
    let calls: Vec<_> = gh_calls.lines().collect();
    let wait_ci = calls
        .iter()
        .position(|call| *call == "pr checks 7 --watch")
        .unwrap_or_else(|| panic!("saved wait-ci snapshot was not run: {gh_calls}"));
    let status_calls: Vec<_> = calls
        .iter()
        .enumerate()
        .filter_map(|(index, call)| {
            (call.contains("graphql") && call.contains("statusCheckRollup")).then_some(index)
        })
        .collect();
    assert!(
        status_calls.len() >= 2,
        "native readiness did not issue a second status query: {gh_calls}"
    );
    let merge_writes: Vec<_> = calls
        .iter()
        .enumerate()
        .filter_map(|(index, call)| {
            (call.starts_with("api ") && call.contains("--method PUT") && call.contains("/merge"))
                .then_some((index, *call))
        })
        .collect();
    assert_eq!(
        merge_writes.len(),
        1,
        "the fake gh log should contain one merge write: {gh_calls}"
    );
    assert!(
        wait_ci < status_calls[0]
            && status_calls[0] < status_calls[1]
            && status_calls[1] < merge_writes[0].0,
        "the merge write must follow the saved wait step and current-head readiness: {gh_calls}"
    );
    assert!(
        merge_writes[0].1.contains(&format!("sha={expected_head}")),
        "merge write was not pinned to the observed expected HEAD: {}",
        merge_writes[0].1
    );
    assert!(
        !calls.iter().any(|call| call.starts_with("pr merge ")),
        "an unguarded gh pr merge was invoked: {gh_calls}"
    );
}

#[test]
fn saved_wait_ci_snapshot_cannot_bypass_expected_head_readiness() {
    let scenario = merge_readiness_race_case();
    let borrowed = borrowed_env(&scenario.env);
    let fixture = &scenario.cli;
    let session_id = create_merge_session(fixture, &borrowed);
    let snapshot_path = pin_inline_config_snapshot(fixture, &session_id);
    let saved_snapshot = std::fs::read_to_string(&snapshot_path)
        .unwrap_or_else(|error| panic!("read saved wait-ci snapshot: {error}"));
    let manager = SessionManager::new(fixture.root.path().join("data/cruise"));
    let run = fixture
        .cli_command(&["run", session_id.as_str()], &borrowed)
        .output()
        .unwrap_or_else(|error| panic!("run saved wait-ci snapshot: {error}"));
    assert_success(&run);
    assert_eq!(
        std::fs::read_to_string(&snapshot_path)
            .unwrap_or_else(|error| panic!("read saved snapshot after run: {error}")),
        saved_snapshot,
        "run must not rewrite the saved workflow snapshot"
    );
    let session = manager
        .load(&session_id)
        .unwrap_or_else(|error| panic!("load completed readiness session: {error}"));
    assert_eq!(
        session.phase,
        SessionPhase::Completed,
        "the session should complete only after the guarded merge: {session:#?}"
    );
    let expected_head = assert_expected_head_readiness(fixture.root.path());
    assert_single_guarded_merge(&scenario, &expected_head);
}
