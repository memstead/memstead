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
# Usage:  scripts/ci-status.sh [branch]     (default: main)
# Exit 0 when every workflow on the branch head succeeded, 1 otherwise.

set -uo pipefail

BRANCH="${1:-main}"
REPO="memstead/memstead"

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
runs=$(gh api "repos/$REPO/commits/$sha/check-runs" \
  --jq '.check_runs[] | "\(.status)\t\(.conclusion // "-")\t\(.name)"' 2>/dev/null)

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
