// Public surface of `@memstead/client`.
//
// Consumers typically only need `MemSyncClient` + `MemSyncError`.
// The `types` re-exports are there for TypeScript users who want to
// strongly-type their own state around the wire shapes.

export { MemSyncClient } from "./client.js";
export { MemSyncError, BRIDGE_CODES, CLIENT_CODES } from "./errors.js";
export type {
  AuthFn,
  CommitEnvelope,
  Entity,
  EntityChange,
  EventSourceLike,
  HealthReport,
  SearchHit,
  SearchQuery,
  SearchResult,
  MemChangedEvent,
  MemSyncClientError,
  MemSyncClientOptions,
  WasmEngineFactory,
  WasmEngineLike,
} from "./types.js";
