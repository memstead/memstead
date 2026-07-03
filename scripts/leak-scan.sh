#!/usr/bin/env bash
#
# leak-scan.sh — fail on any private/leak pattern in the public repo tree.
# Run over the public repo (default: sibling ../public) before its first push,
# and as a CI guard inside the public repo thereafter.
#
# Usage:  scripts/leak-scan.sh [DIR]   (default ../public)
#
# Exits non-zero (and prints the offending hits) on any hit. The OAuth Device
# Flow client ID is intentionally allowed (public by design) and not scanned.

set -uo pipefail

DEST="${1:-../public}"
HITS=0

# Skip build artifacts and the committed-static registry HTTP reference
# (registry.md documents a public HTTP API — an allowed prose mention).
# Exclude this script itself: it *defines* the leak patterns (dev/plans,
# macos/, …) as regex literals, so scanning it self-matches every class.
PRUNE=( --exclude-dir=target --exclude-dir=.git --exclude-dir=node_modules --exclude=leak-scan.sh )

# Allowlist — path-anchored references that match a leak pattern but are
# legitimate and must stay, so they don't mask real leaks elsewhere:
#   * LICENSING.md's per-folder license map MUST name the private/excluded
#     dirs it classifies (`macos/`, `memstead-registry`, `inspector/`,
#     `local-ai/`) — that's the map's whole purpose.
#   * deny-meta-files.test.js uses `dev/plans/...` and `macos/...` strings as
#     deny-logic TEST FIXTURES (verifying the hook denies writes to them).
#   * the pipeline migration's and workspace-loader's `"projection":"macos/graph"`
#     strings are test-fixture projection identifiers, not path references.
ALLOW='leak-scan\.sh:[0-9]+:|LICENSING\.md:[0-9]+:.*(macos/|memstead-registry|inspector/|local-ai/)|deny-meta-files\.test\.js:[0-9]+:.*(dev/plans|macos/)|(pipeline[a-z_]*\.rs|workspace-loader\.test\.js):[0-9]+:.*macos/graph'

scan() { # label  pattern  [extra grep args...]
  local label="$1" pat="$2"; shift 2
  local out
  out="$(grep -rnE "$pat" "$DEST" "${PRUNE[@]}" "$@" 2>/dev/null | grep -vE "$ALLOW" 2>/dev/null || true)"
  if [ -n "$out" ]; then
    echo "✗ LEAK [$label]:"
    echo "$out" | sed 's/^/    /' | head -40
    HITS=$((HITS + 1))
  else
    echo "✓ $label — clean"
  fi
}

echo "→ leak-scanning $DEST"

scan "absolute-user-paths" '/Users/(dasboe|bjornbosenberg)'
# Real secret MATERIAL only — credential values and private-key blocks.
# Env-var *names* (MEMSTEAD_TOKEN, ANTHROPIC_API_KEY) and GitHub Actions
# secret *references* (${{ secrets.X }}) are public-by-design and not leaks.
# The Anthropic key format (`sk-ant-api03-…`) contains hyphens, so it must
# be matched explicitly — the generic `sk-[A-Za-z0-9]{24,}` stops at the
# first hyphen and would pass a real key.
scan "secrets"             '(-----BEGIN [A-Z]+ PRIVATE KEY|ghp_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}|gho_[A-Za-z0-9]{30,}|xox[baprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|sk-ant-[A-Za-z0-9_-]{24,}|sk-[A-Za-z0-9]{24,})'
scan "private-infra"       '(railway\.app|\.up\.railway\.app|railway\.json)'
scan "internal-refs"       '(dev/plans|dev/strategy|dev/ci|LAUNCH\.md)'
scan "stale-product-name"  '\b[Mm]emgno\b'
# Path references to excluded private dirs — anchored to a real path
# boundary so compound tokens (engine-graph/) are not false-matched inside
# hyphenated words.
scan "excluded-private-dirs" '(^|[[:space:]"'"'"'`(:,])(macos|websites|graph|inspector|local-ai)/' \
  --include='*.sh' --include='*.md' --include='*.toml' --include='*.rs' --include='*.mdx' --include='*.yml' \
  --include='*.mjs' --include='*.js' --include='*.json' --include='*.udl' --include='*.swift' --include='*.py'
scan "legacy-domain"       '(mdgv\.io|dasboe/mdgv|dasboe\.github\.io)'
# The sketch product's domain stays out of the public repo until launch.
scan "prelaunch-domain"    'memstead\.ai'

echo
if [ "$HITS" -eq 0 ]; then
  echo "✓ leak scan clean ($DEST)"
  exit 0
else
  echo "✗ leak scan found $HITS pattern class(es) with hits — extraction not leak-free"
  exit 1
fi
