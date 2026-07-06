// Minimal stdio-transport MCP client used by the auto-commit hook family.
// Spawns the engine binary named in `.mcp.json`, completes the `initialize`
// handshake, invokes one or more `tools/call` requests, and shuts the
// child down. No npm deps — plain Node, shelling out to the binary the
// plugin is already configured to launch.

import { spawn } from 'node:child_process';
import { readFileSync, existsSync } from 'node:fs';
import { resolve, dirname, isAbsolute, join, sep } from 'node:path';
import { homedir } from 'node:os';

const CLIENT_NAME = 'memstead-plugin-auto-commit';
const CLIENT_VERSION = '0.1.0';
const PROTOCOL_VERSION = '2024-11-05';

/**
 * Turn an `.mcp.json` `command` into what `spawn` should exec.
 *
 * A bare command name (`sh`, `node`, a binary on `PATH`) carries no path
 * separator — leave it untouched so `spawn` finds it on `PATH`. A path
 * (absolute, or workspace-relative like `target/release/memstead-mcp`) is
 * resolved against the workspace root so the hook's own cwd doesn't matter.
 *
 * The previous unconditional `resolve(workspaceRoot, command)` mangled a bare
 * `"sh"` into `<root>/sh` (ENOENT) — so the real `sh -c "cd <dir> && exec
 * <binary>"` launch form never spawned and the auto-commit Stop hook produced
 * zero commits. Both real launch forms (this shell form, and the bare-absolute
 * binary a fresh `quickstart` writes) now resolve correctly.
 */
export function resolveLaunchCommand(workspaceRoot, command) {
  if (isAbsolute(command) || command.includes('/') || command.includes(sep)) {
    return resolve(workspaceRoot, command);
  }
  return command;
}

/**
 * True when Claude Code would auto-start the named project `.mcp.json` server
 * for `workspaceRoot` — i.e. the user has approved it. The engine-spawning
 * hooks anchor to the SAME trust signals Claude Code uses, so they only ever
 * launch a `.mcp.json` command the user already approved for MCP. A
 * freshly-cloned repo the user has not approved carries none of these signals,
 * so the hook never spawns its `command` — closing the "hostile clone executes
 * code on the first prompt/edit" path. Fail-closed: unreadable/absent config
 * means untrusted.
 */
export function isEngineSpawnTrusted(workspaceRoot, serverName = 'memstead', home = homedir()) {
  const readJson = (path) => {
    try {
      return JSON.parse(readFileSync(path, 'utf-8'));
    } catch {
      return null;
    }
  };
  const layers = [
    readJson(join(workspaceRoot, '.claude', 'settings.local.json')),
    readJson(join(workspaceRoot, '.claude', 'settings.json')),
    readJson(join(home, '.claude', 'settings.json')),
    readJson(join(home, '.claude.json'))?.projects?.[workspaceRoot] ?? null,
  ].filter(Boolean);

  // An explicit deny in any layer wins over every allow signal.
  for (const layer of layers) {
    if (Array.isArray(layer.disabledMcpjsonServers) && layer.disabledMcpjsonServers.includes(serverName)) {
      return false;
    }
  }
  for (const layer of layers) {
    if (layer.enableAllProjectMcpServers === true) return true;
    if (Array.isArray(layer.enabledMcpjsonServers) && layer.enabledMcpjsonServers.includes(serverName)) {
      return true;
    }
  }
  return false;
}

/**
 * Resolve the engine command + args from `.mcp.json` at the given workspace
 * root. Returns `null` if the file or the `memstead` server entry is
 * missing, or if the user has not approved this workspace's `.mcp.json`
 * `memstead` server for MCP (see `isEngineSpawnTrusted`) — callers treat that
 * as a silent no-op (plugin not configured / not trusted for this repo).
 */
export function resolveEngineCommand(workspaceRoot) {
  const mcpJsonPath = resolve(workspaceRoot, '.mcp.json');
  if (!existsSync(mcpJsonPath)) return null;
  let raw;
  try {
    raw = JSON.parse(readFileSync(mcpJsonPath, 'utf-8'));
  } catch {
    return null;
  }
  const entry = raw.mcpServers?.memstead;
  if (!entry?.command) return null;
  // Trust anchor: never spawn a `.mcp.json` command the user hasn't approved
  // for MCP — the same gate Claude Code applies before auto-starting the
  // server. A hostile cloned repo carries no approval, so the hook no-ops.
  if (!isEngineSpawnTrusted(workspaceRoot)) return null;
  const cmd = resolveLaunchCommand(workspaceRoot, entry.command);
  const args = Array.isArray(entry.args) ? entry.args.slice() : [];
  return { cmd, args, cwd: workspaceRoot };
}

/**
 * Spawn the engine and run the given async fn with a client capable of
 * `callTool(name, params)`. The client is shut down after the fn returns
 * or throws. Each invocation pays the engine-boot cost once — batch your
 * tool calls inside the callback.
 */
export async function withEngine(cmdSpec, timeoutMs, fn) {
  const child = spawn(cmdSpec.cmd, cmdSpec.args, {
    cwd: cmdSpec.cwd,
    stdio: ['pipe', 'pipe', 'pipe'],
  });

  let buffer = '';
  const pending = new Map();
  let nextId = 1;
  let stderr = '';
  let closed = false;
  let onClose = null;

  child.stdout.setEncoding('utf-8');
  child.stderr.setEncoding('utf-8');

  child.stdout.on('data', (chunk) => {
    buffer += chunk;
    let nl;
    while ((nl = buffer.indexOf('\n')) !== -1) {
      const line = buffer.slice(0, nl).trim();
      buffer = buffer.slice(nl + 1);
      if (!line) continue;
      let msg;
      try {
        msg = JSON.parse(line);
      } catch {
        continue;
      }
      if (typeof msg.id !== 'undefined' && pending.has(msg.id)) {
        const entry = pending.get(msg.id);
        pending.delete(msg.id);
        entry.resolve(msg);
      }
    }
  });

  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });

  child.on('error', (err) => {
    closed = true;
    for (const entry of pending.values()) entry.reject(err);
    pending.clear();
    if (onClose) onClose();
  });
  child.on('close', () => {
    closed = true;
    for (const entry of pending.values()) {
      entry.reject(new Error(`engine closed before response (stderr: ${stderr.slice(-400)})`));
    }
    pending.clear();
    if (onClose) onClose();
  });

  function send(method, params) {
    if (closed) return Promise.reject(new Error('engine closed'));
    const id = nextId++;
    const req = { jsonrpc: '2.0', id, method, params };
    return new Promise((resolvePromise, rejectPromise) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        rejectPromise(new Error(`MCP ${method} timed out after ${timeoutMs}ms`));
      }, timeoutMs);
      pending.set(id, {
        resolve: (msg) => {
          clearTimeout(timer);
          if (msg.error) rejectPromise(new Error(`MCP ${method} error: ${msg.error.message || JSON.stringify(msg.error)}`));
          else resolvePromise(msg.result);
        },
        reject: (err) => {
          clearTimeout(timer);
          rejectPromise(err);
        },
      });
      child.stdin.write(`${JSON.stringify(req)}\n`);
    });
  }

  function notify(method, params) {
    if (closed) return;
    const req = { jsonrpc: '2.0', method, params };
    child.stdin.write(`${JSON.stringify(req)}\n`);
  }

  // MCP initialize handshake. `initialized` is a notification — no
  // response, but the server expects it before honouring `tools/call`.
  await send('initialize', {
    protocolVersion: PROTOCOL_VERSION,
    capabilities: {},
    clientInfo: { name: CLIENT_NAME, version: CLIENT_VERSION },
  });
  notify('notifications/initialized', {});

  const client = {
    async callTool(name, args) {
      const result = await send('tools/call', {
        name,
        arguments: args ?? {},
      });
      // Tool calls return `{ content, structuredContent, isError }`. Prefer
      // structured content where available — it's JSON the engine already
      // parsed. Fall back to the text channel for older shapes.
      if (result && result.structuredContent !== undefined) return result.structuredContent;
      const text = result?.content?.find?.((c) => c.type === 'text')?.text;
      if (text) {
        try {
          return JSON.parse(text);
        } catch {
          return text;
        }
      }
      return null;
    },
  };

  try {
    return await fn(client);
  } finally {
    try {
      child.stdin.end();
    } catch {
      // ignore — child already gone
    }
    if (!closed) {
      await new Promise((resolvePromise) => {
        onClose = resolvePromise;
        setTimeout(() => {
          if (!closed) {
            try {
              child.kill();
            } catch {
              // ignore
            }
            resolvePromise();
          }
        }, 2000);
      });
    }
  }
}

/**
 * Read raw stdin as UTF-8 and return the parsed JSON, or `{}` when the
 * payload is absent or unparseable. Claude Code delivers hook input on
 * stdin; absence is legitimate in ad-hoc manual runs.
 */
export async function readStdinJson() {
  return new Promise((resolvePromise) => {
    let data = '';
    if (process.stdin.isTTY) {
      resolvePromise({});
      return;
    }
    process.stdin.setEncoding('utf-8');
    process.stdin.on('data', (chunk) => {
      data += chunk;
    });
    process.stdin.on('end', () => {
      if (!data.trim()) {
        resolvePromise({});
        return;
      }
      try {
        resolvePromise(JSON.parse(data));
      } catch {
        resolvePromise({});
      }
    });
    process.stdin.on('error', () => resolvePromise({}));
  });
}
