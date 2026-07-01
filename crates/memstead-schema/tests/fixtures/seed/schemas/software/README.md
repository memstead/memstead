# `software` — schema for software-project vaults

Workspace-level schema for every vault that pins `"schema": "software"` in its `.memstead/config.json`. Nine types organized around four questions a software-project graph must answer:

| Pillar | Types |
|---|---|
| **WHAT** exists | `spec`, `contract` |
| **WHY** it is so | `decision`, `principle`, `requirement` |
| **WHO** is accountable | `actor` |
| **WHAT** has broken | `incident` |
| Support | `concept`, `memo` |

The graph's value comes from type diversity. A graph dominated by any one type — especially `spec` — has lost the structure the schema offers.

## Location

This schema lives at the workspace level and is shared by every vault that pins it. The workspace's `.memstead/workspace.toml` declares `schemas_dir = "schemas"`, and every `schemas/<name>/` directory is discovered at load time. No per-vault copying is required — a vault's `.memstead/config.json` just references the schema by name.

Local usage is **unversioned**: a vault simply writes `"schema": "software"` and the engine resolves against this directory. The `version:` field in `schema.yaml` is metadata preserved for publish/archive workflows; it is not a pin.

## Types

### WHAT — current-state surfaces

| Type | Purpose | Key test |
|---|---|---|
| `spec` | Internal component, module, or subsystem. The catch-all — never the default pick. | Rule out contract, decision, requirement, principle, concept first. |
| `contract` | External wire-level surface (HTTP, gRPC, CLI, library API, file format). | Is there a concrete request/response shape? |

### WHY — reasoning and commitments

| Type | Purpose | Key test |
|---|---|---|
| `decision` | One design choice with rejected alternatives and durable consequences. Lifecycle: proposed → accepted → superseded / retired. | Can you name what was rejected and why? |
| `principle` | Project-wide rule that holds across many entities. | Would a violation be visible in multiple places? |
| `requirement` | Normative MUST / SHOULD / MAY tied to one capability, testable, from a written source. | Could a test verify compliance? |

### WHO — accountability

| Type | Purpose |
|---|---|
| `actor` | Team, person, or service account. Subject of every OWNS and MAINTAINS edge. |

### WHAT BROKE — operations

| Type | Purpose |
|---|---|
| `incident` | Production failure with timeline and post-mortem. Links VIOLATES to broken commitments. |

### Support

| Type | Purpose |
|---|---|
| `concept` | Precise definition of a term used across 2+ entities. |
| `memo` | Lightweight reasoning without rejected alternatives. Migrates to `decision` if alternatives accumulate. |

## Relationship vocabulary

Strict mode. The 37 edges of the built-in `default` schema plus five software-specific additions:

| Edge | From → To | Purpose |
|---|---|---|
| `REALIZES` | spec / contract → requirement | Concrete artifact fulfils an abstract requirement. |
| `OWNS` | actor → any | Actor has final authority over the target. |
| `MAINTAINS` | actor → any | Actor maintains the target without sole ownership. |
| `VIOLATES` | incident → requirement / spec / contract | Incident broke the target commitment. |
| `DEPRECATES` | contract / spec → contract / spec | Source supersedes target at a protocol level; consumers should migrate. |

Common default-schema edges in this schema: `PART_OF`, `DEPENDS_ON`, `IMPLEMENTS`, `USES`, `MOTIVATED_BY`, `SUPERSEDES`, `DERIVED_FROM`, `GOVERNS`, `CONSTRAINS`, `CAUSED`, `GENERALIZES`, `SPECIALIZES`, `CONTRASTS_WITH`.

## Evolving the schema

Because local use is unversioned, shape changes are in-place edits with a working-tree review. Vaults that pin the schema see changes immediately on the next engine reload — plan the rollout carefully and prefer additive changes. When a breaking shape change is needed, coordinate across every vault that pins the schema.

For publishable or portable schema variants, author a copy under `recipes/software/schema/` where the `version` field matters and the pin becomes `software@<version>`.

## Reference

- Paired recipe config (write guidance + projection template): [recipes/software/config.json](../../../recipes/software/config.json)
- Authoring guide: [dev/authoring-schemas.md](../../../dev/authoring-schemas.md)
- Built-in default schema this one draws from: [engine/crates/memstead-schema/builtins/schemas/default/](../../../engine/crates/memstead-schema/builtins/schemas/default/)
