---
title: The fidelity contract
description: "What a verify run measures — coverage, accuracy, and freshness relative to a declared binding — how it reports honestly, and what Memstead deliberately does not measure yet."
sidebar:
  order: 2
---

A [projection](../../glossary/#pipeline-medium--facet--projection) binds a source — a
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

## The population a report answers for

A mem can carry several bindings, and the anchors in it are not all the same
binding's business. A report answers for exactly one population: the anchors the
binding under verification is responsible for. Membership is decided by what the
anchor records about its own origin where it records one, and by the binding's
declared scope where it does not, so an anchor pointing at an artifact this
binding's scope does not cover is not counted against it.

Excluded anchors are **named, never dropped**. The report states how many were
excluded because another binding wrote them and how many because they sit outside
this binding's scope, and lists the artifacts. Exclusion is a reporting decision:
nothing is deleted, rewritten, or marked invalid, and an excluded anchor cannot
raise a finding against a binding that does not answer for it.

The report also states **what its denominator counted**. One artifact legitimately
carries several anchors at different grains or classes, so the row count and the
distinct-artifact count are printed side by side rather than the rows being merged
into one figure a reader would misread as artifacts.

Anchors written before anchors recorded their producing binding carry no such
record. Those are **counted**, not discarded: filtering them out strictly would
empty the axis for every mem written before the field existed, and an empty report
reads as success. The report says how many anchors it counted on that basis, so a
population established by provenance is distinguishable from one resting on the
fallback.

## An anchor has two ends

An anchor ties an entity to a source artifact, and the measurement above checks
the artifact end: does the source still say what was recorded. The other end is
the entity itself, and it can vanish. The engine's own delete and rename paths
take an entity's anchors with it, so this only happens when something writes the
mem from outside: a sibling process, a branch reset, a file removed by hand, an
archive installed over the top.

A sidecar row naming an entity the mem no longer holds is reported as
**dangling**, on the binding report and on the standalone anchor surface alike.
It is not one of the four states, which describe the artifact end: a vanished
entity says nothing about the source, and the repair is not the same one. An
orphaned anchor asks whether its entity should be re-anchored or pruned; a
dangling row asks why the entity went missing. It counts toward no figure, so it
can no longer resolve at a hundred percent for an entity that does not exist,
and it does not make an artifact count as covered.

Nothing repairs it. Deleting the row would tidy the sidecar and erase the only
remaining evidence that something wrote the mem behind the engine's back.

The check reads an absence from the loaded graph, which is only evidence when
the graph holds everything the mem has. Where it does not (a mem that is not
mounted, one that is quarantined, one whose lazy load has not run, or one with
a file that failed to parse) no row is called dangling and the report says why
instead. A clean anchor axis over an entity end nobody examined would be exactly
the false assurance this contract exists to prevent.

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

Three caps bound what the contract can currently claim — positioning decisions and
known gaps, stated plainly rather than left silent or dressed up as imminent
features. If you are gating a pull request on verify ([the CI
guide](../../guides/verify-in-ci/)), these are the edges of what the gate can see:

- **Web-medium sync and enumeration.** A `web` medium can be named and read, but the
  engine does not enumerate or maintain it. Because its capability row advertises no
  enumeration and no retrievable base, a sync or enumeration-dependent operation
  **cannot be declared** against a web medium: `projection init` scaffolds the binding
  build-only and says which operations it dropped and why; `projection enable sync`
  refuses outright; and a record that carries one anyway is refused at validation.
  Asking a web binding to sync therefore names the capability gap itself, never a
  remedy no medium can honour.
- **Preparation is a registry, and it ships three flavours.** A source's
  `preparation` names a preparation the engine registers: `entity-load-bearing`
  for graph sources (an entity anchor hashes its type's load-bearing sections,
  so a notes-only edit does not drift a dependent), `dated-entries` for
  path-shaped sources (a file of dated entries is delivered as units
  `<path>#<stamp>` in stamp order, identical on every pass, and a unit anchor
  drifts only when its own entry changes), and `code-map` for code sources (a
  file anchor hashes the file's interface digest and a tree anchor the code map
  of the scoped files under it, so an implementation edit does not drift an
  anchor and a signature change does, within the digest's recorded limits; the
  digest is heuristic, by language family). A tree anchor on a source without a code map is recorded
  but never hashed: it resolves `recheck`, not drift.
  Non-text media conversion (PDF, DOCX, audio) is a non-goal: an agent with a
  capable read tool extracts, and the prepared-content hash already falls back
  to a raw-byte digest for a binary artifact, so drift over a PDF is detected
  without any preparation. An identifier the registry does not know is refused
  at declaration on every edit path; a hand-edited record carrying one is
  skipped at run time with the registered set named, never run over content the
  engine cannot prepare. The registered set is listed in the
  [binding reference](../../reference/binding/).

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

None of the three is omitted: the honest shape of a contract is to name its own
edges, and to say which of them it intends to keep. The list shrinks when a cap
is fixed rather than when it is quietly dropped — graph-medium verify left it
that way.

## Verify writes

Verify is **not a read-only command**, and no part of this contract should be read as
saying it is. A completed run records its findings store, backfills observed
content hashes onto hash-less anchors in the mem's anchors sidecar, and records a
`#verified` baseline — on a mem-repo-backed mem, that last one is a commit. The
measurement pass itself takes a shared engine borrow and is structurally incapable of
mutating an entity; what it writes is measurement bookkeeping, and a failed or aborted
run never advances the baseline. On CI's ephemeral checkout this is harmless. In a
working tree you need pristine, it is not.
