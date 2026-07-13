---
type: principle
created_date: 2026-07-13T16:43:05Z
last_modified: 2026-07-13T16:44:03Z
authority: established
universality: domain-wide
tags: licensing, open-core, repo-split, strategy, engine
---

# Open-closed boundary runs by trust not engine capability

## Statement
The open/closed boundary of the Memstead codebase runs by trust and strategic moat, never by engine capability. The entire engine — including the collaboration-grade git-branch backend, multi-mem, and history — is open source; only the registry server and the macOS app stay private. There is no crippled or capability-gated open core.

## Scope
The whole Memstead codebase, split across the public engine repo and the private repo. Governs which components are published open-source versus kept proprietary, and what any public artifact (repo, release, published crate) is permitted to contain. Project-wide, not scoped to any one crate or module.

## Relationships
- **REFERENCES**: [[engine:memstead-git-branch-crate]]
- **REFERENCES**: [[engine:public-repo-leak-hygiene-gate]]
- **GOVERNS**: [[engine:public-repo-leak-hygiene-gate]]
- **GOVERNS**: [[engine:binary-release-and-installer-packaging]]

## Justification

The engine was never the moat: in the AI age a model can rebuild a commodity markdown-graph engine from the public `.mem` format, the MCP surface, and observable behaviour regardless of license, so closing the engine buys no real protection while taxing the adoption that builds the actual moat — the network (the registry), the accumulated mems/data, and the brand. The collaboration story is the differentiator, so the git-branch backend that provides it must be free to experience; that is why [[engine--memstead-git-branch-crate]] ships open rather than being held back as a paid tier. Documented in `LICENSING.md`.

## Exceptions

- **Registry server** — kept private because it is the network moat; open-sourcing it would invite a fork that fragments the network. Not private because the open engine couldn't run it.
- **macOS application** — kept private as a commercial product and human-oversight surface, all rights reserved.
Both carve-outs are held back by *trust and strategy*, not by any capability the open engine lacks.

## Consequences

- The `memstead-git-branch` backend, CLI, MCP server, schema definitions, and the `.mem` format/protocol + publish/install client are all open — no paid capability tier inside the engine.
- The [[engine--public-repo-leak-hygiene-gate]] exists to enforce this boundary mechanically: it fails any push whose tree carries private-repo material, keeping side-by-side private/public development in one working tree from leaking across the cut.
- Seven engine crates are publishable to crates.io, while the registry server and macOS app live in the private repo entirely, outside this workspace.
- New engine capability defaults to the open side of the cut; moving something to the private side is the exception that must be justified against this principle.
