//! `cruise mcp-bridge`: the stdio MCP server jcode spawns for cruise's tools.
//!
//! jcode has no in-process tool registration, so cruise registers itself in
//! `$JCODE_HOME/mcp.json` as an MCP server whose command is this very binary
//! (see [`crate::backend::jcode`]). jcode launches it per session, speaks MCP
//! JSON-RPC 2.0 over its stdin/stdout, and exposes whatever it lists as
//! `mcp__cruise__<tool>`.
//!
//! The tools themselves cannot run here: their handlers live in the cruise
//! process that started the run (the terminal `ask_user` prompt, the session's
//! `plan.md`, the title / PR-metadata stores). So `initialize` is answered
//! locally and every `tools/*` frame is relayed verbatim to the parent's
//! [`crate::tool_bridge::ToolBridge`] over the Unix socket named by
//! [`TOOL_SOCKET_ENV`], whose reply is written back verbatim. The socket path
//! arrives through the environment: jcode passes its own environment on to the
//! MCP servers it spawns, and cruise sets the variable on the `jcode` child.
//!
//! This is a hidden subcommand: it is an implementation detail of the `sdk:
//! jcode` backend, not something a user invokes.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::error::{CruiseError, Result};
use crate::tool_bridge::{MCP_PROTOCOL_VERSION, MCP_SERVER_NAME, TOOL_SOCKET_ENV};

/// JSON-RPC "internal error" -- used when the parent socket cannot be reached.
const INTERNAL_ERROR: i64 = -32603;

/// Serve MCP on stdin/stdout until stdin reaches EOF (jcode closing the server).
///
/// `socket_override` comes from `--socket` and wins over [`TOOL_SOCKET_ENV`].
///
/// # Errors
///
/// Returns an error if neither `--socket` nor [`TOOL_SOCKET_ENV`] names a
/// socket, or if stdout cannot be written.
pub fn run(socket_override: Option<PathBuf>) -> Result<()> {
    let socket = socket_override
        .or_else(|| std::env::var_os(TOOL_SOCKET_ENV).map(PathBuf::from))
        .ok_or_else(|| {
            CruiseError::Other(format!(
                "no tool socket to bridge to: set {TOOL_SOCKET_ENV} or pass --socket <path>. \
                 `cruise mcp-bridge` is spawned by jcode during a cruise run and is not \
                 meant to be invoked directly"
            ))
        })?;
    let stdin = std::io::stdin();
    // `Stdout` rather than `StdoutLock`: the lock guard is not `Send`, and
    // [`serve`] hands the writer to relay threads. `Stdout` has its own internal
    // lock, and [`serve`] serializes writes anyway.
    let mut stdout = std::io::stdout();
    serve(&mut stdin.lock(), &mut stdout, &socket)
}

/// Read JSON-RPC frames from `input`, write responses to `output`.
///
/// Frames the parent has to answer are relayed on a thread each: `ask_user`
/// blocks until the user replies, and the read loop must stay free to take the
/// `notifications/cancelled` or second `tools/call` that arrives meanwhile --
/// otherwise the client's tool timeout fires on a call cruise is still serving.
/// Responses carry their request id, so JSON-RPC allows them in any order, and
/// `output` is serialized so two of them cannot interleave on a line.
///
/// Split out from [`run`] so the frame handling is testable without touching
/// the process's real stdio.
fn serve<R: BufRead, W: Write + Send>(input: &mut R, output: &mut W, socket: &Path) -> Result<()> {
    let output = std::sync::Mutex::new(output);
    std::thread::scope(|scope| {
        let mut line = String::new();
        loop {
            line.clear();
            if input.read_line(&mut line)? == 0 {
                return Ok(());
            }
            let frame = line.trim_end();
            if frame.is_empty() {
                continue;
            }
            match route(frame, socket) {
                Route::Answer(response) => write_frame(&output, &response)?,
                Route::Silent => {}
                Route::Relay => {
                    let frame = frame.to_string();
                    let output = &output;
                    scope.spawn(move || {
                        let _ = write_frame(output, &relay(&frame, socket));
                    });
                }
            }
        }
    })
}

/// Write one response line under the stdout lock.
fn write_frame<W: Write>(output: &std::sync::Mutex<&mut W>, response: &str) -> Result<()> {
    let mut guard = output
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.write_all(response.as_bytes())?;
    guard.write_all(b"\n")?;
    guard.flush()?;
    Ok(())
}

/// What the bridge does with one frame.
enum Route {
    /// Answered here, without the parent.
    Answer(String),
    /// A notification: by spec it carries no id and must not be answered.
    Silent,
    /// The parent's business, over the socket.
    Relay,
}

/// Decide how to handle one request frame.
fn route(frame: &str, socket: &Path) -> Route {
    let method = serde_json::from_str::<serde_json::Value>(frame)
        .ok()
        .and_then(|v| {
            v.get("method")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        });
    match method.as_deref() {
        Some("initialize") => Route::Answer(initialize_response(frame, socket)),
        // Liveness. The spec requires a prompt empty result, and the parent
        // routes only the tool methods, so this has to be answered here.
        Some("ping") => Route::Answer(success_frame(frame, &serde_json::json!({}))),
        Some(m) if m.starts_with("notifications/") => Route::Silent,
        // `tools/list`, `tools/call`, and any method the parent may learn to
        // serve.
        _ => Route::Relay,
    }
}

/// Answer `initialize` with this server's identity and tool capability -- but
/// only once the parent bridge answers a connection.
///
/// The identity is local knowledge, so the handshake could always succeed. It
/// must not: a bridge that cannot reach the parent exposes no tools at all, and
/// a successful handshake would present that to jcode as a working server with
/// an empty tool set, so the turn would run on without `submit_plan` and the
/// step would "succeed" having written no plan. Failing the handshake makes
/// jcode report a broken MCP server instead.
fn initialize_response(frame: &str, socket: &Path) -> String {
    if let Err(e) = probe_parent(socket) {
        return unreachable_frame(frame, socket, &e);
    }
    success_frame(
        frame,
        &serde_json::json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": MCP_SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        }),
    )
}

/// Connect to the parent and hang up. The parent treats a payload-free
/// connection as a probe and closes it, so this costs one local round trip.
#[cfg(unix)]
fn probe_parent(socket: &Path) -> std::io::Result<()> {
    std::os::unix::net::UnixStream::connect(socket).map(|_| ())
}

#[cfg(not(unix))]
fn probe_parent(_socket: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Unix domain sockets are not available on this platform",
    ))
}

fn success_frame(frame: &str, result: &serde_json::Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id(frame),
        "result": result,
    })
    .to_string()
}

/// A JSON-RPC `-32603` response for `frame`, naming why the parent is unusable.
fn unreachable_frame(frame: &str, socket: &Path, e: &std::io::Error) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id(frame),
        "error": {
            "code": INTERNAL_ERROR,
            "message": format!("cruise tool bridge unreachable at {}: {e}", socket.display()),
        },
    })
    .to_string()
}

/// Forward `frame` to the parent over `socket` and return its reply line.
///
/// One connection per frame: the parent runs each call on its own thread, so a
/// blocking tool (`ask_user` waits for the user) needs no request multiplexing
/// here. A transport failure becomes a JSON-RPC error rather than a dropped
/// frame, so the model is told the tool is unreachable instead of the client
/// waiting forever.
fn relay(frame: &str, socket: &Path) -> String {
    match relay_over_socket(frame, socket) {
        Ok(reply) => reply,
        Err(e) => unreachable_frame(frame, socket, &e),
    }
}

#[cfg(unix)]
fn relay_over_socket(frame: &str, socket: &Path) -> std::io::Result<String> {
    use std::os::unix::net::UnixStream;

    let stream = UnixStream::connect(socket)?;
    let mut writer = stream.try_clone()?;
    writer.write_all(frame.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    // Half-close so the parent's `read_line` sees the end of the request even
    // if the frame arrived without its newline being flushed as a unit.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    if reader.read_line(&mut reply)? == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "the cruise tool bridge closed the connection without replying",
        ));
    }
    Ok(reply.trim_end().to_string())
}

#[cfg(not(unix))]
fn relay_over_socket(_frame: &str, _socket: &Path) -> std::io::Result<String> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Unix domain sockets are not available on this platform",
    ))
}

/// The request's `id`, defaulting to JSON-RPC `null` when the frame is
/// unparseable or carries none -- the only id a response can honestly claim.
fn request_id(frame: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(frame)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Responses to relayed frames come back on their own threads, so JSON-RPC
    /// order is not guaranteed; sort by id to compare against the requests.
    fn responses(input: &str, socket: &Path) -> Vec<serde_json::Value> {
        let mut reader = BufReader::new(input.as_bytes());
        let mut out: Vec<u8> = Vec::new();
        serve(&mut reader, &mut out, socket).unwrap_or_else(|e| panic!("{e:?}"));
        let mut frames: Vec<serde_json::Value> = String::from_utf8_lossy(&out)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e:?}: {l}")))
            .collect();
        frames.sort_by_key(|f: &serde_json::Value| f["id"].as_i64().unwrap_or(i64::MIN));
        frames
    }

    /// A handshake that succeeds while the parent is unreachable would present
    /// jcode with a working server that has no tools, and the turn would run on
    /// without `submit_plan`. It must fail instead.
    #[test]
    fn initialize_fails_when_the_parent_bridge_is_unreachable() {
        let out = responses(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
            Path::new("/nonexistent/cruise-tools.sock"),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], 1);
        assert_eq!(out[0]["error"]["code"], INTERNAL_ERROR);
        assert!(
            out[0]["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("/nonexistent/cruise-tools.sock"),
            "got {out:?}"
        );
    }

    /// MCP requires the receiver of a `ping` to answer promptly with an empty
    /// result. The parent routes only the tool methods, so relaying a ping would
    /// come back as `method not found` and the client would judge the server
    /// broken mid-session.
    #[cfg(unix)]
    #[test]
    fn ping_is_answered_with_an_empty_result() {
        let bridge =
            crate::tool_bridge::ToolBridge::start(Vec::new()).unwrap_or_else(|e| panic!("{e:?}"));
        let out = responses(
            "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"ping\"}\n",
            bridge.socket_path(),
        );
        assert_eq!(out.len(), 1, "got {out:?}");
        assert_eq!(out[0]["id"], 4);
        assert_eq!(out[0]["result"], serde_json::json!({}));
    }

    /// A notification has no id; answering one would corrupt the client's
    /// request/response pairing.
    #[test]
    fn notifications_get_no_response() {
        let out = responses(
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            Path::new("/nonexistent/cruise-tools.sock"),
        );
        assert!(out.is_empty(), "got {out:?}");
    }

    #[test]
    fn blank_lines_are_skipped() {
        let out = responses("\n\n", Path::new("/nonexistent/cruise-tools.sock"));
        assert!(out.is_empty(), "got {out:?}");
    }

    /// An unreachable parent must surface as a JSON-RPC error naming the socket,
    /// not as silence: the MCP client would otherwise block on the call.
    #[test]
    fn unreachable_socket_becomes_an_internal_error_naming_the_path() {
        let out = responses(
            "{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/list\"}\n",
            Path::new("/nonexistent/cruise-tools.sock"),
        );
        assert_eq!(out[0]["id"], 9);
        assert_eq!(out[0]["error"]["code"], INTERNAL_ERROR);
        let message = out[0]["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("/nonexistent/cruise-tools.sock"),
            "got {message}"
        );
    }

    #[test]
    fn run_without_socket_env_or_flag_errors_with_guidance() {
        let _guard = crate::test_support::lock_process();
        let restore = std::env::var_os(TOOL_SOCKET_ENV);
        // SAFETY: guarded by `lock_process`, which serializes env mutation
        // across the test binary.
        unsafe { std::env::remove_var(TOOL_SOCKET_ENV) };
        let err = run(None).err().map(|e| e.to_string()).unwrap_or_default();
        if let Some(value) = restore {
            // SAFETY: same lock as above.
            unsafe { std::env::set_var(TOOL_SOCKET_ENV, value) };
        }
        assert!(err.contains(TOOL_SOCKET_ENV), "got {err}");
        assert!(err.contains("--socket"), "got {err}");
    }

    /// The end-to-end path: a real [`crate::tool_bridge::ToolBridge`] answering
    /// frames the bridge relays, which is what jcode's MCP client drives.
    #[cfg(unix)]
    #[test]
    fn relays_tools_list_and_tools_call_to_a_live_bridge() {
        use crate::backend::tool::CruiseTool;
        use crate::tool_bridge::ToolBridge;
        use std::sync::Arc;

        let bridge = ToolBridge::start(vec![CruiseTool::new(
            "shout",
            "uppercase the `text` argument",
            serde_json::json!({ "type": "object", "properties": {} }),
            Arc::new(|input: serde_json::Value| {
                Ok(input
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_uppercase())
            }),
        )])
        .unwrap_or_else(|e| panic!("{e:?}"));

        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"shout","arguments":{"text":"hi"}}}"#,
            "\n",
        );
        let out = responses(input, bridge.socket_path());
        assert_eq!(out.len(), 3, "got {out:?}");
        assert_eq!(out[0]["result"]["serverInfo"]["name"], MCP_SERVER_NAME);
        assert_eq!(out[1]["result"]["tools"][0]["name"], "shout");
        assert_eq!(out[2]["result"]["content"][0]["text"], "HI");
    }

    /// `ask_user` blocks until the user answers. If the read loop waited for it,
    /// every frame behind it -- a `notifications/cancelled`, a second
    /// `tools/call` -- would sit in the stdin buffer until the client's tool
    /// timeout fired. Both calls here must therefore be in flight at once: the
    /// first only returns once the second has run.
    #[cfg(unix)]
    #[test]
    fn a_blocking_call_does_not_stall_the_frames_behind_it() {
        use crate::backend::tool::CruiseTool;
        use crate::tool_bridge::ToolBridge;
        use std::sync::Arc;
        use std::time::Duration;

        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
        let bridge = ToolBridge::start(vec![
            CruiseTool::new(
                "block",
                "wait until `release` is called",
                serde_json::json!({ "type": "object", "properties": {} }),
                Arc::new(move |_| {
                    release_rx
                        .lock()
                        .map_err(|e| e.to_string())?
                        .recv_timeout(Duration::from_secs(20))
                        .map_err(|e| e.to_string())?;
                    Ok("unblocked".to_string())
                }),
            ),
            CruiseTool::new(
                "release",
                "let `block` return",
                serde_json::json!({ "type": "object", "properties": {} }),
                Arc::new(move |_| {
                    release_tx.send(()).map_err(|e| e.to_string())?;
                    Ok("released".to_string())
                }),
            ),
        ])
        .unwrap_or_else(|e| panic!("{e:?}"));

        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"block"}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"release"}}"#,
            "\n",
        );
        let socket = bridge.socket_path().to_path_buf();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(responses(input, &socket));
        });
        let out = done_rx
            .recv_timeout(Duration::from_secs(20))
            .unwrap_or_else(|e| panic!("frames behind a blocking call were not served: {e}"));
        assert_eq!(out.len(), 2, "got {out:?}");
        assert_eq!(out[0]["result"]["content"][0]["text"], "unblocked");
        assert_eq!(out[1]["result"]["content"][0]["text"], "released");
    }
}
