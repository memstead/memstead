#!/usr/bin/env node
// PreToolUse hook for the ingest skill — blocks Read/Glob/Grep against the
// paths declared on the active ingest's `deny_paths`.
//
// The active deny list is sourced from a cache file the ENGINE writes during
// brief rendering (memstead-base `write_active_deny_file`):
//   <workspace>/.memstead.cache/ingest/active-deny-paths.json
// Shape: { ingest: <name>, deny_paths: [ ...workspace-relative globs... ] }
// Missing file or empty list → default-open (nothing blocked). The engine
// overwrites this file (remove-then-write) on every render, so the list always
// reflects the ingest whose brief was last produced — never a stale one.
//
// Deny entries are workspace-relative glob patterns (the same dialect and
// resolution root as facet scope), so both the cache file and the deny entries
// are resolved against the WORKSPACE root (`.memstead/workspace.toml`, with
// legacy fallbacks), located exactly as the engine locates it. Outside a
// workspace the deny list is meaningless, so the hook fails open (exit 0).

import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { resolve, dirname, join } from 'node:path';
import {
  checkCandidate,
  extractCandidates,
} from './deny-meta-files-utils.mjs';
import { hasWorkspaceMarker } from './workspace-resolve-utils.mjs';

const input = JSON.parse(await readStdin());
const candidates = extractCandidates(input.tool_input);
if (!candidates.length) process.exit(0);

const cwd = input.cwd || process.cwd();
const workspaceRoot = findWorkspaceDir(cwd);
if (!workspaceRoot) process.exit(0); // fail open outside a workspace

const denyPaths = loadActiveDenyPaths(workspaceRoot);
if (!denyPaths.length) process.exit(0);

for (const c of candidates) {
  const reason = checkCandidate(c, cwd, workspaceRoot, denyPaths);
  if (reason) {
    process.stdout.write(`BLOCKED: ${reason}\nPath/pattern: ${c}\n`);
    process.exit(2);
  }
}

process.exit(0);

function findUp(start, marker) {
  let dir = resolve(start);
  while (true) {
    if (existsSync(join(dir, marker))) return dir;
    const parent = dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

function loadActiveDenyPaths(workspaceRoot) {
  const cachePath = join(
    workspaceRoot,
    '.memstead.cache',
    'ingest',
    'active-deny-paths.json',
  );
  if (!existsSync(cachePath)) return [];
  try {
    const raw = JSON.parse(readFileSync(cachePath, 'utf-8'));
    if (!raw || !Array.isArray(raw.deny_paths)) return [];
    return raw.deny_paths.filter((s) => typeof s === 'string' && s.length > 0);
  } catch {
    return [];
  }
}

// Locate the workspace root — the resolution root for both the cache file and
// every deny glob. The agent's cwd may be inside the workspace (walk-up to the
// workspace marker succeeds) or at the project root with the workspace one
// level beneath (walk-up fails; fall back to a depth-1 scan from the `.git`
// parent).
function findWorkspaceDir(start) {
  return findWorkspaceDirUp(start) ?? findGraphDirBelowProjectRoot(start);
}

function findWorkspaceDirUp(start) {
  let dir = resolve(start);
  while (true) {
    if (hasWorkspaceMarker(dir)) return dir;
    const parent = dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

function findGraphDirBelowProjectRoot(start) {
  const projectRoot = findUp(start, '.git');
  if (!projectRoot) return null;
  try {
    for (const entry of readdirSync(projectRoot, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const candidate = join(projectRoot, entry.name);
      if (hasWorkspaceMarker(candidate)) return candidate;
    }
  } catch {}
  return null;
}

function readStdin() {
  return new Promise((resolveFn) => {
    let data = '';
    process.stdin.setEncoding('utf-8');
    process.stdin.on('data', (chunk) => {
      data += chunk;
    });
    process.stdin.on('end', () => resolveFn(data));
  });
}
