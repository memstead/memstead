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
 * Characters that may legitimately precede an entity path in a shell
 * command: start-of-string, whitespace, quotes, a slash, and the `:` / `=`
 * forms git refs (`HEAD:specs/x.md`) and option values (`of=specs/x.md`)
 * produce.
 */
const PATH_PREFIX = `(?:^|[\\s"'\`/:=])`;

/** What may follow the `.md` for the token to be a path, not a longer word. */
const PATH_SUFFIX = `(?=[\\s"'\`:;)|&]|$)`;

function entityPathSource(memDir) {
  return `(?:\\./)?(?:${escapeRegex(memDir)})/(?:[a-z0-9][a-z0-9_-]*/)*${ENTITY_NAME_RE}`;
}

/**
 * Check if a command references an entity .md file inside the mem directory.
 * Only matches filenames following entity naming convention (lowercase kebab-case).
 * @param {string} command - The shell command
 * @param {string} memDir - The mem directory name (e.g. 'specs')
 * @returns {boolean}
 */
export function referencesEntityFile(command, memDir) {
  const pattern = new RegExp(`${PATH_PREFIX}${entityPathSource(memDir)}${PATH_SUFFIX}`);
  return pattern.test(command);
}

/**
 * Does the command redirect output INTO an entity file (`> specs/x.md`,
 * `>> specs/x.md`, `2> specs/x.md`)? This — not the presence of `echo`,
 * `printf`, a heredoc, or a bare `>` anywhere — is what makes an
 * output-producing command a mutation of the entity. A redirect to any
 * other target is not this guard's business.
 * @param {string} command - The shell command
 * @param {string} memDir - The mem directory name
 * @returns {boolean}
 */
export function redirectsIntoEntityFile(command, memDir) {
  const pattern = new RegExp(
    `\\d*>{1,2}\\s*(?:"|')?${entityPathSource(memDir)}${PATH_SUFFIX}`,
  );
  return pattern.test(command);
}

/**
 * Write patterns that indicate a command manipulates the files it names.
 * Deliberately NOT here: bare `>` redirects, `echo`, `printf`, and heredocs —
 * none of those touches an entity file unless a redirect targets one, which
 * `redirectsIntoEntityFile` detects against the actual target. Testing them
 * command-wide blocked reads that piped output to scratch paths or merely
 * shared a compound command with an `echo` (backlog, five agents rerouting).
 */
export const WRITE_PATTERNS = [
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
  /\bdd\b/,
  /\binstall\b/,
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
 * Blank every entity-path span so write-verb detection runs on the command's
 * verbs, never on its filenames. Without this, `\binstall\b` (and `patch`,
 * `dd`, `mv`, …) match INSIDE kebab-case entity names — `git show
 * HEAD:specs/install-guide.md` was blocked as a mutation while the identical
 * read on another entity passed. Blanking cannot weaken the guard: a write
 * whose only entity mention is the blanked path is either a redirect into it
 * (caught against the real target first) or a verb that survives blanking.
 */
function blankEntityPaths(command, memDir) {
  const pattern = new RegExp(
    `(^|[\\s"'\`/:=])${entityPathSource(memDir)}${PATH_SUFFIX}`,
    'g',
  );
  return command.replace(pattern, '$1 ');
}

/**
 * Full check: should a bash command be blocked?
 * Blocks when the command (a) redirects output into an entity file, or
 * (b) references an entity file AND carries a file-manipulation verb —
 * tested with entity paths blanked, so a verb inside a filename never counts.
 * @param {string} command - The shell command
 * @param {string} memDir - The mem directory name
 * @returns {{ action: 'block'|'allow', reason?: string }}
 */
export function checkBashCommand(command, memDir) {
  if (!command) return { action: 'allow' };
  if (!referencesEntityFile(command, memDir)) return { action: 'allow' };

  const blocked =
    redirectsIntoEntityFile(command, memDir) ||
    isWriteCommand(blankEntityPaths(command, memDir));
  if (!blocked) return { action: 'allow' };

  return {
    action: 'block',
    reason: `Command: ${command.length > 120 ? command.slice(0, 120) + '...' : command}`,
  };
}
