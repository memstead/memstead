# `ingest` — schema for ingest process vaults

Workspace-level schema for every `ingest/<ingest-name>` vault paired 1:1 with one ingest configuration. Three types capture destination-quality debt that survives between agent runs:

| Shape of debt | Type |
|---|---|
| Something *absent* from the destination | `coverage_gap` |
| A *present* destination claim that hasn't been verified | `verification_target` |
| Two destination claims contradict, or a destination claim contradicts the source | `inconsistency` |

The vault is scaffolding, not a session log. Each entry is an objective claim about destination state — the kind of debt the next run can either resolve directly or use to choose what to address. When the destination is fixed, the entry is deleted in the same run.

## Location

The schema lives on `__MEMSTEAD:schemas/ingest@0.1.0/` of the workspace's `vault-repo` (loaded via `memstead-core::vault_repo_schemas::load_schemas_from_ref`). Process vaults pin it via `"schema": "ingest@0.1.0"` (or the unversioned `"ingest"`) in their `.memstead/config.json`.

## Lifecycle

A process vault is created automatically by the ingest skill on first run of `/memstead:ingest <ingest-name>` (operator-mode CLI invocation, `memstead vault create ingest/<ingest-name> --schema ingest@0.1.0`). Subsequent runs reuse it. The operator deletes a process vault explicitly via `/memstead:ingest --clear <ingest-name>` (per-ingest, idempotent on already-deleted vaults). One-shot/lens ingests are by-design ephemeral and do not get a process vault.

## Re-verification expectation

Each run *re-verifies* the existing entries against the current destination state and deletes any that intervening runs have resolved. Without this, stale entries accumulate and the vault loses signal. The schema's `default_writing_guidance.goal` and each type's `write_rules` carry this expectation.

## Relationship vocabulary

Strict mode, three definitions:

- `PART_OF` — hierarchy default; rarely authored (entries are leaf observations).
- `REFERENCES` — auto-emitted from inline wiki-links into destination entities. **Do not author manually.**
- `_default` — fallback weight required by the engine.

No internal edges between quality entries. They reach the destination via auto-emitted `REFERENCES`; they do not link to each other.

## Cross-vault links

Each `ingest/<ingest-name>` vault declares an outbound cross-vault permission to its paired destination in the workspace `[cross_vault_links]` section of `.memstead/workspace.toml`. Wiki-links inside `area`, `claim`, `claim_a`, `claim_b` etc. that point at destination entities produce auto-emitted cross-vault `REFERENCES` edges.

## What this schema is *not*

- Not a session-handover log. Bookmarks ("left off at file X") add nothing — the next run reads `memstead_overview` and the destination directly.
- Not a planning vault. For deliberation in flight, use `planning@0.1.0`.
- Not a workbench. If a fact constrains how a destination entity must be, that fact belongs in the destination entity's Constraints section, not as an entry here.
