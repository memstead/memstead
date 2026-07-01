//! `wasm-bindgen` bindings exposing the Memstead engine to browser thin
//! clients.
//!
//! Phase 1 of the browser-distribution architecture.
//! This crate is a thin wrapper over `memstead-base::Engine`: every entry
//! point delegates to a method that already exists on the engine, and
//! the engine itself has been cfg-gated so it
//! builds clean for `wasm32-unknown-unknown` without tantivy /
//! getrandom-0.2 / memmap2 / rayon transitives.
//!
//! ## JS API
//!
//! ```ignore
//! import init, { Engine, setPanicHook } from "@memstead/wasm";
//!
//! await init();
//! setPanicHook(); // optional — readable JS stack traces on panic
//!
//! const engine = Engine.fromSnapshot(archiveBytes);
//! engine.applyCommit(envelope);
//! const entity = engine.getEntity("specs--alpha");
//! const summary = engine.health();
//! // engine.search(...) throws { code: "SEARCH_UNAVAILABLE_IN_WASM", ... }
//! ```
//!
//! ## Error envelope
//!
//! Every refusing entry point throws a JS object shaped like
//! `{ code: "STABLE_CODE", message: "human-readable detail" }` — same
//! discipline the MCP server uses on the wire. JS callers branch on
//! `code` (stable, UPPER_SNAKE_CASE) and never on `message`.

use memstead_base::engine::{EngineError, FromArchiveBytesError};
use memstead_base::entity::EntityId;
use memstead_base::ops::{CommitEnvelope, SearchScope};
use memstead_base::Engine as BaseEngine;
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// Serialize a value to a `JsValue` with Rust maps rendered as plain JS
/// **objects** rather than ES `Map`s. `serde_wasm_bindgen::to_value`
/// defaults to `Map`, which silently breaks `Object.entries` /
/// `Object.keys` consumers and contradicts the documented
/// `@memstead/client` `Entity` shape (`sections: Record<string,string>`,
/// `metadata: Record<string,unknown>`). Map-bearing reads (`getEntity`,
/// `health`) must go through this.
fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
    value
        .serialize(&serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true))
        .map_err(|e| err_object("SERIALIZATION_FAILED", &e.to_string()))
}

/// Install [`console_error_panic_hook`] so panics from inside the
/// wasm runtime surface as readable JS stack traces. Idempotent —
/// safe to call multiple times. Costs ~3 KB; embedders that care
/// about the bare minimum bundle can omit the call (or disable the
/// `panic-hook` feature at build time).
#[cfg(feature = "panic-hook")]
#[wasm_bindgen(js_name = setPanicHook)]
pub fn set_panic_hook() {
    console_error_panic_hook::set_once();
}

/// JS-visible handle wrapping `memstead_base::Engine`. Owns the in-memory
/// store. One instance per `.mem` snapshot the client hydrates.
///
/// Mutation methods (`memstead_create`, `memstead_update`, etc.) are
/// intentionally **not** exposed — the WASM engine is the read-side
/// of the browser-sync architecture; writes happen on the server and
/// flow back through `applyCommit`.
#[wasm_bindgen]
pub struct Engine {
    inner: BaseEngine,
}

#[wasm_bindgen]
impl Engine {
    /// Hydrate an engine from a `.mem` snapshot. Accepts the same
    /// byte range any `application/zip` response body would carry
    /// (the bridge's `/snapshot` endpoint, an in-page `fetch`, a
    /// pre-bundled asset). Returns the engine handle on success;
    /// throws a `{ code, message }` envelope on validation /
    /// configuration failures.
    #[wasm_bindgen(js_name = fromSnapshot)]
    pub fn from_snapshot(bytes: Vec<u8>) -> Result<Engine, JsValue> {
        BaseEngine::from_archive_bytes(bytes)
            .map(|inner| Engine { inner })
            .map_err(from_archive_bytes_err)
    }

    /// Apply an externally-produced commit envelope to the in-memory
    /// store. Parsed against the mem's pinned schema; on any parse
    /// failure the entire envelope is refused and the store stays at
    /// its prior SHA. Emits the same `MemChangedEvent` every other
    /// mem-advance flows through.
    ///
    /// The `envelope` parameter accepts the JSON shape the bridge's
    /// `/commits` endpoint produces — `serde-wasm-bindgen` decodes
    /// the JS object into a `CommitEnvelope`. Decode failures throw a
    /// `{ code: "INVALID_INPUT", message }` envelope so JS callers
    /// can branch on the same code MCP would surface.
    #[wasm_bindgen(js_name = applyCommit)]
    pub fn apply_commit(&mut self, envelope: JsValue) -> Result<(), JsValue> {
        let env: CommitEnvelope = serde_wasm_bindgen::from_value(envelope)
            .map_err(|e| err_object("INVALID_INPUT", &e.to_string()))?;
        self.inner
            .apply_external_commit(&env)
            .map_err(engine_err)
    }

    /// Read one entity by id (`<mem>--<slug>` shape). Returns
    /// `undefined` when the id is not in the store — same shape the
    /// MCP `memstead_entity` tool surfaces for a miss.
    #[wasm_bindgen(js_name = getEntity)]
    pub fn get_entity(&self, id: &str) -> Result<JsValue, JsValue> {
        let entity_id = EntityId(id.to_string());
        match self.inner.get_entity(&entity_id) {
            Some(entity) => to_js(entity),
            None => Ok(JsValue::UNDEFINED),
        }
    }

    /// Health summary for the engine — entity counts, edge counts,
    /// per-mem breakdown. Mirrors the shape `memstead_health` returns
    /// in MCP.
    #[wasm_bindgen]
    pub fn health(&self) -> Result<JsValue, JsValue> {
        let summary = self.inner.health();
        to_js(&summary)
    }

    /// Full-text search — unavailable in the WASM build. Always
    /// throws `{ code: "SEARCH_UNAVAILABLE_IN_WASM", message }`.
    /// Browser callers route search queries to the bridge's
    /// `memstead_search` endpoint instead.
    ///
    /// The method stays present (rather than absent) so JS call sites
    /// don't need cfg-style branching at the import layer — they call
    /// it, catch the typed code, and route. Same discipline the
    /// native engine uses today on `wasm32` targets at the Rust API
    /// surface.
    #[wasm_bindgen]
    pub fn search(&self, _scope: JsValue) -> Result<JsValue, JsValue> {
        // The underlying `Engine::search` returns
        // `Err(EngineError::SearchUnavailable)` on `wasm32`; we
        // construct a dummy `SearchScope` just to walk through that
        // path and surface the typed code, in case future
        // implementations grow scope-validation before the refuse.
        let scope = SearchScope::default();
        Err(engine_err(
            self.inner
                .search(&scope)
                .err()
                .unwrap_or(EngineError::SearchUnavailable),
        ))
    }

    /// Mem names this engine is mounted against. Cheap accessor —
    /// useful for diagnostic UIs and for routing follow-up reads
    /// without re-deriving the mem list from health output.
    #[wasm_bindgen(js_name = memNames)]
    pub fn mem_names(&self) -> Result<JsValue, JsValue> {
        let names: Vec<String> = self
            .inner
            .mem_names()
            .iter()
            .map(|s| s.to_string())
            .collect();
        serde_wasm_bindgen::to_value(&names)
            .map_err(|e| err_object("SERIALIZATION_FAILED", &e.to_string()))
    }
}

/// `{ code, message }` envelope thrown to JS. Stable string keys so
/// JS callers can branch on `error.code` without parsing the message.
#[derive(Serialize)]
struct ErrEnvelope<'a> {
    code: &'a str,
    message: String,
}

fn err_object(code: &str, message: &str) -> JsValue {
    let env = ErrEnvelope {
        code,
        message: message.to_string(),
    };
    serde_wasm_bindgen::to_value(&env).unwrap_or_else(|_| JsValue::from_str(message))
}

fn engine_err(e: EngineError) -> JsValue {
    err_object(e.code(), &e.to_string())
}

fn from_archive_bytes_err(e: FromArchiveBytesError) -> JsValue {
    let code = match &e {
        FromArchiveBytesError::Validation(_) => "ARCHIVE_VALIDATION_FAILED",
        FromArchiveBytesError::InvalidConfig(_) => "INVALID_MEM_CONFIG",
        FromArchiveBytesError::EmbeddedSchemaInvalid(_) => "EMBEDDED_SCHEMA_INVALID",
        FromArchiveBytesError::Engine(inner) => inner.code(),
    };
    err_object(code, &e.to_string())
}
