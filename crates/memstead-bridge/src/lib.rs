//! Read-only HTTP bridge library for Memstead thin clients.
//!
//! Serves the browser-sync wire format (commit-range snapshots + SSE
//! change events) over HTTP.
//!
//! Surface:
//! - [`wire`] — Serde structs that mirror the JSON wire format
//!   one-to-one (`CommitEnvelope`, `EntityChange`,
//!   `MemChangedEvent`). The durable contract between server
//!   implementations and client code.
//! - [`builder`] — functions that turn engine state into envelopes:
//!   per-commit `build_commit_envelope`, range
//!   `build_commit_envelopes`, snapshot `build_snapshot`. Typed
//!   refusal codes (`UNKNOWN_MEM`, `UNKNOWN_COMMIT`,
//!   `DELTA_TOO_LARGE`).
//! - [`error`] — `BridgeError` typed-refusal envelope.
//!
//! Read-only by construction: nothing in this crate calls any
//! mutating engine operation. The `no_mutating_engine_call_in_bridge_surface`
//! source-scan test (in `builder`) pins that invariant — it fails if
//! any mutating engine method call is introduced into the crate's
//! production source.

pub mod builder;
pub mod error;
pub mod handlers;
pub mod wire;

pub use builder::{
    build_commit_envelope, build_commit_envelopes, build_snapshot, run_search, BuildConfig,
    DEFAULT_DELTA_LIMIT, DEFAULT_SEARCH_LIMIT, DEFAULT_SEARCH_MAX_LIMIT, SnapshotOutput,
};
pub use error::BridgeError;
pub use handlers::{
    BridgeState, CommitsQuery, commit_handler, commits_handler, events_handler, head_handler,
    search_handler, snapshot_handler,
};
pub use wire::{CommitEnvelope, EntityChange, SearchHit, SearchQuery, SearchResult, MemChangedEvent};
