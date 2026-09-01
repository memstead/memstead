---
title: Keep a mem current
description: "The whole maintenance recipe on one page: choose the binding, ingest until the stock-take is clean, run /sync on a loop forever, run /remodel occasionally. Each step ends at a checkable criterion, not a feeling."
sidebar:
  order: 2.5
---

A mem is a typed model of a source, and keeping it current is not one
activity but four, each with its own command and its own definition of
done. This page is the whole recipe. Everything else in these docs is
detail behind one of its steps.

The short version:

1. **Choose the binding** (once): declare which source belongs in which
   mem, and on what terms.
2. **Ingest** (until clean): build the mem from the source, batch by
   batch, until every source file is accounted for.
3. **Sync** (forever, on a loop): keep every statement true as the
   source moves.
4. **Remodel** (rarely, on signals): keep the structure right, not just
   the sentences.

## 1. Choose the binding

A binding is the contract: *this source belongs in that mem*, with a
declared scope, deny paths, and coverage semantics (exhaustive: every
file in scope must eventually be accounted for; curated: only what you
choose). Creating one reads nothing.

```bash
memstead projection init --mem my-graph --source ../some-repo --medium-type codebase
```

Everything after this step is measured against the contract you wrote
here, so spend a minute on the scope: paths that can never be
knowledge (vendored dependencies, build output) belong in `deny_paths`
now, not in a hundred exclusions later.

**Done when:** `memstead projection verify <binding>` runs and reports
a denominator: the engine can enumerate what the contract covers.

## 2. Ingest until the stock-take is clean

The ingest loop fills the mem: an agent session asks the engine for a
batch of unworked source files, reads them, writes entities for what
they owe (each write carrying an **anchor**: the receipt naming which
files the entity accounts for), records honest exclusions for files
that are not knowledge (test suites, lockfiles), and reports the batch
done. The engine keeps the cursor, so the loop stops and resumes
without losing its place. The
[grow-a-mem guide](../grow-a-mem-from-a-source/) runs this end to end.

```
/loop /memstead:ingest <binding>
```

**Done when:** `memstead projection verify <binding> --full` shows
zero uncovered artifacts: every in-scope file either has an owning
entity (via its anchor) or a recorded exclusion with a rationale. Not
a feeling; a list at zero. This is the one expensive step, and it is
paid once.

## 3. Sync on a loop, forever

From here on, maintenance is one line:

```
/loop /memstead:sync --all
```

Each round asks the engine what changed since the last one, repairs
only the affected entities (claim by claim, conservatively: an
ambiguous change is skipped and left as an open finding, never
guessed), lets new files ride in as small build batches, and advances
the baseline. The loop ends itself when every binding reports
quiescence, so running it "too often" costs almost nothing.

Two companions live on the same command: `--verify <binding>` renders
the fidelity report without writing anything, and `--sweep <mem>`
walks a mem's standing claims even where no change signal points,
leaving a check record per entity so the next sweep knows what is
freshly verified.

**Done when:** never. This is the steady state. The observable is the
verify report staying clean and the loop's rounds staying short.

## 4. Remodel occasionally

Sync keeps sentences true; it never asks whether the mem is still
*cut* right: whether every obligation of the subject has exactly one
home entity of the right type, whether substance sits in its declared
sections, whether the graph is wired. Organic growth degrades that
silently even while every sentence stays true.

```
/loop /memstead:remodel --all
```

A cheap signal scan walks every mem first and descends into the
expensive round only where signals justify it: type distributions
collapsing, large source files owned by nobody, entities without
edges, definition-test sections sitting empty. Most runs report
"healthy" within minutes and end: that is the answer working, not the
tool failing. When a round does fire, it derives a target inventory
from contract plus source, has it adversarially checked, and rebuilds
conservatively, with big rebuilds bracketed by a before/after
serviceability probe.

**Done when:** the scan reports no cluster whose signals justify a
round. A sensible cadence is per release, or whenever a subsystem was
restructured at the source.

## The one failure mode worth knowing

If step 2 was never finished (or the mem predates anchors), the verify
report shows files as uncovered even though the knowledge exists in
prose: the gap is receipts, not content. The repair is cheap: anchor
the files to the entities that already describe them, and exclude
what is not knowledge. Everything in steps 3 and 4 relies on those
receipts, which is why step 2's criterion is a hard gate, not a
formality.
