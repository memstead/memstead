// Pure logic for deny-meta-files.mjs — testable without stdin or process.exit.
//
// Each ingest config declares its own `deny_paths`: **workspace-relative glob
// patterns**, the same grammar and resolution root as facet-scope entries
// (e.g. `dev/**`, `**/VISION.md`, `../CLAUDE.md`). The hook resolves them
// identically to the engine (`memstead-base` ingest cursor): a candidate is
// blocked iff its workspace-relative path matches a deny glob. The runner reads
// the active list from an engine-written cache file and passes it here. An
// empty or missing list means default-open: nothing is blocked.
//
// The glob semantics mirror the engine's `globset` with its default options
// (`literal_separator = false`), so `*` / `**` cross `/` exactly as they do
// engine-side — the shared fixture (`deny-dialect-fixture.json`) pins the two
// resolvers together.

import { resolve, relative, isAbsolute } from 'node:path';

// Translate a workspace-relative glob into a RegExp, matching `globset`'s
// default option set (literal_separator = false) token-for-token:
//   - whole-pattern globstar        -> .*
//   - leading globstar              -> (?:/?|.*/)     (RecursivePrefix)
//   - trailing "/globstar"          -> /.*            (RecursiveSuffix)
//   - interior "/globstar/"         -> (?:/|/.*/)     (RecursiveZeroOrMore)
//   - any other globstar            -> .*
//   - single "*"                    -> .*             (crosses "/")
//   - "?"                           -> .
//   - everything else               -> escaped literal
// Character classes / brace alternates are treated literally (outside the
// dialect the dogfood and fixtures use); the engine would expand them, so keep
// deny entries to *, globstar, ?, and literals for guaranteed parity.
//
// @param {string} glob
// @returns {RegExp}
export function globToRegExp(glob) {
  if (glob === '**') return /^.*$/;
  const specials = '\\^$.|+()[]{}';
  let re = '^';
  const n = glob.length;
  let i = 0;
  while (i < n) {
    const c = glob[i];
    if (c === '*') {
      if (glob[i + 1] === '*') {
        const atStart = i === 0;
        const prevSep = i > 0 && glob[i - 1] === '/';
        const nextChar = i + 2 < n ? glob[i + 2] : null;
        const nextSep = nextChar === '/';
        const atEnd = i + 2 >= n;
        if (atStart && (atEnd || nextSep)) {
          re += '(?:/?|.*/)'; // RecursivePrefix
          i += nextSep ? 3 : 2;
        } else if (prevSep && atEnd) {
          if (re.endsWith('/')) re = re.slice(0, -1);
          re += '/.*'; // RecursiveSuffix — absorbs the preceding '/'
          i += 2;
        } else if (prevSep && nextSep) {
          if (re.endsWith('/')) re = re.slice(0, -1);
          re += '(?:/|/.*/)'; // RecursiveZeroOrMore — absorbs both '/'
          i += 3;
        } else {
          re += '.*';
          i += 2;
        }
      } else {
        re += '.*';
        i += 1;
      }
    } else if (c === '?') {
      re += '.';
      i += 1;
    } else {
      re += specials.includes(c) ? '\\' + c : c;
      i += 1;
    }
  }
  return new RegExp(re + '$');
}

/**
 * The literal path prefix of a glob (the portion before the first glob
 * metacharacter), trimmed of a trailing `/`. Used for two things the raw glob
 * cannot express on its own:
 *   - blocking a *read of the directory itself* for a subtree entry
 *     (`dev/**` → base `dev`, so `Read('dev')` / `Grep(path='dev')` is caught);
 *   - degrading an un-migrated legacy bare name (`dev`) to a directory-prefix
 *     block, preserving the pre-glob hook behaviour instead of erroring.
 * Returns `''` when the pattern starts with a metacharacter (e.g. a leading
 * globstar), in which case only the glob match applies.
 * @param {string} entry
 * @returns {string}
 */
function literalBase(entry) {
  const m = entry.search(/[*?[{]/);
  const base = m === -1 ? entry : entry.slice(0, m);
  return base.replace(/\/+$/, '');
}

/**
 * The candidate's path relative to the workspace root, in posix form. Absolute
 * candidates resolve as-is; relative ones resolve against `cwd` first. May
 * contain `..` when the candidate sits outside the workspace (the dogfood
 * mediums point at sibling dirs) — that is expected and matched by `../…`
 * deny globs.
 * @param {string} candidate
 * @param {string} cwd
 * @param {string} workspaceRoot
 * @returns {string}
 */
function toWorkspaceRel(candidate, cwd, workspaceRoot) {
  const abs = isAbsolute(candidate) ? candidate : resolve(cwd, candidate);
  return relative(workspaceRoot, abs).split('\\').join('/');
}

/**
 * Decide whether a single tool-input candidate (path or glob pattern) should
 * be blocked against the supplied deny list. Returns a human-readable reason
 * if so, otherwise null.
 *
 * A candidate is blocked when its workspace-relative path either matches a
 * deny glob (identical dialect to the engine) or equals / sits under the
 * literal directory base of a deny entry. Glob candidates (Glob/Grep patterns
 * that recurse a denied subtree) keep their literal segments through
 * resolution, so the same match logic catches a pattern targeting it.
 *
 * @param {string|undefined|null} candidate - raw value from tool_input
 * @param {string} cwd                       - working dir for relative resolution
 * @param {string} workspaceRoot             - absolute workspace root
 * @param {string[]} denyPaths               - workspace-relative deny globs
 * @returns {string|null}
 */
export function checkCandidate(candidate, cwd, workspaceRoot, denyPaths) {
  if (!candidate || typeof candidate !== 'string') return null;
  if (!Array.isArray(denyPaths) || denyPaths.length === 0) return null;

  const rel = toWorkspaceRel(candidate, cwd, workspaceRoot);

  for (const entry of denyPaths) {
    if (typeof entry !== 'string' || !entry) continue;

    if (globToRegExp(entry).test(rel)) {
      return `${entry} is hidden from the ingest agent by this ingest's deny_paths.`;
    }
    const base = literalBase(entry);
    if (base && (rel === base || rel.startsWith(base + '/'))) {
      return `${entry} is hidden from the ingest agent by this ingest's deny_paths.`;
    }
  }
  return null;
}

/**
 * Pull every path-like field out of a tool_input object. Read uses
 * `file_path`; Glob uses `pattern` + optional `path`; Grep uses `pattern`
 * (regex) + optional `path` + optional `glob`. We extract all of them and
 * let `checkCandidate` decide — non-path strings (e.g. a regex like "TODO")
 * resolve to harmless paths and never match the deny list.
 *
 * @param {object} toolInput
 * @returns {string[]}
 */
export function extractCandidates(toolInput) {
  if (!toolInput || typeof toolInput !== 'object') return [];
  const fields = ['file_path', 'pattern', 'path', 'glob'];
  return fields
    .map((f) => toolInput[f])
    .filter((v) => typeof v === 'string' && v.length > 0);
}
