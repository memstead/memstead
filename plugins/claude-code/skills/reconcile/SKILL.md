---
name: reconcile
description: Sync the knowledge graph to code changes since the last reconcile — read-only on the code, write-only on the graph, commits nothing itself.
allowed-tools: Bash(git status:*), Bash(git diff:*), Bash(git log:*), Bash(git rev-parse:*), Read, Glob, Grep, Write, mcp__memstead__*
---

# /reconcile — Sync the graph to code changes (commits nothing)

Keep the knowledge graph fresh against the code it describes: read what changed in the source since the last reconcile, and update the affected graph entities to match. Reconcile is **read-only on the code, write-only on the graph, and commits nothing itself** — your code commits stay yours (commit them through your normal flow), and graph mutations are committed per-mutation by the engine's auto-commit path. Reconcile's only git use is *reading* (`git status`/`git diff`/`git log`/`git rev-parse`); it never `git add`s or `git commit`s.

## Scope: code-bound vaults only

Reconcile syncs a vault **against a code source** — it applies to code-bound vaults whose entities describe a codebase (specs, contracts, decisions realized in source). A **knowledge-only vault** (no code source — research, worldbuilding, personal knowledge) has nothing to reconcile *from*: refresh it by ingesting new material (`/ingest`) or re-interviewing (`/interview`), not by reconcile. If the target vault has no code source, say so and stop — do not attempt a code-source scan against it.

## Phase 1: Discover vaults and sources

1. **Discover vaults from the engine — not from `.mcp.json`.** The engine is the source of truth for which vaults are mounted and where their files live; it may be launched any way (e.g. `cd graph && exec memstead-mcp`, with no `--vault` arguments at all), so parsing the MCP launch command is unreliable. Call `memstead_overview` (the `## Vaults` list) or `memstead_health { include_config: true }` and take the vault set from there.

2. **Determine where each vault's entity files live (backend-dependent).** This decides whether the source scan needs to exclude anything. Resolve each vault's gitdir via `memstead_health { include_config: true }` → `vaults[].vcs.gitdir`.
   - **git-branch backend** — the vault is a branch inside a nested `vault-repo/` gitdir, itself gitignored by the outer project repo. Its entity `.md` files never appear in the outer `git status`. **Nothing to exclude.**
   - **folder backend** — the vault is a directory of `.md` files in the outer working tree. Its entity files *would* show up in the source scan and must be excluded.

3. **Enumerate source repos.** A source repo is any directory containing a `.git/` folder. Today's layout is one outer project repo with the git-branch `vault-repo/` nested inside it and gitignored; stay correct for future multi-repo or folder-backend workspaces.

## Phase 2: Find what changed since the last sync (the per-source cursor)

Reconcile updates only what changed **since the last time it synced this source** — never the whole tree every run. That "last time" is a **per-source sync cursor**: the source commit SHA reconcile last synced from, recorded per vault-source pair.

1. **Read the cursor.** Look for `.memstead/reconcile-cursors.json` — reconcile's own bookkeeping (not graph state, not engine state). Shape: `{ "<vault>:<repo-path>": "<last-synced-commit-sha>", ... }`. A missing file or a missing pair means this is a first sync for that source.

2. **Compute the changed set per source repo.** Always exclude reconcile's own bookkeeping and config from the source scan — add `':!.memstead/**'`, plus `':!<vault-dir>/**/*.md'` for any *folder-backend* vault under that repo. **Quote every pathspec in single quotes** so the shell doesn't expand `!`.
   - **Have a cursor:** the changed source since last sync is
     ```bash
     git diff --stat <cursor-sha>..HEAD -- ':!.memstead/**' ':!<vaultDir>/**/*.md'
     git diff        <cursor-sha>..HEAD -- ':!.memstead/**' ':!<vaultDir>/**/*.md'
     ```
   - **First sync (no cursor):** there is no prior baseline, so do **not** replay all of history. Take the baseline as the current `git rev-parse HEAD`, and sync the entities the *current* realization state affects (use the realization-path signals + `memstead_search` in Phase 3), then record the cursor. Tell the user this is the first sync for this source.

3. **Skip any source with no change since its cursor.** If nothing changed anywhere, tell the user and stop. (No cursor advances on a no-op.)

## Phase 3: Update the affected entities (write-only on the graph)

For each source with changes since its cursor:

1. **Think about what changed.** Read the changed diff. Which parts of the system were modified? What behaviour changed? What was added or removed?

2. **Find affected entities.** Prefer signals the `check-realization` PostToolUse hook ([hooks/check-realization.mjs](../../hooks/check-realization.mjs)) already emitted earlier in the session — it scans entity realization paths on every `Write`/`Edit` and prints `REALIZATION EDIT: … these entities reference it: <ids>`. When those IDs cover the changed files, use them directly. For gaps, fall back to `memstead_search` with keywords from the changed code — function names, module names, concepts — and realization-path search.

3. **Read each candidate entity** with `memstead_entity` (you need the `_hash` for updates). `_hash` stays valid across subsequent `memstead_relate` calls, so read-once/update/relate saves roundtrips.

4. **Update affected entities** via `memstead_update`:
   - `sections: { "specifies": "…" }` — if code behaviour, API, or data structures changed
   - `sections: { "constraints": "…" }` — if invariants, limits, or rules changed
   - `append_sections: { "rationale": "…" }` — append the *reasoning* for the change (why this approach, which trade-offs), derived from the diff. Rationale is **not** a changelog — never `[commit <hash>]` log-style entries.

   If `memstead_update` returns `HASH_MISMATCH`, call it again with `dry_run: true` (no `expected_hash` required in dry-run). The response carries the current on-disk hash as `content_hash` — pass that as `expected_hash` on the real retry. A mismatch means someone else's write slipped in between your read and your update; don't force-overwrite without a dry-run look.

5. **Check for new relationships.** If the change introduces a new import, dependency, or realization, add the edge with `memstead_relate`. Detection hint: scan the diff for added `use` / `import` / `require` / `include` lines, map them to existing entities, compare against the entity's current edge set. Canonical edge types: `DEPENDS_ON`, `USES`, `REALIZES` — whichever fits the schema. (`REFERENCES` is engine-emitted from body wiki-links via the alias-synthesis pass — never explicitly authored.)

   **Edge removal is out of scope.** A change that drops a dependency leaves the matching edge intact. Removals are ambiguous (temporary refactor vs. permanent cut), and a stale edge is less damaging than an erased real one. `/audit` and `/maintain` flag these later.

6. **Be conservative:**
   - If unsure whether an entity is affected — skip it.
   - Do NOT create new entities unless the change clearly introduces a new concept with no existing entity.
   - Do NOT delete entities unless the change removes the concept entirely.
   - Do NOT rewrite sections that haven't changed — only what the change actually affects.
   - Do NOT add speculative edges — only relationships the diff literally introduces.

These MCP mutations are committed per-mutation by the engine to each vault's gitdir — reconcile does not commit them.

## Phase 4: Advance the cursor + summarize

1. **Advance the cursor.** Write `.memstead/reconcile-cursors.json`, setting each synced `<vault>:<repo-path>` to that source's current `git rev-parse HEAD`. This file is reconcile's only on-disk write outside the graph, and it is **not committed** — it is local bookkeeping; keep it out of your code commits (it is excluded from the source scan above, and belongs in `.gitignore`).

2. **Report:**
   - Which entities were updated and what changed.
   - The commit each source's cursor advanced to.
   - Any entities flagged for attention but skipped (for the user).
   - **Reconcile committed nothing** — your code changes are still uncommitted for you to commit through your normal flow; graph changes were committed per-mutation by the engine.

## Creating and removing vaults

Vault lifecycle is a workspace-policy operation, not part of the reconcile flow. The engine exposes two MCP tools — `memstead_vault_create` and `memstead_vault_delete` — gated by `[[vault_management.create]]` / `[[vault_management.delete]]` rules in `.memstead/workspace.toml` (each rule carries a glob `pattern` over the composed `<path>/<name>` candidate, plus per-rule `schemas[]` on the create side). When neither array is present or both are empty, both tools return `VAULT_PATH_NOT_ALLOWED` for every input — the default posture is conservative, and the workspace operator opts in explicitly per namespace.

Discover the live lifecycle policy via `memstead_overview`'s `## Lifecycle Namespaces` section (each rule lists `pattern`, `actions`, and the create-side `schemas`). Each writable vault's `origin` (`explicit`, `discovered`, or `runtime_created`) ships under `memstead_health { include_config: true }`. Reconcile itself never calls these lifecycle tools — vault creation and deletion are intentional acts by a dedicated skill or the user.

## Rules

- **Commits nothing.** Reconcile never runs `git add`/`git commit` — not for code, not for the graph. Code commits are the developer's; graph commits are the engine/auto-commit hook's. Reconcile's only git use is *reading* (`git status`/`git diff`/`git log`/`git rev-parse`).
- Never edit spec `.md` files directly — always use MCP tools.
- Spec changes are committed **per-mutation** by the engine to each vault's own gitdir (default `<vault>/.git/`, or a shared-gitdir path when configured — resolve the canonical path via `memstead_health { include_config: true }` and read `vaults[].vcs.gitdir`) — not debounced, not batched. Entity `.md` files in the *outer* workspace repo stay dirty until the outer-repo auto-commit lands or `/commit` runs manually. Reconcile does not stage or commit them.
- The cursor file (`.memstead/reconcile-cursors.json`) is reconcile's own bookkeeping, not graph state — keep it out of your code commits and `.gitignore` it.
