import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { extractRealizationPaths, fileToId, pathMatches } from './check-realization-utils.mjs';

// Minimal schema with drift patterns (matches uni-spec)
const SCHEMA = {
  drift: {
    realizationPatterns: {
      fileHeader: /^###\s+Files?:\s*(.+)$/gm,
      backtickPath: /`([^`\n]+\.[a-z]+)`/g,
    },
  },
};

describe('extractRealizationPaths', () => {
  it('extracts paths from ### File: header', () => {
    const content = '### File: `packages/core/lib/store.js`\n\nSome text.';
    assert.deepStrictEqual(extractRealizationPaths(content, SCHEMA), ['packages/core/lib/store.js']);
  });

  it('extracts multiple paths from ### Files: header', () => {
    const content = '### Files: `packages/core/lib/a.js`, `packages/core/lib/b.js`\n';
    const paths = extractRealizationPaths(content, SCHEMA);
    assert.ok(paths.includes('packages/core/lib/a.js'));
    assert.ok(paths.includes('packages/core/lib/b.js'));
  });

  it('extracts inline backtick paths with file extensions', () => {
    const content = 'The module `packages/core/lib/parser.js` handles parsing.';
    assert.deepStrictEqual(extractRealizationPaths(content, SCHEMA), ['packages/core/lib/parser.js']);
  });

  it('ignores inline paths without slash (not file paths)', () => {
    const content = 'Use `store.js` directly.';
    assert.deepStrictEqual(extractRealizationPaths(content, SCHEMA), []);
  });

  it('ignores paths inside code blocks', () => {
    const content = [
      'Some text.',
      '```javascript',
      'import { foo } from `packages/core/lib/hidden.js`',
      '```',
      'See `packages/core/lib/visible.js` for details.',
    ].join('\n');
    const paths = extractRealizationPaths(content, SCHEMA);
    assert.ok(paths.includes('packages/core/lib/visible.js'));
    assert.ok(!paths.includes('packages/core/lib/hidden.js'));
  });

  it('ignores paths with HTML-like content', () => {
    const content = 'The tag `<div class="foo">` is not a path.';
    assert.deepStrictEqual(extractRealizationPaths(content, SCHEMA), []);
  });

  it('deduplicates paths', () => {
    const content = [
      '### File: `packages/core/lib/store.js`',
      'Also see `packages/core/lib/store.js` for details.',
    ].join('\n');
    const paths = extractRealizationPaths(content, SCHEMA);
    assert.equal(paths.length, 1);
  });

  it('handles content with no paths', () => {
    const content = '# My Spec\n\nJust some text without any file references.';
    assert.deepStrictEqual(extractRealizationPaths(content, SCHEMA), []);
  });

  it('handles multiple file extensions', () => {
    const content = [
      'See `src/main.ts` for TypeScript.',
      'And `lib/config.yaml` for config.',
      'And `styles/app.css` for styles.',
    ].join('\n');
    const paths = extractRealizationPaths(content, SCHEMA);
    assert.ok(paths.includes('src/main.ts'));
    assert.ok(paths.includes('lib/config.yaml'));
    assert.ok(paths.includes('styles/app.css'));
  });

  it('accepts previously-rejected extensions (.bpmn, .pdf, .docx)', () => {
    const content = [
      'See `processes/order.bpmn` for the workflow.',
      'And `docs/guide.pdf` for documentation.',
      'And `docs/spec.docx` for the spec.',
    ].join('\n');
    const paths = extractRealizationPaths(content, SCHEMA);
    assert.ok(paths.includes('processes/order.bpmn'));
    assert.ok(paths.includes('docs/guide.pdf'));
    assert.ok(paths.includes('docs/spec.docx'));
  });

  it('rejects non-path content like if/else and input/output', () => {
    const content = [
      'Use `if/else` for branching.',
      'Handle `input/output` carefully.',
    ].join('\n');
    assert.deepStrictEqual(extractRealizationPaths(content, SCHEMA), []);
  });

  it('returns empty array when content is empty/null', () => {
    assert.deepStrictEqual(extractRealizationPaths('', SCHEMA), []);
    assert.deepStrictEqual(extractRealizationPaths(null, SCHEMA), []);
  });
});

describe('fileToId', () => {
  it('converts flat entity path to ID', () => {
    assert.equal(fileToId('test-engine/markdown-parser.md'), 'test-engine--markdown-parser');
  });

  it('takes last segment as slug for nested paths (defensive — flat layout is canonical)', () => {
    assert.equal(fileToId('test-plugin/plugin/audit-command.md'), 'test-plugin--audit-command');
  });

  it('returns null for single-segment path', () => {
    assert.equal(fileToId('orphan.md'), null);
  });

  it('handles Windows backslashes', () => {
    assert.equal(fileToId('test-core\\spec-entity.md'), 'test-core--spec-entity');
  });

  it('handles deeply nested paths', () => {
    assert.equal(fileToId('test/a/b/c/leaf.md'), 'test--leaf');
  });
});

describe('pathMatches', () => {
  it('matches exact path', () => {
    assert.ok(pathMatches('packages/core/lib/store.js', 'packages/core/lib/store.js'));
  });

  it('matches when edited path has project prefix', () => {
    assert.ok(pathMatches('myproject/packages/core/lib/store.js', 'packages/core/lib/store.js'));
  });

  it('matches when realization path has prefix', () => {
    assert.ok(pathMatches('lib/store.js', 'packages/core/lib/store.js'));
  });

  it('does not match unrelated paths', () => {
    assert.ok(!pathMatches('packages/core/lib/parser.js', 'packages/core/lib/store.js'));
  });

  it('does not match partial filename overlap', () => {
    assert.ok(!pathMatches('my-store.js', 'packages/core/lib/store.js'));
  });
});
