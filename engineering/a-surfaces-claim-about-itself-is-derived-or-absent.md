---
type: principle
created_date: 2026-08-15T04:19:43Z
last_modified: 2026-08-15T04:19:43Z
authority: accepted
universality: domain-wide
tags: docs, generated-surfaces, drift, versioning, compatibility
---

# A surface's claim about itself is derived or absent

## Statement
When a surface states a fact about itself or about the artifact it describes — a count, a version, a revision, a compatibility claim, a date — that statement is DERIVED from the thing it describes, or it is not made at all. Correcting a hand-written number is never the fix; it only resets the clock on the same drift.

A derived statement that cannot be produced fails the build. A placeholder that reads like data (`dev`, `unbuilt`, `TBD`) is worse than an absent line, because a reader cannot tell it apart from an answer.

## Scope
Self-descriptive facts on any published surface: docs pages, generated reference, package README and metadata, receipts, and the machine twins of those. It covers facts a page states about a NEIGHBOURING artifact too — which version of a library reads which format, how many skills a plugin ships — because those drift by the same mechanism.

It does not govern prose judgement (what a thing is for, why it exists), only checkable facts. The distinguishing question: could this sentence become false without anyone editing it?

## Justification

The 2026-08-14 cold-start run found three instances at once on the same site, and the site's own pitch is that its pages are generated deterministically from the live source on every push. The hand-written index said "the eight-skill plugin roster" and linked to a generated page that said six — a page which opens by promising it cannot drift from the installed plugin, which is true of that page and was not true of the index linking to it. Every footer read `Generated from dev on unbuilt`, so the one line that would tell a reader WHICH revision they were reading told them nothing — and the run hit two version-skew problems it could not resolve for exactly that reason. And neither crates.io nor npm stated which CLI generation the published library reads, so the only way to find out was to install both and write a client against whatever came out.

The unifying failure is not carelessness. A number a human maintains is a number that will eventually be wrong, and a product whose claim is write-time validation against drift should not be the thing that drifts.

## Exceptions

A captured transcript may legitimately show values from the moment of capture rather than the current build — but it must then say what was refreshed and what was not, rather than presenting an edited capture as verbatim.

Placeholders are acceptable where they are unmistakably placeholders to the reader (`<scope>/<name>` in a command shape), which is the opposite case: a word no one could mistake for data.

## Consequences

The fix for a hand-written fact that contradicts a generated one is to generate it or delete it. Deleting is usually right and always cheap — the generated surface is one link away and is never wrong.

Where a fact is deleted rather than generated, a build step asserts it stays deleted: the docs prebuild fails, naming file and line, if any hand-written page states a skill count at all.

A derived claim needs a source of truth that already exists in the build. The archive compatibility table works because `PUBLISHED_MEM_FORMAT` and `PUBLISHED_MEM_FORMATS_ACCEPTED` are the same constants the reader enforces at load time — the statement cannot disagree with the behaviour without the behaviour changing. Where no such source exists, the honest move is to ship nothing and say so.
