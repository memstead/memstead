#!/usr/bin/env node
// Deterministic hook script for UserPromptSubmit.
// Reads interview state-file and outputs it to stdout.
// All other context (write guidance, write rules, mem info) is
// delivered via MCP Server Instructions (#42).

import { readFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { resolveMemDirsFromCwd } from './workspace-resolve-utils.mjs';

// Consume stdin to avoid EPIPE when Claude Code writes hook input
process.stdin.resume();
process.stdin.on('data', () => {});

// Interview rules (only when active). Re-inject from the first folder-backed
// mem carrying an active interview. Resolution mirrors the guards (walk up
// for .memstead/workspace.toml); empty on a git-branch workspace.
for (const memDir of resolveMemDirsFromCwd()) {
  const interviewFile = join(memDir, '.memstead', 'interview-active');
  if (existsSync(interviewFile)) {
    process.stdout.write(readFileSync(interviewFile, 'utf-8'));
    process.stdout.write('\n');
    break;
  }
}
