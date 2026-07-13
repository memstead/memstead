---
type: decision
created_date: 2026-07-13T16:43:07Z
last_modified: 2026-07-13T16:44:06Z
status: accepted
decided_on: 2026-07-03
deciders: memstead
scope: subsystem
tags: serve, read-surface, api, agent-first, anti-drift, engine
---

# Serve read-only state as HTML and MCP only without a JSON api channel

## Decision
We chose to expose the read-only serve surface as exactly two channels — link-navigated HTML pages for browsers, crawlers, and no-MCP agent fetches, and a native `/mcp` endpoint for MCP-capable agents — and to hard-remove the earlier content-negotiated JSON read channels (`/api/{overview,search,entity,entities,schema,health}`). The removed paths now return 404 on every `Accept` header; they are gone, not redirected.

## Context
The read surface targets two consumer classes: MCP-capable agents and everyone else (browsers, crawlers, no-MCP agent fetch tools). Agents already get structured, typed reads over the `/mcp` endpoint — the same tool surface they use everywhere else — so a parallel JSON `/api/*` read API served no consumer the MCP-plus-HTML pair did not already cover. Worse, it was a third rendering of engine state alongside the shared overview/entity/search composers the HTML pages re-render, and a third projection is a third thing that can silently drift from MCP and CLI. A brief content-negotiated JSON `/api` existed at the open-engine genesis; keeping it meant maintaining a wire shape with no distinct audience.

## Consequences
- Every `/api/*` read path 404s on both `text/markdown` and `application/json`; a dedicated test asserts the removal (`removed_api_routes_are_404`), so a re-addition cannot slip in unnoticed.
- One fewer read projection to keep aligned: the HTML pages and MCP tools share the engine's overview/entity/search composers, so the surface cannot drift from CLI/MCP.
- The read surface stays mutation-free and GET-only, reinforcing the safe-posture default for an unauthenticated public deployment.
- Cost accepted: a non-MCP program that wanted machine-readable JSON must now either speak MCP over `/mcp` or scrape the HTML. The bet is that any such consumer is better served by the native MCP endpoint.

## Relationships
- **REFERENCES**: [[engine:serve-read-only-http-wire-surface]]
- **REFERENCES**: [[engine:mcp-tool-surface]]
- **MOTIVATED_BY**: [[engine:mcp-tool-surface]]

## Options

- Keep the content-negotiated JSON `/api/*` read API alongside HTML and MCP — rejected: a third state projection with no audience the other two miss, and a standing drift risk against the shared composers.
- Redirect `/api/*` to the equivalent HTML page — rejected: masks the removal and hands an agent that requested JSON an HTML body it cannot parse; a clean 404 is the honest signal.
- Chosen: remove the routes entirely and return 404, verified by test.

## Notes

The current read surface is modeled by [[engine--serve-read-only-http-wire-surface]]; the native agent read path this decision leans on is the [[engine--mcp-tool-surface]].
