#!/usr/bin/env node
//
// Guard: the plugin roster's SKILL.md prose stays disciplined.
//
// A roster skill's prose is a job map for the user, not a place to
// regrow into a manual, narrate engine mechanics the engine's own tool
// text already teaches, or leak retired vocabulary. Review goodwill
// catches this once; a lint keeps it caught. Sibling in spirit to
// check-architecture.sh (its own banner, its own failure line, its own
// row in dev/handbook/testing.md) — deliberately a SEPARATE named leg,
// not a glob buried inside the plugin node-test leg, so its absence can
// never go invisible.
//
// Four rules (scope per rule, exclusions honoured exactly):
//
//   (1) Router line cap — a thin router (ingest, commit) shells out to a
//       script; its SKILL.md body stays under ROUTER_BODY_MAX lines. The
//       cap is defined once here.
//
//   (2) Mechanism terms — `_hash`-suffixed names, `dry_run`, "envelope"
//       are banned in roster SKILL.md prose. The engine's MCP/CLI text
//       teaches these to the caller; a skill that re-narrates them adds
//       noise at best and tempts a forbidden "I'll edit that" path at
//       worst. Scope: the six non-exempt roster SKILL.md files. NOT the
//       plugin CLAUDE.md / README — those legitimately teach the agent
//       to react to `_hash` / HASH_MISMATCH (drift guidance), which is
//       action the agent takes, not mechanism narration in a skill.
//
//   (3) Retired vocabulary — the unit noun retired by the 2026-07
//       unit-noun rename ("vault" → mem) and the retired store-layout
//       dir (`ingests/`, collapsed into the binding). The LIVE skill
//       verb "ingest" / "ingesting" is not retired; only the plural
//       store-dir form is. Scope: the six roster SKILL.md files + the
//       plugin README + the plugin CLAUDE.md + public/examples/.
//
//   (4) Description medium-noun rule — a roster skill's `description:`
//       must not presume the source medium is code / a repo / files.
//       An allowlist carries the non-medium senses: "commit" is the
//       graph-commit verb in /commit's own description; "files" as a
//       verb is pre-seeded for the verify router that joins later.
//       Scope: the `description:` frontmatter of the six roster skills.
//
// Exempt from ALL rules: reconcile, audit — frozen interim survivors
// scheduled for removal; linting their prose now would enforce
// discipline on skills already on their way out. Excluded from ALL rules:
// dev/handbook/ (operator digest), serve/ (`graph_projection`),
// registry/ (manifest projection) — noun collisions with the binding
// format, not drift; those live outside every path this guard walks.
//
// Run locally: `plugins/claude-code/scripts/check-skill-prose.mjs`. Also
// invoked as its own named leg from the workspace `run-tests.sh`. Exit
// 0 = clean, 1 = at least one violation (printed), 2 = wiring error.

import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

// ── Policy (single source of truth) ─────────────────────────────────

export const ROUTER_BODY_MAX = 60;
export const THIN_ROUTERS = ['ingest', 'commit', 'sync', 'verify'];

// The roster skills subject to rules (1), (2) and (4). reconcile + audit
// are the frozen interim survivors — exempt everywhere.
export const ROSTER = ['setup', 'interview', 'learn', 'ingest', 'tidy', 'commit', 'verify'];
export const EXEMPT = ['reconcile', 'audit'];

const MECHANISM_TERMS = [
  { re: /\b\w*_hash\b/i, label: '`_hash`-suffixed mechanism name' },
  { re: /\bdry_run\b/i, label: '`dry_run`' },
  { re: /\benvelope\b/i, label: '"envelope"' },
];

const RETIRED_VOCAB = [
  { re: /\bvaults?\b/i, label: 'retired unit noun "vault" — the unit is a mem' },
  { re: /\.memstead\/ingests\b/i, label: 'retired store path `.memstead/ingests`' },
  { re: /(^|[^a-z])ingests\//i, label: 'retired store-layout dir `ingests/` — collapsed into the binding' },
];

const MEDIUM_NOUNS = [
  { re: /\bcode\b/i, term: 'code' },
  { re: /\brepos?\b|\brepositor(y|ies)\b/i, term: 'repo' },
  { re: /\bfiles?\b/i, term: 'file' },
  { re: /\bcommits?\b/i, term: 'commit' },
];

// (skill, term) pairs whose medium-noun sense is legitimate. Keyed
// "<skill>:<term>". commit → the graph-commit verb in /commit's own
// description; verify → "files" as a verb (pre-seeded so that router's
// draft lands lint-clean when it joins the roster).
const MEDIUM_ALLOW = new Set(['commit:commit', 'verify:file']);

// ── Parsing ─────────────────────────────────────────────────────────

// Split a SKILL.md into frontmatter, body, and the parsed description.
export function parseSkill(name, text) {
  const m = text.match(/^---\n([\s\S]*?)\n---\n?([\s\S]*)$/);
  const frontmatter = m ? m[1] : '';
  const body = m ? m[2] : text;
  return { name, frontmatter, body, description: extractDescription(frontmatter) };
}

// Extract the `description:` value, resolving YAML block scalars
// (`description: >` / `|`) by joining their indented continuation lines.
export function extractDescription(frontmatter) {
  const lines = frontmatter.split('\n');
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(/^description:\s*(.*)$/);
    if (!m) continue;
    const val = m[1].trim();
    if (/^[>|][+-]?$/.test(val)) {
      const parts = [];
      for (let j = i + 1; j < lines.length; j++) {
        if (/^\s/.test(lines[j])) parts.push(lines[j].trim());
        else break;
      }
      return parts.join(' ').trim();
    }
    return val;
  }
  return '';
}

// Body line count, ignoring one trailing empty line from the final newline.
export function bodyLineCount(body) {
  const lines = body.split('\n');
  if (lines.length && lines[lines.length - 1] === '') lines.pop();
  return lines.length;
}

// ── Rules ───────────────────────────────────────────────────────────

export function checkRouterCap(skill) {
  if (!THIN_ROUTERS.includes(skill.name)) return [];
  const n = bodyLineCount(skill.body);
  if (n <= ROUTER_BODY_MAX) return [];
  return [`${skill.name}: thin-router body is ${n} lines (cap ${ROUTER_BODY_MAX}) — a router shells out, it does not grow into a manual`];
}

export function checkMechanismTerms(name, text) {
  const out = [];
  for (const { re, label } of MECHANISM_TERMS) {
    if (re.test(text)) out.push(`${name}: mechanism term ${label} in roster prose — the engine's own tool text teaches this; a skill must not narrate it`);
  }
  return out;
}

export function checkRetiredVocab(name, text) {
  const out = [];
  for (const { re, label } of RETIRED_VOCAB) {
    if (re.test(text)) out.push(`${name}: ${label}`);
  }
  return out;
}

export function checkDescriptionMediumNouns(skill) {
  const out = [];
  for (const { re, term } of MEDIUM_NOUNS) {
    if (re.test(skill.description) && !MEDIUM_ALLOW.has(`${skill.name}:${term}`)) {
      out.push(`${skill.name}: description presumes a source medium ("${term}") — descriptions stay medium-neutral (allowlist non-medium senses in MEDIUM_ALLOW)`);
    }
  }
  return out;
}

// ── Surface walk ────────────────────────────────────────────────────

function textFilesUnder(dir) {
  const out = [];
  if (!existsSync(dir)) return out;
  for (const entry of readdirSync(dir)) {
    const p = join(dir, entry);
    const st = statSync(p);
    if (st.isDirectory()) out.push(...textFilesUnder(p));
    else if (/\.(md|ya?ml|json|txt)$/i.test(entry)) out.push(p);
  }
  return out;
}

// Collect every violation across all rules and surfaces. Pure over the
// filesystem inputs so the test can exercise the rules directly.
export function lint(pluginRoot, examplesDir) {
  const violations = [];
  const skillsDir = join(pluginRoot, 'skills');

  for (const name of ROSTER) {
    const path = join(skillsDir, name, 'SKILL.md');
    if (!existsSync(path)) {
      violations.push(`${name}: expected roster SKILL.md at ${path} — roster/scope drift`);
      continue;
    }
    const text = readFileSync(path, 'utf8');
    const skill = parseSkill(name, text);
    violations.push(...checkRouterCap(skill));          // rule 1
    violations.push(...checkMechanismTerms(name, text)); // rule 2
    violations.push(...checkRetiredVocab(name, text));   // rule 3 (roster leg)
    violations.push(...checkDescriptionMediumNouns(skill)); // rule 4
  }

  // rule 3 also walks the plugin README + CLAUDE.md + public/examples/.
  const retiredScope = [join(pluginRoot, 'README.md'), join(pluginRoot, 'CLAUDE.md')];
  for (const path of retiredScope) {
    if (existsSync(path)) violations.push(...checkRetiredVocab(relLabel(path), readFileSync(path, 'utf8')));
  }
  for (const path of textFilesUnder(examplesDir)) {
    violations.push(...checkRetiredVocab(relLabel(path), readFileSync(path, 'utf8')));
  }

  return violations;
}

function relLabel(path) {
  const i = path.indexOf('/public/');
  return i >= 0 ? path.slice(i + 1) : path;
}

// ── CLI ─────────────────────────────────────────────────────────────

function main() {
  const scriptDir = dirname(fileURLToPath(import.meta.url));
  const pluginRoot = join(scriptDir, '..');
  const examplesDir = join(pluginRoot, '..', '..', 'examples');

  if (!existsSync(join(pluginRoot, 'skills'))) {
    console.error(`check-skill-prose: could not locate plugin skills/ from ${scriptDir}`);
    process.exit(2);
  }

  const violations = lint(pluginRoot, examplesDir);
  if (violations.length === 0) {
    console.log('check-skill-prose: OK — roster prose disciplined (line caps, no mechanism terms, no retired vocabulary, medium-neutral descriptions).');
    process.exit(0);
  }
  console.error('check-skill-prose: roster prose discipline violated:');
  for (const v of violations) console.error(`    ${v}`);
  process.exit(1);
}

// Run only as a CLI, not when imported by the test.
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) main();
