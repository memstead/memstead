---
type: decision
created_date: 2026-07-15T07:23:13Z
last_modified: 2026-07-15T07:23:13Z
status: accepted
decided_on: 2026-06-20
deciders: kara-maintainers
scope: component
tags: codegen, hashing, performance
---

# FxHash for Hash Codegen

## Decision
We chose FxHash as the algorithm for the generated `karac_hash_<T>` family, replacing FNV-1a. The switch was driven by a dedicated hash-quality benchmark (`bench/hash_quality`) that measured distribution and speed on Kāra's key shapes.

## Context
Map/Set codegen emits a per-type hash function. FNV-1a was the initial placeholder. A `hash_quality` benchmark was built to compare candidates on realistic key types (i64, char, tuples, unit-enums) before committing the swap.

## Consequences
- `karac_hash_<T>` now uses FxHash across all monomorphized and generic map/set paths.
- Faster hashing on small integer/char keys, which dominate the benchmarked workloads.
- The benchmark stands as the evidence and can be re-run if key-shape assumptions change.

## Options

- Keep FNV-1a — rejected: slower on the benchmarked key shapes.
- SipHash (DoS-resistant) — not chosen: the extra cost is unjustified for a compiled workload with no adversarial-key threat model.
- FxHash — chosen on benchmark evidence.

## Notes


