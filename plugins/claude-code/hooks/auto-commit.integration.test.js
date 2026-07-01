// End-to-end integration tests for the outer-repo commit pipeline.
// Uses real git subprocesses against temporary repos; mocks only the
// MCP engine layer (withEngineFn) so the tests run hermetically.

import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  rmSync,
  existsSync,
  readFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { produceOuterCommit, GIT_EMPTY_TREE_SHA } from './auto-commit-utils.mjs';

function git(args, { cwd, input } = {}) {
  return spawnSync('git', args, { cwd, encoding: 'utf-8', input });
}

function setupOuterRepo() {
  const root = mkdtempSync(join(tmpdir(), 'autocommit-e2e-'));
  git(['init', '-q', '-b', 'main'], { cwd: root });
  git(['config', 'user.email', 'test@example.com'], { cwd: root });
  git(['config', 'user.name', 'Test'], { cwd: root });
  git(['config', 'commit.gpgsign', 'false'], { cwd: root });
  return root;
}

function makeMem(outerRoot, name) {
  const worktree = join(outerRoot, name);
  mkdirSync(worktree, { recursive: true });
  const gitdir = join(worktree, '.git');
  git(['init', '-q', '-b', 'main', worktree], { cwd: outerRoot });
  git(['config', 'user.email', 'test@example.com'], { cwd: worktree });
  git(['config', 'user.name', 'Test'], { cwd: worktree });
  git(['config', 'commit.gpgsign', 'false'], { cwd: worktree });
  return { name, gitdir, worktree };
}

function commitInMem(layout, filename, content, subject, trailers = {}) {
  writeFileSync(join(layout.worktree, filename), content);
  git(['add', filename], { cwd: layout.worktree });
  const lines = [subject, ''];
  if (trailers.note) {
    lines.push(trailers.note);
    lines.push('');
  }
  if (trailers.tool) lines.push(`Tool: ${trailers.tool}`);
  if (trailers.actor) lines.push(`Actor: ${trailers.actor}`);
  if (trailers.client) lines.push(`Client: ${trailers.client}`);
  git(['commit', '-q', '-F', '-'], { cwd: layout.worktree, input: lines.join('\n') });
  return git(['rev-parse', 'HEAD'], { cwd: layout.worktree }).stdout.trim();
}

function healthFor(outerVcs, layouts, extra = {}) {
  // Synthesise per-mem `head` from the test fixture's real git
  // state so the migrated pipeline can read it from the `memstead_health`
  // response (it no longer peels refs itself).
  return {
    writable_mems: layouts.map((l) => l.name),
    mems: layouts.map((l) => {
      const headRes = git(['rev-parse', 'HEAD'], { cwd: l.worktree });
      const head = headRes.status === 0 ? headRes.stdout.trim() : null;
      return {
        name: l.name,
        vcs: { gitdir: l.gitdir, worktree: l.worktree, ...(head ? { head } : {}) },
      };
    }),
    plugin: { claude_code: { outer_vcs: outerVcs } },
    ...extra,
  };
}

/**
 * Parse a single commit body into the engine's `CommitNote`-shaped
 * record. Test-fixture-only: the integration tests build commits with
 * canonical trailer blocks via `commitInMem`, so this parser exists
 * to round-trip them back into the synthesised MCP response. Mirrors
 * what the engine's `parse_commit_message` does in Rust.
 */
function parseTestCommit(sha, body) {
  const trimmed = (body ?? '').trimEnd();
  const lines = trimmed.split('\n');
  const subject = lines[0] ?? '';
  const subjectMatch = subject.match(/^memstead:\s+(\S+)\s*(.*)$/);
  const tool_verb = subjectMatch ? subjectMatch[1] : null;
  const entity_id = subjectMatch ? (subjectMatch[2] || null) : null;
  const trailerIdx = lines.findIndex((l) => /^[A-Z][A-Za-z-]+:\s\S/.test(l));
  let note = null;
  let tool = null;
  let actor = null;
  let client = null;
  if (trailerIdx > 0) {
    const noteSlice = lines.slice(1, trailerIdx).join('\n').trim();
    note = noteSlice.length > 0 ? noteSlice : null;
    for (const line of lines.slice(trailerIdx)) {
      const m = line.match(/^([A-Z][A-Za-z-]+):\s+(.+?)\s*$/);
      if (!m) continue;
      const [, key, val] = m;
      if (key === 'Tool' && tool === null) tool = val;
      else if (key === 'Actor' && actor === null) actor = val.toLowerCase();
      else if (key === 'Client' && client === null) client = val;
    }
  }
  return {
    sha,
    subject,
    tool_verb,
    entity_id,
    note,
    actor,
    tool,
    client,
    timestamp: 0,
  };
}

/**
 * Build a mock `withEngineFn` that synthesises `memstead_health` and
 * `memstead_changes_since(... include_notes: true)` responses by reading
 * the real test mem-repo gitdirs. Test-fixture infrastructure: the
 * git invocations here are looking up fixture state, not running plugin
 * code — exempt from the no-direct-git (engine-owns-mem-repo) rule.
 *
 * `changesByMem` keys a per-mem descriptor `{ changes, head, notes,
 * memstead_ref }`. Any field can be omitted: `notes` defaults to the
 * commits the test wrote into the mem between `args.since` and the
 * recorded head, `memstead_ref` defaults to `undefined` (no ride-along).
 */
function mcpFactory(health, changesByMem) {
  return async (_cmd, _timeout, fn) => {
    const client = {
      async callTool(name, args) {
        if (name === 'memstead_health') return health;
        if (name === 'memstead_changes_since') {
          const v = changesByMem[args.mem] ?? {};
          // Resolve head: explicit override > test mem-repo HEAD > since
          let head = v.head;
          if (!head) {
            const layout = (health.mems ?? []).find((x) => x.name === args.mem);
            const worktree = layout?.vcs?.worktree;
            if (worktree) {
              const r = git(['rev-parse', 'HEAD'], { cwd: worktree });
              if (r.status === 0) head = r.stdout.trim();
            }
          }
          head = head ?? args.since;

          // Default `notes` from the commit log when include_notes is true
          // and the caller didn't supply an explicit override.
          let notes = v.notes;
          if (args.include_notes && notes === undefined) {
            const layout = (health.mems ?? []).find((x) => x.name === args.mem);
            const worktree = layout?.vcs?.worktree;
            notes = [];
            if (worktree && head !== args.since) {
              const range =
                args.since && args.since !== GIT_EMPTY_TREE_SHA
                  ? `${args.since}..${head}`
                  : head;
              const logRes = git(
                ['log', '--format=%H%x00%B%x01', range],
                { cwd: worktree },
              );
              if (logRes.status === 0) {
                for (const chunk of logRes.stdout.split('\x01')) {
                  const stripped = chunk.replace(/^\n+/, '');
                  if (!stripped) continue;
                  const [sha, ...rest] = stripped.split('\x00');
                  if (!sha || rest.length === 0) continue;
                  notes.push(parseTestCommit(sha.trim(), rest.join('\x00')));
                }
              }
            }
          }

          return {
            changes: v.changes ?? [],
            head,
            ...(args.include_notes
              ? {
                  notes: notes ?? [],
                  ...(v.memstead_ref ? { memstead_ref: v.memstead_ref } : {}),
                }
              : {}),
          };
        }
        return null;
      },
    };
    return fn(client);
  };
}

async function runSeed(outerRoot, layouts) {
  const health = healthFor({ enabled: true, mode: 'session_bundle', author: 'inherit' }, layouts);
  return produceOuterCommit({
    engineCommand: { cmd: 'true', args: [], cwd: outerRoot },
    workspaceRoot: outerRoot,
    sessionId: 'sess-seed',
    skipEnabledCheck: false,
    withEngineFn: mcpFactory(health, {}),
    logger: { error: () => {} },
  });
}

describe('integration: seed commit on first run', () => {
  let outerRoot;
  let layout;
  before(() => {
    outerRoot = setupOuterRepo();
    layout = makeMem(outerRoot, 'engine');
    commitInMem(layout, 'foo.md', 'foo', 'memstead: create engine--foo', {
      note: 'added foo',
      tool: 'memstead_create',
      actor: 'Agent',
      client: 'claude-code@2.1.0',
    });
  });
  after(() => rmSync(outerRoot, { recursive: true, force: true }));

  it('writes a seed commit with Memstead-cursor trailers, no retroactive notes', async () => {
    const r = await runSeed(outerRoot, [layout]);
    assert.strictEqual(r.status, 'committed');
    assert.strictEqual(r.kind, 'seed');

    const body = git(['log', '--format=%B', '-n', '1', 'HEAD'], { cwd: outerRoot }).stdout;
    assert.match(body, /^memstead: initialize cursor \(1 mems\)\n/);
    assert.match(body, /Memstead-cursor: engine@[0-9a-f]+/);
    assert.doesNotMatch(body, /Agent notes:/);
    assert.doesNotMatch(body, /\nSession:/);
  });

  it('subsequent run with no new changes returns no-changes', async () => {
    const headSha = git(['rev-parse', 'HEAD'], { cwd: layout.worktree }).stdout.trim();
    const health = healthFor({ enabled: true, mode: 'session_bundle', author: 'inherit' }, [layout]);
    const r = await produceOuterCommit({
      engineCommand: { cmd: 'true', args: [], cwd: outerRoot },
      workspaceRoot: outerRoot,
      sessionId: 'sess-followup',
      skipEnabledCheck: false,
      withEngineFn: mcpFactory(health, { engine: { changes: [], head: headSha } }),
      logger: { error: () => {} },
    });
    assert.strictEqual(r.status, 'no-changes');
  });
});

describe('integration: two mutating turns produce two outer commits', () => {
  let outerRoot;
  let layout;
  before(() => {
    outerRoot = setupOuterRepo();
    layout = makeMem(outerRoot, 'engine');
  });
  after(() => rmSync(outerRoot, { recursive: true, force: true }));

  it('first turn seeds, second turn bundles only the second turn notes', async () => {
    // Turn 1 — commits a per-mem change, pipeline fires as seed path
    const firstHead = commitInMem(layout, 'turn1.md', '1', 'memstead: create engine--turn1', {
      note: 'first turn note',
      tool: 'memstead_create',
      actor: 'Agent',
      client: 'claude-code@2.1.0',
    });
    const health = healthFor({ enabled: true, mode: 'session_bundle', author: 'inherit' }, [layout]);
    const r1 = await produceOuterCommit({
      engineCommand: { cmd: 'true', args: [], cwd: outerRoot },
      workspaceRoot: outerRoot,
      sessionId: 'sess-t1',
      skipEnabledCheck: false,
      withEngineFn: mcpFactory(health, { engine: { changes: [{ sha: firstHead }], head: firstHead } }),
      logger: { error: () => {} },
    });
    assert.strictEqual(r1.status, 'committed');
    assert.strictEqual(r1.kind, 'seed');

    // Turn 2 — another per-mem commit; pipeline produces normal commit
    const secondHead = commitInMem(layout, 'turn2.md', '2', 'memstead: create engine--turn2', {
      note: 'second turn note',
      tool: 'memstead_create',
      actor: 'Agent',
      client: 'claude-code@2.1.0',
    });
    const r2 = await produceOuterCommit({
      engineCommand: { cmd: 'true', args: [], cwd: outerRoot },
      workspaceRoot: outerRoot,
      sessionId: 'sess-t2',
      skipEnabledCheck: false,
      withEngineFn: mcpFactory(health, { engine: { changes: [{ sha: secondHead }], head: secondHead } }),
      logger: { error: () => {} },
    });
    assert.strictEqual(r2.status, 'committed');
    assert.strictEqual(r2.kind, 'normal');

    const body = git(['log', '--format=%B', '-n', '1', 'HEAD'], { cwd: outerRoot }).stdout;
    assert.match(body, /^memstead: session changes \(1 entities, 1 mems\)\n/);
    assert.match(body, /Agent notes:\n- \[engine\] second turn note/);
    assert.doesNotMatch(body, /first turn note/);
    assert.match(body, new RegExp(`Memstead-cursor: engine@${secondHead}`));
    assert.match(body, /\nSession: sess-t2\n/);

    const commits = git(['log', '--oneline'], { cwd: outerRoot }).stdout.trim().split('\n');
    assert.strictEqual(commits.length, 2);
  });
});

describe('integration: skill-hook parity', () => {
  let outerRoot;
  let layout;
  before(() => {
    outerRoot = setupOuterRepo();
    layout = makeMem(outerRoot, 'engine');
  });
  after(() => rmSync(outerRoot, { recursive: true, force: true }));

  it('hook body differs from skill body only by the Session trailer', async () => {
    // Bootstrap: per-mem seed + outer seed commit
    commitInMem(layout, 'seed.md', 's', 'memstead: create engine--seed', {
      note: 'seed note',
      tool: 'memstead_create',
      actor: 'Agent',
      client: 'claude-code@2.1.0',
    });
    await runSeed(outerRoot, [layout]);

    // Hook run — new per-mem commit, hook pipeline
    const hookHead = commitInMem(layout, 'hook.md', 'h', 'memstead: create engine--hook', {
      note: 'hook commit',
      tool: 'memstead_create',
      actor: 'Agent',
      client: 'claude-code@2.1.0',
    });
    const health = healthFor({ enabled: true, mode: 'session_bundle', author: 'inherit' }, [layout]);
    const rh = await produceOuterCommit({
      engineCommand: { cmd: 'true', args: [], cwd: outerRoot },
      workspaceRoot: outerRoot,
      sessionId: 'sess-hook',
      skipEnabledCheck: false,
      withEngineFn: mcpFactory(health, { engine: { changes: [{ sha: hookHead }], head: hookHead } }),
      logger: { error: () => {} },
    });
    assert.strictEqual(rh.status, 'committed');
    const hookBody = git(['log', '--format=%B', '-n', '1', 'HEAD'], { cwd: outerRoot }).stdout;

    // Skill run — new per-mem commit, skill pipeline (sessionId=null)
    const skillHead = commitInMem(layout, 'skill.md', 's', 'memstead: create engine--skill', {
      note: 'skill commit',
      tool: 'memstead_create',
      actor: 'Agent',
      client: 'claude-code@2.1.0',
    });
    const rs = await produceOuterCommit({
      engineCommand: { cmd: 'true', args: [], cwd: outerRoot },
      workspaceRoot: outerRoot,
      sessionId: null,
      skipEnabledCheck: true,
      withEngineFn: mcpFactory(health, { engine: { changes: [{ sha: skillHead }], head: skillHead } }),
      logger: { error: () => {} },
    });
    assert.strictEqual(rs.status, 'committed');
    const skillBody = git(['log', '--format=%B', '-n', '1', 'HEAD'], { cwd: outerRoot }).stdout;

    assert.match(hookBody, /\nSession: sess-hook\n/);
    assert.doesNotMatch(skillBody, /\nSession:/);
    assert.match(hookBody, /^memstead: session changes \(/);
    assert.match(skillBody, /^memstead: session changes \(/);
    assert.match(hookBody, /Memstead-cursor: engine@[0-9a-f]+/);
    assert.match(skillBody, /Memstead-cursor: engine@[0-9a-f]+/);
    assert.match(hookBody, /hook commit/);
    assert.match(skillBody, /skill commit/);
  });
});

describe('integration: external drift attribution', () => {
  let outerRoot;
  let layout;
  before(() => {
    outerRoot = setupOuterRepo();
    layout = makeMem(outerRoot, 'engine');
  });
  after(() => rmSync(outerRoot, { recursive: true, force: true }));

  it('segregates Agent notes from External edits captured; skips no-Actor', async () => {
    // Bootstrap
    commitInMem(layout, 'initial.md', 'x', 'memstead: create engine--initial', {
      note: 'initial',
      tool: 'memstead_create',
      actor: 'Agent',
      client: 'claude-code@2.1.0',
    });
    await runSeed(outerRoot, [layout]);

    // External + agent + orphan (no Actor) — all in the same cursor window
    const extHead = commitInMem(layout, 'ext.md', 'e', 'memstead: external edits (1 files)', {
      tool: 'memstead_external',
      actor: 'External',
    });
    const orphanHead = commitInMem(layout, 'orphan.md', 'o', 'memstead: create engine--orphan', {
      note: 'orphan — no actor',
      tool: 'memstead_create',
      // no actor trailer
    });
    const agentHead = commitInMem(layout, 'agent.md', 'a', 'memstead: create engine--with-agent', {
      note: 'agent updated',
      tool: 'memstead_create',
      actor: 'Agent',
      client: 'claude-code@2.1.0',
    });

    const errors = [];
    const health = healthFor({ enabled: true, mode: 'session_bundle', author: 'inherit' }, [layout]);
    const r = await produceOuterCommit({
      engineCommand: { cmd: 'true', args: [], cwd: outerRoot },
      workspaceRoot: outerRoot,
      sessionId: 'sess-mix',
      skipEnabledCheck: false,
      withEngineFn: mcpFactory(health, {
        engine: {
          changes: [{ sha: extHead }, { sha: orphanHead }, { sha: agentHead }],
          head: agentHead,
        },
      }),
      logger: { error: (m) => errors.push(m) },
    });
    assert.strictEqual(r.status, 'committed');
    const body = git(['log', '--format=%B', '-n', '1', 'HEAD'], { cwd: outerRoot }).stdout;
    assert.match(body, /Agent notes:\n- \[engine\] agent updated/);
    assert.match(body, /External edits captured:\n- \[engine\] external edits \(1 files\)/);
    assert.doesNotMatch(body, /orphan — no actor/);
    assert.match(body, new RegExp(`Memstead-cursor: engine@${agentHead}`));
    assert.ok(errors.some((e) => /no recognized Actor trailer/.test(e)));
  });
});

describe('integration: look-alike commit rejected', () => {
  let outerRoot;
  let layout;
  before(() => {
    outerRoot = setupOuterRepo();
    layout = makeMem(outerRoot, 'engine');
    commitInMem(layout, 'a.md', 'a', 'memstead: create engine--a', {
      note: 'real',
      tool: 'memstead_create',
      actor: 'Agent',
      client: 'claude-code@2.1.0',
    });
  });
  after(() => rmSync(outerRoot, { recursive: true, force: true }));

  it('skips a handcrafted "memstead: session changes" commit without trailers', async () => {
    writeFileSync(join(outerRoot, 'handcrafted.md'), 'x');
    git(['add', 'handcrafted.md'], { cwd: outerRoot });
    git(['commit', '-q', '-m', 'memstead: session changes (handcrafted)'], { cwd: outerRoot });

    const r = await runSeed(outerRoot, [layout]);
    assert.strictEqual(r.status, 'committed');
    assert.strictEqual(r.kind, 'seed');
    const body = git(['log', '--format=%B', '-n', '1', 'HEAD'], { cwd: outerRoot }).stdout;
    assert.match(body, /^memstead: initialize cursor /);
  });
});

describe('integration: stale cursor fallback', () => {
  let outerRoot;
  let layout;
  before(() => {
    outerRoot = setupOuterRepo();
    layout = makeMem(outerRoot, 'engine');
    commitInMem(layout, 'a.md', 'a', 'memstead: create engine--a', {
      note: 'a',
      tool: 'memstead_create',
      actor: 'Agent',
      client: 'claude-code@2.1.0',
    });
  });
  after(() => rmSync(outerRoot, { recursive: true, force: true }));

  it('falls back to empty-tree when the prior cursor SHA is unreachable', async () => {
    // Outer-repo cursor points at a non-existent SHA in the mem gitdir.
    // The migrated pipeline catches the engine's OBJECT_NOT_FOUND error
    // from memstead_changes_since and retries with the empty-tree sentinel.
    const unreachable = 'deadbeefcafebabe0000000000000000deadbeef';
    writeFileSync(join(outerRoot, 'x.md'), 'x');
    git(['add', 'x.md'], { cwd: outerRoot });
    git(['commit', '-q', '-F', '-'], {
      cwd: outerRoot,
      input:
        `memstead: session changes (0 entities, 1 mems)\n\nMems: engine\nMemstead-cursor: engine@${unreachable}\n`,
    });

    const headSha = git(['rev-parse', 'HEAD'], { cwd: layout.worktree }).stdout.trim();
    const errors = [];
    const health = healthFor({ enabled: true, mode: 'session_bundle', author: 'inherit' }, [layout]);
    const baseFactory = mcpFactory(health, {});
    const failingFactory = async (cmd, timeout, fn) => {
      return baseFactory(cmd, timeout, async (client) => {
        const wrapped = {
          async callTool(name, args) {
            if (
              name === 'memstead_changes_since'
              && args?.mem === 'engine'
              && args?.since === unreachable
            ) {
              throw new Error(`MCP memstead_changes_since error: ${unreachable}: OBJECT_NOT_FOUND`);
            }
            return client.callTool(name, args);
          },
        };
        return fn(wrapped);
      });
    };
    const r = await produceOuterCommit({
      engineCommand: { cmd: 'true', args: [], cwd: outerRoot },
      workspaceRoot: outerRoot,
      sessionId: 'sess-stale',
      skipEnabledCheck: false,
      withEngineFn: failingFactory,
      logger: { error: (m) => errors.push(m) },
    });
    assert.strictEqual(r.status, 'committed');
    assert.ok(
      errors.some((e) => /cursor .* unreachable/.test(e)),
      `expected stderr to flag unreachable cursor, got: ${errors.join('\n')}`,
    );
    const body = git(['log', '--format=%B', '-n', '1', 'HEAD'], { cwd: outerRoot }).stdout;
    assert.match(body, new RegExp(`Memstead-cursor: engine@${headSha}`));
  });
});

describe('integration: sanity — removed files and skill rename', () => {
  it('session-init.mjs is gone from hooks/', () => {
    const path = resolve(new URL('.', import.meta.url).pathname, 'session-init.mjs');
    assert.strictEqual(existsSync(path), false);
  });

  it('outer-commit skill exists and is named outer-commit', () => {
    const skillPath = resolve(
      new URL('.', import.meta.url).pathname,
      '../skills/outer-commit/SKILL.md',
    );
    assert.ok(existsSync(skillPath), 'outer-commit SKILL.md should exist');
    const content = readFileSync(skillPath, 'utf-8');
    assert.match(content, /^name: outer-commit$/m);
  });

  it('old commit skill directory is gone', () => {
    const oldPath = resolve(new URL('.', import.meta.url).pathname, '../skills/commit');
    assert.strictEqual(existsSync(oldPath), false);
  });
});
