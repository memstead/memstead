---
type: memo
created_date: 2026-07-15T07:25:37Z
last_modified: 2026-07-15T10:57:12Z
status: active
tags: plan, lesson, deferred
---

# Deferred Work Tracker

## Claim
A running ledger (docs/deferred.md) records design and implementation work deliberately postponed or empirically ruled out, so falsified approaches are not re-attempted and blocked slices are resumed when their prerequisite lands.

## Context
- Kāra is built in small vertical 'slices'; some slices hit blockers or negative results that must be remembered.
- The tracker is append-friendly and cross-referenced from phase checklists.

## Relationships
- **REFERENCES**: [[unsafe-and-ffi-surface]]

## Substance

- `Vec.sort_by` FFI-boundary cost: Path A+ (FFI-boundary comparator inlining) was measured and rejected (2026-05-14), but the goal shipped another way — Slices 6.1 + 6.4 monomorphize sort_by (inline-closure fast path for Vec[i64], plus a tuple/struct fast path with runtime length dispatch), so the FFI-boundary comparator cost is now avoided. Bounds-check elision via `llvm.assume` (brainstorm v68) — negative result, archived.
- Blocked-on-prerequisite: `Vec.get_unchecked` source-level bounds-check elision was blocked on the [[unsafe-and-ffi-surface]] enforcement predecessor before it could land.
- Recursive Drop slices α/β/γ closed with a documented tuple-destructure blocker; residual `bfs_sieve` leak attributed to two further gaps and deferred.
- Vec.new + push-loop → `Vec.filled` lowering deferred as a phase-7 optimization.

- Alias-scope metadata filed as deferred: measured ~0 runtime gain, ~132 B/kernel size cost only. `mut ref` parameters do get `noalias` emitted (from the exclusive-borrow guarantee), but parameter `noalias` beyond that was measured inert.
- Small-string optimization (SSO) scoped as a campaign — a corpus-wide allocation lever — rather than a point fix (docs/spikes/small-string-optimization.md); non-temporal-store levers filed alongside.
- Self-hosted-lexer profiling resolved: string-literal `match` dispatch is the #1 real-world codegen lever and shipped; the noalias/autovec spike resolved.

## Alternatives



## Outcome

- Keeps negative knowledge durable so the same dead ends are not re-explored.
- Each entry is resolved by either a landing slice or a permanent 'won't do' with the measurement that killed it.

- Resolved: the `Vec.get_unchecked` + source-level bounds-check elision slice, formerly blocked on the [[unsafe-and-ffi-surface]] predecessor, has now landed (for-range and Slice[T] read/write elision) once that enforcement predecessor shipped.

- Resolved: an intermittent codegen-suite hang traced to user-vs-seeded-enum disambiguation (fix now covers Json + TcpError); a per-spawn hang watchdog on the Command::output() e2e guards against recurrence.
- Roman-kata codegen investigation filed with deferred follow-ups; kata Slice-4 codegen gaps (Vec.remove, uppercase-receiver dispatch, Vec prefix-literal, byte/range match patterns) surfaced and since shipped.
