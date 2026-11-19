pub mod api;
pub mod client;
pub mod events;
pub mod instance;
pub mod legacy;
pub mod lifecycle;
pub mod log_rotation;
pub mod server;
pub mod shutdown;
pub(crate) mod web_auth;

use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub(crate) struct ServeOptions {
    pub(crate) host: Option<String>,
    pub(crate) port: Option<u16>,
    pub(crate) port_explicit: Option<bool>,
    pub(crate) open: bool,
    pub(crate) webui_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct StopOptions {
    pub(crate) force: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct StatusOptions {
    pub(crate) json: bool,
}
