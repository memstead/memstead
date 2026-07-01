#!/usr/bin/env node
// PreToolUse hook: blocks Bash commands that write to entity markdown files.
// Catches shell-level bypasses like: cat > specs/foo.md, sed -i, tee, mv, cp, etc.
//
// Design: pattern-based detection on the command string. Not foolproof against
// obfuscation, but catches the common accidental cases. (#48)

import { resolve } from 'node:path';
import { checkBashCommand } from './guard-entity-bash-utils.mjs';
import { resolveMemDirsFromCwd } from './workspace-resolve-utils.mjs';

const input = JSON.parse(await readStdin());

const command = input.tool_input?.command;
if (!command) process.exit(0);

// Resolve folder-backed mem dirs the engine way (see workspace-resolve-utils).
// Empty on a git-branch workspace — there are no working-tree entity files a
// shell command could target there.
const memDirs = resolveMemDirsFromCwd();

// Check against every mem directory name — block if any mem is targeted
for (const root of memDirs) {
  const resolved = resolve(root);
  const dirName = resolved.split('/').pop() || 'specs';
  const result = checkBashCommand(command, dirName);
  if (result.action === 'block') {
    process.stdout.write(
      `BLOCKED: Do not modify entity files via shell commands. Use Memstead MCP tools (memstead_create, memstead_update, memstead_delete) to mutate entities. ${result.reason}\n`,
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
