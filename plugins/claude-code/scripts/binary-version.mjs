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

/** First `memstead` release whose mutation tools accept the `anchors[]` param. */
export const ANCHORS_MIN = { major: 0, minor: 2, patch: 0 };

const RECORD_REL = '.memstead.cache/plugin/binary-version.json';

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

// CLI: `node binary-version.mjs record <workspace-root>` — used by /setup.
function main() {
  const [cmd, root] = process.argv.slice(2);
  if (cmd === 'record' && root) {
    const r = recordBinaryVersion(root);
    if (r.ok) {
      console.log(`recorded memstead ${r.version.major}.${r.version.minor}.${r.version.patch}`);
      process.exit(0);
    }
    console.error(`binary-version: ${r.reason}`);
    process.exit(1);
  }
  console.error('usage: binary-version.mjs record <workspace-root>');
  process.exit(2);
}

if (process.argv[1] && (await import('node:url')).fileURLToPath(import.meta.url) === process.argv[1]) main();
