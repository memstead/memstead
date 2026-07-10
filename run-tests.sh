#!/bin/bash
#
# run-tests.sh — the open engine's test surface, root-hoisted.
#
# The engine workspace lives at the repo root (Cargo.toml + crates/ + xtask/),
# so tests run from $ROOT directly — there is no engine/ subdir. The private
# registry, the macOS app, and the internal CI guards are not part of the
# open repo; they live in the sibling private repo and run there.

ROOT=$(cd "$(dirname "$0")" && pwd)
FAILED=()

echo ""
echo "══════════════════════════════════"
echo "  Testing: engine (Rust, full flavour)"
echo "══════════════════════════════════"
if (cd "$ROOT" && cargo nextest run --workspace --features mem-repo); then
  echo "  ✓ engine (full) passed"
else
  FAILED+=("engine-full")
  echo "  ✗ engine (full) FAILED"
fi

echo ""
echo "══════════════════════════════════"
echo "  Testing: engine (Rust, lean flavour)"
echo "══════════════════════════════════"
# lean is the --no-default-features, folder-backend-only build (no gix). Both
# flavours must stay green — public CI runs lean-smoke and full-smoke.
if (cd "$ROOT" && cargo nextest run --workspace --no-default-features); then
  echo "  ✓ engine (lean) passed"
else
  FAILED+=("engine-lean")
  echo "  ✗ engine (lean) FAILED"
fi

echo ""
echo "══════════════════════════════════"
echo "  Testing: memstead-cli (true lean build)"
echo "══════════════════════════════════"
# The workspace-wide lean run above still compiles memstead-cli WITH
# mem-repo: xtask depends on it with that feature on, and cargo unifies
# features across one build graph. Only a targeted -p build exercises
# the cli's real lean flavour (its cfg(not(mem-repo)) branches — e.g.
# the schema-new follow-up that routes through a fresh init).
if (cd "$ROOT" && cargo nextest run -p memstead-cli --no-default-features); then
  echo "  ✓ memstead-cli (true lean) passed"
else
  FAILED+=("memstead-cli-lean")
  echo "  ✗ memstead-cli (true lean) FAILED"
fi

echo ""
echo "══════════════════════════════════"
echo "  Gate: plugin must not call git against mem-repo"
echo "══════════════════════════════════"
# Plugin code must reach mem-repo via memstead-cli (subprocess) or
# memstead-mcp (MCP); writes go through MCP. Outer-repo git operations on
# the user's project repo are explicitly carved out.
if "$ROOT/plugins/claude-code/scripts/check-architecture.sh"; then
  echo "  ✓ plugin architecture guard passed"
else
  FAILED+=("plugin-architecture")
  echo "  ✗ plugin architecture guard FAILED"
fi

echo ""
echo "══════════════════════════════════"
echo "  Testing: plugin (node --test)"
echo "══════════════════════════════════"
if (cd "$ROOT" && node --test plugins/claude-code/hooks/*.test.js plugins/claude-code/skills/ingest/scripts/*.test.js); then
  echo "  ✓ plugin tests passed"
else
  FAILED+=("plugin-tests")
  echo "  ✗ plugin tests FAILED"
fi

echo ""
if [ ${#FAILED[@]} -eq 0 ]; then
  echo "All passed."
  exit 0
else
  echo "Failed: ${FAILED[*]}"
  exit 1
fi
