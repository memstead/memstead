/**
 * inject-git-cursor.test.js — integration tests for the `git` source
 * change-detection strategy and the source-change backoff override.
 *
 * Builds a *real* git repo in a tempdir (the workspace IS the git root),
 * seeds source files, commits, and captures the baseline SHA. The fake
 * `memstead workspace dump` carries that SHA as the engine-held
 * `sync_state` baseline. After committing a change, inject.mjs diffs
 * baseline..HEAD and surfaces the slice. All assertions are over the
 * emitted prompt (and, for backoff, over which ingest the round-robin
 * picks).
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

function git(root, args) {
  const r = spawnSync('git', args, {
    cwd: root,
    encoding: 'utf-8',
    env: {
      ...process.env,
      GIT_AUTHOR_NAME: 'T', GIT_AUTHOR_EMAIL: 't@e',
      GIT_COMMITTER_NAME: 'T', GIT_COMMITTER_EMAIL: 't@e',
    },
  });
  if (r.status !== 0) throw new Error(`git ${args.join(' ')} failed: ${r.stderr}`);
  return (r.stdout || '').trim();
}

function writeFiles(root, files) {
  for (const [rel, content] of Object.entries(files)) {
    const abs = join(root, rel);
    mkdirSync(dirname(abs), { recursive: true });
    writeFileSync(abs, typeof content === 'string' ? content : JSON.stringify(content, null, 2));
  }
}

function writeDump(root, syncState, { name = 'engine-dest' } = {}) {
  const dump = {
    format: 'workspace-dump/v0',
    workspace_root: root,
    mems: [{
      name,
      schema: 'sample@0.1.0',
      description: null,
      writeGuidance: {},
      snapshot_token: 'snap-fixed',
      ...(syncState ? { sync_state: syncState } : {}),
    }],
    schemas: { 'sample@0.1.0': { default_writing_guidance: { goal: 'G', avoid: 'A' } } },
  };
  writeFileSync(join(root, '.fake-dump.json'), JSON.stringify(dump, null, 2));
}

// Build a workspace that is itself a git repo. One discovery ingest over
// a single `git` (or `auto`) codebase facet selecting `sources/engine/**/*.rs`.
function buildGitWorkspace({ changeDetection = 'git' } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'memstead-git-cursor-'));
  writeFiles(root, {
    '.memstead.toml': `format = "memstead-plugin/v0"\n`,
    '.memstead/mediums/engine-dest/src.json': {
      name: 'src', type: 'codebase', pointer: 'sources/engine', change_detection: changeDetection,
    },
    '.memstead/facets/engine-dest/src.json': {
      name: 'src', medium: 'src', scope: [{ path: 'sources/engine/**/*.rs', mode: 'allow' }],
    },
    '.memstead/projections/engine-dest/graph.json': {
      source_facets: ['src'], destination_mem: 'engine-dest',
    },
    '.memstead/ingests/git-run.json': {
      projection: 'engine-dest/graph', mode: 'discovery', trigger: 'manual',
    },
    'sources/engine/lib.rs': '// engine lib v1',
    'sources/engine/main.rs': '// engine main v1',
  });
  git(root, ['init', '-q']);
  git(root, ['add', '-A']);
  git(root, ['commit', '-q', '-m', 'init', '--no-gpg-sign']);
  const base = git(root, ['rev-parse', 'HEAD']);
  writeDump(root, { 'git-run/src': base });
  return { root, base };
}

function runInject(root, args = ['git-run'], extraEnv = {}) {
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

describe('git cursor — changed slice from baseline..HEAD', () => {
  let ws;
  beforeEach(() => { ws = buildGitWorkspace(); });
  afterEach(() => rmSync(ws.root, { recursive: true, force: true }));

  it('surfaces a modified file and offers the new HEAD as the baseline token', () => {
    writeFiles(ws.root, { 'sources/engine/lib.rs': '// engine lib v2 — changed' });
    git(ws.root, ['commit', '-aqm', 'change lib', '--no-gpg-sign']);
    const head = git(ws.root, ['rev-parse', 'HEAD']);

    const r = runInject(ws.root);
    assert.equal(r.status, 0, r.stderr);
    assert.match(r.stdout, /Source changes since the last sync/);
    assert.match(r.stdout, /\*\*Modified:\*\*/);
    assert.match(r.stdout, /sources\/engine\/lib\.rs/);
    // The recorded-baseline command carries the current HEAD (a commit id).
    assert.match(r.stdout, new RegExp(`set-sync-state engine-dest 'git-run/src' '${head}'`));
  });

  it('surfaces a deleted file (git scopes the deletion, no on-disk match needed)', () => {
    rmSync(join(ws.root, 'sources/engine/main.rs'));
    git(ws.root, ['commit', '-aqm', 'rm main', '--no-gpg-sign']);
    const r = runInject(ws.root);
    assert.match(r.stdout, /\*\*Deleted:\*\*/);
    assert.match(r.stdout, /sources\/engine\/main\.rs/);
  });

  it('surfaces an added file in scope', () => {
    writeFiles(ws.root, { 'sources/engine/extra.rs': '// new' });
    git(ws.root, ['add', '-A']);
    git(ws.root, ['commit', '-qm', 'add extra', '--no-gpg-sign']);
    const r = runInject(ws.root);
    assert.match(r.stdout, /\*\*Added:\*\*/);
    assert.match(r.stdout, /sources\/engine\/extra\.rs/);
  });

  it('excludes out-of-scope changes (a file outside the facet globs)', () => {
    writeFiles(ws.root, { 'sources/engine/notes.md': '# not a .rs file', 'elsewhere/x.rs': '// outside pointer' });
    git(ws.root, ['add', '-A']);
    git(ws.root, ['commit', '-qm', 'out of scope', '--no-gpg-sign']);
    const r = runInject(ws.root);
    // Neither the non-.rs file nor the out-of-pointer file is in the slice.
    assert.doesNotMatch(r.stdout, /Source changes since the last sync/, 'no in-scope change ⇒ no slice');
  });

  it('unchanged HEAD ⇒ no slice, prompt identical to today', () => {
    const r = runInject(ws.root); // baseline === HEAD (no new commit)
    assert.doesNotMatch(r.stdout, /Source changes since the last sync/);
    assert.doesNotMatch(r.stdout, /No prior sync baseline/);
  });

  it('degrades (no slice, no crash) when the baseline SHA is unknown to the repo', () => {
    // A 40-hex baseline that is not a real commit: `git diff` fails, so the
    // git strategy returns "no reliable signal" and the run proceeds as
    // today rather than crashing (criterion 9 complement).
    writeDump(ws.root, { 'git-run/src': 'deadbeefdeadbeefdeadbeefdeadbeefdeadbeef' });
    writeFiles(ws.root, { 'sources/engine/lib.rs': '// changed v2' });
    git(ws.root, ['commit', '-aqm', 'change', '--no-gpg-sign']);
    const r = runInject(ws.root);
    assert.equal(r.status, 0, r.stderr);
    assert.doesNotMatch(r.stdout, /Source changes since the last sync/, 'unknown baseline degrades, no slice');
    assert.match(r.stdout, /^##\s+Situation/m, 'still produces the normal prompt');
  });
});

describe('git cursor — re-seed and auto resolution', () => {
  it('emits a re-seed notice when the dump carries no baseline', () => {
    const ws = buildGitWorkspace();
    writeDump(ws.root, undefined); // strip the baseline
    try {
      const r = runInject(ws.root);
      assert.match(r.stdout, /No prior sync baseline/);
      assert.match(r.stdout, /set-sync-state engine-dest 'git-run\/src'/);
    } finally { rmSync(ws.root, { recursive: true, force: true }); }
  });

  it('auto resolves to git when a .git work tree is present', () => {
    const ws = buildGitWorkspace({ changeDetection: 'auto' });
    try {
      writeFiles(ws.root, { 'sources/engine/lib.rs': '// auto→git changed body here' });
      git(ws.root, ['commit', '-aqm', 'change', '--no-gpg-sign']);
      const head = git(ws.root, ['rev-parse', 'HEAD']);
      // baseline still the init SHA; auto must pick git and diff to HEAD.
      const r = runInject(ws.root);
      assert.match(r.stdout, /\*\*Modified:\*\*/);
      // A git token (commit id), not an mtime JSON digest, is offered.
      assert.match(r.stdout, new RegExp(`'git-run/src' '${head}'`));
      assert.doesNotMatch(r.stdout, /"aggregate"/, 'auto→git must not emit an mtime digest token');
    } finally { rmSync(ws.root, { recursive: true, force: true }); }
  });
});

describe('git cursor — additive to destination-snapshot backoff', () => {
  // Two ingests sharing the destination mem: the source-change trigger
  // must override an escalated destination-snapshot backoff so the drifted
  // ingest is not slept through. Driven via `--all` (round-robin + backoff).
  function buildTwoIngest() {
    const root = mkdtempSync(join(tmpdir(), 'memstead-git-backoff-'));
    writeFiles(root, {
      '.memstead.toml': `format = "memstead-plugin/v0"\n`,
      '.memstead/mediums/engine-dest/src.json': {
        name: 'src', type: 'codebase', pointer: 'sources/engine', change_detection: 'git',
      },
      '.memstead/facets/engine-dest/src.json': {
        name: 'src', medium: 'src', scope: [{ path: 'sources/engine/**/*.rs', mode: 'allow' }],
      },
      '.memstead/projections/engine-dest/graph.json': {
        source_facets: ['src'], destination_mem: 'engine-dest',
      },
      '.memstead/ingests/git-run.json': { projection: 'engine-dest/graph', mode: 'discovery', trigger: 'loop' },
      'sources/engine/lib.rs': '// v1',
    });
    git(root, ['init', '-q']);
    git(root, ['add', '-A']);
    git(root, ['commit', '-qm', 'init', '--no-gpg-sign']);
    const base = git(root, ['rev-parse', 'HEAD']);
    return { root, base };
  }

  it('runs the drifted ingest even when its destination snapshot is unchanged and backoff escalated', () => {
    const { root, base } = buildTwoIngest();
    try {
      // Pre-escalate backoff: a stale snapshot the destination still sits on,
      // with skips queued — without the source trigger, inject would skip.
      mkdirSync(join(root, '.memstead.cache', 'ingest'), { recursive: true });
      writeFileSync(
        join(root, '.memstead.cache/ingest/ingest-backoff.json'),
        JSON.stringify({ 'git-run': { skip_remaining: 5, skip_level: 5, snapshot: 'snap-fixed' } }),
      );
      // Destination snapshot UNCHANGED (matches the escalated backoff entry).
      writeDump(root, { 'git-run/src': base });

      // Now move the SOURCE past the baseline.
      writeFiles(root, { 'sources/engine/lib.rs': '// v2 drifted' });
      git(root, ['commit', '-aqm', 'drift', '--no-gpg-sign']);

      const r = runInject(root, ['--all']);
      assert.equal(r.status, 0, r.stderr);
      // Not skipped: the changed-slice prompt is emitted for git-run.
      assert.match(r.stdout, /Source changes since the last sync/, 'source-changed ingest must run despite destination backoff');
      assert.match(r.stdout, /\*\*Modified:\*\*/);
    } finally { rmSync(root, { recursive: true, force: true }); }
  });

  it('still backs off when neither source nor destination changed', () => {
    const { root, base } = buildTwoIngest();
    try {
      mkdirSync(join(root, '.memstead.cache', 'ingest'), { recursive: true });
      writeFileSync(
        join(root, '.memstead.cache/ingest/ingest-backoff.json'),
        JSON.stringify({ 'git-run': { skip_remaining: 5, skip_level: 5, snapshot: 'snap-fixed' } }),
      );
      writeDump(root, { 'git-run/src': base }); // baseline === HEAD, no new commit
      const r = runInject(root, ['--all']);
      assert.equal(r.status, 0, r.stderr);
      // Skipped — the backoff still governs the no-change case.
      assert.doesNotMatch(r.stdout, /Source changes since the last sync/);
      assert.match(r.stdout, /Skipped\./);
    } finally { rmSync(root, { recursive: true, force: true }); }
  });
});
