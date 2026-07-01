import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import { checkEditTarget, findMemDir, findAllMemDirs, isEntityFilename, ENTITY_FILENAME_RE } from './guard-entity-edit-utils.mjs';

describe('ENTITY_FILENAME_RE / isEntityFilename', () => {
  // Valid entity filenames (output of titleToId + .md)
  it('matches kebab-case names', () => {
    assert.ok(isEntityFilename('spec-entity.md'));
    assert.ok(isEntityFilename('markdown-parser.md'));
    assert.ok(isEntityFilename('claude-code-plugin.md'));
  });

  it('matches single-word names', () => {
    assert.ok(isEntityFilename('store.md'));
    assert.ok(isEntityFilename('parser.md'));
  });

  it('matches single-char names', () => {
    assert.ok(isEntityFilename('a.md'));
    assert.ok(isEntityFilename('z.md'));
  });

  it('matches names starting with digit', () => {
    assert.ok(isEntityFilename('3d-model.md'));
    assert.ok(isEntityFilename('42.md'));
  });

  it('matches names with digits in middle', () => {
    assert.ok(isEntityFilename('spec-v2-entity.md'));
    assert.ok(isEntityFilename('my-3d-thing.md'));
  });

  // Invalid — should NOT match
  it('rejects uppercase names', () => {
    assert.ok(!isEntityFilename('README.md'));
    assert.ok(!isEntityFilename('STUPID_USER_FILE.md'));
    assert.ok(!isEntityFilename('MySpec.md'));
  });

  it('rejects names with underscores', () => {
    assert.ok(!isEntityFilename('my_module.md'));
    assert.ok(!isEntityFilename('spec_entity.md'));
  });

  it('rejects names starting with dash', () => {
    assert.ok(!isEntityFilename('-invalid.md'));
  });

  it('rejects names ending with dash', () => {
    assert.ok(!isEntityFilename('invalid-.md'));
  });

  it('rejects hidden files', () => {
    assert.ok(!isEntityFilename('.hidden.md'));
  });

  it('rejects non-.md files', () => {
    assert.ok(!isEntityFilename('spec-entity.js'));
    assert.ok(!isEntityFilename('config.json'));
  });

  it('rejects empty / null', () => {
    assert.ok(!isEntityFilename(''));
    assert.ok(!isEntityFilename('.md'));
  });
});

describe('findMemDir', () => {
  it('finds --mem from memstead server', () => {
    const config = { mcpServers: { memstead: { args: ['--mem', './specs'] } } };
    assert.equal(findMemDir(config), './specs');
  });

  it('finds --mem from any server name', () => {
    const config = { mcpServers: { 'my-graph': { args: ['--mem', './my-specs'] } } };
    assert.equal(findMemDir(config), './my-specs');
  });

  it('returns empty string when no --mem found', () => {
    const config = { mcpServers: { memstead: { args: ['--read-mem', 'file.mem'] } } };
    assert.equal(findMemDir(config), '');
  });

  it('returns empty string for empty config', () => {
    assert.equal(findMemDir({}), '');
    assert.equal(findMemDir(null), '');
    assert.equal(findMemDir(undefined), '');
  });

  it('handles server with no args', () => {
    const config = { mcpServers: { memstead: { command: 'node' } } };
    assert.equal(findMemDir(config), '');
  });
});

describe('findAllMemDirs', () => {
  it('collects all --mem args from single server', () => {
    const config = { mcpServers: { memstead: { args: ['--mem', './specs', '--mem', 'other/specs'] } } };
    assert.deepEqual(findAllMemDirs(config), ['./specs', 'other/specs']);
  });

  it('collects --mem args from multiple servers', () => {
    const config = {
      mcpServers: {
        memstead: { args: ['--mem', './specs'] },
        'memstead-memo': { args: ['--mem', './reasoning'] },
      },
    };
    assert.deepEqual(findAllMemDirs(config), ['./specs', './reasoning']);
  });

  it('deduplicates identical mem paths', () => {
    const config = {
      mcpServers: {
        a: { args: ['--mem', './specs'] },
        b: { args: ['--mem', './specs'] },
      },
    };
    assert.deepEqual(findAllMemDirs(config), ['./specs']);
  });

  it('returns empty array for empty config', () => {
    assert.deepEqual(findAllMemDirs({}), []);
    assert.deepEqual(findAllMemDirs(null), []);
    assert.deepEqual(findAllMemDirs(undefined), []);
  });

  it('returns empty array when no --mem found', () => {
    const config = { mcpServers: { memstead: { args: ['--read-mem', 'file.mem'] } } };
    assert.deepEqual(findAllMemDirs(config), []);
  });

  // Mirrors memstead-agent::guards::tests::find_mem_dirs_server_without_args (Rust legacy parity).
  it('returns empty array when server has no args field', () => {
    const config = { mcpServers: { memstead: { command: 'node' } } };
    assert.deepEqual(findAllMemDirs(config), []);
  });
});

describe('checkEditTarget', () => {
  const memDir = resolve('/project/specs');

  // Blocked: entity-named .md files inside mem dir
  it('blocks entity-named .md files inside specs/', () => {
    const result = checkEditTarget('/project/specs/test-core/spec-entity.md', memDir, true);
    assert.equal(result.action, 'block');
    assert.ok(result.reason.includes('specs/test-core/spec-entity.md'));
  });

  it('blocks nested entity-named .md files inside specs/', () => {
    const result = checkEditTarget('/project/specs/domain/parent/child.md', memDir, true);
    assert.equal(result.action, 'block');
  });

  it('blocks digit-prefixed entity names', () => {
    const result = checkEditTarget('/project/specs/domain/3d-model.md', memDir, true);
    assert.equal(result.action, 'block');
  });

  // Allowed: non-entity files inside mem dir
  it('allows non-markdown files in specs/', () => {
    const result = checkEditTarget('/project/specs/config.json', memDir, true);
    assert.equal(result.action, 'allow');
  });

  it('allows uppercase .md files in specs/ (e.g. README.md)', () => {
    const result = checkEditTarget('/project/specs/README.md', memDir, true);
    assert.equal(result.action, 'allow');
  });

  it('allows underscore .md files in specs/ (e.g. my_notes.md)', () => {
    const result = checkEditTarget('/project/specs/domain/my_notes.md', memDir, true);
    assert.equal(result.action, 'allow');
  });

  it('allows ALLCAPS .md files in specs/', () => {
    const result = checkEditTarget('/project/specs/domain/STUPID_USER_FILE.md', memDir, true);
    assert.equal(result.action, 'allow');
  });

  // Allowed: files outside specs/
  it('allows files outside specs/', () => {
    const result = checkEditTarget('/project/packages/core/lib/store.js', memDir, true);
    assert.equal(result.action, 'allow');
  });

  it('allows .md files outside specs/', () => {
    const result = checkEditTarget('/project/README.md', memDir, true);
    assert.equal(result.action, 'allow');
  });

  it('allows when filePath is empty', () => {
    assert.equal(checkEditTarget('', memDir, true).action, 'allow');
    assert.equal(checkEditTarget(null, memDir, true).action, 'allow');
    assert.equal(checkEditTarget(undefined, memDir, true).action, 'allow');
  });

  // Fail-closed behavior when mem dir doesn't exist
  it('blocks potential entity .md when mem dir missing', () => {
    const result = checkEditTarget('/somewhere/specs/domain/entity.md', memDir, false);
    assert.equal(result.action, 'block');
    assert.ok(result.reason.includes('Cannot verify mem dir'));
  });

  it('allows uppercase .md in specs path when mem dir missing', () => {
    const result = checkEditTarget('/somewhere/specs/domain/README.md', memDir, false);
    assert.equal(result.action, 'allow');
  });

  it('allows non-entity files when mem dir missing', () => {
    const result = checkEditTarget('/project/packages/core/store.js', memDir, false);
    assert.equal(result.action, 'allow');
  });

  it('allows .md files without "specs" in path when mem dir missing', () => {
    const result = checkEditTarget('/project/docs/README.md', memDir, false);
    assert.equal(result.action, 'allow');
  });
});
