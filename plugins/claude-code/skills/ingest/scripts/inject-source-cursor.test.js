/**
 * inject-source-cursor.test.js — integration tests for the source-change
 * targeting (mtime strategy) wired into inject.mjs.
 *
 * Each test seeds a tempdir workspace with a `change_detection: "mtime"`
 * medium, a fake `memstead workspace dump` (which carries the engine-held
 * `sync_state` baseline), runs inject.mjs as a subprocess, and asserts on
 * the emitted prompt — the observability seam the plan names. The dump's
 * `sync_state` is rewritten between runs to simulate the agent advancing
 * (or not advancing) the engine baseline via `mem set-sync-state`.
 */

import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  rmSync,
  readFileSync,
} from 'node:fs';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const INJECT = fileURLToPath(new URL('./inject.mjs', import.meta.url));
const FAKE_MEMSTEAD = fileURLToPath(new URL('./test-fixtures/fake-memstead', import.meta.url));

function makeWorkspace(files) {
  const root = mkdtempSync(join(tmpdir(), 'memstead-source-cursor-'));
  for (const [rel, content] of Object.entries(files)) {
    const abs = join(root, rel);
    mkdirSync(dirname(abs), { recursive: true });
    writeFileSync(abs, typeof content === 'string' ? content : JSON.stringify(content, null, 2));
  }
  return root;
}

// Write/overwrite the fake dump. `syncState` is the per-mem engine
// baseline keyed by `<ingest>/<facet>`; the engine omits it when empty,
// so an undefined value writes no key.
function writeDump(root, syncState) {
  const dump = {
    format: 'workspace-dump/v0',
    workspace_root: root,
    mems: [
      {
        name: 'engine-dest',
        schema: 'sample@0.1.0',
        description: null,
        writeGuidance: {},
        snapshot_token: 'snap-0',
        ...(syncState ? { sync_state: syncState } : {}),
      },
    ],
    schemas: { 'sample@0.1.0': { default_writing_guidance: { goal: 'G', avoid: 'A' } } },
  };
  writeFileSync(join(root, '.fake-dump.json'), JSON.stringify(dump, null, 2));
}

// A workspace with one discovery ingest over a single `mtime` codebase
// facet selecting `sources/engine/**/*.rs`.
function buildWorkspace(srcFiles, { changeDetection = 'mtime' } = {}) {
  const files = {
    '.memstead.toml': `format = "memstead-plugin/v0"\n`,
    '.memstead/mediums/engine-dest/src.json': {
      name: 'src',
      type: 'codebase',
      pointer: 'sources/engine',
      change_detection: changeDetection,
    },
    '.memstead/facets/engine-dest/src.json': {
      name: 'src',
      medium: 'src',
      scope: [{ path: 'sources/engine/**/*.rs', mode: 'allow' }],
    },
    '.memstead/projections/engine-dest/graph.json': {
      source_facets: ['src'],
      destination_mem: 'engine-dest',
    },
    '.memstead/ingests/discovery-run.json': {
      projection: 'engine-dest/graph',
      mode: 'discovery',
      trigger: 'manual',
    },
    '.memstead/ingests/refine-run.json': {
      projection: 'engine-dest/graph',
      mode: 'refinement',
      trigger: 'manual',
      batch_size: 5,
    },
    ...srcFiles,
  };
  const root = makeWorkspace(files);
  writeDump(root); // no baseline yet
  return root;
}

function runInject(root, args = ['discovery-run'], extraEnv = {}) {
  const env = {
    ...process.env,
    MEMSTEAD_INGEST_QUIET: '1',
    CLAUDE_SKILL_DIR: root,
    MEMSTEAD_BIN: FAKE_MEMSTEAD,
    ...extraEnv,
  };
  const res = spawnSync('node', [INJECT, ...args], { cwd: root, env, encoding: 'utf-8' });
  return { stdout: res.stdout, stderr: res.stderr, status: res.status };
}

// Pull the token out of an emitted `memstead mem set-sync-state <mem>
// '<key>' '<token>'` command line.
function extractToken(stdout, key) {
  const re = new RegExp(`set-sync-state\\s+\\S+\\s+'${key.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}'\\s+'(.+)'`);
  const m = stdout.match(re);
  return m ? m[1] : null;
}

const ENGINE_SRC = {
  'sources/engine/lib.rs': '// engine source lib',
  'sources/engine/main.rs': '// engine main entry',
};

describe('source cursor — re-seed (first sync, no baseline)', () => {
  let root;
  beforeEach(() => { root = buildWorkspace(ENGINE_SRC); });
  afterEach(() => rmSync(root, { recursive: true, force: true }));

  it('emits an observable re-seed notice and a baseline-recording command, no priority slice', () => {
    const r = runInject(root);
    assert.equal(r.status, 0, r.stderr);
    assert.match(r.stdout, /No prior sync baseline/, 'first sync states it is seeding the baseline');
    assert.match(r.stdout, /set-sync-state engine-dest 'discovery-run\/src'/, 'emits the engine-routed baseline command');
    assert.doesNotMatch(r.stdout, /\*\*Modified:\*\*|\*\*Added:\*\*|\*\*Deleted:\*\*/, 'no priority slice on first sync');
  });
});

describe('source cursor — unchanged source is identical to today', () => {
  let root;
  beforeEach(() => { root = buildWorkspace(ENGINE_SRC); });
  afterEach(() => rmSync(root, { recursive: true, force: true }));

  it('after the baseline is recorded and nothing changes, the prompt carries no slice', () => {
    const seed = runInject(root);
    const token = extractToken(seed.stdout, 'discovery-run/src');
    assert.ok(token, 'seed run emits a baseline token');
    // Simulate the agent advancing the engine baseline.
    writeDump(root, { 'discovery-run/src': token });

    const r = runInject(root);
    assert.equal(r.status, 0, r.stderr);
    assert.doesNotMatch(r.stdout, /Source changes since the last sync/, 'unchanged source ⇒ no changed-slice block');
    assert.doesNotMatch(r.stdout, /No prior sync baseline/, 'baseline exists, so no re-seed notice');
  });
});

describe('source cursor — changed slice (modify / add / delete)', () => {
  let root;
  beforeEach(() => { root = buildWorkspace(ENGINE_SRC); });
  afterEach(() => rmSync(root, { recursive: true, force: true }));

  function seedAndAdvance() {
    const seed = runInject(root);
    const token = extractToken(seed.stdout, 'discovery-run/src');
    writeDump(root, { 'discovery-run/src': token });
    return token;
  }

  it('surfaces a modified file (size change) in the slice', () => {
    seedAndAdvance();
    writeFileSync(join(root, 'sources/engine/lib.rs'), '// engine source lib — substantially rewritten body');
    const r = runInject(root);
    assert.match(r.stdout, /Source changes since the last sync/);
    assert.match(r.stdout, /\*\*Modified:\*\*/);
    assert.match(r.stdout, /sources\/engine\/lib\.rs/);
  });

  it('surfaces a deleted file in the slice', () => {
    seedAndAdvance();
    rmSync(join(root, 'sources/engine/main.rs'));
    const r = runInject(root);
    assert.match(r.stdout, /\*\*Deleted:\*\*/);
    assert.match(r.stdout, /sources\/engine\/main\.rs/);
  });

  it('surfaces an added file in the slice', () => {
    seedAndAdvance();
    writeFileSync(join(root, 'sources/engine/extra.rs'), '// a new file');
    const r = runInject(root);
    assert.match(r.stdout, /\*\*Added:\*\*/);
    assert.match(r.stdout, /sources\/engine\/extra\.rs/);
  });

  it('carries no coverage/progress figure in the changed-slice briefing', () => {
    seedAndAdvance();
    writeFileSync(join(root, 'sources/engine/lib.rs'), '// changed enough to differ in size now');
    const r = runInject(root);
    assert.match(r.stdout, /Source changes since the last sync/);
    // No percentage, no mapped/unmapped tally, no "X/Y" / "of N" progress.
    assert.doesNotMatch(r.stdout, /\d+\s*%/, 'no percentage');
    assert.doesNotMatch(r.stdout, /\bmapped\b|\bunmapped\b|\bcoverage\b|% covered|complete\b/i, 'no coverage/progress wording');
    assert.doesNotMatch(r.stdout, /\b\d+\s*\/\s*\d+\b/, 'no X/Y tally');
  });
});

describe('source cursor — interrupt-safe advance', () => {
  let root;
  beforeEach(() => { root = buildWorkspace(ENGINE_SRC); });
  afterEach(() => rmSync(root, { recursive: true, force: true }));

  it('re-presents the same slice until the baseline is advanced', () => {
    // Seed + advance baseline.
    const seed = runInject(root);
    const t0 = extractToken(seed.stdout, 'discovery-run/src');
    writeDump(root, { 'discovery-run/src': t0 });

    // Change a file → slice appears.
    writeFileSync(join(root, 'sources/engine/lib.rs'), '// rewritten, definitely a different size here');
    const run2 = runInject(root);
    assert.match(run2.stdout, /\*\*Modified:\*\*/, 'changed file appears');
    const t1 = extractToken(run2.stdout, 'discovery-run/src');
    assert.ok(t1 && t1 !== t0, 'a new baseline token is offered');

    // Simulate an interrupted pass: baseline NOT advanced (still t0).
    const run3 = runInject(root);
    assert.match(run3.stdout, /\*\*Modified:\*\*/, 'same slice re-presented when the baseline did not advance');
    assert.match(run3.stdout, /sources\/engine\/lib\.rs/);

    // Now simulate a completed pass: advance the baseline to t1.
    writeDump(root, { 'discovery-run/src': t1 });
    const run4 = runInject(root);
    assert.doesNotMatch(run4.stdout, /Source changes since the last sync/, 'advanced baseline ⇒ slice not re-presented');
  });
});

describe('source cursor — changed slice in refinement mode', () => {
  let root;
  beforeEach(() => { root = buildWorkspace(ENGINE_SRC); });
  afterEach(() => rmSync(root, { recursive: true, force: true }));

  it('surfaces the changed slice in a refinement-mode prompt too (not just discovery)', () => {
    // Seed + advance the baseline for the refinement ingest.
    const seed = runInject(root, ['refine-run']);
    const token = extractToken(seed.stdout, 'refine-run/src');
    assert.ok(token, 'refinement seed offers a baseline token');
    writeDump(root, { 'refine-run/src': token });

    // Modify a source file, then run again.
    writeFileSync(join(root, 'sources/engine/lib.rs'), '// refinement-mode change, different size');
    const r = runInject(root, ['refine-run']);
    assert.equal(r.status, 0, r.stderr);
    assert.match(r.stdout, /Source changes since the last sync/, 'preface present in refinement mode');
    assert.match(r.stdout, /\*\*Modified:\*\*/);
    assert.match(r.stdout, /sources\/engine\/lib\.rs/);
  });
});

describe('source cursor — durable baseline survives a cache wipe', () => {
  let root;
  beforeEach(() => { root = buildWorkspace(ENGINE_SRC); });
  afterEach(() => rmSync(root, { recursive: true, force: true }));

  it('detects change from the engine baseline alone and degrades to a full scan when the memo is gone', () => {
    // A valid mtime-digest baseline on the dump, but NO skill-cache memo
    // for it (simulates a `.memstead.cache` wipe). Detection must still
    // fire from the digest; the precise slice degrades to the full file
    // set, with an observable degraded note — never a crash.
    const staleBaseline = JSON.stringify({ v: 1, count: 1, watermark: 1, aggregate: '0000000000000000' });
    writeDump(root, { 'discovery-run/src': staleBaseline });
    const r = runInject(root);
    assert.equal(r.status, 0, r.stderr);
    assert.match(r.stdout, /Source changes since the last sync/, 'detection fires from the durable baseline');
    assert.match(r.stdout, /Precise change history .* was unavailable/, 'observable degraded-to-full-scan note');
    // The full current file set is surfaced (degraded targeting).
    assert.match(r.stdout, /sources\/engine\/lib\.rs/);
    assert.match(r.stdout, /sources\/engine\/main\.rs/);
  });
});

describe('source cursor — graceful degradation (change_detection: none)', () => {
  let root;
  beforeEach(() => { root = buildWorkspace(ENGINE_SRC, { changeDetection: 'none' }); });
  afterEach(() => rmSync(root, { recursive: true, force: true }));

  it('runs as today: no changed-slice block, no re-seed, cursor inert', () => {
    const r = runInject(root);
    assert.equal(r.status, 0, r.stderr);
    assert.doesNotMatch(r.stdout, /Source changes since the last sync/);
    assert.doesNotMatch(r.stdout, /No prior sync baseline/);
    assert.doesNotMatch(r.stdout, /set-sync-state/, 'no cursor write instruction for an inert source');
    // Still does useful work — the normal discovery prompt is present.
    assert.match(r.stdout, /^##\s+Situation/m);
  });
});
