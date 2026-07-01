/**
 * change-detection.mjs — source-change targeting primitives for the
 * ingest loop's filesystem (`mtime`) strategy.
 *
 * The ingest loop steers a fresh, memoryless iteration at the *changed*
 * slice of a source rather than re-roaming the whole thing. For sources
 * without a git work tree, "what changed" is computed from a per-file
 * `{mtime, size}` stat map: a file is **added** (new key), **modified**
 * (mtime or size differs), or **deleted** (key gone). Deletions are the
 * cheapest, highest-signal drift — a watermark (`max(mtime)`) is blind to
 * them, so a per-file map is used instead.
 *
 * Two artefacts come out of a stat map:
 *
 *   - a small **digest** `{count, watermark, aggregate}` — the durable
 *     token the engine persists per `(ingest, facet)`. Byte-comparing two
 *     digests answers "did anything change since the last sync"; it
 *     survives a skill-cache wipe because it lives in engine vault config.
 *   - the **full map** — a rebuildable skill-cache memo keyed by digest,
 *     used to compute *which* files changed. On cache miss the caller
 *     degrades to a one-tick full scan (detection from the digest still
 *     fires; only the precise slice is lost).
 *
 * Everything here is pure (no I/O except `computeStatMap`'s `stat()`), so
 * the digest/diff logic is unit-testable without a workspace.
 *
 * The digest token is opaque to the engine — it stores and returns the
 * string verbatim. `parseDigestToken` is deliberately tolerant: an
 * unrecognized shape returns `null` ("no reliable signal"), never throws,
 * so a token produced by a different medium-type strategy (a git commit
 * id, say) degrades gracefully instead of crashing the run.
 */

import { statSync } from 'node:fs';
import { join } from 'node:path';
import { createHash } from 'node:crypto';

const DIGEST_VERSION = 1;

/**
 * Stat every relative path under `root` into `{ [relPath]: {mtime, size} }`.
 * `mtime` is integer milliseconds (rounded) so the value is stable across
 * JSON round-trips. Unreadable / vanished paths are skipped — a file that
 * disappears between enumeration and stat simply isn't in the map, which
 * is the correct "deleted" signal on the next diff.
 *
 * @param {string[]} relPaths — workspace-relative paths (sorted or not).
 * @param {string} root — absolute base the paths resolve against.
 * @returns {Object<string,{mtime:number,size:number}>}
 */
export function computeStatMap(relPaths, root) {
  const map = {};
  for (const rel of relPaths) {
    try {
      const st = statSync(join(root, rel));
      if (!st.isFile()) continue;
      map[rel] = { mtime: Math.round(st.mtimeMs), size: st.size };
    } catch {
      // vanished or unreadable — omit; surfaces as a deletion next diff.
    }
  }
  return map;
}

/**
 * Reduce a stat map to its durable digest. `count` is the entry count,
 * `watermark` the max mtime, `aggregate` a short content hash over the
 * sorted `(path, mtime, size)` tuples. Two maps produce the same digest
 * iff they have the same files with the same `(mtime, size)` — so a
 * digest change is a reliable "something moved" trigger, and `count` /
 * `watermark` shifts make additions and deletions visible even without
 * the full map.
 *
 * @param {Object<string,{mtime:number,size:number}>} statMap
 * @returns {{count:number, watermark:number, aggregate:string}}
 */
export function digestStatMap(statMap) {
  const keys = Object.keys(statMap).sort();
  const h = createHash('sha1');
  let watermark = 0;
  for (const k of keys) {
    const { mtime, size } = statMap[k];
    if (mtime > watermark) watermark = mtime;
    h.update(`${k}\0${mtime}\0${size}\n`);
  }
  return {
    count: keys.length,
    watermark,
    aggregate: h.digest('hex').slice(0, 16),
  };
}

/**
 * Serialize a digest into the opaque token string the engine persists.
 * Carries a version tag so a future digest shape can be told apart from
 * this one (and from a git commit id) by `parseDigestToken`.
 */
export function serializeDigestToken(digest) {
  return JSON.stringify({
    v: DIGEST_VERSION,
    count: digest.count,
    watermark: digest.watermark,
    aggregate: digest.aggregate,
  });
}

/**
 * Parse a token back into a digest, or `null` if it isn't a recognized
 * mtime-digest token. Tolerant by contract: a malformed string, a git
 * commit id, a graph snapshot token, or a future-version digest all
 * return `null` so the caller treats the source as having no usable
 * baseline (degrade, don't crash).
 */
export function parseDigestToken(token) {
  if (typeof token !== 'string' || !token) return null;
  let obj;
  try { obj = JSON.parse(token); } catch { return null; }
  if (!obj || typeof obj !== 'object') return null;
  if (obj.v !== DIGEST_VERSION) return null;
  if (typeof obj.count !== 'number'
    || typeof obj.watermark !== 'number'
    || typeof obj.aggregate !== 'string') return null;
  return { count: obj.count, watermark: obj.watermark, aggregate: obj.aggregate };
}

/** Two digests are equal iff every field matches. */
export function digestsEqual(a, b) {
  if (!a || !b) return false;
  return a.count === b.count
    && a.watermark === b.watermark
    && a.aggregate === b.aggregate;
}

/**
 * Diff two stat maps into `{ added, modified, deleted }` (each a sorted
 * path array). A key only in `now` is added; only in `prev` is deleted;
 * in both with a differing `mtime` *or* `size` is modified. `size` guards
 * against mtime-preserving content writes (`cp -p`, tar extraction).
 *
 * @param {Object<string,{mtime:number,size:number}>} prev
 * @param {Object<string,{mtime:number,size:number}>} now
 */
export function diffStatMaps(prev, now) {
  const added = [];
  const modified = [];
  const deleted = [];
  for (const k of Object.keys(now)) {
    if (!(k in prev)) { added.push(k); continue; }
    const a = prev[k], b = now[k];
    if (a.mtime !== b.mtime || a.size !== b.size) modified.push(k);
  }
  for (const k of Object.keys(prev)) {
    if (!(k in now)) deleted.push(k);
  }
  added.sort();
  modified.sort();
  deleted.sort();
  return { added, modified, deleted };
}
