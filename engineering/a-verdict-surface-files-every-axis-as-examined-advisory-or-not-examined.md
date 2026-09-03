---
type: decision
created_date: 2026-09-03T12:47:32Z
last_modified: 2026-09-03T12:47:32Z
status: accepted
decided_on: 2026-09-03
deciders: backlog-repairs bundle, C2 executing session
scope: system
tags: health, coverage, gates
---

# A verdict surface files every axis as examined, advisory or not examined

## Decision
We will have every verdict surface's coverage declaration file each workspace axis in exactly one of three buckets, and render all three on the wire line: examined (the verdict answers for the axis; a finding there fails it), advisory (the surface renders the axis, always or on request, beside the verdict and never folds it in), and not_examined (the surface never looks at the axis, and the reason names the surface that does). The registry validator refuses an axis filed twice or filed without a reason, so the split is a declaration the gate holds rather than a convention.

## Context
The earlier coverage rule stamped a two-bucket line where not_examined meant 'not folded into the verdict'. Four graders read the word instead of the definition and doubted reports that had in fact rendered the axes they named. The line was right by its definition and wrong by its word. Folding conformance and stale into the verdict was rejected (it changes every gate and fixes two axes of fifteen); keeping the word with a footnote was rejected because every independent reader so far read the word, not the footnote.

## Consequences
- Health's rendered-but-advisory axes read advisory; only projection, which health never renders, stays not_examined.
- Every examined set and every verdict is byte-identical, so no consumer of a verdict moves.
- Every verdict line carries the advisory field even when empty, so a parser reads one shape on every surface.
- A new surface must file every axis in one of the three lists or the registry gate fails it.
- Amended 2026-09-03 by the sibling decision that a coverage line reports the verdict and never the render: this decision's original revisit clause anticipated a surface folding an advisory axis in on request, and named the anchors promotion as the one such case. That promotion has since been removed as the defect it was, so the clause now has no instance.

## Options

- Three buckets with a reason per advisory and per not_examined entry: CHOSEN.
- Fold conformance and stale into the verdict: rejected; it changes every gate reading the verdict to fix two axes of fifteen.
- Keep the two-bucket word and explain it in a footnote: rejected; four independent readers had already read the word rather than the footnote.
