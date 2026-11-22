//! Cruise-owned types for the in-process jcode prompt-execution backend.
//!
//! [`stream`] is what the backend emits while a prompt runs, [`effort`] is the
//! reasoning-effort tier carried by a `model[:effort]` reference, and [`tool`]
//! is a custom tool Cruise injects into a run.

pub(crate) mod conversation;
pub mod effort;
pub mod jcode;
pub mod stream;
pub mod tool;
