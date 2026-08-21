// End-to-end tests for the ingest deny hook (`deny-meta-files.mjs`).
//
// The hook is a thin caller of `memstead projection check-path`, so these
// tests build a REAL workspace with the real CLI (mem-repo init + projection
// init), point the active-binding pointer at a binding, and spawn the actual
// hook — asserting the runtime exit codes, the never-stale property at the
// new seam (enforcement follows the ACTIVE binding, whose deny list is read
// fresh from its record on every check), and fail-open outside a workspace.
//
// Requires the built CLI (target/debug/memstead, or $MEMSTEAD_BIN). The
// suite SKIPS when no binary is available so an isolated `node --test` run
// stays green; under run-tests.sh the binary always exists.

import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  rmSync,
} from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';

const HOOKS_DIR = dirname(fileURLToPath(import.meta.url));
const HOOK = join(HOOKS_DIR, 'deny-meta-files.mjs');
const REPO_ROOT = resolve(HOOKS_DIR, '..', '..', '..');
const MEMSTEAD_BIN =
  process.env.MEMSTEAD_BIN || join(REPO_ROOT, 'target', 'debug', 'memstead');
const HAVE_BIN = existsSync(MEMSTEAD_BIN);

let tmp; // temp root
let ws; // the workspace root inside it

const pointerFile = () =>
  join(ws, '.memstead.cache', 'projection', 'active-binding.json');

/** Point enforcement at a binding (what a consuming brief render publishes). */
function activate(bindingId) {
  mkdirSync(dirname(pointerFile()), { recursive: true });
  writeFileSync(pointerFile(), JSON.stringify({ binding: bindingId }));
}

function deactivate() {
  rmSync(pointerFile(), { force: true });
}

/** Run the hook from `cwd = ws` with a tool_input; return exit status. */
function run(toolInput, cwd = ws) {
  const res = spawnSync('node', [HOOK], {
    input: JSON.stringify({ cwd, tool_input: toolInput }),
    encoding: 'utf-8',
    env: { ...process.env, MEMSTEAD_BIN },
  });
  return { status: res.status, stderr: res.stderr ?? '' };
}

function cli(args, cwd) {
  const res = spawnSync(MEMSTEAD_BIN, args, { cwd, encoding: 'utf-8' });
  assert.equal(
    res.status,
    0,
    `memstead ${args.join(' ')} failed:\n${res.stderr}`,
  );
}

before(function () {
  if (!HAVE_BIN) return;
  tmp = mkdtempSync(join(tmpdir(), 'memstead-deny-'));
  ws = join(tmp, 'ws');
  cli(['mem-repo', 'init', ws, '--no-gitignore'], tmp);
  // Two bindings: alpha carries an extra deny (`secrets/**`, patched into its
  // record the way an author would edit their binding); beta keeps only the
  // scaffold defaults.
  for (const name of ['alpha', 'beta']) {
    cli(
      [
        'projection',
        'init',
        '--mem',
        'ws',
        '--source',
        '../src',
        '--medium-type',
        'codebase',
        '--name',
        name,
      ],
      ws,
    );
  }
  const alphaRecord = join(ws, '.memstead', 'projections', 'ws', 'alpha.json');
  const record = JSON.parse(readFileSync(alphaRecord, 'utf-8'));
  record.deny_paths = [...(record.deny_paths ?? []), 'secrets/**'];
  writeFileSync(alphaRecord, JSON.stringify(record, null, 2));
});

after(() => {
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

describe('deny hook — runtime enforcement (engine-answered)', { skip: !HAVE_BIN && 'no memstead binary built' }, () => {
  it('blocks a Read denied by the active binding (exit 2)', () => {
    activate('ws/alpha');
    const res = run({ file_path: join(ws, 'secrets/key.txt') });
    assert.equal(res.status, 2);
    assert.match(res.stderr, /BLOCKED: secrets\/\*\*/);
  });

  it('blocks a Glob pattern that recurses a denied subtree (exit 2)', () => {
    activate('ws/alpha');
    assert.equal(run({ pattern: 'secrets/**/*.txt' }).status, 2);
  });

  it('blocks a Grep whose path targets a denied subtree (exit 2)', () => {
    activate('ws/alpha');
    assert.equal(run({ pattern: 'TODO', path: 'secrets' }).status, 2);
  });

  it('allows a non-denied Read (exit 0)', () => {
    activate('ws/alpha');
    assert.equal(run({ file_path: join(ws, 'crates/foo/lib.rs') }).status, 0);
  });

  it('fails open when no binding is active (exit 0)', () => {
    deactivate();
    assert.equal(run({ file_path: join(ws, 'secrets/key.txt') }).status, 0);
  });

  it('fails open when the active pointer names a missing binding (exit 0)', () => {
    activate('ws/ghost');
    assert.equal(run({ file_path: join(ws, 'secrets/key.txt') }).status, 0);
  });
});

describe('deny hook — never stale', { skip: !HAVE_BIN && 'no memstead binary built' }, () => {
  it('after the loop moves to binding beta, alpha’s denies no longer bite', () => {
    // Alpha active: its `secrets/**` deny is enforced.
    activate('ws/alpha');
    assert.equal(run({ file_path: join(ws, 'secrets/key.txt') }).status, 2);

    // The loop consumes beta's brief — the pointer moves. Alpha's extra deny
    // must not survive: beta's record (scaffold defaults only) is read fresh.
    activate('ws/beta');
    assert.equal(
      run({ file_path: join(ws, 'secrets/key.txt') }).status,
      0,
      "alpha's deny_paths must not survive the move to beta",
    );
    // Beta's own list is enforced instead (a scaffold-default entry).
    assert.equal(run({ file_path: join(ws, 'x/.DS_Store') }).status, 2);
  });

  it('fails open (exit 0) when cwd is not inside any workspace', () => {
    const outside = mkdtempSync(join(tmpdir(), 'memstead-nows-'));
    try {
      const res = run(
        { file_path: join(outside, 'secrets/x.md') },
        outside,
      );
      assert.equal(res.status, 0, 'no workspace resolvable → inert (fail open)');
    } finally {
      rmSync(outside, { recursive: true, force: true });
    }
  });
});
