---
type: spec
created_date: 2026-07-15T07:29:30Z
last_modified: 2026-07-15T07:33:31Z
level: M0
stability: experimental
tags: codegen, performance, collections, phase-7
---

# Monomorphized Collection Codegen

## Identity
A codegen optimization that emits type-specialized (monomorphized) machine code for hot Map instantiations — e.g. Map[i64,i64] and Map[char,i64] — with inline probe loops instead of routing through the generic KaracMap dispatch.

## Purpose
To close the performance gap between Kāra's generic collection dispatch and hand-written specialized code on the integer/char key shapes that dominate real workloads.

## Relationships
- **REFERENCES**: [[llvm-codegen-backend]]
- **REFERENCES**: [[fxhash-for-hash-codegen]]
- **PART_OF**: [[llvm-codegen-backend]]

## Realization

- src/codegen/mono.rs (generic monomorphization + mono Map specialization)
- KaracMap layout exposure (runtime/src/map.rs) with direct-field len
- docs/implementation_checklist/phase-7-codegen.md (monomorphized-collections record)

## Specifies

- Monomorphized Map[i64,i64]: scaffolding, len dispatch, insert_old symbol + inline fast-path probe loop, get with inline probe.
- Generalized (K,V) mono dispatch; Map[char,i64] with char lowering to LLVM i32 in the type-name path.
- Trait-bounds verification at monomorphization request; call-site bound discharge.

## Constraints

- Specialized paths must be observably identical to the generic path.
- Benchmark findings were mixed for Map[char,i64] — recorded, not assumed.

## Rationale

Phase-7 optimization within the [[llvm-codegen-backend]]; paired with the [[fxhash-for-hash-codegen]] hash swap.
