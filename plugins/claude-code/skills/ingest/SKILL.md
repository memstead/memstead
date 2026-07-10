---
name: ingest
description: >
  Build a mem from a declared source binding — one focused batch per run,
  resumable, suited to running on a loop. No binding yet? A short three-question
  setup creates one; no configuration vocabulary needed. Build only: it grows
  the mem — keeping it current as the source changes is a separate job.
context: fork
allowed-tools: mcp__memstead__memstead_overview, mcp__memstead__memstead_schema, mcp__memstead__memstead_search, mcp__memstead__memstead_entity, mcp__memstead__memstead_create, mcp__memstead__memstead_update, mcp__memstead__memstead_delete, mcp__memstead__memstead_relate, Read, Glob, Grep, Bash, WebSearch, WebFetch
argument-hint: "[--all | --clear <binding> | <binding>]"
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
