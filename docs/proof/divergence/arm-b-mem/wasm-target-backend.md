---
type: spec
created_date: 2026-07-15T10:52:02Z
last_modified: 2026-07-15T17:52:18Z
level: M1
stability: experimental
tags: wasm, targets, phase-10, codegen
---

# WASM Target Backend

## Identity
Kāra's Phase-10 WebAssembly target family: `karac build --target=wasm_wasi` (headless WASI) and `--target=wasm_browser` (browser), with entry-point discovery, WASM Component Model / WIT exports over the canonical ABI, opt-in wasm-threads, WASM SIMD-128, and browser artifact emission (dist/wasm + .d.ts).

## Purpose
To let Kāra compile to WebAssembly for both server-side WASI hosts and the browser, reusing the LLVM backend while honoring each target's provided-resource set and ABI conventions.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[llvm-codegen-backend]]
- **DEPENDS_ON**: [[effect-system]]
- **REFERENCES**: [[llvm-codegen-backend]]
- **REFERENCES**: [[effect-system]]

## Realization

- Targets + gating: src/target.rs, src/effectchecker/target_gate.rs, `#[target(...)]` attribute (src/parser/attributes.rs)
- Exports/ABI: src/wasm_exports.rs, src/wit.rs, src/codegen/cabi.rs, src/componentize.rs, src/wasm_glue.rs
- Runtime: runtime/src/wasm_alloc.rs, wasm_threads_scheduler.rs, seq_scheduler.rs; runtime/stdlib/wasi.kara, web.kara, web_net.kara, web_time.kara
- Examples: examples/wasm_hello/; tests/wasm_codegen.rs
- Tracker: docs/implementation_checklist/phase-10-targets.md

## Specifies

- Build paths: `--target=wasm_wasi` (embedded-WIT component is the default artifact) and `--target=wasm_browser` (project-mode dist/wasm + .d.ts); default-on `net` feature so the wasm32-wasip1 archive compiles; `karac check` multi-target verification; `--bindings` output-shape selector.
- Entry-point discovery A–E: export tagged `pub fn`s; browser .d.ts scalar types; component WIT exports for scalar entry points; canonical-ABI record/Option/Result/string/list component exports with cabi_realloc; browser-rich JS-shape exports.
- wasm-threads (opt-in via `--features`): wasip1-threads machine, pool-backed spawn/TaskGroup/par, shared-memory link, auto-par re-enable, dual-pass build wiring, host-fn synchronous worker→main proxy; `[wasm]` manifest table (pool-size / fallback / max-memory-pages).
- WASM concurrency defaults to sequential (seq_scheduler); WASM SIMD-128 lowering (+simd128 default, v128 Vector ops).
- Gated effect modules: std.wasi (headless WASM effect surface) and std.web (browser/host effect vocabulary); host `fn` declarations lowering to `kara_host` / wasm import entries + JS glue.
- DWARF stripped from emitted .wasm by default (482→30 KiB).
- `std.web.time.after` host-async timer → channel producer (wasm-threads).


- Browser event/host surface (runtime/stdlib/web_events.kara, web_time.kara): std.web.events.* Channel[T] host-async producers — keydown/keyup, clicks, dblclick, contextmenu, wheel, pointer_moves (+ PointerEvent.buttons for click-drag), touchstart/touchmove/touchend, focus/blur, resize — plus std.web.time.every (recurring setInterval). These power the browser dogfood demos (Fathom multi-core Mandelbrot, Plume flow-field, Iris image filters, Slipstream LBM wind tunnel).
- wasm-threads BROWSER parallelism fixed: the earlier glue only proxied the program (not thread creation) to a primary worker, so any par program DEADLOCKED in a real browser (worked under node, so it passed E2E). Completed the Emscripten-style model — every worker's thread-spawn is routed to the always-live main thread via a shared spawn-request ring; verified rendering across 18 cores in headless Chrome. Also fixed shared-ArrayBuffer view rejections in the WASI fd_write / random_get / readString polyfills (copy out before decode).

- `--bindings component` (the wasm_wasi default) now emits **byte-reproducible** component artifacts: the core module links under a source-derived basename inside a process-unique directory, so a per-process pid no longer leaks into the wasm `name` section and identical source yields an identical SHA run-to-run (B-2026-06-22-3, surfaced by the wasm_size bench).

## Constraints

- A function may use only the effect resources its `#[target(...)]` provides; a resource absent for the target is rejected at resolution (E0411).
- The WASM Component Model exports use the canonical ABI; record returns allocate via cabi_realloc.

## Rationale

Phase 10 (targets). Builds on the [[llvm-codegen-backend]]; each target's available effect resources are enforced by the effect-system target-gate (see [[effect-system]]). WASM concurrency defaults to sequential lowering, with wasm-threads as an opt-in escalation.
