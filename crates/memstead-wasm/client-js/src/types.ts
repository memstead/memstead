// Wire-format types — mirror the Rust crates `memstead-bridge::wire` and the
// `memstead-wasm` JS bindings. Field names match the on-wire JSON exactly;
// renaming any of these is a wire-format break.

/** One per-entity change inside a `CommitEnvelope`. Tagged via the
 * `op` discriminator — matches the engine's `#[serde(tag = "op",
 * rename_all = "lowercase")]` derive. */
export type EntityChange =
  | { op: "added"; path: string; content: string }
  | { op: "modified"; path: string; content: string }
  | { op: "deleted"; path: string }
  | { op: "renamed"; from: string; to: string; content: string };

/** One commit's wire envelope — `application/json` shape `/commits`
 * returns one of these per matching commit. */
export interface CommitEnvelope {
  sha: string;
  /** Empty string for the first commit of a branch (no parent). */
  parent?: string;
  vault: string;
  /** RFC 3339 / ISO 8601, UTC, second-precision. */
  timestamp: string;
  trailers?: Record<string, string>;
  changes: EntityChange[];
}

/** SSE `vault_changed` event payload. Pushed when the named vault's
 * HEAD advances. */
export interface VaultChangedEvent {
  vault: string;
  head: string;
  previous: string;
  n_commits: number;
}

/** `/search` request — sent as URL query params. */
export interface SearchQuery {
  /** Required text predicate. Whitespace-separated tokens become
   * `any`-array terms on the engine's structured query. */
  q: string;
  /** Optional entity-type filter (`spec`, `memo`, …). */
  type?: string;
  /** Optional page size; falls back to the server-configured
   * default. */
  limit?: number;
  /** 0-based pagination offset. */
  offset?: number;
}

/** Per-hit shape returned by `/search`. JSON-byte-identical to the
 * engine's own `SearchHit` (plan 08 AC F). */
export interface SearchHit {
  id: string;
  title: string;
  vault: string;
  entity_type: string;
  stub: boolean;
  score: number;
  tokens: number;
  snippet?: string;
  sections: Record<string, string>;
  // Heavyweight optional fields pass through as opaque JSON.
  score_breakdown?: unknown;
  matched_terms?: unknown;
  expansion?: unknown;
}

/** `/search` response envelope. */
export interface SearchResult {
  vault: string;
  query: string;
  hits: SearchHit[];
  total_matched: number;
  truncated: boolean;
  warnings?: string[];
}

/** Entity shape returned by `engine.getEntity`. Field set matches
 * what `memstead_base::Entity` serializes — kept open-ended (Record-
 * typed metadata + sections) so a new schema field surfaces without
 * forcing a `@memstead/client` release. */
export interface Entity {
  id: string;
  title: string;
  vault: string;
  entity_type: string;
  stub: boolean;
  content_hash: string;
  file_path?: string;
  sections: Record<string, string>;
  metadata: Record<string, unknown>;
  relationships: Array<{
    rel_type: string;
    target: string;
    description?: string;
  }>;
}

/** Health summary returned by `engine.health()`. */
export interface HealthReport {
  total_entities: number;
  total_edges: number;
  vaults: Record<string, unknown>;
  // Open-ended — the engine's HealthSummary is a closed Rust struct
  // but new fields land additively and we don't want a `@memstead/client`
  // bump per field. Consumers branch on the keys they care about.
  [key: string]: unknown;
}

/** Minimal interface for the WASM engine the client wraps. Mirrors
 * the public surface `memstead-wasm` exposes via wasm-bindgen — keeping
 * this as an interface (not a class import) lets tests inject a
 * pure-JS mock without spinning up the actual WASM runtime. */
export interface WasmEngineLike {
  applyCommit(envelope: CommitEnvelope): void;
  getEntity(id: string): Entity | undefined;
  health(): HealthReport;
  /** Optional — only present on engine builds with `vaultNames`. */
  vaultNames?(): string[];
}

/** Factory the client uses to hydrate an engine from snapshot bytes.
 * Injected at construction so tests can swap in a JS-only stub. The
 * production binding `@memstead/wasm`'s `Engine.fromSnapshot` matches
 * this shape one-to-one. */
export type WasmEngineFactory = (bytes: Uint8Array) => WasmEngineLike;

/** Optional auth hook — runs once per `fetch` call so the embedder
 * can inject `Authorization: Bearer ...`, CSRF tokens, etc. Mutates
 * the `headers` object in place (axum's `Query` extractor ignores
 * unknown headers, so anything the embedder adds rides through). */
export type AuthFn = (headers: Headers) => void | Promise<void>;

/** Constructor options for [`VaultSyncClient`]. */
export interface VaultSyncClientOptions {
  /** Base URL for the bridge — path prefix the embedder mounted the
   * handlers under. Trailing slashes are tolerated. The final URL
   * pattern is `${baseUrl}/vaults/${vault}/<endpoint>`. */
  baseUrl: string;
  /** Vault name to track. Must match one the bridge mounted. */
  vault: string;
  /** Factory producing a `WasmEngineLike` from snapshot bytes.
   * Typically `(bytes) => Engine.fromSnapshot(bytes)` from
   * `@memstead/wasm`. */
  engineFactory: WasmEngineFactory;
  /** Re-render trigger fired after every successful state advance
   * (snapshot hydrate, commit apply, force-push refresh). Synchronous
   * — keep the body cheap; do heavy work in the next animation frame. */
  onUpdate?: () => void;
  /** Failure callback. Fired for errors the client recovers from
   * autonomously (force-push refresh, SSE reconnect drift) and for
   * fatal errors that close the client. */
  onError?: (err: VaultSyncClientError) => void;
  /** Auth-header injector — runs per outgoing HTTP request. */
  auth?: AuthFn;
  /** Per-request `fetch` options merged into every outgoing call
   * (`credentials`, custom signal upstream of the internal abort,
   * etc.). The client's own `AbortController.signal` always
   * overrides `signal` here. */
  fetchOptions?: RequestInit;
  /** Override the global `EventSource` constructor. Tests pass a
   * mock implementation; production callers can omit this. */
  eventSourceFactory?: (url: string) => EventSourceLike;
  /** Override the global `fetch`. Tests pass a stub; production
   * leaves this `undefined`. */
  fetch?: typeof fetch;
}

/** Minimal interface the client uses against an `EventSource` — kept
 * narrow so tests can inject an in-process emitter that drives
 * `vault_changed` events synchronously. */
export interface EventSourceLike {
  readonly readyState: number;
  addEventListener(
    type: string,
    listener: (event: MessageEvent | Event) => void,
  ): void;
  close(): void;
}

/** Stable error shape thrown by the client. `code` is
 * UPPER_SNAKE_CASE and pattern-matches against the bridge's error
 * envelope `code` plus client-internal sentinel codes (see
 * `errors.ts`). */
export interface VaultSyncClientError extends Error {
  readonly code: string;
  readonly status?: number;
  readonly details?: unknown;
}
