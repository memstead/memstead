---
type: decision
created_date: 2026-09-11T13:20:43Z
last_modified: 2026-09-11T13:46:03Z
status: accepted
decided_on: 2026-09-11
deciders: implementing agent (contributor-ready bundle, 2026-09-11)
scope: system
tags: layout, readability, contributor-ready, thresholds
---

# Cut a source file at two thresholds: tests out above 1,500 lines, production cut by phase above 3,000

## Decision
We will cut a Rust source file at two thresholds, and only these. First, an inline `#[cfg(test)]` module leaves a production file that exceeds 1,500 lines, into a sibling child module declared from the parent, one child file per inline module so every test keeps its path. Second, a production file is cut into child modules above 3,000 production lines, along a seam the file already carries (its section headers, its tool families, its phases), with every public path preserved by re-exports and every method keeping its name, signature and visibility, so no consumer changes an import. Below 3,000 production lines a file is left alone: the threshold is a cost boundary, not an aesthetic one. A production file between 1,500 and 3,000 lines is known and accepted, not a debt item.

## Context
The contributor-ready bundle set out to make the engine navigable to a stranger, and the size survey offered two different remedies that are easy to conflate. Moving tests is mechanical: nothing changes path, no anchor moves onto a new artifact for a reason a reader must understand, and a 5,373-line file whose 4,331 lines are tests becomes a 1,042-line file that reads as what it is. Cutting production code is not mechanical: it moves symbols between files, so every engine-mem entity anchored on the old file has to be re-placed on the file that now holds what it cites, and every prose citation of a moved symbol goes stale silently, because `projection verify` checks anchor hashes and cannot read a claim. Across the four production cuts in this bundle that second cost was consistently the larger one and the one that produced every refutation. Setting the production threshold at 3,000 rather than at the same 1,500 keeps that cost where it buys the most.

## Consequences
- Fifteen production files stay above 1,500 lines and are accepted rather than queued: ops/health.rs 2,908, render.rs 2,612, memstead-schema/loader.rs 2,609, memstead-cli/commands/projection.rs 2,539, engine/error.rs 2,484, mem_management/lifecycle.rs 2,273, memstead-projection/report.rs 2,050, memstead-projection/findings.rs 2,034, anchor.rs 1,948, memstead-projection/brief.rs 1,789, engine/mutation/mod.rs 1,761, memstead-cli/commands/quickstart.rs 1,672, engine/drift.rs 1,612, memstead-cli/commands/mem.rs 1,549, memstead-projection/cursor.rs 1,517 (counts at 2026-09-11, measured by script over crates/*/src excluding tests.rs).
- A contributor still meets files of that size, which the bundle accepts as the price of not paying the sync cost again for a gain nobody measured.
- Every future cut inherits an obligation the bundle learned the hard way: re-place the anchors by the symbol each entity cites, then sweep the prose for citations of every moved symbol, including symbols defined outside the cut module.
- The thresholds are stated as numbers, so a later reader can disagree with the numbers rather than re-derive the reasoning.
- Accepted cost: two thresholds are harder to remember than one, and a file at 2,900 production lines will look arbitrary to someone who does not know why 3,000.

## Relationships
- **REFERENCES**: [[keep-memstead-base-as-one-crate-until-a-boundary-failure-forces-the-cut]]

## Options

- Two thresholds, 1,500 for tests and 3,000 for production: chosen. It puts the cheap remedy everywhere it helps and the expensive one only where the file is genuinely unnavigable.
- One threshold at 1,500 for everything: rejected. It would have required cutting fifteen more production files, each carrying the anchor and citation sync that produced every refutation in this bundle, for files a reader can still navigate.
- One threshold at 3,000 for everything: rejected. It would have left 4,331 lines of tests inside a 5,373-line file, which is the single worst readability case the survey found and the cheapest to fix.
- No threshold, cut by judgement per file: rejected. The survey showed the judgement call is what had not been made for months; a number gets made.

## Notes

Applied across the contributor-ready bundle of 2026-09-11: plan 02 moved the inline test modules out of 34 files over 1,500 lines, and plans 03 to 06 cut four production files over 3,000 lines (memstead-mcp server.rs, ops/mod.rs, engine/query.rs, engine/lifecycle.rs) by tool family, section header, concern and phase respectively. The deferred memstead-base crate cut is recorded separately in [[engineering--keep-memstead-base-as-one-crate-until-a-boundary-failure-forces-the-cut]].
