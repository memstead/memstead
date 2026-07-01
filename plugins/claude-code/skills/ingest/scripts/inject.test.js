/**
 * inject.test.js — unit tests for the rebuilt ingest skill's inject.mjs.
 *
 * Covers the three new shapes:
 *
 *   1. Render structure
 *      - Situation block opens the prompt.
 *      - Goal / Failure modes blocks render the destination schema's
 *        `default_writing_guidance` verbatim (no skill-side restatement).
 *      - No "1. Orient. 2. Read. ..." step lists.
 *      - No numerical entity targets, no behavioral exhortations.
 *
 *   2. Auto-create branch — first run of a discovery/refinement ingest
 *      whose paired `ingest/<name>` vault is absent issues a single
 *      `memstead vault init <name> --org-path ingest --schema ingest@0.1.0`
 *      via the operator-mode CLI before emitting the prompt.
 *
 *   3. `--clear <name>` branch — invokes `memstead vault delete <name>`
 *      via the operator-mode CLI; a non-existent vault is reported as
 *      already-absent (exit 0).
 *
 * The tests run `inject.mjs` as a subprocess against a tempdir
 * workspace. The `memstead` binary is a shell-script stand-in
 * (`test-fixtures/fake-memstead`) that reads a pre-written workspace dump
 * and records each invocation to a JSON-lines log so tests can assert
 * which CLI calls were made.
 */

import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  rmSync,
  existsSync,
  readFileSync,
} from 'node:fs';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const INJECT = fileURLToPath(new URL('./inject.mjs', import.meta.url));
const FAKE_MEMSTEAD = fileURLToPath(new URL('./test-fixtures/fake-memstead', import.meta.url));

// ── Workspace builder ───────────────────────────────────────────────────────

function makeWorkspace(files) {
  const root = mkdtempSync(join(tmpdir(), 'memstead-ingest-rebuild-'));
  for (const [rel, content] of Object.entries(files)) {
    const abs = join(root, rel);
    mkdirSync(dirname(abs), { recursive: true });
    writeFileSync(
      abs,
      typeof content === 'string' ? content : JSON.stringify(content, null, 2),
    );
  }
  return root;
}

function cleanup(root) {
  try { rmSync(root, { recursive: true, force: true }); } catch {}
}

/**
 * Drop a workspace-dump fixture next to `.memstead.toml`. The fake-memstead
 * stand-in cats it on `memstead workspace dump`. `vaults` is a list of
 * `{name, schema?, writeGuidance?, snapshot_token?}`; `schemas` is
 * `{name → {avoid?, goal?}}` — each value is wrapped under
 * `default_writing_guidance` to match the engine's dump shape.
 */
function writeFakeDump(root, { vaults, schemas = {} }) {
  const dump = {
    format: 'workspace-dump/v0',
    workspace_root: root,
    vaults: vaults.map((v, i) => ({
      name: v.name,
      schema: v.schema ?? null,
      description: v.description ?? null,
      writeGuidance: v.writeGuidance ?? {},
      snapshot_token: v.snapshot_token ?? `token-${i}`,
    })),
    schemas: Object.fromEntries(
      Object.entries(schemas).map(([k, v]) => [k, { default_writing_guidance: v }]),
    ),
  };
  writeFileSync(join(root, '.fake-dump.json'), JSON.stringify(dump, null, 2));
}

function runInject(root, args = [], extraEnv = {}) {
  const env = {
    ...process.env,
    MEMSTEAD_INGEST_QUIET: '1',
    CLAUDE_SKILL_DIR: root,
    MEMSTEAD_BIN: FAKE_MEMSTEAD,
    ...extraEnv,
  };
  const res = spawnSync('node', [INJECT, ...args], {
    cwd: root,
    env,
    encoding: 'utf-8',
  });
  return { stdout: res.stdout, stderr: res.stderr, status: res.status };
}

function readInvocations(root) {
  const path = join(root, '.fake-invocations.log');
  if (!existsSync(path)) return [];
  return readFileSync(path, 'utf8')
    .split(/\r?\n/)
    .filter(Boolean)
    .map(line => JSON.parse(line));
}

/**
 * One discovery ingest, one refinement ingest, one one-shot ingest.
 * Each writes into its own destination vault; refinement and discovery
 * pair with auto-creatable `ingest/<name>` process vaults, one-shot
 * does not. The destination schemas carry distinctive `default_writing_guidance`
 * fixtures so the tests can match against schema-rendered output.
 */
function buildWorkspace({ withProcessVault = false } = {}) {
  const toml = `format = "memstead-plugin/v0"\n`;

  // Source files referenced by the codebase facet so refinement's
  // file-batch enumeration produces non-empty batches.
  const srcFiles = {
    'sources/engine/lib.rs': '// engine source',
    'sources/engine/main.rs': '// engine main',
  };

  // Four-primitive workspace store: a codebase Medium, a Facet selecting it,
  // a Projection mapping the facet to the destination vault, and three ingests.
  const files = {
    '.memstead.toml': toml,
    '.memstead/mediums/engine-dest/src.json': {
      name: 'src',
      type: 'codebase',
      pointer: 'sources/engine',
    },
    '.memstead/facets/engine-dest/src.json': {
      name: 'src',
      medium: 'src',
      scope: [{ path: 'sources/engine/**/*.rs', mode: 'allow' }],
    },
    '.memstead/projections/engine-dest/graph.json': {
      source_facets: ['src'],
      destination_vault: 'engine-dest',
    },
    '.memstead/ingests/discovery-run.json': {
      projection: 'engine-dest/graph',
      mode: 'discovery',
      trigger: 'manual',
    },
    '.memstead/ingests/refinement-run.json': {
      projection: 'engine-dest/graph',
      mode: 'refinement',
      trigger: 'loop',
      batch_size: 5,
    },
    '.memstead/ingests/one-shot-run.json': {
      projection: 'engine-dest/graph',
      mode: 'one-shot',
      trigger: 'manual',
    },
    ...srcFiles,
  };

  const root = makeWorkspace(files);

  // Distinctive schema-side guidance — tests grep for these to verify
  // goal/avoid blocks come from the schema, not the skill.
  const schemas = {
    'sample@0.1.0': {
      avoid: '- TEST_AVOID_MARKER: do not duplicate entities.',
      goal: 'TEST_GOAL_MARKER: build a graph an LLM can rebuild from.',
    },
    'ingest@0.1.0': {
      avoid: '- INGEST_AVOID_MARKER: leave no stale quality-entries after fix.',
      goal: 'INGEST_GOAL_MARKER: scaffolding for destination quality.',
    },
  };

  const baseVaults = [
    { name: 'engine-dest', schema: 'sample@0.1.0' },
  ];
  const vaults = withProcessVault
    ? [
        ...baseVaults,
        { name: 'discovery-run', schema: 'ingest@0.1.0' },
        { name: 'refinement-run', schema: 'ingest@0.1.0' },
      ]
    : baseVaults;

  writeFakeDump(root, { vaults, schemas });
  return root;
}

// ═══════════════════════════════════════════════════════════════════════════
//  RENDER STRUCTURE
// ═══════════════════════════════════════════════════════════════════════════

describe('inject.mjs — render structure (discovery mode)', () => {
  let root;
  beforeEach(() => { root = buildWorkspace({ withProcessVault: true }); });
  afterEach(() => cleanup(root));

  it('opens with a Situation block', () => {
    const r = runInject(root, ['discovery-run']);
    assert.equal(r.status, 0, `stderr:\n${r.stderr}`);
    // Situation must be the first heading in the prompt.
    const firstHeading = r.stdout.match(/^##\s+(.+)$/m);
    assert.ok(firstHeading, 'expected at least one ## heading');
    assert.equal(firstHeading[1].trim(), 'Situation');
  });

  it('renders Goal / Failure-modes verbatim from the destination schema', () => {
    const r = runInject(root, ['discovery-run']);
    assert.match(r.stdout, /^## Goal$/m);
    assert.match(r.stdout, /TEST_GOAL_MARKER: build a graph an LLM can rebuild from\./);
    assert.match(r.stdout, /^## Failure modes to avoid$/m);
    assert.match(r.stdout, /TEST_AVOID_MARKER: do not duplicate entities\./);
  });

  it('contains no how-to-do-it step lists, no numerical targets, no exhortations', () => {
    const r = runInject(root, ['discovery-run']);
    // Step lists like "1. Orient → 2. Read → ..."
    assert.doesNotMatch(r.stdout, /^\s*1\.\s+(Orient|Read|Write|Repeat)\b/m);
    // Numerical entity targets such as "15-30+ entities"
    assert.doesNotMatch(r.stdout, /\b\d+\s*-\s*\d+\+?\s+entities\b/i);
    // Behavioral exhortations frozen-in pre-rebuild
    assert.doesNotMatch(r.stdout, /Never report no changes needed/i);
    assert.doesNotMatch(r.stdout, /spot-checking is not reading/i);
  });

  it('Operative-data section names sources, destination, and the paired process vault', () => {
    const r = runInject(root, ['discovery-run']);
    assert.match(r.stdout, /^## Operative data$/m);
    assert.match(r.stdout, /^### Sources$/m);
    assert.match(r.stdout, /^### Destination$/m);
    assert.match(r.stdout, /^### Paired process vault$/m);
    assert.match(r.stdout, /\*\*ingest\/discovery-run\*\*/);
    assert.match(r.stdout, /ingest@0\.1\.0/);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
//  AUTO-CREATE BRANCH
// ═══════════════════════════════════════════════════════════════════════════

describe('inject.mjs — process-vault auto-create', () => {
  let root;
  afterEach(() => cleanup(root));

  it('issues `memstead vault init <name> --org-path ingest --schema ingest@0.1.0` for a discovery ingest whose vault is absent', () => {
    root = buildWorkspace({ withProcessVault: false });
    const r = runInject(root, ['discovery-run']);
    assert.equal(r.status, 0, `stderr:\n${r.stderr}`);
    const calls = readInvocations(root);
    const inits = calls.filter(c => c.argv[0] === 'vault' && c.argv[1] === 'init');
    assert.equal(inits.length, 1, `expected exactly one vault init call, got ${inits.length}: ${JSON.stringify(inits)}`);
    const argv = inits[0].argv;
    assert.deepEqual(
      argv.slice(0, 7),
      ['vault', 'init', 'discovery-run', '--org-path', 'ingest', '--schema', 'ingest@0.1.0'],
    );
  });

  it('does not create a process vault for one-shot ingests', () => {
    root = buildWorkspace({ withProcessVault: false });
    const r = runInject(root, ['one-shot-run']);
    assert.equal(r.status, 0, `stderr:\n${r.stderr}`);
    const calls = readInvocations(root);
    const inits = calls.filter(c => c.argv[0] === 'vault' && c.argv[1] === 'init');
    assert.equal(inits.length, 0, `expected no vault init calls, got: ${JSON.stringify(inits)}`);
    // And the prompt must not advertise a paired process vault.
    assert.doesNotMatch(r.stdout, /^### Paired process vault$/m);
  });

  it('skips creation when the process vault is already present', () => {
    root = buildWorkspace({ withProcessVault: true });
    const r = runInject(root, ['discovery-run']);
    assert.equal(r.status, 0, `stderr:\n${r.stderr}`);
    const calls = readInvocations(root);
    const inits = calls.filter(c => c.argv[0] === 'vault' && c.argv[1] === 'init');
    assert.equal(inits.length, 0, 'should not re-create an existing process vault');
  });

  it('continues with a notice when auto-create fails', () => {
    root = buildWorkspace({ withProcessVault: false });
    writeFileSync(join(root, '.fake-vault-init-mode'), 'fail');
    const r = runInject(root, ['discovery-run']);
    assert.equal(r.status, 0, `stderr:\n${r.stderr}`);
    // Prompt still emits, situation block carries a notice referencing
    // the operator-recovery command.
    assert.match(r.stdout, /^## Situation$/m);
    assert.match(r.stdout, /could not be auto-created/);
    assert.match(r.stdout, /memstead vault init discovery-run --org-path ingest --schema ingest@0\.1\.0/);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
//  --clear BRANCH
// ═══════════════════════════════════════════════════════════════════════════

describe('inject.mjs — --clear', () => {
  let root;
  afterEach(() => cleanup(root));

  it('deletes the named process vault via the operator-mode CLI', () => {
    root = buildWorkspace({ withProcessVault: true });
    const r = runInject(root, ['--clear', 'discovery-run']);
    assert.equal(r.status, 0, `stderr:\n${r.stderr}`);
    assert.match(r.stdout, /ingest\/discovery-run — deleted\./);
    const calls = readInvocations(root);
    const deletes = calls.filter(c => c.argv[0] === 'vault' && c.argv[1] === 'delete');
    assert.equal(deletes.length, 1);
    assert.deepEqual(deletes[0].argv.slice(0, 3), ['vault', 'delete', 'discovery-run']);
  });

  it('reports already-absent (exit 0) when the vault does not exist', () => {
    root = buildWorkspace({ withProcessVault: false });
    writeFileSync(join(root, '.fake-vault-delete-mode'), 'absent');
    const r = runInject(root, ['--clear', 'never-existed']);
    assert.equal(r.status, 0);
    assert.match(r.stdout, /ingest\/never-existed — already absent\./);
  });

  it('treats `--clear` without a name as a usage error (exit 0 with usage line)', () => {
    root = buildWorkspace({ withProcessVault: false });
    const r = runInject(root, ['--clear']);
    assert.equal(r.status, 0);
    assert.match(r.stdout, /Usage:/);
    // No vault delete CLI invocation issued.
    const calls = readInvocations(root);
    const deletes = calls.filter(c => c.argv[0] === 'vault' && c.argv[1] === 'delete');
    assert.equal(deletes.length, 0);
  });
});

// ═══════════════════════════════════════════════════════════════════════════
//  UNSUPPORTED PREPARATION
// ═══════════════════════════════════════════════════════════════════════════

describe('inject.mjs — unsupported preparation', () => {
  let root;
  afterEach(() => cleanup(root));

  it('reports an ingest unsupported (not silently skipped) when its facet declares a preparation step', () => {
    root = buildWorkspace({ withProcessVault: true });
    // Add a preparation step to the source facet — no implementation exists.
    writeFileSync(
      join(root, '.memstead/facets/engine-dest/src.json'),
      JSON.stringify({
        name: 'src',
        medium: 'src',
        scope: [{ path: 'sources/engine/**/*.rs', mode: 'allow' }],
        preparation: 'pdf-to-markdown',
      })
    );
    const r = runInject(root, ['discovery-run']);
    // Reported unsupported, naming the missing preparation type…
    assert.match(r.stdout, /unsupported/i);
    assert.match(r.stdout, /pdf-to-markdown/);
    // …and the agent prompt is NOT rendered (no Situation block opener).
    assert.doesNotMatch(r.stdout, /## Situation/);
  });
});
