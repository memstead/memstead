---
type: overview
title: Kāra overview
updated_round: 9
---

# Kāra overview

**Kāra** is a statically-typed, Rust-inspired systems programming language with a
compiler named **`karac`**, written in Rust. Program files use the `.kara` extension.

## Origin: a complete language redesign

Kāra's root commit records a **complete language redesign** (see
[[history-reversals-and-deprecations]]). The **original design** used an
`fn` / `flow` / `record` / `->` **pipeline** style. That whole design was **replaced**
(superseded at the repo's origin) by the current Rust-inspired systems language. This
whole-language supersession is the single largest change on record.

## Headline design bets

- **Effect system** — six built-in effect verbs plus user-defined resources. See [[design-effect-system]].
- **Auto-concurrency** — parallelism is derived from effect analysis; there is
  **no async/await and no colored functions**. Round 2 added **auto-par reductions** and,
  for I/O, a **compiler-internal state-machine transform** behind the v1.1 event loop (still
  no user-visible `async`). See [[design-concurrency-and-providers]], [[design-runtime-phases]].
- **Tiered ownership** — parameter-mode inference, owned returns by default, explicit
  `ref` for borrows, reference-counting (RC) fallback with budget controls, and
  **no lifetime annotations**. See [[design-ownership]].
- **Data-layout separation** — a logical struct is distinct from its physical memory
  layout; struct-of-arrays (SoA) is opt-in. See [[design-data-layout]].
- **Algebraic data types** — Rust-style enums with exhaustive pattern matching. See
  [[design-adt-and-pattern-matching]].
- **AI-first compiler interface** — structured JSON diagnostics, a compiler query API,
  and canonical formatting. See [[design-ai-first-compiler]].
- **Phased runtime** — v1 blocking I/O, v1.1 network event loop, v2 full hybrid. See
  [[design-runtime-phases]].

## What exists

The compiler covers lexing, parsing/AST, name resolution, type checking, effect
checking, ownership analysis, an interpreter, and an LLVM code generator, plus a
standard library ([[stdlib-and-traits]]), a CLI ([[cli]]), a REPL, a formatter, and
example programs and benchmarks ([[examples-and-benchmarks]]). See
[[implementation-phases]] and [[compiler-pipeline]].

Round 2 added three interactive/agent surfaces and depth in generics and low-level code:
a **[[jupyter-kernel|Jupyter kernel]]**, a **[[playground|browser playground]]** (wasm32),
a full **[[attributes|attribute system]]**, **[[design-unsafe-ffi-and-pointers|unsafe/FFI
and raw-pointer]]** support, **[[design-generics-and-impl-trait|const generics, GATs, and
`impl Trait`]]**, and the **[[design-runtime-phases|v1.1 network event loop]]**.

Round 3 turned the event loop into a working runtime and made `karac` a package manager:
a **[[networking|non-blocking network I/O stack]]** (TCP / rustls TLS / WebSocket / File)
with **[[design-concurrency-and-providers|structured concurrency]]** (`spawn` / `TaskGroup`),
a **[[package-management|dependency resolver + lockfile + vendoring]]**, user-defined `Drop`
+ **`defer`/`errdefer`** + **`Atomic[T]`**, concurrency-safety diagnostics with a
**[[cli|`karac migrate`]]** auto-fixer, and language surface (module-level `let`, `test { }`
blocks, byte literals). The README's v1 positioning was reworded from "backend-first" to a
**"Production-Ready skeleton"** (see [[history-reversals-and-deprecations]]).

Round 4 added a verification surface, a JIT, and productionized the runtime:
**[[design-contracts-and-verification|contracts, refinement types, and distinct types]]**
(Phase 9), a **[[codegen|JIT execution path]]** (LLJIT/orc2, now the default for
`karac test`/`repl`), and an **[[design-runtime-phases|LLVM-coroutine async transform]]**
that supersedes the round-2/3 hand-written state-machine transform. The [[networking|HTTP
stack]] gained a **client**, **HTTP/2**, and **client-side TLS**; concurrency gained
**backpressure primitives** (`Semaphore` / `RateLimiter` / `BoundedChannel`) and
**`par struct`/`par enum`**; the [[design-ai-first-compiler|agent surface]] gained typed
contract-fault categories and DWARF crash diagnostics. The repo adopted a **dual
MIT/Apache-2.0 license**.

Round 5 turned outward — performance, portability, and self-hosting. It began the
**[[self-hosting|Kāra-in-Kāra port]]** (new **Phase 12**, resequenced as the v1 pivot),
landed **[[rc-elision|RC elision]]** (count-free reference-counted chains) as the backbone
of a benchmark-leading **[[examples-and-benchmarks|idle-connection density]]** (verified
against Java/Netty, Go, Node, Phoenix, and .NET), shipped **[[simd|portable SIMD]]** and a
shape-typed **[[numerical-stdlib-and-tensors|`Tensor[T, Shape]`]]** (Phase 11), and built
out **[[wasm-targets|WASM targets]]** (WASI / browser / Component Model / opt-in threads,
with a `#[target]` effect gate and **host fn** — enabling **SSR**). Concurrency gained
**auto-parallel I/O**, **AOT channel lowering**, and a **spinlock → futex `Mutex`**;
Phase 8 gained **[[fallible-allocation|fallible allocation]]**.

Round 6 pushed toward cross-platform maturity and metaprogramming. Native
**[[windows-and-cross-platform|Windows IOCP]]** flipped the **M3 cross-platform-parity gate
to DONE** (Linux + macOS + Windows); a whole **[[metaprogramming|compile-time metaprogramming
(comptime)]]** layer landed — reflection, AST emission, and derive-desugaring — and carried
the new **[[protobuf|protobuf (proto3)]]** stdlib; SoA `layout` blocks now
**[[per-layout-monomorphization|cross function boundaries]]** (proven by the Slipstream LBM
dogfood); the **[[self-hosting|self-host port]]** advanced from lexer to a modular **parser**;
a wide **[[stdlib-and-traits|stdlib completeness]]** sweep (String/char/integer methods,
`SortedMap`, `binary_search`) landed via cross-kata audits; and the project stood up a
**machine-countable [[bug-tracker|bug ledger]]**. Browser [[examples-and-benchmarks|dogfoods]]
(Fathom, Plume, Iris, Cartographer) shipped on the [[wasm-targets|`std.web` event/timer]]
producers.

Round 9 flipped **[[cli|`karac run` to the JIT execution path]]** by default (an `--interp`
escape keeps the interpreter as a dev/debug backend), deliberately collapsing the **run-vs-build
divergence** that had dominated the [[bug-tracker|ledger]]. **[[gpu-compute|GPU compute]]** grew
from element-wise-map to **LBM kernels on Metal** (struct-SoA dispatch, multi-field SoA groups,
`#[gpu]` control flow); an **[[design-unsafe-ffi-and-pointers|additive-interop / "Kāra as a
library"]]** surface landed (producer-mode static/dynamic libraries, raw-pointer methods,
auto-boxing, `#[repr(C)]` enum crossing, and a per-target **`#[repr(C)]` struct-by-value ABI**
gated by a 5-target CI matrix). Concurrency gained **[[design-concurrency-and-providers|A2b-2
auto-parallel network I/O]]** with parameterized-resource keys; the stdlib gained `std.mem` /
`std.cmp`, `SortedSet`/`SortedMap` and protobuf **under codegen**; and **[[self-hosting|Phase-12
self-hosting]]** was un-paused (the expression parser oracle is green). All five of round 8's
open bugs closed.

Round 8 closed the **[[codegen|heap-env closure epic]]** (round 7's headline open bug — an RC
heap env now covers every escaping-closure place), advanced **[[gpu-compute|GPU codegen]]** to
a working **slice-0** (a `wgpu` compute spine proven on Metal, WGSL codegen, end-to-end
`gpu.dispatch`), added **[[columnar-data|`DataFrame` + `Stats.*`]]** on a shared reduce kernel,
matured the **[[stdlib-and-traits|trait system]]** (`Reduce`/`ElementwiseMap` traits, default
methods, primitive & container impls, derived-`Ord` ordering), and built a whole
**[[package-management|dependency-fetching]]** subsystem (git deps, a `registry-proxy` crate,
PubGrub solving).

Round 7 opened new frontiers and closed a long-standing codegen gap. A
**[[gpu-compute|GPU compute-shaders]]** front end landed (explicit **`#[gpu]`**, the
**`GpuSafe`** structural trait, and a call-graph/effect gate — WGSL codegen was a spike then,
device slice-0 landing round 8), adding a device target to the [[wasm-targets|Phase-10]]
roster. **[[codegen|First-class function values]]** were completed in every position and a
large **heap-env closure epic** gave escaping capturing closures a reference-counted heap
environment (round-7 open, **closed round 8**). A nullable
**[[columnar-data|`Column[T]`]]** Arrow type with **SQL three-valued logic** joined the
[[numerical-stdlib-and-tensors|numerical]] stdlib; **[[protobuf|protobuf]]** grew its full
proto3 field-type matrix; new interpreter-slice primitives (`OnceLock`/`OnceCell`, `Arena`,
`Symbol`/`Interner`) landed; and the **[[self-hosting|self-host parser]]** reached trait/impl
items and generics.
