//! Bounded Unix-socket relay for the Linux jcode sandbox.

use std::fs::{self, OpenOptions};
use std::io;
use std::io::Write as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener as StdUnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;

const MAX_CONNECTIONS: usize = 32;
const MAX_DNS_ADDRESSES: usize = 32;
const MAX_CONNECT_HEADER: usize = 8 * 1024;
const CONNECT_READ_CHUNK: usize = 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
const READ_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const WORKER_DRAIN_TIMEOUT: Duration = Duration::from_millis(500);
const SOCKET_CHECK_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Eq, PartialEq)]
pub(super) enum Route {
    /// Lowercase DNS hostnames, without a port. Requests are limited to :443.
    Connect(Vec<String>),
    /// A single trusted host-local IPv4 loopback port.
    Loopback(u16),
}

pub(super) struct ProviderProxy {
    shutdown: watch::Sender<bool>,
    worker: Option<JoinHandle<()>>,
    _socket_path: OwnedPath,
}

impl Drop for ProviderProxy {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Starts the host-side proxy at an exclusive socket path inside a private directory.
pub(super) fn start(path: &Path, route: Route) -> io::Result<ProviderProxy> {
    validate_route(&route)?;
    validate_private_directory(path)?;

    let listener = StdUnixListener::bind(path)?;
    let socket_path = OwnedPath::new(path)?;
    listener.set_nonblocking(true)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;

    let (shutdown, receiver) = watch::channel(false);
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("jcode-provider-proxy".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = started_tx.send(Err(error));
                    return;
                }
            };
            let listener = {
                let _entered = runtime.enter();
                UnixListener::from_std(listener)
            };
            let listener = match listener {
                Ok(listener) => listener,
                Err(error) => {
                    let _ = started_tx.send(Err(error));
                    return;
                }
            };
            let _ = started_tx.send(Ok(()));
            runtime.block_on(host_loop(listener, Arc::new(route), receiver));
            runtime.shutdown_timeout(Duration::from_millis(500));
        })?;

    match started_rx.recv_timeout(CONNECT_TIMEOUT) {
        Ok(Ok(())) => Ok(ProviderProxy {
            shutdown,
            worker: Some(worker),
            _socket_path: socket_path,
        }),
        Ok(Err(error)) => {
            let _ = shutdown.send(true);
            let _ = worker.join();
            Err(error)
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let _ = shutdown.send(true);
            let _ = worker.join();
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "provider proxy startup timed out",
            ))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let _ = shutdown.send(true);
            let _ = worker.join();
            Err(io::Error::other("provider proxy worker exited during startup"))
        }
    }
}

/// Runs the sandbox-side IPv4 loopback relay until the host socket is removed.
pub(crate) fn relay(socket: &Path, ready: &Path, port: Option<u16>) -> io::Result<()> {
    let identity = socket_identity(socket)?;
    if let Some(0) = port {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "relay port must be nonzero",
        ));
    }

    let listener = std::net::TcpListener::bind(SocketAddrV4::new(
        Ipv4Addr::LOCALHOST,
        port.unwrap_or(0),
    ))?;
    listener.set_nonblocking(true)?;
    let bound_port = listener.local_addr()?.port();
    let mut ready_file = OpenOptions::new().write(true).create_new(true).open(ready)?;
    let _ready_path = OwnedPath::new(ready)?;
    ready_file.set_permissions(fs::Permissions::from_mode(0o600))?;
    write!(ready_file, "{bound_port}")?;
    ready_file.sync_all()?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let listener = {
        let _entered = runtime.enter();
        TcpListener::from_std(listener)?
    };
    runtime.block_on(relay_loop(listener, socket.to_path_buf(), identity))
}

fn validate_route(route: &Route) -> io::Result<()> {
    match route {
        Route::Loopback(0) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "loopback route port must be nonzero",
        )),
        Route::Loopback(_) => Ok(()),
        Route::Connect(hostnames) => {
            if hostnames.is_empty()
                || hostnames
                    .iter()
                    .any(|hostname| !valid_dns_hostname(hostname))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "CONNECT route requires lowercase DNS hostnames",
                ));
            }
            Ok(())
        }
    }
}

fn valid_dns_hostname(hostname: &str) -> bool {
    if hostname.is_empty() || hostname.len() > 253 || !hostname.is_ascii() {
        return false;
    }
    hostname.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.as_bytes()[0].is_ascii_alphanumeric()
            && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    })
}

fn validate_private_directory(socket: &Path) -> io::Result<()> {
    let parent = socket
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let metadata = fs::metadata(parent)?;
    let owner = effective_uid()?;
    if !metadata.is_dir() || metadata.uid() != owner || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "provider proxy parent must be owned by the current user with mode 0700",
        ));
    }
    Ok(())
}

fn effective_uid() -> io::Result<u32> {
    let status = fs::read_to_string("/proc/self/status")?;
    let uid_line = status
        .lines()
        .find(|line| line.starts_with("Uid:"))
        .ok_or_else(|| io::Error::other("effective uid is unavailable"))?;
    uid_line
        .split_whitespace()
        .nth(2)
        .and_then(|uid| uid.parse().ok())
        .ok_or_else(|| io::Error::other("effective uid is unavailable"))
}

async fn host_loop(
    listener: UnixListener,
    route: Arc<Route>,
    mut shutdown: watch::Receiver<bool>,
) {
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut workers = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = wait_for_shutdown(&mut shutdown) => break,
            Some(_) = workers.join_next(), if !workers.is_empty() => {},
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    if let Ok(permit) = permits.clone().try_acquire_owned() {
                        let route = route.clone();
                        let stop = shutdown.clone();
                        workers.spawn(async move {
                            let _permit = permit;
                            let _ = host_connection(stream, route, stop).await;
                        });
                    }
                }
                Err(_) => break,
            }
        }
    }
    shutdown_workers(&mut workers).await;
}

async fn host_connection(
    mut client: UnixStream,
    route: Arc<Route>,
    mut shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    match route.as_ref() {
        Route::Loopback(port) => {
            let address = SocketAddr::from((Ipv4Addr::LOCALHOST, *port));
            let upstream = tokio::select! {
                _ = wait_for_shutdown(&mut shutdown) => return Ok(()),
                result = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address)) => {
                    result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "loopback connect timed out"))??
                }
            };
            bridge(client, upstream, shutdown).await
        }
        Route::Connect(allowed_hosts) => {
            let request = match read_connect_request(&mut client, &mut shutdown).await {
                Ok(request) => request,
                Err(ConnectRejection::Malformed) => {
                    write_response(&mut client, b"HTTP/1.1 400 Bad Request\r\n\r\n").await?;
                    return Ok(());
                }
                Err(ConnectRejection::Stopped) => return Ok(()),
            };
            if !is_authorized(allowed_hosts, &request.hostname) {
                write_response(&mut client, b"HTTP/1.1 403 Forbidden\r\n\r\n").await?;
                return Ok(());
            }
            let upstream = match connect_public(&request.hostname, &mut shutdown).await {
                Ok(upstream) => upstream,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(()),
                Err(_) => {
                    write_response(&mut client, b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await?;
                    return Ok(());
                }
            };
            write_response(
                &mut client,
                b"HTTP/1.1 200 Connection Established\r\n\r\n",
            )
            .await?;
            let mut upstream = upstream;
            if !request.buffered.is_empty() {
                tokio::time::timeout(WRITE_TIMEOUT, upstream.write_all(&request.buffered))
                    .await
                    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "CONNECT preface write timed out"))??;
            }
            bridge(client, upstream, shutdown).await
        }
    }
}

struct ConnectRequest {
    hostname: String,
    buffered: Vec<u8>,
}

enum ConnectRejection {
    Malformed,
    Stopped,
}

async fn read_connect_request(
    client: &mut UnixStream,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<ConnectRequest, ConnectRejection> {
    let mut bytes = [0_u8; MAX_CONNECT_HEADER + CONNECT_READ_CHUNK];
    let mut length = 0;
    let deadline = tokio::time::Instant::now() + HEADER_READ_TIMEOUT;
    loop {
        if let Some(end) = header_end(&bytes[..length]) {
            if end > MAX_CONNECT_HEADER {
                return Err(ConnectRejection::Malformed);
            }
            let hostname = parse_connect_header(&bytes[..end])
                .ok_or(ConnectRejection::Malformed)?
                .to_owned();
            return Ok(ConnectRequest {
                hostname,
                buffered: bytes[end..length].to_vec(),
            });
        }
        if length >= MAX_CONNECT_HEADER {
            return Err(ConnectRejection::Malformed);
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(ConnectRejection::Malformed);
        }
        let read_length = CONNECT_READ_CHUNK.min(bytes.len() - length);
        let read = tokio::select! {
            _ = wait_for_shutdown(shutdown) => return Err(ConnectRejection::Stopped),
            result = tokio::time::timeout(remaining, client.read(&mut bytes[length..length + read_length])) => {
                match result {
                    Ok(Ok(read)) => read,
                    Ok(Err(_)) | Err(_) => return Err(ConnectRejection::Malformed),
                }
            }
        };
        if read == 0 {
            return Err(ConnectRejection::Malformed);
        }
        length += read;
    }
}

fn header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n").map(|index| index + 4)
}

fn parse_connect_header(header: &[u8]) -> Option<&str> {
    if !header.is_ascii() {
        return None;
    }
    let text = std::str::from_utf8(header).ok()?.strip_suffix("\r\n\r\n")?;
    let mut lines = text.split("\r\n");
    let mut request = lines.next()?.split(' ');
    if request.next()? != "CONNECT" {
        return None;
    }
    let authority = request.next()?;
    if request.next()? != "HTTP/1.1" || request.next().is_some() {
        return None;
    }
    let (hostname, port) = authority.rsplit_once(':')?;
    if port != "443" || !valid_dns_hostname(hostname) {
        return None;
    }

    let mut host_seen = false;
    for line in lines {
        let (name, value) = line.split_once(':')?;
        if name.is_empty()
            || !name.bytes().all(is_header_name_byte)
            || value
                .bytes()
                .any(|byte| (byte < b' ' && byte != b'\t') || byte == 0x7f)
        {
            return None;
        }
        let value = value.trim_matches(|character| character == ' ' || character == '\t');
        if name.eq_ignore_ascii_case("host") {
            if host_seen || value != authority {
                return None;
            }
            host_seen = true;
        }
        if name.eq_ignore_ascii_case("content-length")
            || name.eq_ignore_ascii_case("transfer-encoding")
        {
            return None;
        }
    }
    host_seen.then_some(hostname)
}

fn is_header_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn is_authorized(allowed_hosts: &[String], requested_host: &str) -> bool {
    allowed_hosts.iter().any(|host| host == requested_host)
}

async fn connect_public(
    hostname: &str,
    shutdown: &mut watch::Receiver<bool>,
) -> io::Result<TcpStream> {
    tokio::select! {
        _ = wait_for_shutdown(shutdown) => Err(io::Error::new(io::ErrorKind::Interrupted, "proxy is shutting down")),
        result = tokio::time::timeout(CONNECT_TIMEOUT, async {
            let mut addresses = tokio::net::lookup_host((hostname, 443)).await?;
            let mut verified = Vec::with_capacity(4);
            for address in &mut addresses {
                if verified.len() == MAX_DNS_ADDRESSES {
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, "too many DNS results"));
                }
                if !is_public_ip(address.ip()) {
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, "CONNECT resolved to a non-public address"));
                }
                verified.push(address);
            }
            if verified.is_empty() {
                return Err(io::Error::new(io::ErrorKind::AddrNotAvailable, "CONNECT hostname did not resolve"));
            }
            let mut last_error = None;
            for address in verified {
                match TcpStream::connect(address).await {
                    Ok(stream) => return Ok(stream),
                    Err(error) => last_error = Some(error),
                }
            }
            Err(last_error.unwrap_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "CONNECT has no address")))
        }) => match result {
            Ok(result) => result,
            Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "CONNECT resolution or dial timed out")),
        }
    }
}

fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && (b == 0 || b == 168))
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    let segments = address.segments();
    // Permit only global-unicast 2000::/3, excluding protocol, documentation,
    // benchmarking, and transition ranges that can embed non-public IPv4.
    segments[0] & 0xe000 == 0x2000
        && !(segments[0] == 0x2001 && segments[1] <= 0x01ff)
        && !(segments[0] == 0x2001 && segments[1] == 0x0db8) // 2001:db8::/32
        && !(segments[0] == 0x3fff && segments[1] <= 0x000f) // 3fff::/20 documentation
        && segments[0] != 0x2002 // 6to4
}

async fn bridge<A, B>(left: A, right: B, shutdown: watch::Receiver<bool>) -> io::Result<()>
where
    A: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    B: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (left_read, left_write) = tokio::io::split(left);
    let (right_read, right_write) = tokio::io::split(right);
    let left_to_right = copy_with_timeouts(left_read, right_write, shutdown.clone());
    let right_to_left = copy_with_timeouts(right_read, left_write, shutdown);
    tokio::try_join!(left_to_right, right_to_left)?;
    Ok(())
}

async fn copy_with_timeouts<R, W>(
    mut reader: R,
    mut writer: W,
    mut shutdown: watch::Receiver<bool>,
) -> io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut copied = 0;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = tokio::select! {
            _ = wait_for_shutdown(&mut shutdown) => return Ok(copied),
            result = tokio::time::timeout(READ_IDLE_TIMEOUT, reader.read(&mut buffer)) => {
                result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "proxy read idle timeout"))??
            }
        };
        if read == 0 {
            tokio::time::timeout(WRITE_TIMEOUT, writer.shutdown())
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "proxy half-close timed out"))??;
            return Ok(copied);
        }
        tokio::select! {
            _ = wait_for_shutdown(&mut shutdown) => return Ok(copied),
            result = tokio::time::timeout(WRITE_TIMEOUT, writer.write_all(&buffer[..read])) => {
                result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "proxy write timed out"))??;
            }
        }
        copied += read as u64;
    }
}

async fn write_response(client: &mut UnixStream, response: &[u8]) -> io::Result<()> {
    tokio::time::timeout(WRITE_TIMEOUT, client.write_all(response))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "proxy response write timed out"))??;
    Ok(())
}

async fn relay_loop(listener: TcpListener, socket: PathBuf, identity: PathIdentity) -> io::Result<()> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut workers = JoinSet::new();
    let mut interval = tokio::time::interval(SOCKET_CHECK_INTERVAL);
    loop {
        tokio::select! {
            biased;
            _ = interval.tick() => {
                if !path_has_identity(&socket, identity) {
                    break;
                }
            }
            Some(_) = workers.join_next(), if !workers.is_empty() => {},
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    if let Ok(permit) = permits.clone().try_acquire_owned() {
                        let socket = socket.clone();
                        let stop = shutdown_rx.clone();
                        workers.spawn(async move {
                            let _permit = permit;
                            let _ = relay_connection(stream, socket, stop).await;
                        });
                    }
                }
                Err(error) => {
                    let _ = shutdown_tx.send(true);
                    shutdown_workers(&mut workers).await;
                    return Err(error);
                }
            }
        }
    }
    let _ = shutdown_tx.send(true);
    shutdown_workers(&mut workers).await;
    Ok(())
}

async fn relay_connection(
    client: tokio::net::TcpStream,
    socket: PathBuf,
    mut shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    let host = tokio::select! {
        _ = wait_for_shutdown(&mut shutdown) => return Ok(()),
        result = tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(socket)) => {
            result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "host proxy connect timed out"))??
        }
    };
    bridge(client, host, shutdown).await
}

async fn wait_for_shutdown(receiver: &mut watch::Receiver<bool>) {
    loop {
        if *receiver.borrow() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

async fn shutdown_workers(workers: &mut JoinSet<()>) {
    let deadline = tokio::time::Instant::now() + WORKER_DRAIN_TIMEOUT;
    while !workers.is_empty() {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() || tokio::time::timeout(remaining, workers.join_next()).await.is_err() {
            break;
        }
    }
    if !workers.is_empty() {
        workers.abort_all();
        while workers.join_next().await.is_some() {}
    }
}

fn socket_identity(path: &Path) -> io::Result<PathIdentity> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "relay endpoint is not a Unix socket",
        ));
    }
    Ok(PathIdentity::from_metadata(&metadata))
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct PathIdentity {
    device: u64,
    inode: u64,
}

impl PathIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

fn path_has_identity(path: &Path, identity: PathIdentity) -> bool {
    socket_identity(path).is_ok_and(|current| current == identity)
}

struct OwnedPath {
    path: PathBuf,
    identity: PathIdentity,
}

impl OwnedPath {
    fn new(path: &Path) -> io::Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
            identity: PathIdentity::from_metadata(&fs::symlink_metadata(path)?),
        })
    }
}

impl Drop for OwnedPath {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| PathIdentity::from_metadata(&metadata) == self.identity)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener as StdUnixListener, UnixStream as StdUnixStream};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "cruise-jcode-proxy-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap_or_else(|error| panic!("create test directory: {error}"));
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .unwrap_or_else(|error| panic!("set test directory permissions: {error}"));
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn read_response(stream: &mut StdUnixStream) -> Vec<u8> {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap_or_else(|error| panic!("set response timeout: {error}"));
        let mut response = Vec::new();
        let mut byte = [0_u8; 1];
        while !response.ends_with(b"\r\n\r\n") {
            stream
                .read_exact(&mut byte)
                .unwrap_or_else(|error| panic!("read response: {error}"));
            response.push(byte[0]);
            assert!(response.len() <= 1024, "response exceeded its bound");
        }
        response
    }

    #[test]
    fn connect_parser_rejects_non_connect_and_non_443_requests() {
        for request in [
            b"GET https://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
            b"CONNECT example.com:80 HTTP/1.1\r\nHost: example.com:80\r\n\r\n",
            b"CONNECT example.com:443 HTTP/1.0\r\nHost: example.com:443\r\n\r\n",
            b"CONNECT example.com:443 HTTP/1.1\r\nHost: other.example:443\r\n\r\n",
            b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nHost: example.com:443\r\n\r\n",
            b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nContent-Length: 1\r\n\r\n",
        ] {
            assert!(parse_connect_header(request).is_none(), "accepted {request:?}");
        }
    }

    #[test]
    fn connect_parser_preserves_tunnel_bytes_and_authorization_is_exact() {
        let header = b"CONNECT api.example.com:443 HTTP/1.1\r\nHost: api.example.com:443\r\n\r\n";
        assert_eq!(parse_connect_header(header), Some("api.example.com"));
        assert!(is_authorized(&["api.example.com".to_owned()], "api.example.com"));
        assert!(!is_authorized(&["api.example.com".to_owned()], "other.example"));

        let (mut client, server) = StdUnixStream::pair()
            .unwrap_or_else(|error| panic!("create Unix pair: {error}"));
        server
            .set_nonblocking(true)
            .unwrap_or_else(|error| panic!("set test socket nonblocking: {error}"));
        let proxy = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap_or_else(|error| panic!("build test runtime: {error}"));
            runtime.block_on(async move {
                let mut async_client = UnixStream::from_std(server)
                    .unwrap_or_else(|error| panic!("convert test socket: {error}"));
                let request = read_connect_request_for_test(&mut async_client).await;
                assert_eq!(request.hostname, "api.example.com");
                assert_eq!(request.buffered.as_slice(), b"client-first-bytes");
            });
        });
        let mut request = Vec::with_capacity(header.len() + b"client-first-bytes".len());
        request.extend_from_slice(header);
        request.extend_from_slice(b"client-first-bytes");
        client
            .write_all(&request)
            .unwrap_or_else(|error| panic!("write CONNECT request: {error}"));
        drop(client);
        proxy.join().unwrap_or_else(|_| panic!("parser worker panicked"));
    }

    async fn read_connect_request_for_test(client: &mut UnixStream) -> ConnectRequest {
        let (_shutdown, mut receiver) = watch::channel(false);
        read_connect_request(client, &mut receiver)
            .await
            .unwrap_or_else(|_| panic!("parse CONNECT request"))
    }

    #[test]
    fn unauthorized_and_malformed_connect_requests_do_not_dial() {
        let dir = TestDir::new();
        let proxy_path = dir.0.join("proxy.sock");
        let proxy = start(
            &proxy_path,
            Route::Connect(vec!["allowed.example".to_owned()]),
        )
        .unwrap_or_else(|error| panic!("start proxy: {error}"));

        for (request, expected) in [
            (
                b"CONNECT denied.example:443 HTTP/1.1\r\nHost: denied.example:443\r\n\r\n".as_slice(),
                b"HTTP/1.1 403 Forbidden\r\n\r\n".as_slice(),
            ),
            (
                b"GET https://denied.example/ HTTP/1.1\r\nHost: denied.example\r\n\r\n",
                b"HTTP/1.1 400 Bad Request\r\n\r\n",
            ),
        ] {
            let mut client = StdUnixStream::connect(&proxy_path)
                .unwrap_or_else(|error| panic!("connect to proxy: {error}"));
            client
                .write_all(request)
                .unwrap_or_else(|error| panic!("write proxy request: {error}"));
            assert_eq!(read_response(&mut client), expected);
        }
        drop(proxy);
        assert!(!proxy_path.exists(), "proxy socket was not cleaned up");
    }

    #[test]
    fn fixed_loopback_route_relays_bytes_and_drop_removes_socket() {
        let host = std::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .unwrap_or_else(|error| panic!("bind loopback fixture: {error}"));
        let port = host
            .local_addr()
            .unwrap_or_else(|error| panic!("get fixture address: {error}"))
            .port();
        let host_worker = thread::spawn(move || {
            let (mut stream, _) = host
                .accept()
                .unwrap_or_else(|error| panic!("accept fixture request: {error}"));
            let mut request = [0_u8; 13];
            stream
                .read_exact(&mut request)
                .unwrap_or_else(|error| panic!("read fixture request: {error}"));
            assert_eq!(&request, b"fixed-request");
            stream
                .write_all(b"fixed-response")
                .unwrap_or_else(|error| panic!("write fixture response: {error}"));
            stream
                .shutdown(std::net::Shutdown::Write)
                .unwrap_or_else(|error| panic!("close fixture write side: {error}"));
        });

        let dir = TestDir::new();
        let socket = dir.0.join("host.sock");
        let proxy = start(&socket, Route::Loopback(port))
            .unwrap_or_else(|error| panic!("start fixed-port proxy: {error}"));
        let mut client = StdUnixStream::connect(&socket)
            .unwrap_or_else(|error| panic!("connect to fixed-port proxy: {error}"));
        client
            .write_all(b"fixed-request")
            .unwrap_or_else(|error| panic!("write fixed request: {error}"));
        let mut response = [0_u8; 14];
        client
            .read_exact(&mut response)
            .unwrap_or_else(|error| panic!("read fixed response: {error}"));
        assert_eq!(&response, b"fixed-response");
        host_worker
            .join()
            .unwrap_or_else(|_| panic!("fixture worker panicked"));
        drop(client);
        drop(proxy);
        assert!(!socket.exists(), "proxy socket was not cleaned up");
    }

    #[test]
    fn relay_binds_only_localhost_forwards_and_cleans_up() {
        let dir = TestDir::new();
        let socket_path = dir.0.join("host.sock");
        let host = StdUnixListener::bind(&socket_path)
            .unwrap_or_else(|error| panic!("bind host socket: {error}"));
        let ready = dir.0.join("ready");
        let host_worker = thread::spawn(move || {
            let (mut stream, _) = host
                .accept()
                .unwrap_or_else(|error| panic!("accept relay socket: {error}"));
            let mut request = [0_u8; 12];
            stream
                .read_exact(&mut request)
                .unwrap_or_else(|error| panic!("read relay request: {error}"));
            assert_eq!(&request, b"guest-bytes!");
            stream
                .write_all(b"host-bytes!")
                .unwrap_or_else(|error| panic!("write relay response: {error}"));
            stream
                .shutdown(std::net::Shutdown::Write)
                .unwrap_or_else(|error| panic!("close host write side: {error}"));
        });
        let relay_socket = socket_path.clone();
        let relay_ready = ready.clone();
        let relay_worker = thread::spawn(move || relay(&relay_socket, &relay_ready, None));

        let deadline = Instant::now() + Duration::from_secs(5);
        let port = loop {
            if let Ok(value) = fs::read_to_string(&ready) {
                if let Ok(port) = value.parse::<u16>() {
                    break port;
                }
            }
            assert!(Instant::now() < deadline, "relay did not publish its port");
            thread::sleep(Duration::from_millis(5));
        };
        assert_ne!(port, 0);
        let mut client = std::net::TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
            .unwrap_or_else(|error| panic!("connect relay loopback: {error}"));
        client
            .write_all(b"guest-bytes!")
            .unwrap_or_else(|error| panic!("write relay request: {error}"));
        let mut response = [0_u8; b"host-bytes!".len()];
        client
            .read_exact(&mut response)
            .unwrap_or_else(|error| panic!("read relay response: {error}"));
        assert_eq!(&response, b"host-bytes!");
        host_worker
            .join()
            .unwrap_or_else(|_| panic!("host worker panicked"));

        fs::remove_file(&socket_path).unwrap_or_else(|error| panic!("remove host socket: {error}"));
        relay_worker
            .join()
            .unwrap_or_else(|_| panic!("relay worker panicked"))
            .unwrap_or_else(|error| panic!("relay returned an error: {error}"));
        assert!(!ready.exists(), "relay ready file was not cleaned up");
    }

    #[test]
    fn rejects_non_public_addresses() {
        for address in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.2",
            "169.254.2.1",
            "100.64.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "203.0.113.4",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "2002::1",
            "3fff::1",
        ] {
            let address = address
                .parse::<IpAddr>()
                .unwrap_or_else(|error| panic!("parse test address: {error}"));
            assert!(!is_public_ip(address), "accepted non-public {address}");
        }
        assert!(is_public_ip(
            "8.8.8.8"
                .parse()
                .unwrap_or_else(|error| panic!("parse public IPv4: {error}"))
        ));
        assert!(is_public_ip(
            "2606:4700:4700::1111"
                .parse()
                .unwrap_or_else(|error| panic!("parse public IPv6: {error}"))
        ));
    }
}
