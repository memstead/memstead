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

## Use

```js
import init, { Engine, setPanicHook } from "@memstead/wasm";

await init();
setPanicHook(); // readable stack traces instead of "unreachable executed"

const bytes = new Uint8Array(await (await fetch("/my-graph.mem")).arrayBuffer());
const engine = Engine.fromSnapshot(bytes);
```

For the full snapshot + live-update (SSE) lifecycle against a
`memstead-bridge` server, use `@memstead/client` (in this repo at
`crates/memstead-wasm/client-js/`, likewise not yet on npm), which wraps
this package behind a single `MemSyncClient` class.

Type definitions (`.d.ts`) ship in the package.

## License

MIT OR Apache-2.0, at your option.
