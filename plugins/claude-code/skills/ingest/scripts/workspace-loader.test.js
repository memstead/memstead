/**
 * workspace-loader.test.js — the four-primitive store reader.
 *
 * Verifies that `loadWorkspace` reading the new `.memstead/{mediums,facets,
 * projections,ingests}/` layout produces the same internal assembled shape
 * (`ingests[].sources[].scope.{type,scope.tree}`, `destinations[].vault`)
 * that `inject.mjs` consumes — so a migrated workspace behaves identically to
 * the legacy one. The engine dump is injected via `opts.fetchDump`.
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { loadWorkspace } from './workspace-loader.mjs';

const DUMP = {
  format: 'workspace-dump/v0',
  vaults: [
    { name: 'macos', schema: 'software@0.1.0' },
    { name: 'engine', schema: 'software@0.1.0' },
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
          reference_vaults: ['engine'],
          destination_vault: 'macos',
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

      // Reference vault preserved as a reference source.
      const ref = ing.sources.find((s) => s.role === 'reference');
      assert.ok(ref, 'reference source present');
      assert.equal(ref.vault, 'engine');

      // Single destination from destination_vault.
      assert.equal(ing.destinations.length, 1);
      assert.equal(ing.destinations[0].vault, 'macos');
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
