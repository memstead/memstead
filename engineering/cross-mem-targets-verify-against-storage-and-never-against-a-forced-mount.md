---
type: decision
created_date: 2026-08-22T04:48:06Z
last_modified: 2026-08-22T04:48:53Z
status: accepted
decided_on: 2026-08-22
deciders: SPEC W7 + implementing agent sessions (flywheel W7/02) under the standing engine-change directive
scope: subsystem
tags: cross-mem, federation, verification, trust, lazy-mounts
---

# Cross-mem targets verify against storage and never against a forced mount

## Decision
Write-time cross-mem target verification asks the mem's REAL STORAGE and never loads or mounts the mem. The primitive is `MemBackend::entity_exists` in memstead-base — a pure existence probe with a correct-everywhere default and cheap per-family overrides: one `symlink_metadata` call on folder backends, a stop-at-entry tree lookup on git-branch (never the blob-reading listing). For a MOUNTED-but-deferred (lazy) target mem the funnel probes the mount's own backend; for an UNMOUNTED mem the full workspace boot installs a discovery hook (`set_unmounted_storage_prober`) that resolves the mem's content branch in the mem-repo, hands back a transient backend, and reads the stored config's schema pin — so the cross-schema edge routing (`cross_mem_relationships:`) keeps its authority without a mount, since the routing check needs only the schema REF, never the loaded schema. The shape check uses the target's real entity type from ONE resolved blob read (`peek_type_from_frontmatter`); it never guesses and never skips because a mem is unloaded. Semantics on the three verdicts: storage HIT → admitted as an ordinary verified reference whose in-store stub carries the load-time kind and NO auto-stub or mem-uncreated warning (those claim the target awaits creation — false); storage MISS in a read-only mount → the existing typed refusal, now answerable without load and never firing for an entity storage contains; storage MISS elsewhere, or no discoverable storage → today's forward-reference auto-stub mechanic, untouched. Verification never converts into a load: relate skips the target-mem reload for deferred targets, batch reloads retain only source and non-deferred mems, and no mount is ever added as a side effect of a write.

## Context
The only way to get a verified cross-mem edge was to mount the target mem, and every mount was a permanent eager load added to every cold command — the sizing document priced a dossier citing twenty small topic mems at ~5+ seconds per command for edges that needed only an existence check. The write-time target-existence refusal fired only for read-only-mounted targets absent from the loaded store; an edge into an entirely unmounted mem auto-stubbed with warnings, unverified. The git-branch storage layer could resolve ref→commit→tree but its single-path helper read the blob's bytes and the public listing walk read every blob — no stop-at-entry existence primitive existed on any API.

## Consequences
The federation tax on citation dies: a dossier citing twenty unmounted topic mems pays twenty tree lookups at write time, zero mounts, zero loads, and no change to any subsequent cold command. No new trust class exists anywhere in the model — every admitted cross-mem edge is either storage-verified at write time or an ordinary forward-reference stub with the pre-existing semantics, and stub kinds remain annotation-not-state (a reload produces the same stub landscape it always produced). The read-only contract sharpened: absence is now judged by storage, so the refusal can no longer fire for an entity that exists merely because its mem is unloaded — the load-scope/answer-scope rule from [[engineering--lazy-mounts-defer-the-entity-walk-and-never-the-gauntlet]] applied to the write path. Lean and embedded engines without the discovery hook keep the old behaviour for unmounted targets, degrading to the forward-reference mechanic, never to a false refusal. One honest softening: a deferred entity relying on its mem config's DEFAULT type (no `type:` frontmatter) shape-checks as unknown type — admitting rather than guessing — where a loaded mem would apply the config default.

## Relationships
- **REFERENCES**: [[lazy-mounts-defer-the-entity-walk-and-never-the-gauntlet]]
- **DERIVED_FROM**: [[lazy-mounts-defer-the-entity-walk-and-never-the-gauntlet]]

## Options

Rejected (by SPEC): MARKED DEFERRED STUBS — a durable unverified claim plus a remembered verification obligation, erased by the very reload meant to discharge it, and a new trust class the refusal contract would explain forever. Rejected: MOUNT-ON-DEMAND — converts a write into a permanent workspace-shape change and still pays a full load for an existence question. Rejected: EXISTENCE WITHOUT TYPE — weakens schema authority exactly for the federated case the feature serves; the single-blob read makes the honest check affordable. Left as form: answering from the loaded store when the target happens to be loaded — the loaded store is itself a faithful view; the constraint is that the unloaded path is storage-verified and both paths agree.

## Notes


