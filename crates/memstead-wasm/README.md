# memstead-wasm

WebAssembly bindings for the
[Memstead](https://github.com/memstead/memstead) engine — hydrate a
knowledge-graph snapshot in the browser and read it with the same typed
engine that runs natively.

The bundle is built from this crate with wasm-bindgen (`--target web`):
instantiate the module, load a `.mem` snapshot, then read entities,
relationships, and graph structure client-side — no server round-trips
after the snapshot fetch.

## Build

The crate is not published as a package: the site that runs the engine
in the browser (memstead.io) builds the bundle from the engine tree it
was built from, so the bundle and the archives it reads always share one
version. Build it the same way yourself:

```bash
cd crates/memstead-wasm
wasm-pack build --target web --release   # output in pkg/
```

(A version of `@memstead/wasm` was published to npm until 0.18.1; that
channel is closed, and the name stays reserved.)

## Compatibility

**The bundle is version-matched to the engine.** A bundle built from a
given engine tree reads the archives that tree's CLI writes — check yours
with `memstead --version`. If the two numbers match, the archive and the
reader agree.

## Use

```js
import init, { Engine, setPanicHook } from "./pkg/memstead_wasm.js";

await init();
setPanicHook(); // readable stack traces instead of "unreachable executed"

const bytes = new Uint8Array(await (await fetch("/my-graph.mem")).arrayBuffer());
const engine = Engine.fromSnapshot(bytes);

// What is in this snapshot? `entityIds()` is how you find out — the
// archive is self-describing, so you never need a separate id list.
const ids = engine.entityIds();          // sorted, every mem
const scoped = engine.entityIds("notes"); // one mem; unknown mem -> []

for (const id of ids) {
  const entity = engine.getEntity(id);   // undefined if absent
  // `entity_type` — one spelling on every wire surface (MCP's entity
  // envelope aligned in the 2026-08 wire batch; the retired `type` key
  // is gone, not aliased). The generated `.d.ts` is authoritative.
  console.log(entity.entity_type, entity.title, Object.keys(entity.sections));
}

engine.memNames();  // the mems this snapshot carries
engine.health();    // entity/edge counts, per-mem breakdown
```

Full-text search is deliberately unavailable in the browser build:
`engine.search(...)` throws `{ code: "SEARCH_UNAVAILABLE_IN_WASM" }` so a
call site can catch the code and route the query elsewhere rather than
branching at the import layer.

Type definitions (`.d.ts`) are emitted beside the bundle.

## License

MIT OR Apache-2.0, at your option.
