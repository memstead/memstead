---
name: setup
description: First-time setup for a filesystem-vault Memstead workspace — resolves the memstead-mcp binary path, prompts for vault name + schema, runs `memstead init`, writes `.mcp.json`, and instructs the user to restart Claude Code so the MCP server registers.
disable-model-invocation: true
allowed-tools: AskUserQuestion, Bash, Read, Write
---

# Memstead — First-time setup

Walk a first-time user through bootstrapping a filesystem-vault Memstead workspace in the current directory. End state: a `.memstead/` directory (carrying `workspace.toml`, the per-vault `config.json`, and engine-managed `state/mounts.json`) plus `.mcp.json` are written, and the user has been told to restart Claude Code.

This is the basis-product onboarding flow — the binary is `memstead` (CLI) plus `memstead-mcp` (MCP server), both expected on `PATH` via `brew install` / cargo-dist install / a local `cargo build`. There is no vault-repo, no multi-vault, no git history layer — that surface is the pro variant and is not reached from this skill.

## Step 1 — Resolve the binaries

Both `memstead` and `memstead-mcp` must be on `PATH`. Resolve them once and use the absolute paths from here on, so the `.mcp.json` we write does not depend on the parent shell's `PATH` at MCP-spawn time.

```bash
command -v memstead     # → /usr/local/bin/memstead (or wherever)
command -v memstead-mcp # → /usr/local/bin/memstead-mcp
```

If either lookup fails (non-zero exit), stop and tell the user how to install:

> Install on macOS / Linux:
>
> ```
> curl -sSf https://memstead.io/install.sh | sh
> ```
>
> Or via Homebrew:
>
> ```
> brew install memstead/homebrew-memstead/memstead
> ```
>
> Then re-run `/setup`.
>
> _The unified `install.sh` and umbrella `memstead` Homebrew formula
> wrap cargo-dist's per-crate artefacts (`memstead-cli` + `memstead-mcp`).
> If `https://memstead.io/install.sh` is not yet wired, build the binaries
> from source (`cargo build --release`) per the project README._

Capture both absolute paths — call them `MEMSTEAD_BIN` and `MEMSTEAD_MCP_BIN` for the rest of the run.

## Step 2 — Verify the workspace is empty

`memstead init` errors out on a non-empty target. Before prompting the user for vault details, check the cwd:

```bash
ls -A .
```

If the directory contains any files (especially `.memstead/`, `.mcp.json`, or unrelated content), stop and ask the user how to proceed:

- If `.memstead/workspace.toml` exists, the workspace is already initialised — skip to step 4 (write `.mcp.json`). Read the vault `name` and `schema` from `.memstead/config.json` (the per-vault config that `memstead init` writes alongside `workspace.toml`).
- If unrelated files are present, surface the list and ask the user to either move to an empty folder or confirm they want to abort. Do not silently overwrite.

## Step 3 — Prompt for vault name and schema

Ask the user two questions. Use `AskUserQuestion` for the schema choice (a fixed list); ask for the vault name in plain text in the same response.

**Vault name.** Slug-shaped (`^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$`). If the cwd basename matches the slug pattern, suggest it as the default — that's the lowest-friction path.

**Schema.** Two options for v1:

- `default@1.0.0` (recommended) — the built-in 10-type schema (`spec`, `memo`, `concept`, `inquiry`, `model`, `narrative`, `perspective`, `principle`, `process`, `assertion`).
- Custom — direct the user to run `memstead link <scope/name>` after setup to attach a registry-published schema; for now, fall back to `default@1.0.0`.

Recommend `default@1.0.0` unless the user explicitly wants something else.

## Step 4 — Run `memstead init`

Once both inputs are confirmed:

```bash
"$MEMSTEAD_BIN" init --name "<vault-name>" --schema "<schema-pin>"
```

The command exits 0 on success and writes:

- `.memstead/workspace.toml` — the marker `memstead-mcp` walks for when discovering a workspace.
- `.memstead/config.json` — per-vault config (`{format, name, schema}`).
- `.memstead/state/mounts.json` — engine-managed mount records.
- `.memstead/cache/` (empty).
- `.memstead/memstead-io/` (empty).

If it errors (non-empty target, invalid name, invalid schema), surface the message verbatim and stop.

## Step 5 — Write `.mcp.json`

Write `.mcp.json` in the workspace root pointing at the resolved `MEMSTEAD_MCP_BIN`. Use the absolute path so the spawned MCP server does not need `memstead-mcp` on the parent shell's `PATH` at agent-spawn time:

```json
{
  "mcpServers": {
    "memstead": {
      "command": "<MEMSTEAD_MCP_BIN>"
    }
  }
}
```

No `args` field — `memstead-mcp` walks up from its working directory looking for `.memstead/workspace.toml`, which `memstead init` just wrote.

If `.mcp.json` already exists, do not overwrite without confirmation. If the existing file already has an `mcpServers.memstead` entry, leave it alone and tell the user — that's the more-common case (workspace re-init under an existing Claude Code project).

## Step 6 — Tell the user to restart Claude Code

The MCP server only registers on Claude Code startup. Tell the user explicitly:

> Setup complete. **Restart Claude Code** for the `memstead` MCP server to register. Once you're back, run `/start` to see your (empty) graph, then `/interview <topic>` to capture knowledge into entities, or just chat — the agent can call `memstead_search` / `memstead_create` / etc. directly.

Do not try to verify the MCP server is reachable from inside this run — the new server only spawns on the next Claude Code session.

## Notes

- **Why `command -v` over `which`.** `command -v` is POSIX, doesn't shell out, and returns absolute paths reliably across `bash`, `zsh`, `dash`, and `sh`. `which` is non-standard and varies by distro.
- **Why no `args: ["--config", ...]`.** That arg is vault-repo-only; the basis `memstead-mcp` walks for `.memstead/workspace.toml` automatically. Passing it would error on the basis binary (the flag is gated out at the clap layer).
- **No drift handling needed.** Filesystem-vault has no git layer, so there's no `vault-repo/.git/` to verify, no schema-pin negotiation against `__SYSTEM`, no multi-vault routing. The setup flow is genuinely just: install → init → register.
