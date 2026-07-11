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
# memstead-mcp (MCP); writes go through MCP. No carve-outs — plugin code
# runs no git at all (outer-repo auto-commit retired 2026-07-11).
if "$ROOT/scripts/check-plugin-architecture.sh"; then
  echo "  ✓ plugin architecture guard passed"
else
  FAILED+=("plugin-architecture")
  echo "  ✗ plugin architecture guard FAILED"
fi

echo ""
echo "══════════════════════════════════"
echo "  Lint: plugin roster prose discipline"
echo "══════════════════════════════════"
# Its own named leg (not a glob inside the node-test leg): router line
# caps, no mechanism-term narration, no retired vocabulary, medium-neutral
# descriptions. See the checker header for the full rule/scope map.
if (cd "$ROOT" && node scripts/check-skill-prose.mjs); then
  echo "  ✓ plugin roster prose lint passed"
else
  FAILED+=("plugin-skill-prose")
  echo "  ✗ plugin roster prose lint FAILED"
fi

echo ""
echo "══════════════════════════════════"
echo "  Testing: plugin (node --test)"
echo "══════════════════════════════════"
if (cd "$ROOT" && node --test plugins/claude-code/hooks/*.test.js plugins/claude-code/skills/ingest/scripts/*.test.js plugins/claude-code/scripts/*.test.mjs scripts/*.test.mjs); then
  echo "  ✓ plugin tests passed"
else
  FAILED+=("plugin-tests")
  echo "  ✗ plugin tests FAILED"
fi

echo ""
echo "══════════════════════════════════"
echo "  Testing: workspace format schemas (v1)"
echo "══════════════════════════════════"
# The v1 format schemas live under docs/schemas (dev/docs tooling, not
# plugin payload). The validator test covers metaschema shape + every
# example. The round-trip pin (init output validates against
# v1/binding.schema.json) is split: the JS half lives in the v1 validator
# test; the Rust half (init still emits that golden) is in memstead-cli's
# suite.
if (cd "$ROOT" && node --test docs/schemas/memstead-plugin/v1/validator.test.mjs); then
  echo "  ✓ workspace format schemas passed"
else
  FAILED+=("format-schemas")
  echo "  ✗ workspace format schemas FAILED"
fi

echo ""
if [ ${#FAILED[@]} -eq 0 ]; then
  echo "All passed."
  exit 0
else
  echo "Failed: ${FAILED[*]}"
  exit 1
fi
