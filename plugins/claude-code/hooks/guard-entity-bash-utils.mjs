// Pure logic for guard-entity-bash.mjs — testable without process.exit or stdin.

/**
 * Escape special regex characters in a string.
 */
export function escapeRegex(str) {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Regex fragment matching an entity filename in a shell command.
 * Only matches kebab-case names that titleToId() would produce.
 * e.g. "my-entity.md", "3d-model.md", "a.md" — but NOT "README.md", "NOTES.md"
 */
const ENTITY_NAME_RE = '[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\\.md';

/**
 * Check if a command references an entity .md file inside the vault directory.
 * Only matches filenames following entity naming convention (lowercase kebab-case).
 * @param {string} command - The shell command
 * @param {string} vaultDir - The vault directory name (e.g. 'specs')
 * @returns {boolean}
 */
export function referencesEntityFile(command, vaultDir) {
  const pattern = new RegExp(
    `(?:^|[\\s"'\`/])(?:\\./)?(?:${escapeRegex(vaultDir)})/(?:[a-z0-9][a-z0-9_-]*/)*${ENTITY_NAME_RE}(?:[\\s"'\`]|$)`,
  );
  return pattern.test(command);
}

/**
 * Write patterns that indicate a command modifies files.
 */
export const WRITE_PATTERNS = [
  // Output redirects (but not pipes — pipes are read-only for the source file)
  />/,
  // In-place editors
  /\bsed\b.*-i/,
  /\bperl\b.*-[ip]/,
  /\bawk\b.*-i/,
  // File manipulation
  /\btee\b/,
  /\bmv\b/,
  /\bcp\b/,
  /\brm\b/,
  /\bpatch\b/,
  /\bchmod\b/,
  /\btruncate\b/,
  // Editors
  /\bdd\b/,
  /\binstall\b/,
  // Write-capable commands with output redirection (cat <<, echo, printf)
  /\bcat\b.*<</, // heredoc
  /\becho\b/,
  /\bprintf\b/,
  // Git operations that overwrite files
  /\bgit\b.*\b(?:checkout|restore|reset|stash\s+pop)\b/,
];

/**
 * Check if a command contains a write operation pattern.
 * @param {string} command - The shell command
 * @returns {boolean}
 */
export function isWriteCommand(command) {
  return WRITE_PATTERNS.some((p) => p.test(command));
}

/**
 * Full check: should a bash command be blocked?
 * @param {string} command - The shell command
 * @param {string} vaultDir - The vault directory name
 * @returns {{ action: 'block'|'allow', reason?: string }}
 */
export function checkBashCommand(command, vaultDir) {
  if (!command) return { action: 'allow' };
  if (!referencesEntityFile(command, vaultDir)) return { action: 'allow' };
  if (!isWriteCommand(command)) return { action: 'allow' };

  return {
    action: 'block',
    reason: `Command: ${command.length > 120 ? command.slice(0, 120) + '...' : command}`,
  };
}
