#!/usr/bin/env node
/**
 * inject.mjs — context assembler for /memstead:ingest.
 *
 * Picks the next ingest from the workspace (round-robin with backoff,
 * unless a specific name is passed), assembles the agent prompt, and
 * exits. The plugin assembles prompts; the agent does the work via
 * Claude Code's tool-use loop.
 *
 * Modes (declared per ingest in `<ingests_dir>/<name>.json`):
 *   discovery (default)   — sources → destination, no fixed cycle.
 *   refinement            — scout/writer cycle with batched source review.
 *   one-shot              — runs once per trigger; lens routing + report.
 *
 * Discovery- and refinement-mode ingests get a paired *process mem*
 * pinned to `ingest@0.1.0` — auto-created on first run, cleared by
 * `/memstead:ingest --clear <ingest-name>`. One-shot ingests skip the
 * process mem by design.
 *
 * The skill's prompt is a *situation brief*, not a procedure. It carries
 * what the schema cannot (loop semantics, mode, paired mem) and routes
 * the agent to the schema for goal + avoid; tool-use mechanics come
 * from the MCP tool descriptions Claude Code already injects.
 *
 * Always exits 0 — failure modes surface as inline notes in the prompt
 * so the run still produces useful work.
 */

import { readFileSync, writeFileSync, existsSync, readdirSync, unlinkSync, mkdirSync, statSync, renameSync } from 'node:fs';
import { resolve, join, dirname, relative } from 'node:path';
import { globSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { loadWorkspace } from './workspace-loader.mjs';
import {
  resolveWritingGuidance,
  renderResolvedGuidance,
} from '../../lib/writing-guidance.mjs';
import {
  computeStatMap,
  digestStatMap,
  serializeDigestToken,
  parseDigestToken,
  digestsEqual,
  diffStatMaps,
} from './change-detection.mjs';

// ── Workspace root discovery ────────────────────────────────────────────────

/**
 * Walk up from a starting directory looking for a workspace root.
 * Recognises three shapes: a `.memstead.toml` directly; a `.mcp.json` whose
 * memstead MCP server arg list carries a `--config <path>.toml`; or a `.mcp.json`
 * whose server launch command `cd`s into the workspace before exec'ing
 * the engine. The engine binary has no `--config` flag — it locates its
 * workspace by cwd — so the `cd <dir>` form is what real configs use.
 */
function findWorkspaceRoot(startDir) {
  let dir = resolve(startDir);
  while (true) {
    if (existsSync(join(dir, '.memstead.toml'))) return dir;

    const mcp = join(dir, '.mcp.json');
    if (existsSync(mcp)) {
      try {
        const cfg = JSON.parse(readFileSync(mcp, 'utf-8'));
        const servers = cfg?.mcpServers || {};
        for (const server of Object.values(servers)) {
          const argList = server.args || [];

          // Form A: explicit `--config <path>.toml` arg.
          for (let i = 0; i < argList.length; i++) {
            if (argList[i] === '--config' && argList[i + 1]) {
              const cfgPath = resolve(dir, argList[i + 1]);
              if (existsSync(cfgPath)) return dirname(cfgPath);
            }
          }

          // Form B: a `cd <dir>` in the launch command (e.g.
          // `sh -c "cd graph && exec …/memstead-mcp"`). The engine has no
          // --config flag and finds its workspace by cwd, so the `cd`
          // target IS the workspace root. Resolve it relative to the
          // .mcp.json's dir and accept it when it holds a `.memstead.toml`.
          const haystack = [server.command, ...argList].filter(s => typeof s === 'string');
          for (const s of haystack) {
            const m = s.match(/(?:^|[\s;&|(])cd\s+("[^"]+"|'[^']+'|[^\s;&|]+)/);
            if (!m) continue;
            const target = m[1].replace(/^["']|["']$/g, '');
            const candidate = resolve(dir, target);
            if (existsSync(join(candidate, '.memstead.toml'))) return candidate;
          }
        }
      } catch {}
    }

    const parent = dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

function resolveWorkspaceRoot() {
  const fromCwd = findWorkspaceRoot(process.cwd());
  if (fromCwd) return fromCwd;
  const fallbackRoot = process.env.CLAUDE_SKILL_DIR ?? dirname(new URL(import.meta.url).pathname);
  return findWorkspaceRoot(resolve(fallbackRoot));
}

// ── Args ────────────────────────────────────────────────────────────────────

const args = process.argv.slice(2);
const positional = args.filter(a => !a.startsWith('--'));

const clearMode = args.includes('--clear');
const allMode = args.includes('--all') || (!clearMode && positional.length === 0);
const nameArg = positional.join(' ').trim();

const STATE_PREFIX = 'ingest';
const PROCESS_MEM_PATH = 'ingest';
const PROCESS_MEM_SCHEMA = 'ingest@0.1.0';
const MAX_SKIP_LEVEL = 10;

// Dry-run mode. Reads happen normally; cache writes, prompt capture
// snapshots, and the auto-create / --clear engine calls are skipped so
// the emitted prompt is byte-for-byte what a real run would produce
// against the current workspace state.
const DRY_RUN = !!process.env.MEMSTEAD_INGEST_DRY_RUN;

// ── Debug logging (stderr) ──────────────────────────────────────────────────

const DEBUG = !process.env.MEMSTEAD_INGEST_QUIET;
function dbg(...parts) {
  if (DEBUG) process.stderr.write(`[ingest:dbg] ${parts.join(' ')}\n`);
}

// ── Workspace cache dir ─────────────────────────────────────────────────────

const WORKSPACE_ROOT = resolveWorkspaceRoot();
// `.memstead.cache/ingest/` keeps round-robin cursor, backoff, prompt
// capture, refinement batch state. `.memstead.cache/.gitignore` (`*\n`) is
// dropped on first write so cache contents never land in git.
const CACHE_DIR = WORKSPACE_ROOT ? join(WORKSPACE_ROOT, '.memstead.cache', 'ingest') : null;

function ensureCacheDir(sub = '') {
  if (!CACHE_DIR) return null;
  const target = sub ? join(CACHE_DIR, sub) : CACHE_DIR;
  if (DRY_RUN) return target;
  try {
    mkdirSync(target, { recursive: true });
    const root = join(WORKSPACE_ROOT, '.memstead.cache');
    const gi = join(root, '.gitignore');
    if (!existsSync(gi)) writeFileSync(gi, '*\n');
  } catch {}
  return target;
}

// ── Prompt capture — saves last 10 prompts to .memstead.cache/ingest/prompts/ ───

const _origStdoutWrite = process.stdout.write.bind(process.stdout);
let _promptCapture = '';
process.stdout.write = (chunk, ...rest) => {
  _promptCapture += chunk;
  return _origStdoutWrite(chunk, ...rest);
};
process.on('exit', () => {
  if (DRY_RUN) return;
  if (!CACHE_DIR || !_promptCapture) return;
  try {
    const promptDir = ensureCacheDir('prompts');
    if (!promptDir) return;
    const ts = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
    writeFileSync(join(promptDir, `${ts}.md`), _promptCapture);
    const files = readdirSync(promptDir).filter(f => f.endsWith('.md')).sort();
    while (files.length > 10) {
      try { unlinkSync(join(promptDir, files.shift())); } catch {}
    }
  } catch {}
});

// ── Cache JSON helpers ──────────────────────────────────────────────────────

function readJsonSafe(path, fallback) {
  if (!path) return fallback;
  try { return JSON.parse(readFileSync(path, 'utf-8')); }
  catch {
    if (!DRY_RUN) {
      try { unlinkSync(path); } catch {}
    }
    return fallback;
  }
}

function writeJsonAtomic(path, value) {
  if (DRY_RUN) return;
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path + '.tmp', JSON.stringify(value, null, 2) + '\n');
  renameSync(path + '.tmp', path);
}

// ── Active deny-paths channel for the deny-meta-files hook ─────────────────
//
// The PreToolUse hook at plugins/claude-code/hooks/deny-meta-files.mjs reads
// this file on every tool call to decide what to block. inject.mjs is the
// sole writer; the hook is a stateless reader. Default-open: missing or
// empty `deny_paths` means the hook permits everything.

function writeActiveDenyPaths(ingest) {
  if (DRY_RUN) return;
  if (!CACHE_DIR) return;
  ensureCacheDir();
  const path = join(CACHE_DIR, 'active-deny-paths.json');
  const payload = {
    ingest: ingest.name,
    deny_paths: Array.isArray(ingest.deny_paths) ? ingest.deny_paths : [],
  };
  try { writeJsonAtomic(path, payload); } catch {}
}

// ── Engine binary invocation (for auto-create / --clear) ────────────────────

/**
 * Run an `memstead` CLI command synchronously against the workspace root.
 * Returns `{ status, stdout, stderr, error }`. Binary is `MEMSTEAD_BIN`
 * env var, or `memstead` on PATH. Caller decides what to do on non-zero
 * exit; this helper never throws.
 */
function runMemstead(argList) {
  if (!WORKSPACE_ROOT) return { status: -1, stdout: '', stderr: '', error: new Error('no workspace root') };
  const bin = process.env.MEMSTEAD_BIN || 'memstead';
  const result = spawnSync(bin, argList, {
    cwd: WORKSPACE_ROOT,
    encoding: 'utf-8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  return {
    status: result.status ?? -1,
    stdout: result.stdout || '',
    stderr: result.stderr || '',
    error: result.error || null,
  };
}

// ── Process-mem lifecycle ─────────────────────────────────────────────────

/**
 * Has the paired process mem been registered in the engine?
 * Process-mem names use the leaf form (`<ingest-name>`) under the
 * `ingest/` org-path; `memMeta` keys are leaf names by convention.
 */
function processMemExists(ingestName, memMeta) {
  return !!memMeta?.[ingestName];
}

/**
 * Auto-create the paired process mem via the operator-mode CLI.
 * Synchronous, silent on success, surfaces a `notice` on failure so
 * the prompt can still produce useful work.
 *
 * Returns `{ ok: true }` on success or `{ ok: false, notice: string }`
 * on failure (binary missing, lifecycle gate denied, gitdir dirty …).
 */
function autoCreateProcessMem(ingestName) {
  if (DRY_RUN) {
    dbg(`[dry-run] would create process mem ingest/${ingestName}`);
    return { ok: false, notice: '(dry-run — process mem create skipped)' };
  }
  const argList = [
    'mem', 'init', ingestName,
    '--org-path', PROCESS_MEM_PATH,
    '--schema', PROCESS_MEM_SCHEMA,
    '--note', `auto-created by /memstead:ingest for ingest "${ingestName}"`,
  ];
  const r = runMemstead(argList);
  if (r.error) {
    return { ok: false, notice: `process mem auto-create failed: ${r.error.message}` };
  }
  if (r.status !== 0) {
    const tail = r.stderr.split('\n').filter(Boolean).slice(-2).join(' / ').trim();
    return { ok: false, notice: `process mem auto-create exited ${r.status}: ${tail || '(no detail)'}` };
  }
  dbg(`created process mem ingest/${ingestName}`);
  return { ok: true };
}

/**
 * Delete the paired process mem. Used by `--clear <ingest-name>`.
 * Idempotent on already-deleted mems — a "unknown writable mem"
 * error from the CLI is treated as success.
 */
function deleteProcessMem(ingestName) {
  const argList = [
    'mem', 'delete', ingestName,
    '--note', `cleared by /memstead:ingest --clear`,
  ];
  const r = runMemstead(argList);
  if (r.error) {
    return { ok: false, notice: `clear failed: ${r.error.message}`, alreadyAbsent: false };
  }
  if (r.status !== 0) {
    const stderr = r.stderr || '';
    if (/unknown writable mem/i.test(stderr)) {
      return { ok: true, alreadyAbsent: true };
    }
    const tail = stderr.split('\n').filter(Boolean).slice(-2).join(' / ').trim();
    return { ok: false, notice: `clear exited ${r.status}: ${tail || '(no detail)'}`, alreadyAbsent: false };
  }
  return { ok: true, alreadyAbsent: false };
}

// ── --clear handler ─────────────────────────────────────────────────────────
//
// Per-ingest only. `--clear` without a name is a usage error. `--clear`
// against a non-existent ingest config is treated by the engine as
// "unknown writable mem" — we report it as already-absent and exit 0.

if (clearMode) {
  if (!nameArg) {
    process.stdout.write(`> **[${STATE_PREFIX} | clear] Usage: /memstead:ingest --clear <ingest-name>** (per-ingest; no global form).\n`);
    process.exit(0);
  }
  const result = deleteProcessMem(nameArg);
  if (result.alreadyAbsent) {
    process.stdout.write(`[${STATE_PREFIX} | clear] ingest/${nameArg} — already absent.\n`);
  } else if (result.ok) {
    process.stdout.write(`[${STATE_PREFIX} | clear] ingest/${nameArg} — deleted.\n`);
  } else {
    process.stdout.write(`> **[${STATE_PREFIX} | clear] ${result.notice}**\n`);
  }
  process.exit(0);
}

// ── Round-robin cursor keyed by ingest filename ─────────────────────────────

function nextIngest(eligible) {
  if (!eligible.length) return null;
  ensureCacheDir();
  const roundFile = join(CACHE_DIR, `${STATE_PREFIX}-cursor.json`);
  const state = readJsonSafe(roundFile, {});
  const names = eligible.map(i => i.name);
  const start = state.last ? names.indexOf(state.last) : -1;
  const pick = names[(start + 1) % names.length];
  try { writeJsonAtomic(roundFile, { last: pick }); } catch {}
  return eligible.find(i => i.name === pick);
}

function markOneShotRan(name) {
  if (!CACHE_DIR) return;
  ensureCacheDir();
  const runsFile = join(CACHE_DIR, `${STATE_PREFIX}-one-shot-runs.json`);
  const runs = readJsonSafe(runsFile, {});
  runs[name] = new Date().toISOString();
  try { writeJsonAtomic(runsFile, runs); } catch {}
}

function shuffle(arr) {
  for (let i = arr.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [arr[i], arr[j]] = [arr[j], arr[i]];
  }
  return arr;
}

// ── Medium resolution for prompt assembly ───────────────────────────────────

const MEDIUMS = JSON.parse(readFileSync(new URL('../mediums.json', import.meta.url), 'utf8'));

function sourceMediumType(src) {
  if (src?.facet?.mediumType) return src.facet.mediumType;
  if (src?.mem) return 'graph';
  return 'codebase';
}

function destinationMediumType(_dst) {
  return 'graph';
}

function resolveMediumTerms(ingest) {
  const srcMediums = [...new Set((ingest.sources || []).map(sourceMediumType).filter(Boolean))];
  const primarySrc = srcMediums[0] || 'codebase';
  const dstKey = destinationMediumType(ingest.destinations?.[0]);
  const src = MEDIUMS.source[primarySrc] || MEDIUMS.source.codebase;
  const dst = MEDIUMS.destination[dstKey] || MEDIUMS.destination.graph;
  return {
    'source.artifact': src.artifact,
    'source.artifacts': src.artifacts,
    'destination.artifact': dst.artifact,
    'destination.artifacts': dst.artifacts,
  };
}

// ── Source-file enumeration (codebase/filesystem facets) ────────────────────

// Enumerate the workspace-relative files a single codebase/filesystem
// source facet selects (allow globs minus deny globs). Returns [] for
// non-file media or a facet with no allow rules.
function enumerateFacetFiles(src) {
  if (!WORKSPACE_ROOT) return [];
  const type = sourceMediumType(src);
  if (type !== 'codebase' && type !== 'filesystem') return [];
  const allows = [];
  const denies = [];
  for (const rule of src.facet?.scope?.tree || []) {
    if (rule.mode === 'allow') allows.push(rule.path);
    else if (rule.mode === 'deny') denies.push(rule.path);
  }
  if (!allows.length) return [];
  const matched = [...new Set(allows.flatMap(p => globSync(p, { cwd: WORKSPACE_ROOT })))];
  const denySet = denies.length
    ? new Set(denies.flatMap(p => globSync(p, { cwd: WORKSPACE_ROOT })))
    : new Set();
  const filtered = denySet.size ? matched.filter(f => !denySet.has(f)) : matched;
  return filtered.sort();
}

function enumerateSourceFiles(ingest) {
  if (!WORKSPACE_ROOT) return [];
  const files = [];
  for (const src of (ingest.sources || [])) {
    files.push(...enumerateFacetFiles(src));
  }
  return [...new Set(files)].sort();
}

// ── Source-change cursor (mtime/filesystem strategy) ────────────────────────
//
// For a source facet without a git work tree, "what changed since the last
// synced pass" is a per-file `{mtime,size}` stat-map diff. The durable
// baseline is a small digest the engine persists per `<ingest>/<facet>`
// (surfaced on the dump as `syncState`); the full prior map is a
// skill-cache memo keyed by digest, used to compute the precise slice.
// On memo miss, detection still fires from the digest but the slice
// degrades to a one-tick full scan.
//
// The cursor *advances* only when the agent records the new baseline via
// `memstead mem set-sync-state` after a complete pass (see the emitted
// instruction). inject.mjs never advances it — so an interrupted pass
// leaves the baseline put and the slice is re-presented next run.

const CURSOR_DIR_NAME = 'source-cursor';
const SLICE_CAP = 25; // per-class cap in the rendered preface

// Resolve a facet's change-detection strategy. A graph-typed source
// always uses the `graph` strategy (its signal is the source mem's
// snapshot token, which the engine provides). Otherwise the declared
// `change_detection` wins; `auto` (the default) probes for a git work
// tree over the medium pointer: present → `git`, absent → `mtime`.
// `git`, `mtime`, and `graph` all produce a changed slice; `none` is
// inert.
function resolveChangeDetection(src) {
  const type = sourceMediumType(src);
  if (type === 'graph') return 'graph';
  const declared = src.facet?.changeDetection || 'auto';
  if (declared === 'none') return 'none';
  if (declared === 'git') return 'git';
  if (declared === 'mtime') return 'mtime';
  // auto: probe for a git work tree over the pointer.
  const pointer = src.facet?.mediumPointer || '';
  const base = pointer ? resolve(WORKSPACE_ROOT, pointer) : WORKSPACE_ROOT;
  return hasGitWorkTree(base) ? 'git' : 'mtime';
}

// Walk up from `startDir` looking for a `.git` entry (dir or file).
// Returns the directory containing it (the git work-tree root), or null.
// Pure fs, no subprocess — deterministic for tests.
function findGitRoot(startDir) {
  let dir = startDir;
  for (let i = 0; i < 64; i++) {
    if (existsSync(join(dir, '.git'))) return dir;
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return null;
}

function hasGitWorkTree(startDir) {
  return findGitRoot(startDir) !== null;
}

// ── git change-detection helpers ────────────────────────────────────────────
//
// For a source under a git work tree, "what changed since the last synced
// pass" is `git diff` between the stored commit id (the baseline token,
// stored verbatim — opaque to the engine) and the current HEAD. Facet
// scope (allow/deny) and the ingest's `deny_paths` are pushed down into
// git as `:(glob)` pathspecs, so git itself scopes the diff — including
// *deleted* paths, which no longer exist on disk and so can't be matched
// by enumerating the work tree.

// A git token is a hex commit id (short or full). An mtime digest token
// (JSON) or any other shape is not — those are treated as "no usable
// baseline" so a strategy switch degrades gracefully.
function isGitToken(s) {
  return typeof s === 'string' && /^[0-9a-f]{7,64}$/i.test(s);
}

// Read HEAD of the work tree at `gitRoot`, or null on any failure
// (not a repo, git absent, detached/empty). Never throws.
function gitHead(gitRoot) {
  if (!gitRoot) return null;
  try {
    const r = spawnSync('git', ['rev-parse', 'HEAD'], {
      cwd: gitRoot, encoding: 'utf-8', stdio: ['ignore', 'pipe', 'ignore'],
    });
    if (r.status !== 0) return null;
    const sha = (r.stdout || '').trim();
    return isGitToken(sha) ? sha : null;
  } catch { return null; }
}

// Translate a workspace-relative facet pattern into a git pathspec
// relative to `gitRoot`, with `:(glob)` magic so `**`/`*` mean what the
// facet scope means. `exclude` flips it into a negative pathspec.
function toGitPathspec(pattern, gitRoot, exclude) {
  const gitRel = relative(gitRoot, resolve(WORKSPACE_ROOT, pattern));
  const magic = exclude ? ':(glob,exclude)' : ':(glob)';
  return `${magic}${gitRel}`;
}

// Compute the git changed slice for one source facet between
// `baselineToken` and the work tree's current HEAD. Returns:
//   { reseed: true, token }           — no usable baseline; seed at HEAD
//   { changed: false, token }         — baseline === HEAD; nothing moved
//   { changed: true, token, slice }   — added/modified/deleted (ws-relative)
//   null                              — no git signal available (degrade)
function computeGitSlice(ingest, src, baselineToken) {
  const pointer = src.facet?.mediumPointer || '';
  const base = pointer ? resolve(WORKSPACE_ROOT, pointer) : WORKSPACE_ROOT;
  const gitRoot = findGitRoot(base);
  if (!gitRoot) return null;
  const head = gitHead(gitRoot);
  if (!head) return null;

  if (!isGitToken(baselineToken)) return { reseed: true, token: head };
  if (baselineToken === head) return { changed: false, token: head };

  // Build pathspecs from the facet scope + the ingest's deny_paths.
  const allows = [];
  const denies = [];
  for (const rule of src.facet?.scope?.tree || []) {
    if (rule.mode === 'allow') allows.push(rule.path);
    else if (rule.mode === 'deny') denies.push(rule.path);
  }
  if (!allows.length) return null; // unscoped — refuse to diff the whole repo
  for (const dp of (ingest.deny_paths || [])) denies.push(dp);

  const specs = [
    ...allows.map(p => toGitPathspec(p, gitRoot, false)),
    ...denies.map(p => toGitPathspec(p, gitRoot, true)),
  ];

  let out;
  try {
    const r = spawnSync('git', [
      'diff', '--no-renames', '--name-status', baselineToken, head, '--', ...specs,
    ], { cwd: gitRoot, encoding: 'utf-8', stdio: ['ignore', 'pipe', 'ignore'] });
    if (r.status !== 0) return null; // unknown baseline (gc'd / rewritten): degrade
    out = r.stdout || '';
  } catch { return null; }

  const added = [], modified = [], deleted = [];
  for (const line of out.split('\n')) {
    if (!line.trim()) continue;
    const tab = line.indexOf('\t');
    if (tab < 0) continue;
    const status = line.slice(0, tab).trim();
    const gitPath = line.slice(tab + 1).trim();
    // Present paths as workspace-relative, matching the rest of the prompt.
    const wsPath = relative(WORKSPACE_ROOT, join(gitRoot, gitPath));
    const code = status[0];
    if (code === 'A') added.push(wsPath);
    else if (code === 'D') deleted.push(wsPath);
    else modified.push(wsPath); // M, T (type change), C, and the rest
  }
  return { changed: true, token: head, slice: { added: added.sort(), modified: modified.sort(), deleted: deleted.sort() } };
}

// ── graph change-detection ──────────────────────────────────────────────────
//
// A graph-typed source is another mem. Its reliable change signal is the
// engine's own surfaces — no new engine work: the baseline is the source
// mem's `snapshot_token` (already on the dump), and the changed set is
// `memstead changes --mem <src> --since <baseline>` (the same entity
// delta the MCP `memstead_changes_since` tool returns). The slice is
// entity ids, not file paths.

// Compute the graph changed slice for one source facet whose medium type
// is `graph` (the medium pointer is the source mem id). Same return
// contract as `computeGitSlice`.
function computeGraphSlice(src, baselineToken, memMeta) {
  const srcMem = src.facet?.mediumPointer || src.mem || '';
  if (!srcMem) return null;
  const current = memMeta?.[srcMem]?.snapshotToken;
  if (!isGitToken(current)) return null; // source has no snapshot signal

  if (!isGitToken(baselineToken)) return { reseed: true, token: current };
  if (baselineToken === current) return { changed: false, token: current };

  const r = runMemstead(['changes', '--mem', srcMem, '--since', baselineToken, '--json', '--quiet']);
  if (!r || r.status !== 0) return null; // engine unavailable / unknown baseline: degrade
  let parsed;
  try { parsed = JSON.parse(r.stdout); } catch { return null; }
  const changes = Array.isArray(parsed?.changes) ? parsed.changes : [];

  const added = [], modified = [], deleted = [];
  for (const c of changes) {
    switch (c?.action) {
      case 'added': if (c.id) added.push(c.id); break;
      case 'removed': if (c.id) deleted.push(c.id); break;
      case 'renamed':
        if (c.to_id) added.push(c.to_id);
        if (c.from_id) deleted.push(c.from_id);
        break;
      default: if (c?.id) modified.push(c.id); // updated + anything else
    }
  }
  return {
    changed: true,
    token: current,
    slice: { added: added.sort(), modified: modified.sort(), deleted: deleted.sort() },
  };
}

function cursorMemoPath(ingestName, facetRef) {
  if (!CACHE_DIR) return null;
  // Facet refs are simple names; sanitise defensively for the filename.
  const safe = String(facetRef).replace(/[^A-Za-z0-9_.-]/g, '_');
  return join(CACHE_DIR, CURSOR_DIR_NAME, ingestName, `${safe}.json`);
}

// Read the digest→map memo for a facet, returning the map stored under
// `aggregate` (or null on miss).
function readCursorMemo(ingestName, facetRef, aggregate) {
  const p = cursorMemoPath(ingestName, facetRef);
  if (!p) return null;
  const memo = readJsonSafe(p, null);
  if (!memo || typeof memo !== 'object') return null;
  const m = memo[aggregate];
  return (m && typeof m === 'object') ? m : null;
}

// Memoize the current map under its aggregate, bounding the file to the
// 3 most-recent aggregates. No-op under dry-run (byte-exact prompt, no
// cache mutation).
function writeCursorMemo(ingestName, facetRef, aggregate, statMap) {
  if (DRY_RUN) return;
  const p = cursorMemoPath(ingestName, facetRef);
  if (!p) return;
  const memo = readJsonSafe(p, {}) || {};
  memo[aggregate] = statMap;
  const aggs = Object.keys(memo);
  if (aggs.length > 3) {
    // Drop oldest insertion-order keys beyond the cap.
    for (const k of aggs.slice(0, aggs.length - 3)) delete memo[k];
  }
  try { writeJsonAtomic(p, memo); } catch {}
}

// Compute the source-change cursor for an ingest against its destination
// mem's engine-held baseline. Returns the union changed slice across
// mtime facets, the per-facet new tokens (for the cursor-write
// instruction), and any facets that had no baseline (re-seed).
function computeSourceCursor(ingest, destMem, memMeta) {
  const baselineMap = (destMem && memMeta?.[destMem]?.syncState) || {};
  const union = { added: [], modified: [], deleted: [] };
  const writeCommands = []; // {key, token} — advance after a complete pass
  const reseed = [];        // {key, token} — first sync, no prior baseline
  let degraded = false;

  const addSlice = (slice) => {
    union.added.push(...slice.added);
    union.modified.push(...slice.modified);
    union.deleted.push(...slice.deleted);
  };

  for (const src of (ingest.sources || [])) {
    const strategy = resolveChangeDetection(src);
    const facetRef = src.facet_ref || src.mem || 'source';
    const key = `${ingest.name}/${facetRef}`;

    if (strategy === 'git' || strategy === 'graph') {
      // git diffs the stored commit id against HEAD; graph diffs the
      // source mem's snapshot token via `memstead changes`. Both reuse
      // an existing history store, so neither needs the skill-cache memo.
      const g = strategy === 'git'
        ? computeGitSlice(ingest, src, baselineMap[key])
        : computeGraphSlice(src, baselineMap[key], memMeta);
      if (!g) continue;            // no reliable signal — degrade to today
      if (g.reseed) { reseed.push({ key, token: g.token }); continue; }
      if (!g.changed) continue;    // baseline === current source state
      addSlice(g.slice);
      writeCommands.push({ key, token: g.token });
      continue;
    }

    if (strategy !== 'mtime') continue; // none: no slice

    const files = enumerateFacetFiles(src);
    const nowMap = computeStatMap(files, WORKSPACE_ROOT);
    const nowDigest = digestStatMap(nowMap);
    const nowToken = serializeDigestToken(nowDigest);

    // Memoise the current map so a future run whose baseline == this
    // digest can diff precisely.
    writeCursorMemo(ingest.name, facetRef, nowDigest.aggregate, nowMap);

    const baseDigest = parseDigestToken(baselineMap[key]);
    if (!baseDigest) {
      // No usable baseline (first sync, or an unrecognized token shape):
      // seed at the current state, no priority slice this pass.
      reseed.push({ key, token: nowToken });
      continue;
    }
    if (digestsEqual(baseDigest, nowDigest)) continue; // unchanged

    // Changed. Diff against the memoised prior map if available; else
    // degrade to a one-tick full scan (every file as "changed").
    const prevMap = readCursorMemo(ingest.name, facetRef, baseDigest.aggregate);
    let slice;
    if (prevMap) {
      slice = diffStatMaps(prevMap, nowMap);
    } else {
      degraded = true;
      slice = { added: files.slice(), modified: [], deleted: [] };
    }
    addSlice(slice);
    writeCommands.push({ key, token: nowToken });
  }

  const dedupeSort = (a) => [...new Set(a)].sort();
  union.added = dedupeSort(union.added);
  union.modified = dedupeSort(union.modified);
  union.deleted = dedupeSort(union.deleted);
  const anyChanges = union.added.length || union.modified.length || union.deleted.length;
  return { union, writeCommands, reseed, anyChanges, degraded, destMem };
}

// Cheap, side-effect-free "did the source move since the baseline?"
// predicate — the *additive* second trigger beside the destination-
// snapshot backoff. Compares the current source token to the engine-held
// baseline without computing the full slice or touching the cache, so it
// can run per-candidate in the round-robin loop. A first sync (no usable
// baseline) is NOT "changed" — it does not defeat backoff on its own.
function sourceChangedSince(ingest, destMem, memMeta) {
  const baselineMap = (destMem && memMeta?.[destMem]?.syncState) || {};
  for (const src of (ingest.sources || [])) {
    const facetRef = src.facet_ref || src.mem || 'source';
    const key = `${ingest.name}/${facetRef}`;
    const baseline = baselineMap[key];
    if (baseline === undefined) continue; // no baseline ⇒ not "changed"

    const strategy = resolveChangeDetection(src);
    if (strategy === 'git') {
      if (!isGitToken(baseline)) continue;
      const pointer = src.facet?.mediumPointer || '';
      const base = pointer ? resolve(WORKSPACE_ROOT, pointer) : WORKSPACE_ROOT;
      const head = gitHead(findGitRoot(base));
      if (head && head !== baseline) return true;
    } else if (strategy === 'graph') {
      if (!isGitToken(baseline)) continue;
      const srcMem = src.facet?.mediumPointer || src.mem || '';
      const current = memMeta?.[srcMem]?.snapshotToken;
      if (isGitToken(current) && current !== baseline) return true;
    } else if (strategy === 'mtime') {
      const baseDigest = parseDigestToken(baseline);
      if (!baseDigest) continue;
      const nowDigest = digestStatMap(computeStatMap(enumerateFacetFiles(src), WORKSPACE_ROOT));
      if (!digestsEqual(baseDigest, nowDigest)) return true;
    }
  }
  return false;
}

// Render the changed-slice preface. Empty string when there is nothing
// to steer at (no changes, no re-seed) — the prompt is then byte-identical
// to today's roam/shuffle. Carries no coverage/progress figure: it states
// *what changed*, never *how far along*.
function changedSliceBlock(cursor) {
  if (!cursor) return '';
  const { union, writeCommands, reseed, anyChanges, degraded, destMem } = cursor;
  if (!anyChanges && !reseed.length) return '';

  const lines = [];
  lines.push('## Source changes since the last sync\n');

  if (anyChanges) {
    lines.push(
      'The source moved since this graph was last synced. Steer this pass at these changed artifacts **first** — they are where the graph is most likely now wrong.\n'
    );
    const renderClass = (label, arr) => {
      if (!arr.length) return;
      const shown = arr.slice(0, SLICE_CAP);
      lines.push(`**${label}:**`);
      for (const f of shown) lines.push(`- \`${f}\``);
      if (arr.length > shown.length) lines.push(`- …and ${arr.length - shown.length} more ${label.toLowerCase()}`);
      lines.push('');
    };
    // Deletions first — cheapest, highest-signal drift (an entity may
    // still claim a source artifact that no longer exists).
    renderClass('Deleted', union.deleted);
    renderClass('Modified', union.modified);
    renderClass('Added', union.added);
    if (degraded) {
      lines.push(
        '_(Precise change history for one or more facets was unavailable, so its full current file set is listed above. Detection still fired from the durable baseline; targeting is coarser this pass only.)_\n'
      );
    }
  }

  if (reseed.length) {
    lines.push(
      `No prior sync baseline exists for ${reseed.map(r => `\`${r.key}\``).join(', ')} — treating the current source state as the baseline (first sync). No priority slice from ${reseed.length === 1 ? 'it' : 'them'} this pass; proceed as usual.\n`
    );
  }

  // Cursor-write instruction. The agent records the new baseline as the
  // FINAL step, so an interrupted pass leaves the baseline unchanged and
  // the slice is re-presented next run. Routed through the engine CLI —
  // never a raw mem-repo write.
  const allCommands = [...writeCommands, ...reseed];
  if (allCommands.length) {
    lines.push('### Recording the new baseline (do this LAST)\n');
    lines.push(
      'Only after you have fully worked the changed artifacts above — and only if this pass was not cut short — record the source state you synced against, so the next pass targets just what changes next. Run, exactly once each:\n'
    );
    lines.push('```sh');
    for (const c of allCommands) {
      lines.push(`memstead mem set-sync-state ${destMem} ${shellQuote(c.key)} ${shellQuote(c.token)}`);
    }
    lines.push('```');
    lines.push(
      'If you were interrupted before finishing, skip this — leaving the baseline where it is re-presents the same slice next run.\n'
    );
  }

  return lines.join('\n') + '\n';
}

// Single-quote a value for the emitted shell command, escaping embedded
// single quotes. The digest token is JSON (contains `"` and `:`), so it
// must be quoted to survive the shell.
function shellQuote(s) {
  return `'${String(s).replace(/'/g, `'\\''`)}'`;
}

// ── Refinement batch state ──────────────────────────────────────────────────

const REF_DIR_NAME = 'refinement';
const FINDINGS_STALE = 10 * 60 * 1000; // 10 minutes

function refinementDir() { return CACHE_DIR ? join(CACHE_DIR, REF_DIR_NAME) : null; }

function loadBatchState(ingestName) {
  const refDir = refinementDir();
  const stateFile = join(refDir, `${ingestName}.json`);
  return { stateFile, refDir, state: readJsonSafe(stateFile, null) };
}

function saveBatchState(stateFile, state) {
  if (DRY_RUN) return;
  mkdirSync(dirname(stateFile), { recursive: true });
  writeFileSync(stateFile, JSON.stringify(state, null, 2) + '\n');
}

function findingsPath(ingestName) {
  return join(refinementDir(), `${ingestName}-findings.md`);
}

function readPendingFindings(ingestName) {
  const fp = findingsPath(ingestName);
  if (!existsSync(fp)) return null;
  try {
    const content = readFileSync(fp, 'utf8');
    const fstat = statSync(fp);
    if (Date.now() - fstat.mtimeMs > FINDINGS_STALE) {
      dbg(`stale findings file for ${ingestName}, deleting`);
      if (!DRY_RUN) unlinkSync(fp);
      return null;
    }
    const trimmed = content.trim();
    if (!trimmed || trimmed.toLowerCase() === 'no findings.' || trimmed.toLowerCase() === 'no findings') return null;
    return trimmed;
  } catch {
    return null;
  }
}

function nextBatch(ingestName, ingest) {
  const batchSize = ingest.batch_size || 20;
  const allFiles = enumerateSourceFiles(ingest);
  if (!allFiles.length) return null;

  let { stateFile, state } = loadBatchState(ingestName);

  if (!state || !state.file_order || state.cursor >= state.file_order.length) {
    const rotation = (state?.rotation || 0) + (state?.file_order ? 1 : 0);
    state = {
      ingest: ingestName,
      rotation,
      cursor: 0,
      file_order: shuffle([...allFiles]),
    };
    dbg(`new rotation ${rotation} for ${ingestName} (${allFiles.length} files)`);
  }

  const files = state.file_order.slice(state.cursor, state.cursor + batchSize);
  const batchIndex = Math.floor(state.cursor / batchSize) + 1;
  const totalBatches = Math.ceil(state.file_order.length / batchSize);

  state.cursor += files.length;
  saveBatchState(stateFile, state);

  return { files, rotation: state.rotation, batchIndex, totalBatches };
}

// ── Backoff ─────────────────────────────────────────────────────────────────

function memSnapshotToken(memName, memMeta) {
  if (!memName) return '';
  const meta = memMeta?.[memName];
  if (!meta || typeof meta.snapshotToken !== 'string') return '';
  return meta.snapshotToken;
}

function loadBackoff() {
  if (!CACHE_DIR) return {};
  return readJsonSafe(join(CACHE_DIR, `${STATE_PREFIX}-backoff.json`), {});
}

function saveBackoff(state) {
  if (!CACHE_DIR) return;
  ensureCacheDir();
  writeJsonAtomic(join(CACHE_DIR, `${STATE_PREFIX}-backoff.json`), state);
}

function hasRemainingBatches(ingestName) {
  const { state } = loadBatchState(ingestName);
  if (!state || !state.file_order) return false;
  return state.cursor < state.file_order.length;
}

function shouldSkip(ingest, memName, memMeta) {
  if (ingest.mode === 'one-shot') return false;
  if (ingest.mode === 'refinement' && hasRemainingBatches(ingest.name)) return false;

  // Additive source-side trigger: when the source moved since the last
  // synced pass, run even if the destination snapshot is unchanged — the
  // drift case the destination-only backoff would otherwise sleep
  // through. Never a hard failure: any error falls through to the
  // destination-snapshot backoff below.
  try {
    if (sourceChangedSince(ingest, memName, memMeta)) {
      dbg(`${ingest.name}: source changed since last sync — not skipping`);
      return false;
    }
  } catch (e) {
    dbg(`${ingest.name}: source-change check skipped: ${e.message}`);
  }

  const backoff = loadBackoff();
  const entry = backoff[ingest.name] || { skip_remaining: 0, skip_level: 0, snapshot: '' };
  const current = memSnapshotToken(memName, memMeta);

  if (entry.snapshot && current !== entry.snapshot) {
    entry.skip_remaining = 0;
    entry.skip_level = 0;
    entry.snapshot = current;
    backoff[ingest.name] = entry;
    saveBackoff(backoff);
    return false;
  }

  if (entry.skip_remaining > 0) {
    const remaining = entry.skip_remaining - 1;
    entry.skip_remaining = remaining;
    backoff[ingest.name] = entry;
    saveBackoff(backoff);
    return true;
  }

  if (entry.snapshot && current === entry.snapshot) {
    entry.skip_level = Math.min(entry.skip_level + 1, MAX_SKIP_LEVEL);
    entry.skip_remaining = entry.skip_level;
  }

  entry.snapshot = current;
  backoff[ingest.name] = entry;
  saveBackoff(backoff);
  return false;
}

// ── Per-mem writing-guidance resolution (engine dump) ─────────────────────

function primaryMemName(ingest) {
  const first = ingest.destinations?.[0]?.mem;
  if (typeof first === 'string') return first;
  if (typeof ingest.projection_mem === 'string') return ingest.projection_mem;
  return null;
}

function getGuidance(memName, memMeta, schemas) {
  if (!memName) return null;
  const meta = memMeta?.[memName];
  if (!meta) return null;
  const config = { writeGuidance: meta.writeGuidance || {} };
  let schemaPayload = null;
  if (typeof meta.schema === 'string') {
    const dwg = schemas?.[meta.schema]?.default_writing_guidance;
    if (dwg && (dwg.avoid || dwg.goal)) {
      schemaPayload = { default_writing_guidance: dwg };
    }
  }
  const merged = resolveWritingGuidance(schemaPayload, config);
  return Object.keys(merged).length === 0 ? null : merged;
}

// ═══════════════════════════════════════════════════════════════════════════
//  PROMPT BLOCKS
// ═══════════════════════════════════════════════════════════════════════════
//
// Each block returns a string ending with a single trailing blank line, or
// the empty string when the block has nothing to say. The main dispatcher
// concatenates and emits.

/**
 * The opening situation block — what the agent cannot derive from
 * tool descriptions or schema bodies alone. Loop semantics, mode,
 * paired mem availability, context-budget signal. Capability
 * framing — no procedure, no targets, no exhortations.
 */
function situationBlock(ingest, mode, processMem) {
  const lines = [];
  lines.push('## Situation');
  lines.push('');
  lines.push(`You are running one iteration of \`${ingest.name}\` (${mode} mode) inside a loop. Each iteration is a fresh agent with no memory of prior runs; the destination graph persists between runs and is your continuity. Backoff is mechanical — when nothing has changed since the last run, the loop skips this ingest silently. Reporting "no changes" is therefore a valid outcome.`);
  lines.push('');
  lines.push(`Mutating the destination is this run's mandate: within the destination mem(s) and paired process mem named under Operative data, create, update, relate, and delete entities without asking. Project-level instructions that make entity creation/deletion ask-first govern interactive dev sessions, not ingest iterations — parking creatable work as a coverage_gap because of that rule defeats the loop. Mems outside the declared destinations remain off-limits.`);
  lines.push('');
  lines.push('Context budget is finite. The `PreCompact` hook fires near the limit and asks you to stop and report. Multiple cycles inside one run are fine when context allows; depth on a coherent area beats breadth across unrelated ones.');
  lines.push('');
  if (processMem.present) {
    lines.push(`A paired process mem \`${processMem.memLabel}\` (schema \`${PROCESS_MEM_SCHEMA}\`) carries destination-quality debt prior runs could not address. Its entries are objective claims about destination state — read them on orientation, write to it when this run also cannot fix some debt, delete entries the destination has since resolved. Call \`memstead_schema(name=${PROCESS_MEM_SCHEMA})\` once for the type vocabulary and write rules.`);
  } else if (processMem.notice) {
    lines.push(`Note: paired process mem \`${processMem.memLabel}\` could not be auto-created — ${processMem.notice}. The run continues without it; the operator can retry with \`memstead mem init ${ingest.name} --org-path ingest --schema ${PROCESS_MEM_SCHEMA}\`.`);
  } else if (processMem.skipped) {
    lines.push(`No process mem is paired with this ingest (mode=${mode}; one-shot ingests are by-design ephemeral).`);
  }
  lines.push('');
  return lines.join('\n') + '\n';
}

/**
 * Render the projection's `intent` field — source-side reading guidance
 * the agent cannot derive from the file globs alone: what kind of
 * material lives in this source tree, what's deliberately excluded and
 * why, how to interpret author-supplied artefacts (CLAUDE.md /
 * DATABASE.md sidecars, generated bindings, migration files). Scoped
 * narrowly: intent describes the *source*, not the destination. The
 * destination's framing comes from the schema's
 * `default_writing_guidance` block rendered next.
 */
function intentBlock(ingest) {
  const intent = ingest?.projection?.intent;
  if (typeof intent !== 'string' || !intent.trim()) return '';
  return `## About the source\n\n${intent.trim()}\n\n`;
}

/**
 * Render the destination's `default_writing_guidance.goal` and `avoid`
 * blocks directly from the resolved schema-side merge — schema authoring
 * is the single source of truth. `resolveWritingGuidance` has already
 * concatenated any per-mem `goal_additions` / `avoid_additions`.
 */
function goalAndAvoidBlock(wg) {
  if (!wg) return '';
  const lines = [];
  if (typeof wg.goal === 'string' && wg.goal.trim()) {
    lines.push('## Goal');
    lines.push('');
    lines.push(wg.goal.trim());
    lines.push('');
  }
  if (typeof wg.avoid === 'string' && wg.avoid.trim()) {
    lines.push('## Failure modes to avoid');
    lines.push('');
    lines.push(wg.avoid.trim());
    lines.push('');
  }
  // Pass-through schema keys other than goal/avoid (rare — destination
  // schemas may add `granularity`, `stack`, etc.). Render minimally so
  // the agent still sees them, but keep the heading hierarchy flat.
  const body = renderResolvedGuidance(wg);
  if (body && !lines.length) {
    return body.trim() + '\n\n';
  }
  return lines.join('\n') + '\n';
}

/**
 * Operative data — what is concretely available to this run. Sources,
 * destination, paired process mem, cross-mem references.
 */
function operativeDataBlock(ingest, processMem, memMeta) {
  const lines = [];
  lines.push('## Operative data');
  lines.push('');

  // Sources
  if (Array.isArray(ingest.sources) && ingest.sources.length) {
    lines.push('### Sources');
    lines.push('');
    const referenceMems = [];
    for (const s of ingest.sources) {
      const label = sourceMediumType(s);
      const roleBit = s.role ? ` (${s.role})` : '';
      const memBit = s.mem ? ` — mem: ${s.mem}` : '';
      lines.push(`- **${label}**${roleBit}${memBit}`);
      const tree = s.facet?.scope?.tree;
      if (tree) {
        const allows = tree.filter(r => r.mode === 'allow').map(r => r.path);
        const denies = tree.filter(r => r.mode === 'deny').map(r => r.path);
        if (allows.length) lines.push(`  - Paths: ${allows.join(', ')}`);
        if (denies.length) lines.push(`  - Ignore: ${denies.join(', ')}`);
      }
      const domains = s.facet?.scope?.domains;
      if (domains) lines.push(`  - Domains: ${domains.join(', ')}`);
      if (s.role === 'reference' && typeof s.mem === 'string' && s.mem) {
        referenceMems.push(s.mem);
      }
    }
    lines.push('');
    if (referenceMems.length) {
      lines.push(`Sources tagged \`(reference)\` are read-only context for cross-mem edges — search them, never write into them. Only \`(primary)\` sources are ingested into the destination.`);
      lines.push('');
      const memList = referenceMems.map(v => `\`memstead_search mem=${v}\``).join(', ');
      lines.push(`**Cross-mem references:** consult ${memList} before authoring cross-mem edges. The target entity must exist — a wiki-link or relationship to a missing target either auto-stubs (silent) or fails authorization (\`CROSS_MEM_RELATION\`).`);
      lines.push('');
    }
  }

  // Destination
  if (Array.isArray(ingest.destinations) && ingest.destinations.length) {
    lines.push('### Destination');
    lines.push('');
    for (const d of ingest.destinations) {
      const meta = memMeta?.[d.mem];
      const schemaBit = meta?.schema ? ` — schema: \`${meta.schema}\`` : '';
      const roleBit = d.role ? ` — ${d.role}` : '';
      lines.push(`- **${d.mem}**${schemaBit}${roleBit}`);
    }
    lines.push('');
  }

  // Paired process mem
  if (processMem.present) {
    lines.push('### Paired process mem');
    lines.push('');
    lines.push(`- **${processMem.memLabel}** — schema: \`${PROCESS_MEM_SCHEMA}\`. Inspect via \`memstead_overview\` / \`memstead_search mem=${processMem.leafName}\`.`);
    lines.push('');
  }

  return lines.join('\n') + '\n';
}

/**
 * Refinement-mode framing. Scout phase emits a batch of source files to
 * review and a findings file path for the writer phase. Writer phase
 * receives the scout's findings and acts on them.
 */
function refinementScoutBlock(ingest, batch) {
  const lines = [];
  const terms = resolveMediumTerms(ingest);
  lines.push(`> [scout | ${ingest.name}] rotation ${batch.rotation}, batch ${batch.batchIndex}/${batch.totalBatches} (${batch.files.length} ${terms['source.artifacts']})`);
  lines.push('');
  lines.push('## Mode: refinement — scout phase');
  lines.push('');
  lines.push(`The scout phase reads ${terms['source.artifacts']} closely and notes discrepancies against the existing destination ${terms['destination.artifacts']}; the writer phase (next iteration of this ingest) acts on those notes. Findings file is the only handover between the two phases.`);
  lines.push('');
  lines.push('### This batch');
  lines.push('');
  for (const f of batch.files) lines.push(`- ${f}`);
  lines.push('');
  lines.push('### Output');
  lines.push('');
  lines.push('Write findings to the file below via Bash:');
  lines.push('');
  lines.push('```bash');
  lines.push(`cat > "${findingsPath(ingest.name)}" << 'FINDINGS_EOF'`);
  lines.push('# findings here');
  lines.push('FINDINGS_EOF');
  lines.push('```');
  lines.push('');
  lines.push('If nothing meaningful turns up: write `No findings.` to the file. The next iteration\'s writer phase reads what you put there and acts. Quality debt that is real but out-of-scope for this batch — record it as `coverage_gap` / `verification_target` / `inconsistency` in the paired process mem if available, and note that you did so in the findings file.');
  return lines.join('\n') + '\n';
}

function refinementWriterBlock(ingest, findings) {
  const lines = [];
  const terms = resolveMediumTerms(ingest);
  lines.push(`> [writer | ${ingest.name}] acting on scout findings`);
  lines.push('');
  lines.push('## Mode: refinement — writer phase');
  lines.push('');
  lines.push(`The previous iteration's scout produced the findings below. Read the cited ${terms['source.artifacts']} yourself before acting — the scout was working under context pressure and may have missed nuance. If you find debt the scout missed, address it too.`);
  lines.push('');
  lines.push('### Scout findings');
  lines.push('');
  lines.push(findings);
  return lines.join('\n') + '\n';
}

/**
 * One-shot lens enrichment — destination set table, routing rule (if any),
 * idempotency contract, end-of-run report shape, optional archive.
 * Same shape as the pre-rebuild script, less prose.
 */
function oneShotLensBlock(ingest, memMeta) {
  const lines = [];
  lines.push('## Mode: one-shot — lens routing');
  lines.push('');
  lines.push('A lens iterates entities once and writes per-destination, then exits. The agent decides per-entity which destinations to target (Routing rule). Re-runs use `memstead_update`; never duplicate.');
  lines.push('');

  // Destination set
  lines.push('### Destination set');
  lines.push('');
  lines.push('| Mem | Schema | Purpose |');
  lines.push('|-------|--------|---------|');
  for (const d of (ingest.destinations || [])) {
    const meta = memMeta?.[d.mem] || {};
    const cell = s => String(s || '').replaceAll('|', '\\|').replaceAll('\n', ' ');
    lines.push(`| ${cell(d.mem)} | ${cell(meta.schema || '(none)')} | ${cell(d.role || meta.description || '(no purpose declared)')} |`);
  }
  lines.push('');

  // Routing rule
  const routing = ingest?.projection?.rules?.routing;
  if (typeof routing === 'string' && routing.trim()) {
    lines.push('### Routing rule');
    lines.push('');
    lines.push('```');
    lines.push(routing);
    lines.push('```');
    lines.push('');
  }

  // Idempotency
  lines.push('### Idempotency');
  lines.push('');
  lines.push('- Search the destination before writing; route changes through `memstead_update` against the existing entity if present.');
  lines.push('- Skip writes when the lifted content matches what is already there (record as `skipped: already-up-to-date`).');
  lines.push('- Use `memstead_create` only when no entity for that concept exists yet.');
  lines.push('');

  // Report
  lines.push('### End-of-run report');
  lines.push('');
  lines.push('After every destination is processed, emit one block per destination on stdout, in Destination-set order:');
  lines.push('');
  lines.push('```');
  lines.push(`### Report: ${ingest.name}`);
  lines.push('');
  lines.push('Destination: <mem>');
  lines.push('  created: <count>');
  lines.push('  updated: <count>');
  lines.push('  skipped: <count>');
  lines.push('  failed:  <count>');
  lines.push('  failures:');
  lines.push('    - <entity-key>: <error verbatim>');
  lines.push('  skipped-detail:');
  lines.push('    - <entity-key>: <one-line reason>');
  lines.push('```');
  lines.push('');
  lines.push('Per-destination commits are independent — partial success is the accepted failure mode. No rollback.');
  lines.push('');

  // Archive
  const archive = ingest?.raw?.post_actions?.archive_source ?? ingest?.post_actions?.archive_source;
  if (archive) {
    lines.push('### Archive after run');
    lines.push('');
    lines.push('After the report has been emitted, archive the source planning mem — `post_actions.archive_source` is set on this ingest.');
    lines.push('');
  }

  return lines.join('\n') + '\n';
}

// ═══════════════════════════════════════════════════════════════════════════
//  MAIN
// ═══════════════════════════════════════════════════════════════════════════

try {
  if (!WORKSPACE_ROOT) {
    process.stdout.write(`> **[${STATE_PREFIX}] No workspace root found (no .memstead.toml in cwd or skill-dir ancestry).**\n`);
    process.exit(0);
  }
  dbg(`workspace=${WORKSPACE_ROOT}`);

  const ws = loadWorkspace(WORKSPACE_ROOT);
  const ingests = ws.ingests;

  if (!ingests.length) {
    process.stdout.write(`[${STATE_PREFIX}] No ingests found under ${ws.dirs.ingests}/.\n`);
    process.exit(0);
  }

  // ── Resolve ingest ──────────────────────────────────────────────────────

  let ingest;
  if (allMode) {
    const runsFile = CACHE_DIR ? join(CACHE_DIR, `${STATE_PREFIX}-one-shot-runs.json`) : null;
    const oneShotRuns = runsFile ? readJsonSafe(runsFile, {}) : {};
    const eligible = ingests.filter(i => !(i.mode === 'one-shot' && oneShotRuns[i.name]));
    if (!eligible.length) {
      process.stdout.write(`[${STATE_PREFIX}] No runnable ingests (all one-shot ingests have completed).\n`);
      process.exit(0);
    }
    const start = nextIngest(eligible);
    const names = eligible.map(i => i.name);
    const startIdx = names.indexOf(start.name);
    ingest = null;
    for (let i = 0; i < names.length; i++) {
      const candidate = eligible[(startIdx + i) % names.length];
      const memName = primaryMemName(candidate);
      if (!shouldSkip(candidate, memName, ws.memMeta)) {
        ingest = candidate;
        break;
      }
      dbg(`${candidate.name} in backoff, trying next`);
    }
    if (!ingest) {
      process.stdout.write(`Skipped.\n`);
      process.exit(0);
    }
    dbg(`round-robin picked: ${ingest.name} from [${names.join(', ')}]`);
  } else {
    const match = ingests.find(i => i.name === nameArg);
    if (!match) {
      const available = ingests.map(i => i.name).join(', ');
      process.stdout.write(`> **[${STATE_PREFIX}] "${nameArg}" not found. Available: ${available}**\n`);
      process.exit(0);
    }
    ingest = match;
  }

  // ── Unsupported preparation step ───────────────────────────────────────
  // A facet may declare a deterministic `preparation` step (e.g. PDF →
  // markdown). No preparation implementation exists yet, so an ingest whose
  // source facet names one is reported unsupported and skipped — never run
  // against raw, unprepared content, and never silently dropped.
  const unprepared = (ingest.sources || []).find((s) => s.facet?.preparation);
  if (unprepared) {
    process.stdout.write(
      `> **[${STATE_PREFIX}] Ingest "${ingest.name}" is unsupported: facet "${unprepared.facet_ref}" ` +
      `declares preparation "${unprepared.facet.preparation}", which has no implementation. Skipping.**\n`
    );
    process.exit(0);
  }

  // Publish the active ingest's deny_paths to the cache file the
  // deny-meta-files hook reads. Done before any prompt emission so the
  // forked agent's first tool call sees the right policy.
  writeActiveDenyPaths(ingest);

  const memName = primaryMemName(ingest);
  const wg = getGuidance(memName, ws.memMeta, ws.schemas);
  const mode = ingest.mode || 'discovery';
  dbg(`ingest=${ingest.name} mode=${mode} mem=${memName || '(none)'}`);

  // Source-change cursor: the changed slice (if any) the agent should
  // steer at first, plus the engine-routed instruction to advance the
  // baseline after a complete pass. Empty preface ⇒ today's behaviour.
  let cursorPreface = '';
  try {
    const cursor = computeSourceCursor(ingest, memName, ws.memMeta);
    cursorPreface = changedSliceBlock(cursor);
    if (cursorPreface) dbg(`source-cursor: ${cursor.anyChanges ? 'changed-slice' : 'reseed'} for ${ingest.name}`);
  } catch (e) {
    // Source-cursor is targeting precision, never correctness: any
    // failure degrades to today's roam/shuffle, never a crash.
    dbg(`source-cursor skipped for ${ingest.name}: ${e.message}`);
  }

  // ── Process mem: auto-create unless one-shot ─────────────────────────

  const processMem = {
    present: false,
    skipped: mode === 'one-shot',
    notice: null,
    leafName: ingest.name,
    memLabel: `${PROCESS_MEM_PATH}/${ingest.name}`,
  };
  if (!processMem.skipped) {
    if (processMemExists(ingest.name, ws.memMeta)) {
      processMem.present = true;
    } else {
      const r = autoCreateProcessMem(ingest.name);
      if (r.ok) {
        processMem.present = true;
      } else {
        processMem.notice = r.notice;
      }
    }
  }

  // ── Refinement: pending findings → writer phase, else → scout phase ────

  if (mode === 'refinement') {
    ensureCacheDir(REF_DIR_NAME);
    const findings = readPendingFindings(ingest.name);
    const parts = [];
    parts.push(situationBlock(ingest, mode, processMem));
    parts.push(intentBlock(ingest));
    parts.push(goalAndAvoidBlock(wg));
    parts.push(operativeDataBlock(ingest, processMem, ws.memMeta));
    if (cursorPreface) parts.push(cursorPreface);
    if (findings) {
      if (!DRY_RUN) {
        try { unlinkSync(findingsPath(ingest.name)); } catch {}
      }
      parts.push(refinementWriterBlock(ingest, findings));
    } else {
      const batch = nextBatch(ingest.name, ingest);
      if (!batch || !batch.files.length) {
        process.stdout.write(`[${STATE_PREFIX} | ${ingest.name}] No source files found for ingest.\n`);
        process.exit(0);
      }
      parts.push(refinementScoutBlock(ingest, batch));
    }
    process.stdout.write(parts.filter(Boolean).join(''));
    process.exit(0);
  }

  // ── One-shot: emit lens enrichment, mark ran ────────────────────────────

  if (mode === 'one-shot') {
    // Mark BEFORE emitting the prompt — a mid-run agent failure leaves
    // the marker in place rather than silently re-firing on the next
    // round. To re-run a one-shot ingest, the operator removes the
    // matching entry from `.memstead.cache/ingest/ingest-one-shot-runs.json`
    // (or the whole file). No `--clear` shortcut for this — `--clear`
    // is the per-ingest process-mem deletion gate; conflating it with
    // cache-file fiddling would muddy a small, sharp tool.
    markOneShotRan(ingest.name);
    const parts = [
      situationBlock(ingest, mode, processMem),
      intentBlock(ingest),
      goalAndAvoidBlock(wg),
      operativeDataBlock(ingest, processMem, ws.memMeta),
      oneShotLensBlock(ingest, ws.memMeta),
    ];
    process.stdout.write(parts.filter(Boolean).join(''));
    process.exit(0);
  }

  // ── Discovery (default) ────────────────────────────────────────────────

  const parts = [
    situationBlock(ingest, mode, processMem),
    intentBlock(ingest),
    goalAndAvoidBlock(wg),
    operativeDataBlock(ingest, processMem, ws.memMeta),
    cursorPreface,
  ];
  process.stdout.write(parts.filter(Boolean).join(''));

} catch (e) {
  process.stderr.write(`[${STATE_PREFIX}] error: ${e.message}\n`);
  process.stdout.write(`> **[${STATE_PREFIX}] Could not load ingests. Check .memstead.toml and ingests/*.json.**\n`);
  process.exit(0);
}
