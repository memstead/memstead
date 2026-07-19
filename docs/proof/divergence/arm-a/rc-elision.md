---
type: architecture
title: RC elision — count-free reference-counted chains
updated_round: 5
---

# RC elision

**New in round 5.** RC elision is a codegen/ownership optimization that **removes
reference-count operations** where the compiler can prove a reference-counted value's
lifetime statically — the performance backbone of round 5's idle-connection-density
[[examples-and-benchmarks|benchmark leadership]]. It complements, and reduces the cost of,
the [[design-ownership|RC fallback]]. The analysis is a large new module,
`src/ownership/elision.rs` (+3521), with `tests/elision.rs` (+1735).

The design was **locked as "Slice 2", phased A → C** (plus a later D), then implemented as
a ladder of increasingly-general cluster analyses.

## The phase ladder

- **Phase A** — trivial intra-fn single-owner shared bindings free **without count ops**.
- **Phase B1** — **append-only chain clusters**: a root drop becomes a **link-following
  free-walk** (walk the chain and free, no per-node counts).
- **Phase B2** — build-side count ops elided for **displacement-free** chain clusters.
- **Phase C1a** — member-type params coexist with clusters (**param walls**).
- **Phase C1b** — **fresh-return cluster summaries** (builders hand off count-free chains).
- **Phase C1c** — **caller adoption** (fresh-return results drop via an option-guarded
  free-walk).
- **Phase C2a** — **borrowed-param walk families** (count-free param-chain walks).
- **Phase C2b** — **program-wide headerless-T**: 16-byte nodes across call boundaries — this
  **completes the C ladder**.
- **Phase D** — **headerless cluster members**: 16-byte nodes with **no rc word at all**. A
  C-phase probe confirmed the lever (**+21% from removing the 8-byte header**), so the
  design was locked.

## Alias metadata (adjacent, mostly measured inert)

Round 5 also probed LLVM alias metadata as an independence-exploitation lever
(`docs/spikes/independence-noalias-ilp.md`):

- **`noalias` on `mut ref` parameters** — emitted from the exclusive-borrow guarantee
  (landed).
- **Param `noalias`** — measured **inert** (no runtime gain); follow-ons re-prioritized.
- **Alias-scope metadata** — filed as **deferred**: measured **~0 runtime gain, ~132 B per
  kernel size cost only** (see [[deferred-work]]).

Related: [[design-ownership]], [[codegen]], [[examples-and-benchmarks]], [[deferred-work]].
