// Shared workspace / mem-dir resolution for the path-aware hooks
// (guard-entity-edit, guard-entity-bash, check-realization, inject-context).
//
// Engine-true: the engine locates its workspace by walking up from the
// current directory for `.memstead/workspace.toml`. These hooks mirror
// that, then read the engine-managed mount list
// (`.memstead/state/mounts.json`) to find which mems are
// *folder*-backed — the only backend whose entities
// are working-tree files a direct Write/Edit or shell command could touch.
// Git-branch mems hold entities as git blobs on per-mem branches and
// archive mems as sealed zip entries; neither has anything on disk for a
// file-path guard to protect, so they contribute no directories.
//
// This replaces the previous `--mem`-arg scan, which fell back to a
// `./specs` directory that no real workspace produces (the engine binaries
// accept no `--mem` flag and find their workspace by cwd) — leaving the
// guards fail-open on every workspace the plugin bootstraps.

import { readFileSync, existsSync } from 'node:fs';
import { resolve, join, dirname } from 'node:path';
import { findAllMemDirs } from './guard-entity-edit-utils.mjs';

const STORE_DIR = '.memstead';

/**
 * True when `dir` is a workspace root: it carries
 * `.memstead/workspace.toml`, or the pre-rebuild standalone
 * `.memstead.toml` marker some older workspaces still carry.
 * @param {string} dir
 * @returns {boolean}
 */
export function hasWorkspaceMarker(dir) {
  return (
    existsSync(join(dir, STORE_DIR, 'workspace.toml')) ||
    existsSync(join(dir, '.memstead.toml'))
  );
}

/**
 * Walk up from a starting directory for the engine's workspace marker
 * (`.memstead/workspace.toml`, see `hasWorkspaceMarker`), exactly as
 * the engine itself does.
 * @param {string} startDir
 * @returns {string|null} Absolute workspace root, or null if none found.
 */
export function findWorkspaceRoot(startDir) {
  let dir = resolve(startDir);
  while (true) {
    if (hasWorkspaceMarker(dir)) return dir;
    const parent = dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

/**
 * Absolute on-disk directories of folder-backed mems, read from the
 * engine-managed mount list. Git-branch and archive mounts are skipped —
 * their entities are not working-tree files.
 * @param {string} workspaceRoot - Absolute workspace root.
 * @returns {string[]} Absolute mem directories (possibly empty).
 */
export function readFolderMemDirs(workspaceRoot) {
  let mounts;
  try {
    const mountsPath = join(workspaceRoot, STORE_DIR, 'state', 'mounts.json');
    mounts = JSON.parse(readFileSync(mountsPath, 'utf-8'))?.mounts ?? [];
  } catch {
    return [];
  }
  const dirs = [];
  for (const m of Array.isArray(mounts) ? mounts : []) {
    if (m?.storage?.type !== 'folder') continue;
    const rel = m.storage.path ?? m.storage.dir ?? m.mem;
    if (!rel) continue;
    dirs.push(resolve(workspaceRoot, rel));
  }
  return dirs;
}

/**
 * Workspace roots named by a `cd <dir>` in an .mcp.json server launch command
 * (e.g. `sh -c "cd graph && exec …/memstead-mcp"`). The engine finds its workspace
 * by cwd, so the `cd` target IS a workspace root. Resolved against `cwd` (the
 * .mcp.json's directory). This is how a workspace that lives in a *subdirectory*
 * of the project root (the common layout) is located — a plain walk-up from the
 * hook's cwd would only climb toward the filesystem root, never descend into it.
 * @param {object|null} mcpConfig
 * @param {string} cwd
 * @returns {string[]} Absolute candidate directories (not existence-checked).
 */
export function mcpConfigCdTargets(mcpConfig, cwd) {
  const targets = [];
  const servers = mcpConfig?.mcpServers || {};
  for (const server of Object.values(servers)) {
    const parts = [server.command, ...(server.args || [])].filter((s) => typeof s === 'string');
    for (const s of parts) {
      const m = s.match(/(?:^|[\s;&|(])cd\s+("[^"]+"|'[^']+'|[^\s;&|]+)/);
      if (!m) continue;
      targets.push(resolve(cwd, m[1].replace(/^["']|["']$/g, '')));
    }
  }
  return targets;
}

/**
 * Resolve the mem directories a path-aware hook should guard.
 *   1. Explicit `--mem <path>` args in .mcp.json (legacy / hand-authored
 *      configs); resolved against cwd.
 *   2. Otherwise locate workspace roots — both `cd <dir>` launch targets in
 *      .mcp.json (workspaces living in a subdirectory) and a walk-up from cwd
 *      (hooks invoked from inside the workspace) — and return the union of
 *      folder-backed mem dirs from each workspace's engine mount list.
 * Returns absolute paths. Empty when no workspace resolves or no workspace has
 * folder-backed mems (e.g. a git-branch workspace, where there are no
 * working-tree entity files to guard).
 * @param {{ cwd?: string, mcpConfig?: object|null }} [opts]
 * @returns {string[]}
 */
export function resolveMemDirs({ cwd = process.cwd(), mcpConfig = null } = {}) {
  const explicit = findAllMemDirs(mcpConfig);
  if (explicit.length > 0) return explicit.map((p) => resolve(cwd, p));

  const roots = new Set();
  for (const dir of mcpConfigCdTargets(mcpConfig, cwd)) {
    if (hasWorkspaceMarker(dir)) roots.add(dir);
  }
  const walked = findWorkspaceRoot(cwd);
  if (walked) roots.add(walked);

  const dirs = [];
  for (const root of roots) {
    for (const d of readFolderMemDirs(root)) {
      if (!dirs.includes(d)) dirs.push(d);
    }
  }
  return dirs;
}

/**
 * Convenience for hooks: read `.mcp.json` from cwd (best-effort) and resolve.
 * @param {string} [cwd]
 * @returns {string[]}
 */
export function resolveMemDirsFromCwd(cwd = process.cwd()) {
  let mcpConfig = null;
  try {
    mcpConfig = JSON.parse(readFileSync(join(cwd, '.mcp.json'), 'utf-8'));
  } catch { /* no/.malformed .mcp.json — walk-up resolution still applies */ }
  return resolveMemDirs({ cwd, mcpConfig });
}
