---
type: decision
created_date: 2026-09-02T02:43:35Z
last_modified: 2026-09-02T02:43:35Z
status: accepted
decided_on: 2026-09-02
deciders: execute-graph-plan loop, evidence-engine bundle
scope: subsystem
tags: export, traversal, ui-api, read-surface
---

# A chain export is a filter over the existing renderers, resolved once by one walker

## Decision
We chose to render a chain, the subgraph reachable from one root along a named rel-type set in one direction, as a FILTER over the surfaces that already exist rather than as a new verb or a new tool. One resolver, `Engine::chain_set`, validates the scope (mounted mem, a real root in that mem, at least one rel-type, every rel-type in the mem's schema vocabulary) and walks it through `reachable_via`, the primitive `memstead_search`'s `expand_via` already uses, so direction means the same thing on every surface: applied at every hop, a pure transitive closure. The resolved set then feeds the existing renderers through one scoped variant each: `render_html_export_scoped`, `render_llms_txt_scoped`, `mem_topology_scoped`, and the CLI's json export; `None` is the whole mem and every unscoped path stays byte-identical. On the CLI this is `memstead export --root <id> --via REL[,REL] [--direction out|in|both] [--depth N]` for the json, html and llms-txt formats; on the web app's path it is the same four query parameters on `GET /mems/{mem}/topology`, whose reduced projection is the chain's induced subgraph (nodes reachable in the mem, edges with both ends in the chain, cross-mem targets marked). The json export carries a `chain` block with the same node and edge set, so the two surfaces can be compared directly, and every rendered entity keeps its metadata, sections, relationships and, in json, its anchors with live state. References to entities outside the chain stay unresolved rather than rendering as links to pages the document does not contain. Refusals are typed and reuse existing codes: `INVALID_REL_TYPE` naming the vocabulary, `ENTITY_NOT_FOUND` for the root, `INVALID_INPUT` for a root without rel-types or the reverse. The MCP surface is unchanged: bulk export stays CLI-only by the standing decision.

## Context
An auditor of an investigative evidence mem asks one question of the graph: what does this conclusion rest on, and does each link still hold. Before this decision no surface answered it whole. Exports rendered a mem entire; `memstead search` walked rel-types only from text hits and returned hits without sections or anchors; `memstead relations` and the served entity page showed one hop; the ui-api topology was whole-mem; the app's tree builder handled one acyclic rel-type. The pieces were all present, in particular the `reachable_via` walker with its per-hop direction contract, and the three renderers. The design question was whether to add a verb, a tool, or an app-side assembly, and each of those would have duplicated something: a `chain` verb duplicates three renderers, a tool contradicts the recorded decision that keeps bulk export off the agent surface, and client-side assembly puts a graph walk into a client that cannot see anchors.

## Consequences
- One question, one export: an auditor gets the chain with sections and anchor states in a single document, in the format they already use.
- Direction semantics cannot diverge between search and export, since both call the same walker with the same enum.
- Every unscoped export and the unscoped topology are byte-identical to before; the scoped variants are the only new code paths, and `None` is the shared implementation.
- The reduced topology is the chain's induced subgraph, not the chain's spanning tree: an edge between two reached nodes appears even when the walk did not traverse it, because the subgraph is what the app draws.
- Links to entities outside the chain render in each format's existing unresolved form (HTML marker, plain text in llms-txt); changing those forms would have altered unscoped output for pre-existing dangling links and was not done.
- A root outside the exported mem refuses; the chain still crosses mem boundaries through edges, with cross-mem members kept in the set so their edges render as reached targets.
- The ui-api contract grew four optional query parameters; the app's generated types followed, and the app's tree builder is unchanged (a chain over a rel-type set is the engine's job).

## Relationships
- **INFORMED_BY**: [[keep-bulk-export-and-mem-rename-cli-only-off-the-mcp-surface]]

## Options

- Extend `memstead_search` with `related_to` plus `expand_via` on a seed: rejected as the solution, hits carry no sections and no anchors, so the auditor would still read every node.
- A dedicated `chain` verb: rejected, the export formats exist and the reduced set is a filter over them; a verb duplicates three renderers.
- Client-side assembly in the app over one rel-type: rejected, the app's tree builder handles one acyclic rel-type and a chain over a rel-type set belongs to the engine.
- Include incoming edges by default: rejected, direction is the caller's choice and the default is what the caller names (`out`).
- An MCP tool for the chain: rejected, bulk export stays off the agent surface by the standing decision; the app's path is ui-api.
- Chosen: one resolver over the existing walker, one scoped variant per existing renderer, the same four parameters on the CLI and the ui-api.

## Notes

Landed in the engine's 0.15.0 line (`graph/chain.rs`, the scoped renderer and topology variants, the CLI flags, the changelog and regenerated references) and in the private `ui-api` (query parameters, regenerated `openapi.json`) with the app's `api-types` regenerated. Tests cover the set semantics per direction and depth, the refusals, the byte-identity of the unscoped paths, the reduced renderings, and the ui-api endpoint.
