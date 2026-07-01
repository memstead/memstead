// `MemSyncClient` — orchestrates the snapshot + SSE + commit-apply
// lifecycle for a single mem.
//
// Lifecycle:
//
//   1. open()  → fetch /snapshot, hydrate engine, subscribe to /events
//   2. event   → fetch /commits?since=<localHead>&until=<eventHead>,
//                apply each envelope, advance cursor, fire onUpdate
//   3. close() → close SSE, drop engine, abort outstanding fetches
//
// Recovery paths:
//
//   - SSE reconnect: re-fetch /head, replay the gap before resuming
//   - 404 UNKNOWN_COMMIT on /commits: full snapshot reload + cursor reset
//   - 409 DELTA_TOO_LARGE: same — snapshot reload
//
// Read paths route to the local WASM engine; search routes to the
// bridge's /search endpoint (the WASM engine refuses search with
// SEARCH_UNAVAILABLE_IN_WASM per plan 07).

import { BRIDGE_CODES, CLIENT_CODES, MemSyncError, errorFromResponse } from "./errors.js";
import type {
  CommitEnvelope,
  Entity,
  EventSourceLike,
  HealthReport,
  SearchQuery,
  SearchResult,
  MemChangedEvent,
  MemSyncClientOptions,
  WasmEngineLike,
} from "./types.js";

export class MemSyncClient {
  readonly #options: MemSyncClientOptions;
  readonly #fetch: typeof fetch;
  readonly #eventSourceFactory: (url: string) => EventSourceLike;
  readonly #abort = new AbortController();
  #engine: WasmEngineLike | null = null;
  #eventSource: EventSourceLike | null = null;
  #head: string = "";
  #closed = false;
  /** When `true`, an SSE reconnect happened since the last event; the
   * next `mem_changed` (or the periodic reconnect-check) re-fetches
   * `/head` before applying anything. EventSource fires a synthetic
   * `error` event on reconnect attempts; we treat that as the signal. */
  #needsHeadResync = false;
  /** Concurrency guard around the apply-commits flow. Two SSE events
   * arriving close together must not race a snapshot-reload mid-apply. */
  #applyInFlight: Promise<void> | null = null;

  constructor(options: MemSyncClientOptions) {
    this.#options = options;
    this.#fetch = options.fetch ?? globalThis.fetch.bind(globalThis);
    this.#eventSourceFactory =
      options.eventSourceFactory ??
      ((url: string) => new (globalThis as { EventSource: new (u: string) => EventSourceLike }).EventSource(url));
  }

  // ---- public surface ---------------------------------------------------

  /** Hydrate the engine from `/snapshot` and subscribe to `/events`.
   * Resolves once the snapshot is loaded and the SSE subscription is
   * established. Subsequent commit-applies happen in the background;
   * consumers receive them via the `onUpdate` callback. */
  async open(): Promise<void> {
    if (this.#closed) {
      throw new MemSyncError(CLIENT_CODES.CLIENT_CLOSED, "client is closed");
    }
    if (this.#engine !== null) return;
    await this.#loadSnapshot();
    this.#openEventSource();
  }

  /** Close the SSE subscription, drop the WASM engine, abort any in-
   * flight HTTP request. Synchronous + best-effort — callers do not
   * need to await cleanup. */
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    if (this.#eventSource !== null) {
      try {
        this.#eventSource.close();
      } catch {
        // Best-effort — never throw out of close().
      }
      this.#eventSource = null;
    }
    this.#abort.abort();
    this.#engine = null;
  }

  /** Current HEAD SHA of the in-memory replica. Empty string before
   * `open()` completes. */
  get head(): string {
    return this.#head;
  }

  /** `true` between `open()` and `close()`. */
  get isOpen(): boolean {
    return !this.#closed && this.#engine !== null;
  }

  /** Read one entity by id. Returns `null` when the id is not in the
   * store. */
  getEntity(id: string): Entity | null {
    const engine = this.#requireEngine();
    const result = engine.getEntity(id);
    return result === undefined ? null : result;
  }

  /** Health summary — entity / edge counts, per-mem breakdown. */
  health(): HealthReport {
    return this.#requireEngine().health();
  }

  /** Full-text search. Routes to the bridge's `/search` endpoint —
   * the WASM engine refuses search with `SEARCH_UNAVAILABLE_IN_WASM`. */
  async search(query: SearchQuery): Promise<SearchResult> {
    if (this.#closed) {
      throw new MemSyncError(CLIENT_CODES.CLIENT_CLOSED, "client is closed");
    }
    const params = new URLSearchParams();
    params.set("q", query.q);
    if (query.type !== undefined) params.set("type", query.type);
    if (query.limit !== undefined) params.set("limit", String(query.limit));
    if (query.offset !== undefined) params.set("offset", String(query.offset));
    const url = `${this.#endpointFor("search")}?${params.toString()}`;
    const response = await this.#authedFetch(url, { method: "GET" });
    if (!response.ok) {
      throw await errorFromResponse(response, "search");
    }
    return (await response.json()) as SearchResult;
  }

  // ---- snapshot / SSE / commit apply -----------------------------------

  async #loadSnapshot(): Promise<void> {
    const url = this.#endpointFor("snapshot");
    const response = await this.#authedFetch(url, { method: "GET" });
    if (!response.ok) {
      throw await errorFromResponse(response, "snapshot");
    }
    const head = response.headers.get("x-memstead-head") ?? "";
    const bytes = new Uint8Array(await response.arrayBuffer());
    this.#engine = this.#options.engineFactory(bytes);
    this.#head = head;
    this.#options.onUpdate?.();
  }

  #openEventSource(): void {
    const url = this.#endpointFor("events");
    const es = this.#eventSourceFactory(url);
    es.addEventListener("mem_changed", (event: MessageEvent | Event) => {
      void this.#handleMemChanged(event as MessageEvent);
    });
    es.addEventListener("error", () => {
      // EventSource fires `error` on every reconnect attempt. Mark the
      // resync flag so the next `mem_changed` re-fetches `/head`
      // before applying — the gap may have grown during the disconnect
      // and EventSource buffers nothing.
      this.#needsHeadResync = true;
    });
    this.#eventSource = es;
  }

  async #handleMemChanged(event: MessageEvent): Promise<void> {
    // Serialise concurrent events so two close-together updates don't
    // race a snapshot reload mid-apply.
    const previous = this.#applyInFlight ?? Promise.resolve();
    const run = previous.then(async () => {
      try {
        // After a reconnect, fetch /head before trusting the event —
        // EventSource doesn't buffer events across disconnects.
        if (this.#needsHeadResync) {
          this.#needsHeadResync = false;
          await this.#applyGapToServerHead();
          return;
        }
        const parsed = this.#parseEvent(event);
        if (parsed === null) return;
        await this.#applyRange(this.#head, parsed.head);
      } catch (err) {
        this.#options.onError?.(this.#wrapError(err));
      }
    });
    this.#applyInFlight = run;
    await run;
  }

  #parseEvent(event: MessageEvent): MemChangedEvent | null {
    if (typeof event.data !== "string" || event.data.length === 0) return null;
    try {
      return JSON.parse(event.data) as MemChangedEvent;
    } catch {
      return null;
    }
  }

  async #applyGapToServerHead(): Promise<void> {
    const headUrl = this.#endpointFor("head");
    const response = await this.#authedFetch(headUrl, { method: "GET" });
    if (!response.ok) {
      throw await errorFromResponse(response, "head");
    }
    const remote = (await response.text()).trim();
    if (remote === this.#head || remote === "") return;
    await this.#applyRange(this.#head, remote);
  }

  async #applyRange(since: string, until: string): Promise<void> {
    if (since === until) return;
    const params = new URLSearchParams();
    if (since !== "") params.set("since", since);
    if (until !== "") params.set("until", until);
    const url = `${this.#endpointFor("commits")}?${params.toString()}`;
    const response = await this.#authedFetch(url, { method: "GET" });
    if (!response.ok) {
      const err = await errorFromResponse(response, "commits");
      // Force-push or rebase that invalidated our cursor — and the
      // delta-too-large guard the bridge throws when the range is
      // unreasonable. Both recover via a full snapshot reload.
      if (
        err.code === BRIDGE_CODES.UNKNOWN_COMMIT ||
        err.code === BRIDGE_CODES.DELTA_TOO_LARGE
      ) {
        await this.#reloadAfterDivergence(err);
        return;
      }
      throw err;
    }
    const envelopes = (await response.json()) as CommitEnvelope[];
    if (envelopes.length === 0) return;
    const engine = this.#requireEngine();
    for (const env of envelopes) {
      engine.applyCommit(env);
      this.#head = env.sha;
    }
    this.#options.onUpdate?.();
  }

  async #reloadAfterDivergence(cause: MemSyncError): Promise<void> {
    // Surface the recovery to callers — they may want to log a metric
    // or warn the user about lost-history scenarios.
    this.#options.onError?.(cause);
    this.#engine = null;
    this.#head = "";
    await this.#loadSnapshot();
  }

  // ---- helpers ----------------------------------------------------------

  #requireEngine(): WasmEngineLike {
    if (this.#closed) {
      throw new MemSyncError(CLIENT_CODES.CLIENT_CLOSED, "client is closed");
    }
    if (this.#engine === null) {
      throw new MemSyncError(
        CLIENT_CODES.NOT_OPEN,
        "client.open() has not completed — call await client.open() first",
      );
    }
    return this.#engine;
  }

  #endpointFor(name: "snapshot" | "head" | "commits" | "events" | "search"): string {
    const base = this.#options.baseUrl.replace(/\/$/, "");
    const mem = encodeURIComponent(this.#options.mem);
    return `${base}/mems/${mem}/${name}`;
  }

  async #authedFetch(url: string, init: RequestInit): Promise<Response> {
    const headers = new Headers(init.headers);
    if (this.#options.auth) {
      await this.#options.auth(headers);
    }
    const merged: RequestInit = {
      ...this.#options.fetchOptions,
      ...init,
      headers,
      signal: this.#abort.signal,
    };
    try {
      return await this.#fetch(url, merged);
    } catch (cause) {
      if ((cause as { name?: string }).name === "AbortError") {
        throw new MemSyncError(CLIENT_CODES.CLIENT_CLOSED, "client aborted in-flight request", {
          cause,
        });
      }
      throw new MemSyncError(CLIENT_CODES.NETWORK, `network failure: ${String(cause)}`, {
        cause,
      });
    }
  }

  #wrapError(err: unknown): MemSyncError {
    if (err instanceof MemSyncError) return err;
    return new MemSyncError(
      CLIENT_CODES.UNEXPECTED_RESPONSE,
      err instanceof Error ? err.message : String(err),
      { cause: err },
    );
  }
}
