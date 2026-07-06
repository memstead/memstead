#!/bin/bash
#
# Guard: no implementation-mechanism leak into an agent-facing MCP
# description.
#
# An agent-facing surface describes what the caller GETS and can ACT ON,
# not how the engine achieves it internally. A description that names an
# internal storage path the agent is forbidden to touch, an internal
# method/type, a data-structure-swap, or a library identity yields the
# agent no action — at best it is noise, at worst it tempts the agent
# toward a forbidden "I'll just edit that file" path. (Sibling to
# check-no-plan-refs.sh, which guards a different leak class — the
# decision trail. Same root discipline: agent-facing strings describe
# what-is from the consumer's perspective.)
#
# The actionability test that draws the line (applied per-string when
# authoring): keep a term iff the agent can act on it — a field it
# polls, a code it branches on, a recovery step it takes, a model it
# must understand to use the tool. Drop it if it names an internal
# artifact the agent cannot touch, or if it narrates mechanism around an
# outcome the agent already gets from the response.
#
# Scope (cleanly machine-scannable agent-facing strings):
#   * MCP tool / param `description = "..."` strings on `#[tool]` /
#     `#[schemars]` attributes, and the server `instructions = "..."`
#     blob. Each is one physical line, so a line-oriented grep is exact.
#   * CLI `///` doc comments that clap RENDERS into `memstead mem |
#     install | export | workspace | batch-update --help`. These are the
#     docs on enum-variant / struct-field declarations (indented, not a
#     `fn`); the awk pre-filter below isolates them. Internal helper-`fn`
#     docs and top-level `const` / `struct` docs are deliberately NOT
#     scanned — CLAUDE.md says internal docs SHOULD describe internals
#     (naming `__MEMSTEAD`, `.memstead/workspace.toml`, `semver::` is accurate
#     there), so a blunt all-`///` grep would false-positive on them.
#     The plan-ref guard, by contrast, scans all `///` because plan refs
#     are banned everywhere.
#
# NOT machine-scanned (curated review surface, per the plan that added
# this guard): engine-emitted message strings (`#[error(...)]`,
# `format!`) and the rendered enum-variant `///` docs that flow into the
# parameter schema. Marker terms legitimately appear in those contexts —
# operator-level archive / mem-repo-corruption diagnostics that name
# `.memstead/config.json` or `__MEMSTEAD` for debugging, action-oriented init
# hints, and internal `///` doc comments (which CLAUDE.md says SHOULD
# describe internals). A blunt grep there false-positives. The audit
# that introduced this guard scrubbed the agent-facing message leaks it
# found (cross-link-policy delete refusals that pointed the agent at
# `.memstead/workspace.toml` instead of `memstead_workspace_revoke_cross_link`);
# new message strings stay a review item.
#
# Known-actionable survivors the marker set deliberately does NOT flag:
#   * `memstead_health include_config=true` — the sanctioned path to a
#     gitdir. (`.memstead/config.json` is a marker; `include_config=true` is
#     not — they do not collide.)
#   * the alias-synthesis edge model in `instructions` — the agent's
#     contract, names no banned term.
#   * `refs/heads/<...>` ref-format examples in `memstead_diff` /
#     `memstead_changes_since` — the agent PASSES these refs as tool input,
#     so the ref form is contract, not leak. `refs/heads/` is therefore
#     deliberately absent from the marker set.
#   * the bare word `gitdir` and `semver` (without `::`) — concepts the
#     agent reasons about; only the library path `semver::` is banned.
#
# Exit 0 when clean, 1 (with the offending lines) when a leak is found.

set -u

# This guard lives in the public engine repo (public/scripts/) and scans
# the engine crates directly. The repo is root-hoisted (Cargo.toml +
# crates/ at the repo root — no engine/ subdir), so ENGINE is the repo root.
ROOT=$(cd "$(dirname "$0")/.." && pwd)
ENGINE="$ROOT"

if [ ! -d "$ENGINE/crates/memstead-mcp/src" ]; then
  echo "check-no-mechanism-leak: $ENGINE/crates/memstead-mcp/src not found — wrong ENGINE path?" >&2
  exit 2
fi

# Mechanism-marker vocabulary (extended regex, portable across GNU and
# BSD grep — no `\b`). Each catches one shape of the leak.
PATTERNS=(
  '__MEMSTEAD'                 # internal registry/config ref name
  '\.memstead/config\.json'    # internal per-mem config storage path
  '\.memstead/workspace\.toml' # internal workspace-policy storage path
  '\.memstead/config\.json'    # legacy spelling of the same path
  '\.memstead/workspace\.toml' # legacy spelling of the same path
  'write_mem_config'     # internal backend method name
  'COW snapshot'           # internal data-structure-swap mechanics
  'ref-edit transaction'   # internal git-ref mutation mechanics
  'semver::'               # library path (the behaviour is the contract)
  'tantivy'                # search-index library identity
  'BM25'                   # ranker identity
  'bm25'                   # ranker identity (lowercase)
  'gix'                    # git library identity
)

ALT=$(IFS='|'; echo "${PATTERNS[*]}")

# Agent-facing strings: `description = "..."` and `instructions = "..."`
# on the MCP tool/param attributes in the single memstead-mcp crate
# (both flavours via the `mem-repo` feature).
desc_lines=$(grep -rnE '(description|instructions)[[:space:]]*=' \
  "$ENGINE/crates/memstead-mcp/src" 2>/dev/null || true)

# CLI clap-rendered `///` docs: a doc block "renders" when the
# declaration it precedes is an indented enum-variant / struct-field
# (not a `fn`). The awk filter buffers each doc block with its
# `file:line:` prefix and emits it only for a clap-rendered item, so
# internal helper-`fn` and top-level `const` docs (which legitimately
# name internals) are excluded. Output shape matches `grep -rn` so the
# downstream marker grep and error report stay uniform.
cli_clap_doc_lines=$(awk '
  function flush(keep,   i) { if (keep) for (i = 1; i <= n; i++) print buf[i]; n = 0 }
  FNR == 1 { flush(0); in_clap_type = 0; pending_clap = 0 }
  /^[[:space:]]*\/\/\// { buf[++n] = FILENAME ":" FNR ":" $0; next }
  # A derive attribute decides whether the type it precedes is one clap
  # renders (Parser / Args / Subcommand) — only those fields/variants
  # reach --help. Internal enums/structs (e.g. a dispatch flavour enum)
  # legitimately name internals and must not be scanned.
  /^[[:space:]]*#\[derive\(/ { pending_clap = ($0 ~ /(Parser|Args|Subcommand)/) ? 1 : 0; next }
  /^[[:space:]]*#\[/ { next }
  /^[[:space:]]*#!\[/ { next }
  /^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?(struct|enum)[[:space:]]/ {
    in_clap_type = pending_clap; pending_clap = 0; flush(0); next
  }
  {
    if (n > 0) {
      keep = (in_clap_type && $0 ~ /^[[:space:]]+/ && $0 !~ /(^|[[:space:]])(pub |pub\(crate\) |async )*fn /)
      flush(keep)
    }
  }
  END { flush(0) }
' $(find "$ENGINE/crates/memstead-cli/src" -name '*.rs' 2>/dev/null) 2>/dev/null || true)

hits=$(printf '%s\n%s\n' "$desc_lines" "$cli_clap_doc_lines" | grep -E "$ALT" || true)

if [ -n "$hits" ]; then
  echo "Implementation-mechanism leak in an agent-facing MCP description."
  echo "Reword to state what the caller gets and can act on — the response"
  echo "fields it polls, the error codes it branches on, the recovery steps —"
  echo "and drop the internal storage path / method / swap / library identity."
  echo "$hits"
  exit 1
fi

exit 0
