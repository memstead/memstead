---
name: ingest-scout
description: >
  Review source artifacts against destination artifacts. Find what's missing or wrong.
  Never writes — produces findings for the writer.
context: fork
allowed-tools: mcp__memstead__memstead_overview, mcp__memstead__memstead_search, mcp__memstead__memstead_entity, Read, Glob, Grep, Bash
argument-hint: "[--all | projection-name]"
hooks:
  PreCompact:
    - matcher: "auto"
      hooks:
        - type: command
          command: "echo 'CONTEXT LIMIT — stop now and report findings.' >&2; exit 2"
---

!`node ${CLAUDE_SKILL_DIR}/scripts/inject.mjs --mode refinement $ARGUMENTS`
