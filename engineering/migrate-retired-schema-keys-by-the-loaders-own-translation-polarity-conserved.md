---
type: decision
created_date: 2026-09-02T00:46:51Z
last_modified: 2026-09-02T00:46:51Z
status: accepted
decided_on: 2026-09-02
deciders: execute-graph-plan loop, evidence-engine bundle
scope: subsystem
tags: schema, migration, cli, sealed-content, polarity
---

# Migrate retired schema keys by the loader's own translation, polarity conserved

## Decision
We chose to give authoring packages the same retired-key translation the sealed read already performs, as an explicit verb: `memstead schema migrate <dir>` rewrites `propagating_relationships` to `no_self_loop_relationships`, inverts the metadata-field `optional:` into `required:`, drops the dead `examples:` list with a pointer at `exemplar:`, and renames the exemplar-relation `to:`/`type:` pair to `target:`/`rel_type:`. The translation has one home: `memstead_schema::migrate::LEGACY_KEYS`, a table the suite pins against the `legacy_*` serde sentinels in `types.rs` in both directions, so a sentinel added without a rewrite (or a rewrite without a sentinel) fails a test. Every run proves itself faithful before writing: the original is loaded through the tolerant sealed-style read under the generation the package was written in, the rewrite through the strict authoring read, and the resolved schemas must agree on requiredness, self-loop sets and exemplar relations, or nothing is written. Dry run is the default and prints one line per rewrite; `--write` edits the files in place as text, so comments, key order and spacing survive. Polarity is conserved rather than flipped soft: a package that carries `optional:` anywhere was written under the pre-flip language where an absent key meant required, which is exactly how its sealed copies still read, so every metadata field declaring neither key receives `required: true` and the report says so; deleting a line is the author's call. The verb never bumps `version` and never touches a sealed copy inside a mem.

## Context
The authoring tier refuses retired keys loudly by design ([[engineering--one-name-per-concept-across-every-surface-and-a-retired-name-refuses]]), while sealed content keeps loading with the keys translated ([[engineering--sealed-content-is-read-by-the-same-reader-that-admitted-it]]). Between the two sat a gap with no tool in it: an author whose package predates a rename could read the refusal and its fix line but had to apply the rename by hand across every type file, and the remedy named by [[engineering--an-unstamped-seal-that-stops-re-authoring-is-announced-not-discovered-at-the-next-install]] (re-author under the current language, then install) had no mechanised first step. Two external schema packages authored in 2026-08 (a grounding schema with 13 violations, a debate schema with 19) had been unable to change since 2026-08-05 for exactly this reason. The polarity question forced the sharpest choice: the loader's own contract for a directory package is fail-soft (absent keys become optional), but the loader's contract for the same package sealed is the legacy reading (absent keys are required), and a migration that silently moved a package from the second to the first would change what the package means while claiming to change only its spelling.

## Consequences
- Authors of pre-rename packages get a reviewable, mechanical path back to installability: dry run, read, `--write`, `validate`, `install`, `mem set-schema`.
- A new retired key costs its author three edits, not two: the serde sentinel, the loader gate, and a `LEGACY_KEYS` row; the pinning test makes the third impossible to forget.
- The faithfulness check makes the rewriter's textual approach safe: a spelling it cannot reach (a quoted key, a flow-style mapping) surfaces as a typed refusal with the loader's own violation, never as a half-migrated package.
- Polarity conservation inserts `required: true` on bare fields of pre-flip packages, which can be many lines (22 rewrites for 13 violations in the grounding pilot, 49 for 19 in the debate pilot). The dry run is the review step for exactly this; an author who meant optional deletes the line.
- The verb does not decide whether a spelling migration deserves a version bump; it says so in its report, consistent with [[engineering--a-built-in-schema-version-is-minted-for-meaning-never-for-spelling]].
- Sealed copies inside mems are untouched; whether and when a mem re-pins to a migrated package stays a `mem set-schema` act.

## Relationships
- **MOTIVATED_BY**: [[an-unstamped-seal-that-stops-re-authoring-is-announced-not-discovered-at-the-next-install]]
- **INFORMED_BY**: [[sealed-content-is-read-by-the-same-reader-that-admitted-it]]
- **REFERENCES**: [[a-built-in-schema-version-is-minted-for-meaning-never-for-spelling]]
- **REFERENCES**: [[one-name-per-concept-across-every-surface-and-a-retired-name-refuses]]
- **REFERENCES**: [[sealed-content-is-read-by-the-same-reader-that-admitted-it]]
- **REFERENCES**: [[an-unstamped-seal-that-stops-re-authoring-is-announced-not-discovered-at-the-next-install]]

## Options

- Tolerant directory loads with warnings instead of a verb: rejected, it weakens the loud authoring gate that exists so the author acts; a tolerance window is a policy decision for a later bundle.
- Auto-migrate on `schema install`: rejected, install must stay a validation gate and never a rewriter; a separate verb keeps the act visible and reviewable.
- `--write` as the default: rejected, an author package is source and a rewrite of source is previewed first.
- A YAML round-trip through the serializer: rejected, `serde_yaml_ng` drops comments and an author package's comments are the author's; the line-level rewriter plus the faithfulness check gives comment preservation without trusting the text edit alone.
- Textual sed in a script: rejected, the loader's sentinels are the only definition of what a legacy key means and a script would drift from them.
- Fail-soft polarity (leave bare fields alone, so they flip to optional): rejected, it would change the package's meaning under the name of a spelling migration; the sealed read of the same bytes says required, and the migration conserves that and shows every inserted line.
- Chosen: the explicit verb over the one table with the run-time faithfulness check and conserved polarity.

## Notes

Landed in engine commit c1423f7 (0.15.0 line) with the CLI coverage declaration, the changelog entry, the guide section in the docs-site schema-authoring guide, and the regenerated CLI and error references. Acceptance evidence ran on scratch copies of the two external pilot packages; their originals were checksummed identical before and after.
