//! Typed refusal envelope for bridge ops.
//!
//! Mirrors the wire-error contract a client sees: a stable `code`
//! token + human message + structured details. Each variant carries
//! the data downstream consumers (axum handlers, embedder webapps)
//! need to map the refusal into the correct HTTP status without
//! re-parsing the message text.

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// Named mem is not mounted in the engine.
    #[error("unknown mem: {0}")]
    UnknownMem(String),
    /// The provided commit SHA does not resolve in the mem-repo
    /// gitdir. Maps to HTTP 404. Force-pushed / GC'd commits surface
    /// here.
    #[error("unknown commit: {0}")]
    UnknownCommit(String),
    /// The requested commit range exceeds the configured delta
    /// threshold (default 50, see [`crate::DEFAULT_DELTA_LIMIT`]).
    /// Clients should re-snapshot rather than paginate the delta.
    /// Maps to HTTP 409.
    #[error("delta too large: {n_commits} > {limit}")]
    DeltaTooLarge { n_commits: u32, limit: u32 },
    /// Underlying engine operation failed (snapshot export, etc.).
    /// Generic wrapper for non-typed failures from `memstead-base`.
    #[error("engine error: {0}")]
    Engine(String),
    /// Underlying gix / I/O failure during tree walk or blob read.
    #[error("git error: {0}")]
    Git(String),
    /// `/search` was invoked with a payload the bridge refuses on its
    /// own (empty `q`, out-of-range `limit`). `reason` carries the
    /// agent-actionable detail. Maps to HTTP 400.
    #[error("invalid search query: {reason}")]
    InvalidSearchQuery { reason: String },
}

impl BridgeError {
    /// Stable UPPER_SNAKE_CASE code token. Wire-stable; axum handlers
    /// pin HTTP status codes against these tokens, and client SDKs
    /// pattern-match on them.
    pub fn code(&self) -> &'static str {
        match self {
            BridgeError::UnknownMem(_) => "UNKNOWN_MEM",
            BridgeError::UnknownCommit(_) => "UNKNOWN_COMMIT",
            BridgeError::DeltaTooLarge { .. } => "DELTA_TOO_LARGE",
            BridgeError::Engine(_) => "ENGINE_ERROR",
            BridgeError::Git(_) => "GIT_ERROR",
            BridgeError::InvalidSearchQuery { .. } => "INVALID_SEARCH_QUERY",
        }
    }
}

/// JSON wire shape for a refusal — what the axum handlers serialise
/// alongside the HTTP status. Stable; clients pattern-match on `code`.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorEnvelope {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl From<&BridgeError> for ErrorEnvelope {
    fn from(e: &BridgeError) -> Self {
        let details = match e {
            BridgeError::UnknownMem(name) => {
                Some(serde_json::json!({ "mem": name }))
            }
            BridgeError::UnknownCommit(sha) => {
                Some(serde_json::json!({ "commit": sha }))
            }
            BridgeError::DeltaTooLarge { n_commits, limit } => {
                Some(serde_json::json!({
                    "n_commits": n_commits,
                    "limit": limit,
                }))
            }
            BridgeError::Engine(_) | BridgeError::Git(_) => None,
            BridgeError::InvalidSearchQuery { reason } => {
                Some(serde_json::json!({ "reason": reason }))
            }
        };
        Self {
            code: e.code(),
            message: e.to_string(),
            details,
        }
    }
}
