// Pure helpers + the drift-detection pipeline for vault-drift-notify.mjs.
// Pure helpers are testable without process.exit, stdin, filesystem, or
// git invocations. `runDriftNotify` is the side-effecting pipeline both
// the hook entry point and the integration tests invoke; it takes an
// injectable `withEngineFn` so tests can mock the MCP layer.
//
// The hook no longer reads `vault-repo/.git/` — HEAD enumeration runs
// through `memstead_health { include_config: true }` and per-vault entity
// deltas through `memstead_changes_since`.

import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { resolveEngineCommand, withEngine } from './mcp-client.mjs';
import {
  ensureCacheDir,
  extractCurrentHeads,
} from './vault-drift-snapshot-utils.mjs';

const MCP_TIMEOUT_MS = 15000;

/**
 * Parse `git for-each-ref --format='%(refname) %(objectname)' refs/heads/`
 * stdout into an array of `{ name, sha }` entries. `name` has the
 * `refs/heads/` prefix stripped so it equals the branch name (which may
 * be hierarchical, e.g. `memstead/engine`).
 *
 * Tolerant: blank lines and lines without two whitespace-separated
 * fields are skipped silently. The hook's job is to fail open, not to
 * fault on git output drift.
 */
export function parseRefList(stdout) {
  if (!stdout) return [];
  const out = [];
  for (const line of stdout.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const [refname, sha] = trimmed.split(/\s+/);
    if (!refname || !sha) continue;
    if (!refname.startsWith('refs/heads/')) continue;
    const name = refname.slice('refs/heads/'.length);
    out.push({ name, sha });
  }
  return out;
}

/**
 * True if the branch name corresponds to a writable vault that should be
 * tracked for drift. Excludes `main` (operator-facing docs) and any
 * registry-class ref whose name starts with `__` (e.g. `__MEMSTEAD`).
 */
export function isTrackedVault(name) {
  if (!name) return false;
  if (name === 'main') return false;
  if (name.startsWith('__')) return false;
  return true;
}

/**
 * Convert a vault-relative file-path list (output of `git diff-tree -r
 * --name-only $old..$new`) into entity ids of the form
 * `<vault>--<slug>`. Drops paths that are not `.md` files. `vault` is
 * the full hierarchical branch name; nested paths inside the vault keep
 * their `/` separators after the `--` (mirrors `file_path_to_id` in the
 * engine: `path/to/leaf.md` → `<vault>--path/to/leaf`).
 */
export function diffPathsToEntityIds(vault, paths) {
  if (!paths || !paths.length) return [];
  const ids = [];
  const seen = new Set();
  for (const raw of paths) {
    if (typeof raw !== 'string') continue;
    const p = raw.trim();
    if (!p.endsWith('.md')) continue;
    const slug = p.slice(0, -'.md'.length);
    if (!slug) continue;
    const id = `${vault}--${slug}`;
    if (seen.has(id)) continue;
    seen.add(id);
    ids.push(id);
  }
  ids.sort();
  return ids;
}

/**
 * Parse the on-disk state file. Returns a `{ vault: sha }` map, or
 * `null` if the input is missing/corrupt/unrecognised. Callers treat
 * `null` like a first run.
 */
export function parseState(raw) {
  if (raw === null || raw === undefined) return null;
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return null;
  const out = {};
  for (const [key, val] of Object.entries(parsed)) {
    if (typeof key !== 'string' || !key) continue;
    if (typeof val !== 'string' || !val) continue;
    out[key] = val;
  }
  return out;
}

/**
 * Diff `prior` against `currentMap` and return the per-vault drift
 * entries. A vault present in `currentMap` but absent from `prior` is
 * **not** drift — it's a first observation for that vault (record
 * silently). A vault in `prior` but absent from `currentMap` (deleted
 * branch) is dropped silently from the next-write state by the caller;
 * this function does not return an entry for it. Same SHA on both
 * sides is not drift either.
 */
export function computeDrift(prior, currentMap) {
  if (!prior) return [];
  const out = [];
  for (const [vault, newSha] of Object.entries(currentMap)) {
    const oldSha = prior[vault];
    if (!oldSha) continue;
    if (oldSha === newSha) continue;
    out.push({ vault, oldSha, newSha });
  }
  out.sort((a, b) => a.vault.localeCompare(b.vault));
  return out;
}

/**
 * Build the system-reminder block for a non-empty drift list. Mirrors
 * the `<system-reminder>...</system-reminder>` envelope the agent
 * already recognises from IDE-selection and file-open notifications.
 * Each entry carries the vault name, old/new SHAs (short), and the
 * sorted entity-id list. Empty drift list returns an empty string —
 * the hook should not emit any output in that case.
 */
export function formatReminder(driftEntries) {
  if (!driftEntries || !driftEntries.length) return '';
  const lines = [];
  lines.push('<system-reminder>');
  lines.push('Vault drift detected since the last user prompt. The following');
  lines.push("vaults advanced under this session's feet — likely from a sibling");
  lines.push('engine instance (parallel terminal, forked subagent, macOS app).');
  lines.push('Re-read affected entities via `memstead_entity` before answering any');
  lines.push('question whose answer depends on their prior content. Cached');
  lines.push('`expected_hash` values for these entities are likely invalid; a');
  lines.push('follow-up `memstead_update` will trip `HASH_MISMATCH` if so.');
  lines.push('');
  for (const { vault, oldSha, newSha, entityIds } of driftEntries) {
    const oldShort = (oldSha || '').slice(0, 12);
    const newShort = (newSha || '').slice(0, 12);
    lines.push(`Vault \`${vault}\` (${oldShort} → ${newShort}):`);
    if (!entityIds || !entityIds.length) {
      lines.push('  (no entity-level diff available)');
    } else {
      for (const id of entityIds) {
        lines.push(`  - ${id}`);
      }
    }
    lines.push('');
  }
  // Drop trailing blank line before closing tag.
  if (lines[lines.length - 1] === '') lines.pop();
  lines.push('</system-reminder>');
  return lines.join('\n');
}

/**
 * Sanitize a session id into a string safe to embed in a filename.
 * Claude Code already provides `session_id` as a UUID-shaped value
 * (alnum + dashes), but we defensively strip anything outside
 * `[A-Za-z0-9._-]` and clamp the length so a hostile id can't escape
 * the cache directory. Empty input → empty string (the hook treats
 * that as "no session id, silent no-op").
 */
export function sanitizeSessionId(s) {
  if (!s || typeof s !== 'string') return '';
  const cleaned = s.replace(/[^A-Za-z0-9._-]/g, '');
  return cleaned.slice(0, 128);
}

/**
 * Build the next-write state map from the current refs list. Already
 * filtered to tracked vaults by the caller; this just collapses to
 * `{ name: sha }`. Vaults present in `prior` but absent here are
 * implicitly dropped (deleted-branch handling).
 */
export function nextStateMap(trackedRefs) {
  const out = {};
  for (const { name, sha } of trackedRefs) {
    if (!name || !sha) continue;
    out[name] = sha;
  }
  return out;
}

/**
 * Flatten a `memstead_changes_since` response into the unique sorted entity
 * ids that touched the per-vault diff. `Added` / `Updated` / `Removed`
 * carry a single `id`; `Renamed` carries `from_id` + `to_id` (both end
 * up in the set so the agent re-reads either side that may live in its
 * context). Unknown shapes silently contribute nothing — drift output
 * degrades to "no entity-level diff available" rather than crashing.
 */
export function entityIdsFromChangesReport(report) {
  if (!report || !Array.isArray(report.changes)) return [];
  const ids = new Set();
  for (const ev of report.changes) {
    if (typeof ev?.id === 'string') ids.add(ev.id);
    if (typeof ev?.from_id === 'string') ids.add(ev.from_id);
    if (typeof ev?.to_id === 'string') ids.add(ev.to_id);
  }
  return [...ids].sort();
}

/**
 * Drift-detection pipeline invoked by both the UserPromptSubmit hook
 * and integration tests. Returns `{ status, ... }` where `status` is
 * one of:
 *
 *   - 'no-engine'     — `.mcp.json` absent or `memstead` server entry missing
 *   - 'probe-failed'  — `memstead_health` errored or MCP boot failed
 *   - 'first-run'     — no prior state for this session, recorded silently
 *   - 'no-drift'      — prior state matched current HEADs
 *   - 'drift'         — drift detected ({ drifted, reminder, statePath })
 *
 * Mocking surface mirrors `runDriftSnapshot` and `produceOuterCommit`:
 *   - `engineCommand` lets a test bypass `resolveEngineCommand`
 *   - `withEngineFn` lets a test inject a fake MCP client. The mock
 *     must answer two tools: `memstead_health` (writable_vaults / vaults
 *     with vcs.head) and `memstead_changes_since` (per-vault delta).
 */
export async function runDriftNotify({
  workspaceRoot,
  sessionId,
  engineCommand,
  withEngineFn = withEngine,
  timeoutMs = MCP_TIMEOUT_MS,
} = {}) {
  if (!sessionId) return { status: 'no-engine' };

  let cmdSpec = engineCommand;
  if (!cmdSpec) {
    cmdSpec = resolveEngineCommand(workspaceRoot);
    if (!cmdSpec) return { status: 'no-engine' };
  }

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

  // Read prior state (treat missing / unparseable as first run).
  let prior = null;
  if (existsSync(statePath)) {
    try {
      prior = parseState(readFileSync(statePath, 'utf-8'));
    } catch {
      prior = null;
    }
  }

  // One MCP session — `memstead_health` to enumerate writable HEADs, then
  // a per-drifted-vault `memstead_changes_since` to get entity ids.
  let envelope;
  try {
    envelope = await withEngineFn(cmdSpec, timeoutMs, async (client) => {
      const health = await client.callTool('memstead_health', { include_config: true });
      const currentMap = extractCurrentHeads(health);
      if (prior === null) {
        return { kind: 'first-run', currentMap };
      }
      const drifted = computeDrift(prior, currentMap);
      for (const entry of drifted) {
        try {
          const report = await client.callTool('memstead_changes_since', {
            vault: entry.vault,
            since: entry.oldSha,
          });
          entry.entityIds = entityIdsFromChangesReport(report);
        } catch {
          // Per-vault diff failure: degrade to "no entity-level diff
          // available" — formatReminder already handles an empty array.
          entry.entityIds = [];
        }
      }
      return { kind: 'drift', currentMap, drifted };
    });
  } catch (err) {
    return { status: 'probe-failed', message: err?.message ?? String(err) };
  }

  // Persist the next-state map — drops vaults absent from `currentMap`
  // (deleted-branch handling) and adds newly-observed ones.
  try {
    writeFileSync(statePath, JSON.stringify(envelope.currentMap, null, 2) + '\n');
  } catch (err) {
    return {
      status: 'probe-failed',
      message: `failed to write state: ${err.message}`,
    };
  }

  if (envelope.kind === 'first-run') {
    return { status: 'first-run', statePath };
  }
  if (envelope.drifted.length === 0) {
    return { status: 'no-drift', statePath };
  }
  const reminder = formatReminder(envelope.drifted);
  return {
    status: 'drift',
    drifted: envelope.drifted,
    reminder,
    statePath,
  };
}
