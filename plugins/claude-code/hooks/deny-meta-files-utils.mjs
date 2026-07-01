// Pure logic for deny-meta-files.mjs — testable without stdin or process.exit.
//
// Each ingest config declares its own `deny_paths` (workspace-root-relative).
// The hook's runner reads the active list from a cache file and passes it
// here. An empty or missing list means default-open: nothing is blocked.

import { resolve, isAbsolute } from 'node:path';

/**
 * Decide whether a single tool-input candidate (path or glob pattern) should
 * be blocked against the supplied deny list. Returns a human-readable reason
 * if so, otherwise null.
 *
 * Each `denyPaths` entry is resolved against `workspaceRoot`. A candidate
 * matches if it equals the resolved entry (file-shape match) or starts with
 * the resolved entry plus `/` (directory-shape match). Glob candidates with
 * a wildcard are matched on their literal prefix.
 *
 * @param {string|undefined|null} candidate - raw value from tool_input
 * @param {string} cwd                       - working dir for relative resolution
 * @param {string} workspaceRoot             - absolute project root
 * @param {string[]} denyPaths               - workspace-root-relative deny entries
 * @returns {string|null}
 */
export function checkCandidate(candidate, cwd, workspaceRoot, denyPaths) {
  if (!candidate || typeof candidate !== 'string') return null;
  if (!Array.isArray(denyPaths) || denyPaths.length === 0) return null;

  const normalized = isAbsolute(candidate) ? candidate : resolve(cwd, candidate);

  for (const entry of denyPaths) {
    if (typeof entry !== 'string' || !entry) continue;
    const denyAbs = resolve(workspaceRoot, entry);
    if (normalized === denyAbs) {
      return `${entry} is hidden from the ingest agent by this ingest's deny_paths.`;
    }
    if (normalized.startsWith(denyAbs + '/')) {
      return `${entry}/ is hidden from the ingest agent by this ingest's deny_paths.`;
    }
  }

  // Glob/Grep heuristic: a pattern like `dev/**/*.md` would never match an
  // exact file_path but should still be blocked. Inspect the literal prefix.
  if (candidate.includes('*')) {
    const literalPrefix = candidate.split('*')[0];
    const literalNorm = isAbsolute(literalPrefix)
      ? literalPrefix
      : resolve(cwd, literalPrefix);
    for (const entry of denyPaths) {
      if (typeof entry !== 'string' || !entry) continue;
      const denyAbs = resolve(workspaceRoot, entry);
      if (literalNorm === denyAbs || literalNorm.startsWith(denyAbs + '/')) {
        return `${entry}/ is hidden — pattern targets it.`;
      }
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
