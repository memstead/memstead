//! Parameter structs for the runtime mem-lifecycle tools —
//! `memstead_mem_create` and `memstead_mem_delete`.
//!
//! These are the on-the-wire shapes agents send; the full MCP handlers
//! translate them before calling the orchestrators. Full-only — lean
//! `FilesystemMcpServer` does not expose mem lifecycle.

use rmcp::schemars;

/// On-the-wire shape mirroring `memstead_schema::VcsConfig` with a
/// `JsonSchema` derivation for rmcp tool routing. Kept separate from the
/// core type so the schema crate does not need a `schemars` dependency
/// just to support one MCP-facing parameter. The fields and semantics
/// match 1:1 — see `memstead_schema::VcsConfig` for the canonical
/// documentation.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VcsConfigInput {
    #[schemars(description = "Path to the gitdir relative to the new mem's root.")]
    pub gitdir: String,
    #[schemars(
        description = "Path to the worktree relative to the new mem's root. Defaults to `\".\"` (mem root) when omitted."
    )]
    #[serde(default = "default_worktree")]
    pub worktree: String,
}

fn default_worktree() -> String {
    ".".to_string()
}

impl From<VcsConfigInput> for memstead_schema::VcsConfig {
    fn from(v: VcsConfigInput) -> Self {
        Self {
            gitdir: v.gitdir,
            worktree: v.worktree,
        }
    }
}

/// Wire-shape recovery action for `memstead_mem_create`. The
/// storage-residue refusal path exposes three explicit
/// recovery options the caller picks via this enum. The wire
/// tokens (`reattach` / `force_overwrite` / `hard_cleanup_first`)
/// match `memstead_engine::RecoveryAction::as_wire_str()` so the
/// MCP serde shape and the CLI flag bridge converge on a single
/// engine-side enum.
#[derive(Debug, Clone, Copy, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryActionInput {
    /// Adopt the residual entities; skip the seed commit. Default
    /// when the residue was left by a deliberate `memstead mem
    /// unregister`. Explicit `reattach` overrides the default for
    /// crash-residue scenarios where the operator has verified the
    /// content is safe to adopt.
    Reattach,
    /// Destroy the residue, then proceed with the normal create
    /// path. Prior entities are gone. **Not yet implemented** — the
    /// orchestrator currently refuses with `INVALID_INPUT` pointing
    /// at `memstead mem delete <name>`.
    ForceOverwrite,
    /// Refuse with `MEM_STORAGE_RESIDUE_DETECTED`, instructing the
    /// caller to run `memstead mem delete <name>` first. Hard barrier
    /// against destructive auto-recovery — for operators who want
    /// the cleanup to be a separate, named operation.
    HardCleanupFirst,
}

impl From<RecoveryActionInput> for memstead_engine::RecoveryAction {
    fn from(v: RecoveryActionInput) -> Self {
        match v {
            RecoveryActionInput::Reattach => memstead_engine::RecoveryAction::Reattach,
            RecoveryActionInput::ForceOverwrite => memstead_engine::RecoveryAction::ForceOverwrite,
            RecoveryActionInput::HardCleanupFirst => {
                memstead_engine::RecoveryAction::HardCleanupFirst
            }
        }
    }
}

/// Parameters for `memstead_mem_create`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemCreateParams {
    #[schemars(
        description = "Unique name for the new mem — the full hierarchical identifier (e.g. `\"sub-mem\"` for flat layouts or `\"team/sub-mem\"` for hierarchical layouts); the value flows through verbatim. Grammar: lowercase ASCII letters, digits, hyphens; segments separated by `/`; no leading, trailing, or double slashes. Must not collide with any currently-registered mem."
    )]
    pub name: String,
    #[schemars(
        description = "Target filesystem location. Absolute path, or relative to the workspace root. Canonicalized before the allowlist check — `./a/../b` is reduced to `./b` prior to matching."
    )]
    pub location: String,
    #[schemars(
        description = "Schema pin for the new mem. Format: `name@x.y.z` — e.g. `default@1.0.0`. Resolved against the per-mem schema registry at init time."
    )]
    pub schema: String,
    #[schemars(
        description = "Optional VCS layout override. Shape: `{ \"gitdir\": \".git\", \"worktree\": \".\" }` (default isolated) or `{ \"gitdir\": \"../.git\", \"worktree\": \"..\" }` (shared-gitdir idiom). Paths are relative to the new mem's root. When absent, the engine uses the isolated default."
    )]
    pub vcs: Option<VcsConfigInput>,
    #[schemars(
        description = "Agent-authored provenance note recorded in the seed commit's body (≤280 chars). One sentence describing why this mem was created."
    )]
    pub note: Option<String>,
    #[schemars(
        description = "Explicit recovery action when on-disk storage residue is detected at the composed branch path. Three accepted values: `reattach` (adopt the residual entities, skip the seed commit), `force_overwrite` (destroy the residue, currently refuses with `INVALID_INPUT` — implementation pending), `hard_cleanup_first` (refuse with `MEM_STORAGE_RESIDUE_DETECTED`, instructing the caller to run `memstead_mem_delete` first). When omitted, the engine routes by whether the residue was left by a deliberate `memstead mem unregister`: such residue defaults to `reattach` and emits a `MEM_REATTACHED_AFTER_UNREGISTER` warning; residue from a crash refuses with `MEM_STORAGE_RESIDUE_DETECTED`. Bare create against a name with no residue ignores this field."
    )]
    pub recovery: Option<RecoveryActionInput>,
    #[schemars(
        description = "Inline the resolved schema body on the response (byte-identical to `memstead_schema(name=<resolved-schema>)` at the same verbosity). Default `false` — the response carries only `schema_ref`, `name`, `location`, and `seed_commit_sha`. Set to `true` for first-time-schema callers that want one round-trip instead of two; the schema is workspace-stable, so for the agent's second+ mem on the same schema the omitted default is the right call."
    )]
    #[serde(default)]
    pub include_schema: bool,
    #[schemars(
        description = "Verbosity of the inlined schema body when `include_schema: true`. `\"lite\"` (default, absent) inlines the cheap cold-start skeleton (entity-type names + section keys + field shapes, relationship names + endpoints, the alias pointer; prose dropped) — the right pairing for a first-mem create that only needs to orient, and byte-identical to `memstead_schema`'s default reply. `\"full\"` inlines the complete schema — byte-identical to `memstead_schema(name=<resolved-schema>, verbosity=\"full\")`. Ignored when `include_schema` is false. Any value other than `\"full\"`/`\"lite\"` returns `INVALID_INPUT` naming the bad value."
    )]
    pub schema_verbosity: Option<String>,
    #[schemars(
        description = "Optional per-instance writing guidance, written verbatim into the new mem's config `writeGuidance` map in the seed commit. An opaque string-keyed JSON object — e.g. `{ \"phase_context\": \"early design\", \"stack\": \"Rust\" }`. The engine never interprets the keys (schema-strictness D8 — `writeGuidance` is client-owned vocabulary); a client that read the resolved schema package's `mem-template.json` fills the instance keys and passes them here. Omit (or pass `{}`) to seed no guidance."
    )]
    #[serde(default)]
    pub write_guidance: std::collections::HashMap<String, serde_json::Value>,
}

/// Parameters for `memstead_mem_set_schema` — the integrity-driven
/// schema-migration trigger.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemSetSchemaParams {
    #[schemars(description = "Name of the writable mem whose schema pin is being set.")]
    pub mem: String,
    #[schemars(
        description = "Target schema ref, exact `name@x.y.z`. Must resolve against the loaded schema catalogue (mem-pinned, workspace, built-in); unresolvable refs refuse with SCHEMA_NOT_FOUND, malformed refs with INVALID_INPUT."
    )]
    pub schema: String,
    #[schemars(
        description = "Optional provenance note (≤280 chars). Reserved: the pin lives in workspace state today (no mem commit is produced), so the note is accepted for wire-compat and recorded once the pin-relocation cut moves the schema pin into mem config."
    )]
    pub note: Option<String>,
}

/// Parameters for `memstead_mem_set_version`. F1.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemSetVersionParams {
    #[schemars(description = "Name of the mem whose `version` field is being updated.")]
    pub name: String,
    #[schemars(
        description = "New semver version (e.g. `0.2.0`, `1.0.0-beta.1`). Validated as semver; malformed values refuse with `INVALID_INPUT`. The version is consumed by `memstead_export --format mem` to stamp the archive filename and the `.mem` archive's published config — bump before publishing. Initial mem-create seeds `0.1.0` so this surface is the only path that needs to be invoked when an agent or operator is ready to ship."
    )]
    pub version: String,
    #[schemars(
        description = "Optional provenance note (≤280 chars) recorded on the version-bump commit body. When the workspace sets `require_notes`, omitting it rides a non-blocking `NOTE_MISSING` warning (the bump still lands)."
    )]
    pub note: Option<String>,
}

/// Parameters for `memstead_mem_delete`.
///
/// The MCP surface collapses to one verb that always means destructive.
/// The earlier `delete_files: bool` parameter retired — agents have no legitimate
/// need to "preserve storage but unregister"; the router-only
/// unregister-preserve-storage workflow stays reachable via the CLI's
/// `memstead mem unregister` verb (operator-only). The MCP wrapper
/// hardcodes `delete_files: true` when invoking the engine, so the
/// promised refusals (`MEM_REFERENCED_BY_POLICY`,
/// `MEM_HAS_INCOMING_REFS`) and the policy scrub on success always
/// fire.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemDeleteParams {
    #[schemars(description = "Name of the mem to destroy.")]
    pub name: String,
    #[schemars(
        description = "Agent-authored provenance note (≤280 chars). Surfaces in the outer-repo Stop-hook aggregation via the engine's trace surface; no per-mem commit is produced by delete."
    )]
    pub note: Option<String>,
}

// ---------------------------------------------------------------------------
// Wire-shape param structs for the
// six `memstead_workspace_*` tools wrapping the engine-located
// `workspace_config_edit` writers. The MCP surface mirrors the CLI
// verbs (`memstead workspace grant-cross-link`, etc.), so an
// MCP-driven agent can complete the dynamic mem lifecycle
// (`mem_create → workspace_grant_cross_link → relate → unrelate
// → workspace_revoke_cross_link → mem_delete`) without dropping
// to the CLI.
// ---------------------------------------------------------------------------

/// Parameters for `memstead_workspace_grant_cross_link`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceGrantCrossLinkParams {
    #[schemars(
        description = "Source mem. The grantee — the mem permitted to author cross-mem edges into `to`."
    )]
    pub from: String,
    #[schemars(
        description = "Target mem. Pass a named mem (e.g. `\"specs\"`) to append to the named-allowlist shape, or the literal `\"*\"` to set the wildcard shape (any target). Wildcard vs. named is mutually exclusive per `from`-mem — switching between requires revoking the prior shape first; mixing surfaces `CROSS_LINK_CONFLICT`."
    )]
    pub to: String,
}

/// Parameters for `memstead_workspace_revoke_cross_link`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRevokeCrossLinkParams {
    #[schemars(description = "Source mem. The grantee whose existing grant is being revoked.")]
    pub from: String,
    #[schemars(
        description = "Target mem, or `\"*\"` to revoke the wildcard shape. When the underlying list becomes empty, the `from` key is dropped entirely."
    )]
    pub to: String,
}

/// Parameters for `memstead_workspace_allow_create`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceAllowCreateParams {
    #[schemars(
        description = "Glob pattern matched against composed mem candidates (`<mem_path>/<name>` for hierarchical, bare `<name>` for flat). First-match-wins; lower index = higher priority."
    )]
    pub pattern: String,
    #[schemars(
        description = "Schema pins admitted by this rule. `[\"*\"]` is the any-schema escape. Each entry is a canonical `name@version` pin (e.g. `\"default@1.0.0\"`)."
    )]
    pub schemas: Vec<String>,
    #[schemars(
        description = "Existing pattern to insert before — lifts the new rule above the named pattern in the priority list. Omit to append at the end (lowest priority)."
    )]
    pub before: Option<String>,
    #[schemars(
        description = "Default cross-mem link grants for mems matching this rule. Each entry is a target-mem name (`\"specs\"`) or `\"*\"` (any). Pre-populates `[cross_mem_links]` for matching new mems so agents don't have to grant a second time."
    )]
    pub default_cross_links: Option<Vec<String>>,
}

/// Parameters for `memstead_workspace_revoke_create`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRevokeCreateParams {
    #[schemars(
        description = "Glob pattern of the `[[mem_management.create]]` rule to drop. Matched exactly against the rule's `pattern` field."
    )]
    pub pattern: String,
}

/// Parameters for `memstead_workspace_allow_delete`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceAllowDeleteParams {
    #[schemars(
        description = "Glob pattern matched against composed mem candidates. Appended to `[[mem_management.delete]]` — the symmetric allowlist for `memstead_mem_delete`. Agent-creatable equals agent-deletable in spirit; mirror the create-side `pattern` to keep parity."
    )]
    pub pattern: String,
}

/// Parameters for `memstead_workspace_revoke_delete`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRevokeDeleteParams {
    #[schemars(
        description = "Glob pattern of the `[[mem_management.delete]]` rule to drop. Matched exactly against the rule's `pattern` field."
    )]
    pub pattern: String,
}
