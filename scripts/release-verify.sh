#!/bin/bash
#
# release-verify.sh: does every distribution channel actually serve the
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
# changes nothing and needs no credentials beyond a plain HTTPS fetch; the
# publish-job readout uses `gh` when it is on the machine.
#
# Since 2026-08-23 it also runs inside the release workflow itself, as the
# post-announce job `custom-release-verify` (declared in dist-workspace.toml,
# rendered into release.yml by `dist generate`), and on dispatch against any
# tag (.github/workflows/release-verify.yml). There it adds the question a
# channel read cannot answer: did every publish job in the run actually run?
# dist's `announce` accepts a *skipped* publish job by design (prereleases
# skip theirs), so a non-prerelease whose publish job skipped announces a
# release that one channel never received. That is a failure here.
#
# Usage:
#   scripts/release-verify.sh                     # verify the latest published release
#   scripts/release-verify.sh 0.6.0               # verify a specific version (v-prefix optional)
#   scripts/release-verify.sh v0.10.0 --run-id N  # also read the publish jobs of release run N
#
# Options:
#   --run-id <id>     the Release workflow run whose publish jobs are read
#                     (defaults to the newest run of release.yml for the tag)
#   --repo <o/n>      GitHub repository (default: derived from origin, else memstead/memstead)
#   --prose           the published-tag prose report: download the newest
#                     tag's release archive (cached under
#                     $MEMSTEAD_VERIFY_CACHE, default ~/.cache/memstead/release-verify),
#                     run ci/check_prose.py over the user-facing prose (README,
#                     the docs-site guides, the plugin's markdown) against THAT
#                     binary, never a local one, and print one REPORT line per
#                     documented command or flag the published binary lacks
#   --prose-set <p>   a file or directory of the prose set (repeatable;
#                     replaces the default user-facing subset)
#
# Exit codes:
#   0  every channel serves the version and nothing is reported
#   1  fatal: a channel disagrees, a publish job skipped on a non-prerelease,
#      or the invocation itself is invalid (unknown option)
#   2  no channel disagrees, with report-only findings printed (REPORT:
#      lines) and/or channels the script could not read (UNMEASURED lines:
#      a failed read is not a failing subject — the channel may well serve
#      the tag; the script could not look)
#   3  skipped: no network, a rate-limited API root, or the target release
#      could not be resolved (also forced by MEMSTEAD_VERIFY_OFFLINE=1), or
#      the release archive cannot be fetched for --prose
#
# Report-only lines today: the tree-vs-tag gap (the local workspace version
# is ahead of the verified tag, i.e. unreleased changes exist); the changelog
# check (every `## [X.Y.Z]` header has a tag on origin or a "never published"
# note, every compare link names tags that exist); and, with --prose, the
# documented commands and flags the published binary does not accept.
#
# Test seams: MEMSTEAD_VERIFY_TAGS (space-separated bare versions) replaces
# the `git ls-remote` tag read; MEMSTEAD_VERIFY_CACHE points the archive
# cache somewhere a fixture can pre-populate.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAP_RAW="https://raw.githubusercontent.com/memstead/homebrew-memstead/main/Formula"
DISAGREE=0
REPORTS=0
WANT=""
RUN_ID=""
REPO=""
PROSE=0
PROSE_SET=()

while [ $# -gt 0 ]; do
  case "$1" in
    --run-id) RUN_ID="${2:-}"; shift 2 ;;
    --repo) REPO="${2:-}"; shift 2 ;;
    --prose) PROSE=1; shift ;;
    --prose-set) PROSE_SET+=("${2:?--prose-set needs a path}"); shift 2 ;;
    -h|--help) sed -n '2,66p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    # Exit 1, never 2: the CI wrapper renders 2 as a green notice, so a
    # mistyped flag used to produce a line claiming a verification that
    # never ran. An invalid invocation is fatal.
    --*) echo "release-verify: unknown option '$1'" >&2; exit 1 ;;
    *) WANT="${1#v}"; shift ;;
  esac
done

if [ -z "$REPO" ]; then
  remote_url=$(git -C "$ROOT" remote get-url origin 2>/dev/null || true)
  REPO=$(printf '%s' "$remote_url" | sed -n 's#.*github\.com[:/]\([^/]*/[^/]*\)$#\1#p' | sed 's/\.git$//')
  REPO="${REPO:-memstead/memstead}"
fi

say()    { printf '  %-34s %s\n' "$1" "$2"; }
fail()   { printf '  %-34s \033[31m%s\033[0m\n' "$1" "$2"; DISAGREE=$((DISAGREE + 1)); }
ok()     { printf '  %-34s \033[32m%s\033[0m\n' "$1" "$2"; }
report() { printf '  REPORT: %s\n' "$1"; REPORTS=$((REPORTS + 1)); }
# The third state: the script could not read the channel. Not a
# disagreement (the channel may well serve the tag) and not agreement
# (nothing was observed). Rides the report counter — exit 2, never 1 —
# and the verdict line names it separately.
UNMEASURED=0
unmeasured() { printf '  %-34s \033[33mUNMEASURED: %s\033[0m\n' "$1" "$2"; UNMEASURED=$((UNMEASURED + 1)); REPORTS=$((REPORTS + 1)); }

# fetch <url> [curl args...]: on HTTP 2xx returns 0 with the body in the
# file $FETCH_BODY (a body file, not stdout: a command substitution would
# run fetch in a subshell and lose the reason). On anything else returns
# non-zero with the reason in FETCH_REASON — drawn from the response itself
# (status code, rate-limit headers, the reset time), never inferred from an
# empty extraction, so an HTTP error body is told apart from a field that
# is genuinely absent. Deliberately anonymous: no authorization header,
# ever — the point of this script is what an unauthenticated stranger
# receives.
FETCH_REASON=""
FETCH_BODY="$(mktemp)"
FETCH_HDR="$(mktemp)"
trap 'rm -f "$FETCH_BODY" "$FETCH_HDR"' EXIT
fetch() {
  local url="$1"; shift
  local code reason reset when
  : > "$FETCH_BODY"; : > "$FETCH_HDR"
  if ! code=$(curl -sS --max-time 20 -D "$FETCH_HDR" -o "$FETCH_BODY" -w '%{http_code}' "$@" "$url" 2>/dev/null); then
    FETCH_REASON="network error reading $url"
    return 1
  fi
  if [ "$code" -ge 200 ] && [ "$code" -lt 300 ]; then
    FETCH_REASON=""
    return 0
  fi
  reason="HTTP $code from $url"
  if [ "$code" = "403" ] || [ "$code" = "429" ]; then
    if grep -qi 'rate limit' "$FETCH_BODY" 2>/dev/null || grep -qi '^x-ratelimit-remaining: *0' "$FETCH_HDR" 2>/dev/null; then
      reset=$(sed -n 's/^[Xx]-[Rr]ate[Ll]imit-[Rr]eset: *//p' "$FETCH_HDR" | tr -d '\r' | head -1)
      if [ -n "$reset" ]; then
        when=$(date -u -r "$reset" '+%H:%M:%S UTC' 2>/dev/null || date -u -d "@$reset" '+%H:%M:%S UTC' 2>/dev/null || echo "epoch $reset")
        reason="rate limited (anonymous quota; resets $when)"
      else
        reason="rate limited (anonymous quota)"
      fi
    fi
  fi
  FETCH_REASON="$reason"
  return 1
}

# ── is there a network at all ────────────────────────────────────────────────
# No network is not a red release; it is no verdict. Exit 3 says so, and the
# callers (the release chapter, the hygiene lane later) treat it as a named
# skip rather than a failure or a silent green.
if [ "${MEMSTEAD_VERIFY_OFFLINE:-0}" = "1" ]; then
  echo "SKIPPED: no network (MEMSTEAD_VERIFY_OFFLINE=1)"
  exit 3
fi
# The probe must fail on every response shape the channel reads themselves
# would fail on — a rate-limited 403 completes the request, so a bare curl
# exit test passed the probe and then read eight channels it could not read.
if ! fetch "https://api.github.com"; then
  case "$FETCH_REASON" in
    "rate limited"*) echo "SKIPPED: api.github.com is $FETCH_REASON; the channels cannot be read anonymously right now" ;;
    *) echo "SKIPPED: no network (api.github.com unreachable)" ;;
  esac
  exit 3
fi

# ── the tags origin carries (bare versions), read once ──────────────────────
remote_tags() {
  if [ -n "${MEMSTEAD_VERIFY_TAGS:-}" ]; then
    printf '%s\n' $MEMSTEAD_VERIFY_TAGS
    return 0
  fi
  git -C "$ROOT" ls-remote --tags --refs origin 2>/dev/null \
    | awk '{ print $2 }' | sed 's#^refs/tags/##; s/^v//' \
    | grep -E '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'
}
TAGS="$(remote_tags)"
has_tag() { printf '%s\n' "$TAGS" | grep -qx "$1"; }

# ── --prose: the published-tag prose report ──────────────────────────────────
# The report-only question the channel reads cannot answer: does the prose a
# user reads TODAY describe the binary a user installs TODAY? The newest tag's
# own archive is downloaded (once, cached) and the prose checker runs against
# that binary; a local binary's version string is never consulted, because
# the local binary is exactly what was 33 commits ahead of the tag when the
# README described flags no published binary accepted.
if [ "$PROSE" -eq 1 ]; then
  if [ -n "$WANT" ]; then
    tag="$WANT"
  elif [ -n "${MEMSTEAD_VERIFY_TAGS:-}" ]; then
    tag="$(printf '%s\n' "$TAGS" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)"
  else
    tag="$("$ROOT/scripts/untagged-release.sh" --highest-tag 2>/dev/null || true)"
  fi
  if [ -z "$tag" ]; then
    echo "SKIPPED: no network (the newest tag could not be read from origin)"
    exit 3
  fi
  cache="${MEMSTEAD_VERIFY_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/memstead/release-verify}/$tag"
  bin="$cache/memstead"
  if [ ! -x "$bin" ]; then
    triple="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')"
    if [ -z "$triple" ]; then
      case "$(uname -s)-$(uname -m)" in
        Darwin-arm64) triple=aarch64-apple-darwin ;;
        Darwin-x86_64) triple=x86_64-apple-darwin ;;
        Linux-aarch64) triple=aarch64-unknown-linux-gnu ;;
        Linux-x86_64) triple=x86_64-unknown-linux-gnu ;;
        *) echo "release-verify: no release archive for $(uname -s)-$(uname -m)"; exit 3 ;;
      esac
    fi
    mkdir -p "$cache"
    asset="memstead-cli-$triple.tar.xz"
    url="https://github.com/$REPO/releases/download/v$tag/$asset"
    if ! curl -sSfL --max-time 120 -o "$cache/$asset" "$url" 2>/dev/null; then
      rm -f "$cache/$asset"
      echo "SKIPPED: no network (could not fetch $url)"
      exit 3
    fi
    if ! tar -xJf "$cache/$asset" -C "$cache" 2>/dev/null; then
      echo "release-verify: could not extract $cache/$asset"; exit 1
    fi
    found="$(find "$cache" -type f -name memstead -perm -u+x | head -1)"
    if [ -z "$found" ]; then echo "release-verify: no memstead binary inside $asset"; exit 1; fi
    [ "$found" != "$bin" ] && cp "$found" "$bin"
    chmod +x "$bin"
  fi
  echo "→ prose report against the published v$tag binary ($("$bin" --version 2>/dev/null | awk '{print $2}'))"
  if [ "${#PROSE_SET[@]}" -eq 0 ]; then
    PROSE_SET=("$ROOT/README.md" "$ROOT/docs-site/src/content/docs/guides" "$ROOT/plugins/claude-code")
  fi
  files="$(for p in "${PROSE_SET[@]}"; do
    if [ -d "$p" ]; then find "$p" \( -name '*.md' -o -name '*.mdx' \); elif [ -f "$p" ]; then echo "$p"; fi
  done | sort -u)"
  report_out="$(echo "$files" | xargs python3 "$ROOT/ci/check_prose.py" --memstead "$bin" --scope fenced \
    --allow "$ROOT/xtask/docs-guard-allow.txt" --routes-root "$ROOT/docs-site/src/content/docs" 2>&1 || true)"
  gaps="$(printf '%s\n' "$report_out" | grep -E ': (command|flag|link): ' || true)"
  if [ -n "$gaps" ]; then
    while IFS= read -r line; do
      [ -z "$line" ] && continue
      report "prose ahead of v$tag: ${line#    }"
    done <<GAPS
$gaps
GAPS
  else
    echo "  prose at v$tag: every documented command and flag resolves against the published binary"
  fi
fi

# ── what are we verifying against ────────────────────────────────────────────
if [ -z "$WANT" ]; then
  # A failed read here is no verdict about any channel: without a target
  # nothing can be verified, so this is a named skip (exit 3), never a red.
  if ! fetch "https://api.github.com/repos/$REPO/releases/latest"; then
    echo "SKIPPED: could not resolve the latest release ($FETCH_REASON); nothing verified" >&2
    exit 3
  fi
  WANT=$(sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' "$FETCH_BODY" | head -1)
  if [ -z "$WANT" ]; then
    echo "SKIPPED: the latest-release response carries no tag_name; nothing verified" >&2
    exit 3
  fi
  echo "→ verifying the latest published release: $WANT"
else
  echo "→ verifying release: $WANT"
fi
echo ""

# ── 1. the GitHub Release ────────────────────────────────────────────────────
# The source of truth every other channel is supposed to follow.
if fetch "https://api.github.com/repos/$REPO/releases/latest"; then
  latest=$(sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' "$FETCH_BODY" | head -1)
  if [ "$latest" = "$WANT" ]; then
    ok "GitHub Release (Latest)" "$latest"
  else
    fail "GitHub Release (Latest)" "${latest:-no tag_name in the response} (expected $WANT)"
  fi
else
  unmeasured "GitHub Release (Latest)" "$FETCH_REASON"
fi

# ── 2. install.sh, the primary channel ───────────────────────────────────────
# install.sh resolves "latest" through the same API, so what matters is that
# the release carries the installers a user's shell will actually fetch.
if fetch "https://api.github.com/repos/$REPO/releases/tags/v$WANT"; then
  assets=$(grep -o '"name": "[^"]*installer.sh"' "$FETCH_BODY" | wc -l | tr -d ' ')
  if [ "${assets:-0}" -ge 2 ]; then
    ok "install.sh installers" "$assets present"
  else
    fail "install.sh installers" "${assets:-0} found (expected 2: cli + mcp)"
  fi
else
  unmeasured "install.sh installers" "$FETCH_REASON"
fi

# ── 3. Homebrew tap ──────────────────────────────────────────────────────────
# The channel that silently went stale. Read the formulas as brew reads them.
for f in memstead-cli memstead-mcp; do
  if fetch "$TAP_RAW/$f.rb"; then
    v=$(sed -n 's/^ *version "\([^"]*\)".*/\1/p' "$FETCH_BODY" | head -1)
    if [ "$v" = "$WANT" ]; then
      ok "Homebrew $f" "$v"
    else
      fail "Homebrew $f" "${v:-no version in the formula} (expected $WANT)"
    fi
  else
    unmeasured "Homebrew $f" "$FETCH_REASON"
  fi
done

# ── 4. the Claude Code plugin + its marketplace entry ────────────────────────
# Lockstep with the engine is a standing decision; xtask bumps these with
# the release commit, and this is where a forgotten bump would show.
for pair in "plugins/claude-code/.claude-plugin/plugin.json:plugin" \
            ".claude-plugin/marketplace.json:marketplace"; do
  path="${pair%%:*}"; label="${pair##*:}"
  if fetch "https://raw.githubusercontent.com/$REPO/v$WANT/$path"; then
    v=$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' "$FETCH_BODY" | head -1)
    if [ "$v" = "$WANT" ]; then
      ok "Claude Code $label" "$v"
    else
      fail "Claude Code $label" "${v:-no version in the manifest} (expected $WANT)"
    fi
  else
    unmeasured "Claude Code $label" "$FETCH_REASON"
  fi
done

# ── 5. registries, compared, since the tag publishes them ────────────────────
# Until 2026-08-15 this was a stated skip: binaries were the primary channel
# and the registry followed by hand when someone remembered. It stopped
# being remembered, which is how crates.io sat two minor versions back (and
# the npm package, a channel until 2026-09-05, six). The tag publishes
# crates.io now, so it is held to the release like every other channel.
if fetch "https://crates.io/api/v1/crates/memstead-cli" \
    -H "User-Agent: memstead-release-verify (ci@memstead.com)"; then
  crates=$(sed -n 's/.*"max_version": *"\([^"]*\)".*/\1/p' "$FETCH_BODY" | head -1)
  if [ "$crates" = "$WANT" ]; then
    ok   "crates.io" "$crates"
  elif [ -z "$crates" ]; then
    fail "crates.io" "no max_version in the response (expected $WANT)"
  else
    fail "crates.io" "$crates (want $WANT)"
  fi
else
  unmeasured "crates.io" "$FETCH_REASON"
fi
# ── 6. the publish jobs of the release run ───────────────────────────────────
# dist's `announce` runs when every publish job is `success` OR `skipped`;
# skipping is how prereleases opt out. On a non-prerelease a skipped publish
# job is a channel that was never fed while the release announced itself as
# complete. A *failed* publish job never reaches this point (announce is
# skipped, and so is this job), which is why the skipped case is the one
# worth asking about.
echo ""
case "$WANT" in *-*) prerelease=1 ;; *) prerelease=0 ;; esac
if ! command -v gh >/dev/null 2>&1; then
  say "publish jobs" "not read (gh is not installed)"
else
  if [ -z "$RUN_ID" ]; then
    RUN_ID=$(gh run list --repo "$REPO" --workflow release.yml --branch "v$WANT" \
      --limit 1 --json databaseId --jq '.[0].databaseId' 2>/dev/null || true)
  fi
  if [ -z "$RUN_ID" ] || [ "$RUN_ID" = "null" ]; then
    say "publish jobs" "not read (no release.yml run found for v$WANT)"
  else
    jobs=$(gh api "repos/$REPO/actions/runs/$RUN_ID/jobs" --paginate \
      --jq '.jobs[] | "\(.name)\t\(.conclusion // .status)"' 2>/dev/null || true)
    publish_jobs=$(printf '%s\n' "$jobs" | grep -E '^(custom-)?publish-' || true)
    if [ -z "$publish_jobs" ]; then
      say "publish jobs (run $RUN_ID)" "none listed"
    else
      while IFS=$'\t' read -r name conclusion; do
        [ -z "$name" ] && continue
        case "$conclusion" in
          success) ok "publish job $name" "success" ;;
          skipped)
            if [ "$prerelease" = 1 ]; then
              say "publish job $name" "skipped (prerelease: by design)"
            else
              fail "publish job $name" "SKIPPED on a non-prerelease: that channel was never fed"
            fi ;;
          *) fail "publish job $name" "$conclusion" ;;
        esac
      done <<EOF
$publish_jobs
EOF
    fi
  fi
fi

# ── 7. this machine, informational, never a failure ──────────────────────────
# Not a channel a user receives, so it cannot make the release wrong. But a
# maintainer's own install is the version they EXPERIENCE as the product, and
# nothing keeps it in step. The workspace's own binary check
# (scripts/check-local-binaries.sh in the private repo) is the hard gate;
# this line is the reminder.
echo ""
local_bin="$(command -v memstead 2>/dev/null || true)"
if [ -z "$local_bin" ]; then
  say "this machine (not a channel)" "no memstead on PATH"
else
  local_v="$("$local_bin" --version 2>/dev/null | awk '{print $2}' | cut -d'+' -f1)"
  if [ "$local_v" = "$WANT" ]; then
    say "this machine (not a channel)" "$local_v, in step"
  else
    say "this machine (not a channel)" "${local_v:-unreadable}, BEHIND $WANT"
    say "" "→ curl -sSf https://memstead.io/install.sh | sh"
  fi
fi

# ── 8a. report-only: the changelog against the tags ──────────────────────────
# Every versioned header is a published release or says it is not; every
# compare link names tags that exist. 0.9.0 sat as a plain `## [0.9.0]` with
# a compare link to a tag that never existed for four days.
changelog="$ROOT/CHANGELOG.md"
if [ -f "$changelog" ] && [ -n "$TAGS" ]; then
  while IFS= read -r header; do
    v="$(printf '%s' "$header" | sed -n 's/^## \[\([0-9][^]]*\)\].*/\1/p')"
    [ -z "$v" ] && continue
    if ! has_tag "$v" && ! printf '%s' "$header" | grep -qi "never published"; then
      report "changelog: \`## [$v]\` has no tag on origin and no \"never published\" note"
    fi
  done <<HEADERS
$(grep -E '^## \[[0-9]' "$changelog")
HEADERS
  while IFS= read -r link; do
    [ -z "$link" ] && continue
    name="$(printf '%s' "$link" | sed -n 's/^\[\([^]]*\)\]:.*/\1/p')"
    for ref in $(printf '%s' "$link" | sed -n 's#.*/compare/\([^/ ]*\)\.\.\.\([^/ ]*\).*#\1 \2#p'); do
      [ "$ref" = "HEAD" ] && continue
      has_tag "${ref#v}" && continue
      # A bare commit (the real cut of a release that was never tagged)
      # resolves when the repository knows it.
      if printf '%s' "$ref" | grep -qE '^[0-9a-f]{7,40}$' && git -C "$ROOT" cat-file -e "$ref^{commit}" 2>/dev/null; then
        continue
      fi
      report "changelog: compare link for [$name] names $ref, which is neither a tag on origin nor a known commit"
    done
  done <<LINKS
$(grep -E '^\[[^]]+\]: https?://.*/compare/' "$changelog")
LINKS
fi

# ── 8. report-only: the tree against the tag ─────────────────────────────────
# The workspace version in Cargo.toml ahead of the verified tag means the
# tree carries a cut (or work) no user has. Not a channel defect, so it does
# not fail the release; it is printed so the gap is never silent (0.9.0 sat
# one version ahead of every channel for four days).
echo ""
tree_version=$(sed -n '/^\[workspace\.package\]/,/^\[/{ s/^version *= *"\([^"]*\)".*/\1/p; }' "$ROOT/Cargo.toml" 2>/dev/null | head -1)
if [ -n "$tree_version" ] && [ "$tree_version" != "$WANT" ]; then
  report "tree is at $tree_version, verified tag is v$WANT: the tree carries what no channel serves"
fi

# ── verdict ──────────────────────────────────────────────────────────────────
echo ""
if [ "$DISAGREE" -gt 0 ]; then
  echo "✗ $DISAGREE channel(s) or publish job(s) disagree: a user's version depends on how they installed"
  exit 1
elif [ "$UNMEASURED" -gt 0 ]; then
  # The verdict line must not assert more than was observed: an unmeasured
  # channel is neither wrong nor confirmed.
  echo "− $UNMEASURED channel(s) UNMEASURED (could not be read); every channel that was read serves $WANT ($REPORTS finding(s) above)"
  exit 2
elif [ "$REPORTS" -gt 0 ]; then
  echo "✓ every channel serves $WANT ($REPORTS report-only finding(s) above)"
  exit 2
else
  echo "✓ every channel serves $WANT"
  exit 0
fi
