---
title: "Surface Parity Matrix"
---

# Surface Parity Matrix

Every public engine operation across the three programmatic surfaces (MCP, CLI, WASM). Rows are aligned by the hand-maintained `xtask/operations.toml` registry; cells render the surface-specific name when present and `—` when the surface doesn't expose the operation. The Registry HTTP surface is its own publication layer and not in this matrix.

## Matrix

| Operation | MCP | CLI | WASM |
|-----------|-----|-----|------|
| `entity-read` | `memstead_entity` *(lean + full)* | `entity` *(lean + full)* | `getEntity` |
| `search` | `memstead_search` *(lean + full)* | `search` *(lean + full)* | `search` |
| `list` | — | `list` *(lean + full)* | — |
| `relations-read` | — | `relations` *(lean + full)* | — |
| `context` | — | `context` *(lean + full)* | — |
| `overview` | `memstead_overview` *(lean + full)* | `overview` *(lean + full)* | — |
| `status` | — | `status` *(lean + full)* | — |
| `schema-describe` | `memstead_schema` *(lean + full)* | `type` *(lean + full)* | — |
| `health` | `memstead_health` *(lean + full)* | `health` *(lean + full)* | `health` |
| `changes-since` | `memstead_changes_since` *(lean + full)* | `changes` *(lean + full)* | — |
| `reload` | `memstead_reload` *(full only)* | `reload` *(lean + full)* | — |
| `fetch` | — | `fetch` *(full only)* | — |
| `pull` | — | `pull` *(full only)* | — |
| `push` | — | `push` *(full only)* | — |
| `branch-reset` | — | `branch-reset` *(full only)* | — |
| `create` | `memstead_create` *(lean + full)* | `create` *(lean + full)* | — |
| `update` | `memstead_update` *(lean + full)* | `update` *(lean + full)* | — |
| `relate` | `memstead_relate` *(lean + full)* | `relate` *(lean + full)* | — |
| `delete` | `memstead_delete` *(lean + full)* | `delete` *(lean + full)* | — |
| `rename` | `memstead_rename` *(lean + full)* | `rename` *(lean + full)* | — |
| `mem-create` | `memstead_mem_create` *(full only)* | `mem` *(full only)* | — |
| `mem-delete` | `memstead_mem_delete` *(full only)* | `mem` *(full only)* | — |
| `mem-set-version` | `memstead_mem_set_version` *(full only)* | `mem` *(full only)* | — |
| `workspace-allow-create` † | — | `workspace` *(full only)* | — |
| `workspace-revoke-create` † | — | `workspace` *(full only)* | — |
| `workspace-allow-delete` † | — | `workspace` *(full only)* | — |
| `workspace-revoke-delete` † | — | `workspace` *(full only)* | — |
| `workspace-grant-cross-link` † | — | `workspace` *(full only)* | — |
| `workspace-revoke-cross-link` † | — | `workspace` *(full only)* | — |
| `projection-brief` | — | `projection` *(lean + full)* | — |
| `projection-init` | — | `projection` *(lean + full)* | — |
| `projection-migrate` | — | `projection` *(lean + full)* | — |
| `projection-advance` | — | `projection` *(lean + full)* | — |
| `projection-enable` | — | `projection` *(lean + full)* | — |
| `parse-recovery` | — | `recover` *(full only)* | — |
| `agent-notes` | — | — | — |
| `mem-head-sha` | — | — | — |
| `from-snapshot` | — | — | `fromSnapshot` |
| `apply-commit` | — | — | `applyCommit` |
| `mem-names` | — | — | `memNames` |
| `set-panic-hook` | — | — | `setPanicHook` |

† These absences are decisions, not gaps. Every other empty cell is simply an operation that surface does not carry.

- `workspace-allow-create` — workspace policy decides what an agent may do, so it is an operator surface: the CLI and the operator-authenticated web API, never the agent's MCP surface. A policy-gated mutation refuses with the exact command to report.
- `workspace-revoke-create` — workspace policy decides what an agent may do, so it is an operator surface: the CLI and the operator-authenticated web API, never the agent's MCP surface. A policy-gated mutation refuses with the exact command to report.
- `workspace-allow-delete` — workspace policy decides what an agent may do, so it is an operator surface: the CLI and the operator-authenticated web API, never the agent's MCP surface. A policy-gated mutation refuses with the exact command to report.
- `workspace-revoke-delete` — workspace policy decides what an agent may do, so it is an operator surface: the CLI and the operator-authenticated web API, never the agent's MCP surface. A policy-gated mutation refuses with the exact command to report.
- `workspace-grant-cross-link` — workspace policy decides what an agent may do, so it is an operator surface: the CLI and the operator-authenticated web API, never the agent's MCP surface. A policy-gated mutation refuses with the exact command to report.
- `workspace-revoke-cross-link` — workspace policy decides what an agent may do, so it is an operator surface: the CLI and the operator-authenticated web API, never the agent's MCP surface. A policy-gated mutation refuses with the exact command to report.

## Unaligned

Surface entries the registry does not pin to a logical operation. Either add a row to `xtask/operations.toml` or, if the entry is intentionally surface-local (e.g. CLI-only registry / setup commands), leave it here as a deliberate gap.

### Unaligned — MCP

- `memstead_check`
- `memstead_diff`
- `memstead_mem_configure`
- `memstead_mem_set_schema`

### Unaligned — CLI

- `admin`
- `anchors`
- `batch-create`
- `batch-relate`
- `batch-update`
- `check`
- `conflicts`
- `domain`
- `due`
- `export`
- `init`
- `install`
- `link`
- `login`
- `logout`
- `mem-repo`
- `publish`
- `quickstart`
- `review-mark`
- `schema`
- `uninstall`
- `unpublish`
- `verify-anchors`

### Unaligned — WASM

- `entityIds`

