/**
 * inject.mjs — thin client for `/memstead:ingest`.
 *
 * The ingest orchestration now lives in the engine: selection (round-robin),
 * backoff, change-detection (git/graph/mtime + source cursor), and the whole
 * run-brief assembly are `memstead ingest brief` / `memstead ingest brief
 * --all`. This script only **routes the operator arguments** to the engine and
 * emits its output as the agent prompt — no selection, backoff,
 * change-detection, or brief-assembly logic of its own.
 *
 * Operator arguments (unchanged contract):
 *   <ingest-name>        render that ingest's run-brief
 *   --all | (no args)    select the next due ingest (round-robin + backoff)
 *                        and render its brief
 *   --clear <name>       delete the ingest's paired process mem (per-ingest,
 *                        idempotent)
 *
 * The full pre-rebuild orchestration is preserved in the `old-ingest` skill as
 * a fallback.
 */

import { spawnSync } from 'node:child_process';

const MEMSTEAD_BIN = process.env.MEMSTEAD_BIN || 'memstead';
const PREFIX = 'ingest';

const args = process.argv.slice(2);
const positional = args.filter((a) => !a.startsWith('--'));
const clearMode = args.includes('--clear');
const allMode = args.includes('--all') || (!clearMode && positional.length === 0);
const name = positional.join(' ').trim();

function memstead(argList) {
  return spawnSync(MEMSTEAD_BIN, argList, { encoding: 'utf-8' });
}

function stderrTail(result, lines = 3) {
  return (result.stderr || '')
    .split('\n')
    .filter(Boolean)
    .slice(-lines)
    .join('\n')
    .trim();
}

// ── --clear: delete the paired process mem (per-ingest, idempotent) ──────────
if (clearMode) {
  if (!name) {
    process.stdout.write(
      `> **[${PREFIX} | clear] Usage: /memstead:ingest --clear <ingest-name>** (per-ingest; no global form).\n`,
    );
    process.exit(0);
  }
  const r = memstead(['mem', 'delete', name, '--note', 'cleared by /memstead:ingest --clear']);
  if (r.status === 0) {
    process.stdout.write(`[${PREFIX} | clear] ingest/${name} — deleted.\n`);
  } else if (/unknown (writable )?mem/i.test(r.stderr || '')) {
    process.stdout.write(`[${PREFIX} | clear] ingest/${name} — already absent.\n`);
  } else {
    process.stdout.write(`> **[${PREFIX} | clear] clear failed: ${stderrTail(r, 2) || '(no detail)'}**\n`);
  }
  process.exit(0);
}

// ── brief: named ingest, or --all round-robin selection ──────────────────────
const briefArgs = allMode ? ['ingest', 'brief', '--all'] : ['ingest', 'brief', name];
const r = memstead(briefArgs);
if (r.stdout) process.stdout.write(r.stdout);
if (r.status !== 0) {
  // Surface the engine's refusal (unknown ingest, unsupported mode, …) so the
  // agent sees a clear message rather than an empty prompt.
  const tail = stderrTail(r);
  if (tail) process.stdout.write(`> **[${PREFIX}] ${tail}**\n`);
}
process.exit(0);
