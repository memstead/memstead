/**
 * inject-graph-cursor.test.js — integration tests for the `graph` source
 * change-detection strategy. A graph source is another vault; its change
 * signal is the source vault's `snapshot_token` (on the dump) plus
 * `memstead changes --vault <src> --since <baseline>` (stubbed by the
 * fake `memstead`). The changed slice is entity ids, not file paths.
 */

import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const INJECT = fileURLToPath(new URL('./inject.mjs', import.meta.url));
const FAKE_MEMSTEAD = fileURLToPath(new URL('./test-fixtures/fake-memstead', import.meta.url));

const BASELINE = '1111111111111111111111111111111111111111';
const CURRENT = '2222222222222222222222222222222222222222';

function writeFiles(root, files) {
  for (const [rel, content] of Object.entries(files)) {
    const abs = join(root, rel);
    mkdirSync(dirname(abs), { recursive: true });
    writeFileSync(abs, typeof content === 'string' ? content : JSON.stringify(content, null, 2));
  }
}

// Dump with a source graph vault (`engine`, carrying snapshot_token) and a
// destination vault (`engine-dest`, carrying the sync_state baseline).
function writeDump(root, { srcSnapshot, baseline }) {
  const dest = {
    name: 'engine-dest', schema: 'sample@0.1.0', description: null,
    writeGuidance: {}, snapshot_token: 'dest-snap',
    ...(baseline ? { sync_state: { 'graph-run/graphsrc': baseline } } : {}),
  };
  const src = {
    name: 'engine', schema: 'sample@0.1.0', description: null,
    writeGuidance: {}, snapshot_token: srcSnapshot,
  };
  const dump = {
    format: 'workspace-dump/v0',
    workspace_root: root,
    vaults: [dest, src],
    schemas: { 'sample@0.1.0': { default_writing_guidance: { goal: 'G', avoid: 'A' } } },
  };
  writeFileSync(join(root, '.fake-dump.json'), JSON.stringify(dump, null, 2));
}

function buildGraphWorkspace({ srcSnapshot = CURRENT, baseline = BASELINE, changes } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'memstead-graph-cursor-'));
  writeFiles(root, {
    '.memstead.toml': `format = "memstead-plugin/v0"\n`,
    // A graph-typed medium whose pointer is the SOURCE vault id.
    '.memstead/mediums/engine-dest/graphsrc.json': { name: 'graphsrc', type: 'graph', pointer: 'engine' },
    '.memstead/facets/engine-dest/graphsrc.json': { name: 'graphsrc', medium: 'graphsrc' },
    '.memstead/projections/engine-dest/graph.json': { source_facets: ['graphsrc'], destination_vault: 'engine-dest' },
    '.memstead/ingests/graph-run.json': { projection: 'engine-dest/graph', mode: 'discovery', trigger: 'manual' },
  });
  if (changes !== undefined) writeFileSync(join(root, '.fake-changes.json'), JSON.stringify(changes));
  writeDump(root, { srcSnapshot, baseline });
  return root;
}

function runInject(root, args = ['graph-run']) {
  const env = {
    ...process.env,
    MEMSTEAD_INGEST_QUIET: '1',
    CLAUDE_SKILL_DIR: root,
    MEMSTEAD_BIN: FAKE_MEMSTEAD,
  };
  const res = spawnSync('node', [INJECT, ...args], { cwd: root, env, encoding: 'utf-8' });
  return { stdout: res.stdout, stderr: res.stderr, status: res.status };
}

describe('graph cursor — changed slice from snapshot-token diff', () => {
  let root;
  afterEach(() => { if (root) rmSync(root, { recursive: true, force: true }); root = null; });

  it('classifies added/updated/removed entity ids into the slice', () => {
    root = buildGraphWorkspace({
      changes: {
        changes: [
          { action: 'updated', id: 'engine--operations-layer' },
          { action: 'added', id: 'engine--new-thing' },
          { action: 'removed', id: 'engine--gone-thing' },
        ],
      },
    });
    const r = runInject(root);
    assert.equal(r.status, 0, r.stderr);
    assert.match(r.stdout, /Source changes since the last sync/);
    assert.match(r.stdout, /\*\*Added:\*\*[\s\S]*engine--new-thing/);
    assert.match(r.stdout, /\*\*Modified:\*\*[\s\S]*engine--operations-layer/);
    assert.match(r.stdout, /\*\*Deleted:\*\*[\s\S]*engine--gone-thing/);
    // The recorded-baseline token is the source vault's current snapshot.
    assert.match(r.stdout, new RegExp(`set-sync-state engine-dest 'graph-run/graphsrc' '${CURRENT}'`));
  });

  it('renamed entity surfaces as new-id added + old-id deleted', () => {
    root = buildGraphWorkspace({
      changes: { changes: [{ action: 'renamed', from_id: 'engine--old', to_id: 'engine--renamed' }] },
    });
    const r = runInject(root);
    assert.match(r.stdout, /\*\*Added:\*\*[\s\S]*engine--renamed/);
    assert.match(r.stdout, /\*\*Deleted:\*\*[\s\S]*engine--old/);
  });

  it('unchanged source snapshot ⇒ no slice, no changes CLI call', () => {
    // baseline === current snapshot: computeGraphSlice short-circuits.
    root = buildGraphWorkspace({ srcSnapshot: BASELINE, baseline: BASELINE });
    const r = runInject(root);
    assert.doesNotMatch(r.stdout, /Source changes since the last sync/);
  });

  it('no baseline ⇒ observable re-seed at the source snapshot', () => {
    root = buildGraphWorkspace({ baseline: null });
    const r = runInject(root);
    assert.match(r.stdout, /No prior sync baseline/);
    assert.match(r.stdout, new RegExp(`set-sync-state engine-dest 'graph-run/graphsrc' '${CURRENT}'`));
  });
});
