/**
 * workspace-loader.test.js — the four-primitive store reader.
 *
 * Verifies that `loadWorkspace` reading the new `.memstead/{mediums,facets,
 * projections,ingests}/` layout produces the same internal assembled shape
 * (`ingests[].sources[].scope.{type,scope.tree}`, `destinations[].mem`)
 * that `inject.mjs` consumes — so a migrated workspace behaves identically to
 * the legacy one. The engine dump is injected via `opts.fetchDump`.
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { loadWorkspace } from './workspace-loader.mjs';

// Fixture mirrors the REAL `memstead workspace dump --json` wire — uniformly
// snake_case (`schema_ref`, `write_guidance`), captured from an actual CLI
// run. Do NOT "simplify" the keys to the loader's internal camelCase shape:
// a fixture emitting the loader's own casing masked a live bug where every
// mem's schema silently resolved to null against the real CLI.
const DUMP = {
  format: 'workspace-dump/v0',
  mems: [
    {
      name: 'macos',
      capability: 'writable',
      schema_ref: 'software@0.1.0',
      description: null,
      write_guidance: { granularity: 'one entity per subsystem' },
      snapshot_token: '14d738d9b0d9852c8e9b1ac67692f6a118c90d1a',
      sync_state: { 'macos-graph/source-tree': '1ddc4bf5c5b251ab613af323cfa90d4b8bdae5db' },
    },
    {
      name: 'engine',
      capability: 'writable',
      schema_ref: 'software@0.1.0',
      description: null,
      write_guidance: {},
      // `snapshot_token` / `sync_state` are omitted-when-empty on the wire.
    },
  ],
  schemas: {},
};

describe('workspace-loader — four-primitive store', () => {
  it('reads .memstead/{mediums,facets,projections,ingests} into the legacy assembled shape', () => {
    const root = mkdtempSync(join(tmpdir(), 'wsl-fourprim-'));
    try {
      writeFileSync(join(root, '.memstead.toml'), 'format = "memstead-plugin/v0"\n');
      const mk = (p) => mkdirSync(join(root, p), { recursive: true });
      mk('.memstead/mediums/macos');
      mk('.memstead/facets/macos');
      mk('.memstead/projections/macos');
      mk('.memstead/ingests');
      writeFileSync(
        join(root, '.memstead/mediums/macos/source-tree.json'),
        JSON.stringify({ name: 'source-tree', type: 'codebase', pointer: '../macos' })
      );
      writeFileSync(
        join(root, '.memstead/facets/macos/source-tree.json'),
        JSON.stringify({
          name: 'source-tree',
          medium: 'source-tree',
          scope: [{ path: '../macos/**/*.swift', mode: 'allow' }],
        })
      );
      writeFileSync(
        join(root, '.memstead/projections/macos/graph.json'),
        JSON.stringify({
          intent: 'i',
          source_facets: ['source-tree'],
          reference_mems: ['engine'],
          destination_mem: 'macos',
        })
      );
      writeFileSync(
        join(root, '.memstead/ingests/macos-graph.json'),
        JSON.stringify({ projection: 'macos/graph', mode: 'discovery', trigger: 'loop', batch_size: 20 })
      );

      const ws = loadWorkspace(root, { fetchDump: () => DUMP });

      assert.equal(ws.ingests.length, 1);
      const ing = ws.ingests[0];
      assert.equal(ing.name, 'macos-graph');
      assert.equal(ing.mode, 'discovery');
      assert.equal(ing.batch_size, 20);

      // A primary source reconstructed from facet + its medium: the legacy
      // `scope.type` (from the medium) and `scope.scope.tree` (from the facet).
      const primary = ing.sources.find((s) => s.role === 'primary');
      assert.ok(primary, 'primary source present');
      assert.equal(primary.facet.mediumType, 'codebase');
      assert.deepEqual(primary.facet.scope.tree, [{ path: '../macos/**/*.swift', mode: 'allow' }]);

      // Reference mem preserved as a reference source.
      const ref = ing.sources.find((s) => s.role === 'reference');
      assert.ok(ref, 'reference source present');
      assert.equal(ref.mem, 'engine');

      // Single destination from destination_mem.
      assert.equal(ing.destinations.length, 1);
      assert.equal(ing.destinations[0].mem, 'macos');

      // Per-mem metadata resolved off the real snake_case wire. These
      // assertions are the regression net for the casing bug: reading
      // `v.schema` / `v.writeGuidance` off a `schema_ref` / `write_guidance`
      // wire made schema null and guidance {} for every mem.
      assert.equal(ws.memMeta.macos.schema, 'software@0.1.0');
      assert.deepEqual(ws.memMeta.macos.writeGuidance, { granularity: 'one entity per subsystem' });
      assert.equal(ws.memMeta.macos.snapshotToken, '14d738d9b0d9852c8e9b1ac67692f6a118c90d1a');
      assert.deepEqual(ws.memMeta.macos.syncState, { 'macos-graph/source-tree': '1ddc4bf5c5b251ab613af323cfa90d4b8bdae5db' });
      assert.equal(ws.memMeta.engine.schema, 'software@0.1.0');
      assert.deepEqual(ws.memMeta.engine.writeGuidance, {});
      assert.equal(ws.memMeta.engine.snapshotToken, null);
      assert.deepEqual(ws.memMeta.engine.syncState, {});
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it('reads empty when the workspace store has no pipeline directories', () => {
    const root = mkdtempSync(join(tmpdir(), 'wsl-empty-'));
    try {
      writeFileSync(join(root, '.memstead.toml'), 'format = "memstead-plugin/v0"\n');
      // No `.memstead/{mediums,facets,projections,ingests}/` and no legacy
      // folders — the legacy reader is gone, so this is simply empty (not a
      // fallback to `scopes|projections|ingests/`).
      const ws = loadWorkspace(root, { fetchDump: () => DUMP });
      assert.equal(ws.ingests.length, 0);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

describe('workspace-loader — front-door discovery (fresh engine-marker workspace)', () => {
  it('loads a workspace with only the engine marker (no legacy .memstead.toml)', () => {
    // A fresh `init`/`quickstart` workspace has `.memstead/workspace.toml` and
    // NO `.memstead.toml`. Requiring the legacy marker made `/ingest` fail with
    // "not found" for every non-maintainer user.
    const root = mkdtempSync(join(tmpdir(), 'wsl-enginemarker-'));
    try {
      mkdirSync(join(root, '.memstead'), { recursive: true });
      writeFileSync(join(root, '.memstead', 'workspace.toml'), 'name = "fresh"\n');
      const ws = loadWorkspace(root, { fetchDump: () => DUMP });
      assert.equal(ws.format, null); // no legacy toml → current-format default
      assert.equal(ws.ingests.length, 0);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it('throws only when NEITHER marker is present', () => {
    const root = mkdtempSync(join(tmpdir(), 'wsl-nomarker-'));
    try {
      assert.throws(
        () => loadWorkspace(root, { fetchDump: () => DUMP }),
        /no Memstead workspace/,
      );
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it('degrades to empty when `workspace dump` is unavailable (folder-backed workspace)', () => {
    // `workspace dump` is mem-repo-only; on a folder-backed workspace it errors.
    // The loader must degrade to a useful "no ingests" result, not hard-fail.
    const root = mkdtempSync(join(tmpdir(), 'wsl-nodump-'));
    try {
      mkdirSync(join(root, '.memstead'), { recursive: true });
      writeFileSync(join(root, '.memstead', 'workspace.toml'), 'name = "fresh"\n');
      const ws = loadWorkspace(root, {
        fetchDump: () => { throw new Error('mem-repo-only'); },
      });
      assert.equal(ws.ingests.length, 0);
      assert.deepEqual(ws.mems, []);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
