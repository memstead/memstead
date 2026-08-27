//! Parameter structs for mutation (write) tools.

use indexmap::IndexMap;
use rmcp::schemars;

/// Shared `note` description rendered identically on every mutation-tool
/// parameter. One sentence, ≤280 chars, agent-authored — it lands in the
/// commit body between the subject and the provenance trailers, and is
/// what outer-repo session-bundling hooks aggregate per session.
pub(crate) const NOTE_PARAM_DESCRIPTION: &str = "Agent-authored provenance note (≤280 chars, one sentence describing \
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
    #[schemars(
        description = "Initial relationships, wired in the same call that creates the entity. An entry is literally `{ \"target\": \"<mem>--<slug>\", \"rel_type\": \"REL_TYPE\", \"description\": \"…\" }` — `description` optional, and there is no `from`: the entity being created is the source, so the far end is `target`. (`memstead_relate` names both ends: `{from, to, rel_type, …}`.) An unresolved `target` auto-creates a stub."
    )]
    pub relations: Option<Vec<RelationInput>>,
    #[schemars(
        description = "Optional provenance anchors to attach to the new entity — durable records tying it to the source artifacts it describes (which artifact, at which grain, under which provenance class). Written into the mem-branch anchors sidecar in the SAME commit as the entity (atomic); omitting it is byte-identical to a create without anchors. Anchor writes MERGE: later `memstead_update` calls carrying `anchors` add to this set (same `(artifact, grain, class)` triple replaces, otherwise appends) and never silently discard it — removal is explicit via `memstead_update`'s `anchors_unset`. A malformed element refuses the whole create with `INVALID_ANCHOR` (`details` carries the offending field + allowed set) and the entity is not written. A payload naming one `(artifact, grain, class)` triple TWICE refuses: that triple is one row, so the repeats would collapse to the last one unannounced. A `span` anchor's locator must be usable (`#L<start>-L<end>`, `#<unit-key>`, or no locator for the whole file); one that addresses nothing refuses, and a span the engine could not check (no `content` supplied) is accepted and recorded as unverified. Anchors do NOT participate in `_hash`."
    )]
    #[serde(default)]
    pub anchors: Option<Vec<AnchorInputParam>>,
    #[schemars(
        description = "Validate and preview the create without executing — no disk write, no store mutation, no VCS commit, no edges added. dry_run runs the SAME validation a real call runs; it is not a softer check. On a VALID entity the response carries the prospective `id`, `file_path`, and `_hash` (bit-identical to what a real call with the same arguments would produce, EXCEPT for engine-auto-stamped timestamps: the hash covers `created_date`, which is stamped from wall-clock `now()` independently in the dry-run and the real call, so the two `_hash` values diverge whenever a second ticks between them; the hash also covers `sections`, `metadata`, and `relations`, so a dry_run that omits `relations` will not match a real call that supplies them; `_hash` does NOT cover `anchors` — the anchors sidecar persists on the mem branch under `.memstead/` and is never folded into content hashing, so attaching or refreshing anchors never changes `_hash` or invalidates a cached `expected_hash`), plus any `warnings` and any `incoming` edges that would be adopted from a pre-existing stub at this id, with `write_id` empty. On an INVALID entity dry_run does NOT return a warnings-list preview: it refuses with the IDENTICAL typed envelope a real call would return (`MISSING_REQUIRED_SECTION`, `UNKNOWN_SECTION`, `UNKNOWN_METADATA_FIELD`, `INVALID_ENUM_VALUE`, `INVALID_FIELD_VALUE`, `REQUIRED_FIELD_UNSET`, …), carrying the same recovery `details.*` (e.g. `details.sections[]`). That typed refusal IS the pre-flight signal — read its `details` to fix coverage, then retry. So dry_run never reports a problem entity as clean: it and a real write agree on validity. Use to verify the id slug, or to pre-flight required-section / field coverage and pre-existing references before committing."
    )]
    pub dry_run: Option<bool>,
    #[schemars(description = NOTE_PARAM_DESCRIPTION)]
    pub note: Option<String>,
    #[schemars(
        description = "The role this mutation is performed in, from the closed vocabulary `author` | `checker` | `verifier` (agent-trust plan 13). Recorded immutably alongside the mutation (commit trailer / ledger) — caller-declared but tamper-evident: bound to this operation in append-only history, it cannot be edited afterwards and identities can be cross-checked across operations. Omit to record the session default (or unspecified — legal forever, never refused, treated downstream as cannot-confirm). An unknown value refuses INVALID_ROLE naming the vocabulary."
    )]
    pub role: Option<String>,
}

/// One `anchors[]` element on `memstead_create` / `memstead_update` — a
/// provenance record tying the entity to a source artifact. Permissive by
/// design: every field is optional / string-typed so a malformed element
/// (unknown class or grain, missing artifact, hash on a non-hash class,
/// grain the medium's namespace cannot express) refuses the whole mutation
/// with a typed `INVALID_ANCHOR` envelope carrying recovery `details` —
/// rather than an opaque schema-deserialisation error. Converts to the
/// engine's `AnchorInput` which validates it. Not folded into `_hash`.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnchorInputParam {
    #[schemars(
        description = "Artifact reference in the medium's own namespace — a repo-relative path, `path@commit`, URL, or entity id, interpreted per `grain`. Required; a missing/empty value refuses INVALID_ANCHOR."
    )]
    #[serde(default)]
    pub artifact: Option<String>,
    #[schemars(
        description = "Granularity of the artifact reference: `span` | `file` | `tree` | `url` | `entity`. Must be expressible in the resolving medium's anchor namespace (path-shaped for span/file/tree, `url` for url, `entity` for entity) or the mutation refuses INVALID_ANCHOR. An unknown value refuses INVALID_ANCHOR."
    )]
    #[serde(default)]
    pub grain: Option<String>,
    #[schemars(
        description = "Provenance class — the entity's epistemic standing toward the artifact: `anchored` | `derived` | `authored` | `informed-by`. `anchored`/`derived` carry hash semantics (a `hash` is permitted and participates in drift adjudication); `authored`/`informed-by` do not (supplying `hash` refuses INVALID_ANCHOR). An unknown value refuses INVALID_ANCHOR."
    )]
    #[serde(default)]
    pub class: Option<String>,
    #[schemars(
        description = "Medium-typed pinned version this anchor was recorded against: `{ kind: \"commit\"|\"snapshot\"|\"etag\", value: \"<token>\" }`. Omit for a plain-path medium with no retrievable version."
    )]
    #[serde(default)]
    pub at_version: Option<AnchorVersionParam>,
    #[schemars(
        description = "Content hash over the PREPARED artifact form (never raw bytes). Permitted only on hash-bearing classes (`anchored`/`derived`); supplying it on `authored`/`informed-by` refuses INVALID_ANCHOR."
    )]
    #[serde(default)]
    pub hash: Option<String>,
    #[schemars(
        description = "The observed artifact CONTENT (UTF-8 text), for the engine to compute `hash` from through its preparation registry — the write-time observation for a grain the engine cannot observe itself: a `url` anchor (the engine never fetches; what you read is canonicalized exactly as a path grain's bytes are). Also accepted for `span`/`file`. Mutually exclusive with `hash` (both refuses INVALID_ANCHOR); refused on `authored`/`informed-by`, and on the `entity`/`tree` grains, whose prepared form is never computed from supplied bytes."
    )]
    #[serde(default)]
    pub content: Option<String>,
    #[schemars(
        description = "Medium's declared hash stability: `stable` | `unstable` (defaults per grain: `url` to `unstable`, every other grain to `stable`). An unstable-source hash break resolves `recheck`, not `drifted`."
    )]
    #[serde(default)]
    pub hash_stability: Option<String>,
    #[schemars(
        description = "For a `derived` class: the input artifact refs the entity was derived from. Empty/omitted for every other class."
    )]
    #[serde(default)]
    pub derived_from: Option<Vec<String>>,
    #[schemars(
        description = "`hash(D)` of the binding that produced this anchor, when a binding produced it. Omit for a manually-authored anchor."
    )]
    #[serde(default)]
    pub binding: Option<String>,
    #[schemars(
        description = "NAME of the source (as declared in the producing binding's `sources[]`) that produced this anchor — lets a discovery run be measured per entry point. Name the source you are working from whenever the binding declares more than one. Present-but-empty refuses INVALID_ANCHOR; a name the (resolvable) producing binding does not declare refuses with the declared names in `details.declared`. Omit for a manually-authored anchor."
    )]
    #[serde(default)]
    pub source: Option<String>,
}

/// The medium-typed pinned version sub-object of an [`AnchorInputParam`].
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnchorVersionParam {
    #[schemars(
        description = "Version kind: `commit` (git / path+commit) | `snapshot` (graph) | `etag` (web)."
    )]
    pub kind: String,
    #[schemars(description = "The version token — commit id, graph snapshot token, or web ETag.")]
    pub value: String,
}

impl AnchorInputParam {
    /// Lower the permissive wire element into the engine's `AnchorInput`
    /// (which performs the typed `INVALID_ANCHOR` validation). An
    /// unrecognised `at_version.kind` is dropped (best-effort — version
    /// kind is not part of the anchor-validation contract this cut fires).
    pub(crate) fn into_engine(self) -> memstead_base::anchor::AnchorInput {
        use memstead_base::anchor::AnchorVersion;
        let at_version = self.at_version.and_then(|v| match v.kind.as_str() {
            "commit" => Some(AnchorVersion::Commit(v.value)),
            "snapshot" => Some(AnchorVersion::Snapshot(v.value)),
            "etag" => Some(AnchorVersion::Etag(v.value)),
            _ => None,
        });
        memstead_base::anchor::AnchorInput {
            artifact: self.artifact,
            grain: self.grain,
            source: self.source,
            class: self.class,
            at_version,
            hash: self.hash,
            content: self.content,
            hash_stability: self.hash_stability,
            derived_from: self.derived_from,
            binding: self.binding,
        }
    }
}

/// A relationship input for create/batch tools.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelationInput {
    #[schemars(description = "Full target entity ID")]
    pub target: String,
    #[schemars(
        description = "Relationship type. Canonical form is UPPER_SNAKE_CASE (USES, PART_OF, DEPENDS_ON) and is what the engine stores; case-insensitive inputs (`uses`, `Part_Of`) are accepted and echoed back in the response as their canonical form. The JSON Schema `pattern` advertises `^[A-Za-z][A-Za-z_]*$` for client-side validators; the engine enforces the same character set independently — characters outside it return `INVALID_REL_TYPE` at the engine boundary regardless of whether the client pre-filters.",
        regex(pattern = r"^[A-Za-z][A-Za-z_]*$")
    )]
    pub rel_type: String,
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
        description = "Hash from memstead_entity response (_hash field). Required for any update that changes content (sections, metadata, relations) — read the entity first. OMIT it for an anchors-only update (`anchors` / `anchors_unset` and nothing else): the anchors sidecar is outside `_hash` by design, so the token would compare a value the write cannot move, and requiring it taxed exactly the backfill flows anchors exist for. An update that changes content without it refuses `EXPECTED_HASH_REQUIRED`. Mismatch returns code HASH_MISMATCH with details.current carrying the current on-disk hash; pass dry_run=true to bypass the check as a recovery path."
    )]
    #[serde(default)]
    pub expected_hash: Option<String>,
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
        description = "Metadata keys to remove. Silent no-op if absent. Errors on the engine-stamped timestamp fields (created_date / last_modified) and on schema-required fields. The reserved identity triple (mem / id / type) is asymmetric by design: SET refuses (READ_ONLY_FIELD, here and on create) but UNSET is allowed — the sanctioned repair for an entity that acquired a smuggled reserved key before the write gates closed. Unsetting `type` never leaves the entity typeless (the engine re-seeds the authoritative discriminator; on a healthy entity it is a no-op). Cannot overlap with `metadata` keys — pass one or the other per key."
    )]
    pub metadata_unset: Option<Vec<String>>,
    #[schemars(
        description = "Validate and preview what would change without executing. On a valid update the response carries the unchanged on-disk hash as `_hash` plus the post-write `prospective_hash` — pass `_hash` as `expected_hash` on the follow-up real call. `dry_run` deliberately bypasses ONLY the `expected_hash` check (the returned `_hash` is the current on-disk hash, safe to reuse on the real follow-up), making it the designated recovery path for stale hashes. It does NOT relax the rest of validation: an update that a real call would refuse on section/field grounds (`UNKNOWN_SECTION`, `UNKNOWN_METADATA_FIELD`, `INVALID_ENUM_VALUE`, `INVALID_FIELD_VALUE`, `REQUIRED_FIELD_UNSET`, `PATCH_OLD_NOT_FOUND`, …) refuses under dry_run with the same typed envelope and the same recovery `details.*` — that refusal is the pre-flight signal, not a clean preview. So dry_run and a real write agree on validity (modulo the intentionally-skipped hash check)."
    )]
    pub dry_run: Option<bool>,
    #[schemars(
        description = "Atomic batched relation declarations applied before the section/metadata changes land. Each `{ target, rel_type }` is validated like a `memstead_relate` call (schema-shape, cross-mem policy, target-id grammar) and appended to the entity's relations; absent Write-mem targets are auto-stubbed identically to the relate path. The strict wiki-link/relation validator then runs against the post-mutation state with the freshly-declared relations in place — so adding a `[[target]]` body wiki-link + declaring the backing `REFERENCES` relation can land in a single `memstead_update` call (without `declare_relations`, the post-migration strict validator would refuse the body link). Each successful entry is echoed in `relations_declared` on the response with `target_was_stubbed` flagging whether the target was absent at call time. Omit for mutations that don't introduce new relations."
    )]
    pub declare_relations: Option<Vec<RelationInput>>,
    #[schemars(
        description = "Repair-shaped relation removals `[{ rel_type, target }]`, applied atomically within this update. Accepted only when the entity currently FAILS the conformance check (see memstead_health include=conformance) — on a conformant entity the call refuses with REPAIR_NOT_NEEDED and the entity is unmodified; use memstead_relate(remove=true) for everyday edge detachment. Absent pairs are silent no-ops (symmetric with metadata_unset). The strict-write post-condition is unchanged: the post-repair entity must validate or the whole update refuses with the relevant write-time code. During a schema migration every not-yet-repaired entity is non-conformant against the target, so this param works on exactly those entities with no mode flag."
    )]
    pub relations_unset: Option<Vec<RelationUnsetInput>>,
    #[schemars(
        description = "Optional provenance anchors to attach to this entity — durable records tying it to the source artifacts it describes. Anchors MERGE into the entity's existing set: an incoming anchor replaces the existing anchor with the same `(artifact, grain, class)` triple and appends otherwise — writing anchors never removes an anchor this call did not name in `anchors_unset` (an empty or omitted list leaves the stored set untouched; incremental anchoring works). Written into the mem-branch anchors sidecar in the SAME commit as the update (atomic). An update carrying only `anchors` (no section/metadata change) still commits the sidecar. A malformed element refuses the whole update with `INVALID_ANCHOR` and nothing is written. A payload naming one `(artifact, grain, class)` triple TWICE refuses (`INVALID_ANCHOR`): that triple is one row, so the repeats would collapse to the last one and an anchor you sent would vanish unannounced. A re-pin that omits `hash` KEEPS the stored baseline rather than dropping it; supply `hash` to replace it, or `anchors_unset` the row and write it fresh to clear it. A `span` anchor's locator must be usable (`#L<start>-L<end>`, `#<unit-key>`, or no locator for the whole file); a locator that addresses nothing refuses, and a span the engine could not check (no `content` supplied) is accepted and recorded as unverified. Anchors do NOT participate in `_hash`."
    )]
    #[serde(default)]
    pub anchors: Option<Vec<AnchorInputParam>>,
    #[schemars(
        description = "Explicit anchor removals, applied BEFORE the `anchors` merge in the same mutation (mirroring `metadata_unset` / `relations_unset`) — removal is explicit, never a side effect of writing. Each entry names an `artifact` and may narrow by `grain` and/or `class`; a bare artifact removes every anchor on it. Unsetting an anchor that does not exist is a no-op, not an error. Full-replace stays expressible: unset the artifact(s) and write the new set in one call. A malformed selector (missing artifact, unknown grain/class) refuses the whole update with `INVALID_ANCHOR`."
    )]
    #[serde(default)]
    pub anchors_unset: Option<Vec<AnchorUnsetParam>>,
    #[schemars(description = NOTE_PARAM_DESCRIPTION)]
    pub note: Option<String>,
    #[schemars(
        description = "The role this mutation is performed in, from the closed vocabulary `author` | `checker` | `verifier` (agent-trust plan 13). Recorded immutably alongside the mutation (commit trailer / ledger) — caller-declared but tamper-evident: bound to this operation in append-only history, it cannot be edited afterwards and identities can be cross-checked across operations. Omit to record the session default (or unspecified — legal forever, never refused, treated downstream as cannot-confirm). An unknown value refuses INVALID_ROLE naming the vocabulary."
    )]
    pub role: Option<String>,
}

/// One `anchors_unset[]` entry on `memstead_update` — an explicit anchor-
/// removal selector. Permissive like [`AnchorInputParam`]: a malformed
/// selector refuses the whole mutation with a typed `INVALID_ANCHOR`
/// envelope rather than an opaque schema-deserialisation error.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnchorUnsetParam {
    #[schemars(
        description = "Artifact reference whose anchors to remove, exactly as stored. Required; a missing/empty value refuses INVALID_ANCHOR. Bare (no grain/class) removes every anchor on the artifact."
    )]
    #[serde(default)]
    pub artifact: Option<String>,
    #[schemars(
        description = "Optional narrowing: only remove anchors of this grain (`span` | `file` | `tree` | `url` | `entity`). An unknown value refuses INVALID_ANCHOR."
    )]
    #[serde(default)]
    pub grain: Option<String>,
    #[schemars(
        description = "Optional narrowing: only remove anchors of this provenance class (`anchored` | `derived` | `authored` | `informed-by`). An unknown value refuses INVALID_ANCHOR."
    )]
    #[serde(default)]
    pub class: Option<String>,
}

impl AnchorUnsetParam {
    /// Lower the permissive wire element into the engine's
    /// `AnchorUnsetInput` (which performs the typed `INVALID_ANCHOR`
    /// validation).
    pub(crate) fn into_engine(self) -> memstead_base::anchor::AnchorUnsetInput {
        memstead_base::anchor::AnchorUnsetInput {
            artifact: self.artifact,
            grain: self.grain,
            class: self.class,
        }
    }
}

/// One `relations_unset` entry — `{ rel_type, target }`.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelationUnsetInput {
    #[schemars(
        description = "Relationship type of the edge to remove (canonical UPPER_SNAKE_CASE; case-insensitive input accepted)"
    )]
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

/// One relation operation in `memstead_relate`'s list.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelateOpInput {
    #[schemars(description = "Full source entity ID")]
    pub from: String,
    #[schemars(description = "Full target entity ID")]
    pub to: String,
    #[schemars(
        description = "Relationship type. Canonical form is UPPER_SNAKE_CASE (USES, PART_OF, DEPENDS_ON) and is what the engine stores; case-insensitive inputs (`uses`, `Part_Of`) are accepted and echoed back in the response as their canonical form. The JSON Schema `pattern` advertises `^[A-Za-z][A-Za-z_]*$` for client-side validators; the engine enforces the same character set independently — characters outside it return `INVALID_REL_TYPE` at the engine boundary regardless of whether the client pre-filters.",
        regex(pattern = r"^[A-Za-z][A-Za-z_]*$")
    )]
    pub rel_type: String,
    #[schemars(description = "Set true to remove the relationship instead of creating it")]
    pub remove: Option<bool>,
    #[schemars(
        description = "Optional per-edge description applied on add. Validated against the rel-type's `per_edge_description` posture in the pinned schema: `forbidden` (default) rejects a non-empty description with `DESCRIPTION_NOT_PERMITTED`; `required` rejects its absence with `MISSING_REQUIRED_DESCRIPTION`; `optional` accepts both. Empty / whitespace-only strings normalise to absent before validation. Ignored on the remove path."
    )]
    #[serde(default)]
    pub description: Option<String>,
}

/// Parameters for memstead_relate — a list of relation operations
/// applied atomically. The single-relation call is a list of one.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelateParams {
    #[schemars(
        description = "Relation operations, applied atomically in order — all-or-nothing in one commit per touched mem. Each entry is `{from, to, rel_type, remove?, description?}` with per-entry validation identical to a single call; later entries validate against the graph state produced by earlier ones (an acyclic check sees edges added earlier in the list). A single failing entry refuses the WHOLE list and the refusal reports every failing entry."
    )]
    pub relations: Vec<RelateOpInput>,
    #[schemars(description = NOTE_PARAM_DESCRIPTION)]
    pub note: Option<String>,
    #[schemars(
        description = "The role this mutation is performed in, from the closed vocabulary `author` | `checker` | `verifier` (agent-trust plan 13). Recorded immutably alongside the mutation (commit trailer / ledger) — caller-declared but tamper-evident: bound to this operation in append-only history, it cannot be edited afterwards and identities can be cross-checked across operations. Omit to record the session default (or unspecified — legal forever, never refused, treated downstream as cannot-confirm). An unknown value refuses INVALID_ROLE naming the vocabulary."
    )]
    pub role: Option<String>,
    #[schemars(
        description = "Validate and preview the relation operations without executing — no edge lands, no stub is created, no VCS commit. dry_run runs the SAME validation a real call runs (cross-mem policy, vocabulary, description posture, acyclicity, self-loop refusal); an illegal operation refuses with the IDENTICAL typed envelope a real call would return, and a legal one reports the would-be action with `_hash` set to the PROSPECTIVE post-write source hash, `write_id` empty (the rehearsal marker), and any would-be `AUTO_STUB_CREATED` warning for an absent target — reported, never created. The follow-up real call on an unchanged mem succeeds; like create's dry_run, its `_hash` diverges from the rehearsed one whenever a wall-clock second ticks between the calls (the auto-stamped `last_modified` enters the hash) — a timestamp shift, not drift."
    )]
    pub dry_run: Option<bool>,
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
    #[schemars(
        description = "The role this mutation is performed in, from the closed vocabulary `author` | `checker` | `verifier` (agent-trust plan 13). Recorded immutably alongside the mutation (commit trailer / ledger) — caller-declared but tamper-evident: bound to this operation in append-only history, it cannot be edited afterwards and identities can be cross-checked across operations. Omit to record the session default (or unspecified — legal forever, never refused, treated downstream as cannot-confirm). An unknown value refuses INVALID_ROLE naming the vocabulary."
    )]
    pub role: Option<String>,
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
    #[schemars(
        description = "The role this mutation is performed in, from the closed vocabulary `author` | `checker` | `verifier` (agent-trust plan 13). Recorded immutably alongside the mutation (commit trailer / ledger) — caller-declared but tamper-evident: bound to this operation in append-only history, it cannot be edited afterwards and identities can be cross-checked across operations. Omit to record the session default (or unspecified — legal forever, never refused, treated downstream as cannot-confirm). An unknown value refuses INVALID_ROLE naming the vocabulary."
    )]
    pub role: Option<String>,
}

/// Parameters for `memstead_check` — the check operation
/// (agent-trust plan 14). Deliberately minimal: entity, verdict,
/// optional method note, optional role, optional kind.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CheckParams {
    #[schemars(description = "Full entity id (`mem--slug`) of the entity that was checked")]
    pub entity: String,
    #[schemars(
        description = "The verdict, from the closed vocabulary `ok` | `failed`. Nuance goes in `method` or in process-mem entities — an unknown value refuses `INVALID_VERDICT` naming the vocabulary."
    )]
    pub verdict: String,
    #[schemars(
        description = "Optional free-text method note — how the check was performed (e.g. \"diffed against source spec\")."
    )]
    pub method: Option<String>,
    #[schemars(
        description = "The role this check is performed in, from the closed vocabulary `author` | `checker` | `verifier`. Recorded immutably on the check record — same trust model as mutation roles: caller-declared but tamper-evident. Omit to record the session default (or unspecified — legal, but an unspecified-role check cannot confirm independence downstream)."
    )]
    pub role: Option<String>,
    #[schemars(
        description = "The check kind, from the closed vocabulary `verification` | `conformance`. Omit for `verification` — exactly today's behaviour. `conformance` records a semantic judgment (\"does this entity satisfy its type's schema prose\"): the engine stamps the mem's schema pin into the record (never caller-supplied), and the verdict derives stale when the content hash moves OR the pin changes; a mem with no pin refuses `INVALID_INPUT`. State derives per (entity, kind) — the kinds never supersede each other. An unknown value refuses `INVALID_CHECK_KIND` naming the vocabulary."
    )]
    pub kind: Option<String>,
}
