---
title: The fidelity contract
description: "What a verify run measures — coverage, accuracy, and freshness relative to a declared binding — how it reports honestly, and what Memstead deliberately does not measure yet."
sidebar:
  order: 2
---

A [projection](/glossary/#pipeline-medium--facet--projection) binds a source — a
codebase, a filesystem, another mem — to a destination mem and populates it. The
**fidelity contract** is the promise the engine makes about that mem afterward:
it will tell you, deterministically and without inventing numbers, how faithfully
the mem still reflects the sources its binding declares.

`memstead projection verify <binding>` computes that measurement. It mutates no
entity in the destination — it reads the store, the anchors, and the source, records
durable findings, and renders a report. It is not a pure read, though: see
[Verify writes](#verify-writes) below. Repair is a separate operation (`sync`); verify only
measures.

## What the contract measures

Fidelity is reported as three things, always relative to a **declared binding** — a
mem is never judged against an absolute ideal, only against what its own binding
says it should contain:

- **Coverage** — how much of the source in scope is accounted for in the mem. The
  denominator is the per-medium enumeration of the source set, `S(D)`; the report
  states that provenance rather than quoting a bare percentage.
- **Accuracy** — of the content that *is* anchored back to a source, how much still
  matches that source (anchor resolution).
- **Freshness** — how the mem's recorded baselines compare to the current state of
  the source it was built from.

The report never blends these into a single score. Each is stated on its own, with
the honest denominator behind it.

## Coverage is grain-weighted, never laundered

Anchors come at different grains. A file anchor covers one file; a **tree anchor**
covers a directory that may fan out over hundreds of files. Folding a one-entity,
two-hundred-file tree anchor into a flat "coverage %" would make a sparsely-anchored
mem look exhaustively covered.

So the report keeps tree-anchor fan-out on its **own axis** — a tree anchor shows as
one anchor fanning out over N files, never merged into the file-level percentage. A
reader sees direct file/span coverage and tree fan-out as two separate facts, and
can judge for themselves how much confidence the tree anchor earns.

## Provenance classes, and what is excluded

Every anchored entity carries a **provenance class** that says how it came to
reference its source:

- **anchored** — a hash-bearing anchor to a specific source artifact; its content
  can be checked against the source.
- **derived** — produced from the source but without a byte-level hash anchor.
- **authored** — a human or agent wrote it directly; the source *informed* the
  author but does not own the bytes.
- **informed-by** — the artifact shaped the entity without being reproduced in it.

`authored` content is **excluded from the coverage and accuracy denominators** and
reported as its own bucket. Measuring authored prose against a source it was never
meant to reproduce would manufacture false drift; the contract refuses to do that,
and says so where the excluded bucket appears.

## Three tiers of scrutiny

Not every check costs the same, so verification is layered — cheap deterministic
measurement first, expensive judgment last and only under a budget:

1. **Deterministic measurement** — coverage, anchor resolution, and freshness are
   computed on every verify with no judgment call. This is the tier-1 fidelity
   report.
2. **Hash adjudication** — for hash-bearing anchors over a `stable` medium, the
   prepared-content hash is compared to the recorded one. A mismatch is a `drifted`
   finding. Still deterministic, still no model call.
3. **Sampled deeper adjudication** — mismatches a hash cannot settle are adjudicated
   under a per-run cap, sampled on a rotation so no artifact is starved, with a
   level-triggered full walk that guarantees eventual coverage of an enumerable
   source. Whatever the cap defers is queued as the **adjudication backlog**, and its
   depth is reported — deferred work is visible, never silently dropped.

## The report leads with a verdict

The report is engine-rendered and deterministic — no model call, so two runs over
the same state produce byte-identical output. It opens with a **rollup verdict** and
the top concrete actions ("3 entities describe deleted code — run sync"), with the
underlying numbers available as drill-down. An operator reads the verdict; an agent
reads the actions; both come from one computation.

Where a medium cannot support a measurement, the report says so as a **degradation**
rather than faking a green result. A medium with no change signal renders freshness
as "unknowable," and a green freshness verdict is structurally unreachable for it —
the contract would rather admit a blind spot than paper over one.

## What the contract does not cover yet

Four caps bound what the contract can currently claim — positioning decisions and
known gaps, stated plainly rather than left silent or dressed up as imminent
features. If you are gating a pull request on verify ([the CI
guide](/guides/verify-in-ci/)), these are the edges of what the gate can see:

- **Web-medium sync and enumeration.** A `web` medium can be named and read, but the
  engine does not enumerate or maintain it. Because its capability row advertises no
  enumeration and no retrievable base, a sync or enumeration-dependent operation
  **cannot be declared** against a web medium: `projection init` scaffolds the binding
  build-only and says which operations it dropped and why; `projection enable sync`
  refuses outright; and a record that carries one anyway is refused at validation.
  Asking a web binding to sync therefore names the capability gap itself, never a
  remedy no medium can honour.
- **Preparation of non-text media (e.g. PDF).** A facet may declare a preparation
  step (PDF→markdown, audio→transcript), but no preparation implementation ships
  today. A source that declares one is **skipped at run time**: the record is
  accepted at declaration, and the run that would consume it reports the
  unsupported preparation and exits without doing work. The gap is named when
  you hit it, not when you declare it — which is the honest description of
  where the edge currently sits.

- **The mtime `#synced` baseline does not survive a fresh checkout.** A binding using
  mtime change-detection compares modification times against a recorded baseline, and a
  CI clone gives every file a new mtime, so the baseline is meaningless there. This
  bounds the *changed-source slice* — what `sync` acts on. `verify` adjudicates anchors
  by content hash, so a bumped-mtime checkout still verifies clean; the cap is real for
  the loop, milder for the gate. Use git change-detection for anything you intend to
  gate.

The first two are **positioning decisions** — deliberate boundaries, not
roadmap promises. The last is a **known defect with a fix planned**: the
mtime baseline's non-portability is a portability bug, not a choice. The
distinction matters to anyone deciding what to build a gate on — a boundary
will still be there next year; a defect should not be. When the defect is
fixed, its refusal becomes a measurement and the paragraph retires.

Graph-medium verify was on this list until it started measuring: a
graph-source binding now enumerates the source mem's in-scope entities as a
real denominator and resolves its entity anchors against the live graph, so
a stale-pinned anchor over a changed source entity reports `drifted` like
any other.

None of the four is omitted: the honest shape of a contract is to name its own
edges, and to say which of them it intends to keep.

## Verify writes

Verify is **not a read-only command**, and no part of this contract should be read as
saying it is. A completed run records its findings store, backfills observed
content hashes onto hash-less anchors in the mem's anchors sidecar, and records a
`#verified` baseline — on a mem-repo-backed mem, that last one is a commit. The
measurement pass itself takes a shared engine borrow and is structurally incapable of
mutating an entity; what it writes is measurement bookkeeping, and a failed or aborted
run never advances the baseline. On CI's ephemeral checkout this is harmless. In a
working tree you need pristine, it is not.
