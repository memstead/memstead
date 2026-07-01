// Unit tests against the pure helpers in mem-drift-notify-utils.mjs.
// No git invocations, no tempdirs — those live in the integration test.

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
  parseRefList,
  isTrackedMem,
  diffPathsToEntityIds,
  parseState,
  computeDrift,
  formatReminder,
  sanitizeSessionId,
  nextStateMap,
} from './mem-drift-notify-utils.mjs';

describe('parseRefList', () => {
  it('parses well-formed for-each-ref output', () => {
    const stdout = [
      'refs/heads/main aaaa1111',
      'refs/heads/memstead/engine bbbb2222',
      'refs/heads/__SCHEMAS cccc3333',
      '',
    ].join('\n');
    assert.deepStrictEqual(parseRefList(stdout), [
      { name: 'main', sha: 'aaaa1111' },
      { name: 'memstead/engine', sha: 'bbbb2222' },
      { name: '__SCHEMAS', sha: 'cccc3333' },
    ]);
  });

  it('skips lines without two whitespace-separated fields', () => {
    const stdout = 'refs/heads/main\nrefs/heads/x yyyy\n   \nbogus\n';
    assert.deepStrictEqual(parseRefList(stdout), [{ name: 'x', sha: 'yyyy' }]);
  });

  it('returns [] for empty/null input', () => {
    assert.deepStrictEqual(parseRefList(''), []);
    assert.deepStrictEqual(parseRefList(null), []);
    assert.deepStrictEqual(parseRefList(undefined), []);
  });

  it('ignores refs outside refs/heads/', () => {
    const stdout = 'refs/tags/v1 deadbeef\nrefs/heads/main aaaa\n';
    assert.deepStrictEqual(parseRefList(stdout), [{ name: 'main', sha: 'aaaa' }]);
  });
});

describe('isTrackedMem', () => {
  it('drops main and registry-class refs', () => {
    assert.strictEqual(isTrackedMem('main'), false);
    assert.strictEqual(isTrackedMem('__SYSTEM'), false);
    assert.strictEqual(isTrackedMem('__SCHEMAS'), false);
  });

  it('keeps writable mem names including hierarchical', () => {
    assert.strictEqual(isTrackedMem('memstead/engine'), true);
    assert.strictEqual(isTrackedMem('ingest/engine-graph'), true);
    assert.strictEqual(isTrackedMem('exec-foo'), true);
  });

  it('rejects empty/falsy input', () => {
    assert.strictEqual(isTrackedMem(''), false);
    assert.strictEqual(isTrackedMem(null), false);
    assert.strictEqual(isTrackedMem(undefined), false);
  });
});

describe('diffPathsToEntityIds', () => {
  it('flattens flat-layout paths into <mem>--<slug>', () => {
    assert.deepStrictEqual(
      diffPathsToEntityIds('memstead/engine', ['engine.md', 'cap-foo.md']),
      ['memstead/engine--cap-foo', 'memstead/engine--engine'],
    );
  });

  it('preserves hierarchical paths inside the mem after the --', () => {
    assert.deepStrictEqual(
      diffPathsToEntityIds('specs', ['architecture/result.md', 'a.md']),
      ['specs--a', 'specs--architecture/result'],
    );
  });

  it('drops non-md paths', () => {
    assert.deepStrictEqual(
      diffPathsToEntityIds('specs', ['README.txt', 'foo.md', '.gitignore']),
      ['specs--foo'],
    );
  });

  it('deduplicates repeated entries', () => {
    assert.deepStrictEqual(
      diffPathsToEntityIds('v', ['a.md', 'a.md', 'b.md']),
      ['v--a', 'v--b'],
    );
  });

  it('returns [] for empty input', () => {
    assert.deepStrictEqual(diffPathsToEntityIds('v', []), []);
    assert.deepStrictEqual(diffPathsToEntityIds('v', null), []);
  });
});

describe('parseState', () => {
  it('parses well-formed state JSON', () => {
    assert.deepStrictEqual(
      parseState('{"a": "111", "memstead/engine": "222"}'),
      { a: '111', 'memstead/engine': '222' },
    );
  });

  it('returns null for corrupt JSON', () => {
    assert.strictEqual(parseState('{not valid'), null);
    assert.strictEqual(parseState(''), null);
  });

  it('returns null for non-object payloads', () => {
    assert.strictEqual(parseState('null'), null);
    assert.strictEqual(parseState('"x"'), null);
    assert.strictEqual(parseState('[1, 2]'), null);
  });

  it('drops non-string values defensively', () => {
    assert.deepStrictEqual(
      parseState('{"a": "ok", "b": 42, "c": null, "d": "fine"}'),
      { a: 'ok', d: 'fine' },
    );
  });

  it('returns null for null/undefined input', () => {
    assert.strictEqual(parseState(null), null);
    assert.strictEqual(parseState(undefined), null);
  });
});

describe('computeDrift', () => {
  it('returns entries only for mems with changed SHAs', () => {
    const drift = computeDrift({ a: 'old', b: 'same' }, { a: 'new', b: 'same', c: 'fresh' });
    assert.deepStrictEqual(drift, [{ mem: 'a', oldSha: 'old', newSha: 'new' }]);
  });

  it('returns [] when prior is null (first-run)', () => {
    assert.deepStrictEqual(computeDrift(null, { a: 'x' }), []);
  });

  it('ignores deleted mems (in prior, not in current)', () => {
    assert.deepStrictEqual(computeDrift({ removed: 'x' }, { kept: 'y' }), []);
  });

  it('ignores mems newly observed (in current, not in prior)', () => {
    assert.deepStrictEqual(computeDrift({}, { fresh: 'y' }), []);
  });

  it('sorts entries by mem name', () => {
    const drift = computeDrift(
      { 'memstead/engine': '1', 'ingest/x': '2' },
      { 'memstead/engine': '1a', 'ingest/x': '2a' },
    );
    assert.deepStrictEqual(drift.map((d) => d.mem), ['ingest/x', 'memstead/engine']);
  });
});

describe('formatReminder', () => {
  it('returns empty string for empty drift list', () => {
    assert.strictEqual(formatReminder([]), '');
    assert.strictEqual(formatReminder(null), '');
  });

  it('emits a single system-reminder block listing each mem and its entity ids', () => {
    const out = formatReminder([
      {
        mem: 'memstead/engine',
        oldSha: 'aaaaaaaaaaaaaaaa',
        newSha: 'bbbbbbbbbbbbbbbb',
        entityIds: ['memstead/engine--engine', 'memstead/engine--foo'],
      },
    ]);
    assert.match(out, /^<system-reminder>/);
    assert.match(out, /<\/system-reminder>$/);
    assert.match(out, /Mem `memstead\/engine` \(aaaaaaaaaaaa → bbbbbbbbbbbb\):/);
    assert.match(out, /- memstead\/engine--engine/);
    assert.match(out, /- memstead\/engine--foo/);
    assert.match(out, /memstead_entity/);
  });

  it('handles drift with no entity-level diff (degraded git)', () => {
    const out = formatReminder([
      { mem: 'v', oldSha: 'a', newSha: 'b', entityIds: [] },
    ]);
    assert.match(out, /\(no entity-level diff available\)/);
  });

  it('lists multiple mems in one block', () => {
    const out = formatReminder([
      { mem: 'a', oldSha: '1', newSha: '2', entityIds: ['a--x'] },
      { mem: 'b', oldSha: '3', newSha: '4', entityIds: ['b--y'] },
    ]);
    const matches = out.match(/<system-reminder>/g) || [];
    assert.strictEqual(matches.length, 1);
    assert.match(out, /Mem `a`/);
    assert.match(out, /Mem `b`/);
    assert.match(out, /a--x/);
    assert.match(out, /b--y/);
  });
});

describe('sanitizeSessionId', () => {
  it('keeps UUID-shaped ids intact', () => {
    const id = '01h8z9q3v8x2-abc_def.999';
    assert.strictEqual(sanitizeSessionId(id), id);
  });

  it('strips path-separator characters so the id stays a single filename component', () => {
    // `..` is preserved (allowed in the regex) but the slashes are
    // stripped, which collapses any traversal into a single
    // filename component — `..etcpasswd` cannot escape the cache dir.
    assert.strictEqual(sanitizeSessionId('../etc/passwd'), '..etcpasswd');
    assert.strictEqual(sanitizeSessionId('a/b\\c'), 'abc');
  });

  it('clamps long input', () => {
    const long = 'a'.repeat(500);
    assert.strictEqual(sanitizeSessionId(long).length, 128);
  });

  it('returns empty string for falsy/non-string input', () => {
    assert.strictEqual(sanitizeSessionId(''), '');
    assert.strictEqual(sanitizeSessionId(null), '');
    assert.strictEqual(sanitizeSessionId(undefined), '');
    assert.strictEqual(sanitizeSessionId(42), '');
  });
});

describe('nextStateMap', () => {
  it('collapses tracked refs to a {name: sha} map', () => {
    assert.deepStrictEqual(
      nextStateMap([
        { name: 'a', sha: '1' },
        { name: 'memstead/engine', sha: '2' },
      ]),
      { a: '1', 'memstead/engine': '2' },
    );
  });

  it('drops entries with missing fields', () => {
    assert.deepStrictEqual(
      nextStateMap([
        { name: 'a', sha: '1' },
        { name: '', sha: '2' },
        { name: 'b', sha: '' },
      ]),
      { a: '1' },
    );
  });
});
