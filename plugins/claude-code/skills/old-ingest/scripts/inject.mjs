#!/usr/bin/env node
/**
 * inject.mjs — context assembler for /memstead:ingest
 *
 * Iterates `<workspace>/<ingests_dir>/*.json`. Each ingest file carries its
 * mode, trigger, batch_size, and a reference to a projection at
 * `<projections_dir>/<mem>/<name>.json`. The workspace-loader (Session 1
 * of phase 3) produces a normalised ingest list where each entry has its
 * projection, sources (with `scope_ref` inlined), and destinations
 * pre-resolved. This script picks the next ingest (round-robin), assembles
 * the agent prompt, and exits.
 *
 * Modes:
 *   discovery (default)     — minimal context, no scout/writer cycle
 *   refinement              — scout/writer cycle with system-assigned batches
 *   one-shot                — runs exactly once per trigger, never re-picked
 *
 * Always exits 0.
 */

import { readFileSync, writeFileSync, existsSync, readdirSync, unlinkSync, mkdirSync, statSync, renameSync } from 'node:fs';
import { resolve, join, dirname } from 'node:path';
import { globSync } from 'node:fs';
import { loadWorkspace } from './workspace-loader.mjs';
import {
  resolveWritingGuidance,
  renderResolvedGuidance,
} from '../../lib/writing-guidance.mjs';

// ── Workspace root discovery ────────────────────────────────────────────────

/**
 * Walk up from a starting directory looking for a workspace root. Two shapes
 * are recognised (in this order, per-directory):
 *   1. The directory contains `.memstead.toml` directly — that directory is root.
 *   2. The directory contains `.mcp.json` whose memstead server arg list includes
 *      `--config <relative-path-to-.memstead.toml>` — the workspace root is the
 *      directory containing that TOML, resolved relative to the `.mcp.json`.
 * Returns the absolute workspace root or null.
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
          for (let i = 0; i < argList.length; i++) {
            if (argList[i] === '--config' && argList[i + 1]) {
              const cfgPath = resolve(dir, argList[i + 1]);
              if (existsSync(cfgPath)) return dirname(cfgPath);
            }
          }
        }
      } catch {}
    }

    const parent = dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

/**
 * Resolve the workspace root. Walks up from cwd first; falls back to
 * `CLAUDE_SKILL_DIR` (set by Claude Code during skill execution) so the
 * skill works even when the runtime cwd is unrelated to the workspace.
 */
function resolveWorkspaceRoot() {
  const fromCwd = findWorkspaceRoot(process.cwd());
  if (fromCwd) return fromCwd;
  const fallbackRoot = process.env.CLAUDE_SKILL_DIR ?? dirname(new URL(import.meta.url).pathname);
  return findWorkspaceRoot(resolve(fallbackRoot));
}

// ── Args ────────────────────────────────────────────────────────────────────

const args       = process.argv.slice(2);
const cleanRun   = args.includes('--clean');
const allMode    = args.includes('--all') || !args.filter(a => !a.startsWith('--')).join('').trim();
const nameArg    = args.filter(a => !a.startsWith('--')).join(' ').trim();

const STATE_PREFIX = 'ingest';
const MAX_SKIP_LEVEL = 10;

// Dry-run mode. When set (e.g. `MEMSTEAD_INGEST_DRY_RUN=1`), every cache
// write the script would otherwise perform is short-circuited:
// round-robin cursor, backoff snapshot, refinement batch cursor,
// one-shot completion marker, and the prompt-capture-on-exit handler.
// Reads still happen normally and stdout/stderr are unaffected, so the
// emitted prompt is byte-for-byte what a real run would produce given
// the current cache contents — but the cache itself is left intact.
//
// The same env-var name is read by another Memstead component that sets it
// when invoking this script; if you change the variable name, change it on
// both sides.
const DRY_RUN = !!process.env.MEMSTEAD_INGEST_DRY_RUN;

// ── Debug logging (stderr — visible to user, not to agent) ──────────────────

const DEBUG = !process.env.MEMSTEAD_INGEST_QUIET;
function dbg(...parts) {
  if (DEBUG) process.stderr.write(`[ingest:dbg] ${parts.join(' ')}\n`);
}

// ── Workspace state dir ─────────────────────────────────────────────────────

const WORKSPACE_ROOT = resolveWorkspaceRoot();
// Ingest state (round-robin cursor, backoff, prompt capture, refinement
// findings) lives under a workspace-level cache dir. `.memstead.cache/` at the
// workspace root carries a `.gitignore` with `*` so its contents never land
// in git. The cache dir is created lazily on first write.
const CACHE_DIR = WORKSPACE_ROOT ? join(WORKSPACE_ROOT, '.memstead.cache', 'ingest') : null;

function ensureCacheDir(sub = '') {
  if (!CACHE_DIR) return null;
  const target = sub ? join(CACHE_DIR, sub) : CACHE_DIR;
  if (DRY_RUN) return target; // path still resolved; no on-disk side effects
  try {
    mkdirSync(target, { recursive: true });
    // Drop a .gitignore at the cache root the first time we create it.
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
    // Keep only the latest 10.
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

// ── Round-robin cursor keyed by ingest filename ─────────────────────────────

/**
 * Pick the next ingest from the provided list and advance the cursor.
 * Cursor state lives at `<cache>/ingest-cursor.json` and is keyed by the
 * ingest filename. Returns the picked ingest.
 */
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

// ── Clean ───────────────────────────────────────────────────────────────────

if (cleanRun) {
  let n = 0;
  if (CACHE_DIR && existsSync(CACHE_DIR)) {
    try {
      for (const f of readdirSync(CACHE_DIR)) {
        const p = join(CACHE_DIR, f);
        try {
          const st = statSync(p);
          if (st.isFile() && f.startsWith(STATE_PREFIX)) { unlinkSync(p); n++; }
        } catch {}
      }
      // Wipe refinement findings and batches.
      const refDir = join(CACHE_DIR, 'refinement');
      if (existsSync(refDir)) {
        for (const f of readdirSync(refDir)) {
          try { unlinkSync(join(refDir, f)); } catch {}
        }
      }
    } catch {}
  }
  process.stdout.write(`[${STATE_PREFIX} | clean] Deleted ${n} state file(s). Ready for fresh start.\n\n`);
  process.stdout.write('**STOP — clean complete. Report and exit.**\n');
  process.exit(0);
}

// ── Medium resolution for prompt assembly ───────────────────────────────────

const MEDIUMS = JSON.parse(readFileSync(new URL('../mediums.json', import.meta.url), 'utf8'));

/**
 * Scope files have shape `{ type: "codebase" | ..., scope: { tree | domains } }`.
 * Surface each source's medium type via `source.scope.type`. Destinations are
 * always graph type in the externalised layout — there is no medium field on
 * them; the ingest writes into a mem graph.
 *
 * Sources without an inlined scope (lens sources that read from another
 * mem's graph — `{mem: "plan", role: "planning graph"}`) have medium
 * type "graph" by convention.
 */
function sourceMediumType(src) {
  if (src?.scope?.type) return src.scope.type;
  if (src?.mem) return 'graph';
  return 'codebase';
}

function destinationMediumType(_dst) {
  return 'graph';
}

/**
 * Resolve medium terminology for an ingest. Returns a flat map of
 * `{{key}}` → value for template substitution.
 */
function resolveMediumTerms(ingest) {
  const srcMediums = [...new Set((ingest.sources || []).map(sourceMediumType).filter(Boolean))];
  const primarySrc = srcMediums[0] || 'codebase';
  const dstKey = destinationMediumType(ingest.destinations?.[0]);

  const src = MEDIUMS.source[primarySrc] || MEDIUMS.source.codebase;
  const dst = MEDIUMS.destination[dstKey] || MEDIUMS.destination.graph;

  return {
    'source.artifact': src.artifact,
    'source.artifacts': src.artifacts,
    'source.readVerb': src.readVerb,
    'source.readTools': src.readTools,
    'source.readInstruction': src.readInstruction,
    'source.deepReadInstruction': src.deepReadInstruction,
    'source.tip': src.tip,
    'destination.artifact': dst.artifact,
    'destination.artifacts': dst.artifacts,
    'destination.writeVerb': dst.writeVerb || 'Write',
    'destination.writeTools': dst.writeTools || '',
    'destination.exploreTools': dst.exploreTools || '',
    'destination.writeInstruction': dst.writeInstruction || '',
    'destination.tip': dst.tip || '',
  };
}

function loadMediumPrompt(medium) {
  try {
    const promptPath = new URL(`../prompts/${medium}.md`, import.meta.url);
    return readFileSync(promptPath, 'utf8').trim() + '\n\n';
  } catch {
    return '';
  }
}

function loadTemplate(name, terms) {
  try {
    const path = new URL(`../prompts/${name}`, import.meta.url);
    let content = readFileSync(path, 'utf8');
    for (const [key, value] of Object.entries(terms)) {
      content = content.replaceAll(`{{${key}}}`, value);
    }
    return content.trim() + '\n\n';
  } catch {
    return '';
  }
}

function taskFraming(ingest) {
  const srcMediums = [...new Set((ingest.sources || []).map(sourceMediumType).filter(Boolean))];
  const dstKey = destinationMediumType(ingest.destinations?.[0]);
  const dst = MEDIUMS.destination[dstKey] || MEDIUMS.destination.graph;

  const srcParts = srcMediums.map(k => (MEDIUMS.source[k] || { framing: 'reading sources' }).framing);
  const srcFraming = srcParts.length <= 1
    ? (srcParts[0] || 'reading sources')
    : srcParts.slice(0, -1).join(', ') + ' and ' + srcParts.at(-1);

  const tips = [];
  for (const k of srcMediums) {
    const t = (MEDIUMS.source[k] || { tip: null }).tip;
    if (t && !tips.includes(t)) tips.push(t);
  }
  if (dst.tip && !tips.includes(dst.tip)) tips.push(dst.tip);

  const lines = ['## Task', '', `You are ${srcFraming} and ${dst.framing}.`, ''];
  if (tips.length) lines.push(...tips, '');
  return lines.join('\n');
}

// ── Render ingest config ────────────────────────────────────────────────────

function renderIngest(ingest) {
  const out = [`## Projection: ${ingest.projection_ref}`, ''];
  const proj = ingest.projection || {};

  if (proj.intent) out.push(`**Intent:** ${proj.intent}`, '');

  if (Array.isArray(ingest.sources) && ingest.sources.length) {
    out.push('### Sources', '');
    const referenceMems = [];
    for (const s of ingest.sources) {
      const label = sourceMediumType(s);
      const roleBit = s.role ? ` (${s.role})` : '';
      const memBit = s.mem ? ` — mem: ${s.mem}` : '';
      out.push(`- **${label}**${roleBit}${memBit}`);
      const tree = s.scope?.scope?.tree;
      if (tree) {
        const allows = tree.filter(r => r.mode === 'allow').map(r => r.path);
        const denies = tree.filter(r => r.mode === 'deny').map(r => r.path);
        if (allows.length) out.push(`  - Paths: ${allows.join(', ')}`);
        if (denies.length) out.push(`  - Ignore: ${denies.join(', ')}`);
      }
      const domains = s.scope?.scope?.domains;
      if (domains) out.push(`  - Domains: ${domains.join(', ')}`);
      if (s.role === 'reference' && typeof s.mem === 'string' && s.mem) {
        referenceMems.push(s.mem);
      }
    }
    out.push('');
    // Cross-mem edge guidance — surfaces when a projection lists a
    // reference mem. Cross-mem links are workspace-policy-gated
    // (`[cross_mem_links]` in `.memstead.toml`) and only succeed when
    // the target entity exists. The first line defines what `(reference)`
    // means; the second names the actual reference mems and their
    // failure modes. Both are gated on the presence of a reference source —
    // projections without one need neither.
    if (referenceMems.length) {
      out.push(
        `Sources tagged \`(reference)\` are read-only context for cross-mem edges — search them but never write into them. Only \`(primary)\` sources are ingested into the destination.`,
        ''
      );
      const memList = referenceMems
        .map(v => `\`memstead_search mem=${v}\``)
        .join(', ');
      out.push(
        `**Cross-mem references:** consult ${memList} before authoring cross-mem edges. Targets must exist in the reference mem before linking — a wiki-link or relationship to a missing target either auto-stubs (silent) or fails authorization (\`CROSS_MEM_RELATION\`).`,
        ''
      );
    }
  }

  if (Array.isArray(ingest.destinations) && ingest.destinations.length) {
    out.push('### Destinations', '');
    for (const d of ingest.destinations) {
      const roleBit = d.role ? ` — ${d.role}` : '';
      out.push(`- **${d.mem}**${roleBit}`);
    }
    out.push('');
  }

  if (proj.rules && typeof proj.rules === 'object') {
    out.push('### Rules', '', '| Field | Value |', '|-------|-------|');
    for (const [k, v] of Object.entries(proj.rules)) {
      if (k.startsWith('_')) continue;
      const s = Array.isArray(v) ? v.map(i => `• ${i}`).join(' ') : String(v);
      out.push(`| **${k}** | ${s} |`);
    }
    out.push('');
  }

  return out.join('\n');
}

// ── Write guidance + destination metadata (sourced from the engine dump) ────

/**
 * Pick the ingest's primary destination mem name. Falls back to the
 * projection's owning mem for source-only refinement cases. Returns
 * `null` when no name resolves.
 *
 * The engine dump is the source of truth for "what mems exist"; this
 * helper does not validate against `memMeta` because the
 * workspace-loader already rejected unknown mem names at projection
 * resolution time.
 */
function primaryMemName(ingest) {
  const first = ingest.destinations?.[0]?.mem;
  if (typeof first === 'string') return first;
  if (typeof ingest.projection_mem === 'string') return ingest.projection_mem;
  return null;
}

/**
 * Resolve the merged writing-guidance object for one mem.
 *
 * Merges the mem's `writeGuidance` block from the engine dump with
 * the schema's `default_writing_guidance` (also from the dump) via the
 * shared resolver at `lib/writing-guidance.mjs`. Returns `null` when
 * neither side declares anything (the renderer upstream short-circuits
 * on `null`).
 */
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

/**
 * Map each destination to `{mem, role, schema, description}` using
 * the engine dump's per-mem metadata. Used by the one-shot lens
 * enrichment so the agent sees per-destination metadata without
 * round-trips to memstead_health. Returns one entry per destination,
 * preserving order.
 */
function mapDestinationMeta(destinations, memMeta) {
  const out = [];
  for (const dst of (destinations || [])) {
    const mem = dst?.mem;
    if (!mem) { out.push({ ...dst }); continue; }
    const meta = memMeta?.[mem] || {};
    out.push({
      mem,
      role: dst.role ?? null,
      schema: typeof meta.schema === 'string' ? meta.schema : null,
      description: typeof meta.description === 'string' ? meta.description : null,
    });
  }
  return out;
}

/**
 * Lens-mode prompt enrichment for one-shot ingests with multiple
 * destinations. Emits four parseable sections — agents iterate destinations,
 * call memstead_create / memstead_update per destination, and emit the structured
 * report at end-of-run. Plugin assembles the prompt; agent owns the work.
 *
 * Sections:
 *   - Destination set     — per-destination mem, schema, role/purpose
 *   - Routing rule        — verbatim from projection.rules.routing
 *   - Idempotency         — re-runs use memstead_update, never duplicate
 *   - Report schema       — per-destination success/failure/skipped contract
 *   - Archive instruction — only when ingest.post_actions.archive_source set
 */
function assembleLensEnrichment(ingest, destinationsMeta) {
  const lines = [];

  // ── Destination set ────────────────────────────────────────────────────
  lines.push('## Destination set\n');
  lines.push('You write to each destination independently. Each row below describes one destination — the agent decides per-entity which destinations to target (see Routing rule).\n');
  lines.push('| Mem | Schema | Purpose |');
  lines.push('|-------|--------|---------|');
  for (const d of destinationsMeta) {
    const mem = d.mem || '(unknown)';
    const schema = d.schema || '(no schema declared)';
    const purpose = d.role || d.description || '(no purpose declared)';
    // Escape pipe characters in cell values to keep the table parseable.
    const cell = s => String(s).replaceAll('|', '\\|').replaceAll('\n', ' ');
    lines.push(`| ${cell(mem)} | ${cell(schema)} | ${cell(purpose)} |`);
  }
  lines.push('');

  // ── Routing rule (verbatim) ────────────────────────────────────────────
  const routing = ingest?.projection?.rules?.routing;
  if (typeof routing === 'string' && routing.trim()) {
    lines.push('## Routing rule\n');
    lines.push('Decide per-entity which destinations to target. The projection\'s routing rule, verbatim:\n');
    lines.push('```');
    lines.push(routing);
    lines.push('```\n');
  }

  // ── Idempotency guidance ───────────────────────────────────────────────
  lines.push('## Idempotency\n');
  lines.push('A lens may be re-run. Never duplicate an entity that already exists in a destination:\n');
  lines.push('- Before writing, call `memstead_search` (and `memstead_entity` for the candidate) to check whether the target entity already exists in the destination mem.');
  lines.push('- If it exists and the lifted content is meaningfully different, route the change through `memstead_update` against the existing entity.');
  lines.push('- If it exists and the lifted content matches what is already there, skip the write — record it as `skipped` with reason `already-up-to-date`.');
  lines.push('- Only call `memstead_create` when no entity for that concept exists in the destination yet.\n');

  // ── Report schema ──────────────────────────────────────────────────────
  lines.push('## End-of-run report\n');
  lines.push('After iterating every destination, emit a structured report on stdout. One block per destination, in the order listed in the Destination set. Format:\n');
  lines.push('```');
  lines.push('### Report: <ingest-name>');
  lines.push('');
  lines.push('Destination: <mem>');
  lines.push('  created: <count>   # entities created via memstead_create');
  lines.push('  updated: <count>   # entities updated via memstead_update');
  lines.push('  skipped: <count>   # writes skipped (idempotent or out-of-scope)');
  lines.push('  failed:  <count>   # writes that errored');
  lines.push('  failures:');
  lines.push('    - <entity-key>: <error message verbatim>');
  lines.push('  skipped-detail:');
  lines.push('    - <entity-key>: <one-line reason, e.g. already-up-to-date or out-of-scope>');
  lines.push('```\n');
  lines.push('Partial success is the accepted failure mode: each destination is an independent commit target. If one destination fails, continue iterating the remaining destinations and report per-destination outcomes. Do not attempt rollback.\n');

  // ── Archive instruction (optional) ─────────────────────────────────────
  const archive = ingest?.raw?.post_actions?.archive_source ?? ingest?.post_actions?.archive_source;
  if (archive) {
    lines.push('## Archive after run\n');
    lines.push('After every destination has been processed and the report above has been emitted, archive the source planning mem. The ingest declares `post_actions.archive_source` — once the lens has lifted what belongs to each destination, the source mem is moved to its archive location and is no longer a writable target.\n');
  }

  return lines.join('\n');
}

function renderGuidance(wg) {
  if (!wg) return '';
  if (typeof wg === 'string') return `## Write Guidance\n\n${wg}\n\n`;
  const body = renderResolvedGuidance(wg);
  if (!body) return '';
  return `## Write Guidance\n\n${body}\n\n`;
}

// ── Enumerate source files (codebase / filesystem scopes) ───────────────────

/**
 * Scope paths inside scope files resolve relative to the workspace root
 * (per the comment in `.memstead.toml`). globSync runs with cwd=WORKSPACE_ROOT.
 */
function enumerateSourceFiles(ingest) {
  if (!WORKSPACE_ROOT) return [];
  const files = [];
  for (const src of (ingest.sources || [])) {
    const type = sourceMediumType(src);
    if (type !== 'codebase' && type !== 'filesystem') continue;
    const allows = [];
    const denies = [];
    for (const rule of src.scope?.scope?.tree || []) {
      if (rule.mode === 'allow') allows.push(rule.path);
      else if (rule.mode === 'deny') denies.push(rule.path);
    }
    if (!allows.length) continue;
    const matched = [...new Set(allows.flatMap(p => globSync(p, { cwd: WORKSPACE_ROOT })))];
    const denySet = denies.length
      ? new Set(denies.flatMap(p => globSync(p, { cwd: WORKSPACE_ROOT })))
      : new Set();
    const filtered = denySet.size ? matched.filter(f => !denySet.has(f)) : matched;
    files.push(...filtered);
  }
  return [...new Set(files)].sort();
}

// ═══════════════════════════════════════════════════════════════════════════
//  REFINEMENT MODE — scout/writer cycle with system-assigned batches
// ═══════════════════════════════════════════════════════════════════════════

const REF_DIR_NAME   = 'refinement';
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

function buildEntityOverview(_ingest) {
  return '**IMPORTANT: Start by calling `memstead_search` (omit `query` for a full structural listing) to see all entities. For each source file in your batch, call `memstead_entity` to read the full content of mapped entities. You cannot evaluate coverage without reading both the source file AND the entity.**\n';
}

function assembleScoutPrompt(ingest, batch, wgBlock, memName) {
  const terms = resolveMediumTerms(ingest);
  const lines = [];

  lines.push(`> [scout | ${ingest.name}] rotation ${batch.rotation}, batch ${batch.batchIndex}/${batch.totalBatches} (${batch.files.length} ${terms['source.artifacts']})\n`);
  lines.push(taskFraming(ingest));

  lines.push(`## Your review batch\n`);
  lines.push(`Review these ${terms['source.artifacts']} against the ${terms['destination.artifacts']}:\n`);
  for (const f of batch.files) lines.push(`- ${f}`);
  lines.push('');

  lines.push(buildEntityOverview(ingest));

  lines.push(loadTemplate('scout-template.md', terms));

  lines.push(loadMediumPrompt(destinationMediumType(ingest.destinations?.[0])));
  lines.push(wgBlock);
  lines.push(renderIngest(ingest));

  if (memName) lines.push(`**Mem:** ${memName}\n`);

  lines.push('## Save your findings\n');
  lines.push('When done, write your findings to this file via Bash:\n');
  lines.push('```bash');
  lines.push(`cat > "${findingsPath(ingest.name)}" << 'FINDINGS_EOF'`);
  lines.push('# your findings here');
  lines.push('FINDINGS_EOF');
  lines.push('```\n');
  lines.push('If no findings: write `No findings.` to the file and stop.\n');

  return lines.join('\n');
}

function assembleWriterPrompt(ingest, findings, wgBlock, memName) {
  const terms = resolveMediumTerms(ingest);
  const lines = [];

  lines.push(`> [writer | ${ingest.name}] fixing scout findings\n`);
  lines.push(taskFraming(ingest));

  lines.push('## Scout findings\n');
  lines.push(`The scout reviewed ${terms['source.artifacts']} and found these issues. Start here — but you are a fully capable agent.`);
  lines.push(`**Read the ${terms['source.artifacts']} referenced in each finding before writing.** Verify against the actual source. If you find additional problems while reading, fix those too.\n`);
  lines.push(findings);
  lines.push('');

  lines.push(loadMediumPrompt(destinationMediumType(ingest.destinations?.[0])));
  lines.push(wgBlock);
  lines.push(renderIngest(ingest));

  if (memName) lines.push(`**Mem:** ${memName}\n`);

  lines.push('End with: `[writer | ' + ingest.name + '] {what you fixed}`\n');

  return lines.join('\n');
}

// ═══════════════════════════════════════════════════════════════════════════
//  BACKOFF — skip idle ingests with linear backoff
// ═══════════════════════════════════════════════════════════════════════════

/**
 * Look up the engine-supplied opaque snapshot token for a mem. The
 * token changes iff mem content has changed since the last dump; the
 * plugin's only legal operation on it is byte-equality. Returns the
 * empty string when the mem has no token (unknown mem, or backend
 * couldn't compute one — treated as "no snapshot recorded yet").
 */
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

/**
 * Decide whether this ingest should be skipped this fire.
 *
 * inject.mjs runs BEFORE the agent — it can't know whether the agent will
 * change specs. Flow:
 *   1. This invocation: save snapshot, output prompt, exit
 *   2. Agent runs, may create/update entities
 *   3. Next invocation: compare new snapshot to stored one
 *      - Changed → reset backoff, run
 *      - Unchanged + skip_remaining > 0 → decrement, skip
 *      - Unchanged + skip_remaining = 0 → increase level, set new window, run
 *
 * For refinement mode: never skip while the current rotation has unreviewed
 * batches. A "no findings" batch doesn't mean the ingest is idle.
 *
 * For one-shot mode: backoff does not apply — one-shot ingests run once and
 * are removed from the eligible list via `markOneShotRan`.
 */
function shouldSkip(ingest, memName, memMeta) {
  if (ingest.mode === 'one-shot') return false;

  if (ingest.mode === 'refinement' && hasRemainingBatches(ingest.name)) {
    dbg(`backoff suppressed for ${ingest.name} (refinement, batches remaining)`);
    return false;
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
    dbg(`backoff reset for ${ingest.name} (specs changed)`);
    return false;
  }

  if (entry.skip_remaining > 0) {
    const remaining = entry.skip_remaining - 1;
    entry.skip_remaining = remaining;
    backoff[ingest.name] = entry;
    saveBackoff(backoff);
    dbg(`backoff skip ${ingest.name} (${remaining} remaining after this skip, level ${entry.skip_level})`);
    return true;
  }

  if (entry.snapshot && current === entry.snapshot) {
    entry.skip_level = Math.min(entry.skip_level + 1, MAX_SKIP_LEVEL);
    entry.skip_remaining = entry.skip_level;
    dbg(`backoff increased for ${ingest.name}: level ${entry.skip_level}, will skip next ${entry.skip_remaining}`);
  }

  entry.snapshot = current;
  backoff[ingest.name] = entry;
  saveBackoff(backoff);
  return false;
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
    // Round-robin with backoff. Filter out one-shot ingests that already ran;
    // iterate the remaining ingests starting from the cursor's next pick.
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

  const memName = primaryMemName(ingest);
  const wg = getGuidance(memName, ws.memMeta, ws.schemas);
  const wgBlock = renderGuidance(wg);
  const mode = ingest.mode || 'discovery';
  dbg(`ingest=${ingest.name} mode=${mode} mem=${memName || '(none)'}`);

  // ── REFINEMENT MODE ─────────────────────────────────────────────────────

  if (mode === 'refinement') {
    dbg(`refinement mode for "${ingest.name}"`);
    ensureCacheDir(REF_DIR_NAME);

    const findings = readPendingFindings(ingest.name);
    if (findings) {
      dbg('pending findings found, running writer');
      if (!DRY_RUN) {
        try { unlinkSync(findingsPath(ingest.name)); } catch {}
      }
      process.stdout.write(assembleWriterPrompt(ingest, findings, wgBlock, memName));
    } else {
      const batch = nextBatch(ingest.name, ingest);
      if (!batch || !batch.files.length) {
        process.stdout.write(`[${STATE_PREFIX} | ${ingest.name}] No source files found for ingest.\n`);
        process.exit(0);
      }
      dbg(`scout batch: ${batch.files.length} files, rotation ${batch.rotation}, batch ${batch.batchIndex}/${batch.totalBatches}`);
      process.stdout.write(assembleScoutPrompt(ingest, batch, wgBlock, memName));
    }
    process.exit(0);
  }

  // ── ONE-SHOT MODE ───────────────────────────────────────────────────────

  if (mode === 'one-shot') {
    dbg(`one-shot mode for "${ingest.name}"`);
    // Mark as ran BEFORE emitting the prompt. If the agent fails mid-run, a
    // re-run requires the operator to clear the marker (or pass --clean).
    // This matches the plan's invariant: "runs exactly once per trigger, is
    // not re-picked on the next round".
    markOneShotRan(ingest.name);

    const terms = resolveMediumTerms(ingest);
    const header = allMode
      ? `## Running: ${ingest.name} (${ingests.findIndex(i => i.name === ingest.name) + 1}/${ingests.length})\n\n`
      : '';
    const memLine = memName ? `**Mem:** ${memName}\n\n` : '';
    const framing = taskFraming(ingest);
    const dstMedium = destinationMediumType(ingest.destinations?.[0]);
    const mediumPrompt = loadMediumPrompt(dstMedium);
    const skillTemplate = loadTemplate('skill-template.md', terms);

    // Lens enrichment — destination set, routing rule, idempotency, report
    // schema, optional archive. The agent iterates destinations and runs
    // memstead_create / memstead_update per destination via Claude Code's tool-use
    // loop; this prompt block carries the framing it needs.
    const destinationsMeta = mapDestinationMeta(ingest.destinations, ws.memMeta);
    const lensEnrichment = assembleLensEnrichment(ingest, destinationsMeta);

    const promptParts = [
      `> [one-shot | ${ingest.name}]\n\n`,
      header,
      framing,
      skillTemplate,
      mediumPrompt,
      wgBlock,
      renderIngest(ingest),
      lensEnrichment,
      memLine,
    ].filter(Boolean);
    process.stdout.write(promptParts.join(''));
    process.exit(0);
  }

  // ── DISCOVERY MODE (default) ────────────────────────────────────────────

  dbg(`discovery mode for "${ingest.name}"`);

  const terms = resolveMediumTerms(ingest);
  const header = allMode
    ? `## Running: ${ingest.name} (${ingests.findIndex(i => i.name === ingest.name) + 1}/${ingests.length})\n\n`
    : '';
  const memLine = memName ? `**Mem:** ${memName}\n\n` : '';
  const framing = taskFraming(ingest);
  const dstMedium = destinationMediumType(ingest.destinations?.[0]);
  const mediumPrompt = loadMediumPrompt(dstMedium);
  const skillTemplate = loadTemplate('skill-template.md', terms);

  const promptParts = [header, framing, skillTemplate, mediumPrompt, wgBlock, renderIngest(ingest), memLine].filter(Boolean);
  const promptLen = promptParts.reduce((n, s) => n + s.length, 0);
  dbg(`prompt assembled: ~${Math.round(promptLen / 4)} tokens (${promptLen} chars)`);
  process.stdout.write(promptParts.join(''));

} catch (e) {
  process.stderr.write(`[${STATE_PREFIX}] error: ${e.message}\n`);
  process.stdout.write(`> **[${STATE_PREFIX}] Could not load ingests. Check .memstead.toml and ingests/*.json.**\n`);
  process.exit(0);
}
