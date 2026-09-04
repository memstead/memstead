---
title: "Surface Parity Matrix"
---

# Surface Parity Matrix

Every public engine operation across the three programmatic surfaces (MCP, CLI, WASM). Rows are aligned by the hand-maintained `xtask/operations.toml` registry; cells render the surface-specific name when present and `—` when the surface doesn't expose the operation. The Registry HTTP surface is its own publication layer and not in this matrix.

## Matrix

| Operation | MCP | CLI | WASM |
|-----------|-----|-----|------|
| `entity-read` | `memstead_entity` | `entity` | `getEntity` |
| `search` | `memstead_search` | `search` | `search` |
| `list` | — | `list` | — |
| `relations-read` | — | `relations` | — |
| `context` | — | `context` | — |
| `overview` | `memstead_overview` | `overview` | — |
| `status` | — | `status` | — |
| `schema-describe` | `memstead_schema` | `type` | — |
| `health` | `memstead_health` | `health` | `health` |
| `changes-since` | `memstead_changes_since` | `changes` | — |
| `reload` | `memstead_reload` | `reload` | — |
| `fetch` | — | `fetch` | — |
| `pull` | — | `pull` | — |
| `push` | — | `push` | — |
| `branch-reset` | — | `branch-reset` | — |
| `create` | `memstead_create` | `create` | — |
| `update` | `memstead_update` | `update` | — |
| `relate` | `memstead_relate` | `relate` | — |
| `delete` | `memstead_delete` | `delete` | — |
| `rename` | `memstead_rename` | `rename` | — |
| `mem-create` | `memstead_mem_create` | `mem` | — |
| `mem-delete` | `memstead_mem_delete` | `mem` | — |
| `mem-set-version` | `memstead_mem_set_version` | `mem` | — |
| `workspace-allow-create` † | — | `workspace` | — |
| `workspace-revoke-create` † | — | `workspace` | — |
| `workspace-allow-delete` † | — | `workspace` | — |
| `workspace-revoke-delete` † | — | `workspace` | — |
| `workspace-grant-cross-link` † | — | `workspace` | — |
| `workspace-revoke-cross-link` † | — | `workspace` | — |
| `projection-brief` | — | `projection` | — |
| `projection-init` | — | `projection` | — |
| `projection-migrate` | — | `projection` | — |
| `projection-advance` | — | `projection` | — |
| `projection-enable` | — | `projection` | — |
| `parse-recovery` | — | `recover` | — |
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
- `memstead_retype`

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
- `gates`
- `init`
- `install`
- `login`
- `logout`
- `mem-repo`
- `publish`
- `quickstart`
- `retype`
- `review-mark`
- `schema`
- `uninstall`
- `unpublish`
- `verify-anchors`

### Unaligned — WASM

- `entityIds`

