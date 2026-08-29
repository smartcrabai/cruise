//! Cruise-owned types for the in-process prompt-execution backends, plus the
//! backends themselves.
//!
//! [`stream`] is what a backend emits while a prompt runs, [`effort`] is the
//! reasoning-effort tier carried by a `model[:effort]` reference, and [`tool`]
//! is a custom tool cruise injects into a run. Each backend adapts these to
//! whatever shape its own SDK wants, so that the SDK's type layout stays behind
//! that adapter instead of spreading into `executor.rs`, `planning.rs`, or the
//! tool definitions. [`claude`] is the `sdk: claude` backend and [`jcode`] the
//! `sdk: jcode` one; the seher backends are adapted in `executor.rs` until they
//! are removed.

pub(crate) mod claude;
pub mod effort;
pub mod jcode;
pub mod stream;
pub mod tool;
