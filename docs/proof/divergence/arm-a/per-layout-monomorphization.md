---
type: architecture
title: Per-layout monomorphization — SoA across function boundaries
updated_round: 6
---

# Per-layout monomorphization — SoA across function boundaries

A round-6 capability: making **struct-of-arrays (SoA) `layout` blocks cross function boundaries**. Kāra separates a struct's **logical shape** from its **physical layout**; a `layout` block declares an SoA physical layout for a `Vec[E]`, splitting element fields into **cache groups** (see [[design-data-layout]]). Before this round SoA layouts only worked **within** the declaring function — passing a SoA-laid-out `Vec` to another function miscompiled. ADR + spike in `docs/spikes/per-layout-monomorphization.md` (+429), recorded (042f156b); new compiler file `src/layout_queries.rs` (+454). Tracked as **B-2026-06-19-14** (a large multi-slice ledger entry).

## The problem

- A `layout` makes the binding a **4-field SoA value** `{group0_ptr, …, len, cap}`, but a callee's `Vec[E]` parameter was always compiled with the default **array-of-structs (AoS)** `{ptr, len, cap}` representation.
- **By-value** passing → LLVM `Call parameter type does not match function signature` (loud failure).
- **By-`ref`** passing → the callee GEP'd the pointer's bytes as the struct → garbage length → **SIGTRAP** (silent miscompile).
- The fix is **per-layout monomorphization**: a function is monomorphized per **argument layout**, keyed on a new **LayoutId** axis. See [[codegen]] for the monomorphization machinery.

## Six slices

- **Slice 1** (47f2813f) — **LayoutId axis scaffolding**: a new monomorphization axis alongside the existing type axes.
- **Slice 2** (9beaa258) — **forward arg-layout monomorphization**: a by-value SoA `Vec[E]` crosses a call boundary regardless of parameter name, routed to an on-demand layout monomorph keyed on the **caller's argument layout**.
- **Slice 3** (13be3254) — **SoA return values** via **backward inference**: the return layout is keyed off the receiving `let recv = f()` binding.
- **Slice 4** (238e1388) — **multi-buffer / differing-name kernels** for the `ref` / `mut ref` borrow forms; multiple SoA buffers of one element type flow through shared by-ref helpers, each a distinct monomorph.
- **Slice 5** (8ad88b24) — **origin-only `soa_layouts`** via a per-binding **LayoutId carrier**: the access-path trigger reads a per-binding LayoutId value rather than re-deriving SoA-ness from the binding **name**. Retired a footgun where a base parameter merely **sharing a name** with a layout block lowered SoA on the name alone.
- **Slice 6** (93c8823c, marked COMPLETE in 71c63b8c) — **cross-function completeness**, proven by the **Slipstream full-SoA dogfood** (an LBM wind-tunnel simulation, `examples/slipstream/`, see [[examples-and-benchmarks]]). The proof surfaced and fixed five more cross-function gaps:
  - `with_capacity` SoA constructor
  - returned-local base-symbol clash
  - SoA reassignment `grid = substep(grid, …)`
  - tail-CALL SoA-return propagation
  - SoA across a **coroutine suspend** for the browser render loop
  - The native oracle's framebuffer checksums are **byte-identical** between AoS and SoA.

## P1.5 layout-choice query

A companion compiler query **P1.5 layout-choice query** (7018ec31) surfaces **struct-of-arrays opportunities** to agents/users (`src/layout_queries.rs`) — part of the AI-first query surface (see [[design-ai-first-compiler]], exposed via [[cli]]).

## Follow-ons landed later in the round

- **Whole-element SoA index-store** `grid[i] = E{…}` (61e5a5c6). Field-level index-store fix is **B-2026-06-20-7**.
- **Branch-leaf / multi-`return` SoA returns** (22e2f35f).
- **SoA elements with String/Vec heap fields** (94ecb380, **B-2026-06-20-18**): per-element heap-field drops across the push/store/cleanup lifecycle.
- **Residual**: reading a heap SoA field back as a method receiver / index base needs the field's **address** — deferred to the next slice (see [[bug-tracker]]).

Related: [[design-data-layout]], [[codegen]], [[examples-and-benchmarks]], [[design-ai-first-compiler]], [[cli]], [[bug-tracker]], [[implementation-phases]], [[index]]
