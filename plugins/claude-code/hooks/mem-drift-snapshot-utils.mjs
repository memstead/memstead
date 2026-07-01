// Pure helpers + the snapshot pipeline for the mem-drift-snapshot
// Stop hook. Import-safe: this module exports functions and runs no
// top-level side effects. Tests import directly; the hook entry point
// (`mem-drift-snapshot.mjs`) wires `runDriftSnapshot` to the real
// stdin / MCP-client / fs surface.

import { existsSync, writeFileSync, mkdirSync, readdirSync, statSync, unlinkSync } from 'node:fs';
import { join } from 'node:path';
import { isTrackedMem } from './mem-drift-notify-utils.mjs';
import { resolveEngineCommand, withEngine } from './mcp-client.mjs';

const MCP_TIMEOUT_MS = 15000;
export const FOURTEEN_DAYS_MS = 14 * 24 * 60 * 60 * 1000;

export function ensureCacheDir(workspaceRoot) {
  const cacheRoot = join(workspaceRoot, '.memstead.cache');
  const driftDir = join(cacheRoot, 'drift');
  mkdirSync(driftDir, { recursive: true });
  const gi = join(cacheRoot, '.gitignore');
  if (!existsSync(gi)) writeFileSync(gi, '*\n');
  return driftDir;
}

// Drop state files for sessions that haven't reported a turn-end in
// `maxAgeMs`. Without this sweep `.memstead.cache/drift/` grows by one
// file per past Claude Code session forever. 14 days is well past
// any realistic active-session window — an idle session whose mtime
// fell behind that threshold is decisively abandoned.
export function pruneStaleStateFiles(driftDir, maxAgeMs) {
  let entries;
  try {
    entries = readdirSync(driftDir);
  } catch {
    return;
  }
  const cutoff = Date.now() - maxAgeMs;
  for (const name of entries) {
    if (!name.startsWith('last-seen-heads-') || !name.endsWith('.json')) continue;
    const path = join(driftDir, name);
    try {
      if (statSync(path).mtimeMs < cutoff) unlinkSync(path);
    } catch {
      // ignore — racing instance may have unlinked it, or stat failed
    }
  }
}

/**
 * Build the `{ mem: head_sha }` map of writable-mem HEADs from an
 * `memstead_health { include_config: true }` response. Filters via
 * `isTrackedMem` (drops `main` / `__*`). Mems whose `vcs.head` is
 * absent (fresh, no commits yet) are silently skipped — without a head
 * SHA there is nothing to compare against on the next prompt.
 */
export function extractCurrentHeads(healthResponse) {
  const writable = new Set(
    Array.isArray(healthResponse?.writable_mems)
      ? healthResponse.writable_mems
      : [],
  );
  const mems = Array.isArray(healthResponse?.mems) ? healthResponse.mems : [];
  const out = {};
  for (const entry of mems) {
    if (!entry || typeof entry.name !== 'string') continue;
    if (!writable.has(entry.name)) continue;
    if (!isTrackedMem(entry.name)) continue;
    const head = entry?.vcs?.head;
    if (typeof head !== 'string' || head.length === 0) continue;
    out[entry.name] = head;
  }
  return out;
}

/**
 * Snapshot pipeline invoked by both the Stop hook and integration
 * tests. Returns `{ status, ... }` where `status` is one of:
 *
 *   - 'no-engine'      — `.mcp.json` absent or `memstead` server entry missing
 *   - 'probe-failed'   — `memstead_health` errored or MCP boot failed
 *   - 'snapshot-empty' — health returned but no writable mems to track
 *   - 'snapshotted'    — state file refreshed ({ heads, statePath })
 *
 * Mocking surface mirrors `produceOuterCommit` in `auto-commit-utils.mjs`:
 *   - `engineCommand` lets a test bypass `resolveEngineCommand`
 *   - `withEngineFn` lets a test inject a fake MCP client
 */
export async function runDriftSnapshot({
  workspaceRoot,
  sessionId,
  engineCommand,
  withEngineFn = withEngine,
  pruneMaxAgeMs = FOURTEEN_DAYS_MS,
  timeoutMs = MCP_TIMEOUT_MS,
} = {}) {
  if (!sessionId) return { status: 'no-engine' };

  let cmdSpec = engineCommand;
  if (!cmdSpec) {
    cmdSpec = resolveEngineCommand(workspaceRoot);
    if (!cmdSpec) return { status: 'no-engine' };
  }

  let healthResponse;
  try {
    healthResponse = await withEngineFn(cmdSpec, timeoutMs, async (client) => {
      return client.callTool('memstead_health', { include_config: true });
    });
  } catch (err) {
    return { status: 'probe-failed', message: err?.message ?? String(err) };
  }

  const heads = extractCurrentHeads(healthResponse);

  let driftDir;
  try {
    driftDir = ensureCacheDir(workspaceRoot);
  } catch (err) {
    return {
      status: 'probe-failed',
      message: `failed to prepare cache dir: ${err.message}`,
    };
  }
  const statePath = join(driftDir, `last-seen-heads-${sessionId}.json`);

  try {
    writeFileSync(statePath, JSON.stringify(heads, null, 2) + '\n');
  } catch (err) {
    return {
      status: 'probe-failed',
      message: `failed to write state: ${err.message}`,
    };
  }

  // Drop abandoned-session state files so the cache stays bounded.
  // Runs after our own write so we never prune the file we just refreshed.
  pruneStaleStateFiles(driftDir, pruneMaxAgeMs);

  if (Object.keys(heads).length === 0) {
    return { status: 'snapshot-empty', statePath };
  }
  return { status: 'snapshotted', heads, statePath };
}
