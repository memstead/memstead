import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { isSecretFile } from './guard-secrets-read-utils.mjs';

describe('isSecretFile', () => {
  // Mirrors memstead-agent::guards::tests::secret_detects_env (Rust legacy parity).
  it('detects bare and absolute .env files', () => {
    assert.ok(isSecretFile('.env'));
    assert.ok(isSecretFile('/project/.env'));
  });

  // Mirrors secret_detects_env_variants
  it('detects .env.* variants', () => {
    assert.ok(isSecretFile('.env.local'));
    assert.ok(isSecretFile('.env.production'));
    assert.ok(isSecretFile('/home/user/.env.test'));
  });

  // Mirrors secret_detects_key_extensions
  it('detects key/cert file extensions', () => {
    assert.ok(isSecretFile('server.pem'));
    assert.ok(isSecretFile('private.key'));
    assert.ok(isSecretFile('cert.p12'));
    assert.ok(isSecretFile('store.pfx'));
    assert.ok(isSecretFile('java.keystore'));
  });

  // Mirrors secret_detects_blocked_names
  it('detects exact blocked filenames', () => {
    assert.ok(isSecretFile('.netrc'));
    assert.ok(isSecretFile('.pypirc'));
    assert.ok(isSecretFile('.npmrc'));
    assert.ok(isSecretFile('serviceAccountKey.json'));
    assert.ok(isSecretFile('credentials.json'));
  });

  // Mirrors secret_detects_aws_credentials
  it('detects .aws/credentials path segment', () => {
    assert.ok(isSecretFile('/home/user/.aws/credentials'));
  });

  // Mirrors secret_detects_ssh_keys
  it('detects .ssh/id_* private key paths', () => {
    assert.ok(isSecretFile('/home/user/.ssh/id_rsa'));
    assert.ok(isSecretFile('/home/user/.ssh/id_ed25519'));
  });

  // Mirrors secret_allows_normal_files
  it('allows normal non-secret files', () => {
    assert.ok(!isSecretFile('src/main.rs'));
    assert.ok(!isSecretFile('package.json'));
    assert.ok(!isSecretFile('README.md'));
  });
});
