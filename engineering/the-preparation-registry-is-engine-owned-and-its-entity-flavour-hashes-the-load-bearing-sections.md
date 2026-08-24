---
type: decision
created_date: 2026-08-23T22:05:01Z
last_modified: 2026-08-24T01:07:54Z
status: accepted
decided_on: 2026-08-24
deciders: implementing agent, executing the operator-authored preparation plan bundle
scope: subsystem
tags: preparation, anchors, binding, registry, drift
---

# The preparation registry is engine-owned and its entity flavour hashes the load-bearing sections

## Decision
We chose to realize the source `preparation` slot as an engine-owned registry (`memstead-base::preparation`) consulted at exactly two touchpoints: anchor observation, which asks the registry for the prepared form an artifact hashes as, and ingest delivery, which will ask it for a source's unit sequence (reserved, no entry yet). The refusal is narrowed to "identifier not in this engine's registry" (`PreparationUnsupported`, same shape, the registered set named) on every edit path, and mirrored on the brief-render path for a hand-edited record; a registered identifier over a medium whose anchor namespace admits none of its grains refuses too (`PreparationGrainMismatch`). The first entry, `entity-load-bearing`, defines an entity-grain anchor's prepared form as the stable serialization of the type's load-bearing sections, keyed by section key in the type's declared order, resolved as: sections declaring the new schema flag `load_bearing: true`, else the required sections minus any declaring `load_bearing: false`, else every section. A source declaring nothing observes byte-for-byte as before (the canonical rendered markdown). The `url` grain acquires its prepared form through the same canonicalization the path grains use, over content the observer supplies at write time (`AnchorInput.content`, additive on the MCP and CLI anchor shapes; the engine computes the hash, never fetches), and defaults to `hash_stability: unstable`. `PREPARATION_IMPL_VERSION` bumps globally to 1, superseding every prior finding by construction.

## Context
The graded design `engineering--one-preparation-slot-three-flavours-two-engine-touchpoints` (2026-08-22) fixed the shape: one registry, two touchpoints, refusal narrowed, entity and url grains first, a global impl-version bump. This decision records the first implementation slice of it: the registry, the entity flavour, the url entry, the impl-version bump. Two facts forced the concrete choices recorded here. First, W5's entity-anchor observation already hashed an entity's canonical rendered markdown, so a comma in a notes section drifted every dependent (the anker channel's finding 5: false-positive drift to zero); the registry had to change the prepared form for declaring sources only, leaving undeclaring sources untouched. Second, the engine cannot observe a `url` artifact (the web medium is neither enumerable nor base-retrievable in the capability matrix), so the url grain's prepared form can only come from what its observer read. Note for the record: on the machine that executed plan 01 the design entity named above was absent from every store (graph, `public/engineering/`, mem-repo branches, remotes); it was authored on another machine and not yet hand-committed. This decision therefore links to the engine surfaces it changes, [[engine--anchor-primitive]] and [[engine--pipeline]], not to the design.

## Consequences
- Every binding's `hash(D)` changed with the impl-version bump: prior findings are segregated as superseded and re-derived by the next verify; anchors' recorded `binding` hashes no longer resolve until the next build or sync writes fresh ones (per-source measurement degrades gracefully in between).
- A graph source declaring `entity-load-bearing` gets the anker metric mechanically: notes-only edits keep dependents resolving, load-bearing edits drift them. The standalone `verify-anchors` inherits it through the shared observation site.
- `load_bearing` is a new optional schema-section flag; the committed type-definition meta-schema regenerated. Existing schemas need nothing: required sections are the default load-bearing set.
- A `url` anchor gains a prepared hash only when its writer supplies `content`; engine-side observation of url anchors stays `None` (unobserved), never fabricated. A url anchor's hash break resolves `recheck` by default.
- An unregistered identifier reaching a record by hand yields unobserved entity anchors (the form cannot be computed) and a skipped brief, never a fabricated hash.
- The binding reference page renders the registry from the engine (`xtask generate-docs`), so the documented roster cannot lag the code.
- Delivery preparation (touchpoint B) and code-map preparation (touchpoint A, path grains) follow in plans 02 and 03; each landed implementation bumps the impl version once more.

## Relationships
- **REFERENCES**: [[engine:anchor-primitive]]
- **REFERENCES**: [[engine:pipeline]]
- **REFERENCES**: [[delivery-preparation-delivers-dated-entries-as-units-in-one-stamp-ordered-sequence]]
- **REFERENCES**: [[code-map-preparation-hashes-a-heuristic-interface-digest-and-closes-the-tree-grain-for-code-sources]]

## Options

- Hash the impl version only on sources that declare a preparation, sparing undeclaring bindings the invalidation: rejected. The design binds a global bump, and "which preparation implementation is live" is an engine-wide fact; findings are re-derivable measurements, so the cost is one verify pass.
- Load-bearing sections equal required sections, no schema flag: rejected. A schema author needs to opt an optional section in (a `claim` that is not required) or a required bookkeeping section out; the flag is a few lines and the required-sections default keeps every existing schema working unchanged.
- Let the engine fetch URLs at observation time: rejected. The web medium's capability row advertises no enumeration and no retrievable base; fetching would make verify network-dependent and non-deterministic. The observer supplies content; the engine canonicalizes and hashes.
- A dedicated MCP tool to compute a prepared hash from content: rejected under the tool-count policy. An additive optional `content` field on the existing anchor input does the same without a new tool or a shape change.
- Refuse the brief-render path entirely once validation refuses at declaration: rejected. A hand-edited record bypasses every edit path; the render mirror is what keeps "nothing silently half-runs" true.

## Notes

Landed in `memstead-base::preparation` with tests pinning: registry membership, the per-grain stability default, the url canonicalization identity, explicit-then-required-then-all resolution, the notes-versus-claim metric, the end-to-end drift contract over a folder mem with a graph binding (including the unregistered-identifier complement), the hash-identity change, and the findings-store invalidation.


2026-08-24, later the same day: touchpoint B's first entry landed; "reserved, no entry yet" above describes the state at this decision, not the current one. See [[engineering--delivery-preparation-delivers-dated-entries-as-units-in-one-stamp-ordered-sequence]]; `PREPARATION_IMPL_VERSION` is 2 since.


2026-08-24, later still: the code-map flavour landed as touchpoint A's second entry, see [[engineering--code-map-preparation-hashes-a-heuristic-interface-digest-and-closes-the-tree-grain-for-code-sources]]; `PREPARATION_IMPL_VERSION` is 3 since.
