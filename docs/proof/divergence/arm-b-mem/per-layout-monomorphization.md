---
type: decision
created_date: 2026-07-15T17:34:19Z
last_modified: 2026-07-15T17:34:19Z
status: accepted
decided_on: 2026-06-19
deciders: Kāra maintainers
scope: subsystem
tags: data-layout, soa, codegen, monomorphization
---

# Per-Layout Monomorphization

## Decision
Carry a struct-of-arrays (SoA) data-layout choice across function call boundaries via per-layout monomorphization: a layout-carrying Vec[E] is lowered to a per-layout monomorphized function instance (keyed on a per-binding LayoutId), rather than reconciled at each call site through a name-keyed codegen map.

## Context
The original SoA implementation was name-keyed (a `soa_layouts` codegen map) and only worked WITHIN the declaring function. Passing a SoA-laid-out Vec[E] to another function miscompiled — by-value produced an LLVM call-signature mismatch (4-field SoA value into a 3-field AoS param), by-ref read the pointer's own bytes as the struct header (garbage length → SIGTRAP). The Slipstream LBM wind-tunnel dogfood splits its grid across `ref Vec[LbmNode]` helpers, so it could not use SoA at all — exactly the capability the roster billed it on. ADR: docs/spikes/per-layout-monomorphization.md.

## Consequences
SoA layouts now flow through by-value params, `ref`/`mut ref` params (multi-buffer, differing param names), return values (backward inference), carried-buffer reassignment, tail-call returns, and coroutine suspends — each argument layout producing a DISTINCT monomorph. The name-keyed by-value param ABI was retired, removing a footgun where a base param merely SHARING a name with a layout block lowered SoA on the name alone. Proven byte-identical AoS↔SoA by the Slipstream native oracle (framebuffer checksums) with the browser flagship running on SoA in headless Chrome. Follow-ons (whole-element / field-level SoA index-store, and String/Vec[POD] heap-field SoA elements) landed subsequently. Realized in [[data-layout-separation]].

## Relationships
- **REFERENCES**: [[data-layout-separation]]

## Options



## Notes

One of the seven committed design pillars (data layout separation); this decision is the cross-function completion of it.
