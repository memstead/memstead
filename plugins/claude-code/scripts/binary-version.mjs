// Generic workspace-local recorded-binary-version + capability gate.
//
// Setup records the installed `memstead` binary's version once; any capability
// gating reads it. This is NOT a sync-only side channel — it is a generic
// mechanism (a version-gated capability reads `anchorsGate`, and future gates
// can add their own threshold the same way).
//
// The gate FAILS CLOSED TO DEGRADED: a missing, unparseable, below-threshold
// or UNCONFIRMABLE record means "proceed without the capability and say so" —
// never probe by sending a capability-bearing call and catching the engine's
// rejection.
//
// "Unconfirmable" is the third state, and it is why the record keeps the raw
// banner and not just three numbers. Thresholds below are RELEASE numbers, so
// only a release binary can be placed on that ladder. A binary reporting
// `0.18.0+g1a2b3c` is some build that calls itself 0.18.0 — between releases
// the crate version does not move, so a build from any commit after the 0.17.0
// tag still says 0.17.0, including builds predating the commit that added the
// capability. Reading such a build as "at least 0.17.0, therefore capable" is
// how the plugin talked itself into sending a parameter the engine rejected.
// The engine cooperates: `crates/memstead-base/build.rs` emits the sha for
// every build EXCEPT a clean release build, so bare semver means release and
// `+g<sha>` means "cannot confirm". (A published release binary carries no sha
// from 0.18.0 on; 0.17.0 and earlier did, so on those the gate degrades — the
// safe direction, and it clears on upgrade.)
//
// The record is also REFRESHED on read: `record` runs once at setup, but the
// binary it measured gets upgraded and nothing re-ran setup. The gate compares
// the record against the live `--version` and re-records on a mismatch, so an
// upgrade takes effect without the user knowing the record exists. A failed
// probe leaves the record standing — a binary that cannot be reached says
// nothing about the version that was recorded when it could be.
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
/** First release whose `quickstart` accepts `--repo <PATH>` (the setup skill's
 * point-at-your-repository form). */
export const REPO_MIN = { major: 0, minor: 10, patch: 0 };
/** First release whose `projection brief --all` accepts `--consume` (taking
 * the rotation slot; the sync skill and the ingest router pass it). Cut in
 * 0.9.0, which was never published, so 0.10.0 is the first binary a user
 * can hold that accepts it. */
export const CONSUME_MIN = { major: 0, minor: 10, patch: 0 };

/**
 * Every version-gated capability, by name: the threshold, how the flag or
 * parameter is written, and what "proceeding without it" means. A new gate
 * is a new row; the gate logic below is shared.
 */
export const CAPABILITIES = {
  anchors: { min: ANCHORS_MIN, what: 'anchors', without: 'proceeding without anchors' },
  repo: { min: REPO_MIN, what: '`--repo`', without: 'proceeding with the plain `quickstart` form' },
  consume: { min: CONSUME_MIN, what: '`--consume`', without: 'rendering the brief as a pure read' },
};

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

/**
 * The `+g<sha>` build metadata in a `memstead --version` line, or null when
 * there is none. Its presence is the "not a release build" signal; the value
 * is carried only so the degraded sentence can name the build.
 *
 * Matched on the semver build-metadata suffix that follows the version core,
 * so a sha-looking string elsewhere in the banner cannot be mistaken for one.
 */
export function parseBuildMetadata(text) {
  if (typeof text !== 'string') return null;
  const m = text.match(/\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?\+([0-9A-Za-z.-]+)/);
  return m ? m[1] : null;
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

/**
 * Read the whole record as `{version, build, raw}` — the parsed
 * `{major,minor,patch}`, the `+g<sha>` build metadata (null on a release
 * build), and the raw banner as recorded. Null if absent, unreadable, or
 * carrying no parseable version.
 *
 * The build metadata is read from `raw`, which holds the banner verbatim;
 * the `version` field is the bare three numbers by construction.
 */
export function readRecord(workspaceRoot) {
  try {
    const rec = JSON.parse(readFileSync(join(workspaceRoot, RECORD_REL), 'utf-8'));
    const version = parseVersion(rec.version);
    if (!version) return null;
    const raw = typeof rec.raw === 'string' ? rec.raw : rec.version;
    return { version, build: parseBuildMetadata(raw), raw };
  } catch {
    return null;
  }
}

/**
 * Read the recorded version, or null if absent/unreadable/unparseable.
 *
 * Presence-only callers (the realization hook asks "is there a binary to
 * query at all") want exactly this and must not pay for a subprocess, so
 * this reader never probes and never re-records.
 */
export function readRecordedVersion(workspaceRoot) {
  return readRecord(workspaceRoot)?.version ?? null;
}

/**
 * Bring the record in step with the binary actually on PATH, returning the
 * record to gate on (or null when there is none).
 *
 * `record` runs once, at setup. Upgrading the binary afterwards re-runs
 * nothing, so the record silently describes a version that is no longer
 * installed — observed holding 0.14.0 against a 0.17.0 on PATH, which
 * understates the binary on every gate and would overstate it after a
 * downgrade. A `--version` call is cheap next to the work every gated caller
 * is about to do, so the gate pays it and rewrites the record when the banner
 * disagrees.
 *
 * Best-effort in the one direction that is safe: a probe that fails to run or
 * fails to parse leaves the existing record untouched and in force. An
 * unreachable binary is not evidence about the recorded version, and dropping
 * a good record over a transient spawn failure would degrade a capable setup.
 */
function refreshedRecord(workspaceRoot, { bin = process.env.MEMSTEAD_BIN || 'memstead', run = spawnSync } = {}) {
  const existing = readRecord(workspaceRoot);
  let live;
  try {
    live = run(bin, ['--version'], { encoding: 'utf-8' });
  } catch {
    return existing;
  }
  if (!live || live.error || live.status !== 0) return existing;
  const raw = (live.stdout || '').trim();
  if (!parseVersion(raw)) return existing;
  if (existing && existing.raw.trim() === raw) return existing;
  const written = recordBinaryVersion(workspaceRoot, { bin, run: () => live });
  return written.ok ? readRecord(workspaceRoot) : existing;
}

/**
 * One capability gate by name (`anchors` | `repo` | `consume`). Returns
 * `{capable, version, reason}`: `capable: true` only when the binary's
 * version is present, >= the capability's threshold, AND identifies a
 * release. Any other state (no record, unparseable, older, or a build that
 * is not a release) → `capable: false` with a one-line reason that names
 * what was found and what was needed, which the caller prints — never a
 * probe-by-error. An unknown capability name is a programming error and
 * throws.
 *
 * The record is refreshed against the live binary first; pass `run` to
 * control that probe (tests, and any caller that has already measured).
 *
 * Order of the two failing states is deliberate. Below-threshold is checked
 * BEFORE not-a-release, because it is the sounder statement of the two: a
 * build calling itself 0.2.0 is necessarily older than the 0.3.0 that first
 * carried the capability, dev build or not, and "predates support" tells the
 * user more than "cannot confirm". Only at-or-above the threshold does the
 * release question decide anything.
 */
export function capabilityGate(workspaceRoot, name, opts = {}) {
  const cap = CAPABILITIES[name];
  if (!cap) throw new Error(`unknown capability '${name}' (known: ${Object.keys(CAPABILITIES).join(', ')})`);
  const record = refreshedRecord(workspaceRoot, opts);
  const min = `${cap.min.major}.${cap.min.minor}.${cap.min.patch}`;
  if (!record) {
    return { capable: false, version: null, reason: `no recorded binary version — run /setup to record it; ${cap.without}` };
  }
  const { version, build } = record;
  const v = `${version.major}.${version.minor}.${version.patch}`;
  if (!isAtLeast(version, cap.min)) {
    return { capable: false, version, reason: `recorded binary ${v} predates ${cap.what} support (needs ${min}); ${cap.without}` };
  }
  if (build) {
    return { capable: false, version, build, reason: `cannot confirm ${cap.what} support — binary ${v} is a dev build at ${build}, not release ${v} (needs ${min}); ${cap.without}` };
  }
  return { capable: true, version, reason: `recorded binary ${v} supports ${cap.what}` };
}

/** The anchors capability gate: `capabilityGate(root, 'anchors')`. */
export function anchorsGate(workspaceRoot, opts = {}) {
  return capabilityGate(workspaceRoot, 'anchors', opts);
}

// CLI: `record <dir>` (used by /setup) writes the record; `gate <dir>
// [capability]` (used by capability-gated routers and skills) prints the
// `{capable, version, reason}` gate as JSON on stdout and always exits 0 —
// the caller branches on `capable`, never on the exit code; the capability
// defaults to `anchors`. `root <dir>` prints the resolved
// workspace root path — skills use it to pass `--workspace` explicitly on
// every `memstead` CLI call instead of inheriting cwd (the CLI's own upward
// walk cannot find a workspace that lives *below* the session cwd, the
// common `cd <dir>` `.mcp.json` layout). `<dir>` may be any directory in the
// project: all commands resolve the actual workspace root from it (walk-up
// + `.mcp.json` cd-target probe), so `$(pwd)` is safe even when the
// workspace lives in a subdirectory.
function main() {
  const [cmd, dir, capability] = process.argv.slice(2);
  const root = dir ? resolveWorkspaceRootFrom(dir) : null;
  if (cmd === 'root' && root) {
    console.log(root);
    process.exit(0);
  }
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
    const name = capability || 'anchors';
    if (!CAPABILITIES[name]) {
      console.error(`binary-version: unknown capability '${name}' (known: ${Object.keys(CAPABILITIES).join(', ')})`);
      process.exit(2);
    }
    console.log(JSON.stringify(capabilityGate(root, name)));
    process.exit(0);
  }
  console.error('usage: binary-version.mjs (record|gate [anchors|repo|consume]|root) <dir-anywhere-in-project>');
  process.exit(2);
}

if (process.argv[1] && (await import('node:url')).fileURLToPath(import.meta.url) === process.argv[1]) main();
