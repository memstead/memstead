#!/usr/bin/env node
// UserPromptSubmit hook — warns the agent before its next reasoning turn
// when one or more writable vaults advanced since the last prompt in
// this session. Tracks per-session last-seen-HEAD per vault in
// `<workspace_root>/.memstead.cache/drift/last-seen-heads-<session_id>.json`
// and emits a single `<system-reminder>` block listing drifted vaults
// and their changed entity ids.
//
// HEAD enumeration goes through the engine via
// `memstead_health { include_config: true }`; per-vault entity deltas via
// `memstead_changes_since`. The plugin no longer reads `vault-repo/.git/`
// directly. Fails open: any internal error writes a one-line
// `[vault-drift-notify]` diagnostic to stderr and exits 0 with empty
// stdout. Never blocks a prompt.
//
// First run for a session is silent: a fresh agent has nothing stale
// to warn about — the hook just records the current HEADs.

import { existsSync, readdirSync } from 'node:fs';
import { resolve, dirname, join } from 'node:path';
import { sanitizeSessionId, runDriftNotify } from './vault-drift-notify-utils.mjs';
import { hasWorkspaceMarker } from './workspace-resolve-utils.mjs';
import { readStdinJson } from './mcp-client.mjs';

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

  const result = await runDriftNotify({ workspaceRoot, sessionId });
  if (result.status === 'drift') {
    process.stdout.write(result.reminder);
    process.stdout.write('\n');
    return;
  }
  if (result.status === 'probe-failed') {
    process.stderr.write(`[vault-drift-notify] ${result.message}\n`);
    return;
  }
  // 'no-engine' / 'first-run' / 'no-drift' all silent paths.
}

main().catch((err) => {
  process.stderr.write(`[vault-drift-notify] unexpected error: ${err.message}\n`);
  process.exit(0);
});
