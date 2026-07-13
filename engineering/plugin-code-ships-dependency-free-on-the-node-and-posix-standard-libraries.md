---
type: principle
created_date: 2026-07-13T16:43:05Z
last_modified: 2026-07-13T16:44:04Z
authority: established
universality: domain-wide
tags: plugin, dependencies, packaging, portability
---

# Plugin code ships dependency-free on the Node and POSIX standard libraries

## Statement
Every executable file shipped in `plugins/claude-code/` MUST run on the Node and POSIX standard libraries alone — no npm dependencies, no `package.json`, no `node_modules`, no bare-package imports. A plugin `.mjs` script may import only `node:`-prefixed builtins and relative-path modules; shell wrappers use POSIX utilities only.

## Scope
All plugin-owned scripts under `plugins/claude-code/`: the `hooks/*.mjs` lifecycle hooks, the skill scripts (`skills/ingest/scripts/inject.mjs`, `workspace-loader.mjs`, `change-detection.mjs`, `skills/lib/writing-guidance.mjs`), the schema validator and workspace-walker under `schemas/`, and their `.sh` wrappers. It does NOT extend to the Rust engine crates or the macOS app's Swift packages — those carry their own dependency policies — those carry their own dependency policies.

## Relationships
- **REFERENCES**: [[plugin:memstead-plugin-v0-schema-validator-runtime]]
- **REFERENCES**: [[plugin:hook-mcp-client]]
- **GOVERNS**: [[plugin:memstead-plugin-v0-schema-validator-runtime]]
- **GOVERNS**: [[plugin:hook-mcp-client]]
- **GOVERNS**: [[plugin:architecture-guard-check-script]]

## Justification

The plugin installs as a Claude Code plugin with no build or install step; it cannot assume an `npm install` ever ran on the user's machine. Shipping zero dependencies makes every script runnable directly under the user's Node, keeps the install auditable, and removes a supply-chain surface. The trade-off is accepted explicitly in the schema validator, which is a hand-rolled JSON-Schema keyword subset rather than a conformant draft-2020-12 implementation precisely to avoid pulling a dependency — see [[plugin--memstead-plugin-v0-schema-validator-runtime]]. The same constraint shapes the stdio MCP client [[plugin--hook-mcp-client]], which speaks the wire protocol by hand instead of using an SDK.

## Exceptions

- None within plugin code itself. The rule constrains the plugin's own scripts, not the artifacts they invoke as subprocesses: the compiled Rust engine binary that [[plugin--hook-mcp-client]] spawns over stdio has its own dependencies and is run as a child process, never imported.
- Test files run under `node --test` (a `node:` builtin), so the suite stays dependency-free as well.

## Consequences

- Functionality that an npm package would provide must be hand-rolled when needed: the JSON-Schema validator, the MCP stdio client, and all argument parsing use `node:util` `parseArgs` and built-ins only.
- Implementations stay small and auditable but cover only the subset actually exercised (e.g. the validator supports only the JSON-Schema keywords the v0 schemas use).
- A reviewer can enforce the rule mechanically: any new `package.json`, `node_modules`, or non-`node:` bare import in `plugins/claude-code/` violates it.
