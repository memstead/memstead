# `reimpl-target@0.1.0` — Target design schema

Paired with [`reimpl-source@0.1.0`](../../reimpl-source/schema/). This schema lives
in a **Target vault** — the design and build surface for a new
implementation of the system captured in the Legacy vault. A single
Legacy vault can feed multiple Target vaults (different apps, different
stacks, different divergences); each Target carries its own divergences.

## Purpose

The Target vault is where the new system is designed and grown. Every
target-spec links back to the capabilities from the Legacy vault it
realizes. Every deliberate deviation from legacy behavior is recorded as
a first-class `divergence` entity — not buried in prose.

After the reimplementation completes, a Target vault can swap its schema
to `software@0.1.0`; target-specs become regular specs, divergences are
archived as decisions, and the vault continues as the living spec
inventory of the new app.

## Types

| Type | Purpose |
|---|---|
| `target-spec` | Design of a new component that realizes one or more legacy capabilities. Grows from intent → design → built → validated as the implementation progresses. |
| `divergence` | An authorized deviation from legacy behavior — drop, simplify, antipattern-out, tech-swap, or add. Carries rationale and links both to the affected capability (cross-vault to Legacy) and to the target-spec where it manifests. |

## How to use

```
memstead schema install examples/schemas/reimpl-target
memstead vault init <target-vault> --schema reimpl-target@0.1.0
```

In the Target vault's `.memstead/config.json`:

```json
{
  "name": "<project>-target-<variant>",
  "schema": "reimpl-target@0.1.0"
}
```

## Cross-vault edges

The Target schema declares four edges that cross into the paired Legacy
vault:

| Edge | From → To | Purpose |
|---|---|---|
| `REALIZES_CAPABILITY` | target-spec → capability (Legacy) | A new design realizes a legacy capability. Drives the coverage query ("which capabilities are still unrealized?"). |
| `DIVERGES_FROM` | divergence → capability (Legacy) | A divergence attaches to the capability it modifies or drops. |
| `MANIFESTS_IN` | divergence → target-spec (intra) | Divergences that are kept (not dropped) land in a target-spec. |
| `EVOLVES_INTO` | target-spec → target-spec (intra) | Planned iteration: v1 evolves into v2. Optional — useful when the new design is phased. |

Cross-vault typed edges require the paired Legacy vault to be loaded in
the same workspace. The engine validates both endpoints on every write.

## Relationships

Inherits the default schema's 37 edges plus the four above.

## Evolving the schema

Target-design practice is new. Expect `0.1.0` to evolve based on the
first real reimplementation project. Ship new versions alongside old
ones rather than editing in place.
