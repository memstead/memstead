---
type: decision
created_date: 2026-08-19T02:24:11Z
last_modified: 2026-08-19T02:24:11Z
status: accepted
decided_on: 2026-08-19
deciders: operator (bundle decisions 26+29, backlog-sweep README), implemented in plan 03a
scope: subsystem
tags: anchors, provenance, resolution, dialect, projection-pipeline
---

# Anchor artifact paths speak the source dialect and write time resolves or refuses

## Decision
An anchor's artifact path is SOURCE-relative first: at resolution and at write time, the path joins onto the declaring source's `pointer` (looked up by the anchor's `source` name in the mem's bindings) before touching the filesystem — out-of-root pointers included, the joined path may deliberately leave the workspace root. The workspace-relative form is the fallback, tried only when the source-join does not resolve; a path resolving under both joins is decided by that priority, deterministically (bundle decision 29). And the write path resolves or refuses — a path-grain anchor that resolves under NO candidate join refuses `INVALID_ANCHOR` with every candidate tried in the payload; no mutation stores a silently dead (orphaned-at-birth) reference. Realized in [[engine--anchor-primitive]]'s observation mechanism and the mutation validation seam.

## Context
The first external field deployment (WOENENN, 38 entities) paused on exactly this: every surface around a binding — scope globs, the brief's path list, disposition artifact ids, the provenance instructions — speaks source-relative paths, but anchor resolution alone spoke workspace-relative. An ingest agent following the rendered brief wrote anchors that were accepted at write time and orphaned at read time, silently, on the recommended in-root layout; verify/coverage/drift went blind to the whole mem. The dogfood's out-of-root bindings (`../public`) were the same defect's other face. The write path already validated the anchor's source NAME against the binding — it had the context to join or refuse the path half and stored a dead reference instead.

## Consequences
The existing field convention becomes correct with zero data migration — the 38 WOENENN anchors and any future source-dialect anchors resolve as written; pre-existing workspace-relative anchors keep resolving via the fallback (the dogfood graph is the live regression bed — a post-fix verify over the `../public` binding observes 110/110 anchors). Hand-authored mems are untouched: an anchor without a `source` name observes workspace-relative exactly as before, and the check degrades to accept whenever the candidate set cannot be completed (no workspace root, unresolvable binding) — validation never requires the binding to resolve. `orphaned` stays in the vocabulary for anchors that WERE resolvable and lost their artifact; what is removed is its use as the silent destiny of every brief-conformant write. Fixture cost, accepted: tests that anchor deliberately-nonexistent artifacts now create the file first and delete it to produce the orphaned read state.

## Relationships
- **REFERENCES**: [[engine:anchor-primitive]]

## Options

Rejected (decision 26): declaring workspace-relative the anchor dialect — it re-teaches every source-relative surface, needs a brief rewrite, a migration verb, and a field migration of 38+ entities, all to preserve the one surface that disagreed with the other four. Rejected: warn-only acceptance of dead references — the write moment has the binding and the source root in hand; storing a reference it knows is dead is the defect itself.

## Notes


