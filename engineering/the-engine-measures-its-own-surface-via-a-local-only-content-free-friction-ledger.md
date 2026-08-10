---
type: decision
created_date: 2026-08-08T19:39:18Z
last_modified: 2026-08-10T13:32:16Z
status: accepted
decided_on: 2026-08-08
deciders: operator (agent-trust plan 08)
scope: subsystem
tags: friction, telemetry-boundary, privacy, agent-surface, health
---

# The engine measures its own surface via a local-only content-free friction ledger

## Decision
Every typed refusal the CLI or MCP surface returns is appended to a workspace-local friction ledger, and three boundaries on it are permanent commitments, not implementation details: (1) **closed-vocabulary** — every recorded field's value space is a closed, engine-defined vocabulary: values that exist as literals in engine source (`cli`/`mcp`, the subcommand or tool name, the UPPER_SNAKE_CASE refusal code, and — where a code computes one — a reason discriminator gated per-code by `closed_reason`'s vocabulary table) plus the epoch-seconds timestamp; parameters, entity ids, mem content, message text, and free-form strings never enter the ledger, enforced by type (reasons pass only as `&'static str`); (2) **local-only, forever** — the ledger lives under `.memstead/state/friction/` (gitignored via the state-tier's self-ignoring convention), is never transmitted anywhere, and no registry ever sees it; its sole read surface is the include-gated `friction` axis on `memstead health` and the MCP counterpart (both flavours) — no new tool; (3) **best-effort, never perturbing** — recording happens after the refusal envelope is built, at one seam per surface (CLI `main.rs` error exit, both MCP flavours' `call_tool` dispatch), every ledger failure is swallowed, and the refusal returns byte-unchanged whether or not the append landed. Successes are never recorded — this measures friction, not usage. Appends are single-write whole JSONL lines on an append-mode handle, so the project's normal concurrent state (a CLI invocation beside a running MCP server) interleaves entries without corruption; the size bound is two-generation rotation.

## Context
Every debate about whether the agent surface is learnable was a taste debate — the agent-trust bundle itself was prioritized off two field projects' manually written friction reports. The project's method ([[engineering--sync-stays-brief-driven-as-measured-against-a-goal-driven-arm]] is the precedent: measure first, change on evidence) demanded the same instrument for the surface: with refusal frequencies per code and per verb queryable, "is this schema confusing" and "did the capability-announcement work reduce blindness" become ledger queries. This is also the substrate the operator-obsolescence directive keeps asking for — machine gates need machine evidence, and refusal frequency IS the evidence for surface changes. The typed-refusal envelope ([[engine--typed-error-and-warning-envelope]]) made this cheap: the code is already on every refusal at the surface boundary; the state-tier precedent (findings store) already established engine-owned, gitignored, workspace-local residue.

## Consequences
Surface-design decisions gain a free evidence stream: the next friction retrospective reads `memstead health --include friction` instead of commissioning manual reports, and before/after comparisons for surface changes (capability announcements, schema examples) become measurable. Costs and limits accepted: no session/agent attribution (an identity dimension with privacy weight; per-code/per-verb counts answer the current design questions — if attribution is ever wanted it composes with the provenance work, not here); refusals that occur outside any resolvable workspace are not recorded (the CLI's pre-boot refusals in a bare directory have no ledger home — accepted, they are not surface-learnability signal); and the ledger measures refusal COUNTS, not outcomes — a high count can mean a confusing surface or one agent retrying in a loop, so reading it stays an act of judgment.

## Relationships
- **REFERENCES**: [[sync-stays-brief-driven-as-measured-against-a-goal-driven-arm]]
- **REFERENCES**: [[engine:typed-error-and-warning-envelope]]

## Options

Not building it (keep relying on anecdote) — rejected: three plan bundles in a row were prioritized off manually written friction reports; the ledger makes the next round's evidence free. Session-scoped attribution — rejected for privacy weight and lack of a consuming question. Recording into the mem-repo (versioned history) — rejected: operational telemetry is not knowledge and does not belong in a mem's history; the gitignored state dir is the established home for exactly this residue.

## Notes


