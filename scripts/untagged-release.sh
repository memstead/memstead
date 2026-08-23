#!/bin/bash
#
# untagged-release.sh: was a release cut and then never tagged?
#
# Two releases in a row reached `main` and never reached a user, and no
# machine noticed either time: 0.9.0 was cut, committed and pushed on
# 2026-08-19 and never tagged; 0.10.0's first cut sat on a divergent
# checkout. Every channel served 0.8.1 while the tree said 0.10.0. The
# tag is the entire outward release, so a release commit without its tag
# is a release that does not exist for anyone but the committer.
#
# This script compares the workspace version against the newest tag the
# remote actually carries. It reads tags with `git ls-remote`, never from
# the local ref store (a tag created and never pushed is exactly the
# failure it must not be fooled by), and it compares by SemVer precedence,
# not lexically (`v0.9.0` sorts above `v0.10.0` as a string).
#
# The gate trips when the workspace version exceeds the highest remote tag
# AND the commit that set that version has been on the remote's main for
# more than the grace period (default one day; a release is normally tagged
# within minutes of its CI going green). The release commit is the newest
# commit on <remote>/main whose diff touched the line-anchored `version = "X"`
# in Cargo.toml; its committer date is the best proxy for when it landed.
#
# Usage:  scripts/untagged-release.sh [--remote <name>] [--max-age-hours <n>]
# Exit 0 when the workspace version is tagged or the cut is still within
# the grace period, 1 when an untagged release is overdue (the sha and date
# are named), 3 when the remote cannot be read (SKIPPED: this check fails
# open on a missing network, like ci-status.sh).
#
# `scripts/ci-status.sh` runs this before its CI readout; the
# `untagged-release` workflow runs it daily and files an issue when it
# trips (see scripts/untagged-release-issue.sh).

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REMOTE="origin"
MAX_AGE_HOURS=24

while [ $# -gt 0 ]; do
  case "$1" in
    --remote) REMOTE="${2:-}"; shift 2 ;;
    --max-age-hours) MAX_AGE_HOURS="${2:-}"; shift 2 ;;
    -h|--help) sed -n '2,34p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "untagged-release: unknown argument '$1'" >&2; exit 2 ;;
  esac
done

# ── the workspace version: the single source of truth ───────────────────────
version=$(sed -n '/^\[workspace\.package\]/,/^\[/{ s/^version *= *"\([^"]*\)".*/\1/p; }' "$ROOT/Cargo.toml" | head -1)
if [ -z "$version" ]; then
  echo "untagged-release: could not read [workspace.package] version from Cargo.toml" >&2
  exit 2
fi

# ── SemVer precedence, in awk so macOS (no `sort -V`) and Linux agree ────────
# Prints the highest of the versions on stdin, one per line, bare
# `X.Y.Z[-pre]` form. A prerelease sorts below the release of the same
# core; build metadata (`+…`) is ignored, as the spec says.
semver_max() {
  awk '
    function cmp(a, b,    pa, pb, na, nb, i, x, y) {
      sub(/\+.*/, "", a); sub(/\+.*/, "", b)
      na = split(a, pa, /[.-]/); nb = split(b, pb, /[.-]/)
      for (i = 1; i <= 3; i++) {
        x = pa[i] + 0; y = pb[i] + 0
        if (x != y) return (x > y) ? 1 : -1
      }
      # same core: a release beats a prerelease; two prereleases compare
      # as strings (good enough for the -prerelease.N convention)
      if (na == 3 && nb == 3) return 0
      if (na == 3) return 1
      if (nb == 3) return -1
      x = substr(a, index(a, "-")); y = substr(b, index(b, "-"))
      if (x == y) return 0
      return (x > y) ? 1 : -1
    }
    NF { if (best == "" || cmp($1, best) > 0) best = $1 }
    END { if (best != "") print best }
  '
}

# ── the newest tag the remote carries ───────────────────────────────────────
tags=$(git -C "$ROOT" ls-remote --tags --refs "$REMOTE" 2>/dev/null \
  | awk '{ print $2 }' | sed 's#^refs/tags/##' | sed 's/^v//' \
  | grep -E '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$')
if [ -z "$tags" ]; then
  echo "untagged-release: could not read tags from remote '$REMOTE': SKIPPED, this check fails open"
  exit 3
fi
highest=$(printf '%s\n' "$tags" | semver_max)

ahead=$(printf '%s\n%s\n' "$version" "$highest" | semver_max)
if [ "$highest" = "$version" ]; then
  echo "✓ workspace $version is tagged (v$version on $REMOTE)"
  exit 0
fi
if [ "$ahead" != "$version" ]; then
  echo "· workspace $version is below the newest tag v$highest on $REMOTE: the tree is behind the release line"
  exit 0
fi

# ── the release commit and how long it has been public ──────────────────────
if ! git -C "$ROOT" fetch --quiet "$REMOTE" main 2>/dev/null; then
  echo "untagged-release: could not fetch $REMOTE/main: SKIPPED, this check fails open"
  exit 3
fi
# `-G` on the line-anchored workspace version: the inter-crate pins carry
# the same number mid-line and must not count; the pickaxe matches the
# diff line that set `[workspace.package] version` itself.
version_re=$(printf '%s' "$version" | sed 's/\./\\./g')
release_commit=$(git -C "$ROOT" log -1 --format='%H %ct %cI' -G"^version = \"$version_re\"" "$REMOTE/main" -- Cargo.toml 2>/dev/null)
if [ -z "$release_commit" ]; then
  echo "· workspace $version is above the newest tag v$highest, but no commit on $REMOTE/main sets that version yet (unpushed cut)"
  exit 0
fi
sha=$(echo "$release_commit" | awk '{ print $1 }')
cut_epoch=$(echo "$release_commit" | awk '{ print $2 }')
cut_date=$(echo "$release_commit" | awk '{ print $3 }')
now_epoch=$(date +%s)
age_hours=$(( (now_epoch - cut_epoch) / 3600 ))

if [ "$age_hours" -gt "$MAX_AGE_HOURS" ]; then
  echo "✗ UNTAGGED RELEASE: workspace $version was cut in ${sha:0:7} on $cut_date (${age_hours}h ago) and $REMOTE carries no v$version tag; the newest tag is v$highest."
  echo "  Nothing has been released: the tag IS the outward release. Tag it (git tag -a v$version -m v$version && git push $REMOTE v$version) or record the skip."
  exit 1
fi

echo "· workspace $version was cut in ${sha:0:7} on $cut_date (${age_hours}h ago) and is not yet tagged; the gate trips after ${MAX_AGE_HOURS}h"
exit 0
