#!/bin/bash
#
# release-verify.sh — does every distribution channel actually serve the
# version we think we released?
#
# A release is not "the tag was pushed". A release is "everywhere a user can
# get Memstead, they get the new version". Those are different claims, and on
# 2026-08-11 they came apart: v0.6.0 was tagged, built, attested and published
# as Latest while the Homebrew tap quietly kept serving 0.4.0, because one job
# in the release workflow died and nothing downstream cared. The GitHub
# Release still showed green. Nobody would have noticed except by installing.
#
# So this script asks the channels themselves, over the network, from outside:
# it reads what a *user* would get, never what the local tree believes. It
# changes nothing and needs no credentials beyond a plain HTTPS fetch.
#
# Usage:
#   scripts/release-verify.sh            # verify the latest published release
#   scripts/release-verify.sh 0.6.0      # verify a specific version
#
# Exit 0 when every channel agrees, 1 when any channel disagrees or cannot be
# read. A channel that is deliberately not published (crates.io, npm) is
# reported as a stated skip, never as a failure — see the release chapter of
# the handbook for why those two run on their own track.

set -uo pipefail

REPO="memstead/memstead"
TAP_RAW="https://raw.githubusercontent.com/memstead/homebrew-memstead/main/Formula"
DISAGREE=0

say()  { printf '  %-34s %s\n' "$1" "$2"; }
fail() { printf '  %-34s \033[31m%s\033[0m\n' "$1" "$2"; DISAGREE=$((DISAGREE + 1)); }
ok()   { printf '  %-34s \033[32m%s\033[0m\n' "$1" "$2"; }

# ── what are we verifying against ────────────────────────────────────────────
WANT="${1:-}"
if [ -z "$WANT" ]; then
  WANT=$(curl -sS "https://api.github.com/repos/$REPO/releases/latest" \
    | sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' | head -1)
  if [ -z "$WANT" ]; then
    echo "could not read the latest release from GitHub — is the network up?" >&2
    exit 1
  fi
  echo "→ verifying the latest published release: $WANT"
else
  echo "→ verifying release: $WANT"
fi
echo ""

# ── 1. the GitHub Release ────────────────────────────────────────────────────
# The source of truth every other channel is supposed to follow.
latest=$(curl -sS "https://api.github.com/repos/$REPO/releases/latest" \
  | sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' | head -1)
if [ "$latest" = "$WANT" ]; then
  ok "GitHub Release (Latest)" "$latest"
else
  fail "GitHub Release (Latest)" "${latest:-unreadable} — expected $WANT"
fi

# ── 2. install.sh — the primary channel ──────────────────────────────────────
# install.sh resolves "latest" through the same API, so what matters is that
# the release carries the installers a user's shell will actually fetch.
assets=$(curl -sS "https://api.github.com/repos/$REPO/releases/tags/v$WANT" \
  | grep -o '"name": "[^"]*installer.sh"' | wc -l | tr -d ' ')
if [ "${assets:-0}" -ge 2 ]; then
  ok "install.sh installers" "$assets present"
else
  fail "install.sh installers" "${assets:-0} found — expected 2 (cli + mcp)"
fi

# ── 3. Homebrew tap ──────────────────────────────────────────────────────────
# The channel that silently went stale. Read the formulas as brew reads them.
for f in memstead-cli memstead-mcp; do
  v=$(curl -sS "$TAP_RAW/$f.rb" | sed -n 's/^ *version "\([^"]*\)".*/\1/p' | head -1)
  if [ "$v" = "$WANT" ]; then
    ok "Homebrew $f" "$v"
  else
    fail "Homebrew $f" "${v:-unreadable} — expected $WANT"
  fi
done

# ── 4. the Claude Code plugin + its marketplace entry ────────────────────────
# Lockstep with the engine is a standing decision, and xtask does NOT bump
# these — they are hand-edited into the release commit, so they are exactly
# the pair most likely to be forgotten.
for pair in "plugins/claude-code/.claude-plugin/plugin.json:plugin" \
            ".claude-plugin/marketplace.json:marketplace"; do
  path="${pair%%:*}"; label="${pair##*:}"
  v=$(curl -sS "https://raw.githubusercontent.com/$REPO/v$WANT/$path" \
    | sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' | head -1)
  if [ "$v" = "$WANT" ]; then
    ok "Claude Code $label" "$v"
  else
    fail "Claude Code $label" "${v:-unreadable} — expected $WANT"
  fi
done

# ── 5. registries — stated skips, not failures ───────────────────────────────
# Pre-launch posture: binaries are the primary channel and may run ahead.
# Reported so the skew stays a decision someone can see, not a surprise.
crates=$(curl -sS -H "User-Agent: memstead-release-verify (ci@memstead.com)" \
  "https://crates.io/api/v1/crates/memstead-cli" \
  | sed -n 's/.*"max_version": *"\([^"]*\)".*/\1/p' | head -1)
npmv=$(curl -sS "https://registry.npmjs.org/@memstead/wasm" \
  | sed -n 's/.*"latest": *"\([^"]*\)".*/\1/p' | head -1)
say "crates.io (deliberate skip)" "${crates:-unreadable}"
# npm joined the release line: the package is version-matched to the engine
# now, so it is compared like any other channel rather than reported as a
# bare number on a track of its own. Its own line is exactly how it came to
# sit at 0.1.2 against a 0.7.0 CLI with nothing anywhere saying so.
if [ "$npmv" = "$WANT" ]; then
  ok   "npm @memstead/wasm" "$npmv"
elif [ -z "$npmv" ]; then
  fail "npm @memstead/wasm" "unreadable"
else
  fail "npm @memstead/wasm" "$npmv (want $WANT)"
fi

# ── verdict ──────────────────────────────────────────────────────────────────
echo ""
if [ "$DISAGREE" -eq 0 ]; then
  echo "✓ every channel serves $WANT"
  exit 0
else
  echo "✗ $DISAGREE channel(s) disagree — a user's version depends on how they installed"
  exit 1
fi
