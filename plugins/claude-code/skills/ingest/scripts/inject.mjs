/**
 * inject.mjs — router for the `/ingest` skill.
 *
 * The build orchestration lives in the engine: selection (round-robin +
 * backoff), change detection, and the whole run-brief assembly are
 * `memstead projection brief` / `... brief --all`. This script only routes the
 * operator arguments to the engine and emits the result as the agent prompt —
 * it carries no selection, backoff, or brief-assembly logic of its own.
 *
 * Modes (operator arguments):
 *   <binding>            render that binding's build brief (id `<mem>/<stem>`)
 *   --all | (no args)    select the next due binding (round-robin + backoff)
 *                        and render its brief; when nothing is set up yet,
 *                        emit the setup ramp instead of a brief
 *   --clear <binding>    delete the binding's paired process mem (idempotent)
 *
 * Engine calls use `--json` so the outcome is a structured envelope the router
 * branches on ({brief} | {skipped} | {no_bindings} | error). Refusals are
 * surfaced to the agent verbatim.
 */

import { spawnSync } from 'node:child_process';

const MEMSTEAD_BIN = process.env.MEMSTEAD_BIN || 'memstead';
const LABEL = 'ingest';

const args = process.argv.slice(2);
const positional = args.filter((a) => !a.startsWith('--'));
const clearMode = args.includes('--clear');
const allMode = args.includes('--all') || (!clearMode && positional.length === 0);
const target = positional.join(' ').trim();

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

// The engine's refusal message, verbatim. Under `--json` an error routes the
// `{code, message, details}` envelope to stdout; fall back to the stderr tail
// (the human error form) when that payload is absent or unparseable.
function refusalMessage(result) {
  try {
    const env = JSON.parse(result.stdout || '');
    if (env && typeof env.message === 'string' && env.message) return env.message;
  } catch {
    /* not JSON — fall through to the stderr tail */
  }
  return stderrTail(result);
}

// ── --clear: delete the binding's paired process mem (idempotent) ────────────
if (clearMode) {
  if (!target) {
    process.stdout.write(
      `> **[${LABEL} | clear] Usage: /ingest --clear <binding>** (per-binding; no global form).\n`,
    );
    process.exit(0);
  }
  const r = memstead(['mem', 'delete', target, '--note', 'cleared by /ingest --clear']);
  if (r.status === 0) {
    process.stdout.write(`[${LABEL} | clear] ${target} — deleted.\n`);
  } else if (/unknown (writable )?mem/i.test(r.stderr || '')) {
    process.stdout.write(`[${LABEL} | clear] ${target} — already absent.\n`);
  } else {
    process.stdout.write(
      `> **[${LABEL} | clear] clear failed: ${stderrTail(r, 2) || '(no detail)'}**\n`,
    );
  }
  process.exit(0);
}

// ── brief: named binding, or --all round-robin selection ─────────────────────
const briefArgs = allMode
  ? ['--json', 'projection', 'brief', '--all']
  : ['--json', 'projection', 'brief', target];
const r = memstead(briefArgs);

if (r.status !== 0) {
  // Unknown binding, unsupported mode, load failure — surface the engine's
  // own message so the agent sees the refusal, not an empty prompt.
  const msg = refusalMessage(r);
  if (msg) process.stdout.write(`> **[${LABEL}] ${msg}**\n`);
  process.exit(0);
}

let outcome;
try {
  outcome = JSON.parse(r.stdout || '{}');
} catch {
  // Should not happen under `--json`; emit whatever came back rather than
  // dropping the agent into an empty prompt.
  if (r.stdout) process.stdout.write(r.stdout);
  process.exit(0);
}

if (outcome.no_bindings) {
  process.stdout.write(setupRamp());
} else if (outcome.skipped) {
  process.stdout.write(
    `> **[${LABEL}] Nothing due right now — every source is waiting out its backoff. Try again later.**\n`,
  );
} else if (typeof outcome.brief === 'string') {
  // The brief IS the agent prompt — emit it verbatim, no added trailing newline.
  process.stdout.write(outcome.brief);
} else {
  process.stdout.write(`> **[${LABEL}] Unexpected empty response from the engine.**\n`);
}
process.exit(0);

// The no-source setup ramp: agent-facing instructions to hold a short, plain
// conversation with the user (three questions, no jargon) and then set things
// up in one non-interactive call. The user is never asked to produce any
// configuration vocabulary — the questions are in everyday language and the
// agent maps the answers onto the engine flags.
function setupRamp() {
  return [
    `> **[${LABEL}] Nothing is set up to build into a mem here yet — let's set that up first.**`,
    ``,
    `Ask the user these three questions, **one at a time**, in plain words. Do not show`,
    `them any settings, flags, or jargon — just talk. Wait for each answer before asking`,
    `the next.`,
    ``,
    `1. **Where should Claude read from?** A folder of code, a folder of documents, a`,
    `   git history, a website or online docs, or another mem you already have.`,
    `2. **What should this mem capture from it?** One sentence, in the user's own words —`,
    `   what this mem is for.`,
    `3. **Which mem should it go into?** The name of a new or existing mem to write into.`,
    ``,
    `Then, without asking anything more, run this once to set it up silently — infer the`,
    `type from answer 1 (a code folder → \`codebase\`, other files → \`filesystem\`, a git`,
    `history → \`git\`, a website → \`web\`, another mem → \`graph\`):`,
    ``,
    `    memstead projection init \\`,
    `      --mem "<answer 3>" \\`,
    `      --source "<answer 1: the path, URL, or mem id>" \\`,
    `      --medium-type <codebase|filesystem|git|graph|web> \\`,
    `      --intent "<answer 2>"`,
    ``,
    `When that succeeds, run \`/ingest\` again to produce the first build batch.`,
    ``,
  ].join('\n');
}
