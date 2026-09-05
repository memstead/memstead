---
type: decision
created_date: 2026-09-05T08:17:31Z
last_modified: 2026-09-05T08:17:31Z
status: accepted
decided_on: 2026-09-04
deciders: operator, engine team
scope: system
tags: architecture,flavours,features,crates,engine
---

# The engine ships one flavour

## Decision
We chose to ship the engine in one build. The mem-repo Cargo feature and its 212 cfg sites are gone from memstead-cli and memstead-mcp; the git-branch backend, the multi-mem policy and the lifecycle verbs are always compiled, and --no-default-features builds the same binaries as the default. The lean FilesystemMcpServer and its separate description set are deleted; memstead-mcp boots the one McpServer on every workspace shape, including the folder-only workspace memstead quickstart produces, with the same tool roster on both. The memstead-engine boundary crate is folded into memstead-base (memstead_base::mem_management for the lifecycle orchestrators, memstead_base::workspace_config_edit for the policy writer, memstead_base::FullEngineError at the crate root, the name kept so no consumer renames); the crates.io name memstead-engine stays reserved and receives no further publish. The git-object-storage feature on memstead-git-branch goes the same way. The layering rule that matters, no gix in memstead-base, stays and is proven by the wasm CI job alone. Landed as public commits 1f83d47 and 96e2476 (2026-09-04) and the workspace adoption of 2026-09-05.

## Context
The 2026-09-04 census counted the lean flavour among the apparatus with no shipped consumer: a build nothing shipped, kept green only by dedicated CI, that sat red for two days in 2026-06 with 33 errors because no developer machine compiled it, at a standing cost of a second test matrix, a second MCP server of 5,020 lines, a boundary crate of 4,507 lines and a second description set. The decision basket of the same day retired it (line 3) under the cause-before-gate directive: the flavour was a cause of contradictions, and the kernel's portability, the one thing it stood for, has a cheaper proof in the wasm job.

## Consequences
- One build to test, one binary shape to document, one MCP server to describe; the parity matrix has no flavour column and the MCP reference no lean count.
- The folder-only workspace is served by the full engine; what the lean flavour served, the one server serves, and the mem-repo-only subcommands refuse there with UNSUPPORTED_WORKSPACE_SHAPE.
- The rot hazard of code compiled on no developer machine is gone with the cfg sites.
- Consumers of memstead-engine (serve, ui-api, any external crate) depend on memstead-base instead; the crates.io page of memstead-engine keeps its last README.
- Accepted cost: every binary links gix; a consumer wanting a folder-only engine without git takes the memstead-base library, not a lean binary.
- The strictness and mutation CI probes that lived in the lean lane run against the one binary in the smoke lane.

## Relationships
- **SUPERSEDES**: [[split-engine-into-lean-and-full-flavours]]
- **MOTIVATED_BY**: [[engine:storage-backend]]

## Options

- Fold memstead-engine into memstead-base (chosen): the boundary crate existed to keep the lean kernel free of lifecycle code; with one flavour the boundary has no consumer.
- Keep memstead-engine as a crate but drop the feature: rejected, a crate whose only reason was a flavour split outlives its reason; the layering rule that matters (no gix in the kernel) is about memstead-git-branch.
- Keep the folder-only server as the default for quickstart workspaces: rejected, two servers is the flavour split under another name; the full engine already mounts folder workspaces.
- Remove the feature but leave the cfg sites as no-ops: rejected, 212 dead attributes are the residue class the census counted.
