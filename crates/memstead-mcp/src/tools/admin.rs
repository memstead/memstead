//! Parameter structs for admin tools.

use rmcp::schemars;

/// Parameters for memstead_health.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct HealthParams {
    #[schemars(
        description = "Detail sections to include (default: none — summary counts only). Allowed keys: orphans, stubs, most_connected, missing_fields, stale, dangling_links, tags, missing_required_outgoing, conformance, integrity. `conformance` lints every entity against the effective schema and returns per-entity `findings` (`{id, axis, code, detail}` with write-time typed codes); `integrity` additionally projects the consistency axis (dangling links, stubs) into the same findings list. Unknown keys surface as UNKNOWN_INCLUDE_KEY on warnings."
    )]
    pub include: Option<Vec<String>>,
    #[schemars(
        description = "Schema ref (`name@x.y.z`) the `conformance`/`integrity` includes lint against instead of each vault's current pin. Omit (default) to lint against the current pin. Only consulted when `include` requests the conformance axis; an unresolvable ref refuses with SCHEMA_NOT_FOUND."
    )]
    pub target_schema: Option<String>,
    #[schemars(description = "Max results for most_connected (default: 10, max: 100)")]
    pub limit: Option<usize>,
    #[schemars(
        description = "Scope counts, distributions, and detail lists to a single writable vault. `writable_vaults`/`read_vaults` still show the full roster so the agent sees the whole workspace. Omit (default) for global aggregates."
    )]
    pub vault: Option<String>,
    #[schemars(
        description = "When true, the response carries the `[mutations]` posture (`mutations.require_notes`), the opaque `[plugin.*]` pass-through map, and a per-writable-vault `vaults` detail array with `{ name, origin, vcs: { gitdir, worktree, head } }` — absolute canonical paths plus the cached branch-tip SHA (omitted on fresh vaults with no commits yet) for the Stop-hook / reconcile flows so they never hardcode a layout or peel refs themselves. Defaults to false — the absence of these fields is the default-posture signal. **Lifecycle policy** (`[[vault_management.create]]` / `[[vault_management.delete]]`) is surfaced via `memstead_overview`, not here — `memstead_health` is drift/diagnostics."
    )]
    #[serde(default)]
    pub include_config: bool,
    #[schemars(
        description = "Max tokens for the rendered-markdown text channel. If the report exceeds this, the text returns chunk 1 of N with `_chunk`/`_total_chunks`/`_truncated` frontmatter; page with the `chunk` param. The `structured_content` envelope is never chunked — it always ships whole. Omit to use the server's configured default budget."
    )]
    pub token_budget: Option<usize>,
    #[schemars(
        description = "Which chunk of the rendered-markdown text channel to read (1-based). Only needed when a multi-include report exceeds the token budget. `structured_content` is whole regardless."
    )]
    pub chunk: Option<usize>,
}

/// Parameters for memstead_reload.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct ReloadParams {
    #[schemars(
        description = "Writable vault name to reload. Omit to reload every writable vault. Use the per-vault form for cheap, targeted refreshes when you know which vault drifted; use the workspace-wide form (omit `vault`) when an out-of-band `git pull` may have advanced multiple branches at once, or to pick up CLI-driven workspace-policy edits (allowlist / cross-link / mutation policy) — per-vault reload skips that workspace-level settings refresh."
    )]
    pub vault: Option<String>,
}

/// Parameters for memstead_diff. Two-ref structural diff at entity
/// granularity; the response wire shape is `Diff` / `EntityDiff` from
/// `memstead_base::ops::diff`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DiffParams {
    #[schemars(
        description = "Vault that selects the storage context (the gitdir, for git-branch mounts). `ref_a` / `ref_b` are arbitrary refs resolved inside that gitdir; cross-vault diffs work via fully-qualified refs (`refs/heads/<other-vault>`). Folder / archive mounts refuse the call with `INVALID_INPUT` — they carry no git refs to diff."
    )]
    pub vault: String,
    #[schemars(
        description = "First ref to diff. Branch name (`main`), full ref (`refs/heads/specs`), commit SHA, or tag. Unknown refs refuse with `UNKNOWN_REF` and `details.ref` carrying the raw input."
    )]
    pub ref_a: String,
    #[schemars(
        description = "Second ref to diff. Same input shape as `ref_a`."
    )]
    pub ref_b: String,
    #[schemars(
        description = "Rename detection threshold for content-similarity, in [0.1, 1.0]. Default (None) → 0.6. Out-of-range values refuse with `INVALID_INPUT` (`details.allowed_range`, `details.requested`)."
    )]
    pub rename_similarity: Option<f32>,
    #[schemars(
        description = "When true (default), each entry carries the full markdown body on both sides. When false, only metadata (id, title, type, status) survives — smaller payload, useful for audit counts."
    )]
    #[serde(default = "default_true")]
    pub include_content: bool,
    #[schemars(
        description = "When true (default), each entry's `ripple` carries per-side `{from_id, side}` entries for entities with inbound wiki-links to the affected entry — `side: \"ref_a\"` lists referrers at the `ref_a` snapshot, `side: \"ref_b\"` at `ref_b` — so a consumer sees what would break if the change were applied or skipped. Pass false to omit the field (e.g. for large vaults where the per-side wiki-link scan dominates cost)."
    )]
    #[serde(default = "default_true")]
    pub include_ripple: bool,
}

fn default_true() -> bool {
    true
}

/// Parameters for memstead_changes_since.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ChangesSinceParams {
    #[schemars(description = "Writable vault name. Call memstead_health for the list.")]
    pub vault: String,
    #[schemars(
        description = "Commit SHA to diff against. Pass the `commit_sha` returned by a prior mutation, or the canonical git empty-tree hash `4b825dc642cb6eb9a060e54bf8d69288fbee4904` to get every entity as `added` (fresh-client first sync)."
    )]
    pub since: String,
    #[schemars(
        description = "Rename detection threshold for content-similarity, in [0.1, 1.0]. Default (None) → 0.6. Lower values widen the recall window at the cost of false-positive rename pairing; raise it to 0.9+ when you want only near-byte-identical renames collapsed. Out-of-range values refuse with `INVALID_INPUT` naming `details.allowed_range` and `details.requested` — agents recover by reissuing with a value inside `[0.1, 1.0]`."
    )]
    pub rename_similarity: Option<f32>,
    #[schemars(
        description = "Fold per-commit agent-notes into the response. When true, the report carries a `notes[]` array (one entry per commit between `since` and `head`, with `sha`, `subject`, `tool_verb`, `entity_id`, `note`, `actor`, `tool`, `client`, `timestamp`) plus `memstead_ref` — the SHA of the unified schema + per-vault-config registry, absent when the workspace has not been migrated yet. Default false (entity-delta only). Outer-repo auto-commit consumers turn this on to receive notes and the registry-ref sha in one round-trip; agents that just need entity events leave it off."
    )]
    #[serde(default)]
    pub include_notes: bool,
}
