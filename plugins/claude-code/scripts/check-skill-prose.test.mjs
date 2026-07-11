// Tests for the roster-prose lint (check-skill-prose.mjs).
//
// A seeded violation of EACH rule class must be caught, and the
// reconcile/audit exemption must hold. These exercise the pure rule
// functions directly (the CLI `lint()` composes them over the real
// filesystem; here we feed synthetic inputs so a green run proves the
// rules bite, not merely that today's surfaces happen to be clean).

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  parseSkill,
  extractDescription,
  bodyLineCount,
  checkRouterCap,
  checkMechanismTerms,
  checkRetiredVocab,
  checkDescriptionMediumNouns,
  ROUTER_BODY_MAX,
  EXEMPT,
} from './check-skill-prose.mjs';

// ── parsing ──────────────────────────────────────────────────────────

test('parseSkill splits frontmatter, body, and a single-line description', () => {
  const text = '---\nname: x\ndescription: A one-line summary.\n---\n\nBody line.\n';
  const s = parseSkill('x', text);
  assert.equal(s.description, 'A one-line summary.');
  assert.match(s.body, /Body line\./);
});

test('extractDescription resolves a YAML block scalar across indented lines', () => {
  const fm = 'name: x\ndescription: >\n  First part\n  second part.\ncontext: fork';
  assert.equal(extractDescription(fm), 'First part second part.');
});

// ── rule 1: router line cap ──────────────────────────────────────────

test('rule 1 flags a thin router whose body exceeds the cap', () => {
  const body = Array.from({ length: ROUTER_BODY_MAX + 5 }, (_, i) => `line ${i}`).join('\n');
  const skill = { name: 'ingest', body, description: '' };
  const out = checkRouterCap(skill);
  assert.equal(out.length, 1);
  assert.match(out[0], /thin-router body is \d+ lines/);
});

test('rule 1 passes a thin router under the cap and ignores non-routers', () => {
  assert.deepEqual(checkRouterCap({ name: 'ingest', body: 'one\ntwo\n', description: '' }), []);
  const big = Array.from({ length: 300 }, (_, i) => `l${i}`).join('\n');
  assert.deepEqual(checkRouterCap({ name: 'setup', body: big, description: '' }), []);
});

test('bodyLineCount ignores the single trailing newline', () => {
  assert.equal(bodyLineCount('a\nb\n'), 2);
  assert.equal(bodyLineCount('a\nb'), 2);
});

// ── rule 2: mechanism terms ──────────────────────────────────────────

for (const [seed, why] of [
  ['Cached expected_hash values are stale.', '_hash-suffixed name'],
  ['Pass dry_run to preview.', 'dry_run'],
  ['Branch on the structured envelope.', 'envelope'],
]) {
  test(`rule 2 flags mechanism term (${why})`, () => {
    const out = checkMechanismTerms('tidy', seed);
    assert.equal(out.length, 1, `expected 1 violation for: ${seed}`);
  });
}

test('rule 2 passes prose free of mechanism terms', () => {
  assert.deepEqual(checkMechanismTerms('tidy', 'Assess structure, propose fixes, apply approved ones.'), []);
});

// ── rule 3: retired vocabulary ───────────────────────────────────────

test('rule 3 flags the retired unit noun "vault"', () => {
  assert.equal(checkRetiredVocab('setup', 'Create the vault in the current dir.').length, 1);
  assert.equal(checkRetiredVocab('examples/x.md', 'a Vault of typed entities').length, 1);
});

test('rule 3 flags the retired store-layout dir but NOT the live ingest verb', () => {
  assert.ok(checkRetiredVocab('ingest', 'reads .memstead/ingests/foo.json').length >= 1);
  assert.equal(checkRetiredVocab('ingest', 'writes to ingests/ under the store').length, 1);
  // "ingest" / "ingesting" as the live skill verb must never trip.
  assert.deepEqual(checkRetiredVocab('ingest', 'Ingest a source; ingesting one batch per run.'), []);
});

// ── rule 4: description medium nouns ─────────────────────────────────

for (const term of ['code', 'repo', 'repository', 'file', 'files']) {
  test(`rule 4 flags medium noun "${term}" in a non-commit description`, () => {
    const out = checkDescriptionMediumNouns({ name: 'ingest', description: `Build a mem from your ${term}.` });
    assert.ok(out.length >= 1, `expected a violation for "${term}"`);
  });
}

test('rule 4 allowlists "commit" in /commit and stays medium-neutral otherwise', () => {
  // /commit's own graph-commit verb is allowlisted.
  assert.deepEqual(
    checkDescriptionMediumNouns({ name: 'commit', description: 'Commit pending graph changes; a previous commit failed.' }),
    [],
  );
  // the same word in another skill is not allowlisted.
  assert.equal(
    checkDescriptionMediumNouns({ name: 'ingest', description: 'Auto-commit your entities.' }).length,
    1,
  );
  // a genuinely medium-neutral description passes.
  assert.deepEqual(
    checkDescriptionMediumNouns({ name: 'tidy', description: 'Graph hygiene for your mems; it never reads your sources.' }),
    [],
  );
});

// ── exemption ────────────────────────────────────────────────────────

test('nothing is exempt — the full roster is linted after the S1b retirement', () => {
  // reconcile + audit (the former frozen interim survivors) were deleted at
  // S1b, so the exemption list is now empty and every roster skill is scanned.
  assert.deepEqual([...EXEMPT], []);
});
