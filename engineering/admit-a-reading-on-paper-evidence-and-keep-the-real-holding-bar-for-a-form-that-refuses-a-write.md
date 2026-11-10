---
type: decision
created_date: 2026-11-10T10:43:10Z
last_modified: 2026-11-10T10:43:10Z
status: accepted
decided_on: 2026-11-10
deciders: operator, implementing agent
scope: subsystem
tags: schema, constraints, admission-bar, readings, evidence
---

# Admit a reading on paper evidence and keep the real-holding bar for a form that refuses a write

## Decision
We chose to split the admission bar for a new schema-language form by what the form can do. A form that only adds a reading (a line in a brief or on a health axis that refuses nothing, with byte-identical output for every schema that does not declare it) may be admitted on paper evidence: a schema package the engine validates, exercised in a scratch workspace so that the missing reading is shown by a run and not asserted, plus an independent adversarial review of the design against the code. A form that can refuse a write keeps the bar [[engineering--declare-unhealthy-to-keep-invariants-through-a-six-form-constraint-vocabulary]] set: a real holding writing a check none of the existing forms express.

## Context
The single bar was set on 2026-08-06 to stop forms built on speculation, and it held once. It presumes that holdings exist to produce the demand. For a kind of use nobody runs yet it closes the direction for good: no holding without the capability, no capability without the holding, and no operator capacity to build a holding only to produce the evidence.

The occasion was a design proposal of 2026-11-10 to let schemas declare check points whose recorded checks hold for a time. Four independent readers tested it against the published tree, two of them models of other vendors. Three said to build a smaller version, one said not to build it, and two rested their objection on the bar. The findings that did not depend on the bar were about construction, and they shaped what a reading may be:

- Time inside a write gate fell. A `transition_requires_self_check` gate fires on every write while the gated value holds, so a check that expired by date would refuse every later edit of the entity and turn `health --strict` red by the calendar alone.
- Anchors on a check record fell. Anchors are sidecar rows on the mem branch, written in commits and rewritten by adjudication; a check produces no commit and its ledger is append-only.
- A per-check opt-out of hash freshness fell. It removes the one binding between a check and the state it saw.
- An instruction channel carried by the schema fell as specified. The prose of a third-party schema is withheld, so a published rulebook would deliver none of it.

One thing stood with all four readers: a due date read from the newest confirming check of a named kind plus a number of days, as a reading in the due brief. Today that date has to be a field an agent types into the entity, which is the kind of self-report the check ledger exists to replace ([[engineering--a-schema-declares-what-is-due-and-what-would-resolve-an-open-entity-and-health-reads-both]]).

## Consequences
- A reading can enter the schema language without an outside holding, on evidence a session can produce and another can check.
- A reading admitted this way names its evidence in its own decision, and it is removed without a deprecation cycle if no holding takes it up; pre-1.0 that costs one schema-language generation.
- A reading never becomes a gate in a later release by this route. Promotion to a refusal needs the real-holding bar, met on its own.
- Paper evidence has three parts and all three are required: the engine validates the package, a run in a scratch workspace shows what cannot be expressed, and a reader who did not write the design reviews it against the code. A schema that exists only as text is not evidence.
- The first candidate is the due reading named above. It has a precondition: the newest check of a kind is defined by the time of the act, where today it is the last line in ledger order.
- Cost accepted: readings can accumulate, and each one still costs a parser path, a guide section and an example. The bar against forms on speculation is lower than it was for this one class.
- The review that prompted this also raised three questions about how a check stands today (a licensing check carried across later edits, independence compared with the first author only, newest by ledger order). They are open work and no part of this decision.

## Relationships
- **MOTIVATED_BY**: [[declare-unhealthy-to-keep-invariants-through-a-six-form-constraint-vocabulary]]
- **REFERENCES**: [[a-schema-declares-what-is-due-and-what-would-resolve-an-open-entity-and-health-reads-both]]
- **REFERENCES**: [[declare-unhealthy-to-keep-invariants-through-a-six-form-constraint-vocabulary]]

## Options

- **Keep the single bar for every form.** Rejected: it presumes holdings that a new kind of use does not have yet, and so decides the direction by default instead of on its merits.
- **Drop the bar.** Rejected: it held once and kept the vocabulary from becoming folklore, and a wrong form that refuses writes blocks work in every holding that declares it.
- **Count the project's own holdings as a real holding for every form.** Rejected for forms that refuse a write: the author of the form would be its only witness. For a reading the own holdings are welcome as additional evidence and not required.
- **Admit on paper evidence without a run.** Rejected: the proposal that prompted this misdescribed the engine's current behaviour in its premise, and only reading the code and running the case showed it.
- **Split by what the form can do: a reading on paper evidence, a refusal on a real holding.** Chosen: a reading cannot refuse anyone's write and leaves every non-declaring schema byte-identical, so the cost of being wrong is one removed key.
