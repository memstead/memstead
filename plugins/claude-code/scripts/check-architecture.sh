#!/usr/bin/env bash
# Locks the no-direct-git rule for the Claude Code plugin.
#
# Plugin code MUST NOT call `git` against `vault-repo/.git/` or read
# `vault-repo/{schemas,configs}/` directly. All vault-repo reads go
# through `memstead-cli` (subprocess) or `memstead-mcp` (MCP); writes go through
# MCP. The single allowed exception is outer-repo operations on the
# user's project repo (the cwd containing the workspace) — those land in
# `auto-commit-utils.mjs` as `git add` / `git commit` / `git log` with
# `cwd: workspaceRoot` and never carry `--git-dir` / `vault-repo` markers.
#
# This check fails if any of those guardrails are broken. Patterns:
#   - `'--git-dir'` / `"--git-dir"` (quoted argv tokens) anywhere in
#     non-test plugin source — vault-repo gitdir access marker.
#   - `'vault-repo/.git` / `"vault-repo/.git` (quoted path string) —
#     direct gitdir filesystem access.
#   - `'vault-repo/schemas` / `'vault-repo/configs` (quoted path
#     strings) — direct working-tree filesystem access.
#
# Carve-outs (excluded from the scan):
#   - `*.test.js` and `*.integration.test.js` — test fixtures build
#     their own ephemeral vault-repo via `git init` and use git to
#     synthesise MCP responses for hook unit-tests. Test infrastructure
#     is not plugin code under the rule.
#   - `plugins/claude-code/skills/old-ingest/` — frozen pre-rebuild
#     ingest surface. Slated for removal; the carve-out drops in the
#     same commit that removes the directory.
#
# The rule: the plugin must reach vault-repo via memstead-cli or
# memstead-mcp, never via direct git. These patterns enforce it.
#
# Run locally: `plugins/claude-code/scripts/check-architecture.sh`. It
# is also invoked from the workspace `run-tests.sh`. Return code 0 =
# clean, 1 = violation.

set -u

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
plugin_root="$(cd "$script_dir/.." && pwd)"

if [[ ! -d "$plugin_root" ]]; then
    echo "check-architecture: could not locate plugin root from $script_dir" >&2
    exit 2
fi

fail=0
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

# Forbidden patterns. The `-l` form is unsuitable here — we want hits to
# show up so the operator can see what to fix. `-E` enables alternation;
# the alternations are wrapped to keep the grep arguments readable.
forbidden_patterns=(
    # Quoted `--git-dir` argv token, in any string-literal form.
    "['\"]--git-dir['\"]"
    # Quoted `vault-repo/.git` path string (matches `.git/` and `.git`).
    "['\"]vault-repo/\\.git"
    # Quoted `vault-repo/schemas` or `vault-repo/configs` path strings.
    "['\"]vault-repo/schemas"
    "['\"]vault-repo/configs"
)

for pattern in "${forbidden_patterns[@]}"; do
    # The find expression scopes the scan to plugin source files only,
    # excluding test files and the frozen old-ingest tree. Using `find`
    # rather than `grep --exclude` keeps the carve-outs explicit and
    # easy to audit.
    if find "$plugin_root" \
        -type f \
        \( -name "*.mjs" -o -name "*.js" -o -name "*.json" \) \
        ! -name "*.test.js" \
        ! -name "*.integration.test.js" \
        ! -path "*/skills/old-ingest/*" \
        -print0 \
        | xargs -0 grep -nE "$pattern" 2>/dev/null > "$tmp"
    then
        if [[ -s "$tmp" ]]; then
            echo "check-architecture: forbidden pattern '$pattern' found in plugin source." >&2
            echo "  (the plugin must reach vault-repo via memstead-cli or memstead-mcp; see" >&2
            echo "   route writes through MCP and reads through the CLI.)" >&2
            sed 's/^/    /' "$tmp" >&2
            fail=1
        fi
    fi
    : > "$tmp"
done

if [[ $fail -eq 0 ]]; then
    echo "check-architecture: OK — no direct vault-repo git/filesystem access in plugin source."
fi

exit $fail
