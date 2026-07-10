import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, chmodSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  pickReferencedEntityIds,
  formatRealizationNotice,
} from './check-realization-utils.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const HOOK = join(HERE, 'check-realization.mjs');

// ── pure helpers ──────────────────────────────────────────────────────────

describe('pickReferencedEntityIds', () => {
  it('includes span- and file-grain anchors', () => {
    const reply = {
      count: 2,
      anchors: [
        { grain: 'file', entity_id: 'specs--store', artifact: 'lib/store.js' },
        { grain: 'span', entity_id: 'specs--parser', artifact: 'lib/parser.js' },
      ],
    };
    assert.deepStrictEqual(pickReferencedEntityIds(reply), ['specs--store', 'specs--parser']);
  });

  it('excludes tree, url, and entity grains', () => {
    const reply = {
      anchors: [
        { grain: 'tree', entity_id: 'specs--dir', artifact: 'src/' },
        { grain: 'url', entity_id: 'specs--web', artifact: 'https://x' },
        { grain: 'entity', entity_id: 'specs--ent', artifact: 'other--e' },
      ],
    };
    assert.deepStrictEqual(pickReferencedEntityIds(reply), []);
  });

  it('deduplicates entity ids in first-seen order', () => {
    const reply = {
      anchors: [
        { grain: 'file', entity_id: 'specs--a', artifact: 'a.js' },
        { grain: 'span', entity_id: 'specs--a', artifact: 'a.js' },
        { grain: 'file', entity_id: 'specs--b', artifact: 'b.js' },
      ],
    };
    assert.deepStrictEqual(pickReferencedEntityIds(reply), ['specs--a', 'specs--b']);
  });

  it('is tolerant of malformed / empty replies', () => {
    assert.deepStrictEqual(pickReferencedEntityIds(null), []);
    assert.deepStrictEqual(pickReferencedEntityIds({}), []);
    assert.deepStrictEqual(pickReferencedEntityIds({ anchors: 'nope' }), []);
    assert.deepStrictEqual(pickReferencedEntityIds({ count: 0, anchors: [] }), []);
    assert.deepStrictEqual(pickReferencedEntityIds({ anchors: [{ grain: 'file' }] }), []);
  });
});

describe('formatRealizationNotice', () => {
  it('names the ids and references memstead_entity, never /audit', () => {
    const msg = formatRealizationNotice('lib/store.js', ['specs--store', 'specs--x']);
    assert.match(msg, /lib\/store\.js/);
    assert.match(msg, /specs--store, specs--x/);
    assert.match(msg, /memstead_entity/);
    assert.ok(!msg.includes('/audit'), 'notice must not mention the retired /audit skill');
  });
});

// ── end-to-end (subprocess, fail-open) ──────────────────────────────────────

/** Stand up a temp workspace with the engine marker and a fake `memstead`. */
function setupWorkspace(fakeReplyJson) {
  const root = mkdtempSync(join(tmpdir(), 'check-realization-e2e-'));
  mkdirSync(join(root, '.memstead'), { recursive: true });
  writeFileSync(join(root, '.memstead', 'workspace.toml'), 'format = "memstead-plugin/v0"\n');
  mkdirSync(join(root, 'src'), { recursive: true });
  writeFileSync(join(root, 'src', 'lib.rs'), 'fn main() {}\n');

  const binDir = join(root, 'fakebin');
  mkdirSync(binDir, { recursive: true });
  if (fakeReplyJson !== null) {
    const script = join(binDir, 'memstead');
    // Ignores its args; echoes a fixed anchors --json reply and exits 0.
    writeFileSync(script, `#!/bin/sh\ncat <<'JSON'\n${fakeReplyJson}\nJSON\n`);
    chmodSync(script, 0o755);
  }
  return { root, binDir };
}

function runHook(root, binDir, filePath) {
  // When binDir is null, PATH excludes any `memstead` → spawn ENOENT (fail-open).
  const PATH = binDir ? `${binDir}:/usr/bin:/bin` : '/nonexistent-empty-dir';
  // Use the running node's absolute path so the interpreter is found even when
  // PATH is overridden to hide `memstead` from the hook's own lookup.
  return spawnSync(process.execPath, [HOOK], {
    cwd: root,
    input: JSON.stringify({ tool_input: { file_path: filePath }, cwd: root }),
    env: { ...process.env, PATH },
    encoding: 'utf-8',
  });
}

describe('integration: anchored edit emits a notice', () => {
  let ws;
  before(() => {
    ws = setupWorkspace(
      JSON.stringify({
        count: 1,
        anchors: [
          { grain: 'file', entity_id: 'specs--engine', artifact: 'src/lib.rs', class: 'anchored' },
        ],
        composition: {},
      }),
    );
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('names the referencing entity id and exits 0', () => {
    const r = runHook(ws.root, ws.binDir, join(ws.root, 'src', 'lib.rs'));
    assert.strictEqual(r.status, 0);
    assert.match(r.stdout, /REALIZATION EDIT/);
    assert.match(r.stdout, /specs--engine/);
    assert.match(r.stdout, /src\/lib\.rs/);
    assert.ok(!r.stdout.includes('/audit'), 'must not mention /audit');
  });
});

describe('integration: unreferenced edit is silent', () => {
  let ws;
  before(() => {
    ws = setupWorkspace(JSON.stringify({ count: 0, anchors: [], composition: {} }));
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('emits nothing and exits 0', () => {
    const r = runHook(ws.root, ws.binDir, join(ws.root, 'src', 'lib.rs'));
    assert.strictEqual(r.status, 0);
    assert.strictEqual(r.stdout.trim(), '');
  });
});

describe('integration: no memstead on PATH is fail-open', () => {
  let ws;
  before(() => {
    ws = setupWorkspace(null); // no fake binary planted
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('passes through silently (exit 0, no output) when the binary is absent', () => {
    const r = runHook(ws.root, null, join(ws.root, 'src', 'lib.rs'));
    assert.strictEqual(r.status, 0);
    assert.strictEqual(r.stdout.trim(), '');
  });
});
