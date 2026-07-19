---
type: architecture
title: GPU compute shaders (Phase 10) — the #[gpu] front end
updated_round: 10
---

# GPU compute shaders — `#[gpu]` and `GpuSafe`

**Updated in round 10.** Kāra began building **GPU compute shaders** as a
[[wasm-targets|Phase 10 target]]. Round 4 had only *relocated a GPU note* in the README (see
[[history-reversals-and-deprecations]]); round 7 built the **front-end validation layer** plus
a **WGSL codegen spike**; round 8 turned the spike into a **working slice-0** (a real `wgpu`
compute spine, WGSL codegen, end-to-end `gpu.dispatch` — proven on Metal); round 9 grew the
device backend from element-wise-map to real LBM (lattice-Boltzmann) kernels on Metal —
struct-SoA dispatch, multi-field SoA groups, and value control flow. **Round 10 finishes the
LBM kernel work: the GPU-LBM cluster is COMPLETE, a full Slipstream LBM substep (collide→stream)
runs on the device double-buffered, and the round-9 open bugs are closed.**

## The routing decision — Option B (explicit `#[gpu]`)

The first thing settled was **how GPU code gets identified** (`c1391c4c`,
`docs/implementation_checklist/phase-10-targets.md`). The resolved decision is **Option B:
an explicit `#[gpu]` attribute** on the functions destined for the device — the compiler
does **not** infer GPU-eligibility implicitly. This mirrors the explicit-marking style of
the [[wasm-targets|`#[target(...)]` attribute]]: the programmer declares intent, the compiler
verifies it. The Phase-10 GPU compute-shaders gate was then **split into tracked slices**
(`afa3336c`) — front-end gates first (FE-*), codegen later.

## `GpuSafe` — the structural marker trait (SL-1)

**`GpuSafe`** is a **built-in structural marker trait** (`037d407e`, SL-1): a type is
`GpuSafe` iff it is structurally safe to cross the host↔device boundary (POD-like — no
heap-bearing / RC / host-owned fields). Like Kāra's other structural markers it is derived
from a type's *shape*, not declared. It is the gate every `#[gpu]` signature and local
binding is checked against. Implemented in `src/typechecker/gpu_safe.rs` (+847).

## Front-end gates (FE-1 … FE-4b)

The `#[gpu]` front end is a stack of rejections that keep a shader well-formed **before** any
codegen exists — each an honest compile error rather than a device-side crash. Landed this
round:

- **FE-1 — `#[gpu]` semantic marker** (`6056231f`): the attribute itself, with a **capture +
  placement gate** (where `#[gpu]` may appear, what it may close over).
- **FE-2 — `GpuSafe` on signatures** (`e03e5732`): a **structural type-check** that every
  parameter/return type of a `#[gpu]` fn is [[#`GpuSafe` — the structural marker trait (SL-1)|`GpuSafe`]].
- **FE-2b — `GpuSafe` on local bindings** (`fa349980`): the same structural check on every
  `let`-bound local type inside a `#[gpu]` fn.
- **FE-3a — no recursion** (`f18775d9`): recursion is **rejected** in `#[gpu]` call graphs
  (GPUs have no unbounded call stack).
- **FE-3b — no generic non-`#[gpu]` callees** (`da5cc171`): a `#[gpu]` fn may not call a
  generic function that is not itself `#[gpu]`.
- **FE-3c — no host-capturing closures** (`0945b425`): closures inside `#[gpu]` fns may not
  capture host state.
- **FE-4 — effect enforcement from roots** (`2c67268f`): effects are enforced **transitively
  from `#[gpu]` roots** across the call graph (a device kernel cannot do host effects).
- **FE-4b — no explicit panics** (`dc011239`): an explicit `panic` anywhere in a `#[gpu]`
  call graph is rejected.

The call-graph walk (recursion, generic-callee, effect propagation from roots) lives in
`src/typechecker/gpu_call_graph.rs` (+550); the effect side is a dedicated
`src/effectchecker/gpu_effect_gate.rs` (+230), sitting alongside the round-5
[[design-effect-system|`#[target]` target-gate pass]]. Coverage is in `tests/typechecker.rs`
and `tests/effectchecker.rs`.

## WGSL codegen — slice-0 landed and proven on hardware

**New in round 8.** The device backend advanced from spike to a **working slice-0**
(`816fa751` marks 0a+0b+0c complete), documented in `docs/spikes/gpu-wgsl-slice0.md` (+139)
and the phase-10 checklist. The three sub-slices:

- **Runtime wgpu compute spine (slice-0a, `30f4fd81`)** — a real `wgpu` compute pipeline,
  **proven on Metal** (Apple GPU). Runtime file `runtime/src/gpu.rs` (+244).
- **WGSL codegen for element-wise-map kernels (slice-0b, `b7bb8236`)** — [[codegen]] now bakes
  WGSL shader source. Compiler file `src/gpu_wgsl.rs` (+433).
- **`gpu.dispatch` (slice-0c)** — typing + run + runtime symbol (`737a3cd7`, part 1), then
  codegen WGSL bake + `wgpu` call (`fee9b4c7`, part 2). `gpu.dispatch` is now a real
  end-to-end path (typecheck → WGSL bake → `wgpu` dispatch), not a sketch.

Two ergonomic/coverage follow-ons landed alongside:

- **Auto-select the GPU runtime archive (`bbe2df6a`)** — the GPU-enabled runtime is picked
  automatically; no `KARAC_RUNTIME` env var needed.
- **i32/u32 element types via byte-oriented dispatch (`35c40958`)** — a spike follow-on
  extending GPU element types beyond the initial set.

Further kernel shapes beyond element-wise-map are the round-9 work below.

## Round 9 — struct-SoA dispatch, LBM kernels, and control flow (Metal)

Round 9 tracks the Slipstream LBM GPU-path blockers as **GPU-LBM-1 … 6** and advances the
device backend well past element-wise-map:

- **GPU-LBM-1 → f32** (`1256c559`) — the LBM path is settled on **`f32`**, and the v1 GPU gate
  was **decoupled from the Slipstream LBM** (the GPU surface no longer waits on the demo). The
  earlier **"build once in Kāra" GPU-LBM deferral was dropped** (owner directive, `1cd66c46`).
- **CG-4 — struct-SoA `gpu.dispatch`** (Path A on Metal, `89e01e44`): the runtime gained a
  **multi-buffer dispatch** (`karac_runtime_gpu_map_multi`, `88c4da19`), the typechecker
  **accepts a struct-SoA buffer** and the WGSL emitter emits SoA sub-structs (`cdba16bf`), and
  codegen lowers the struct-SoA dispatch (`54c2903e`). **GPU-GATE-1 now runs on Metal.**
- **GPU-LBM-3 — multi-field SoA groups on Metal** (`518c2dd9`): the SoA emitter supports
  **multi-field groups** as WGSL sub-structs (`59c25da3`, slice 3a) and the groups **run on
  Metal via per-field interleave** (`519772af`, slice 3b) — the [[per-layout-monomorphization|
  SoA `layout`]] grouping now crosses to the device.
- **GPU-LBM-4 — value control flow in `#[gpu]` kernels** (DONE, `bf998a32` / `514ee12c`): an
  `if`-expression inside a `#[gpu]` fn lowers to a WGSL **`select`**. This co-fixed a general
  **float `if`-return codegen** bug (`B-2026-07-10-8`) — a float literal branch beside an `f32`
  sibling fell through the phi merge to an `i64 0` placeholder, emitting `ret i64 0` against a
  `float` return type; now `fptrunc`-reconciled.

The Slipstream GPU status was corrected to reflect the landed slice-0 spine (`59316198`).

### Round-9 open GPU gaps — both closed in round 10

- **`B-2026-07-10-7` — FIXED** (`e86942e`) — reading a field from a **LITERAL-constructed SoA
  binding** (`let world: Vec[Particle] = […]; world[1].pos`) no longer segfaults: the SoA
  let-init now scatters a `[…]` / `Vec[…]` literal RHS through the SoA push path, so the
  per-group backing arrays get allocated. This unblocks the literal-init variant of GPU-GATE-1
  (the `push`-only workaround is no longer required). See [[bug-tracker]].
- **`B-2026-07-10-6` — FIXED** (`05d72ed`) — `karac run` can now handle a `#[gpu]` program: a
  program declaring a `#[gpu]` kernel is routed to the **tree-walk interpreter** (element-wise
  CPU) under `karac run`, regardless of the [[cli|JIT-default]], so `karac run` matches
  `karac build`. The heavy `wgpu` dependency tree deliberately stays out of the JIT-runner rlib.
  See [[cli]], [[codegen]], [[bug-tracker]].

## Round 10 — GPU-LBM cluster COMPLETE, and a full LBM substep on the device

Round 10 finishes the LBM kernel work on Metal. Two clusters landed: **GPU-LBM** (the last
kernel-shape gaps) went COMPLETE, and **GPU-SLIP** (Slipstream LBM as the driving dogfood)
drove a full collide→stream substep onto the device.

### GPU-LBM — cluster COMPLETE

- **GPU-LBM-2 — scalar uniforms** — a scalar (e.g. the collide `omega`) is passed as a **struct
  uniform** in `gpu.dispatch`, not just buffers.
- **GPU-LBM-4b — whole-struct-valued if/guard** — an `if`/guard whose value is a whole struct
  (not just a scalar) lowers correctly inside a `#[gpu]` kernel.
- **GPU-LBM-5 — `#[gpu]` helper functions** — a `#[gpu]` helper fn is **emitted as a WGSL
  function** and called from the kernel body.
- **GPU-LBM-6 — stencil / neighbour-read kernels** — the LBM **`stream`** shape (a kernel that
  reads neighbouring cells) is supported. With this the **"GPU-LBM cluster COMPLETE"**
  (`f35c9d00`).

### GPU-SLIP — Slipstream LBM on the device (the new headline), COMPLETE

- **GPU-SLIP-1** — an **f32 GPU collide** on Metal: let-bindings inside struct-SoA `#[gpu]`
  kernel bodies.
- **GPU-SLIP-2a** — expression primitives in GPU bodies: `let`, and/or, `sqrt`, cast.
- **GPU-SLIP-2b** — bool / mixed-type / `let` `#[gpu]` helpers.
- **GPU-SLIP-2c** — the full LBM **`stream`** on Metal (cell-alias local + a helper-gathering
  fix).
- **GPU-SLIP-3** — a full GPU LBM **substep**: chained **collide→stream on the device**,
  double-buffered. With this the **"GPU-SLIP COMPLETE"** (`ef7e7e2b`).
- **GPU-SLIP-4 — perf slice** — 4a caches the `wgpu` **device + compiled pipelines** across
  dispatches, a **1.9× speedup** (no longer rebuilding the pipeline per dispatch); 4b is planned.

### Bugs surfaced and fixed by the substep work

- **`B-2026-07-11-20`** — `#[gpu]` **helper-call gathering** missed calls sitting inside `Index`
  / let-RHS / `MethodCall` / `Cast` positions — the LBM `stream` indexes a buffer by a `#[gpu]`
  helper result, so the helper was never emitted. Fixed to gather from those positions.
- **`B-2026-07-11-27`** (`437008a5`) — a `gpu.dispatch` result bound/assigned to a **SoA
  `layout` variable** SIGSEGV'd: the AoS dispatch result must be **scattered AoS→SoA** into the
  destination layout, not memcpy'd.
- **`B-2026-07-10-8`** (`514ee12c`) — a **float `if`-return phi width mismatch** (general, not
  GPU-specific; the round-9 co-fix, recorded here as it recurred in this work).

### Runtime and proof surface

`runtime/src/gpu.rs` grew substantially (**+584**). The GPU runtime is **Metal-proven**: there
is **no headless GPU in CI**, so the scatter/dispatch tests are **Metal-only** (they run on
Apple-GPU hardware, not in the CI matrix).

## Where it sits

GPU compute is a **Phase 10 target** feature — a new device target alongside
[[wasm-targets|WASI / browser / Component Model / wasm-threads]]. The pattern is deliberate:
an explicit attribute (`#[gpu]`, like `#[target]`), a structural safety trait (`GpuSafe`),
a call-graph + effect gate that makes the whole shader provably well-formed at the front end,
and only then codegen. It reuses the effect system rather than inventing a parallel one.

Related: [[wasm-targets]], [[design-effect-system]], [[implementation-phases]],
[[deferred-work]], [[history-reversals-and-deprecations]], [[compiler-pipeline]],
[[design-generics-and-impl-trait]].
