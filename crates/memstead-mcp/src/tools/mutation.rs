//! Parameter structs for mutation (write) tools.

use indexmap::IndexMap;
use rmcp::schemars;

/// Shared `note` description rendered identically on every mutation-tool
/// parameter. One sentence, ≤280 chars, agent-authored — it lands in the
/// commit body between the subject and the provenance trailers, and is
/// what outer-repo session-bundling hooks aggregate per session.
pub(crate) const NOTE_PARAM_DESCRIPTION: &str =
    "Agent-authored provenance note (≤280 chars, one sentence describing \
     why this mutation happened). Lands in the per-mem commit body between \
     the mechanical subject line and the provenance trailers (`Tool:`, \
     `Actor:`, `Client:`), and is surfaced by the outer-repo Stop hook when \
     aggregating session activity. Omit for pure-housekeeping edits; when \
     `[mutations].require_notes = true` in workspace config a missing note \
     adds a `NOTE_MISSING` `WarningHint` to the response (the mutation still \
     commits).";

/// Parameters for memstead_create.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateParams {
    #[schemars(description = "Entity title (ID is derived automatically as mem--slug(title))")]
    pub title: String,
    #[schemars(
        description = "Entity type. Required. Allowed values are pinned by the target mem's schema — fetch them via `memstead_schema(name=<mem.schema_ref>)` (cached per session). Unknown types refuse with `UNKNOWN_ENTITY_TYPE`."
    )]
    pub entity_type: String,
    #[schemars(description = "Mem name (directory name of the write mem)")]
    pub mem: Option<String>,
    #[schemars(description = "Section contents: { \"identity\": \"...\", \"purpose\": \"...\" }")]
    pub sections: Option<IndexMap<String, String>>,
    #[schemars(description = "Metadata overrides: { \"level\": \"M1\", \"tags\": \"a, b\" }")]
    pub metadata: Option<IndexMap<String, String>>,
    #[schemars(description = "Initial relationships to create after entity is created")]
    pub relations: Option<Vec<RelationInput>>,
    #[schemars(
        description = "Validate and preview the create without executing — no disk write, no store mutation, no VCS commit, no edges added. dry_run runs the SAME validation a real call runs; it is not a softer check. On a VALID entity the response carries the prospective `id`, `file_path`, and `_hash` (bit-identical to what a real call with the same arguments would produce, EXCEPT for engine-auto-stamped timestamps: the hash covers `created_date`, which is stamped from wall-clock `now()` independently in the dry-run and the real call, so the two `_hash` values diverge whenever a second ticks between them; the hash also covers `sections`, `metadata`, and `relations`, so a dry_run that omits `relations` will not match a real call that supplies them), plus any `warnings` and any `incoming` edges that would be adopted from a pre-existing stub at this id, with `commit_sha` empty. On an INVALID entity dry_run does NOT return a warnings-list preview: it refuses with the IDENTICAL typed envelope a real call would return (`MISSING_REQUIRED_SECTION`, `UNKNOWN_SECTION`, `UNKNOWN_METADATA_FIELD`, `INVALID_ENUM_VALUE`, `REQUIRED_FIELD_UNSET`, …), carrying the same recovery `details.*` (e.g. `details.sections[]`). That typed refusal IS the pre-flight signal — read its `details` to fix coverage, then retry. So dry_run never reports a problem entity as clean: it and a real write agree on validity. Use to verify the id slug, or to pre-flight required-section / field coverage and pre-existing references before committing."
    )]
    pub dry_run: Option<bool>,
    #[schemars(description = NOTE_PARAM_DESCRIPTION)]
    pub note: Option<String>,
}

/// A relationship input for create/batch tools.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelationInput {
    #[schemars(description = "Full target entity ID")]
    pub to: String,
    #[schemars(
        description = "Relationship type. Canonical form is UPPER_SNAKE_CASE (USES, PART_OF, DEPENDS_ON) and is what the engine stores; case-insensitive inputs (`uses`, `Part_Of`) are accepted and echoed back in the response as their canonical form. The JSON Schema `pattern` advertises `^[A-Za-z][A-Za-z_]*$` for client-side validators; the engine enforces the same character set independently — characters outside it return `INVALID_REL_TYPE` at the engine boundary regardless of whether the client pre-filters.",
        regex(pattern = r"^[A-Za-z][A-Za-z_]*$")
    )]
    pub r#type: String,
    #[schemars(
        description = "Optional per-edge description text. Validated against the rel-type's `per_edge_description` posture in the pinned schema: `forbidden` (default) rejects a non-empty description with `DESCRIPTION_NOT_PERMITTED`; `required` rejects its absence with `MISSING_REQUIRED_DESCRIPTION`; `optional` accepts both. Empty / whitespace-only strings normalise to absent before validation. Surfaces on `memstead_entity` and round-trips through the `## Relationships` markdown via the canonical em-dash delimiter (` — `)."
    )]
    #[serde(default)]
    pub description: Option<String>,
}

/// Parameters for memstead_update.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateParams {
    #[schemars(description = "Full entity ID to update")]
    pub id: String,
    #[schemars(
        description = "Hash from memstead_entity response (_hash field). Required — read the entity first. Mismatch returns code HASH_MISMATCH with details.current carrying the current on-disk hash; pass dry_run=true to bypass the check as a recovery path."
    )]
    pub expected_hash: String,
    #[schemars(
        description = "Section fields to set (replaces content): { \"identity\": \"new content\" }"
    )]
    pub sections: Option<IndexMap<String, String>>,
    #[schemars(description = "Section fields to append to: { \"specifies\": \"extra content\" }")]
    pub append_sections: Option<IndexMap<String, String>>,
    #[schemars(
        description = "Section fields to patch (find-and-replace): { \"specifies\": { \"old\": \"...\", \"new\": \"...\" } }"
    )]
    pub patch_sections: Option<IndexMap<String, PatchInput>>,
    #[schemars(description = "Metadata fields to set: { \"level\": \"M1\" }")]
    pub metadata: Option<IndexMap<String, String>>,
    #[schemars(
        description = "Metadata keys to remove. Silent no-op if absent. Errors on read-only fields (mem, id, type, plus the engine-stamped created_date / last_modified) and on schema-required fields. Cannot overlap with `metadata` keys — pass one or the other per key."
    )]
    pub metadata_unset: Option<Vec<String>>,
    #[schemars(
        description = "Validate and preview what would change without executing. On a valid update the response carries the unchanged on-disk hash as `_hash` plus the post-write `prospective_hash` — pass `_hash` as `expected_hash` on the follow-up real call. `dry_run` deliberately bypasses ONLY the `expected_hash` check (the returned `_hash` is the current on-disk hash, safe to reuse on the real follow-up), making it the designated recovery path for stale hashes. It does NOT relax the rest of validation: an update that a real call would refuse on section/field grounds (`UNKNOWN_SECTION`, `UNKNOWN_METADATA_FIELD`, `INVALID_ENUM_VALUE`, `REQUIRED_FIELD_UNSET`, `PATCH_OLD_NOT_FOUND`, …) refuses under dry_run with the same typed envelope and the same recovery `details.*` — that refusal is the pre-flight signal, not a clean preview. So dry_run and a real write agree on validity (modulo the intentionally-skipped hash check)."
    )]
    pub dry_run: Option<bool>,
    #[schemars(
        description = "Atomic batched relation declarations applied before the section/metadata changes land. Each `{ to, type }` is validated like a `memstead_relate` call (schema-shape, cross-mem policy, target-id grammar) and appended to the entity's relations; absent Write-mem targets are auto-stubbed identically to the relate path. The strict wiki-link/relation validator then runs against the post-mutation state with the freshly-declared relations in place — so adding a `[[target]]` body wiki-link + declaring the backing `REFERENCES` relation can land in a single `memstead_update` call (without `declare_relations`, the post-migration strict validator would refuse the body link). Each successful entry is echoed in `relations_declared` on the response with `target_was_stubbed` flagging whether the target was absent at call time. Omit for mutations that don't introduce new relations."
    )]
    pub declare_relations: Option<Vec<RelationInput>>,
    #[schemars(
        description = "Repair-shaped relation removals `[{ rel_type, target }]`, applied atomically within this update. Accepted only when the entity currently FAILS the conformance check (see memstead_health include=conformance) — on a conformant entity the call refuses with REPAIR_NOT_NEEDED and the entity is unmodified; use memstead_relate(remove=true) for everyday edge detachment. Absent pairs are silent no-ops (symmetric with metadata_unset). The strict-write post-condition is unchanged: the post-repair entity must validate or the whole update refuses with the relevant write-time code. During a schema migration every not-yet-repaired entity is non-conformant against the target, so this param works on exactly those entities with no mode flag."
    )]
    pub relations_unset: Option<Vec<RelationUnsetInput>>,
    #[schemars(description = NOTE_PARAM_DESCRIPTION)]
    pub note: Option<String>,
}

/// One `relations_unset` entry — `{ rel_type, target }`.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelationUnsetInput {
    #[schemars(description = "Relationship type of the edge to remove (canonical UPPER_SNAKE_CASE; case-insensitive input accepted)")]
    pub rel_type: String,
    #[schemars(description = "Full target entity ID of the edge to remove")]
    pub target: String,
}

/// Find-and-replace input.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchInput {
    #[schemars(description = "Exact substring to find in current content")]
    pub old: String,
    #[schemars(description = "Replacement (empty string = delete)")]
    pub new: String,
    #[schemars(
        description = "Replace every occurrence of `old` when true; replace only the first when false or omitted. Literal match, case-sensitive."
    )]
    pub all: Option<bool>,
}

/// Parameters for memstead_relate.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelateParams {
    #[schemars(description = "Full source entity ID")]
    pub from: String,
    #[schemars(description = "Full target entity ID")]
    pub to: String,
    #[schemars(
        description = "Relationship type. Canonical form is UPPER_SNAKE_CASE (USES, PART_OF, DEPENDS_ON) and is what the engine stores; case-insensitive inputs (`uses`, `Part_Of`) are accepted and echoed back in the response as their canonical form. The JSON Schema `pattern` advertises `^[A-Za-z][A-Za-z_]*$` for client-side validators; the engine enforces the same character set independently — characters outside it return `INVALID_REL_TYPE` at the engine boundary regardless of whether the client pre-filters.",
        regex(pattern = r"^[A-Za-z][A-Za-z_]*$")
    )]
    pub r#type: String,
    #[schemars(description = "Set true to remove the relationship instead of creating it")]
    pub remove: Option<bool>,
    #[schemars(
        description = "Optional per-edge description applied on add. Validated against the rel-type's `per_edge_description` posture in the pinned schema: `forbidden` (default) rejects a non-empty description with `DESCRIPTION_NOT_PERMITTED`; `required` rejects its absence with `MISSING_REQUIRED_DESCRIPTION`; `optional` accepts both. Empty / whitespace-only strings normalise to absent before validation. Ignored on the remove path."
    )]
    #[serde(default)]
    pub description: Option<String>,
    #[schemars(description = NOTE_PARAM_DESCRIPTION)]
    pub note: Option<String>,
}

/// Parameters for memstead_delete.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteParams {
    #[schemars(description = "Full entity ID to delete")]
    pub id: String,
    #[schemars(
        description = "Hash from memstead_entity response (_hash field). Required for real entities — read first. Mirrors memstead_update / memstead_rename. Mismatch returns code HASH_MISMATCH with details.current carrying the current on-disk hash. Stubs carry an empty `_hash` (they have no on-disk file); pass the empty string to delete a stub — the hash check is skipped because there is nothing to compare."
    )]
    pub expected_hash: String,
    #[schemars(description = NOTE_PARAM_DESCRIPTION)]
    pub note: Option<String>,
}

/// Parameters for memstead_rename.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameParams {
    #[schemars(description = "Full current entity ID")]
    pub id: String,
    #[schemars(description = "New title for the entity")]
    pub new_title: String,
    #[schemars(
        description = "Hash from memstead_entity (_hash). Required. Mismatch returns code HASH_MISMATCH with details.current carrying the current on-disk hash."
    )]
    pub expected_hash: String,
    #[schemars(description = NOTE_PARAM_DESCRIPTION)]
    pub note: Option<String>,
}

