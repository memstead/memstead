---
type: principle
created_date: 2026-07-13T16:43:03Z
last_modified: 2026-08-18T19:34:48Z
authority: established
universality: domain-wide
tags: invariant, mem-repo, mutation, integrity, engine, plugin, mutation-boundary
---

# Engine owns mem-repo state

## Statement
External consumers — plugin code, scripts, in-process embedders, anything outside the engine crates — MUST NOT mutate mem-repo state directly: no `git` commands against the mem-repo, no raw `.md` file writes, no `mem-repo/.git/` introspection for mutation. All mutations route through the engine.

## Scope
Applies to every consumer of a Memstead [[engine--storage-backend]] across all repos in the project — the Claude Code plugin, helper scripts, any embedder. The two sanctioned mutation surfaces are MCP (`memstead-mcp`, the documented agent contract) and the CLI (`memstead`, human and script use); an in-process embedder of the Rust crates routes through the engine API itself. (UniFFI was a third sanctioned surface until the macOS app — its only consumer — retired on 2026-08-18.) Reads may use any surface, including direct file reads — read paths do not violate engine invariants.

On the plugin side this applies to all plugin hooks, skills, and scripts; the restriction is on mutation and on `.git/` introspection.

## Relationships
- **REFERENCES**: [[engine:storage-backend]]
- **REFERENCES**: [[engine:graph]]
- **REFERENCES**: [[engine:entity]]
- **REFERENCES**: [[engine:cross-mem-edge]]
- **GOVERNS**: [[engine:memstead-cli-crate]]
- **GOVERNS**: [[engine:memstead-mcp-crate]]
- **GOVERNS**: [[plugin:plugin-hook-system]]
- **GOVERNS**: [[plugin:entity-edit-guard-hook]]
- **GOVERNS**: [[plugin:entity-bash-guard-hook]]
- **GOVERNS**: [[plugin:architecture-guard-check-script]]
- **GOVERNS**: [[plugin:subagents-must-preserve-the-entity-mutation-guard-boundary]]

## Justification

Direct mutations skip schema validation, write rules, link-graph integrity, search-index updates, optimistic locking, and commit provenance — the [[engine--graph]] corrupts silently. The engine is the single point where [[engine--entity]] conformance and [[engine--cross-mem-edge]] integrity are enforced; bypassing it bypasses all of them at once.

## Exceptions

Auto-commit hooks operate on the OUTER project repo (not the mem-repo), which is not Memstead-managed — that is a documented carve-out, not a violation. `memstead-cli`'s outer-repo gitignore management (mem-repo bootstrap) is one such outer-repo operation.

- Auto-commit hooks operate on the *outer* project repo (the workspace root the user's `.git` lives in), which is not Memstead-managed — `git add` / `git commit` / `git log` there are permitted.
- Reads of mem state are unrestricted and may use `memstead-cli` subprocess or `memstead-mcp`.
- No mem-repo mutation carve-out exists — the principle permits no direct `git checkout` or raw entity-file write against mem state, on any backend. Restoring a graph to a prior commit routes through the engine's own history-rewrite path (`memstead branch-reset`), never an external git operation.

## Consequences

Weakening this rule — e.g. having a script write a `.md` file or run `git commit` inside the mem-repo to make something work — is explicitly forbidden. Every cross-surface operation should be reachable via the CLI alongside MCP so consumers never need a direct-mutation shortcut.

The plugin's guard hooks (`guard-entity-edit`, `guard-entity-bash`) enforce this at the tool-call boundary, blocking direct Write/Edit/Bash mutations of entity files. Drift hooks read HEADs through `memstead_health` rather than `mem-repo/.git/`.
