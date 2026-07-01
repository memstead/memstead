# `project@0.1.0` — knowledge-graph schema for a running project

A copy-paste-ready memstead schema for **one running project** modeled
as a queryable whole. Ten types in five clusters capture identity,
holdings, motion, surroundings, and rules:

| Cluster | Type | Purpose |
|---|---|---|
| Identity | `vision` | Durable north star, one to three per vault |
| Identity | `positioning` | Versioned pitch + audience, one current per audience |
| Identity | `brand` | Name + identity layer per surface |
| Holdings | `pillar` | Top-level subsystem, bridges to a code vault |
| Holdings | `evidence` | Empirical anchor for a strategic claim |
| Motion | `bet` | Strategic wager carrying risk |
| Motion | `milestone` | Project-level checkpoint |
| Surroundings | `competitor` | Outside party doing something close |
| Surroundings | `market_signal` | External trend with a source |
| Rules | `principle` | Project-level rule that constrains design or strategy |

The graph carries durable project posture — not phase-scoped plans
(use `planning@0.1.0`) and not code state (use `software@0.1.0`).
Pair this vault with one or more code-side vaults that the project
vault references via cross-vault links; the code vaults stay
autonomous and unaware of the project layer.

This schema ships **built into the engine** — every install resolves
`project@0.1.0` with no copy step. Pin it directly when you create a
project vault (below); fork it only when you want to customize the
vocabulary.

## How to use

### One project, one project vault

A project vault carries the strategic, operational, and competitive
view of one project. The convention that pairs with this schema:

```
<workspace>/<project>/                  ← the project vault (pinned project@0.1.0)
<workspace>/<code-vault-1>/             ← e.g. engine
<workspace>/<code-vault-2>/             ← e.g. app
…
```

1. Create the project vault pinned to the built-in schema, filling the
   instance write-guidance keys the package's `vault-template.json`
   lists (here, `scope`):

   ```
   memstead vault init <workspace>/<project> --schema project@0.1.0 \
       --write-guidance '{"scope": "<one paragraph: what this project is and which code vaults it references>"}'
   ```

   The agent should refuse to write blind if `scope` is left as a
   placeholder at run time. To customize the vocabulary itself, fork the
   schema into local storage first with `memstead schema install project`.

2. Configure cross-vault links so the project vault may reference
   the code-side vaults:

   ```
   memstead workspace grant-cross-link project engine
   memstead workspace grant-cross-link project app
   memstead workspace grant-cross-link project plugin
   ```

   The reverse direction (code vaults referencing the project vault)
   is intentionally not granted — the project layer observes; it
   does not become an authority the code vaults reason against.
   Hand-editing the `[cross_vault_links]` block in
   `.memstead/workspace.toml` is the advanced fallback for batch edits;
   the CLI is the primary surface and the only path that triggers the
   live-engine reload pairing on `memstead_reload`.

5. Call `memstead_reload` (or restart the MCP server) so the registry
   picks up the new schema. `memstead_reload` without a `vault` parameter
   also re-reads `.memstead/workspace.toml`, so the cross-link grants
   from step 4 become visible without restart.

6. Author. Start with the high-degree entities (active `vision`,
   current `positioning`, top-tier `bet`s, current `pillar`s);
   `evidence`, `competitor`, `market_signal`, `principle`, and
   `milestone` accumulate around them.

### Lifecycle

```
  vision (1-3, durable, superseded not edited)
     ↑ MOTIVATED_BY
   bet (active wager, falsifiable)
     ↑ STRENGTHENS / WEAKENS
   evidence (snapshot observation, sourced)
     ↑ VALIDATES / CONTRADICTS
   market_signal (external trend, time-bounded)
     |
     | THREATENS
     ↓
   pillar (subsystem) ←-- REFERENCES --→ <code-vault>
     ↑ MOTIVATED_BY              (auto-emitted from
   milestone (committed              wiki-links)
     checkpoint)
     ↑ GOVERNS / CONSTRAINS
   principle (project rule)
     ↑ SUPERSEDES (across rebrands /
   brand        repositionings)
   positioning
```

The project vault is **observer**: it references code-side vaults
but they do not reference back. When a `vision`, `positioning`, or
`brand` evolves, supersede the old entity rather than editing it —
the lineage of how the project's identity and posture evolved is
high-value.

## Types

### Identity — what we are

| Type | Purpose | Key test |
|---|---|---|
| `vision` | Long-arc destination | One to three per vault; superseded, not edited |
| `positioning` | Audience-facing pitch | At most one `current` per audience |
| `brand` | Name + identity per surface | At most one `active` per surface |

### Holdings — what we have

| Type | Purpose | Key test |
|---|---|---|
| `pillar` | Top-level subsystem | Must REFERENCES a code-vault entity |
| `evidence` | Empirical anchor | Must STRENGTHENS / WEAKENS / VALIDATES / CONTRADICTS something |

### Motion — what we do

| Type | Purpose | Key test |
|---|---|---|
| `bet` | Strategic wager carrying risk | Must MOTIVATED_BY a vision; must have falsification criteria |
| `milestone` | Committed checkpoint | Definition of Done is checkable by an external observer |

### Surroundings — what's around us

| Type | Purpose | Key test |
|---|---|---|
| `competitor` | Tracked external party | `last_checked` recent, has Our Distinction and Would Eat Our Lunch If |
| `market_signal` | Time-bounded external trend | Has a source and a window |

### Rules — what we hold to

| Type | Purpose | Key test |
|---|---|---|
| `principle` | Project-level rule | Must GOVERNS or CONSTRAINS at least one bet/milestone/positioning to be active |

## Relationship vocabulary

Strict mode. A curated subset of the default edges plus three
project-specific:

| Edge | From → To | Purpose |
|---|---|---|
| `STRENGTHENS` | evidence → bet/positioning | The wager is being validated |
| `WEAKENS` | evidence → bet, market_signal → bet | The wager is being broken |
| `THREATENS` | competitor → pillar, market_signal → bet/vision | External pressure on a project element |

Reused default edges in active use: `PART_OF` (hierarchy),
`REFERENCES` (auto-emitted from wiki-links), `SUPERSEDES` (versioned
identity), `MOTIVATED_BY` / `MOTIVATES` (why-chain), `BLOCKS`
(forward dependency), `INFORMED_BY` (soft input), `VALIDATES` /
`CONTRADICTS` (strong evidence claims), `GOVERNS` / `CONSTRAINS`
(principles limiting choices), `CONTRASTS_WITH` (disambiguating
neighbors).

The schema explicitly excludes code-specific edges (`REALIZES`,
`OWNS`, `MAINTAINS`, `VIOLATES`, `DEPRECATES`) — those belong in the
code-side schema this project vault pairs with.

## No `risk` type

By design. A risk that does not change posture is noise; a risk
that changes posture is a `bet` whose `status` moved from `winning`
to `losing`, with `evidence` WEAKENS-edges supporting the move. A
separate `risk` type would duplicate the bet-failure-mode dimension
already carried by `bet`.

## Evolving the schema

Bump `version` in `schema.yaml` on any shape change. Project vaults
pin exact versions (`project@0.1.0`), so an active project vault
keeps working against the pinned version until explicitly updated.
Ship new versions alongside old ones rather than editing in place.

`project@0.1.0` is the launch version — unstable, no semver
discipline yet. The first published version with public consumers
is the right time to harden v1.

## Reference

- Per-vault scaffolding (instance write guidance): [`vault-template.json`](./vault-template.json)
- Companion code-side schema: [../../software/](../../software/)
- Companion phase-scoped schema: [../../planning/](../../planning/)
- Authoring guide: [dev/authoring-schemas.md](../../../dev/authoring-schemas.md)
- Built-in schema this one draws from: [engine/crates/memstead-schema/builtins/schemas/default/](../../../engine/crates/memstead-schema/builtins/schemas/default/)
