#!/usr/bin/env node
// PreToolUse hook for the ingest skill — blocks Read/Glob/Grep against the
// active binding's `deny_paths`, by asking the ENGINE:
//
//   memstead --json projection check-path --batch
//
// One deny dialect, one implementation: the engine answers with the same
// globset machinery its enumeration path uses (plus the directory-prefix
// rule), reading the active binding's record fresh on every call — this hook
// is a thin subprocess caller and holds no dialect knowledge. It replaces a
// 167-line JavaScript re-implementation of the engine's glob semantics that
// enforced an engine-written deny-list cache; with the list read at check
// time, a stale list can no longer be enforced by construction.
//
// Fail-open cases (exit 0, nothing blocked) — an unanswerable check never
// blocks work, matching the retired cache's missing-file semantics:
//   - cwd resolves to no workspace
//   - the check refuses typed (no active binding, unknown or quarantined
//     binding, workspace not initialised)
//   - the memstead binary is unavailable (a stderr note names the loss)

import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync } from 'node:fs';
import { resolve, dirname, join } from 'node:path';
import {
  findWorkspaceRoot,
  hasWorkspaceMarker,
} from './workspace-resolve-utils.mjs';

const MEMSTEAD_BIN = process.env.MEMSTEAD_BIN || 'memstead';

const input = JSON.parse(await readStdin());
const candidates = extractCandidates(input.tool_input);
if (!candidates.length) process.exit(0);

const cwd = input.cwd || process.cwd();
const workspaceRoot = findWorkspaceDir(cwd);
if (!workspaceRoot) process.exit(0); // fail open outside a workspace

// The engine resolves the workspace from its own cwd by ancestor walk, so the
// check runs FROM the workspace root (this hook's resolution also covers the
// agent-at-project-root shape the walk cannot see); candidates still resolve
// against the agent's cwd, carried in the batch payload.
const res = spawnSync(
  MEMSTEAD_BIN,
  ['--json', 'projection', 'check-path', '--batch'],
  {
    cwd: workspaceRoot,
    input: JSON.stringify({ cwd, paths: candidates }),
    encoding: 'utf-8',
  },
);
if (res.error) {
  process.stderr.write(
    `deny hook: ${MEMSTEAD_BIN} unavailable (${res.error.message}) — ` +
      'deny enforcement inactive\n',
  );
  process.exit(0);
}
if (res.status !== 0) process.exit(0); // typed refusal → fail open

let payload;
try {
  payload = JSON.parse(res.stdout);
} catch {
  process.exit(0);
}

const denied = (payload.results ?? []).find((r) => r && r.denied);
if (denied) {
  // stderr, not stdout: Claude Code's exit-2 hook contract feeds STDERR back
  // to the agent as the block reason — a message on stdout is dropped and the
  // agent sees an empty "hook error".
  process.stderr.write(
    `BLOCKED: ${denied.matched} is hidden from the ingest agent by this ` +
      `ingest's deny_paths.\nPath/pattern: ${denied.path}\n`,
  );
  process.exit(2);
}

process.exit(0);

// Pull every path-like field out of a tool_input object. Read uses
// `file_path`; Glob uses `pattern` + optional `path`; Grep uses `pattern`
// (regex) + optional `path` + optional `glob`. All of them are candidates —
// non-path strings (e.g. a regex like "TODO") resolve to harmless paths and
// never match a deny list.
function extractCandidates(toolInput) {
  if (!toolInput || typeof toolInput !== 'object') return [];
  const fields = ['file_path', 'pattern', 'path', 'glob'];
  return fields
    .map((f) => toolInput[f])
    .filter((v) => typeof v === 'string' && v.length > 0);
}

function findUp(start, marker) {
  let dir = resolve(start);
  while (true) {
    if (existsSync(join(dir, marker))) return dir;
    const parent = dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

// Locate the workspace root. The agent's cwd may be inside the workspace
// (walk-up to the workspace marker succeeds) or at the project root with the
// workspace one level beneath (walk-up fails; fall back to a depth-1 scan
// from the `.git` parent).
function findWorkspaceDir(start) {
  return findWorkspaceRoot(start) ?? findGraphDirBelowProjectRoot(start);
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
