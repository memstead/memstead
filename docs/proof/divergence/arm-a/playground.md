---
type: architecture
title: Browser playground and wasm32 target
updated_round: 8
---

# Browser playground and wasm32 target

New in round 2: Kāra has an in-browser **playground**, tracked as **Phase 5 line 703**
(5 slices) and delivered in a new `playground/` crate. It runs the compiler front end in
the browser via WebAssembly.

## The wasm32 target

- **wasm32 compile target** (slice 2) — the compiler front end compiles to `wasm32`, with
  a **`wasm-bindgen` wrapper** exposing an entrypoint to JS.
- `playground/src/lib.rs` — the wasm entrypoint (slice 1); `playground/build.sh` — the
  build script (slice 5).

## The web shell

- **Static HTML/JS playground shell** (slice 3) — `playground/web/index.html`,
  `playground.js`, `playground.css`: an editor + output surface with no server dependency.
- **URL share via compressed fragment** (slice 4) — a program can be shared by encoding it
  into a compressed URL fragment, so a playground link round-trips the source.

## Round-8 correctness fix — the playground was 100% broken

For a stretch the browser playground **trapped on every `run()`**, even
`fn main() { println("hi") }` — two `wasm32-unknown-unknown`-only faults surfaced by a
headless-Chrome probe (`B-2026-07-02-29/-30`, fixed `0624f0fe`):

- The Windows fat-stack fix had lifted every interpreter run onto a `spawn_scoped` thread,
  which wasm cannot spawn — so the run trapped as "failed to spawn interpreter thread". Fixed
  by running the closure inline on wasm.
- `Interpreter::new` seeded its RNG from `SystemTime::now()`, which panics "time not
  implemented" on wasm — replaced with a fixed golden-ratio seed. The same slice guarded the
  rest of the sys/unsupported class reachable from playground programs (`sleep_ms` no-op,
  `par{}` → sequential, `Clock.now` / `RateLimiter` → runtime diagnostics). See [[bug-tracker]].

## Relationship to other surfaces

The playground is a sibling interactive surface to the [[jupyter-kernel|Jupyter kernel]]
and the [[cli|REPL]]; all three drive the same compiler front end. It reinforces the
[[design-runtime-phases|backend-first v1]] and [[design-ai-first-compiler|AI-first]]
positioning by making the language trivially reachable.

Related: [[cli]], [[jupyter-kernel]], [[implementation-phases]].
