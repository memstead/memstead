---
name: rollback
user-invocable: false
description: Restore the Memstead graph to the state of an earlier git commit by checking out entities and creating a forward commit.
allowed-tools: Bash(git *), Read, mcp__memstead__*
argument-hint: "[commit-ref]"
---

# Memstead Rollback

Restore the graph to the state of an earlier git commit.

## Steps

1. **Resolve paths**: Read `.mcp.json` and extract from `mcpServers.memstead.args`:
   - `memstead-home` (path after `--memstead-home`, if present — otherwise Memstead is local to the project)
   - `vault` (path after `--vault`, default `./specs`)
2. **Show commits**: `git log --oneline -10 -- specs/` — show last 10 entity commits
3. **User selects**: Ask which commit to restore (unless specified in $ARGUMENTS)
4. **Restore entities**: `git checkout <commit> -- specs/`
5. **Rebuild store**: The MCP server must restart to rebuild the in-memory store from the restored markdown files. Inform the user that a Claude Code restart is needed.
6. **Verify**: Call `memstead_health` and show the counts (only possible after restart)
7. **Clean up git**: Record the restored entities as a new commit:
   ```bash
   git add specs/
   git commit -m "memstead: rollback to <commit-ref>"
   ```

## Rules

- ALWAYS show the commit list and get confirmation first
- NEVER use `git reset --hard`
- The rollback creates a new forward commit (no history rewrite)
- After rollback: show `memstead_health` so the user can see the state

$ARGUMENTS
