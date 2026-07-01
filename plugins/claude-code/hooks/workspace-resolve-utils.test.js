import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import {
  findWorkspaceRoot,
  readFolderVaultDirs,
  resolveVaultDirs,
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
    JSON.stringify({ format: 'memstead-mounts-2', mounts }),
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

describe('readFolderVaultDirs', () => {
  after(cleanup);

  it('returns [] for a pure git-branch workspace (no working-tree entity files)', () => {
    const root = makeWorkspace([
      { vault: 'engine', storage: { type: 'git-branch', gitdir: 'vault-repo/.git', branch: 'refs/heads/memstead/engine' } },
      { vault: 'plugin', storage: { type: 'git-branch', gitdir: 'vault-repo/.git', branch: 'refs/heads/memstead/plugin' } },
    ]);
    fixtures.push(root);
    assert.deepEqual(readFolderVaultDirs(root), []);
  });

  it('returns folder mount dirs resolved against the workspace root', () => {
    const root = makeWorkspace([
      { vault: 'engine', storage: { type: 'folder', path: 'engine' } },
      { vault: 'notes', storage: { type: 'folder', path: 'sub/notes' } },
      { vault: 'sealed', storage: { type: 'archive', path: 'x.mem' } },
    ]);
    fixtures.push(root);
    assert.deepEqual(readFolderVaultDirs(root), [
      resolve(root, 'engine'),
      resolve(root, 'sub/notes'),
    ]);
  });

  it('falls back to the vault name when a folder mount omits a path', () => {
    const root = makeWorkspace([{ vault: 'engine', storage: { type: 'folder' } }]);
    fixtures.push(root);
    assert.deepEqual(readFolderVaultDirs(root), [resolve(root, 'engine')]);
  });

  it('returns [] when mounts.json is absent', () => {
    const root = mkdtempSync(join(tmpdir(), 'memstead-nomounts-'));
    mkdirSync(join(root, '.memstead'), { recursive: true });
    writeFileSync(join(root, '.memstead', 'workspace.toml'), 'format = "test"\n');
    fixtures.push(root);
    assert.deepEqual(readFolderVaultDirs(root), []);
  });
});

describe('resolveVaultDirs', () => {
  after(cleanup);

  it('honors explicit --vault args (legacy/hand-authored configs) over walk-up', () => {
    const cfg = { mcpServers: { memstead: { args: ['--vault', './specs'] } } };
    assert.deepEqual(
      resolveVaultDirs({ cwd: '/project', mcpConfig: cfg }),
      [resolve('/project', './specs')],
    );
  });

  it('falls back to walk-up + folder mounts when no --vault arg is present', () => {
    const root = makeWorkspace([{ vault: 'engine', storage: { type: 'folder', path: 'engine' } }]);
    fixtures.push(root);
    const cfg = { mcpServers: { memstead: { command: 'sh', args: ['-c', 'cd x && exec memstead-mcp'] } } };
    assert.deepEqual(
      resolveVaultDirs({ cwd: root, mcpConfig: cfg }),
      [resolve(root, 'engine')],
    );
  });

  it('returns [] for a git-branch workspace (the real memstead layout)', () => {
    const root = makeWorkspace([
      { vault: 'engine', storage: { type: 'git-branch', gitdir: 'vault-repo/.git', branch: 'refs/heads/memstead/engine' } },
    ]);
    fixtures.push(root);
    const cfg = { mcpServers: { memstead: { command: 'sh', args: ['-c', 'cd graph && exec ../memstead-mcp'] } } };
    assert.deepEqual(resolveVaultDirs({ cwd: root, mcpConfig: cfg }), []);
  });

  it('returns [] when cwd is not inside any workspace', () => {
    const orphan = mkdtempSync(join(tmpdir(), 'memstead-orphan2-'));
    fixtures.push(orphan);
    assert.deepEqual(resolveVaultDirs({ cwd: orphan, mcpConfig: null }), []);
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
      JSON.stringify({ mounts: [{ vault: 'engine', storage: { type: 'folder', path: 'engine' } }] }),
    );
    const cfg = { mcpServers: { memstead: { command: 'sh', args: ['-c', 'cd sub-ws && exec ../memstead-mcp'] } } };
    assert.deepEqual(
      resolveVaultDirs({ cwd: projectRoot, mcpConfig: cfg }),
      [resolve(wsDir, 'engine')],
    );
  });

  it('returns [] for a git-branch workspace in a subdirectory (memstead itself)', () => {
    const projectRoot = mkdtempSync(join(tmpdir(), 'memstead-proj2-'));
    fixtures.push(projectRoot);
    const wsDir = join(projectRoot, 'graph');
    mkdirSync(join(wsDir, '.memstead', 'state'), { recursive: true });
    writeFileSync(join(wsDir, '.memstead', 'workspace.toml'), 'format = "memstead-git-branch-1"\n');
    writeFileSync(
      join(wsDir, '.memstead', 'state', 'mounts.json'),
      JSON.stringify({ mounts: [{ vault: 'engine', storage: { type: 'git-branch', gitdir: 'vault-repo/.git', branch: 'refs/heads/memstead/engine' } }] }),
    );
    const cfg = { mcpServers: { memstead: { command: 'sh', args: ['-c', 'cd graph && exec ../engine/target/release/memstead-mcp'] } } };
    assert.deepEqual(resolveVaultDirs({ cwd: projectRoot, mcpConfig: cfg }), []);
  });
});

// End-to-end: on a folder workspace the resolved dir feeds checkEditTarget and
// a real entity edit is blocked — the behavior the previous ./specs fallback lost.
describe('resolved dir blocks a real folder-vault entity edit', () => {
  after(cleanup);

  it('blocks an entity-named .md inside a resolved folder vault', () => {
    const root = makeWorkspace([{ vault: 'engine', storage: { type: 'folder', path: 'engine' } }]);
    mkdirSync(join(root, 'engine'), { recursive: true });
    fixtures.push(root);

    const cfg = { mcpServers: { memstead: { command: 'sh', args: ['-c', 'cd . && exec memstead-mcp'] } } };
    const [vaultDir] = resolveVaultDirs({ cwd: root, mcpConfig: cfg });
    assert.ok(vaultDir, 'a folder vault dir resolves');

    const entityEdit = checkEditTarget(join(vaultDir, 'cross-vault-edge.md'), vaultDir, existsSync(vaultDir));
    assert.equal(entityEdit.action, 'block');

    const readmeEdit = checkEditTarget(join(vaultDir, 'README.md'), vaultDir, existsSync(vaultDir));
    assert.equal(readmeEdit.action, 'allow');
  });
});
