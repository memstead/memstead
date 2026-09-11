---
type: decision
created_date: 2026-09-11T10:40:20Z
last_modified: 2026-09-11T13:46:03Z
status: accepted
decided_on: 2026-09-11
deciders: implementing agent (contributor-ready bundle, 2026-09-11)
scope: system
tags: crate-structure, contributor-ready, deferred
---

# Keep memstead-base as one crate until a boundary failure forces the cut

## Decision
We will leave `memstead-base` as one crate. On 2026-09-11 it held 144,426 of the 289,809 lines under `crates/` (`git ls-files 'crates/*.rs' | xargs cat | wc -l` at commit a55f6958, tests included), which is half the engine in a single compilation unit, and the readability work of the contributor-ready bundle stops at moving inline test modules out of the largest files rather than drawing new crate boundaries. The question reopens on a named trigger and not on size alone: the first external contribution that fails at the crate boundary, or a second work bundle blocked by it.

## Context
The bundle's goal is a codebase a stranger can navigate, and the size survey put `memstead-base` at the top of every measure. Cutting it is the structural answer, and it is the expensive one: the projection-crate cut of 2026-09 is the cost precedent in this repository, where extracting one crate moved every binding, every engineering spec that named a path, the handbook, and the describing-surfaces manifest, and each of those is a surface that must be re-graded. The cheap half of the readability problem is different in kind: a 5,373-line file whose 4,331 lines are tests reads as unnavigable for a reason that a mechanical move fixes without touching a single path. Doing the cheap half first leaves the expensive half available on evidence rather than on a size number that has been true for months without anyone tripping over it.

## Consequences
- The engine keeps one compilation unit of 144,426 lines, so a full rebuild stays slow and a contributor still reads one large crate's module tree to find anything.
- No binding, spec, handbook chapter or manifest moves, so this bundle's remaining plans stay anchor-cheap and a refuted grade stays cheap to redo.
- The question is now falsifiable rather than open: the trigger names what evidence would reopen it, so nobody has to re-argue the size number.
- The deferral is recorded where a later agent looks, which is the point of writing it down instead of leaving it in a plan that archives to the attic.
- Accepted risk: the first external contributor who does hit the boundary pays for the deferral before we learn of it.

## Options

- Keep one crate, deferred with a named trigger: chosen. The move of inline test modules buys most of the navigability at a fraction of the cost, and the trigger converts the deferral into a testable condition.
- Cut `memstead-base` inside this bundle: rejected. The projection-crate precedent says every binding, engineering spec, handbook chapter and the describing-surfaces manifest move with it, and the bundle would spend its whole budget on path churn.
- Split the four largest production files instead: rejected for this plan and left to its own. A cut changes paths and costs a sync each, where the test move changes no path at all.
- Set a line-count threshold that triggers the cut automatically: rejected. Size has been high for months with no observed failure; a threshold would fire on a number rather than on a cost anyone actually paid.
