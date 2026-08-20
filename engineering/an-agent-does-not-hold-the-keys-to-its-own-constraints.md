---
type: principle
created_date: 2026-08-20T15:29:00Z
last_modified: 2026-08-20T15:29:00Z
authority: established
universality: domain-wide
tags: mcp, trust-model, workspace-policy, tool-surface, operator
---

# An agent does not hold the keys to its own constraints

## Statement
A switch that decides what an agent is permitted to do does not appear on that agent's own tool surface. Workspace policy — which mems may be created or deleted, which cross-mem links are granted — is the operator's decision about the agent, so it is reachable from the CLI and never from MCP.

The division of labour is the refusal contract: a policy-gated mutation refuses with a typed code and the exact command that would unblock it; the agent reports that command; a person runs it. The refusal is the whole mechanism — nothing is hidden from the agent except the ability to grant itself permission.

## Scope
The MCP tool surface of every deployment. Distinct from **mem lifecycle** (`memstead_mem_create` / `_delete` / `_set_schema` / `_set_version`), which stays on the agent surface: creating a mem is work the operator has already authorised by writing an allowlist rule. The line is not "mutation vs read" but "acting within a permission vs editing the permission".

## Justification

Symmetry with the other mutation families is the reasoning that put the six `memstead_workspace_*` tools on the surface in the first place, and symmetry is the wrong axis here. An agent that can call `memstead_workspace_allow_create` can grant itself the rule that makes its next refused `memstead_mem_create` succeed — the gate becomes advisory, and the operator's intent survives only as long as no agent thinks to route around it. No capability is lost by removing them, because the CLI carries the same engine functions; only wire exposure changes.

The removal also serves the tool-count budget — the pro server went from 25 tools to 19 — but that is a secondary benefit. The primary argument would hold at any tool count.

## Exceptions

None on MCP. A future non-agent programmatic surface (a management API, an operator UI) may carry policy editing: the constraint is about who holds the switch, not about which protocol carries it.

## Consequences

A refusal that names a remedy names one the reader can act on. When the six tools existed, `MEM_PATH_NOT_ALLOWED` carried `details.remedy.mcp` pointing at a tool the agent could call; after removal that field would have pointed at nothing, so the remedy now carries the CLI command alone and states that policy is an operator act.

A deliberate surface asymmetry has to be legible as deliberate. The parity matrix could not distinguish a considered exception from an oversight — both rendered as an empty cell — so the operations registry gained a general `rationale` field, marking the row and rendering a footnote. Any future operation that is CLI-only on purpose says so the same way.
