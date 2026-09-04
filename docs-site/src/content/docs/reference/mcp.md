---
title: "MCP tools"
---

# MCP tools

Generated from the live `tool_router().list_all()` catalogue on `McpServer`. Every tool the running server exposes appears below.

**Count:** the server exposes 20 tools.

## Index

- [`memstead_changes_since`](#memstead-changes-since)
- [`memstead_check`](#memstead-check)
- [`memstead_create`](#memstead-create)
- [`memstead_delete`](#memstead-delete)
- [`memstead_diff`](#memstead-diff)
- [`memstead_entity`](#memstead-entity)
- [`memstead_health`](#memstead-health)
- [`memstead_mem_configure`](#memstead-mem-configure)
- [`memstead_mem_create`](#memstead-mem-create)
- [`memstead_mem_delete`](#memstead-mem-delete)
- [`memstead_mem_set_schema`](#memstead-mem-set-schema)
- [`memstead_mem_set_version`](#memstead-mem-set-version)
- [`memstead_overview`](#memstead-overview)
- [`memstead_relate`](#memstead-relate)
- [`memstead_reload`](#memstead-reload)
- [`memstead_rename`](#memstead-rename)
- [`memstead_retype`](#memstead-retype)
- [`memstead_schema`](#memstead-schema)
- [`memstead_search`](#memstead-search)
- [`memstead_update`](#memstead-update)

## `memstead_changes_since`

Per-mem change feed. **`since` is backend-specific and NEVER a mutation's `write_id`** (on a folder mem it refuses `INVALID_CURSOR`: the token sorts below every ledger timestamp and would otherwise replay the whole history). Git-branch mem: `since` is a commit SHA — the `head` a prior call returned, or the empty-tree hash `4b825dc642cb6eb9a060e54bf8d69288fbee4904` for a first sync. Folder mem: `since` is an RFC3339 timestamp — the `ts` of the last entry you read, or empty for a first sync (no `head`). Returns a flat list of entity-level events — each event's `action` is one of `added`, `updated`, `removed`, `renamed`. Non-`removed` events carry `entity_type` (schema type name, e.g. spec, memo), looked up from the post-diff store; `removed` events carry `entity_type: null` alongside `title: null`. Engine-authored renames pair via commit-note provenance (`memstead: rename <old> → <new>`) — exact, similarity-independent, transitively composed across multi-step rename chains in the same window. Non-engine renames (`git mv`, pre-provenance migrations) fall back to a content-similarity scorer (default 0.6, tunable via `rename_similarity` in [0.1, 1.0]), capped at 1000 rewrite pairs per diff. Either path surfaces as a single `renamed` event with `from_id` and `to_id` rather than a removed+added pair. Out-of-range `rename_similarity` values refuse with `INVALID_INPUT` naming `details.allowed_range` and `details.requested`. On a git-branch mem `head` echoes the current HEAD SHA — save it as the next polling cursor (prefer full SHAs over refs). No pagination — every qualifying commit ships in one response. Pass `include_notes: true` to fold per-commit agent-notes (`notes[]`) and `memstead_ref` (SHA of the unified schema + per-mem-config registry) into the response — a commit-mirroring client gets deltas, notes, and the registry-ref sha in one round-trip. Unknown or malformed `since` returns `INVALID_CURSOR` with `details.mem` and `details.since`.

**Hints:** `read_only` = true, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "include_notes": {
      "default": false,
      "description": "Fold per-commit agent-notes into the response. When true, the report carries a `notes[]` array (one entry per commit between `since` and `head`, with `sha`, `subject`, `tool_verb`, `entity_id`, `note`, `actor`, `tool`, `client`, `timestamp`) plus `memstead_ref` — the SHA of the unified schema + per-mem-config registry, absent when the workspace has not been migrated yet. Default false (entity-delta only). Commit-mirroring clients turn this on to receive notes and the registry-ref sha in one round-trip; agents that just need entity events leave it off.",
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
      "description": "Change cursor to diff against. Backend-specific, and never a mutation's `write_id`: on a git-branch mem a commit SHA (pass the `head` a prior call returned, or the canonical git empty-tree hash `4b825dc642cb6eb9a060e54bf8d69288fbee4904` to get every entity as `added` on a fresh-client first sync); on a folder mem an RFC3339 timestamp (the `ts` of the last entry you received, or empty for a first sync).",
      "type": "string"
    }
  },
  "required": [
    "mem",
    "since"
  ],
  "type": "object"
}
```

## `memstead_check`

Record a check: "entity E checked, verdict ok | failed, via method M" — the engine-recorded act of verification (never a mutation: entity markdown, `_hash` and mem commits are untouched, which is what makes check-staleness computable). The record carries the caller-declared `role` plus actor/client identity and the entity's `_hash` at check time, appended to the workspace's append-only check ledger — a newer check of the same kind supersedes older ones for state, never erases them. Derived check state (`never_checked` | `checked_ok` | `check_failed` | `check_stale` — stale means the entity changed after its last check, computed by hash comparison, never stamped) is served in `memstead_entity`'s opt-in `mutation_provenance` block and echoed in this response as `check_state`. Verdict vocabulary is closed (`ok` | `failed`) — nuance goes in `method` or in process-mem entities; an unknown verdict refuses `INVALID_VERDICT`. The check `kind` vocabulary is closed too (`verification` | `conformance`; omitted = `verification`, exactly the prior behaviour): a `conformance` record is the semantic judgment "this entity satisfies its type's schema prose", carries the mem's schema pin as `schema_ref` stamped by the engine (never caller-supplied), and derives stale when the content hash OR the pin moves; state derives per (entity, kind), the kinds never supersede each other, a caller-declared `x-<name>` kind is recorded verbatim and never interpreted (no pin, no state, listed by count in health), and any other kind refuses `INVALID_CHECK_KIND` naming the vocabulary. An optional structured `finding` (`code`, `message`, `section`?, `evidence`?) rides the record and is served under the entity's latest verdict in health; a malformed one refuses `INVALID_CHECK_FINDING` naming the shape. Refuses typed on unknown entity (`ENTITY_NOT_FOUND`), unknown/quarantined mems, read-only mems (`READ_ONLY_MOUNT`), and persistence failure (`CHECK_NOT_RECORDED` — recording is never best-effort).

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$defs": {
    "CheckFindingParam": {
      "description": "One `finding` on `memstead_check` — the wire shape of\n`memstead_base::check::CheckFinding`, validated as a whole by the\nengine type (unknown keys refuse there).",
      "properties": {
        "code": {
          "description": "The checker's finding code, its own vocabulary (`hidden-premise`, `stale-source`). Required, non-empty.",
          "type": "string"
        },
        "evidence": {
          "description": "What the finding rests on: a quote, a coordinate, a reference.",
          "type": [
            "string",
            "null"
          ]
        },
        "message": {
          "description": "One or two sentences a reader can act on. Required, non-empty.",
          "type": "string"
        },
        "section": {
          "description": "The section key the finding concerns, when it concerns one.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "code",
        "message"
      ],
      "type": "object"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "entity": {
      "description": "Full entity id (`mem--slug`) of the entity that was checked",
      "type": "string"
    },
    "finding": {
      "anyOf": [
        {
          "$ref": "#/$defs/CheckFindingParam"
        },
        {
          "type": "null"
        }
      ],
      "description": "Optional structured finding: `{code, message, section?, evidence?}` — `code` is your own vocabulary (`hidden-premise`, `stale-source`), `message` one or two sentences a reader can act on, `section` the section key it concerns, `evidence` what it rests on. Persisted on the ledger line, echoed in this response, rendered by `memstead_health` `include: [\"checks\"]` under the entity's latest verdict. The wrapper shape is fixed: a missing `code` or `message`, an empty value, or an unknown key refuses `INVALID_CHECK_FINDING` naming the shape, and nothing is recorded."
    },
    "identity": {
      "description": "WHO is checking: an opaque identity string (agent-trust plan 15), recorded immutably on the check record. The independence gate compares the author's recorded identity against this one and nothing else — declare a stable identity per agent/session and author≠checker becomes machine-checkable. Omit to record the session default, or nothing (the check then reads unconfirmable). Over-length refuses INVALID_IDENTITY (cap 128 chars).",
      "type": [
        "string",
        "null"
      ]
    },
    "kind": {
      "description": "The check kind: `verification` | `conformance` (the engine's two kinds), or a caller-declared `x-<name>` kind (lowercase letters, digits, hyphens) the engine records verbatim and never interprets — it stamps no pin, moves no `check_state`, and health lists it by count. Omit for `verification` — exactly today's behaviour. `conformance` records a semantic judgment (\"does this entity satisfy its type's schema prose\"): the engine stamps the mem's schema pin into the record (never caller-supplied), and the verdict derives stale when the content hash moves OR the pin changes; a mem with no pin refuses `INVALID_INPUT`. State derives per (entity, engine kind) — the kinds never supersede each other. Any other value refuses `INVALID_CHECK_KIND` naming the vocabulary.",
      "type": [
        "string",
        "null"
      ]
    },
    "method": {
      "description": "Optional free-text method note — how the check was performed (e.g. \"diffed against source spec\").",
      "type": [
        "string",
        "null"
      ]
    },
    "role": {
      "description": "The role this check is performed in, from the closed vocabulary `author` | `checker` | `verifier`. Recorded immutably on the check record — same trust model as mutation roles: caller-declared but tamper-evident. Omit to record the session default (or unspecified — legal, but an unspecified-role check cannot confirm independence downstream).",
      "type": [
        "string",
        "null"
      ]
    },
    "verdict": {
      "description": "The verdict, from the closed vocabulary `ok` | `failed`. Nuance goes in `method` or in process-mem entities — an unknown value refuses `INVALID_VERDICT` naming the vocabulary.",
      "type": "string"
    }
  },
  "required": [
    "entity",
    "verdict"
  ],
  "type": "object"
}
```

## `memstead_create`

Create a new entity. Read the target mem's schema first (`memstead_schema`). Required: `title`, `entity_type`, plus the type's required sections. Id: mem--slug(title). Titles accept any single-line text (control characters such as tab/newline are rejected); the title is stored verbatim as display text, while characters outside Unicode alphanumerics, whitespace, and hyphen are dropped from the derived slug — warning TITLE_CHARS_DROPPED_FROM_SLUG names them (`INVALID_TITLE` refuses control chars, empty-deriving or over-long titles). `mem` defaults to the primary writable mem. Pass `relations` to wire edges inline — `{target, rel_type, description?}`, no source field; unresolved targets auto-stub. Optional `note` (see instructions). Schema-bound failures (`UNKNOWN_SECTION`, `UNKNOWN_METADATA_FIELD`, `INVALID_ENUM_VALUE`, `INVALID_FIELD_VALUE`, `SECTION_CONTENT_INVALID`, `REQUIRED_FIELD_UNSET`, `INVALID_REL_TYPE`, …) carry recovery payloads (see instructions). Create-specific: `REQUIRED_FIELD_UNSET` also fires on an omitted required-no-default metadata field (retires warning `MISSING_REQUIRED_FIELD`); `MISSING_REQUIRED_SECTION` refuses on create — an entity never lands with a placeholder body — shipping per-section `write_rules`, `type_guidance`, and `details.pre_announced` naming still-unset required metadata so one retry clears both gates. Warnings (entity still lands): `UNDECLARED_RELATIONSHIP_OPEN`; `INLINE_WIKI_LINK_AUTO_STUBBED` (body `[[wiki-links]]` auto-stub unresolved targets; `details.stubs`); `CROSS_SCHEMA_LINK_UNDECLARED` (no cross_mem_relationships entry for the target schema: NO edge, prose only); `MISSING_REQUIRED_OUTGOING` (`details.missing[]={relationships, cardinality}`; then memstead_relate). Real writes return `write_id` (identity, not cursor). `dry_run: true` previews a VALID entity (prospective `id`, `file_path`, `_hash`, warnings, `type_guidance`, `incoming` edges adopted from a pre-existing stub; empty `write_id`) — an INVALID one refuses with the real call's typed envelope.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$defs": {
    "AnchorInputParam": {
      "additionalProperties": false,
      "description": "One `anchors[]` element on `memstead_create` / `memstead_update` — a\nprovenance record tying the entity to a source artifact. Permissive by\ndesign: every field is optional / string-typed so a malformed element\n(unknown class or grain, missing artifact, hash on a non-hash class,\ngrain the medium's namespace cannot express) refuses the whole mutation\nwith a typed `INVALID_ANCHOR` envelope carrying recovery `details` —\nrather than an opaque schema-deserialisation error. Converts to the\nengine's `AnchorInput` which validates it. Not folded into `_hash`.",
      "properties": {
        "artifact": {
          "default": null,
          "description": "Artifact reference in the medium's own namespace — a repo-relative path, `path@commit`, URL, or entity id, interpreted per `grain`. Required; a missing/empty value refuses INVALID_ANCHOR.",
          "type": [
            "string",
            "null"
          ]
        },
        "at_version": {
          "anyOf": [
            {
              "$ref": "#/$defs/AnchorVersionParam"
            },
            {
              "type": "null"
            }
          ],
          "description": "Medium-typed pinned version this anchor was recorded against: `{ kind: \"commit\"|\"snapshot\"|\"etag\", value: \"<token>\" }`. Omit for a plain-path medium with no retrievable version."
        },
        "binding": {
          "default": null,
          "description": "`hash(D)` of the binding that produced this anchor, when a binding produced it. Omit for a manually-authored anchor.",
          "type": [
            "string",
            "null"
          ]
        },
        "class": {
          "default": null,
          "description": "Provenance class — the entity's epistemic standing toward the artifact: `anchored` | `derived` | `authored` | `informed-by`. `anchored`/`derived` carry hash semantics (a `hash` is permitted and participates in drift adjudication); `authored`/`informed-by` do not (supplying `hash` refuses INVALID_ANCHOR). An unknown value refuses INVALID_ANCHOR.",
          "type": [
            "string",
            "null"
          ]
        },
        "content": {
          "default": null,
          "description": "The observed artifact CONTENT (UTF-8 text), for the engine to compute `hash` from through its preparation registry — the write-time observation for a grain the engine cannot observe itself: a `url` anchor (the engine never fetches; what you read is canonicalized exactly as a path grain's bytes are). Also accepted for `span`/`file`. Mutually exclusive with `hash` (both refuses INVALID_ANCHOR); refused on `authored`/`informed-by`, and on the `entity`/`tree` grains, whose prepared form is never computed from supplied bytes.",
          "type": [
            "string",
            "null"
          ]
        },
        "derived_from": {
          "default": null,
          "description": "For a `derived` class: the input artifact refs the entity was derived from. Empty/omitted for every other class.",
          "items": {
            "type": "string"
          },
          "type": [
            "array",
            "null"
          ]
        },
        "grain": {
          "default": null,
          "description": "Granularity of the artifact reference: `span` | `file` | `tree` | `url` | `entity`. `span`/`file`/`tree` need a path-shaped medium namespace and `entity` the entity namespace, or the mutation refuses INVALID_ANCHOR; `url` is admitted beside every medium (a URL never enters a path namespace). A path-shaped grain whose artifact is a URL refuses INVALID_ANCHOR naming that rule: use `grain: url`. An unknown value refuses INVALID_ANCHOR.",
          "type": [
            "string",
            "null"
          ]
        },
        "hash": {
          "default": null,
          "description": "Content hash over the PREPARED artifact form (never raw bytes). Permitted only on hash-bearing classes (`anchored`/`derived`); supplying it on `authored`/`informed-by` refuses INVALID_ANCHOR.",
          "type": [
            "string",
            "null"
          ]
        },
        "hash_stability": {
          "default": null,
          "description": "Medium's declared hash stability: `stable` | `unstable` (defaults per grain: `url` to `unstable`, every other grain to `stable`). An unstable-source hash break resolves `recheck`, not `drifted`.",
          "type": [
            "string",
            "null"
          ]
        },
        "source": {
          "default": null,
          "description": "NAME of the source (as declared in the producing binding's `sources[]`) that produced this anchor — lets a discovery run be measured per entry point. Name the source you are working from whenever the binding declares more than one. Present-but-empty refuses INVALID_ANCHOR; a name the (resolvable) producing binding does not declare refuses with the declared names in `details.declared`. Omit for a manually-authored anchor.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "type": "object"
    },
    "AnchorVersionParam": {
      "additionalProperties": false,
      "description": "The medium-typed pinned version sub-object of an [`AnchorInputParam`].",
      "properties": {
        "kind": {
          "description": "Version kind: `commit` (git / path+commit) | `snapshot` (graph) | `etag` (web).",
          "type": "string"
        },
        "value": {
          "description": "The version token — commit id, graph snapshot token, or web ETag.",
          "type": "string"
        }
      },
      "required": [
        "kind",
        "value"
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
        "rel_type": {
          "description": "Relationship type. Canonical form is UPPER_SNAKE_CASE (USES, PART_OF, DEPENDS_ON) and is what the engine stores; case-insensitive inputs (`uses`, `Part_Of`) are accepted and echoed back in the response as their canonical form. The JSON Schema `pattern` advertises `^[A-Za-z][A-Za-z_]*$` for client-side validators; the engine enforces the same character set independently — characters outside it return `INVALID_REL_TYPE` at the engine boundary regardless of whether the client pre-filters.",
          "pattern": "^[A-Za-z][A-Za-z_]*$",
          "type": "string"
        },
        "target": {
          "description": "Full target entity ID",
          "type": "string"
        }
      },
      "required": [
        "target",
        "rel_type"
      ],
      "type": "object"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "anchors": {
      "description": "Optional provenance anchors to attach to the new entity — durable records tying it to the source artifacts it describes (which artifact, at which grain, under which provenance class). Written into the mem-branch anchors sidecar in the SAME commit as the entity (atomic); omitting it is byte-identical to a create without anchors. Anchor writes MERGE: later `memstead_update` calls carrying `anchors` add to this set (same `(artifact, grain, class)` triple replaces, otherwise appends) and never silently discard it — removal is explicit via `memstead_update`'s `anchors_unset`. A malformed element refuses the whole create with `INVALID_ANCHOR` (`details` carries the offending field + allowed set) and the entity is not written. A payload naming one `(artifact, grain, class)` triple TWICE refuses: that triple is one row, so the repeats would collapse to the last one unannounced. A `span` anchor's locator must be usable (`#L<start>-L<end>`, `#<unit-key>`, or no locator for the whole file); one that addresses nothing refuses, and a span the engine could not check (no `content` supplied) is accepted and recorded as unverified. Anchors do NOT participate in `_hash`.",
      "items": {
        "$ref": "#/$defs/AnchorInputParam"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "dry_run": {
      "description": "Validate and preview the create without executing — no disk write, no store mutation, no VCS commit, no edges added. dry_run runs the SAME validation a real call runs; it is not a softer check. On a VALID entity the response carries the prospective `id`, `file_path`, and `_hash` (bit-identical to what a real call with the same arguments would produce, EXCEPT for engine-auto-stamped timestamps: the hash covers `created_date`, which is stamped from wall-clock `now()` independently in the dry-run and the real call, so the two `_hash` values diverge whenever a second ticks between them; the hash also covers `sections`, `metadata`, and `relations`, so a dry_run that omits `relations` will not match a real call that supplies them; `_hash` does NOT cover `anchors` — the anchors sidecar persists on the mem branch under `.memstead/` and is never folded into content hashing, so attaching or refreshing anchors never changes `_hash` or invalidates a cached `expected_hash`), plus any `warnings` and any `incoming` edges that would be adopted from a pre-existing stub at this id, with `write_id` empty. On an INVALID entity dry_run does NOT return a warnings-list preview: it refuses with the IDENTICAL typed envelope a real call would return (`MISSING_REQUIRED_SECTION`, `UNKNOWN_SECTION`, `UNKNOWN_METADATA_FIELD`, `INVALID_ENUM_VALUE`, `INVALID_FIELD_VALUE`, `REQUIRED_FIELD_UNSET`, …), carrying the same recovery `details.*` (e.g. `details.sections[]`). That typed refusal IS the pre-flight signal — read its `details` to fix coverage, then retry. So dry_run never reports a problem entity as clean: it and a real write agree on validity. Use to verify the id slug, or to pre-flight required-section / field coverage and pre-existing references before committing.",
      "type": [
        "boolean",
        "null"
      ]
    },
    "entity_type": {
      "description": "Entity type. Required. Allowed values are pinned by the target mem's schema — fetch them via `memstead_schema(name=<mem.schema_ref>)` (cached per session). Unknown types refuse with `UNKNOWN_ENTITY_TYPE`.",
      "type": "string"
    },
    "identity": {
      "description": "WHO is acting: an opaque identity string of your choosing — an agent name, a session handle (agent-trust plan 15). Recorded immutably alongside the mutation (commit trailer / ledger); the author≠checker independence gate compares identities and nothing else. Caller-declared and unverified, but tamper-evident in append-only history. Omit to record the session default, or nothing — legal forever, never refused; identity-less records read unconfirmable at the gate. Over-length values refuse INVALID_IDENTITY (cap 128 chars).",
      "type": [
        "string",
        "null"
      ]
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
      "description": "Initial relationships, wired in the same call that creates the entity. An entry is literally `{ \"target\": \"<mem>--<slug>\", \"rel_type\": \"REL_TYPE\", \"description\": \"…\" }` — `description` optional, and there is no `from`: the entity being created is the source, so the far end is `target`. (`memstead_relate` names both ends: `{from, to, rel_type, …}`.) An unresolved `target` auto-creates a stub.",
      "items": {
        "$ref": "#/$defs/RelationInput"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "role": {
      "description": "The role this mutation is performed in, from the closed vocabulary `author` | `checker` | `verifier` (agent-trust plan 13). Recorded immutably alongside the mutation (commit trailer / ledger) — caller-declared but tamper-evident: bound to this operation in append-only history, it cannot be edited afterwards and identities can be cross-checked across operations. Omit to record the session default (or unspecified — legal forever, never refused, treated downstream as cannot-confirm). An unknown value refuses INVALID_ROLE naming the vocabulary.",
      "type": [
        "string",
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
  "type": "object"
}
```

## `memstead_delete`

Remove an entity permanently. Deletes the entity's store record, every edge touching it (both directions), and its markdown file on disk. Requires `expected_hash` (read the entity via memstead_entity first — mirrors memstead_update / memstead_rename for optimistic locking); mismatch emits `HASH_MISMATCH` with `details.current` carrying the current on-disk hash. Binary semantics: any incoming reference from another entity in a Write-Mem refuses the delete with `HAS_INCOMING_REFS` and `details.referrers` listing each `{from_id, rel_types, mem}` (one entry per unique source, rel_types collapses multi-edge cases) — the agent removes the offending references via `memstead_relate --remove` (or `memstead_update` for body wiki-links) before retrying. There is no force flag. When the only incoming references come from ReadOnly mounts (archives), the delete proceeds: the on-disk file is removed and the in-memory entity is demoted to a stub at the same id so the surviving edges keep a valid target — the response carries a `RESIDUAL_STUB_FOR_READONLY_REFERRERS` warning naming the surviving referrers. PART_OF children survive the delete: their parent edge is removed; file paths are unaffected (every entity already lives at `{mem}/{slug}.md`). Stubs (`_hash` empty) are deleted with `expected_hash: ""` — the hash check is skipped because there is nothing to compare. Optional `note` (≤280 chars) — shared provenance contract, see memstead_create. Response carries `relations_removed` (edges removed by this delete), `orphan_stubs_removed` (ids of stub entities whose last incoming edge was this entity — they are GC'd in the same op so the graph stays tidy; field is serde-omitted when empty), `warnings` (residual-stub warning when the demote path applied), and `write_id` (an identity, never a change cursor). Provenance anchors, if any, are removed in the same commit (no orphaned anchor survives).

**Hints:** `read_only` = false, `destructive` = true, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "expected_hash": {
      "description": "Hash from memstead_entity response (_hash field). Required for real entities — read first. Mirrors memstead_update / memstead_rename. Mismatch returns code HASH_MISMATCH with details.current carrying the current on-disk hash. Stubs carry an empty `_hash` (they have no on-disk file); pass the empty string to delete a stub — the hash check is skipped because there is nothing to compare.",
      "type": "string"
    },
    "id": {
      "description": "Full entity ID to delete",
      "type": "string"
    },
    "identity": {
      "description": "WHO is acting: an opaque identity string of your choosing — an agent name, a session handle (agent-trust plan 15). Recorded immutably alongside the mutation (commit trailer / ledger); the author≠checker independence gate compares identities and nothing else. Caller-declared and unverified, but tamper-evident in append-only history. Omit to record the session default, or nothing — legal forever, never refused; identity-less records read unconfirmable at the gate. Over-length values refuse INVALID_IDENTITY (cap 128 chars).",
      "type": [
        "string",
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
    "role": {
      "description": "The role this mutation is performed in, from the closed vocabulary `author` | `checker` | `verifier` (agent-trust plan 13). Recorded immutably alongside the mutation (commit trailer / ledger) — caller-declared but tamper-evident: bound to this operation in append-only history, it cannot be edited afterwards and identities can be cross-checked across operations. Omit to record the session default (or unspecified — legal forever, never refused, treated downstream as cannot-confirm). An unknown value refuses INVALID_ROLE naming the vocabulary.",
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
  "type": "object"
}
```

## `memstead_diff`

Return a two-ref structural diff at entity granularity. Walks the tree at `ref_a` and the tree at `ref_b` in the mem's gitdir, surfacing per-entity changes as `entries[]` whose `status` is one of `added`, `modified`, `deleted`, `renamed`, `invalid_entity`. Each entry carries the full markdown body on both sides by default in `content_before` / `content_after`; pass `include_content: false` for the metadata-only shape (`id`, `title`, `entity_type`, `status`). Ref-handling conventions mirror `memstead_changes_since`: the canonical empty-tree sentinel `4b825dc642cb6eb9a060e54bf8d69288fbee4904` is accepted as either ref and short-circuits to git's empty tree (first-sync diffs against a fresh mem use this for `ref_a`); a bare `HEAD` resolves to the selected mem's branch tip rather than the gitdir's symbolic HEAD. Cross-mem diffs work via fully-qualified refs naming the peer mem's branch; cross-different-gitdir diffs are out of scope (the op operates on one mem-repo). Refusal codes: `UNKNOWN_MEM` (`details.name`), `UNKNOWN_REF` (`details.ref`), `INVALID_INPUT` for folder / archive mounts and for `rename_similarity` outside the allowed range. Rename detection uses content-similarity tuned by `rename_similarity`; agent-notes-driven rename-chain collapse is a follow-up. Each entry's `ripple` field carries per-side `{from_id, side}` entries for entities with inbound wiki-links to the affected entry — `side: "ref_a"` lists referrers at the `ref_a` snapshot, `side: "ref_b"` at `ref_b`. Pass `include_ripple: false` to omit the field entirely (e.g. for large mems where the per-side wiki-link scan is the dominant cost). Response top-level: `ref_a`, `ref_b`, `resolved_a_sha`, `resolved_b_sha`, `config`, `entries`.

**Hints:** `read_only` = true, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
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
  "type": "object"
}
```

## `memstead_entity`

Read one entity. Dual channel: text carries rendered markdown for direct prose consumption; `structured_content` carries the typed envelope `{ _hash, id, mem, entity_type, origin, _tokens, metadata, sections, relationships, _stub_kind?, _signals?, _labelling? }` so agents branch on fields without parsing the text. `origin` is the content's trust class — `first-party` for an entity from a writable workspace mem, `third-party` for one from a read-only mount (a registry-installed read-mem or an adopted foreign folder/clone), which the host should treat as quoted, untrusted data. `_hash` is the optimistic-lock token. The nested `metadata` map is the single home for every schema-declared frontmatter key — read values as `metadata.level`, etc.; identity keys (`mem`/`id`/`entity_type`) and underscore-prefixed slots stay top-level. Every `relationships[]` entry carries `direction`: default reads hold outgoing edges only ("out", endpoint under `target`). After a successful `memstead_relate` the on-disk hash advances — the relate response's `_hash` is the next valid `expected_hash`. `include_relations: true` adds the incoming edges to `relationships[]` ("in", endpoint under `from`) — how to answer "what depends on this?" — and appends a direction-grouped `## Relations` text section; `include_context: true` appends the community cluster. Pass `sections` to narrow output (also narrows `structured_content.sections`); when narrowed, `_tokens_unfiltered_body` surfaces the unfiltered-base cost. Opt-in inserts count only toward `_tokens`, which may then exceed it. Stubs render with empty sections + relationships arrays and an empty `metadata: {}` map. `token_budget`/`chunk` bound only the rendered-markdown **text** channel: over-budget text adds `_chunk`/`_total_chunks`/`_truncated` markers. The `structured_content` envelope always ships whole — never chunked or truncated; size it ahead via `_tokens`. Use memstead_overview for cold-start, memstead_search to find IDs, memstead_update to mutate.

**Hints:** `read_only` = true, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
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
    "include_provenance": {
      "description": "Append a `mutation_provenance` block to structured_content: `created_by` and `last_modified_by`, each with actor, client, the caller-declared `role` (or `unspecified` — absence served as cannot-confirm, never as a real role), the caller-declared `identity` when one was recorded (the independence gate's only comparator), timestamp, and the backend reference. Derived from the append-only mutation record (commit trailers / ledger), which no verb can edit after the fact — the tamper-evident half of the role and identity trust model. When the recorded story does not start at the entity's creation, `created_by` is absent and `story_truncated` is true (stated, never fabricated). Default false: responses are byte-unchanged without the flag.",
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
  "type": "object"
}
```

## `memstead_health`

Return graph health. Typed payload on `structured_content`; text chunks past `token_budget` via `chunk`. Default: counts (`orphans_by_schema`/`communities_by_schema`), node/edge totals, distributions, `writable_mems`/`read_mems`, `default_writable_mem` (omitted-`mem` target), `mem_schemas`, `verdict_coverage` (examined/advisory/not_examined). `include` keys: `orphans`, `stubs`, `most_connected`, `missing_fields`, `stale` (anchor clock first; `anchor_fresh`), `dangling_links`, `tags`, `missing_required_outgoing`, `constraints`, `conformance`, `integrity`, `config`, `anchors`, `friction`, `open_questions` (unknowns; resolution missing/unchecked), `stale_derivations` (moved targets), `checks` (state counts, independence), `signals`, `labelling`, `ledger` (FOLDER ledger vs files; git-branch absent), `vital_signs`. `missing_fields` adds `issues[]` (`code` beside `missing`). `config` = the `include_config` projection. `conformance` lints entities against `target_schema` or each pin into `findings` `{id, axis, code, detail}` + `body_observations` (undeclared body; kept by a write?); `integrity` adds `UNRESOLVED_STUB`, `CROSS_MEM_EDGE_UNGRANTED` (no grant, target mounted; unmounted target dangles instead) and `DANGLING_LINK_TARGET_MISSING`/`DANGLING_LINK_NOT_RELATED`/`DANGLING_RELATION_TARGET_MISSING`; `dangling_links` carry theirs in `kind`. `tags`: `tag_distribution` (`limit`-capped), `tag_distribution_folded`, `untagged_entities`. `missing_required_outgoing`/`constraints`: unsatisfied blocks/violations. `anchors`: per-mem `resolves`/`drifted`/`recheck`/`unresolvable` (artifact gone)/`unobserved`/`dangling` (entity gone), `entity_end_unreconciled`, `population`. Unknown keys: `UNKNOWN_INCLUDE_KEY`; `limit` caps at 10 (>100: `LIMIT_CLAMPED`). Warnings: see instructions. `mem` scopes every section and every warning to one mem (only the mem rosters stay global). `include_config: true` adds `mutations` (`require_notes`), the opaque `plugin` map, per-mem `mems` (`origin`, `vcs`, `write_guidance`, `extra`).

**Hints:** `read_only` = true, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
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
      "description": "Detail sections to include (default: none — summary counts only). Allowed keys: orphans, stubs, most_connected, missing_fields, stale, dangling_links, tags, missing_required_outgoing, constraints, conformance, integrity, config, anchors, friction, open_questions, stale_derivations, checks, signals, labelling, ledger, vital_signs. `conformance` lints every entity against the effective schema and returns per-entity `findings` (`{id, axis, code, detail}` with write-time typed codes); `integrity` additionally projects the consistency axis into the same findings list: UNRESOLVED_STUB; CROSS_MEM_EDGE_UNGRANTED (an existing cross-mem edge the workspace grant table no longer permits — a state the default-deny write gate would refuse to create today; reported and strict, never a load refusal, and the detail names the referrer, the target and that the cause is the absent grant rather than a missing target); plus three dangling conditions, each with its own repair and each finding carrying it as a `repair` detail — DANGLING_LINK_TARGET_MISSING (a body wiki-link with no markdown file behind it: create the target entity, or remove the wiki-link), DANGLING_LINK_NOT_RELATED (a body wiki-link to a written entity the referrer does not relate to: relate the two, so the link is backed by a relationship row), and DANGLING_RELATION_TARGET_MISSING (a relationships row naming an entity absent from the store: remove the row, or create the target). A stub target on a relationships row is a legitimate forward reference and is not flagged. `friction` summarizes the workspace-local refusal ledger — counts per typed refusal code and per verb, with a per-code `by_reason` breakdown where the code carries a closed engine-owned discriminator, whole-ledger plus a recent 24h window; the ledger is local-only, refusals-only, and every recorded value is drawn from a closed engine-defined vocabulary (never parameters, free-form strings, or payload text). `open_questions` composes a per-mem worklist of what the holding does not know — stubs, anchors that are recheck / unresolvable (artifact gone) / unobserved (not measured) / dangling (entity gone), unsatisfied constraints, dangling links, and a paired process mem's open entries with negative findings under a distinct already-searched heading (done, keep off); every list is capped with an explicit `more` remainder. `stale_derivations` lists per mem every explicit edge on a derivation-declared rel-type whose target's current hash differs from the recorded baseline (`stale`) or that has no baseline (`unbaselined` — never fabricated as fresh or stale); re-assert the edge via memstead_relate to refresh the baseline. `ledger` reconciles a FOLDER mem's change ledger against the markdown files beside it (`{ledger_without_file, file_without_ledger}` per mem) — read-only, it never writes or tidies a ledger line; git-branch mems are ABSENT from the map rather than clean in it, because their change set is a real two-tree diff and the divergence cannot arise. Unknown keys surface as UNKNOWN_INCLUDE_KEY on warnings.",
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
      "description": "When true, the response carries the `[mutations]` posture (`mutations.require_notes`), the opaque `[plugin.*]` pass-through map, and a per-writable-mem `mems` detail array with `{ name, origin, vcs: { gitdir, worktree, head } }` — absolute canonical paths plus the cached branch-tip SHA (omitted on fresh mems with no commits yet) for the Stop-hook / sync flows so they never hardcode a layout or peel refs themselves. Documented alias: `include: [\"config\"]` renders the identical projection (the catalogue form every surface shares, including the CLI's `--include config`); passing both renders it once. Defaults to false — the absence of these fields is the default-posture signal. **Lifecycle policy** (`[[mem_management.create]]` / `[[mem_management.delete]]`) is surfaced via `memstead_overview`, not here — `memstead_health` is drift/diagnostics.",
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
  "type": "object"
}
```

## `memstead_mem_configure`

Update a mem's curation fields — display title, one-line description, and subject block — in one call: set what is present. Absent field = untouched; empty string (`title` / `description`) = clear; `clear_subject: true` clears the subject block as a unit (mutually exclusive with `subject`, both set refuses `INVALID_INPUT`). `subject` is `{scope, method?, exclusions?}` — what the mem covers, how its content was arrived at, what was deliberately left out. Display text, never identity: the mem stays addressed by `name` everywhere; a title is roster/UI text only. Same validation and storage as the CLI's `mem set-title` / `set-description` / `set-subject` — one config commit per touched field. Gate-free like the sibling setters (no `[[mem_management.*]]` allowlist applies), but every structural gate holds: unknown mem refuses `UNKNOWN_MEM`; read-only mounts refuse `READ_ONLY_MOUNT`. A call with no field present is a no-op returning the unchanged state. Response `{mem, title, description, subject, warnings}` carries the post-call values (null = unset); `MEM_RELOADED` rides on `warnings` when a sibling engine commit landed since the prior snapshot. `CONFIG_WRITE_INTERVENED` rides there when the stored config had moved since this engine read it: the write lands ON TOP of theirs and `details.fields` names what they had changed. Optional `note` (≤280 chars) rides each field's config commit.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$defs": {
    "MemSubjectInput": {
      "additionalProperties": false,
      "description": "Subject block for mem curation — mirrors\n`memstead_schema::MemSubject`.",
      "properties": {
        "exclusions": {
          "default": null,
          "description": "What was considered and deliberately left out — prose statements, order preserved.",
          "items": {
            "type": "string"
          },
          "type": [
            "array",
            "null"
          ]
        },
        "method": {
          "default": null,
          "description": "How the mem's content was arrived at.",
          "type": [
            "string",
            "null"
          ]
        },
        "scope": {
          "description": "What this mem covers. Required to set the block.",
          "type": "string"
        }
      },
      "required": [
        "scope"
      ],
      "type": "object"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "clear_subject": {
      "default": false,
      "description": "Clear the subject block as a unit. Mutually exclusive with `subject` (both set refuses `INVALID_INPUT`).",
      "type": "boolean"
    },
    "description": {
      "default": null,
      "description": "New one-line description. Absent = untouched; empty string = clear.",
      "type": [
        "string",
        "null"
      ]
    },
    "name": {
      "description": "Name of the mem to configure (must be a registered writable mem).",
      "type": "string"
    },
    "note": {
      "default": null,
      "description": "Optional provenance note (≤280 chars) recorded on each field's config commit.",
      "type": [
        "string",
        "null"
      ]
    },
    "subject": {
      "anyOf": [
        {
          "$ref": "#/$defs/MemSubjectInput"
        },
        {
          "type": "null"
        }
      ],
      "description": "New subject block `{scope, method?, exclusions?}`. Absent = untouched; to clear the block as a unit pass `clear_subject: true` instead."
    },
    "title": {
      "default": null,
      "description": "New display title. Absent = untouched; empty string = clear (the roster falls back to the mem name).",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "required": [
    "name"
  ],
  "type": "object"
}
```

## `memstead_mem_create`

Create and register a new writable mem at runtime. Requires workspace opt-in via `[[mem_management.create]]` rules (each `pattern` + `schemas[]`) — discover via `memstead_overview`'s `## Lifecycle Namespaces`. Engine composes the lifecycle candidate, canonicalizes `location`, runs first-match-wins glob over the rule list, then checks `schema` against the matched rule's `schemas[]` (`["*"]` admits any). Two error envelopes: `MEM_PATH_NOT_ALLOWED` carries `details.candidate`, `details.patterns`, `details.reason` (`no_allowlist_configured` / `no_match` / `outside_workspace`); `MEM_SCHEMA_NOT_ALLOWED` carries `details.candidate`, `details.matched_pattern`, `details.requested_schema`, `details.allowed_schemas`. Name-collision check runs only after a path match — out-of-namespace collision surfaces as `MEM_PATH_NOT_ALLOWED`, not `MEM_NAME_COLLISION`. Storage-residue probe catches residue surviving a prior `memstead mem unregister` or a crash; residue left by a deliberate unregister reattaches and emits `MEM_REATTACHED_AFTER_UNREGISTER` (audit signal); residue from a crash refuses with `MEM_STORAGE_RESIDUE_DETECTED` — run `memstead mem delete <name>` first. Cross-mem edge authorization is workspace policy (`[cross_mem_links]`); the matched create-rule may carry `default_cross_links`. Bootstraps the gitdir per `vcs`, loads any pre-existing markdown, and produces a seed commit carrying `note` (≤280 chars). Response carries `location`, `seed_write_id` (the identity the mem's backend minted for the seed write: a commit SHA on a git-branch mem, an opaque synthetic token on a folder mem; not a change cursor), and `schema_ref`. Pass `include_schema: true` to additionally inline the full schema body — byte-identical to `memstead_schema(name=<resolved-schema>)`. Default `false`. A mem already present at the location returns `CONFIG_ERROR`. Seed-commit failure leaves partial disk state — no implicit rollback.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$defs": {
    "MemSubjectInput": {
      "additionalProperties": false,
      "description": "Subject block for mem curation — mirrors\n`memstead_schema::MemSubject`.",
      "properties": {
        "exclusions": {
          "default": null,
          "description": "What was considered and deliberately left out — prose statements, order preserved.",
          "items": {
            "type": "string"
          },
          "type": [
            "array",
            "null"
          ]
        },
        "method": {
          "default": null,
          "description": "How the mem's content was arrived at.",
          "type": [
            "string",
            "null"
          ]
        },
        "scope": {
          "description": "What this mem covers. Required to set the block.",
          "type": "string"
        }
      },
      "required": [
        "scope"
      ],
      "type": "object"
    },
    "RecoveryActionInput": {
      "description": "Wire-shape recovery action for `memstead_mem_create`. The\nstorage-residue refusal path exposes three explicit\nrecovery options the caller picks via this enum. The wire\ntokens (`reattach` / `force_overwrite` / `hard_cleanup_first`)\nmatch `memstead_base::RecoveryAction::as_wire_str()` so the\nMCP serde shape and the CLI flag bridge converge on a single\nengine-side enum.",
      "oneOf": [
        {
          "const": "reattach",
          "description": "Adopt the residual entities; skip the seed commit. Default\nwhen the residue was left by a deliberate `memstead mem\nunregister`. Explicit `reattach` overrides the default for\ncrash-residue scenarios where the operator has verified the\ncontent is safe to adopt.",
          "type": "string"
        },
        {
          "const": "force_overwrite",
          "description": "Destroy the residue, then proceed with the normal create\npath: the residual branch and its `__MEMSTEAD` config blob are\npruned in one ref-edit transaction before the fresh seed\ncommit. Prior entities are gone by design.",
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
  "properties": {
    "description": {
      "default": null,
      "description": "Optional one-line description applied at creation — embedded in `.mem` archive exports and surfaced on rosters. Same storage as the CLI's `mem set-description`.",
      "type": [
        "string",
        "null"
      ]
    },
    "include_schema": {
      "default": false,
      "description": "Inline the resolved schema body on the response (byte-identical to `memstead_schema(name=<resolved-schema>)` at the same verbosity). Default `false` — the response carries only `schema_ref`, `name`, `location`, and `seed_write_id`. Set to `true` for first-time-schema callers that want one round-trip instead of two; the schema is workspace-stable, so for the agent's second+ mem on the same schema the omitted default is the right call.",
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
      "description": "Explicit recovery action when on-disk storage residue is detected at the composed branch path. Three accepted values: `reattach` (adopt the residual entities, skip the seed commit), `force_overwrite` (destroy the residue — it is removed atomically, so either the residue is gone and the mem is created or nothing changed; the prior entities are gone by design), `hard_cleanup_first` (refuse with `MEM_STORAGE_RESIDUE_DETECTED`, instructing the caller to run `memstead_mem_delete` first). When omitted, the engine routes by whether the residue was left by a deliberate `memstead mem unregister`: such residue defaults to `reattach` and emits a `MEM_REATTACHED_AFTER_UNREGISTER` warning; residue from a crash refuses with `MEM_STORAGE_RESIDUE_DETECTED`. Bare create against a name with no residue ignores this field."
    },
    "schema": {
      "description": "Schema pin for the new mem. Format: `name@x.y.z` — e.g. `default@1.3.0`. Resolved against the per-mem schema registry at init time.",
      "type": "string"
    },
    "schema_verbosity": {
      "description": "Verbosity of the inlined schema body when `include_schema: true`. `\"lite\"` (default, absent) inlines the cheap cold-start skeleton (entity-type names + section keys + field shapes, relationship names + endpoints, the alias pointer; prose dropped) — the right pairing for a first-mem create that only needs to orient, and byte-identical to `memstead_schema`'s default reply. `\"full\"` inlines the complete schema — byte-identical to `memstead_schema(name=<resolved-schema>, verbosity=\"full\")`. Ignored when `include_schema` is false. Any value other than `\"full\"`/`\"lite\"` returns `INVALID_INPUT` naming the bad value.",
      "type": [
        "string",
        "null"
      ]
    },
    "subject": {
      "anyOf": [
        {
          "$ref": "#/$defs/MemSubjectInput"
        },
        {
          "type": "null"
        }
      ],
      "description": "Optional subject block applied at creation — `{scope, method?, exclusions?}`: what the mem covers, how its content was arrived at, and what was deliberately left out. Same storage as the CLI's `mem set-subject`."
    },
    "title": {
      "default": null,
      "description": "Optional human-readable display title applied at creation — display text, not identity (the mem is always addressed by `name`). Same validation and storage as the CLI's `mem set-title`. Omit to leave the mem untitled.",
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
  "type": "object"
}
```

## `memstead_mem_delete`

Remove a writable mem at runtime — always destructive: removes the mem and prunes every backend-visible artifact. Requires workspace opt-in via `[[mem_management.delete]]` rules — discover the current policy via `memstead_overview`'s `## Lifecycle Namespaces` section. Engine resolves `name` (`UNKNOWN_MEM` otherwise), composes the lifecycle candidate from the mem's full hierarchical path (or the bare name for flat-layout mems), runs first-match-wins glob lookup over the delete rule list (refusing `MEM_PATH_NOT_ALLOWED` — `details.candidate`/`details.patterns`/`details.reason` discriminate `no_allowlist_configured` vs `no_match`). Refuses `MEM_REFERENCED_BY_POLICY` when the workspace `cross_mem_links` policy grants this mem as a write target (`details.referring_mems` names them). Refuses `MEM_HAS_INCOMING_REFS` when write-mem graph edges still target it (`details.referrers` lists each `{from_id, rel_types, mem}` — remove via `memstead_relate` / `memstead_update` first). On success the mem is gone — reads no longer see it and its backing storage is removed. The workspace policy is atomically scrubbed of the now-dangling `[cross_mem_links]` grants naming the deleted mem on either side. The `[[mem_management.*]]` allowlist rules are PRESERVED (exact-name and wildcard alike) — forward-looking permissions for the name; re-creating the same name needs no fresh allow rules. No per-mem commit on any backend — `note` (≤280 chars) rides on the provenance context. Response: `name`, `deleted_from_router: true`, `files_deleted: true`, and `allowlist_entries_removed[{table, pattern?, from?, to?}]` listing the scrubbed cross-link grants (`table` is always `cross_mem_links`; empty when none named the mem). On partial cleanup failure `files_deleted` ends `false` and `MEM_FILES_NOT_DELETED` warnings name the survivors: `details.reason` is `rmdir_failed` (with `details.path` + `details.error`) or `backend_prune_failed` (with `details.error`).

**Hints:** `read_only` = false, `destructive` = true, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
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
  "type": "object"
}
```

## `memstead_mem_set_schema`

Update a mem's schema pin — the integrity-driven schema-migration trigger. Stable response `{mem, schema_pin, migration_target, outcome, findings, stamped_schema}` (`stamped_schema`: the generation the mem's mutation stamp, the `ENGINE_VERSION_SKEW` marker, names after the call — a completed switch re-stamps it with the target, a dual-pin entry leaves it); branch on `outcome`: `noop` (requested == current pin), `switched` (mem already integral against the target — pin moved atomically), `migration_started` (not integral — mem enters dual-pin: writes now validate against the target, `findings` lists the non-integral entities as `{id, axis, code, detail}`), `migration_pending` (same target re-issued while repairs remain — `findings` carries the remaining entities). Migration loop: read `findings`, read both schemas via `memstead_schema`, repair each entity via `memstead_update` (validated strictly against the target; `relations_unset` is available on non-conformant entities), then re-issue this call — once every entity is integral it completes the switch. Reads stay permissive throughout; the dual-pin state survives engine restarts. Unknown mem refuses `UNKNOWN_MEM`; a schema ref that resolves to no loaded schema refuses `SCHEMA_NOT_FOUND`; malformed refs refuse `INVALID_INPUT`. Distinct from `memstead_mem_set_version`, which sets the mem *content* version, never the pin.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
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
  "type": "object"
}
```

## `memstead_mem_set_version`

Update a registered mem's `version` field. The version is consumed by `memstead_export --format mem` to stamp the archive filename and the `.mem` archive's published config — bump before publishing. Mem-create seeds `0.1.0` automatically, so this tool is the only surface that needs to fire when an agent or operator is ready to ship a new version. Gate-free: no `[[mem_management.*]]` allowlist check, no operator-mode bypass needed. Validates the new version as semver; malformed values refuse with `INVALID_INPUT`. Unknown mem name refuses with `UNKNOWN_MEM`; read-only mem refuses with `READ_ONLY_MOUNT`; a mem whose config failed to load returns `INVALID_INPUT`. Response carries `{mem, old_version, new_version, warnings}`; `MEM_RELOADED` rides on `warnings` when a sibling engine commit landed between the engine's prior snapshot and this write (no extra read needed to learn the drift). `CONFIG_WRITE_INTERVENED` rides there too when the stored config had moved since this engine read it: the write lands ON TOP of theirs (nothing of theirs is lost) and `details.fields` names what they had changed.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
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
  "type": "object"
}
```

## `memstead_overview`

Start here. Returns the schema catalogue, mem inventory, and community clusters as markdown. Schemas list as `{ref, description}` only — call `memstead_schema(name=<ref>)` for per-type bodies (lite skeleton by default; `verbosity: "full"` for prose write_rules/guidance) before any `memstead_create` / `memstead_update` / `memstead_relate`; cache per session, schema is workspace-stable. Token-budget-driven: hard-required content (mem roster, schema refs, community titles, workspace policy) always ships; heavy content is greedy-filled into the remaining budget by default-priority. Anything that didn't fit appears in the `## Hints` section with `estimated_tokens`; re-query by passing `key` into `include[]`. Override priority with `include`: keys there always ship, even past budget. Allowed `include` keys: `community_members`, `community_bridges`, `mem_distribution`, `dangling_links`. Control the budget via `token_budget` (default 8000). Frontmatter `_overview_mode` is `"complete"` (nothing dropped), `"reduced"` (heavy content omitted — see the Hints section), or `"overbudget"` (hard-required content alone exceeded the budget; raise `token_budget` or scope with `mem`). Workspace-level mutation and link policy is surfaced in `## Workspace policy` and mirrored into the `_policy` frontmatter slot — entries appear only when the value deviates from the engine default (`require_notes`, `cross_mem_links` posture). Frontmatter `_verdict_coverage`: examined/advisory/not_examined buckets. Frontmatter `_workspace_root` is the engine's absolute workspace path — target CLI calls with it, never with cwd. Pass `mem` to scope mems and schemas to any one visible mem (read-only mounts included); community detection stays workspace-global — `mem` only filters which clusters are reported (caveats on the `mem` param). `rebuild: true` recomputes the global partition. Non-fatal issues surface under `## Warnings` with a stable `code`.

**Hints:** `read_only` = true, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
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
      "description": "Opt into heavy content. Allowed keys: \"community_members\" (entity lists per cluster), \"community_bridges\" (inter-cluster edge aggregation with up to 3 sample edges per pair), \"mem_distribution\" (per-mem type_distribution), \"dangling_links\" (renders a `## Dangling Links` section, one line per unresolved reference as `[kind] source → target (in section)`, where `kind` is `DANGLING_LINK_TARGET_MISSING` (no markdown file behind the link: create the target, or remove the link), `DANGLING_LINK_NOT_RELATED` (the target is written but the referrer does not relate to it: relate the two) or `DANGLING_RELATION_TARGET_MISSING` (a relationships row naming an entity absent from the store: drop the row, or create the target); richer aggregation tracked in #12/#13). `include` keys are always shipped regardless of the token budget — use it to force content you need. Unknown keys emit a typed `warnings` entry. Schema bodies are not in this set — call memstead_schema(name=...) for the full per-type catalogue.",
      "items": {
        "type": "string"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "mem": {
      "description": "Restrict `mems[]` and `schemas[]` to any single visible mem — read-only mounts included. `used_by` inside each schema still lists all mems sharing it. Community scope: `mem` filters which clusters are *reported* (and makes `community_bridges` source-in-mem only) — it does NOT re-run detection per mem. Detection is always workspace-global and cluster ids stay the global-pass ids; passing `mem` never renumbers or re-scopes the partition. Because detection is global and disconnected / sparsely-connected nodes collapse into a single catch-all rather than forming their own cluster, a small or isolated mem-local subgraph may surface as no cluster at all under a `mem` filter.",
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
  "type": "object"
}
```

## `memstead_relate`

Connect entities with typed edges — a list of relation operations applied atomically. `relations` carries one or more `{from, to, rel_type, remove?, description?}` entries; the whole list is all-or-nothing in ONE commit per touched mem, per-entry validation identical to a single operation, in-order semantics (later entries validate against the state earlier entries produced; an acyclic check sees edges added earlier in the list). A single-relation call is a list of one. Pre-fetch the mem's schema via `memstead_schema` (see server instructions). Type names case-insensitive; stored UPPER_SNAKE_CASE. One failing entry refuses the WHOLE list — nothing commits, every failing entry reported: a list of one surfaces its entry's own typed code top-level (`INVALID_REL_TYPE` with `details.allowed` + `suggestion`, `INVALID_REL_SHAPE`, `CROSS_MEM_LINK_NOT_ALLOWED`, `CROSS_MEM_TARGET_NOT_FOUND`, `RELATIONSHIP_CYCLE` with `details.existing_path`, `INVALID_ENTITY_ID`); larger lists wrap under `BATCH_REFUSED` with `details.entries[]` of `{index, from, to, rel_type, code, message, details}` (`errors_suppressed` counts envelopes past the cap). Remove skips shape validation. Per entry: `remove: true` deletes; `from` must be real; `to` may auto-stub (`AUTO_STUB_CREATED`; into an uncreated mem: `CROSS_MEM_TARGET_MEM_UNCREATED`). Add-existing / remove-missing are typed-warning no-ops (`DUPLICATE_RELATIONSHIP` / `NO_SUCH_RELATIONSHIP`, `action: "noop"`). Response: `results[]` in submission order, each `{from, to, rel_type, action, source, _hash}` — `_hash` is that source's next `expected_hash`; top-level `write_id` (not a cursor; empty when all no-op), `warnings`, `orphan_stubs_removed` (stubs GC'd when a removed edge was their last referrer; surviving body wiki-links refuse `RELATION_HAS_BODY_LINKS`). Optional `note` rides every entry. `dry_run: true` rehearses the list: same validation and refusals, would-be actions and stubs reported, nothing lands; `write_id` stays empty (the rehearsal marker). Edges never move files.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$defs": {
    "RelateOpInput": {
      "additionalProperties": false,
      "description": "One relation operation in `memstead_relate`'s list.",
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
        "rel_type": {
          "description": "Relationship type. Canonical form is UPPER_SNAKE_CASE (USES, PART_OF, DEPENDS_ON) and is what the engine stores; case-insensitive inputs (`uses`, `Part_Of`) are accepted and echoed back in the response as their canonical form. The JSON Schema `pattern` advertises `^[A-Za-z][A-Za-z_]*$` for client-side validators; the engine enforces the same character set independently — characters outside it return `INVALID_REL_TYPE` at the engine boundary regardless of whether the client pre-filters.",
          "pattern": "^[A-Za-z][A-Za-z_]*$",
          "type": "string"
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
        }
      },
      "required": [
        "from",
        "to",
        "rel_type"
      ],
      "type": "object"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "dry_run": {
      "description": "Validate and preview the relation operations without executing — no edge lands, no stub is created, no VCS commit. dry_run runs the SAME validation a real call runs (cross-mem policy, vocabulary, description posture, acyclicity, self-loop refusal); an illegal operation refuses with the IDENTICAL typed envelope a real call would return, and a legal one reports the would-be action with `_hash` set to the PROSPECTIVE post-write source hash, `write_id` empty (the rehearsal marker), and any would-be `AUTO_STUB_CREATED` warning for an absent target — reported, never created. The follow-up real call on an unchanged mem succeeds; like create's dry_run, its `_hash` diverges from the rehearsed one whenever a wall-clock second ticks between the calls (the auto-stamped `last_modified` enters the hash) — a timestamp shift, not drift.",
      "type": [
        "boolean",
        "null"
      ]
    },
    "identity": {
      "description": "WHO is acting: an opaque identity string of your choosing — an agent name, a session handle (agent-trust plan 15). Recorded immutably alongside the mutation (commit trailer / ledger); the author≠checker independence gate compares identities and nothing else. Caller-declared and unverified, but tamper-evident in append-only history. Omit to record the session default, or nothing — legal forever, never refused; identity-less records read unconfirmable at the gate. Over-length values refuse INVALID_IDENTITY (cap 128 chars).",
      "type": [
        "string",
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
      "description": "Relation operations, applied atomically in order — all-or-nothing in one commit per touched mem. Each entry is `{from, to, rel_type, remove?, description?}` with per-entry validation identical to a single call; later entries validate against the graph state produced by earlier ones (an acyclic check sees edges added earlier in the list). A single failing entry refuses the WHOLE list and the refusal reports every failing entry.",
      "items": {
        "$ref": "#/$defs/RelateOpInput"
      },
      "type": "array"
    },
    "role": {
      "description": "The role this mutation is performed in, from the closed vocabulary `author` | `checker` | `verifier` (agent-trust plan 13). Recorded immutably alongside the mutation (commit trailer / ledger) — caller-declared but tamper-evident: bound to this operation in append-only history, it cannot be edited afterwards and identities can be cross-checked across operations. Omit to record the session default (or unspecified — legal forever, never refused, treated downstream as cannot-confirm). An unknown value refuses INVALID_ROLE naming the vocabulary.",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "required": [
    "relations"
  ],
  "type": "object"
}
```

## `memstead_reload`

Reload one writable mem's slice of the in-memory store from its on-disk branch tip — or every writable mem when `mem` is omitted. For multi-engine coexistence: a sibling (forked subagent, parallel terminal) or out-of-band `git pull` may have advanced HEAD past this engine's snapshot. The auto-reload-on-read pipeline surfaces `MEM_RELOADED` on the next read; this tool is explicit operator-driven refresh for the rare cases the throttle missed. Not a workaround for direct .md edits — restart the server instead. Per-mem form is cheap (~10 ms per few-hundred-entity mem); workspace-wide scales linearly. Response: `reports[]`, each entry `{ mem, head_before, head_after, entities_loaded, changed_entity_ids[] }`. `head_before` is the engine's prior cached SHA (canonical empty-tree hash for fresh mems); `head_after` is the freshly-peeled branch tip. `changed_entity_ids` is the union of added ∪ content-hash-changed ∪ removed entity ids — pass `head_before` to `memstead_changes_since` for the full per-entity diff. The workspace-wide form (omit `mem`) additionally picks up CLI writes to allowlist / cross-link / mutation policy (via `memstead workspace allow-create` etc.) without process restart. Per-mem form skips that workspace-level settings refresh. **Membership and the schema catalogue are fixed at boot for both default forms.** `full: true` (workspace-wide only) adds the ADDITIVE re-scan: out-of-band schema installs become resolvable, out-of-band mems mount cold, no restart; removals are skipped and reported — a deleted mem leaves the roster only on restart (its content sweep still reads current storage, so a hard-deleted branch reads empty while membership stays). Response adds `refresh` `{schemas_added, schema_removals_skipped, mems_mounted, mem_removals_skipped, failures, elapsed_ms}`; per-item failures never surface as available and never abort the rest. In-band lifecycle: `memstead_mem_create` / `memstead_mem_delete`.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "full": {
      "description": "Full refresh. `true` re-scans the schema sources and reconciles the mount roster ON TOP of the workspace-wide content reload — the same reconciliation every operation runs before it serves (`MEM_ROSTER_CHANGED`), forced here so the report is authoritative: schema versions installed out of band become resolvable, mems registered out of band mount cold, mems unregistered out of band are UNMOUNTED (their entities, search index and community partition gone; an operation naming one refuses `MEM_UNMOUNTED`), a roster entry that fails to mount is quarantined with its reason. Schema removals stay skipped. The response gains a `refresh` block — `schemas_added`, `schema_removals_skipped`, `mems_mounted`, `mems_unmounted`, `mems_quarantined`, `failures[]` (per-item; a failed source or mount never surfaces as newly available and does not abort the others), `elapsed_ms`. Incompatible with `mem`. Default `false`: the content reload alone (membership is still reconciled by the per-operation probe).",
      "type": [
        "boolean",
        "null"
      ]
    },
    "mem": {
      "description": "Writable mem name to reload. Omit to reload every writable mem. Use the per-mem form for cheap, targeted refreshes when you know which mem drifted; use the workspace-wide form (omit `mem`) when an out-of-band `git pull` may have advanced multiple branches at once, or to pick up CLI-driven workspace-policy edits (allowlist / cross-link / mutation policy) — per-mem reload skips that workspace-level settings refresh. Reload scope covers **mem-scoped** state: a mem's `sync_state` (the projection baselines `#synced`/`#verified`) rides its destination mem's config, so an out-of-band `sync_state` write — e.g. from `memstead projection advance` / `mem set-sync-state` in a sibling process — is picked up by a per-mem (or workspace-wide) reload of that mem. The engine-owned advance/disposition store (`.memstead/state/advance/`) is **workspace-store** state read fresh from disk per operation — it is reload-independent by design and is neither refreshed nor invalidated by this call.",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "type": "object"
}
```

## `memstead_rename`

Rename an entity by changing its title. Updates the entity id and its file path (`{new_slug}.md`). Atomic referrer rewrite: every Write-Mem entity whose `relationships` or section bodies point at the old id has its `[[old-slug]]` tokens rewritten in one per-mem commit. Cross-mem referrers are gated by `cross_mem_links` policy in the propagated edge's direction — a blocked direction aborts up-front with `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` (`details.from_mem`, `details.blocked_referrers`). Per-peer commits are parent-pinned; sibling-writer drift mid-rename surfaces `RENAME_PARTIAL_FAILURE` (`details.committed_mems`, `details.failed_mem`, `details.failure_cause`) — retry after reloading. Per-mem commits share a `logical_operation_id` — correlate via `memstead_changes_since`. ReadOnly referrers can't be rewritten; the old id demotes to a stub holding their edges (warning `RESIDUAL_STUB_FOR_READONLY_REFERRERS`). Requires `expected_hash`; mismatch emits `HASH_MISMATCH` (`details.current`). Slug-noop: a new title whose slug matches the current one returns `old_id` == `new_id`, empty `write_id`, and warning `TITLE_NORMALIZED_TO_SLUG_NOOP`. ID collisions error — pick a different title. Titles accept any single-line text (control characters such as tab/newline are rejected); the title is stored verbatim as display text, while characters outside Unicode alphanumerics, whitespace, and hyphen are dropped from the derived slug — warning TITLE_CHARS_DROPPED_FROM_SLUG names them (`INVALID_TITLE` remains for control chars, empty-deriving titles, over-long ids). Stubs cannot be renamed (create a real entity instead). Optional `note` (≤280 chars) — shared provenance contract, see memstead_create. Response: `old_id`, `new_id`, `_hash` (the next `expected_hash`), `warnings`, and `write_id` (an identity, never a change cursor). Provenance anchors move to the new id in the same commit.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "expected_hash": {
      "description": "Hash from memstead_entity (_hash). Required. Mismatch returns code HASH_MISMATCH with details.current carrying the current on-disk hash.",
      "type": "string"
    },
    "id": {
      "description": "Full current entity ID",
      "type": "string"
    },
    "identity": {
      "description": "WHO is acting: an opaque identity string of your choosing — an agent name, a session handle (agent-trust plan 15). Recorded immutably alongside the mutation (commit trailer / ledger); the author≠checker independence gate compares identities and nothing else. Caller-declared and unverified, but tamper-evident in append-only history. Omit to record the session default, or nothing — legal forever, never refused; identity-less records read unconfirmable at the gate. Over-length values refuse INVALID_IDENTITY (cap 128 chars).",
      "type": [
        "string",
        "null"
      ]
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
    },
    "role": {
      "description": "The role this mutation is performed in, from the closed vocabulary `author` | `checker` | `verifier` (agent-trust plan 13). Recorded immutably alongside the mutation (commit trailer / ledger) — caller-declared but tamper-evident: bound to this operation in append-only history, it cannot be edited afterwards and identities can be cross-checked across operations. Omit to record the session default (or unspecified — legal forever, never refused, treated downstream as cannot-confirm). An unknown value refuses INVALID_ROLE naming the vocabulary.",
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
  "type": "object"
}
```

## `memstead_retype`

Modify an entity's type in place. The id, file path, and every incoming edge stay; nothing is deleted or re-created, so history and provenance survive. The entity's existing sections and metadata are validated against the TARGET type and every problem is reported together in one refusal: unknown sections (`UNKNOWN_SECTION`, with `details.target_sections`, `details.target_catch_all` and a `details.proposed_section_map` to retry with), `MISSING_REQUIRED_SECTION`, metadata refusals (`UNKNOWN_METADATA_FIELD`, `INVALID_ENUM_VALUE`, `INVALID_FIELD_VALUE`, `REQUIRED_FIELD_UNSET`), block-tier constraints (`MISSING_REQUIRED_OUTGOING`, `CONSTRAINT_UNSATISFIED`), and every incoming or outgoing edge, cross-mem included, that the target type's relationship pins refuse (`INVALID_REL_SHAPE`, each edge listed with referrer and rel-type). The envelope's code is `UNKNOWN_SECTION` whenever a section key misses (fix the map first), otherwise the shared code when every problem is of one class, and `RETYPE_REFUSED` when they mix; `details.problems` carries each with its own code. Rename section keys on the way with `section_map`, and let go of fields the target does not declare with `drop_metadata`; nothing is moved or dropped silently. Referrers in a lazy (unloaded) mem are probed through storage; a mem that cannot be probed refuses `RETYPE_REFERRER_UNPROBEABLE`. Requires `expected_hash` (mismatch emits `HASH_MISMATCH` with `details.current`); `dry_run` validates and returns `prospective_hash` without writing and needs no hash. The current type refuses `RETYPE_NO_OP`. Response: `old_type`, `new_type`, `_hash` (the next `expected_hash`), `sections_renamed`, `edges_rechecked`, `write_id` (an identity, never a change cursor), and `checks_stale` with `staleness_note`: the content hash moved, so check records and derivation baselines on this entity are stale. One commit lands with the `retype` provenance kind. Optional `note` — shared provenance contract, see memstead_create.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "drop_metadata": {
      "description": "Metadata keys to drop explicitly: fields the current type declares and the target does not (a spec's `level` on the way to a memo). Never inferred — an undeclared field not listed here refuses UNKNOWN_METADATA_FIELD, since dropping data unannounced is what the write gates prevent. A key the entity does not carry is a no-op.",
      "items": {
        "type": "string"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "dry_run": {
      "description": "Validate everything (sections, metadata, every incoming and outgoing edge, block-tier constraints) and return the prospective `_hash` without writing, committing, or changing the store. The optimistic lock is skipped on a dry run.",
      "type": [
        "boolean",
        "null"
      ]
    },
    "expected_hash": {
      "description": "Hash from memstead_entity (_hash). Required unless `dry_run` is true. Mismatch returns code HASH_MISMATCH with details.current carrying the current on-disk hash.",
      "type": [
        "string",
        "null"
      ]
    },
    "id": {
      "description": "Full entity ID (`mem--slug`) of the entity to retype",
      "type": "string"
    },
    "identity": {
      "description": "WHO is acting: an opaque identity string of your choosing (agent-trust plan 15). Recorded immutably alongside the mutation; the author≠checker independence gate compares identities and nothing else. Omit to record the session default. Over-length values refuse INVALID_IDENTITY (cap 128 chars).",
      "type": [
        "string",
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
    "role": {
      "description": "The role this mutation is performed in, from the closed vocabulary `author` | `checker` | `verifier` (agent-trust plan 13). Recorded immutably alongside the mutation (commit trailer / ledger). Omit to record the session default. An unknown value refuses INVALID_ROLE naming the vocabulary.",
      "type": [
        "string",
        "null"
      ]
    },
    "section_map": {
      "additionalProperties": {
        "type": "string"
      },
      "description": "Section key renames applied before validation, `{\"old_key\": \"new_key\"}`. A section the target type does not declare refuses UNKNOWN_SECTION with `details.target_sections`, `details.target_catch_all` and a `details.proposed_section_map` to retry with; sections are never moved silently.",
      "type": [
        "object",
        "null"
      ]
    },
    "target_type": {
      "description": "The target entity type, as declared by the mem's schema (`memstead_schema`). Unknown types refuse UNKNOWN_ENTITY_TYPE; the current type refuses RETYPE_NO_OP.",
      "type": "string"
    }
  },
  "required": [
    "id",
    "target_type"
  ],
  "type": "object"
}
```

## `memstead_schema`

Read one schema. Default `verbosity` is "lite": a structural skeleton — entity-type names with their section keys (`required` flags kept) and metadata-field shapes (`enum` values + `default` + `pattern`), `last_resort` on the fallback type, `required_outgoing`, relationship names with endpoint constraints, plus `relationship_mode` (strict|open), `community.{resolution, seed}`, `used_by[]`, top-level `origin` (`first-party` for an engine built-in or workspace-authored schema; `third-party` otherwise), and top-level `alias_target_rel_type` (names the rel-type body wiki-links auto-emit; absent means unbacked wiki-links refuse with `WIKILINK_WITHOUT_RELATION`). The skeleton carries every legality flag needed to author a valid write. Pass `verbosity: "full"` for the prose layer — per-section `write_rules`, type-level `writing_guidance`, `system_context`, relationship prose, `default_writing_guidance` — before substantial authoring; scope it with `types: ["<name>", …]` to get the complete prose for exactly the types you will write (unserved types listed in `types_omitted`; an unknown name refuses `UNKNOWN_ENTITY_TYPE` naming the valid set). An unscoped full reply exceeding `token_budget` degrades visibly — per-type prose drops to the skeleton, `_schema_mode: "reduced"` + `_hint` steer to `types` — never silent truncation. Full and lite ship the heavy arrays under distinct keys (`types`/`relationships` vs `types_summary`/`relationships_summary`) — decode by key presence. A `third-party` schema is served structural-only regardless of `verbosity` — its prose-instruction fields never reach the agent as instructions. Pass exactly one of `name` — bare ("default") or canonical pin — or `mem`; both or neither returns `INVALID_INPUT`. Call once per writable mem per session before create/update/relate (schema-discovery contract); cache — schema is workspace-stable. Returns `ENTITY_NOT_FOUND` for an unknown `name`, `UNKNOWN_MEM` for an unmounted `mem` (`details.known_mems` lists the roster).

**Hints:** `read_only` = true, `destructive` = false, `idempotent` = true, `open_world` = false

**Input schema:**

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "mem": {
      "description": "Mem name as listed in memstead_overview's `## Mems` section. The engine resolves the mem's pinned `schema_ref` from the workspace's mount roster and proceeds identically to the `name`-driven path. Mutually exclusive with `name`. Returns `UNKNOWN_MEM` when the mem is not mounted.",
      "type": [
        "string",
        "null"
      ]
    },
    "name": {
      "description": "Schema name as listed in memstead_overview's `## Schemas` section (e.g. \"default\" or \"default@1.3.0\"). Schemas are workspace-globally unique by name; the workspace registry resolves a bare name to the pinned version. Mutually exclusive with `mem`.",
      "type": [
        "string",
        "null"
      ]
    },
    "token_budget": {
      "description": "Token budget for an UNSCOPED `verbosity: \"full\"` reply. When the whole-package full payload would exceed it, the reply degrades visibly instead of overflowing the response cap: per-type prose drops to the lite skeleton, `_schema_mode: \"reduced\"` is stamped, and `_hint` steers to per-type retrieval via `types`. Scoped (`types`) requests are never degraded. Omit to use the server's default budget.",
      "format": "uint",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "types": {
      "description": "Scope the per-type payload to these entity-type names (as listed in the lite skeleton). With `verbosity: \"full\"` this is the way to drill before substantial authoring: the reply carries the full package-level context plus the complete prose for exactly the named types, with every unserved type listed in `types_omitted` — one under-budget reply instead of a whole-package spill. A name not in the schema refuses `UNKNOWN_ENTITY_TYPE` naming the valid types. Omit for the whole roster.",
      "items": {
        "type": "string"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "verbosity": {
      "description": "Verbosity of the schema body. `\"lite\"` (default, absent) returns a cheap cold-start skeleton: entity-type names with their section keys (and `required` markers) and metadata-field shapes (name, `required`, `enum`, `default`), relationship-type names with their `allowed_sources`/`allowed_targets`, `manual_authoring`, `acyclic`, and `per_edge_description` — plus the top-level `alias_target_rel_type` pointer — with the long-form prose dropped. The lite skeleton carries every flag needed to author a legal write. `\"full\"` returns the complete payload — every description, `when_to_use`, write-rule, and writing-guidance string; escalate to full for the human-readable guidance before substantial authoring. Heavy arrays ship under distinct keys per mode (`types`/`relationships` vs. `types_summary`/`relationships_summary`). Any value other than `\"full\"`/`\"lite\"` returns `INVALID_INPUT` naming the bad value.",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "type": "object"
}
```

## `memstead_search`

Search entities by lexical content + structural filters. Dual channel: rendered markdown plus typed `SearchResultEnvelope` on `structured_content`: `{ _total, _returned, _offset, _total_tokens, hits[], facets, warnings }`; each hit: `score`, `score_breakdown`, `matched_terms`, `expansion`, `origin` (trust class), `snippet`. A page is bounded to `token_budget` (default 12000); an overflowing page trims with a `SEARCH_RESULTS_TRUNCATED` warning (`kept`/`budget`); `_total` stays full; page with `offset`. Expand a concept into keyword variants in `query.any` (OR, rank-boosted); excludes in `query.not`; `query.phrase` for exact adjacency; `query.field` to restrict to one field. Set `expand_via` to relationship types — reached hits carry `expansion` metadata incl. `via_direction` + decayed score (0.5^depth); `direction` (out|in|both) narrows the walk per hop. `facets` (by_type, by_mem, by_level, by_status, by_confidence, by_subsection, by_expansion) compose results. Sub-heading matches carry `heading_path`. `stub: true|false` filters stub status (with `entity_type` it flags `STUB_FILTER_EXCLUDES_ALL`). Equality filters on `filterable: equality` fields ride on `filters` (e.g. `{"level": "M0"}`); one code per outcome: `FILTER_TYPE_SCOPED` (applied, type-narrowed), `FIELD_NOT_FILTERABLE` (ignored — result unfiltered, never emptied), `UNKNOWN_FILTER_KEY` (ignored), `INVALID_ENUM_VALUE` (applies but matches nothing; `details.allowed`). `related_to`: proximity-ranked neighbourhood, bounded with `NEIGHBOURHOOD_CAPPED`. Range filters on `filterable: range` fields ride on `range_filters` (`min_<field>`/`max_<field>`/`<field>_before`/`<field>_after`), same contract: `RANGE_FILTER_KEY_MALFORMED`, `RANGE_FILTER_TYPE_SCOPED`, `UNKNOWN_RANGE_FILTER_FIELD`, `FIELD_NOT_RANGE_FILTERABLE`. A `mem` filter naming no visible mem refuses `UNKNOWN_MEM`; an empty result is a valid mem with no matches. A failing mem index surfaces `SEARCH_MEM_INDEX_UNAVAILABLE` (`details.mem`/`details.reason`). Omit `query` for a pure metadata filter.

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
    },
    "TraversalDirection": {
      "description": "Traversal direction relative to the seed, applied at EVERY hop —\ndepth > 1 is a pure transitive closure in the chosen direction,\nnever a mixed walk (an entity reachable only by alternating\ndirections is not in an `out` or `in` result at any depth; that\nper-hop property is what makes a fall-through analysis correct).\n\n`in`/`out` describe the edge relative to the seed and match the\nStore's own vocabulary — domain words (ancestors/upstream) invert\nper schema, so the engine does not use them.",
      "oneOf": [
        {
          "const": "out",
          "description": "Follow edges pointing away from the seed (seed → target).",
          "type": "string"
        },
        {
          "const": "in",
          "description": "Follow edges pointing at the seed (source → seed).",
          "type": "string"
        },
        {
          "const": "both",
          "description": "Follow both — the historical undirected walk, and the default:\na query that omits the selector returns exactly what it always\nreturned.",
          "type": "string"
        }
      ]
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
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
    "direction": {
      "anyOf": [
        {
          "$ref": "#/$defs/TraversalDirection"
        },
        {
          "type": "null"
        }
      ],
      "description": "Traversal direction for `related_to` and `expand_via`, applied at EVERY hop: \"out\" follows edges pointing away from the seed (what does this rest on), \"in\" follows edges pointing at it (what rests on this), \"both\" (default) is the historical undirected walk. Depth > 1 is a pure transitive closure in the chosen direction — never a mixed walk. Expanded hits report the reaching edge's direction as `expansion.via_direction`."
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
  "type": "object"
}
```

## `memstead_update`

Modify an existing entity. Pre-fetch the target mem's schema (`memstead_schema`). Pass the entity's `_hash` as `expected_hash` when the update CHANGES CONTENT (anchors-only may omit it; a stored triple is replaced, hash-less unless restated: `anchors_changed`) — mismatch emits `HASH_MISMATCH` (`details.current` = live hash). Warnings `INLINE_WIKI_LINK_AUTO_STUBBED`, `CROSS_SCHEMA_LINK_UNDECLARED`, `MISSING_REQUIRED_OUTGOING` mirror memstead_create's. Section modes (one per key): `sections` (replace), `append_sections`, `patch_sections` (find-and-replace; every occurrence via `all: true`; errors on missing `old` or empty section), `sections_unset` (removes heading+body; no-op if absent; refused for REQUIRED). Schema-bound errors (`UNKNOWN_SECTION`, `UNKNOWN_METADATA_FIELD`, `INVALID_ENUM_VALUE`, `INVALID_FIELD_VALUE`, `SECTION_CONTENT_INVALID`, `REQUIRED_FIELD_UNSET`, …) carry recovery payloads — fix from `details` (see server instructions). `metadata` sets frontmatter; `metadata_unset` removes it (no-ops on absent or section keys). Setting and unsetting one key is a hard error. Read-only on SET (`READ_ONLY_FIELD`): `mem`/`id`/`type` (memstead_rename for title) and engine-stamped `created_date`/`last_modified` (also unset-refused). Unset MAY name the reserved triple (sanctioned repair; no-op when healthy). Stubs cannot be updated — memstead_create as real first. Optional `note` (≤280 chars), see instructions. No-op short-circuit: post-state bytes-identical to disk returns `UPDATE_NOOP`, empty `write_id`, unchanged `_hash` — `expected_hash` stays stable. `dry_run: true` validates then previews OR recovers from a stale hash: it bypasses ONLY the `expected_hash` check (returns current `_hash` + `prospective_hash`); section/field validation still refuses with the same typed envelope — never an invalid update reported clean. Reuse `_hash` as `expected_hash`, never `prospective_hash`. A body-link removal orphaning its stub target GC's it into `orphan_stubs_removed`. Real writes carry `write_id`.

**Hints:** `read_only` = false, `destructive` = false, `idempotent` = false, `open_world` = false

**Input schema:**

```json
{
  "$defs": {
    "AnchorInputParam": {
      "additionalProperties": false,
      "description": "One `anchors[]` element on `memstead_create` / `memstead_update` — a\nprovenance record tying the entity to a source artifact. Permissive by\ndesign: every field is optional / string-typed so a malformed element\n(unknown class or grain, missing artifact, hash on a non-hash class,\ngrain the medium's namespace cannot express) refuses the whole mutation\nwith a typed `INVALID_ANCHOR` envelope carrying recovery `details` —\nrather than an opaque schema-deserialisation error. Converts to the\nengine's `AnchorInput` which validates it. Not folded into `_hash`.",
      "properties": {
        "artifact": {
          "default": null,
          "description": "Artifact reference in the medium's own namespace — a repo-relative path, `path@commit`, URL, or entity id, interpreted per `grain`. Required; a missing/empty value refuses INVALID_ANCHOR.",
          "type": [
            "string",
            "null"
          ]
        },
        "at_version": {
          "anyOf": [
            {
              "$ref": "#/$defs/AnchorVersionParam"
            },
            {
              "type": "null"
            }
          ],
          "description": "Medium-typed pinned version this anchor was recorded against: `{ kind: \"commit\"|\"snapshot\"|\"etag\", value: \"<token>\" }`. Omit for a plain-path medium with no retrievable version."
        },
        "binding": {
          "default": null,
          "description": "`hash(D)` of the binding that produced this anchor, when a binding produced it. Omit for a manually-authored anchor.",
          "type": [
            "string",
            "null"
          ]
        },
        "class": {
          "default": null,
          "description": "Provenance class — the entity's epistemic standing toward the artifact: `anchored` | `derived` | `authored` | `informed-by`. `anchored`/`derived` carry hash semantics (a `hash` is permitted and participates in drift adjudication); `authored`/`informed-by` do not (supplying `hash` refuses INVALID_ANCHOR). An unknown value refuses INVALID_ANCHOR.",
          "type": [
            "string",
            "null"
          ]
        },
        "content": {
          "default": null,
          "description": "The observed artifact CONTENT (UTF-8 text), for the engine to compute `hash` from through its preparation registry — the write-time observation for a grain the engine cannot observe itself: a `url` anchor (the engine never fetches; what you read is canonicalized exactly as a path grain's bytes are). Also accepted for `span`/`file`. Mutually exclusive with `hash` (both refuses INVALID_ANCHOR); refused on `authored`/`informed-by`, and on the `entity`/`tree` grains, whose prepared form is never computed from supplied bytes.",
          "type": [
            "string",
            "null"
          ]
        },
        "derived_from": {
          "default": null,
          "description": "For a `derived` class: the input artifact refs the entity was derived from. Empty/omitted for every other class.",
          "items": {
            "type": "string"
          },
          "type": [
            "array",
            "null"
          ]
        },
        "grain": {
          "default": null,
          "description": "Granularity of the artifact reference: `span` | `file` | `tree` | `url` | `entity`. `span`/`file`/`tree` need a path-shaped medium namespace and `entity` the entity namespace, or the mutation refuses INVALID_ANCHOR; `url` is admitted beside every medium (a URL never enters a path namespace). A path-shaped grain whose artifact is a URL refuses INVALID_ANCHOR naming that rule: use `grain: url`. An unknown value refuses INVALID_ANCHOR.",
          "type": [
            "string",
            "null"
          ]
        },
        "hash": {
          "default": null,
          "description": "Content hash over the PREPARED artifact form (never raw bytes). Permitted only on hash-bearing classes (`anchored`/`derived`); supplying it on `authored`/`informed-by` refuses INVALID_ANCHOR.",
          "type": [
            "string",
            "null"
          ]
        },
        "hash_stability": {
          "default": null,
          "description": "Medium's declared hash stability: `stable` | `unstable` (defaults per grain: `url` to `unstable`, every other grain to `stable`). An unstable-source hash break resolves `recheck`, not `drifted`.",
          "type": [
            "string",
            "null"
          ]
        },
        "source": {
          "default": null,
          "description": "NAME of the source (as declared in the producing binding's `sources[]`) that produced this anchor — lets a discovery run be measured per entry point. Name the source you are working from whenever the binding declares more than one. Present-but-empty refuses INVALID_ANCHOR; a name the (resolvable) producing binding does not declare refuses with the declared names in `details.declared`. Omit for a manually-authored anchor.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "type": "object"
    },
    "AnchorUnsetParam": {
      "additionalProperties": false,
      "description": "One `anchors_unset[]` entry on `memstead_update` — an explicit anchor-\nremoval selector. Permissive like [`AnchorInputParam`]: a malformed\nselector refuses the whole mutation with a typed `INVALID_ANCHOR`\nenvelope rather than an opaque schema-deserialisation error.",
      "properties": {
        "artifact": {
          "default": null,
          "description": "Artifact reference whose anchors to remove, exactly as stored. Required; a missing/empty value refuses INVALID_ANCHOR. Bare (no grain/class) removes every anchor on the artifact.",
          "type": [
            "string",
            "null"
          ]
        },
        "class": {
          "default": null,
          "description": "Optional narrowing: only remove anchors of this provenance class (`anchored` | `derived` | `authored` | `informed-by`). An unknown value refuses INVALID_ANCHOR.",
          "type": [
            "string",
            "null"
          ]
        },
        "grain": {
          "default": null,
          "description": "Optional narrowing: only remove anchors of this grain (`span` | `file` | `tree` | `url` | `entity`). An unknown value refuses INVALID_ANCHOR.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "type": "object"
    },
    "AnchorVersionParam": {
      "additionalProperties": false,
      "description": "The medium-typed pinned version sub-object of an [`AnchorInputParam`].",
      "properties": {
        "kind": {
          "description": "Version kind: `commit` (git / path+commit) | `snapshot` (graph) | `etag` (web).",
          "type": "string"
        },
        "value": {
          "description": "The version token — commit id, graph snapshot token, or web ETag.",
          "type": "string"
        }
      },
      "required": [
        "kind",
        "value"
      ],
      "type": "object"
    },
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
    "PatchesInput": {
      "anyOf": [
        {
          "$ref": "#/$defs/PatchInput"
        },
        {
          "items": {
            "$ref": "#/$defs/PatchInput"
          },
          "type": "array"
        }
      ],
      "description": "One patch or a list of patches for a single section — the wire\naccepts both shapes (`{...}` and `[{...}, ...]`); a single object is\nthe historical form and stays valid unchanged."
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
        "rel_type": {
          "description": "Relationship type. Canonical form is UPPER_SNAKE_CASE (USES, PART_OF, DEPENDS_ON) and is what the engine stores; case-insensitive inputs (`uses`, `Part_Of`) are accepted and echoed back in the response as their canonical form. The JSON Schema `pattern` advertises `^[A-Za-z][A-Za-z_]*$` for client-side validators; the engine enforces the same character set independently — characters outside it return `INVALID_REL_TYPE` at the engine boundary regardless of whether the client pre-filters.",
          "pattern": "^[A-Za-z][A-Za-z_]*$",
          "type": "string"
        },
        "target": {
          "description": "Full target entity ID",
          "type": "string"
        }
      },
      "required": [
        "target",
        "rel_type"
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
  "properties": {
    "anchors": {
      "description": "Optional provenance anchors to attach to this entity — durable records tying it to the source artifacts it describes. Anchors MERGE into the entity's existing set: an incoming anchor replaces the existing anchor with the same `(artifact, grain, class)` triple and appends otherwise — writing anchors never removes an anchor this call did not name in `anchors_unset` (an empty or omitted list leaves the stored set untouched; incremental anchoring works). Written into the mem-branch anchors sidecar in the SAME commit as the update (atomic). An update carrying only `anchors` (no section/metadata change) still commits the sidecar. A malformed element refuses the whole update with `INVALID_ANCHOR` and nothing is written. A payload naming one `(artifact, grain, class)` triple TWICE refuses (`INVALID_ANCHOR`): that triple is one row, so the repeats would collapse to the last one and an anchor you sent would vanish unannounced. A re-pin that omits `hash` KEEPS the stored baseline rather than dropping it; supply `hash` to replace it, or `anchors_unset` the row and write it fresh to clear it. A `span` anchor's locator must be usable (`#L<start>-L<end>`, `#<unit-key>`, or no locator for the whole file); a locator that addresses nothing refuses, and a span the engine could not check (no `content` supplied) is accepted and recorded as unverified. Anchors do NOT participate in `_hash`.",
      "items": {
        "$ref": "#/$defs/AnchorInputParam"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "anchors_unset": {
      "description": "Explicit anchor removals, applied BEFORE the `anchors` merge in the same mutation (mirroring `metadata_unset` / `relations_unset`) — removal is explicit, never a side effect of writing. Each entry names an `artifact` and may narrow by `grain` and/or `class`; a bare artifact removes every anchor on it. Unsetting an anchor that does not exist is a no-op, not an error. Full-replace stays expressible: unset the artifact(s) and write the new set in one call. A malformed selector (missing artifact, unknown grain/class) refuses the whole update with `INVALID_ANCHOR`.",
      "items": {
        "$ref": "#/$defs/AnchorUnsetParam"
      },
      "type": [
        "array",
        "null"
      ]
    },
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
      "description": "Atomic batched relation declarations applied before the section/metadata changes land. Each `{ target, rel_type }` is validated like a `memstead_relate` call (schema-shape, cross-mem policy, target-id grammar) and appended to the entity's relations; absent Write-mem targets are auto-stubbed identically to the relate path. The strict wiki-link/relation validator then runs against the post-mutation state with the freshly-declared relations in place — so adding a `[[target]]` body wiki-link + declaring the backing `REFERENCES` relation can land in a single `memstead_update` call (without `declare_relations`, the post-migration strict validator would refuse the body link). Each successful entry is echoed in `relations_declared` on the response with `target_was_stubbed` flagging whether the target was absent at call time. Omit for mutations that don't introduce new relations.",
      "items": {
        "$ref": "#/$defs/RelationInput"
      },
      "type": [
        "array",
        "null"
      ]
    },
    "dry_run": {
      "description": "Validate and preview what would change without executing. On a valid update the response carries the unchanged on-disk hash as `_hash` plus the post-write `prospective_hash` — pass `_hash` as `expected_hash` on the follow-up real call. `dry_run` deliberately bypasses ONLY the `expected_hash` check (the returned `_hash` is the current on-disk hash, safe to reuse on the real follow-up), making it the designated recovery path for stale hashes. It does NOT relax the rest of validation: an update that a real call would refuse on section/field grounds (`UNKNOWN_SECTION`, `UNKNOWN_METADATA_FIELD`, `INVALID_ENUM_VALUE`, `INVALID_FIELD_VALUE`, `REQUIRED_FIELD_UNSET`, `PATCH_OLD_NOT_FOUND`, …) refuses under dry_run with the same typed envelope and the same recovery `details.*` — that refusal is the pre-flight signal, not a clean preview. So dry_run and a real write agree on validity (modulo the intentionally-skipped hash check).",
      "type": [
        "boolean",
        "null"
      ]
    },
    "expected_hash": {
      "default": null,
      "description": "Hash from memstead_entity response (_hash field). Required for any update that changes content (sections, metadata, relations) — read the entity first. OMIT it for an anchors-only update (`anchors` / `anchors_unset` and nothing else): the anchors sidecar is outside `_hash` by design, so the token would compare a value the write cannot move, and requiring it taxed exactly the backfill flows anchors exist for. An update that changes content without it refuses `EXPECTED_HASH_REQUIRED`. Mismatch returns code HASH_MISMATCH with details.current carrying the current on-disk hash; pass dry_run=true to bypass the check as a recovery path.",
      "type": [
        "string",
        "null"
      ]
    },
    "id": {
      "description": "Full entity ID to update",
      "type": "string"
    },
    "identity": {
      "description": "WHO is acting: an opaque identity string of your choosing — an agent name, a session handle (agent-trust plan 15). Recorded immutably alongside the mutation (commit trailer / ledger); the author≠checker independence gate compares identities and nothing else. Caller-declared and unverified, but tamper-evident in append-only history. Omit to record the session default, or nothing — legal forever, never refused; identity-less records read unconfirmable at the gate. Over-length values refuse INVALID_IDENTITY (cap 128 chars).",
      "type": [
        "string",
        "null"
      ]
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
      "description": "Metadata keys to remove. Silent no-op if absent. Errors on the engine-stamped timestamp fields (created_date / last_modified) and on schema-required fields. The reserved identity triple (mem / id / type) is asymmetric by design: SET refuses (READ_ONLY_FIELD, here and on create; a type change is `memstead_retype`, which re-validates the entity against the target type) but UNSET is allowed — the sanctioned repair for an entity that acquired a smuggled reserved key before the write gates closed. Unsetting `type` never leaves the entity typeless (the engine re-seeds the authoritative discriminator; on a healthy entity it is a no-op). Cannot overlap with `metadata` keys — pass one or the other per key.",
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
        "$ref": "#/$defs/PatchesInput"
      },
      "description": "Section fields to patch (find-and-replace): { \"specifies\": { \"old\": \"...\", \"new\": \"...\" } } — or a LIST of patches per section, applied in order against the evolving body: { \"specifies\": [{...}, {...}] }. Batched edits to one section land in one call.",
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
    "role": {
      "description": "The role this mutation is performed in, from the closed vocabulary `author` | `checker` | `verifier` (agent-trust plan 13). Recorded immutably alongside the mutation (commit trailer / ledger) — caller-declared but tamper-evident: bound to this operation in append-only history, it cannot be edited afterwards and identities can be cross-checked across operations. Omit to record the session default (or unspecified — legal forever, never refused, treated downstream as cannot-confirm). An unknown value refuses INVALID_ROLE naming the vocabulary.",
      "type": [
        "string",
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
    },
    "sections_unset": {
      "description": "Section keys to REMOVE from the entity — heading and body both: [\"notes\"]. The close gesture for a declared-but-empty heading with nothing to receive, and the repair for a legacy undeclared heading. Silent no-op on an absent key (symmetric with metadata_unset). Refuses for a schema-REQUIRED section (MISSING_REQUIRED_SECTION — fill it instead), for `relationships` (SECTION_NOT_UPDATABLE), and for a key also named in sections / append_sections / patch_sections (CONFLICTING_SECTION_MODES).",
      "items": {
        "type": "string"
      },
      "type": [
        "array",
        "null"
      ]
    }
  },
  "required": [
    "id"
  ],
  "type": "object"
}
```

