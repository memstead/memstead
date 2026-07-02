---
title: Author a schema
description: "Scaffold a custom schema with memstead schema new, validate and install it, and pin a mem to it — no hand-copied YAML."
sidebar:
  order: 2
---

A [schema](../../glossary/#schema) is what makes a mem *typed*: it declares the entity types, the sections each type must carry, the metadata fields, and the relationship vocabulary — and the engine enforces all of it on every write. The built-in `default` schema is a general-purpose starting point; the moment your domain has its own vocabulary, author your own.

This guide takes you from a working workspace (see [Getting started](../../guides/getting-started/)) to a mem pinned to a custom schema. You never hand-copy YAML — `memstead schema new` scaffolds a valid package, and the whole remaining flow is the commands it prints.

## 1. Scaffold the package

From inside your workspace (this walkthrough uses a workspace/mem named `recipes`):

```bash
memstead schema new cookbook
```

```text
# Schema package scaffolded

`cookbook@0.1.0` at `cookbook` (schema.yaml + types/note.yaml, one commented example type).

Edit the package, then:

1. `memstead schema validate cookbook`
2. `memstead schema install cookbook`
3. `memstead delete recipes--welcome-to-memstead` — the quickstart seed — the pin below switches atomically only when every entity conforms to the new schema
4. `memstead mem set-schema recipes cookbook@0.1.0`
```

The scaffold is a complete schema package — one folder, two files:

- **`cookbook/schema.yaml`** — the manifest: name, version, description, the `types:` roster, the relationship vocabulary (`strict` mode with `PART_OF`, `RELATES_TO`, `REFERENCES`, and the required `_default` fallback), and `alias_target_rel_type: REFERENCES` so body wiki-links auto-emit edges.
- **`cookbook/types/note.yaml`** — one commented example type: a required `summary` section, an optional catch-all `details` section, a filterable `status` metadata field, search weights, and `write_rules` guidance served to agents.

Every line carries a comment explaining what to change. The scaffold validates clean *unmodified* — you can run the printed follow-up first and shape the schema afterwards.

## 2. Validate

```bash
memstead schema validate cookbook
```

```text
# Schema valid

`cookbook@0.1.0` — 1 type(s) at `cookbook`
```

This is the same validation the engine runs at load, without touching the workspace. Any conformance error exits non-zero (`SCHEMA_VALIDATION_FAILED`) with the YAML line and column where the parse layer provides it — re-run after every edit.

## 3. Install into the workspace

```bash
memstead schema install cookbook
```

```text
# Schema installed

`cookbook@0.1.0` → `<workspace>/.memstead/schemas/cookbook@0.1.0` (2 file(s))
```

Installing copies the validated package into the workspace's schema store so mems can pin it. The command validates before copying and is idempotent.

## 4. Pin the mem

A mem switches schema atomically only when every entity it holds conforms to the target. A quickstart workspace carries one seed entity of the default schema's `concept` type, which the scaffold doesn't declare — remove it first (the scaffold's printed follow-up includes this step):

```bash
memstead delete recipes--welcome-to-memstead
memstead mem set-schema recipes cookbook@0.1.0
```

```text
# Mem `recipes` schema: Switched

- Pin: cookbook@0.1.0
- Migration target: <none>
```

If non-conforming entities remain instead, the mem enters *dual-pin migration*: writes validate against the target schema and the response lists the entities to repair; re-issue `set-schema` after repairing to complete the switch.

## 5. Write against your schema

The mem now accepts your types — and refuses what your schema refuses:

```bash
memstead create --type note --title "Sourdough starter" \
  --section summary="Flour and water, fed daily; ready when it doubles in four hours."
```

```text
# Created `recipes--sourdough-starter`

- Title: Sourdough starter
- Mem: recipes
- File: sourdough-starter.md
- Hash: `d749c7f127a6ef2e`
```

Leave out the required section and the refusal quotes your own `write_rules` back:

```text
memstead: ERROR [MISSING_REQUIRED_SECTION]: missing 1 required section(s) for type 'note':
  - 'summary' (Summary) — write_rules: One or two sentences. Must stand alone in a search result.
```

## 6. Shape it into your domain

Now iterate on the package: rename `note` into your first real type (the filename stem, its `name:` field, and the manifest's `types:` entry must agree), add types one file at a time, and grow the relationship vocabulary. After each edit: `schema validate`, `schema install`, and bump `version:` when a published mem depends on it. The scaffold's comments cover each knob — sections, metadata fields, search weights, hierarchy and propagation, staleness.

For full worked schemas to read (not copy — the scaffold already gave you a valid base), see the [examples](https://github.com/memstead/memstead/tree/main/examples): `agent-program` (a single-mem, execution-flavoured schema) and the `reimpl-source`/`reimpl-target` pair (a two-mem model with cross-mem links).

## Where next

- The [Glossary](../../glossary/#schema) defines schema, schema pin, and migration precisely.
- [Publish a mem](../../guides/publish-a-mem/) — a published `.mem` archive carries its schema with it.
