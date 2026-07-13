---
type: decision
created_date: 2026-07-13T16:43:08Z
last_modified: 2026-07-13T16:44:07Z
status: accepted
decided_on: 2026-07-03
deciders: memstead-core
scope: component
tags: cli, init, schema-pin, filesystem-mem, lean, engine
---

# Warn rather than refuse when memstead init pins an uninstalled schema

## Decision
We chose to let `memstead init` write a filesystem-mem workspace whose `--schema` pin resolves to no currently-loaded schema, emitting a loud `SCHEMA_NOT_FOUND` warning rather than refusing. The pin is validated for shape (exact `name@version`) but not for resolvability; the workspace lands, and the warning names the recovery command (`memstead schema install <package-dir>`, run inside the new workspace). The sibling registration paths on the [[engine--cli-command-surface]] — `memstead mem init` and the MCP `memstead_mem_create` — refuse an unresolved pin instead.

## Context
`memstead init` bootstraps a filesystem-mem before any `.memstead/schemas/` directory exists, and `memstead schema install` only operates *inside* an already-initialised workspace (the write side of [[engine--schema-storage-source-seam]]). On the lean build there is additionally no `mem set-schema` to re-pin later. So for any pin outside the [[engine--built-in-schema-catalogue]], the only path to a custom-schema mem is init-with-pin followed by install — which is impossible if init refuses an unresolved pin. The sibling `create_mem` / `memstead_mem_create` path ([[engine--mem-lifecycle-operations]]) runs against an already-existing workspace where install-before-pin is always possible, so it can afford to resolve the pin eagerly and refuse.

## Consequences
- The lean custom-schema authoring flow works: `memstead schema new` -> `schema validate` -> `memstead init --schema <new>@<v>` -> `schema install`, exactly the lean follow-up the [[engine--cli-cold-start-quickstart-and-schema-scaffold-surface]] documents.
- A silent trap is avoided: without the warning, init would report success and every later engine-booting command would die on `SCHEMA_NOT_FOUND` with no hint how the workspace reached that state.
- Cost: init can leave a valid-but-unbootable workspace that no engine-booting command opens until the schema is installed — an intermediate state the eager-refuse sibling paths never produce.
- The two init verbs now diverge on pin handling (init warns, `mem init` refuses); the divergence is deliberate and recorded here rather than read as an inconsistency.

## Relationships
- **REFERENCES**: [[engine:cli-command-surface]]
- **REFERENCES**: [[engine:schema-storage-source-seam]]
- **REFERENCES**: [[engine:built-in-schema-catalogue]]
- **REFERENCES**: [[engine:mem-lifecycle-operations]]
- **REFERENCES**: [[engine:cli-cold-start-quickstart-and-schema-scaffold-surface]]
- **REFERENCES**: [[refuse-memstead-init-in-a-non-empty-folder-rather-than-adopting-existing-files]]
- **MOTIVATED_BY**: [[engine:cli-cold-start-quickstart-and-schema-scaffold-surface]]

## Options

- **Refuse eagerly (as `create_mem` does)** — resolve the pin at init and reject an unresolved one: rejected; it makes the lean custom-schema flow impossible, because the schema can only be installed after the workspace exists.
- **Silently proceed** — write the workspace with no warning: rejected; the next engine-booting command fails with a bare `SCHEMA_NOT_FOUND` and no trail back to the init-time pin choice.
- **Warn and proceed** — chosen: the workspace lands, the stderr warning names the recovery command, and an additive `warnings` field carries the same `SCHEMA_NOT_FOUND` code on the `--json` surface (a stable-shape addition, not a response-shape fork).

## Notes

Enforced in `crates/memstead-cli/src/commands/init.rs` (`unresolved_pin_warning`, the `pin_unresolved` guard, and the additive `warnings` JSON field). The sibling eager-resolve-and-refuse lives in the engine's mount-load path (`crates/memstead-base/src/engine/lifecycle.rs`, `SchemaResolver::resolve` -> `EngineError::SchemaNotFound`). Companion init-doorway choice: [[engineering--refuse-memstead-init-in-a-non-empty-folder-rather-than-adopting-existing-files]]. Tests: `init_succeeds_but_warns_on_unresolvable_schema_pin`, `unresolved_pin_warning_names_pin_recovery_and_builtins`, `init_accepts_every_builtin_schema_pin`.
