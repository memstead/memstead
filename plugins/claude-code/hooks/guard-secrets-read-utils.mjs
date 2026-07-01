// Pure logic for guard-secrets-read.mjs — testable without process.exit or stdin.

/**
 * Check whether a file path refers to a known secret-storing file.
 * Fail-closed: unknown paths matching any pattern are flagged as secrets.
 * @param {string} filePath
 * @returns {boolean}
 */
export function isSecretFile(filePath) {
  if (!filePath) return false;
  const normalized = filePath.replace(/\\/g, '/');
  const filename = normalized.split('/').pop();

  // Exact filename matches
  const blockedNames = new Set([
    '.env', '.netrc', '.pypirc', '.npmrc',
    'serviceAccountKey.json', 'credentials.json',
  ]);
  if (blockedNames.has(filename)) return true;

  // .env.* variants (.env.local, .env.production, .env.test, ...)
  if (filename.startsWith('.env.')) return true;

  // Extension matches
  const blockedExtensions = ['.pem', '.key', '.p12', '.pfx', '.keystore'];
  if (blockedExtensions.some((ext) => filename.endsWith(ext))) return true;

  // Path segment matches
  if (normalized.includes('.aws/credentials')) return true;
  if (normalized.includes('.ssh/id_')) return true;

  return false;
}
