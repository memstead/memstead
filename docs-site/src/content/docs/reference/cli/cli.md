---
title: "CLI (`memstead`)"
---

# Command-Line Help for `memstead`

This document contains the help content for the `memstead` command-line program.

**Command Overview:**

* [`memstead`↴](#memstead)
* [`memstead status`↴](#memstead-status)
* [`memstead entity`↴](#memstead-entity)
* [`memstead relations`↴](#memstead-relations)
* [`memstead search`↴](#memstead-search)
* [`memstead list`↴](#memstead-list)
* [`memstead context`↴](#memstead-context)
* [`memstead overview`↴](#memstead-overview)
* [`memstead type`↴](#memstead-type)
* [`memstead health`↴](#memstead-health)
* [`memstead export`↴](#memstead-export)
* [`memstead init`↴](#memstead-init)
* [`memstead quickstart`↴](#memstead-quickstart)
* [`memstead install`↴](#memstead-install)
* [`memstead link`↴](#memstead-link)
* [`memstead publish`↴](#memstead-publish)
* [`memstead unpublish`↴](#memstead-unpublish)
* [`memstead domain`↴](#memstead-domain)
* [`memstead domain keygen`↴](#memstead-domain-keygen)
* [`memstead domain manifest`↴](#memstead-domain-manifest)
* [`memstead admin`↴](#memstead-admin)
* [`memstead admin takedown`↴](#memstead-admin-takedown)
* [`memstead admin denylist`↴](#memstead-admin-denylist)
* [`memstead login`↴](#memstead-login)
* [`memstead logout`↴](#memstead-logout)
* [`memstead create`↴](#memstead-create)
* [`memstead update`↴](#memstead-update)
* [`memstead relate`↴](#memstead-relate)
* [`memstead delete`↴](#memstead-delete)
* [`memstead rename`↴](#memstead-rename)
* [`memstead batch-update`↴](#memstead-batch-update)
* [`memstead recover`↴](#memstead-recover)
* [`memstead changes`↴](#memstead-changes)
* [`memstead reload`↴](#memstead-reload)
* [`memstead fetch`↴](#memstead-fetch)
* [`memstead pull`↴](#memstead-pull)
* [`memstead push`↴](#memstead-push)
* [`memstead branch-reset`↴](#memstead-branch-reset)
* [`memstead mem`↴](#memstead-mem)
* [`memstead mem init`↴](#memstead-mem-init)
* [`memstead mem unregister`↴](#memstead-mem-unregister)
* [`memstead mem delete`↴](#memstead-mem-delete)
* [`memstead mem set-version`↴](#memstead-mem-set-version)
* [`memstead mem set-schema`↴](#memstead-mem-set-schema)
* [`memstead mem set-description`↴](#memstead-mem-set-description)
* [`memstead mem set-sync-state`↴](#memstead-mem-set-sync-state)
* [`memstead mem set-internal`↴](#memstead-mem-set-internal)
* [`memstead mem list`↴](#memstead-mem-list)
* [`memstead mem-repo`↴](#memstead-mem-repo)
* [`memstead mem-repo init`↴](#memstead-mem-repo-init)
* [`memstead mem-repo remote-add`↴](#memstead-mem-repo-remote-add)
* [`memstead workspace`↴](#memstead-workspace)
* [`memstead workspace dump`↴](#memstead-workspace-dump)
* [`memstead workspace show`↴](#memstead-workspace-show)
* [`memstead workspace allow-create`↴](#memstead-workspace-allow-create)
* [`memstead workspace revoke-create`↴](#memstead-workspace-revoke-create)
* [`memstead workspace allow-delete`↴](#memstead-workspace-allow-delete)
* [`memstead workspace revoke-delete`↴](#memstead-workspace-revoke-delete)
* [`memstead workspace grant-cross-link`↴](#memstead-workspace-grant-cross-link)
* [`memstead workspace revoke-cross-link`↴](#memstead-workspace-revoke-cross-link)
* [`memstead workspace set-mutations`↴](#memstead-workspace-set-mutations)
* [`memstead schema`↴](#memstead-schema)
* [`memstead schema new`↴](#memstead-schema-new)
* [`memstead schema validate`↴](#memstead-schema-validate)
* [`memstead schema install`↴](#memstead-schema-install)
* [`memstead pipeline`↴](#memstead-pipeline)
* [`memstead pipeline migrate`↴](#memstead-pipeline-migrate)
* [`memstead projection`↴](#memstead-projection)
* [`memstead projection init`↴](#memstead-projection-init)
* [`memstead projection migrate`↴](#memstead-projection-migrate)
* [`memstead projection enable`↴](#memstead-projection-enable)
* [`memstead projection advance`↴](#memstead-projection-advance)
* [`memstead ingest`↴](#memstead-ingest)
* [`memstead ingest brief`↴](#memstead-ingest-brief)

## `memstead`

Command-line interface for Memstead — query and mutate typed entity graphs from the shell. Default build produces the full `memstead` binary (multi-mem, git-backed); `--no-default-features` builds the lean folder-only surface.

**Usage:** `memstead [OPTIONS] <COMMAND>`

Exit codes:
  0  success
  1  generic failure (catch-all for non-classified errors)
  2  usage error (clap argument-parse failure — unknown flag, bad value)
  3  not found (entity / mem / resource missing)
  4  hash mismatch (optimistic-locking failure on a mutation)
  5  validation / schema / policy refusal

  For programmatic branching, prefer `--json` over the exit code:
    memstead &lt;subcommand> ... --json | jq -r .code
  The JSON envelope's `code` field carries the typed token
  (e.g. INVALID_TITLE, HAS_INCOMING_REFS, CROSS_MEM_LINK_NOT_ALLOWED)
  with structured recovery details under `.details`.

###### **Subcommands:**

* `status` — Node / edge counts, schema distribution, and per-binding projection state
* `entity` — Read one entity as markdown
* `relations` — List typed edges for an entity
* `search` — Find entities by text or graph proximity
* `list` — Filter entities by metadata (no text match — use `search` for that)
* `context` — Read an entity's community cluster
* `overview` — All clusters with summaries and member lists. The full build renders the same rich content the MCP `memstead_overview` tool emits — both surfaces share the engine composer in `memstead-engine`
* `type` — Describe one type, or list all types when no name given
* `health` — Health summary (orphans, stubs, stale entities, missing fields)
* `export` — Export the write mem as markdown (in place) or as a portable `.mem` archive
* `init` — Initialise a filesystem mem in the current (or named) folder. Strict: errors out when the target is not empty
* `quickstart` — One-command cold start: workspace + default-schema mem + seed entity + MCP wiring for your agent(s), in the current (or named) folder. Tolerates dotfiles and README-grade files; derives the mem name from the folder. For the strict, script-safe variant use `memstead init`
* `install` — Install a sealed `.mem` mem — either a local file, or `<scope>/<name>` from the memstead.io registry
* `link` — Link a filesystem mem to a registry-published dependency. `memstead link <scope/name>` fetches the archive into the workspace and records the dependency in the workspace config
* `publish` — Publish a `.mem` archive to the registry. Triggers GitHub Device Flow on first use; subsequent runs are silent
* `unpublish` — Unpublish (hard-delete) `<scope>/<name>` from the registry. Permitted to the original uploader and to admins. The same `<scope>/<name>` becomes immediately re-publishable
* `domain` — Domain-authority publishing: generate the signing key for a domain you control and print the `.well-known` manifest to host. `publish --scope <domain>:<handle>` then signs with that key — no GitHub account needed
* `admin` — Admin-only registry moderation: take a mem down or deny-list bytes. Gated server-side by the `MEMSTEAD_ADMINS` allowlist; every action is recorded in the registry's append-only audit log
* `login` — Authenticate with a registry via GitHub Device Flow. Optional — `publish` auto-triggers the same flow on first use
* `logout` — Remove stored credentials for a registry
* `create` — Create a new entity. Provide `--title`, `--type`, and the required section fields, or pass `--from <file.json>` with the full payload
* `update` — Modify an existing entity. `--expected-hash` is required unless `--auto-hash` (refetch before write) or `--force` (skip check) is given
* `relate` — Add or remove a typed relationship between two entities
* `delete` — Delete an entity. Use `--dry-run` to preview impact first. Delete is hashless by design (no post-state to race on); race protection comes from `HAS_INCOMING_REFS` — and `RESIDUAL_STUB_FOR_READONLY_REFERRERS` for read-only-referrer cases
* `rename` — Rename an entity (changes ID, file path, and every incoming wiki-link)
* `batch-update` — Update many entities in one atomic call. Input is a JSON file with a top-level `updates: [...]` array (one entry per entity, each with its own hash mode and mutation fields). All-or-nothing: if any entry fails (validation, hash mismatch, missing entity) the whole batch is refused and NOTHING is committed — fix the named entry and resubmit. On success the batch lands as one commit. Mirrors `memstead update` per entry
* `recover` — Apply parse-time-drift recovery across writable mems. Walks `PARSED_RELATION_INVALID` warnings, re-renders affected source entities to drop the stale rows, and reports per-entry outcomes. Read-only-origin drops surface as skipped
* `changes` — Diff a mem's HEAD against a commit SHA. Pass `--since` = a prior `commit_sha` from a mutation, or the canonical empty-tree hash `4b825dc642cb6eb9a060e54bf8d69288fbee4904` for a first sync
* `reload` — Reload one writable mem's slice of the in-memory store from its on-disk branch tip — or every writable mem when `--mem` is omitted. CLI parity with the MCP `memstead_reload` tool
* `fetch` — Fetch a mem's branch refs from a git remote into the mem-repo (no local branch moves — inspect first, then `pull`). Requires a git-branch-backed mem (`INVALID_INPUT` on folder mounts); refuses `UNKNOWN_REMOTE` when the remote is not configured
* `pull` — Fast-forward a mem's branch to its fetched remote counterpart and reload the in-memory store. Refuses `LOCAL_DIVERGENCE` when the local branch is not an ancestor of the remote — reconcile via `branch-reset`, or resolve on another clone and push
* `push` — Push a mem's branch to a git remote. `--force` uses force-with-lease semantics; without it, non-fast-forward pushes refuse (`NON_FAST_FORWARD`). Refuses `UNKNOWN_REMOTE` when the remote is not configured
* `branch-reset` — Reset a mem's branch pointer to a target ref/SHA. Refuses to discard commits reachable from any remote ref (`PUSHED_COMMITS_PROTECTED`)
* `mem` — Mem lifecycle commands
* `mem-repo` — Mem-repo-git lifecycle commands
* `workspace` — Introspect and configure workspace policy — `dump` reads the effective config; `allow-create`/`revoke-create`/`allow-delete`/ `revoke-delete`/`grant-cross-link`/`revoke-cross-link`/`set-mutations` write the mem-lifecycle allowlist, cross-mem link grants, and mutation policy
* `schema` — Author-time schema tooling. `memstead schema validate <path>` checks a schema package directory against the engine's loader without touching a workspace
* `pipeline` — Pipeline-config tooling. `memstead pipeline migrate` converts the legacy `scopes|projections|ingests/` JSON folders into the `.memstead/` workspace store's four-primitive shape
* `projection` — Binding (projection-promotion) tooling. `memstead projection init` scaffolds a fresh v1 binding non-interactively; `memstead projection migrate` promotes gen-2 four-primitive configs (per-mem projection + flat ingest) into v1 bindings; `memstead projection enable <build|sync|verify> <binding>` adds a missing operation block — one versioned record per source→mem obligation, with an `operations { build, sync, verify }` block
* `ingest` — Engine-side ingest orchestration. `memstead ingest brief <name>` renders an ingest's run-brief — the Markdown prompt an agent consumes — from the four-primitive config and the destination mem's schema / writing guidance

###### **Options:**

* `--json` — Emit JSON instead of markdown. Matches MCP `structured_content` shape
* `--quiet` — Suppress engine startup logs on stderr



## `memstead status`

Node / edge counts, schema distribution, and per-binding projection state

**Usage:** `memstead status`



## `memstead entity`

Read one entity as markdown

**Usage:** `memstead entity [OPTIONS] <ID>`

###### **Arguments:**

* `<ID>` — Entity ID (e.g. `specs--my-entity`)

###### **Options:**

* `--section <KEY>` — Restrict output to specific section keys (repeatable)
* `--include-relations` — Append relations as a trailing JSON code block
* `--token-budget <TOKEN_BUDGET>` — Token budget for chunking. Omit for no chunking
* `--chunk <CHUNK>` — 1-based chunk index to return (requires `--token-budget`)



## `memstead relations`

List typed edges for an entity

**Usage:** `memstead relations <ID>`

###### **Arguments:**

* `<ID>` — Entity ID



## `memstead search`

Find entities by text or graph proximity

**Usage:** `memstead search [OPTIONS] [TEXT]`

Filter surface:
  Named-flag shortcuts (frozen — no new ones are added; use --filter
  for any other schema-declared filterable field):
    --type &lt;T>          Filter by entity_type (engine first-class axis).
    --level &lt;L>         Filter by level (e.g. M0, M1).
    --status &lt;S>        Filter by status (e.g. active, closed).
    --edge-type &lt;E>     Filter by edge type (engine first-class axis).

  Generic equality filter:
    --filter KEY=VALUE  Filter by any schema-declared `filterable: equality`
                        field. Repeatable. Examples:
                          --filter tags=auth
                          --filter scope=subsystem
                          --filter confidence=high
                          --filter tags=auth --filter level=M0
  Unknown keys are silently dropped by the engine and surface as a
  warning. Named-flag shortcuts and `--filter` populate the same
  underlying filter map; if both set the same key, `--filter` wins
  (declared last in the iteration order).

###### **Arguments:**

* `<TEXT>` — Free-text query. Omit for a pure structural filter

###### **Options:**

* `--mem <MEM>`
* `--type <ENTITY_TYPE>`
* `--field <FIELD>` — Restrict text matching to a single field (title or section key). Maps to `Query.field` — narrows `any`, `not`, and `phrase` for the query. Replaces the former repeatable plural form, which was orphaned at the engine level
* `--exclude <TOKEN>` — Exclude entities whose text matches this token. Repeatable — `--exclude OAuth --exclude SAML` drops every hit driven by either. Maps to `Query.not`. When combined with `--field`, the exclude scopes to that field via the engine's existing `Query.field` semantics.

   Example: `memstead search auth --exclude OAuth` returns "auth"-matching entities that are not driven by an `OAuth` match.
* `--phrase <TEXT>` — Restrict hits to entities containing this exact phrase (adjacency-sensitive). Maps to `Query.phrase`. Composable with `--field` (narrows the phrase match to one field) and `--exclude` (drops phrase-matching hits that also match the excluded token). Shell quoting is stripped before the binary sees the positional text argument — use this flag rather than quoting in the positional to express adjacency
* `--edge-type <EDGE_TYPE>` — Filter by edge type (e.g. USES, IMPLEMENTS)
* `--related-to <RELATED_TO>` — Only entities within `--depth` hops of this ID
* `--depth <DEPTH>`
* `--limit <LIMIT>`
* `--offset <OFFSET>`
* `--level <LEVEL>`
* `--status <STATUS>`
* `--filter <KEY=VALUE>` — Equality filter on any schema-declared filterable field: repeatable `--filter KEY=VALUE`. The four named-flag shortcuts (`--type` / `--level` / `--status` / `--edge-type`) handle their common cases; every other `filterable: equality` field (e.g. `tags`, `scope`) is reachable via this generic flag. Unknown keys are dropped and surface as engine warnings. There is no `--confidence` shortcut: a field reached only when a schema declares it goes through `--filter <field>=<value>` rather than a dedicated flag
* `--stub` — Return only stub entities (conflicts with --no-stub)
* `--no-stub` — Return only real (non-stub) entities (conflicts with --stub)



## `memstead list`

Filter entities by metadata (no text match — use `search` for that)

**Usage:** `memstead list [OPTIONS]`

Filter surface:
  Named-flag shortcuts (frozen — no new ones are added; use --filter
  for any other schema-declared filterable field):
    --type &lt;T>          Filter by entity_type (engine first-class axis).
    --level &lt;L>         Filter by level (e.g. M0, M1).
    --status &lt;S>        Filter by status (e.g. active, closed).
    --edge-type &lt;E>     Filter by edge type (engine first-class axis).

  Generic equality filter:
    --filter KEY=VALUE  Filter by any schema-declared `filterable: equality`
                        field. Repeatable. Examples:
                          --filter tags=auth
                          --filter scope=subsystem
                          --filter confidence=high
                          --filter tags=auth --filter level=M0
  Unknown keys are silently dropped by the engine and surface as a
  warning. Named-flag shortcuts and `--filter` populate the same
  underlying filter map; if both set the same key, `--filter` wins
  (declared last in the iteration order).

###### **Options:**

* `--mem <MEM>`
* `--type <ENTITY_TYPE>`
* `--level <LEVEL>`
* `--status <STATUS>`
* `--edge-type <EDGE_TYPE>`
* `--filter <KEY=VALUE>` — Equality filter on any schema-declared filterable field: repeatable `--filter KEY=VALUE`. The four named-flag shortcuts (`--type` / `--level` / `--status` / `--edge-type`) handle their common cases; every other `filterable: equality` field (e.g. `tags`, `scope`) is reachable via this generic flag. Unknown keys are dropped and surface as engine warnings. There is no `--confidence` shortcut: a field reached only when a schema declares it goes through `--filter <field>=<value>` rather than a dedicated flag
* `--limit <LIMIT>`
* `--offset <OFFSET>`



## `memstead context`

Read an entity's community cluster

**Usage:** `memstead context [OPTIONS] <ID_OR_QUERY>`

###### **Arguments:**

* `<ID_OR_QUERY>` — Entity ID (exact match preferred) or search text (fallback)

###### **Options:**

* `--chunk <CHUNK>` — 1-based chunk index for large contexts



## `memstead overview`

All clusters with summaries and member lists. The full build renders the same rich content the MCP `memstead_overview` tool emits — both surfaces share the engine composer in `memstead-engine`

**Usage:** `memstead overview [OPTIONS]`

###### **Options:**

* `--rebuild` — Re-run Louvain community detection before rendering
* `--chunk <CHUNK>` — 1-based chunk index for large overviews
* `--mem <MEM>` — Scope schemas + mem inventory to any single visible mem (read-only mounts included)
* `--include <KEY>` — Opt heavy content into the response: `community_members`, `community_bridges`, `mem_distribution`, `dangling_links`. Keys listed here are always included even past `token_budget`; keys omitted may surface in the `Hints` section instead. Repeatable (`--include K --include K`) AND comma-string (`--include K1,K2`) forms both parse — uniform with `memstead health --include`. Unknown keys emit `UNKNOWN_INCLUDE_KEY` warnings
* `--token-budget <N>` — Token budget for heavy content only (`community_members`, `community_bridges`, `mem_distribution`, `dangling_links`). Hard-required content (mem roster, schema refs, community titles, workspace policy) always ships in addition — total response size will exceed this budget. Default 8000 (matches the MCP tool). Budgets below ~10 tokens are safe but unproductive — the response still arrives as a structured envelope (`_overview_mode: overbudget`), but no useful chunking happens and the full body ships as one chunk



## `memstead type`

Describe one type, or list all types when no name given

**Usage:** `memstead type [OPTIONS] [NAME]`

###### **Arguments:**

* `<NAME>`

###### **Options:**

* `--mem <MEM>` — Resolve the schema from this writable mem's pin. Required when the workspace has more than one writable mem; defaults to the lone writable mem otherwise



## `memstead health`

Health summary (orphans, stubs, stale entities, missing fields)

**Usage:** `memstead health [OPTIONS]`

###### **Options:**

* `--include <INCLUDE>` — Opt heavy content into the response: orphans, stubs, most_connected, missing_fields, stale, dangling_links, tags, missing_required_outgoing, conformance, integrity. `conformance` lints every entity against the effective schema into a `findings` array (write-time typed codes); `integrity` adds the consistency axis (dangling links, stubs) to the same list. Repeatable (`--include K --include K`) AND comma-string (`--include K1,K2`) forms both parse — uniform with `memstead overview --include`
* `--target-schema <TARGET_SCHEMA>` — Schema ref (`name@x.y.z`) the conformance/integrity includes lint against instead of each mem's current pin
* `--limit <LIMIT>` — Max rows for `most_connected` and `tag_distribution` (default: 10)

  Default value: `10`
* `--strict` — Exit non-zero (1) when any included Tier-2 warning kind has present violations. The output is rendered first, then the non-zero exit fires. Today only `missing_required_outgoing` participates; new Tier-2 codes opt in additively without breaking the flag's semantics. With no Tier-2 `--include` token, `--strict` is a no-op



## `memstead export`

Export the write mem as markdown (in place) or as a portable `.mem` archive

**Usage:** `memstead export [OPTIONS]`

###### **Options:**

* `--format <FORMAT>` — Output format. `markdown` regenerates the mem directory in place (folder-backed mems only); `mem` writes a portable `.mem` zip suitable for sharing (every backend)

  Default value: `markdown`

  Possible values:
  - `markdown`:
    Regenerate markdown files in place
  - `mem`:
    Write a `.mem` zip archive to `--output`

* `-o`, `--output <PATH>` — Output path for `--format mem`. Defaults to `./<name>-<version>.mem` in the current directory, matching the "external vs cache filename" convention for portable mem archives. Ignored for `--format markdown`
* `--mem <NAME>` — Which mem to export (by name). For `--format markdown`, omitting this argument runs a workspace-wide export and reports any declined mounts under `skipped_mounts`. For `--format mem`, required when more than one write mem is loaded; defaults to the first writable mem otherwise



## `memstead init`

Initialise a filesystem mem in the current (or named) folder. Strict: errors out when the target is not empty

**Usage:** `memstead init --name <NAME> --schema <SCHEMA> [PATH]`

###### **Arguments:**

* `<PATH>` — Target folder. Defaults to the current working directory

###### **Options:**

* `--name <NAME>` — Mem name. Slug-shaped: `^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$`
* `--schema <SCHEMA>` — Schema pin in exact `<name>@<version>` form (e.g. `default@1.0.0`). Bare-name pins are rejected. filesystem-mem v1 resolves against the engine's builtin schema set; registry-resolved schemas land in a follow-up



## `memstead quickstart`

One-command cold start: workspace + default-schema mem + seed entity + MCP wiring for your agent(s), in the current (or named) folder. Tolerates dotfiles and README-grade files; derives the mem name from the folder. For the strict, script-safe variant use `memstead init`

**Usage:** `memstead quickstart [OPTIONS] [PATH]`

###### **Arguments:**

* `<PATH>` — Target folder. Defaults to the current working directory

###### **Options:**

* `--name <NAME>` — Mem name. Normally derived from the directory name; pass this when the derivation fails (or to override it). Slug-shaped: `^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$`
* `--agent <AGENTS>` — Agent target(s) to write MCP wiring for. Repeatable. Skips the interactive selection prompt. Without a TTY and without this flag, quickstart defaults to `claude-code`

  Possible values:
  - `claude-code`:
    Claude Code — project `.mcp.json`
  - `codex`:
    OpenAI Codex — prints the `codex mcp add` one-liner (Codex has no project-scoped MCP config file)
  - `cursor`:
    Cursor — project `.cursor/mcp.json`
  - `gemini`:
    Gemini CLI — project `.gemini/settings.json`




## `memstead install`

Install a sealed `.mem` mem — either a local file, or `<scope>/<name>` from the memstead.io registry

**Usage:** `memstead install [OPTIONS] <PATH or SCOPE/NAME>`

###### **Arguments:**

* `<PATH or SCOPE/NAME>` — Either a path to a `.mem` file, or `<scope>/<name>` for registry installs (no `@` prefix)

###### **Options:**

* `--mem <NAME>` — Which writable mem to register this read-mem into (by name). Defaults to the first writable mem when omitted.

   This flag selects the *host* mem — the writable workspace mem that will list the archive in its read-mems set. It does NOT rename the archive's internal mem; the archive's internal name is the canonical identity used by all cross-mem references and shadow checks.
* `--registry <URL>` — Registry URL for `<scope>/<name>` installs. Ignored for local paths. Overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io



## `memstead link`

Link a filesystem mem to a registry-published dependency. `memstead link <scope/name>` fetches the archive into the workspace and records the dependency in the workspace config

**Usage:** `memstead link [OPTIONS] <SCOPE/NAME>`

###### **Arguments:**

* `<SCOPE/NAME>` — Cross-mem dependency in `scope/name` form (no `@` prefix — that is the `memstead install` shape). Tier 3 wiki-links use the same form, so the input here matches what users will type inside `[[scope/name:slug]]`

###### **Options:**

* `--registry <URL>` — Override the registry URL. Falls back to `MEMSTEAD_REGISTRY` then the default `https://memstead.io`
* `--workspace <PATH>` — Override the workspace root. When omitted, the command walks up from the current working directory to find it



## `memstead publish`

Publish a `.mem` archive to the registry. Triggers GitHub Device Flow on first use; subsequent runs are silent

**Usage:** `memstead publish [OPTIONS] [PATH]`

###### **Arguments:**

* `<PATH>` — Path to a `.mem` archive on disk. Omit to assemble the archive from the surrounding filesystem-mem workspace (walks up from cwd to find the workspace root)

###### **Options:**

* `--workspace <PATH>` — Override the workspace root for the no-arg / `--mem` shapes. Ignored when an archive PATH is provided. Defaults to walking up from cwd
* `--mem <NAME>` — Export-and-publish a named mem from the current workspace in one step — the path for mem-repo (multi-mem, git-branch) workspaces, which have no folder to wrap up. Ignored when an archive PATH is provided. A single-mem folder workspace can omit this and just run `memstead publish`
* `--scope <NAME>` — Override the auto-derived scope — admin-only, reserved scopes only (currently just `memstead`). Without this flag the registry stores the mem under your GitHub username
* `--token <TOKEN>` — Explicit token override. Takes precedence over `MEMSTEAD_TOKEN` and stored credentials
* `--registry <URL>` — Registry URL (overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io)
* `--version <SEMVER>` — Set the mem's version to this semver and publish in one step, persisting the bump to the mem config (like `npm version` + `npm publish`). Requires `--mem <name>`; not valid with a pre-built archive PATH, whose version is already baked in. Omit to publish whatever version the mem config currently carries
* `--dry-run` — Assemble and resolve everything, print exactly what would be published (mem, version, scope, archive size), but POST nothing and mutate nothing — including no version bump. The safe way to confirm a publish before it goes out



## `memstead unpublish`

Unpublish (hard-delete) `<scope>/<name>` from the registry. Permitted to the original uploader and to admins. The same `<scope>/<name>` becomes immediately re-publishable

**Usage:** `memstead unpublish [OPTIONS] <SCOPE/NAME>`

###### **Arguments:**

* `<SCOPE/NAME>` — `<scope>/<name>` of the mem to unpublish

###### **Options:**

* `--token <TOKEN>` — Explicit token override. Takes precedence over `MEMSTEAD_TOKEN` and stored credentials
* `--registry <URL>` — Registry URL (overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io)



## `memstead domain`

Domain-authority publishing: generate the signing key for a domain you control and print the `.well-known` manifest to host. `publish --scope <domain>:<handle>` then signs with that key — no GitHub account needed

**Usage:** `memstead domain <COMMAND>`

###### **Subcommands:**

* `keygen` — Generate a signing keypair for a domain and print the manifest to host
* `manifest` — Re-print the `.well-known` manifest for a domain's existing key



## `memstead domain keygen`

Generate a signing keypair for a domain and print the manifest to host

**Usage:** `memstead domain keygen [OPTIONS] --domain <DOMAIN> --contact <EMAIL_OR_URI>`

###### **Options:**

* `--domain <DOMAIN>` — The domain you control, e.g. `acme.com`. Mems publish under `<domain>:<handle>`
* `--contact <EMAIL_OR_URI>` — Abuse / ownership contact (email or URI). Repeatable; at least one is required — a takedown notice must be able to reach you
* `--force` — Replace an existing key for this domain (rotation). The hosted manifest must then be updated to the new public key



## `memstead domain manifest`

Re-print the `.well-known` manifest for a domain's existing key

**Usage:** `memstead domain manifest --domain <DOMAIN> --contact <EMAIL_OR_URI>`

###### **Options:**

* `--domain <DOMAIN>` — The domain whose stored key to render a manifest for
* `--contact <EMAIL_OR_URI>` — Abuse / ownership contact (email or URI). Repeatable; at least one is required



## `memstead admin`

Admin-only registry moderation: take a mem down or deny-list bytes. Gated server-side by the `MEMSTEAD_ADMINS` allowlist; every action is recorded in the registry's append-only audit log

**Usage:** `memstead admin <COMMAND>`

###### **Subcommands:**

* `takedown` — Take down a published mem (admin-only): deny-list its bytes, tombstone every version, and burn the `<scope>/<name>` so neither the bytes nor the name can be re-published. The notice reference is recorded as the DSA statement-of-reasons in the audit log
* `denylist` — Add a canonical-bytes SHA-256 to the content deny-list (admin-only) so a publish of exactly those bytes is refused — even before they are ever uploaded



## `memstead admin takedown`

Take down a published mem (admin-only): deny-list its bytes, tombstone every version, and burn the `<scope>/<name>` so neither the bytes nor the name can be re-published. The notice reference is recorded as the DSA statement-of-reasons in the audit log

**Usage:** `memstead admin takedown [OPTIONS] --notice <REF> <SCOPE/NAME>`

###### **Arguments:**

* `<SCOPE/NAME>` — `<scope>/<name>` of the mem to take down (e.g. `github:alice/my-mem`)

###### **Options:**

* `--notice <REF>` — Statement-of-reasons / notice reference recorded with the action (e.g. an abuse-ticket id or legal-notice ref). Required so the audit log can justify the takedown
* `--token <TOKEN>` — Explicit token override. Takes precedence over `MEMSTEAD_TOKEN` and stored credentials
* `--registry <URL>` — Registry URL (overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io)



## `memstead admin denylist`

Add a canonical-bytes SHA-256 to the content deny-list (admin-only) so a publish of exactly those bytes is refused — even before they are ever uploaded

**Usage:** `memstead admin denylist [OPTIONS] <SHA256>`

###### **Arguments:**

* `<SHA256>` — Canonical-bytes SHA-256 (64 hex chars) to block

###### **Options:**

* `--reason <TEXT>` — Free-text reason recorded on the deny-list row
* `--token <TOKEN>` — Explicit token override. Takes precedence over `MEMSTEAD_TOKEN` and stored credentials
* `--registry <URL>` — Registry URL (overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io)



## `memstead login`

Authenticate with a registry via GitHub Device Flow. Optional — `publish` auto-triggers the same flow on first use

**Usage:** `memstead login [OPTIONS]`

###### **Options:**

* `--registry <URL>` — Registry URL (overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io)



## `memstead logout`

Remove stored credentials for a registry

**Usage:** `memstead logout [OPTIONS]`

###### **Options:**

* `--registry <URL>` — Registry URL (overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io)



## `memstead create`

Create a new entity. Provide `--title`, `--type`, and the required section fields, or pass `--from <file.json>` with the full payload

**Usage:** `memstead create [OPTIONS]`

Section / append / patch flag values:
  `--section KEY=VALUE`, `--append KEY=VALUE`, and `--patch KEY=OLD=>NEW`
  store the right-hand side as bytes verbatim. The CLI does NOT
  interpret backslash escapes — `--section purpose="line1\nline2"`
  writes the literal two-character sequence `\n` into the section
  body, not a newline.

  For multi-line section content, use `--from <FILE>` where FILE is a
  JSON payload matching the MCP `memstead_create` / `memstead_update` shape.
  The JSON parser de-escapes `\n`, `\t`, etc. before the engine
  sees the value, so a JSON-quoted `"line1\nline2"` round-trips as
  two lines on disk.

Slug derivation:
  The entity slug derives from the title in five steps:
    1. NFC-normalize (combining sequences fold to precomposed form);
    2. Unicode case-fold to lowercase;
    3. rewrite each whitespace character to '-';
    4. drop every character that is not Unicode alphanumeric and not '-';
    5. collapse hyphen runs, trim leading/trailing hyphens.

  The mutation entry refuses titles where step 4 would drop any
  character, or where the pipeline output is empty (whitespace- or
  hyphen-only input). Errors carry a `proposed_slug` recovery hint
  so a sanitised retry is mechanical.

  The title body is stored as-sent (byte-form preserved); slug bytes
  derive from the NFC-normalised form. An NFD-spelled title therefore
  produces an NFC-spelled slug — the two byte forms are semantically
  equivalent and compare equal under NFC normalization.

  Pre-gate entities (created before this stricter rule landed) remain
  readable. The gate runs at mutation entry only — it does not
  retroactively reject entities loaded from disk.

###### **Options:**

* `--title <TITLE>` — Entity title. Required unless `--from` is given
* `--type <ENTITY_TYPE>` — Entity type (e.g. `spec`, `memo`, `concept`). Required unless `--from` is given
* `--mem <MEM>` — Mem name. Defaults to the first writable mem
* `--section <KEY=VALUE>` — Section content: repeatable `--section key=value`. Body wiki-links must take slug-form (`[[idempotency]]`, not the title-case `[[Idempotency]]`) — a non-slug target refuses with `INVALID_WIKI_LINK_TARGET` carrying a `proposed_slug` to retry with
* `--metadata <KEY=VALUE>` — Metadata override: repeatable `--metadata key=value`
* `--relation <TYPE:TARGET>` — Initial relationship: repeatable `--relation TYPE:target-id`. Mem-repo workspaces only — on filesystem mems this refuses; use `memstead relate` after creation there
* `--from <FILE>` — JSON file matching the MCP `memstead_create` args shape. If set, all `--title` / `--type` / `--section` / `--metadata` / `--relation` flags are ignored (the file is the single source of truth). The JSON type field is `entity_type` (not `type`), matching the response envelopes — a previous `--json` response pipes back in unchanged
* `--dry-run` — Preview only — validate and compute the result without writing to disk, mutating the store, or producing a commit. Response carries the prospective id / file_path / content_hash plus any warnings
* `--note <NOTE>` — Agent-authored provenance note (≤280 chars, one sentence describing why this mutation happened). Lands in the per-mem commit body between the mechanical subject line and the provenance trailers. When `[mutations].require_notes = true` in workspace config a missing note adds a `NOTE_MISSING` warning to the response (the mutation still commits). When `--from` also carries a `note`, this flag takes precedence



## `memstead update`

Modify an existing entity. `--expected-hash` is required unless `--auto-hash` (refetch before write) or `--force` (skip check) is given

**Usage:** `memstead update [OPTIONS] [ID]`

###### **Arguments:**

* `<ID>` — Full entity ID (e.g. `specs--my-entity`). Required unless `--from` is given

###### **Options:**

* `--expected-hash <HASH>` — Hash from `memstead entity <id>` (the `_hash` field). Required unless `--auto-hash` or `--force` is given
* `--auto-hash` — Refetch the current hash immediately before writing. Convenient for interactive use; accepts the race window between the refetch and the write
* `--force` — Skip the hash check entirely (explicit overwrite)
* `--section <KEY=VALUE>` — Replace section content: repeatable `--section key=value`. Body wiki-links must take slug-form (`[[idempotency]]`, not the title-case `[[Idempotency]]`) — a non-slug target refuses with `INVALID_WIKI_LINK_TARGET` carrying a `proposed_slug` to retry with
* `--append <KEY=VALUE>` — Append to section content: repeatable `--append key=value`
* `--patch <KEY=OLD=>NEW>` — Find-and-replace inside a section: repeatable `--patch key=OLD=>NEW`. Use `=>` (two chars) as the separator between old and new. Exact match of the first occurrence; use `--patch-all` to replace every occurrence
* `--patch-all <KEY=OLD=>NEW>` — Replace every occurrence of OLD in the section — sibling of `--patch`. Repeatable `--patch-all key=OLD=>NEW`
* `--metadata <KEY=VALUE>` — Metadata field: repeatable `--metadata key=value`
* `--metadata-unset <KEY>` — Remove a metadata field: repeatable `--metadata-unset KEY`. Silent no-op if the key is absent; errors on read-only fields (mem/id/type plus the engine-stamped created_date/last_modified) or schema-required fields
* `--declare-relations <REL_TYPE:TARGET_ID>` — Atomic batched relation declaration: repeatable `--declare-relations REL_TYPE:TARGET_ID`. Each entry is validated like an individual `memstead relate` call (schema-shape, cross-mem policy, target-id grammar) and appended to the entity's relations BEFORE the strict wiki-link/relation validator runs. Lets the agent add `[[target]]` body wiki-links AND declare the backing relation in one `memstead update` call without an interleaved `memstead relate`. Absent Write-mem targets are auto-stubbed identically to `memstead relate`'s add path. Each successful declaration is echoed in the response's `relations_declared` (with `target_was_stubbed` flagging the auto-stub case)
* `--dry-run` — Preview what would change without writing
* `--from <FILE>` — JSON file matching MCP `memstead_update` args shape. When set, flags above except the hash-mode flags are ignored
* `--note <NOTE>` — Agent-authored provenance note (≤280 chars). When `[mutations].require_notes = true` a missing note adds a `NOTE_MISSING` warning



## `memstead relate`

Add or remove a typed relationship between two entities

**Usage:** `memstead relate [OPTIONS] [FROM] [REL_TYPE] [TO]`

###### **Arguments:**

* `<FROM>` — Source entity ID (positional). Flag synonym: `--from`
* `<REL_TYPE>` — Relationship type (positional). Flag synonym: `--rel-type`. UPPER_SNAKE_CASE, e.g. `USES`, `PART_OF`
* `<TO>` — Target entity ID (positional). Flag synonym: `--to`. Creates a stub if the target doesn't exist

###### **Options:**

* `--from <ID>` — Source entity ID (named flag form)
* `--rel-type <REL_TYPE>` — Relationship type (named flag form)
* `--to <ID>` — Target entity ID (named flag form)
* `--remove` — Remove the relationship instead of creating it
* `--description <DESCRIPTION>` — Per-edge description applied on add. Validated against the rel-type's `per_edge_description` posture; rel-types declared `forbidden` reject this flag, `required` reject its absence
* `--note <NOTE>` — Agent-authored provenance note (≤280 chars). When `[mutations].require_notes = true` a missing note adds a `NOTE_MISSING` warning



## `memstead delete`

Delete an entity. Use `--dry-run` to preview impact first. Delete is hashless by design (no post-state to race on); race protection comes from `HAS_INCOMING_REFS` — and `RESIDUAL_STUB_FOR_READONLY_REFERRERS` for read-only-referrer cases

**Usage:** `memstead delete [OPTIONS] <ID>`

###### **Arguments:**

* `<ID>` — Entity ID to delete

###### **Options:**

* `--dry-run` — Show what would be removed without deleting anything
* `--note <NOTE>` — Agent-authored provenance note (≤280 chars). When `[mutations].require_notes = true` a missing note adds a `NOTE_MISSING` warning



## `memstead rename`

Rename an entity (changes ID, file path, and every incoming wiki-link)

**Usage:** `memstead rename [OPTIONS] <ID> <NEW_TITLE>`

Slug derivation:
  The entity slug derives from the title in five steps:
    1. NFC-normalize (combining sequences fold to precomposed form);
    2. Unicode case-fold to lowercase;
    3. rewrite each whitespace character to '-';
    4. drop every character that is not Unicode alphanumeric and not '-';
    5. collapse hyphen runs, trim leading/trailing hyphens.

  The mutation entry refuses titles where step 4 would drop any
  character, or where the pipeline output is empty (whitespace- or
  hyphen-only input). Errors carry a `proposed_slug` recovery hint
  so a sanitised retry is mechanical.

  The title body is stored as-sent (byte-form preserved); slug bytes
  derive from the NFC-normalised form. An NFD-spelled title therefore
  produces an NFC-spelled slug — the two byte forms are semantically
  equivalent and compare equal under NFC normalization.

  Pre-gate entities (created before this stricter rule landed) remain
  readable. The gate runs at mutation entry only — it does not
  retroactively reject entities loaded from disk.

###### **Arguments:**

* `<ID>` — Current entity ID
* `<NEW_TITLE>` — New title. The ID is re-derived from the title

###### **Options:**

* `--expected-hash <HASH>` — Hash from `memstead entity <id>`. Required unless `--auto-hash` or `--force`
* `--auto-hash` — Refetch the current hash immediately before writing
* `--force` — Skip the hash check (explicit overwrite)
* `--note <NOTE>` — Agent-authored provenance note (≤280 chars). When `[mutations].require_notes = true` a missing note adds a `NOTE_MISSING` warning



## `memstead batch-update`

Update many entities in one atomic call. Input is a JSON file with a top-level `updates: [...]` array (one entry per entity, each with its own hash mode and mutation fields). All-or-nothing: if any entry fails (validation, hash mismatch, missing entity) the whole batch is refused and NOTHING is committed — fix the named entry and resubmit. On success the batch lands as one commit. Mirrors `memstead update` per entry

**Usage:** `memstead batch-update --from <FILE>`

###### **Options:**

* `--from <FILE>` — JSON file with a top-level `updates: [...]` array



## `memstead recover`

Apply parse-time-drift recovery across writable mems. Walks `PARSED_RELATION_INVALID` warnings, re-renders affected source entities to drop the stale rows, and reports per-entry outcomes. Read-only-origin drops surface as skipped

**Usage:** `memstead recover [OPTIONS]`

###### **Options:**

* `--note <NOTE>` — Optional commit-body note recorded on every per-source re-render commit the recovery produces



## `memstead changes`

Diff a mem's HEAD against a commit SHA. Pass `--since` = a prior `commit_sha` from a mutation, or the canonical empty-tree hash `4b825dc642cb6eb9a060e54bf8d69288fbee4904` for a first sync

**Usage:** `memstead changes [OPTIONS] --since <SINCE>`

###### **Options:**

* `--mem <MEM>` — Writable mem name. Defaults to the first loaded mem
* `--since <SINCE>` — Commit SHA to diff against. Pass a prior mutation's `commit_sha`, or the git canonical empty-tree hash `4b825dc642cb6eb9a060e54bf8d69288fbee4904` for a fresh-client first sync
* `--rename-similarity <RENAME_SIMILARITY>` — Rename detection threshold in [0.1, 1.0]; mirrors the MCP `rename_similarity` parameter. Default 0.6. Engine-authored renames pair via commit-note provenance and bypass this threshold; the value drives the rename-similarity fallback for non-engine renames (external `git mv`, pre-provenance migrations). Lower widens the recall window at the cost of false-positive pairing on that path
* `--include-notes` — Fold per-commit agent-notes (subject, note, actor, tool, client) and the workspace-level schema/registry ref tip (unified schemas + per-mem configs) into the response. Default off — entity- delta only. Outer-repo auto-commit consumers turn this on so they get notes + the registry-ref sha in one round-trip without re-walking the gitdir



## `memstead reload`

Reload one writable mem's slice of the in-memory store from its on-disk branch tip — or every writable mem when `--mem` is omitted. CLI parity with the MCP `memstead_reload` tool

**Usage:** `memstead reload [OPTIONS]`

###### **Options:**

* `--mem <MEM>` — Writable mem name to reload. Omit to reload every writable mem. Mirrors the MCP `memstead_reload` parameter shape and the op's semantics: per-mem form is cheap and skips the workspace-level settings refresh; workspace-wide form (omit `--mem`) reloads every mem and also re-reads the workspace policy to pick up edits



## `memstead fetch`

Fetch a mem's branch refs from a git remote into the mem-repo (no local branch moves — inspect first, then `pull`). Requires a git-branch-backed mem (`INVALID_INPUT` on folder mounts); refuses `UNKNOWN_REMOTE` when the remote is not configured

**Usage:** `memstead fetch [OPTIONS] <MEM> [REFSPECS]...`

###### **Arguments:**

* `<MEM>`
* `<REFSPECS>` — Optional refspecs forwarded to the underlying `git fetch`. Empty list uses the remote's configured defaults

###### **Options:**

* `--remote <REMOTE>`

  Default value: `origin`



## `memstead pull`

Fast-forward a mem's branch to its fetched remote counterpart and reload the in-memory store. Refuses `LOCAL_DIVERGENCE` when the local branch is not an ancestor of the remote — reconcile via `branch-reset`, or resolve on another clone and push

**Usage:** `memstead pull [OPTIONS] <MEM>`

###### **Arguments:**

* `<MEM>`

###### **Options:**

* `--remote <REMOTE>`

  Default value: `origin`



## `memstead push`

Push a mem's branch to a git remote. `--force` uses force-with-lease semantics; without it, non-fast-forward pushes refuse (`NON_FAST_FORWARD`). Refuses `UNKNOWN_REMOTE` when the remote is not configured

**Usage:** `memstead push [OPTIONS] <MEM>`

###### **Arguments:**

* `<MEM>`

###### **Options:**

* `--remote <REMOTE>`

  Default value: `origin`
* `--force` — Force-push (`--force-with-lease` under the hood). Refused non-fast-forward pushes only happen here. Use with care — the remote's view of the branch is overwritten

  Default value: `false`



## `memstead branch-reset`

Reset a mem's branch pointer to a target ref/SHA. Refuses to discard commits reachable from any remote ref (`PUSHED_COMMITS_PROTECTED`)

**Usage:** `memstead branch-reset <MEM> <TARGET_SHA>`

###### **Arguments:**

* `<MEM>` — Mem whose branch pointer to reset. Must be git-branch-backed
* `<TARGET_SHA>` — Target ref or SHA. Accepts anything `git rev-parse` admits — branch names, abbreviated SHAs, full SHAs, tags



## `memstead mem`

Mem lifecycle commands

**Usage:** `memstead mem <COMMAND>`

###### **Subcommands:**

* `init` — Register a new mem via the engine's mem-management orchestrator
* `unregister` — Router-only removal — unregisters the mem from the workspace but leaves its stored content in place for archive workflows. Cross-mem grants pointing at the unregistered mem stay valid (the data they rely on survives); a follow-up `memstead mem init <same name>` re-attaches against the preserved storage. Refuses with `MEM_HAS_INCOMING_REFS` when entities in other mems still link into this one — remove those incoming cross-mem references first (mirrors `mem delete`'s precondition)
* `delete` — Storage-destroying removal — unregisters the mem AND deletes its stored content. Refuses with `MEM_REFERENCED_BY_POLICY` when any other writable mem has a `cross_mem_links` grant pointing at the target (revoke the grant first). For router-only removal that keeps the storage, use `memstead mem unregister`
* `set-version` — Update a mem's `version` field. The version is consumed by `memstead export --format mem` to stamp the archive filename and the `.mem` archive's published config. `version` is seeded at init (`0.1.0`); bump via this command before publishing
* `set-schema` — Set a mem's schema pin — the integrity-driven schema-migration trigger. Already-integral mems switch immediately; otherwise the mem enters dual-pin migration (writes validate against the target) and the response lists the non-integral entities. Re-issue after repairing to complete the switch
* `set-description` — Set a mem's one-line `description` — embedded in `.mem` archive exports and surfaced on the registry card at publish time. An empty string clears the field. Set it before `memstead export` / `memstead publish` so the shared archive carries its card text
* `set-sync-state` — Set (or clear) one opaque sync-state token in a mem's config — the ingest layer's durable "last synced source state" baseline. `<KEY>` and `<TOKEN>` are opaque to the engine (the ingest layer keys per `(ingest, facet)` and owns the token's meaning). An empty `<TOKEN>` clears the key. Written into the per-mem config and surfaced verbatim on `memstead workspace dump`
* `set-internal` — Mark (or unmark) a mem as internal — hidden from the default `memstead overview` roster and public projections, while staying a real, inspectable (`overview --mem <name>`), deletable mem. Ingest process-state mems are flagged this way
* `list` — Enumerate every mounted mem in the workspace with its schema pin, version, entity count, and capability (writable vs read-only). Markdown by default; pass `--json` (root flag) for the structured envelope



## `memstead mem init`

Register a new mem via the engine's mem-management orchestrator

**Usage:** `memstead mem init [OPTIONS] <PATH>`

###### **Arguments:**

* `<PATH>` — Mem name — the full hierarchical identifier (e.g. `foo` for a flat-layout mem, `team/sub-mem` for a hierarchical layout). The value flows through to the engine verbatim with no auto-split or composition step. Grammar: `[a-z0-9-]+(/[a-z0-9-]+)*` — lowercase ASCII letters, digits, hyphens; segments separated by `/`; no leading, trailing, or double slashes (validated engine-side; bad names return `INVALID_INPUT`)

###### **Options:**

* `--schema <SCHEMA>` — Schema pin (`name@x.y.z`) for the new mem. Defaults to `default@1.0.0` so the common case stays one argument

  Default value: `default@1.0.0`
* `--vcs-shared` — Pass a shared-gitdir `vcs` block to `memstead_mem_create`: `{ "gitdir": "../.git", "worktree": ".." }`. Without this flag the engine uses the default isolated layout
* `--no-gitignore` — Skip outer-repo `.gitignore` auto-append. Useful when the user intends to track the workspace as a git submodule, or when the detection heuristic would pick the wrong outer repo
* `--note <NOTE>` — Optional provenance note recorded in the seed commit's body (≤280 chars). Forwarded as the MCP tool's `note` parameter
* `--reattach` — Adopt residual entities left by a prior `memstead mem unregister` at this mem's path instead of failing on detected residue. Default when the residue carries an `unregistered_at` tombstone (the deliberate unregister signal); pass `--reattach` explicitly to override for crash-residue you have verified is safe to adopt. Mutually exclusive with `--force-overwrite` and `--hard-cleanup-first`
* `--force-overwrite` — Destroy residual storage at this mem's path and proceed with a fresh create. **Not yet implemented** — currently refuses with `INVALID_INPUT` pointing at `memstead mem delete <name>`. Mutually exclusive with `--reattach` and `--hard-cleanup-first`
* `--hard-cleanup-first` — Refuse with `MEM_STORAGE_RESIDUE_DETECTED` instructing the caller to run `memstead mem delete <name>` first — a hard barrier that keeps residue cleanup a separate, named operation rather than destructive auto-recovery. Mutually exclusive with `--reattach` and `--force-overwrite`
* `--operator-mode` — Bypass the workspace `[[mem_management.create]]` allowlist for this invocation. The CLI honours the allowlist by default (matching the MCP-surface posture); operator-mode is explicit opt-in. Also settable via the `MEMSTEAD_OPERATOR_MODE=1` env var for script convenience; the flag wins when both are set. Use this when the CLI invocation is the operator administering the workspace itself (initial scaffold, recovery flows) rather than scripted/agent usage
* `--write-guidance <WRITE_GUIDANCE>` — Optional per-instance writing guidance as a JSON object, written verbatim into the new mem's config `writeGuidance` map — e.g. `--write-guidance '{"phase_context":"early design","stack":"Rust"}'`. Opaque to the engine (schema-strictness D8 — the keys are client-owned vocabulary); a wrapper that read the schema package's `mem-template.json` fills the instance keys. Omit to seed no guidance. Must be a JSON object; anything else refuses with `INVALID_INPUT`



## `memstead mem unregister`

Router-only removal — unregisters the mem from the workspace but leaves its stored content in place for archive workflows. Cross-mem grants pointing at the unregistered mem stay valid (the data they rely on survives); a follow-up `memstead mem init <same name>` re-attaches against the preserved storage. Refuses with `MEM_HAS_INCOMING_REFS` when entities in other mems still link into this one — remove those incoming cross-mem references first (mirrors `mem delete`'s precondition)

**Usage:** `memstead mem unregister [OPTIONS] <NAME>`

###### **Arguments:**

* `<NAME>` — Name of the mem to unregister

###### **Options:**

* `--note <NOTE>` — Optional provenance note (≤280 chars). Captured on the engine trace surface; surfaces via the outer-repo Stop hook
* `--operator-mode` — Bypass the workspace `[[mem_management.delete]]` allowlist for this invocation. See `InitArgs::operator_mode` for the full design rationale. Also settable via `MEMSTEAD_OPERATOR_MODE=1`



## `memstead mem delete`

Storage-destroying removal — unregisters the mem AND deletes its stored content. Refuses with `MEM_REFERENCED_BY_POLICY` when any other writable mem has a `cross_mem_links` grant pointing at the target (revoke the grant first). For router-only removal that keeps the storage, use `memstead mem unregister`

**Usage:** `memstead mem delete [OPTIONS] <NAME>`

###### **Arguments:**

* `<NAME>` — Name of the mem to destroy

###### **Options:**

* `--note <NOTE>` — Optional provenance note (≤280 chars). Captured on the engine trace surface; surfaces via the outer-repo Stop hook. No per-mem commit is produced by delete
* `--operator-mode` — Bypass the workspace `[[mem_management.delete]]` allowlist for this invocation. See `InitArgs::operator_mode` for the full design rationale. Also settable via `MEMSTEAD_OPERATOR_MODE=1`



## `memstead mem set-version`

Update a mem's `version` field. The version is consumed by `memstead export --format mem` to stamp the archive filename and the `.mem` archive's published config. `version` is seeded at init (`0.1.0`); bump via this command before publishing

**Usage:** `memstead mem set-version [OPTIONS] <NAME> <VERSION>`

###### **Arguments:**

* `<NAME>` — Mem name (the leaf-folder identifier the engine assigned at init time). Must already be registered in the workspace
* `<VERSION>` — New semver version (e.g. `0.2.0`, `1.0.0-beta.1`). Malformed values refuse with `INVALID_INPUT`. The engine bypasses the mem-create allowlist for this surface — set-version is gate-free

###### **Options:**

* `--note <NOTE>` — Optional provenance note (≤280 chars) recorded on the version-bump commit body, like the other commit-producing mem-lifecycle commands. When the workspace sets `require_notes`, omitting it rides a non-blocking `NOTE_MISSING` warning (the bump still lands)



## `memstead mem set-schema`

Set a mem's schema pin — the integrity-driven schema-migration trigger. Already-integral mems switch immediately; otherwise the mem enters dual-pin migration (writes validate against the target) and the response lists the non-integral entities. Re-issue after repairing to complete the switch

**Usage:** `memstead mem set-schema <NAME> <SCHEMA>`

###### **Arguments:**

* `<NAME>` — Mem name (must be registered in the workspace)
* `<SCHEMA>` — Target schema ref, exact `name@x.y.z`. Must resolve against the loaded schema catalogue; unresolvable refs refuse with `SCHEMA_NOT_FOUND`, malformed refs with `INVALID_INPUT`



## `memstead mem set-description`

Set a mem's one-line `description` — embedded in `.mem` archive exports and surfaced on the registry card at publish time. An empty string clears the field. Set it before `memstead export` / `memstead publish` so the shared archive carries its card text

**Usage:** `memstead mem set-description [OPTIONS] <NAME> <DESCRIPTION>`

###### **Arguments:**

* `<NAME>` — Mem name (must be registered in the workspace)
* `<DESCRIPTION>` — One-line description of the mem — what a registry visitor (or an agent browsing the catalogue) should know before installing. An empty string clears the field

###### **Options:**

* `--note <NOTE>` — Optional provenance note (≤280 chars) recorded on the commit body, like the other commit-producing mem-lifecycle commands



## `memstead mem set-sync-state`

Set (or clear) one opaque sync-state token in a mem's config — the ingest layer's durable "last synced source state" baseline. `<KEY>` and `<TOKEN>` are opaque to the engine (the ingest layer keys per `(ingest, facet)` and owns the token's meaning). An empty `<TOKEN>` clears the key. Written into the per-mem config and surfaced verbatim on `memstead workspace dump`

**Usage:** `memstead mem set-sync-state [OPTIONS] <NAME> <KEY> <TOKEN>`

###### **Arguments:**

* `<NAME>` — Mem name (must be registered in the workspace)
* `<KEY>` — Opaque sync-state key. The ingest layer keys per `(ingest, facet)`, conventionally `"<ingest>/<facet>"`, but the engine treats it as an arbitrary string
* `<TOKEN>` — Opaque token recording the source state last synced under `<KEY>` (git → commit id, graph → snapshot token, filesystem → a JSON-stringified stat digest). An **empty** value clears the key. The engine never parses it

###### **Options:**

* `--note <NOTE>` — Optional provenance note (≤280 chars) recorded on the commit body, like the other commit-producing mem-lifecycle commands



## `memstead mem set-internal`

Mark (or unmark) a mem as internal — hidden from the default `memstead overview` roster and public projections, while staying a real, inspectable (`overview --mem <name>`), deletable mem. Ingest process-state mems are flagged this way

**Usage:** `memstead mem set-internal [OPTIONS] <NAME>`

###### **Arguments:**

* `<NAME>` — Mem name (must be registered in the workspace)

###### **Options:**

* `--off` — Unmark the mem as internal (make it visible in the default overview again). Without this flag, the mem is marked internal
* `--note <NOTE>` — Optional provenance note (≤280 chars) recorded on the commit body



## `memstead mem list`

Enumerate every mounted mem in the workspace with its schema pin, version, entity count, and capability (writable vs read-only). Markdown by default; pass `--json` (root flag) for the structured envelope

**Usage:** `memstead mem list`



## `memstead mem-repo`

Mem-repo-git lifecycle commands

**Usage:** `memstead mem-repo <COMMAND>`

###### **Subcommands:**

* `init` — Bootstrap a fresh mem-repo-git workspace
* `remote-add` — Configure (or re-point) a named git remote on the mem-repo, so `memstead fetch` / `pull` / `push` have somewhere to go. Upsert: re-running with a new URL re-points the remote



## `memstead mem-repo init`

Bootstrap a fresh mem-repo-git workspace

**Usage:** `memstead mem-repo init [OPTIONS] [PATH]`

###### **Arguments:**

* `<PATH>` — Workspace directory to bootstrap. Created if missing. Defaults to the current directory

  Default value: `.`

###### **Options:**

* `--no-gitignore` — Skip outer-repo `.gitignore` auto-append. Useful when the user intends to track `mem-repo/` as a git submodule, or when the detection heuristic would pick the wrong outer repo



## `memstead mem-repo remote-add`

Configure (or re-point) a named git remote on the mem-repo, so `memstead fetch` / `pull` / `push` have somewhere to go. Upsert: re-running with a new URL re-points the remote

**Usage:** `memstead mem-repo remote-add <NAME> <URL>`

###### **Arguments:**

* `<NAME>` — Remote name (e.g. `origin`)
* `<URL>` — Remote URL (e.g. `git@github.com:you/mem-backup.git` or a local bare-repo path)



## `memstead workspace`

Introspect and configure workspace policy — `dump` reads the effective config; `allow-create`/`revoke-create`/`allow-delete`/ `revoke-delete`/`grant-cross-link`/`revoke-cross-link`/`set-mutations` write the mem-lifecycle allowlist, cross-mem link grants, and mutation policy

**Usage:** `memstead workspace <COMMAND>`

###### **Subcommands:**

* `dump` — Emit a JSON document describing the workspace's mems, the schema each is pinned to, and per-mem opaque snapshot tokens. Output is always JSON (the global `--json` is a no-op here)
* `show` — Render the active workspace configuration: mem-management allowlists, cross-mem permissions, mutation policy, plugin sections. Markdown by default; `--json` emits a structured document. Counterpart to the `allow-create / grant-cross-link / set-mutations` write surface — read what those commands have composed
* `allow-create` — Add a `[[mem_management.create]]` allowlist rule. Pattern uses gitignore-style globs (`*` does not cross `/`, `**` matches zero-or-more segments). Schemas pin which schemas the agent may bring into existence under this namespace; `--schema *` allows any schema. Order: appended (lowest priority) by default; `--before <pattern>` lifts it above the named pattern
* `revoke-create` — Remove a `[[mem_management.create]]` rule by pattern
* `allow-delete` — Add a `[[mem_management.delete]]` allowlist rule
* `revoke-delete` — Remove a `[[mem_management.delete]]` rule by pattern
* `grant-cross-link` — Grant a `[cross_mem_links]` permission: `<from>` may write edges into `<to>`. `<to>` is `*` for the wildcard shape or a mem name for the allowlist shape. Mixing the two for one `from`-mem is rejected
* `revoke-cross-link` — Revoke a `[cross_mem_links]` permission. Removes the named target from the allowlist; drops the `from`-key entirely when the allowlist becomes empty. `*` revokes the wildcard shape
* `set-mutations` — Set a `[mutations]` field. Today exposes `--require-notes` only; additional keys land additively



## `memstead workspace dump`

Emit a JSON document describing the workspace's mems, the schema each is pinned to, and per-mem opaque snapshot tokens. Output is always JSON (the global `--json` is a no-op here)

**Usage:** `memstead workspace dump`



## `memstead workspace show`

Render the active workspace configuration: mem-management allowlists, cross-mem permissions, mutation policy, plugin sections. Markdown by default; `--json` emits a structured document. Counterpart to the `allow-create / grant-cross-link / set-mutations` write surface — read what those commands have composed

**Usage:** `memstead workspace show`



## `memstead workspace allow-create`

Add a `[[mem_management.create]]` allowlist rule. Pattern uses gitignore-style globs (`*` does not cross `/`, `**` matches zero-or-more segments). Schemas pin which schemas the agent may bring into existence under this namespace; `--schema *` allows any schema. Order: appended (lowest priority) by default; `--before <pattern>` lifts it above the named pattern

**Usage:** `memstead workspace allow-create [OPTIONS] --schema <SCHEMA> <PATTERN>`

###### **Arguments:**

* `<PATTERN>` — Glob pattern (gitignore semantics) the rule matches against the lifecycle candidate `<path>/<name>` (or `<name>` for flat-layout mems)

###### **Options:**

* `--schema <SCHEMA>` — Schema pins the rule permits. Repeat or pass as a single comma-separated value. `*` is the any-schema escape
* `--cross-link <CROSS_LINK>` — Cross-mem permission conferred on every mem matching this rule. Rule-derived and evaluated lazily at relate time — not written into `[cross_mem_links]`; `workspace show` and `memstead_overview` surface it under the rule. Repeat or pass as a single comma-separated value; `*` for wildcard
* `--before <BEFORE>` — Insert this rule before the named pattern (lifts it above the target in the first-match-wins order). Omit to append at the lowest priority



## `memstead workspace revoke-create`

Remove a `[[mem_management.create]]` rule by pattern

**Usage:** `memstead workspace revoke-create <PATTERN>`

###### **Arguments:**

* `<PATTERN>` — Pattern identifying the rule



## `memstead workspace allow-delete`

Add a `[[mem_management.delete]]` allowlist rule

**Usage:** `memstead workspace allow-delete <PATTERN>`

###### **Arguments:**

* `<PATTERN>` — Pattern identifying the rule



## `memstead workspace revoke-delete`

Remove a `[[mem_management.delete]]` rule by pattern

**Usage:** `memstead workspace revoke-delete <PATTERN>`

###### **Arguments:**

* `<PATTERN>` — Pattern identifying the rule



## `memstead workspace grant-cross-link`

Grant a `[cross_mem_links]` permission: `<from>` may write edges into `<to>`. `<to>` is `*` for the wildcard shape or a mem name for the allowlist shape. Mixing the two for one `from`-mem is rejected

**Usage:** `memstead workspace grant-cross-link <FROM> <TO>`

###### **Arguments:**

* `<FROM>` — Source mem (the `from` side of the permission)
* `<TO>` — Target mem or `*` for the wildcard shape



## `memstead workspace revoke-cross-link`

Revoke a `[cross_mem_links]` permission. Removes the named target from the allowlist; drops the `from`-key entirely when the allowlist becomes empty. `*` revokes the wildcard shape

**Usage:** `memstead workspace revoke-cross-link <FROM> <TO>`

###### **Arguments:**

* `<FROM>` — Source mem (the `from` side of the permission)
* `<TO>` — Target mem or `*` for the wildcard shape



## `memstead workspace set-mutations`

Set a `[mutations]` field. Today exposes `--require-notes` only; additional keys land additively

**Usage:** `memstead workspace set-mutations [OPTIONS]`

###### **Options:**

* `--require-notes <BOOL>` — Toggle `[mutations] require_notes`. When set, mutations without a `note` field surface a `note_missing` warning (the mutation still lands — provenance is best-effort)

  Possible values: `true`, `false`




## `memstead schema`

Author-time schema tooling. `memstead schema validate <path>` checks a schema package directory against the engine's loader without touching a workspace

**Usage:** `memstead schema <COMMAND>`

###### **Subcommands:**

* `new` — Scaffold a new schema package at `./<name>/` — a manifest plus one commented example type — that `memstead schema validate` passes unmodified. Prints the follow-up commands that take the package from folder to pinned mem
* `validate` — Validate a schema package directory (`schema.yaml` plus an optional `types/*.yaml`) against the engine's schema loader — the same validation the engine runs at load. Exits non-zero (`SCHEMA_VALIDATION_FAILED`) on any conformance error, with the YAML line/column in the message where the parse layer provides it
* `install` — Install a schema package into the current folder workspace's `.memstead/schemas/<name>@<version>/` so a mem can pin it. `<source>` is a built-in name (`planning`, `planning@0.1.0`) or a path to a package directory. Validates before copying; idempotent



## `memstead schema new`

Scaffold a new schema package at `./<name>/` — a manifest plus one commented example type — that `memstead schema validate` passes unmodified. Prints the follow-up commands that take the package from folder to pinned mem

**Usage:** `memstead schema new <NAME>`

###### **Arguments:**

* `<NAME>` — Schema name. Grammar: starts with a lowercase letter, then lowercase letters, digits, and hyphens. The package is written to `./<name>/`



## `memstead schema validate`

Validate a schema package directory (`schema.yaml` plus an optional `types/*.yaml`) against the engine's schema loader — the same validation the engine runs at load. Exits non-zero (`SCHEMA_VALIDATION_FAILED`) on any conformance error, with the YAML line/column in the message where the parse layer provides it

**Usage:** `memstead schema validate <PATH>`

###### **Arguments:**

* `<PATH>` — Path to the schema package directory (the folder containing `schema.yaml`)



## `memstead schema install`

Install a schema package into the current folder workspace's `.memstead/schemas/<name>@<version>/` so a mem can pin it. `<source>` is a built-in name (`planning`, `planning@0.1.0`) or a path to a package directory. Validates before copying; idempotent

**Usage:** `memstead schema install <SOURCE>`

###### **Arguments:**

* `<SOURCE>` — Built-in schema name (`planning`, `planning@0.1.0`) or a path to a schema package directory



## `memstead pipeline`

Pipeline-config tooling. `memstead pipeline migrate` converts the legacy `scopes|projections|ingests/` JSON folders into the `.memstead/` workspace store's four-primitive shape

**Usage:** `memstead pipeline <COMMAND>`

###### **Subcommands:**

* `migrate` — Migrate the legacy `scopes|projections|ingests/` JSON folders at the workspace root into the four-primitive workspace-store shape under `.memstead/`. A legacy scope splits into a Medium (territory) and a Facet (engagement). Idempotent — re-running reproduces identical files. The legacy folders are left in place; remove them when ready



## `memstead pipeline migrate`

Migrate the legacy `scopes|projections|ingests/` JSON folders at the workspace root into the four-primitive workspace-store shape under `.memstead/`. A legacy scope splits into a Medium (territory) and a Facet (engagement). Idempotent — re-running reproduces identical files. The legacy folders are left in place; remove them when ready

**Usage:** `memstead pipeline migrate`



## `memstead projection`

Binding (projection-promotion) tooling. `memstead projection init` scaffolds a fresh v1 binding non-interactively; `memstead projection migrate` promotes gen-2 four-primitive configs (per-mem projection + flat ingest) into v1 bindings; `memstead projection enable <build|sync|verify> <binding>` adds a missing operation block — one versioned record per source→mem obligation, with an `operations { build, sync, verify }` block

**Usage:** `memstead projection <COMMAND>`

###### **Subcommands:**

* `init` — Scaffold a fresh v1 binding non-interactively: a `Medium`, a `Facet`, and a v1 binding under `.memstead/{mediums,facets,projections}/<mem>/`. All inputs are flags — no prompts ever (parity across callers). The default binding declares build+sync+verify capability-permitting (D6): a `web` source scaffolds build-only, with the deferral named in `warnings[]`. Refuses `PROJECTION_EXISTS` (without touching disk) when a binding of the same id already exists — never overwrites
* `migrate` — Migrate gen-2 four-primitive configs (per-mem `Projection` + flat `Ingest`) into v1 bindings, merging each ingest into the projection its `projection` ref names. The binding takes the projection's file identity (`.memstead/projections/<mem>/<stem>.json`); the merged ingest is removed. `refinement` mode and dangling projection refs refuse with a typed error. Use `--dry-run` to preview without writing
* `enable` — Enable a `build` / `sync` / `verify` operation on an existing binding by adding its block (with sensible defaults) if absent. This is the remedy a refused *mutating* operation cites (D6): `projection enable sync <binding>`. Before writing, the operation is checked against the medium-capability matrix (D6) — enabling `sync`/`verify` over a medium that cannot support it (e.g. a `web` source) refuses with the capability gap and writes nothing. Enabling an already-present operation refuses `PROJECTION_OP_ALREADY_ENABLED`; a missing binding refuses `PROJECTION_NOT_FOUND`
* `advance` — Advance a binding's sync baseline by recording per-artifact dispositions (D7). The engine freezes the presented changed slice, subtracts already-disposed artifacts on re-presentation, appends new-HEAD deltas when the source moves mid-pass, and — when the remainder empties — advances the destination mem's `#synced` token via the sync-state writer (provenance piggybacks that commit). Dispositions are durable (`.memstead/state/advance/`), so a partial pass resumes across process restarts. The gate accepts **only** artifact ids the engine presented — an unknown id refuses the whole call atomically (`PROJECTION_ADVANCE_UNKNOWN_ARTIFACT`). In this cycle the agent supplies a disposition for **every** artifact explicitly (auto-derivation lands later)



## `memstead projection init`

Scaffold a fresh v1 binding non-interactively: a `Medium`, a `Facet`, and a v1 binding under `.memstead/{mediums,facets,projections}/<mem>/`. All inputs are flags — no prompts ever (parity across callers). The default binding declares build+sync+verify capability-permitting (D6): a `web` source scaffolds build-only, with the deferral named in `warnings[]`. Refuses `PROJECTION_EXISTS` (without touching disk) when a binding of the same id already exists — never overwrites

**Usage:** `memstead projection init [OPTIONS] --mem <MEM> --source <SOURCE> --medium-type <MEDIUM_TYPE>`

###### **Options:**

* `--mem <MEM>` — Destination mem the binding writes into — the `<mem>` half of the binding id `<mem>/<stem>` and the per-mem tier the three files live under
* `--source <SOURCE>` — The medium pointer — a path (codebase / filesystem / git) or a mem id / URL (graph / web). Becomes the scaffolded medium's `pointer`
* `--medium-type <MEDIUM_TYPE>` — The medium type — decides the capability matrix (D6) that filters which operations the default binding declares

  Possible values:
  - `codebase`:
    A source tree of code
  - `filesystem`:
    A directory of files (non-code)
  - `git`:
    A git history
  - `graph`:
    Another mem's graph
  - `web`:
    Web sources (build-only this cycle — no change signal)

* `--intent <INTENT>` — Intent prose for the agent (the binding's `intent`). Optional
* `--name <NAME>` — Binding stem — the `<stem>` half of the binding id and the shared file name of the scaffolded medium / facet / binding. Defaults to the final path component of `--source`



## `memstead projection migrate`

Migrate gen-2 four-primitive configs (per-mem `Projection` + flat `Ingest`) into v1 bindings, merging each ingest into the projection its `projection` ref names. The binding takes the projection's file identity (`.memstead/projections/<mem>/<stem>.json`); the merged ingest is removed. `refinement` mode and dangling projection refs refuse with a typed error. Use `--dry-run` to preview without writing

**Usage:** `memstead projection migrate [OPTIONS]`

###### **Options:**

* `--dry-run` — Preview the produced bindings (and any warnings) without writing them to disk or removing the merged ingest files



## `memstead projection enable`

Enable a `build` / `sync` / `verify` operation on an existing binding by adding its block (with sensible defaults) if absent. This is the remedy a refused *mutating* operation cites (D6): `projection enable sync <binding>`. Before writing, the operation is checked against the medium-capability matrix (D6) — enabling `sync`/`verify` over a medium that cannot support it (e.g. a `web` source) refuses with the capability gap and writes nothing. Enabling an already-present operation refuses `PROJECTION_OP_ALREADY_ENABLED`; a missing binding refuses `PROJECTION_NOT_FOUND`

**Usage:** `memstead projection enable <OPERATION> <BINDING>`

###### **Arguments:**

* `<OPERATION>` — The operation to enable: `build` | `sync` | `verify`

  Possible values:
  - `build`:
    The build operation (always present — enabling refuses as already-enabled)
  - `sync`:
    The sync (maintenance-write) operation
  - `verify`:
    The verify (measurement) operation

* `<BINDING>` — The binding id `<mem>/<stem>` (D3) — e.g. `engine/graph`



## `memstead projection advance`

Advance a binding's sync baseline by recording per-artifact dispositions (D7). The engine freezes the presented changed slice, subtracts already-disposed artifacts on re-presentation, appends new-HEAD deltas when the source moves mid-pass, and — when the remainder empties — advances the destination mem's `#synced` token via the sync-state writer (provenance piggybacks that commit). Dispositions are durable (`.memstead/state/advance/`), so a partial pass resumes across process restarts. The gate accepts **only** artifact ids the engine presented — an unknown id refuses the whole call atomically (`PROJECTION_ADVANCE_UNKNOWN_ARTIFACT`). In this cycle the agent supplies a disposition for **every** artifact explicitly (auto-derivation lands later)

**Usage:** `memstead projection advance --dispositions <DISPOSITIONS> <BINDING>`

###### **Arguments:**

* `<BINDING>` — The binding id `<mem>/<stem>` (D3) — e.g. `engine/graph`

###### **Options:**

* `--dispositions <DISPOSITIONS>` — A JSON object mapping each judged artifact id to its disposition, e.g. `'{"src/lib.rs": "worked", "src/old.rs": "irrelevant"}'`. Only ids the engine presented in the brief's changed slice are accepted — an unknown id refuses the whole call. Pass `'{}'` to re-present the remainder without recording anything



## `memstead ingest`

Engine-side ingest orchestration. `memstead ingest brief <name>` renders an ingest's run-brief — the Markdown prompt an agent consumes — from the four-primitive config and the destination mem's schema / writing guidance

**Usage:** `memstead ingest <COMMAND>`

###### **Subcommands:**

* `brief` — Render the run-brief for an ingest — the Markdown prompt an agent consumes — on stdout. Reads the four-primitive config and the destination mem's schema / writing guidance



## `memstead ingest brief`

Render the run-brief for an ingest — the Markdown prompt an agent consumes — on stdout. Reads the four-primitive config and the destination mem's schema / writing guidance

**Usage:** `memstead ingest brief [OPTIONS] [NAME]`

###### **Arguments:**

* `<NAME>` — The ingest name (its `.memstead/ingests/<name>.json` file stem). Omit (or pass `--all`) to select the next due ingest by round-robin + backoff

###### **Options:**

* `--all` — Select the next due ingest across all ingests (round-robin + backoff) and render its brief, instead of naming one



&lt;hr/>

&lt;small>&lt;i>
    This document was generated automatically by
    &lt;a href="https://crates.io/crates/clap-markdown">&lt;code>clap-markdown&lt;/code>&lt;/a>.
&lt;/i>&lt;/small>
