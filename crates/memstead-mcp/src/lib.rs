//! Memstead MCP server — exposes the Memstead engine via the Model
//! Context Protocol over STDIO.
//!
//! One crate, two build configs. The default build (`vault-repo`
//! feature on) is the full server: the multi-vault, git-backed
//! `ServerHandler` ([`server::McpServer`]) and its tool router, plus the
//! support modules ([`config`], [`lifecycle`], [`read_vaults`],
//! [`error_envelope`]). `--no-default-features` drops the git-branch
//! backend, leaving the folder + archive `ServerHandler`
//! ([`filesystem_server::FilesystemMcpServer`]) — a CI / wasm-adjacent
//! config, not shipped.
//!
//! Shared building blocks used by both servers live here unconditionally:
//! [`error_envelopes`] (validation envelope) and [`tools`] (tool
//! parameter structs).
//!
//! The binary entry point ([`main.rs`](main.rs)) stays thin: argument
//! parsing, logging, then delegation into this crate.

pub mod error_envelopes;
pub mod filesystem_server;
pub mod tools;

// Multi-vault / git-backed server, compiled into the full `memstead-mcp`
// binary (default features); absent from the lean `--no-default-features`
// build, which has no git-branch backend.
#[cfg(feature = "vault-repo")]
pub mod config;
#[cfg(feature = "vault-repo")]
pub mod error_envelope;
#[cfg(feature = "vault-repo")]
pub mod lifecycle;
#[cfg(feature = "vault-repo")]
pub mod read_vaults;
#[cfg(feature = "vault-repo")]
pub mod server;
