#!/usr/bin/env node
// PostToolUse hook: checks if an edited file is referenced by any entity.
// Fires on Write/Edit tools. Outputs a reminder to review relevant entities.
//
// The direct-edit warning always runs (guard behavior).
// The realization scan is conditional on schema drift support — if the mem's
// schema has no drift section, realization scanning is skipped silently.
//
// Schema is loaded from the mem's .memstead/config.json "schema" field.

import { readFileSync, existsSync, readdirSync } from 'node:fs';
import { basename, resolve, join, relative } from 'node:path';
import { pathToFileURL } from 'node:url';
import { extractRealizationPaths, fileToId, pathMatches } from './check-realization-utils.mjs';
import { isEntityFilename } from './guard-entity-edit-utils.mjs';
import { resolveMemDirsFromCwd } from './workspace-resolve-utils.mjs';

const input = JSON.parse(await readStdin());

// Extract the file path from the tool input
const filePath = input.tool_input?.file_path;
if (!filePath) process.exit(0);

// Resolve folder-backed mem dirs the engine way (walk up for
// .memstead/workspace.toml, read the mount list). Empty on a git-branch
// workspace — entities are branch blobs, nothing to scan on disk.
const absFilePath = resolve(filePath);
for (const memDir of resolveMemDirsFromCwd()) {
  if (!existsSync(memDir)) continue;

  // Normalize edited path to relative (from project root = parent of mem dir)
  const projectRoot = resolve(memDir, '..');
  const relPath = relative(projectRoot, absFilePath);
  const dirName = memDir.split('/').pop() || 'specs';
  const insideMem = relPath.startsWith(dirName + '/') || relPath.startsWith(dirName + '\\');

  // --- Guard behavior (always active) ---
  // Warn if an entity markdown was edited directly (backup check — the
  // PreToolUse guard should block this first).
  if (insideMem && isEntityFilename(basename(relPath))) {
    process.stdout.write(
      `WARNING: Entity file \`${relPath}\` was edited directly. Always use Memstead MCP tools (memstead_create, memstead_update) to mutate entities.\n`,
    );
    process.exit(0);
  }
  // A non-entity file inside the mem dir is this mem's concern but needs
  // no realization scan; stop here.
  if (insideMem) process.exit(0);

  // --- Realization scan (conditional on this mem's schema drift support) ---
  const SCHEMA = await loadMemSchema(memDir);
  if (!SCHEMA?.drift) continue;

  const matches = [];
  for (const file of findMarkdownFiles(memDir)) {
    const paths = extractRealizationPaths(readFileSync(file, 'utf-8'), SCHEMA);
    for (const p of paths) {
      if (pathMatches(relPath, p)) {
        const entityId = fileToId(relative(memDir, file));
        if (entityId) matches.push(entityId);
        break;
      }
    }
  }

  if (matches.length > 0) {
    process.stdout.write(
      `REALIZATION EDIT: You changed \`${relPath}\`. These entities reference it: ${matches.join(', ')}. Consider reviewing them with memstead_entity or /audit.\n`,
    );
    process.exit(0);
  }
}

// --- Helpers ---

// Load a folder mem's schema module from its `.memstead/config.json`
// "schema" field. Returns null when the config or schema is unavailable
// (e.g. a git-branch mem whose schema lives on the __SCHEMAS ref, not
// on disk).
async function loadMemSchema(memDir) {
  try {
    const configPath = join(memDir, '.memstead', 'config.json');
    if (!existsSync(configPath)) return null;
    const schemaRef = JSON.parse(readFileSync(configPath, 'utf-8')).schema;
    if (!schemaRef) return null;
    const isRelative = schemaRef.startsWith('./') || schemaRef.startsWith('../');
    const resolved = isRelative ? resolve(memDir, schemaRef) : schemaRef;
    const mod = resolved.startsWith('/')
      ? await import(pathToFileURL(resolved).href)
      : await import(resolved);
    return mod.default?.schema || mod.default || mod.schema;
  } catch {
    return null;
  }
}

function findMarkdownFiles(dir) {
  const results = [];
  try {
    const entries = readdirSync(dir, { withFileTypes: true });
    for (const e of entries) {
      if (e.name.startsWith('.')) continue;
      const full = join(dir, e.name);
      if (e.isDirectory()) {
        results.push(...findMarkdownFiles(full));
      } else if (e.name.endsWith('.md')) {
        results.push(full);
      }
    }
  } catch { /* silent */ }
  return results;
}

function readStdin() {
  return new Promise((resolve) => {
    let data = '';
    process.stdin.setEncoding('utf-8');
    process.stdin.on('data', chunk => { data += chunk; });
    process.stdin.on('end', () => resolve(data));
  });
}
