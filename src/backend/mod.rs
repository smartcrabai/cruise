//! Cruise-owned types for the in-process prompt-execution backends.
//!
//! [`stream`] is what a backend emits while a prompt runs, [`effort`] is the
//! reasoning-effort tier carried by a `model[:effort]` reference, and [`tool`]
//! is a custom tool cruise injects into a run. Each backend adapts these to
//! whatever shape its own SDK wants, so that the SDK's type layout stays behind
//! that adapter instead of spreading into `executor.rs`, `planning.rs`, or the
//! tool definitions. The seher backends are adapted in `executor.rs` until they
//! are removed.

pub mod effort;
pub mod stream;
pub mod tool;
