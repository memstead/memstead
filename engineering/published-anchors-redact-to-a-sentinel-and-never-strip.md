---
type: decision
created_date: 2026-08-21T19:10:53Z
last_modified: 2026-08-21T19:10:53Z
status: accepted
decided_on: 2026-08-21
deciders: SPEC W6 + operator-ratified field resolution (RUBRIC 2026-08-19); implemented by agent session (flywheel W6/03)
scope: subsystem
tags: anchors, provenance, publish, privacy, registry
---

# Published anchors redact to a sentinel and never strip

## Decision
`memstead publish --redact-anchors` blanks every artifact reference in the packaged anchors sidecar — the `artifact` field and each `derived_from` entry — to the fixed sentinel `[redacted]`, keeping the trust metadata: provenance class, `at_version`, grain, hash, hash stability, binding hash, and source name. Redact, never strip: a stripped package is indistinguishable from a never-anchored one, so consumers would lose the trust grade; the sentinel keeps every entity's epistemic standing readable while naming no source. Publish-time only — the transform (`redact_archive_anchors`, operating on finished archive bytes so any packaging caller can use it) runs on the staged copy; the workspace sidecar is never touched. The honest default is unchanged: without the flag, published anchors ride byte-identical. The pre-built `.mem` shape refuses the flag typed (`INVALID_INPUT`), per the `--version` precedent for content-shaping flags on baked bytes.

## Context
Anchors deliberately travel in published archives — the trust metadata is the point of the E3a contract — but an author publishing a mem built over a private source disclosed that source's file paths and URLs with every anchor. The operator confirmed the need on 2026-07-11 (backlog); SPEC W6 chose redact over strip; the field resolution (keep `hash`, `source`, `at_version` — the SPEC sentence named only "artifact paths/URLs") was operator-ratified 2026-08-19. Residual disclosure is stated plainly rather than implied away: `grain` reveals the medium shape, `at_version` may carry a commit SHA or ETag, `source` is the author's chosen name, and `hash` permits confirming guessed content against the prepared form — redaction removes identity, not existence.

## Consequences
Consumers of a redacted mem still see how strongly each entity claims fidelity to a source without learning which source; reverse artifact lookup on an installed redacted mem finds nothing, by design. Archive validation now refuses an anchors member with an empty artifact reference, so a botched redaction (blanked to nothing instead of the sentinel) is caught rather than shipped — and the same check hardens every archive, redacted or not. Landed alongside: the engine-agnostic folder assembler now embeds the mem's anchors sidecar, closing the gap where a bare `memstead publish` of a folder mem silently shipped without the anchors its engine-exported sibling carries — the publish-strip failure the anchors contract exists to close. Realized in [[engine--filesystem-mem-archive-assembler]] and the publish surface of [[engine--cli-command-surface]].

## Relationships
- **REFERENCES**: [[engine:filesystem-mem-archive-assembler]]
- **REFERENCES**: [[engine:cli-command-surface]]

## Options

Rejected: strip mode (drop the sidecar — loses the trust grade, the feature's point). Rejected: blanking `hash` too (it is the fidelity claim's substance for hash-bearing classes; the confirmation-attack caveat is carried honestly in the docs, and revisiting is its own decision). Rejected: keeping `derived_from` readable (its entries are artifact references — blanking `artifact` alone would redact nothing for exactly the derived class). Deferred, not granted by the SPEC: extending the flag to `export --format mem` — the export→publish-file route cannot silently ship unredacted anchors under the flag because the pre-built shape refuses it, naming the shapes that redact.

## Notes


