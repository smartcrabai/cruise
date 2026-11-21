use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::application::CruiseApplication;
use crate::daemon::ServeOptions;
use crate::daemon::instance::{self, InstanceRecord, Registry};
use crate::error::{CruiseError, Result};
use crate::session::SessionManager;

use crate::webui::{
    assets, css::GeneratedCss, events::EventHub, run_all_state::RunAllState, templates::Templates,
};

#[derive(Clone)]
pub(crate) struct WebState {
    pub(crate) application: CruiseApplication,
    pub(crate) hub: Arc<EventHub>,
    pub(crate) templates: Arc<Templates>,
    pub(crate) run_all: Arc<RunAllState>,
    pub(crate) templates_dir: PathBuf,
    pub(crate) static_dir: PathBuf,
    pub(crate) css: Arc<GeneratedCss>,
    pub(crate) process_locale: crate::i18n::Locale,
    pub(crate) api_token: Option<String>,
    pub(crate) origin: Option<String>,
    pub(crate) login_nonces: Arc<Mutex<HashMap<String, Instant>>>,
    pub(crate) web_sessions: Arc<Mutex<HashSet<String>>>,
    pub(crate) control: Option<Arc<ControlGate>>,
    pub(crate) manager_startup_error: Option<String>,
}

pub(crate) struct ControlGate {
    pub(crate) sender: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl std::fmt::Debug for WebState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("WebState").finish_non_exhaustive()
    }
}

impl WebState {
    pub(crate) fn new(application: CruiseApplication, assets: &assets::Assets) -> Result<Self> {
        let generated = if assets.dev_mode {
            None
        } else {
            Some(crate::webui::css::generate(
                &assets.templates_dir,
                &assets.static_dir,
            )?)
        };
        let hub = Arc::new(EventHub::with_notifications(application.clone()));
        Ok(Self {
            application,
            hub,
            templates: Arc::new(Templates::new(assets.templates_dir.clone())),
            run_all: Arc::new(RunAllState::default()),
            templates_dir: assets.templates_dir.clone(),
            static_dir: assets.static_dir.clone(),
            css: Arc::new(GeneratedCss::new(generated, assets.dev_mode)),
            process_locale: crate::i18n::process_locale(),
            api_token: None,
            origin: None,
            login_nonces: Arc::new(Mutex::new(HashMap::new())),
            web_sessions: Arc::new(Mutex::new(HashSet::new())),
            control: None,
            manager_startup_error: None,
        })
    }
}

/// Run the daemon foreground with the same registry and listener lifecycle as a detached daemon.
pub(crate) async fn run_foreground(args: ServeOptions) -> Result<()> {
    crate::daemon::legacy::ensure_stopped_before_start_at_state_dir()?;
    let registry = Registry::new()?;
    let lock = registry.lock()?;
    if let Some(record) = registry.read_for_start()?
        && record.verify_identity()?
    {
        return Err(CruiseError::Other(format!(
            "daemon is already running at {} (PID {}); use cruise daemon stop",
            record.origin, record.pid
        )));
    }
    run_instance(
        args,
        uuid::Uuid::new_v4().to_string(),
        false,
        None,
        registry,
        Some(lock),
    )
    .await
}

pub(crate) async fn run_daemon(
    instance_id: String,
    host: String,
    port: Option<u16>,
    port_implicit: bool,
    webui_dir: Option<PathBuf>,
) -> Result<()> {
    crate::daemon::legacy::ensure_stopped_before_start_at_state_dir()?;
    let registry = Registry::new()?;
    run_instance(
        ServeOptions {
            host: Some(host),
            port,
            open: false,
            webui_dir,
            port_explicit: port_implicit.then_some(false),
        },
        instance_id,
        true,
        port_implicit.then_some(false),
        registry,
        None,
    )
    .await
}

fn filesystem_root(path: &Path) -> Result<PathBuf> {
    path.ancestors()
        .last()
        .filter(|root| root.is_absolute())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            CruiseError::Other(format!(
                "cannot determine filesystem root for daemon data directory {}",
                path.display()
            ))
        })
}

#[expect(
    clippy::too_many_lines,
    reason = "Instance startup, serving, and cleanup form one daemon lifecycle boundary."
)]
async fn run_instance(
    args: ServeOptions,
    instance_id: String,
    daemon: bool,
    port_explicit_override: Option<bool>,
    registry: Registry,
    lock: Option<instance::RegistryLock>,
) -> Result<()> {
    let webui_dir = args
        .webui_dir
        .map(|path| {
            if path.is_absolute() {
                Ok(path)
            } else {
                std::env::current_dir().map(|current| current.join(path))
            }
        })
        .transpose()?;
    let home_dir = home::home_dir().filter(|home| home.is_dir());
    let data_dir = crate::paths::data_dir()?;
    let data_dir = if data_dir.is_absolute() {
        data_dir
    } else {
        let base = if let Some(home) = &home_dir {
            home.clone()
        } else {
            std::env::current_dir()?
        };
        base.join(data_dir)
    };
    // WebUI requests without a client context default to the home directory,
    // or the data directory when no home is available. Keep the daemon itself
    // at the data volume's root so user data can be removed while it is alive.
    let workdir = home_dir.unwrap_or_else(|| data_dir.clone());
    std::fs::create_dir_all(&workdir)?;
    std::env::set_current_dir(filesystem_root(&data_dir)?)?;
    let assets = assets::prepare(webui_dir, &data_dir)?;
    let application = CruiseApplication::new(SessionManager::new(data_dir));
    let mut state = WebState::new(application, &assets)?;
    let signals = crate::daemon::shutdown::ShutdownSignals::new()?;

    let host = args.host.unwrap_or_else(|| "127.0.0.1".to_owned());
    let requested_port = match args.port {
        Some(port) => Some(port),
        None => std::env::var_os("CRUISE_DAEMON_PORT")
            .map(|value| {
                let value = value.to_string_lossy();
                value.parse::<u16>().map_err(|error| {
                    CruiseError::Other(format!("invalid CRUISE_DAEMON_PORT '{value}': {error}"))
                })
            })
            .transpose()?,
    };
    let port_explicit = port_explicit_override.unwrap_or(requested_port.is_some());
    let requested = requested_port.unwrap_or(8484);
    let listener = match tokio::net::TcpListener::bind((host.as_str(), requested)).await {
        Ok(listener) => listener,
        Err(error) if requested_port.is_none() && error.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!(
                "warning: default daemon port {requested} is busy; using an ephemeral port. Set --port or CRUISE_DAEMON_PORT explicitly on shared hosts."
            );
            tokio::net::TcpListener::bind((host.as_str(), 0))
                .await
                .map_err(|error| {
                    CruiseError::Other(format!(
                        "failed to bind {host}:0 after default port collision: {error}"
                    ))
                })?
        }
        Err(error) => {
            return Err(CruiseError::Other(format!(
                "failed to bind {host}:{requested}: {error}"
            )));
        }
    };
    let local_addr = listener
        .local_addr()
        .map_err(|error| CruiseError::Other(error.to_string()))?;
    let origin = format!("http://{local_addr}/");
    state.origin = Some(origin.clone());
    if let Some(record) = registry.read_for_start()?
        && record.verify_identity()?
    {
        return Err(CruiseError::Other(format!(
            "daemon is already running at {} (PID {})",
            record.origin, record.pid
        )));
    }
    let token = instance::new_token();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    state.api_token = Some(token.clone());
    state.control = Some(Arc::new(ControlGate {
        sender: Mutex::new(Some(sender)),
    }));
    let pid = std::process::id();
    let record = InstanceRecord {
        schema_version: 1,
        instance_id: instance_id.clone(),
        mode: if daemon { "daemon" } else { "foreground" }.to_owned(),
        pid,
        process_started_at: instance::process_start_time(pid).ok_or_else(|| {
            CruiseError::Other("cannot determine daemon process start time".to_owned())
        })?,
        process_identity: Some(instance::precise_identity(pid).ok_or_else(|| {
            CruiseError::Other("cannot determine precise daemon process identity".to_owned())
        })?),
        host,
        port: local_addr.port(),
        origin: origin.clone(),
        token,
        port_explicit,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        executable: std::env::current_exe()?,
        working_dir: std::env::current_dir()?,
        log_path: daemon.then(|| registry.log_path()),
        started_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| CruiseError::Other(error.to_string()))?
            .as_secs(),
    };
    registry.write(&record)?;
    // Hold ownership through recovery, before accepting requests.
    state.manager_startup_error = manager_startup_error(&state.application).await;
    drop(lock);
    if daemon {
        crate::daemon::log_rotation::start(registry.log_path());
    }
    println!("cruise daemon listening on {origin}");
    let login_url = crate::daemon::web_auth::new_login_url(&state, &origin);
    println!("cruise daemon WebUI login: {login_url}");
    if !local_addr.ip().is_loopback() {
        eprintln!(
            "warning: non-loopback WebUI binds use plain HTTP; use only on trusted networks or behind a TLS proxy"
        );
    }
    if args.open
        && let Err(error) = crate::platform::open_login_redirect(&login_url)
    {
        eprintln!("could not open a browser ({error}); open {login_url} manually");
    }
    let watch_registry = registry.clone();
    let watch_id = instance_id.clone();
    let signal = crate::daemon::shutdown::production_signal(signals, receiver, async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            match watch_registry.read() {
                Ok(Some(record)) if record.instance_id == watch_id => {}
                Ok(None | Some(_)) => break,
                Err(error) => {
                    eprintln!(
                        "warning: failed to read daemon registry while watching for shutdown: {error}"
                    );
                }
            }
        }
    });
    let result = serve_reconciled(listener, state.clone(), signal).await;
    let cleanup = registry.remove_if_owned(&instance_id);
    result?;
    cleanup?;
    Ok(())
}

/// Serve the `WebUI` and daemon API until shutdown begins, then drain operations.
#[cfg(test)]
pub(crate) async fn serve<F>(
    listener: tokio::net::TcpListener,
    mut state: WebState,
    signal: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    state.manager_startup_error = manager_startup_error(&state.application).await;
    serve_reconciled(listener, state, signal).await
}

async fn manager_startup_error(application: &CruiseApplication) -> Option<String> {
    let mut failures = Vec::new();
    match crate::manager_coordinator::reconcile_applied(application).await {
        Ok(errors) => failures.extend(errors),
        Err(error) => failures.push(format!("effect recovery: {error}")),
    }
    match crate::manager::reconcile_startup() {
        Ok(errors) => failures.extend(errors),
        Err(error) => failures.push(format!("startup reconciliation: {error}")),
    }
    if failures.is_empty() {
        None
    } else {
        let error = failures.join("; ");
        eprintln!("warning: Manager scheduling disabled until daemon restart: {error}");
        Some(error)
    }
}

async fn serve_reconciled<F>(
    listener: tokio::net::TcpListener,
    state: WebState,
    signal: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let shutdown_state = state.clone();
    let loopback = listener
        .local_addr()
        .map_err(|error| CruiseError::Other(error.to_string()))?
        .ip()
        .is_loopback();
    let router = application_router(state.clone(), loopback);
    let manager_stop = crate::cancellation::CancellationToken::new();
    let shutdown_manager = manager_stop.clone();
    let (manager_sink, manager_log_sink) = state.hub.sinks();
    let manager_task = state.manager_startup_error.is_none().then(|| {
        tokio::spawn(crate::manager_coordinator::run_daemon(
            state.application.clone(),
            manager_sink,
            manager_log_sink,
            manager_stop.clone(),
        ))
    });
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            signal.await;
            crate::manager_coordinator::stop(&shutdown_manager);
            crate::daemon::shutdown::begin(&shutdown_state);
        })
        .await;
    crate::manager_coordinator::stop(&manager_stop);
    // This is idempotent and also covers a server error before the signal.
    let _ = state.application.runtime().begin_shutdown();
    if let Some(manager_task) = manager_task {
        manager_task.await.map_err(|error| {
            CruiseError::Other(format!("Manager coordinator task failed: {error}"))
        })?;
    }
    crate::daemon::shutdown::wait_for_operations(&state.application).await;
    result.map_err(|error| CruiseError::Other(error.to_string()))?;
    Ok(())
}

pub(crate) fn application_router(state: WebState, loopback: bool) -> axum::Router {
    crate::webui::router(state.clone())
        .layer(axum::middleware::from_fn(webui_security_headers))
        .nest(
            "/api/manager",
            crate::daemon::api::manager_router(state.clone()),
        )
        .nest("/api/v1", crate::daemon::api::router(state))
        .layer(axum::middleware::from_fn_with_state(
            crate::daemon::web_auth::HostGuard { loopback },
            crate::daemon::web_auth::guard,
        ))
}

async fn webui_security_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        axum::http::HeaderName::from_static("x-frame-options"),
        axum::http::HeaderValue::from_static("DENY"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("content-security-policy"),
        axum::http::HeaderValue::from_static(
            "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; frame-src 'none'; form-action 'self'; script-src 'self' https://cdn.jsdelivr.net; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' http: https:; font-src 'self'",
        ),
    );
    response
}

pub(crate) fn signal_shutdown(state: &WebState) -> Option<usize> {
    let control = state.control.as_ref()?;
    let active = state.application.runtime().begin_shutdown()
        + crate::manager_coordinator::active_operations();
    if let Ok(mut sender) = control.sender.lock()
        && let Some(sender) = sender.take()
    {
        let _ = sender.send(());
    }
    Some(active)
}
