// Tests for the recorded-binary-version capability gate (binary-version.mjs).

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import {
  parseVersion,
  parseBuildMetadata,
  readRecord,
  isAtLeast,
  ANCHORS_MIN,
  REPO_MIN,
  CONSUME_MIN,
  CAPABILITIES,
  recordBinaryVersion,
  readRecordedVersion,
  anchorsGate,
  capabilityGate,
  resolveWorkspaceRootFrom,
} from './binary-version.mjs';

function ws() {
  return mkdtempSync(join(tmpdir(), 'binver-'));
}

// The gate re-measures the live binary before deciding. Tests that are about
// the RECORD pass this: a probe that cannot run leaves the record standing,
// which is the documented best-effort behaviour and keeps these cases about
// the one thing they assert. Without it they would spawn whatever `memstead`
// happens to be on the developer's PATH and assert against that.
const noProbe = { run: () => ({ status: 127, error: new Error('no binary on PATH') }) };

/** A probe answering with `banner`, as a successful `--version` call. */
const probe = (banner) => ({ run: () => ({ status: 0, stdout: `${banner}\n` }) });

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
  const g = anchorsGate(root, noProbe);
  assert.equal(g.capable, true);
  assert.match(g.reason, /supports anchors/);
  rmSync(root, { recursive: true, force: true });
});

test('gate FAILS CLOSED with no record — degraded, with a printable reason', () => {
  const root = ws();
  const g = anchorsGate(root, noProbe);
  assert.equal(g.capable, false);
  assert.equal(g.version, null);
  assert.match(g.reason, /no recorded binary version/);
  assert.match(g.reason, /without anchors/);
  rmSync(root, { recursive: true, force: true });
});

test('resolveWorkspaceRootFrom walks up to the workspace marker', () => {
  const root = ws();
  mkdirSync(join(root, '.memstead'), { recursive: true });
  writeFileSync(join(root, '.memstead', 'workspace.toml'), '');
  const sub = join(root, 'a', 'b');
  mkdirSync(sub, { recursive: true });
  assert.equal(resolveWorkspaceRootFrom(sub), root);
  rmSync(root, { recursive: true, force: true });
});

test('resolveWorkspaceRootFrom follows an .mcp.json cd-target into a subdirectory workspace', () => {
  // The loop-session case: pwd is the project root, the workspace lives in
  // a subdirectory — a plain walk-up never descends, so the gate must probe
  // the `.mcp.json` `cd <dir>` launch target (same resolution the hooks use).
  const project = ws();
  const graph = join(project, 'graph');
  mkdirSync(join(graph, '.memstead'), { recursive: true });
  writeFileSync(join(graph, '.memstead', 'workspace.toml'), '');
  writeFileSync(
    join(project, '.mcp.json'),
    JSON.stringify({ mcpServers: { memstead: { command: 'sh', args: ['-c', 'cd graph && exec memstead-mcp'] } } }),
  );
  assert.equal(resolveWorkspaceRootFrom(project), graph);
  // gate/record therefore land in the subdirectory workspace:
  recordBinaryVersion(graph, { run: () => ({ status: 0, stdout: 'memstead 0.3.0' }) });
  assert.equal(anchorsGate(resolveWorkspaceRootFrom(project), noProbe).capable, true);
  rmSync(project, { recursive: true, force: true });
});

test('resolveWorkspaceRootFrom falls back to the given directory when nothing resolves', () => {
  const dir = ws();
  assert.equal(resolveWorkspaceRootFrom(dir), dir);
  rmSync(dir, { recursive: true, force: true });
});

test('gate FAILS CLOSED for a below-threshold recorded version', () => {
  const root = ws();
  mkdirSync(join(root, '.memstead.cache/plugin'), { recursive: true });
  // 0.2.0 predates anchors (they land in 0.3.0) — the recorded binary must
  // fail closed rather than pass the gate and then hard-fail on anchored writes.
  writeFileSync(join(root, '.memstead.cache/plugin/binary-version.json'), JSON.stringify({ version: '0.2.0' }));
  const g = anchorsGate(root, noProbe);
  assert.equal(g.capable, false);
  assert.match(g.reason, /predates anchors support/);
  assert.match(g.reason, /without anchors/);
  rmSync(root, { recursive: true, force: true });
});

// ── the two flag gates on skill paths: --repo (setup) and --consume (sync, ingest router)

test('repo and consume gates: a below-minimum record degrades with a sentence naming both versions', () => {
  const root = ws();
  recordBinaryVersion(root, { run: () => ({ status: 0, stdout: 'memstead 0.9.0' }) });
  for (const [name, flag] of [['repo', '--repo'], ['consume', '--consume']]) {
    const g = capabilityGate(root, name, noProbe);
    assert.equal(g.capable, false, name);
    assert.deepEqual(g.version, { major: 0, minor: 9, patch: 0 });
    assert.ok(g.reason.includes(`recorded binary 0.9.0 predates \`${flag}\` support (needs 0.10.0)`), g.reason);
  }
  assert.match(capabilityGate(root, 'repo', noProbe).reason, /proceeding with the plain `quickstart` form/);
  assert.match(capabilityGate(root, 'consume', noProbe).reason, /rendering the brief as a pure read/);
  rmSync(root, { recursive: true, force: true });
});

test('repo and consume gates: an at-or-above record passes silently (capable, no degraded sentence)', () => {
  const root = ws();
  // A RELEASE banner: bare semver, no build metadata. The sha-bearing form of
  // the same version is the "cannot confirm" case, asserted separately below.
  recordBinaryVersion(root, { run: () => ({ status: 0, stdout: 'memstead 0.10.0' }) });
  for (const name of ['repo', 'consume']) {
    const g = capabilityGate(root, name, noProbe);
    assert.equal(g.capable, true, name);
    assert.doesNotMatch(g.reason, /predates|proceeding|pure read/);
  }
  recordBinaryVersion(root, { run: () => ({ status: 0, stdout: 'memstead 0.11.2' }) });
  assert.equal(capabilityGate(root, 'repo', noProbe).capable, true);
  rmSync(root, { recursive: true, force: true });
});

test('repo and consume gates: a missing or unparseable record degrades like below-minimum', () => {
  const root = ws();
  for (const name of ['repo', 'consume']) {
    const g = capabilityGate(root, name, noProbe);
    assert.equal(g.capable, false, name);
    assert.equal(g.version, null);
    assert.match(g.reason, /no recorded binary version/);
  }
  mkdirSync(join(root, '.memstead.cache', 'plugin'), { recursive: true });
  writeFileSync(join(root, '.memstead.cache', 'plugin', 'binary-version.json'), '{not json');
  assert.equal(capabilityGate(root, 'consume', noProbe).capable, false);
  assert.throws(() => capabilityGate(root, 'fail-on-findings', noProbe), /unknown capability/);
  assert.deepEqual(Object.keys(CAPABILITIES), ['anchors', 'repo', 'consume']);
  assert.deepEqual(REPO_MIN, { major: 0, minor: 10, patch: 0 });
  assert.deepEqual(CONSUME_MIN, { major: 0, minor: 10, patch: 0 });
  rmSync(root, { recursive: true, force: true });
});

// ── the third state: a build that is not a release cannot be placed on the ladder

test('parseBuildMetadata picks the +g<sha> suffix, and only from the version', () => {
  assert.equal(parseBuildMetadata('memstead 0.17.0+gbea3438'), 'gbea3438');
  assert.equal(parseBuildMetadata('memstead 0.17.0+gbea3438-dirty'), 'gbea3438-dirty');
  assert.equal(parseBuildMetadata('memstead 0.17.0'), null);
  // A release binary from 0.18.0 on: bare semver, nothing to find.
  assert.equal(parseBuildMetadata('memstead 1.0.0\n'), null);
  assert.equal(parseBuildMetadata(undefined), null);
});

test('gate CANNOT CONFIRM an at-threshold dev build — the C9 path back', () => {
  const root = ws();
  // The exact shape that bit us: the crate version does not move between
  // releases, so a build from any commit after the 0.10.0 tag still reports
  // 0.10.0 — including builds predating the commit that added `--consume`.
  // Passing it as "at least 0.10.0, therefore capable" is how the plugin
  // talked itself into sending a parameter the engine rejects.
  recordBinaryVersion(root, { run: () => ({ status: 0, stdout: 'memstead 0.10.0+gdeadbee' }) });
  for (const name of ['repo', 'consume']) {
    const g = capabilityGate(root, name, noProbe);
    assert.equal(g.capable, false, name);
    assert.equal(g.build, 'gdeadbee');
    assert.match(g.reason, /cannot confirm/);
    assert.match(g.reason, /dev build at gdeadbee/);
  }
  // ...and the degraded path is still named, so the caller can print one line.
  assert.match(capabilityGate(root, 'consume', noProbe).reason, /rendering the brief as a pure read/);
  // A dirty build of the same version is no more confirmable.
  recordBinaryVersion(root, { run: () => ({ status: 0, stdout: 'memstead 0.10.0+gdeadbee-dirty' }) });
  assert.equal(capabilityGate(root, 'consume', noProbe).capable, false);
  rmSync(root, { recursive: true, force: true });
});

test('below-threshold outranks cannot-confirm: the sounder sentence wins', () => {
  const root = ws();
  // 0.2.0+g… IS confidently below the 0.3.0 anchors threshold — the crate
  // version never reached 0.3.0 — so "predates" is true and more useful than
  // "cannot confirm". Only at or above the threshold does the release
  // question decide anything.
  recordBinaryVersion(root, { run: () => ({ status: 0, stdout: 'memstead 0.2.0+gfeed123' }) });
  const g = anchorsGate(root, noProbe);
  assert.equal(g.capable, false);
  assert.match(g.reason, /predates anchors support/);
  assert.doesNotMatch(g.reason, /cannot confirm/);
  rmSync(root, { recursive: true, force: true });
});

// ── the record follows the binary: setup runs once, upgrades do not re-run it

test('gate re-records when the live binary disagrees with the record', () => {
  const root = ws();
  // The observed state: recorded 0.14.0, PATH running 0.17.0. Nothing
  // re-records on upgrade, so every gate understated the binary indefinitely.
  recordBinaryVersion(root, { run: () => ({ status: 0, stdout: 'memstead 0.14.0+gold' }) });
  const g = capabilityGate(root, 'consume', probe('memstead 0.17.0'));
  assert.equal(g.capable, true);
  assert.deepEqual(g.version, { major: 0, minor: 17, patch: 0 });
  // The refresh is durable, not a per-call override.
  assert.deepEqual(readRecord(root).version, { major: 0, minor: 17, patch: 0 });
  assert.equal(readRecord(root).build, null);
  rmSync(root, { recursive: true, force: true });
});

test('a downgrade is caught the same way — the record follows, in both directions', () => {
  const root = ws();
  recordBinaryVersion(root, { run: () => ({ status: 0, stdout: 'memstead 0.17.0' }) });
  const g = capabilityGate(root, 'consume', probe('memstead 0.9.0'));
  assert.equal(g.capable, false);
  assert.match(g.reason, /recorded binary 0\.9\.0 predates/);
  rmSync(root, { recursive: true, force: true });
});

test('a failed or unparseable probe leaves the record standing rather than degrading it', () => {
  const root = ws();
  recordBinaryVersion(root, { run: () => ({ status: 0, stdout: 'memstead 0.17.0' }) });
  // An unreachable binary says nothing about the version recorded when it was
  // reachable; dropping the record over a transient spawn failure would
  // degrade a capable setup for no evidence.
  for (const bad of [
    { run: () => ({ status: 127, error: new Error('ENOENT') }) },
    { run: () => ({ status: 1, stdout: '', stderr: 'boom' }) },
    { run: () => ({ status: 0, stdout: 'memstead (unknown)' }) },
    { run: () => { throw new Error('spawn exploded'); } },
  ]) {
    const g = capabilityGate(root, 'consume', bad);
    assert.equal(g.capable, true);
    assert.deepEqual(g.version, { major: 0, minor: 17, patch: 0 });
  }
  assert.deepEqual(readRecord(root).version, { major: 0, minor: 17, patch: 0 });
  rmSync(root, { recursive: true, force: true });
});

test('an agreeing probe leaves the record byte-identical — no rewrite churn', () => {
  const root = ws();
  recordBinaryVersion(root, { run: () => ({ status: 0, stdout: 'memstead 0.17.0+gabc123' }) });
  const path = join(root, '.memstead.cache', 'plugin', 'binary-version.json');
  const before = readFileSync(path, 'utf-8');
  capabilityGate(root, 'consume', probe('memstead 0.17.0+gabc123'));
  assert.equal(readFileSync(path, 'utf-8'), before);
  rmSync(root, { recursive: true, force: true });
});

test('no record at all: the probe establishes one, so a wiped cache self-heals', () => {
  const root = ws();
  const g = capabilityGate(root, 'consume', probe('memstead 0.17.0'));
  assert.equal(g.capable, true);
  assert.deepEqual(readRecord(root).version, { major: 0, minor: 17, patch: 0 });
  rmSync(root, { recursive: true, force: true });
});
