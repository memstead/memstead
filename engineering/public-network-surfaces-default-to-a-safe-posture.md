---
type: principle
created_date: 2026-07-13T16:43:06Z
last_modified: 2026-07-13T16:44:05Z
authority: accepted
universality: domain-wide
tags: security, serve, defaults, defense-in-depth, engine
---

# Public network surfaces default to a safe posture

## Statement
Every externally-reachable Memstead surface defaults to the narrowest exposure and least authority it can, and widens only on explicit configuration. A new public route or binary must default closed — broadening reach or write scope is always a deliberate, reviewable config act, never the fallback behaviour.

## Scope
The network-reachable HTTP + MCP surfaces the `memstead-serve` binaries expose: the read-only serve surface, the writable per-session sketch surface, and the unified `memstead-serve-full`. Does NOT govern the local stdio MCP server or in-process UniFFI — those are not network-reachable and carry their own trust model (a local operator with filesystem access).

## Relationships
- **GOVERNS**: [[engine:serve-read-only-http-wire-surface]]
- **GOVERNS**: [[engine:session-sketch-server-http-wire-surface]]

## Justification

These surfaces run public with no auth (the agent-first showcase is open by design), so safety rests on defaults rather than on an operator remembering to lock down. Concrete instances that share this posture: loopback-default bind (see the bind decision below), read-only-by-construction routing on the read surface (GET-only routes, a five-tool MCP allowlist, a sealed archive mount), the sketch session's write-scope (a 10-tool allowlist plus a read-only content mount refusing writes as a second guard), and per-IP rate limiting that refuses over-budget requests with a typed 429.

## Exceptions

A set `PORT` env (injected by Railway and most PaaS) deliberately binds all interfaces — container deploys are the intended broadcast case. An explicit `MEMSTEAD_SERVE_BIND` / `MEMSTEAD_SESSION_BIND` value wins verbatim, trusting the operator who set it.

## Consequences

A reader can assume any unconfigured public surface is in its safe state without auditing each route. Adding a surface that defaults open (a 0.0.0.0 bind, a writable route on the read tier, an unscoped tool list) is a violation visible across every serve binary, not a local style choice.
