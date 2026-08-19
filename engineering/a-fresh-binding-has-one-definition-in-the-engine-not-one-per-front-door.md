---
type: principle
created_date: 2026-08-19T19:33:39Z
last_modified: 2026-08-19T19:33:52Z
authority: accepted
universality: domain-wide
tags: bindings, scaffold, engine-ownership
---

# A fresh binding has one definition in the engine, not one per front door

## Statement
The record a caller gets when it asks for a fresh binding is decided **in the engine**, by one function, not composed at each command that happens to create one. `memstead_base::binding::scaffold_binding` owns the whole shape — the source scoped `**/*`, the enumerable-medium deny defaults materialised into the record, the capability-matrix filter that strips operations the medium cannot serve (and names the deferral as a warning), and the prune block wherever sync survived. A front door supplies only what it alone knows: destination mem, source name, pointer, medium type, intent, and any extra deny globs it can justify.

The same rule covers the honest caveats a scaffold owes its caller: `ingest::cursor::out_of_root_layout_warning` is the single wording for an out-of-root medium base, so the layout split is named once, in the same terms, wherever it is decided.

## Scope
Governs every surface that creates a binding record: `memstead projection init`, the guided `memstead quickstart --repo` path, and any embedder. Does NOT govern editing an existing binding — `projection enable` and hand-edits stay per-operation, because a record already on disk is the author's, not the scaffold's.

## Relationships
- **GOVERNS**: [[the-guided-first-session-binds-the-repository-instead-of-refusing-it]]

## Justification

The scaffold had accumulated a decision per line — which denies to write, when to strip sync, whether prune rides along — inside one CLI command. A second front door composing its own version is how two commands come to write records that differ in ways nobody chose: the second one is always a little behind, and the drift is invisible until a binding behaves unlike its sibling. Making it an engine function makes divergence a compile-time impossibility rather than a review obligation, and it satisfies the standing rule that a change landing for one task must be generically useful — the guided quickstart path needed a scaffold, so the scaffold became the engine's.

## Exceptions



## Consequences

- A new default (another deny entry, a changed batch size) lands once and reaches every front door.
- A caller that needs scaffold behaviour the engine lacks extends `ScaffoldParams` generically — `additional_deny_paths` is that shape — rather than branching privately. Engine state and mount storage locations never travel that way: their exclusion is unconditional in the strategy layer, never a record entry.
- The silent dead-deny exemption stays keyed on the engine's own default list, so a front door cannot widen it by scaffolding extra entries; a user-authored entry that matches nothing still gets the loud lint.
