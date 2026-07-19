---
type: spec
created_date: 2026-07-15T17:50:37Z
last_modified: 2026-07-15T19:08:07Z
level: M1
stability: experimental
tags: gpu, phase-10, wgsl, effects, codegen
---

# GPU Compute Shaders

## Identity
Kāra's Phase-10 GPU compute-shader subsystem: an explicit `#[gpu]` function marker whose front-end enforces a `GpuSafe` structural dialect (safe signatures/locals, non-recursive host-free call graph, no host-capturing closures, no panics, effects flowing from `#[gpu]` roots), with a WGSL codegen backend (slice-0) that bakes element-wise-map kernels and drives them through a wgpu runtime spine.

## Purpose
To let Kāra functions compile to GPU compute shaders while statically guaranteeing they only use constructs the GPU can safely run — catching host-only shapes at compile time instead of miscompiling or faulting on-device.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[type-checker]]
- **DEPENDS_ON**: [[effect-checker]]
- **MOTIVATED_BY**: [[route-gpu-compute-through-an-explicit-function-marker]]
- **REFERENCES**: [[route-gpu-compute-through-an-explicit-function-marker]]

## Realization

- Marker + placement: `#[gpu]` attribute (FE-1 semantic marker, capture + placement gate)
- GpuSafe type-check: src/typechecker/gpu_safe.rs (structural GpuSafe on signatures and local-binding types); SL-1 `GpuSafe` as a built-in structural marker trait
- Call-graph checks: src/typechecker/gpu_call_graph.rs (recursion, generic non-`#[gpu]` callees, host-capturing closures)
- Effect gate: src/effectchecker/gpu_effect_gate.rs (effect enforcement from `#[gpu]` roots; explicit-panic rejection)
- WGSL codegen: src/gpu_wgsl.rs (WGSL emission for element-wise-map kernels + `gpu.dispatch` bake); scoping sketch docs/spikes/gpu-wgsl-slice0.md
- Runtime GPU spine: runtime/src/gpu.rs (wgpu compute dispatch, proven on Metal); the gpu runtime archive is auto-selected (no `KARAC_RUNTIME` needed)
- Tracker: docs/implementation_checklist/phase-10-targets.md; tests/typechecker.rs, tests/effectchecker.rs

## Specifies

- FE-1: the `#[gpu]` semantic marker — a capture + placement gate that recognizes marked functions and rejects invalid placement.
- FE-2 / FE-2b: `GpuSafe` structural type-check on `#[gpu]` signatures and on local-binding types (only structurally GPU-safe types cross the boundary or are bound inside).
- FE-3a: reject recursion in `#[gpu]` call graphs.
- FE-3b: reject calls from `#[gpu]` code to generic non-`#[gpu]` functions.
- FE-3c: reject host-capturing closures inside `#[gpu]` functions.
- FE-4: effect enforcement from `#[gpu]` roots (the reachable call graph must stay within the GPU-permitted effect set).
- FE-4b: reject explicit panics in the `#[gpu]` call graph.
- SL-1: `GpuSafe` is a compiler built-in structural marker trait (membership is structural, not user-implemented).

- WGSL codegen (slice-0): the runtime wgpu compute spine (0a), WGSL emission for element-wise-map kernels (0b), and `gpu.dispatch` typing + run + WGSL bake + wgpu call (0c).
- i32/u32 GPU element types via byte-oriented dispatch; the gpu runtime archive is auto-selected at link time.


- Backend progress past slice-0 (all proven on Metal): CG-4 struct-SoA `gpu.dispatch` (codegen struct-SoA dispatch, typecheck of a struct-SoA buffer, SoA WGSL emitter) closing GPU-GATE-1 ("Path A on Metal"); multi-field layout groups run on Metal (SoA emitter multi-field WGSL sub-structs + per-field interleave); and value control flow inside `#[gpu]` kernels (`if`/return codegen, GPU-LBM-4).
- The Slipstream-LBM GPU path is tracked as blockers GPU-LBM-1..6. Owner directives this round: GPU-LBM-1 decided → `f32`; the v1 GPU gate is decoupled from the Slipstream LBM demo; and the "build once in Kāra" GPU-LBM deferral was dropped.


- GPU progress this round (all proven on Metal): the **GPU-LBM cluster is COMPLETE** (GPU-LBM-1..6 — f32 collide, scalar uniforms, multi-field groups, value control flow, #[gpu] helper functions emitted as WGSL fns, and stencil/neighbour-read kernels for the LBM `stream`). A second cluster, **GPU-SLIP**, landed the full Slipstream LBM substep on the GPU: expression primitives in kernel bodies (let/and-or/sqrt/cast), bool/mixed-type/let helpers, the full stream on Metal, and chaining `collide` → `stream` across dispatches (double-buffered substep). Perf: the wgpu device + compiled pipelines are now cached across dispatches (GPU-SLIP-4a, ~1.9x).
- `karac run` (JIT-default) routes any program declaring a `#[gpu]` kernel to the tree-walk interpreter (element-wise CPU), since the JIT runner's runtime rlib is not built with the `gpu` feature (B-2026-07-10-6); `karac build` and `--interp` are unaffected.

## Constraints

- GPU offload is never inferred — only `#[gpu]`-marked functions are compiled for the GPU (see [[route-gpu-compute-through-an-explicit-function-marker]]).
- A `#[gpu]` function and its transitive callees must be GpuSafe, non-recursive, host-capture-free, panic-free, and within the GPU effect set, or the program is rejected with an honest diagnostic.

## Rationale

Phase 10 (targets). Front-end gates landed ahead of the backend so unsupported constructs fail loudly rather than miscompiling; the WGSL codegen backend (slice-0) has now landed — the runtime wgpu compute spine (proven on Metal), element-wise-map WGSL kernels, and `gpu.dispatch` end-to-end. Realizes the explicit-marker routing decision [[route-gpu-compute-through-an-explicit-function-marker]].
