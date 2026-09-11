---
type: decision
created_date: 2026-09-11T00:05:15Z
last_modified: 2026-09-11T00:05:15Z
status: accepted
decided_on: 2026-09-11
deciders: architecture-seams bundle plan 05 (agent, under the standing engine-change rule)
scope: subsystem
tags: registry, projection, topology, published-artifact
---

# The registry's published graph derives from the engine's per-mem projection and keeps its own layout

## Decision
We will derive the nodes, edges and communities of the registry's published graph.json from the engine's shared per-mem topology projection, made a store-level function for that purpose (project_mem_topology over any store and partition; Engine::mem_topology is that function over the live store, [[engine--bulk-per-mem-topology-projection]]), and keep the registry's deterministic 3D layout as a pass applied on top. The manifest's published counts come from the same projection under the same live filter (published_counts), so the manifest and the graph payload cannot disagree. A contract test byte-compares the published graph.json and manifest.json for a fixed, source-built fixture against committed goldens, regenerated only deliberately; the switch commit left the goldens untouched, which was the guarantee.

## Context
Three services carried three wire dialects of one projection: serve had moved onto Engine::mem_topology, ui-api consumed it in a third shape, and the registry re-derived nodes, edges and communities privately, justified by keeping the engine free to evolve its store. The 2026-09-10 assessment read that as a doctrine contradiction; both counter-checks corrected the reading: the registry's graph.json is an immutable published artifact with a deployed consumer, a different constraint from a live projection, so the layout stays and only the derivation moves. The registry had no contract test on the one artifact third parties download; ui-api had the house pattern for one.

## Consequences
- One derivation of the projection for every consumer; an evolution of the engine's store reaches the registry through the projection, not through a private walk.
- The registry's manifest counts and graph payload share one derivation (a grader's refutation found the recount walking the store on its own; closed the same session).
- The goldens proved byte-stable across macOS and Linux (registry-ci green on both switch commits), which settles the cross-platform question the layout's trig raised.
- Cost accepted: the goldens are one more artifact to regenerate deliberately when the wire changes; that is the point.

## Relationships
- **REFERENCES**: [[engine:bulk-per-mem-topology-projection]]

## Options

- Delete the server-side layout and let the viewer lay out, as serve and ui-api do: rejected, published archives are immutable and their consumer is deployed; the deterministic first paint is a recorded decision.
- Compare the goldens structurally instead of byte for byte: rejected, consumers receive bytes and a formatting change would pass unseen.
- Give the registry an OpenAPI document: rejected, the surface freeze of 2026-09-10 names a generated reference as a new surface.
