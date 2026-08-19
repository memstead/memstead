---
type: decision
created_date: 2026-08-19T05:19:54Z
last_modified: 2026-08-19T05:19:54Z
status: accepted
decided_on: 2026-08-19
deciders: bundle decisions 1-4 (backlog-sweep README), implemented in plan 05
scope: subsystem
tags: schema, seal, format-marker, archive, search, batch-notes, contracts
---

# A seal carries its source's generation and never asserts one

## Decision
Four stated engine contracts became true in one pass (backlog-sweep plan 05). (1) **Seals carry the source's generation:** the format marker (`schema-format.json`) is minted only by the authoring resolver, which just validated the package under the current language; every seal seam (engine install, below-boot repair, CLI install) writes the package AS-GIVEN — a legacy package seals unmarked because absence IS its legacy claim. (2) **Every archive reader consults the one format predicate:** the byte-hydration path (`Engine::from_archive_bytes`, hence the wasm package) refuses an unknown `format` typed instead of hydrating past it. (3) **A search mem filter naming no visible mem refuses `UNKNOWN_MEM`**, matching every other mem-naming surface — an empty result always means a valid mem with no matches. (4) **Per-entry batch notes ride the one batch commit's note record** as `<id>: <note>` lines on all three batch families, retrievable on the git-branch backend; a note-less batch carries no note record.

## Context
Each was a documented guarantee the code did not keep, and the marker one actively manufactured defective artifacts: `schema install` stamped every unmarked package as current-language, so a legacy builtin sealed onto `__MEMSTEAD` read back with every bare metadata field silently flipped from required to optional — the exact flip the 0.6.0 marker contract promises cannot happen. The archive predicate's "single predicate every reader gate consults" doc was false on the byte path (`format: 99` hydrated through wasm); the search asymmetry made absence-of-mem indistinguishable from absence-of-matches; and the batch-note contract was empty exactly where most writes happen.

## Consequences
The generation decision now lives at the one seam that can verify it (the resolver), and the exposure of already-mis-stamped artifacts in the wild is recorded honestly as a backlog entry with a detection one-liner and a reinstall repair path — this workspace's dogfood is clean, field workspaces may not be. Readers of sealed packages, archives, search, and batch notes can now trust the doc comment and the behaviour to say the same thing; the negative tests (unmarked legacy seal reads required; `format: 99` refuses; nonexistent-mem refuses; note-less batch writes no artifact) pin each contract.

## Options

Rejected: validate-and-refuse legacy content at install (installing legacy builtins is a supported act — the defect was the label, not the install). Rejected: accept-and-warn on unknown archive formats (a reader proceeding past an unknown format is the consults-nothing state with extra steps; wasm has no warning channel worth trusting). Rejected: per-entry commits to preserve batch notes (the single batch commit is the feature; multiplicity belongs in the note record). Rejected: keeping search's silent success as a graceful read (success-with-zero-hits is the one thing a typed surface must never be ambiguous about).

## Notes


