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

## 7. Declare keep-health constraints

Beyond what is *legal to write*, a type can declare what is *unhealthy to keep* — a `constraints:` list on the type definition, drawn from a closed vocabulary of five forms under one uniform `severity` model: `warn` (a health finding on the health report) or `block` (a write-time refusal, plus a health finding for pre-existing violations). The forms:

- **`requires_when`** — a field or section becomes required when another metadata field holds a declared value (`status: checked` requires `checked_by`). Defaults to `warn`.
- **`required_outgoing` severity** — each required-edge block on the type can carry `severity: block`, promoting its historical `MISSING_REQUIRED_OUTGOING` warning to a refusal.
- **`unique`** — a tuple of metadata fields unique among entities of this type within the mem. Defaults to `block`: the point is bouncing the duplicate at write time.
- **`enum_from_neighbour`** — a field whose legal values are the bullet-list entries of a named section on the entity reached via a named outgoing edge; a value nothing backs is a violation. Defaults to `warn`.
- **`status_propagation`** — a terminal value of a named status field taints every entity reaching it via a named rel-type and direction; tainted entities surface as health findings naming their tainting ancestor. Always warn-tier (a parent falling *after* the child was written cannot retroactively make that write illegal). It supersedes the retired `propagating_relationships` key, whose sole self-loop-refusal behaviour now lives under the honestly named `no_self_loop_relationships` (optional; the old key refuses at authoring load with a typed rename error).

```yaml
# types/recipe.yaml
constraints:
  - kind: requires_when          # status: retired requires a replaced_by value
    field: replaced_by
    when_field: status
    when_value: retired
  - kind: unique                 # one recipe per (name, cuisine)
    fields: [name, cuisine]      # severity defaults to block for unique
```

A malformed declaration — an unknown `kind`, an undeclared field or relationship — refuses at `schema validate` / `schema install` with a typed error; nothing loads and gets silently ignored. Declared constraints render on the `memstead_schema` MCP response at both verbosity levels. The exact shape of every form is in the generated IDE-validation reference [`crates/memstead-schema/generated/type-definition.schema.json`](https://github.com/memstead/memstead/tree/main/crates/memstead-schema/generated).

## 8. Declare a section's markdown shape

A section declaration can also pin the markdown structure of its body — a flat `content` expression over the mdast block vocabulary (`paragraph`, `list`, `table`, `code`, `blockquote`, `heading`, `thematicBreak`, `html`), with attribute forms (`list(bullet)`, `list(ordered)`, `heading(3)`–`heading(6)`, `code(lang=json)`) and regular operators: sequence by space, alternation `(paragraph | list)`, repetition `+` `*` `?`. Optional companions:

- **`item_pattern`** — an implicitly anchored regex applied to each repeating unit (list items, paragraph lines); named capture groups name the parts in refusal payloads. Legal only when the expression contains exactly one of `list` / `paragraph`.
- **`table`** — the table contract: `columns` pins header names and order; `column_patterns` maps column name → per-cell regex.
- **`example`** — one conforming snippet, echoed verbatim in every format refusal.
- **`format_severity`** — reuses the constraint severity model; defaults to `block` (a shape violation is deterministic and one-round-trip repairable), `warn` stays available per section.

```yaml
# types/recipe.yaml
sections:
  - key: ingredients
    heading: Ingredients
    required: true
    search_weight: 1.0
    content: "list(bullet)"
    item_pattern: '(?<amount>[\d/.]+\s?\w*) (?<ingredient>.+)'
    example: |
      - 500g flour
      - 300ml water
```

A section with no `content` stays free-form, exactly as before. Violations refuse (or warn) with `SECTION_CONTENT_MISMATCH`, `SECTION_ITEM_PATTERN_MISMATCH`, or `INVALID_TABLE_COLUMNS` — each carrying the declared expression, the found block sequence, and the `example` — indexed in the [Error Code Index](../../reference/errors/). The full key shapes live in the same generated reference as the constraints.

## 9. Teach by example — one validated exemplar per type

LLMs learn from examples far better than from rules. A type may carry one canonical `exemplar:` — a complete entity in the mem markdown shape — and the engine **validates it through the real create path** at `memstead schema validate` and at install: a package whose exemplar does not conform refuses with a typed error naming the type and the defect. There is no warn-and-carry mode, so an exemplar can never drift into teaching a shape the validator would refuse — the failure mode that kills every hand-maintained example in ordinary documentation.

```yaml
# types/recipe.yaml
exemplar:
  title: "Classic Bolognese"
  metadata:
    cuisine: italian
  sections:
    summary: "Slow-simmered Italian meat sauce over fresh pasta."
    ingredients: |
      - 500g flour
      - 300ml water
  relations:
    - to: canned-san-marzano-tomatoes
      type: CONTAINS
```

The pieces:

- **`title`** drives the id slug exactly as a real create would.
- **`metadata`** is validated like a real create's overrides — enums included; required-no-default fields must be present. Engine-stamped fields (`created_date`, …) are omitted.
- **`sections`** must cover every required section, and each body must satisfy the section's declared `content` format.
- **`relations`** use **bare placeholder slugs** as targets (no `mem--` prefix — an exemplar lives outside any mem). Rel-type legality and edge shape are validated; target existence never is.

Exemplars are optional per type; the built-in reference schemas carry one on every type. Agents see them via `memstead_schema` at `verbosity: full` and in `memstead type <name>` — the lite skeleton every session fetches stays unchanged. The retired `examples:` list (never validated, never served) refuses at authoring load with a pointer here.

## Where next

- The [Glossary](../../glossary/#schema) defines schema, schema pin, and migration precisely.
- [Publish a mem](../../guides/publish-a-mem/) — a published `.mem` archive carries its schema with it.
