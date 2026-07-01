//! axum handler helpers for the bridge HTTP surface.
//!
//! Embedders mount these four handlers under a path prefix of their
//! choice (`/api/mems/:name/...` is the canonical layout from the
//! plan body) behind their own auth middleware. The handlers know
//! nothing about auth, sessions, or tenancy — those are the
//! embedder's concern.
//!
//! Endpoint contract:
//! - `GET <prefix>/snapshot` — `application/zip` archive bytes.
//! - `GET <prefix>/head` — `text/plain` HEAD SHA.
//! - `GET <prefix>/commits?since=<sha>&until=<sha>` — JSON array of
//!   `CommitEnvelope`s.
//! - `GET <prefix>/events` — SSE stream emitting `mem_changed`
//!   events.
//!
//! Refusal status mapping: `UNKNOWN_MEM` → 404, `UNKNOWN_COMMIT` →
//! 404, `DELTA_TOO_LARGE` → 409, everything else → 500. Each refusal
//! also serialises an `ErrorEnvelope` JSON body.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::Stream;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio_stream::StreamExt;

use crate::builder::{
    BuildConfig, build_commit_envelope, build_commit_envelopes, build_snapshot, run_search,
};
use crate::error::{BridgeError, ErrorEnvelope};
use crate::wire::{CommitEnvelope, SearchQuery, MemChangedEvent};

/// State the embedder constructs and threads through axum. The
/// engine is shared behind a `tokio::sync::Mutex` so the
/// non-`Sync` `Engine` type lives behind an `Arc` the axum Router
/// can clone freely.
#[derive(Clone)]
pub struct BridgeState {
    pub engine: Arc<Mutex<memstead_base::Engine>>,
    pub config: BuildConfig,
    /// Optional allowlist of mems the bridge will surface. `None`
    /// means "expose every mem the engine knows about". Useful for
    /// embedders that mount the bridge under a multi-tenant routing
    /// layer and want a defence-in-depth filter on top of their own
    /// path-based mem disambiguation.
    pub allowlist: Option<Vec<String>>,
}

impl BridgeState {
    /// Construct a state with no allowlist (every engine-mounted
    /// mem is exposed).
    pub fn new(engine: Arc<Mutex<memstead_base::Engine>>) -> Self {
        Self {
            engine,
            config: BuildConfig::default(),
            allowlist: None,
        }
    }

    /// Builder-style override for the [`BuildConfig`] — typical use
    /// is tuning `delta_limit` per deployment.
    pub fn with_config(mut self, config: BuildConfig) -> Self {
        self.config = config;
        self
    }

    /// Builder-style override for the optional mem allowlist.
    pub fn with_allowlist(mut self, mems: Vec<String>) -> Self {
        self.allowlist = Some(mems);
        self
    }

    fn mem_allowed(&self, mem: &str) -> bool {
        match &self.allowlist {
            Some(list) => list.iter().any(|n| n == mem),
            None => true,
        }
    }
}

/// Map a `BridgeError` to an axum response with the right HTTP
/// status + JSON envelope body. Wire-stable.
fn error_response(err: BridgeError) -> Response {
    let status = match &err {
        BridgeError::UnknownMem(_) | BridgeError::UnknownCommit(_) => StatusCode::NOT_FOUND,
        BridgeError::DeltaTooLarge { .. } => StatusCode::CONFLICT,
        BridgeError::InvalidSearchQuery { .. } => StatusCode::BAD_REQUEST,
        BridgeError::Engine(_) | BridgeError::Git(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let envelope = ErrorEnvelope::from(&err);
    (status, Json(envelope)).into_response()
}

/// `GET <prefix>/snapshot` — returns the mem's archive bytes
/// (`application/zip`).
pub async fn snapshot_handler(
    State(state): State<BridgeState>,
    Path(mem): Path<String>,
) -> Response {
    if !state.mem_allowed(&mem) {
        return error_response(BridgeError::UnknownMem(mem));
    }
    let engine = state.engine.lock().await;
    match build_snapshot(&engine, &mem) {
        Ok(snap) => {
            let mut response = (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/zip")],
                snap.bytes,
            )
                .into_response();
            // Surface the resolved HEAD SHA as an HTTP header so
            // clients can persist their polling cursor without a
            // second round-trip to `/head`.
            response.headers_mut().insert(
                "x-memstead-head",
                header::HeaderValue::from_str(&snap.head)
                    .unwrap_or_else(|_| header::HeaderValue::from_static("")),
            );
            response
        }
        Err(e) => error_response(e),
    }
}

/// `GET <prefix>/head` — returns the mem's HEAD SHA as
/// `text/plain` (no trailing newline). Empty response when the
/// branch does not exist locally.
pub async fn head_handler(
    State(state): State<BridgeState>,
    Path(mem): Path<String>,
) -> Response {
    if !state.mem_allowed(&mem) {
        return error_response(BridgeError::UnknownMem(mem));
    }
    let engine = state.engine.lock().await;
    match build_snapshot(&engine, &mem) {
        // Reuse the snapshot path's HEAD resolution; the snapshot
        // bytes go to waste here but we keep one canonical lookup
        // path. If profiling shows the wasted export becomes a
        // bottleneck, split off a dedicated `head_for_mem` builder.
        Ok(snap) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain")],
            snap.head,
        )
            .into_response(),
        Err(e) => error_response(e),
    }
}

/// Query parameters for `GET <prefix>/commits`.
#[derive(Debug, Deserialize)]
pub struct CommitsQuery {
    /// Lower bound of the range (exclusive on git's commit walker).
    /// Empty / missing → walk from the empty-tree sentinel.
    #[serde(default)]
    pub since: String,
    /// Upper bound of the range (inclusive). Empty / missing →
    /// mem branch tip.
    #[serde(default)]
    pub until: Option<String>,
}

/// `GET <prefix>/commits?since=<sha>&until=<sha>` — JSON array of
/// commit envelopes.
pub async fn commits_handler(
    State(state): State<BridgeState>,
    Path(mem): Path<String>,
    Query(q): Query<CommitsQuery>,
) -> Response {
    if !state.mem_allowed(&mem) {
        return error_response(BridgeError::UnknownMem(mem));
    }
    let engine = state.engine.lock().await;
    let since = if q.since.is_empty() {
        memstead_base::ops::EMPTY_TREE_SHA
    } else {
        q.since.as_str()
    };
    let until = q.until.as_deref();
    match build_commit_envelopes(&engine, &mem, since, until, &state.config) {
        Ok(envs) => Json::<Vec<CommitEnvelope>>(envs).into_response(),
        Err(e) => error_response(e),
    }
}

/// `GET <prefix>/events` — SSE stream pushing `mem_changed` events
/// every time the named mem's HEAD advances. Each SSE response
/// owns a fresh subscription to the engine's broadcast channel; the
/// subscription drops automatically when the client disconnects (axum
/// drops the future, which drops the `SubscriptionHandle` we capture
/// inside the stream).
pub async fn events_handler(
    State(state): State<BridgeState>,
    Path(mem): Path<String>,
) -> Response {
    if !state.mem_allowed(&mem) {
        return error_response(BridgeError::UnknownMem(mem));
    }
    let (handle, rx) = {
        let engine = state.engine.lock().await;
        match engine.subscribe_mem_changes_broadcast(&mem) {
            Ok(pair) => pair,
            Err(memstead_base::EngineError::UnknownMem(name)) => {
                return error_response(BridgeError::UnknownMem(name));
            }
            Err(other) => return error_response(BridgeError::Engine(other.to_string())),
        }
    };
    Sse::new(events_stream(rx, handle))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

/// Map a tokio broadcast `Receiver<memstead_base::MemChangedEvent>`
/// into the SSE event stream the handler returns. `handle` is moved
/// in so the engine subscription stays alive for the lifetime of the
/// stream — dropping the stream drops the handle, which unsubscribes
/// from the engine.
fn events_stream(
    rx: tokio::sync::broadcast::Receiver<memstead_base::engine::MemChangedEvent>,
    handle: memstead_base::engine::SubscriptionHandle,
) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
    let stream = tokio_stream::wrappers::BroadcastStream::new(rx);
    // The closure captures `handle` so dropping the stream drops the
    // engine subscription. `handle` itself doesn't need to be
    // surfaced to the SSE consumer — it just lives for the duration
    // of the stream.
    let kept = Arc::new(handle);
    stream.map(
        move |item: Result<
            memstead_base::engine::MemChangedEvent,
            tokio_stream::wrappers::errors::BroadcastStreamRecvError,
        >| {
            // Keep the handle reference inside the closure so it shares
            // the stream's lifetime. The `_alive` clone is otherwise
            // unused — but the `Arc` strong-count keeps the
            // subscription alive for the stream's lifetime regardless
            // of which clone the runtime holds onto last.
            let _alive = kept.clone();
            let event = match item {
                Ok(core_event) => {
                    let wire: MemChangedEvent = core_event.into();
                    let data = serde_json::to_string(&wire).unwrap_or_else(|_| "{}".to_string());
                    Event::default().event("mem_changed").data(data)
                }
                // Lagged subscribers get a structured note rather than
                // silent gaps. The client re-syncs via `/commits` from
                // its last-known cursor.
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                    Event::default()
                        .event("lagged")
                        .data(format!("{{\"missed\":{n}}}"))
                }
            };
            Ok::<Event, Infallible>(event)
        },
    )
}

/// Single envelope endpoint — useful for clients that already know
/// the SHA they want (e.g. resolving a `Replays:` trailer pointer
/// from an earlier envelope). Not part of the canonical four
/// endpoints in the plan's body, but `build_commit_envelope` is
/// exposed in the public API and a thin axum wrapper keeps the
/// HTTP surface symmetric for embedders who prefer endpoint reads
/// over the range API.
pub async fn commit_handler(
    State(state): State<BridgeState>,
    Path((mem, sha)): Path<(String, String)>,
) -> Response {
    if !state.mem_allowed(&mem) {
        return error_response(BridgeError::UnknownMem(mem));
    }
    let engine = state.engine.lock().await;
    match build_commit_envelope(&engine, &mem, &sha) {
        Ok(env) => Json(env).into_response(),
        Err(e) => error_response(e),
    }
}

/// `GET <prefix>/search?q=<query>&type=<entity-type>&limit=<n>&offset=<n>`
/// — JSON [`crate::wire::SearchResult`].
///
/// Read-only path that delegates to [`run_search`]; the engine lock
/// only spans the actual `Engine::search` call (held inside
/// `run_search`). Refusal status mapping inherits from
/// [`error_response`]: `INVALID_SEARCH_QUERY` → 400,
/// `UNKNOWN_MEM` → 404, `ENGINE_ERROR` → 500.
///
/// The canonical consumer is the WASM engine (`memstead-wasm`)
/// — its `engine.search(...)` always throws
/// `SEARCH_UNAVAILABLE_IN_WASM`; browser callers route the query
/// here.
pub async fn search_handler(
    State(state): State<BridgeState>,
    Path(mem): Path<String>,
    Query(q): Query<SearchQuery>,
) -> Response {
    if !state.mem_allowed(&mem) {
        return error_response(BridgeError::UnknownMem(mem));
    }
    let engine = state.engine.lock().await;
    match run_search(&engine, &mem, q, &state.config) {
        Ok(result) => Json(result).into_response(),
        Err(e) => error_response(e),
    }
}
