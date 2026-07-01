import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
  resolveOuterVcsConfig,
  extractMemLayouts,
  classifyMemNotes,
  memsteadRefToArray,
  parseCursorTrailers,
  formatCursorTrailers,
  buildOuterCommitMessage,
  buildSeedCommitMessage,
  readPriorCursor,
  produceOuterCommit,
  GIT_EMPTY_TREE_SHA,
} from './auto-commit-utils.mjs';

describe('resolveOuterVcsConfig', () => {
  it('returns null when plugin branch is missing', () => {
    assert.strictEqual(resolveOuterVcsConfig({}), null);
    assert.strictEqual(resolveOuterVcsConfig({ plugin: {} }), null);
    assert.strictEqual(resolveOuterVcsConfig({ plugin: { claude_code: {} } }), null);
  });

  it('applies defaults for partial configurations', () => {
    const cfg = resolveOuterVcsConfig({
      plugin: { claude_code: { outer_vcs: { enabled: true } } },
    });
    assert.deepStrictEqual(cfg, {
      enabled: true,
      mode: 'session_bundle',
      author: 'inherit',
    });
  });

  it('only treats author="claude" as non-inherit', () => {
    const cfg = resolveOuterVcsConfig({
      plugin: { claude_code: { outer_vcs: { enabled: true, author: 'claude' } } },
    });
    assert.strictEqual(cfg.author, 'claude');

    const cfgWeird = resolveOuterVcsConfig({
      plugin: { claude_code: { outer_vcs: { enabled: true, author: 'somebody-else' } } },
    });
    assert.strictEqual(cfgWeird.author, 'inherit');
  });

  it('non-boolean enabled resolves to false', () => {
    const cfg = resolveOuterVcsConfig({
      plugin: { claude_code: { outer_vcs: { enabled: 'yes' } } },
    });
    assert.strictEqual(cfg.enabled, false);
  });
});

describe('extractMemLayouts', () => {
  it('returns writable mems with vcs subobject only', () => {
    const response = {
      writable_mems: ['a', 'b', 'c'],
      mems: [
        { name: 'a', vcs: { gitdir: '/tmp/a/.git', worktree: '/tmp/a' } },
        { name: 'b' }, // no vcs
        { name: 'c', vcs: { gitdir: '/tmp/c/.git', worktree: '/tmp/c' } },
        { name: 'ro', vcs: { gitdir: '/tmp/ro/.git', worktree: '/tmp/ro' } }, // read-only
      ],
    };
    const layouts = extractMemLayouts(response);
    assert.deepStrictEqual(
      layouts.map((v) => v.name),
      ['a', 'c'],
    );
  });

  it('returns empty list when mems field is missing', () => {
    assert.deepStrictEqual(extractMemLayouts({}), []);
    assert.deepStrictEqual(extractMemLayouts({ mems: null }), []);
  });
});

describe('classifyMemNotes', () => {
  it('bins agent/external actors into separate buckets', () => {
    const notes = [
      {
        sha: 'sha-foo',
        subject: 'memstead: create engine--foo',
        tool_verb: 'create',
        entity_id: 'engine--foo',
        note: 'added foo invariant',
        actor: 'agent',
        tool: 'memstead_create',
        client: 'claude-code@2.1.0',
        timestamp: 0,
      },
      {
        sha: 'sha-ext',
        subject: 'memstead: external edits (2 files)',
        tool_verb: 'external',
        entity_id: 'edits',
        note: '',
        actor: 'external',
        tool: 'memstead_external',
        client: '',
        timestamp: 0,
      },
    ];
    const { agentNotes, externalNotes } = classifyMemNotes({
      memName: 'engine',
      notes,
      logger: { error: () => {} },
    });
    assert.strictEqual(agentNotes.length, 1);
    assert.strictEqual(agentNotes[0].note, 'added foo invariant');
    assert.strictEqual(agentNotes[0].mem, 'engine');
    assert.strictEqual(externalNotes.length, 1);
    assert.strictEqual(externalNotes[0].mem, 'engine');
    assert.strictEqual(externalNotes[0].summary, 'external edits (2 files)');
  });

  it('skips notes with no recognized actor (stderr warning)', () => {
    const errors = [];
    const { agentNotes, externalNotes } = classifyMemNotes({
      memName: 'engine',
      notes: [
        {
          sha: 'sha-orphan',
          subject: 'memstead: update engine--orphan',
          actor: null,
          note: 'missing actor',
        },
      ],
      logger: { error: (msg) => errors.push(msg) },
    });
    assert.strictEqual(agentNotes.length, 0);
    assert.strictEqual(externalNotes.length, 0);
    assert.match(errors[0], /no recognized Actor trailer/);
  });

  it('returns empty buckets for empty input', () => {
    const r = classifyMemNotes({
      memName: 'engine',
      notes: [],
      logger: { error: () => {} },
    });
    assert.deepStrictEqual(r, { agentNotes: [], externalNotes: [] });
  });

  it('tolerates non-array notes input', () => {
    const r = classifyMemNotes({ memName: 'engine', notes: undefined });
    assert.deepStrictEqual(r, { agentNotes: [], externalNotes: [] });
  });
});

describe('memsteadRefToArray', () => {
  it('wraps a hex sha as a single __MEMSTEAD entry', () => {
    const arr = memsteadRefToArray('aabbccdd');
    assert.deepStrictEqual(arr, [{ name: '__MEMSTEAD', sha: 'aabbccdd' }]);
  });

  it('returns empty array for non-hex sha values', () => {
    assert.deepStrictEqual(memsteadRefToArray('not-a-sha'), []);
    assert.deepStrictEqual(memsteadRefToArray(''), []);
  });

  it('returns empty array for missing or non-string input', () => {
    assert.deepStrictEqual(memsteadRefToArray(null), []);
    assert.deepStrictEqual(memsteadRefToArray(undefined), []);
    assert.deepStrictEqual(memsteadRefToArray({}), []);
  });
});

describe('parseCursorTrailers', () => {
  it('extracts one cursor per line into a Map', () => {
    const body = [
      'memstead: session changes (1 entities, 1 mems)',
      '',
      'body',
      '',
      'Mems: engine',
      'Memstead-cursor: engine@a1b2c3d4',
      'Memstead-cursor: plugin@e5f6a7b8',
    ].join('\n');
    const cursors = parseCursorTrailers(body);
    assert.strictEqual(cursors.size, 2);
    assert.strictEqual(cursors.get('engine'), 'a1b2c3d4');
    assert.strictEqual(cursors.get('plugin'), 'e5f6a7b8');
  });

  it('returns empty map when no trailers', () => {
    const body = 'memstead: session changes (1 entities, 1 mems)\n\nbody\n\nMems: engine';
    assert.strictEqual(parseCursorTrailers(body).size, 0);
  });

  it('ignores malformed trailer values', () => {
    const body = [
      'Memstead-cursor: engine@NOTVALIDSHA',
      'Memstead-cursor: engine@deadbeef',
    ].join('\n');
    const cursors = parseCursorTrailers(body);
    // First matches because [0-9a-f]+ catches the lowercase letters, the
    // second overrides it. The important behavior: a valid-shape entry
    // makes it into the map.
    assert.strictEqual(cursors.get('engine'), 'deadbeef');
  });
});

describe('formatCursorTrailers', () => {
  it('emits one trailer per layout in order', () => {
    const layouts = [
      { name: 'engine', gitdir: '/a/.git', worktree: '/a' },
      { name: 'plugin', gitdir: '/b/.git', worktree: '/b' },
    ];
    const heads = new Map([
      ['engine', 'aaaaaaa'],
      ['plugin', 'bbbbbbb'],
    ]);
    const out = formatCursorTrailers(layouts, heads);
    assert.strictEqual(
      out,
      'Memstead-cursor: engine@aaaaaaa\nMemstead-cursor: plugin@bbbbbbb',
    );
  });

  it('fills empty-tree for mems without a head', () => {
    const layouts = [
      { name: 'engine', gitdir: '/a/.git', worktree: '/a' },
      { name: 'fresh', gitdir: '/b/.git', worktree: '/b' },
    ];
    const heads = new Map([['engine', 'aaaaaaa']]);
    const out = formatCursorTrailers(layouts, heads);
    assert.match(out, new RegExp(`Memstead-cursor: fresh@${GIT_EMPTY_TREE_SHA}$`));
  });

  it('appends the registry-ref trailer after the per-mem block', () => {
    const layouts = [{ name: 'engine', gitdir: '/a/.git', worktree: '/a' }];
    const heads = new Map([['engine', 'aaaaaaa']]);
    const refs = [{ name: '__MEMSTEAD', sha: 'sys1234' }];
    const out = formatCursorTrailers(layouts, heads, refs);
    assert.strictEqual(
      out,
      'Memstead-cursor: engine@aaaaaaa\nMemstead-cursor: __MEMSTEAD@sys1234',
    );
  });

  it('round-trips the registry-ref trailer through parseCursorTrailers', () => {
    const layouts = [{ name: 'engine', gitdir: '/a/.git', worktree: '/a' }];
    const heads = new Map([['engine', 'aaaaaaa']]);
    const refs = [{ name: '__MEMSTEAD', sha: 'cafe1234' }];
    const out = formatCursorTrailers(layouts, heads, refs);
    const parsed = parseCursorTrailers(out);
    assert.strictEqual(parsed.get('engine'), 'aaaaaaa');
    assert.strictEqual(parsed.get('__MEMSTEAD'), 'cafe1234');
  });
});

describe('buildOuterCommitMessage', () => {
  const defaultLayouts = [
    { name: 'engine', gitdir: '/a/.git', worktree: '/a' },
  ];
  const defaultHeads = new Map([['engine', 'deadbeef']]);

  it('returns null when both lists are empty', () => {
    assert.strictEqual(
      buildOuterCommitMessage({
        agentNotes: [],
        externalNotes: [],
        memsTouched: [],
        sessionId: 'sess-1',
        layouts: defaultLayouts,
        perMemHeads: defaultHeads,
      }),
      null,
    );
  });

  it('renders Agent notes subsection and Memstead-cursor trailer', () => {
    const msg = buildOuterCommitMessage({
      agentNotes: [
        { mem: 'engine', note: 'added foo invariant', toolVerb: 'create', entityId: 'engine--foo' },
        { mem: 'engine', note: 'clarified bar purpose', toolVerb: 'update', entityId: 'engine--bar' },
      ],
      externalNotes: [],
      memsTouched: ['engine'],
      sessionId: 'sess-42',
      layouts: defaultLayouts,
      perMemHeads: defaultHeads,
    });
    assert.match(msg, /^memstead: session changes \(2 entities, 1 mems\)\n/);
    assert.match(msg, /Agent notes:\n- \[engine\] added foo invariant/);
    assert.match(msg, /\nMems: engine\n/);
    assert.match(msg, /\nSession: sess-42\n/);
    assert.match(msg, /\nMemstead-cursor: engine@deadbeef$/);
  });

  it('omits Session trailer when sessionId is null (skill path)', () => {
    const msg = buildOuterCommitMessage({
      agentNotes: [
        { mem: 'engine', note: 'manual commit', toolVerb: 'update', entityId: 'engine--foo' },
      ],
      externalNotes: [],
      memsTouched: ['engine'],
      sessionId: null,
      layouts: defaultLayouts,
      perMemHeads: defaultHeads,
    });
    assert.doesNotMatch(msg, /\nSession:/);
    assert.match(msg, /\nMemstead-cursor: engine@deadbeef$/);
  });

  it('renders External edits captured section alongside Agent notes', () => {
    const msg = buildOuterCommitMessage({
      agentNotes: [
        { mem: 'engine', note: 'updated doc', toolVerb: 'update', entityId: 'engine--foo' },
      ],
      externalNotes: [
        { mem: 'engine', summary: 'external edits (2 files)' },
      ],
      memsTouched: ['engine'],
      sessionId: 'sess-1',
      layouts: defaultLayouts,
      perMemHeads: defaultHeads,
    });
    assert.match(msg, /Agent notes:\n- \[engine\] updated doc\n/);
    assert.match(msg, /External edits captured:\n- \[engine\] external edits \(2 files\)\n/);
  });

  it('omits Agent notes section when only external edits exist', () => {
    const msg = buildOuterCommitMessage({
      agentNotes: [],
      externalNotes: [
        { mem: 'engine', summary: 'external edits (3 files)' },
      ],
      memsTouched: ['engine'],
      sessionId: 'sess-1',
      layouts: defaultLayouts,
      perMemHeads: defaultHeads,
    });
    assert.doesNotMatch(msg, /Agent notes:/);
    assert.match(msg, /External edits captured:\n- \[engine\] external edits \(3 files\)/);
  });

  it('falls back to mechanical subject for un-noted agent commits', () => {
    const msg = buildOuterCommitMessage({
      agentNotes: [
        { mem: 'plugin', note: '', toolVerb: 'update', entityId: 'plugin--baz' },
      ],
      externalNotes: [],
      memsTouched: ['plugin'],
      sessionId: 'sess-1',
      layouts: [{ name: 'plugin', gitdir: '/b/.git', worktree: '/b' }],
      perMemHeads: new Map([['plugin', 'ffffeee']]),
    });
    assert.match(msg, /- \[plugin\] update plugin--baz/);
  });

  it('emits registry-ref trailers after the per-mem cursors', () => {
    const msg = buildOuterCommitMessage({
      agentNotes: [
        { mem: 'engine', note: 'updated doc', toolVerb: 'update', entityId: 'engine--foo' },
      ],
      externalNotes: [],
      memsTouched: ['engine'],
      sessionId: 'sess-7',
      layouts: defaultLayouts,
      perMemHeads: defaultHeads,
      registryRefs: [{ name: '__MEMSTEAD', sha: 'sys42' }],
    });
    assert.match(msg, /\nMemstead-cursor: engine@deadbeef\n/);
    assert.match(msg, /\nMemstead-cursor: __MEMSTEAD@sys42$/);
  });
});

describe('buildSeedCommitMessage', () => {
  it('lists mems with their heads and emits cursor trailers', () => {
    const layouts = [
      { name: 'engine', gitdir: '/a/.git', worktree: '/a' },
      { name: 'fresh', gitdir: '/b/.git', worktree: '/b' },
    ];
    const heads = new Map([
      ['engine', 'aaa111'],
      ['fresh', GIT_EMPTY_TREE_SHA],
    ]);
    const msg = buildSeedCommitMessage(layouts, heads);
    assert.match(msg, /^memstead: initialize cursor \(2 mems\)\n/);
    assert.match(msg, /- engine @ aaa111\n/);
    assert.match(msg, new RegExp(`- fresh @ ${GIT_EMPTY_TREE_SHA} \\(no commits yet\\)\n`));
    assert.match(msg, /\nMemstead-cursor: engine@aaa111\n/);
    assert.match(msg, new RegExp(`\nMemstead-cursor: fresh@${GIT_EMPTY_TREE_SHA}$`));
    // No Session trailer on seeds
    assert.doesNotMatch(msg, /\nSession:/);
  });

  it('emits the registry-ref body line and trailer when supplied', () => {
    const layouts = [{ name: 'engine', gitdir: '/a/.git', worktree: '/a' }];
    const heads = new Map([['engine', 'aaa111']]);
    const refs = [{ name: '__MEMSTEAD', sha: 'sys111' }];
    const msg = buildSeedCommitMessage(layouts, heads, refs);
    assert.match(msg, /- engine @ aaa111\n/);
    assert.match(msg, /- __MEMSTEAD @ sys111\n/);
    assert.match(msg, /\nMemstead-cursor: engine@aaa111\n/);
    assert.match(msg, /\nMemstead-cursor: __MEMSTEAD@sys111$/);
  });
});

describe('readPriorCursor', () => {
  it('returns null when git log fails (fresh outer repo)', () => {
    const fakeGit = () => ({ status: 128, stdout: '', stderr: 'fatal: ambiguous argument' });
    const r = readPriorCursor({ workspaceRoot: '/tmp', git: fakeGit });
    assert.strictEqual(r, null);
  });

  it('returns null when no commit carries Memstead-cursor trailers', () => {
    const fakeGit = () => ({
      status: 0,
      stdout:
        'abc123\nmemstead: session changes (handcrafted)\n\nno trailers\n--EOC--\n',
      stderr: '',
    });
    const r = readPriorCursor({ workspaceRoot: '/tmp', git: fakeGit, logger: { error: () => {} } });
    assert.strictEqual(r, null);
  });

  it('parses the first trailer-bearing commit and reports source', () => {
    const fakeGit = () => ({
      status: 0,
      stdout: [
        'sha_newer',
        'memstead: session changes (1 entities, 1 mems)',
        '',
        'Agent notes:',
        '- [engine] something',
        '',
        'Mems: engine',
        'Session: sess-1',
        'Memstead-cursor: engine@feedface',
        '--EOC--',
        'sha_older',
        'memstead: initialize cursor (1 mems)',
        '',
        'Seeded cursors at current per-mem HEAD for:',
        '- engine @ abc000',
        '',
        'Memstead-cursor: engine@abc000',
        '--EOC--',
        '',
      ].join('\n'),
      stderr: '',
    });
    const r = readPriorCursor({ workspaceRoot: '/tmp', git: fakeGit });
    assert.strictEqual(r.source, 'session-changes');
    assert.strictEqual(r.commitSha, 'sha_newer');
    assert.strictEqual(r.cursors.get('engine'), 'feedface');
  });

  it('skips look-alike commits without trailers and picks the next', () => {
    const fakeGit = () => ({
      status: 0,
      stdout: [
        'sha_lookalike',
        'memstead: session changes (handcrafted)',
        '',
        'no trailers here',
        '--EOC--',
        'sha_real',
        'memstead: session changes (1 entities, 1 mems)',
        '',
        'Mems: engine',
        'Memstead-cursor: engine@deadbee',
        '--EOC--',
        '',
      ].join('\n'),
      stderr: '',
    });
    const r = readPriorCursor({ workspaceRoot: '/tmp', git: fakeGit });
    assert.strictEqual(r.commitSha, 'sha_real');
    assert.strictEqual(r.cursors.get('engine'), 'deadbee');
  });

  it('reports initialize-cursor source for seed commits', () => {
    const fakeGit = () => ({
      status: 0,
      stdout: [
        'sha_seed',
        'memstead: initialize cursor (1 mems)',
        '',
        'Memstead-cursor: engine@aaa000',
        '--EOC--',
        '',
      ].join('\n'),
      stderr: '',
    });
    const r = readPriorCursor({ workspaceRoot: '/tmp', git: fakeGit });
    assert.strictEqual(r.source, 'initialize-cursor');
  });

  it('bails and returns null when walk exceeds 1000 look-alikes', () => {
    // Construct 1001 look-alike blocks — no trailers anywhere.
    const blocks = [];
    for (let i = 0; i < 1001; i++) {
      blocks.push(`sha_${i}`);
      blocks.push(`memstead: session changes (fake ${i})`);
      blocks.push('');
      blocks.push('no trailers');
      blocks.push('--EOC--');
    }
    const fakeGit = () => ({ status: 0, stdout: blocks.join('\n') + '\n', stderr: '' });
    const errors = [];
    const r = readPriorCursor({
      workspaceRoot: '/tmp',
      git: fakeGit,
      logger: { error: (m) => errors.push(m) },
    });
    assert.strictEqual(r, null);
    assert.match(errors[0], /cursor walk exceeded/);
  });
});

describe('produceOuterCommit', () => {
  // These tests mock the MCP client (withEngineFn) and git subprocess
  // calls (git) so the pipeline runs without booting the engine or
  // touching the filesystem.
  const baseLayout = {
    name: 'engine',
    vcs: { gitdir: '/tmp/engine/.git', worktree: '/tmp/engine' },
  };

  function fakeClientFactory(health, changesByMem = {}) {
    return async (_cmd, _timeout, fn) => {
      const client = {
        async callTool(name, args) {
          if (name === 'memstead_health') return health;
          if (name === 'memstead_changes_since') {
            const r = changesByMem[args.mem] ?? { changes: [], head: GIT_EMPTY_TREE_SHA };
            return r;
          }
          return null;
        },
      };
      return fn(client);
    };
  }

  it('returns disabled when outer_vcs.enabled is false and skipEnabledCheck=false', async () => {
    const health = {
      writable_mems: ['engine'],
      mems: [baseLayout],
      plugin: { claude_code: { outer_vcs: { enabled: false } } },
    };
    const r = await produceOuterCommit({
      engineCommand: { cmd: 'memstead', args: [], cwd: '/tmp/ws' },
      workspaceRoot: '/tmp/ws',
      sessionId: 'sess-1',
      skipEnabledCheck: false,
      withEngineFn: fakeClientFactory(health),
      git: () => ({ status: 0, stdout: '', stderr: '' }),
      logger: { error: () => {} },
    });
    assert.strictEqual(r.status, 'disabled');
  });

  it('skill path (skipEnabledCheck=true) ignores enabled=false', async () => {
    const health = {
      writable_mems: ['engine'],
      mems: [baseLayout],
      plugin: { claude_code: { outer_vcs: { enabled: false } } },
    };
    let probed = false;
    const gitFake = (args) => {
      probed = true;
      if (args[0] === 'log') return { status: 128, stdout: '', stderr: 'fatal' };
      return { status: 0, stdout: '', stderr: '' };
    };
    const r = await produceOuterCommit({
      engineCommand: { cmd: 'memstead', args: [], cwd: '/tmp/ws' },
      workspaceRoot: '/tmp/ws',
      sessionId: null,
      skipEnabledCheck: true,
      withEngineFn: fakeClientFactory(health),
      git: gitFake,
      logger: { error: () => {} },
    });
    assert.notStrictEqual(r.status, 'disabled');
    assert.ok(probed);
  });

  it('returns no-mems when writable_mems is empty', async () => {
    const health = {
      writable_mems: [],
      mems: [],
      plugin: { claude_code: { outer_vcs: { enabled: true } } },
    };
    const r = await produceOuterCommit({
      engineCommand: { cmd: 'memstead', args: [], cwd: '/tmp/ws' },
      workspaceRoot: '/tmp/ws',
      sessionId: 'sess-1',
      skipEnabledCheck: false,
      withEngineFn: fakeClientFactory(health),
      git: () => ({ status: 0, stdout: '', stderr: '' }),
      logger: { error: () => {} },
    });
    assert.strictEqual(r.status, 'no-mems');
  });

  it('returns probe-failed when memstead_health throws', async () => {
    const r = await produceOuterCommit({
      engineCommand: { cmd: 'memstead', args: [], cwd: '/tmp/ws' },
      workspaceRoot: '/tmp/ws',
      sessionId: 'sess-1',
      skipEnabledCheck: false,
      withEngineFn: async () => {
        throw new Error('engine crashed');
      },
      git: () => ({ status: 0, stdout: '', stderr: '' }),
      logger: { error: () => {} },
    });
    assert.strictEqual(r.status, 'probe-failed');
    assert.match(r.message, /engine crashed/);
  });

  it('logs bootstrap-log-line when a writable mem is new to the cursor', async () => {
    // Prior cursor references `engine`; writable mems also include
    // `plugin` which is new to the cursor. Expect a stderr log line.
    const health = {
      writable_mems: ['engine', 'plugin'],
      mems: [
        { name: 'engine', vcs: { gitdir: '/tmp/engine/.git', worktree: '/tmp/engine' } },
        { name: 'plugin', vcs: { gitdir: '/tmp/plugin/.git', worktree: '/tmp/plugin' } },
      ],
      plugin: { claude_code: { outer_vcs: { enabled: true } } },
    };
    let logCall = 0;
    const gitFake = (args) => {
      logCall += 1;
      if (args[0] === 'log' && args.includes('HEAD')) {
        // outer-repo cursor-walk output
        return {
          status: 0,
          stdout: [
            'sha_prior',
            'memstead: session changes (1 entities, 1 mems)',
            '',
            'Mems: engine',
            'Memstead-cursor: engine@deadbee',
            '--EOC--',
            '',
          ].join('\n'),
          stderr: '',
        };
      }
      if (args.includes('cat-file')) return { status: 0, stdout: '', stderr: '' };
      return { status: 0, stdout: '', stderr: '' };
    };
    const errors = [];
    const r = await produceOuterCommit({
      engineCommand: { cmd: 'memstead', args: [], cwd: '/tmp/ws' },
      workspaceRoot: '/tmp/ws',
      sessionId: 'sess-1',
      skipEnabledCheck: false,
      withEngineFn: fakeClientFactory(health, {
        engine: { changes: [], head: 'deadbee' },
        plugin: { changes: [], head: GIT_EMPTY_TREE_SHA },
      }),
      git: gitFake,
      logger: { error: (m) => errors.push(m) },
    });
    // Either no-changes (no mutations) or commit-failed (no real git)
    // — what we care about is the stderr signal.
    assert.ok(errors.some((e) => /mem 'plugin' is new to the cursor/.test(e)));
    assert.ok(logCall > 0);
    assert.notStrictEqual(r.status, 'disabled');
  });

  it('returns no-changes when no mem had changes', async () => {
    const health = {
      writable_mems: ['engine'],
      mems: [baseLayout],
      plugin: { claude_code: { outer_vcs: { enabled: true } } },
    };
    const gitFake = (args) => {
      if (args[0] === 'log' && args.includes('HEAD')) {
        return {
          status: 0,
          stdout: [
            'sha_prior',
            'memstead: session changes (1 entities, 1 mems)',
            '',
            'Mems: engine',
            'Memstead-cursor: engine@deadbee',
            '--EOC--',
            '',
          ].join('\n'),
          stderr: '',
        };
      }
      if (args.includes('cat-file')) return { status: 0, stdout: '', stderr: '' };
      return { status: 0, stdout: '', stderr: '' };
    };
    const r = await produceOuterCommit({
      engineCommand: { cmd: 'memstead', args: [], cwd: '/tmp/ws' },
      workspaceRoot: '/tmp/ws',
      sessionId: 'sess-1',
      skipEnabledCheck: false,
      withEngineFn: fakeClientFactory(health, {
        engine: { changes: [], head: 'deadbee' },
      }),
      git: gitFake,
      logger: { error: () => {} },
    });
    assert.strictEqual(r.status, 'no-changes');
  });
});
