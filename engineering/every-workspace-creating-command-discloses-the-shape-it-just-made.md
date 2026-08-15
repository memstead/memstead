---
type: decision
created_date: 2026-08-14T21:35:13Z
last_modified: 2026-08-14T21:35:13Z
status: accepted
decided_on: 2026-08-14
deciders: operator, implementing agent
scope: subsystem
tags: onboarding, workspace-shape, cli, mcp, cold-start, disclosure
---

# Every workspace-creating command discloses the shape it just made

## Decision
`memstead quickstart`, `memstead init`, and `memstead mem-repo init` each close their success output with the same three-part block: which of the two workspace shapes they just created, at least one concrete thing that shape cannot do, and the exact command that produces the other shape. The block is symmetric — the mem-repo verb states its own cost (a git repository, every mutation a commit) and points at `memstead quickstart`, exactly as the filesystem verbs state the registry refusal and point at `memstead mem-repo init`. One renderer serves all three, and the command it names is feature-gated so a lean binary never advertises a verb it lacks.

Two supporting changes land with it. `memstead-mcp`'s boot line names the shape it actually opened (`boot: filesystem-mem workspace at …` / `boot: mem-repo workspace at …`) instead of naming its build config; the full binary serves both shapes and previously logged `mem-repo` for either. And the shape probe itself becomes one engine primitive — `memstead_base::is_mem_repo_shaped` / `workspace_shape_label` — that the CLI's shape resolution and the MCP boot line both route through, so no surface can name a shape another surface contradicts. See [[engine--workspace-root-discovery-and-shape-detection]].

What this decision does NOT do: change which shape `quickstart` picks. The default stays as it is; only the disclosure is new.

## Context
`memstead quickstart` — the command every public surface recommends — silently picks the filesystem-mem shape, and that shape cannot consume the registry. The 2026-08-14 cold-start run followed the documented path exactly and then hit `UNSUPPORTED_WORKSPACE_SHAPE` on `memstead install <scope>/<name>`, the headline command on memstead.io. The refusal was clear and named the fix, but it arrived after a workspace existed and had already been modelled; recovering means starting a second workspace and rebuilding.

A sentence elsewhere had already been tried: the `install --help` text carries the clause, and the run proves it does not reach the reader, because it lives on a command the newcomer has no reason to read before they need it. The same run also found `memstead-mcp` logging `boot: mem-repo workspace at …` for the very directory `memstead install` refuses as not-mem-repo — two shipped binaries describing one directory in contradictory terms, which makes the genuine refusal read as spurious to anyone debugging from the log.

The receipt is the narrow path everybody walks. It is the moment the fork is decided and the output the newcomer is already reading, which is why the disclosure belongs there rather than on a page or in a sibling command's help. This realizes [[engineering--advertised-front-door-commands-serve-a-fresh-non-maintainer-workspace]] for the disclosure half: the front door works on the produced shape, and now also says what that shape is.

## Consequences
Every future workspace-creating verb inherits the obligation: it states its shape, one real limit, and the other shape's command. A new verb that creates a workspace silently is a regression against this decision, not merely an omission.

The shape vocabulary is now fixed at two spellings — `mem-repo` and `filesystem-mem` — shared by the receipts, the `UNSUPPORTED_WORKSPACE_SHAPE` refusals, and the MCP boot line. Changing a spelling means changing it at the engine primitive, not per surface.

Disclosure is explicitly not permission: the mem-repo-only subcommands still refuse on a filesystem-mem workspace with the same typed code and the same recovering command. Tests pin both halves — that the receipts carry the disclosure, and that the refusal is unchanged.

The open item this decision deliberately leaves standing: whether `quickstart` should default to the shape that keeps every advertised surface open. That is a product decision with a wider blast radius (a mem-repo shape means a git repository where today there is none) and is tracked separately.

## Relationships
- **REFERENCES**: [[advertised-front-door-commands-serve-a-fresh-non-maintainer-workspace]]
- **REFERENCES**: [[engine:workspace-root-discovery-and-shape-detection]]

## Options

**State it in the creating command's receipt (chosen).** The receipt already enumerates what was created, and it is the exact moment the fork is decided.

**Warn only when the user first hits a mem-repo-only command (rejected).** Zero noise for people who never install a mem — but this is today's behaviour, and the cold-start run shows it arrives after the workspace exists. The cost of the surprise is a second workspace.

**Explain both shapes on the entry page and in the tutorial (rejected as the primary fix, wanted as a follow-on).** The run read every public page and still did not learn the fork; a concept explained on a website is not where a person is looking while typing a command.

## Notes


