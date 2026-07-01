---
name: outer-commit
user-invocable: false
description: Commit pending entity changes to git — shares the Stop hook's commit pipeline. Use when auto-commit is disabled or a previous Stop failed.
allowed-tools: Bash, Read, mcp__memstead__memstead_health
---

# Memstead Outer-Commit

Manually fires the same commit pipeline the Stop hook uses. Both paths
produce `memstead: session changes (...)` commits with `Memstead-cursor:`
trailers; only the `Session:` trailer differs (present on the hook, absent
on manual invocations).

Works in both auto-commit-enabled and auto-commit-disabled workspaces —
the skill bypasses the `outer_vcs.enabled` gate.

## Steps

1. Resolve the plugin root (normally `$CLAUDE_PLUGIN_ROOT`; falls back
   to `<repo>/plugins/claude-code`).
2. Run the shared pipeline with a small Node invocation that imports
   `produceOuterCommit` and passes `sessionId: null` plus
   `skipEnabledCheck: true`:

   ```sh
   node -e "import('${CLAUDE_PLUGIN_ROOT}/hooks/auto-commit-utils.mjs').then(async (m) => { \
     const r = await m.produceOuterCommit({ \
       workspaceRoot: process.cwd(), \
       sessionId: null, \
       skipEnabledCheck: true, \
       logger: { error: (msg) => process.stderr.write(msg + '\\n') }, \
     }); \
     process.stdout.write(JSON.stringify(r) + '\\n'); \
   })"
   ```

3. Parse the returned `{ status, ... }` JSON and report to the user:

   - `committed` → "Committed `<sha>` across `<N>` mems."
   - `no-changes` → "No pending mem changes to commit."
   - `no-mems` → "No writable mems in this workspace."
   - `probe-failed` → "Engine probe failed: `<message>`."
   - `commit-failed` → "Commit failed: `<stderr>`."

## Rules

- Never amend, never force push, never rewrite history.
- Only the writable-mem worktrees are staged — no code, no config
  outside mems.
- The skill does not build its own commit message — the shared pipeline
  owns subject, body, and trailer shape. This keeps the outer-repo log
  uniform across automatic and manual commits.
