---
type: principle
created_date: 2026-08-08T07:41:35Z
last_modified: 2026-08-08T07:41:35Z
authority: accepted
universality: domain-wide
tags: boot, repair, cli, recovery, agent-trust
---

# Repair verbs operate below boot

## Statement
Any verb that is the named remedy for a boot-blocking state must run on exactly the workspace whose boot it repairs. A below-boot verb loads the workspace *description* (mount roster) and touches only configuration and schema storage — never mem content, never entities. "Below boot" means below workspace load, never outside the engine's write discipline: below-boot forms live in the engine (`memstead-git-branch/src/repair.rs`) and share the booted path's implementations — one target-ref resolver, one config-pin writer, one package-validation gate — so the booted and below-boot paths cannot fork into two validation regimes. Where an operation genuinely needs a booted engine (e.g. `projection migrate`'s reconcile-cursor seeding), it defers with a typed notice naming the follow-up rather than deadlocking or silently dropping state.

## Scope
Every CLI verb that a boot-failure message names as its repair command — today `memstead mem set-schema` (below-boot fallback when boot fails), `memstead schema install` (never boots), and `memstead projection migrate` (typed cursor deferral). MCP counterparts are excluded: an MCP server on an unbootable workspace is the quarantine-boot concern; the CLI is the floor an agent falls back to. A future repair verb that boots the workspace is a regression against this principle.

## Relationships
- **REFERENCES**: [[type-boot-failures-at-the-seam-with-one-shared-message-across-surfaces]]

## Justification

During the 2026-08-06/07 plenum outage, both escape routes the engine itself named — `memstead schema install` and `memstead mem set-schema` — failed on exactly the boot they were supposed to repair; the workspace was saved only by hand-copying a schema package that happened to survive in another checkout's build directory. An error message that names a repair command which cannot run in the failed state is worse than no message: it burns the agent's trust in every other repair command the engine names. The engine already carried the correct pattern (`memstead projection migrate` deliberately operates below engine boot); this principle generalizes it. Realized alongside [[engineering--type-boot-failures-at-the-seam-with-one-shared-message-across-surfaces]], which makes boot failures name these commands — the naming is truthful only while this principle holds.

## Exceptions



## Consequences

A dedicated `memstead repair` verb stays rejected: boot-failure messages name the ordinary verbs, so the ordinary verbs must work where they are named — a second entry point would split the recovery path. Below-boot `set-schema` skips the booted path's entity-conformance gate (entities are unreadable before boot); the trade is explicit in its output (`conformance_checked: false`) and the next green boot's health carries any findings. Repair never force-writes a pin that resolves nowhere — the shared resolver refuses with the same `SCHEMA_NOT_FOUND` trail the boot produces.
