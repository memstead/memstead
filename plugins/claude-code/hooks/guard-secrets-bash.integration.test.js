// Wiring + end-to-end refusal tests for the secret-file guards.
//
// Unlike `guard-secrets-bash.test.js` (which tests the pure
// `checkSecretsInCommand` util), this suite asserts the guards are
// actually WIRED in `hooks.json` and refuse at runtime. It fails if
// `guard-secrets-bash.mjs` is removed from the `Bash` matcher — the
// security fail-open this plan closes (a Bash `tool_input` has no
// `file_path`, so the read-guard no-ops on `cat .env`).

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const HOOKS_DIR = dirname(fileURLToPath(import.meta.url));
const HOOKS_JSON = join(HOOKS_DIR, 'hooks.json');

/** PreToolUse entries whose matcher (a `|`-separated alternation) admits `tool`. */
function preToolUseEntriesFor(tool) {
  const cfg = JSON.parse(readFileSync(HOOKS_JSON, 'utf-8'));
  const entries = cfg.hooks?.PreToolUse ?? [];
  return entries.filter((e) =>
    (e.matcher ?? '').split('|').map((s) => s.trim()).includes(tool),
  );
}

/** Does any PreToolUse entry for `tool` invoke `scriptBasename`? */
function isWiredFor(tool, scriptBasename) {
  return preToolUseEntriesFor(tool).some((e) =>
    (e.hooks ?? []).some((h) => (h.command ?? '').includes(scriptBasename)),
  );
}

/** Run a hook script with a `tool_input` payload; return `{ status, stdout }`. */
function runGuard(script, toolInput) {
  const res = spawnSync('node', [join(HOOKS_DIR, script)], {
    input: JSON.stringify({ tool_input: toolInput }),
    encoding: 'utf-8',
  });
  return { status: res.status, stdout: res.stdout ?? '' };
}

describe('secret-guard wiring (hooks.json)', () => {
  it('wires guard-secrets-bash.mjs on the Bash matcher', () => {
    assert.ok(
      isWiredFor('Bash', 'guard-secrets-bash.mjs'),
      'guard-secrets-bash.mjs must be registered on a PreToolUse Bash matcher — ' +
        'without it, `cat .env` is not blocked (the read-guard no-ops on Bash, which has no file_path)',
    );
  });

  it('keeps guard-secrets-read.mjs wired on the Read matcher', () => {
    assert.ok(
      isWiredFor('Read', 'guard-secrets-read.mjs'),
      'guard-secrets-read.mjs must stay registered on a PreToolUse Read matcher',
    );
  });
});

describe('secret-guard runtime refusal', () => {
  it('blocks `cat .env` on the Bash surface (exit 2)', () => {
    const { status, stdout } = runGuard('guard-secrets-bash.mjs', { command: 'cat .env' });
    assert.equal(status, 2, 'a Bash read of a secret file must be blocked (exit 2)');
    assert.match(stdout, /SECURITY VIOLATION/);
  });

  it('blocks shell bypasses like `grep password .env.local` (exit 2)', () => {
    const { status } = runGuard('guard-secrets-bash.mjs', {
      command: 'grep password .env.local',
    });
    assert.equal(status, 2);
  });

  it('allows a benign Bash command (exit 0)', () => {
    const { status } = runGuard('guard-secrets-bash.mjs', { command: 'ls -la' });
    assert.equal(status, 0, 'a non-secret command must pass through');
  });

  it('still refuses a Read of a secret path via the read-guard (exit 2)', () => {
    const { status } = runGuard('guard-secrets-read.mjs', { file_path: '.env' });
    assert.equal(status, 2, 'reading a secret path via Read must stay blocked');
  });
});
