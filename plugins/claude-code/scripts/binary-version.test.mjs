// Tests for the recorded-binary-version capability gate (binary-version.mjs).

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import {
  parseVersion,
  isAtLeast,
  ANCHORS_MIN,
  recordBinaryVersion,
  readRecordedVersion,
  anchorsGate,
} from './binary-version.mjs';

function ws() {
  return mkdtempSync(join(tmpdir(), 'binver-'));
}

test('parseVersion extracts major/minor/patch from the CLI banner', () => {
  assert.deepEqual(parseVersion('memstead 0.2.0'), { major: 0, minor: 2, patch: 0 });
  assert.deepEqual(parseVersion('memstead 1.13.4\n'), { major: 1, minor: 13, patch: 4 });
  assert.equal(parseVersion('no version here'), null);
  assert.equal(parseVersion(undefined), null);
});

test('isAtLeast implements semver >=', () => {
  assert.ok(isAtLeast({ major: 0, minor: 3, patch: 0 }, ANCHORS_MIN));
  assert.ok(isAtLeast({ major: 0, minor: 3, patch: 5 }, ANCHORS_MIN));
  assert.ok(isAtLeast({ major: 1, minor: 0, patch: 0 }, ANCHORS_MIN));
  assert.ok(!isAtLeast({ major: 0, minor: 2, patch: 9 }, ANCHORS_MIN));
  assert.ok(!isAtLeast(null, ANCHORS_MIN));
});

test('record → read round-trips the version', () => {
  const root = ws();
  const fakeRun = () => ({ status: 0, stdout: 'memstead 0.2.0\n' });
  const r = recordBinaryVersion(root, { run: fakeRun });
  assert.ok(r.ok);
  assert.deepEqual(readRecordedVersion(root), { major: 0, minor: 2, patch: 0 });
  rmSync(root, { recursive: true, force: true });
});

test('a failed `--version` call records nothing and reports why', () => {
  const root = ws();
  const r = recordBinaryVersion(root, { run: () => ({ status: 127, stderr: 'not found' }) });
  assert.ok(!r.ok);
  assert.match(r.reason, /failed/);
  assert.equal(readRecordedVersion(root), null);
  rmSync(root, { recursive: true, force: true });
});

test('gate: capable only when a recorded version >= threshold', () => {
  const root = ws();
  recordBinaryVersion(root, { run: () => ({ status: 0, stdout: 'memstead 0.3.0' }) });
  const g = anchorsGate(root);
  assert.equal(g.capable, true);
  assert.match(g.reason, /supports anchors/);
  rmSync(root, { recursive: true, force: true });
});

test('gate FAILS CLOSED with no record — degraded, with a printable reason', () => {
  const root = ws();
  const g = anchorsGate(root);
  assert.equal(g.capable, false);
  assert.equal(g.version, null);
  assert.match(g.reason, /no recorded binary version/);
  assert.match(g.reason, /without anchors/);
  rmSync(root, { recursive: true, force: true });
});

test('gate FAILS CLOSED for a below-threshold recorded version', () => {
  const root = ws();
  mkdirSync(join(root, '.memstead.cache/plugin'), { recursive: true });
  // 0.2.0 predates anchors (they land in 0.3.0) — the recorded binary must
  // fail closed rather than pass the gate and then hard-fail on anchored writes.
  writeFileSync(join(root, '.memstead.cache/plugin/binary-version.json'), JSON.stringify({ version: '0.2.0' }));
  const g = anchorsGate(root);
  assert.equal(g.capable, false);
  assert.match(g.reason, /predates anchors support/);
  assert.match(g.reason, /without anchors/);
  rmSync(root, { recursive: true, force: true });
});
