# Build Guide

What gets built, when, and how. Use this when you don't remember which command rebuilds which binary, or what to do after pulling fresh code.

## TL;DR — the simple way

After pulling main, after editing Rust code, after a `Cargo.toml` change — basically any time you want the local engine binaries in sync with the repo:

```bash
./build-engine.sh
```

That's it. The script:

1. Builds the whole Rust workspace (sanity-checks everything compiles)
2. Installs the `memstead` CLI globally at `~/.cargo/bin/memstead` (with `--locked` so dependency resolution stays predictable)
3. Builds the release `memstead-mcp` binary that an MCP client (e.g. Claude Code) spawns

When it finishes (typically 30 s – 2 min depending on what changed), it reminds you to **reconnect the `memstead` MCP server in your MCP client** so the new binary is picked up. That's the one step the script can't automate — the client caches the previously-spawned subprocess and needs an explicit reconnect.

## What's in the repo

The engine workspace is root-hoisted: `Cargo.toml` + `crates/` + `xtask/` live at the repo root.

| Path | What it is | Build system | Output |
|---|---|---|---|
| `crates/memstead-cli/` | `memstead` CLI binary | Cargo | `target/<profile>/memstead` |
| `crates/memstead-mcp/` | MCP server binary | Cargo | `target/<profile>/memstead-mcp` |
| `crates/memstead-git-branch/` | Engine library (no binary, used by CLI/MCP) | Cargo | `target/<profile>/libmemstead_git_branch.rlib` |
| `crates/memstead-schema/` | Schema layer (library, no binary) | Cargo | linked into others |
| `crates/memstead-swift/` | UniFFI bindings for in-process embedders | Cargo + UniFFI | static lib + Swift bindings |
| `crates/memstead-wasm/` | WASM bindings | Cargo (wasm target) | wasm module + JS glue |
| `plugins/claude-code/` | Claude Code plugin | none — plain `.mjs`/`.json` | runs as-is |

The plugin runs without compilation. Everything else needs a build step. `./build-engine.sh` covers the two binaries (CLI + MCP) plus the workspace sanity-build.

## Prerequisites

| Tool | What for | Install | Needed by `build-engine.sh`? |
|---|---|---|---|
| Rust toolchain (stable, edition 2024) | every Cargo command | `rustup` | yes |
| `cargo-nextest` | running tests fast | `brew install cargo-nextest` | no (tests are not part of the build script) |
| Node.js | running the plugin's `node --test` suite | `brew install node` | no |

On macOS, before running the test suite the first time, follow [docs/macos-dev-setup.md](macos-dev-setup.md) — the Developer Tools exemption is required or `cargo nextest run` deadlocks. Not relevant for `build-engine.sh` itself, only for tests.

## Two flavours: basis and pro

The engine has two build flavours:

- **basis** — default features, folder backend only, no `gix`. `cargo build --no-default-features`.
- **pro** — adds the git-branch storage backend. `cargo build --features vault-repo`. This is the local-dev default.

Both must stay green. `./run-tests.sh` runs both flavours plus the plugin gates; CI runs them as separate smoke jobs.

## Specific workflows

`./build-engine.sh` covers the common case. The sub-sections below are for when you want a subset.

### "I just edited Rust code — does it still compile and pass tests?"

```bash
cargo build --workspace                  # debug build, fast incremental
cargo nextest run --workspace --features vault-repo   # run all tests (~5 s when warm)
```

Cargo handles incremental builds — only changed crates and their dependents recompile. Workspace-crate edits typically rebuild in <30 s. Use this during active iteration; full `./build-engine.sh` is overkill until you actually need the new CLI/MCP binaries.

### "I only need the CLI globally available, skip the rest"

```bash
cargo install --path crates/memstead-cli --locked
```

The `--locked` flag is mandatory — without it Cargo re-resolves dependency versions and breaks on `gix-object` / `winnow` mismatches. Same step as `build-engine.sh` step 2, just standalone.

### "I only need the MCP server, skip the rest"

```bash
cargo build --release -p memstead-mcp
```

Then reconnect the MCP server in your client. Same as step 3 of the script, standalone.

## When does Cargo rebuild what

Cargo's incremental compiler reuses cached artifacts whenever possible. What invalidates the cache:

| Change | Triggers rebuild of |
|---|---|
| Edit a file in `memstead-git-branch/src/` | `memstead-git-branch` and every crate depending on it (~all of them) |
| Edit a file in a leaf crate (e.g. `memstead-cli/src/`) | only that crate |
| `Cargo.toml` workspace edit (deps, profiles) | every crate |
| `Cargo.lock` change (after `cargo update` or `git pull`) | dependencies that changed |
| Switching `--profile` (debug ↔ release) | everything in that profile (separate caches per profile) |
| `cargo clean` | everything |
| Cutting a `--features` set differently | the affected crate and its tree |

Practical implication: edits to `memstead-git-branch` are slower than edits to `memstead-mcp` or `memstead-cli` because `memstead-git-branch` is at the bottom of the dependency graph. Plan your iteration to stay in leaf crates when possible. `./build-engine.sh` is incremental — it'll notice if nothing changed and finish in seconds.

## Cargo profiles

Defined in the root `Cargo.toml`:

```toml
[profile.dev]
debug = "line-tables-only"           # smaller debug binaries → faster linking
split-debuginfo = "unpacked"

[profile.dev.package."*"]
opt-level = 1                        # dependencies compile with light optimisation
                                     # (gix, tantivy, tokio runtime is fast in tests)
```

These mean a one-time slow rebuild (~2–3 min) when you first pull this profile, but every test cycle afterwards is significantly faster — dependency-heavy tests (gix tree walks, tantivy index builds) run optimised.

Workspace crates stay unoptimised so incremental rebuilds during development are fast.

`profile.release` uses Cargo's standard release profile — full optimisation, no debug info.

## Cargo features (memstead-git-branch)

| Feature | Purpose | Default? |
|---|---|---|
| `git-object-storage` | The git-object write path. Mutations go through `gix::object::tree::Editor` against the vault-repo's `.git/`. No working tree. | Yes |
| `disk-storage` | Legacy path: every mutation writes a real file and `vcs::commit` rebuilds the tree by walking disk. Kept compiled-in for rollback. | No |
| `test-support` | Exposes `init_vault_db_stub` for downstream test crates. | No |

Default `cargo build --workspace` (and `./build-engine.sh`) builds with `git-object-storage`. To compile the rollback path:

```bash
cargo build --workspace --features disk-storage --no-default-features
```

You only need this if you're investigating a regression and want to A/B against the legacy storage path.

## Output paths

| Build | Output |
|---|---|
| `cargo build` | `target/debug/<binary>` |
| `cargo build --release` | `target/release/<binary>` |
| `cargo install --path crates/memstead-cli --locked` | `~/.cargo/bin/memstead` |

The `target/` directory grows large (~5 GB+ after several builds across profiles). It's git-ignored. Periodically `cargo clean` it if disk pressure matters — first build after `clean` is slow (~3–5 min).

## Troubleshooting

**`memstead: command not found`**
You haven't installed the CLI. Easiest fix: `./build-engine.sh`. Or just step 2 of it: `cargo install --path crates/memstead-cli --locked`.

**`CONFIG_ERROR: no \`.memstead/workspace.toml\` workspace found in cwd or any ancestor`**
The binary discovers its workspace by walking up from the current directory looking for `.memstead/workspace.toml`. Run the command from inside (or under) a workspace, or `cd` into one first. To create a new workspace, run `memstead init --name <slug> --schema default@1.0.0` in an empty directory.

**MCP server "Failed" in the client**
The binary at `target/release/memstead-mcp` is missing or stale. Run `./build-engine.sh` (or just step 3: `cargo build --release -p memstead-mcp`) and reconnect the MCP server. If the binary runs but exits during engine init, check that the spawn directory (or one of its ancestors) carries a `.memstead/workspace.toml`; the same `CONFIG_ERROR` shape applies as for the CLI.

**`cargo install` fails with `winnow` / `gix-object` version mismatch**
You forgot `--locked`. `./build-engine.sh` always passes `--locked`; if you ran the install command manually, re-run with the flag.

**`cargo nextest run` hangs at the discovery phase on macOS**
The Developer Tools exemption is missing. See [docs/macos-dev-setup.md](macos-dev-setup.md).

**Linker errors after pulling fresh deps**
Run `cargo clean` then `./build-engine.sh`. Cargo's incremental cache occasionally desyncs after large dep upgrades; the clean rebuild costs ~3–5 min.

**Stale `memstead-mcp` processes holding gitdir locks**
```bash
ps aux | grep memstead-mcp                   # find them
pkill -9 -f 'memstead-mcp --config'          # nuke them
```
Common after a client crashes or aborts a session. Stale processes can deadlock test runs and CLI commands.

**`./build-engine.sh` failed at one of the steps**
Read the failure summary at the bottom (`Failed: <step-name>`). Each step's stdout is shown above it — scroll up to find the actual error. Common cases:
- `workspace-build`: Rust compile error in your edits, not a build-system issue. Fix the code.
- `cli-install`: usually a `--locked` Cargo.lock mismatch resolved by pulling main.
- `mcp-build`: same root causes as `workspace-build`, since memstead-mcp depends on memstead-git-branch.
