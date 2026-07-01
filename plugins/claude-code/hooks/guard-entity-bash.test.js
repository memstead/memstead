import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { referencesEntityFile, isWriteCommand, checkBashCommand, escapeRegex } from './guard-entity-bash-utils.mjs';

describe('escapeRegex', () => {
  it('escapes special regex characters', () => {
    assert.equal(escapeRegex('my.specs'), 'my\\.specs');
    assert.equal(escapeRegex('a+b'), 'a\\+b');
  });

  it('leaves plain strings unchanged', () => {
    assert.equal(escapeRegex('specs'), 'specs');
  });
});

describe('referencesEntityFile', () => {
  // Matches — valid entity filenames
  it('matches specs/domain/entity.md', () => {
    assert.ok(referencesEntityFile('cat specs/test-core/spec-entity.md', 'specs'));
  });

  it('matches ./specs/domain/entity.md', () => {
    assert.ok(referencesEntityFile('cat ./specs/test-core/spec-entity.md', 'specs'));
  });

  it('matches quoted paths', () => {
    assert.ok(referencesEntityFile('cat "specs/test-core/spec-entity.md"', 'specs'));
  });

  it('matches nested entity paths', () => {
    assert.ok(referencesEntityFile('cat specs/domain/parent/child.md', 'specs'));
  });

  it('matches digit-prefixed entity names', () => {
    assert.ok(referencesEntityFile('cat specs/domain/3d-model.md', 'specs'));
  });

  // Does NOT match — non-entity filenames
  it('does not match non-.md files in specs/', () => {
    assert.ok(!referencesEntityFile('cat specs/config.json', 'specs'));
  });

  it('does not match uppercase .md in specs/ (e.g. README.md)', () => {
    assert.ok(!referencesEntityFile('cat specs/README.md', 'specs'));
    assert.ok(!referencesEntityFile('cat specs/domain/NOTES.md', 'specs'));
  });

  it('does not match underscore .md in specs/', () => {
    assert.ok(!referencesEntityFile('cat specs/domain/my_notes.md', 'specs'));
  });

  it('does not match specs in other directory names', () => {
    assert.ok(!referencesEntityFile('cat myspecs/foo.md', 'specs'));
  });

  it('does not match commands without entity files', () => {
    assert.ok(!referencesEntityFile('ls -la', 'specs'));
    assert.ok(!referencesEntityFile('echo hello', 'specs'));
  });

  it('works with custom specs directory name', () => {
    assert.ok(referencesEntityFile('cat knowledge/domain/entity.md', 'knowledge'));
    assert.ok(!referencesEntityFile('cat specs/domain/entity.md', 'knowledge'));
  });
});

describe('isWriteCommand', () => {
  // Should detect as write
  it('detects output redirect >', () => {
    assert.ok(isWriteCommand('echo test > file.md'));
  });

  it('detects append redirect >>', () => {
    assert.ok(isWriteCommand('echo test >> file.md'));
  });

  it('detects sed -i', () => {
    assert.ok(isWriteCommand('sed -i "" s/foo/bar/ file.md'));
  });

  it('detects tee', () => {
    assert.ok(isWriteCommand('echo test | tee file.md'));
  });

  it('detects mv', () => {
    assert.ok(isWriteCommand('mv file.md file.bak'));
  });

  it('detects cp', () => {
    assert.ok(isWriteCommand('cp /dev/null file.md'));
  });

  it('detects rm', () => {
    assert.ok(isWriteCommand('rm file.md'));
  });

  it('detects git checkout', () => {
    assert.ok(isWriteCommand('git checkout -- file.md'));
  });

  it('detects git restore', () => {
    assert.ok(isWriteCommand('git restore file.md'));
  });

  it('detects echo', () => {
    assert.ok(isWriteCommand('echo "content"'));
  });

  it('detects heredoc', () => {
    assert.ok(isWriteCommand('cat <<EOF'));
  });

  // Should NOT detect as write
  it('allows plain cat (no redirect)', () => {
    assert.ok(!isWriteCommand('cat file.md'));
  });

  it('allows head', () => {
    assert.ok(!isWriteCommand('head -5 file.md'));
  });

  it('allows tail', () => {
    assert.ok(!isWriteCommand('tail -20 file.md'));
  });

  it('allows git log', () => {
    assert.ok(!isWriteCommand('git log --oneline file.md'));
  });

  it('allows git diff', () => {
    assert.ok(!isWriteCommand('git diff file.md'));
  });

  it('allows wc', () => {
    assert.ok(!isWriteCommand('wc -l file.md'));
  });

  it('allows pipe without write target (cat | head)', () => {
    assert.ok(!isWriteCommand('cat file.md'));
  });
});

describe('checkBashCommand', () => {
  const memDir = 'specs';

  // Blocked: write operations on entity files
  it('blocks echo redirect to entity file', () => {
    const result = checkBashCommand('echo test > specs/test-core/spec-entity.md', memDir);
    assert.equal(result.action, 'block');
  });

  it('blocks sed -i on entity file', () => {
    const result = checkBashCommand('sed -i "" s/foo/bar/ specs/test-core/spec-entity.md', memDir);
    assert.equal(result.action, 'block');
  });

  it('blocks cp to entity file', () => {
    const result = checkBashCommand('cp /dev/null specs/test-core/spec-entity.md', memDir);
    assert.equal(result.action, 'block');
  });

  it('blocks mv of entity file', () => {
    const result = checkBashCommand('mv specs/test-core/spec-entity.md specs/test-core/renamed.md', memDir);
    assert.equal(result.action, 'block');
  });

  it('blocks rm of entity file', () => {
    const result = checkBashCommand('rm specs/test-core/spec-entity.md', memDir);
    assert.equal(result.action, 'block');
  });

  it('blocks git checkout on entity file', () => {
    const result = checkBashCommand('git checkout -- specs/test-core/spec-entity.md', memDir);
    assert.equal(result.action, 'block');
  });

  it('blocks git restore on entity file', () => {
    const result = checkBashCommand('git restore specs/test-core/spec-entity.md', memDir);
    assert.equal(result.action, 'block');
  });

  it('blocks tee to entity file', () => {
    const result = checkBashCommand('echo test | tee specs/test-core/spec-entity.md', memDir);
    assert.equal(result.action, 'block');
  });

  // Allowed: read operations on entity files
  it('allows cat of entity file', () => {
    const result = checkBashCommand('cat specs/test-core/spec-entity.md', memDir);
    assert.equal(result.action, 'allow');
  });

  it('allows cat piped to head', () => {
    const result = checkBashCommand('cat specs/test-core/spec-entity.md | head -5', memDir);
    assert.equal(result.action, 'allow');
  });

  it('allows git log on entity file', () => {
    const result = checkBashCommand('git log --oneline specs/test-core/spec-entity.md', memDir);
    assert.equal(result.action, 'allow');
  });

  it('allows git diff on entity file', () => {
    const result = checkBashCommand('git diff specs/test-core/spec-entity.md', memDir);
    assert.equal(result.action, 'allow');
  });

  it('allows wc on entity file', () => {
    const result = checkBashCommand('wc -l specs/test-core/spec-entity.md', memDir);
    assert.equal(result.action, 'allow');
  });

  // Allowed: write operations on non-entity files
  it('allows echo redirect to non-entity file', () => {
    const result = checkBashCommand('echo test > packages/core/lib/store.js', memDir);
    assert.equal(result.action, 'allow');
  });

  it('allows sed -i on non-entity file', () => {
    const result = checkBashCommand('sed -i "" s/foo/bar/ packages/core/lib/store.js', memDir);
    assert.equal(result.action, 'allow');
  });

  // Allowed: write operations on non-entity-named .md in specs/
  it('allows write to README.md in specs/', () => {
    const result = checkBashCommand('echo test > specs/README.md', memDir);
    assert.equal(result.action, 'allow');
  });

  it('allows write to UPPERCASE.md in specs/', () => {
    const result = checkBashCommand('echo test > specs/domain/STUPID_USER_FILE.md', memDir);
    assert.equal(result.action, 'allow');
  });

  it('allows write to underscore .md in specs/', () => {
    const result = checkBashCommand('echo test > specs/domain/my_notes.md', memDir);
    assert.equal(result.action, 'allow');
  });

  // Edge cases
  it('allows empty command', () => {
    assert.equal(checkBashCommand('', memDir).action, 'allow');
    assert.equal(checkBashCommand(null, memDir).action, 'allow');
    assert.equal(checkBashCommand(undefined, memDir).action, 'allow');
  });

  it('truncates long commands in reason', () => {
    const longCmd = 'echo ' + 'x'.repeat(200) + ' > specs/domain/entity.md';
    const result = checkBashCommand(longCmd, memDir);
    assert.equal(result.action, 'block');
    assert.ok(result.reason.endsWith('...'));
  });

  // Multi-mem: reasoning directory protection
  it('blocks write to entity file in reasoning/ mem', () => {
    const result = checkBashCommand('echo test > reasoning/architecture/my-memo.md', 'reasoning');
    assert.equal(result.action, 'block');
  });

  it('allows read of entity file in reasoning/ mem', () => {
    const result = checkBashCommand('cat reasoning/architecture/my-memo.md', 'reasoning');
    assert.equal(result.action, 'allow');
  });

  it('does not match reasoning/ when checking specs mem', () => {
    const result = checkBashCommand('echo test > reasoning/architecture/my-memo.md', 'specs');
    assert.equal(result.action, 'allow');
  });
});
