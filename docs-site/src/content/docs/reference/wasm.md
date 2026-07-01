---
title: "WASM surface"
---

# WASM surface

Auto-generated from `engine/crates/memstead-wasm/src/lib.rs`. Every entry point annotated with `#[wasm_bindgen]` is listed below with the JS-visible name, the underlying Rust signature, and the doc comment captured from the source.

The WASM surface is the **read-side** of the browser-sync architecture — writes happen server-side and flow back through `applyCommit`. Full-text search is intentionally unavailable in the WASM build; the method exists as a typed-refusal stub so JS call sites can branch on the stable error code (`SEARCH_UNAVAILABLE_IN_WASM`) instead of cfg-style imports.

## Free functions

### `setPanicHook` *(feature: `panic-hook`)*

*Underlying Rust function: `set_panic_hook`*

```rust
pub fn set_panic_hook()
```

Install [`console_error_panic_hook`] so panics from inside the
wasm runtime surface as readable JS stack traces. Idempotent —
safe to call multiple times. Costs ~3 KB; embedders that care
about the bare minimum bundle can omit the call (or disable the
`panic-hook` feature at build time).

## `Engine` class

The `Engine` class owns the in-memory store. One instance per `.mem` snapshot the client hydrates.

### `Engine.fromSnapshot`

*Underlying Rust function: `from_snapshot`*

```rust
pub fn from_snapshot(bytes: Vec<u8>) -> Result<Engine, JsValue>
```

Hydrate an engine from a `.mem` snapshot. Accepts the same
byte range any `application/zip` response body would carry
(the bridge's `/snapshot` endpoint, an in-page `fetch`, a
pre-bundled asset). Returns the engine handle on success;
throws a `{ code, message }` envelope on validation /
configuration failures.

### `Engine.applyCommit`

*Underlying Rust function: `apply_commit`*

```rust
pub fn apply_commit(&mut self, envelope: JsValue) -> Result<(), JsValue>
```

Apply an externally-produced commit envelope to the in-memory
store. Parsed against the vault's pinned schema; on any parse
failure the entire envelope is refused and the store stays at
its prior SHA. Emits the same `VaultChangedEvent` every other
vault-advance flows through.

The `envelope` parameter accepts the JSON shape the bridge's
`/commits` endpoint produces — `serde-wasm-bindgen` decodes
the JS object into a `CommitEnvelope`. Decode failures throw a
`{ code: "INVALID_INPUT", message }` envelope so JS callers
can branch on the same code MCP would surface.

### `Engine.getEntity`

*Underlying Rust function: `get_entity`*

```rust
pub fn get_entity(&self, id: &str) -> Result<JsValue, JsValue>
```

Read one entity by id (`<vault>--<slug>` shape). Returns
`undefined` when the id is not in the store — same shape the
MCP `memstead_entity` tool surfaces for a miss.

### `Engine.health`

```rust
pub fn health(&self) -> Result<JsValue, JsValue>
```

Health summary for the engine — entity counts, edge counts,
per-vault breakdown. Mirrors the shape `memstead_health` returns
in MCP.

### `Engine.search`

```rust
pub fn search(&self, _scope: JsValue) -> Result<JsValue, JsValue>
```

Full-text search — unavailable in the WASM build. Always
throws `{ code: "SEARCH_UNAVAILABLE_IN_WASM", message }`.
Browser callers route search queries to the bridge's
`memstead_search` endpoint instead.

The method stays present (rather than absent) so JS call sites
don't need cfg-style branching at the import layer — they call
it, catch the typed code, and route. Same discipline the
native engine uses today on `wasm32` targets at the Rust API
surface.

### `Engine.vaultNames`

*Underlying Rust function: `vault_names`*

```rust
pub fn vault_names(&self) -> Result<JsValue, JsValue>
```

Vault names this engine is mounted against. Cheap accessor —
useful for diagnostic UIs and for routing follow-up reads
without re-deriving the vault list from health output.

