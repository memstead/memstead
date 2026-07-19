---
type: spec
created_date: 2026-07-15T10:52:56Z
last_modified: 2026-07-15T10:52:56Z
level: M1
stability: evolving
tags: ownership, rc, optimization, phase-7
---

# Reference-Counting Elision

## Identity
The Kāra RC-elision optimization: a static analysis that removes reference-count operations — and, at its endpoint, the per-node rc word itself — for shared bindings whose ownership follows provable single-owner or append-only chain-cluster shapes.

## Purpose
To make the [[tiered-ownership-model]]'s RC fallback nearly free where the shape allows it, so reaching for `Rc` no longer implies paying atomic/count traffic on every hand-off.

## Relationships
- **PART_OF**: [[ownership-checker]]
- **IMPLEMENTS**: [[tiered-ownership-model]]
- **REFERENCES**: [[tiered-ownership-model]]
- **REFERENCES**: [[ownership-checker]]

## Realization

- src/ownership/elision.rs (the elision analysis), src/ownership/rc_promote.rs
- tests/elision.rs
- docs/implementation_checklist/phase-7-codegen.md (RC-elision design record)

## Specifies

- Phase A: trivial intra-fn single-owner shared bindings free without count ops.
- Phase B: append-only chain clusters — root drop becomes a link-following free-walk (B1); build-side count ops elided for displacement-free chain clusters (B2).
- Phase C ladder: member-type params coexist with clusters via param walls (C1a); fresh-return cluster summaries (C1b); caller adoption of fresh-return results via option-guarded free-walk (C1c); borrowed-param walk families / count-free param-chain walks (C2a); program-wide headerless-T — 16-byte nodes across call boundaries (C2b, completing the C ladder).
- Phase D: headerless cluster members — 16-byte nodes with no rc word.
- Interacts with the niche call ABI for `Option[shared T]` (walk-cursor refcounts, unwrap alias, RC-fallback boxing).

## Constraints

- Elision applies only where the ownership shape is provably single-owner or an append-only cluster.
- Headerless nodes drop the rc word only for cluster members the analysis proves never need an independent count.

## Rationale

A Phase-7 optimization layered onto the [[ownership-checker]]. Design locked as Slice 2, phased A→D; the phase-D headerless lever was confirmed by a phase-C probe (+21% from removing the 8-byte header). Complements the [[tiered-ownership-model]]'s existing budget controls and cost summary.
