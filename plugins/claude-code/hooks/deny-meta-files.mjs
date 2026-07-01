#!/usr/bin/env node
// PreToolUse hook for the ingest skill — blocks Read/Glob/Grep against the
// paths declared on the active ingest's `deny_paths`.
//
// The active deny list is sourced from a cache file written by inject.mjs:
//   <memstead-dir>/.memstead.cache/ingest/active-deny-paths.json
// Shape: { ingest: <name>, deny_paths: [ ... ] }
// Missing file or empty list → default-open (nothing blocked).
//
// Project root is discovered by walking up from the input cwd until a `.git`
// directory is found. The cache-file location is discovered by an
// independent walk looking for the workspace marker
// (`.memstead/workspace.toml`, with legacy fallbacks). Either walk failing causes the
// hook to fail open — the deny list is project-relative and meaningless
// outside one, and the cache file is meaningless without a workspace.

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
const projectRoot = findUp(cwd, '.git');
if (!projectRoot) process.exit(0);

const denyPaths = loadActiveDenyPaths(cwd);
if (!denyPaths.length) process.exit(0);

for (const c of candidates) {
  const reason = checkCandidate(c, cwd, projectRoot, denyPaths);
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

function loadActiveDenyPaths(start) {
  const cachePath = findCachePath(start);
  if (!cachePath || !existsSync(cachePath)) return [];
  try {
    const raw = JSON.parse(readFileSync(cachePath, 'utf-8'));
    if (!raw || !Array.isArray(raw.deny_paths)) return [];
    return raw.deny_paths.filter((s) => typeof s === 'string' && s.length > 0);
  } catch {
    return [];
  }
}

// Locate `<memstead-dir>/.memstead.cache/ingest/active-deny-paths.json`. The agent's
// cwd may be inside the workspace (walk-up to the workspace marker
// succeeds) or at the project root with the workspace one level beneath
// (walk-up fails; fallback to a depth-1 scan from `.git/` parent).
function findCachePath(start) {
  const memsteadDir = findWorkspaceDirUp(start)
    ?? findGraphDirBelowProjectRoot(start);
  if (!memsteadDir) return null;
  return join(memsteadDir, '.memstead.cache', 'ingest', 'active-deny-paths.json');
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
