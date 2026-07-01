---
name: old-ingest
user-invocable: false
description: >
  Frozen pre-rebuild ingest skill. See `/memstead:ingest` for the current shape;
  this slash command remains as a fallback during the ingest-skill rebuild.
context: fork
allowed-tools: mcp__memstead__memstead_overview, mcp__memstead__memstead_search, mcp__memstead__memstead_entity, mcp__memstead__memstead_create, mcp__memstead__memstead_update, mcp__memstead__memstead_delete, mcp__memstead__memstead_relate, Read, Glob, Grep, Bash
argument-hint: "[--all | --mode refinement [--all] | projection-name]"
hooks:
  PreCompact:
    - matcher: "auto"
      hooks:
        - type: command
          command: "echo 'CONTEXT LIMIT — stop now and report.' >&2; exit 2"
  PreToolUse:
    - matcher: "Read|Glob|Grep"
      hooks:
        - type: command
          command: "node ${CLAUDE_PLUGIN_ROOT}/hooks/deny-meta-files.mjs"
---

!`node ${CLAUDE_SKILL_DIR}/scripts/inject.mjs $ARGUMENTS`
