import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import { checkEditTarget, findVaultDir, findAllVaultDirs, isEntityFilename, ENTITY_FILENAME_RE } from './guard-entity-edit-utils.mjs';

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

describe('findVaultDir', () => {
  it('finds --vault from memstead server', () => {
    const config = { mcpServers: { memstead: { args: ['--vault', './specs'] } } };
    assert.equal(findVaultDir(config), './specs');
  });

  it('finds --vault from any server name', () => {
    const config = { mcpServers: { 'my-graph': { args: ['--vault', './my-specs'] } } };
    assert.equal(findVaultDir(config), './my-specs');
  });

  it('returns empty string when no --vault found', () => {
    const config = { mcpServers: { memstead: { args: ['--read-vault', 'file.mdgv'] } } };
    assert.equal(findVaultDir(config), '');
  });

  it('returns empty string for empty config', () => {
    assert.equal(findVaultDir({}), '');
    assert.equal(findVaultDir(null), '');
    assert.equal(findVaultDir(undefined), '');
  });

  it('handles server with no args', () => {
    const config = { mcpServers: { memstead: { command: 'node' } } };
    assert.equal(findVaultDir(config), '');
  });
});

describe('findAllVaultDirs', () => {
  it('collects all --vault args from single server', () => {
    const config = { mcpServers: { memstead: { args: ['--vault', './specs', '--vault', 'other/specs'] } } };
    assert.deepEqual(findAllVaultDirs(config), ['./specs', 'other/specs']);
  });

  it('collects --vault args from multiple servers', () => {
    const config = {
      mcpServers: {
        memstead: { args: ['--vault', './specs'] },
        'memstead-memo': { args: ['--vault', './reasoning'] },
      },
    };
    assert.deepEqual(findAllVaultDirs(config), ['./specs', './reasoning']);
  });

  it('deduplicates identical vault paths', () => {
    const config = {
      mcpServers: {
        a: { args: ['--vault', './specs'] },
        b: { args: ['--vault', './specs'] },
      },
    };
    assert.deepEqual(findAllVaultDirs(config), ['./specs']);
  });

  it('returns empty array for empty config', () => {
    assert.deepEqual(findAllVaultDirs({}), []);
    assert.deepEqual(findAllVaultDirs(null), []);
    assert.deepEqual(findAllVaultDirs(undefined), []);
  });

  it('returns empty array when no --vault found', () => {
    const config = { mcpServers: { memstead: { args: ['--read-vault', 'file.mdgv'] } } };
    assert.deepEqual(findAllVaultDirs(config), []);
  });

  // Mirrors memstead-agent::guards::tests::find_vault_dirs_server_without_args (Rust legacy parity).
  it('returns empty array when server has no args field', () => {
    const config = { mcpServers: { memstead: { command: 'node' } } };
    assert.deepEqual(findAllVaultDirs(config), []);
  });
});

describe('checkEditTarget', () => {
  const vaultDir = resolve('/project/specs');

  // Blocked: entity-named .md files inside vault dir
  it('blocks entity-named .md files inside specs/', () => {
    const result = checkEditTarget('/project/specs/test-core/spec-entity.md', vaultDir, true);
    assert.equal(result.action, 'block');
    assert.ok(result.reason.includes('specs/test-core/spec-entity.md'));
  });

  it('blocks nested entity-named .md files inside specs/', () => {
    const result = checkEditTarget('/project/specs/domain/parent/child.md', vaultDir, true);
    assert.equal(result.action, 'block');
  });

  it('blocks digit-prefixed entity names', () => {
    const result = checkEditTarget('/project/specs/domain/3d-model.md', vaultDir, true);
    assert.equal(result.action, 'block');
  });

  // Allowed: non-entity files inside vault dir
  it('allows non-markdown files in specs/', () => {
    const result = checkEditTarget('/project/specs/config.json', vaultDir, true);
    assert.equal(result.action, 'allow');
  });

  it('allows uppercase .md files in specs/ (e.g. README.md)', () => {
    const result = checkEditTarget('/project/specs/README.md', vaultDir, true);
    assert.equal(result.action, 'allow');
  });

  it('allows underscore .md files in specs/ (e.g. my_notes.md)', () => {
    const result = checkEditTarget('/project/specs/domain/my_notes.md', vaultDir, true);
    assert.equal(result.action, 'allow');
  });

  it('allows ALLCAPS .md files in specs/', () => {
    const result = checkEditTarget('/project/specs/domain/STUPID_USER_FILE.md', vaultDir, true);
    assert.equal(result.action, 'allow');
  });

  // Allowed: files outside specs/
  it('allows files outside specs/', () => {
    const result = checkEditTarget('/project/packages/core/lib/store.js', vaultDir, true);
    assert.equal(result.action, 'allow');
  });

  it('allows .md files outside specs/', () => {
    const result = checkEditTarget('/project/README.md', vaultDir, true);
    assert.equal(result.action, 'allow');
  });

  it('allows when filePath is empty', () => {
    assert.equal(checkEditTarget('', vaultDir, true).action, 'allow');
    assert.equal(checkEditTarget(null, vaultDir, true).action, 'allow');
    assert.equal(checkEditTarget(undefined, vaultDir, true).action, 'allow');
  });

  // Fail-closed behavior when vault dir doesn't exist
  it('blocks potential entity .md when vault dir missing', () => {
    const result = checkEditTarget('/somewhere/specs/domain/entity.md', vaultDir, false);
    assert.equal(result.action, 'block');
    assert.ok(result.reason.includes('Cannot verify vault dir'));
  });

  it('allows uppercase .md in specs path when vault dir missing', () => {
    const result = checkEditTarget('/somewhere/specs/domain/README.md', vaultDir, false);
    assert.equal(result.action, 'allow');
  });

  it('allows non-entity files when vault dir missing', () => {
    const result = checkEditTarget('/project/packages/core/store.js', vaultDir, false);
    assert.equal(result.action, 'allow');
  });

  it('allows .md files without "specs" in path when vault dir missing', () => {
    const result = checkEditTarget('/project/docs/README.md', vaultDir, false);
    assert.equal(result.action, 'allow');
  });
});
