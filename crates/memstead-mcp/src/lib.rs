//! Memstead MCP server — exposes the Memstead engine via the Model
//! Context Protocol over STDIO.
//!
//! One server: the multi-mem, git-backed `ServerHandler`
//! ([`server::McpServer`]) and its tool router, plus the support modules
//! ([`config`], [`lifecycle`], [`read_mems`], [`error_envelope`]). The
//! same server serves folder-only workspaces (the shape `memstead
//! quickstart` produces) and mem-repo workspaces; the tool roster is
//! the same on both. [`error_envelopes`] carries the validation
//! envelope and [`tools`] the tool parameter structs.
//!
//! The binary entry point ([`main.rs`](main.rs)) stays thin: argument
//! parsing, logging, then delegation into this crate.

pub mod coverage;
pub mod descriptions;
pub mod error_envelope;
pub mod error_envelopes;
pub mod tools;

pub mod config;
pub mod lifecycle;
pub mod read_mems;
pub mod server;

/// Acquire an engine mutex on a tool dispatch path and open the
/// operation scope over it: the guard comes back wrapped in a
/// `memstead_base::OperationScope`, which derefs to the engine and is
/// the only channel for the reload notices (`finish` hands them out
/// with the guard; a dropped scope discards them). A poisoned lock
/// (a prior tool call panicked) early-returns the typed
/// `ENGINE_LOCK_POISONED` envelope instead of panicking the server —
/// usable only inside functions returning `CallToolResult`.
#[macro_export]
macro_rules! lock_engine {
    ($mutex:expr) => {
        match $mutex.lock() {
            Ok(guard) => ::memstead_base::OperationScope::begin(guard),
            Err(_) => return $crate::error_envelopes::engine_lock_poisoned(),
        }
    };
}
