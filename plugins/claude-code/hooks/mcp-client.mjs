// Minimal stdio-transport MCP client used by the auto-commit hook family.
// Spawns the engine binary named in `.mcp.json`, completes the `initialize`
// handshake, invokes one or more `tools/call` requests, and shuts the
// child down. No npm deps — plain Node, shelling out to the binary the
// plugin is already configured to launch.

import { spawn } from 'node:child_process';
import { readFileSync, existsSync } from 'node:fs';
import { resolve, dirname } from 'node:path';

const CLIENT_NAME = 'memstead-plugin-auto-commit';
const CLIENT_VERSION = '0.1.0';
const PROTOCOL_VERSION = '2024-11-05';

/**
 * Resolve the engine command + args from `.mcp.json` at the given workspace
 * root. Returns `null` if the file or the `memstead` server entry is
 * missing — callers treat that as a silent no-op (plugin not configured
 * for this repo).
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
  // `command` in `.mcp.json` may be workspace-relative (e.g. `engine/
  // target/release/memstead-mcp`). Resolve against the workspace root so the
  // hook doesn't care what the hook's cwd happens to be.
  const cmd = resolve(workspaceRoot, entry.command);
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
