// End-to-end integration tests for vault-drift-notify.mjs.
// Each test stands up a real `git init`'d vault-repo under a tempdir
// laid out as a workspace (with `.memstead.toml`), drives the drift
// pipeline against it via mocked MCP, and inspects the returned
// reminder block plus the state file. Mirrors auto-commit's pattern:
// MCP is mocked through `withEngineFn` so tests don't need a built
// `memstead-mcp` binary, while the test fixture still uses real git for
// vault-repo state (test-fixture infrastructure, exempt from the
// engine-owns-vault-repo rule).

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
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  isTrackedVault,
  parseRefList,
  diffPathsToEntityIds,
  runDriftNotify,
} from './vault-drift-notify-utils.mjs';

function git(args, { cwd, input } = {}) {
  return spawnSync('git', args, { cwd, encoding: 'utf-8', input });
}

function setupWorkspace() {
  const root = mkdtempSync(join(tmpdir(), 'drift-notify-e2e-'));
  writeFileSync(join(root, '.memstead.toml'), 'format = "memstead-plugin/v0"\n');
  const vaultRepo = join(root, 'vault-repo');
  mkdirSync(vaultRepo, { recursive: true });
  git(['init', '-q', '-b', 'main', vaultRepo], { cwd: root });
  git(['config', 'user.email', 'test@example.com'], { cwd: vaultRepo });
  git(['config', 'user.name', 'Test'], { cwd: vaultRepo });
  git(['config', 'commit.gpgsign', 'false'], { cwd: vaultRepo });
  writeFileSync(join(vaultRepo, 'README.md'), 'workspace\n');
  git(['add', 'README.md'], { cwd: vaultRepo });
  git(['commit', '-q', '-m', 'init'], { cwd: vaultRepo });
  return { root, vaultRepo };
}

function commitOnBranch(vaultRepo, branch, files, message) {
  const refExists = git(['rev-parse', '--verify', `refs/heads/${branch}`], {
    cwd: vaultRepo,
  }).status === 0;
  if (!refExists) {
    git(['checkout', '--orphan', branch], { cwd: vaultRepo });
    git(['rm', '-rf', '-q', '.'], { cwd: vaultRepo });
  } else {
    git(['checkout', '-q', branch], { cwd: vaultRepo });
  }
  for (const [name, content] of Object.entries(files)) {
    writeFileSync(join(vaultRepo, name), content);
    git(['add', name], { cwd: vaultRepo });
  }
  git(['commit', '-q', '-m', message], { cwd: vaultRepo });
  return git(['rev-parse', 'HEAD'], { cwd: vaultRepo }).stdout.trim();
}

/**
 * Build a mocked `withEngineFn` that synthesises responses for the two
 * MCP tools the notify pipeline depends on:
 *   - `memstead_health { include_config: true }` — reads writable branch
 *     heads via `for-each-ref` against the test vault-repo
 *   - `memstead_changes_since { vault, since }` — reads `diff-tree
 *     --name-only since..HEAD-of-branch` and translates paths to entity
 *     ids
 *
 * Test-fixture-only: `git` here is fixture-state lookup, not plugin
 * code. The plan's no-direct-git rule applies to plugin code under
 * test, not test fixtures.
 */
export function mockEngine(vaultRepo) {
  return async (_cmd, _timeout, fn) => {
    const client = {
      async callTool(name, args) {
        if (name === 'memstead_health') {
          const refsResult = git(
            ['for-each-ref', '--format=%(refname) %(objectname)', 'refs/heads/'],
            { cwd: vaultRepo },
          );
          const refs = parseRefList(refsResult.stdout || '').filter((r) =>
            isTrackedVault(r.name),
          );
          return {
            writable_vaults: refs.map((r) => r.name),
            vaults: refs.map((r) => ({
              name: r.name,
              vcs: { gitdir: join(vaultRepo, '.git'), worktree: vaultRepo, head: r.sha },
            })),
          };
        }
        if (name === 'memstead_changes_since') {
          const branch = args?.vault;
          const since = args?.since;
          const headRes = git(['rev-parse', `refs/heads/${branch}`], { cwd: vaultRepo });
          if (headRes.status !== 0) {
            const err = new Error(`branch ${branch} not found`);
            err.code = 'UNKNOWN_VAULT';
            throw err;
          }
          const head = headRes.stdout.trim();
          if (head === since) return { changes: [], head };
          const diffRes = git(
            ['diff-tree', '-r', '--name-only', '--no-commit-id', `${since}..${head}`],
            { cwd: vaultRepo },
          );
          const paths = (diffRes.stdout || '')
            .split('\n')
            .map((s) => s.trim())
            .filter(Boolean);
          const ids = diffPathsToEntityIds(branch, paths);
          // The real engine returns `{ changes: [{ action, id, ... }] }`.
          // Action doesn't matter for drift — just emit `updated` per id.
          return {
            changes: ids.map((id) => ({ action: 'updated', id })),
            head,
          };
        }
        return null;
      },
    };
    return fn(client);
  };
}

async function runNotify({ workspaceRoot, sessionId, vaultRepo }) {
  return runDriftNotify({
    workspaceRoot,
    sessionId,
    engineCommand: { cmd: 'true', args: [], cwd: workspaceRoot },
    withEngineFn: mockEngine(vaultRepo),
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

describe('integration: first run records HEADs silently', () => {
  let ws;
  before(() => {
    ws = setupWorkspace();
    commitOnBranch(ws.vaultRepo, 'memstead/engine', { 'engine.md': 'engine\n' }, 'initial');
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('writes state and returns first-run on first run', async () => {
    const r = await runNotify({ workspaceRoot: ws.root, sessionId: 'sess-first', vaultRepo: ws.vaultRepo });
    assert.strictEqual(r.status, 'first-run');
    const state = readState(ws.root, 'sess-first');
    assert.ok(state, 'state file should exist');
    assert.ok(state['memstead/engine'], 'state should record memstead/engine HEAD');
  });

  it('writes the .memstead.cache/.gitignore when creating the cache dir', () => {
    assert.ok(existsSync(join(ws.root, '.memstead.cache', '.gitignore')));
    assert.strictEqual(readFileSync(join(ws.root, '.memstead.cache', '.gitignore'), 'utf-8'), '*\n');
  });
});

describe('integration: no-drift run is silent', () => {
  let ws;
  before(async () => {
    ws = setupWorkspace();
    commitOnBranch(ws.vaultRepo, 'memstead/engine', { 'engine.md': 'engine\n' }, 'initial');
    await runNotify({ workspaceRoot: ws.root, sessionId: 'sess-quiet', vaultRepo: ws.vaultRepo });
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('returns no-drift when no HEAD advanced', async () => {
    const before = readState(ws.root, 'sess-quiet');
    const r = await runNotify({ workspaceRoot: ws.root, sessionId: 'sess-quiet', vaultRepo: ws.vaultRepo });
    assert.strictEqual(r.status, 'no-drift');
    const after = readState(ws.root, 'sess-quiet');
    assert.deepStrictEqual(after, before);
  });
});

describe('integration: single-vault drift', () => {
  let ws;
  let sessionId;
  before(async () => {
    ws = setupWorkspace();
    commitOnBranch(ws.vaultRepo, 'memstead/engine', { 'engine.md': 'v1\n' }, 'initial');
    sessionId = 'sess-single';
    await runNotify({ workspaceRoot: ws.root, sessionId, vaultRepo: ws.vaultRepo });
    commitOnBranch(
      ws.vaultRepo,
      'memstead/engine',
      { 'cap-foo.md': 'foo\n', 'engine.md': 'v2\n' },
      'add foo, update engine',
    );
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('returns drift with a system-reminder block listing the changed entity ids', async () => {
    const r = await runNotify({ workspaceRoot: ws.root, sessionId, vaultRepo: ws.vaultRepo });
    assert.strictEqual(r.status, 'drift');
    assert.match(r.reminder, /^<system-reminder>/);
    assert.match(r.reminder, /<\/system-reminder>/);
    assert.match(r.reminder, /Vault `memstead\/engine`/);
    assert.match(r.reminder, /- memstead\/engine--cap-foo/);
    assert.match(r.reminder, /- memstead\/engine--engine/);
    const state = readState(ws.root, sessionId);
    const liveSha = git(['-C', ws.vaultRepo, 'rev-parse', 'refs/heads/memstead/engine'], {})
      .stdout.trim();
    assert.strictEqual(state['memstead/engine'], liveSha);
  });
});

describe('integration: multi-vault drift', () => {
  let ws;
  const sessionId = 'sess-multi';
  before(async () => {
    ws = setupWorkspace();
    commitOnBranch(ws.vaultRepo, 'memstead/engine', { 'a.md': '1\n' }, 'eng v1');
    commitOnBranch(ws.vaultRepo, 'ingest/engine-graph', { 'q.md': '1\n' }, 'ing v1');
    await runNotify({ workspaceRoot: ws.root, sessionId, vaultRepo: ws.vaultRepo });
    commitOnBranch(ws.vaultRepo, 'memstead/engine', { 'a.md': '2\n' }, 'eng v2');
    commitOnBranch(ws.vaultRepo, 'ingest/engine-graph', { 'q.md': '2\n' }, 'ing v2');
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('lists both vaults in one system-reminder block', async () => {
    const r = await runNotify({ workspaceRoot: ws.root, sessionId, vaultRepo: ws.vaultRepo });
    assert.strictEqual(r.status, 'drift');
    const opens = (r.reminder.match(/<system-reminder>/g) || []).length;
    assert.strictEqual(opens, 1);
    assert.match(r.reminder, /Vault `memstead\/engine`/);
    assert.match(r.reminder, /Vault `ingest\/engine-graph`/);
    assert.match(r.reminder, /memstead\/engine--a/);
    assert.match(r.reminder, /ingest\/engine-graph--q/);
  });
});

describe('integration: hierarchical vault names round-trip', () => {
  let ws;
  const sessionId = 'sess-hier';
  before(async () => {
    ws = setupWorkspace();
    commitOnBranch(ws.vaultRepo, 'foo/bar', { 'leaf.md': '1\n' }, 'v1');
    await runNotify({ workspaceRoot: ws.root, sessionId, vaultRepo: ws.vaultRepo });
    commitOnBranch(ws.vaultRepo, 'foo/bar', { 'leaf.md': '2\n' }, 'v2');
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('produces entity ids prefixed with the full hierarchical name', async () => {
    const r = await runNotify({ workspaceRoot: ws.root, sessionId, vaultRepo: ws.vaultRepo });
    assert.strictEqual(r.status, 'drift');
    assert.match(r.reminder, /- foo\/bar--leaf/);
  });
});

describe('integration: vault added/removed between runs', () => {
  let ws;
  const sessionId = 'sess-add-remove';
  before(async () => {
    ws = setupWorkspace();
    commitOnBranch(ws.vaultRepo, 'memstead/engine', { 'a.md': '1\n' }, 'eng v1');
    commitOnBranch(ws.vaultRepo, 'exec-old', { 'gone.md': '1\n' }, 'old v1');
    await runNotify({ workspaceRoot: ws.root, sessionId, vaultRepo: ws.vaultRepo });
    commitOnBranch(ws.vaultRepo, 'exec-new', { 'fresh.md': '1\n' }, 'new v1');
    git(['branch', '-D', 'exec-old'], { cwd: ws.vaultRepo });
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('records the new vault silently and drops the deleted one', async () => {
    const r = await runNotify({ workspaceRoot: ws.root, sessionId, vaultRepo: ws.vaultRepo });
    assert.strictEqual(r.status, 'no-drift');
    const state = readState(ws.root, sessionId);
    assert.ok(state['memstead/engine'], 'tracked vault still recorded');
    assert.ok(state['exec-new'], 'newly observed vault recorded');
    assert.strictEqual(state['exec-old'], undefined, 'deleted vault dropped');
  });
});

describe('integration: branch filter excludes main and __* refs', () => {
  let ws;
  const sessionId = 'sess-filter';
  before(async () => {
    ws = setupWorkspace();
    commitOnBranch(ws.vaultRepo, 'memstead/engine', { 'a.md': '1\n' }, 'v1');
    commitOnBranch(ws.vaultRepo, '__SYSTEM', { 'sys.md': '1\n' }, 'sys v1');
    await runNotify({ workspaceRoot: ws.root, sessionId, vaultRepo: ws.vaultRepo });
    commitOnBranch(ws.vaultRepo, '__SYSTEM', { 'sys.md': '2\n' }, 'sys v2');
    git(['checkout', '-q', 'main'], { cwd: ws.vaultRepo });
    writeFileSync(join(ws.vaultRepo, 'README.md'), 'updated\n');
    git(['add', 'README.md'], { cwd: ws.vaultRepo });
    git(['commit', '-q', '-m', 'main bump'], { cwd: ws.vaultRepo });
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('does not report main or __SYSTEM as drift', async () => {
    const r = await runNotify({ workspaceRoot: ws.root, sessionId, vaultRepo: ws.vaultRepo });
    assert.strictEqual(r.status, 'no-drift');
    const state = readState(ws.root, sessionId);
    assert.strictEqual(state.main, undefined);
    assert.strictEqual(state.__SYSTEM, undefined);
    assert.ok(state['memstead/engine']);
  });
});

describe('integration: corrupt state file is treated as first run', () => {
  let ws;
  const sessionId = 'sess-corrupt';
  before(() => {
    ws = setupWorkspace();
    commitOnBranch(ws.vaultRepo, 'memstead/engine', { 'a.md': '1\n' }, 'v1');
    mkdirSync(join(ws.root, '.memstead.cache', 'drift'), { recursive: true });
    writeFileSync(statePath(ws.root, sessionId), '{not valid json');
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('overwrites the file with current HEADs and returns first-run', async () => {
    const r = await runNotify({ workspaceRoot: ws.root, sessionId, vaultRepo: ws.vaultRepo });
    assert.strictEqual(r.status, 'first-run');
    const state = readState(ws.root, sessionId);
    assert.ok(state['memstead/engine']);
  });
});

describe('integration: probe failure returns probe-failed', () => {
  let ws;
  const sessionId = 'sess-probe-fail-notify';
  before(() => {
    ws = setupWorkspace();
    commitOnBranch(ws.vaultRepo, 'memstead/engine', { 'a.md': '1\n' }, 'v1');
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('surfaces probe-failed without crashing the hook', async () => {
    const failingFn = async () => {
      throw new Error('mock engine failure');
    };
    const r = await runDriftNotify({
      workspaceRoot: ws.root,
      sessionId,
      engineCommand: { cmd: 'true', args: [], cwd: ws.root },
      withEngineFn: failingFn,
    });
    assert.strictEqual(r.status, 'probe-failed');
    assert.match(r.message, /mock engine failure/);
  });
});

describe('integration: latency budget', () => {
  let ws;
  before(async () => {
    ws = setupWorkspace();
    for (let i = 0; i < 8; i++) {
      commitOnBranch(ws.vaultRepo, `memstead/v${i}`, { 'a.md': '1\n' }, `v${i}`);
    }
    await runNotify({ workspaceRoot: ws.root, sessionId: 'sess-latency', vaultRepo: ws.vaultRepo });
    for (let i = 0; i < 8; i++) {
      commitOnBranch(ws.vaultRepo, `memstead/v${i}`, { 'a.md': '2\n' }, `bump v${i}`);
    }
  });
  after(() => rmSync(ws.root, { recursive: true, force: true }));

  it('runs without catastrophic regression on a typical vault count', async () => {
    const t0 = Date.now();
    const r = await runNotify({ workspaceRoot: ws.root, sessionId: 'sess-latency', vaultRepo: ws.vaultRepo });
    const elapsed = Date.now() - t0;
    assert.strictEqual(r.status, 'drift');
    assert.ok(elapsed < 5000, `notify took ${elapsed}ms`);
  });
});
