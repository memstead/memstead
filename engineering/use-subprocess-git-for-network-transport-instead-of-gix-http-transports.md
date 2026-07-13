---
type: decision
created_date: 2026-07-13T16:43:08Z
last_modified: 2026-07-13T16:43:08Z
status: accepted
decided_on: 2026-05-19
deciders: memstead-core
scope: component
tags: transport, git, gix, auth, subprocess, mem-repo, engine
---

# Use subprocess git for network transport instead of gix HTTP transports

## Decision
We chose to reach the network through subprocess calls to the user's installed `git` binary for the fetch/pull/push legs of the [[engine--git-transport-and-history-surface]], rather than activating `gix`'s `blocking-http-transport-*` features in the [[engine--memstead-git-branch-crate]]. Every network operation spawns `git -C <gitdir> <args>` with prompts disabled (`GIT_TERMINAL_PROMPT=0`); only local-repo work (ref reads, tree walks, commit construction) stays on the in-process `gix` library.

## Context
The transport surface (`Engine::fetch`, `pull`, `push`) needed to move history between a local mem-repo and arbitrary user remotes — GitHub over SSH, HTTPS with credential helpers, OAuth-token setups. `gix` ships optional blocking HTTP transport features, but enabling them would have meant building an auth story the user has already solved once in their git config: ssh agents, credential helpers, and OAuth flows all live in the installed `git` toolchain. Any consumer of a mem-repo already has `git` on PATH, so the runtime dependency the subprocess path adds was already a de-facto requirement.

## Consequences
- Auth works wherever the user's `git` works: ssh agents, credential helpers, and OAuth tokens are inherited with zero engine-side auth code, and every protocol the installed `git` supports is supported.
- The Cargo dependency tree stays unchanged — no `gix` transport features, no TLS stack pulled into the engine.
- Cost: refusal classification is stderr-parsing — `run_git` wraps trimmed stderr into `BackendError::Other` with an in-band marker the engine layer un-marshals into typed refusals, which is inherently coupled to `git`'s message wording.
- Cost: a hard runtime requirement on a `git` binary on the operator's PATH (accepted: already true for any mem-repo consumer).
- `GIT_TERMINAL_PROMPT=0` plus a null stdin means a missing credential fails typed instead of hanging a non-interactive agent process.
- The split model — subprocess for network, in-process `gix` for local object/ref work — means two git implementations coexist in the same module.

## Relationships
- **REFERENCES**: [[engine:git-transport-and-history-surface]]
- **REFERENCES**: [[engine:memstead-git-branch-crate]]

## Options

- **Enable `gix`'s `blocking-http-transport-*` features** — rejected: requires an engine-owned auth story (no inheritance of ssh agents, credential helpers, or OAuth setups), grows the dependency tree with an HTTP/TLS stack, and covers fewer protocols than the user's installed `git`.
- **Subprocess calls to the user's `git` binary** — chosen: inherits the user's complete auth configuration, supports every protocol the installed `git` does, keeps the dep tree unchanged; pays with a PATH requirement and stderr-parsing for refusal classification.

## Notes

The trade-off is documented in the module header of `crates/memstead-git-branch/src/ops/transport.rs`, which frames it as the V1 posture. The same header records an open atomicity caveat: `git fetch` advances remote-tracking refs in place, so a schema-violating remote tip is visible on `refs/remotes/*` even when the engine refuses to apply it locally — a quarantine-ref pipeline is the named safe shape that has not landed.
