---
type: decision
created_date: 2026-09-10T22:10:15Z
last_modified: 2026-09-10T22:10:15Z
status: accepted
decided_on: 2026-09-11
deciders: architecture-seams bundle plan 02 (agent, under the delegated schema-change rule of 2026-09-02)
scope: subsystem
tags: constraints, schema-language, planning, gates, checks
---

# Gated transitions carry a floor and a self-check form so the planning contract is engine-refused

## Decision
We chose to give the constraint vocabulary two generic additions rather than keep two plan-process invariants in the walker skill's prose: transition_requires_checks takes an optional min_related (default 0, the sealed semantics of every earlier generation), so min_related: 1 refuses the vacuous case in which an entity nothing points at lands the gated value with nothing to check; and a new form, transition_requires_self_check, gates a transition on the entity's OWN fresh confirming check record of a declared check_kind (an engine kind or a foreign x-name kind), recorded under an identity other than the entity's author. The check-kind wire grammar has one definition, memstead_schema::check_kind_wire_is_well_formed, shared by the schema loader and the engine's check surface. The workspace-local planning generation 0.7.0 is the first user: a plan with no criterion cannot complete, and a bundle completes only on a recorded independent x-projection check.

## Context
The 2026-09-10 architecture assessment and both of its adversarial counter-checks named the same gap: the criterion floor (a criterion-less plan completes vacuously) and the closing contract (project the durable decisions before archiving) were enforced only by the execute-graph-plan skill's prose, and one bundle had already lost a decision that way on 2026-09-01. The census decision basket of 2026-09-04 (line 1, cause before gate: move the check into the engine) and the delegation of constraint declarations and new generations to the agent (2026-09-02) made the engine the right home. The engine cannot judge whether decisions were projected; it can require that the act was recorded, fresh against the entity's content, by someone else, which is exactly the shape criterion verdicts already have.

## Consequences
- A plan pinned to planning@0.7.0 refuses status complete with CONSTRAINT_UNSATISFIED naming the floor when no acceptance criterion verifies it; on 0.6.0 the vacuous completion still lands (sealed generations keep their meaning).
- A bundle on 0.7.0 refuses status complete until a grader identity records memstead check <bundle> --kind x-projection --verdict ok; the author's own record reads self_checked and does not open the gate; a record of another kind does not either.
- Both forms are declarable by any schema: the loader validates field, enum value and check-kind grammar, and refuses a malformed declaration at schema validate and at install.
- The gates brief names the floor; the health constraints axis reports a self-check gone stale after the transition as a standing violation.
- The walker skill keeps its prose checks only for bundles born on 0.6.0 until they archive; then those rows leave the skill.
- Cost accepted: a foreign check kind (x-projection) carries meaning only by convention between the schema's prose and the skill; the engine records it verbatim and never interprets it. A schema that spells the kind differently from its skill never opens the gate, which is the honest failure (refuse) rather than the silent pass.

## Options

- Keep both invariants in the skill's prose: rejected, a prompt-resident invariant; the recorded loss of 2026-09-01 is what this decision ends.
- Have the engine judge whether decisions were projected: rejected, no engine can judge content; requiring the recorded act under an independent identity is the same pattern criteria use.
- Express the floor as must_reach on the plan type: rejected, reachability declarations warn by design and a warning cannot refuse a transition.
- A new constraint kind for the floor instead of a field on the existing one: rejected, the floor is a property of the same gate (how many related entities it quantifies over), and a second kind would duplicate the field, value, relationship and direction declarations.
- Migrate the running 0.6.0 bundles to 0.7.0 at once: rejected, changing transition semantics under a bundle mid-walk; they finish on the generation they were born on.
- Ship the forms in a built-in planning generation: deferred, the built-ins stop at planning 0.4.0 and the dogfood line is workspace-local by design; the forms are engine work, the generation is workspace work.
