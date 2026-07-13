---
type: decision
created_date: 2026-07-13T16:43:07Z
last_modified: 2026-07-13T16:44:06Z
status: accepted
decided_on: 2026-06-08
deciders: engine team
scope: system
tags: architecture, flavours, features, crates, engine
---

# Split engine into lean and full flavours

## Decision
The engine ships in two flavours selected by the `mem-repo` Cargo feature, which is **on by default** — full is the shipped product, lean is a `--no-default-features` dependency-hygiene / CI guard, not a separately distributed artifact. Both flavours compile into the same surface crates rather than separate `full` crates. **Lean** (`--no-default-features`, folder + archive [[engine--storage-backend]] only, no `gix`): `memstead-base` core, the lean `FilesystemMcpServer` in `memstead-mcp`, and the folder-only surface of `memstead-cli`. **Full** (default, `mem-repo`, adds the git-branch backend and multi-mem policy): pulls in `memstead-git-branch` and the full-engine extension crate `memstead-engine` (mem-lifecycle orchestrators + the lifecycle-only `FullEngineError` variants over `memstead_base::Engine`), and compiles the full `McpServer` (in `memstead-mcp`, alongside `FilesystemMcpServer`, gated by `mem-repo`) plus the full mem-repo `memstead` CLI surface (the cfg-gated `CliEngine::MemRepo` variant in `memstead-cli`). The two MCP servers share `memstead-mcp`'s tool-parameter structs and validation envelope; `main.rs` picks `FilesystemMcpServer` under `not(feature = "mem-repo")` and constructs the unified engine via `memstead_git_branch::workspace_store::engine_from_workspace_root` under `mem-repo`.

## Context
The git-branch backend depends on `gix`, a heavy git library. A single build pulling it in unconditionally would force the `gix` dependency on every consumer, including those who only want the folder backend for simple single-context notes. The product position treats folder and git-branch as distinct affordances (folder = simple notes, git-branch = multi-actor history-bearing knowledge), not one as a lite version of the other.

## Consequences
CI runs both flavours. Because `mem-repo` is the default feature, a plain `cargo build` / `cargo nextest run` is already the full flavour; the lean flavour is reached with `--no-default-features` (the canonical test invocations are `cargo nextest run --workspace --features mem-repo` — explicit-flag form of the default — and `--no-default-features` for lean). The shipped binaries are the single `memstead-mcp` (from `memstead-mcp`) and `memstead` (from `memstead-cli`), defaulting to full; under `--no-default-features` those same crates produce the lean binaries (`memstead-mcp` boots `FilesystemMcpServer`, `memstead` rejects mem-repo workspaces with an actionable error). There are no separate lean-named binaries. Surface-crate reuse means MCP/CLI parameter-shape changes propagate through both flavours in one session.
The split also creates an asymmetric rot hazard: because local dev runs the full cfg, code reachable only under `cfg(not(feature = "mem-repo"))` (lean-only error arms, feature-gate boundaries) is dead code on every developer machine and can drift without a compile error — the lean build sat red from the workspace-store rebuild until 2026-06-10 (33 errors: an over-gated module, over-gated imports, and a lean-only arm constructing a stale field shape that no full build ever compiled). The `--no-default-features` suite is the only guard that exercises this surface; skipping it locally defers lean breakage to CI or later.

## Relationships
- **REFERENCES**: [[engine:storage-backend]]
- **MOTIVATED_BY**: [[engine:storage-backend]]

## Options

An earlier deliberation weighed three ways to keep `gix` out of the folder-only build:
- **Cargo features selecting the flavour at build time (chosen mechanism).** `mem-repo` is on by default, so a plain build is full (git-branch backend included) and `--no-default-features` yields lean (`memstead-base` only, no `gix`). Build-time exclusion is verifiable with `cargo tree`; idiomatic Rust. (Realization: the full surface is hosted in the same `memstead-mcp` / `memstead-cli` crates as lean, gated by the default `mem-repo` feature over the shared `MemBackend` seam, with `memstead-engine` carrying the full-only lifecycle extension — there are no separate `memstead-full-*` crates.)
- **Runtime detection only — one build, dispatch at startup (rejected).** Lowest distribution friction, but the single binary would still link `gix` and carry mem-repo code, defeating the explicit requirement that the lean artifact be free of it.
- **A fully separate crate per flavour with no shared surface (rejected).** Maximally clean dependency graph but duplicates ~80% of the CLI/MCP surface that works identically against both engines once validators are shared — drift-prone.

## Notes

The `MemBackend` trait in `memstead-base::backend` is the seam that makes the split possible: the lean engine talks to folder + archive through it, and the full flavour registers the git-branch backend via a backend factory rather than a compile-time branch in the engine core.
