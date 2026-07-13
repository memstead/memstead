---
type: decision
created_date: 2026-07-13T16:43:02Z
last_modified: 2026-07-13T16:43:59Z
status: accepted
decided_on: 2026-07-03
deciders: dasboe
scope: subsystem
tags: serve, security, bind, deployment, engine
---

# Bind serve listeners to loopback unless a port is injected

## Decision
We chose to resolve every serve binary's listen address through one shared `resolve_bind` helper (part of the private serve crate since the 2026-07-04 privatization): an explicit `MEMSTEAD_SERVE_BIND` / `MEMSTEAD_SESSION_BIND` value wins verbatim; otherwise a set `PORT` (Railway and most PaaS inject it) binds all interfaces on that port (`0.0.0.0:$PORT`); otherwise the listener falls back to loopback `127.0.0.1:8080`. The resolved address is logged at startup so the polarity is always visible. This realizes [[engineering--public-network-surfaces-default-to-a-safe-posture]] for the transport layer.

## Context
The [[engine--memstead-serve-crate]] binaries — read-only serve, session sketch, and unified serve-full — previously bound `0.0.0.0:8080` unconditionally. A local experimenter running the writable sketch server broadcast a no-auth, writable MCP endpoint to their LAN without intending to. The default posture had to be the safe one, without breaking container deployments that legitimately need all-interface binding.

## Consequences
- A local run with no `PORT` and no explicit bind is reachable only from the host — a writable endpoint no longer leaks to the LAN by accident.
- Container/PaaS deploys are unchanged: the injected `PORT` still binds all interfaces, so nothing downstream needed rewiring.
- Operators who need a specific bind set `MEMSTEAD_SERVE_BIND` / `MEMSTEAD_SESSION_BIND` explicitly and that value is trusted verbatim.
- The startup log line naming the bound address is the operator's confirmation of which polarity resolved.

## Relationships
- **REFERENCES**: [[public-network-surfaces-default-to-a-safe-posture]]
- **REFERENCES**: [[engine:memstead-serve-crate]]
- **MOTIVATED_BY**: [[public-network-surfaces-default-to-a-safe-posture]]

## Options

- Bind `0.0.0.0:8080` unconditionally (prior behaviour) — rejected: any local run of the writable sketch server exposes a writable endpoint to the whole LAN.
- Bind loopback always, require an explicit env to broadcast — rejected: every PaaS deploy would need extra wiring, and the containers that inject `PORT` are exactly the intended-broadcast case.
- PORT-gated default (chosen): explicit bind wins; a set `PORT` broadcasts on that port; else loopback. Container deploys are unchanged, local experiments are safe by default.

## Notes


