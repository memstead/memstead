import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
  checkCandidate,
  extractCandidates,
} from './deny-meta-files-utils.mjs';

const WS = '/home/dev/memstead';
const MEMSTEAD_LIST = ['VISION.md', 'CLAUDE.md', 'dev'];

describe('checkCandidate — allowed paths (with memstead list)', () => {
  it('allows code paths inside scope', () => {
    assert.equal(
      checkCandidate(`${WS}/engine/src/lib.rs`, WS, WS, MEMSTEAD_LIST),
      null,
    );
    assert.equal(
      checkCandidate('engine/Cargo.toml', WS, WS, MEMSTEAD_LIST),
      null,
    );
  });

  it('allows sub-CLAUDE.md files (per-area context)', () => {
    assert.equal(
      checkCandidate(`${WS}/macos/CLAUDE.md`, WS, WS, MEMSTEAD_LIST),
      null,
    );
    assert.equal(
      checkCandidate('macos/CLAUDE.md', WS, WS, MEMSTEAD_LIST),
      null,
    );
    assert.equal(
      checkCandidate(
        'engine/crates/memstead-registry/CLAUDE.md',
        WS,
        WS,
        MEMSTEAD_LIST,
      ),
      null,
    );
    assert.equal(
      checkCandidate(`${WS}/plugins/claude-code/CLAUDE.md`, WS, WS, MEMSTEAD_LIST),
      null,
    );
  });

  it('does not over-match similarly-named subdirs', () => {
    assert.equal(
      checkCandidate('engine/dev-tools/foo.rs', WS, WS, MEMSTEAD_LIST),
      null,
    );
    assert.equal(
      checkCandidate(`${WS}/VISION-draft.md`, WS, WS, MEMSTEAD_LIST),
      null,
    );
  });

  it('allows paths outside the workspace', () => {
    assert.equal(
      checkCandidate('/etc/hosts', WS, WS, MEMSTEAD_LIST),
      null,
    );
    assert.equal(
      checkCandidate('/tmp/something.md', WS, WS, MEMSTEAD_LIST),
      null,
    );
  });

  it('returns null for empty / undefined input', () => {
    assert.equal(checkCandidate(undefined, WS, WS, MEMSTEAD_LIST), null);
    assert.equal(checkCandidate(null, WS, WS, MEMSTEAD_LIST), null);
    assert.equal(checkCandidate('', WS, WS, MEMSTEAD_LIST), null);
  });
});

describe('checkCandidate — denied paths (with memstead list)', () => {
  it('blocks workspace-root CLAUDE.md (absolute and relative)', () => {
    assert.match(
      checkCandidate(`${WS}/CLAUDE.md`, WS, WS, MEMSTEAD_LIST),
      /CLAUDE\.md/,
    );
    assert.match(
      checkCandidate('CLAUDE.md', WS, WS, MEMSTEAD_LIST),
      /CLAUDE\.md/,
    );
  });

  it('blocks workspace-root VISION.md', () => {
    assert.match(
      checkCandidate(`${WS}/VISION.md`, WS, WS, MEMSTEAD_LIST),
      /VISION\.md/,
    );
    assert.match(
      checkCandidate('VISION.md', WS, WS, MEMSTEAD_LIST),
      /VISION\.md/,
    );
  });

  it('blocks any file under dev/', () => {
    assert.match(
      checkCandidate(`${WS}/dev/plans/foo.md`, WS, WS, MEMSTEAD_LIST),
      /dev/,
    );
    assert.match(
      checkCandidate('dev/archive/complete/bar.md', WS, WS, MEMSTEAD_LIST),
      /dev/,
    );
    assert.match(
      checkCandidate('dev/', WS, WS, MEMSTEAD_LIST),
      /dev/,
    );
    assert.match(
      checkCandidate('dev', WS, WS, MEMSTEAD_LIST),
      /dev/,
    );
  });

  it('blocks glob patterns targeting dev/', () => {
    assert.match(
      checkCandidate('dev/**/*.md', WS, WS, MEMSTEAD_LIST),
      /dev/,
    );
    assert.match(
      checkCandidate(`${WS}/dev/**`, WS, WS, MEMSTEAD_LIST),
      /dev/,
    );
    assert.match(
      checkCandidate('dev/plans/*', WS, WS, MEMSTEAD_LIST),
      /dev/,
    );
  });
});

describe('checkCandidate — default-open (empty deny list)', () => {
  it('permits previously-blocked candidates when denyPaths is empty', () => {
    assert.equal(checkCandidate(`${WS}/CLAUDE.md`, WS, WS, []), null);
    assert.equal(checkCandidate('VISION.md', WS, WS, []), null);
    assert.equal(checkCandidate('dev/plans/foo.md', WS, WS, []), null);
    assert.equal(checkCandidate('dev/**/*.md', WS, WS, []), null);
    assert.equal(checkCandidate('dev', WS, WS, []), null);
  });

  it('treats undefined or null deny list as default-open', () => {
    assert.equal(checkCandidate('CLAUDE.md', WS, WS, undefined), null);
    assert.equal(checkCandidate('CLAUDE.md', WS, WS, null), null);
  });

  it('portability: workspace with dev/ source code and no deny list lets dev/ through', () => {
    // Simulates a fresh checkout in a workspace whose `dev/` is real code.
    assert.equal(
      checkCandidate(`${WS}/dev/foo.ts`, WS, WS, []),
      null,
    );
    assert.equal(
      checkCandidate('dev/lib/index.ts', WS, WS, []),
      null,
    );
  });
});

describe('checkCandidate — alternate deny lists', () => {
  it('blocks only what is in the supplied list', () => {
    const list = ['secrets'];
    assert.match(
      checkCandidate('secrets/keys.txt', WS, WS, list),
      /secrets/,
    );
    assert.equal(
      checkCandidate('CLAUDE.md', WS, WS, list),
      null,
    );
    assert.equal(
      checkCandidate('dev/plans/foo.md', WS, WS, list),
      null,
    );
  });

  it('handles a single-file deny entry', () => {
    const list = ['NOTES.md'];
    assert.match(
      checkCandidate('NOTES.md', WS, WS, list),
      /NOTES\.md/,
    );
    assert.equal(
      checkCandidate('NOTES-draft.md', WS, WS, list),
      null,
    );
  });
});

describe('extractCandidates', () => {
  it('extracts file_path from Read input', () => {
    assert.deepEqual(extractCandidates({ file_path: '/foo/bar.md' }), [
      '/foo/bar.md',
    ]);
  });

  it('extracts pattern + path from Glob input', () => {
    assert.deepEqual(
      extractCandidates({ pattern: 'dev/**/*.md', path: '/work' }),
      ['dev/**/*.md', '/work'],
    );
  });

  it('extracts pattern + path + glob from Grep input', () => {
    assert.deepEqual(
      extractCandidates({ pattern: 'TODO', path: '/work', glob: '*.rs' }),
      ['TODO', '/work', '*.rs'],
    );
  });

  it('skips missing or non-string fields', () => {
    assert.deepEqual(extractCandidates({}), []);
    assert.deepEqual(extractCandidates({ file_path: null, pattern: 42 }), []);
  });

  it('returns empty array for invalid input', () => {
    assert.deepEqual(extractCandidates(null), []);
    assert.deepEqual(extractCandidates(undefined), []);
    assert.deepEqual(extractCandidates('string'), []);
  });
});
