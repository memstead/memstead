// Typed error envelope. `code` mirrors the bridge's `BridgeError`
// codes (`UNKNOWN_VAULT`, `UNKNOWN_COMMIT`, `DELTA_TOO_LARGE`,
// `INVALID_SEARCH_QUERY`, `ENGINE_ERROR`, `GIT_ERROR`) plus client-
// internal sentinels (`CLIENT_CLOSED`, `NOT_OPEN`, `NETWORK`,
// `UNEXPECTED_RESPONSE`).

import type { VaultSyncClientError } from "./types.js";

/** Stable bridge-side codes the client recognises and routes on. */
export const BRIDGE_CODES = {
  UNKNOWN_VAULT: "UNKNOWN_VAULT",
  UNKNOWN_COMMIT: "UNKNOWN_COMMIT",
  DELTA_TOO_LARGE: "DELTA_TOO_LARGE",
  INVALID_SEARCH_QUERY: "INVALID_SEARCH_QUERY",
  ENGINE_ERROR: "ENGINE_ERROR",
  GIT_ERROR: "GIT_ERROR",
} as const;

/** Client-internal codes — surface from the client itself, never
 * the bridge. */
export const CLIENT_CODES = {
  CLIENT_CLOSED: "CLIENT_CLOSED",
  NOT_OPEN: "NOT_OPEN",
  NETWORK: "NETWORK",
  UNEXPECTED_RESPONSE: "UNEXPECTED_RESPONSE",
} as const;

/** Concrete `Error` subclass — `instanceof VaultSyncError` is the
 * recommended branch for consumers; `error.code` is the agent-
 * actionable token. */
export class VaultSyncError extends Error implements VaultSyncClientError {
  override readonly name = "VaultSyncError";
  readonly code: string;
  readonly status?: number;
  readonly details?: unknown;

  constructor(
    code: string,
    message: string,
    opts: { status?: number; details?: unknown; cause?: unknown } = {},
  ) {
    super(message, opts.cause === undefined ? undefined : { cause: opts.cause });
    this.code = code;
    if (opts.status !== undefined) this.status = opts.status;
    if (opts.details !== undefined) this.details = opts.details;
  }
}

/** JSON envelope shape the bridge emits on refusal — mirrors
 * `memstead_bridge::error::ErrorEnvelope`. */
interface BridgeErrorEnvelope {
  code?: string;
  message?: string;
  details?: unknown;
}

/** Turn a non-OK `Response` into a `VaultSyncError`. Tries to parse
 * the body as a typed envelope; falls back to a generic
 * `UNEXPECTED_RESPONSE` when the body isn't JSON or omits `code`. */
export async function errorFromResponse(
  response: Response,
  fallbackContext: string,
): Promise<VaultSyncError> {
  let envelope: BridgeErrorEnvelope | undefined;
  try {
    envelope = (await response.json()) as BridgeErrorEnvelope;
  } catch {
    // Body isn't JSON — fall through to fallback code.
  }
  const code = envelope?.code ?? CLIENT_CODES.UNEXPECTED_RESPONSE;
  const message =
    envelope?.message ?? `${fallbackContext}: HTTP ${response.status}`;
  return new VaultSyncError(code, message, {
    status: response.status,
    details: envelope?.details,
  });
}
