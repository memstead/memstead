#!/usr/bin/env node
// PreToolUse hook: blocks Read/Write/Edit on secret files.
// Discourages access to sensitive files like .env, private keys, credentials.
// Best-effort, pattern-based defense-in-depth — NOT a hard boundary: it only
// sees Read/Write/Edit tool paths that match its patterns, and a differently
// spelled path or another access route slips through.
// Design: fail-closed on the paths it does see — unknown paths that match
// patterns are blocked.

import { isSecretFile } from './guard-secrets-read-utils.mjs';

const SECURITY_MESSAGE = `SECURITY VIOLATION: Access to secret file blocked.
File: {file}

This is a best-effort guard (defense-in-depth, not a hard boundary). Honour it — do NOT:
- Use Bash to read this file (cat, head, tail, grep, sed, awk)
- Use any other tool to access this file or its contents
- Suggest workarounds to the user
- Try alternative paths to the same file

Skip this file and continue with the next task.`;

const input = JSON.parse(await readStdin());
const filePath = input.tool_input?.file_path;
if (!filePath) process.exit(0);

if (isSecretFile(filePath)) {
  process.stdout.write(SECURITY_MESSAGE.replace('{file}', filePath) + '\n');
  process.exit(2);
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
