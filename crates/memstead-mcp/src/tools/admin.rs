//! Parameter structs for admin tools.

use rmcp::schemars;

/// Parameters for memstead_health.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HealthParams {
    #[schemars(
        description = "Detail sections to include (default: none — summary counts only). Allowed keys: orphans, stubs, most_connected, missing_fields, stale, dangling_links, tags, missing_required_outgoing, constraints, conformance, integrity, config, anchors, friction, open_questions, stale_derivations, checks, signals, labelling, ledger, vital_signs. `conformance` lints every entity against the effective schema and returns per-entity `findings` (`{id, axis, code, detail}` with write-time typed codes); `integrity` additionally projects the consistency axis into the same findings list: UNRESOLVED_STUB; CROSS_MEM_EDGE_UNGRANTED (an existing cross-mem edge the workspace grant table no longer permits — a state the default-deny write gate would refuse to create today; reported and strict, never a load refusal, and the detail names the referrer, the target and that the cause is the absent grant rather than a missing target); plus three dangling conditions, each with its own repair and each finding carrying it as a `repair` detail — DANGLING_LINK_TARGET_MISSING (a body wiki-link with no markdown file behind it: create the target entity, or remove the wiki-link), DANGLING_LINK_NOT_RELATED (a body wiki-link to a written entity the referrer does not relate to: relate the two, so the link is backed by a relationship row), and DANGLING_RELATION_TARGET_MISSING (a relationships row naming an entity absent from the store: remove the row, or create the target). A stub target on a relationships row is a legitimate forward reference and is not flagged. `friction` summarizes the workspace-local refusal ledger — counts per typed refusal code and per verb, with a per-code `by_reason` breakdown where the code carries a closed engine-owned discriminator, whole-ledger plus a recent 24h window; the ledger is local-only, refusals-only, and every recorded value is drawn from a closed engine-defined vocabulary (never parameters, free-form strings, or payload text). `open_questions` composes a per-mem worklist of what the holding does not know — stubs, anchors that are recheck / unresolvable (artifact gone) / unobserved (not measured) / dangling (entity gone), unsatisfied constraints, dangling links, and a paired process mem's open entries with negative findings under a distinct already-searched heading (done, keep off); every list is capped with an explicit `more` remainder. `stale_derivations` lists per mem every explicit edge on a derivation-declared rel-type whose target's current hash differs from the recorded baseline (`stale`) or that has no baseline (`unbaselined` — never fabricated as fresh or stale); re-assert the edge via memstead_relate to refresh the baseline. `ledger` reconciles a FOLDER mem's change ledger against the markdown files beside it (`{ledger_without_file, file_without_ledger}` per mem) — read-only, it never writes or tidies a ledger line; git-branch mems are ABSENT from the map rather than clean in it, because their change set is a real two-tree diff and the divergence cannot arise. Unknown keys surface as UNKNOWN_INCLUDE_KEY on warnings."
    )]
    pub include: Option<Vec<String>>,
    #[schemars(
        description = "Schema ref (`name@x.y.z`) the `conformance`/`integrity` includes lint against instead of each mem's current pin. Omit (default) to lint against the current pin. Only consulted when `include` requests the conformance axis; an unresolvable ref refuses with SCHEMA_NOT_FOUND."
    )]
    pub target_schema: Option<String>,
    #[schemars(description = "Max results for most_connected (default: 10, max: 100)")]
    pub limit: Option<usize>,
    #[schemars(
        description = "Scope counts, distributions, and detail lists to a single writable mem. `writable_mems`/`read_mems` still show the full roster so the agent sees the whole workspace. Omit (default) for global aggregates."
    )]
    pub mem: Option<String>,
    #[schemars(
        description = "When true, the response carries the `[mutations]` posture (`mutations.require_notes`), the opaque `[plugin.*]` pass-through map, and a per-writable-mem `mems` detail array with `{ name, origin, vcs: { gitdir, worktree, head } }` — absolute canonical paths plus the cached branch-tip SHA (omitted on fresh mems with no commits yet) for the Stop-hook / sync flows so they never hardcode a layout or peel refs themselves. Documented alias: `include: [\"config\"]` renders the identical projection (the catalogue form every surface shares, including the CLI's `--include config`); passing both renders it once. Defaults to false — the absence of these fields is the default-posture signal. **Lifecycle policy** (`[[mem_management.create]]` / `[[mem_management.delete]]`) is surfaced via `memstead_overview`, not here — `memstead_health` is drift/diagnostics."
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
#[serde(deny_unknown_fields)]
pub struct ReloadParams {
    #[schemars(
        description = "Writable mem name to reload. Omit to reload every writable mem. Use the per-mem form for cheap, targeted refreshes when you know which mem drifted; use the workspace-wide form (omit `mem`) when an out-of-band `git pull` may have advanced multiple branches at once, or to pick up CLI-driven workspace-policy edits (allowlist / cross-link / mutation policy) — per-mem reload skips that workspace-level settings refresh. Reload scope covers **mem-scoped** state: a mem's `sync_state` (the projection baselines `#synced`/`#verified`) rides its destination mem's config, so an out-of-band `sync_state` write — e.g. from `memstead projection advance` / `mem set-sync-state` in a sibling process — is picked up by a per-mem (or workspace-wide) reload of that mem. The engine-owned advance/disposition store (`.memstead/state/advance/`) is **workspace-store** state read fresh from disk per operation — it is reload-independent by design and is neither refreshed nor invalidated by this call."
    )]
    pub mem: Option<String>,
    #[schemars(
        description = "Full refresh. `true` re-scans the schema sources and reconciles the mount roster ON TOP of the workspace-wide content reload — the same reconciliation every operation runs before it serves (`MEM_ROSTER_CHANGED`), forced here so the report is authoritative: schema versions installed out of band become resolvable, mems registered out of band mount cold, mems unregistered out of band are UNMOUNTED (their entities, search index and community partition gone; an operation naming one refuses `MEM_UNMOUNTED`), a roster entry that fails to mount is quarantined with its reason. Schema removals stay skipped. The response gains a `refresh` block — `schemas_added`, `schema_removals_skipped`, `mems_mounted`, `mems_unmounted`, `mems_quarantined`, `failures[]` (per-item; a failed source or mount never surfaces as newly available and does not abort the others), `elapsed_ms`. Incompatible with `mem`. Default `false`: the content reload alone (membership is still reconciled by the per-operation probe)."
    )]
    pub full: Option<bool>,
}

/// Parameters for memstead_diff. Two-ref structural diff at entity
/// granularity; the response wire shape is `Diff` / `EntityDiff` from
/// `memstead_base::ops::diff`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffParams {
    #[schemars(
        description = "Mem that selects the storage context (the gitdir, for git-branch mounts). `ref_a` / `ref_b` are arbitrary refs resolved inside that gitdir; cross-mem diffs work via fully-qualified refs (`refs/heads/<other-mem>`). Folder / archive mounts refuse the call with `INVALID_INPUT` — they carry no git refs to diff."
    )]
    pub mem: String,
    #[schemars(
        description = "First ref to diff. Branch name (`main`), full ref (`refs/heads/specs`), commit SHA, or tag. Unknown refs refuse with `UNKNOWN_REF` and `details.ref` carrying the raw input."
    )]
    pub ref_a: String,
    #[schemars(description = "Second ref to diff. Same input shape as `ref_a`.")]
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
        description = "When true (default), each entry's `ripple` carries per-side `{from_id, side}` entries for entities with inbound wiki-links to the affected entry — `side: \"ref_a\"` lists referrers at the `ref_a` snapshot, `side: \"ref_b\"` at `ref_b` — so a consumer sees what would break if the change were applied or skipped. Pass false to omit the field (e.g. for large mems where the per-side wiki-link scan dominates cost)."
    )]
    #[serde(default = "default_true")]
    pub include_ripple: bool,
}

fn default_true() -> bool {
    true
}

/// Parameters for memstead_changes_since.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangesSinceParams {
    #[schemars(description = "Writable mem name. Call memstead_health for the list.")]
    pub mem: String,
    #[schemars(
        description = "Change cursor to diff against. Backend-specific, and never a mutation's `write_id`: on a git-branch mem a commit SHA (pass the `head` a prior call returned, or the canonical git empty-tree hash `4b825dc642cb6eb9a060e54bf8d69288fbee4904` to get every entity as `added` on a fresh-client first sync); on a folder mem an RFC3339 timestamp (the `ts` of the last entry you received, or empty for a first sync)."
    )]
    pub since: String,
    #[schemars(
        description = "Rename detection threshold for content-similarity, in [0.1, 1.0]. Default (None) → 0.6. Lower values widen the recall window at the cost of false-positive rename pairing; raise it to 0.9+ when you want only near-byte-identical renames collapsed. Out-of-range values refuse with `INVALID_INPUT` naming `details.allowed_range` and `details.requested` — agents recover by reissuing with a value inside `[0.1, 1.0]`."
    )]
    pub rename_similarity: Option<f32>,
    #[schemars(
        description = "Fold per-commit agent-notes into the response. When true, the report carries a `notes[]` array (one entry per commit between `since` and `head`, with `sha`, `subject`, `tool_verb`, `entity_id`, `note`, `actor`, `tool`, `client`, `timestamp`) plus `memstead_ref` — the SHA of the unified schema + per-mem-config registry, absent when the workspace has not been migrated yet. Default false (entity-delta only). Commit-mirroring clients turn this on to receive notes and the registry-ref sha in one round-trip; agents that just need entity events leave it off."
    )]
    #[serde(default)]
    pub include_notes: bool,
}
