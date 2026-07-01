#!/usr/bin/env node
// PreToolUse hook: blocks direct Write/Edit on entity markdown files.
// Entity files must be mutated through Memstead MCP tools, never directly.
//
// Design: fail-closed — if we cannot determine whether a file is safe to edit,
// we block the operation rather than silently allowing it. (#48)

import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { checkEditTarget } from './guard-entity-edit-utils.mjs';
import { resolveVaultDirsFromCwd } from './workspace-resolve-utils.mjs';

const input = JSON.parse(await readStdin());

const filePath = input.tool_input?.file_path;
if (!filePath) process.exit(0);

// Resolve folder-backed vault dirs the engine way: explicit --vault args,
// else walk up for .memstead/workspace.toml and read the mount list. Empty on a
// git-branch workspace (entities are branch blobs, not working-tree files).
const vaultDirs = resolveVaultDirsFromCwd();

// Check against every vault — block if any vault claims the file
for (const root of vaultDirs) {
  const resolved = resolve(root);
  const result = checkEditTarget(filePath, resolved, existsSync(resolved));
  if (result.action === 'block') {
    process.stdout.write(
      `BLOCKED: Do not edit entity files directly. Use Memstead MCP tools (memstead_create, memstead_update, memstead_delete) to mutate entities. ${result.reason}\n`,
    );
    process.exit(2);
  }
}

process.exit(0);

function readStdin() {
  return new Promise((resolve) => {
    let data = '';
    process.stdin.setEncoding('utf-8');
    process.stdin.on('data', chunk => { data += chunk; });
    process.stdin.on('end', () => resolve(data));
  });
}
