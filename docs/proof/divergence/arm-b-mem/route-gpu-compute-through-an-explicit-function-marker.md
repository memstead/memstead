---
type: decision
created_date: 2026-07-15T17:50:16Z
last_modified: 2026-07-15T17:50:16Z
status: accepted
decided_on: 2026-06-22
deciders: kara-maintainers
scope: subsystem
tags: gpu, codegen, phase-10, effects
---

# Route GPU Compute Through an Explicit Function Marker

## Decision
We chose to route GPU compute-shader compilation through an explicit `#[gpu]` function marker (Option B), not through automatic inference of GPU-eligible code. A `#[gpu]` function opts into a strict, statically-enforced dialect: its signature and local bindings must be structurally `GpuSafe`, its call graph must be non-recursive and free of calls to generic non-`#[gpu]` functions, it may not capture host state in closures, and it may not panic — with effect enforcement flowing from `#[gpu]` roots. WGSL is the first shader-codegen target.

## Context
Phase 10 (targets) added GPU compute shaders as a gate. Kāra's house style derives parallelism automatically — [[auto-parallelization]] infers concurrent regions from effect signatures with no user construct. GPU offload sits in tension with that philosophy: GPU-eligible code needs hard structural guarantees (no host pointers, no recursion, no panics, no arbitrary allocation) that ordinary Kāra code does not satisfy, and silently promoting code onto the GPU would be a footgun. The decision was whether GPU targeting should be inferred like auto-par, or opted into explicitly. Resolved in docs/implementation_checklist/phase-10-targets.md, which also split the compute-shaders gate into tracked front-end slices (FE-1..FE-4) and a WGSL codegen slice.

## Consequences
- The front-end enforcement is a distinct, staged subsystem ([[gpu-compute-shaders]]): a semantic marker (capture + placement gate), a `GpuSafe` structural type-check on signatures and locals, call-graph checks (no recursion, no generic non-`#[gpu]` callees, no host-capturing closures), and effect enforcement from `#[gpu]` roots (including a no-explicit-panics rule).
- Unlike auto-par, GPU offload is never implicit — a deliberate divergence from Kāra's derive-it-automatically norm, accepted because GPU safety cannot be silently assumed.
- WGSL codegen is scaffolded as a spike (slice-0) and remains the open backend leg; the front-end gates landed first so unsupported constructs fail with honest diagnostics rather than miscompiling.

## Relationships
- **CONTRASTS_WITH**: [[auto-parallelization]]
- **REFERENCES**: [[gpu-compute-shaders]]
- **REFERENCES**: [[auto-parallelization]]

## Options

- Option A — infer GPU-eligibility automatically (mirror auto-par: derive which functions can/should run on the GPU from their effects and structure). Rejected: GPU eligibility rests on structural guarantees the user must consciously accept, and automatic promotion hides a large behavioral/precision boundary; the gate would surface as confusing rejections on ordinary code.
- Option B — an explicit `#[gpu]` marker that opts a function into the GpuSafe dialect and its enforcement. Chosen: the boundary is visible in source, enforcement diagnostics are scoped to marked code, and ordinary Kāra is unaffected.

## Notes

Realized incrementally: FE-1 (#[gpu] semantic marker), FE-2/FE-2b (GpuSafe on signatures/locals), FE-3a/3b/3c (recursion / generic-callee / host-capturing-closure rejection), FE-4/4b (effect enforcement + panic rejection), SL-1 (GpuSafe as a built-in structural marker trait).
