// Generic workspace-local recorded-binary-version + capability gate.
//
// Setup records the installed `memstead` binary's version once; any capability
// gating reads it. This is NOT a sync-only side channel — it is a generic
// mechanism (a version-gated capability reads `anchorsGate`, and future gates
// can add their own threshold the same way).
//
// The gate FAILS CLOSED TO DEGRADED: a missing, unparseable, or below-threshold
// record means "proceed without the capability and say so" — never probe by
// sending a capability-bearing call and catching the engine's rejection.
//
// Record lives under the plugin cache (`.memstead.cache/plugin/binary-version.json`,
// gitignored) so it never touches mem-repo state. If the cache is wiped the gate
// degrades safely until setup re-runs. Node built-ins only.

import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import {
  findWorkspaceRoot,
  hasWorkspaceMarker,
  mcpConfigCdTargets,
} from '../hooks/workspace-resolve-utils.mjs';

/** First `memstead` release whose mutation tools accept the `anchors[]` param. */
export const ANCHORS_MIN = { major: 0, minor: 3, patch: 0 };

const RECORD_REL = '.memstead.cache/plugin/binary-version.json';

/**
 * Resolve the workspace root the record belongs to, from any directory in
 * the project. A skill runs `record`/`gate` with `$(pwd)` — which may be the
 * project root while the workspace lives in a subdirectory (the common
 * `cd <dir>` `.mcp.json` layout). Resolution mirrors the path-aware hooks:
 * (1) walk up for the engine's workspace marker; (2) probe `.mcp.json`
 * `cd <dir>` launch targets for a marker-bearing subdirectory; (3) fall back
 * to the given directory unchanged (the pre-resolution behavior).
 */
export function resolveWorkspaceRootFrom(dir) {
  const walked = findWorkspaceRoot(dir);
  if (walked) return walked;
  try {
    const mcpConfig = JSON.parse(readFileSync(join(dir, '.mcp.json'), 'utf-8'));
    for (const target of mcpConfigCdTargets(mcpConfig, dir)) {
      if (hasWorkspaceMarker(target)) return target;
    }
  } catch {
    /* no or malformed .mcp.json — fall through */
  }
  return dir;
}

/** Parse a `memstead --version` line ("memstead 0.2.0") to {major,minor,patch} or null. */
export function parseVersion(text) {
  if (typeof text !== 'string') return null;
  const m = text.match(/(\d+)\.(\d+)\.(\d+)/);
  if (!m) return null;
  return { major: Number(m[1]), minor: Number(m[2]), patch: Number(m[3]) };
}

/** semver-style `a >= min`. */
export function isAtLeast(a, min) {
  if (!a) return false;
  if (a.major !== min.major) return a.major > min.major;
  if (a.minor !== min.minor) return a.minor > min.minor;
  return a.patch >= min.patch;
}

/** Record the installed binary's version under the workspace's plugin cache. */
export function recordBinaryVersion(workspaceRoot, { bin = process.env.MEMSTEAD_BIN || 'memstead', run = spawnSync } = {}) {
  const r = run(bin, ['--version'], { encoding: 'utf-8' });
  if (r.error || r.status !== 0) {
    return { ok: false, reason: `\`${bin} --version\` failed: ${r.error?.message || (r.stderr || '').trim() || `exit ${r.status}`}` };
  }
  const version = parseVersion(r.stdout);
  if (!version) return { ok: false, reason: `could not parse a version from: ${JSON.stringify((r.stdout || '').trim())}` };
  const path = join(workspaceRoot, RECORD_REL);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, JSON.stringify({ version: `${version.major}.${version.minor}.${version.patch}`, raw: r.stdout.trim() }, null, 2) + '\n');
  return { ok: true, version, path };
}

/** Read the recorded version, or null if absent/unreadable/unparseable. */
export function readRecordedVersion(workspaceRoot) {
  try {
    const rec = JSON.parse(readFileSync(join(workspaceRoot, RECORD_REL), 'utf-8'));
    return parseVersion(rec.version);
  } catch {
    return null;
  }
}

/**
 * The anchors capability gate. Returns `{capable, version, reason}`:
 * `capable: true` only when a recorded version is present AND >= ANCHORS_MIN.
 * Any other state (no record, unparseable, older) → `capable: false` with a
 * one-line reason a router can print — never a probe-by-error.
 */
export function anchorsGate(workspaceRoot) {
  const version = readRecordedVersion(workspaceRoot);
  if (!version) {
    return { capable: false, version: null, reason: 'no recorded binary version — run /setup to record it; proceeding without anchors' };
  }
  const v = `${version.major}.${version.minor}.${version.patch}`;
  if (!isAtLeast(version, ANCHORS_MIN)) {
    const min = `${ANCHORS_MIN.major}.${ANCHORS_MIN.minor}.${ANCHORS_MIN.patch}`;
    return { capable: false, version, reason: `recorded binary ${v} predates anchors support (needs ${min}); proceeding without anchors` };
  }
  return { capable: true, version, reason: `recorded binary ${v} supports anchors` };
}

// CLI: `record <dir>` (used by /setup) writes the record; `gate <dir>`
// (used by capability-gated routers) prints the `{capable, version, reason}`
// gate as JSON on stdout and always exits 0 — the caller branches on
// `capable`, never on the exit code. `<dir>` may be any directory in the
// project: both commands resolve the actual workspace root from it (walk-up
// + `.mcp.json` cd-target probe), so `$(pwd)` is safe even when the
// workspace lives in a subdirectory.
function main() {
  const [cmd, dir] = process.argv.slice(2);
  const root = dir ? resolveWorkspaceRootFrom(dir) : null;
  if (cmd === 'record' && root) {
    const r = recordBinaryVersion(root);
    if (r.ok) {
      console.log(`recorded memstead ${r.version.major}.${r.version.minor}.${r.version.patch}`);
      process.exit(0);
    }
    console.error(`binary-version: ${r.reason}`);
    process.exit(1);
  }
  if (cmd === 'gate' && root) {
    console.log(JSON.stringify(anchorsGate(root)));
    process.exit(0);
  }
  console.error('usage: binary-version.mjs (record|gate) <dir-anywhere-in-project>');
  process.exit(2);
}

if (process.argv[1] && (await import('node:url')).fileURLToPath(import.meta.url) === process.argv[1]) main();
