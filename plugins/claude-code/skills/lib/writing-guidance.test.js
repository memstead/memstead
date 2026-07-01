/**
 * writing-guidance.test.js — tests for the writing-guidance resolver.
 *
 * Run via `node --test` from the repo root:
 *   node --test plugins/claude-code/skills/lib/writing-guidance.test.js
 *
 * Pins the resolver's six contract surfaces:
 *   - happy path:       schema default + mem additions concatenate
 *   - schema-only:      no additions → schema default verbatim
 *   - empty:            nothing on either side → no `avoid` / `goal` key
 *   - legacy fallback:  mem keeps pre-migration `avoid` → use it, log
 *   - legacy precedence: legacy + additions both present → legacy wins
 *   - pass-through:     stack/language/granularity flow verbatim
 *
 * Plus three for the YAML extractor:
 *   - block scalar (`|`) extraction
 *   - both keys present
 *   - missing block returns {}
 */

import { describe, it, beforeEach } from 'node:test';
import assert from 'node:assert/strict';
import {
  resolveWritingGuidance,
  extractDefaultWritingGuidance,
  renderResolvedGuidance,
  _resetLegacyWarnCacheForTests,
} from './writing-guidance.mjs';

// ── Resolver ────────────────────────────────────────────────────────────────

describe('resolveWritingGuidance — happy path', () => {
  beforeEach(_resetLegacyWarnCacheForTests);

  it('concatenates schema default + mem avoid_additions with one blank line', () => {
    const schemaPayload = {
      default_writing_guidance: { avoid: 'Schema-default avoid prose.' },
    };
    const memConfig = {
      name: 'engine',
      writeGuidance: { avoid_additions: 'Engine-specific extra.' },
    };
    const merged = resolveWritingGuidance(schemaPayload, memConfig);
    assert.equal(merged.avoid, 'Schema-default avoid prose.\n\nEngine-specific extra.');
  });

  it('returns schema default verbatim when mem has no additions', () => {
    const schemaPayload = {
      default_writing_guidance: { avoid: 'Default only.', goal: 'Default goal.' },
    };
    const memConfig = { name: 'engine', writeGuidance: { stack: 'Rust' } };
    const merged = resolveWritingGuidance(schemaPayload, memConfig);
    assert.equal(merged.avoid, 'Default only.');
    assert.equal(merged.goal, 'Default goal.');
    assert.equal(merged.stack, 'Rust');
  });

  it('omits avoid/goal keys entirely when nothing on either side', () => {
    const merged = resolveWritingGuidance(null, { name: 'v', writeGuidance: { stack: 'x' } });
    assert.ok(!('avoid' in merged), 'avoid key absent');
    assert.ok(!('goal' in merged), 'goal key absent');
    assert.equal(merged.stack, 'x');
  });

  it('passes through non-reserved keys (granularity, stack, language, phase_context)', () => {
    const schemaPayload = { default_writing_guidance: { avoid: 'd' } };
    const memConfig = {
      name: 'plan',
      writeGuidance: {
        stack: 'Rust',
        language: 'English',
        granularity: 'one entity per concept',
        phase_context: 'TEMPLATE',
        avoid_additions: 'extra',
      },
    };
    const merged = resolveWritingGuidance(schemaPayload, memConfig);
    assert.equal(merged.stack, 'Rust');
    assert.equal(merged.language, 'English');
    assert.equal(merged.granularity, 'one entity per concept');
    assert.equal(merged.phase_context, 'TEMPLATE');
    assert.equal(merged.avoid, 'd\n\nextra');
    // `avoid_additions` itself is consumed, not passed through.
    assert.ok(!('avoid_additions' in merged), 'avoid_additions consumed, not passed through');
  });
});

describe('resolveWritingGuidance — legacy fallback', () => {
  beforeEach(_resetLegacyWarnCacheForTests);

  it('returns mem.writeGuidance.avoid verbatim when present (pre-migration)', () => {
    const schemaPayload = {
      default_writing_guidance: { avoid: 'Schema default that should be ignored.' },
    };
    const memConfig = {
      name: 'unmigrated',
      writeGuidance: { avoid: 'Legacy literal.' },
    };
    // Capture the deprecation warning so tests don't emit noise.
    const original = console.warn;
    const warnings = [];
    console.warn = (msg) => warnings.push(msg);
    try {
      const merged = resolveWritingGuidance(schemaPayload, memConfig);
      assert.equal(merged.avoid, 'Legacy literal.');
      assert.equal(warnings.length, 1);
      assert.match(warnings[0], /unmigrated/);
      assert.match(warnings[0], /writeGuidance\.avoid/);
    } finally {
      console.warn = original;
    }
  });

  it('legacy avoid wins over avoid_additions (half-finished migration)', () => {
    const schemaPayload = { default_writing_guidance: { avoid: 'def' } };
    const memConfig = {
      name: 'half',
      writeGuidance: { avoid: 'legacy', avoid_additions: 'extra' },
    };
    const original = console.warn;
    console.warn = () => {};
    try {
      const merged = resolveWritingGuidance(schemaPayload, memConfig);
      assert.equal(merged.avoid, 'legacy', 'legacy block must win even when additions are present');
    } finally {
      console.warn = original;
    }
  });

  it('logs the deprecation warning at most once per (mem, field)', () => {
    const schemaPayload = { default_writing_guidance: {} };
    const memConfig = { name: 'noisy', writeGuidance: { avoid: 'l' } };
    const original = console.warn;
    const warnings = [];
    console.warn = (msg) => warnings.push(msg);
    try {
      resolveWritingGuidance(schemaPayload, memConfig);
      resolveWritingGuidance(schemaPayload, memConfig);
      resolveWritingGuidance(schemaPayload, memConfig);
      assert.equal(warnings.length, 1, 'cache must dedup repeated calls');
    } finally {
      console.warn = original;
    }
  });
});

describe('resolveWritingGuidance — null inputs', () => {
  beforeEach(_resetLegacyWarnCacheForTests);

  it('null schema payload + null mem config → empty object', () => {
    const merged = resolveWritingGuidance(null, null);
    assert.deepEqual(merged, {});
  });

  it('null schema payload + mem additions only → additions become the avoid', () => {
    const merged = resolveWritingGuidance(null, {
      name: 'v',
      writeGuidance: { avoid_additions: 'just additions' },
    });
    assert.equal(merged.avoid, 'just additions');
  });
});

// ── YAML extractor ──────────────────────────────────────────────────────────

describe('extractDefaultWritingGuidance', () => {
  it('extracts both `avoid:` and `goal:` block scalars', () => {
    const yaml = `name: software
version: 0.1.0
default_writing_guidance:
  avoid: |
    First line.

    - Bullet one.
    - Bullet two.
  goal: |
    Goal prose.
`;
    const got = extractDefaultWritingGuidance(yaml);
    assert.equal(
      got.avoid,
      'First line.\n\n- Bullet one.\n- Bullet two.',
    );
    assert.equal(got.goal, 'Goal prose.');
  });

  it('returns {} when default_writing_guidance is absent', () => {
    const yaml = `name: x
version: 1.0.0
types:
  - spec
`;
    assert.deepEqual(extractDefaultWritingGuidance(yaml), {});
  });

  it('handles avoid only / goal only', () => {
    const yamlAvoidOnly = `default_writing_guidance:
  avoid: |
    Only avoid.
`;
    assert.deepEqual(extractDefaultWritingGuidance(yamlAvoidOnly), { avoid: 'Only avoid.' });

    const yamlGoalOnly = `default_writing_guidance:
  goal: |
    Only goal.
`;
    assert.deepEqual(extractDefaultWritingGuidance(yamlGoalOnly), { goal: 'Only goal.' });
  });

  it('terminates the block at the next top-level key', () => {
    const yaml = `default_writing_guidance:
  avoid: |
    A
    B
community:
  resolution: 1.0
  seed: 42
`;
    const got = extractDefaultWritingGuidance(yaml);
    assert.equal(got.avoid, 'A\nB');
    assert.ok(!('goal' in got));
  });

  it('returns {} for non-string input', () => {
    assert.deepEqual(extractDefaultWritingGuidance(null), {});
    assert.deepEqual(extractDefaultWritingGuidance(undefined), {});
    assert.deepEqual(extractDefaultWritingGuidance(42), {});
  });
});

// ── Renderer ────────────────────────────────────────────────────────────────

describe('renderResolvedGuidance', () => {
  it('prepends the consistent framing for the avoid block', () => {
    const got = renderResolvedGuidance({ avoid: '- bullet one\n- bullet two' });
    assert.match(got, /\*\*Authoring norms\*\*/, 'must label as authoring norms');
    assert.match(got, /required_outgoing/, 'must reference required_outgoing for context');
    assert.match(got, /- bullet one/, 'bullet content must reach the output');
    assert.match(got, /- bullet two/, 'all bullets must reach the output');
  });

  it('renders Goal under its own header', () => {
    const got = renderResolvedGuidance({ goal: 'The graph is...' });
    assert.match(got, /\*\*Goal:\*\*/);
    assert.match(got, /The graph is/);
  });

  it('passes through other keys as **key:** value pairs', () => {
    const got = renderResolvedGuidance({ stack: 'Rust', language: 'English' });
    assert.match(got, /\*\*stack:\*\* Rust/);
    assert.match(got, /\*\*language:\*\* English/);
  });

  it('separates blocks with a blank line', () => {
    const got = renderResolvedGuidance({
      avoid: '- bullet',
      goal: 'goal text',
      stack: 'Rust',
    });
    // Avoid block, blank, Goal block, blank, stack pass-through.
    assert.match(got, /- bullet\n\n\*\*Goal:\*\*/, 'avoid → goal blank-separated');
    assert.match(got, /goal text\n\n\*\*stack:\*\* Rust/, 'goal → stack blank-separated');
  });

  it('returns empty string for null / empty / non-object input', () => {
    assert.equal(renderResolvedGuidance(null), '');
    assert.equal(renderResolvedGuidance(undefined), '');
    assert.equal(renderResolvedGuidance({}), '');
    assert.equal(renderResolvedGuidance('string'), '');
  });

  it('omits avoid block when avoid is empty / whitespace-only', () => {
    const got = renderResolvedGuidance({ avoid: '   ', stack: 'Rust' });
    assert.ok(!/Authoring norms/.test(got), 'must not emit framing for empty avoid');
    assert.match(got, /\*\*stack:\*\* Rust/);
  });

  it('drops _additions keys (consumed upstream by the resolver)', () => {
    // Defensive: if a caller passes pre-resolution data by mistake,
    // these keys must not leak into the rendered prompt.
    const got = renderResolvedGuidance({
      avoid: '- bullet',
      avoid_additions: 'should not appear',
      goal_additions: 'also not',
    });
    assert.ok(!/should not appear/.test(got));
    assert.ok(!/also not/.test(got));
  });
});

// ── End-to-end against representative schema YAML ───────────────────────
//
// Before the engine-owns-mem-repo rule these tests read shipping
// schema YAML directly from the mem-repo `schemas/...` tree and asserted
// properties of the ON-DISK content. Two concerns conflated: extractor
// behaviour, and "is the shipping schema's prose correct." The plan
// migrates the plugin off direct mem-repo file reads — so this block
// now exercises the extractor against an inline schema YAML carrying
// the same shape (block-scalar `avoid` and `goal` under
// `default_writing_guidance`, preceded and followed by other top-level
// keys). The "is the shipping schema correct" question moves to the
// engine's schema-loading tests where it belongs.

describe('end-to-end against representative schema YAML', () => {
  it('extracts both avoid and goal from a realistic shipping-shaped schema', () => {
    // Mirrors the layout of a real shipping schema: top-level metadata,
    // a `default_writing_guidance` block with multi-line block scalars,
    // and other sections (`types`, `community`) on either side. The
    // assertion checks that the extractor returns the prose verbatim
    // (modulo the block-scalar dedent) and stops at the next top-level
    // key.
    const yaml = `name: example
version: 1.0.0
description: Representative schema for extractor regression coverage.
default_writing_guidance:
  avoid: |
    Misfiled entities under the wrong type.

    - Stuffing prose into the wrong section.
    - Fabricated cross-mem edges.
  goal: |
    The graph is a comprehensive, unambiguous mirror of the codebase.
types:
  - name: spec
community:
  resolution: 1.0
`;
    const got = extractDefaultWritingGuidance(yaml);
    assert.equal(
      got.avoid,
      'Misfiled entities under the wrong type.\n\n- Stuffing prose into the wrong section.\n- Fabricated cross-mem edges.',
    );
    assert.equal(
      got.goal,
      'The graph is a comprehensive, unambiguous mirror of the codebase.',
    );
  });
});
