---
type: decision
created_date: 2026-08-08T21:23:49Z
last_modified: 2026-08-08T21:23:49Z
status: accepted
decided_on: 2026-08-08
deciders: operator (agent-trust plan 11)
scope: subsystem
tags: health, open-questions, composition, agent-orientation, worklist
---

# A mem enumerates its own unknowns through a composed open-questions axis that computes nothing new

## Decision
`memstead health --include open_questions` (identical on both MCP flavours and the CLI, one shared composer) serves per mem a worklist of what the holding does not know: its stubs, its never-confirmed (`recheck`) and `unresolvable` anchors, its unsatisfied constraints, its dangling links, and — when a paired process mem resolves for the destination via the ingest-name convention — that process mem's open entries, with negative findings under a DISTINCT `already_searched` heading whose operational meaning is "done, keep off". Binding rules: the axis is COMPOSITION ONLY — no new detection logic, no new stored state; every signal is read from the same source its own axis serves, so the composition can structurally never disagree with the per-signal axes (and the tests assert counts against those axes in the same response, never against fixture constants). Include-gated with byte-unchanged health otherwise; per-kind item cap (20) stated in the output with an explicit `more` remainder — silent truncation is the named anti-pattern; an unresolvable process pairing is stated per mem, never silent and never an error.

## Context
A fresh agent arriving at a mem asked the operator "what should I work on?" while the engine knew most of the answer — scattered across five axes an agent would have to know to ask for (stub flags, the anchors include, constraint findings, the dangling-links include, and the process-mem entry types behind ingest pairing). Each is a hole the holding knows about itself. Composing them turns a mem into a self-directing work source — "where are your holes" becomes one query — which is the most direct content-level lever the operator-obsolescence directive has. The negative-finding type supplied the already-searched category; flattening it into the todo pile would cause exactly the re-searching it exists to prevent.

## Consequences
A fresh agent orients from one include instead of five queries and walks straight from each item's kind and hanging id to the work. The composition-only rule makes future signals (stale-derivation findings, when they exist) a one-line adoption decision. Costs accepted: the axis inherits every source signal's blind spots by design (fixing a wrong signal happens at its source, never patched in the composition); process-mem reach is limited to the ingest-name pairing until pairing becomes declarative — stated per mem in the output rather than hidden; and no prioritization is served (kinds and counts are ground truth; ranking bakes in one holding's priorities and belongs to the consuming agent). Observation for the skill layer: memstead:learn (orientation) and memstead:tidy (hygiene proposals) would both plausibly benefit from reading this axis first — an observation, not an obligation.

## Options

A new `memstead_unknowns` tool — rejected: health is the one dashboard (surface policy); an include is the established shape. Putting it in `memstead_overview` — rejected: overview answers "what is here", health answers "what is wrong or missing". Waiting for declarative pairing — rejected: the destination-local signals are the majority of the value now, and the axis states pairing resolvability honestly. Engine-side urgency scoring — rejected: ranking is the consuming agent's job.

## Notes


