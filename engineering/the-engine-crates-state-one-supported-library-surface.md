---
type: decision
created_date: 2026-11-22T13:15:48Z
last_modified: 2026-11-22T13:16:03Z
status: accepted
decided_on: 2026-11-22
deciders: operator (shared-engine bundle, go of 2026-11-21), implementing agent
scope: system
tags: library,release,crates
---

# The engine crates state one supported library surface

## Decision
We chose to state one small supported library surface for the engine crates: the module `memstead_mcp::library`, which re-exports a closed list (booting an engine, the MCP server over an owned engine and a further instance for another caller, the identity setters, the tool filter and list, the default token budget, laying a mem-repo, mounting and unregistering a mem, and the types those signatures name). Before 1.0 the surface changes only in a minor release, with a changelog entry under the heading "Library surface", and never in a patch release. Everything else in the crates is internal and may change in any release. An item joins the list only by such an entry. A test that imports the module alone names every item, so a removal or a rename stops the build.

## Context
Programs that link the engine as a library exist as a category, and the crate READMEs promised them nothing. 0.26.1, a patch release, changed what two public function-pointer types of the git-branch operations table take.

## Consequences
- A linking program knows which items it may build on and when they may move.
- The promise binds the release procedure: a change to the surface forces a minor version and an entry under the heading.
- The engine ships no transport beyond the stdio binary; a linking program serves the server through the `rmcp` service loop over a transport of its own, and `rmcp` is not re-exported.
- Most of the crates' public API stays unpromised, so engine internals stay free to move.

## Relationships
- **DERIVED_FROM**: [[published-libraries-ride-the-engines-version-line]]

## Options

- A separate facade crate: rejected, a new package is a new surface and a new link in the publish chain.
- Promise the whole public API of the crates: rejected, most of it is engine internals.
- No promise at all: rejected, see the context.
- One module with a closed list and a compile test: chosen.
