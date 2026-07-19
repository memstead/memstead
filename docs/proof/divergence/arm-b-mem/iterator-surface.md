---
type: spec
created_date: 2026-07-15T07:29:11Z
last_modified: 2026-07-15T19:07:31Z
level: M0
stability: stable
tags: stdlib, iterator, collections
---

# Iterator Surface

## Identity
Kāra's Iterator trait and its adaptor/terminal surface: a lazy value-iterator model implemented for collections, ranges, and slices, with the full adaptor family and terminal operations.

## Purpose
To give collections a uniform, composable, lazy iteration API in the Rust idiom — map/filter/fold and friends — across the interpreter and codegen.

## Relationships
- **REFERENCES**: [[standard-library]]
- **REFERENCES**: [[collections]]
- **PART_OF**: [[standard-library]]

## Realization

- runtime/stdlib/iterator.kara, into_iterator.kara, peekable.kara
- Interpreter: src/interpreter/method_call_iter.rs, iter_eval.rs; Value::Iterator plumbing
- Typechecker: src/typechecker/stdlib_iter.rs

## Specifies

- Adaptors: map, filter, flat_map, enumerate, take/skip, take_while/skip_while, chain, zip, step_by, cycle, inspect, scan, peekable/peek, chunk_by, chunks(n)/windows(n).
- Terminals: collect, fold, count, any, all.
- `iter()`/`into_iter()`/`next()`; for-loops consume Value::Iterator.
- Implemented for Range/RangeInclusive and Slice[T] (Phase 8), not just owned collections.


- Terminal codegen (this round): the fused-chain terminals now lower under `karac build` (previously interpreter-only), and the terminal set gained `sum` (typed numeric zero), `reduce` (`Option[A]`, scalar-payload codegen; heap-payload defers to `--interp`), and `for_each`. A `for x in xs.iter().map(...).filter(...)` loop lowers via a shared map/filter fusion (peel to a base source, thread the element, inline the body as the per-element sink); `count` on a chain desugars to a fold. A `let it = v.iter(); it.<terminal>` materialized-iterator binding and a `for x in it` over one both lower by inlining the recorded chain at each use. `|_|` wildcard closure params work in a chain.
- Loud-bail policy: an unlowered/unsupported for-loop adaptor (single-var `enumerate`, `zip`, `skip`/`take`/`chain` and the wider lazy-adaptor family, `iter_mut`) fails the build with an actionable message rather than silently skipping the loop body — the interpreter still runs them. `for (i, x) in xs.iter().enumerate()` (2-tuple destructure) is the supported enumerate form.

## Constraints

- Adaptors are lazy; terminals drive evaluation.

## Rationale

The 14-subtask Iterator trait surface bullet; a core part of the [[standard-library]] and [[collections]] ergonomics.
