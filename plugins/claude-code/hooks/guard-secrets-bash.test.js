import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { checkSecretsInCommand } from './guard-secrets-bash-utils.mjs';

describe('checkSecretsInCommand', () => {
  // Mirrors memstead-agent::guards::tests::secret_cmd_detects_env (Rust legacy parity).
  it('detects cat .env', () => {
    assert.equal(checkSecretsInCommand('cat .env'), '.env / .env.*');
  });

  // Mirrors secret_cmd_detects_env_local
  it('detects grep on .env.local', () => {
    assert.equal(checkSecretsInCommand('grep password .env.local'), '.env / .env.*');
  });

  // Mirrors secret_cmd_detects_key_files
  it('detects key/certificate file extensions in commands', () => {
    assert.equal(checkSecretsInCommand('cat server.pem'), 'private key / certificate');
  });

  // Mirrors secret_cmd_detects_credentials
  it('detects credentials.json in commands', () => {
    assert.equal(checkSecretsInCommand('cat credentials.json'), 'credentials.json');
  });

  // Mirrors secret_cmd_detects_aws
  it('detects .aws/credentials path in commands', () => {
    assert.equal(checkSecretsInCommand('cat .aws/credentials'), '.aws/credentials');
  });

  // Mirrors secret_cmd_detects_ssh
  it('detects .ssh private key paths in commands', () => {
    assert.equal(checkSecretsInCommand('cat .ssh/id_rsa'), '.ssh private key');
  });

  // Mirrors secret_cmd_allows_normal_commands
  it('allows normal non-secret commands', () => {
    assert.equal(checkSecretsInCommand('ls -la'), null);
    assert.equal(checkSecretsInCommand('cat src/main.rs'), null);
  });
});
