---
type: decision
created_date: 2026-08-19T19:33:22Z
last_modified: 2026-08-19T19:33:52Z
status: accepted
decided_on: 2026-08-19
deciders: flywheel W1 plan 01
scope: subsystem
tags: onboarding, quickstart, bindings, honesty
---

# The guided first session binds the repository instead of refusing it

## Decision
`memstead quickstart` gains a `--repo <PATH>` mode. Its layout is the decision: **the repository becomes the workspace root, and the mem takes a folder of its own inside it** — `<repo>/.memstead/` for workspace state and the binding record, `<repo>/<mem-name>/` for the entities, agent wiring at the repo root. On top of that layout the mode scaffolds a `codebase` binding over the repository and prints an honest brief.

Three properties follow from the layout rather than from new special-casing:

- **The repo's files are not adopted.** A filesystem mem owns every `.md` file *in its folder*; that folder is now the subdirectory, so the repository's own `README.md` and docs belong to no mem. The tolerant-emptiness gate is not weakened — it simply applies to the mem's folder, which is where the adoption risk actually lives.
- **The mem folder never re-enters through the binding.** A mount's resolved storage location is already excluded from every binding's input set unconditionally in the strategy layer, so the seed entity is not a source artifact. Measured on a three-file fixture repo: denominator 4 (`.mcp.json`, `README.md`, `docs/design.md`, `src/main.rs`), mem folder and `.memstead/` absent.
- **Artifact ids stay repo-relative.** The pointer is `.`, in-root, so the `../…` chain the out-of-root shape costs never appears. Passing a target path as well (`quickstart ./graph --repo ./app`) keeps today's collapsed workspace and puts the binding out-of-root, where the layout warning of [[engineering--layout-guidance-fires-at-the-layout-decision-and-claims-only-what-measurement-supports]] fires with its relocation recipe.

## Context
A stranger with an existing repository was told to start in an empty directory: their actual project was the one place `quickstart` refused to look, because a non-empty target trips `TARGET_NOT_EMPTY`. That refusal is correct and stays — see [[engineering--refuse-memstead-init-in-a-non-empty-folder-rather-than-adopting-existing-files]] — but it left the flagship first session pointed away from the only content the newcomer has.

The binding-plus-loop shape is what makes binding the repo safe: nothing is ingested during quickstart, so no unvalidated content enters the graph ([[engineering--never-silently-admit-unvalidated-content-into-the-graph]]) and the ~20-minute budget buys orientation, not an unbounded batch job on a stranger's tree.

The MCP server resolves its workspace by walking *up* from its working directory. A workspace nested below the repo root would therefore be invisible to an agent running in the repo — which is what forces the workspace root onto the repo and the mem into a subdirectory, rather than the reverse.

## Consequences
- The receipt gains a brief that states what the starter mem holds (one seed entity), what it does not (anything from the repository), the binding's scope with the deny list read off the written record, and the files newly present in the reader's tree — a claim they can check with `git status` in seconds.
- The workspace-shape disclosure ([[engineering--every-workspace-creating-command-discloses-the-shape-it-just-made]]) became folder-aware: the filesystem summary said "plain `.md` files in this folder", which is untrue when the entities live one folder down. It now names the mem's actual folder.
- The mem folder is a folder the graph then owns, so a collision with an existing directory refuses (`TARGET_NOT_EMPTY`) carrying the `--name` retry rather than adopting it.
- Every command the guided output prints goes through the existing command builder ([[engineering--a-command-a-surface-prints-is-built-never-formatted]]) and is replayed verbatim by test.
- Not settled here: whether the plain empty-directory path should eventually become the *secondary* entry. Both are presented side by side on the public onboarding surfaces; which one leads is a later call.

## Relationships
- **REFERENCES**: [[every-workspace-creating-command-discloses-the-shape-it-just-made]]
- **REFERENCES**: [[a-command-a-surface-prints-is-built-never-formatted]]
- **REFERENCES**: [[refuse-memstead-init-in-a-non-empty-folder-rather-than-adopting-existing-files]]
- **REFERENCES**: [[never-silently-admit-unvalidated-content-into-the-graph]]
- **REFERENCES**: [[layout-guidance-fires-at-the-layout-decision-and-claims-only-what-measurement-supports]]
- **INFORMED_BY**: [[refuse-memstead-init-in-a-non-empty-folder-rather-than-adopting-existing-files]]

## Options

- **A new top-level verb** (`memstead adopt`): rejected — a second front door splits the discovery path, and `quickstart` is the command every public surface already names.
- **Adopt the repo's `.md` files as entities**: rejected — it contradicts the refuse-rather-than-adopt and never-silently-admit postures, and produces a mem the honest brief could not stand behind.
- **Run the first ingest batch inside quickstart**: rejected — unbounded on a stranger's repo, and it duplicates the loop the brief points at. The starter mem's emptiness is a fact the brief states, not a gap to hide.
- **Workspace beside the repo as the default**: rejected as the default — it costs `../…` artifact ids and leaves the agent wiring where the agent is not. Retained as the explicit two-argument form.

## Notes


