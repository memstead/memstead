#!/bin/bash
# Build the engine binaries needed for local development.
#
# Steps:
#   1. cargo build --workspace   — sanity-check that everything compiles
#   2. cargo install --path crates/memstead-cli --locked   — `memstead` available on PATH
#   3. cargo install --path crates/memstead-mcp --locked   — `memstead-mcp` available next to `memstead` (the CLI's sibling-of-current-exe resolution looks here for mem-lifecycle subprocess spawns)
#   4. cargo build --release -p memstead-mcp   — in-tree release binary for Claude Code's MCP server (`.mcp.json` points at this path, keeping per-checkout isolation distinct from the ~/.cargo/bin/ install)
#
# Layout: one crate per surface. `memstead-cli` produces the `memstead`
# binary and `memstead-mcp` produces the `memstead-mcp` binary; both are
# the full multi-mem, git-backed build by default (the `mem-repo`
# feature is on by default). `--no-default-features` yields the lean
# folder-only build — a CI / dependency-hygiene config, not installed
# here.
#
# Skips the registry (a separate deploy binary) and the macOS app (needs
# Xcode + xcodegen). Run those separately when you actually need them —
# see docs/build.md.

ROOT=$(cd "$(dirname "$0")" && pwd)
FAILED=()

echo ""
echo "══════════════════════════════════"
echo "  Step 1: cargo build --workspace"
echo "══════════════════════════════════"
if (cd "$ROOT" && cargo build --workspace); then
  echo "  ✓ workspace built"
else
  FAILED+=("workspace-build")
  echo "  ✗ workspace build FAILED"
fi

echo ""
echo "══════════════════════════════════"
echo "  Step 2: install memstead CLI"
echo "══════════════════════════════════"
if (cd "$ROOT" && cargo install --path crates/memstead-cli --locked --force); then
  echo "  ✓ memstead installed at ~/.cargo/bin/memstead"
else
  FAILED+=("cli-install")
  echo "  ✗ memstead install FAILED"
fi

echo ""
echo "══════════════════════════════════"
echo "  Step 3: install memstead-mcp (PATH)"
echo "══════════════════════════════════"
if (cd "$ROOT" && cargo install --path crates/memstead-mcp --locked --force); then
  echo "  ✓ memstead-mcp installed at ~/.cargo/bin/memstead-mcp (sibling-of-current-exe target)"
else
  FAILED+=("mcp-install")
  echo "  ✗ memstead-mcp install FAILED"
fi

echo ""
echo "══════════════════════════════════"
echo "  Step 4: build memstead-mcp (release)"
echo "══════════════════════════════════"
if (cd "$ROOT" && cargo build --release -p memstead-mcp); then
  echo "  ✓ memstead-mcp built at target/release/memstead-mcp"
else
  FAILED+=("mcp-build")
  echo "  ✗ memstead-mcp build FAILED"
fi

echo ""
if [ ${#FAILED[@]} -eq 0 ]; then
  echo "All passed."
  echo ""
  echo "Reminder: Claude Code's MCP server caches the previously-spawned"
  echo "binary. Reconnect the 'memstead' MCP server (or restart Claude Code)"
  echo "so the new memstead-mcp binary is picked up."
  exit 0
else
  echo "Failed: ${FAILED[*]}"
  exit 1
fi
