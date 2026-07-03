// Pure helpers + the shared commit pipeline for the auto-commit Stop
// hook and the `/outer-commit` skill.
//
// Pure-helper section (top) is synchronous, side-effect-free, and
// unit-testable without mocking stdin, child processes, or the
// filesystem. The `produceOuterCommit` function at the bottom is the
// single side-effecting pipeline both the hook and the skill invoke.
//
// Per-mem data — the workspace `__MEMSTEAD` ref tip, agent-notes per
// commit, current per-mem HEADs — comes from `memstead_changes_since(...,
// include_notes: true)` (which folds `notes[]` and `memstead_ref` into the
// response) and `memstead_health { include_config: true }` (per-mem
// `vcs.head`).
// The plugin no longer reads `mem-repo/.git/` directly. Outer-repo
// `git add` / `git commit` / `git log` against the user's project repo
// (the workspace root) stay — that's the user's git, not Memstead-managed.

import { spawnSync } from 'node:child_process';
import { resolve as resolvePath } from 'node:path';
import { resolveEngineCommand, withEngine } from './mcp-client.mjs';

const MCP_TIMEOUT_MS = 15000;
const CLAUDE_AUTHOR_NAME = 'Claude';
const CLAUDE_AUTHOR_EMAIL = 'noreply@anthropic.com';
const CURSOR_WALK_LIMIT = 1000;
// READ pattern — matches the `memstead:` cursor-commit subjects.
const CURSOR_SUBJECT_GREP =
  '^memstead: session changes\\|^memstead: initialize cursor';
const COMMIT_BLOCK_MARKER = '--EOC--';

/**
 * The canonical git empty-tree hash. Used as the sentinel cursor value
 * for "this mem has no commits yet" and as a fallback when a recorded
 * cursor is no longer reachable in its mem gitdir.
 */
export const GIT_EMPTY_TREE_SHA = '4b825dc642cb6eb9a060e54bf8d69288fbee4904';

// Engine error codes the pipeline branches on. Stable strings from
// `memstead-mcp`'s error envelope — the engine's own
// `EngineError -> { code }` mapping.
const ENGINE_OBJECT_NOT_FOUND_CODES = new Set(['OBJECT_NOT_FOUND', 'VCS_ERROR']);

// ---------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------

/**
 * Pull `plugin.claude_code.outer_vcs` out of an `memstead_health
 * { include_config: true }` response. Returns the settled config object
 * with defaults filled in, or `null` when the plugin hasn't been
 * configured (silent no-op at the call site).
 *
 * Defaults:
 *   enabled = false  — opt-in to avoid surprising first-run users
 *   mode    = "session_bundle"
 *   author  = "inherit"
 */
export function resolveOuterVcsConfig(healthResponse) {
  const raw = healthResponse?.plugin?.claude_code?.outer_vcs;
  if (!raw || typeof raw !== 'object') return null;
  return {
    enabled: raw.enabled === true,
    mode: typeof raw.mode === 'string' ? raw.mode : 'session_bundle',
    author: raw.author === 'claude' ? 'claude' : 'inherit',
  };
}

/**
 * Given an `memstead_health { include_config: true }` response, return the
 * `[{ name, gitdir, worktree, head }]` list for every writable mem
 * that advertises a `vcs` subobject. `head` is the cached branch-tip
 * SHA; `null` for fresh mems with no commits yet (consumers
 * substitute the empty-tree sentinel). Read-only mems and mems
 * whose gitdir couldn't be resolved are silently dropped — the hook
 * ships the outer-repo commit with the coverage it could actually
 * reach.
 */
export function extractMemLayouts(healthResponse) {
  const mems = Array.isArray(healthResponse?.mems) ? healthResponse.mems : [];
  const writable = new Set(
    Array.isArray(healthResponse?.writable_mems) ? healthResponse.writable_mems : [],
  );
  const out = [];
  for (const v of mems) {
    if (!v || typeof v !== 'object') continue;
    if (typeof v.name !== 'string') continue;
    if (!writable.has(v.name)) continue;
    const vcs = v.vcs;
    if (!vcs || typeof vcs.gitdir !== 'string' || typeof vcs.worktree !== 'string') continue;
    const head = typeof vcs.head === 'string' && vcs.head.length > 0 ? vcs.head : null;
    out.push({ name: v.name, gitdir: vcs.gitdir, worktree: vcs.worktree, head });
  }
  return out;
}

/**
 * Translate the engine's `memstead_ref` string (SHA of the unified
 * `__MEMSTEAD` ref — schemas + per-mem configs) into the `[{name, sha}]`
 * shape the cursor-trailer formatter expects. Returns a single-element
 * array (one registry-class cursor) when the SHA is present, empty
 * otherwise — pre-migration workspaces leave the field absent.
 */
export function memsteadRefToArray(memsteadRef) {
  if (typeof memsteadRef !== 'string' || !/^[0-9a-f]+$/i.test(memsteadRef)) return [];
  return [{ name: '__MEMSTEAD', sha: memsteadRef }];
}

/**
 * Classify the `notes[]` ride-along returned by `memstead_changes_since(
 * include_notes: true)` into agent-authored bullets and externally-
 * captured drift bullets. Mirrors the prior plugin's
 * `parseAgentNotesFromCommitBodies` semantics, except the engine has
 * already done the trailer parsing — actor / tool_verb / entity_id /
 * note are pre-decoded fields on each entry.
 *
 * Commits with no recognised `Actor:` trailer are skipped with a
 * stderr warning; the cursor still advances past them on the caller's
 * side.
 */
export function classifyMemNotes({ memName, notes, logger = console }) {
  const agentNotes = [];
  const externalNotes = [];
  if (!Array.isArray(notes)) return { agentNotes, externalNotes };
  for (const n of notes) {
    if (!n || typeof n !== 'object') continue;
    const actor = typeof n.actor === 'string' ? n.actor.toLowerCase() : null;
    if (actor === 'agent') {
      agentNotes.push({
        mem: memName,
        note: typeof n.note === 'string' ? n.note : '',
        toolVerb: typeof n.tool_verb === 'string' ? n.tool_verb : '',
        entityId: typeof n.entity_id === 'string' ? n.entity_id : '',
      });
    } else if (actor === 'external') {
      const subject = typeof n.subject === 'string' ? n.subject : '';
      externalNotes.push({
        mem: memName,
        // Strips the `memstead:` prefix.
        summary: subject.replace(/^memstead:\s+/, ''),
      });
    } else {
      logger.error?.(
        `auto-commit: skipping commit in '${memName}' with no recognized Actor trailer: ${n.subject ?? '(no subject)'}`,
      );
    }
  }
  return { agentNotes, externalNotes };
}

/**
 * Build one `Memstead-cursor:` trailer line per writable mem, in the
 * order the layouts are provided. `perMemHeads` is a Map keyed by mem
 * name whose values are SHAs (or the empty-tree SHA for mems with no
 * commits yet).
 *
 * `registryRefs` (default `[]`) appends `Memstead-cursor: __MEMSTEAD@<sha>`
 * lines after the per-mem block — registry-state pointers so
 * outer-repo replays don't lose track of workspace-level
 * configs/schemas. Order: mems first, registry refs last, in the
 * order the caller provided them.
 */
export function formatCursorTrailers(layouts, perMemHeads, registryRefs = []) {
  const lines = [];
  for (const layout of layouts) {
    const sha = perMemHeads.get(layout.name) ?? GIT_EMPTY_TREE_SHA;
    lines.push(`Memstead-cursor: ${layout.name}@${sha}`);
  }
  for (const ref of registryRefs) {
    if (!ref || typeof ref.name !== 'string' || typeof ref.sha !== 'string') continue;
    lines.push(`Memstead-cursor: ${ref.name}@${ref.sha}`);
  }
  return lines.join('\n');
}

/**
 * Parse the `Memstead-cursor:` trailers out of a commit body into a Map
 * of `memName -> sha`. Trailers not matching the `<name>@<sha>` shape
 * are skipped. Returns an empty map when no trailers are present.
 */
export function parseCursorTrailers(body) {
  const out = new Map();
  if (!body) return out;
  for (const line of body.split(/\r?\n/)) {
    const m = line.match(/^Memstead-cursor:\s*(\S+)@([0-9a-f]+)\s*$/i);
    if (!m) continue;
    out.set(m[1], m[2].toLowerCase());
  }
  return out;
}

/**
 * Aggregate per-mem agent + external notes into a single outer-repo
 * commit message. Output shape:
 *
 *   memstead: session changes (<N entities>, <M mems>)
 *
 *   Agent notes:
 *   - [<mem>] <note or fallback>
 *
 *   External edits captured:
 *   - [<mem>] <summary>
 *
 *   Mems: mem-a, mem-b
 *   Session: <sessionId>   (omitted when sessionId is null)
 *   Memstead-cursor: mem-a@<sha>
 *   Memstead-cursor: mem-b@<sha>
 *
 * Either subsection is omitted when empty. Returns `null` when both
 * lists are empty (caller skips the commit).
 */
export function buildOuterCommitMessage({
  agentNotes,
  externalNotes,
  memsTouched,
  sessionId,
  layouts,
  perMemHeads,
  registryRefs = [],
}) {
  if (agentNotes.length === 0 && externalNotes.length === 0) return null;

  const entityIds = new Set();
  for (const n of agentNotes) entityIds.add(n.entityId);
  for (const n of externalNotes) entityIds.add(`${n.mem}--external`);

  const subject = `memstead: session changes (${entityIds.size} entities, ${memsTouched.length} mems)`;

  const bodyParts = [];
  if (agentNotes.length > 0) {
    bodyParts.push('Agent notes:');
    for (const n of agentNotes) {
      const firstLine = n.note ? n.note.split(/\r?\n/)[0] : `${n.toolVerb} ${n.entityId}`;
      bodyParts.push(`- [${n.mem}] ${firstLine}`);
      if (n.note) {
        const rest = n.note.split(/\r?\n/).slice(1);
        for (const line of rest) bodyParts.push(`  ${line}`);
      }
    }
  }
  if (externalNotes.length > 0) {
    if (bodyParts.length > 0) bodyParts.push('');
    bodyParts.push('External edits captured:');
    for (const n of externalNotes) {
      bodyParts.push(`- [${n.mem}] ${n.summary}`);
    }
  }

  const trailers = [];
  trailers.push('');
  trailers.push(`Mems: ${memsTouched.join(', ')}`);
  if (sessionId) trailers.push(`Session: ${sessionId}`);
  trailers.push(formatCursorTrailers(layouts, perMemHeads, registryRefs));

  return [subject, '', ...bodyParts, ...trailers].join('\n');
}

/**
 * Build the seed-commit message for first-run bootstrap. Subject is
 * `memstead: initialize cursor (N mems)`; body lists each writable
 * mem and its current HEAD; `Memstead-cursor:` trailers point at those
 * HEADs (or the empty-tree SHA for mems with no commits yet). No
 * `Session:` trailer — seeds have no associated agent session.
 */
export function buildSeedCommitMessage(layouts, perMemHeads, registryRefs = []) {
  const subject = `memstead: initialize cursor (${layouts.length} mems)`;
  const bodyLines = ['Seeded cursors at current per-mem HEAD for:'];
  for (const l of layouts) {
    const sha = perMemHeads.get(l.name) ?? GIT_EMPTY_TREE_SHA;
    const note = sha === GIT_EMPTY_TREE_SHA ? ' (no commits yet)' : '';
    bodyLines.push(`- ${l.name} @ ${sha}${note}`);
  }
  for (const ref of registryRefs) {
    if (!ref || typeof ref.name !== 'string' || typeof ref.sha !== 'string') continue;
    bodyLines.push(`- ${ref.name} @ ${ref.sha}`);
  }
  const trailers = ['', formatCursorTrailers(layouts, perMemHeads, registryRefs)];
  return [subject, '', ...bodyLines, ...trailers].join('\n');
}

// ---------------------------------------------------------------------
// Outer-repo git I/O (operates against the user's project repo at
// `workspaceRoot` — explicitly NOT mem-repo). The carve-out from the
// no-direct-git rule lives here.
// ---------------------------------------------------------------------

/**
 * Read the prior `Memstead-cursor:` trailer block from the outer repo by
 * walking `git log` for the matching subject prefix. Returns
 * `{ commitSha, cursors, source }` or `null` when no trailer-bearing
 * commit is found within the bounded walk (bootstrap path).
 *
 * The walk is bounded at 1000 matches — beyond that the helper logs
 * a stderr warning and returns `null` so the caller falls through to
 * the bootstrap path.
 */
export function readPriorCursor({ workspaceRoot, logger = console, git = runGit } = {}) {
  const result = git(
    [
      'log',
      `--grep=${CURSOR_SUBJECT_GREP}`,
      `--format=%H%n%B%n${COMMIT_BLOCK_MARKER}`,
      'HEAD',
    ],
    { cwd: workspaceRoot },
  );
  if (result.status !== 0) {
    // No commits at all (fresh outer repo) — git log exits 128. Treat as
    // bootstrap.
    return null;
  }
  const blocks = splitCommitBlocks(result.stdout);
  if (blocks.length > CURSOR_WALK_LIMIT) {
    logger.error?.(
      `auto-commit: cursor walk exceeded ${CURSOR_WALK_LIMIT} look-alike commits with no Memstead-cursor trailers — treating as bootstrap`,
    );
    return null;
  }
  for (const block of blocks) {
    const cursors = parseCursorTrailers(block.body);
    if (cursors.size === 0) continue;
    const subject = (block.body.split(/\r?\n/)[0] ?? '').trim();
    const source = subject.startsWith('memstead: initialize cursor')
      ? 'initialize-cursor'
      : 'session-changes';
    return { commitSha: block.sha, cursors, source };
  }
  return null;
}

function splitCommitBlocks(raw) {
  if (!raw) return [];
  const blocks = [];
  const lines = raw.split(/\r?\n/);
  let currentSha = null;
  let currentBody = [];
  let awaitingSha = true;
  for (const line of lines) {
    if (awaitingSha) {
      if (!line.trim()) continue;
      currentSha = line.trim();
      currentBody = [];
      awaitingSha = false;
      continue;
    }
    if (line === COMMIT_BLOCK_MARKER) {
      blocks.push({ sha: currentSha, body: currentBody.join('\n') });
      currentSha = null;
      currentBody = [];
      awaitingSha = true;
      continue;
    }
    currentBody.push(line);
  }
  if (currentSha && currentBody.length > 0) {
    blocks.push({ sha: currentSha, body: currentBody.join('\n') });
  }
  return blocks;
}

function runGit(args, { cwd } = {}) {
  return spawnSync('git', args, {
    cwd,
    encoding: 'utf-8',
  });
}

// ---------------------------------------------------------------------
// Shared commit pipeline invoked by both the Stop hook and the
// `/outer-commit` skill.
// ---------------------------------------------------------------------

/**
 * Inspect an error thrown by an MCP `memstead_changes_since` call and
 * decide whether the per-mem cursor should fall back to the
 * empty-tree sentinel. The engine maps unreachable `since` refs to
 * `OBJECT_NOT_FOUND` (the dominant case) or `VCS_ERROR` (gix-level
 * failures). Anything else is rethrown — those are real probe
 * failures, not cursor staleness.
 */
function isUnreachableSinceError(err) {
  if (!err) return false;
  // The MCP client wraps engine errors as `Error('MCP <method> error: ...')`.
  // The error code is embedded in the message text; the underlying
  // `structuredContent.code` is not surfaced through `withEngine`. Match
  // on the substring to keep the dependency loose.
  const message = String(err?.message ?? err);
  for (const code of ENGINE_OBJECT_NOT_FOUND_CODES) {
    if (message.includes(code)) return true;
  }
  return false;
}

/**
 * Produce one outer-repo commit from the current state of the workspace.
 * Returns `{ status, ... }` where `status` is one of:
 *
 *   - 'disabled'       — hook path only: outer_vcs.enabled is not true
 *   - 'no-mems'      — no writable mems in this workspace
 *   - 'no-changes'     — nothing pending to commit across all mems
 *   - 'committed'      — commit landed ({ sha, memsTouched })
 *   - 'commit-failed'  — git commit returned non-zero ({ stderr })
 *   - 'probe-failed'   — memstead_health or MCP boot errored ({ message })
 *
 * Flags:
 *   skipEnabledCheck — when true, ignore `outer_vcs.enabled`. Used by
 *     the `/outer-commit` skill, which is always a manual invocation.
 *   sessionId — included in the `Session:` trailer when non-null;
 *     omitted when null. Hook passes the Claude Code session id; skill
 *     passes null.
 */
export async function produceOuterCommit({
  engineCommand,
  workspaceRoot,
  sessionId,
  skipEnabledCheck = false,
  logger = console,
  withEngineFn = withEngine,
  git = runGit,
  timeoutMs = MCP_TIMEOUT_MS,
} = {}) {
  let cmdSpec = engineCommand;
  if (!cmdSpec) {
    cmdSpec = resolveEngineCommand(workspaceRoot);
    if (!cmdSpec) return { status: 'disabled' };
  }

  let healthResponse;
  try {
    healthResponse = await withEngineFn(cmdSpec, timeoutMs, async (client) => {
      return client.callTool('memstead_health', { include_config: true });
    });
  } catch (err) {
    return { status: 'probe-failed', message: err?.message ?? String(err) };
  }

  const outerVcs = resolveOuterVcsConfig(healthResponse);
  if (!skipEnabledCheck) {
    if (!outerVcs || !outerVcs.enabled) {
      return { status: 'disabled' };
    }
  }

  const layouts = extractMemLayouts(healthResponse);
  if (layouts.length === 0) {
    return { status: 'no-mems' };
  }

  // 1. Read the prior cursor from the outer repo log.
  const prior = readPriorCursor({ workspaceRoot, logger, git });
  if (prior === null) {
    // Bootstrap path — seed commit anchors the cursor chain. The
    // `__MEMSTEAD` ref lands on the seed too; we fetch it via one no-op
    // `memstead_changes_since(include_notes: true)` call (the empty-tree
    // sentinel surfaces every reachable commit but we only need the
    // ride-along `memstead_ref`).
    let registryRefs = [];
    try {
      registryRefs = await withEngineFn(cmdSpec, timeoutMs, async (client) => {
        const report = await client.callTool('memstead_changes_since', {
          mem: layouts[0].name,
          since: GIT_EMPTY_TREE_SHA,
          include_notes: true,
        });
        return memsteadRefToArray(report?.memstead_ref);
      });
    } catch (err) {
      // If the seed-time registry lookup fails the seed still goes
      // out — the outer commit just lacks the registry cursor line.
      logger.error?.(
        `auto-commit: seed memstead_ref lookup failed: ${err?.message ?? err}`,
      );
    }
    return writeSeedCommit({
      workspaceRoot,
      layouts,
      registryRefs,
      outerVcs: outerVcs ?? { enabled: true, mode: 'session_bundle', author: 'inherit' },
      logger,
      git,
    });
  }

  // 2. For each writable mem, fetch the entity-delta + agent-notes
  //    + workspace-level `__MEMSTEAD` ref in one `memstead_changes_since(
  //    include_notes: true)` call. Mems with no changes still get a
  //    cursor trailer (their HEAD re-stated). Cursor unreachability
  //    falls back to the empty-tree sentinel — the engine reports
  //    `OBJECT_NOT_FOUND` when the prior cursor is no longer in the
  //    mem's history.
  let perMem;
  let registryRefs = [];
  try {
    perMem = await withEngineFn(cmdSpec, timeoutMs, async (client) => {
      const acc = [];
      for (const layout of layouts) {
        const priorSha = prior.cursors.get(layout.name);
        const isNewToCursor = priorSha === undefined;
        if (isNewToCursor) {
          logger.error?.(
            `auto-commit: mem '${layout.name}' is new to the cursor, bundling commits since mem inception`,
          );
        }
        const initialSince = priorSha ?? GIT_EMPTY_TREE_SHA;
        let report;
        try {
          report = await client.callTool('memstead_changes_since', {
            mem: layout.name,
            since: initialSince,
            include_notes: true,
          });
        } catch (err) {
          if (initialSince !== GIT_EMPTY_TREE_SHA && isUnreachableSinceError(err)) {
            logger.error?.(
              `auto-commit: mem '${layout.name}' cursor ${initialSince} unreachable — falling back to empty-tree`,
            );
            report = await client.callTool('memstead_changes_since', {
              mem: layout.name,
              since: GIT_EMPTY_TREE_SHA,
              include_notes: true,
            });
          } else {
            throw err;
          }
        }
        const head =
          typeof report?.head === 'string' && report.head.length > 0
            ? report.head
            : GIT_EMPTY_TREE_SHA;
        const notes = Array.isArray(report?.notes) ? report.notes : [];
        acc.push({ layout, head, notes });
        // Workspace-level `__MEMSTEAD` ref is identical across mems —
        // capture from the first response that carries it.
        if (registryRefs.length === 0 && report?.memstead_ref) {
          registryRefs = memsteadRefToArray(report.memstead_ref);
        }
      }
      return acc;
    });
  } catch (err) {
    return { status: 'probe-failed', message: err?.message ?? String(err) };
  }

  // 3. Classify per-mem notes into agent-authored and external-
  //    captured bullets. Agent-authored bullets become the body of the
  //    outer commit; external bullets are preserved in their own
  //    section. Mems whose notes window is empty contribute nothing.
  const agentNotes = [];
  const externalNotes = [];
  const memsTouched = [];
  const worktreesToStage = [];
  for (const v of perMem) {
    if (v.notes.length === 0) continue;
    const { agentNotes: an, externalNotes: en } = classifyMemNotes({
      memName: v.layout.name,
      notes: v.notes,
      logger,
    });
    if (an.length === 0 && en.length === 0) continue;
    agentNotes.push(...an);
    externalNotes.push(...en);
    memsTouched.push(v.layout.name);
    worktreesToStage.push(v.layout.worktree);
  }

  if (memsTouched.length === 0) {
    return { status: 'no-changes' };
  }

  // 4. Build the per-mem head map for the trailer block — every
  //    writable mem gets a cursor entry, not just the ones touched.
  const perMemHeads = new Map();
  for (const v of perMem) perMemHeads.set(v.layout.name, v.head);

  const commitMessage = buildOuterCommitMessage({
    agentNotes,
    externalNotes,
    memsTouched,
    sessionId,
    layouts,
    perMemHeads,
    registryRefs,
  });
  if (!commitMessage) {
    return { status: 'no-changes' };
  }

  // 5. Stage and commit in the outer repo (operates in the user's
  //    project repo at `workspaceRoot`, not mem-repo).
  const addArgs = ['add', '--'];
  for (const w of worktreesToStage) addArgs.push(w);
  const addResult = git(addArgs, { cwd: workspaceRoot });
  if (addResult.status !== 0) {
    return {
      status: 'commit-failed',
      stderr: addResult.stderr?.trim?.() ?? 'git add failed',
    };
  }

  const commitArgs = [];
  if ((outerVcs?.author ?? 'inherit') === 'claude') {
    commitArgs.push(
      '-c', `user.name=${CLAUDE_AUTHOR_NAME}`,
      '-c', `user.email=${CLAUDE_AUTHOR_EMAIL}`,
    );
  }
  commitArgs.push('commit', '-F', '-');

  const commitResult = spawnSync('git', commitArgs, {
    cwd: workspaceRoot,
    input: commitMessage,
    encoding: 'utf-8',
  });
  if (commitResult.status !== 0) {
    const stderr = commitResult.stderr ?? '';
    if (/nothing to commit|no changes added/i.test(stderr)) {
      return { status: 'no-changes' };
    }
    return { status: 'commit-failed', stderr: stderr.trim() };
  }

  const shaRes = git(['rev-parse', 'HEAD'], { cwd: workspaceRoot });
  const sha = shaRes.status === 0 ? shaRes.stdout.trim() : null;
  return {
    status: 'committed',
    sha,
    memsTouched,
    kind: 'normal',
  };
}

/**
 * Write the first-run seed commit. Distinct subject (`memstead: initialize
 * cursor`) signals "this is where cursor bookkeeping started." Any
 * pending writable-mem files are staged alongside; `--allow-empty` is
 * passed so a fresh workspace with no pending files still lands the
 * seed as an empty-diff commit (carrying only trailers).
 *
 * Per-mem HEADs come from `layouts[i].head` (sourced from
 * `memstead_health.mems[].vcs.head` — see `extractMemLayouts`); fresh
 * mems with no commits yet show as `null` and fall back to the
 * empty-tree sentinel.
 */
export function writeSeedCommit({
  workspaceRoot,
  layouts,
  registryRefs,
  outerVcs,
  logger = console,
  git = runGit,
}) {
  const perMemHeads = new Map();
  for (const layout of layouts) {
    perMemHeads.set(
      layout.name,
      typeof layout.head === 'string' && layout.head.length > 0
        ? layout.head
        : GIT_EMPTY_TREE_SHA,
    );
  }

  const refs = Array.isArray(registryRefs) ? registryRefs : [];
  const message = buildSeedCommitMessage(layouts, perMemHeads, refs);

  // Stage any pending mem files. Missing worktrees (rare; mem root
  // outside the outer repo) produce add errors — proceed with what we
  // can stage.
  for (const layout of layouts) {
    const worktree = resolvePath(workspaceRoot, layout.worktree);
    git(['add', '--', worktree], { cwd: workspaceRoot });
  }

  const commitArgs = [];
  if ((outerVcs?.author ?? 'inherit') === 'claude') {
    commitArgs.push(
      '-c', `user.name=${CLAUDE_AUTHOR_NAME}`,
      '-c', `user.email=${CLAUDE_AUTHOR_EMAIL}`,
    );
  }
  commitArgs.push('commit', '--allow-empty', '-F', '-');

  const commitResult = spawnSync('git', commitArgs, {
    cwd: workspaceRoot,
    input: message,
    encoding: 'utf-8',
  });
  if (commitResult.status !== 0) {
    return {
      status: 'commit-failed',
      stderr: commitResult.stderr?.trim?.() ?? 'seed commit failed',
      kind: 'seed',
    };
  }

  const shaRes = git(['rev-parse', 'HEAD'], { cwd: workspaceRoot });
  const sha = shaRes.status === 0 ? shaRes.stdout.trim() : null;
  logger.error?.(`auto-commit: seed commit landed — ${sha}`);
  return {
    status: 'committed',
    sha,
    memsTouched: layouts.map((l) => l.name),
    kind: 'seed',
  };
}
