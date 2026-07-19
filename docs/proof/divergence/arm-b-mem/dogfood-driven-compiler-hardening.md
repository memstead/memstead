---
type: memo
created_date: 2026-07-15T17:34:28Z
last_modified: 2026-07-15T19:08:43Z
status: active
tags: dogfooding, examples, quality, bug-discovery
---

# Dogfood-Driven Compiler Hardening

## Claim
Kāra's real-program dogfood demos are the primary engine for discovering and closing compiler/stdlib gaps — each new demo drives a burst of numbered bug fixes and proves a headline feature end-to-end.

## Context
Pre-1.0, the compiler is exercised against a growing roster of non-synthetic programs (browser + systems) alongside katas; the demos live under examples/ and are tracked in docs/dogfooding.md. Dogfood-sourced bugs (source tags `dogfood:*`) dominate the machine-countable bug ledger — see [[ci-test-coverage-tiers-and-the-leak-gate]].

## Relationships
- **REFERENCES**: [[ci-test-coverage-tiers-and-the-leak-gate]]
- **REFERENCES**: [[wasm-target-backend]]
- **REFERENCES**: [[data-layout-separation]]
- **REFERENCES**: [[network-runtime-and-cooperative-scheduling]]
- **REFERENCES**: [[compiler-query-api]]

## Substance

Roster this round:
- **Fathom** — browser multi-core Mandelbrot explorer (interactive pan/zoom, SIMD-128 kernel); proved [[wasm-target-backend]] threaded browser parallelism (surfaced the main-thread spawn-proxy deadlock, shared-ArrayBuffer polyfill rejections, for-over-collection body leak, and non-scalar TaskHandle.join).
- **Plume** — pointer-steered browser flow field.
- **Iris** — browser image-filter studio with a native/wasm A/B verify harness.
- **Slipstream** — LBM wind tunnel; the full-SoA proof for [[data-layout-separation]] per-layout monomorphization.
- **Relay** — HTTP/1.1 keep-alive reverse proxy (L7 path routing, round-robin load balancing, live par-struct metrics) with a 3-language wrk cross-host benchmark; hardened [[network-runtime-and-cooperative-scheduling]] spawn-capture ownership.
- **Cartographer** — live WASM compiler-in-browser effect-graph studio; dogfoods the [[compiler-query-api]] whole-program effect/concurrency graph.
- **Weave** — refinement types + contracts + effects CSV ETL.
- **Tangle** — shared-mutable-state demos (tree-walking interpreter, doubly-linked list, undo/redo).


New example programs this round (each drove a burst of numbered fixes):
- **json** (examples/json.kara) — a recursive JSON parser over a `shared enum`; surfaced ref-enum payload binding, Vec-of-struct enum-payload field access, and `?`-on-Result[concrete-enum] payload-recovery bugs (B-2026-07-11-5/6/7).
- **vm** (examples/vm.kara) — a stack-based bytecode interpreter; surfaced the `with_capacity(0)` zero-capacity leak (B-2026-07-11-15).
- **pipeline** (examples/pipeline.kara) — a functional log-analytics pipeline; drove the iterator-terminal codegen (fold/for-over-chain, B-2026-07-11-17/18/19).
- **heap** (examples/heap.kara) — a generic binary min-heap `Heap[T: Ord]` + heapsort; surfaced generic-container monomorphization + non-Copy-element swap bugs (B-2026-07-11-31/32/35).
- **semantic_search** (examples/semantic_search.kara) — a std.embeddings end-to-end demo.
- Overnight autonomous dogfooding probes (paired interpreter-oracle vs JIT/native) surfaced a wave of ownership, shadowing, and closure-return-type codegen bugs (e.g. the nested-scope shadow leak B-2026-07-13-6).

## Alternatives



## Outcome

Each demo lands with its fixes; the practice keeps compiler hardening grounded in real programs rather than synthetic tests, and feeds the ledger's bug-discovery curve.
