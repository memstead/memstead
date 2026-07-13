---
type: memo
created_date: 2026-07-13T16:43:07Z
last_modified: 2026-07-13T16:43:07Z
status: closed
tags: constraint, observation, security, plugin, hooks
---

# Secret-file ruleset is duplicated across the two guard detectors

## Claim
The secret-file ruleset — which filenames, extensions, and path segments count as secret — is defined independently in the two secret-guard detectors, so adding a new secret type means editing both copies to keep the file-path and shell tool surfaces in sync.


**Closed 2026-07-11 (plugin diet):** both secret-guard detectors were removed from the plugin, dissolving the duplication — secrets hygiene is delegated to Claude Code's `permissions.deny`.

## Context
- Two PreToolUse guards cover secret access on disjoint tool surfaces: [[plugin--secret-file-guard-hook]] reads `tool_input.file_path` (effective on Read/Write/Edit), and [[plugin--secret-file-bash-guard-hook]] scans `tool_input.command` (Bash). Together they satisfy [[plugin--agent-must-not-access-secret-files-through-any-tool-surface]], whose statement is that NO tool surface may reach a secret.
- Each guard carries its own copy of the ruleset rather than a shared one: `guard-secrets-read-utils.mjs` uses a filename `Set` + extension array + path-segment substring checks (`isSecretFile`); `guard-secrets-bash-utils.mjs` uses a separate `SECRET_PATTERNS` regex array (`checkSecretsInCommand`).
- The two lists are in sync as of this writing: `.env` / `.env.*`, `.pem` / `.key` / `.p12` / `.pfx` / `.keystore`, `.netrc`, `.pypirc`, `.npmrc`, `serviceAccountKey.json`, `credentials.json`, `.aws/credentials`, `.ssh/id_*`.

## Relationships
- **REFERENCES**: [[plugin:secret-file-guard-hook]]
- **REFERENCES**: [[plugin:secret-file-bash-guard-hook]]
- **REFERENCES**: [[plugin:agent-must-not-access-secret-files-through-any-tool-surface]]

## Substance

- The duplication is a DRY-drift hazard: a secret type added to one detector but not the other silently leaves one tool surface uncovered while the requirement still claims both are guarded. No shared module and no parity test currently enforce that the two lists agree — the sync is maintained by hand.
- The two representations are not trivially unifiable into one matcher: the file-path guard matches the final path segment (filename-anchored, exact/prefix/suffix), whereas the shell guard must match a secret token anywhere inside a free-form command string using word-boundary-guarded regexes. A single shared source-of-truth list would still need two surface-specific matchers over it.

## Alternatives



## Outcome

- Open constraint. Any change to the set of secret files must touch both `guard-secrets-read-utils.mjs` and `guard-secrets-bash-utils.mjs`; verify list parity when either changes.
