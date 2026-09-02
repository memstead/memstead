---
type: decision
created_date: 2026-09-02T12:41:03Z
last_modified: 2026-09-02T12:41:03Z
status: accepted
decided_on: 2026-09-02
deciders: operator (backlog-engine bundle A, go of 2026-09-02), implementing agent
scope: subsystem
tags: health,schema,remodel,vital-signs
---

# A schema declares one last-resort type and the vital-signs health axis reads model truth from it

## Decision
A type definition may declare `last_resort: true`, at most one per schema (`MultipleLastResortTypes` refuses a second); the health axis `vital_signs` reads per mem five model-truth signals that need no verdict: the last-resort type's share per community, unclaimed source files (sizes, no threshold), contested unowned files, zero-outgoing entities folded into their subject, and empty declared sections, counts plus capped lists (`_item_cap` 20). The `remodel` skill reads the axis as its second step. The built-in schemas do not declare a last-resort type yet: a built-in version is minted for meaning, so the declaration lands with the next software schema version, and every dogfood mem reads `not_declared` until then.

## Context
The model-truth campaign (2026-08) found that a mem's health said nothing about whether its model still fit its subject: a catch-all type quietly absorbing a community, files no entity claims, entities that only receive. Those are signals for a remodel, not violations, so they belong on an axis that computes counts and never a verdict.

## Consequences
The axis is byte-identical between CLI and MCP (pinned by test). A quiet fixture's type-share signal carries one row per community with a zero share: the signal's shape, not a finding. The unclaimed-file signal carries sizes and no threshold; the remodel skill holds its own (8 kB).

## Options

Declaring the last-resort type in place on `software@0.4.0`: rejected, a built-in version is minted for meaning. A verdict on the axis: rejected, model fit is the remodel skill's judgement.
