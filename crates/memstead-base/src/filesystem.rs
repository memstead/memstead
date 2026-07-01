//! filesystem-mem surfaces — single-mem, history-free, filesystem-backed
//! workspaces.
//!
//! Holds the helper modules consumed by the unified
//! [`crate::Engine`] when it routes through a folder-backed mem:
//! [`changelog`] (the JSONL provenance log), [`config`]
//! (`.memstead/config.json` reader / writer), [`publish`] (the
//! `.mem` archive assembler — engine-free), [`tier3`] (cross-mem
//! cache references).
//!
//! filesystem-mem `.memstead/config.json` is a different shape from the archive's
//! `.memstead/config.json` (which lives inside a published `.mem` zip):
//! the workspace shape carries cross-mem deps and other
//! workspace-local fields, while the archive shape is the strict
//! whitelist projection enforced by [`super::validator::config`]. Both
//! validators live in this crate.
//!
//! [`super::validator::config`]: super::validator::config

pub mod changelog;
pub mod config;
pub mod publish;
pub mod tier3;
