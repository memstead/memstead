---
type: decision
created_date: 2026-08-23T22:57:46Z
last_modified: 2026-08-23T22:57:46Z
status: accepted
decided_on: 2026-08-24
deciders: implementing agent, executing the operator-authored preparation plan bundle
scope: subsystem
tags: preparation, delivery, ingest, anchors, ordering
---

# Delivery preparation delivers dated entries as units in one stamp-ordered sequence

## Decision
We chose to realize touchpoint B of the preparation registry as a per-source unit sequence the ingest delivery path derives from the registry: a source declaring a delivery preparation is delivered as units addressed `<path>#<key>`, sorted by the units' own order keys, then by id, never by discovery or directory order, so the same source state yields the same sequence on every pass. The first registered delivery flavour is `dated-entries` on path-shaped sources: a unit begins at every line that opens with an ISO date or date-time (after leading markdown markers), runs to the next such line, and is keyed by the stamp normalized to `YYYY-MM-DDTHH:MM:SS` with an ordinal suffix for later same-stamp entries in the same file; text before the first stamp folds into the first unit; a file without a stamp is one unit keyed `whole` that sorts before every dated unit. A first run (no baseline) delivers every unit of every in-scope file; a change run delivers only the units that differ, diffed against the file's content at the git baseline commit, and degrades to every unit of a changed file, saying so, when no baseline content is retrievable (mtime detection, a foreign baseline). The brief presents the sequence numbered by total-order position, capped at the build operation's `batch_size`, subtracting units already disposed in the in-progress advance store so each pass presents the next ones in order; unit ids ride the advance gate, an anchor over exactly a unit auto-disposes it, and a file-level anchor never disposes a unit. Under a delivery preparation a span anchor `<path>#<key>` hashes its unit rather than its file (touchpoint A honouring touchpoint B's unit): an unchanged unit in a changed file resolves, a removed unit orphans, an edited one drifts. `PREPARATION_IMPL_VERSION` bumps to 2.

## Context
The design `engineering--one-preparation-slot-three-flavours-two-engine-touchpoints` binds touchpoint B to two separable needs from the plenum channel's finding 17: sub-file unitization (one file carrying many delivery units: transcripts, logs, mail threads) and a deterministic total order holding across first run and change run (a chronological corpus is never shuffled; the agent at unit N may know only the units before it; same source state, same sequence), as engine properties rather than caller discipline. Before this decision the delivery path listed changed files per class, alphabetically, and a first run presented nothing (a reseed), so neither property existed below file granularity. The first slice of the registry ([[engineering--the-preparation-registry-is-engine-owned-and-its-entity-flavour-hashes-the-load-bearing-sections]]) had named the touchpoint and reserved it. Two facts shaped the concrete rules: the ingest brief re-presents the live cursor on every pass and never subtracted an in-progress advance store, which is why the sequence itself marks disposed units; and the git strategy is the only change-detection strategy that can retrieve a file's baseline content, which is why unit-precise change runs are a git-strategy property and mtime degrades visibly. Engine surfaces changed: [[engine--pipeline]] and [[engine--anchor-primitive]].

## Consequences
- A chronological corpus under `dated-entries` is consumed in its own order on every pass, first or change run, with unit N never presented before units 1..N-1 are disposed or listed.
- Keys survive growth: appending entries never renames an existing unit, so a change run re-delivers nothing that did not change. Inserting an entry with a stamp already present earlier in the same file shifts the later same-stamp ordinals; a corpus that reuses one stamp for several entries pays this, an append-only one never does.
- `batch_size` on the build operation finally means something on the delivery path: how many not-yet-disposed units a pass presents. Sources without a delivery preparation are unaffected and keep file-granularity delivery byte-for-byte.
- Unit ids are accepted by `projection advance`; the frozen slice and the sequence stay in step because the sequence's ids replace the file ids in the slice.
- Unit anchors get the drift contract at unit grain; file anchors on the same source keep whole-file hashing. A span locator that is not a unit key on a delivery-prepared source reads as a vanished unit (orphaned), which is the honest reading of a key the file no longer yields.
- Every binding's `hash(D)` changes again (impl version 2): prior findings are superseded by construction, re-derived by the next verify.
- Later delivery flavours (record-per-line, mail separators, heading sections) are registry entries; each adds one `unitize` arm and its own key rule, and bumps the impl version once.

## Relationships
- **REFERENCES**: [[the-preparation-registry-is-engine-owned-and-its-entity-flavour-hashes-the-load-bearing-sections]]
- **REFERENCES**: [[engine:pipeline]]
- **REFERENCES**: [[engine:anchor-primitive]]

## Options

- Order units by file path, then position in file, with no stamp parsing: rejected. It is deterministic, but it is directory order dressed up; a corpus whose file names are not chronological would deliver later entries before earlier ones, which is exactly the shuffling the finding forbids.
- Key units by a content hash instead of a stamp plus ordinal: rejected. Stable under insertion but not under any edit of the entry itself, so every edited unit would present as a delete plus an add and lose its identity across passes.
- Keep first runs as plain reseeds and unitize only change runs: rejected. The finding's order property must hold on the first run, which is where a whole corpus is consumed.
- Persist per-unit hashes in the engine cache to make mtime change runs unit-precise: rejected for now. The cache is disposable by contract; the git baseline gives a stateless, exact answer, and the mtime degradation is visible in the brief rather than silent.
- A separate registry for delivery flavours beside the prepared-form one: rejected. One registry with a touchpoint field keeps the refusal rule single (unknown identifier) and lets validation reject a delivery flavour over a graph or web source through the same grain-namespace check.

## Notes

Landed in `memstead-base::preparation` (unitize, diff_units, the stamp parser) and the ingest cursor (deliver_units, sequence_units), rendered by the brief's delivery block, consumed by the advance gate, and honoured by span-anchor observation. Pinned: unitization and stamp normalization, key stability under growth, the diff classes, shuffled-collection sorting, the brief's ordering/batch/disposed rendering, and an end-to-end git corpus covering first run, advance with auto-dispose, change run at ordered positions, and unit-anchor observation.
