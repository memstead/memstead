#!/usr/bin/env node
/**
 * workspace-loader.mjs — load the externalised workspace config and the
 * engine-supplied workspace dump.
 *
 * Reads `<workspace_root>/.memstead.toml` for plugin-side keys (`format`).
 * Walks the four-primitive workspace store:
 *   <workspace>/.memstead/mediums/<vault>/<name>.json
 *   <workspace>/.memstead/facets/<vault>/<name>.json
 *   <workspace>/.memstead/projections/<vault>/<name>.json
 *   <workspace>/.memstead/ingests/<name>.json
 * and translates each Facet (+ its Medium) into the per-source engagement
 * object the rest of the loader and `inject.mjs` consume.
 *
 * Per-vault config (`schema`, `writeGuidance`, `description`), per-vault
 * `snapshot_token`, and per-schema `default_writing_guidance` come from
 * `memstead workspace dump --json` — invoked once per `loadWorkspace` call.
 * The plugin no longer reads `<vault>/.mdgv/config.json`, no longer
 * walks vault `**.md` for backoff hashes, and no longer reads schema
 * YAML from disk. The engine's storage backend (vault-db-git or
 * legacy disk-flat) is private to it.
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
 * Extract the plugin-side keys from `.memstead.toml`. Only `format` is read
 * now — the former `scopes_dir`/`projections_dir`/`ingests_dir` keys are
 * retired (pipeline configs live at the fixed `.memstead/` store locations).
 * The `vaults` array was historically read here as a vault registry; that
 * role is fulfilled by the engine's workspace dump. Engine-only keys and any
 * other top-level keys are silently ignored.
 */
export function loadMemsteadToml(tomlPath) {
  const src = readFileSync(tomlPath, 'utf-8');
  const lines = src.split(/\r?\n/);

  let format = null;

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
    }
    i++;
  }

  return { format };
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

  // Fixed four-primitive store locations under `.memstead/` (surfaced for
  // diagnostic messages). The legacy `scopes_dir`/`projections_dir`/
  // `ingests_dir` keys are no longer consulted.
  const dirs = {
    mediums: '.memstead/mediums',
    facets: '.memstead/facets',
    projections: '.memstead/projections',
    ingests: '.memstead/ingests',
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
      // Engine-held source-change baseline, keyed per `<ingest>/<facet>`.
      // The dump emits opaque string tokens; the ingest loop diffs the
      // current source state against these to steer at the changed slice.
      // Omitted-when-empty on the wire, so default to `{}`.
      syncState: (v.sync_state && typeof v.sync_state === 'object') ? v.sync_state : {},
    };
  }
  const vaults = Object.keys(vaultMeta).sort();
  const schemas = (dump.schemas && typeof dump.schemas === 'object') ? dump.schemas : {};

  const schemasLoaded = loadSchemas();

  // Pipeline configs come from the four-primitive workspace store
  // (`.memstead/{mediums,facets,projections,ingests}/`, written by `memstead
  // pipeline migrate`). The legacy `scopes|projections|ingests/` folders are
  // no longer read — `memstead pipeline migrate` is the only path from
  // old-shape configs into the store. Absent store directories resolve to
  // empty. The result is the internal shape (`facetViews`/`projections`/
  // `ingests`) the rest of the loader and `inject.mjs` consume.
  const { facetViews, projections, ingests } = loadFourPrimitiveStore(root, vaultMeta, schemasLoaded);

  return {
    workspaceRoot: root,
    dirs,
    vaults,
    vaultMeta,
    schemas,
    facetViews,
    projections,
    ingests,
    format,
    /** Raw engine dump — kept for callers that want to inspect untreated fields. */
    dump,
  };
}

// ── Schema loading + validation helpers ─────────────────────────────────────

function loadSchemas() {
  const out = {
    memsteadToml: null,
    projection: null,
    ingest: null,
    medium: null,
    facet: null,
  };
  const map = {
    memsteadToml: 'memstead-toml.schema.json',
    projection: 'projection.schema.json',
    ingest: 'ingest.schema.json',
    medium: 'medium.schema.json',
    facet: 'facet.schema.json',
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
    deny_paths: Array.isArray(raw.deny_paths)
      ? raw.deny_paths.filter((s) => typeof s === 'string' && s.length > 0)
      : [],
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

// ── Four-primitive store reader (`.memstead/{mediums,facets,projections,ingests}/`) ──

/**
 * Read the four-primitive workspace store and translate it to the same
 * internal `{scopes, projections, ingests}` shape the legacy reader produces,
 * so `inject.mjs` and the rest of the loader are unchanged. A legacy scope's
 * `{type, scope:{tree}}` object is reconstructed from each Facet plus the
 * Medium it references (`facet.medium` → that vault's medium → its `type`),
 * and a four-primitive Projection (`source_facets` / `reference_vaults` /
 * `destination_vault`) is translated back into the assembled
 * `{sources:[{role, scope_ref|vault, scope}], destinations:[{vault}]}` form.
 *
 * Engagement metadata still comes from the skill's `mediums.json` (keyed by
 * medium type) — moving it into per-facet `engagement` records is a separate
 * step; this reader does not yet consume `facet.engagement`.
 */
function loadFourPrimitiveStore(root, vaultMeta, schemasLoaded) {
  const storeDir = join(root, '.memstead');
  const mediums = readStoreVaultScoped(join(storeDir, 'mediums'), schemasLoaded.medium, 'medium', vaultMeta);
  const facets = readStoreVaultScoped(join(storeDir, 'facets'), schemasLoaded.facet, 'facet', vaultMeta);

  // Build a per-vault facet view keyed by facet name: the medium type it
  // engages (from its referenced medium) plus its allow/deny selection. This
  // is the engagement object each projection source carries.
  const facetViews = {};
  for (const [vault, facetMap] of Object.entries(facets)) {
    facetViews[vault] = {};
    for (const [name, facet] of Object.entries(facetMap)) {
      const medium = (mediums[vault] || {})[facet.medium];
      const mediumType = medium ? medium.type : 'codebase';
      facetViews[vault][name] = {
        mediumType,
        // Where the medium's body lives (path/URL/vault id), and its
        // declared change-detection strategy. Both flow from the Medium
        // so `inject.mjs` can resolve the source-cursor capability
        // without re-reading the medium file. `change_detection` is
        // optional (defaults to `auto` at resolution time).
        mediumPointer: medium && typeof medium.pointer === 'string' ? medium.pointer : '',
        changeDetection: medium && typeof medium.change_detection === 'string'
          ? medium.change_detection
          : null,
        scope: { tree: Array.isArray(facet.scope) ? facet.scope : [] },
        // A deterministic preparation step (e.g. `pdf-to-markdown`). No
        // implementation exists yet — `inject.mjs` reports any ingest whose
        // facet declares one as unsupported rather than silently skipping it.
        preparation: typeof facet.preparation === 'string' ? facet.preparation : null,
      };
    }
  }

  // Projections: translate the four-primitive shape to the assembled form.
  const projections = {};
  const projectionsDir = join(storeDir, 'projections');
  if (existsSync(projectionsDir)) {
    for (const vault of listSubdirs(projectionsDir)) {
      if (!vaultMeta[vault]) { warnStoreUnregistered('projections', vault, vaultMeta); continue; }
      projections[vault] = {};
      for (const f of readdirSync(join(projectionsDir, vault)).filter(x => x.endsWith('.json'))) {
        const name = f.slice(0, -5);
        const raw = readJson(join(projectionsDir, vault, f));
        // `projection.schema.json` is a oneOf of the four-primitive and the
        // legacy shape; the four-primitive branch validates `source_facets` /
        // `reference_vaults` / `destination_vault` here.
        validateOrThrow(schemasLoaded.projection, raw, `projection .memstead/projections/${vault}/${f}`);
        const sources = [];
        for (const facetName of (raw.source_facets || [])) {
          sources.push({ role: 'primary', facet_ref: facetName, facet: (facetViews[vault] || {})[facetName] });
        }
        for (const refVault of (raw.reference_vaults || [])) {
          sources.push({ role: 'reference', vault: refVault });
        }
        const destinations = (typeof raw.destination_vault === 'string' && raw.destination_vault)
          ? [{ vault: raw.destination_vault }]
          : [];
        projections[vault][name] = { ...raw, sources, destinations, rules: raw.rules ?? null, _owningVault: vault };
      }
    }
  }

  // Ingests: flat, unchanged shape — reuse assembleIngest against the
  // translated projections.
  const ingests = [];
  const ingestsDir = join(storeDir, 'ingests');
  const dirs = { projections: '.memstead/projections', ingests: '.memstead/ingests' };
  if (existsSync(ingestsDir)) {
    for (const f of readdirSync(ingestsDir).filter(x => x.endsWith('.json'))) {
      const name = f.slice(0, -5);
      const raw = readJson(join(ingestsDir, f));
      validateOrThrow(schemasLoaded.ingest, raw, `ingest .memstead/ingests/${f}`);
      try {
        ingests.push(assembleIngest(name, raw, projections, dirs));
      } catch (e) {
        if (/no projections directory for vault/.test(e.message)) {
          console.warn(`${e.message}; skipping ingest "${name}"`);
        } else {
          throw e;
        }
      }
    }
  }
  ingests.sort((a, b) => a.name.localeCompare(b.name));
  return { facetViews, projections, ingests };
}

function readStoreVaultScoped(dir, schema, kind, vaultMeta) {
  const out = {};
  if (!existsSync(dir)) return out;
  for (const vault of listSubdirs(dir)) {
    if (!vaultMeta[vault]) { warnStoreUnregistered(`${kind}s`, vault, vaultMeta); continue; }
    out[vault] = {};
    for (const f of readdirSync(join(dir, vault)).filter(x => x.endsWith('.json'))) {
      const name = f.slice(0, -5);
      const content = readJson(join(dir, vault, f));
      validateOrThrow(schema, content, `${kind} .memstead/${kind}s/${vault}/${f}`);
      out[vault][name] = content;
    }
  }
  return out;
}

function warnStoreUnregistered(kindPlural, vault, vaultMeta) {
  console.warn(
    `workspace-loader: .memstead/${kindPlural}/${vault} does not match any registered vault ` +
    `(known: ${Object.keys(vaultMeta).sort().join(', ') || '(none)'}); ignoring`
  );
}
