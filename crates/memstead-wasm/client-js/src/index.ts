// Public surface of `@memstead/client`.
//
// Consumers typically only need `VaultSyncClient` + `VaultSyncError`.
// The `types` re-exports are there for TypeScript users who want to
// strongly-type their own state around the wire shapes.

export { VaultSyncClient } from "./client.js";
export { VaultSyncError, BRIDGE_CODES, CLIENT_CODES } from "./errors.js";
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
  VaultChangedEvent,
  VaultSyncClientError,
  VaultSyncClientOptions,
  WasmEngineFactory,
  WasmEngineLike,
} from "./types.js";
