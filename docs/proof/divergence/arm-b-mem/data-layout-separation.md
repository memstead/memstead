---
type: spec
created_date: 2026-07-15T07:26:21Z
last_modified: 2026-07-15T17:32:20Z
level: M1
stability: experimental
tags: data-layout, performance, memory
---

# Data Layout Separation

## Identity
Kāra's separation of a type's logical struct definition from its physical memory layout, allowing opt-in layout transforms such as struct-of-arrays (SoA) without changing the logical API.

## Purpose
To let programmers write ordinary struct code while independently choosing a cache- or SIMD-friendly physical layout, so data-oriented performance does not force an awkward source shape.

## Relationships
- **REFERENCES**: [[backend-first-v1-positioning]]
- **REFERENCES**: [[per-layout-monomorphization]]

## Realization

- Layout declarations in the parser/AST (layout items) and the `#[layout]` attribute family
- src/codegen/types_lowering.rs for physical lowering
- design.md 'Data layout separation' chapter; docs/book ch15-data-layout.md

- SoA codegen paths in src/codegen (index-read, field-access, whole-collection drop); resolver heap-bearing-field rejection

## Specifies

- A logical struct and one or more physical layouts for it.
- Opt-in SoA (struct-of-arrays) as the primary alternative layout.
- Layout names are case-checked (parser case-class checks for layout names).

- SoA codegen: index-read and `entities[i].field` field access; SoA `pop` / `pop_back` / `pop_front` / `remove`; whole-collection drop frees every group buffer. The resolver now ADMITS String + Vec[POD] SoA element fields (with per-element heap-field drops synthesized across push / store / cleanup / reassignment), while still rejecting Map/Set/VecDeque/Sorted*/Vec[heap]/shared element fields.


- **Per-layout monomorphization is COMPLETE**: SoA layouts now cross function boundaries. A layout-carrying Vec[E] flows as a 4-field SoA struct through by-value params, `ref`/`mut ref` params (multi-buffer, differing param names — each a distinct monomorph), SoA return values (backward inference off the receiving binding), carried-buffer reassignment (`grid = substep(grid,..)`), tail-call return propagation, and even across a coroutine suspend. The name-keyed model was retired for a per-binding LayoutId carrier (origin-only soa_layouts). Whole-element index-store `grid[i] = E{..}` and field-level index-store `vec[i].field = expr` both land (the latter was a silent heap-overflow before it was fixed).

## Constraints

- Layout choice must not change the observable logical semantics of field access.

## Rationale

One of the seven committed design pillars from the CHANGELOG; supports the data-processing 'bonus' framing in [[backend-first-v1-positioning]].


The cross-function design is the per-layout monomorphization ADR ([[per-layout-monomorphization]]); proven end-to-end by the Slipstream LBM wind-tunnel dogfood (native oracle framebuffer checksums byte-identical AoS↔SoA, flagship runs on SoA in a real browser).
