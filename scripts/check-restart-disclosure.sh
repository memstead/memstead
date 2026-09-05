#!/usr/bin/env bash
# Every surface that teaches an install states the restart wall, in the one
# shared phrasing.
#
# WHY: a running agent session does not attach an MCP server added while it
# runs, and (Claude Code) picks up a freshly installed plugin's skills only
# after `/reload-plugins`. Neither is a Memstead defect, and neither was
# stated anywhere the install is taught: the 2026-08-22 sealed newcomer
# installed the plugin, saw six skills confirmed, ran the documented next
# step and got `Unknown skill`. The wall is platform-owned; the silence was
# ours.
#
# WHY A SCANNER, NOT A LIST: this guard shipped as a hand-kept list of
# surfaces, and three consecutive grading rounds each found one more surface
# the list did not know about — twice a published crate readme, the exact
# class the list already covered. A list can only hold what its author
# remembered. So the guard now DISCOVERS the class: every tracked text file
# under this repository that teaches an install or a wiring is required to
# carry the applicable sentence, and anything that legitimately does not is
# an EXEMPTION with a stated reason, reviewed here rather than forgotten
# elsewhere.
#
# Comparison normalises whitespace, backticks, quotes, backslashes,
# line-leading comment markers and HTML tags, so a surface may wrap the
# sentence across lines, quote it, code-span it or wrap it in markup. It
# does NOT see through markdown links, emphasis inside the sentence, HTML
# entities, or an escaped `\n` inside a JSON string: those read as
# different text and fail, which is the safe direction.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# The plugin half: skills reach a running session via reload OR restart.
PLUGIN_DISCLOSURE="A session that is already running picks the new skills up only after /reload-plugins or a restart"
# The MCP half: no reload path exists — restart is the only door.
MCP_DISCLOSURE="Restart the agent session afterwards: a session that is already running does not attach an MCP server added while it runs"

# What "teaches an install" looks like in text. Kept deliberately literal:
# a pattern that fires on prose about the surface (rather than on the
# teaching itself) buys exemptions, not coverage.
# Prose surfaces (docs, manifests, site copy) teach in sentences, so the
# patterns there are wide. Code files (`.mjs`, `.rs`) mention the same
# nouns while merely PARSING config, so there the pattern is only the
# literal command a reader would be told to run.
PLUGIN_PATTERN='plugin install memstead|/plugin install|plugin marketplace add'
MCP_PATTERN='mcp add|mcpServers|MCP wiring|MCP config|quickstart --agent'
PLUGIN_PATTERN_CODE='claude plugin install|plugin marketplace add'
MCP_PATTERN_CODE='claude mcp add'

# Discovery finds what a literal command betrays. Some surfaces teach an
# install with no command in them at all: a catalogue description, a page
# generator, a skill's own prose. Those are held BY NAME, because a pattern
# that matched them would match half the repository. A grade proved the
# need: the plugin browser's description field is what a user reads at the
# moment of install, and it matched nothing.
plugin_required() {
  case "$1" in
    .claude-plugin/marketplace.json) return 0 ;;
    plugins/claude-code/.claude-plugin/plugin.json) return 0 ;;
    docs-site/scripts/prebuild.mjs) return 0 ;;
    *) return 1 ;;
  esac
}
mcp_required() {
  case "$1" in
    plugins/claude-code/skills/setup/README.md) return 0 ;;
    crates/memstead-cli/src/cli.rs) return 0 ;;
    docs-site/scripts/prebuild.mjs) return 0 ;;
    *) return 1 ;;
  esac
}

# Surfaces that match a pattern but do not owe a disclosure. Each line is
# `path # reason`. An exemption is a claim about the file: state why the
# reader is not being taught an install there, or why the wall does not
# apply to what it teaches.
exempt() {
  case "$1" in
    # Neither teaches an install. This guard holds the sentences as
    # constants; the changelog describes the change in its own words (it
    # paraphrases, deliberately, and a grade caught an earlier version of
    # this comment claiming otherwise).
    CHANGELOG.md|scripts/check-restart-disclosure.sh) return 0 ;;
    # Reconnecting a server the session ALREADY knows works, and is what
    # this file teaches after a local rebuild. Telling the reader to restart
    # would be false advice, not a disclosure.
    docs/build.md) return 0 ;;
    # The receipt is the wiring act's own output, not a surface teaching a
    # future install: it gates the restart as the single next action and
    # names the concrete agents it just wired. Plan flywheel
    # 10-first-session-residue/02 requires it to stay as it is.
    crates/memstead-cli/src/commands/quickstart.rs) return 0 ;;
    # Build output, regenerated at prebuild from prebuild.mjs, which
    # this guard holds by name. Untracked, so the sweep never reaches it;
    # the entry stands as the record of why that is safe.
    docs-site/src/content/docs/skills.md) return 0 ;;
    *) return 1 ;;
  esac
}

fail=0
normalise() {
  # Line-leading comment markers are stripped before the join, so a
  # sentence wrapped across `///` doc-comment lines still reads as one
  # sentence. Slashes elsewhere are left alone: `/reload-plugins` is part
  # of the phrasing.
  sed -e 's|^[[:space:]]*///*||' -e 's|<[^>]*>||g' "$1" \
    | tr -d '`'"'"'"\\' | tr '\n' ' ' | tr -s ' '
}

check_one() {
  local file="$1" want="$2" kind="$3"
  if ! normalise "$file" | grep -qF "$want"; then
    echo "  ✗ $file — teaches a $kind install without the disclosure:" >&2
    echo "      $want" >&2
    fail=1
  fi
}

# Reader-facing text only. Code that BUILDS wiring (the eval harness, the
# server, the arg parser) is not a surface that teaches it, and sweeping it
# would buy exemptions rather than coverage. Two `.rs` files are in the net
# anyway: `cli.rs`, whose doc comments render into `--help` and the generated
# CLI reference, is held BY NAME (a grade proved discovery could not see it,
# because its prose quotes no command), and the receipt reaches `exempt()`
# with its reason.
while IFS= read -r file; do
  case "$file" in
    crates/memstead-cli/src/cli.rs) ;;
    crates/memstead-cli/src/commands/quickstart.rs) ;;
    *.md|*.mdx|*.sh|*.json|*.mjs|*.js|*.txt|*.astro|*.ts|*.html|*.toml|*.yaml|*.yml) ;;
    *) continue ;;
  esac
  case "$file" in
    # Fixtures carry deliberately malformed and historical text; the proof
    # folder is a frozen transcript; `engineering/` is the standing-knowledge
    # mem, which DESCRIBES the system rather than instructing a reader (the
    # entities that teach an install live in the flagship mem, another repo).
    fuzz/corpus/*|*/tests/fixtures/*|ci/fixtures/*|docs/proof/*|engineering/*) continue ;;
  esac
  plugin_pat="$PLUGIN_PATTERN" mcp_pat="$MCP_PATTERN"
  case "$file" in
    *.mjs|*.js|*.rs) plugin_pat="$PLUGIN_PATTERN_CODE" mcp_pat="$MCP_PATTERN_CODE" ;;
  esac
  [ -f "$file" ] || continue
  exempt "$file" && continue
  if plugin_required "$file" || grep -qE "$plugin_pat" "$file" 2>/dev/null; then
    check_one "$file" "$PLUGIN_DISCLOSURE" "plugin"
  fi
  if mcp_required "$file" || grep -qE "$mcp_pat" "$file" 2>/dev/null; then
    check_one "$file" "$MCP_DISCLOSURE" "MCP"
  fi
done < <(git ls-files)

if [ "$fail" -ne 0 ]; then
  echo "  ✗ restart-disclosure guard FAILED" >&2
  echo "    Teaching an install without the disclosure is the defect this" >&2
  echo "    guard exists to prevent. Add the sentence above verbatim; if the" >&2
  echo "    file teaches no install, exempt it here with the reason why." >&2
  exit 1
fi

echo "  ✓ restart disclosure present on every install-teaching surface"
