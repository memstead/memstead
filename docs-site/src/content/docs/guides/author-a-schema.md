---
title: Author a schema
description: "Scaffold a custom schema with memstead schema new, validate and install it, and pin a mem to it — no hand-copied YAML."
sidebar:
  order: 3
---

A [schema](../../glossary/#schema) is what makes a mem *typed*: it declares the entity types, the sections each type must carry, the metadata fields, and the relationship vocabulary — and the engine enforces all of it on every write. The built-in `default` schema is a general-purpose starting point; the moment your domain has its own vocabulary, author your own — or fork a closer built-in. For document-and-deadline domains (contracts, compliance, permits, grants, maintenance), start from `obligation` — the first non-software built-in: dated duties with stated consequences, the agreements they arise from, the parties they bind, and the decisions taken over them, with the `due:` axis wired so `memstead due` answers "what is due next". Nothing in it names a domain; it is deliberately a fork target (`memstead schema install obligation@0.1.0`, copy, rename, extend).

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
- **`cookbook/types/note.yaml`** — one commented example type: a required `summary` section, an optional catch-all `details` section, one required and one optional metadata field, search weights, and `write_rules` guidance served to agents.

Every line carries a comment explaining what to change. The scaffold validates clean *unmodified* — you can run the printed follow-up first and shape the schema afterwards.

One rule covers sections and metadata fields alike: **a declaration is optional unless it says `required: true`** — absence means optional. A required metadata field with a `default_value` is auto-filled and never refused (required-with-default means "always present", not "caller must type it"). The retired `optional:` key refuses at authoring load with the one-line inversion fix; sealed schemas that still carry it keep loading with equivalent semantics.

## 2. Validate

```bash
memstead schema validate cookbook
```

```text
# Schema valid

`cookbook@0.1.0` — 1 type(s) at `cookbook`
```

This is the same validation the engine runs at load, without touching the workspace. Any conformance error exits non-zero (`SCHEMA_VALIDATION_FAILED`) with the YAML line and column where the parse layer provides it — re-run after every edit.

### Retired keys: migrate instead of hand-editing

A package written under an older schema language can fail validation on nothing but retired spellings — `propagating_relationships` (now `no_self_loop_relationships`), the metadata-field `optional:` (now `required:`, opposite polarity), the `examples:` list (replaced by the validated `exemplar:`), the exemplar-relation `to:`/`type:` pair (now `target:`/`rel_type:`). `memstead schema migrate` rewrites them by exactly the translations the engine applies when it reads sealed content:

```bash
memstead schema migrate cookbook            # dry run: one line per rewrite, nothing written
memstead schema migrate cookbook --write    # apply in place; comments and key order stay
memstead schema validate cookbook
```

The dry run is the review step. One thing to read carefully there: a package that carries `optional:` was written when an absent key meant *required*, and its sealed copies still read that way. The migration conserves that meaning by inserting `required: true` on every metadata field that declared neither key — delete the line where you did not mean it. The verb never bumps `version` (whether a spelling change deserves a new one is your call) and never edits a sealed copy inside a mem; those keep loading as they are.

## 3. Install into the workspace

```bash
memstead schema install cookbook
```

```text
# Schema installed

`cookbook@0.1.0` → `<workspace>/.memstead/schemas/cookbook@0.1.0` (2 file(s))
```

Installing copies the validated package into the workspace's schema store so mems can pin it. The command validates before copying and is idempotent.

### Repair: mis-stamped legacy seals (engines 0.6.0 – 0.8.1)

In that engine window, `schema install` stamped every unmarked package as
current-language at seal time. A **legacy** package (one authored before the
metadata-polarity flip, with bare `optional:`-style fields) installed by an
affected engine therefore carries a wrong generation marker: its bare
metadata fields silently read as *optional* instead of *required*.

**Detect** — the marker's presence inside a legacy package's sealed copy IS
the mis-stamp. On a git-branch workspace:

```bash
git --git-dir mem-repo/.git cat-file -e '__MEMSTEAD:schemas/<legacy-pkg>/schema-format.json' \
  && echo "mis-stamped"
```

Current-language packages legitimately carry the marker — only a package you
know to be legacy-authored is implicated.

**Repair** — re-run `memstead schema install` for the affected schema with a
fixed engine (0.8.2+). The seal then carries the source's generation
as-found, and the package's fields read with their written meaning again.

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

A metadata field can declare the shape of its values with `value_pattern`, a regular expression the engine anchors at both ends and checks on every write; on a `csv_array` field each member is checked on its own, so a list of shaped entries (`name=constant` pairs, ticket keys, quarter labels) refuses the one malformed member by name (`INVALID_FIELD_VALUE`, the pattern quoted as the expected format). A pattern that does not compile refuses at install. The schema render and the MCP skeleton show it as `pattern`.

For full worked schemas to read (not copy — the scaffold already gave you a valid base), see the [examples](https://github.com/memstead/memstead/tree/main/examples): `agent-program` (a single-mem, execution-flavoured schema) and the `reimpl-source`/`reimpl-target` pair (a two-mem model with cross-mem links).

## 7. Declare keep-health constraints

Beyond what is *legal to write*, a type can declare what is *unhealthy to keep* — a `constraints:` list on the type definition, drawn from a closed vocabulary of five forms under one uniform `severity` model: `warn` (a health finding on the health report) or `block` (a write-time refusal, plus a health finding for pre-existing violations). The forms:

- **`requires_when`** — a field or section becomes required when another metadata field holds a declared value (`status: checked` requires `checked_by`). Defaults to `warn`.
- **`required_outgoing` severity and condition** — each required-edge block on the type can carry `severity: block`, promoting its historical `MISSING_REQUIRED_OUTGOING` warning to a refusal, and an optional `when_field` / `when_value` pair (the same two keys `requires_when` uses): the block then applies only while that metadata field holds that enum value. The trigger field must be declared with `enum_values`; every payload naming the unsatisfied block echoes the trigger.
- **`unique`** — a tuple of metadata fields unique among entities of this type within the mem. Defaults to `block`: the point is bouncing the duplicate at write time.
- **`enum_from_neighbour`** — a field whose legal values are the bullet-list entries of a named section on the entity reached via a named outgoing edge; a value nothing backs is a violation. Defaults to `warn`.
- **`status_propagation`** — a terminal value of a named status field taints every entity reaching it via a named rel-type and direction (or a relation set: `rel_types: [A, B]` walks the union, so a taint crosses rel-type boundaries; declare exactly one of `rel_type` / `rel_types`); tainted entities surface as health findings naming their tainting ancestor. Always warn-tier (a parent falling *after* the child was written cannot retroactively make that write illegal). It supersedes the retired `propagating_relationships` key, whose sole self-loop-refusal behaviour now lives under the honestly named `no_self_loop_relationships` (optional; the old key refuses at authoring load with a typed rename error).
- **`transition_requires_checks`** — a write landing `field` at `to_value` requires every entity related via the named `relationships` in `direction` (`incoming` / `outgoing`) to carry a fresh confirming check record (derived verification state `checked_ok` — stale and failed do not confirm). The graded-completion gate as data: "a plan may become `complete` only when every criterion pointing VERIFIES at it has been checked". Defaults to `block`; the refusal (and the health `constraints` finding, when a check goes stale after the transition) lists each unconfirmed entity with the state it derived. An empty related set satisfies — pair with `required_outgoing` where at least one related entity must exist. Verdicts ride the check ledger (`memstead check`), never an entity field, so the gate cannot be satisfied by editing the entity.

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

## 8. Reasoning forms: reachability, acyclicity sets, signals, labelling

Four further declarations turn chain structure into something the engine checks or reports. All are optional, additive, and served on the `memstead_schema` response at both verbosity levels; schema packages declaring any of them need engine 0.10.0 or later (older engines refuse the unknown keys at parse).

**`must_reach`** (on a type) — entities of the type must reach at least one entity of a named terminal-type set, following an inline relation set in a named direction (`out` / `in`), within an optional `max_depth`. Health-sweep only, always warn-tier (the loader refuses `block`: a reachability gap is created by writes on *other* entities). The incoming direction with `max_depth: 1` covers "must have at least one incoming edge of these types".

```yaml
# types/claim.yaml
must_reach:
  - relationships: [GROUNDS, CONCLUDES]
    direction: out
    terminal_types: [evidence]
    max_depth: 12          # optional; absent = unbounded
```

**`acyclic_sets`** (on the manifest's `relationships:` block) — acyclicity over the *union* subgraph of a set of rel-types, for cycles no single rel-type contains (a support chain alternating two rel-types, two hierarchies that must jointly stay acyclic). A write closing a cycle in the union refuses with `RELATIONSHIP_CYCLE`; the payload echoes the set and one rel-type per hop of the existing path. Each set names two or more declared rel-types; a rel-type may appear in at most one set; the per-relationship `acyclic: true` flag keeps its exact meaning and may coexist.

```yaml
# schema.yaml
relationships:
  mode: strict
  acyclic_sets:
    - [GROUNDS, CONCLUDES]
```

**`signals`** (on a type) — exact, parameter-free counts with declared thresholds, served with their evidence on every read of the type. One kind in this generation, `edge_load`: count edges of an inline relation set in a named direction, optionally restricted to edges whose counterpart holds a named enum value (`neighbour_field` / `neighbour_value`). Thresholds map counts to `notice` / `warn` (its own two-member level enum, not the constraint severity; below the first threshold the level is `none`). Served as `_signals` on the entity envelope and the `signals` health axis (`warn` participates in `health --strict`); a write moving a signal across a threshold carries the out-of-band `SIGNAL_THRESHOLD_CROSSED` warning, never an error. Values are computed at read time — never stored, never part of `_hash` — and nothing multiplies, averages, or decays. A raw count is gameable by one author repeating one objection; bind your schema's prose to the numbers accordingly.

```yaml
# types/claim.yaml
signals:
  - name: attack_load
    kind: edge_load
    relationships: [REBUTS, UNDERCUTS]
    direction: in
    thresholds:
      - at_least: 1
        level: notice
      - at_least: 3
        level: warn
```

**`labelling`** (on the manifest's `relationships:` block) — name which rel-types constitute *attack*, and the engine serves the grounded labelling of that attack graph: unattacked entities `accepted`, targets of an accepted attacker `defeated`, entities whose attackers are all defeated `accepted`, the rest `undecided` (cycles stay open). The one argumentation semantics that is parameter-free, unique, and polynomial — and the most sceptical: one unanswered attack defeats a well-supported claim, so a defeated label always names its accepted direct attackers and an undecided one the open attacker set. Served as `_labelling` on entity reads and the `labelling` health axis; computed per mem (cross-mem attack edges are excluded and counted); a label is a reported observation, never a stored value, never a write gate. The labelling is deliberately **support-blind**: a defeated supporter never flips what it supports — the optional `support` walk adds chain-shape statistics (`depth`, `branching`, `terminal_share`, `defeated_in_support`, `undecided_in_support`) so the reader sees that defeat as a number and judges.

```yaml
# schema.yaml
relationships:
  mode: strict
  labelling:
    attack: [REBUTS, UNDERCUTS]
    support:               # optional; enables the shape statistics
      relationships: [GROUNDS]
      direction: out
      terminal_types: [evidence]
```

## 8b. Declare what is due

A type whose entities carry a deadline can declare a **due axis**, so the
engine can say what is due next without knowing your vocabulary:

```yaml
due:
  date_field: target_date      # a date-typed metadata field of this type
  status_field: status         # an enum-typed metadata field
  open_values: [planned, active]
  lead_section: blockers       # optional: quoted beside the entry
```

`memstead due` renders the brief across every mounted mem whose schema
declares the axis: open entities whose date has gone by under **overdue**
with the days past, open entities due inside the window (default 90 days,
`--within 30d`) under **due_soon** with the days until, each quoting the
lead section when the entity carries it. `--json` carries both lists as
data beside the prose. The reading rides the declaration and nothing
else: the engine names the state and never judges it (no severity, no
recommendation), the stale axis keeps measuring edit recency, and a type
without `due:` contributes nothing. A closed entity (its status outside
`open_values`) never appears. The loader validates the declaration at
install: the fields must exist with those shapes and every open value
must be in the status enum.

## 8c. Declare what would resolve an open entity

A type whose entities can be open (a question, a criterion, a risk) can
declare a **resolution condition**: the section where an open entity says
what would resolve it, and the check kind under which that condition
counts as checked. In the mould of the due axis:

```yaml
resolution:
  status_field: status               # optional: an enum-typed metadata field
  open_values: [open, investigating] # its values under which the entity is open
  condition_section: answer_approach # a declared section of the type
  check_kind: verification           # optional: verification (default),
                                     # conformance, or an x-<name> kind
```

With it `memstead health --include open_questions` answers two questions
per mem without reading prose: the open entities whose condition section
is empty (`resolution_missing`) and the open entities whose condition
nobody has checked under the declared kind (`resolution_unchecked`, read
from the check ledger at the entity's current content; a caller-declared
`x-` kind counts by name). A type without `status_field` is open in every
entity, which suits a criterion whose assertion is its own condition. The
declaration is a reading: the write path refuses nothing new, the
section's `required` flag stays your choice, and the engine never judges
whether a condition is well-formed. The loader validates the shape at
install (the section must exist, the field must be an enum carrying every
open value, the kind must be well-formed); an engine older than the key
refuses the package at parse, naming `resolution`, as every new key.

## 9. Declare a section's markdown shape

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

## 10. Teach by example — one validated exemplar per type

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
    - target: canned-san-marzano-tomatoes
      rel_type: CONTAINS
```

The pieces:

- **`title`** drives the id slug exactly as a real create would.
- **`metadata`** is validated like a real create's overrides — enums included; required-no-default fields must be present. Engine-stamped fields (`created_date`, …) are omitted.
- **`sections`** must cover every required section, and each body must satisfy the section's declared `content` format.
- **`relations`** speak the mutation vocabulary (`target:` / `rel_type:` — the same keys `memstead_create` takes, so what an agent copies from the served exemplar is exactly what the write gate accepts; the retired `to:` / `type:` spelling refuses at authoring load with a rename pointer). Targets are **bare placeholder slugs** (no `mem--` prefix — an exemplar lives outside any mem). Rel-type legality and edge shape are validated; target existence never is.

Exemplars are optional per type; the built-in reference schemas carry one on every type. Agents see them via `memstead_schema` at `verbosity: full` and in `memstead type <name>` — the lite skeleton every session fetches stays unchanged. The retired `examples:` list (never validated, never served) refuses at authoring load with a pointer here.

## Where next

- The [Glossary](../../glossary/#schema) defines schema, schema pin, and migration precisely.
- [Publish a mem](../../guides/publish-a-mem/) — a published `.mem` archive carries its schema with it.
