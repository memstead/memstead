//! filesystem-mem surfaces — single-mem, history-free, filesystem-backed
//! workspaces.
//!
//! Holds the helper modules consumed by the unified
//! [`crate::Engine`] when it routes through a folder-backed mem:
//! [`changelog`] (the JSONL provenance log), [`config`]
//! (`.memstead/config.json` reader / writer) and [`publish`] (the
//! `.mem` archive assembler — engine-free).
//!
//! filesystem-mem `.memstead/config.json` is a different shape from the archive's
//! `.memstead/config.json` (which lives inside a published `.mem` zip):
//! the workspace shape carries workspace-local fields the archive
//! never publishes, while the archive shape is the strict
//! whitelist projection enforced by [`super::validator::config`]. Both
//! validators live in this crate.
//!
//! [`super::validator::config`]: super::validator::config

pub mod changelog;
pub mod config;
pub mod publish;
