---
title: "MCP tools"
---

# MCP tools

Generated from the live `tool_router().list_all()` catalogues on `FilesystemMcpServer` (the lean `--no-default-features` build) and `McpServer` (the full default build). Every tool the running server exposes appears below; each section is tagged with the flavour pair (`lean + full`, `lean only`, or `full only`).

**Counts:** the lean build exposes 12 tools; the full build exposes 23 (a strict superset on shared names).

## Index

- [`memstead_changes_since`](#memstead-changes-since)
- [`memstead_create`](#memstead-create)
- [`memstead_delete`](#memstead-delete)
- [`memstead_diff`](#memstead-diff)
- [`memstead_entity`](#memstead-entity)
- [`memstead_health`](#memstead-health)
- [`memstead_mem_create`](#memstead-mem-create)
- [`memstead_mem_delete`](#memstead-mem-delete)
- [`memstead_mem_set_schema`](#memstead-mem-set-schema)
- [`memstead_mem_set_version`](#memstead-mem-set-version)
- [`memstead_overview`](#memstead-overview)
- [`memstead_relate`](#memstead-relate)
- [`memstead_reload`](#memstead-reload)
- [`memstead_rename`](#memstead-rename)
- [`memstead_schema`](#memstead-schema)
- [`memstead_search`](#memstead-search)
- [`memstead_update`](#memstead-update)
- [`memstead_workspace_allow_create`](#memstead-workspace-allow-create)
- [`memstead_workspace_allow_delete`](#memstead-workspace-allow-delete)
- [`memstead_workspace_grant_cross_link`](#memstead-workspace-grant-cross-link)
- [`memstead_workspace_revoke_create`](#memstead-workspace-revoke-create)
- [`memstead_workspace_revoke_cross_link`](#memstead-workspace-revoke-cross-link)
- [`memstead_workspace_revoke_delete`](#memstead-workspace-revoke-delete)

## `memstead_changes_since`

**Flavour:** lean + full

Per-mem commit-delta feed — reads the mem's own git repo (gitdir via `memstead_health include_config=true`). Pass `since` = a commit SHA previously returned by any mutation (`commit_sha` from create / update / delete / rename / relate responses), or the canonical git empty-tree hash `4b825dc642cb6eb9a060e54bf8d69288fbee4904` for a fresh-client first sync (fresh mems also return that hash as `head`). Returns a flat list of entity-level events — each event's `action` is one of `added`, `updated`, `removed`, `renamed`. Non-`removed` events carry `entity_type` (schema type name, e.g. spec, memo), looked up from the post-diff store; `removed` events carry `entity_type: null` alongside `title: null`. Engine-authored renames pair via commit-note provenance (`memstead: rename <old> → <new>`) — exact, similarity-independent, transitively composed across multi-step rename chains in the same window. Non-engine renames (`git mv`, pre-provenance migrations) fall back to a content-similarity scorer (default 0.6, tunable via `rename_similarity` in [0.1, 1.0]), capped at 1000 rewrite pairs per diff. Either path surfaces as a single `renamed` event with `from_id` and `to_id` rather than a removed+added pair. Out-of-range `rename_similarity` values refuse with `INVALID_INPUT` naming `details.allowed_range` and `details.requested`. `head` echoes the current HEAD SHA — save it as the next polling cursor (prefer full SHAs over refs). No pagination — every qualifying commit ships in one response. Pass `include_notes: true` to fold per-commit agent-notes (`notes[]`) and `memstead_ref` (SHA of the unified schema + per-mem-config registry) into the response — outer-repo auto-commit gets deltas, notes, and the registry-ref sha in one round-trip. Unknown or malformed `since` returns `INVALID_CURSOR` with `details.mem` and `details.since`.

**Hints:** `read_only` = true, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for memstead_changes_since.",
  "properties": {
    "include_notes": {
      "default": false,
      "description": "Fold per-commit agent-notes into the response. When true, the report carries a `notes[]` array (one entry per commit between `since` and `head`, with `sha`, `subject`, `tool_verb`, `entity_id`, `note`, `actor`, `tool`, `client`, `timestamp`) plus `memstead_ref` — the SHA of the unified schema + per-mem-config registry, absent when the workspace has not been migrated yet. Default false (entity-delta only). Outer-repo auto-commit consumers turn this on to receive notes and the registry-ref sha in one round-trip; agents that just need entity events leave it off.",
      "type": "boolean"
    },
    "mem": {
      "description": "Writable mem name. Call memstead_health for the list.",
      "type": "string"
    },
    "rename_similarity": {
      "description": "Rename detection threshold for content-similarity, in [0.1, 1.0]. Default (None) → 0.6. Lower values widen the recall window at the cost of false-positive rename pairing; raise it to 0.9+ when you want only near-byte-identical renames collapsed. Out-of-range values refuse with `INVALID_INPUT` naming `details.allowed_range` and `details.requested` — agents recover by reissuing with a value inside `[0.1, 1.0]`.",
      "format": "float",
      "type": [
        "number",
        "null"
      ]
    },
    "since": {
      "description": "Commit SHA to diff against. Pass the `commit_sha` returned by a prior mutation, or the canonical git empty-tree hash `4b825dc642cb6eb9a060e54bf8d69288fbee4904` to get every entity as `added` (fresh-client first sync).",
      "type": "string"
    }
  },
  "required": [
    "mem",
    "since"
  ],
  "title": "ChangesSinceParams",
  "type": "object"
}
```

## `memstead_create`

**Flavour:** lean + full

Create a new entity. Read the target mem's schema first via `memstead_schema(name=<mem.schema_ref>)` (cached per session) — required sections, allowed metadata fields, relationship vocabulary, and write_rules live there. Required: `title`, `entity_type`, plus the type's required sections. The entity ID is the mem name plus a Unicode-aware slug of the title (e.g. "Große Änderung" → "große-änderung"). A title the slug pipeline cannot represent (emoji, punctuation, non-alphanumerics) or that slugifies to empty is refused with `INVALID_TITLE` carrying a `proposed_slug` to retry. `mem` defaults to the primary writable mem. Pass `relations` to wire edges inline (e.g. `[{to: "specs--parent-id", type: "PART_OF"}]`); unresolved targets auto-create stubs at that ID. Optional `note` (≤280 chars) lands in the commit body; missing when `[mutations].require_notes=true` emits `NOTE_MISSING`. Schema-bound failures carry recovery payload: `UNKNOWN_SECTION`/`UNKNOWN_METADATA_FIELD` ship `details.declared` + nearest-match `suggestion`; `INVALID_ENUM_VALUE` ships `details.allowed`, `details.field_description`, `suggestion`, `details.type_write_rules`; `REQUIRED_FIELD_UNSET` ships `details.field_description`, `details.enum_values`, `details.type_write_rules` — also fires on create when the caller omits a required-no-default metadata field, superseding the `MISSING_REQUIRED_FIELD` warning; `MISSING_REQUIRED_SECTION` ships per-section `write_rules` plus the top-level `type_guidance` map — refused on create so it never lands with a placeholder body; `INVALID_REL_TYPE` ships `details.allowed` (`{name, when_to_use}`) + `suggestion`. Other warnings (entity still lands): `UNDECLARED_RELATIONSHIP_OPEN`, `INLINE_WIKI_LINK_AUTO_STUBBED` (`[[wiki-link]]` in bodies auto-stubs unresolved targets; review `details.stubs` to catch prose-induced ghosts), `MISSING_REQUIRED_OUTGOING` (lists unsatisfied `required_outgoing` per `details.missing[]={relationships, cardinality}`; follow up with memstead_relate). Real writes return `commit_sha` (per-mem git; gitdir via `memstead_health include_config=true`) for polling via memstead_changes_since. `dry_run: true` validates then previews a VALID entity: response carries prospective `id`, `file_path`, `_hash`, warnings, `type_guidance`, and any `incoming` edges adopted from a pre-existing stub, with `commit_sha` empty — but an INVALID entity refuses with the same typed envelope a real call returns, not a warnings-list preview. Use memstead_relate for edges, memstead_update for sections.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$defs": {
    "RelationInput": {
      "additionalProperties": false,
      "description": "A relationship input for create/batch tools.",
      "properties": {
        "description": {
          "default": null,
          "description": "Optional per-edge description text. Validated against the rel-type's `per_edge_description` posture in the pinned schema: `forbidden` (default) rejects a non-empty description with `DESCRIPTION_NOT_PERMITTED`; `required` rejects its absence with `MISSING_REQUIRED_DESCRIPTION`; `optional` accepts both. Empty / whitespace-only strings normalise to absent before validation. Surfaces on `memstead_entity` and round-trips through the `## Relationships` markdown via the canonical em-dash delimiter (` — `).",
          "type": [
            "string",
            "null"
          ]
        },
        "to": {
          "description": "Full target entity ID",
          "type": "string"
        },
        "type": {
          "description": "Relationship type. Canonical form is UPPER_SNAKE_CASE (USES, PART_OF, DEPENDS_ON) and is what the engine stores; case-insensitive inputs (`uses`, `Part_Of`) are accepted and echoed back in the response as their canonical form. The JSON Schema `pattern` advertises `^[A-Za-z][A-Za-z_]*$` for client-side validators; the engine enforces the same character set independently — characters outside it return `INVALID_REL_TYPE` at the engine boundary regardless of whether the client pre-filters.",
          "pattern": "^[A-Za-z][A-Za-z_]*$",
          "type": "string"
        }
      },
      "required": [
        "to",
        "type"
      ],
      "type": "object"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for memstead_create.",
  "properties": {
    "dry_run": {
      "description": "Validate and preview the create without executing — no disk write, no store mutation, no VCS commit, no edges added. dry_run runs the SAME validation a real call runs; it is not a softer check. On a VALID entity the response carries the prospective `id`, `file_path`, and `_hash` (bit-identical to what a real call with the same arguments would produce, EXCEPT for engine-auto-stamped timestamps: the hash covers `created_date`, which is stamped from wall-clock `now()` independently in the dry-run and the real call, so the two `_hash` values diverge whenever a second ticks between them; the hash also covers `sections`, `metadata`, and `relations`, so a dry_run that omits `relations` will not match a real call that supplies them), plus any `warnings` and any `incoming` edges that would be adopted from a pre-existing stub at this id, with `commit_sha` empty. On an INVALID entity dry_run does NOT return a warnings-list preview: it refuses with the IDENTICAL typed envelope a real call would return (`MISSING_REQUIRED_SECTION`, `UNKNOWN_SECTION`, `UNKNOWN_METADATA_FIELD`, `INVALID_ENUM_VALUE`, `REQUIRED_FIELD_UNSET`, …), carrying the same recovery `details.*` (e.g. `details.sections[]`). That typed refusal IS the pre-flight signal — read its `details` to fix coverage, then retry. So dry_run never reports a problem entity as clean: it and a real write agree on validity. Use to verify the id slug, or to pre-flight required-section / field coverage and pre-existing references before committing.",
      "type": [
        "boolean",
        "null"
      ]
    },
    "entity_type": {
      "description": "Entity type. Required. Allowed values are pinned by the target mem's schema — fetch them via `memstead_schema(name=<mem.schema_ref>)` (cached per session). Unknown types refuse with `UNKNOWN_ENTITY_TYPE`.",
      "type": "string"
    },
    "mem": {
      "description": "Mem name (directory name of the write mem)",
      "type": [
        "string",
        "null"
      ]
    },
    "metadata": {
      "additionalProperties": {
        "type": "string"
      },
      "description": "Metadata overrides: { \"level\": \"M1\", \"tags\": \"a, b\" }",
      "type": [
        "object",
        "null"
      ]
    },
    "note": {
      "description": "Agent-authored provenance note (≤280 chars, one sentence describing why this mutation happened). Lands in the per-mem commit body between the mechanical subject line and the provenance trailers (`Tool:`, `Actor:`, `Client:`), and is surfaced by the outer-repo Stop hook when aggregating session activity. Omit for pure-housekeeping edits; when `[mutations].require_notes = true` in workspace config a missing note adds a `NOTE_MISSING` `WarningHint` to the response (the mutation still commits).",
      "type": [
        "string",
        "null"
      ]
    },
    "relations": {
      "description": "Initial relationships to create after entity is created",
      "items": {
        "$ref": "#/$defs/RelationInput"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "sections": {
      "additionalProperties": {
        "type": "string"
      },
      "description": "Section contents: { \"identity\": \"...\", \"purpose\": \"...\" }",
      "type": [
        "object",
        "null"
      ]
    },
    "title": {
      "description": "Entity title (ID is derived automatically as mem--slug(title))",
      "type": "string"
    }
  },
  "required": [
    "title",
    "entity_type"
  ],
  "title": "CreateParams",
  "type": "object"
}
```

## `memstead_delete`

**Flavour:** lean + full

Remove an entity permanently. Deletes the entity's store record, every edge touching it (both directions), and its markdown file on disk. Requires `expected_hash` (read the entity via memstead_entity first — mirrors memstead_update / memstead_rename for optimistic locking); mismatch emits `HASH_MISMATCH` with `details.current` carrying the current on-disk hash. Binary semantics: any incoming reference from another entity in a Write-Mem refuses the delete with `HAS_INCOMING_REFS` and `details.referrers` listing each `{from_id, rel_types, mem}` (one entry per unique source, rel_types collapses multi-edge cases) — the agent removes the offending references via `memstead_relate --remove` (or `memstead_update` for body wiki-links) before retrying. There is no force flag. When the only incoming references come from ReadOnly mounts (archives), the delete proceeds: the on-disk file is removed and the in-memory entity is demoted to a stub at the same id so the surviving edges keep a valid target — the response carries a `RESIDUAL_STUB_FOR_READONLY_REFERRERS` warning naming the surviving referrers. PART_OF children survive the delete: their parent edge is removed; file paths are unaffected (every entity already lives at `{mem}/{slug}.md`). Stubs (`_hash` empty) are deleted with `expected_hash: ""` — the hash check is skipped because there is nothing to compare. Optional `note` (≤280 chars) — shared provenance contract, see memstead_create. Response carries `relations_removed` (edges removed by this delete), `orphan_stubs_removed` (ids of stub entities whose last incoming edge was this entity — they are GC'd in the same op so the graph stays tidy; field is serde-omitted when empty), `warnings` (residual-stub warning when the demote path applied), and `commit_sha` (per-mem git; gitdir via `memstead_health include_config=true`) for polling via memstead_changes_since.

**Hints:** `read_only` = false, `destructive` = true, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for memstead_delete.",
  "properties": {
    "expected_hash": {
      "description": "Hash from memstead_entity response (_hash field). Required for real entities — read first. Mirrors memstead_update / memstead_rename. Mismatch returns code HASH_MISMATCH with details.current carrying the current on-disk hash. Stubs carry an empty `_hash` (they have no on-disk file); pass the empty string to delete a stub — the hash check is skipped because there is nothing to compare.",
      "type": "string"
    },
    "id": {
      "description": "Full entity ID to delete",
      "type": "string"
    },
    "note": {
      "description": "Agent-authored provenance note (≤280 chars, one sentence describing why this mutation happened). Lands in the per-mem commit body between the mechanical subject line and the provenance trailers (`Tool:`, `Actor:`, `Client:`), and is surfaced by the outer-repo Stop hook when aggregating session activity. Omit for pure-housekeeping edits; when `[mutations].require_notes = true` in workspace config a missing note adds a `NOTE_MISSING` `WarningHint` to the response (the mutation still commits).",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "required": [
    "id",
    "expected_hash"
  ],
  "title": "DeleteParams",
  "type": "object"
}
```

## `memstead_diff`

**Flavour:** lean + full

Return a two-ref structural diff at entity granularity. Walks the tree at `ref_a` and the tree at `ref_b` in the mem's gitdir, surfacing per-entity changes as `entries[]` whose `status` is one of `added`, `modified`, `deleted`, `renamed`, `invalid_entity`. Each entry carries the full markdown body on both sides by default in `content_before` / `content_after`; pass `include_content: false` for the metadata-only shape (`id`, `title`, `entity_type`, `status`). Ref-handling conventions mirror `memstead_changes_since`: the canonical empty-tree sentinel `4b825dc642cb6eb9a060e54bf8d69288fbee4904` is accepted as either ref and short-circuits to git's empty tree (first-sync diffs against a fresh mem use this for `ref_a`); a bare `HEAD` resolves to the selected mem's branch tip rather than the gitdir's symbolic HEAD. Cross-mem diffs work via fully-qualified refs naming the peer mem's branch; cross-different-gitdir diffs are out of scope (the op operates on one mem-repo). Refusal codes: `UNKNOWN_MEM` (`details.name`), `UNKNOWN_REF` (`details.ref`), `INVALID_INPUT` for folder / archive mounts and for `rename_similarity` outside the allowed range. Rename detection uses content-similarity tuned by `rename_similarity`; agent-notes-driven rename-chain collapse is a follow-up. Each entry's `ripple` field carries per-side `{from_id, side}` entries for entities with inbound wiki-links to the affected entry — `side: "ref_a"` lists referrers at the `ref_a` snapshot, `side: "ref_b"` at `ref_b`. Pass `include_ripple: false` to omit the field entirely (e.g. for large mems where the per-side wiki-link scan is the dominant cost). Response top-level: `ref_a`, `ref_b`, `resolved_a_sha`, `resolved_b_sha`, `config`, `entries`.

**Hints:** `read_only` = true, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for memstead_diff. Two-ref structural diff at entity\ngranularity; the response wire shape is `Diff` / `EntityDiff` from\n`memstead_base::ops::diff`.",
  "properties": {
    "include_content": {
      "default": true,
      "description": "When true (default), each entry carries the full markdown body on both sides. When false, only metadata (id, title, type, status) survives — smaller payload, useful for audit counts.",
      "type": "boolean"
    },
    "include_ripple": {
      "default": true,
      "description": "When true (default), each entry's `ripple` carries per-side `{from_id, side}` entries for entities with inbound wiki-links to the affected entry — `side: \"ref_a\"` lists referrers at the `ref_a` snapshot, `side: \"ref_b\"` at `ref_b` — so a consumer sees what would break if the change were applied or skipped. Pass false to omit the field (e.g. for large mems where the per-side wiki-link scan dominates cost).",
      "type": "boolean"
    },
    "mem": {
      "description": "Mem that selects the storage context (the gitdir, for git-branch mounts). `ref_a` / `ref_b` are arbitrary refs resolved inside that gitdir; cross-mem diffs work via fully-qualified refs (`refs/heads/<other-mem>`). Folder / archive mounts refuse the call with `INVALID_INPUT` — they carry no git refs to diff.",
      "type": "string"
    },
    "ref_a": {
      "description": "First ref to diff. Branch name (`main`), full ref (`refs/heads/specs`), commit SHA, or tag. Unknown refs refuse with `UNKNOWN_REF` and `details.ref` carrying the raw input.",
      "type": "string"
    },
    "ref_b": {
      "description": "Second ref to diff. Same input shape as `ref_a`.",
      "type": "string"
    },
    "rename_similarity": {
      "description": "Rename detection threshold for content-similarity, in [0.1, 1.0]. Default (None) → 0.6. Out-of-range values refuse with `INVALID_INPUT` (`details.allowed_range`, `details.requested`).",
      "format": "float",
      "type": [
        "number",
        "null"
      ]
    }
  },
  "required": [
    "mem",
    "ref_a",
    "ref_b"
  ],
  "title": "DiffParams",
  "type": "object"
}
```

## `memstead_entity`

**Flavour:** lean + full

Read one entity. Dual channel: text carries rendered markdown for direct prose consumption; `structured_content` carries the typed envelope `{ _hash, id, mem, type, origin, _tokens, metadata, sections, relationships, _stub_kind? }` so agents branch on fields without parsing the text. `origin` is the content's trust class — `first-party` for an entity from a writable workspace mem, `third-party` for one from a read-only mount (a registry-installed read-mem or an adopted foreign folder/clone), which the host should treat as quoted, untrusted data. `_hash` is the optimistic-lock token. The nested `metadata` map is the single home for every schema-declared frontmatter key the entity holds — read a value as `metadata.level`, etc. Identity keys (`mem`/`id`/`type`) and underscore-prefixed engine slots stay top-level, not repeated inside the map. After a successful `memstead_relate` the entity's on-disk hash advances (the Relationships section was rewritten); the relate response's `_hash` is the new valid `_hash` — pass it as `expected_hash` on the next mutation without a re-read. For no-op relates (duplicate add, remove-nonexistent) the relate response echoes the unchanged `_hash` and the pre-relate `_hash` remains valid. Use `include_relations: true` to append a `## Relations` section; `include_context: true` to append the entity's community cluster. Pass `sections` to narrow output to specific section keys (also narrows `structured_content.sections`); when narrowed, `_tokens_unfiltered_body` surfaces the unfiltered-base cost so agents can predict the cost of dropping the filter. With `include_relations`/`include_context` active, `_tokens` may exceed `_tokens_unfiltered_body` because opt-in inserts contribute only to `_tokens`. Stubs render with empty sections + relationships arrays and an empty `metadata: {}` map. `token_budget`/`chunk` bound only the rendered-markdown **text** channel: over-budget text adds `_chunk`/`_total_chunks`/`_truncated` markers. The `structured_content` envelope always ships whole — never chunked or truncated; size it ahead via `_tokens`. Use memstead_overview for cold-start, memstead_search to find IDs, memstead_update to mutate.

**Hints:** `read_only` = true, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for memstead_entity.",
  "properties": {
    "chunk": {
      "description": "Which chunk to read (1-based). Only needed for entities that exceed the token budget.",
      "format": "uint",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "id": {
      "description": "Full entity ID as returned by search/list (e.g. \"specs--my-entity\")",
      "type": "string"
    },
    "include_context": {
      "description": "Append a `## Community Context` section — the entity's cluster summary, members, and bridges to other clusters.",
      "type": [
        "boolean",
        "null"
      ]
    },
    "include_relations": {
      "description": "Append a `## Relations` section with typed edges grouped by direction.",
      "type": [
        "boolean",
        "null"
      ]
    },
    "sections": {
      "description": "Only return these sections (default: all). Use to read specific parts of large entities.",
      "items": {
        "type": "string"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "token_budget": {
      "description": "Max tokens for the rendered-markdown text channel only. If the text exceeds this, returns chunk 1 of N with _truncated in its frontmatter; use the chunk param to read subsequent chunks. The structured_content envelope is never chunked or truncated by this — it always ships whole (size it ahead via its _tokens field).",
      "format": "uint",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    }
  },
  "required": [
    "id"
  ],
  "title": "EntityParams",
  "type": "object"
}
```

## `memstead_health`

**Flavour:** lean + full

Return graph health metrics. The typed payload is `structured_content` (always whole); the text channel is pretty JSON, becoming chunkable markdown only past `token_budget` (page via `chunk`). Default: summary counts (entities, orphans, stubs, stale, missing-fields, communities; also per-schema via `orphans_by_schema`/`communities_by_schema`, raw totals kept), node/edge totals, type/edge distributions, `writable_mems` + `read_mems` roster, `default_writable_mem` (omitted-`mem` target), and `mem_schemas`. Pass `include` to drill in — allowed keys: `orphans`, `stubs`, `most_connected`, `missing_fields`, `stale`, `dangling_links`, `tags`, `missing_required_outgoing`, `conformance`, `integrity`. `conformance` lints entities against `target_schema` or each mem's pin into `findings` (`{id, axis, code, detail}`, write-time typed codes); `integrity` adds `DANGLING_LINK`/`ORPHAN_STUB`. `dangling_links` scans bodies for `[[id]]` refs lacking on-disk files; entries carry `from`, `target_id`, `target_path`, `section`. `tags` aggregates authored tag strings into `tag_distribution` (count desc, capped by `limit`), `tag_distribution_folded` (drift sidecar; entries when ≥2 casings share a canonical tag), and `untagged_entities`. `missing_required_outgoing` lists entities with unsatisfied `required_outgoing` blocks (`id`/`title`/`entity_type`/`mem`/`missing[]`). `most_connected` entries carry `total`/`incoming`/`outgoing` (all edges) plus `typed_total`/`typed_incoming`/`typed_outgoing` (mention edges excluded); ranked by `typed_total` then `total` then id, so a co-mention hub doesn't outrank a dependency hub. Unknown include keys emit `UNKNOWN_INCLUDE_KEY` on warnings. `limit` caps `most_connected`/`tag_distribution` at 10; above 100 clamped via `LIMIT_CLAMPED`. `SUSPICIOUS_NESTED_PREFIX` flags nested-prefix drift (fix via memstead_update). `DUPLICATE_SECTION_HEADING` flags a section key whose `## Heading` was declared twice (first body kept). `OUTER_REPO_NOT_IGNORING_MEM_REPO` surfaces when the workspace is embedded in another git checkout not ignoring `mem-repo/`. `MEM_RELOADED` flags an auto-reload after a sibling writer advanced the on-disk HEAD. Pass `mem` to scope counts/details to one writable mem; roster fields stay global. Under a filter, edge counts are source-in-mem only, `dangling_links` and `warnings` filter to in-filter entities. Set `include_config: true` to add `mutations` (`require_notes`), opaque `plugin` map, and a `mems` detail array: per entry `origin`, optional `vcs` (`gitdir`/`worktree`/`head`), opaque `write_guidance` map, and `extra` (forward-compat catch-all).

**Hints:** `read_only` = true, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for memstead_health.",
  "properties": {
    "chunk": {
      "description": "Which chunk of the rendered-markdown text channel to read (1-based). Only needed when a multi-include report exceeds the token budget. `structured_content` is whole regardless.",
      "format": "uint",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "include": {
      "description": "Detail sections to include (default: none — summary counts only). Allowed keys: orphans, stubs, most_connected, missing_fields, stale, dangling_links, tags, missing_required_outgoing, conformance, integrity. `conformance` lints every entity against the effective schema and returns per-entity `findings` (`{id, axis, code, detail}` with write-time typed codes); `integrity` additionally projects the consistency axis (dangling links, stubs) into the same findings list. Unknown keys surface as UNKNOWN_INCLUDE_KEY on warnings.",
      "items": {
        "type": "string"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "include_config": {
      "default": false,
      "description": "When true, the response carries the `[mutations]` posture (`mutations.require_notes`), the opaque `[plugin.*]` pass-through map, and a per-writable-mem `mems` detail array with `{ name, origin, vcs: { gitdir, worktree, head } }` — absolute canonical paths plus the cached branch-tip SHA (omitted on fresh mems with no commits yet) for the Stop-hook / reconcile flows so they never hardcode a layout or peel refs themselves. Defaults to false — the absence of these fields is the default-posture signal. **Lifecycle policy** (`[[mem_management.create]]` / `[[mem_management.delete]]`) is surfaced via `memstead_overview`, not here — `memstead_health` is drift/diagnostics.",
      "type": "boolean"
    },
    "limit": {
      "description": "Max results for most_connected (default: 10, max: 100)",
      "format": "uint",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "mem": {
      "description": "Scope counts, distributions, and detail lists to a single writable mem. `writable_mems`/`read_mems` still show the full roster so the agent sees the whole workspace. Omit (default) for global aggregates.",
      "type": [
        "string",
        "null"
      ]
    },
    "target_schema": {
      "description": "Schema ref (`name@x.y.z`) the `conformance`/`integrity` includes lint against instead of each mem's current pin. Omit (default) to lint against the current pin. Only consulted when `include` requests the conformance axis; an unresolvable ref refuses with SCHEMA_NOT_FOUND.",
      "type": [
        "string",
        "null"
      ]
    },
    "token_budget": {
      "description": "Max tokens for the rendered-markdown text channel. If the report exceeds this, the text returns chunk 1 of N with `_chunk`/`_total_chunks`/`_truncated` frontmatter; page with the `chunk` param. The `structured_content` envelope is never chunked — it always ships whole. Omit to use the server's configured default budget.",
      "format": "uint",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    }
  },
  "title": "HealthParams",
  "type": "object"
}
```

## `memstead_mem_create`

**Flavour:** full only

Create and register a new writable mem at runtime. Requires workspace opt-in via `[[mem_management.create]]` rules (each `pattern` + `schemas[]`) — discover via `memstead_overview`'s `## Lifecycle Namespaces`. Engine composes the lifecycle candidate, canonicalizes `location`, runs first-match-wins glob over the rule list, then checks `schema` against the matched rule's `schemas[]` (`["*"]` admits any). Two error envelopes: `MEM_PATH_NOT_ALLOWED` carries `details.candidate`, `details.patterns`, `details.reason` (`no_allowlist_configured` / `no_match` / `outside_workspace`); `MEM_SCHEMA_NOT_ALLOWED` carries `details.candidate`, `details.matched_pattern`, `details.requested_schema`, `details.allowed_schemas`. Name-collision check runs only after a path match — out-of-namespace collision surfaces as `MEM_PATH_NOT_ALLOWED`, not `MEM_NAME_COLLISION`. Storage-residue probe catches residue surviving a prior `memstead mem unregister` or a crash; residue left by a deliberate unregister reattaches and emits `MEM_REATTACHED_AFTER_UNREGISTER` (audit signal); residue from a crash refuses with `MEM_STORAGE_RESIDUE_DETECTED` — run `memstead mem delete <name>` first. Cross-mem edge authorization is workspace policy (`[cross_mem_links]`); the matched create-rule may carry `default_cross_links`. Bootstraps the gitdir per `vcs`, loads any pre-existing markdown, and produces a seed commit carrying `note` (≤280 chars). Response carries `location`, `seed_commit_sha` for `memstead_changes_since` polling, and `schema_ref` (gitdir via `memstead_health include_config=true`). Pass `include_schema: true` to additionally inline the full schema body — byte-identical to `memstead_schema(name=<resolved-schema>)`. Default `false`. A mem already present at the location returns `CONFIG_ERROR`. Seed-commit failure leaves partial disk state — no implicit rollback.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$defs": {
    "RecoveryActionInput": {
      "description": "Wire-shape recovery action for `memstead_mem_create`. The\nstorage-residue refusal path exposes three explicit\nrecovery options the caller picks via this enum. The wire\ntokens (`reattach` / `force_overwrite` / `hard_cleanup_first`)\nmatch `memstead_engine::RecoveryAction::as_wire_str()` so the\nMCP serde shape and the CLI flag bridge converge on a single\nengine-side enum.",
      "oneOf": [
        {
          "const": "reattach",
          "description": "Adopt the residual entities; skip the seed commit. Default\nwhen the residue was left by a deliberate `memstead mem\nunregister`. Explicit `reattach` overrides the default for\ncrash-residue scenarios where the operator has verified the\ncontent is safe to adopt.",
          "type": "string"
        },
        {
          "const": "force_overwrite",
          "description": "Destroy the residue, then proceed with the normal create\npath. Prior entities are gone. **Not yet implemented** — the\norchestrator currently refuses with `INVALID_INPUT` pointing\nat `memstead mem delete <name>`.",
          "type": "string"
        },
        {
          "const": "hard_cleanup_first",
          "description": "Refuse with `MEM_STORAGE_RESIDUE_DETECTED`, instructing the\ncaller to run `memstead mem delete <name>` first. Hard barrier\nagainst destructive auto-recovery — for operators who want\nthe cleanup to be a separate, named operation.",
          "type": "string"
        }
      ]
    },
    "VcsConfigInput": {
      "additionalProperties": false,
      "description": "On-the-wire shape mirroring `memstead_schema::VcsConfig` with a\n`JsonSchema` derivation for rmcp tool routing. Kept separate from the\ncore type so the schema crate does not need a `schemars` dependency\njust to support one MCP-facing parameter. The fields and semantics\nmatch 1:1 — see `memstead_schema::VcsConfig` for the canonical\ndocumentation.",
      "properties": {
        "gitdir": {
          "description": "Path to the gitdir relative to the new mem's root.",
          "type": "string"
        },
        "worktree": {
          "default": ".",
          "description": "Path to the worktree relative to the new mem's root. Defaults to `\".\"` (mem root) when omitted.",
          "type": "string"
        }
      },
      "required": [
        "gitdir"
      ],
      "type": "object"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for `memstead_mem_create`.",
  "properties": {
    "include_schema": {
      "default": false,
      "description": "Inline the full resolved schema body on the response (byte-identical to `memstead_schema(name=<resolved-schema>)`). Default `false` — the response carries only `schema_ref`, `name`, `location`, and `seed_commit_sha`. Set to `true` for first-time-schema callers that want one round-trip instead of two; for the agent's second+ mem on the same schema this opt-in saves ~25 KB of context per call since the schema is workspace-stable and already cached.",
      "type": "boolean"
    },
    "location": {
      "description": "Target filesystem location. Absolute path, or relative to the workspace root. Canonicalized before the allowlist check — `./a/../b` is reduced to `./b` prior to matching.",
      "type": "string"
    },
    "name": {
      "description": "Unique name for the new mem — the full hierarchical identifier (e.g. `\"sub-mem\"` for flat layouts or `\"team/sub-mem\"` for hierarchical layouts); the value flows through verbatim. Grammar: lowercase ASCII letters, digits, hyphens; segments separated by `/`; no leading, trailing, or double slashes. Must not collide with any currently-registered mem.",
      "type": "string"
    },
    "note": {
      "description": "Agent-authored provenance note recorded in the seed commit's body (≤280 chars). One sentence describing why this mem was created.",
      "type": [
        "string",
        "null"
      ]
    },
    "recovery": {
      "anyOf": [
        {
          "$ref": "#/$defs/RecoveryActionInput"
        },
        {
          "type": "null"
        }
      ],
      "description": "Explicit recovery action when on-disk storage residue is detected at the composed branch path. Three accepted values: `reattach` (adopt the residual entities, skip the seed commit), `force_overwrite` (destroy the residue, currently refuses with `INVALID_INPUT` — implementation pending), `hard_cleanup_first` (refuse with `MEM_STORAGE_RESIDUE_DETECTED`, instructing the caller to run `memstead_mem_delete` first). When omitted, the engine routes by whether the residue was left by a deliberate `memstead mem unregister`: such residue defaults to `reattach` and emits a `MEM_REATTACHED_AFTER_UNREGISTER` warning; residue from a crash refuses with `MEM_STORAGE_RESIDUE_DETECTED`. Bare create against a name with no residue ignores this field."
    },
    "schema": {
      "description": "Schema pin for the new mem. Format: `name@x.y.z` — e.g. `default@1.0.0`. Resolved against the per-mem schema registry at init time.",
      "type": "string"
    },
    "schema_verbosity": {
      "description": "Verbosity of the inlined schema body when `include_schema: true`. `\"full\"` (default, absent) inlines the complete schema — byte-identical to `memstead_schema(name=<resolved-schema>)`. `\"lite\"` inlines the cheap cold-start skeleton instead (entity-type names + section keys + field shapes, relationship names + endpoints, the alias pointer; prose dropped) — the recommended pairing for a first-mem create that only needs to orient. Ignored when `include_schema` is false. Any value other than `\"full\"`/`\"lite\"` returns `INVALID_INPUT` naming the bad value.",
      "type": [
        "string",
        "null"
      ]
    },
    "vcs": {
      "anyOf": [
        {
          "$ref": "#/$defs/VcsConfigInput"
        },
        {
          "type": "null"
        }
      ],
      "description": "Optional VCS layout override. Shape: `{ \"gitdir\": \".git\", \"worktree\": \".\" }` (default isolated) or `{ \"gitdir\": \"../.git\", \"worktree\": \"..\" }` (shared-gitdir idiom). Paths are relative to the new mem's root. When absent, the engine uses the isolated default."
    },
    "write_guidance": {
      "additionalProperties": true,
      "default": {},
      "description": "Optional per-instance writing guidance, written verbatim into the new mem's config `writeGuidance` map in the seed commit. An opaque string-keyed JSON object — e.g. `{ \"phase_context\": \"early design\", \"stack\": \"Rust\" }`. The engine never interprets the keys (schema-strictness D8 — `writeGuidance` is client-owned vocabulary); a client that read the resolved schema package's `mem-template.json` fills the instance keys and passes them here. Omit (or pass `{}`) to seed no guidance.",
      "type": "object"
    }
  },
  "required": [
    "name",
    "location",
    "schema"
  ],
  "title": "MemCreateParams",
  "type": "object"
}
```

## `memstead_mem_delete`

**Flavour:** full only

Remove a writable mem at runtime — always destructive: removes the mem and prunes every backend-visible artifact. Requires workspace opt-in via `[[mem_management.delete]]` rules — discover the current policy via `memstead_overview`'s `## Lifecycle Namespaces` section. Engine resolves `name` (`UNKNOWN_MEM` otherwise), composes the lifecycle candidate from the mem's full hierarchical path (or the bare name for flat-layout mems), runs first-match-wins glob lookup over the delete rule list (rejecting `no_allowlist_configured` or `no_match` with `MEM_PATH_NOT_ALLOWED`; `details.candidate` carries the composed string, `details.patterns` lists rules checked, `details.reason` discriminates). Refuses `MEM_REFERENCED_BY_POLICY` when the workspace `cross_mem_links` policy grants this mem as a write target (`details.referring_mems` names them). Refuses `MEM_HAS_INCOMING_REFS` when write-mem graph edges still target it (`details.referrers` lists each `{from_id, rel_types, mem}` — remove via `memstead_relate` / `memstead_update` first). On success the mem is gone — reads no longer see it and its backing storage is removed. The workspace policy is atomically scrubbed of the now-dangling `[cross_mem_links]` grants naming the deleted mem on either side. The `[[mem_management.create]]` / `[[mem_management.delete]]` allowlist rules are PRESERVED (exact-name and wildcard alike) — they are forward-looking permissions for the name, so re-creating a mem of the same name needs no fresh allow-create/allow-delete. No per-mem commit — `note` (≤280 chars) rides on the provenance context. Response: `name`, `deleted_from_router: true`, `files_deleted: true`, and `allowlist_entries_removed[{table, pattern?, from?, to?}]` listing the scrubbed cross-link grants (`table` is always `cross_mem_links`; empty when none named the mem). On partial cleanup failure `files_deleted` ends `false` and `MEM_FILES_NOT_DELETED` warnings name the survivors: `details.reason` is `rmdir_failed` (with `details.path` + `details.error`) or `backend_prune_failed` (with `details.error`).

**Hints:** `read_only` = false, `destructive` = true, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for `memstead_mem_delete`.\n\nThe MCP surface collapses to one verb that always means destructive.\nThe earlier `delete_files: bool` parameter retired — agents have no legitimate\nneed to \"preserve storage but unregister\"; the router-only\nunregister-preserve-storage workflow stays reachable via the CLI's\n`memstead mem unregister` verb (operator-only). The MCP wrapper\nhardcodes `delete_files: true` when invoking the engine, so the\npromised refusals (`MEM_REFERENCED_BY_POLICY`,\n`MEM_HAS_INCOMING_REFS`) and the policy scrub on success always\nfire.",
  "properties": {
    "name": {
      "description": "Name of the mem to destroy.",
      "type": "string"
    },
    "note": {
      "description": "Agent-authored provenance note (≤280 chars). Surfaces in the outer-repo Stop-hook aggregation via the engine's trace surface; no per-mem commit is produced by delete.",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "required": [
    "name"
  ],
  "title": "MemDeleteParams",
  "type": "object"
}
```

## `memstead_mem_set_schema`

**Flavour:** full only

Update a mem's schema pin — the integrity-driven schema-migration trigger. Stable response `{mem, schema_pin, migration_target, outcome, findings}`; branch on `outcome`: `noop` (requested == current pin), `switched` (mem already integral against the target — pin moved atomically), `migration_started` (not integral — mem enters dual-pin: writes now validate against the target, `findings` lists the non-integral entities as `{id, axis, code, detail}`), `migration_pending` (same target re-issued while repairs remain — `findings` carries the remaining entities). Migration loop: read `findings`, read both schemas via `memstead_schema`, repair each entity via `memstead_update` (validated strictly against the target; `relations_unset` is available on non-conformant entities), then re-issue this call — once every entity is integral it completes the switch. Reads stay permissive throughout; the dual-pin state survives engine restarts. Unknown mem refuses `UNKNOWN_MEM`; a schema ref that resolves to no loaded schema refuses `SCHEMA_NOT_FOUND`; malformed refs refuse `INVALID_INPUT`. Distinct from `memstead_mem_set_version`, which sets the mem *content* version, never the pin.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for `memstead_mem_set_schema` — the integrity-driven\nschema-migration trigger.",
  "properties": {
    "mem": {
      "description": "Name of the writable mem whose schema pin is being set.",
      "type": "string"
    },
    "note": {
      "description": "Optional provenance note (≤280 chars). Reserved: the pin lives in workspace state today (no mem commit is produced), so the note is accepted for wire-compat and recorded once the pin-relocation cut moves the schema pin into mem config.",
      "type": [
        "string",
        "null"
      ]
    },
    "schema": {
      "description": "Target schema ref, exact `name@x.y.z`. Must resolve against the loaded schema catalogue (mem-pinned, workspace, built-in); unresolvable refs refuse with SCHEMA_NOT_FOUND, malformed refs with INVALID_INPUT.",
      "type": "string"
    }
  },
  "required": [
    "mem",
    "schema"
  ],
  "title": "MemSetSchemaParams",
  "type": "object"
}
```

## `memstead_mem_set_version`

**Flavour:** full only

Update a registered mem's `version` field. The version is consumed by `memstead_export --format mem` to stamp the archive filename and the `.mem` archive's published config — bump before publishing. Mem-create seeds `0.1.0` automatically, so this tool is the only surface that needs to fire when an agent or operator is ready to ship a new version. Gate-free: no `[[mem_management.*]]` allowlist check, no operator-mode bypass needed. Validates the new version as semver; malformed values refuse with `INVALID_INPUT`. Unknown mem name refuses with `UNKNOWN_MEM`; read-only mem refuses with `READ_ONLY_MOUNT`; a mem whose config failed to load returns `INVALID_INPUT`. Response carries `{mem, old_version, new_version, warnings}`; `MEM_RELOADED` rides on `warnings` when a sibling engine commit landed between the engine's prior snapshot and this write (no extra read needed to learn the drift).

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for `memstead_mem_set_version`. F1.",
  "properties": {
    "name": {
      "description": "Name of the mem whose `version` field is being updated.",
      "type": "string"
    },
    "note": {
      "description": "Optional provenance note (≤280 chars) recorded on the version-bump commit body. When the workspace sets `require_notes`, omitting it rides a non-blocking `NOTE_MISSING` warning (the bump still lands).",
      "type": [
        "string",
        "null"
      ]
    },
    "version": {
      "description": "New semver version (e.g. `0.2.0`, `1.0.0-beta.1`). Validated as semver; malformed values refuse with `INVALID_INPUT`. The version is consumed by `memstead_export --format mem` to stamp the archive filename and the `.mem` archive's published config — bump before publishing. Initial mem-create seeds `0.1.0` so this surface is the only path that needs to be invoked when an agent or operator is ready to ship.",
      "type": "string"
    }
  },
  "required": [
    "name",
    "version"
  ],
  "title": "MemSetVersionParams",
  "type": "object"
}
```

## `memstead_overview`

**Flavour:** lean + full

Start here. Returns the schema catalogue, mem inventory, and community clusters as markdown. Schemas list as `{ref, description}` only — call `memstead_schema(name=<ref>)` for full per-type bodies (sections, fields, relationship vocabulary, write_rules) before any `memstead_create` / `memstead_update` / `memstead_relate`; cache per session, schema is workspace-stable. Token-budget-driven: hard-required content (mem roster, schema refs, community titles, workspace policy) always ships; heavy content is greedy-filled into the remaining budget by default-priority. Anything that didn't fit appears in the `## Hints` section with `estimated_tokens`; re-query by passing `key` into `include[]`. Override priority with `include`: keys there always ship, even past budget. Allowed `include` keys: `community_members`, `community_bridges`, `mem_distribution`, `dangling_links`. Control the budget via `token_budget` (default 8000). Frontmatter `_overview_mode` is `"complete"` (nothing dropped), `"reduced"` (heavy content omitted — see the Hints section), or `"overbudget"` (hard-required content alone exceeded the budget; raise `token_budget` or scope with `mem`). Workspace-level mutation and link policy is surfaced in `## Workspace policy` and mirrored into the `_policy` frontmatter slot — entries appear only when the value deviates from the engine default (`require_notes`, `cross_mem_links` posture). Pass `mem` to scope mems and schemas to one writable mem. Community detection is workspace-global: `mem` scopes which clusters are *reported* (and makes `community_bridges` source-in-mem only, asymmetric — matches memstead_health), but never re-runs detection per mem and never renumbers cluster ids, so a small or disconnected mem-local subgraph may surface as no cluster (sparsely-connected / edge-less nodes collapse into one catch-all rather than forming their own). `rebuild: true` recomputes that same global Louvain partition. Non-fatal issues surface under `## Warnings` with a stable `code`. Use memstead_schema for full schema bodies; memstead_entity to read a specific entity; memstead_search to find IDs; memstead_health for node/edge counts.

**Hints:** `read_only` = true, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for memstead_overview.",
  "properties": {
    "chunk": {
      "description": "Which chunk to read (1-based). Only needed if overview exceeds the MCP response cap.",
      "format": "uint",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "include": {
      "description": "Opt into heavy content. Allowed keys: \"community_members\" (entity lists per cluster), \"community_bridges\" (inter-cluster edge aggregation with up to 3 sample edges per pair), \"mem_distribution\" (per-mem type_distribution), \"dangling_links\" (renders a `## Dangling Links` section listing each unresolved body wiki-link as `source → target (in section)`; richer aggregation tracked in #12/#13). `include` keys are always shipped regardless of the token budget — use it to force content you need. Unknown keys emit a typed `warnings` entry. Schema bodies are not in this set — call memstead_schema(name=...) for the full per-type catalogue.",
      "items": {
        "type": "string"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "mem": {
      "description": "Restrict `mems[]` and `schemas[]` to a single writable mem. `used_by` inside each schema still lists all mems sharing it. Community scope: `mem` filters which clusters are *reported* (and makes `community_bridges` source-in-mem only) — it does NOT re-run detection per mem. Detection is always workspace-global and cluster ids stay the global-pass ids; passing `mem` never renumbers or re-scopes the partition. Because detection is global and disconnected / sparsely-connected nodes collapse into a single catch-all rather than forming their own cluster, a small or isolated mem-local subgraph may surface as no cluster at all under a `mem` filter.",
      "type": [
        "string",
        "null"
      ]
    },
    "rebuild": {
      "description": "Re-run community detection before returning overview (default: false). Detection is workspace-global: `rebuild` recomputes the Louvain partition over the *whole* workspace graph — it never scopes to `mem`, even when `mem` is also passed.",
      "type": [
        "boolean",
        "null"
      ]
    },
    "token_budget": {
      "description": "Target token budget for heavy content only (`community_members`, `community_bridges`, `mem_distribution`, `dangling_links`). Default: 8000. Hard-required content (mem roster, schema refs with relationship vocabulary, community titles, workspace policy) always ships in addition — total response size will exceed this budget. When hard-required content alone exceeds the budget, `overview_mode=\"overbudget\"` signals the agent to raise the budget or scope via `mem`. Heavy content not in `include` is greedy-filled until the budget is exhausted; anything left over is advertised in `hints[]` with `estimated_tokens`. `include` keys bypass the budget. Budgets below ~10 tokens are safe but unproductive — the structured envelope still arrives (`overview_mode=\"overbudget\"`) but no useful chunking happens and the full body ships as one chunk.",
      "format": "uint",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    }
  },
  "title": "OverviewParams",
  "type": "object"
}
```

## `memstead_relate`

**Flavour:** lean + full

Connect two entities with a typed edge. Pre-fetch the target mem's schema via `memstead_schema(name=<mem.schema_ref>)` once per session — relationship vocabulary and shape live there. Type names are case-insensitive; stored canonically as UPPER_SNAKE_CASE. Unknown rel-types return `INVALID_REL_TYPE` with `details.allowed` (each `{name, when_to_use}`) and nearest-match `suggestion`. Shape pinned via `source_types` / `target_types` — add-path violations return `INVALID_REL_SHAPE` with `details.rel_type`, `details.from_type`, `details.to_type`, `details.allowed_source_types`, `details.allowed_target_types`, `suggestion`. Remove skips shape validation; existing violations surface via `memstead_health`. Pass `remove: true` to delete an edge. Source (`from`) must be real; target (`to`) may be auto-stubbed (wiki-link slug grammar — malformed ids return `INVALID_ENTITY_ID` with `details.id` / `details.reason`). Cross-mem edges policy-gated by `cross_mem_links` / `default_cross_links`: denial returns `CROSS_MEM_LINK_NOT_ALLOWED`; absent ReadOnly targets return `CROSS_MEM_TARGET_NOT_FOUND`; cross-different-schema edges undeclared in `cross_mem_relationships` return `CROSS_MEM_EDGE_NOT_DECLARED`. Auto-stubs into an uncreated mem emit `CROSS_MEM_TARGET_MEM_UNCREATED`. Cycle-closing edges on `acyclic: true` types return `RELATIONSHIP_CYCLE` with `details.rel_type`, `details.from`, `details.to`, `details.existing_path`, `details.path_truncated`. Add-existing / remove-missing are typed-warning no-ops (`DUPLICATE_RELATIONSHIP` / `NO_SUCH_RELATIONSHIP`, empty `commit_sha`). Optional `note` (≤280 chars) — see memstead_create. Response `_hash` is next mutation's `expected_hash`. Edges never move files — entities live at `{mem}/{slug}.md`. On `remove: true`, a stub whose last incoming edge dropped is GC'd and listed in `orphan_stubs_removed`; surviving body wiki-links refuse with `RELATION_HAS_BODY_LINKS` (`details.body_links` — drop them via `memstead_update` and retry). Real-writes carry `commit_sha` (per-mem git; gitdir via `memstead_health include_config=true`) for polling via memstead_changes_since.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for memstead_relate.",
  "properties": {
    "description": {
      "default": null,
      "description": "Optional per-edge description applied on add. Validated against the rel-type's `per_edge_description` posture in the pinned schema: `forbidden` (default) rejects a non-empty description with `DESCRIPTION_NOT_PERMITTED`; `required` rejects its absence with `MISSING_REQUIRED_DESCRIPTION`; `optional` accepts both. Empty / whitespace-only strings normalise to absent before validation. Ignored on the remove path.",
      "type": [
        "string",
        "null"
      ]
    },
    "from": {
      "description": "Full source entity ID",
      "type": "string"
    },
    "note": {
      "description": "Agent-authored provenance note (≤280 chars, one sentence describing why this mutation happened). Lands in the per-mem commit body between the mechanical subject line and the provenance trailers (`Tool:`, `Actor:`, `Client:`), and is surfaced by the outer-repo Stop hook when aggregating session activity. Omit for pure-housekeeping edits; when `[mutations].require_notes = true` in workspace config a missing note adds a `NOTE_MISSING` `WarningHint` to the response (the mutation still commits).",
      "type": [
        "string",
        "null"
      ]
    },
    "remove": {
      "description": "Set true to remove the relationship instead of creating it",
      "type": [
        "boolean",
        "null"
      ]
    },
    "to": {
      "description": "Full target entity ID",
      "type": "string"
    },
    "type": {
      "description": "Relationship type. Canonical form is UPPER_SNAKE_CASE (USES, PART_OF, DEPENDS_ON) and is what the engine stores; case-insensitive inputs (`uses`, `Part_Of`) are accepted and echoed back in the response as their canonical form. The JSON Schema `pattern` advertises `^[A-Za-z][A-Za-z_]*$` for client-side validators; the engine enforces the same character set independently — characters outside it return `INVALID_REL_TYPE` at the engine boundary regardless of whether the client pre-filters.",
      "pattern": "^[A-Za-z][A-Za-z_]*$",
      "type": "string"
    }
  },
  "required": [
    "from",
    "to",
    "type"
  ],
  "title": "RelateParams",
  "type": "object"
}
```

## `memstead_reload`

**Flavour:** full only

Reload one writable mem's slice of the in-memory store from its on-disk branch tip — or every writable mem when `mem` is omitted. For multi-engine coexistence: a sibling (forked subagent, macOS app, parallel terminal) or out-of-band `git pull` may have advanced HEAD past this engine's snapshot. The auto-reload-on-read pipeline surfaces `MEM_RELOADED` on the next read; this tool is explicit operator-driven refresh for the rare cases the throttle missed. Not a workaround for direct .md edits — restart the server instead. Per-mem form is cheap (~10 ms per few-hundred-entity mem); workspace-wide scales linearly. Response: `reports[]`, each entry `{ mem, head_before, head_after, entities_loaded, changed_entity_ids[] }`. `head_before` is the engine's prior cached SHA (canonical empty-tree hash for fresh mems); `head_after` is the freshly-peeled branch tip. `changed_entity_ids` is the union of added ∪ content-hash-changed ∪ removed entity ids — pass `head_before` to `memstead_changes_since` for the full per-entity diff. The workspace-wide form (omit `mem`) additionally picks up CLI writes to allowlist / cross-link / mutation policy (via `memstead workspace allow-create` etc.) without process restart. Per-mem form skips that workspace-level settings refresh. **Mem membership is fixed at process boot** — neither form re-scans the mount manifest. In-band lifecycle goes through `memstead_mem_create` / `memstead_mem_delete`; out-of-band creates / deletes require an MCP server restart.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for memstead_reload.",
  "properties": {
    "mem": {
      "description": "Writable mem name to reload. Omit to reload every writable mem. Use the per-mem form for cheap, targeted refreshes when you know which mem drifted; use the workspace-wide form (omit `mem`) when an out-of-band `git pull` may have advanced multiple branches at once, or to pick up CLI-driven workspace-policy edits (allowlist / cross-link / mutation policy) — per-mem reload skips that workspace-level settings refresh.",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "title": "ReloadParams",
  "type": "object"
}
```

## `memstead_rename`

**Flavour:** lean + full

Rename an entity by changing its title. Updates the entity ID (mem prefix preserved) and its markdown file path (`{new_slug}.md` at mem root). Atomic referrer rewrite: every Write-Mem entity whose `relationships` or section bodies point at the old id has its `[[old-slug]]` tokens rewritten in one per-mem commit. Cross-mem referrers are gated by `cross_mem_links` policy in the propagated edge's actual direction (`referrer_mem → renamed_mem`) — a blocked direction aborts up-front with `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` (`details.from_mem`, `details.blocked_referrers[{from_mem, to_mem, count}]`) before any write lands. Per-peer commits are parent-pinned; sibling-writer drift mid-rename surfaces `RENAME_PARTIAL_FAILURE` (`details.committed_mems`, `details.failed_mem`, `details.failure_cause`) so the agent retries the failed mem after reloading. Every per-mem commit in one rename shares a `logical_operation_id` in its provenance — correlate multi-mem renames via `memstead_changes_since`. ReadOnly referrers can't be rewritten; the old id is demoted to a stub in-memory holding the surviving incoming edges, and the response carries `RESIDUAL_STUB_FOR_READONLY_REFERRERS` naming each surviving referrer. Requires `expected_hash` (read via memstead_entity first); mismatch emits `HASH_MISMATCH` with `details.current` carrying the current on-disk hash. Slug-noop short-circuit: when the new title's slug matches the current one, `old_id` equals `new_id`, `commit_sha` is empty, and `warnings` carries `TITLE_NORMALIZED_TO_SLUG_NOOP`. ID collisions error — pick a different title. Stubs cannot be renamed (create a real entity instead). Optional `note` (≤280 chars) — shared provenance contract, see memstead_create. Response carries `old_id`, `new_id`, `_hash` (post-rename on-disk hash — pass as `expected_hash` on the next mutation, mirrors `memstead_relate`), `commit_sha` (per-mem git; gitdir via `memstead_health include_config=true`), and `warnings`.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for memstead_rename.",
  "properties": {
    "expected_hash": {
      "description": "Hash from memstead_entity (_hash). Required. Mismatch returns code HASH_MISMATCH with details.current carrying the current on-disk hash.",
      "type": "string"
    },
    "id": {
      "description": "Full current entity ID",
      "type": "string"
    },
    "new_title": {
      "description": "New title for the entity",
      "type": "string"
    },
    "note": {
      "description": "Agent-authored provenance note (≤280 chars, one sentence describing why this mutation happened). Lands in the per-mem commit body between the mechanical subject line and the provenance trailers (`Tool:`, `Actor:`, `Client:`), and is surfaced by the outer-repo Stop hook when aggregating session activity. Omit for pure-housekeeping edits; when `[mutations].require_notes = true` in workspace config a missing note adds a `NOTE_MISSING` `WarningHint` to the response (the mutation still commits).",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "required": [
    "id",
    "new_title",
    "expected_hash"
  ],
  "title": "RenameParams",
  "type": "object"
}
```

## `memstead_schema`

**Flavour:** lean + full

Read one schema's full body — section list (with per-section `write_rules` and `required` flag), metadata fields (with `enum` allowed values + `default` when schema-declared), type-level `writing_guidance`, `system_context`, the relationship vocabulary (each entry's `name`, `description`, `when_to_use`, `default_weight`), `community.{resolution, seed}`, `relationship_mode` (strict|open), `used_by[]`, top-level `origin` (`first-party` for an engine built-in or a schema authored/trusted in this workspace; `third-party` otherwise), top-level `default_writing_guidance` (when authored), and top-level `alias_target_rel_type` (when authored — names the rel-type that body wiki-links `[[target]]` auto-emit through the alias-synthesis pass; absent means the schema opts out and unbacked wiki-links refuse with `WIKILINK_WITHOUT_RELATION`). A `third-party` schema is served structural-only regardless of `verbosity` — its prose-instruction fields (`system_context`, `writing_guidance`, `write_rules`, `when_to_use`, prose `description`) are omitted so a stranger's free-text never reaches the agent as instructions. Pass exactly one of: `name` — a bare name ("default") or canonical pin ("default@1.0.0"); `mem` — a mem name whose pinned `mem.schema_ref` the engine resolves from the workspace's mount roster. Supplying both returns `INVALID_INPUT`; supplying neither returns `INVALID_INPUT`. Workflow: each writable mem pins one schema (see `memstead_overview`'s `## Schemas` and `## Mems` sections). Before any `memstead_create` / `memstead_update` / `memstead_relate` against mem X, call this tool with `mem=<X>` (or `name=<X.schema_ref>`) once per session to learn section names, field shapes, and write_rules. Cache for the session — schema is workspace-stable. Schema-conformance errors carry recovery payloads as a fallback (`UNKNOWN_SECTION`, `UNKNOWN_METADATA_FIELD`, `INVALID_ENUM_VALUE`, `REQUIRED_FIELD_UNSET`, `INVALID_REL_TYPE`); fix from `details` rather than re-fetching. Returns `ENTITY_NOT_FOUND` when `name` is unknown (envelope's `details.id` echoes the name; `details.suggestions` is empty for schemas) or `UNKNOWN_MEM` when `mem` is not mounted (envelope's `details.known_mems` lists the writable roster).

**Hints:** `read_only` = true, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for memstead_schema. Exactly one of `name` or `mem`\nmust be supplied; passing both is an `INVALID_INPUT` error.",
  "properties": {
    "mem": {
      "description": "Mem name as listed in memstead_overview's `## Mems` section. The engine resolves the mem's pinned `schema_ref` from the workspace's mount roster and proceeds identically to the `name`-driven path. Mutually exclusive with `name`. Returns `UNKNOWN_MEM` when the mem is not mounted.",
      "type": [
        "string",
        "null"
      ]
    },
    "name": {
      "description": "Schema name as listed in memstead_overview's `## Schemas` section (e.g. \"default\" or \"default@1.0.0\"). Schemas are workspace-globally unique by name; the workspace registry resolves a bare name to the pinned version. Mutually exclusive with `mem`.",
      "type": [
        "string",
        "null"
      ]
    },
    "verbosity": {
      "description": "Verbosity of the schema body. `\"full\"` (default, absent) returns the complete payload — every description, `when_to_use`, write-rule, and writing-guidance string. `\"lite\"` returns a cheap cold-start skeleton: entity-type names with their section keys (and `required` markers) and metadata-field shapes (name, `required`, `enum`, `default`), relationship-type names with their `allowed_sources`/`allowed_targets`, `manual_authoring`, `acyclic`, and `per_edge_description` — plus the top-level `alias_target_rel_type` pointer — with the long-form prose dropped. The lite skeleton carries every flag needed to author a legal write; escalate to full only for the human-readable guidance. Heavy arrays ship under distinct keys per mode (`types`/`relationships` vs. `types_summary`/`relationships_summary`). Any value other than `\"full\"`/`\"lite\"` returns `INVALID_INPUT` naming the bad value.",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "title": "SchemaParams",
  "type": "object"
}
```

## `memstead_search`

**Flavour:** lean + full

Search entities by lexical content + structural filters. Dual channel: text carries the rendered markdown (prose with score lines + frontmatter counters); `structured_content` carries the typed `SearchResultEnvelope` `{ _total, _returned, _offset, _total_tokens, hits[], facets, warnings }` where each hit ships `score`, `score_breakdown`, `matched_terms`, `expansion`, `origin` (`first-party`/`third-party` trust class), and `snippet` (section bodies via memstead_entity). A page is bounded to `token_budget` (default 12000); an overflowing page is trimmed with a `SEARCH_RESULTS_TRUNCATED` warning (`kept`/`budget`), `_total` stays the full count, page with `offset`. Warnings ride as structured `{code, details, message}` entries (same shape every other tool emits) — branch on `code`. The caller expands a concept into keyword variants. Put variants into `query.any` (OR — ranks higher matches automatically); add excludes to `query.not`; use `query.phrase` for exact adjacency; use `query.field` to restrict to a single field. Set `expand_via` to relationship types — reached hits surface with `expansion` metadata + decayed score (0.5^depth). `facets` (by_type, by_mem, by_level, by_status, by_confidence, by_subsection, by_expansion) compose results structurally. Sub-heading matches carry `heading_path`. `stub: true|false` filters by stub status (combining with `entity_type` flags `STUB_FILTER_EXCLUDES_ALL`). Equality filters on `filterable: equality` fields ride on `filters` (e.g. `{"level": "M0"}`); one code per outcome, branch on `code`: `FILTER_TYPE_SCOPED` (declared on other types — applied with type-narrowing), `FIELD_NOT_FILTERABLE` (declared but not filterable — ignored, result unfiltered not emptied), `UNKNOWN_FILTER_KEY` (no schema declares it — ignored), `INVALID_ENUM_VALUE` (value outside the field's `enum_values` — applies but matches nothing, `details.allowed` lists the values). A `related_to` neighbourhood is ranked by proximity (nearer first) and bounded with `NEIGHBOURHOOD_CAPPED`. Range filters on `filterable: range` fields ride on `range_filters` (`min_<field>`/`max_<field>`/`<field>_before`/`<field>_after`), same contract: `RANGE_FILTER_KEY_MALFORMED`, `RANGE_FILTER_TYPE_SCOPED`, `UNKNOWN_RANGE_FILTER_FIELD`, `FIELD_NOT_RANGE_FILTERABLE`. Per-mem search-index unavailability (missing index or search-index execution failure) surfaces `SEARCH_MEM_INDEX_UNAVAILABLE` with `details.mem` and `details.reason`. Omit `query` for a pure metadata filter.

**Hints:** `read_only` = true, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$defs": {
    "Query": {
      "description": "Flat query shape for full-text search. Four optional fields, all\ncombined with implicit AND across fields.\n\nWithin `any`: at least one term must match (OR semantics). Entities\nmatching more terms rank higher automatically — no explicit `and`.\nWithin `not`: none of the listed terms may appear. `phrase` requires\nexact adjacency (case- and diacritic-folded). `field` narrows the match\nregion for all three to a single indexed field; `None` = match anywhere\nindexed.\n\nEmpty/unset everywhere ⇒ no text predicate; `search` behaves as a\nmetadata-only filter (subsumes the former `list` semantics).\n\nNo stemming, wildcards, or regex — the caller expands morphology and\nsynonyms by enumerating variants in `any`.",
      "properties": {
        "any": {
          "description": "Terms where at least one must match (OR semantics).",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "field": {
          "description": "Restrict `any` / `not` / `phrase` to a single field (title or section\nkey). `None` = match anywhere indexed.",
          "type": [
            "string",
            "null"
          ]
        },
        "not": {
          "description": "Terms that must not match (exclusion).",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "phrase": {
          "description": "Exact phrase that must appear (case- and diacritic-folded).",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "type": "object"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for memstead_search.",
  "properties": {
    "depth": {
      "description": "Max hops from related_to (default: 1, ignored without related_to)",
      "format": "uint",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "edge_type": {
      "description": "Only entities having this edge type (e.g. IMPLEMENTS, USES)",
      "type": [
        "string",
        "null"
      ]
    },
    "entity_type": {
      "description": "Only entities of this type (e.g. \"spec\", \"memo\")",
      "type": [
        "string",
        "null"
      ]
    },
    "expand_depth": {
      "description": "Max hops to traverse via `expand_via` (default: 1).",
      "format": "uint",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "expand_via": {
      "description": "Relationship types to follow from primary hits to pull in graph-proximal neighbours (e.g. [\"REALIZES\", \"REFERENCES\"]). Expanded hits carry `expansion: { of, via_edge, depth }` and a decayed score (0.5^depth). `by_expansion` facet shows the primary/expanded composition.",
      "items": {
        "type": "string"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "filters": {
      "additionalProperties": {
        "type": "string"
      },
      "description": "Equality filters on schema-declared filterable fields, keyed by field name (e.g. `{\"level\": \"M0\", \"status\": \"active\", \"tags\": \"auth\", \"scope\": \"subsystem\"}`). Every field with `filterable: equality` in the type's schema is reachable here. One typed warning per outcome, branch on `code`: `FILTER_TYPE_SCOPED` (a *filterable* key declared only on other types — applied with strict type-narrowing), `FIELD_NOT_FILTERABLE` (declared but not filterable on any reachable type — ignored in both the scoped and unscoped case, result unfiltered not emptied), `UNKNOWN_FILTER_KEY` (no schema declares it — ignored), `INVALID_ENUM_VALUE` (a value outside the field's `enum_values` — the filter applies but matches nothing, so a 0-hit result isn't a true no-match; `details.allowed` lists the values). The per-field `level`/`status`/`confidence` parameters are retired — agents declare any filterable field uniformly through this map. Use `entity_type` (typed parameter) and `edge_type` (typed parameter) for the engine's first-class graph axes, not for metadata filters.",
      "type": [
        "object",
        "null"
      ]
    },
    "limit": {
      "description": "Max results to return (default: all, max: 200)",
      "format": "uint",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "mem": {
      "description": "Only entities in this mem",
      "type": [
        "string",
        "null"
      ]
    },
    "offset": {
      "description": "Skip first N results for pagination. Use with limit.",
      "format": "uint",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "query": {
      "anyOf": [
        {
          "$ref": "#/$defs/Query"
        },
        {
          "type": "null"
        }
      ],
      "description": "Structured flat query. Fields: `any: [terms]` (OR, ranks entities matching more terms higher — no explicit AND needed), `not: [terms]` (exclusion), `phrase: \"exact adjacency\"`, `field: \"title\"|section-key` (narrow all three). Omit (or pass `{}`) to use search as a pure structural/metadata filter — hits come back in title-ascending order. No stemming: include morphological variants explicitly (run, running, runs)."
    },
    "range_filters": {
      "additionalProperties": {
        "type": "string"
      },
      "description": "Range filters on schema-declared range-filterable fields, keyed by `min_<field>` / `max_<field>` (numeric) or `<field>_before` / `<field>_after` (date). Example: `{\"created_date_after\": \"2026-01-01\", \"max_score\": \"5\"}`. Every field with `filterable: range` in the type's schema is reachable here. Composable with `filters` (equality). One typed warning per outcome, branch on `code`: `RANGE_FILTER_KEY_MALFORMED` (key lacks a `min_`/`max_`/`*_before`/`*_after` shape), `RANGE_FILTER_TYPE_SCOPED` (a *range-filterable* field declared only on other types — applied with strict type-narrowing), `UNKNOWN_RANGE_FILTER_FIELD` (derived field name not declared on any reachable schema — ignored), `FIELD_NOT_RANGE_FILTERABLE` (field declared but not `filterable: range` on any reachable type — ignored in both the scoped and unscoped case, result unfiltered not emptied).",
      "type": [
        "object",
        "null"
      ]
    },
    "related_to": {
      "description": "Full entity ID — only return entities within depth hops (BFS, undirected). Results are ranked by proximity: nearer hops first, then a typed (dependency) link to the anchor before a co-mention at the same hop. A neighbourhood larger than the cap is bounded to its nearest members with a `NEIGHBOURHOOD_CAPPED` warning (`kept`/`total`).",
      "type": [
        "string",
        "null"
      ]
    },
    "stub": {
      "description": "Filter by stub status. Omit (default) = both stubs and real entities. `true` = stubs only. `false` = real entities only.",
      "type": [
        "boolean",
        "null"
      ]
    },
    "token_budget": {
      "description": "Token budget bounding the returned hit payload (default: 12000). A page whose hits exceed it is greedily trimmed to the highest-ranked hits that fit (at least one always returns) and a `SEARCH_RESULTS_TRUNCATED` warning carries `kept`/`budget`; `_total` still reflects the full match count, so page the remainder with `offset` or narrow the query. Raise it to pull more hits in one call when the agent can afford the tokens. Independent of `limit`, which caps the count before the budget trims by size.",
      "format": "uint",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    }
  },
  "title": "SearchParams",
  "type": "object"
}
```

## `memstead_update`

**Flavour:** lean + full

Modify an existing entity. Pre-fetch the target mem's schema via `memstead_schema(name=<mem.schema_ref>)` once per session — section names and write_rules live there. Read the entity first via memstead_entity and pass its hash as `expected_hash` — mismatch emits `HASH_MISMATCH` (`details.current` carries the live hash). `INLINE_WIKI_LINK_AUTO_STUBBED` warns when `[[…]]` parses to unresolved ids; `details.stubs` lists ghosts. `MISSING_REQUIRED_OUTGOING` warns when the type's `required_outgoing` blocks stay unsatisfied (payload mirrors memstead_create's; clear via memstead_relate). Three section modes: `sections` (replace), `append_sections` (append), `patch_sections` (find-and-replace, first or every via `all: true`). One mode per key. `patch_sections` errors on missing `old` or empty section. Schema-bound errors carry recovery payloads: `UNKNOWN_SECTION` / `UNKNOWN_METADATA_FIELD` ship `details.declared` + nearest-match `suggestion`; `INVALID_ENUM_VALUE` ships `details.allowed`, `details.field_description`, `suggestion`, `details.type_write_rules`; `REQUIRED_FIELD_UNSET` ships the same field+enum+rules payload. `metadata` sets frontmatter; `metadata_unset` removes it (silently no-ops on absent or section keys). Setting and unsetting the same key is a hard error. Read-only (set/unset → `READ_ONLY_FIELD`): `mem`, `id`, `type` (memstead_rename for title; delete+create for type/mem) plus engine-stamped `created_date` / `last_modified`. Stubs cannot be updated — memstead_create as real first. Optional `note` (≤280 chars) — see memstead_create; missing emits `NOTE_MISSING`. No-op short-circuit: post-state bytes-identical to disk (e.g. same-day auto-stamp, already-declared relation, absent-key unset, empty payload) returns `UPDATE_NOOP`, empty `commit_sha`, unchanged `_hash` — `expected_hash` stays stable. `dry_run: true` validates then previews OR recovers from a stale hash: it bypasses ONLY the `expected_hash` check (returns current `_hash` + `prospective_hash`), but section/field validation still refuses with the same typed envelope a real call returns — dry_run never reports an invalid update as clean. Reuse the current `_hash` as `expected_hash`, never `prospective_hash` — auto-stamped `last_modified` shifts the latter. A body-link removal orphaning its stub target GC's it into `orphan_stubs_removed`. Real-write responses carry `commit_sha` (per-mem git; gitdir via `memstead_health include_config=true`) for polling via memstead_changes_since.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$defs": {
    "PatchInput": {
      "additionalProperties": false,
      "description": "Find-and-replace input.",
      "properties": {
        "all": {
          "description": "Replace every occurrence of `old` when true; replace only the first when false or omitted. Literal match, case-sensitive.",
          "type": [
            "boolean",
            "null"
          ]
        },
        "new": {
          "description": "Replacement (empty string = delete)",
          "type": "string"
        },
        "old": {
          "description": "Exact substring to find in current content",
          "type": "string"
        }
      },
      "required": [
        "old",
        "new"
      ],
      "type": "object"
    },
    "RelationInput": {
      "additionalProperties": false,
      "description": "A relationship input for create/batch tools.",
      "properties": {
        "description": {
          "default": null,
          "description": "Optional per-edge description text. Validated against the rel-type's `per_edge_description` posture in the pinned schema: `forbidden` (default) rejects a non-empty description with `DESCRIPTION_NOT_PERMITTED`; `required` rejects its absence with `MISSING_REQUIRED_DESCRIPTION`; `optional` accepts both. Empty / whitespace-only strings normalise to absent before validation. Surfaces on `memstead_entity` and round-trips through the `## Relationships` markdown via the canonical em-dash delimiter (` — `).",
          "type": [
            "string",
            "null"
          ]
        },
        "to": {
          "description": "Full target entity ID",
          "type": "string"
        },
        "type": {
          "description": "Relationship type. Canonical form is UPPER_SNAKE_CASE (USES, PART_OF, DEPENDS_ON) and is what the engine stores; case-insensitive inputs (`uses`, `Part_Of`) are accepted and echoed back in the response as their canonical form. The JSON Schema `pattern` advertises `^[A-Za-z][A-Za-z_]*$` for client-side validators; the engine enforces the same character set independently — characters outside it return `INVALID_REL_TYPE` at the engine boundary regardless of whether the client pre-filters.",
          "pattern": "^[A-Za-z][A-Za-z_]*$",
          "type": "string"
        }
      },
      "required": [
        "to",
        "type"
      ],
      "type": "object"
    },
    "RelationUnsetInput": {
      "additionalProperties": false,
      "description": "One `relations_unset` entry — `{ rel_type, target }`.",
      "properties": {
        "rel_type": {
          "description": "Relationship type of the edge to remove (canonical UPPER_SNAKE_CASE; case-insensitive input accepted)",
          "type": "string"
        },
        "target": {
          "description": "Full target entity ID of the edge to remove",
          "type": "string"
        }
      },
      "required": [
        "rel_type",
        "target"
      ],
      "type": "object"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for memstead_update.",
  "properties": {
    "append_sections": {
      "additionalProperties": {
        "type": "string"
      },
      "description": "Section fields to append to: { \"specifies\": \"extra content\" }",
      "type": [
        "object",
        "null"
      ]
    },
    "declare_relations": {
      "description": "Atomic batched relation declarations applied before the section/metadata changes land. Each `{ to, type }` is validated like a `memstead_relate` call (schema-shape, cross-mem policy, target-id grammar) and appended to the entity's relations; absent Write-mem targets are auto-stubbed identically to the relate path. The strict wiki-link/relation validator then runs against the post-mutation state with the freshly-declared relations in place — so adding a `[[target]]` body wiki-link + declaring the backing `REFERENCES` relation can land in a single `memstead_update` call (without `declare_relations`, the post-migration strict validator would refuse the body link). Each successful entry is echoed in `relations_declared` on the response with `target_was_stubbed` flagging whether the target was absent at call time. Omit for mutations that don't introduce new relations.",
      "items": {
        "$ref": "#/$defs/RelationInput"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "dry_run": {
      "description": "Validate and preview what would change without executing. On a valid update the response carries the unchanged on-disk hash as `_hash` plus the post-write `prospective_hash` — pass `_hash` as `expected_hash` on the follow-up real call. `dry_run` deliberately bypasses ONLY the `expected_hash` check (the returned `_hash` is the current on-disk hash, safe to reuse on the real follow-up), making it the designated recovery path for stale hashes. It does NOT relax the rest of validation: an update that a real call would refuse on section/field grounds (`UNKNOWN_SECTION`, `UNKNOWN_METADATA_FIELD`, `INVALID_ENUM_VALUE`, `REQUIRED_FIELD_UNSET`, `PATCH_OLD_NOT_FOUND`, …) refuses under dry_run with the same typed envelope and the same recovery `details.*` — that refusal is the pre-flight signal, not a clean preview. So dry_run and a real write agree on validity (modulo the intentionally-skipped hash check).",
      "type": [
        "boolean",
        "null"
      ]
    },
    "expected_hash": {
      "description": "Hash from memstead_entity response (_hash field). Required — read the entity first. Mismatch returns code HASH_MISMATCH with details.current carrying the current on-disk hash; pass dry_run=true to bypass the check as a recovery path.",
      "type": "string"
    },
    "id": {
      "description": "Full entity ID to update",
      "type": "string"
    },
    "metadata": {
      "additionalProperties": {
        "type": "string"
      },
      "description": "Metadata fields to set: { \"level\": \"M1\" }",
      "type": [
        "object",
        "null"
      ]
    },
    "metadata_unset": {
      "description": "Metadata keys to remove. Silent no-op if absent. Errors on read-only fields (mem, id, type, plus the engine-stamped created_date / last_modified) and on schema-required fields. Cannot overlap with `metadata` keys — pass one or the other per key.",
      "items": {
        "type": "string"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "note": {
      "description": "Agent-authored provenance note (≤280 chars, one sentence describing why this mutation happened). Lands in the per-mem commit body between the mechanical subject line and the provenance trailers (`Tool:`, `Actor:`, `Client:`), and is surfaced by the outer-repo Stop hook when aggregating session activity. Omit for pure-housekeeping edits; when `[mutations].require_notes = true` in workspace config a missing note adds a `NOTE_MISSING` `WarningHint` to the response (the mutation still commits).",
      "type": [
        "string",
        "null"
      ]
    },
    "patch_sections": {
      "additionalProperties": {
        "$ref": "#/$defs/PatchInput"
      },
      "description": "Section fields to patch (find-and-replace): { \"specifies\": { \"old\": \"...\", \"new\": \"...\" } }",
      "type": [
        "object",
        "null"
      ]
    },
    "relations_unset": {
      "description": "Repair-shaped relation removals `[{ rel_type, target }]`, applied atomically within this update. Accepted only when the entity currently FAILS the conformance check (see memstead_health include=conformance) — on a conformant entity the call refuses with REPAIR_NOT_NEEDED and the entity is unmodified; use memstead_relate(remove=true) for everyday edge detachment. Absent pairs are silent no-ops (symmetric with metadata_unset). The strict-write post-condition is unchanged: the post-repair entity must validate or the whole update refuses with the relevant write-time code. During a schema migration every not-yet-repaired entity is non-conformant against the target, so this param works on exactly those entities with no mode flag.",
      "items": {
        "$ref": "#/$defs/RelationUnsetInput"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "sections": {
      "additionalProperties": {
        "type": "string"
      },
      "description": "Section fields to set (replaces content): { \"identity\": \"new content\" }",
      "type": [
        "object",
        "null"
      ]
    }
  },
  "required": [
    "id",
    "expected_hash"
  ],
  "title": "UpdateParams",
  "type": "object"
}
```

## `memstead_workspace_allow_create`

**Flavour:** full only

Append a `[[mem_management.create]]` rule admitting mem names matching `pattern` with the given schema pins. The allowlist gates `memstead_mem_create`; without a matching rule, mem creation refuses with `MEM_PATH_NOT_ALLOWED`. Pass `before` to lift the new rule above an existing pattern; without `before` the rule appends at the end (lowest priority). Pass `default_cross_links` to confer a cross-mem link grant on every mem matching `pattern` — saves a follow-up `memstead_workspace_grant_cross_link`. The grant is rule-derived and evaluated lazily at relate time (it is NOT written into the `[cross_mem_links]` table); `memstead_overview` surfaces it under the matching pattern in `## Lifecycle Namespaces` and as the `cross_mem_links_from_rules` workspace-policy posture. Idempotent: re-add with the same `pattern` AND the same `schemas` set returns success with `RULE_ALREADY_PRESENT` warning, file unchanged (schema-set comparison is order- and duplicate-insensitive). Re-adding an existing `pattern` with a *different* `schemas` set is refused with `RULE_EXISTS_SCHEMAS_DIFFER` (`details.stored_schemas`, `details.requested_schemas`, `details.recovery`) — this verb only adds rules, it does not modify a rule's schema pins; to change them, `memstead_workspace_revoke_create` the pattern then re-add with the new schemas. `before` resolution failure surfaces as `BEFORE_PATTERN_NOT_FOUND`. Refuses with `WORKSPACE_NOT_INITIALISED` when the workspace config is missing.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for `memstead_workspace_allow_create`.",
  "properties": {
    "before": {
      "description": "Existing pattern to insert before — lifts the new rule above the named pattern in the priority list. Omit to append at the end (lowest priority).",
      "type": [
        "string",
        "null"
      ]
    },
    "default_cross_links": {
      "description": "Default cross-mem link grants for mems matching this rule. Each entry is a target-mem name (`\"specs\"`) or `\"*\"` (any). Pre-populates `[cross_mem_links]` for matching new mems so agents don't have to grant a second time.",
      "items": {
        "type": "string"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "pattern": {
      "description": "Glob pattern matched against composed mem candidates (`<mem_path>/<name>` for hierarchical, bare `<name>` for flat). First-match-wins; lower index = higher priority.",
      "type": "string"
    },
    "schemas": {
      "description": "Schema pins admitted by this rule. `[\"*\"]` is the any-schema escape. Each entry is a canonical `name@version` pin (e.g. `\"default@1.0.0\"`).",
      "items": {
        "type": "string"
      },
      "type": "array"
    }
  },
  "required": [
    "pattern",
    "schemas"
  ],
  "title": "WorkspaceAllowCreateParams",
  "type": "object"
}
```

## `memstead_workspace_allow_delete`

**Flavour:** full only

Append a `[[mem_management.delete]]` rule admitting deletes of mem names matching `pattern`. Symmetric counterpart to `memstead_workspace_allow_create` — agent-creatable equals agent-deletable. Without a matching rule, `memstead_mem_delete` refuses with `MEM_PATH_NOT_ALLOWED`. Idempotent: re-add with the same `pattern` returns success with `RULE_ALREADY_PRESENT` warning, file unchanged. Refuses with `WORKSPACE_NOT_INITIALISED` when the workspace config is missing.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for `memstead_workspace_allow_delete`.",
  "properties": {
    "pattern": {
      "description": "Glob pattern matched against composed mem candidates. Appended to `[[mem_management.delete]]` — the symmetric allowlist for `memstead_mem_delete`. Agent-creatable equals agent-deletable in spirit; mirror the create-side `pattern` to keep parity.",
      "type": "string"
    }
  },
  "required": [
    "pattern"
  ],
  "title": "WorkspaceAllowDeleteParams",
  "type": "object"
}
```

## `memstead_workspace_grant_cross_link`

**Flavour:** full only

Grant mem `from` permission to author cross-mem links into mem `to`. Mutates the `[cross_mem_links]` workspace policy. Dynamic-mem-lifecycle workflow: `memstead_mem_create → memstead_workspace_grant_cross_link → memstead_relate cross-mem → memstead_relate remove → memstead_workspace_revoke_cross_link → memstead_mem_delete`. Idempotent: re-grant of an existing grant returns success with `GRANT_ALREADY_PRESENT` warning, file unchanged. Conflict mode (wildcard against an existing specific list, or a named target against an existing wildcard) returns `CROSS_LINK_CONFLICT` — operators pick a single shape per `from`-mem. Refuses with `WORKSPACE_NOT_INITIALISED` when the workspace config is missing, `INVALID_TOML` when the file fails to parse, `IO_ERROR` on write failure. Response carries `{from, to, warnings}`.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for `memstead_workspace_grant_cross_link`.",
  "properties": {
    "from": {
      "description": "Source mem. The grantee — the mem permitted to author cross-mem edges into `to`.",
      "type": "string"
    },
    "to": {
      "description": "Target mem. Pass a named mem (e.g. `\"specs\"`) to append to the named-allowlist shape, or the literal `\"*\"` to set the wildcard shape (any target). Wildcard vs. named is mutually exclusive per `from`-mem — switching between requires revoking the prior shape first; mixing surfaces `CROSS_LINK_CONFLICT`.",
      "type": "string"
    }
  },
  "required": [
    "from",
    "to"
  ],
  "title": "WorkspaceGrantCrossLinkParams",
  "type": "object"
}
```

## `memstead_workspace_revoke_create`

**Flavour:** full only

Remove a `[[mem_management.create]]` rule by `pattern`. Counterpart to `memstead_workspace_allow_create`. Idempotent: revoking a `pattern` with no matching rule returns success with `RULE_NOT_FOUND_NOOP` warning, file unchanged. Refuses with `WORKSPACE_NOT_INITIALISED` when the workspace config is missing, `INVALID_TOML` on parse failure, `IO_ERROR` on write failure. Response carries `{pattern, warnings}`.

**Hints:** `read_only` = false, `destructive` = true, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for `memstead_workspace_revoke_create`.",
  "properties": {
    "pattern": {
      "description": "Glob pattern of the `[[mem_management.create]]` rule to drop. Matched exactly against the rule's `pattern` field.",
      "type": "string"
    }
  },
  "required": [
    "pattern"
  ],
  "title": "WorkspaceRevokeCreateParams",
  "type": "object"
}
```

## `memstead_workspace_revoke_cross_link`

**Flavour:** full only

Revoke mem `from`'s permission to author cross-mem links into mem `to`. Mutates the `[cross_mem_links]` workspace policy; when the underlying list becomes empty, the `from` key is dropped entirely. Dynamic-mem-lifecycle workflow: revoke before `memstead_mem_delete` to clear the `MEM_REFERENCED_BY_POLICY` refusal. Idempotent: re-revoke of an absent grant returns success with `GRANT_NOT_FOUND` warning, file unchanged. Refuses with `WORKSPACE_NOT_INITIALISED` when the workspace config is missing, `INVALID_TOML` when the file fails to parse, `IO_ERROR` on write failure. Response carries `{from, to, warnings}`.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for `memstead_workspace_revoke_cross_link`.",
  "properties": {
    "from": {
      "description": "Source mem. The grantee whose existing grant is being revoked.",
      "type": "string"
    },
    "to": {
      "description": "Target mem, or `\"*\"` to revoke the wildcard shape. When the underlying list becomes empty, the `from` key is dropped entirely.",
      "type": "string"
    }
  },
  "required": [
    "from",
    "to"
  ],
  "title": "WorkspaceRevokeCrossLinkParams",
  "type": "object"
}
```

## `memstead_workspace_revoke_delete`

**Flavour:** full only

Remove a `[[mem_management.delete]]` rule by `pattern`. Counterpart to `memstead_workspace_allow_delete`. Idempotent: revoking a `pattern` with no matching rule returns success with `RULE_NOT_FOUND_NOOP` warning, file unchanged. Refuses with `WORKSPACE_NOT_INITIALISED` when the workspace config is missing, `INVALID_TOML` on parse failure, `IO_ERROR` on write failure. Response carries `{pattern, warnings}`.

**Hints:** `read_only` = false, `destructive` = true, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Parameters for `memstead_workspace_revoke_delete`.",
  "properties": {
    "pattern": {
      "description": "Glob pattern of the `[[mem_management.delete]]` rule to drop. Matched exactly against the rule's `pattern` field.",
      "type": "string"
    }
  },
  "required": [
    "pattern"
  ],
  "title": "WorkspaceRevokeDeleteParams",
  "type": "object"
}
```

