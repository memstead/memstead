#!/bin/bash
#
# Guard: no plan / subsystem / finding reference leaks into a
# user-facing surface.
#
# CLAUDE.md forbids referencing plans (`dev/plans/`, `dev/archive/`)
# from code, specs, or itself — "the decision trail lives in `git log`
# and the archive; code and specs describe *what is*, not how we got
# there." Manual review missed the class (a probe found plan-tag stamps
# in `--help` text across ten CLI subcommands), so this gate enforces it
# on the surfaces a user/agent actually reads.
#
# Scope (the surfaces, deliberately narrow):
#   * CLI `///` doc comments — clap renders these into `memstead <sub> --help`.
#     The single `memstead-cli` crate carries both flavours (lean/full via
#     the `mem-repo` feature). ALL `///` are scanned, not only clap-rendered
#     ones: CLAUDE.md bans plan references in *all* code, so an internal
#     helper doc carrying a plan tag is a leak too.
#   * MCP `description = "..."` strings — the agent-facing tool/param
#     descriptions on `#[tool]` / `#[schemars]` attributes.
# Engine-emitted error/warning message strings were audited clean and
# carry no plan refs today; they are not machine-scannable without false
# positives, so they stay a review item rather than a grep target.
#
# NOT in scope: `//` implementation comments and `dev/` plan-to-plan
# links — CLAUDE.md permits the latter, and the former is a grey zone
# this gate deliberately leaves alone.
#
# Exit 0 when clean, 1 (with the offending lines) when a leak is found.

set -u

# This guard lives in the public engine repo (public/scripts/) and scans
# the engine crates directly. The repo is root-hoisted (Cargo.toml +
# crates/ at the repo root — no engine/ subdir), so ENGINE is the repo root.
ROOT=$(cd "$(dirname "$0")/.." && pwd)
ENGINE="$ROOT"

if [ ! -d "$ENGINE/crates/memstead-cli/src" ]; then
  echo "check-no-plan-refs: $ENGINE/crates/memstead-cli/src not found — wrong ENGINE path?" >&2
  exit 2
fi

# Leak patterns (extended regex, portable across GNU and BSD grep —
# no `\b`). Each catches one shape of the decision-trail reference.
PATTERNS=(
  'Plan [0-9]+ of '                 # "Plan 07 of `probe-followups-…`"
  'Subsystem [A-Z]([^A-Za-z]|$)'    # "Subsystem B", "Subsystem C"
  'probe-followups-'                # campaign-folder name
  '`[0-9][0-9]-[a-z0-9-]+\.md'      # backtick plan file, e.g. `05-cli-surface-policy.md`
  'dev/(plans|archive)/'            # plan-tree path
  '(^|[^A-Za-z0-9])F[0-9]+([^A-Za-z0-9]|$)'  # probe finding number, e.g. "F25." or bare "F18"/"(F9)"
)

# Build one alternation for a single grep pass.
ALT=$(IFS='|'; echo "${PATTERNS[*]}")

hits=""

# 1. CLI `///` doc comments — the single memstead-cli crate (both
#    flavours). All `///` are in scope (plan refs are banned everywhere,
#    not just in help text).
cli_doc_lines=$(grep -rnE '^[[:space:]]*///' \
  "$ENGINE/crates/memstead-cli/src" 2>/dev/null || true)
cli_hits=$(printf '%s\n' "$cli_doc_lines" | grep -E "$ALT" || true)
if [ -n "$cli_hits" ]; then
  hits="${hits}
CLI doc comments (rendered into \`memstead <sub> --help\`):
${cli_hits}"
fi

# 2. MCP tool / param `description = "..."` strings.
mcp_desc_lines=$(grep -rnE 'description[[:space:]]*=' \
  "$ENGINE/crates/memstead-mcp/src" 2>/dev/null || true)
mcp_hits=$(printf '%s\n' "$mcp_desc_lines" | grep -E "$ALT" || true)
if [ -n "$mcp_hits" ]; then
  hits="${hits}
MCP tool/param descriptions:
${mcp_hits}"
fi

if [ -n "$hits" ]; then
  echo "Plan/subsystem/finding reference leaked into a user-facing surface."
  echo "Rewrite the docstring/description to state what-is; the decision"
  echo "trail belongs in the commit message and dev/archive/, not in help text."
  echo "$hits"
  exit 1
fi

exit 0
