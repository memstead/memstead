---
title: Agent recipes
description: "Worked MCP tool-call sequences with real request and response payloads: cold start, search then read, create with recovery, optimistic locking, typed edges."
sidebar:
  order: 4
---

The [MCP tools reference](../../reference/mcp/) is the authoritative surface description — every tool, every parameter, every error code. This page is its on-ramp: five worked sequences an agent actually runs, captured verbatim against a live `memstead-mcp` server over a small workspace (one mem, `my-graph`, pinned to `default@1.0.0`). Requests show the `tools/call` params; responses show the `structured_content` envelope. Long payloads are trimmed where marked (`…`) — nothing is paraphrased.

## Recipe 1 — cold start: orient before you touch anything

New session, unknown workspace. `memstead_overview` returns the schema catalogue, mem inventory, and community clusters; then fetch the full schema body for the mem you'll write to. Cache it — schema is workspace-stable, one fetch per session.

**Call 1: `memstead_overview`**

```json
{ "name": "memstead_overview", "arguments": {} }
```

The response's text channel is budget-controlled markdown:

```text
---
_overview_mode: complete
_budget_requested: 8000
_budget_used: 168
_cluster_count: 2
_entity_count: 3
_modularity: 0
_chunk: 1 of 1
_total_chunks: 1
---

…

## Schemas

_(call `memstead_schema(name=<ref>)` for the full per-type catalogue, sections, fields, and relationship vocabulary)_

### default@1.0.0

Built-in memstead schema covering ten knowledge-type kinds: spec, memo, assertion,
concept, inquiry, model, narrative, perspective, principle, process. Spans
spec-authoring and knowledge-capture in a single relationship vocabulary.

## Mems

### my-graph

- **Schema:** default@1.0.0
- **Version:** 0.1.0
- **Entities:** 3
- **By type:** concept=3

## Communities

### Cluster 0 (2 entities)
Idempotency · Retry
- my-graph--idempotency
- my-graph--retry
…
```

**Call 2: `memstead_schema`** — the mem's pin is `default@1.0.0`, so:

```json
{ "name": "memstead_schema", "arguments": { "name": "default@1.0.0" } }
```

```json
{
  "ref": "default@1.0.0",
  "origin": "first-party",
  "relationship_mode": "strict",
  "alias_target_rel_type": "REFERENCES",
  "used_by": ["my-graph"],
  "description": "Built-in memstead schema covering ten knowledge-type kinds: spec, memo, assertion,\nconcept, inquiry, model, narrative, perspective, principle, process. Spans\nspec-authoring and knowledge-capture in a single relationship vocabulary.\n",
  "when_to_use": "Use when you want a general-purpose knowledge graph without authoring a\ncustom schema. …",
  "relationships": [
    {
      "name": "PART_OF",
      "description": "Hierarchical containment — the source is structurally part of the target and meaningless without it.",
      "when_to_use": "Taxonomy. Parent owns the child as a structural part. …",
      "default_weight": 3.0,
      "acyclic": true,
      "manual_authoring": "allow",
      "per_edge_description": "forbidden",
      "allowed_sources": [],
      "allowed_targets": []
    },
    {
      "name": "REFERENCES",
      "description": "Soft reference from the source to the target without ownership or dependency.",
      "when_to_use": "Mentioning, pointing at, citing. Emitted automatically from inline wiki-links — avoid authoring by hand.",
      "default_weight": 0.5,
      "acyclic": false,
      "manual_authoring": "forbidden",
      "per_edge_description": "forbidden",
      "allowed_sources": [],
      "allowed_targets": []
    },
    "… 35 more relationship definitions …"
  ],
  "types": [
    {
      "name": "concept",
      "description": "A precise definition of an abstract idea, term, or mental model —\nenough to distinguish instances from non-instances.\n",
      "sections": [
        {
          "key": "definition",
          "heading": "Definition",
          "required": true,
          "write_rules": ["One or two sentences. What this concept IS — necessary and sufficient conditions. Must enable someone to distinguish instances from non-instances."]
        },
        "… explanation (required), boundaries, significance …"
      ],
      "fields": [
        {
          "name": "maturity",
          "required": true,
          "default": "emerging",
          "enum": ["emerging", "stable", "established"],
          "filterable": "equality",
          "description": "How settled the definition is — emerging, stable, or established."
        },
        "… type, created_date, last_modified, abstraction_level, tags …"
      ],
      "propagating_relationships": ["DEPENDS_ON", "GENERALIZES"]
    },
    "… 9 more types …"
  ]
}
```

Now you know the section keys a `create` must carry, the metadata enums, and which edge types are legal — before the first write, not after the first refusal.

## Recipe 2 — search, then read

No stemming, no wildcards: expand a concept into keyword variants in `query.any` (OR semantics — hits matching more terms rank higher). Then read the winning entity by id.

**Call 1: `memstead_search`**

```json
{
  "name": "memstead_search",
  "arguments": {
    "query": { "any": ["idempotency", "idempotent"] },
    "limit": 5
  }
}
```

```json
{
  "_total": 2,
  "_returned": 2,
  "_offset": 0,
  "_total_tokens": 209,
  "facets": {
    "by_type": { "concept": 2 },
    "by_mem": { "my-graph": 2 },
    "by_expansion": { "primary": 2 },
    "by_level": {}, "by_status": {}, "by_confidence": {}, "by_subsection": []
  },
  "hits": [
    {
      "id": "my-graph--idempotency",
      "title": "Idempotency",
      "entity_type": "concept",
      "mem": "my-graph",
      "origin": "first-party",
      "stub": false,
      "score": 194.1524658203125,
      "score_breakdown": {
        "bm25": 0.0,
        "title_boost": 138.68032836914062,
        "field_weights": { "definition": 55.472129821777344 }
      },
      "matched_terms": {
        "idempotency": [{ "field": "title", "snippet": "**Idempotency**" }],
        "idempotent": [{ "field": "definition", "snippet": "An operation is **idempotent** when applying it twice has the same effect as app..." }]
      },
      "snippet": "**Idempotency**",
      "summary_heading": "Definition",
      "summary_value": "An operation is idempotent when applying it twice has the same effect as applying it once.",
      "tokens": 101
    },
    "… 1 more hit (my-graph--retry, score 59.7) …"
  ]
}
```

**Call 2: `memstead_entity`**

```json
{ "name": "memstead_entity", "arguments": { "id": "my-graph--idempotency" } }
```

```json
{
  "id": "my-graph--idempotency",
  "mem": "my-graph",
  "type": "concept",
  "origin": "first-party",
  "_hash": "f668d8042f4499ee",
  "_mem_schema": "default@1.0.0",
  "_tokens": 117,
  "metadata": {
    "abstraction_level": "concrete",
    "created_date": "2026-07-02T21:13:14Z",
    "last_modified": "2026-07-02T21:13:14Z",
    "maturity": "emerging"
  },
  "sections": {
    "definition": "An operation is idempotent when applying it twice has the same effect as applying it once.",
    "explanation": "It matters for retries — a client can safely resend a request without double-applying it.",
    "boundaries": "",
    "significance": ""
  },
  "relationships": []
}
```

Keep `_hash` — it is the optimistic-lock token every mutation on this entity wants (Recipe 4). Note the hit's `tokens` field: size a read before making it.

## Recipe 3 — create, and recover from a refusal

The engine validates every write against the schema. A refusal is not a dead end: the error envelope's `details` carries exactly what's missing, including the section's `write_rules` — fix from `details` rather than re-fetching the schema.

**Call 1 — refused** (the `concept` type requires an `explanation` section this call doesn't carry):

```json
{
  "name": "memstead_create",
  "arguments": {
    "entity_type": "concept",
    "title": "Optimistic locking",
    "sections": {
      "definition": "Concurrency control that detects conflicts at write time via a version token instead of holding locks."
    }
  }
}
```

```json
{
  "code": "MISSING_REQUIRED_SECTION",
  "message": "missing 1 required section(s) for type 'concept':\n  - 'explanation' (Explanation) — write_rules: Expand the definition. How the concept works, what it entails, why it matters. Use examples and analogies where helpful.\nType guidance:\n  - concept: Concepts are precise definitions …",
  "details": {
    "entity_type": "concept",
    "missing_count": 1,
    "sections": [
      {
        "entity_type": "concept",
        "key": "explanation",
        "heading": "Explanation",
        "write_rules": ["Expand the definition. How the concept works, what it entails, why it matters. Use examples and analogies where helpful."]
      }
    ],
    "type_guidance": { "concept": ["Concepts are precise definitions — each must enable distinguishing instances from non-instances.", "…"] }
  }
}
```

**Call 2 — corrected** (same call plus the named section):

```json
{
  "name": "memstead_create",
  "arguments": {
    "entity_type": "concept",
    "title": "Optimistic locking",
    "sections": {
      "definition": "Concurrency control that detects conflicts at write time via a version token instead of holding locks.",
      "explanation": "Each write carries the hash the writer last saw; the engine refuses when the stored hash moved, so lost updates surface instead of silently winning."
    }
  }
}
```

```json
{
  "id": "my-graph--optimistic-locking",
  "mem": "my-graph",
  "title": "Optimistic locking",
  "file_path": "optimistic-locking.md",
  "_hash": "2027b9e3bed49f5a",
  "_mem_schema": "default@1.0.0",
  "created_date": "2026-07-02T21:15:35Z",
  "commit_sha": "000000000000000018be958d21f850000000000000000000",
  "durable": true,
  "type_guidance": {},
  "warnings": []
}
```

## Recipe 4 — update under optimistic locking

Every `memstead_update` carries `expected_hash` — the `_hash` from your last read of that entity. If someone else wrote in between, the engine refuses instead of silently overwriting, and hands you the current token in `details`.

**Call 1 — stale hash, refused:**

```json
{
  "name": "memstead_update",
  "arguments": {
    "id": "my-graph--optimistic-locking",
    "expected_hash": "0000000000000000",
    "sections": { "significance": "The engine uses this token on every memstead_update — pass the _hash from your last read as expected_hash." }
  }
}
```

```json
{
  "code": "HASH_MISMATCH",
  "message": "hash mismatch for my-graph--optimistic-locking — entity was modified concurrently (current: 2027b9e3bed49f5a)",
  "details": {
    "id": "my-graph--optimistic-locking",
    "current": "2027b9e3bed49f5a",
    "is_stub": false
  }
}
```

Before retrying with `details.current`, re-read the entity when your edit depended on its content — the concurrent write that moved the hash may have changed what you're editing.

**Call 2 — fresh hash, accepted:**

```json
{
  "name": "memstead_update",
  "arguments": {
    "id": "my-graph--optimistic-locking",
    "expected_hash": "2027b9e3bed49f5a",
    "sections": { "significance": "The engine uses this token on every memstead_update — pass the _hash from your last read as expected_hash." }
  }
}
```

```json
{
  "id": "my-graph--optimistic-locking",
  "title": "Optimistic locking",
  "_hash": "ea7c45d663f67f89",
  "_mem_schema": "default@1.0.0",
  "modified_date": "2026-07-02T21:16:17Z",
  "modified_sections": { "replaced": ["significance"] },
  "modified_metadata": {},
  "commit_sha": "000000000000000018be9596cf886a800000000000000000",
  "durable": true,
  "orphan_stubs_removed": [],
  "warnings": []
}
```

The response's `_hash` is the *new* token — chain it into your next mutation without a re-read.

## Recipe 5 — typed edges: the vocabulary is closed, the refusal is the lookup

In a `strict`-mode schema only declared relationship types are legal. Guess wrong and the refusal enumerates the whole legal vocabulary — no separate lookup call needed.

**Call 1 — refused** (`RELATES_TO` is not in the default schema's vocabulary):

```json
{
  "name": "memstead_relate",
  "arguments": {
    "from": "my-graph--optimistic-locking",
    "type": "RELATES_TO",
    "to": "my-graph--idempotency"
  }
}
```

```json
{
  "code": "INVALID_REL_TYPE",
  "details": {
    "allowed": [
      { "name": "AGREES_WITH", "when_to_use": null },
      { "name": "BLOCKS", "when_to_use": null },
      { "name": "CAUSED", "when_to_use": null },
      { "name": "DEPENDS_ON", "when_to_use": "Logical dependency where removing the target breaks the source. Not hierarchy (use PART_OF)." },
      { "name": "IMPLEMENTS", "when_to_use": "Concrete implementations pointing at the abstract spec they satisfy." },
      "… 32 more …"
    ]
  }
}
```

**Call 2 — corrected** (a declared type that fits the semantics):

```json
{
  "name": "memstead_relate",
  "arguments": {
    "from": "my-graph--retry",
    "type": "DEPENDS_ON",
    "to": "my-graph--idempotency"
  }
}
```

```json
{
  "from": "my-graph--retry",
  "to": "my-graph--idempotency",
  "rel_type": "DEPENDS_ON",
  "source": "explicit",
  "_hash": "f559cb6a71019a85",
  "_mem_schema": "default@1.0.0",
  "commit_sha": "000000000000000018be959d068d4dd80000000000000000",
  "durable": true,
  "orphan_stubs_removed": [],
  "warnings": []
}
```

Two notes on edges: the relate response's `_hash` is the source entity's new lock token (relating rewrote its Relationships section). And `REFERENCES` is `manual_authoring: forbidden` — it's emitted automatically from `[[wiki-links]]` in section bodies; write the link, not the edge.

## Where the reference takes over

Parameter schemas, every error code with its recovery payload, token-budget and chunking behaviour, warnings contracts: [MCP tools](../../reference/mcp/) and the [Error Code Index](../../reference/errors/). For the concepts behind the vocabulary — mem, schema, entity, mount — the [Glossary](../../glossary/) is normative.
