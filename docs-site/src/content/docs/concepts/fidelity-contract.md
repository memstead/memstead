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

`memstead projection verify <binding>` computes that measurement. It never mutates
the destination — it reads the store, the anchors, and the source, records durable
findings, and renders a report. Repair is a separate operation (`sync`); verify only
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

Two capabilities are **deferred this cycle** — a positioning decision, stated plainly
rather than left as a silent gap or dressed up as an imminent feature:

- **Web-medium sync and enumeration.** A `web` medium can be named and read, but the
  engine does not enumerate or maintain it. Because its capability row advertises no
  enumeration and no retrievable base, **binding validation refuses** a sync or
  enumeration-dependent operation against a web medium — with a remedy-bearing error,
  at declaration time, not at run time.
- **Preparation of non-text media (e.g. PDF).** A facet may declare a preparation
  step (PDF→markdown, audio→transcript), but no preparation implementation ships
  today. A facet that declares one its medium cannot support is **refused at binding
  validation** with a capability error naming the unsupported operation — again at
  declaration time, so a workspace never discovers the gap mid-run.

Neither is a roadmap promise, and neither is omitted: the honest shape of a contract
is to name its own edges. When these land, they will enter the capability matrix the
report already renders, and the refusals will become measurements.
