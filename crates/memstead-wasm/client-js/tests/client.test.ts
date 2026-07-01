// MemSyncClient unit tests.
//
// Drive the client against mocked `fetch` + `EventSource` so the
// suite runs under Node without any browser or Rust toolchain.
// The mocks expose the same surface the real DOM types provide so
// the client code is exercised end-to-end except for the actual
// network + WASM boundaries.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MemSyncClient } from "../src/client.ts";
import { MemSyncError, BRIDGE_CODES, CLIENT_CODES } from "../src/errors.ts";
import type {
  CommitEnvelope,
  Entity,
  EventSourceLike,
  HealthReport,
  SearchResult,
  WasmEngineLike,
} from "../src/types.ts";

/** In-process EventSource stand-in. Tests drive `mem_changed` by
 * calling `emitMessage`; the SSE reconnect path is simulated via
 * `emitError`. */
class FakeEventSource implements EventSourceLike {
  readyState = 1;
  readonly listeners: Map<string, ((event: MessageEvent | Event) => void)[]> = new Map();
  readonly url: string;
  closed = false;

  constructor(url: string) {
    this.url = url;
  }

  addEventListener(type: string, listener: (event: MessageEvent | Event) => void): void {
    const list = this.listeners.get(type) ?? [];
    list.push(listener);
    this.listeners.set(type, list);
  }

  close(): void {
    this.closed = true;
  }

  emitMessage(type: string, data: string): void {
    const event = { data, type } as MessageEvent;
    for (const listener of this.listeners.get(type) ?? []) {
      listener(event);
    }
  }

  emitError(): void {
    for (const listener of this.listeners.get("error") ?? []) {
      listener(new Event("error"));
    }
  }
}

/** JS-only engine stub mirroring the wasm-bindgen surface. */
class FakeEngine implements WasmEngineLike {
  readonly entities: Map<string, Entity> = new Map();
  readonly applied: CommitEnvelope[] = [];

  constructor(seed: Entity[] = []) {
    for (const e of seed) this.entities.set(e.id, e);
  }

  applyCommit(envelope: CommitEnvelope): void {
    this.applied.push(envelope);
    for (const change of envelope.changes) {
      if (change.op === "deleted") {
        this.entities.delete(`${envelope.mem}--${stripMd(change.path)}`);
      } else if (change.op === "renamed") {
        this.entities.delete(`${envelope.mem}--${stripMd(change.from)}`);
        const id = `${envelope.mem}--${stripMd(change.to)}`;
        this.entities.set(id, makeEntity(id, envelope.mem));
      } else {
        const id = `${envelope.mem}--${stripMd(change.path)}`;
        this.entities.set(id, makeEntity(id, envelope.mem));
      }
    }
  }

  getEntity(id: string): Entity | undefined {
    return this.entities.get(id);
  }

  health(): HealthReport {
    return { total_entities: this.entities.size, total_edges: 0, mems: {} };
  }
}

function stripMd(path: string): string {
  return path.replace(/\.md$/, "");
}

function makeEntity(id: string, mem: string): Entity {
  return {
    id,
    title: id.split("--").slice(1).join("--"),
    mem,
    entity_type: "spec",
    stub: false,
    content_hash: "deadbeef",
    sections: {},
    metadata: {},
    relationships: [],
  };
}

/** Routed fetch mock — registers handlers per URL substring; the
 * test asserts which calls landed and in what order. */
class FetchRecorder {
  readonly calls: { url: string; init?: RequestInit }[] = [];
  readonly handlers: Array<(url: string) => Response | Promise<Response> | undefined> = [];

  fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = typeof input === "string" ? input : input.toString();
    this.calls.push({ url, ...(init === undefined ? {} : { init }) });
    for (const handler of this.handlers) {
      const response = await handler(url);
      if (response !== undefined) return response;
    }
    throw new Error(`no fetch handler matched ${url}`);
  };

  on(matcher: (url: string) => boolean, response: () => Response | Promise<Response>): void {
    this.handlers.push(async (url) => (matcher(url) ? response() : undefined));
  }
}

function jsonResponse(body: unknown, init: ResponseInit = {}): Response {
  return new Response(JSON.stringify(body), {
    ...init,
    headers: {
      "content-type": "application/json",
      ...(init.headers ?? {}),
    },
  });
}

function snapshotResponse(headSha: string): Response {
  // A small ArrayBuffer body; FakeEngine doesn't actually parse it.
  const body = new Uint8Array([0x50, 0x4b, 0x05, 0x06]).buffer;
  return new Response(body, {
    status: 200,
    headers: { "content-type": "application/zip", "x-memstead-head": headSha },
  });
}

// Pin a fresh fake EventSource per test so emit / close state doesn't
// leak across cases.
let fakeES: FakeEventSource | null = null;
let recorder: FetchRecorder;
let engine: FakeEngine;

beforeEach(() => {
  recorder = new FetchRecorder();
  engine = new FakeEngine([makeEntity("specs--alpha", "specs")]);
  fakeES = null;
});

afterEach(() => {
  vi.clearAllMocks();
});

function newClient(headSha = "sha-initial"): MemSyncClient {
  recorder.on(
    (url) => url.endsWith("/snapshot"),
    () => snapshotResponse(headSha),
  );
  return new MemSyncClient({
    baseUrl: "https://example.test/api",
    mem: "specs",
    engineFactory: () => engine,
    fetch: recorder.fetch,
    eventSourceFactory: (url) => {
      fakeES = new FakeEventSource(url);
      return fakeES;
    },
  });
}

describe("MemSyncClient.open", () => {
  it("fetches /snapshot, hydrates the engine, and sets the head cursor", async () => {
    const client = newClient("sha-initial");
    await client.open();
    expect(client.head).toBe("sha-initial");
    expect(client.isOpen).toBe(true);
    expect(recorder.calls[0]?.url).toBe("https://example.test/api/mems/specs/snapshot");
    expect(fakeES?.url).toBe("https://example.test/api/mems/specs/events");
  });

  it("fires onUpdate after snapshot hydration", async () => {
    const onUpdate = vi.fn();
    recorder.on((u) => u.endsWith("/snapshot"), () => snapshotResponse("sha-initial"));
    const client = new MemSyncClient({
      baseUrl: "https://example.test/api",
      mem: "specs",
      engineFactory: () => engine,
      onUpdate,
      fetch: recorder.fetch,
      eventSourceFactory: (url) => {
        fakeES = new FakeEventSource(url);
        return fakeES;
      },
    });
    await client.open();
    expect(onUpdate).toHaveBeenCalledTimes(1);
  });

  it("throws a typed MemSyncError when /snapshot refuses with UNKNOWN_MEM", async () => {
    recorder.on(
      (u) => u.endsWith("/snapshot"),
      () =>
        jsonResponse({ code: "UNKNOWN_MEM", message: "unknown mem: specs" }, { status: 404 }),
    );
    const client = new MemSyncClient({
      baseUrl: "https://example.test/api",
      mem: "specs",
      engineFactory: () => engine,
      fetch: recorder.fetch,
      eventSourceFactory: (url) => new FakeEventSource(url),
    });
    await expect(client.open()).rejects.toMatchObject({
      code: BRIDGE_CODES.UNKNOWN_MEM,
      status: 404,
    });
  });
});

describe("MemSyncClient SSE → commit apply", () => {
  it("applies envelopes from /commits when /events emits mem_changed", async () => {
    const onUpdate = vi.fn();
    const envelopes: CommitEnvelope[] = [
      {
        sha: "sha-next",
        parent: "sha-initial",
        mem: "specs",
        timestamp: "2026-05-19T10:00:00Z",
        changes: [{ op: "modified", path: "alpha.md", content: "..." }],
      },
    ];
    recorder.on((u) => u.endsWith("/commits?since=sha-initial&until=sha-next"), () =>
      jsonResponse(envelopes),
    );
    recorder.on((u) => u.endsWith("/snapshot"), () => snapshotResponse("sha-initial"));

    const client = new MemSyncClient({
      baseUrl: "https://example.test/api",
      mem: "specs",
      engineFactory: () => engine,
      onUpdate,
      fetch: recorder.fetch,
      eventSourceFactory: (url) => {
        fakeES = new FakeEventSource(url);
        return fakeES;
      },
    });
    await client.open();
    onUpdate.mockClear();

    fakeES!.emitMessage(
      "mem_changed",
      JSON.stringify({ mem: "specs", head: "sha-next", previous: "sha-initial", n_commits: 1 }),
    );
    // Allow the microtask queue to drain — apply runs async.
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));

    expect(engine.applied).toHaveLength(1);
    expect(engine.applied[0]?.sha).toBe("sha-next");
    expect(client.head).toBe("sha-next");
    expect(onUpdate).toHaveBeenCalledTimes(1);
  });

  it("re-fetches /head after a reconnect signal before applying the next event", async () => {
    recorder.on((u) => u.endsWith("/snapshot"), () => snapshotResponse("sha-initial"));
    // After reconnect we expect /head, then /commits using the value
    // it returns — even though the next mem_changed payload also
    // carries a sha. This pins that EventSource buffer gaps are
    // closed via the authoritative head query.
    recorder.on(
      (u) => u.endsWith("/head"),
      () =>
        new Response("sha-jumped", {
          status: 200,
          headers: { "content-type": "text/plain" },
        }),
    );
    recorder.on(
      (u) => u.endsWith("/commits?since=sha-initial&until=sha-jumped"),
      () =>
        jsonResponse([
          {
            sha: "sha-jumped",
            parent: "sha-initial",
            mem: "specs",
            timestamp: "2026-05-19T10:00:00Z",
            changes: [{ op: "added", path: "beta.md", content: "..." }],
          } satisfies CommitEnvelope,
        ]),
    );

    const client = newClient("sha-initial");
    await client.open();

    fakeES!.emitError(); // signals reconnect
    fakeES!.emitMessage(
      "mem_changed",
      JSON.stringify({ mem: "specs", head: "sha-stale", previous: "x", n_commits: 1 }),
    );
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));

    expect(client.head).toBe("sha-jumped");
    const urls = recorder.calls.map((c) => c.url);
    expect(urls).toContain("https://example.test/api/mems/specs/head");
  });
});

describe("MemSyncClient force-push recovery", () => {
  it("reloads the snapshot when /commits refuses with UNKNOWN_COMMIT", async () => {
    let snapshotHead = "sha-initial";
    recorder.on((u) => u.endsWith("/snapshot"), () => snapshotResponse(snapshotHead));
    recorder.on((u) => u.includes("/commits?"), () =>
      jsonResponse(
        { code: BRIDGE_CODES.UNKNOWN_COMMIT, message: "unknown commit: sha-initial" },
        { status: 404 },
      ),
    );

    const onError = vi.fn();
    const onUpdate = vi.fn();
    const client = new MemSyncClient({
      baseUrl: "https://example.test/api",
      mem: "specs",
      engineFactory: () => engine,
      onUpdate,
      onError,
      fetch: recorder.fetch,
      eventSourceFactory: (url) => {
        fakeES = new FakeEventSource(url);
        return fakeES;
      },
    });
    await client.open();
    onUpdate.mockClear();
    snapshotHead = "sha-after-force-push";

    fakeES!.emitMessage(
      "mem_changed",
      JSON.stringify({ mem: "specs", head: "sha-rebased", previous: "sha-x", n_commits: 0 }),
    );
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));

    expect(client.head).toBe("sha-after-force-push");
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0]?.[0]).toMatchObject({ code: BRIDGE_CODES.UNKNOWN_COMMIT });
    expect(onUpdate).toHaveBeenCalledTimes(1); // re-load fired one update
  });

  it("reloads the snapshot when /commits refuses with DELTA_TOO_LARGE", async () => {
    let snapshotHead = "sha-initial";
    recorder.on((u) => u.endsWith("/snapshot"), () => snapshotResponse(snapshotHead));
    recorder.on((u) => u.includes("/commits?"), () =>
      jsonResponse(
        {
          code: BRIDGE_CODES.DELTA_TOO_LARGE,
          message: "delta too large: 99 > 50",
          details: { n_commits: 99, limit: 50 },
        },
        { status: 409 },
      ),
    );
    const onError = vi.fn();
    const client = new MemSyncClient({
      baseUrl: "https://example.test/api",
      mem: "specs",
      engineFactory: () => engine,
      onError,
      fetch: recorder.fetch,
      eventSourceFactory: (url) => {
        fakeES = new FakeEventSource(url);
        return fakeES;
      },
    });
    await client.open();
    snapshotHead = "sha-resynced";

    fakeES!.emitMessage(
      "mem_changed",
      JSON.stringify({ mem: "specs", head: "sha-far", previous: "sha-i", n_commits: 99 }),
    );
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));

    expect(client.head).toBe("sha-resynced");
    expect(onError.mock.calls[0]?.[0]).toMatchObject({
      code: BRIDGE_CODES.DELTA_TOO_LARGE,
      status: 409,
    });
  });
});

describe("MemSyncClient.search", () => {
  it("issues a GET /search with URL query params and parses the result", async () => {
    const expected: SearchResult = {
      mem: "specs",
      query: "alpha",
      hits: [
        {
          id: "specs--alpha",
          title: "Alpha",
          mem: "specs",
          entity_type: "spec",
          stub: false,
          score: 1.2,
          tokens: 30,
          sections: {},
        },
      ],
      total_matched: 1,
      truncated: false,
    };
    recorder.on((u) => u.includes("/search?"), () => jsonResponse(expected));

    const client = newClient();
    await client.open();
    const got = await client.search({ q: "alpha", limit: 10 });

    expect(got).toEqual(expected);
    const searchCall = recorder.calls.find((c) => c.url.includes("/search?"));
    expect(searchCall?.url).toBe("https://example.test/api/mems/specs/search?q=alpha&limit=10");
  });

  it("translates 400 INVALID_SEARCH_QUERY into a typed MemSyncError", async () => {
    recorder.on((u) => u.includes("/search?"), () =>
      jsonResponse(
        {
          code: BRIDGE_CODES.INVALID_SEARCH_QUERY,
          message: "`q` is required and must contain at least one non-whitespace character",
          details: { reason: "empty q" },
        },
        { status: 400 },
      ),
    );
    const client = newClient();
    await client.open();
    const err = await client.search({ q: "" }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(MemSyncError);
    expect((err as MemSyncError).code).toBe(BRIDGE_CODES.INVALID_SEARCH_QUERY);
    expect((err as MemSyncError).status).toBe(400);
  });
});

describe("MemSyncClient.close", () => {
  it("closes the SSE subscription and refuses subsequent reads", async () => {
    const client = newClient();
    await client.open();
    expect(fakeES!.closed).toBe(false);

    client.close();

    expect(fakeES!.closed).toBe(true);
    expect(client.isOpen).toBe(false);
    expect(() => client.health()).toThrowError(/closed/);
    await expect(client.search({ q: "anything" })).rejects.toMatchObject({
      code: CLIENT_CODES.CLIENT_CLOSED,
    });
  });
});

describe("MemSyncClient.getEntity", () => {
  it("returns the entity from the local engine without an HTTP round-trip", async () => {
    const client = newClient();
    await client.open();
    const callsBefore = recorder.calls.length;

    const entity = client.getEntity("specs--alpha");

    expect(entity?.id).toBe("specs--alpha");
    // No additional HTTP traffic.
    expect(recorder.calls.length).toBe(callsBefore);
  });

  it("returns null on a miss instead of throwing", async () => {
    const client = newClient();
    await client.open();
    expect(client.getEntity("specs--missing")).toBeNull();
  });
});
