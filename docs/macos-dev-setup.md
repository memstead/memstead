# macOS Dev Setup

One-time configuration that makes `cargo nextest run --workspace` complete in seconds instead of minutes on Apple Silicon. Required if you intend to run the test suite regularly; without these steps, macOS Gatekeeper serialises code-signing checks for fresh test binaries and parallel test discovery deadlocks.

Tested on macOS Sequoia (15.x), Apple Silicon (M1 Pro and later). Intel Macs are unaffected by the Gatekeeper bottleneck and can skip the Developer Tools exemption.

## Prerequisites

- **Xcode Command Line Tools** — `xcode-select --install`
- **Rust toolchain** — `rustup` ≥ recent stable; the workspace is on Edition 2024
- **Homebrew** — for `cargo-nextest`

## Required: Developer Tools exemption

This is the fix for the parallel-binary discovery hang. Without it, `cargo nextest run --workspace` will hang indefinitely after `Finished test profile` while macOS' AMFI daemon serialises code-signing verification across the spawned `--list` subprocesses.

1. Enable the global Developer Tools mode:

   ```bash
   sudo spctl developer-mode enable-terminal
   ```

2. Open **System Settings → Privacy & Security → Developer Tools**.

3. Make sure your terminal app appears in the list and the toggle is **on**. If it isn't listed (common for iTerm2, WezTerm, Ghostty, etc.), click **`+`** and add the `.app` bundle manually. The default `Terminal.app` usually appears automatically after step 1.

4. **Quit and relaunch the terminal** (a new tab/window is not enough — the exemption is loaded at process start).

5. After relaunch, drop any cached binaries that were built under the previous regime:

   ```bash
   cargo clean
   ```

The first build after `cargo clean` re-compiles everything (~2–3 minutes on M1 Pro). All subsequent test cycles use the new binaries and run within Gatekeeper's exemption.

Reference: [nextest macOS installation docs](https://nexte.st/docs/installation/macos/).

## Required: cargo-nextest

The workspace's documented test runner. `cargo test` works too but is significantly slower because it serialises test execution per binary.

```bash
brew install cargo-nextest
cargo nextest --version    # sanity check
```

Run the suite:

```bash
cargo nextest run --workspace
```

Expected wallclock after the steps in this document: **under 10 seconds** for ~1300 tests on M1 Pro. If you see anything in the multi-minute range, the Developer Tools exemption above is not active — re-check step 3.

## Optional: Spotlight exclusion on `target/`

Reduces compile-and-test cycles by another ~10–20% by stopping `mdimport` from indexing every freshly built test binary.

1. **System Settings → Spotlight → Search Privacy** (older macOS: Spotlight → Privacy)
2. Click **`+`** and add the repo's `target/` directory (drag the folder in or browse to it)

No restart needed; the exclusion takes effect immediately for new files.

## Already in the repo: Cargo profile tuning

For reference — you don't need to do anything for these; they're committed in the root `Cargo.toml`:

- `[profile.dev] debug = "line-tables-only"` — keeps stack traces and line numbers but trims debug-info bloat. Smaller debug binaries shorten code-signing verification per cold start.
- `[profile.dev.package."*"] opt-level = 1` — compiles dependencies (gix, tantivy, tokio, …) with light optimisation. Workspace crates stay unoptimised so incremental rebuilds remain fast. Tests that exercise those dependencies (gix tree walks, tantivy index builds) run substantially faster at runtime.

Trade: a one-time fresh dependency rebuild after pulling these settings is ~2–3 minutes longer than it would be without them; cached after that.

## Verifying everything works

```bash
cargo nextest run --workspace
```

Healthy output: `Summary [   ~7s] 1300+ tests run: all passed, N skipped`. Single-digit seconds for the full suite.

If any test binary still hangs at the discovery phase, see the troubleshooting block below.

## Troubleshooting

**Symptom: `cargo nextest run` prints `Finished test profile in 0.XXs` and then nothing for minutes.**

→ Developer Tools exemption is not active for the terminal you're running from. Re-check the Privacy & Security panel; if the entry is there but toggled off, toggle it on, then quit and relaunch the terminal. If the entry is missing, `sudo spctl developer-mode enable-terminal` again and check whether the terminal app you're using is supported (it adds `Terminal.app` only; third-party terminals must be added with `+`).

**Symptom: `Finished` appears, then a 20–30 second pause, then tests start running.**

→ Known macOS behaviour after a fresh build; subsequent runs are fast (the kernel caches the verification result per binary signature). See [nextest issue #1161](https://github.com/nextest-rs/nextest/issues/1161). Not a bug in your setup.

**Symptom: a specific test hangs indefinitely (not the discovery phase).**

→ Check for stale `memstead-mcp` processes holding gitdir locks: `ps aux | grep memstead-mcp`. Kill any that have been running for hours: `pkill -9 -f memstead-mcp`. These can accumulate from previous Claude Code sessions or aborted runs.
