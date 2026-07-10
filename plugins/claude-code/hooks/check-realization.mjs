#!/usr/bin/env node
// PostToolUse hook (Write/Edit): when the edited file is anchored by an
// entity, surface a cheap notice naming those entities so the agent can
// re-check that the entity still describes the file.
//
// How it works (E3a): the engine owns provenance anchors. This hook shells
// the CLI — `memstead anchors --artifact <edited-path> --json` — and, when a
// span/file-grain anchor references that path, emits a one-line notice naming
// the referencing entity ids. Resolution stays engine-side; the hook is a thin
// subprocess call. It never mutates anything.
//
// ACCEPTED FAIL-OPEN POSTURE (deliberate): this hook must never block or error
// a Write/Edit. Every failure mode — the `memstead` binary is absent from PATH
// (the standing case for external plugin users who installed the plugin but
// never ran setup's binary install, so the hook is permanently inert by
// design), the engine is unavailable, the query times out, or the reply isn't
// parseable JSON — results in silent pass-through (no output, exit 0). A hook
// that fires on *every* edit cannot afford to be noisy or slow, so it also
// hard-caps the subprocess with a timeout and never loads any code from a
// workspace-controlled path (no dynamic module import).
//
// A backup direct-entity-edit warning (the PreToolUse guard is the primary
// gate) is preserved for folder-backed mems.

import { spawn } from 'node:child_process';
import { basename, resolve, relative } from 'node:path';
import { isEntityFilename } from './guard-entity-edit-utils.mjs';
import { resolveMemDirsFromCwd, findWorkspaceRoot } from './workspace-resolve-utils.mjs';
import { pickReferencedEntityIds, formatRealizationNotice } from './check-realization-utils.mjs';

// Hard subprocess cap. Warm invocations complete in well under this; the cap
// only bounds a pathological hang so the edit is never visibly delayed.
const ANCHOR_QUERY_TIMEOUT_MS = 2000;

const input = JSON.parse(await readStdin());

const filePath = input.tool_input?.file_path;
if (!filePath) process.exit(0);

const cwd = input.cwd || process.cwd();
const absFilePath = resolve(cwd, filePath);

// --- Backup direct-entity-edit guard (folder-backed mems only) ---
// The PreToolUse guard-entity-edit hook blocks these first; this is a
// belt-and-suspenders warning. A direct edit of an entity markdown is the one
// case we short-circuit on.
for (const memDir of resolveMemDirsFromCwd(cwd)) {
  const projectRoot = resolve(memDir, '..');
  const relToMem = relative(projectRoot, absFilePath);
  const dirName = memDir.split('/').pop() || 'specs';
  const insideMem =
    relToMem.startsWith(dirName + '/') || relToMem.startsWith(dirName + '\\');
  if (insideMem && isEntityFilename(basename(relToMem))) {
    process.stdout.write(
      `WARNING: Entity file \`${relToMem}\` was edited directly. Always use Memstead MCP tools (memstead_create, memstead_update) to mutate entities.\n`,
    );
    process.exit(0);
  }
  // A non-entity file inside the mem dir is this mem's own concern; the
  // anchor realization query below targets source artifacts, not mem files.
  if (insideMem) process.exit(0);
}

// --- Anchor realization query ---
// Resolve the workspace root the engine way (walk up for the marker), then ask
// the engine which entities anchored the edited path. Anchors store
// workspace-relative artifact paths, so we query with the same form.
const workspaceRoot = findWorkspaceRoot(cwd);
if (!workspaceRoot) process.exit(0);

const relPath = relative(workspaceRoot, absFilePath);

let reply;
try {
  reply = await queryAnchors(relPath, workspaceRoot);
} catch {
  // Fail-open: no binary / spawn error / timeout / nonzero / non-JSON.
  process.exit(0);
}

const ids = pickReferencedEntityIds(reply);
if (ids.length > 0) {
  process.stdout.write(formatRealizationNotice(relPath, ids));
}
process.exit(0);

// --- Helpers ---

// Shell `memstead anchors --artifact <relPath> --json` in the workspace root
// with a hard timeout. Resolves the parsed JSON reply, or rejects on any
// failure (the caller treats every rejection as silent pass-through). The
// binary is looked up on PATH by bare name — absent ⇒ ENOENT ⇒ reject.
function queryAnchors(relPath, cwdRoot) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn('memstead', ['anchors', '--artifact', relPath, '--json'], {
      cwd: cwdRoot,
      stdio: ['ignore', 'pipe', 'ignore'],
    });
    let out = '';
    let settled = false;
    const finish = (fn, arg) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      fn(arg);
    };
    const timer = setTimeout(() => {
      try {
        child.kill('SIGKILL');
      } catch {
        // ignore
      }
      finish(rejectPromise, new Error('memstead anchors timed out'));
    }, ANCHOR_QUERY_TIMEOUT_MS);

    child.on('error', (err) => finish(rejectPromise, err));
    child.stdout.setEncoding('utf-8');
    child.stdout.on('data', (chunk) => {
      out += chunk;
    });
    child.on('close', (code) => {
      if (code !== 0) {
        finish(rejectPromise, new Error(`memstead anchors exited ${code}`));
        return;
      }
      try {
        finish(resolvePromise, JSON.parse(out));
      } catch (err) {
        finish(rejectPromise, err);
      }
    });
  });
}

function readStdin() {
  return new Promise((resolvePromise) => {
    let data = '';
    process.stdin.setEncoding('utf-8');
    process.stdin.on('data', (chunk) => {
      data += chunk;
    });
    process.stdin.on('end', () => resolvePromise(data || '{}'));
    process.stdin.on('error', () => resolvePromise('{}'));
  });
}
