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
 * @param {string} vaultDir - Resolved absolute path to vault directory
 * @param {boolean} vaultDirExists - Whether vaultDir exists on disk
 * @returns {{ action: 'block'|'allow', reason?: string }}
 */
export function checkEditTarget(filePath, vaultDir, vaultDirExists) {
  if (!filePath) return { action: 'allow' };

  // Fail-closed: if vault dir doesn't exist, block potential entity files
  if (!vaultDirExists) {
    const absPath = resolve(filePath);
    if (absPath.includes('specs') && isEntityFilename(basename(absPath))) {
      return {
        action: 'block',
        reason: `Cannot verify vault dir at ${vaultDir} — refusing edit on potential entity file as precaution. File: ${filePath}`,
      };
    }
    return { action: 'allow' };
  }

  const dirName = vaultDir.split('/').pop() || 'specs';
  const projectRoot = resolve(vaultDir, '..');
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
 * Find --vault from .mcp.json config object (first vault of first server).
 * @param {object} mcpConfig - Parsed .mcp.json content
 * @returns {string} The vault dir path or empty string
 */
export function findVaultDir(mcpConfig) {
  const roots = findAllVaultDirs(mcpConfig);
  return roots[0] || '';
}

/**
 * Collect all --vault paths from all MCP servers in .mcp.json.
 * Deduplicates paths to avoid checking the same vault twice.
 * @param {object} mcpConfig - Parsed .mcp.json content
 * @returns {string[]} All unique vault dir paths
 */
export function findAllVaultDirs(mcpConfig) {
  const seen = new Set();
  const roots = [];
  const servers = mcpConfig?.mcpServers || {};
  for (const server of Object.values(servers)) {
    const args = server.args || [];
    for (let i = 0; i < args.length; i++) {
      if (args[i] === '--vault' && args[i + 1]) {
        const vault = args[i + 1];
        if (!seen.has(vault)) {
          seen.add(vault);
          roots.push(vault);
        }
      }
    }
  }
  return roots;
}
