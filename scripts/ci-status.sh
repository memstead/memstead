#!/bin/bash
#
# ci-status.sh — is main green right now?
#
# On 2026-08-09 CI went red on main. Nobody looked for two days, and the next
# session that needed a green tree — the 0.6.0 release — inherited four
# unrelated failures at once and turned into a repair job. The signal was
# there the whole time. Reading it cost nobody anything; not reading it cost a
# day.
#
# So: one command, one line of output, no excuses. Run it before you start
# work on this repo, and before you cut a release.
#
# Note honestly what this is NOT: a script only helps someone who runs it. The
# real guard against a red main going unnoticed is GitHub's own notification
# on a failed workflow run, which is a per-account setting (Settings →
# Notifications → Actions) and therefore the operator's to enable, not this
# repo's. This script makes the state cheap to check; the notification makes
# it impossible to miss. Have both.
#
# Since 2026-08-23 it also asks the question a green main cannot answer:
# was the last release cut and then never tagged? 0.9.0 sat on a green
# main for four days with every channel serving 0.8.1, because the tag,
# not the commit, is the outward release. scripts/untagged-release.sh
# owns that check (tags read from the remote, SemVer precedence, a
# one-day grace period); this script runs it first and refuses on it.
#
# Usage:  scripts/ci-status.sh [branch]     (default: main)
# Exit 0 when every workflow on the branch head succeeded and no release
# is overdue for its tag, 1 otherwise.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BRANCH="${1:-main}"

# ── 1. an untagged release is a red state, whatever CI says ─────────────────
"$ROOT/scripts/untagged-release.sh"
case $? in
  0|3) ;;                      # tagged / within grace / skipped (fails open, notice printed)
  1) exit 1 ;;
  *) echo "ci-status: untagged-release check could not run, skipping it (this check fails open)" ;;
esac

# ── 2. the CI readout ───────────────────────────────────────────────────────
# The repo is read from the remote, not hard-coded, so a fork or a scratch
# clone asks about itself; a remote that is not on GitHub fails open.
remote_url=$(git -C "$ROOT" remote get-url origin 2>/dev/null || true)
REPO=$(printf '%s' "$remote_url" | sed -n 's#.*github\.com[:/]\([^/]*/[^/]*\)$#\1#p' | sed 's/\.git$//')
if [ -z "$REPO" ]; then
  echo "ci-status: origin is not a GitHub remote (${remote_url:-none}), skipping the CI readout (this check fails open)"
  exit 0
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "ci-status: the gh CLI is not installed — skipping (this check fails open)"
  exit 0
fi

sha=$(gh api "repos/$REPO/commits/$BRANCH" --jq '.sha' 2>/dev/null)
if [ -z "$sha" ]; then
  echo "ci-status: could not reach GitHub — skipping (this check fails open)"
  exit 0
fi

# Check-runs rather than workflow runs: a workflow's run-level status can lag
# behind its jobs (observed on the 0.6.0 tag — every job completed successfully
# while the run still reported in_progress for over half an hour), whereas the
# check-runs of a commit reflect what actually finished.
#
# Only THIS repository's workflows count. GitHub runs its own workflows
# against the same commit (the Dependabot updater, path
# `dynamic/dependabot/...`), and their check-runs sit beside ours under
# the same app; on 2026-08-23 one of those failed inside GitHub's updater
# and painted a green main red here. The check suites of runs whose
# workflow file lives under `.github/workflows/` are the repository's;
# everything else is reported as ignored, never as red or green.
suites=$(gh api "repos/$REPO/actions/runs?head_sha=$sha&per_page=100" \
  --jq '.workflow_runs[] | select(.path | startswith(".github/workflows/")) | .check_suite_id' 2>/dev/null | sort -u | paste -sd, -)
all_runs=$(gh api "repos/$REPO/commits/$sha/check-runs" --paginate \
  --jq '.check_runs[] | "\(.status)\t\(.conclusion // "-")\t\(.name)\t\(.check_suite.id)"' 2>/dev/null)
if [ -n "$suites" ]; then
  runs=$(echo "$all_runs" | awk -F'\t' -v suites="$suites" 'BEGIN { n = split(suites, s, ","); for (i = 1; i <= n; i++) keep[s[i]] = 1 } keep[$4] { print $1 "\t" $2 "\t" $3 }')
  ignored=$(echo "$all_runs" | awk -F'\t' -v suites="$suites" 'BEGIN { n = split(suites, s, ","); for (i = 1; i <= n; i++) keep[s[i]] = 1 } !keep[$4] { print $3 " (" $2 ")" }')
  [ -n "$ignored" ] && echo "ci-status: ignoring checks from workflows this repository does not define: $(echo "$ignored" | tr '\n' ',' | sed 's/,$//; s/,/, /g')"
else
  # The runs query fails open to the old readout: every check-run counts.
  runs=$(echo "$all_runs" | awk -F'\t' '{ print $1 "\t" $2 "\t" $3 }')
fi

if [ -z "$runs" ]; then
  echo "ci-status: no checks reported yet on ${BRANCH} @ ${sha:0:7}"
  exit 0
fi

total=$(echo "$runs" | grep -c .)
bad=$(echo "$runs" | grep -cE "^completed	(failure|cancelled|timed_out|action_required)")
pending=$(echo "$runs" | grep -vc "^completed")

if [ "$bad" -gt 0 ]; then
  echo "✗ ${BRANCH} @ ${sha:0:7} — $bad of $total checks RED:"
  echo "$runs" | grep -E "^completed	(failure|cancelled|timed_out|action_required)" \
    | awk -F'\t' '{print "    " $3 " (" $2 ")"}'
  echo "  Fix this before building on top of it — you are about to inherit it."
  exit 1
elif [ "$pending" -gt 0 ]; then
  echo "· ${BRANCH} @ ${sha:0:7} — $pending of $total checks still running"
  exit 0
else
  echo "✓ ${BRANCH} @ ${sha:0:7} — all $total checks green"
  exit 0
fi
