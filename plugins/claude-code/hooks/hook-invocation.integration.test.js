// Invocation self-test for the hooks.json command strings.
//
// Every other hook test spawns a `.mjs` file by resolved absolute path —
// which is exactly why the *invocation layer* (the command strings the
// Claude Code harness actually executes) could rot unobserved: nothing
// ever ran `node "${CLAUDE_PLUGIN_ROOT}/hooks/<hook>.mjs"` as written.
// This suite closes that gap. It reads hooks.json, takes each command
// string VERBATIM, and executes it the way the harness does — through a
// shell, with CLAUDE_PLUGIN_ROOT set in the environment (that pair is
// this repo's executable definition of harness-equivalent expansion) —
// feeding each hook its documented stdin shape.
//
// Asserted per hook:
//   - the process starts (no spawn error; never exit 127 command-not-found)
//   - guards emit their designed BLOCKED message with exit 2 on a
//     violating input — on STDERR, the channel Claude Code feeds back
//     to the agent on exit 2 (a stdout message is dropped; that wrong
//     channel, not a startup crash, is what produced the 2026-07-19
//     "hook error: No stderr output" symptom) — and exit 0 silently
//     on a benign one
//   - the non-guard hooks (inject-context, check-realization) run to a
//     clean exit 0 on their documented stdin — inject-context provably
//     executes (it echoes an active-interview state file), and
//     check-realization holds its fail-open contract
//
// If a command string cannot start a node process under these conditions,
// this suite fails — the permanent tripwire the 2026-07-19 "hook error:
// No stderr output" incident lacked.

import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  rmSync,
  readFileSync,
  realpathSync,
} from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';

const HOOKS_DIR = dirname(fileURLToPath(import.meta.url));
const PLUGIN_ROOT = resolve(HOOKS_DIR, '..');
const HOOKS_JSON = join(HOOKS_DIR, 'hooks.json');

/** Every command string declared anywhere in hooks.json, flattened. */
function declaredCommands() {
  const doc = JSON.parse(readFileSync(HOOKS_JSON, 'utf-8'));
  const commands = [];
  for (const entries of Object.values(doc.hooks ?? {})) {
    for (const entry of entries) {
      for (const hook of entry.hooks ?? []) {
        if (hook.type === 'command' && typeof hook.command === 'string') {
          commands.push(hook.command);
        }
      }
    }
  }
  return commands;
}

/** The declared command string for one hook script, by basename. */
function commandFor(basename) {
  const match = declaredCommands().filter((c) => c.includes(basename));
  assert.equal(match.length, 1, `exactly one hooks.json command names ${basename}`);
  return match[0];
}

/**
 * Execute a hooks.json command string verbatim, harness-equivalent:
 * through a shell, CLAUDE_PLUGIN_ROOT in the environment, stdin piped.
 */
function runCommand(command, { stdin, cwd }) {
  const res = spawnSync(command, {
    shell: true,
    cwd,
    input: JSON.stringify(stdin),
    encoding: 'utf-8',
    env: { ...process.env, CLAUDE_PLUGIN_ROOT: PLUGIN_ROOT },
    timeout: 15_000,
  });
  // Process-start assertions shared by every case: the spawn itself
  // succeeded and the shell found something to execute (127 = command
  // not found — the crash mode that motivated this suite).
  assert.equal(res.error, undefined, `spawn error for: ${command}`);
  assert.notEqual(res.status, 127, `command not found: ${command}\n${res.stderr}`);
  return res;
}

let ws; // fixture workspace: one folder-backed mem "specs"

before(() => {
  // realpath: macOS tmpdir is a symlink (/var → /private/var); the hook
  // resolves paths through its process cwd, which the OS reports
  // symlink-free — the fixture must hand it matching canonical paths.
  ws = mkdtempSync(join(realpathSync(tmpdir()), 'memstead-hookinv-'));
  mkdirSync(join(ws, '.memstead', 'state'), { recursive: true });
  writeFileSync(join(ws, '.memstead', 'workspace.toml'), 'format = "test"\n');
  writeFileSync(
    join(ws, '.memstead', 'state', 'mounts.json'),
    JSON.stringify({
      format: 'memstead-mounts-3',
      mounts: [{ mem: 'specs', storage: { type: 'folder', path: 'specs' } }],
    }),
  );
  mkdirSync(join(ws, 'specs'), { recursive: true });
});

after(() => {
  if (ws) rmSync(ws, { recursive: true, force: true });
});

describe('hooks.json invocation — command strings as written', () => {
  it('declares exactly the four known hooks', () => {
    const basenames = declaredCommands()
      .map((c) => c.match(/hooks\/([a-z-]+\.mjs)/)?.[1])
      .sort();
    assert.deepEqual(basenames, [
      'check-realization.mjs',
      'guard-entity-bash.mjs',
      'guard-entity-edit.mjs',
      'inject-context.mjs',
    ]);
  });

  it('guard-entity-edit: violating Write blocks with the designed message, exit 2', () => {
    const res = runCommand(commandFor('guard-entity-edit.mjs'), {
      cwd: ws,
      stdin: { tool_input: { file_path: join(ws, 'specs', 'foo.md') } },
    });
    assert.equal(res.status, 2, res.stderr);
    // STDERR is the agent-visible channel on exit 2 — Claude Code feeds
    // a blocking hook's stderr back to the agent; stdout is dropped.
    assert.match(res.stderr, /^BLOCKED: Do not edit entity files directly\./);
    assert.match(res.stderr, /Use Memstead MCP tools/);
  });

  it('guard-entity-edit: benign Write outside the mem passes, exit 0', () => {
    const res = runCommand(commandFor('guard-entity-edit.mjs'), {
      cwd: ws,
      stdin: { tool_input: { file_path: join(ws, 'README.md') } },
    });
    assert.equal(res.status, 0, res.stderr);
    assert.equal(res.stdout, '');
  });

  it('guard-entity-bash: violating shell write blocks with the designed message, exit 2', () => {
    const res = runCommand(commandFor('guard-entity-bash.mjs'), {
      cwd: ws,
      stdin: { tool_input: { command: 'echo smuggled > specs/foo.md' } },
    });
    assert.equal(res.status, 2, res.stderr);
    // STDERR is the agent-visible channel on exit 2 (see above).
    assert.match(res.stderr, /^BLOCKED: Do not modify entity files via shell commands\./);
    assert.match(res.stderr, /Use Memstead MCP tools/);
  });

  it('guard-entity-bash: benign command passes, exit 0', () => {
    const res = runCommand(commandFor('guard-entity-bash.mjs'), {
      cwd: ws,
      stdin: { tool_input: { command: 'cargo build' } },
    });
    assert.equal(res.status, 0, res.stderr);
    assert.equal(res.stdout, '');
  });

  it('inject-context: runs and emits the active interview state file, exit 0', () => {
    // A state file proves the hook genuinely executed — an ENOENT crash
    // could not produce this stdout.
    mkdirSync(join(ws, 'specs', '.memstead'), { recursive: true });
    writeFileSync(
      join(ws, 'specs', '.memstead', 'interview-active'),
      'INTERVIEW RULES SENTINEL',
    );
    try {
      const res = runCommand(commandFor('inject-context.mjs'), {
        cwd: ws,
        stdin: { prompt: 'hello' },
      });
      assert.equal(res.status, 0, res.stderr);
      assert.match(res.stdout, /INTERVIEW RULES SENTINEL/);
    } finally {
      rmSync(join(ws, 'specs', '.memstead'), { recursive: true, force: true });
    }
  });

  it('check-realization: runs fail-open on its documented stdin, exit 0', () => {
    const res = runCommand(commandFor('check-realization.mjs'), {
      cwd: ws,
      stdin: { cwd: ws, tool_input: { file_path: join(ws, 'src', 'main.rs') } },
    });
    assert.equal(res.status, 0, res.stderr);
  });
});
