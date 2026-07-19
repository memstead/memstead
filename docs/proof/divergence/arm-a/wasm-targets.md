---
type: architecture
title: WASM targets and the target/host-fn model (Phase 10)
updated_round: 9
---

# WASM targets, `#[target]`, and host functions

**Major round-5 work (Phase 10 targets).** What was a single wasm32 target for the
[[playground|browser playground]] became a full multi-target WASM story: **headless WASI**,
**browser**, the **WebAssembly Component Model**, and **opt-in threads**, plus a
first-class **target model** (`src/target.rs`, +518) and a **target-gated effect surface**.
Tracked in `docs/implementation_checklist/phase-10-targets.md` (+210). New crate surfaces:
`src/wasm_exports.rs`, `src/wasm_glue.rs`, `src/wit.rs`, `src/componentize.rs`,
`src/codegen/cabi.rs`, `runtime/src/wasm_alloc.rs`, `runtime/src/wasm_threads_scheduler.rs`,
and stdlib `wasi.kara` / `web.kara` / `web_net.kara` / `web_time.kara`.

## Build targets

- **`karac build --target=wasm_wasi`** — a WASM/WASI build path end-to-end (a default-on
  `net` feature makes the `wasm32-wasip1` archive compile).
- **`--target=wasm_browser`** — host fn lowers to wasm import entries + JS glue.
- **`--target-cpu`** / **`--target-features`** overrides (flag + env + `[release]` table),
  and **`--bindings`** selects the WASM output shape. See [[cli]].

## WASM entry points, bindings, and the Component Model

- **Entry-point discovery** (A–E) — export tagged `pub fn`s; browser `.d.ts` types;
  **canonical-ABI component exports** for scalars (C), record return + `cabi_realloc`
  (D.1), record params (D.2), `Option`/`Result` (D.3), string/list (E), and **browser-rich
  JS-shape exports** (D.4).
- **Component Model** — WIT component exports (`src/wit.rs`, `src/componentize.rs`); the
  **embedded-WIT component is the `wasm_wasi` default**; a paired `.component.wit` descriptor
  artifact was added and then the **paired component form was removed** (consolidated on the
  embedded form; see [[history-reversals-and-deprecations]]).

## Threads (opt-in) and concurrency lowering

- **WASM concurrency default is sequential** (Phase 10 line 125) — a `seq_scheduler`
  (`runtime/src/seq_scheduler.rs`) runs `par`/`spawn` sequentially on plain wasm32.
- **`--features wasm-threads`** — opt-in threaded WASM: a pool-backed spawn/TaskGroup/par on
  `wasm32-wasip1-threads`, a threaded codegen pass (wasip1-threads machine, auto-par
  re-enable, shared-memory link), dual-pass build wiring, and a synchronous
  worker→main **host-fn proxy**. The flag first landed **accepted-but-inert**.
- **`[wasm]` manifest table** — `pool-size` / `fallback` / `max-memory-pages` knobs.
- **`std.web.time.after`** — a host-async timer lowered to a channel producer (wasm-threads).

## Host functions and the target-gated effect surface

- **`host fn`** — declarations (parser + resolver + typechecker) that lower to import
  entries; a **native lowering** provides the **server-WASM** target via `kara_host` import
  entries.
- **`#[target(...)]` attribute** — parser validation + absence-at-resolution; a
  **target-gate pass** (`src/effectchecker/target_gate.rs`) enforces **provided-resource
  sets per target** with **`E0411`** (a resource not provided on the selected target).
- **Ambient resources** — lowercase aliases `clock` / `rand` / `stdin` / `stdout` /
  `stderr` / `fs` wired (L646), with codegen lowering for `rand.next_u64()`, `env.args()`,
  `env.var()`, `Stdin.read_line/read_to_string`, `FileSystem.write`, and explicit
  `Stdout`/`Stderr` `print`/`println`/`flush`. `with_provider` can override no-vtable-slot
  ambient methods.
- **Gated `std.wasi` / `std.web` modules** — the headless-WASM and browser/host effect
  vocabularies, gated to their targets.

## SSR (server-side rendering) — a dual-target payoff

WASM-browser + native together enable **SSR**: the [[examples-and-benchmarks|`ssr_counter`
example]] renders the same Kāra on the server and in the browser via a **provider-injection
pattern**, documented in the book (`ch17-ssr.md`).

## Round-6 — browser event/timer producers and the threaded-glue fixes

The [[examples-and-benchmarks|browser dogfoods]] (Fathom, Plume, Iris) turned the WASM
target from "hello world" into interactive multi-core programs, and surfaced the round's
threaded-glue bugs.

- **`std.web.events`** (`runtime/stdlib/web_events.kara`, +605) — **host-async event
  producers** lowered to `Channel[T]`: `clicks`, `pointer_moves`, `wheel`, `keydown` /
  `keyup`, `focus` / `blur`, `resize`, `contextmenu`, `dblclick`, and `touchstart` /
  `touchmove` / `touchend` (`PointerEvent.buttons` for click-drag pan). **`std.web.time.every`**
  (`web_time.kara`) is a recurring `setInterval` channel producer (alongside round 5's
  `after`).
- **wasm-threads browser deadlock fixed** (`B-2026-06-14-17`) — the threaded glue only
  proxied the *program* to a primary worker, not *thread creation*; a blocked worker never
  turned its event loop so sibling module scripts never loaded. Completed the Emscripten-style
  model: route every worker's `thread-spawn` through the always-live **main thread** via a
  shared spawn-request ring. Fathom now renders across 18 cores.
- **SharedArrayBuffer decode fixes** (`B-2026-06-14-22`) — `fd_write` / `random_get` /
  `readString` copied out of shared memory before `TextDecoder` / `crypto.getRandomValues`
  (which reject shared-backed views), fixing a fatal `TypeError` on any threaded-browser
  print/panic.
- **Numeric f-string on wasm** (`B-2026-06-14-15`) — `snprintf`'s `size_t` is pointer-width
  (i32 on wasm32); the hardcoded i64 declaration trapped `signature_mismatch`, so
  `println(f"{x}")` for any numeric `x` aborted. Fixed to a pointer-width `size_t`.
- **`karac project build --target=wasm_*`** now drives platform-suffix module selection
  (`c631507c`).

## Round-7 — reproducible components and the GPU sibling target

- **Byte-reproducible component builds** (`B-2026-06-22-3`) — `--bindings component` (the
  `wasm_wasi` default) had emitted a **different SHA256 per build**: the C-ABI core module was
  linked under a **process-unique `karac_<pid>_<stem>.core.wasm`** scratch name, which
  `wasm-ld` baked verbatim into the module `name` section and `wasm-tools component
  embed/new` carried into the final component (so two builds differed in the pid digits).
  `--bindings none` (core module) was always reproducible. Fixed by linking the core under a
  **source-derived basename inside a pid-unique directory** (`componentize::link_core_scratch`),
  so parallel builds stay collision-free on disk while the component is byte-identical run to
  run — and the shipped artifact no longer leaks a temp path. Surfaced by the new
  **[[examples-and-benchmarks|`bench/wasm_size`]]** receipt. See [[bug-tracker]], [[cli]].
- **[[gpu-compute|GPU compute shaders]]** — round 7 opened a **new Phase-10 device target**
  alongside the WASM targets: an explicit **`#[gpu]`** attribute, the **`GpuSafe`** structural
  trait, and a call-graph + effect gate. WGSL codegen was a spike at round 7; round 8 landed a
  working device slice-0; **round 9 advanced to LBM kernels on Metal** (struct-SoA dispatch,
  multi-field SoA groups, `#[gpu]` control flow). Same explicit-marking model as `#[target]`.
  See [[gpu-compute]].

## Round-9 — a browser DOM value channel

- **`std.web.events.input`** (`1b8a0608`) — a browser **DOM value channel** for `input`
  events, wired end-to-end with a **Slipstream slider** (the LBM demo's parameter control), a
  round-6 `std.web` browser-producer follow-on. See [[design-concurrency-and-providers]].

Related: [[cli]], [[simd]], [[design-effect-system]], [[design-runtime-phases]],
[[codegen]], [[examples-and-benchmarks]], [[playground]], [[windows-and-cross-platform]],
[[gpu-compute]].
