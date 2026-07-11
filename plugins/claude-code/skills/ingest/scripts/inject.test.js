// Router tests for `inject.mjs` — the `/ingest` thin client.
//
// The router shells out to `memstead` (overridable via MEMSTEAD_BIN). These
// tests point MEMSTEAD_BIN at a stub that logs its argv and emits a canned
// response keyed by STUB_MODE, so we can assert the router's routing and
// output shaping without a live engine:
//   - `projection brief --all` / `<binding>` under `--json`, and how the
//     router branches on {brief} | {skipped} | {no_bindings} | error;
//   - the no-source setup ramp (three plain questions, no configuration
//     vocabulary the user must produce);
//   - `--clear <binding>` → `mem delete <binding>` (deleted / already-absent /
//     failed);
//   - engine refusals surfaced verbatim.
//
// This file is what makes the `skills/ingest/scripts/*.test.js` leg in
// run-tests.sh non-vacuous — before it, that glob matched zero files and the
// router had zero coverage while looking covered.

import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, chmodSync, readFileSync, rmSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';

const SCRIPTS_DIR = dirname(fileURLToPath(import.meta.url));
const ROUTER = join(SCRIPTS_DIR, 'inject.mjs');

// A stub `memstead` binary: logs argv to $STUB_LOG (one JSON array per line)
// and emits a canned response selected by $STUB_MODE.
const STUB_SRC = `#!/usr/bin/env node
import { appendFileSync } from 'node:fs';
const args = process.argv.slice(2);
if (process.env.STUB_LOG) appendFileSync(process.env.STUB_LOG, JSON.stringify(args) + '\\n');
const mode = process.env.STUB_MODE || '';
const out = (s) => process.stdout.write(s);
const err = (s) => process.stderr.write(s);
switch (mode) {
  case 'brief-nobind': out(JSON.stringify({ no_bindings: true })); process.exit(0);
  case 'brief-ok': out(JSON.stringify({ brief: '# Build brief\\nDo the thing.\\n' })); process.exit(0);
  case 'brief-skipped': out(JSON.stringify({ skipped: true })); process.exit(0);
  case 'brief-error':
    out(JSON.stringify({ code: 'PROJECTION_NOT_FOUND', message: 'binding "x/y" not found' }));
    process.exit(3);
  case 'delete-ok': process.exit(0);
  case 'delete-absent': err('memstead: ERROR [UNKNOWN_MEM]: unknown mem: app/graph\\n'); process.exit(3);
  case 'delete-fail': err('memstead: ERROR [GENERIC]: disk on fire\\n'); process.exit(1);
  default: process.exit(0);
}
`;

let dir;
let stub;
let logFile;

before(() => {
  dir = mkdtempSync(join(tmpdir(), 'ingest-router-'));
  stub = join(dir, 'memstead-stub.mjs');
  logFile = join(dir, 'calls.log');
  writeFileSync(stub, STUB_SRC);
  chmodSync(stub, 0o755);
});

after(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
});

/** Run the router with the given args and STUB_MODE; return {stdout, calls}. */
function runRouter(routerArgs, mode) {
  if (existsSync(logFile)) rmSync(logFile);
  const res = spawnSync('node', [ROUTER, ...routerArgs], {
    encoding: 'utf-8',
    env: { ...process.env, MEMSTEAD_BIN: stub, STUB_MODE: mode, STUB_LOG: logFile },
  });
  const calls = existsSync(logFile)
    ? readFileSync(logFile, 'utf-8').split('\n').filter(Boolean).map((l) => JSON.parse(l))
    : [];
  return { stdout: res.stdout ?? '', status: res.status, calls };
}

describe('ingest router — brief routing', () => {
  it('renders a due binding brief verbatim', () => {
    const { stdout, calls } = runRouter(['--all'], 'brief-ok');
    assert.equal(stdout, '# Build brief\nDo the thing.\n');
    assert.deepEqual(calls[0], ['--json', 'projection', 'brief', '--all']);
  });

  it('routes a named binding to `projection brief <binding>`', () => {
    const { calls } = runRouter(['app/graph'], 'brief-ok');
    assert.deepEqual(calls[0], ['--json', 'projection', 'brief', 'app/graph']);
  });

  it('reports the backing-off pass without a brief', () => {
    const { stdout } = runRouter(['--all'], 'brief-skipped');
    assert.match(stdout, /backoff/i);
    assert.doesNotMatch(stdout, /Build brief/);
  });

  it('surfaces an engine refusal verbatim', () => {
    const { stdout } = runRouter(['x/y'], 'brief-error');
    assert.match(stdout, /binding "x\/y" not found/);
  });

  it('points at /setup when the memstead binary is missing (never an empty prompt)', () => {
    if (existsSync(logFile)) rmSync(logFile);
    const res = spawnSync('node', [ROUTER, '--all'], {
      encoding: 'utf-8',
      env: { ...process.env, MEMSTEAD_BIN: join(dir, 'no-such-binary') },
    });
    assert.match(res.stdout, /run \/setup first/);
    assert.notEqual(res.stdout.trim(), '', 'the agent must never receive an empty prompt');
  });
});

describe('ingest router — no-source setup ramp', () => {
  it('asks exactly three plain questions and then sets up silently', () => {
    const { stdout } = runRouter(['--all'], 'brief-nobind');

    // The three user-facing questions, in plain language.
    assert.match(stdout, /Where should Claude read from\?/);
    assert.match(stdout, /What should this mem capture from it\?/);
    assert.match(stdout, /Which mem should it go into\?/);

    // One-at-a-time, no-jargon instruction.
    assert.match(stdout, /one at a time/i);

    // The silent, non-interactive setup call.
    assert.match(stdout, /memstead projection init/);

    // The user is never asked to produce configuration vocabulary: everything
    // the user reads (before the literal setup command) is free of the
    // taxonomy nouns. The command itself may name the real subcommand/flag.
    const userFacing = stdout.slice(0, stdout.indexOf('memstead projection init'));
    assert.doesNotMatch(
      userFacing,
      /\b(medium|facet|binding|projection)\b/i,
      'the ramp the user reads must carry no taxonomy nouns',
    );
  });

  it('does not call the engine again on the no-source path (brief only)', () => {
    const { calls } = runRouter(['--all'], 'brief-nobind');
    // Only the `brief --all` probe ran; no init is spawned by the router — the
    // agent runs init after the conversation.
    assert.equal(calls.length, 1);
    assert.deepEqual(calls[0], ['--json', 'projection', 'brief', '--all']);
  });
});

describe('ingest router — --clear <binding>', () => {
  it('deletes the paired process mem for the named binding', () => {
    const { stdout, calls } = runRouter(['--clear', 'app/graph'], 'delete-ok');
    assert.deepEqual(calls[0].slice(0, 3), ['mem', 'delete', 'app/graph']);
    assert.match(stdout, /app\/graph — deleted/);
  });

  it('is idempotent — an already-absent mem is not an error', () => {
    const { stdout } = runRouter(['--clear', 'app/graph'], 'delete-absent');
    assert.match(stdout, /already absent/);
  });

  it('reports a genuine clear failure', () => {
    const { stdout } = runRouter(['--clear', 'app/graph'], 'delete-fail');
    assert.match(stdout, /clear failed/);
  });

  it('refuses --clear with no binding (per-binding, no global form)', () => {
    const { stdout, calls } = runRouter(['--clear'], 'delete-ok');
    assert.match(stdout, /Usage: \/ingest --clear <binding>/);
    assert.equal(calls.length, 0, 'no mem delete is attempted without a binding');
  });
});
