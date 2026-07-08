---
title: "Surface Parity Matrix"
---

# Surface Parity Matrix

Every public engine operation across the four programmatic surfaces (MCP, CLI, UniFFI, WASM). Rows are aligned by the hand-maintained `xtask/operations.toml` registry; cells render the surface-specific name when present and `—` when the surface doesn't expose the operation. The Registry HTTP surface is its own publication layer and not in this matrix.

## Matrix

| Operation | MCP | CLI | UniFFI | WASM |
|-----------|-----|-----|--------|------|
| `entity-read` | `memstead_entity` *(lean + full)* | `entity` *(lean + full)* | `get_entity` | `getEntity` |
| `search` | `memstead_search` *(lean + full)* | `search` *(lean + full)* | `search` | `search` |
| `list` | — | `list` *(lean + full)* | `list_entities` | — |
| `relations-read` | — | `relations` *(lean + full)* | `get_relations` | — |
| `context` | — | `context` *(lean + full)* | — | — |
| `overview` | `memstead_overview` *(lean + full)* | `overview` *(lean + full)* | `get_overview` | — |
| `stats` | — | `stats` *(lean + full)* | `get_stats` | — |
| `schema-describe` | `memstead_schema` *(lean + full)* | `type` *(lean + full)* | `schema_json` | — |
| `health` | `memstead_health` *(lean + full)* | `health` *(lean + full)* | `get_health` | `health` |
| `changes-since` | `memstead_changes_since` *(lean + full)* | `changes` *(lean + full)* | `changes_since` | — |
| `reload` | `memstead_reload` *(full only)* | `reload` *(lean + full)* | `reload` | — |
| `fetch` | — | `fetch` *(full only)* | — | — |
| `pull` | — | `pull` *(full only)* | — | — |
| `push` | — | `push` *(full only)* | — | — |
| `branch-reset` | — | `branch-reset` *(full only)* | `branch_reset` | — |
| `create` | `memstead_create` *(lean + full)* | `create` *(lean + full)* | — | — |
| `update` | `memstead_update` *(lean + full)* | `update` *(lean + full)* | — | — |
| `relate` | `memstead_relate` *(lean + full)* | `relate` *(lean + full)* | — | — |
| `delete` | `memstead_delete` *(lean + full)* | `delete` *(lean + full)* | — | — |
| `rename` | `memstead_rename` *(lean + full)* | `rename` *(lean + full)* | — | — |
| `mem-create` | `memstead_mem_create` *(full only)* | `mem` *(full only)* | — | — |
| `mem-delete` | `memstead_mem_delete` *(full only)* | `mem` *(full only)* | — | — |
| `mem-set-version` | `memstead_mem_set_version` *(full only)* | `mem` *(full only)* | — | — |
| `workspace-allow-create` | `memstead_workspace_allow_create` *(full only)* | `workspace` *(full only)* | — | — |
| `workspace-revoke-create` | `memstead_workspace_revoke_create` *(full only)* | `workspace` *(full only)* | — | — |
| `workspace-allow-delete` | `memstead_workspace_allow_delete` *(full only)* | `workspace` *(full only)* | — | — |
| `workspace-revoke-delete` | `memstead_workspace_revoke_delete` *(full only)* | `workspace` *(full only)* | — | — |
| `workspace-grant-cross-link` | `memstead_workspace_grant_cross_link` *(full only)* | `workspace` *(full only)* | — | — |
| `workspace-revoke-cross-link` | `memstead_workspace_revoke_cross_link` *(full only)* | `workspace` *(full only)* | — | — |
| `parse-recovery` | — | `recover` *(full only)* | `apply_parse_recovery` | — |
| `agent-notes` | — | — | `agent_notes` | — |
| `mem-head-sha` | — | — | `mem_head_sha` | — |
| `from-snapshot` | — | — | — | `fromSnapshot` |
| `apply-commit` | — | — | — | `applyCommit` |
| `mem-names` | — | — | — | `memNames` |
| `set-panic-hook` | — | — | — | `setPanicHook` |

## Unaligned

Surface entries the registry does not pin to a logical operation. Either add a row to `xtask/operations.toml` or, if the entry is intentionally surface-local (e.g. CLI-only registry / setup commands), leave it here as a deliberate gap.

### Unaligned — MCP

- `memstead_diff`
- `memstead_mem_set_schema`

### Unaligned — CLI

- `admin`
- `batch-update`
- `domain`
- `export`
- `ingest`
- `init`
- `install`
- `link`
- `login`
- `logout`
- `mem-repo`
- `pipeline`
- `publish`
- `quickstart`
- `schema`
- `unpublish`

### Unaligned — UniFFI

- `add_facet`
- `add_ingest`
- `add_medium`
- `add_projection`
- `branch_reset_stranded_refs`
- `create_mem`
- `delete_facet`
- `delete_ingest`
- `delete_medium`
- `delete_mem`
- `delete_projection`
- `diff`
- `export_mem`
- `mem_roster`
- `pipeline_configs_json`
- `rename_facet`
- `rename_ingest`
- `rename_medium`
- `rename_projection`
- `set_mem_schema`
- `set_mem_version`
- `update_facet`
- `update_ingest`
- `update_medium`
- `update_projection`
- `workspace_requires_notes`

