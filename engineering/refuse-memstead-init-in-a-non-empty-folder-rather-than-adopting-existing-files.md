---
type: decision
created_date: 2026-07-13T16:43:06Z
last_modified: 2026-07-13T16:44:06Z
status: accepted
decided_on: 2026-05-08
deciders: memstead-core
scope: component
tags: cli, init, filesystem-mem, bootstrap, strict-mode, engine
---

# Refuse memstead init in a non-empty folder rather than adopting existing files

## Decision
We chose strict mode for `memstead init`: a non-empty target folder errors out cleanly (`ensure_empty`), forcing the user to explicitly clear or move files before initialising a filesystem mem — rather than adopting whatever the folder already contains. `memstead init` never silently ingests pre-existing `.md` files into a fresh mem.

## Context
`memstead init` bootstraps the lean filesystem-mem product surface ([[engine--cli-command-surface]]): it writes `.memstead/config.json` with the mem name and [[engine--schema]] pin into a folder that becomes the mem root. Folders users run `init` in often already contain markdown — notes, READMEs, exports from other tools. Whatever the folder holds at init time becomes mem content on the next load, parsed against the pinned schema, so the bootstrap posture decides whether unrelated files silently become (probably schema-invalid) entities.

## Consequences
- A user can never accidentally turn a notes folder into a half-broken mem: the failure is explicit, at init time, before any state is written.
- Cost: there is no one-command migration path for an existing markdown folder — adopting pre-existing content means moving files in *after* init, where each file then surfaces its own parse/validation outcome on load.
- The init code path stays trivial: no adoption heuristics, no schema-mapping prompts, no partial-success states to report.

## Relationships
- **REFERENCES**: [[engine:cli-command-surface]]
- **REFERENCES**: [[engine:schema]]
- **REFERENCES**: [[engine:cli-cold-start-quickstart-and-schema-scaffold-surface]]
- **MOTIVATED_BY**: [[never-silently-admit-unvalidated-content-into-the-graph]]

## Options

- **Adopt mode** — initialise in place and treat existing `.md` files as mem content: rejected; pre-existing files were not authored against the pinned schema, so adoption silently creates invalid or misparsed entities, and the user discovers the mess only on first load.
- **Strict mode** — refuse to initialise a non-empty folder: chosen; the user makes the content decision explicitly, file by file, after the mem shape exists.

## Notes

Recorded in the module header of `crates/memstead-cli/src/commands/init.rs` ("Adopt vs. strict"); enforced by `ensure_empty` on the existing-directory arm of `run`. Landed with the mode-b product surface, commit 239cd281.


The 2026-07-02 `memstead quickstart` command relaxes the doorway for newcomers **without touching this decision**: `init` keeps `ensure_empty` strictness, while quickstart's separate tolerant-emptiness gate admits only files the folder backend can never read into the graph (dotfiles, non-`.md` README-grade files) and still blocks on every `.md` — so the core invariant (never silently adopt user `.md` content) holds on both verbs. See [[engine--cli-cold-start-quickstart-and-schema-scaffold-surface]].
