// Pure logic for guard-secrets-bash.mjs — testable without process.exit or stdin.

export const SECRET_PATTERNS = [
  { pattern: /(?<![a-zA-Z0-9])\.env(?:\.[a-z][a-z0-9]*)?(?![a-zA-Z0-9])/, label: '.env / .env.*' },
  { pattern: /\.(?:pem|key|p12|pfx|keystore)(?![a-zA-Z0-9])/, label: 'private key / certificate' },
  { pattern: /\.netrc(?![a-zA-Z0-9])/, label: '.netrc' },
  { pattern: /\.pypirc(?![a-zA-Z0-9])/, label: '.pypirc' },
  { pattern: /\.npmrc(?![a-zA-Z0-9])/, label: '.npmrc' },
  { pattern: /serviceAccountKey\.json/, label: 'serviceAccountKey.json' },
  { pattern: /credentials\.json/, label: 'credentials.json' },
  { pattern: /\.aws[/\\]credentials/, label: '.aws/credentials' },
  { pattern: /\.ssh[/\\]id_/, label: '.ssh private key' },
];

/**
 * Returns the matching pattern label if the command appears to access a secret,
 * or null otherwise.
 * @param {string} command
 * @returns {string|null}
 */
export function checkSecretsInCommand(command) {
  if (!command) return null;
  for (const { pattern, label } of SECRET_PATTERNS) {
    if (pattern.test(command)) return label;
  }
  return null;
}
