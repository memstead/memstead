---
type: design-decision
title: Data-layout separation
updated_round: 6
---

# Data-layout separation

Kāra separates a struct's **logical shape** from its **physical memory layout**. The
programmer writes a struct as a logical record; the physical layout (including
**struct-of-arrays / SoA**) is **opt-in** and independent of the logical definition.

- Declared as a first-class language feature in the original redesign (CHANGELOG).
- Parser support: `layout` items, with case-class checks for layout names.
- Chapter 15 of the language book is "Data layout".

This lets performance-oriented memory layouts (e.g. SoA for cache-friendly iteration) be
adopted without changing the logical data model.

**Round 6 made SoA `layout` blocks cross function boundaries** via
**[[per-layout-monomorphization|per-layout monomorphization]]** — a function is
monomorphized per argument layout on a **LayoutId** axis, so an SoA `Vec[E]` flows through
by-value / by-`ref` / return boundaries as a distinct monomorph (proven by the
[[examples-and-benchmarks|Slipstream]] full-SoA LBM dogfood). Before this, SoA layouts only
worked within the declaring function. A companion **`karac query layout`** (P1.5) surfaces
struct-of-arrays opportunities. See [[per-layout-monomorphization]], [[codegen]], [[cli]].

The round-5 **[[numerical-stdlib-and-tensors|`Tensor[T, Shape]`]]** type is a separate,
purpose-built numerical layout — its **shape is carried in the type** (via `Dim`/`Shape`
generic kinds) and it has its own dense codegen layout, distinct from the SoA layout system
here.

Related: [[codegen]] (which lowers concrete layouts), [[design-adt-and-pattern-matching]],
[[numerical-stdlib-and-tensors]], [[per-layout-monomorphization]].
