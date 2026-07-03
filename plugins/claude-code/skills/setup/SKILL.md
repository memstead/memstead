---
name: setup
description: First-time setup for a filesystem-mem Memstead workspace — resolves the memstead binaries (installing them if needed), then delegates workspace + mem + MCP wiring to `memstead quickstart` and instructs the user to restart Claude Code so the MCP server registers.
disable-model-invocation: true
allowed-tools: AskUserQuestion, Bash, Read, Write
---

# Memstead — First-time setup

Walk a first-time user through bootstrapping a filesystem-mem Memstead workspace in the current directory. End state: a `.memstead/` workspace store plus `.mcp.json` are written (both by `memstead quickstart`), and the user has been told to restart Claude Code.

This is the lean-product onboarding flow — the binaries are `memstead` (CLI) plus `memstead-mcp` (MCP server). There is no mem-repo, no multi-mem, no git history layer — that surface is the full variant and is not reached from this skill.

## Step 1 — Resolve the binaries

Both `memstead` and `memstead-mcp` must be on `PATH`:

```bash
command -v memstead
command -v memstead-mcp
```

If **both** resolve, skip to step 2.

If either lookup fails (non-zero exit), install them. Work through these options **in order** — try option 1 first, and move to the next option only when its stated fallback condition applies:

1. **Installer script** (preferred — no package manager needed):

   ```bash
   curl -sSf https://memstead.io/install.sh | sh
   ```

   *Fall through to option 2 when:* the download fails (network error, non-200), the script errors out, or `curl` is not available.

2. **Homebrew** (macOS / Linux with `brew` installed):

   ```bash
   brew install memstead/homebrew-memstead/memstead
   ```

   *Fall through to option 3 when:* `brew` is not installed, or the tap/formula cannot be found.

3. **Build from source** (needs a Rust toolchain — `rustup` provides one):

   ```bash
   git clone https://github.com/memstead/memstead
   cd memstead && cargo build --release
   ```

   The binaries land at `target/release/memstead` and `target/release/memstead-mcp`. Either add that directory to `PATH` or capture both absolute paths for the rest of this run.

   *If this also fails:* stop and report the build error verbatim to the user, with the pointer that a Rust toolchain (`curl https://sh.rustup.rs -sSf | sh`) is the usual missing piece. Do not continue to step 2 without working binaries.

After whichever option succeeded, re-run the two `command -v` checks (or use the absolute source-build paths). Only proceed once both binaries resolve.

> **Why `command -v` over `which`:** `command -v` is POSIX, doesn't shell out, and returns absolute paths reliably across `bash`, `zsh`, `dash`, and `sh`. `which` is non-standard and varies by distro.

## Step 2 — Bootstrap the workspace with `memstead quickstart`

All workspace creation is delegated to the CLI — do not hand-write any `.memstead/` file or `.mcp.json`:

```bash
memstead quickstart --agent claude-code
```

One command does everything: workspace store, a default-schema mem named after the folder, a seed entity, and the Claude Code MCP wiring (`.mcp.json` pointing at `memstead-mcp`). It tolerates dotfiles and README-grade files in the target folder.

Handle its outcomes:

- **Success** — the summary names the workspace, mem, schema pin (`default@1.0.0`), and seed entity. Go to step 3.
- **`WORKSPACE_ALREADY_INITIALISED`** — the folder is already a Memstead workspace; nothing to bootstrap. Tell the user, suggest `memstead overview` to inspect it, and go to step 3 (a restart may still be needed if `.mcp.json` is new to this Claude Code project).
- **`TARGET_NOT_EMPTY`** — the folder has content quickstart won't touch. Surface the error verbatim (it names the offending files) and ask the user whether to move the content out or start in a fresh folder (`mkdir my-graph && cd my-graph`). Do not delete or move anything without confirmation.
- **Mem-name derivation failure** (folder name is not slug-shaped `^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$`) — re-run with an explicit name: `memstead quickstart --agent claude-code --name <slug>`. Ask the user for the name; suggest a slugified form of the folder name.
- **Any other error** — surface the message verbatim and stop.

If the user wants a schema other than the default, point them at `memstead link <scope/name>` (registry-published schemas) after setup — quickstart always pins `default@1.0.0`, the built-in 10-type schema.

For scripted / CI use the strict variant is `memstead init` — this skill never needs it; quickstart is the interactive path.

## Step 3 — Tell the user to restart Claude Code

The MCP server only registers on Claude Code startup. Tell the user explicitly:

> Setup complete. **Restart Claude Code** for the `memstead` MCP server to register. Once you're back, run `/start` to see your graph, then `/interview <topic>` to capture knowledge into entities, or just chat — the agent can call `memstead_search` / `memstead_create` / etc. directly.

Do not try to verify the MCP server is reachable from inside this run — the new server only spawns on the next Claude Code session.

## Notes

- **No hand-written config.** `memstead quickstart` owns the workspace store layout (`.memstead/workspace.toml`, `.memstead/config.json`, `.memstead/state/mounts.json`) and `.mcp.json`. The skill never writes or edits these — duplicated init logic drifts from the CLI. In particular, `.memstead/config.json` carries no mem name: the engine derives the mem name from the folder and rejects a stray `name` key (`LEGACY_FIELD_PRESENT`).
- **`.mcp.json` conflicts.** quickstart handles the existing-file case itself; if the user reports a pre-existing hand-authored `.mcp.json` with a different `memstead` entry, inspect it with the user rather than overwriting.
- **No drift handling needed.** Filesystem-mem has no git layer, so there's no `mem-repo/.git/` to verify, no schema-pin negotiation against `__MEMSTEAD`, no multi-mem routing. The setup flow is genuinely just: install → quickstart → restart.
