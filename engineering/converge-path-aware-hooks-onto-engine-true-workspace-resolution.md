---
type: decision
created_date: 2026-07-13T16:43:03Z
last_modified: 2026-07-13T16:44:00Z
status: accepted
decided_on: 2026-06-09
deciders: dasboe
scope: subsystem
tags: plugin, hooks, guard, mem-resolution, convergence
---

# Converge path-aware hooks onto engine-true workspace resolution

## Decision
We converged the four path-aware hooks — [[plugin--entity-edit-guard-hook]], [[plugin--entity-bash-guard-hook]], [[plugin--realization-check-hook]], and [[plugin--context-injection-hook]] — onto a shared resolver (`workspace-resolve-utils.mjs`) that locates the workspace the way the engine does: a walk-up for `.memstead/workspace.toml` plus a `.mcp.json` `cd`-target probe, then reads `.memstead/state/mounts.json` for folder-backed mem dirs. This replaces the `--mem`-only `.mcp.json` scan and its non-existent `./specs` fallback, reversing [[engineering--defer-convergence-of-path-aware-hooks-onto-walk-up-mem-resolution]]. The mechanism is documented in [[plugin--path-aware-hook-workspace-resolution]].

## Context
The deferral [[engineering--defer-convergence-of-path-aware-hooks-onto-walk-up-mem-resolution]] left the entity-mutation guards fail-open on every workspace the plugin bootstraps, holding [[plugin--entity-mutation-must-route-through-the-engine]] at `violated`. The operator scheduled the convergence and it was implemented and tested this session.

## Consequences
- The entity-edit and entity-bash guards fire at the tool call on folder-backed workspaces; [[plugin--entity-mutation-must-route-through-the-engine]] returns to `verified`.
- On git-branch workspaces the resolver yields no folder dirs and the file/shell guards are correctly inert — entities are branch blobs with no working-tree file to edit; mem-repo integrity there rests on [[engineering--engine-owns-mem-repo-state]] and the CI [[plugin--architecture-guard-check-script]].
- A second engine-true resolver now exists alongside `inject.mjs` and the drift hooks' `.memstead.toml` walk-ups; full convergence of those remains open.
- The shared resolver carries its own fixture tests (`workspace-resolve-utils.test.js`), so a regression to the fallback behaviour is caught in CI.

## Relationships
- **REFERENCES**: [[plugin:entity-edit-guard-hook]]
- **REFERENCES**: [[plugin:entity-bash-guard-hook]]
- **REFERENCES**: [[plugin:realization-check-hook]]
- **REFERENCES**: [[plugin:context-injection-hook]]
- **REFERENCES**: [[defer-convergence-of-path-aware-hooks-onto-walk-up-mem-resolution]]
- **REFERENCES**: [[plugin:path-aware-hook-workspace-resolution]]
- **REFERENCES**: [[plugin:entity-mutation-must-route-through-the-engine]]
- **REFERENCES**: [[engine-owns-mem-repo-state]]
- **REFERENCES**: [[plugin:architecture-guard-check-script]]
- **SUPERSEDES**: [[defer-convergence-of-path-aware-hooks-onto-walk-up-mem-resolution]]
- **MOTIVATED_BY**: [[plugin:entity-mutation-must-route-through-the-engine]]

## Options

- Converge onto a shared engine-true resolver now — chosen; restores runtime enforcement on folder workspaces and is covered by tests.
- Keep the `--mem`-only scan — rejected; the prior deferral, now reversed, left a security-relevant fail-open with only CI as a backstop.
- Reuse `inject.mjs`'s existing `findWorkspaceRoot` verbatim — rejected; it returns a single workspace root and keys on the plugin-side `.memstead.toml` companion file (which is present in real workspaces, so it does resolve them), whereas the guards need the per-mem folder dirs — with git-branch and archive mems excluded — that only the engine mount list provides. The shared resolver therefore keys on the engine's own `.memstead/workspace.toml` marker and reads `.memstead/state/mounts.json`.

## Notes


