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
//!
//! A program that links the engine as a library builds on [`library`],
//! the one supported library surface of the engine crates, and on
//! nothing else here.

pub mod coverage;
pub mod descriptions;
pub mod error_envelope;
pub mod error_envelopes;
pub mod tools;

pub mod config;
pub mod library;
pub mod lifecycle;
pub mod read_mems;
pub mod server;

/// Acquire an engine mutex on a tool dispatch path and open the
/// operation scope over it for the server's caller: the guard comes
/// back wrapped in a [`server::ToolScope`], which derefs to the engine
/// and is the channel for the change notices (`finish` hands them out
/// with the guard; a scope dropped without it leaves them for the
/// dispatch to add to the response). The
/// notices are relative to the caller of `$server`, and the client the
/// engine records for writes it stamps itself is that instance's, so
/// several server instances over one engine are each told what the
/// others wrote and never write under another's client. A poisoned lock
/// (a prior tool call panicked) early-returns the typed
/// `ENGINE_LOCK_POISONED` envelope instead of panicking the server —
/// usable only inside functions returning `CallToolResult`.
#[macro_export]
macro_rules! lock_engine {
    ($mutex:expr, $server:expr) => {
        match $mutex.lock() {
            // What belongs to this instance's caller is set for this
            // operation: several instances may share the engine.
            Ok(guard) => $server.open_operation(guard),
            Err(_) => return $crate::error_envelopes::engine_lock_poisoned(),
        }
    };
}
