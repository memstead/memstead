# @memstead/client

Browser thin-client for Memstead mems — orchestrates the snapshot +
SSE + commit-apply lifecycle on top of `@memstead/wasm` and the
`memstead-bridge` HTTP+SSE surface as a single `MemSyncClient` class.

## Install

```bash
npm install @memstead/client @memstead/wasm
# or
pnpm add @memstead/client @memstead/wasm
```

> `@memstead/wasm` is a peer dependency — install it alongside.

## Usage

```ts
import init, { Engine } from "@memstead/wasm";
import { MemSyncClient } from "@memstead/client";

await init();

const client = new MemSyncClient({
  baseUrl: "https://example.com/api",
  mem: "specs",
  engineFactory: (bytes) => Engine.fromSnapshot(bytes),
  onUpdate: () => rerender(client),
});

await client.open();

const entity = client.getEntity("specs--alpha");
const result = await client.search({ q: "knowledge graph", limit: 20 });

// ...later
client.close();
```

## What it does

- **`open()`** hydrates the engine from `GET /mems/<v>/snapshot`
  and subscribes to `GET /mems/<v>/events`.
- Every incoming `mem_changed` event triggers
  `GET /mems/<v>/commits?since=<localHead>&until=<eventHead>` and
  applies the returned envelopes to the local WASM engine.
- On SSE reconnect (transparent EventSource behaviour), the next
  event re-fetches `/head` so the gap can be filled before resuming.
- `404 UNKNOWN_COMMIT` or `409 DELTA_TOO_LARGE` on `/commits` →
  full snapshot reload + cursor reset (force-push / rebase recovery).
- **`search()`** routes to `GET /mems/<v>/search` — the local WASM
  engine refuses search with `SEARCH_UNAVAILABLE_IN_WASM`.
- **`getEntity()` / `health()`** route to the local engine — no
  round-trip.
- **`close()`** drops the SSE subscription, aborts any pending fetch,
  releases the WASM engine.

## Errors

Every refusal surfaces as a `MemSyncError` with a stable `code`:

| code                     | source                                          |
|--------------------------|-------------------------------------------------|
| `UNKNOWN_MEM`          | bridge — mem not in the embedder's allowlist  |
| `UNKNOWN_COMMIT`         | bridge — force-push or rebase ate the cursor    |
| `DELTA_TOO_LARGE`        | bridge — too many commits in the requested range|
| `INVALID_SEARCH_QUERY`   | bridge — empty `q` or out-of-range `limit`      |
| `ENGINE_ERROR`           | bridge — internal `memstead-base` failure           |
| `GIT_ERROR`              | bridge — git operation against the mem-repo failed|
| `CLIENT_CLOSED`          | client — `close()` was called                   |
| `NOT_OPEN`               | client — read before `open()` resolved          |
| `NETWORK`                | client — `fetch` rejected                       |
| `UNEXPECTED_RESPONSE`    | client — non-JSON body where one was expected   |

Branch on `error.code` — never on `error.message`.

## Build / test

```bash
npm install
npm run build       # writes to dist/
npm test            # vitest unit suite
npm run typecheck   # tsc --noEmit
```

## Demo

`examples/index.html` boots a minimal browser demo against a
running `memstead-bridge` embedder. Reproduce:

```bash
# 1. Build the WASM bundle
cd ../memstead-wasm
wasm-pack build --target web --release

# 2. Build this package
cd ../client-js
npm install
npm run build

# 3. Run an memstead-bridge embedder on localhost:8000 (separate process)

# 4. Serve the static files
cd ../../../..
python3 -m http.server 8000
# open http://localhost:8000/engine/crates/memstead-wasm/client-js/examples/index.html
```

## Out of scope (v1)

- Snapshot caching (OPFS, IndexedDB) — every `open()` re-fetches.
- Multi-tab coordination via `BroadcastChannel`.
- Mutations — this is the read-side library.
