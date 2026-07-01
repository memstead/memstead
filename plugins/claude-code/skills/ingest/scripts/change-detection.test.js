/**
 * change-detection.test.js — unit tests for the mtime/stat-map source
 * change-detection primitives. Pure logic, no subprocess: exercises the
 * digest, token round-trip, tolerance, and the added/modified/deleted
 * diff that drives the ingest changed slice.
 */

import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, utimesSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import {
  computeStatMap,
  digestStatMap,
  serializeDigestToken,
  parseDigestToken,
  digestsEqual,
  diffStatMaps,
} from './change-detection.mjs';

describe('digest token round-trip and tolerance', () => {
  it('round-trips a digest through serialize/parse', () => {
    const d = { count: 3, watermark: 1700000000000, aggregate: 'abc123def456abcd' };
    const back = parseDigestToken(serializeDigestToken(d));
    assert.deepEqual(back, d);
  });

  it('parses an unrecognized token shape as null (no reliable signal)', () => {
    // A git commit id, a graph snapshot token, junk, empty, non-string —
    // all degrade to null rather than throwing.
    assert.equal(parseDigestToken('a1b2c3d4e5f6'), null); // 40-char-ish git oid
    assert.equal(parseDigestToken('{"v":2,"count":1}'), null); // future version
    assert.equal(parseDigestToken('not json'), null);
    assert.equal(parseDigestToken(''), null);
    assert.equal(parseDigestToken(null), null);
    assert.equal(parseDigestToken('{"v":1,"count":"x"}'), null); // wrong types
  });

  it('digestsEqual is true only when every field matches', () => {
    const a = { count: 1, watermark: 10, aggregate: 'x' };
    assert.equal(digestsEqual(a, { ...a }), true);
    assert.equal(digestsEqual(a, { ...a, count: 2 }), false);
    assert.equal(digestsEqual(a, { ...a, watermark: 11 }), false);
    assert.equal(digestsEqual(a, { ...a, aggregate: 'y' }), false);
    assert.equal(digestsEqual(a, null), false);
  });
});

describe('digest is stable and change-sensitive', () => {
  it('identical maps produce identical digests; a size change moves it', () => {
    const m1 = { 'a.rs': { mtime: 100, size: 10 }, 'b.rs': { mtime: 200, size: 20 } };
    const m2 = { 'b.rs': { mtime: 200, size: 20 }, 'a.rs': { mtime: 100, size: 10 } }; // reordered
    assert.deepEqual(digestStatMap(m1), digestStatMap(m2), 'key order must not matter');

    const m3 = { ...m1, 'a.rs': { mtime: 100, size: 11 } };
    assert.notDeepEqual(digestStatMap(m1), digestStatMap(m3), 'a size change moves the digest');

    const m4 = { ...m1, 'a.rs': { mtime: 101, size: 10 } };
    assert.notDeepEqual(digestStatMap(m1), digestStatMap(m4), 'an mtime change moves the digest');
  });

  it('watermark is the max mtime; count is the entry count', () => {
    const d = digestStatMap({ 'a': { mtime: 5, size: 1 }, 'b': { mtime: 99, size: 1 } });
    assert.equal(d.count, 2);
    assert.equal(d.watermark, 99);
  });
});

describe('diffStatMaps classifies added / modified / deleted', () => {
  it('detects each class and tolerates an mtime-only touch as modified', () => {
    const prev = {
      'keep.rs': { mtime: 100, size: 10 },
      'touch.rs': { mtime: 100, size: 10 },
      'grow.rs': { mtime: 100, size: 10 },
      'gone.rs': { mtime: 100, size: 10 },
    };
    const now = {
      'keep.rs': { mtime: 100, size: 10 }, // unchanged
      'touch.rs': { mtime: 200, size: 10 }, // mtime touched
      'grow.rs': { mtime: 100, size: 99 }, // size grew (mtime preserved)
      'new.rs': { mtime: 300, size: 5 }, // added
      // gone.rs deleted
    };
    const { added, modified, deleted } = diffStatMaps(prev, now);
    assert.deepEqual(added, ['new.rs']);
    assert.deepEqual(modified, ['grow.rs', 'touch.rs']);
    assert.deepEqual(deleted, ['gone.rs']);
    assert.ok(!modified.includes('keep.rs'), 'identical (mtime,size) is absent from the slice');
  });

  it('empty-vs-empty yields no changes', () => {
    assert.deepEqual(diffStatMaps({}, {}), { added: [], modified: [], deleted: [] });
  });
});

describe('computeStatMap over a real directory', () => {
  let root;
  beforeEach(() => { root = mkdtempSync(join(tmpdir(), 'cd-statmap-')); });
  afterEach(() => { rmSync(root, { recursive: true, force: true }); });

  it('stats listed files, skips missing ones, and reflects size/mtime', () => {
    mkdirSync(join(root, 'sub'), { recursive: true });
    writeFileSync(join(root, 'a.txt'), 'hello');
    writeFileSync(join(root, 'sub/b.txt'), 'worldworld');
    // Date ctor takes epoch milliseconds; mtimeMs reads back the same.
    utimesSync(join(root, 'a.txt'), new Date(1_000), new Date(1_700_000_000));

    const map = computeStatMap(['a.txt', 'sub/b.txt', 'missing.txt'], root);
    assert.equal('missing.txt' in map, false, 'a path that does not exist is omitted');
    assert.equal(map['a.txt'].size, 5);
    assert.equal(map['sub/b.txt'].size, 10);
    assert.equal(map['a.txt'].mtime, 1_700_000_000, 'mtime is integer ms');
  });
});
