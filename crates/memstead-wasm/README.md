# @memstead/wasm

WebAssembly bindings for the
[Memstead](https://github.com/memstead/memstead) engine — hydrate a
knowledge-graph snapshot in the browser and read it with the same typed
engine that runs natively.

The bundle is built from the `memstead-wasm` crate with wasm-bindgen
(`--target web`): instantiate the module, load a `.mem` snapshot, then
read entities, relationships, and graph structure client-side — no server
round-trips after the snapshot fetch.

## Install

```bash
npm install @memstead/wasm
```

Or build the bundle from source:

```bash
cd crates/memstead-wasm
wasm-pack build --target web --release   # output in pkg/
```

## Compatibility

**This package is version-matched to the engine.** Each release of
`@memstead/wasm` is built from the Memstead engine of the same version
and reads the archives that version's CLI writes — check yours with
`memstead --version`. If the two numbers match, the archive and the
reader agree.

The package previously ran on its own version line, which is how it came
to sit at 0.1.2 against a 0.7.0 CLI with nothing on either page saying so.
One number now answers the question.

## Use

```js
import init, { Engine, setPanicHook } from "@memstead/wasm";

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
  // is gone, not aliased). The shipped `.d.ts` is authoritative.
  console.log(entity.entity_type, entity.title, Object.keys(entity.sections));
}

engine.memNames();  // the mems this snapshot carries
engine.health();    // entity/edge counts, per-mem breakdown
```

Full-text search is deliberately unavailable in the browser build:
`engine.search(...)` throws `{ code: "SEARCH_UNAVAILABLE_IN_WASM" }` so a
call site can catch the code and route the query elsewhere rather than
branching at the import layer.

Type definitions (`.d.ts`) ship in the package.

## License

MIT OR Apache-2.0, at your option.
