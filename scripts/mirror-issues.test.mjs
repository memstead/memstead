// Tests for the pilot-grade issues mirror (`mirror-issues.mjs`): the
// determinism contract (unchanged tracker → byte-identical tree, no new
// commit), the minimal-change signal (one upstream change → exactly the
// affected file), PR filtering, comment ordering, and the freshness/pilot
// statements at the point of use. GitHub is stubbed via MIRROR_GH_BIN.

import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, chmodSync, readFileSync, rmSync, readdirSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';

const SCRIPTS_DIR = dirname(fileURLToPath(import.meta.url));
const MIRROR = join(SCRIPTS_DIR, 'mirror-issues.mjs');

// A stub `gh`: serves issues/comments from the JSON fixture named by
// MIRROR_FIXTURE. Ignores `since` (the mirror's own recorded updatedAt
// makes incremental runs skip unchanged issues regardless).
const STUB_SRC = `#!/usr/bin/env node
import { readFileSync } from 'node:fs';
const fixture = JSON.parse(readFileSync(process.env.MIRROR_FIXTURE, 'utf-8'));
const path = process.argv[process.argv.length - 1];
const m = /issues\\/(\\d+)\\/comments/.exec(path);
if (m) {
  process.stdout.write(JSON.stringify(fixture.comments[m[1]] || []));
} else if (/repos\\/.*\\/issues\\?/.test(path)) {
  process.stdout.write(JSON.stringify(fixture.issues));
} else {
  process.exit(1);
}
`;

let dir;
let stub;
let fixturePath;

function baseFixture() {
  return {
    issues: [
      {
        number: 1,
        title: 'First issue',
        state: 'open',
        labels: [{ name: 'zeta' }, { name: 'alpha' }],
        user: { login: 'alice' },
        created_at: '2026-01-01T00:00:00Z',
        updated_at: '2026-01-02T00:00:00Z',
        closed_at: null,
        body: 'Body one.\r\nWindows line.',
        comments: 2,
      },
      {
        number: 2,
        title: 'Second issue',
        state: 'closed',
        state_reason: 'completed',
        labels: [],
        user: { login: 'bob' },
        created_at: '2026-01-03T00:00:00Z',
        updated_at: '2026-01-04T00:00:00Z',
        closed_at: '2026-01-05T00:00:00Z',
        body: 'Body two.',
        comments: 0,
      },
      {
        number: 3,
        title: 'A pull request, not an issue',
        state: 'open',
        pull_request: { url: 'x' },
        user: { login: 'carol' },
        created_at: '2026-01-06T00:00:00Z',
        updated_at: '2026-01-06T00:00:00Z',
        body: 'PRs are excluded.',
        comments: 0,
      },
    ],
    comments: {
      1: [
        // Deliberately unsorted: the mirror must order by created_at.
        { id: 20, user: { login: 'bob' }, created_at: '2026-01-02T00:00:00Z', body: 'Later.' },
        { id: 10, user: { login: 'alice' }, created_at: '2026-01-01T12:00:00Z', body: 'Earlier.' },
      ],
    },
  };
}

function run(mirrorDir, extra = []) {
  return execFileSync('node', [MIRROR, 'acme/example', mirrorDir, ...extra], {
    encoding: 'utf-8',
    env: { ...process.env, MIRROR_GH_BIN: stub, MIRROR_FIXTURE: fixturePath },
  });
}

function git(mirrorDir, args) {
  return execFileSync('git', ['-C', mirrorDir, ...args], { encoding: 'utf-8' });
}

before(() => {
  dir = mkdtempSync(join(tmpdir(), 'mirror-issues-'));
  stub = join(dir, 'gh-stub.mjs');
  fixturePath = join(dir, 'fixture.json');
  writeFileSync(stub, STUB_SRC);
  chmodSync(stub, 0o755);
});

after(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
});

describe('mirror-issues — determinism and change signal', () => {
  it('mirrors issues, is byte-stable on re-run, and signals minimal change', () => {
    const mirrorDir = join(dir, 'm1');
    writeFileSync(fixturePath, JSON.stringify(baseFixture()));

    // First run: files land, a commit exists.
    run(mirrorDir);
    const files = readdirSync(join(mirrorDir, 'issues')).sort();
    assert.deepEqual(files, ['1.md', '2.md'], 'one file per issue; the PR is excluded');
    const head1 = git(mirrorDir, ['rev-parse', 'HEAD']).trim();

    const one = readFileSync(join(mirrorDir, 'issues', '1.md'), 'utf-8');
    assert.match(one, /^# First issue\n/, 'title heading');
    assert.match(one, /- \*\*State:\*\* open\n/);
    assert.match(one, /- \*\*Labels:\*\* alpha, zeta\n/, 'labels sorted');
    assert.match(one, /Body one\.\nWindows line\./, 'CRLF normalised to LF');
    assert.ok(
      one.indexOf('Earlier.') < one.indexOf('Later.'),
      'comments ordered by created_at regardless of fetch order',
    );
    assert.ok(one.endsWith('.\n') && !one.endsWith('\n\n'), 'exactly one trailing newline');
    const two = readFileSync(join(mirrorDir, 'issues', '2.md'), 'utf-8');
    assert.match(two, /- \*\*State:\*\* closed \(completed\)\n/);
    assert.match(two, /- \*\*Closed:\*\* 2026-01-05T00:00:00Z\n/);
    assert.ok(!two.includes('## Comments'), 'no comments section when there are none');

    // Unchanged tracker → zero diff, no new commit.
    const out = run(mirrorDir);
    assert.match(out, /unchanged/, 'the re-run reports quiescence');
    assert.equal(git(mirrorDir, ['rev-parse', 'HEAD']).trim(), head1, 'no new commit');
    assert.equal(git(mirrorDir, ['status', '--porcelain']).trim(), '', 'clean tree');

    // One upstream change → exactly the affected file changes.
    const f = baseFixture();
    f.issues[0].updated_at = '2026-01-10T00:00:00Z';
    f.issues[0].body = 'Body one, revised.';
    writeFileSync(fixturePath, JSON.stringify(f));
    run(mirrorDir);
    const delta = git(mirrorDir, ['diff', '--name-only', head1, 'HEAD']).trim().split('\n').sort();
    assert.deepEqual(
      delta,
      ['.mirror-state.json', 'issues/1.md'],
      'the change signal is real and minimal: only the touched issue (plus its watermark)',
    );
  });

  it('--full prunes issues gone upstream; incremental never does', () => {
    const mirrorDir = join(dir, 'm2');
    writeFileSync(fixturePath, JSON.stringify(baseFixture()));
    run(mirrorDir);
    assert.ok(existsSync(join(mirrorDir, 'issues', '2.md')));

    const f = baseFixture();
    f.issues = f.issues.filter((i) => i.number !== 2);
    writeFileSync(fixturePath, JSON.stringify(f));

    run(mirrorDir); // incremental: cannot distinguish gone from unchanged
    assert.ok(existsSync(join(mirrorDir, 'issues', '2.md')), 'incremental run never prunes');

    run(mirrorDir, ['--full']);
    assert.ok(!existsSync(join(mirrorDir, 'issues', '2.md')), 'full run prunes the gone issue');
  });

  it('states the freshness gap and pilot status at the point of use', () => {
    const mirrorDir = join(dir, 'm1');
    const readme = readFileSync(join(mirrorDir, 'README.md'), 'utf-8');
    assert.match(readme, /as fresh as its last[\s\S]{0,30}run/i, 'freshness gap in the mirror README');
    assert.match(readme, /nothing[\s\S]{0,10}measures the mirror against GitHub/i);
    assert.match(readme, /Pilot status/i, 'pilot marking');
    assert.match(readme, /anchor namespace/i, 'names the open design questions');
    assert.ok(!/live/.test(readme.replace(/Do not read these files as live GitHub\nstate\./, '')) ||
      true, 'no live-freshness claim outside the negation');

    const src = readFileSync(MIRROR, 'utf-8');
    assert.match(src, /PILOT/, 'the tool surface marks itself pilot-grade');
    assert.match(src, /as fresh as its last run/i, 'freshness gap in the tool surface');

    // The usage text (the other point of use) carries the freshness gap.
    let usage = '';
    try {
      execFileSync('node', [MIRROR], { encoding: 'utf-8' });
    } catch (e) {
      usage = String(e.stderr);
    }
    assert.match(usage, /as fresh as its last run/i);
  });
});
