---
type: decision
created_date: 2026-08-07T09:08:49Z
last_modified: 2026-08-07T09:08:49Z
status: accepted
decided_on: 2026-08-06
deciders: operator (agent-toolbox bundle decision 4), implementing agent
scope: subsystem
tags: schema, section-format, commonmark, mdast, write-gates, health
---

# Enforce schema-declared section content formats with a real CommonMark parser

## Decision
We chose to let a schema section declare its markdown shape through up to four optional keys on the section definition ([[engine--schema-definition-format]]), all absent by default (absent = free-form, exactly the prior behaviour): **`content`** — a flat content expression over the mdast block-node vocabulary used verbatim (`paragraph`, `list(bullet)`/`list(ordered)`, `table`, `code(lang=…)`, `heading(3)`…`heading(6)`, …) with sequence, alternation, and `+ * ?` repetition; the grammar is deliberately regular — no nesting, no recursion — so it compiles once at schema load and refusals can state expected-next at a position (the ProseMirror precedent for repairable errors); **`item_pattern`** — an anchored regex over the repeating unit (list items with lazy-continuation lines joined, or paragraph source lines) with named capture groups surfacing in refusal payloads; **`table`** — pinned column names/order plus optional per-column cell regexes, with column-count enforcement ours by decision because GFM silently pads/truncates; **`example`** — one conforming snippet echoed verbatim in every format refusal (for an agent, the single highest-leverage part of the payload); and **`format_severity`** — the uniform warn/block model of [[engineering--declare-unhealthy-to-keep-invariants-through-a-six-form-constraint-vocabulary]], defaulting to block because a shape violation is caused by the write being validated and is one-round-trip repairable. Enforcement reduces the section body to a top-level block sequence with `pulldown-cmark` (`default-features = false`, tables via runtime option) so the validator can never disagree with the renderers agents write for; the write path judges the *composed* body on append/patch, standing violations ride the health `constraints` include as a `format_violations` axis ([[engine--graph-health-report-surface]]), and a sealed archive carrying a bad declaration keeps loading with the defect as a health finding, never a boot failure. `planning@0.2.0` in the [[engine--built-in-schema-catalogue]] is the first consumer (a version bump, never an in-place edit), with its optional bullet sections declaring `list(bullet)?` so omission stays legal; engine-side, the hand-rolled Relationships line check was replaced by a declaration through the same mechanism, proving it general.

## Context
A schema could prescribe a section's form only as prose `write_rules`, which did double duty as guidance and pseudo-grammar, and nothing evaluated content against any of it. The field evidence is measured: anker's Evidence sections held 136 conforming lines and 4 silent deviations nothing caught; plenum validated its coordinate line grammar with a project-local Python regex checker because the engine could not. A 2026-08-06 research pass verified with local prototypes that a line-scanner disagrees with CommonMark on exactly the constructs agents produce — lazy continuation, mixed bullet markers, indented code blocks containing `- `, GFM tables degrading to paragraphs — and a validator that disagrees with the parser every renderer uses traps agents in a repair loop that cannot converge. Operator decision 2026-08-06 (agent-toolbox bundle decision 4, separate conversation): build it now.

## Consequences
- The grammar half of `write_rules` is mechanized; the guidance half stays prose — the two are no longer conflated.
- Three typed refusal codes (`SECTION_CONTENT_MISMATCH` with `failed_at`/`expected_next`, `SECTION_ITEM_PATTERN_MISMATCH` with named groups, `INVALID_TABLE_COLUMNS`) carry the echoed example; no format refusal collapses to `INTERNAL`.
- Reserved headings hardened globally: `^# ` joined the `^## ` write-time guard for all sections (the one declared exception to no-noise), and format-checked sections additionally refuse setext h1/h2 the line guard cannot see.
- The engine takes its first markdown parser dependency — bounded to three transitive crates, wasm-lane verified.
- Loader honesty: malformed declarations refuse at install naming every offender; nothing loads-and-ignores.
- A schema declaring no format behaves byte-identically; the field-evidence proof holds — anker's two-halves citation grammar and plenum's coordinate grammar are declared without project Python.
- Bare-name `planning` lookups are ambiguous by design now that 0.1.0 and 0.2.0 coexist; callers pin versions.

## Relationships
- **REFERENCES**: [[engine:schema-definition-format]]
- **REFERENCES**: [[declare-unhealthy-to-keep-invariants-through-a-six-form-constraint-vocabulary]]
- **REFERENCES**: [[engine:graph-health-report-surface]]
- **REFERENCES**: [[engine:built-in-schema-catalogue]]

## Options

- **Regex/line-scanner enforcement without a parser** — rejected on verified evidence, not purity: the divergence cases are exactly what agents produce, and the repair loop cannot converge.
- **`comrak` or `markdown-rs` instead of `pulldown-cmark`** — rejected: heavy default dependency tree, respectively ~20× slower plus 42k LoC into the wasm build, for a full tree this feature never walks.
- **A nestable grammar (per-level rules inside list items)** — rejected: ProseMirror restricted content expressions to a regular language precisely so errors can say "expected X at line N"; nesting reverses that for the LLM consumer. Multi-field items are served by `table` with column patterns or by promotion to entities.
- **Invented format names (`bullet_list`, `definition_list`)** — rejected: mdast is the de-facto vocabulary the models were trained on; a definition list is a `list` plus an `item_pattern`.
- **A separate check verb** — rejected: write path and health are the two enforcement surfaces; a third splits the audience.
- **Severity default `warn`** — rejected for this constraint class: the violation is caused by the write under validation, repairable in one round-trip with the echoed example; blocking is the point, `warn` stays available per section.

## Notes

Shipped 2026-08-06/07 as public-repo commits `ceae7ff` (expression grammar + CommonMark reduction), `3ede4d4` (declaration surface + write-path enforcement), `4fba87f` (health axis, sealed leniency, first consumer, `planning@0.2.0`), `cf964c9` and `de29eff` (grader-driven fixes, including `list(bullet)?` on the optional sections so omission stays legal); independently graded — all ten acceptance criteria confirmed. Complementary half of the constraint vocabulary: that decision covers cross-entity/metadata invariants, this one the markdown shape inside a section; its `enum_from_neighbour` reads "the entries of a section" as the items of a section declaring `content: list`.
