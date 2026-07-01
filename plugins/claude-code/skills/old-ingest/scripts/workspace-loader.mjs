#!/usr/bin/env node
/**
 * workspace-loader.mjs — load the externalised workspace config and the
 * engine-supplied workspace dump.
 *
 * Reads `<workspace_root>/.memstead.toml` for plugin-side keys (`format`,
 * `scopes_dir`, `projections_dir`, `ingests_dir`). Walks:
 *   <workspace>/<scopes_dir>/<vault>/<name>.json
 *   <workspace>/<projections_dir>/<vault>/<name>.json
 *   <workspace>/<ingests_dir>/<name>.json
 *
 * Per-vault config (`schema`, `writeGuidance`, `description`), per-vault
 * `snapshot_token`, and per-schema `default_writing_guidance` come from
 * `memstead workspace dump --json` — invoked once per `loadWorkspace` call.
 * The plugin no longer reads `<vault>/.memstead/config.json`, no longer
 * walks vault `**.md` for backoff hashes, and no longer reads schema
 * YAML from disk. The engine's storage backend is private to it.
 *
 * Exports:
 *   loadWorkspace(workspaceRoot, opts?) → workspace bundle
 *   loadMemsteadToml(tomlPath) → plugin-side keys
 *
 * `opts.fetchDump(workspaceRoot)` is the injection seam for tests —
 * defaults to spawning the `memstead` CLI.
 */

import { readFileSync, readdirSync, existsSync, statSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { resolve, join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

import { validate as validateSchema } from '../../../schemas/memstead-plugin/v0/validator.mjs';

const __dirname = dirname(fileURLToPath(import.meta.url));

// Versions this loader recognises — see header comment in inject.mjs /
// the v0 schemas. The plugin gates on the `.memstead.toml` `format` key
// (workspace-shape contract) and on the `format` field of the engine
// dump (workspace-dump contract). The two are independent versioning
// surfaces — bump them separately.
const SUPPORTED_FORMATS = ['memstead-plugin/v0'];
const SUPPORTED_DUMP_FORMATS = ['workspace-dump/v0'];

const SCHEMAS_ROOT = resolve(__dirname, '../../../schemas/memstead-plugin/v0');

// ── Minimal TOML extractor ──────────────────────────────────────────────────

/**
 * Extract the plugin-side keys from `.memstead.toml`: `format`,
 * `scopes_dir`, `projections_dir`, `ingests_dir`. The `vaults` array
 * was historically read here as a vault registry; that role is now
 * fulfilled by the engine's workspace dump, so this function intentionally
 * does not return it. Engine-only keys (`schemas_dir`, `[vault_management]`,
 * `[[vault_repos]]`, etc.) are silently ignored.
 */
export function loadMemsteadToml(tomlPath) {
  const src = readFileSync(tomlPath, 'utf-8');
  const lines = src.split(/\r?\n/);

  let format = null;
  let scopesDir = null;
  let projectionsDir = null;
  let ingestsDir = null;

  let i = 0;
  while (i < lines.length) {
    const raw = lines[i];
    const line = stripComment(raw).trim();
    // Stop at the first table header — everything we need is top-level.
    if (line.startsWith('[')) break;
    if (!line) { i++; continue; }

    const eq = line.indexOf('=');
    if (eq < 0) { i++; continue; }
    const key = line.slice(0, eq).trim();
    const valStart = line.slice(eq + 1).trim();

    if (key === 'format') {
      format = parseString(valStart);
    } else if (key === 'scopes_dir') {
      scopesDir = parseString(valStart);
    } else if (key === 'projections_dir') {
      projectionsDir = parseString(valStart);
    } else if (key === 'ingests_dir') {
      ingestsDir = parseString(valStart);
    }
    i++;
  }

  return { format, scopesDir, projectionsDir, ingestsDir };
}

function stripComment(line) {
  const hash = line.indexOf('#');
  if (hash < 0) return line;
  const before = line.slice(0, hash);
  const quoteCount = (before.match(/"/g) || []).length;
  return quoteCount % 2 === 0 ? before : line;
}

function parseString(s) {
  const m = s.match(/^"((?:[^"\\]|\\.)*)"$/);
  if (!m) throw new Error(`workspace-loader: expected quoted string, got ${JSON.stringify(s)}`);
  return JSON.parse(`"${m[1]}"`);
}

// ── Engine dump fetcher ─────────────────────────────────────────────────────

/**
 * Spawn `memstead workspace dump` against `workspaceRoot` and return the
 * parsed dump document. The binary is discovered via `MEMSTEAD_BIN` first
 * (set by tests, by `engine/target/debug/memstead` during dev, or by
 * the operator), `memstead` on `PATH` second. Either failure mode raises
 * an error that names the override mechanism.
 */
export function fetchDumpFromCli(workspaceRoot) {
  const bin = process.env.MEMSTEAD_BIN || 'memstead';
  const result = spawnSync(bin, ['workspace', 'dump'], {
    cwd: workspaceRoot,
    encoding: 'utf-8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });

  if (result.error) {
    if (result.error.code === 'ENOENT') {
      throw new Error(
        `workspace-loader: could not run \`${bin} workspace dump\` — ` +
        `binary not found. Set MEMSTEAD_BIN to an absolute path, or install \`memstead\` on PATH.`
      );
    }
    throw new Error(
      `workspace-loader: \`${bin} workspace dump\` failed to start: ${result.error.message}`
    );
  }

  if (result.status !== 0) {
    const stderr = (result.stderr || '').trim();
    let envelope = null;
    try { envelope = stderr ? JSON.parse(stderr) : null; } catch {}
    const code = envelope?.code ?? result.status;
    const msg = envelope?.error ?? stderr ?? '(no error message)';
    throw new Error(
      `workspace-loader: \`${bin} workspace dump\` exited ${code}: ${msg}`
    );
  }

  let dump;
  try { dump = JSON.parse(result.stdout); }
  catch (e) {
    throw new Error(
      `workspace-loader: could not parse dump JSON from \`${bin} workspace dump\`: ${e.message}`
    );
  }

  if (typeof dump !== 'object' || dump === null) {
    throw new Error(`workspace-loader: dump JSON is not an object`);
  }
  return dump;
}

// ── Workspace loader ────────────────────────────────────────────────────────

/**
 * Load the workspace config from `.memstead.toml`, fetch the engine dump,
 * and walk scopes / projections / ingests. Returns a normalised
 * structure where each ingest entry carries its fully resolved
 * projection (with scope-refs inlined) and destination list, and each
 * vault carries the dump's per-vault metadata and snapshot token.
 *
 * @param {string} workspaceRoot — directory containing `.memstead.toml`.
 * @param {object} [opts]
 * @param {(workspaceRoot: string) => object} [opts.fetchDump] —
 *   injection seam for tests; defaults to `fetchDumpFromCli`.
 * @returns workspace bundle (see in-line shape below).
 */
export function loadWorkspace(workspaceRoot, opts = {}) {
  const root = resolve(workspaceRoot);
  const tomlPath = join(root, '.memstead.toml');
  if (!existsSync(tomlPath)) {
    throw new Error(`workspace-loader: .memstead.toml not found at ${tomlPath}`);
  }
  const toml = loadMemsteadToml(tomlPath);

  const format = toml.format;
  if (format !== null && !SUPPORTED_FORMATS.includes(format)) {
    throw new Error(
      `workspace-loader: unsupported plugin format "${format}" in ${tomlPath}; supported versions: ${SUPPORTED_FORMATS.join(', ')}`
    );
  }
  if (format === null) {
    console.warn(
      `workspace-loader: .memstead.toml at ${tomlPath} is missing the \`format\` key — ` +
      `treating as legacy and validating against ${SUPPORTED_FORMATS[0]}.`
    );
  }

  const dirs = {
    scopes: toml.scopesDir || 'scopes',
    projections: toml.projectionsDir || 'projections',
    ingests: toml.ingestsDir || 'ingests',
  };

  // Fetch the engine dump. In dev / test the caller may inject a
  // fixture; the production path spawns the CLI. The dump-format gate
  // applies regardless of source — both surfaces must agree on v0.
  const fetchDump = opts.fetchDump || fetchDumpFromCli;
  const dump = fetchDump(root);
  if (!SUPPORTED_DUMP_FORMATS.includes(dump.format)) {
    throw new Error(
      `workspace-loader: unsupported workspace-dump format "${dump.format}"; ` +
      `supported versions: ${SUPPORTED_DUMP_FORMATS.join(', ')}`
    );
  }

  // Reduce the dump to plugin-friendly shapes:
  //   vaults[] — sorted vault names, the canonical "what vaults exist"
  //              answer (replaces the legacy `vaults = [...]` array)
  //   vaultMeta — name → { schema, description, writeGuidance, snapshotToken }
  //   schemas   — schemaName → { default_writing_guidance }
  const vaultMeta = {};
  for (const v of (dump.vaults || [])) {
    if (typeof v?.name !== 'string') continue;
    vaultMeta[v.name] = {
      schema: v.schema ?? null,
      description: v.description ?? null,
      writeGuidance: (v.writeGuidance && typeof v.writeGuidance === 'object') ? v.writeGuidance : {},
      snapshotToken: typeof v.snapshot_token === 'string' ? v.snapshot_token : null,
    };
  }
  const vaults = Object.keys(vaultMeta).sort();
  const schemas = (dump.schemas && typeof dump.schemas === 'object') ? dump.schemas : {};

  const schemasLoaded = loadSchemas();

  // Walk scopes: <root>/<scopesDir>/<vault>/<name>.json
  //
  // Pre-engine-dump the loader hard-failed when a `scopes/<dir>/`
  // didn't match any registered vault — `vaults = [...]` in
  // `.memstead.toml` was an authored allowlist that included planning
  // vaults before they existed. The engine dump is now the source of
  // truth and only knows about vaults that actually have a vault-repo
  // branch, so a `scopes/<planning-vault>/` directory whose vault has
  // already been archived (or hasn't been created yet) is a transient
  // state, not an error. Warn-and-continue so unrelated ingests still
  // load; per-ingest fail-fast at run time covers the typo path.
  const scopes = {};
  const scopesDirAbs = join(root, dirs.scopes);
  if (existsSync(scopesDirAbs)) {
    for (const vaultDir of listSubdirs(scopesDirAbs)) {
      if (!vaultMeta[vaultDir]) {
        console.warn(
          `workspace-loader: scopes directory "${dirs.scopes}/${vaultDir}" does not match any registered vault (known: ${vaults.join(', ') || '(none)'}); ignoring — likely a planning vault that has not been created yet or has been archived`
        );
        continue;
      }
      scopes[vaultDir] = {};
      const scopeFiles = readdirSync(join(scopesDirAbs, vaultDir)).filter(f => f.endsWith('.json'));
      for (const f of scopeFiles) {
        const name = f.slice(0, -5);
        const fileAbs = join(scopesDirAbs, vaultDir, f);
        const content = readJson(fileAbs);
        validateOrThrow(schemasLoaded.scope, content, `scope ${dirs.scopes}/${vaultDir}/${f}`);
        scopes[vaultDir][name] = content;
      }
    }
  }

  // Walk projections: <root>/<projectionsDir>/<vault>/<name>.json
  //
  // Same softening as scopes/ above. A projection authored against an
  // unregistered vault stays loadable so unrelated ingests can still
  // run; the eventual error fires when an ingest actually targets the
  // missing vault.
  const projections = {};
  const projectionsDirAbs = join(root, dirs.projections);
  if (existsSync(projectionsDirAbs)) {
    for (const vaultDir of listSubdirs(projectionsDirAbs)) {
      if (!vaultMeta[vaultDir]) {
        console.warn(
          `workspace-loader: projections directory "${dirs.projections}/${vaultDir}" does not match any registered vault (known: ${vaults.join(', ') || '(none)'}); ignoring — likely a planning vault that has not been created yet or has been archived`
        );
        continue;
      }
      projections[vaultDir] = {};
      const projFiles = readdirSync(join(projectionsDirAbs, vaultDir)).filter(f => f.endsWith('.json'));
      for (const f of projFiles) {
        const name = f.slice(0, -5);
        const raw = readJson(join(projectionsDirAbs, vaultDir, f));
        validateOrThrow(schemasLoaded.projection, raw, `projection ${dirs.projections}/${vaultDir}/${f}`);
        projections[vaultDir][name] = resolveProjection(raw, vaultDir, scopes, vaultMeta, dirs);
      }
    }
  }

  // Walk ingests: <root>/<ingestsDir>/<name>.json
  //
  // An ingest whose projection has been dropped (its owning vault is
  // unregistered) is skipped with a warning rather than failing the
  // whole load — same rationale as the scopes/projections softening
  // above. Other ingests in the same workspace still run. The
  // structurally-broken cases (malformed JSON, missing `projection`
  // field) still hard-fail because they reflect authoring errors, not
  // transient vault state.
  const ingests = [];
  const ingestsDirAbs = join(root, dirs.ingests);
  if (existsSync(ingestsDirAbs)) {
    for (const f of readdirSync(ingestsDirAbs).filter(x => x.endsWith('.json'))) {
      const name = f.slice(0, -5);
      const raw = readJson(join(ingestsDirAbs, f));
      validateOrThrow(schemasLoaded.ingest, raw, `ingest ${dirs.ingests}/${f}`);
      try {
        ingests.push(assembleIngest(name, raw, projections, dirs));
      } catch (e) {
        // Distinguish authoring errors (malformed shape) from missing
        // projection-vault errors (transient). The "no projections
        // directory for vault" branch is the transient one — warn and
        // skip. Everything else propagates.
        if (/no projections directory for vault/.test(e.message)) {
          // `e.message` already carries the `workspace-loader:` prefix
          // from assembleIngest — emit it as-is to avoid a doubled prefix.
          console.warn(`${e.message}; skipping ingest "${name}"`);
        } else {
          throw e;
        }
      }
    }
  }
  ingests.sort((a, b) => a.name.localeCompare(b.name));

  return {
    workspaceRoot: root,
    dirs,
    vaults,
    vaultMeta,
    schemas,
    scopes,
    projections,
    ingests,
    format,
    /** Raw engine dump — kept for callers that want to inspect untreated fields. */
    dump,
  };
}

// ── Schema loading + validation helpers ─────────────────────────────────────

function loadSchemas() {
  const out = { memsteadToml: null, scope: null, projection: null, ingest: null };
  const map = {
    memsteadToml: 'memstead-toml.schema.json',
    scope: 'scope.schema.json',
    projection: 'projection.schema.json',
    ingest: 'ingest.schema.json',
  };
  for (const [key, file] of Object.entries(map)) {
    const path = join(SCHEMAS_ROOT, file);
    if (!existsSync(path)) {
      console.warn(
        `workspace-loader: schema ${file} not found at ${path}; skipping ${key} validation`
      );
      continue;
    }
    try {
      out[key] = JSON.parse(readFileSync(path, 'utf-8'));
    } catch (e) {
      console.warn(`workspace-loader: failed to parse schema ${file}: ${e.message}`);
    }
  }
  return out;
}

function validateOrThrow(schema, instance, subject) {
  if (!schema) return;
  const result = validateSchema(schema, instance);
  if (!result.valid) {
    const summary = result.errors
      .slice(0, 5)
      .map(e => `${e.path}: ${e.message}`)
      .join('\n  ');
    const more = result.errors.length > 5 ? `\n  …and ${result.errors.length - 5} more` : '';
    throw new Error(`workspace-loader: ${subject} failed schema validation:\n  ${summary}${more}`);
  }
}

function listSubdirs(dir) {
  return readdirSync(dir).filter(name => {
    try { return statSync(join(dir, name)).isDirectory(); }
    catch { return false; }
  });
}

function readJson(path) {
  try {
    return JSON.parse(readFileSync(path, 'utf-8'));
  } catch (e) {
    throw new Error(`workspace-loader: failed to parse JSON at ${path}: ${e.message}`);
  }
}

function resolveProjection(raw, owningVault, scopes, vaultMeta, dirs) {
  const sources = Array.isArray(raw.sources) ? raw.sources.map(s => resolveSource(s, owningVault, scopes, vaultMeta, dirs)) : [];
  const destinations = Array.isArray(raw.destinations) ? raw.destinations.map(d => resolveDestination(d, vaultMeta)) : [];
  return { ...raw, sources, destinations, _owningVault: owningVault };
}

function resolveSource(src, owningVault, scopes, vaultMeta, dirs) {
  if (!src || typeof src !== 'object') return src;
  const out = { ...src };

  if (typeof src.scope_ref === 'string') {
    const vaultScopes = scopes[owningVault] || {};
    const scope = vaultScopes[src.scope_ref];
    if (!scope) {
      const expectedPath = join(dirs.scopes, owningVault, `${src.scope_ref}.json`);
      const available = Object.keys(vaultScopes);
      throw new Error(
        `workspace-loader: scope_ref "${src.scope_ref}" in projection for vault "${owningVault}" not found; expected file at "${expectedPath}" (available in "${owningVault}": ${available.join(', ') || '(none)'})`
      );
    }
    out.scope = scope;
  }

  // Soft validation: an unregistered vault in a projection source is
  // fine at load time — planning vaults may legitimately not exist
  // yet. Per-ingest fail-fast at run time covers the typo path.
  if (typeof src.vault === 'string' && !vaultMeta[src.vault]) {
    console.warn(
      `workspace-loader: projection source references vault "${src.vault}" which is not a registered vault (known: ${Object.keys(vaultMeta).sort().join(', ') || '(none)'})`
    );
  }

  return out;
}

function resolveDestination(dst, vaultMeta) {
  if (!dst || typeof dst !== 'object') return dst;
  // Same softening as resolveSource — destination resolution is a
  // load-time concern; the actual write fails at agent runtime via
  // MCP if the vault isn't there.
  if (typeof dst.vault === 'string' && !vaultMeta[dst.vault]) {
    console.warn(
      `workspace-loader: projection destination references vault "${dst.vault}" which is not a registered vault (known: ${Object.keys(vaultMeta).sort().join(', ') || '(none)'})`
    );
  }
  return { ...dst };
}

function assembleIngest(name, raw, projections, dirs) {
  if (!raw || typeof raw !== 'object') {
    throw new Error(`workspace-loader: ingest "${name}" is not a JSON object`);
  }
  const ref = raw.projection;
  if (typeof ref !== 'string' || !ref.includes('/')) {
    throw new Error(
      `workspace-loader: ingest "${name}" missing or malformed "projection" field; expected "<vault>/<name>", got ${JSON.stringify(ref)}`
    );
  }
  const slash = ref.indexOf('/');
  const projVault = ref.slice(0, slash);
  const projName = ref.slice(slash + 1);
  const vaultProjections = projections[projVault];
  if (!vaultProjections) {
    throw new Error(
      `workspace-loader: ingest "${name}" references projection "${ref}" but no projections directory for vault "${projVault}" was found under "${dirs.projections}/"`
    );
  }
  const projection = vaultProjections[projName];
  if (!projection) {
    const expected = join(dirs.projections, projVault, `${projName}.json`);
    const available = Object.keys(vaultProjections);
    throw new Error(
      `workspace-loader: ingest "${name}" references projection "${ref}" not found; expected file at "${expected}" (available in "${projVault}": ${available.join(', ') || '(none)'})`
    );
  }
  return {
    name,
    mode: typeof raw.mode === 'string' ? raw.mode : 'discovery',
    trigger: typeof raw.trigger === 'string' ? raw.trigger : null,
    batch_size: typeof raw.batch_size === 'number' ? raw.batch_size : null,
    projection_ref: ref,
    projection_vault: projVault,
    projection_name: projName,
    projection,
    sources: projection.sources,
    destinations: projection.destinations,
    rules: projection.rules ?? null,
    raw,
  };
}
