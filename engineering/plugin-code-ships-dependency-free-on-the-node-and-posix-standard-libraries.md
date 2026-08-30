---
type: principle
created_date: 2026-07-13T16:43:05Z
last_modified: 2026-08-30T00:33:19Z
authority: established
universality: domain-wide
tags: plugin, dependencies, packaging, portability
---

# Plugin code ships dependency-free on the Node and POSIX standard libraries

## Statement
Every executable file shipped in `plugins/claude-code/` MUST run on the Node and POSIX standard libraries alone — no npm dependencies, no `package.json`, no `node_modules`, no bare-package imports. A plugin `.mjs` script may import only `node:`-prefixed builtins and relative-path modules; shell wrappers use POSIX utilities only.

## Scope
All plugin-owned scripts under `plugins/claude-code/`. Rechecked 2026-08-30, that set is: the `hooks/*.mjs` lifecycle hooks (`guard-entity-edit`, `guard-entity-bash`, `check-realization`, `inject-context`, `deny-meta-files`, plus the shared `guard-entity-edit-utils.mjs`, `guard-entity-bash-utils.mjs`, `check-realization-utils.mjs` and `workspace-resolve-utils.mjs`), `scripts/binary-version.mjs`, the ingest skill's `skills/ingest/scripts/inject.mjs`, and the `node --test` suites beside them. Three script sets named in earlier revisions of this scope are gone and should not be looked for: `workspace-loader.mjs`, `change-detection.mjs` and `skills/lib/writing-guidance.mjs`, and the whole `schemas/` tree that held the hand-rolled schema validator and workspace-walker, all removed in the 2026-07-11 plugin diet. The plugin currently ships no `.sh` wrapper, though the POSIX-utilities-only half of the rule stands for any that returns. The rule does NOT extend to the Rust engine crates, which carry their own dependency policies.

## Relationships
- **REFERENCES**: [[plugin:memstead-plugin-v0-schema-validator-runtime]]
- **REFERENCES**: [[plugin:hook-mcp-client]]
- **GOVERNS**: [[plugin:memstead-plugin-v0-schema-validator-runtime]]
- **GOVERNS**: [[plugin:hook-mcp-client]]
- **GOVERNS**: [[plugin:architecture-guard-check-script]]

## Justification

The plugin installs as a Claude Code plugin with no build or install step; it cannot assume an `npm install` ever ran on the user's machine. Shipping zero dependencies makes every script runnable directly under the user's Node, keeps the install auditable, and removes a supply-chain surface.

The two artifacts this section used to cite as the trade-off being paid openly, the hand-rolled JSON-Schema keyword subset in [[plugin--memstead-plugin-v0-schema-validator-runtime]] and the hand-written stdio wire protocol in [[plugin--hook-mcp-client]], were both removed in the 2026-07-11 plugin diet and no longer ship (corrected 2026-08-30). They remain the clearest illustration of what the rule costs: rather than pull a dependency, the plugin wrote the subset it needed by hand. The live equivalent is smaller in scope, the hooks parse arguments and JSON with `node:` builtins alone.

## Exceptions

- None within plugin code itself. The rule constrains the plugin's own scripts, not the artifacts they invoke as subprocesses: the compiled `memstead` binary that `check-realization.mjs` shells for `memstead anchors --artifact --json` has its own dependencies and is run as a child process, never imported. (Corrected 2026-08-30: this carve-out previously cited the stdio spawn in [[plugin--hook-mcp-client]], which the plugin diet removed; the carve-out itself is unchanged.)
- Test files run under `node --test` (a `node:` builtin), so the suite stays dependency-free as well.

## Consequences

- Functionality that an npm package would provide must be hand-rolled when needed: argument and JSON handling across the hooks uses `node:util` `parseArgs` and other `node:` builtins only. (The two largest historical examples, the JSON-Schema validator and the MCP stdio client, were removed in the 2026-07-11 plugin diet; noted here 2026-08-30 so the list names only what ships.)
- Implementations stay small and auditable but cover only the subset actually exercised.
- A reviewer can enforce the rule mechanically: any new `package.json`, `node_modules`, or non-`node:` bare import in `plugins/claude-code/` violates it. Verified clean 2026-08-30: no `package.json`, no `node_modules`, and no bare-package import anywhere under that tree.
