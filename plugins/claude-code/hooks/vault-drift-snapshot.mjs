#!/usr/bin/env node
// Stop hook — snapshots current writable-vault HEADs into the same
// per-session state file the `vault-drift-notify` UserPromptSubmit hook
// reads. Runs at turn-end, after any agent mutations have committed, so
// the next prompt's `vault-drift-notify` reads post-own-commits state
// and only flags HEAD advances the agent did not author.
//
// HEAD enumeration goes through the engine via
// `memstead_health { include_config: true }` — the `vaults[].vcs.head` field
// each entry carries gives the cached branch-tip SHA. The plugin no
// longer reads `vault-repo/.git/` directly. Fails open: any internal
// error writes a one-line `[vault-drift-snapshot]` diagnostic to stderr
// and exits 0. Never blocks turn completion.
//
// Idempotent: each invocation overwrites the state file with the
// current HEADs. Multiple Stop fires per turn (rare) are harmless.

import { existsSync, readdirSync } from 'node:fs';
import { resolve, dirname, join } from 'node:path';
import { sanitizeSessionId } from './vault-drift-notify-utils.mjs';
import { hasWorkspaceMarker } from './workspace-resolve-utils.mjs';
import { readStdinJson } from './mcp-client.mjs';
import { runDriftSnapshot } from './vault-drift-snapshot-utils.mjs';

function findWorkspaceRoot(start) {
  let dir = resolve(start);
  while (true) {
    if (hasWorkspaceMarker(dir)) return dir;
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  let probe = resolve(start);
  while (true) {
    if (existsSync(join(probe, '.git'))) {
      try {
        for (const entry of readdirSync(probe, { withFileTypes: true })) {
          if (!entry.isDirectory()) continue;
          const candidate = join(probe, entry.name);
          if (hasWorkspaceMarker(candidate)) return candidate;
        }
      } catch {}
      return null;
    }
    const parent = dirname(probe);
    if (parent === probe) return null;
    probe = parent;
  }
}

async function main() {
  const input = await readStdinJson();
  const sessionId = sanitizeSessionId(input.session_id || input.sessionId || '');
  if (!sessionId) return;

  const cwd = input.cwd || process.cwd();
  const workspaceRoot = findWorkspaceRoot(cwd);
  if (!workspaceRoot) return;

  const result = await runDriftSnapshot({ workspaceRoot, sessionId });
  if (result.status === 'no-engine') {
    // Plugin not configured for this workspace — silent no-op.
    return;
  }
  if (result.status === 'probe-failed') {
    process.stderr.write(`[vault-drift-snapshot] ${result.message}\n`);
    return;
  }
}

main().catch((err) => {
  process.stderr.write(`[vault-drift-snapshot] unexpected error: ${err.message}\n`);
  process.exit(0);
});
