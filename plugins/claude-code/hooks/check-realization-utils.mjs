// Pure utility functions for check-realization hook — no side effects, testable.

/**
 * Extract realization file paths from an entity's specifies field.
 *
 * Recognizes two patterns:
 * 1. File headers:  ### File: `path`  or  ### Files: `path1`, `path2`
 * 2. Inline backtick paths with file extensions (only outside code blocks)
 *
 * Returns a deduplicated array of relative paths (e.g. ['packages/core/lib/store.js']).
 */
export function extractRealizationPaths(specifies, schema) {
  if (!specifies) return [];
  const FILE_HEADER_RE = schema.drift.realizationPatterns.fileHeader;
  const BACKTICK_PATH_RE = schema.drift.realizationPatterns.backtickPath;
  const paths = new Set();

  // 1. File headers: ### File: `path` or ### Files: `p1`, `p2`
  for (const match of specifies.matchAll(FILE_HEADER_RE)) {
    const line = match[1];
    for (const tick of line.matchAll(/`([^`]+)`/g)) {
      const p = tick[1].trim();
      if (p && !/<[^>]+>/.test(p)) paths.add(p);
    }
  }

  // 2. Inline backtick paths (strip code blocks first)
  const stripped = specifies.replace(/^```[\s\S]*?^```/gm, '');
  for (const match of stripped.matchAll(BACKTICK_PATH_RE)) {
    const p = match[1].trim();
    if (p.includes('/') && !/<[^>]+>/.test(p)) paths.add(p);
  }

  return [...paths];
}

/**
 * Convert an entity file path (relative to memRoot) to an entity ID.
 * E.g. "test-engine/markdown-parser.md" → "test-engine--markdown-parser"
 * Defensive: takes the last path segment as slug, so a stray nested
 * file from a pre-flat-layout mem still maps to a usable ID.
 */
export function fileToId(relPath) {
  const parts = relPath.replace(/\\/g, '/').replace(/\.md$/, '').split('/');
  if (parts.length < 2) return null;
  const mem = parts[0];
  const name = parts[parts.length - 1];
  return `${mem}--${name}`;
}

/**
 * Check if an edited file path matches a realization path.
 */
export function pathMatches(editedRelPath, realizationPath) {
  return (
    editedRelPath === realizationPath ||
    editedRelPath.endsWith('/' + realizationPath) ||
    realizationPath.endsWith('/' + editedRelPath)
  );
}
