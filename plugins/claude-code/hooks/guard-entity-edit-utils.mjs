// Pure logic for guard-entity-edit.mjs — testable without process.exit or stdin.

import { basename, relative, resolve } from 'node:path';

/**
 * Regex matching entity filenames: lowercase kebab-case with optional digits.
 * Must start with a-z or 0-9, end with a-z or 0-9, only a-z, 0-9, - in between.
 * Matches the output of titleToId() in packages/core/lib/graph/id.js.
 *
 * Examples that match:    spec-entity.md, markdown-parser.md, 3d-model.md, a.md
 * Examples that DON'T:    README.md, STUPID_FILE.md, .hidden.md, my_module.md
 */
export const ENTITY_FILENAME_RE = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.md$/;

/**
 * Check if a filename follows the entity naming convention.
 * @param {string} filename - Just the filename (not a full path)
 * @returns {boolean}
 */
export function isEntityFilename(filename) {
  return ENTITY_FILENAME_RE.test(filename);
}

/**
 * Determine if a file edit should be blocked.
 * @param {string} filePath - The file path from tool_input
 * @param {string} memDir - Resolved absolute path to mem directory
 * @param {boolean} memDirExists - Whether memDir exists on disk
 * @returns {{ action: 'block'|'allow', reason?: string }}
 */
export function checkEditTarget(filePath, memDir, memDirExists) {
  if (!filePath) return { action: 'allow' };

  // Fail-closed: if mem dir doesn't exist, block potential entity files
  if (!memDirExists) {
    const absPath = resolve(filePath);
    if (absPath.includes('specs') && isEntityFilename(basename(absPath))) {
      return {
        action: 'block',
        reason: `Cannot verify mem dir at ${memDir} — refusing edit on potential entity file as precaution. File: ${filePath}`,
      };
    }
    return { action: 'allow' };
  }

  const dirName = memDir.split('/').pop() || 'specs';
  const projectRoot = resolve(memDir, '..');
  const relPath = relative(projectRoot, resolve(filePath));

  const prefix = dirName + '/';
  const prefixWin = dirName + '\\';
  if (relPath.startsWith(prefix) || relPath.startsWith(prefixWin)) {
    // Only block files matching entity naming convention
    if (!isEntityFilename(basename(relPath))) return { action: 'allow' };
    return { action: 'block', reason: `File: ${relPath}` };
  }

  return { action: 'allow' };
}

/**
 * Find --mem from .mcp.json config object (first mem of first server).
 * @param {object} mcpConfig - Parsed .mcp.json content
 * @returns {string} The mem dir path or empty string
 */
export function findMemDir(mcpConfig) {
  const roots = findAllMemDirs(mcpConfig);
  return roots[0] || '';
}

/**
 * Collect all --mem paths from all MCP servers in .mcp.json.
 * Deduplicates paths to avoid checking the same mem twice.
 * @param {object} mcpConfig - Parsed .mcp.json content
 * @returns {string[]} All unique mem dir paths
 */
export function findAllMemDirs(mcpConfig) {
  const seen = new Set();
  const roots = [];
  const servers = mcpConfig?.mcpServers || {};
  for (const server of Object.values(servers)) {
    const args = server.args || [];
    for (let i = 0; i < args.length; i++) {
      if (args[i] === '--mem' && args[i + 1]) {
        const mem = args[i + 1];
        if (!seen.has(mem)) {
          seen.add(mem);
          roots.push(mem);
        }
      }
    }
  }
  return roots;
}
