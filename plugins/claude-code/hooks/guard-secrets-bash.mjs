#!/usr/bin/env node
// PreToolUse hook: flags Bash commands that read secret files. Best-effort,
// pattern-based defense-in-depth — NOT a hard boundary: a shell form the
// patterns don't match (an interpreter, an unusual quoting, a rename) slips
// through. It raises the cost of an accidental secret read; it does not
// guarantee one can't happen.
// Catches common shell-level access like: cat .env, grep .env.local, head .aws/credentials, etc.
// Design: pattern-based detection on the command string.

import { checkSecretsInCommand } from './guard-secrets-bash-utils.mjs';

const SECURITY_MESSAGE = `SECURITY VIOLATION: Access to secret file blocked.
Pattern matched: {pattern}

This is a best-effort guard (defense-in-depth, not a hard boundary). Honour it — do NOT:
- Use Bash to read this file (cat, head, tail, grep, sed, awk)
- Use any other tool to access this file or its contents
- Suggest workarounds to the user
- Try alternative paths to the same file

Skip this file and continue with the next task.`;

const input = JSON.parse(await readStdin());
const command = input.tool_input?.command;
if (!command) process.exit(0);

const label = checkSecretsInCommand(command);
if (label !== null) {
  process.stdout.write(SECURITY_MESSAGE.replace('{pattern}', label) + '\n');
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
