#!/usr/bin/env node
// Stop hook — invokes the shared commit pipeline (`produceOuterCommit`)
// with the current Claude Code session id. Silent no-op when the plugin
// isn't configured, `outer_vcs.enabled = false`, or no mem saw any
// mutation this turn. The cursor-in-trailer mechanism reads the prior
// cursor from the outer-repo log, so no per-session state file is
// required.

import { readStdinJson, resolveEngineCommand } from './mcp-client.mjs';
import { produceOuterCommit } from './auto-commit-utils.mjs';

async function main() {
  const input = await readStdinJson();
  const sessionId = input.session_id || input.sessionId || null;
  const workspaceRoot = input.cwd || process.cwd();

  const engineCommand = resolveEngineCommand(workspaceRoot);
  if (!engineCommand) return; // plugin not configured for this workspace

  const result = await produceOuterCommit({
    engineCommand,
    workspaceRoot,
    sessionId,
    skipEnabledCheck: false,
    logger: { error: (msg) => process.stderr.write(`${msg}\n`) },
  });

  switch (result.status) {
    case 'committed':
    case 'no-changes':
    case 'no-mems':
    case 'disabled':
      return;
    case 'probe-failed':
      process.stderr.write(`auto-commit: memstead probe failed: ${result.message}\n`);
      return;
    case 'commit-failed':
      process.stderr.write(`auto-commit: git commit failed: ${result.stderr}\n`);
      return;
    default:
      process.stderr.write(`auto-commit: unexpected status '${result.status}'\n`);
  }
}

main().catch((err) => {
  process.stderr.write(`auto-commit: unexpected error: ${err.message}\n`);
  process.exit(0);
});
