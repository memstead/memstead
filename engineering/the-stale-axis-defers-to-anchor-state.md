---
type: decision
created_date: 2026-09-02T20:10:21Z
last_modified: 2026-09-02T20:10:21Z
status: accepted
decided_on: 2026-09-02
deciders: coordinating session (bundle B), implementing agent, operator directive 2026-09-02
scope: subsystem
tags: health, anchors, staleness
---

# The stale axis defers to anchor state

## Decision
For an entity carrying at least one adjudicated hash-bearing anchor the stale health axis reports the anchor-derived reading instead of the wall clock: `resolves` keeps it off the stale list (and lists it under `anchor_fresh` when the day threshold would have named it), `drifted` and `recheck` list it as their own condition whatever its age, each anchor-produced row naming `clock: anchors` and its state. Entities with no adjudicated anchor keep the `staleness_threshold_days` reading, byte-identical to before, and an anchor-less workspace renders unchanged.

## Context
Bundle B plan 7 (2026-09-02). `staleness_threshold_days` ran a second clock beside the source-derived baseline, and the two disagreed the moment a correct, unchanged entity crossed the threshold.

## Consequences
One reading per entity; the overlay runs on every health call at the cost of one anchor verification per in-scope mem, the same work the anchors axis does. The health clock is pinnable with `MEMSTEAD_TODAY`.

## Options

Setting the threshold to none in the software schema rejected (it blinds anchor-less entities too); two readings side by side rejected (the entry was filed on exactly that disagreement).
