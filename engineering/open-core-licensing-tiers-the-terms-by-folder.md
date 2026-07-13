---
type: principle
created_date: 2026-07-13T16:43:05Z
last_modified: 2026-07-13T16:44:04Z
authority: established
universality: domain-wide
tags: licensing, open-core, strategy, plugin
---

# Open-core licensing tiers the terms by folder

## Statement
The project's license terms follow an open-core model, resolved per-folder by the repo-root licensing map: the open core (engine, CLI, MCP server, schemas, docs, the `.mem` format + publish/install client) ships permissively as dual **MIT OR Apache-2.0** at the user's option; the monetized commercial layer (registry server, the hosted-deployment serve/bridge crates, macOS app) is proprietary; and a plugin that extends a third-party host ships under the single most permissive license that maximizes reuse, forking, and re-embedding — because a host plugin's value is breadth of adoption, not defensibility.

## Scope
Governs the license terms of every shippable folder in the repository — engine, CLI, MCP server, schemas, docs, the registry server, the hosted-deployment serve/bridge crates, the macOS app, and every third-party-host plugin. A file's folder location, resolved through the repo-root licensing map, determines its terms; moving a file across a folder boundary changes its license.

## Relationships
- **GOVERNS**: [[plugin-ships-under-mit-distinct-from-the-engine]]

## Justification

The three tiers face different economic pressures, so one license cannot serve all. The embeddable core maximizes trust and standard-library compatibility (the Rust ecosystem's dual MIT/Apache default). The commercial layer funds the project and must stay proprietary. A host plugin lives inside someone else's tool and is expected to be copied and adapted freely; the posture that maximizes ecosystem adoption is more permissive than the posture chosen for the core, and lighter than dual-licensing warrants.

## Exceptions



## Consequences

- Each folder's license is authoritative from the repo-root licensing map, not from any manifest descriptor (e.g. `plugin.json` carries no license field).
- A permissively-licensed host plugin (MIT-only) offers no patent grant of its own; a consumer needing the explicit patent protection must take the engine under its Apache-2.0 option.
- Contributions and code moves must respect folder boundaries, because location determines terms.
