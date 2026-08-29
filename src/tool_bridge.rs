//! Parent-side tool server for the `sdk: jcode` backend.
//!
//! jcode cannot register custom tools in-process (its harness API has no tool
//! registration request), so cruise's tools reach the model through a stdio MCP
//! server -- a `cruise mcp-bridge` child that jcode spawns. That child is a
//! *different process* from the cruise run, while cruise's tool handlers capture
//! in-process state (the [`crate::ask_handler::AskHandler`]'s channel to the
//! terminal, the plan-persist flag, the title / PR-metadata stores). So the
//! child forwards every call back here over a Unix socket, and [`ToolBridge`]
//! executes the real handler.
//!
//! The wire format is MCP's own JSON-RPC 2.0, one request or response object per
//! line: the bridge child relays the frame it received on stdin verbatim and
//! writes back the line it gets. That keeps the result shapes
//! (`tools/list` -> `{"tools": [...]}`, `tools/call` -> `{"content": [...],
//! "isError": bool}`) defined in exactly one place -- here.
//!
//! One connection carries one request. jcode's MCP client is sequential per
//! server, but connection-per-call needs no request-id multiplexing and lets a
//! blocking handler (`ask_user` waits for the user) hold its own thread without
//! stalling the accept loop.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::backend::tool::CruiseTool;
use crate::error::{CruiseError, Result};

/// Environment variable naming the socket a `cruise mcp-bridge` child should
/// dial. Set on the `jcode` child process, which passes its environment on to
/// the MCP servers it spawns.
pub const TOOL_SOCKET_ENV: &str = "CRUISE_TOOL_SOCKET";

/// MCP server name cruise registers under, so tools reach the model as
/// `mcp__cruise__<tool>`. Matches the `sdk: claude` toolbox name, which is what
/// lets the prompt templates refer to bare tool names on either backend.
pub const MCP_SERVER_NAME: &str = "cruise";

/// MCP protocol revision the bridge speaks.
///
/// Fixed, not negotiated: it is the revision jcode 0.81.1's client offers in its
/// `initialize` request, and the frame shapes below are that revision's.
/// Answering with whatever version a client asked for would claim support the
/// bridge has not been written against.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Longest socket path accepted from a candidate directory.
///
/// `sockaddr_un::sun_path` is 104 bytes on macOS (108 on Linux) *including* the
/// NUL terminator, and macOS's per-user `TMPDIR` (`/var/folders/../T/`) already
/// spends about half of that. Binding an over-long path fails with `EINVAL`, so
/// an over-long candidate is skipped in favour of the next one.
const MAX_SOCKET_PATH_LEN: usize = 100;

/// Socket file name inside the per-run directory. The run identity is in the
/// directory name, so this can stay fixed and short.
const SOCKET_FILE: &str = "tools.sock";

/// A JSON-RPC error code plus message, as returned to the MCP client.
///
/// Only the two codes the bridge can produce are modelled: an unroutable method
/// and a malformed frame.
struct RpcError {
    code: i64,
    message: String,
}

/// JSON-RPC "method not found".
const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC "invalid request".
const INVALID_REQUEST: i64 = -32600;

/// Serve cruise's tools on a Unix socket for the lifetime of a prompt run.
///
/// [`ToolBridge::start`] binds a fresh socket inside a fresh owner-only
/// directory and spawns an accept loop; the socket path goes to the `jcode`
/// child as [`TOOL_SOCKET_ENV`]. Dropping the bridge stops the accept loop and
/// removes both.
pub(crate) struct ToolBridge {
    /// Owner-only directory holding the socket, removed on drop.
    socket_dir: PathBuf,
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    accept_thread: Option<std::thread::JoinHandle<()>>,
}

impl ToolBridge {
    /// Bind a per-run socket and start serving `tools`.
    ///
    /// # Errors
    ///
    /// Returns an error if no directory yields a bindable socket path, or if
    /// binding fails.
    pub(crate) fn start(tools: Vec<CruiseTool>) -> Result<Self> {
        #[cfg(unix)]
        {
            unix::start(tools)
        }
        #[cfg(not(unix))]
        {
            let _ = tools;
            Err(CruiseError::Other(
                "`sdk: jcode` needs Unix domain sockets to expose cruise's tools to jcode, \
                 which this platform does not provide"
                    .to_string(),
            ))
        }
    }

    /// Path to pass to the bridge child as [`TOOL_SOCKET_ENV`].
    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for ToolBridge {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // The accept loop is parked in a blocking `accept()`, so it can only
        // observe the flag after one more connection arrives: dial the socket
        // once to hand it that connection. The thread is joined only when that
        // dial succeeded -- if it failed the socket is already unusable and the
        // loop will never return, so joining would hang the caller. Leaking a
        // parked thread until process exit is the lesser cost.
        let woken = wake_accept_loop(&self.socket_path);
        if let Some(handle) = self.accept_thread.take()
            && woken
        {
            let _ = handle.join();
        }
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = std::fs::remove_dir(&self.socket_dir);
    }
}

/// Dial the bridge socket once so a parked `accept()` returns. `true` when the
/// connection was made.
#[cfg(unix)]
fn wake_accept_loop(socket_path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(socket_path).is_ok()
}

/// No accept loop exists on a platform without Unix sockets --
/// [`ToolBridge::start`] fails before spawning one.
#[cfg(not(unix))]
fn wake_accept_loop(_socket_path: &Path) -> bool {
    false
}

/// Handle one JSON-RPC request frame against `tools`, returning the response
/// frame to write back.
///
/// `initialize` is answered by the bridge child itself, so only the two
/// tool methods are routed here; anything else is a `method not found`, which is
/// what an MCP client expects for a capability the server never advertised.
fn dispatch(tools: &[CruiseTool], line: &str) -> serde_json::Value {
    let request: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                &serde_json::Value::Null,
                &RpcError {
                    code: INVALID_REQUEST,
                    message: format!("malformed JSON-RPC frame: {e}"),
                },
            );
        }
    };
    let id = request
        .get("id")
        .unwrap_or(&serde_json::Value::Null)
        .clone();
    let method = request.get("method").and_then(serde_json::Value::as_str);
    match method {
        Some("tools/list") => success_response(&id, &tools_list_result(tools)),
        Some("tools/call") => match call_tool(tools, request.get("params")) {
            Ok(result) => success_response(&id, &result),
            Err(e) => error_response(&id, &e),
        },
        other => error_response(
            &id,
            &RpcError {
                code: METHOD_NOT_FOUND,
                message: format!("unsupported method '{}'", other.unwrap_or("<missing>")),
            },
        ),
    }
}

/// The `tools/list` result: cruise's tool definitions in MCP's shape.
fn tools_list_result(tools: &[CruiseTool]) -> serde_json::Value {
    let listed: Vec<serde_json::Value> = tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.parameters,
            })
        })
        .collect();
    serde_json::json!({ "tools": listed })
}

/// Run the named tool's handler and shape its outcome as an MCP `tools/call`
/// result.
///
/// A handler `Err` is *not* a JSON-RPC error: MCP reports tool failures as a
/// result with `isError: true` so the model sees the message and can recover,
/// which is the contract cruise's tools are written against (a stale
/// `update_plan` snippet must be retryable). Only an unknown tool name or a
/// malformed `params` object is a protocol-level error.
fn call_tool(
    tools: &[CruiseTool],
    params: Option<&serde_json::Value>,
) -> std::result::Result<serde_json::Value, RpcError> {
    let name = params
        .and_then(|p| p.get("name"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| RpcError {
            code: INVALID_REQUEST,
            message: "tools/call requires a string `params.name`".to_string(),
        })?;
    let tool = tools
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| RpcError {
            code: METHOD_NOT_FOUND,
            message: format!("unknown tool '{name}'"),
        })?;
    // MCP allows `arguments` to be absent for a no-argument tool; the handlers
    // read their fields out of an object, so an empty object is the right stand-in.
    let arguments = params
        .and_then(|p| p.get("arguments"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let (text, is_error) = match (tool.handler)(arguments) {
        Ok(text) => (text, false),
        Err(message) => (message, true),
    };
    Ok(serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    }))
}

fn success_response(id: &serde_json::Value, result: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: &serde_json::Value, error: &RpcError) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": error.code, "message": error.message },
    })
}

/// Candidate parents for the run's socket directory, in preference order:
/// `$XDG_RUNTIME_DIR` when set (the per-user runtime dir on Linux), then the
/// platform temp dir, then `/tmp`.
///
/// Preference is not proof: a set `XDG_RUNTIME_DIR` can be stale or unwritable
/// (an `su`'d session, a systemd-less container), so [`unix::bind_socket`] tries
/// each in turn and only fails when none works.
fn socket_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::with_capacity(3);
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        dirs.push(PathBuf::from(runtime));
    }
    dirs.push(std::env::temp_dir());
    dirs.push(PathBuf::from("/tmp"));
    dirs
}

/// Name for this run's socket directory: `cruise-<pid>-<nonce>`.
fn socket_dir_name() -> String {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    format!("cruise-{}-{}", std::process::id(), &nonce[..12])
}

#[cfg(unix)]
mod unix {
    use super::{
        Arc, AtomicBool, CruiseError, CruiseTool, MAX_SOCKET_PATH_LEN, Ordering, PathBuf, Result,
        SOCKET_FILE, ToolBridge, dispatch, socket_dir_name, socket_dirs,
    };
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::os::unix::fs::DirBuilderExt as _;
    use std::os::unix::net::{UnixListener, UnixStream};

    /// How long the accept loop pauses after a failed `accept()`.
    ///
    /// A transient failure (the peer went away between connect and accept) is
    /// per-connection, but a persistent one -- `EMFILE` when the process is out
    /// of descriptors -- returns immediately and forever, which would otherwise
    /// spin this thread on a core for the rest of the run.
    const ACCEPT_ERROR_BACKOFF: std::time::Duration = std::time::Duration::from_millis(50);

    pub(super) fn start(tools: Vec<CruiseTool>) -> Result<ToolBridge> {
        let (socket_dir, socket_path, listener) = bind_socket()?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let tools = Arc::new(tools);
        let accept_shutdown = Arc::clone(&shutdown);
        let accept_thread = std::thread::spawn(move || {
            accept_loop(&listener, &tools, &accept_shutdown);
        });
        Ok(ToolBridge {
            socket_dir,
            socket_path,
            shutdown,
            accept_thread: Some(accept_thread),
        })
    }

    /// Bind the run's socket inside a fresh owner-only directory, returning
    /// `(directory, socket path, listener)`.
    ///
    /// The *directory* carries the permission, not the socket: `bind` creates
    /// the socket with the process umask, so setting the mode afterwards leaves
    /// a window in which another local user on a world-writable `/tmp` could
    /// connect and drive cruise's tools (they rewrite the session's `plan.md`
    /// and answer the terminal prompt). A `0700` parent created first denies
    /// traversal before the socket exists at all.
    ///
    /// Candidates from [`socket_dirs`] are tried in order and any failure --
    /// an over-long path, an unwritable or missing directory -- moves to the
    /// next, so a stale `XDG_RUNTIME_DIR` cannot take `sdk: jcode` down while
    /// the temp dir would have worked.
    fn bind_socket() -> Result<(PathBuf, PathBuf, UnixListener)> {
        let name = socket_dir_name();
        let mut failures = Vec::new();
        for parent in socket_dirs() {
            let dir = parent.join(&name);
            let socket = dir.join(SOCKET_FILE);
            if socket.as_os_str().len() > MAX_SOCKET_PATH_LEN {
                failures.push(format!(
                    "{}: path exceeds {MAX_SOCKET_PATH_LEN} bytes",
                    socket.display()
                ));
                continue;
            }
            if let Err(e) = std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                failures.push(format!("{}: {e}", dir.display()));
                continue;
            }
            match UnixListener::bind(&socket) {
                Ok(listener) => return Ok((dir, socket, listener)),
                Err(e) => {
                    failures.push(format!("{}: {e}", socket.display()));
                    let _ = std::fs::remove_dir(&dir);
                }
            }
        }
        Err(CruiseError::Other(format!(
            "`sdk: jcode` could not bind a Unix socket to expose cruise's tools \
             ({}); set XDG_RUNTIME_DIR or TMPDIR to a short, writable path",
            failures.join("; ")
        )))
    }

    /// Accept connections until the bridge is dropped.
    ///
    /// Each connection is served on its own thread so that a handler which
    /// blocks (`ask_user` waits for the user's answer) cannot stall the next
    /// tool call or the shutdown wake-up connection. Those threads are
    /// deliberately not joined: a pending `ask_user` outlives the bridge and
    /// ends when the user answers or the process exits, and its reply then goes
    /// to an already-closed socket, which is a harmless write error.
    fn accept_loop(listener: &UnixListener, tools: &Arc<Vec<CruiseTool>>, shutdown: &AtomicBool) {
        loop {
            let accepted = listener.accept();
            if shutdown.load(Ordering::SeqCst) {
                return;
            }
            match accepted {
                Ok((stream, _)) => {
                    let tools = Arc::clone(tools);
                    std::thread::spawn(move || serve_connection(stream, &tools));
                }
                Err(_) => std::thread::sleep(ACCEPT_ERROR_BACKOFF),
            }
        }
    }

    /// Read one JSON-RPC request line, run it, write one response line.
    fn serve_connection(stream: UnixStream, tools: &[CruiseTool]) {
        let Ok(write_half) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        // A closed connection with no payload is the shutdown wake-up or a
        // probe; nothing to answer.
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let response = dispatch(tools, line.trim_end());
        let mut writer = write_half;
        let mut body = response.to_string();
        body.push('\n');
        let _ = writer.write_all(body.as_bytes());
        let _ = writer.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo_tool(name: &str) -> CruiseTool {
        CruiseTool::new(
            name,
            "echo the `text` argument back",
            serde_json::json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"],
            }),
            Arc::new(|input: serde_json::Value| {
                input
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| "missing `text`".to_string())
            }),
        )
    }

    #[test]
    fn tools_list_reports_name_description_and_input_schema() {
        let tools = vec![echo_tool("echo")];
        let response = dispatch(&tools, r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#);
        assert_eq!(response["id"], 7);
        let listed = &response["result"]["tools"];
        assert_eq!(listed[0]["name"], "echo");
        assert_eq!(listed[0]["description"], "echo the `text` argument back");
        assert_eq!(
            listed[0]["inputSchema"]["properties"]["text"]["type"],
            "string"
        );
    }

    #[test]
    fn tools_call_runs_the_handler_and_returns_its_text() {
        let tools = vec![echo_tool("echo")];
        let response = dispatch(
            &tools,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"echo","arguments":{"text":"hi"}}}"#,
        );
        assert_eq!(response["result"]["content"][0]["text"], "hi");
        assert_eq!(response["result"]["isError"], false);
    }

    /// A handler `Err` must reach the model as a *successful* JSON-RPC response
    /// carrying `isError: true`, not as a protocol error: cruise's tools rely on
    /// the model reading the message and retrying (e.g. a stale `update_plan`).
    #[test]
    fn handler_error_becomes_is_error_result_not_rpc_error() {
        let tools = vec![echo_tool("echo")];
        let response = dispatch(
            &tools,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo","arguments":{}}}"#,
        );
        assert!(response.get("error").is_none(), "got {response}");
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(response["result"]["content"][0]["text"], "missing `text`");
    }

    /// MCP permits omitting `arguments` entirely for a no-argument call.
    #[test]
    fn tools_call_without_arguments_passes_an_empty_object() {
        let tools = vec![CruiseTool::new(
            "probe",
            "report the received input",
            serde_json::json!({ "type": "object", "properties": {} }),
            Arc::new(|input: serde_json::Value| Ok(input.to_string())),
        )];
        let response = dispatch(
            &tools,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"probe"}}"#,
        );
        assert_eq!(response["result"]["content"][0]["text"], "{}");
    }

    #[test]
    fn unknown_tool_is_a_method_not_found_error() {
        let tools = vec![echo_tool("echo")];
        let response = dispatch(
            &tools,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope"}}"#,
        );
        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("nope"),
            "got {response}"
        );
    }

    #[test]
    fn unsupported_method_is_a_method_not_found_error() {
        let response = dispatch(&[], r#"{"jsonrpc":"2.0","id":5,"method":"resources/list"}"#);
        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn malformed_frame_is_an_invalid_request_error() {
        let response = dispatch(&[], "not json");
        assert_eq!(response["error"]["code"], INVALID_REQUEST);
        assert_eq!(response["id"], serde_json::Value::Null);
    }

    /// The socket must live somewhere `sockaddr_un` can hold, which macOS's
    /// deep per-user `TMPDIR` makes non-obvious.
    #[cfg(unix)]
    #[test]
    fn socket_path_fits_the_sun_path_limit() {
        let bridge = ToolBridge::start(Vec::new()).unwrap_or_else(|e| panic!("{e:?}"));
        let path = bridge.socket_path();
        assert!(
            path.as_os_str().len() <= MAX_SOCKET_PATH_LEN,
            "{} is {} bytes",
            path.display(),
            path.as_os_str().len()
        );
    }

    /// The socket's directory must be owner-only *before* the socket exists:
    /// `bind` honours the umask, so permissions set afterwards leave a window in
    /// which another local user on a world-writable `/tmp` could drive cruise's
    /// tools.
    #[cfg(unix)]
    #[test]
    fn the_socket_lives_in_an_owner_only_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let bridge = ToolBridge::start(Vec::new()).unwrap_or_else(|e| panic!("{e:?}"));
        let dir = bridge
            .socket_path()
            .parent()
            .unwrap_or_else(|| panic!("socket has no parent"));
        let mode = std::fs::metadata(dir)
            .unwrap_or_else(|e| panic!("{e:?}"))
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "{} is {mode:o}", dir.display());
    }

    #[cfg(unix)]
    mod over_socket {
        use super::*;
        use std::io::{BufRead as _, BufReader, Write as _};
        use std::os::unix::net::UnixStream;

        /// Send one JSON-RPC line to the bridge and read its response line,
        /// exercising the real socket transport rather than [`dispatch`] alone.
        fn round_trip(bridge: &ToolBridge, request: &str) -> serde_json::Value {
            let stream =
                UnixStream::connect(bridge.socket_path()).unwrap_or_else(|e| panic!("{e:?}"));
            let write_half = stream.try_clone().unwrap_or_else(|e| panic!("{e:?}"));
            let mut writer = write_half;
            writer
                .write_all(format!("{request}\n").as_bytes())
                .unwrap_or_else(|e| panic!("{e:?}"));
            writer.flush().unwrap_or_else(|e| panic!("{e:?}"));
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .unwrap_or_else(|e| panic!("{e:?}"));
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e:?}: {line}"))
        }

        #[test]
        fn serves_tools_list_and_tools_call_over_the_socket() {
            let bridge =
                ToolBridge::start(vec![echo_tool("echo")]).unwrap_or_else(|e| panic!("{e:?}"));
            let listed = round_trip(&bridge, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
            assert_eq!(listed["result"]["tools"][0]["name"], "echo");
            let called = round_trip(
                &bridge,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo","arguments":{"text":"over-socket"}}}"#,
            );
            assert_eq!(called["result"]["content"][0]["text"], "over-socket");
        }

        /// Handler state is shared, not copied: two calls must hit the same
        /// closure, which is what makes the plan-persist flag and the title
        /// store observable to the parent after the turn.
        #[test]
        fn successive_calls_share_the_handler_state() {
            let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let recorded = Arc::clone(&seen);
            let tool = CruiseTool::new(
                "record",
                "record the `text` argument",
                serde_json::json!({ "type": "object", "properties": {} }),
                Arc::new(move |input: serde_json::Value| {
                    let text = input
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    recorded.lock().map_err(|e| e.to_string())?.push(text);
                    Ok("ok".to_string())
                }),
            );
            let bridge = ToolBridge::start(vec![tool]).unwrap_or_else(|e| panic!("{e:?}"));
            for text in ["a", "b"] {
                round_trip(
                    &bridge,
                    &format!(
                        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"record","arguments":{{"text":"{text}"}}}}}}"#
                    ),
                );
            }
            let seen = seen.lock().unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(*seen, vec!["a".to_string(), "b".to_string()]);
        }

        #[test]
        fn dropping_the_bridge_unlinks_the_socket_and_its_directory() {
            let bridge =
                ToolBridge::start(vec![echo_tool("echo")]).unwrap_or_else(|e| panic!("{e:?}"));
            let path = bridge.socket_path().to_path_buf();
            let dir = path
                .parent()
                .unwrap_or_else(|| panic!("socket has no parent"))
                .to_path_buf();
            assert!(path.exists(), "{} should exist", path.display());
            drop(bridge);
            assert!(!path.exists(), "{} should be unlinked", path.display());
            assert!(!dir.exists(), "{} should be removed", dir.display());
        }
    }
}
