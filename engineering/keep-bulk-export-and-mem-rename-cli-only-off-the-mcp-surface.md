---
type: decision
created_date: 2026-08-07T09:09:29Z
last_modified: 2026-08-07T09:09:29Z
status: accepted
decided_on: 2026-08-06
deciders: operator (agent-toolbox bundle decision 3), implementing agent
scope: component
tags: mcp, tools, cli, abstention, agent-ergonomics
---

# Keep bulk export and mem rename CLI-only off the MCP surface

## Decision
We chose to ship two agent-toolbox capabilities as CLI-only, deliberately absent from the [[engine--mcp-tool-surface]]: **bulk read** — `memstead export --format json` emits a mem's (or the whole workspace's) complete non-stub entity set as one JSON document in one engine boot — and **`memstead mem rename`** — the complete, atomic-per-affected-mem rename that sweeps entity-id prefixes, cross-mem edges, wiki-links, grants, router, bindings, sync-state, and findings store. Both abstentions instantiate [[engineering--mcp-tool-surface-stays-small]], and the export abstention is machine-pinned by the tool_surface abstention test in `memstead-mcp/tests/tool_surface.rs` — a later addition must consciously retire the pin, not drift past it.

## Context
The 2026-08-06 agent-toolbox bundle fixed the MCP surface contract (operator decision 3): an MCP-only agent must be able to complete the full working loop on existing mems plus mem lifecycle, always within the configured workspace permissions — while distribution (export/publish/install), schema authoring/install, and `unregister` stay deliberately off MCP. The two capabilities sit outside that completeness bar for different reasons. Export: the CLI's per-call engine boot made bulk reads scale to hours, which is why an external field project read mems with raw `git --git-dir`; the MCP server has no such problem — it is warm — and a bulk dump into an agent context is an anti-feature regardless (agents use `memstead_search`/`memstead_entity` on the [[engine--cli-command-surface]]'s MCP counterpart). Rename: a rare curation act on the same tier as `unregister`; agents that need it have the CLI.

## Consequences
- The MCP tool count stays inside the budget the governing principle defends; new capability landed without new tools.
- External projections and check scripts get a legitimate bulk read path (one boot, one JSON document), retiring the raw-git workaround without touching the agent surface.
- The posture is revisit-on-evidence, not permanent: exposing rename over MCP was rejected for tool-count reasons and is explicitly reopenable on demonstrated need.
- The abstention is enforced, not conventional — the tool_surface test refuses a silently added `memstead_export`.

## Relationships
- **IMPLEMENTS**: [[mcp-tool-surface-stays-small]]
- **REFERENCES**: [[engine:cli-command-surface]]
- **REFERENCES**: [[engine:mcp-tool-surface]]
- **REFERENCES**: [[mcp-tool-surface-stays-small]]

## Options

- **Expose export over MCP** — rejected: the server is warm, so the per-call boot cost that motivated the feature does not apply there; a full-mem dump into an agent context works against the token-shaped read tools that exist precisely to avoid it.
- **Expose rename over MCP** — rejected on tool-count grounds under the governing principle; the surface-contract completeness bar places rename outside the MCP working loop.
- **A new bulk-read verb (`memstead dump`)** — rejected: `export` already means "hand the contents out"; a third content-egress verb splits discovery.
- **Rename as a documented delete-and-recreate recipe** — rejected: that is exactly the measured 40-minute hand migration, and it loses commit history and anchors.

## Notes

Shipped 2026-08-06 as public-repo commits `c48cd07` (JSON export; agent-toolbox plan 01) and `7fba177`/`43e9a7e`/`5221bef` (mem rename; plan 03); both independently graded with all criteria confirmed.
