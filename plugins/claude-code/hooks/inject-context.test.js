import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { renderWriteRules } from './inject-context-utils.mjs';

describe('renderWriteRules', () => {
  it('returns constraints from the first non-private node', () => {
    const json = {
      nodes: [
        { private: true },
        { constraints: '- Rule one\n- Rule two' },
      ],
    };
    assert.equal(renderWriteRules(json), '- Rule one\n- Rule two');
  });

  it('returns empty for null or no nodes', () => {
    assert.equal(renderWriteRules(null), '');
    assert.equal(renderWriteRules({}), '');
    assert.equal(renderWriteRules({ nodes: [] }), '');
  });

  it('skips nodes without constraints', () => {
    const json = {
      nodes: [
        { title: 'No constraints' },
        { constraints: '- Found it' },
      ],
    };
    assert.equal(renderWriteRules(json), '- Found it');
  });
});
