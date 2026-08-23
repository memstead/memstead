#!/bin/bash
#
# run-tests.sh — THE definition of "green" for this repo.
#
# CI runs this script and nothing else (.github/workflows/ci.yml), so there is
# exactly one answer to "does this tree pass?" and it is the same answer here
# and there. Two gates that must be kept in sync always drift; one gate cannot.
# Anything you want CI to check belongs in this file, not in a workflow.
#
# The two exceptions are declared, not accidental: the wasm32 dependency gate
# needs a cross-compilation target installed, and the RustSec audit needs the
# network and a fresh advisory database. Both stay their own CI jobs because
# neither can honestly run on an offline laptop.
#
# Order is deliberate: seconds-long gates (format, lint, leak) run before
# minutes-long ones, so a tree that cannot pass `cargo fmt` learns it in
# seconds. Every leg still runs — the script reports ALL failures, never just
# the first, because a run that stops at the first problem makes you pay the
# full wall-clock cost once per problem.
#
# The engine workspace lives at the repo root (Cargo.toml + crates/ + xtask/),
# so tests run from $ROOT directly — there is no engine/ subdir. The private
# registry and the internal CI guards are not part of the
# open repo; they live in the sibling private repo and run there.

ROOT=$(cd "$(dirname "$0")" && pwd)
FAILED=()

echo ""
echo "══════════════════════════════════"
echo "  Lint: rustfmt + clippy (both flavours)"
echo "══════════════════════════════════"
# Byte-identical to the commands CI runs. If you change one, change it HERE —
# CI has no copy of its own; see the header note.
if (cd "$ROOT" \
  && cargo fmt --check \
  && cargo clippy --workspace --all-targets --features mem-repo -- -D warnings \
  && cargo clippy --workspace --all-targets --no-default-features -- -D warnings \
  && cargo clippy -p memstead-cli --all-targets --no-default-features -- -D warnings); then
  echo "  ✓ rustfmt + clippy passed"
else
  FAILED+=("lint")
  echo "  ✗ rustfmt + clippy FAILED"
fi

echo ""
echo "══════════════════════════════════"
echo "  Guards: nothing private or internal leaks to the public tree"
echo "══════════════════════════════════"
if "$ROOT/scripts/leak-scan.sh" "$ROOT" \
  && "$ROOT/scripts/check-no-plan-refs.sh" \
  && "$ROOT/scripts/check-no-mechanism-leak.sh"; then
  echo "  ✓ publication guards passed"
else
  FAILED+=("guards")
  echo "  ✗ publication guards FAILED"
fi

echo ""
echo "══════════════════════════════════"
echo "  Gate: the generated reference matches the binaries"
echo "══════════════════════════════════"
# docs-site/reference is DERIVED — clap help, the MCP tool table,
# the error index. Editing a help string without regenerating leaves the
# published reference asserting something the shipped binary no longer
# does, and that page is more discoverable than `--help`. This gate is
# read-only (`--check` writes nothing); the failure message names the
# regeneration command.
if (cd "$ROOT" && cargo run -q -p xtask -- generate-docs \
      --output docs-site/src/content/docs/reference --check); then
  echo "  ✓ generated reference is current"
else
  FAILED+=("generated-docs")
  echo "  ✗ generated reference is STALE"
fi

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
echo "  Testing: doctests (cargo test --doc)"
echo "══════════════════════════════════"
# nextest skips doctests by design, so without this leg every doc example
# in the crates was a stated API contract that never executed anywhere —
# not locally, not in CI.
if (cd "$ROOT" && cargo test --doc --workspace --features mem-repo); then
  echo "  ✓ doctests passed"
else
  FAILED+=("doctests")
  echo "  ✗ doctests FAILED"
fi

echo ""
echo "══════════════════════════════════"
echo "  Testing: engine (Rust, lean flavour)"
echo "══════════════════════════════════"
# lean is the --no-default-features, folder-backend-only build (no gix). Both
# flavours must stay green — public CI runs lean-smoke and full-smoke.
# Lean legs build into their own target dir (target/lean): sharing the
# default dir left a degraded --no-default-features binary at
# target/debug/memstead after every full run — a binary that is not what
# its path says it is (cost two sessions a false-negative probe round
# each). Isolation also keeps lean artifacts cached across runs.
if (cd "$ROOT" && cargo nextest run --workspace --no-default-features --target-dir target/lean); then
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
if (cd "$ROOT" && cargo nextest run -p memstead-cli --no-default-features --target-dir target/lean); then
  echo "  ✓ memstead-cli (true lean) passed"
else
  FAILED+=("memstead-cli-lean")
  echo "  ✗ memstead-cli (true lean) FAILED"
fi

echo ""
echo "══════════════════════════════════"
echo "  Testing: memstead-mcp (true lean build)"
echo "══════════════════════════════════"
# Same feature-unification trap as the CLI leg above, and it had the same
# consequence: memstead-mcp's `cfg(not(mem-repo))` tests (boot_lean.rs,
# wire_shape_lean.rs) were compiled out of every leg, so they existed
# without ever running. A targeted -p build is the only way to reach the
# lean MCP binary's own behaviour.
if (cd "$ROOT" && cargo nextest run -p memstead-mcp --no-default-features --target-dir target/lean); then
  echo "  ✓ memstead-mcp (true lean) passed"
else
  FAILED+=("memstead-mcp-lean")
  echo "  ✗ memstead-mcp (true lean) FAILED"
fi

# Decision 9: the Rust gate must not hard-depend on node. Every node leg
# below shares this check; a node-less environment skips each one LOUDLY
# (a degraded green that names what was not checked), never silently.
HAS_NODE=0
if command -v node >/dev/null 2>&1; then HAS_NODE=1; fi

echo ""
echo "══════════════════════════════════"
echo "  Gate: docs-site guard prebuild"
echo "══════════════════════════════════"
# The prebuild (docs-site/scripts/copy-openapi.mjs) carries the skills
# roster guard: it reads the live SKILL.md directories, asserts the
# roster is exactly the expected set, and throws on drift. Before this
# leg it ran only in the post-merge deploy workflow — a guard that can
# only fail AFTER the tree merged. Node-free environments skip loudly
# (decision 9): a skip is a degraded mode, not a silent pass.
if [ "$HAS_NODE" = 1 ]; then
  if (cd "$ROOT" && node docs-site/scripts/copy-openapi.mjs); then
    echo "  ✓ docs-site guard prebuild passed"
  else
    FAILED+=("docs-site-guards")
    echo "  ✗ docs-site guard prebuild FAILED"
  fi
else
  echo "  ⚠⚠⚠ SKIPPED — node is not installed. The docs-site guards did NOT run:"
  echo "  ⚠⚠⚠   - skills roster guard (roster set, frontmatter, invocation posture)"
  echo "  ⚠⚠⚠   - glossary/openapi prebuild sync"
  echo "  ⚠⚠⚠ A green run WITHOUT node is a degraded green. Install node to close it."
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
if [ "$HAS_NODE" = 1 ]; then
  if (cd "$ROOT" && node scripts/check-skill-prose.mjs); then
    echo "  ✓ plugin roster prose lint passed"
  else
    FAILED+=("plugin-skill-prose")
    echo "  ✗ plugin roster prose lint FAILED"
  fi
else
  echo "  ⚠⚠⚠ SKIPPED — node is not installed. The roster prose lint did NOT run."
fi

echo ""
echo "══════════════════════════════════"
echo "  Testing: plugin (node --test)"
echo "══════════════════════════════════"
if [ "$HAS_NODE" = 1 ]; then
  if (cd "$ROOT" && node --test plugins/claude-code/hooks/*.test.js plugins/claude-code/skills/ingest/scripts/*.test.js plugins/claude-code/scripts/*.test.mjs scripts/*.test.mjs); then
    echo "  ✓ plugin tests passed"
  else
    FAILED+=("plugin-tests")
    echo "  ✗ plugin tests FAILED"
  fi
else
  echo "  ⚠⚠⚠ SKIPPED — node is not installed. The plugin hook/router/script tests did NOT run."
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
if [ "$HAS_NODE" = 1 ]; then
  if (cd "$ROOT" && node --test docs/schemas/memstead-plugin/v1/validator.test.mjs); then
    echo "  ✓ workspace format schemas passed"
  else
    FAILED+=("format-schemas")
    echo "  ✗ workspace format schemas FAILED"
  fi
else
  echo "  ⚠⚠⚠ SKIPPED — node is not installed. The workspace format-schema validators did NOT run."
fi

echo ""
echo "══════════════════════════════════"
echo "  Gate: the public prose describes the binary"
echo "══════════════════════════════════"
# Every `memstead <cmd>` and long flag in a fenced shell block or a `run:`
# line of the public prose set (README, CONTRIBUTING, GLOSSARY, VISION,
# the examples README, docs/ minus the divergence corpus, the docs-site
# guides and concepts, the plugin's markdown) must resolve against the
# binary the engine leg built, and every relative link must exist. The
# generated reference was the only machine-gated prose for weeks while
# the README, the guides and the plugin described flags no published
# binary accepted. The checker (ci/check_prose.py) hard-codes no path:
# the file set is computed here, the binary is an argument, and
# xtask release runs the same checker over the flagship at whole-file
# scope. Its own fixtures run first, so a checker that stopped seeing
# defects would fail here before it could pass anything.
if [ -x "$ROOT/target/debug/memstead" ]; then
  PROSE_SET="$( { cd "$ROOT" && ls README.md CONTRIBUTING.md GLOSSARY.md VISION.md examples/README.md 2>/dev/null; \
      find docs -name '*.md' -not -path 'docs/proof/divergence/*'; \
      find docs-site/src/content/docs/guides docs-site/src/content/docs/concepts \( -name '*.md' -o -name '*.mdx' \) 2>/dev/null; \
      find plugins/claude-code -name '*.md'; } | sort -u )"
  if (cd "$ROOT" && python3 ci/check_prose.py --self-test ci/fixtures/prose >/dev/null \
      && echo "$PROSE_SET" | xargs python3 ci/check_prose.py --memstead target/debug/memstead \
           --scope fenced --allow xtask/docs-guard-allow.txt --routes-root docs-site/src/content/docs); then
    echo "  ✓ public prose resolves against the binary"
  else
    FAILED+=("prose-vs-binary")
    echo "  ✗ public prose does NOT resolve against the binary (or the checker's fixtures failed)"
  fi
else
  FAILED+=("prose-vs-binary")
  echo "  ✗ no target/debug/memstead — the prose-vs-binary gate could not run"
fi

echo ""
echo "══════════════════════════════════"
echo "  Gate: the documented verify-in-CI example still works"
echo "══════════════════════════════════"
# The guide prints a GitHub Actions job strangers copy. A printed example
# nobody runs rots silently, so the same command runs here against the
# committed fixture in both polarities (clean -> 0, drifted -> 6) plus an
# operational failure (-> 3, never 6). The harness also asserts the guide
# still prints the command it runs, so the example and the exercise cannot
# drift apart. Needs the full-feature binary the engine leg above built.
if [ -x "$ROOT/target/debug/memstead" ]; then
  if (cd "$ROOT" && python3 ci/verify_gate.py --memstead target/debug/memstead); then
    echo "  ✓ verify-in-CI example exercised"
  else
    FAILED+=("verify-gate-example")
    echo "  ✗ the documented verify-in-CI example FAILED"
  fi
else
  FAILED+=("verify-gate-example")
  echo "  ✗ no target/debug/memstead — the verify-in-CI gate could not run"
  echo "    (a gate that silently skips is a gate that silently stops gating;"
  echo "     the engine leg above builds this binary, so its absence is a bug)"
fi

echo ""
echo "══════════════════════════════════"
echo "  Gate: target/debug/memstead is the full-feature binary"
echo "══════════════════════════════════"
# The behavioural invariant behind the lean target-dir isolation above: a
# green run leaves behind a binary that is what its path says it is. Only
# checked when the binary exists (a docs-only tree may never build it).
if [ -x "$ROOT/target/debug/memstead" ]; then
  if "$ROOT/target/debug/memstead" mem --help >/dev/null 2>&1; then
    echo "  ✓ full-feature binary intact (mem subcommand answers)"
  else
    FAILED+=("binary-integrity")
    echo "  ✗ target/debug/memstead lost its full-feature surface (lean leg leaked?)"
  fi
else
  echo "  (no target/debug/memstead built — nothing to check)"
fi

echo ""
if [ ${#FAILED[@]} -eq 0 ]; then
  if [ "$HAS_NODE" = 1 ]; then
    echo "All passed."
  else
    echo "All passed — DEGRADED: node legs (docs-site guards, prose lint, plugin tests, format schemas) were SKIPPED."
  fi
  exit 0
else
  echo "Failed: ${FAILED[*]}"
  exit 1
fi
