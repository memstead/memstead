#!/bin/bash
#
# untagged-release-issue.sh: turn the untagged-release check into one issue.
#
# A check only helps someone who runs it. 0.9.0 went untagged for four days
# because nothing ran on a schedule and nothing wrote anywhere a person
# looks. The `untagged-release` workflow runs this script daily: it runs
# scripts/untagged-release.sh and keeps exactly one open issue in step with
# the result.
#
#   tripped (exit 1)  -> open the issue if none is open, else update its
#                        body with the current reading (never a second issue)
#   clear   (exit 0)  -> close the open issue with a comment, if there is one;
#                        file nothing on a tagged state
#   skipped (exit 3)  -> touch nothing; the remote could not be read and an
#                        unreadable remote is not evidence either way
#
# The issue is found by its title prefix, not by a label, so the repository
# needs no label created by hand. Needs `gh` with `issues: write`.
#
# Usage:  scripts/untagged-release-issue.sh [--repo <owner/name>]
# Exit: the check's own exit code (1 when tripped, so the scheduled run is
# red beside the issue), 0 when clear or skipped, 2 on a `gh` failure.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TITLE_PREFIX="Untagged release:"
REPO_ARGS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --repo) REPO_ARGS=(--repo "${2:-}"); shift 2 ;;
    -h|--help) sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "untagged-release-issue: unknown argument '$1'" >&2; exit 2 ;;
  esac
done

if ! command -v gh >/dev/null 2>&1; then
  echo "untagged-release-issue: the gh CLI is not installed, nothing filed" >&2
  exit 2
fi

reading=$("$ROOT/scripts/untagged-release.sh")
status=$?
printf '%s\n' "$reading"

# The one open issue this script owns, by title prefix; empty when none.
open_issue=$(gh issue list ${REPO_ARGS[@]+"${REPO_ARGS[@]}"} --state open --limit 50 \
  --json number,title --jq ".[] | select(.title | startswith(\"$TITLE_PREFIX\")) | .number" 2>/dev/null | head -1)

case "$status" in
  1)
    version=$(printf '%s' "$reading" | sed -n 's/.*workspace \([0-9][^ ]*\) was cut.*/\1/p' | head -1)
    title="$TITLE_PREFIX v${version:-?} was cut but never tagged"
    body=$(printf '%s\n\n%s\n\n%s\n' \
      "The daily untagged-release check trips. The tag is the entire outward release: until it exists, every channel keeps serving the previous version." \
      "\`\`\`
$reading
\`\`\`" \
      "This issue is kept in step by the \`untagged-release\` workflow: it is updated on every run while the condition holds and closed by the run that sees it clear. Tag the release, or record the skip in the release chapter and bump past it.")
    if [ -n "$open_issue" ]; then
      if gh issue edit ${REPO_ARGS[@]+"${REPO_ARGS[@]}"} "$open_issue" --title "$title" --body "$body" >/dev/null; then
        echo "untagged-release-issue: updated open issue #$open_issue"
      else
        echo "untagged-release-issue: could not update issue #$open_issue" >&2; exit 2
      fi
    else
      if url=$(gh issue create ${REPO_ARGS[@]+"${REPO_ARGS[@]}"} --title "$title" --body "$body"); then
        echo "untagged-release-issue: filed $url"
      else
        echo "untagged-release-issue: could not file the issue" >&2; exit 2
      fi
    fi
    exit 1
    ;;
  0)
    if [ -n "$open_issue" ]; then
      if gh issue close ${REPO_ARGS[@]+"${REPO_ARGS[@]}"} "$open_issue" --comment "Cleared by the untagged-release workflow: $reading" >/dev/null; then
        echo "untagged-release-issue: closed issue #$open_issue (condition cleared)"
      else
        echo "untagged-release-issue: could not close issue #$open_issue" >&2; exit 2
      fi
    else
      echo "untagged-release-issue: nothing to file"
    fi
    exit 0
    ;;
  3)
    echo "untagged-release-issue: check skipped, issue state left as is"
    exit 0
    ;;
  *)
    echo "untagged-release-issue: the check failed to run (exit $status)" >&2
    exit 2
    ;;
esac
