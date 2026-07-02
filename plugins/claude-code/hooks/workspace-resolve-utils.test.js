import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import {
  findWorkspaceRoot,
  readFolderMemDirs,
  resolveMemDirs,
} from './workspace-resolve-utils.mjs';
import { checkEditTarget } from './guard-entity-edit-utils.mjs';

// Build a workspace fixture under a tempdir.
//   layout: <root>/.memstead/workspace.toml + <root>/.memstead/state/mounts.json
function makeWorkspace(mounts, storeDir = '.memstead') {
  const root = mkdtempSync(join(tmpdir(), 'memstead-ws-'));
  mkdirSync(join(root, storeDir, 'state'), { recursive: true });
  writeFileSync(join(root, storeDir, 'workspace.toml'), 'format = "test"\n');
  writeFileSync(
    join(root, storeDir, 'state', 'mounts.json'),
    JSON.stringify({ format: 'memstead-mounts-3', mounts }),
  );
  return root;
}

const fixtures = [];
function cleanup() {
  for (const f of fixtures) { try { rmSync(f, { recursive: true, force: true }); } catch {} }
  fixtures.length = 0;
}

describe('findWorkspaceRoot', () => {
  let root;
  before(() => { root = makeWorkspace([]); fixtures.push(root); });
  after(cleanup);

  it('finds the workspace root from the root itself', () => {
    assert.equal(findWorkspaceRoot(root), resolve(root));
  });

  it('walks up from a nested subdirectory', () => {
    const nested = join(root, 'a', 'b', 'c');
    mkdirSync(nested, { recursive: true });
    assert.equal(findWorkspaceRoot(nested), resolve(root));
  });

  it('returns null when no workspace marker is found above', () => {
    const orphan = mkdtempSync(join(tmpdir(), 'memstead-orphan-'));
    fixtures.push(orphan);
    assert.equal(findWorkspaceRoot(orphan), null);
  });
});

describe('readFolderMemDirs', () => {
  after(cleanup);

  it('returns [] for a pure git-branch workspace (no working-tree entity files)', () => {
    const root = makeWorkspace([
      { mem: 'engine', storage: { type: 'git-branch', gitdir: 'mem-repo/.git', branch: 'refs/heads/memstead/engine' } },
      { mem: 'plugin', storage: { type: 'git-branch', gitdir: 'mem-repo/.git', branch: 'refs/heads/memstead/plugin' } },
    ]);
    fixtures.push(root);
    assert.deepEqual(readFolderMemDirs(root), []);
  });

  it('returns folder mount dirs resolved against the workspace root', () => {
    const root = makeWorkspace([
      { mem: 'engine', storage: { type: 'folder', path: 'engine' } },
      { mem: 'notes', storage: { type: 'folder', path: 'sub/notes' } },
      { mem: 'sealed', storage: { type: 'archive', path: 'x.mem' } },
    ]);
    fixtures.push(root);
    assert.deepEqual(readFolderMemDirs(root), [
      resolve(root, 'engine'),
      resolve(root, 'sub/notes'),
    ]);
  });

  it('falls back to the mem name when a folder mount omits a path', () => {
    const root = makeWorkspace([{ mem: 'engine', storage: { type: 'folder' } }]);
    fixtures.push(root);
    assert.deepEqual(readFolderMemDirs(root), [resolve(root, 'engine')]);
  });

  it('resolves path: "" to the workspace root (the shape `memstead init`/`quickstart` write)', () => {
    // The engine's single root-level mem is recorded with an EMPTY path —
    // the mem's entity files live at the workspace root itself. A
    // truthiness check on the path used to drop this mount, leaving every
    // quickstart workspace unguarded and the interview state file unread.
    const root = makeWorkspace([{ mem: 'probe', storage: { type: 'folder', path: '' } }]);
    fixtures.push(root);
    assert.deepEqual(readFolderMemDirs(root), [resolve(root)]);
  });

  it('returns [] when mounts.json is absent', () => {
    const root = mkdtempSync(join(tmpdir(), 'memstead-nomounts-'));
    mkdirSync(join(root, '.memstead'), { recursive: true });
    writeFileSync(join(root, '.memstead', 'workspace.toml'), 'format = "test"\n');
    fixtures.push(root);
    assert.deepEqual(readFolderMemDirs(root), []);
  });
});

describe('resolveMemDirs', () => {
  after(cleanup);

  it('honors explicit --mem args (legacy/hand-authored configs) over walk-up', () => {
    const cfg = { mcpServers: { memstead: { args: ['--mem', './specs'] } } };
    assert.deepEqual(
      resolveMemDirs({ cwd: '/project', mcpConfig: cfg }),
      [resolve('/project', './specs')],
    );
  });

  it('falls back to walk-up + folder mounts when no --mem arg is present', () => {
    const root = makeWorkspace([{ mem: 'engine', storage: { type: 'folder', path: 'engine' } }]);
    fixtures.push(root);
    const cfg = { mcpServers: { memstead: { command: 'sh', args: ['-c', 'cd x && exec memstead-mcp'] } } };
    assert.deepEqual(
      resolveMemDirs({ cwd: root, mcpConfig: cfg }),
      [resolve(root, 'engine')],
    );
  });

  it('returns [] for a git-branch workspace (the real memstead layout)', () => {
    const root = makeWorkspace([
      { mem: 'engine', storage: { type: 'git-branch', gitdir: 'mem-repo/.git', branch: 'refs/heads/memstead/engine' } },
    ]);
    fixtures.push(root);
    const cfg = { mcpServers: { memstead: { command: 'sh', args: ['-c', 'cd graph && exec ../memstead-mcp'] } } };
    assert.deepEqual(resolveMemDirs({ cwd: root, mcpConfig: cfg }), []);
  });

  it('returns [] when cwd is not inside any workspace', () => {
    const orphan = mkdtempSync(join(tmpdir(), 'memstead-orphan2-'));
    fixtures.push(orphan);
    assert.deepEqual(resolveMemDirs({ cwd: orphan, mcpConfig: null }), []);
  });

  // The real memstead shape: .mcp.json at the project root, workspace in a
  // SUBDIRECTORY reached via `cd <dir>`. A walk-up from cwd never descends, so
  // the cd-target must be honored or the workspace is missed entirely.
  it('finds a folder workspace living in a subdirectory via the cd-target', () => {
    const projectRoot = mkdtempSync(join(tmpdir(), 'memstead-proj-'));
    fixtures.push(projectRoot);
    const wsDir = join(projectRoot, 'sub-ws');
    mkdirSync(join(wsDir, '.memstead', 'state'), { recursive: true });
    writeFileSync(join(wsDir, '.memstead', 'workspace.toml'), 'format = "test"\n');
    writeFileSync(
      join(wsDir, '.memstead', 'state', 'mounts.json'),
      JSON.stringify({ mounts: [{ mem: 'engine', storage: { type: 'folder', path: 'engine' } }] }),
    );
    const cfg = { mcpServers: { memstead: { command: 'sh', args: ['-c', 'cd sub-ws && exec ../memstead-mcp'] } } };
    assert.deepEqual(
      resolveMemDirs({ cwd: projectRoot, mcpConfig: cfg }),
      [resolve(wsDir, 'engine')],
    );
  });

  it('returns [] for a git-branch workspace in a subdirectory (memstead itself)', () => {
    const projectRoot = mkdtempSync(join(tmpdir(), 'memstead-proj2-'));
    fixtures.push(projectRoot);
    const wsDir = join(projectRoot, 'graph');
    mkdirSync(join(wsDir, '.memstead', 'state'), { recursive: true });
    writeFileSync(join(wsDir, '.memstead', 'workspace.toml'), 'format = "memstead-git-branch-2"\n');
    writeFileSync(
      join(wsDir, '.memstead', 'state', 'mounts.json'),
      JSON.stringify({ mounts: [{ mem: 'engine', storage: { type: 'git-branch', gitdir: 'mem-repo/.git', branch: 'refs/heads/memstead/engine' } }] }),
    );
    const cfg = { mcpServers: { memstead: { command: 'sh', args: ['-c', 'cd graph && exec ../engine/target/release/memstead-mcp'] } } };
    assert.deepEqual(resolveMemDirs({ cwd: projectRoot, mcpConfig: cfg }), []);
  });
});

// End-to-end: on a folder workspace the resolved dir feeds checkEditTarget and
// a real entity edit is blocked — the behavior the previous ./specs fallback lost.
describe('resolved dir blocks a real folder-mem entity edit', () => {
  after(cleanup);

  it('blocks an entity-named .md inside a resolved folder mem', () => {
    const root = makeWorkspace([{ mem: 'engine', storage: { type: 'folder', path: 'engine' } }]);
    mkdirSync(join(root, 'engine'), { recursive: true });
    fixtures.push(root);

    const cfg = { mcpServers: { memstead: { command: 'sh', args: ['-c', 'cd . && exec memstead-mcp'] } } };
    const [memDir] = resolveMemDirs({ cwd: root, mcpConfig: cfg });
    assert.ok(memDir, 'a folder mem dir resolves');

    const entityEdit = checkEditTarget(join(memDir, 'cross-mem-edge.md'), memDir, existsSync(memDir));
    assert.equal(entityEdit.action, 'block');

    const readmeEdit = checkEditTarget(join(memDir, 'README.md'), memDir, existsSync(memDir));
    assert.equal(readmeEdit.action, 'allow');
  });
});
