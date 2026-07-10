// End-to-end tests for the ingest deny hook runner (`deny-meta-files.mjs`).
//
// Unlike the pure-`checkCandidate` unit suite, these spawn the actual hook
// against a temp workspace and the engine-written cache file
// (`.memstead.cache/projection/active-deny-paths.json`), asserting the runtime
// exit codes AND the stale-file failure mode (evidence 5 of the fidelity plan)
// is fixed: the hook enforces exactly the ingest whose brief was rendered LAST,
// never a leftover from a previous ingest.

import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';

const HOOKS_DIR = dirname(fileURLToPath(import.meta.url));
const HOOK = join(HOOKS_DIR, 'deny-meta-files.mjs');

let ws; // temp workspace root
const cacheFile = () =>
  join(ws, '.memstead.cache', 'projection', 'active-deny-paths.json');

/** Write the engine-managed active-deny cache (simulates a brief render). */
function publishDeny(ingest, denyPaths) {
  mkdirSync(dirname(cacheFile()), { recursive: true });
  writeFileSync(cacheFile(), JSON.stringify({ ingest, deny_paths: denyPaths }));
}

/** Run the hook from `cwd = ws` with a tool_input; return exit status. */
function run(toolInput) {
  const res = spawnSync('node', [HOOK], {
    input: JSON.stringify({ cwd: ws, tool_input: toolInput }),
    encoding: 'utf-8',
  });
  return { status: res.status, stdout: res.stdout ?? '' };
}

before(() => {
  ws = mkdtempSync(join(tmpdir(), 'memstead-deny-'));
  // Minimal workspace marker so the hook resolves this dir as the workspace.
  mkdirSync(join(ws, '.memstead'), { recursive: true });
  writeFileSync(join(ws, '.memstead', 'workspace.toml'), '');
});

after(() => {
  if (ws) rmSync(ws, { recursive: true, force: true });
});

describe('deny hook — runtime enforcement (glob dialect)', () => {
  it('blocks a Read denied by the active list (exit 2)', () => {
    publishDeny('engine-graph', ['dev/**', '**/VISION.md']);
    assert.equal(run({ file_path: join(ws, 'dev/notes/a.md') }).status, 2);
    assert.equal(run({ file_path: join(ws, 'VISION.md') }).status, 2);
  });

  it('blocks a Glob pattern that recurses a denied subtree (exit 2)', () => {
    publishDeny('engine-graph', ['dev/**']);
    assert.equal(run({ pattern: 'dev/**/*.md' }).status, 2);
  });

  it('blocks a Grep whose path targets a denied subtree (exit 2)', () => {
    publishDeny('engine-graph', ['dev/**']);
    assert.equal(run({ pattern: 'TODO', path: 'dev' }).status, 2);
  });

  it('allows a non-denied Read (exit 0)', () => {
    publishDeny('engine-graph', ['dev/**', '**/VISION.md']);
    assert.equal(run({ file_path: join(ws, 'crates/foo/lib.rs') }).status, 0);
  });

  it('allows everything when the active list is empty (exit 0)', () => {
    publishDeny('project-graph', []); // an empty-deny ingest
    assert.equal(run({ file_path: join(ws, 'dev/notes/a.md') }).status, 0);
    assert.equal(run({ file_path: join(ws, 'VISION.md') }).status, 0);
  });
});

describe('deny hook — never stale (evidence 5 regression)', () => {
  it('after a render for Y, X’s entries are no longer enforced', () => {
    // Ingest X denied `dev/**`; its brief was rendered, publishing X's list.
    publishDeny('x-graph', ['dev/**']);
    assert.equal(run({ file_path: join(ws, 'dev/notes/a.md') }).status, 2);

    // Now ingest Y renders — the engine OVERWRITES the cache with Y's list
    // (which denies something else). X's `dev/**` must no longer bite.
    publishDeny('y-graph', ['secrets/**']);
    assert.equal(
      run({ file_path: join(ws, 'dev/notes/a.md') }).status,
      0,
      "X's deny_paths must not survive a render for Y",
    );
    // Y's own list is enforced instead.
    assert.equal(run({ file_path: join(ws, 'secrets/key.txt') }).status, 2);
  });

  it('fails open (exit 0) when cwd is not inside any workspace', () => {
    const outside = mkdtempSync(join(tmpdir(), 'memstead-nows-'));
    try {
      const res = spawnSync('node', [HOOK], {
        input: JSON.stringify({
          cwd: outside,
          tool_input: { file_path: join(outside, 'dev/x.md') },
        }),
        encoding: 'utf-8',
      });
      assert.equal(res.status, 0, 'no workspace resolvable → inert (fail open)');
    } finally {
      rmSync(outside, { recursive: true, force: true });
    }
  });
});
