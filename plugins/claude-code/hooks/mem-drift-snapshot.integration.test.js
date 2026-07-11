// End-to-end integration tests for mem-drift-snapshot.mjs.
// Covers the own-write-suppression contract: a Stop hook fires after
// agent mutations to bring the per-session state file in sync with
// end-of-turn HEADs, so the next prompt's UserPromptSubmit hook
// (mem-drift-notify.mjs) only flags HEAD advances the agent did not
// author.
//
// MCP layer is mocked via `withEngineFn` (the hook family's
// pattern) — the mock reads from a real `git init`'d mem-repo to
// synthesise the `memstead_health` response, so the test fixture stays
// realistic without requiring a built `memstead-mcp` binary at test time.
// Test fixtures using `git init` are explicitly carved out from the
// no-direct-git (engine-owns-mem-repo) rule.

import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  rmSync,
  existsSync,
  readFileSync,
  utimesSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { runDriftSnapshot } from './mem-drift-snapshot-utils.mjs';
import {
  isTrackedMem,
  parseRefList,
  runDriftNotify,
} from './mem-drift-notify-utils.mjs';
import { mockEngine as notifyMockEngine } from './mem-drift-notify.integration.test.js';

function git(args, { cwd, input } = {}) {
  return spawnSync('git', args, { cwd, encoding: 'utf-8', input });
}

function setupWorkspace() {
  const root = mkdtempSync(join(tmpdir(), 'drift-snapshot-e2e-'));
  writeFileSync(join(root, '.memstead.toml'), 'format = "memstead-plugin/v0"\n');
  const memRepo = join(root, 'mem-repo');
  mkdirSync(memRepo, { recursive: true });
  git(['init', '-q', '-b', 'main', memRepo], { cwd: root });
  git(['config', 'user.email', 'test@example.com'], { cwd: memRepo });
  git(['config', 'user.name', 'Test'], { cwd: memRepo });
  git(['config', 'commit.gpgsign', 'false'], { cwd: memRepo });
  writeFileSync(join(memRepo, 'README.md'), 'workspace\n');
  git(['add', 'README.md'], { cwd: memRepo });
  git(['commit', '-q', '-m', 'init'], { cwd: memRepo });
  return { root, memRepo };
}

function commitOnBranch(memRepo, branch, files, message) {
  const refExists = git(['rev-parse', '--verify', `refs/heads/${branch}`], {
    cwd: memRepo,
  }).status === 0;
  if (!refExists) {
    git(['checkout', '--orphan', branch], { cwd: memRepo });
    git(['rm', '-rf', '-q', '.'], { cwd: memRepo });
  } else {
    git(['checkout', '-q', branch], { cwd: memRepo });
  }
  for (const [name, content] of Object.entries(files)) {
    writeFileSync(join(memRepo, name), content);
    git(['add', name], { cwd: memRepo });
  }
  git(['commit', '-q', '-m', message], { cwd: memRepo });
  return git(['rev-parse', 'HEAD'], { cwd: memRepo }).stdout.trim();
}

/**
 * Build a mocked `withEngineFn` that synthesises an `memstead_health` response
 * by reading the test mem-repo gitdir. Test infrastructure only — git
 * here is fixture-state, not plugin code; the no-direct-git rule applies
 * to plugin code under test, not to test fixtures.
 */
function mockEngine(memRepo) {
  return async (_cmd, _timeout, fn) => {
    const client = {
      async callTool(name, _args) {
        if (name !== 'memstead_health') return null;
        const refsResult = git(
          ['for-each-ref', '--format=%(refname) %(objectname)', 'refs/heads/'],
          { cwd: memRepo },
        );
        const refs = parseRefList(refsResult.stdout || '').filter((r) => isTrackedMem(r.name));
        const writable_mems = refs.map((r) => r.name);
        const mems = refs.map((r) => ({
          name: r.name,
          vcs: { gitdir: join(memRepo, '.git'), worktree: memRepo, head: r.sha },
        }));
        return { writable_mems, mems };
      },
    };
    return fn(client);
  };
}

async function runSnapshot({ workspaceRoot, sessionId, memRepo }) {
  return runDriftSnapshot({
    workspaceRoot,
    sessionId,
    engineCommand: { cmd: 'true', args: [], cwd: workspaceRoot },
    withEngineFn: mockEngine(memRepo),
    logger: { error: () => {} },
  });
}

async function runNotify({ workspaceRoot, sessionId, memRepo }) {
  // Notify also goes through the MCP-mocked pipeline. Both hooks share
  // the same state-file format, so the cross-hook tests still pin the
  // contract between snapshot writes and notify reads.
  return runDriftNotify({
    workspaceRoot,
    sessionId,
    engineCommand: { cmd: 'true', args: [], cwd: workspaceRoot },
    withEngineFn: notifyMockEngine(memRepo),
  });
}

function statePath(workspaceRoot, sessionId) {
  return join(
    workspaceRoot,
    '.memstead.cache',
    'drift',
    `last-seen-heads-${sessionId}.json`,
  );
}

function readState(workspaceRoot, sessionId) {
  const path = statePath(workspaceRoot, sessionId);
  if (!existsSync(path)) return null;
  return JSON.parse(readFileSync(path, 'utf-8'));
}

describe('snapshot: turn with own mutations is followed by a silent next prompt', () => {
  let ws;
  const sessionId = 'sess-own-only';
  before(async () => {
    ws = setupWorkspace();
    commitOnBranch(ws.memRepo, 'memstead/engine', { 'a.md': 'v1\n' }, 'eng v1');
    await runNotify({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    commitOnBranch(
      ws.memRepo,
      'memstead/engine',
      { 'a.md': 'v2\n', 'b.md': 'v1\n' },
      'agent mutation in turn 1',
    );
    await runSnapshot({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('emits no system-reminder on the next UserPromptSubmit', async () => {
    const r = await runNotify({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    assert.strictEqual(r.status, 'no-drift', 'own-only mutation must not surface as drift');
  });
});

describe('snapshot: mixed turn (own + sibling) flags only the sibling mem', () => {
  let ws;
  const sessionId = 'sess-mixed';
  before(async () => {
    ws = setupWorkspace();
    commitOnBranch(ws.memRepo, 'memstead/engine', { 'a.md': 'v1\n' }, 'eng v1');
    commitOnBranch(ws.memRepo, 'memstead/macos', { 'm.md': 'v1\n' }, 'mac v1');
    await runNotify({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    commitOnBranch(ws.memRepo, 'memstead/engine', { 'a.md': 'v2\n' }, 'agent in eng');
    await runSnapshot({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    commitOnBranch(ws.memRepo, 'memstead/macos', { 'm.md': 'v2\n' }, 'sibling in mac');
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('reports only memstead/macos on the next UserPromptSubmit', async () => {
    const r = await runNotify({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    assert.strictEqual(r.status, 'drift');
    assert.match(r.reminder, /<system-reminder>/);
    assert.match(r.reminder, /Mem `memstead\/macos`/);
    assert.doesNotMatch(r.reminder, /Mem `memstead\/engine`/, 'own mutation must not appear');
  });
});

describe('snapshot: pure sibling drift still surfaces (regression guard)', () => {
  let ws;
  const sessionId = 'sess-sibling';
  before(async () => {
    ws = setupWorkspace();
    commitOnBranch(ws.memRepo, 'memstead/engine', { 'a.md': 'v1\n' }, 'eng v1');
    await runNotify({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    await runSnapshot({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    commitOnBranch(ws.memRepo, 'memstead/engine', { 'a.md': 'v2\n' }, 'sibling in eng');
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('reports the sibling-only drift on the next prompt', async () => {
    const r = await runNotify({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    assert.strictEqual(r.status, 'drift');
    assert.match(r.reminder, /Mem `memstead\/engine`/);
    assert.match(r.reminder, /- memstead\/engine--a/);
  });
});

describe('snapshot: first-ever fire on a session with no prior state writes silently', () => {
  let ws;
  const sessionId = 'sess-first-snapshot';
  before(() => {
    ws = setupWorkspace();
    commitOnBranch(ws.memRepo, 'memstead/engine', { 'a.md': 'v1\n' }, 'eng v1');
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('writes the state file silently and exits cleanly', async () => {
    const r = await runSnapshot({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    assert.strictEqual(r.status, 'snapshotted', `unexpected status: ${JSON.stringify(r)}`);
    const state = readState(ws.root, sessionId);
    assert.ok(state, 'state file must exist');
    assert.ok(state['memstead/engine'], 'state must record current HEAD');
  });

  it('makes the next UserPromptSubmit silent (no reminder, no drift)', async () => {
    const r = await runNotify({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    // first run records state silently; subsequent run with no advance is no-drift.
    assert.ok(
      r.status === 'first-run' || r.status === 'no-drift',
      `unexpected status: ${r.status}`,
    );
  });
});

describe('snapshot: missing Stop fire degrades to current behaviour (own commits surface)', () => {
  let ws;
  const sessionId = 'sess-stop-skipped';
  before(async () => {
    ws = setupWorkspace();
    commitOnBranch(ws.memRepo, 'memstead/engine', { 'a.md': 'v1\n' }, 'eng v1');
    await runNotify({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    // Turn 1: agent commits. Stop hook is **not** fired.
    commitOnBranch(ws.memRepo, 'memstead/engine', { 'a.md': 'v2\n' }, 'agent in eng');
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('reports own commit as drift (acceptable degraded mode, matches pre-Stop-hook behaviour)', async () => {
    const r = await runNotify({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    assert.strictEqual(r.status, 'drift');
    assert.match(r.reminder, /Mem `memstead\/engine`/);
  });
});

describe('snapshot: probe failure surfaces as probe-failed status', () => {
  let ws;
  const sessionId = 'sess-probe-fail';
  before(() => {
    ws = setupWorkspace();
    commitOnBranch(ws.memRepo, 'memstead/engine', { 'a.md': 'v1\n' }, 'v1');
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('returns a probe-failed result without crashing', async () => {
    const failingFn = async () => {
      throw new Error('mock engine failure');
    };
    const r = await runDriftSnapshot({
      workspaceRoot: ws.root,
      sessionId,
      engineCommand: { cmd: 'true', args: [], cwd: ws.root },
      withEngineFn: failingFn,
      logger: { error: () => {} },
    });
    assert.strictEqual(r.status, 'probe-failed');
    assert.match(r.message, /mock engine failure/);
  });
});

describe('snapshot: corrupt prior state is overwritten cleanly', () => {
  let ws;
  const sessionId = 'sess-corrupt-snapshot';
  before(() => {
    ws = setupWorkspace();
    commitOnBranch(ws.memRepo, 'memstead/engine', { 'a.md': 'v1\n' }, 'eng v1');
    mkdirSync(join(ws.root, '.memstead.cache', 'drift'), { recursive: true });
    writeFileSync(statePath(ws.root, sessionId), '{not valid json');
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('overwrites the corrupt file with the current HEADs', async () => {
    const r = await runSnapshot({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    assert.strictEqual(r.status, 'snapshotted');
    const state = readState(ws.root, sessionId);
    assert.ok(state, 'state file must be readable JSON now');
    assert.ok(state['memstead/engine']);
  });
});

describe('snapshot: branch filter excludes main and __* refs', () => {
  let ws;
  const sessionId = 'sess-snapshot-filter';
  before(() => {
    ws = setupWorkspace();
    commitOnBranch(ws.memRepo, 'memstead/engine', { 'a.md': 'v1\n' }, 'v1');
    commitOnBranch(ws.memRepo, '__SYSTEM', { 'sys.md': 'v1\n' }, 'sys v1');
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('records only writable mem branches in the state file', async () => {
    const r = await runSnapshot({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    assert.strictEqual(r.status, 'snapshotted');
    const state = readState(ws.root, sessionId);
    assert.ok(state['memstead/engine']);
    assert.strictEqual(state.main, undefined);
    assert.strictEqual(state.__SYSTEM, undefined);
  });
});

describe('snapshot: prunes abandoned-session state files older than 14 days', () => {
  let ws;
  const sessionId = 'sess-prune-current';
  const oldSessions = ['sess-prune-old-a', 'sess-prune-old-b'];
  const recentSession = 'sess-prune-recent';
  before(() => {
    ws = setupWorkspace();
    commitOnBranch(ws.memRepo, 'memstead/engine', { 'a.md': 'v1\n' }, 'eng v1');
    mkdirSync(join(ws.root, '.memstead.cache', 'drift'), { recursive: true });
    const now = Date.now();
    const stale = (now - 30 * 24 * 60 * 60 * 1000) / 1000;
    const recent = (now - 1 * 24 * 60 * 60 * 1000) / 1000;
    for (const sid of oldSessions) {
      const p = statePath(ws.root, sid);
      writeFileSync(p, '{}\n');
      utimesSync(p, stale, stale);
    }
    const recentPath = statePath(ws.root, recentSession);
    writeFileSync(recentPath, '{}\n');
    utimesSync(recentPath, recent, recent);
    writeFileSync(join(ws.root, '.memstead.cache', 'drift', 'unrelated.txt'), 'keep me\n');
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('removes only stale last-seen-heads-*.json files; keeps fresh ones and unrelated files', async () => {
    const r = await runSnapshot({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    assert.strictEqual(r.status, 'snapshotted');
    assert.ok(existsSync(statePath(ws.root, sessionId)));
    for (const sid of oldSessions) {
      assert.strictEqual(
        existsSync(statePath(ws.root, sid)),
        false,
        `stale state file ${sid} should have been pruned`,
      );
    }
    assert.ok(
      existsSync(statePath(ws.root, recentSession)),
      'recent state file must not be pruned',
    );
    assert.ok(existsSync(join(ws.root, '.memstead.cache', 'drift', 'unrelated.txt')));
  });
});

describe('snapshot: latency budget', () => {
  let ws;
  const sessionId = 'sess-snapshot-latency';
  before(() => {
    ws = setupWorkspace();
    for (let i = 0; i < 8; i++) {
      commitOnBranch(ws.memRepo, `memstead/v${i}`, { 'a.md': '1\n' }, `v${i}`);
    }
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('runs without catastrophic regression on a typical mem count', async () => {
    const t0 = Date.now();
    const r = await runSnapshot({ workspaceRoot: ws.root, sessionId, memRepo: ws.memRepo });
    const elapsed = Date.now() - t0;
    assert.strictEqual(r.status, 'snapshotted');
    assert.ok(elapsed < 10000, `snapshot took ${elapsed}ms`);
  });
});
