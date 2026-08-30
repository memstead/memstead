---
type: decision
created_date: 2026-07-13T16:43:06Z
last_modified: 2026-08-30T00:24:19Z
status: accepted
decided_on: 2026-05-19
deciders: dasboe
scope: component
tags: plugin
---

# Plugin ships under MIT distinct from the engine

## Decision
The Claude Code plugin (`plugins/claude-code/`) is licensed under the MIT License, deliberately distinct from the project's default dual MIT-OR-Apache-2.0 and the proprietary terms of the commercial layer (then including the macOS app). The plugin's `LICENSE` file carries the MIT text (Copyright 2026 Björn Bösenberg), and the repo-root licensing map records `plugins/claude-code/` → MIT. It is now the ONLY folder deviating from the dual MIT-OR-Apache-2.0 default: the second deviating row left the map with the macOS app's 2026-08-18 retirement, so `public/LICENSING.md` carries two rows, the `/` default and this one (verified 2026-08-30).

## Context
The project follows an open-core model with a per-folder license map (the repo-root `LICENSING.md` is the authority): the open core — engine, CLI, MCP server, schemas, docs, and the `.mem` format + publish/install client — ships dual **MIT OR Apache-2.0** (the Rust-ecosystem standard, at the user's option); the registry server is the proprietary commercial layer (the macOS app was part of it until its 2026-08-18 retirement); plugins for third-party tools sit between them. A plugin extends someone else's host (Claude Code today, other tools later) and is expected to be copied, forked, and embedded freely — the licensing posture that maximizes ecosystem adoption differs from the posture chosen for the embeddable engine core. The packaging manifest ([[plugin--plugin-packaging-manifest]]) makes the plugin installable but declares no license itself (`plugin.json` carries only name/description/version); the license lives in the folder's `LICENSE` file and the repo-root mapping, not in the manifest descriptors.

## Consequences
- Consumers may copy, fork, modify, and redistribute the plugin's skills and hooks with only the MIT attribution requirement — the same attribution-only obligation as the engine under its MIT option, and lighter than the engine's Apache-2.0 option (which adds notice-of-modification and a patent grant).
- The plugin folder is governed by terms separate from the rest of the repo: a file's location under `plugins/claude-code/` determines its license, so code moved into or out of that folder changes license. The repo-root licensing map is the authority on which folder carries which terms.
- The plugin offers no patent grant of its own; a consumer relying on the explicit patent protection must take the engine under its Apache-2.0 option, not the plugin.

## Relationships
- **REFERENCES**: [[plugin:plugin-packaging-manifest]]
- **MOTIVATED_BY**: [[open-core-licensing-tiers-the-terms-by-folder]]

## Options

- **MIT (chosen)** — lighter-weight than the Apache-2.0 option for the plugin-ecosystem case; permits broad reuse, forking, and re-embedding with only attribution preserved.
- **MIT OR Apache-2.0 (the engine/project default)** — the open core is dual-licensed at the user's option; the Apache-2.0 option adds an explicit patent grant. Rejected as the plugin's sole terms because dual-licensing is heavier than the plugin-ecosystem case warrants.
- **Proprietary** — the commercial layer's terms (then carried by the macOS app as the monetized product); never a candidate for the plugin, whose value is breadth of adoption.

## Notes


