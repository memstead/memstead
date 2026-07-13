---
type: decision
created_date: 2026-07-13T16:43:03Z
last_modified: 2026-07-13T16:44:00Z
status: superseded
decided_on: 2026-06-08
deciders: dasboe
scope: subsystem
tags: plugin, hooks, guard, mem-resolution, deferral, tech-debt
---

# Defer convergence of path-aware hooks onto walk-up mem resolution

## Decision
We will leave the path-aware hooks — [[plugin--entity-edit-guard-hook]], [[plugin--entity-bash-guard-hook]], [[plugin--realization-check-hook]], and [[plugin--context-injection-hook]] — on their shared `--mem`-only `.mcp.json` scan with a `./specs` fallback, rather than converging them onto the walk-up workspace resolution that the ingest skill's `inject.mjs` `findWorkspaceRoot` already performs. The fail-open this leaves on every plugin-bootstrapped workspace is accepted as known debt for now.

## Context
No `.mcp.json` shape a current workspace actually emits carries a `--mem` arg (the engine binaries accept no such flag), so the `--mem` scan always returns empty and the dependent hooks fall back to a `./specs` directory that does not exist on the lean `<mem-name>/<slug>.md` layout. The guards thereby fail open and interview re-injection never fires — the defect documented in [[plugin--path-aware-hook-workspace-resolution]]. A correct resolver already exists in the plugin (`inject.mjs`'s `findWorkspaceRoot` walks up for a direct `.memstead.toml`, then for a `cd <dir>` launch-command form — the reachable real-config shapes; its `--config` branch is vestigial, matching nothing a real workspace emits, per [[plugin--path-aware-hook-workspace-resolution]]), so the fix is a known convergence, not a research problem. The choice is whether to do that convergence now or defer it.

## Consequences
- The entity-edit, entity-bash, and realization-check guard hooks remain effectively inert on every workspace the plugin bootstraps; the [[plugin--entity-mutation-must-route-through-the-engine]] requirement stays `violated` at the runtime surface until this is reversed.
- The [[plugin--architecture-guard-check-script]] CI guard is the only enforcement of the mutation invariant that currently holds, so a direct-edit regression is caught in CI but not at the agent's tool call.
- Deferring avoids touching four hooks plus their shared utils and tests in a session focused elsewhere; the cost is carrying a security-relevant fail-open with no runtime backstop.
- Reversal is a bounded refactor: point `findAllMemDirs` (or its callers) at a walk-up that locates `.memstead/workspace.toml` from cwd, as the engine itself does, and update the `./specs` fallback.

## Relationships
- **REFERENCES**: [[plugin:entity-edit-guard-hook]]
- **REFERENCES**: [[plugin:entity-bash-guard-hook]]
- **REFERENCES**: [[plugin:realization-check-hook]]
- **REFERENCES**: [[plugin:context-injection-hook]]
- **REFERENCES**: [[plugin:path-aware-hook-workspace-resolution]]
- **REFERENCES**: [[plugin:entity-mutation-must-route-through-the-engine]]
- **REFERENCES**: [[plugin:architecture-guard-check-script]]
- **MOTIVATED_BY**: [[plugin:entity-mutation-must-route-through-the-engine]]

## Options

- Converge the hooks onto the existing walk-up resolution now (reuse `inject.mjs`'s `findWorkspaceRoot` shape) — closes the fail-open but was not in scope for the session that surfaced it; deferred.
- Keep the `--mem`-only scan — chosen; lowest immediate effort, leaves the fail-open and relies on the CI architecture-guard as the sole live enforcement.
- Make the guards hard-fail (block) when no mem dir resolves instead of falling back to `./specs` — rejected for now: would block legitimate edits on every bootstrapped workspace until the resolver is fixed, trading a silent fail-open for a loud fail-closed that breaks normal use.

## Notes

Revisit when the hook-resolution convergence work is scheduled. The deferral rationale lives in the git history of the hook-relocation work; this entity surfaces the choice so the requirement's `violated` status has a recorded cause.
