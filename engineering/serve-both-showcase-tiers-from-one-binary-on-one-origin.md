---
type: decision
created_date: 2026-07-13T16:43:07Z
last_modified: 2026-07-13T16:44:06Z
status: accepted
decided_on: 2026-06-19
deciders: registry-team
scope: subsystem
tags: serve, deployment, showcase, single-origin, memstead.ai, railway, engine
---

# Serve both showcase tiers from one binary on one origin

## Decision
The agent-first two-tier showcase ships as a single binary, `memstead-serve-full` (a binary of the private serve crate since the 2026-07-04 privatization), that serves the human site, the read-only HTML read pages + read `/mcp` ([[engine--serve-read-only-http-wire-surface]]), and the writable connection-born session `/mcp` + view-data routes ([[engine--session-sketch-server-http-wire-surface]]) from one process under one origin. The composition is `build_unified_app`: it `merge`s the read-only `build_router` (which carries the static-site fallback and the HTML read pages) with `session::sketch_router` (which carries `/mcp` and the `/v/{id}/graph|stream|export` data routes plus only a default 404 fallback), then wraps the whole surface in a single per-IP `tower_governor` rate limiter keyed by `SmartIpKeyExtractor`. The read engine in `AppState` backs the read HTML pages; the `SessionRegistry` backs `/mcp` and the view data. The standalone single-surface binaries (`memstead-serve` read-only, `memstead-session-serve` writable) are retained for deployments that want one tier alone.

## Context
The showcase has two surfaces — a public read tier over a curated graph and a per-visitor writable sketch tier — and a human website. memstead.ai is hosted on Railway (one service, one injected `PORT`). Splitting the surfaces across separate backends would force an edge/reverse-proxy to splice them under one origin so the browser viewer (fetching `/v/{id}/graph` and `/v/{id}/stream`) and the website share a host; the writable `/mcp` and the read surface (the HTML read pages + the read-only `/mcp`) would otherwise live on different origins, complicating CORS, rate-limit accounting, and deployment.

## Consequences
- A single Railway deployment serves the entire showcase: site + read HTML pages + writable MCP + live-view SSE, no edge to wire two backends together. The MCP handshake can hand back a relative `/v/{id}` live-view URL because the viewer is same-origin.
- One per-IP rate limiter spans both tiers, so a visitor's budget is shared across read and write — accounting is unified rather than split per surface.
- The merge relies on only the read router carrying a static-site fallback and the sketch router carrying just the default 404, so the two route tables compose without a fallback conflict; adding a second catch-all fallback to either side would break the merge.
- Two env families must both be configured for the full binary: `MEMSTEAD_SERVE_*` (read side: authority, archive/mem, bind) and `MEMSTEAD_SESSION_*` (sketch side: content mount, TTL, entity cap, session ceiling, view base, rate limits). Operators running one tier in isolation still use the dedicated single-surface binaries.
- The read engine and the session registry are independent in one process: the read tier serves a sealed archive (or the embedded content folder) over its HTML pages while each session mints its own in-memory engine, so the two tiers' memory and reload behaviour stay decoupled despite sharing the origin.

## Relationships
- **REFERENCES**: [[engine:serve-read-only-http-wire-surface]]
- **REFERENCES**: [[engine:session-sketch-server-http-wire-surface]]
- **MOTIVATED_BY**: [[engine:memstead-serve-crate]]
- **IMPLEMENTS**: [[engine:serve-read-only-http-wire-surface]]
- **IMPLEMENTS**: [[engine:session-sketch-server-http-wire-surface]]

## Options

- **One unified binary, one origin** (chosen): `memstead-serve-full` merges both routers; the deployment is a single Railway service.
- **Two separate deployments behind an edge**: run `memstead-serve` and `memstead-session-serve` independently and splice them under one host with a reverse proxy/CDN edge. Rejected for the showcase — it adds an edge component and cross-origin coordination for no benefit at this scale, and the dedicated binaries already exist for anyone who wants the split.

## Notes

Both single-surface binaries remain first-class — this decision adds the unified composition for the showcase deployment, it does not retire the standalone surfaces. The curated read graph both tiers default to is the compile-time-embedded content mem (`EMBEDDED_CONTENT` / `materialize_embedded_content` / the compile-time-embedded content mem), so a Rust-only checkout serves real content with no external files.


Soft-launch gate (fixed 2026-07-02): `memstead-serve-full` now reads `MEMSTEAD_SOFT_LAUNCH` — default ON, `0`/`off`/`false` to go public, the identical parse `memstead-session-serve` already used — and threads it into `AppState::with_soft_launch`, relocating read pages and the sketch `/mcp` mount under `/try/…`. Before the fix the binary always served the public routes while the embedded static face (built with the same variable defaulting ON) emitted `/try`-prefixed links, so the served connect instructions 404ed live. The face and the binary MUST agree: one env var set once governs both the face's build and the binary's runtime routes.
