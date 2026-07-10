//! Engine mutation entrypoints — split per mutation kind.
//!
//! Each sub-module implements one mutation of `Engine`: `create`,
//! `update` (with batch), `delete`, `relate`, `rename`. The shared
//! helpers (`today_iso`, `make_stub`, `gc_orphan_stubs`,
//! `lookup_title_and_type`, `unknown_type_error`) plus the typed
//! constants `PATCH_OLD_NOT_FOUND_CONTENT_CAP` and
//! `RELATIONSHIP_CYCLE_PATH_CAP` live here.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::entity::{Entity, EntityId};
use crate::store::Store;

use super::EngineError;

pub mod create;
pub mod delete;
pub mod parse_recovery;
pub mod relate;
pub mod rename;
pub mod update;

/// Look up an entity's `(title, entity_type)` pair in `store`. Both
/// `None` for missing-from-store ids — matches full's `title_for` /
/// `type_for` lossy-lookup contract. Used by [`Engine::changes_since`]
/// to enrich id-only envelopes the backend returned with metadata
/// from the in-memory store.
pub(super) fn lookup_title_and_type(
    store: &Store,
    id: &EntityId,
) -> (Option<String>, Option<String>) {
    match store.get(id) {
        Some(e) => (Some(e.title.clone()), Some(e.entity_type.clone())),
        None => (None, None),
    }
}

/// Maximum byte length of the truncated `current_content` snapshot
/// that [`EngineError::PatchOldNotFound`] carries. Keeps the wire
/// envelope bounded for sections with large bodies. Mirrors full's
/// `memstead_git_branch::PATCH_OLD_NOT_FOUND_CONTENT_CAP`.
pub const PATCH_OLD_NOT_FOUND_CONTENT_CAP: usize = 500;

/// Maximum number of entity IDs retained in
/// [`EngineError::RelationshipCycle::existing_path`]. Keeps the cycle
/// envelope bounded for pathologically long chains. Mirrors full's
/// `memstead_git_branch::RELATIONSHIP_CYCLE_PATH_CAP`.
pub const RELATIONSHIP_CYCLE_PATH_CAP: usize = 20;

/// Build an [`EngineError::UnknownType`] populated with the schema's
/// declared type names (sorted) and a fuzzy suggestion. Mirrors full's
/// `UnknownEntityType` recovery payload so MCP envelopes carry the
/// same `name` / `schema_ref` / `declared` / `suggestion` keys
/// regardless of which engine served the call.
pub(crate) fn unknown_type_error(schema: &memstead_schema::Schema, attempted: &str) -> EngineError {
    let mut declared: Vec<String> = schema.types.keys().cloned().collect();
    declared.sort();
    let (sname, sver) = schema.id();
    EngineError::UnknownType {
        name: attempted.to_string(),
        schema_ref: format!("{sname}@{sver}"),
        declared,
        suggestion: schema.suggest_type(attempted),
    }
}

/// The lowercase wire string for a [`crate::pipeline::MediumType`] — the
/// value the `INVALID_ANCHOR` recovery detail carries so an agent sees
/// which medium's namespace rejected the grain. Matches the enum's
/// `#[serde(rename_all = "lowercase")]` form.
pub(crate) fn medium_type_wire(t: crate::pipeline::MediumType) -> &'static str {
    use crate::pipeline::MediumType::*;
    match t {
        Codebase => "codebase",
        Filesystem => "filesystem",
        Graph => "graph",
        Git => "git",
        Web => "web",
    }
}

impl super::Engine {
    /// Resolve the single-medium anchor-namespace context for `mem`, when
    /// unambiguous. An `anchors[]` element carries no medium name, so the
    /// grain/namespace refusal ([`crate::anchor::AnchorValidationError::GrainNamespaceUnsupported`])
    /// can only be fired deterministically when the mem declares exactly
    /// one medium; with zero or several the namespace check is skipped
    /// (the vocabulary + hash-semantics rules still apply). Returns the
    /// `(medium_type_wire, anchor_namespace)` pair the validator consumes.
    pub(crate) fn resolve_anchor_medium(&self, mem: &str) -> Option<(String, &'static str)> {
        let mut mediums = self
            .pipeline_configs()
            .mediums
            .iter()
            .filter(|r| r.mem == mem);
        let first = mediums.next()?;
        if mediums.next().is_some() {
            // Ambiguous — the anchor does not name which medium it targets;
            // skip the namespace refinement rather than guess.
            return None;
        }
        let caps = crate::binding::medium_capabilities(first.config.medium_type);
        Some((
            medium_type_wire(first.config.medium_type).to_string(),
            caps.anchor_namespace,
        ))
    }

    /// Validate the permissive `anchors[]` inputs for a mutation against
    /// `mem`'s medium context into strict [`crate::anchor::Anchor`]s, or
    /// refuse the whole mutation with a typed
    /// [`EngineError::InvalidAnchor`]. Empty input yields an empty vec (no
    /// sidecar write); a single malformed element aborts before any state
    /// change so the entity is never written.
    pub(crate) fn validate_anchor_inputs(
        &self,
        mem: &str,
        inputs: &[crate::anchor::AnchorInput],
    ) -> Result<Vec<crate::anchor::Anchor>, EngineError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let medium = self.resolve_anchor_medium(mem);
        let medium_ref = medium.as_ref().map(|(t, ns)| (t.as_str(), *ns));
        inputs
            .iter()
            .map(|i| i.validate(medium_ref).map_err(EngineError::from))
            .collect()
    }
}

/// Stage a write of `entity_id`'s anchors into the mem's anchors sidecar
/// through `backend`, merged over the existing sidecar and buffered into
/// the SAME pending op set the entity write used — so the next
/// [`crate::backend::MemBackend::commit`] carries entity + anchors as one
/// atomic commit. An empty `anchors` vec prunes the entity's row (the
/// delete / rename-residual legs lean on this). Reads honour pending-buffer
/// precedence, so successive stages within one transaction compose.
pub(crate) fn stage_anchors_sidecar(
    backend: &dyn crate::backend::MemBackend,
    entity_id: &EntityId,
    anchors: Vec<crate::anchor::Anchor>,
) -> Result<(), EngineError> {
    let mut sidecar = match backend.read_anchors_sidecar()? {
        Some(bytes) => crate::anchor::AnchorSidecar::from_bytes(&bytes).map_err(|e| {
            EngineError::Backend(crate::backend::BackendError::Other(format!(
                "anchors sidecar parse: {e}"
            )))
        })?,
        None => crate::anchor::AnchorSidecar::default(),
    };
    sidecar.set(entity_id.as_ref(), anchors);
    backend.write_anchors_sidecar(&sidecar.to_bytes())?;
    Ok(())
}

/// Load the mem's anchors sidecar through `backend`, or the empty
/// document when none exists yet. Shared by the delete / rename legs
/// which must decide whether the entity actually has anchor rows before
/// staging a sidecar write (so an entity with none stays byte-identical
/// to a pre-anchor mutation).
fn read_sidecar(
    backend: &dyn crate::backend::MemBackend,
) -> Result<crate::anchor::AnchorSidecar, EngineError> {
    match backend.read_anchors_sidecar()? {
        Some(bytes) => crate::anchor::AnchorSidecar::from_bytes(&bytes).map_err(|e| {
            EngineError::Backend(crate::backend::BackendError::Other(format!(
                "anchors sidecar parse: {e}"
            )))
        }),
        None => Ok(crate::anchor::AnchorSidecar::default()),
    }
}

/// Stage removal of `entity_id`'s anchor row into the same commit as an
/// entity delete — a no-op (no sidecar write, so byte-identical to today)
/// when the entity carries no anchors. Returns whether a write was staged.
pub(crate) fn stage_anchors_removal(
    backend: &dyn crate::backend::MemBackend,
    entity_id: &EntityId,
) -> Result<bool, EngineError> {
    let mut sidecar = read_sidecar(backend)?;
    if sidecar.get(entity_id.as_ref()).is_empty() {
        return Ok(false);
    }
    sidecar.remove(entity_id.as_ref());
    backend.write_anchors_sidecar(&sidecar.to_bytes())?;
    Ok(true)
}

/// Stage a move of `from`'s anchor row to `to` into the same commit as an
/// entity rename — leaving zero rows under the old id. A no-op (byte-
/// identical to today) when the renamed entity carries no anchors. Returns
/// whether a write was staged.
pub(crate) fn stage_anchors_rename(
    backend: &dyn crate::backend::MemBackend,
    from: &EntityId,
    to: &EntityId,
) -> Result<bool, EngineError> {
    let mut sidecar = read_sidecar(backend)?;
    if sidecar.get(from.as_ref()).is_empty() {
        return Ok(false);
    }
    sidecar.rename(from.as_ref(), to.as_ref());
    backend.write_anchors_sidecar(&sidecar.to_bytes())?;
    Ok(true)
}

/// Now as a full ISO-8601 datetime string `YYYY-MM-DDTHH:MM:SSZ`
/// (UTC). Used by mutation paths that auto-stamp metadata fields
/// (e.g. `last_modified` on update, `created_date` on create).
///
/// This is second-resolution (rather than
/// date-only `YYYY-MM-DD`) so intra-day
/// updates produce distinguishable timestamps and drift / staleness
/// queries become per-update aware. The strict-mode date validator
/// already accepts both forms (`^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}:\d{2}Z)?$`)
/// so existing entities written with the date-only form continue to
/// load; new writes carry the wider form.
///
/// Pure function: no allocation outside the `format!` invocation,
/// no error path (the system-clock fallback to UNIX epoch on a
/// clock that hasn't been set yet is acceptable for a best-effort
/// timestamp). Howard-Hinnant civil-from-days for the date half;
/// trivial modular arithmetic for the time half.
pub(super) fn today_iso() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = secs / 86400;
    let secs_of_day = secs % 86400;
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    let ss = secs_of_day % 60;
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Sweep stubs whose last incoming edge has just disappeared. Returns
/// the dropped ids so callers can surface them to the agent (e.g. via
/// [`DeleteEntityOutcome::orphan_stubs_removed`]).
///
/// Stubs are auto-created when a relate names an absent target — a
/// "promise" that a real entity will land there later (see
/// [`make_stub`]). When the last referrer drops its edge or is itself
/// deleted, the promise has no holder and becomes pure bloat. Only
/// stubs are eligible — real entities never count as orphans via this
/// path.
pub(super) fn gc_orphan_stubs(store: &mut Store) -> Vec<EntityId> {
    let stub_ids: Vec<EntityId> = store
        .all_entities()
        .filter(|e| e.stub)
        .map(|e| e.id.clone())
        .collect();
    gc_orphan_stubs_among(store, &stub_ids)
}

/// Scoped orphan-stub sweep: GC only the stubs *among `candidates`*
/// whose last incoming edge has just disappeared, returning the dropped
/// ids. This is the single home of the orphan-stub predicate (`stub &&
/// no incoming`) — the three write paths that can sever a stub's last
/// referrer all funnel through here so they cannot drift:
/// [`gc_orphan_stubs`] (delete's full-store sweep) supplies every stub
/// id; the `memstead_relate(remove)` path supplies the just-severed target;
/// the `memstead_update` alias-resync path supplies the entity's
/// pre-mutation body-link targets (the only edges that commit could
/// have dropped). Scoping to a candidate set rather than walking the
/// whole store keeps each path from GC'ing pre-existing orphans that
/// aren't its responsibility. Candidates are de-duplicated; a candidate
/// that is absent, not a stub, or still has a referrer is left
/// untouched.
pub(super) fn gc_orphan_stubs_among<'a>(
    store: &mut Store,
    candidates: impl IntoIterator<Item = &'a EntityId>,
) -> Vec<EntityId> {
    let mut removed: Vec<EntityId> = Vec::new();
    let mut seen: std::collections::HashSet<&EntityId> = std::collections::HashSet::new();
    for id in candidates {
        if !seen.insert(id) {
            continue;
        }
        if store.get(id).is_some_and(|e| e.stub) && store.incoming(id).is_empty() {
            store.remove(id);
            removed.push(id.clone());
        }
    }
    removed
}

/// Shared target-id grammar validator. The wiki-link grammar gate
/// runs on every relation-authoring path (`memstead_relate`,
/// `memstead_create.relations[]`, future inline-relation surfaces) so a
/// malformed target id (e.g. `bad@chars$here`) cannot land an
/// auto-stub at the literal id — that stub would later fail every
/// wiki-link parse that referenced it. Pre-Item-02 the gate lived
/// only on `memstead_relate`; the create path admitted the same input
/// silently.
pub(super) fn validate_relation_target_grammar(target: &EntityId) -> Result<(), EngineError> {
    if let Err(reason) = crate::entity::id::validate_mem_name_grammar(target.mem()) {
        return Err(EngineError::InvalidEntityId {
            id: target.to_string(),
            reason,
        });
    }
    if let Err(reason) = crate::entity::id::validate_id_path_grammar(target.path()) {
        return Err(EngineError::InvalidEntityId {
            id: target.to_string(),
            reason,
        });
    }
    Ok(())
}

/// Auto-stamp `auto_timestamp` metadata fields on an entity that's
/// about to be re-written. Extracted from the update-path hot loop so
/// the relate-path (add and remove) and the rename-path (the renaming
/// entity plus every referrer the rewrite cascade touched) can
/// invoke the same engine-driven stamp.
///
/// Walks the type's metadata-field declarations; any field flagged
/// `auto_timestamp: true` (the default schema declares this on
/// `last_modified`) is set to the supplied `today` ISO string. The
/// helper is a no-op on schemas that declare no auto-timestamp
/// fields. Callers pre-compute `today` via [`today_iso`] so a single
/// mutation that touches multiple entities (rename's referrer rewrite
/// cascade) stamps them all with the same value.
pub(super) fn auto_stamp_timestamps(
    entity: &mut Entity,
    type_def: &memstead_schema::TypeDefinition,
    today: &str,
) {
    for field_def in &type_def.metadata_fields {
        if field_def.auto_timestamp {
            entity.metadata.insert(
                field_def.key.clone(),
                crate::entity::MetadataValue::String(today.to_string()),
            );
        }
    }
}

/// Build a stub [`Entity`] for an unresolved relate target. Callers
/// declare the stub's origin via [`crate::entity::StubKind`] —
/// `ForwardReference` for `memstead_relate` to an absent target,
/// `Residual { since_commit, readonly_referrers }` for the
/// delete/rename demote path. The kind persists for the engine
/// instance's lifetime; a reload reduces every stub to `LoadTime`
/// — the kind is annotation, not state.
///
/// The stub is in-store but unwritten to disk — `entity_type` empty,
/// `file_path` empty, no metadata, no sections, `stub: true` and
/// `stub_kind: Some(kind)` set together. A later
/// [`Engine::create_entity`] at the same id promotes the stub to a
/// real entity (loader / parse-result merge handles the upgrade
/// path; `stub_kind` clears to `None`).
pub(super) fn make_stub(id: &EntityId, kind: crate::entity::StubKind) -> Entity {
    Entity {
        id: id.clone(),
        title: id.name().to_string(),
        entity_type: String::new(),
        mem: id.mem().to_string(),
        file_path: String::new(),
        metadata: IndexMap::new(),
        sections: IndexMap::new(),
        relationships: Vec::new(),
        content_hash: String::new(),
        stub: true,
        stub_kind: Some(kind),
        heading_spans: HashMap::new(),
    }
}

/// Cross-mem add-path policy gate. Same-mem writes bypass; the
/// `[cross_mem_links]` table only gates writes that cross the
/// mem boundary. Cross-mem writes consult
/// [`super::Engine::cross_mem_link_allowed`] in the edge's actual
/// direction (`source_mem → target_mem`). Disallowed pairings
/// surface [`EngineError::CrossMemLinkNotAllowed`] with the
/// `(from_mem, to_mem)` payload an agent already sees on
/// `memstead_relate`.
///
/// After the grant admits the pairing, a target absent from a
/// `MountCapability::ReadOnly` mount refuses with
/// [`EngineError::CrossMemTargetNotFound`]: the engine cannot
/// persist a stub through the read-only boundary, and a read-only
/// mem never gains the entity later — a missing target there is a
/// wrong link, not a pending forward reference. Same-mem targets,
/// cross-mem targets in Write mounts, and unmounted target mems all
/// retain the auto-stub mechanic.
///
/// Funnel point for every add-shaped edge write — `memstead_relate`,
/// `memstead_create.relations[]`, `memstead_update.declare_relations`,
/// body-wiki-link alias synthesis, and any future add-path mutation
/// surface route through one gate so the policy can't drift between
/// sites. Remove-shaped writes (cleanup) remain permissive and call
/// this helper not at all.
pub(super) fn validate_cross_mem_add_policy(
    engine: &super::Engine,
    source_mem: &str,
    target: &EntityId,
) -> Result<(), EngineError> {
    let target_mem = target.mem();
    if source_mem == target_mem {
        return Ok(());
    }
    if !engine.cross_mem_link_allowed(source_mem, target_mem) {
        return Err(EngineError::CrossMemLinkNotAllowed {
            from_mem: source_mem.to_string(),
            to_mem: target_mem.to_string(),
        });
    }
    if let Some(mount) = engine.mount(target_mem)
        && mount.capability == crate::workspace::MountCapability::ReadOnly
        && !engine.store.contains(target)
    {
        return Err(EngineError::CrossMemTargetNotFound {
            target_id: target.to_string(),
            target_mem: target_mem.to_string(),
        });
    }
    Ok(())
}

/// Outcome of the engine's edge-validation router for a single
/// inline / explicit relate. Carries the optional open-mode warning
/// from the intra-mem flow; the cross-mem flow has no
/// open-mode (cross-mem entries are declared vocabulary).
pub(super) enum EdgeRouteOutcome {
    Ok,
    OpenModeWarning(Box<crate::ops::WarningHint>),
}

/// Run rel-type + shape validation for one edge, routing through
/// intra-mem vocabulary or the source schema's
/// `cross_mem_relationships:` section as appropriate.
///
/// The routing rule:
/// when `source_mem != target_mem` AND the target mem's
/// pinned schema differs from the source schema by name or by
/// version, the source schema's `cross_mem_relationships:` entry
/// for the target schema is the sole authority for both the
/// vocabulary check (`INVALID_REL_TYPE`) and the shape check
/// (`INVALID_REL_SHAPE`). If no matching entry exists, surface
/// [`EngineError::CrossMemEdgeNotDeclared`].
///
/// Otherwise (same-mem, same-schema cross-mem, or target mem
/// unmounted) the call falls through to the existing intra-mem
/// validators — the same behaviour the intra-mem path always had.
///
/// `check_shape` mirrors the relate path's add-only shape posture:
/// pass `false` to skip the shape check (currently only the
/// `memstead_relate --remove` path). The vocabulary check still fires
/// in that case, matching the intra-mem behaviour where
/// `validate_rel_type` runs on both add and remove.
// The nine parameters are one edge's full coordinates; a params struct
// would restate the same fields at every call site without grouping
// anything that travels together elsewhere.
#[allow(clippy::too_many_arguments)]
pub(super) fn route_edge_validation(
    engine: &super::Engine,
    rel_type: &str,
    from_type: &str,
    to_type: Option<&str>,
    source_mem: &str,
    target_mem: &str,
    from_id: &EntityId,
    to_id: &EntityId,
    check_shape: bool,
) -> Result<EdgeRouteOutcome, EngineError> {
    use crate::runtime_validator::{
        CrossMemRelCheck, RelationshipCheck, validate_cross_mem_edge, validate_rel_shape,
        validate_rel_type,
    };
    use memstead_schema::SchemaRef;

    let source_schema = engine
        .schemas
        .get(source_mem)
        .expect("schema present for every registered mount");

    let target_schema_arc = if source_mem == target_mem {
        None
    } else {
        engine.schemas.get(target_mem).cloned()
    };
    let target_schema_ref: Option<SchemaRef> = target_schema_arc.as_ref().map(|s| {
        let (name, version) = s.id();
        SchemaRef::new(name, version)
    });
    let cross_mem_different = match (&target_schema_ref, source_schema.id()) {
        (Some(target), (src_name, _)) => target.name != src_name,
        (None, _) => false,
    };

    if cross_mem_different {
        let target_ref = target_schema_ref
            .as_ref()
            .expect("target_schema_ref is Some when cross_mem_different");
        if !check_shape {
            // Cleanup posture: cross-mem remove stays permissive so
            // pre-tightening edges remain droppable without first
            // re-declaring them. Mirrors the intra-mem shape gate's
            // add-only stance.
            return Ok(EdgeRouteOutcome::Ok);
        }
        match validate_cross_mem_edge(
            rel_type,
            from_type,
            to_type,
            source_schema.as_ref(),
            target_ref,
        ) {
            CrossMemRelCheck::Ok => Ok(EdgeRouteOutcome::Ok),
            CrossMemRelCheck::EdgeNotDeclared => {
                let (src_name, src_version) = source_schema.id();
                Err(EngineError::CrossMemEdgeNotDeclared {
                    source_schema: SchemaRef::new(src_name, src_version).as_display(),
                    target_schema: target_ref.as_display(),
                    rel_type: rel_type.to_string(),
                    from_id: from_id.to_string(),
                    to_id: to_id.to_string(),
                })
            }
            CrossMemRelCheck::Invalid(v) => Err(EngineError::Validation(v)),
        }
    } else {
        let warning_hint = match validate_rel_type(rel_type, source_schema.as_ref())? {
            RelationshipCheck::Ok => None,
            RelationshipCheck::OpenWarning(message) => {
                Some(crate::ops::WarningHint::UndeclaredRelationshipOpen {
                    rel_type: rel_type.to_string(),
                    message,
                })
            }
        };
        if check_shape {
            validate_rel_shape(rel_type, from_type, to_type, source_schema.as_ref())?;
        }
        Ok(match warning_hint {
            Some(w) => EdgeRouteOutcome::OpenModeWarning(Box::new(w)),
            None => EdgeRouteOutcome::Ok,
        })
    }
}

/// Validate the per-edge description posture declared on the rel-type
/// in the routing-appropriate definition (intra-mem when source and
/// target share the schema; cross-mem entry when they don't). Emits
/// `MissingRequiredDescription` / `DescriptionNotPermitted` on
/// violations; `optional` and unknown rel-types are no-ops (the
/// vocabulary / shape gates already catch undeclared names — posture
/// only fires for declared names).
///
/// `description` is the normalised value (empty / whitespace-only
/// collapses to `None` before reaching this gate). Called from every
/// add path: `memstead_relate`, `declare_relations` on `memstead_create` and
/// `memstead_update`.
pub(super) fn validate_description_posture(
    engine: &super::Engine,
    rel_type: &str,
    description: Option<&str>,
    source_mem: &str,
    target_mem: &str,
    from_id: &EntityId,
    to_id: &EntityId,
) -> Result<(), EngineError> {
    use memstead_schema::{PerEdgeDescription, SchemaRef};

    let source_schema = engine
        .schemas
        .get(source_mem)
        .expect("schema present for every registered mount");
    let target_schema_arc = if source_mem == target_mem {
        None
    } else {
        engine.schemas.get(target_mem).cloned()
    };
    let target_schema_ref: Option<SchemaRef> = target_schema_arc.as_ref().map(|s| {
        let (name, version) = s.id();
        SchemaRef::new(name, version)
    });
    let cross_mem_different = match (&target_schema_ref, source_schema.id()) {
        (Some(target), (src_name, _)) => target.name != src_name,
        (None, _) => false,
    };

    let posture = if cross_mem_different {
        // Look up the matching cross-mem entry's definition. If the
        // entry exists but the rel-type isn't enumerated under it, the
        // vocabulary gate (route_edge_validation) will surface
        // `CROSS_MEM_EDGE_NOT_DECLARED`; posture is a no-op there.
        let target_ref = target_schema_ref
            .as_ref()
            .expect("target_schema_ref is Some when cross_mem_different");
        source_schema
            .cross_mem_entry(&target_ref.name)
            .and_then(|entry| entry.definitions.iter().find(|d| d.name == rel_type))
            .map(|d| d.per_edge_description)
    } else {
        source_schema
            .relationship_def(rel_type)
            .map(|d| d.per_edge_description)
    };

    match posture {
        Some(PerEdgeDescription::Required) if description.is_none() => {
            Err(EngineError::MissingRequiredDescription {
                rel_type: rel_type.to_string(),
                from_id: from_id.to_string(),
                to_id: to_id.to_string(),
            })
        }
        Some(PerEdgeDescription::Forbidden) if description.is_some() => {
            Err(EngineError::DescriptionNotPermitted {
                rel_type: rel_type.to_string(),
                from_id: from_id.to_string(),
                to_id: to_id.to_string(),
            })
        }
        _ => Ok(()),
    }
}

/// Validate the manual-authoring posture declared on the rel-type.
/// Fires only on explicit-author paths (`memstead_relate`, inline
/// `relations:` on `memstead_create`, `declare_relations` on
/// `memstead_update`). The body-link → relation alias machinery
/// synthesises relations from wiki-links — that path bypasses this
/// gate by construction (it never calls this function), keeping the
/// alias path for `manual_authoring: forbidden` rel-types (e.g.
/// REFERENCES) intact.
pub(super) fn validate_manual_authoring_posture(
    engine: &super::Engine,
    rel_type: &str,
    source_mem: &str,
    from_id: &EntityId,
    to_id: &EntityId,
) -> Result<(), EngineError> {
    use memstead_schema::ManualAuthoring;

    let source_schema = engine
        .schemas
        .get(source_mem)
        .expect("schema present for every registered mount");
    let posture = source_schema.relationship_manual_authoring(rel_type);
    if matches!(posture, ManualAuthoring::Forbidden) {
        let guidance = source_schema
            .relationship_when_to_use(rel_type)
            .unwrap_or_default();
        return Err(EngineError::RelationManualAuthoringForbidden {
            rel_type: rel_type.to_string(),
            from_id: from_id.to_string(),
            to_id: to_id.to_string(),
            guidance,
        });
    }
    Ok(())
}

/// Alias-synthesis pass — populates `next.relationships` with engine-
/// emitted relations of the source schema's `alias_target_rel_type`
/// pointer for every body wiki-link not already backed by an
/// in-section-body explicit relation. Runs before the
/// `scan_wikilinks_without_relation` validator; after this pass the
/// validator finds zero missing wiki-links for the pointer rel-type.
///
/// Three cases:
/// 1. Schema has no pointer (`alias_target_rel_type` absent): no-op.
///    Caller's validator continues to refuse unbacked links exactly as
///    today.
/// 2. Schema has a pointer, body wiki-link target is in the same mem
///    OR cross-mem policy admits it: append `Relationship { rel_type:
///    pointer, target, description: None }` to `next.relationships` if
///    no relation of `(pointer, target)` is already present. Dedupe is
///    `(target, rel_type)` — a USES or DEPENDS_ON edge to the same
///    target does not suppress synthesis of the pointer rel-type.
/// 3. Schema has a pointer but a body wiki-link crosses a mem
///    boundary the workspace doesn't grant — or targets an entity
///    absent from a read-only mount: return the funnel's typed
///    refusal ([`EngineError::CrossMemLinkNotAllowed`] /
///    [`EngineError::CrossMemTargetNotFound`], via
///    [`validate_cross_mem_add_policy`]). The entire mutation
///    aborts — no partial state.
///
/// GC: when `prev` is `Some`, the pass also drops pointer-rel-type
/// relations whose target was a body wiki-link in `prev` but no longer
/// appears in `next.sections`. The loader forces `manual_authoring:
/// forbidden` on every schema's `alias_target_rel_type` pointer, so the
/// only path to a pointer-rel-type edge is the body-link channel; the
/// GC rule therefore reduces to "drop pointer-rel-type relations whose
/// target is not in the new body". Targeting prev's wiki-link set
/// specifically (rather than every pointer-rel-type relation) keeps the
/// pass correct even for an explicit-author relation that predates the
/// forbid posture.
///
/// Returns the list of relations the pass emitted (in body iteration
/// order) — `create.rs` / `update.rs` use it to surface
/// `relations_emitted` on the response envelope.
/// Returns the synthesised relations (in body iteration order) and a flag
/// signalling whether a body wiki-link to the entity's own id was dropped
/// (F11). The caller surfaces that as a `SELF_LINK_IGNORED` warning — the
/// pass has no warning channel of its own.
pub(super) fn synthesise_alias_relations(
    engine: &super::Engine,
    prev_body_targets: &std::collections::HashSet<EntityId>,
    next: &mut Entity,
) -> Result<(Vec<crate::entity::Relationship>, bool), super::EngineError> {
    let schema = engine
        .schemas
        .get(next.mem.as_str())
        .expect("schema present for every registered mount");
    let Some(pointer) = schema.alias_target_rel_type().map(str::to_string) else {
        return Ok((Vec::new(), false));
    };

    // 1. GC: drop pointer-rel-type relations whose target was a body
    //    wiki-link in the prev entity state but isn't in next. Targets
    //    not in prev's wiki-link set are explicit-author relations and
    //    are never touched — the rule preserves explicit edges even
    //    while the 5 built-ins still admit explicit REFERENCES.
    //
    //    `extract_inline_links` is strict — non-slug-form targets refuse
    //    here with the typed `InvalidWikiLinkTarget` envelope rather
    //    than silently flowing into the GC's retain set as malformed
    //    EntityIds. Section context comes from the iteration key.
    let mut next_targets: std::collections::HashSet<EntityId> = std::collections::HashSet::new();
    for (section_key, body) in next.sections.iter() {
        let ids = crate::entity::parser::extract_inline_links(body, &next.mem)
            .map_err(|errs| map_wiki_link_errors(section_key, errs))?;
        next_targets.extend(ids);
    }
    next.relationships.retain(|r| {
        !(r.rel_type == pointer
            && prev_body_targets.contains(&r.target)
            && !next_targets.contains(&r.target))
    });

    // 2. Walk body wiki-links in section iteration order and append
    //    one relation per `(target, pointer)` pair not already
    //    present. Cross-mem gate fires on the first refusal.
    let existing: std::collections::HashSet<(String, EntityId)> = next
        .relationships
        .iter()
        .map(|r| (r.rel_type.clone(), r.target.clone()))
        .collect();
    let mut emitted: Vec<crate::entity::Relationship> = Vec::new();
    let mut already_synthesised: std::collections::HashSet<EntityId> =
        std::collections::HashSet::new();
    let mut self_link_ignored = false;
    for (section_key, body) in next.sections.iter() {
        let ids = crate::entity::parser::extract_inline_links(body, &next.mem)
            .map_err(|errs| map_wiki_link_errors(section_key, errs))?;
        for target in ids {
            // F11: a body wiki-link to the entity's own id is a vacuous
            // self-edge (renders as both Outgoing and Incoming, inflates
            // connectivity). Drop it — but don't refuse: the author may
            // have written their own slug. The caller surfaces
            // `SELF_LINK_IGNORED` so the dropped link stays observable.
            if target == next.id {
                self_link_ignored = true;
                continue;
            }
            let key = (pointer.clone(), target.clone());
            if existing.contains(&key) || already_synthesised.contains(&target) {
                continue;
            }
            validate_cross_mem_add_policy(engine, &next.mem, &target)?;
            let rel = crate::entity::Relationship::new(pointer.clone(), target.clone());
            next.relationships.push(rel.clone());
            already_synthesised.insert(target);
            emitted.push(rel);
        }
    }
    Ok((emitted, self_link_ignored))
}

/// Map the first [`crate::entity::id::WikiLinkError`] from a body
/// wiki-link extraction into the typed [`EngineError`] envelope,
/// attaching the offending section's key. Errors after the first are
/// dropped — the agent reads the error, fixes the link, retries, and
/// surfaces the next one on the follow-up call. Keeps the envelope
/// shape stable (single typed payload rather than a list) so MCP /
/// CLI / UniFFI clients don't need a fan-out renderer.
pub(super) fn map_wiki_link_errors(
    section_key: &str,
    errors: Vec<crate::entity::id::WikiLinkError>,
) -> EngineError {
    use crate::entity::id::WikiLinkError;
    let first = errors
        .into_iter()
        .next()
        .expect("map_wiki_link_errors called with non-empty error list");
    match first {
        WikiLinkError::InvalidTarget {
            raw,
            suggested,
            reason,
        } => EngineError::InvalidWikiLinkTarget {
            raw,
            suggested,
            section: section_key.to_string(),
            link_source: "body_link".to_string(),
            reason,
        },
        WikiLinkError::InvalidMemName { raw, reason } => EngineError::InvalidWikiLinkMem {
            raw,
            section: section_key.to_string(),
            reason,
        },
    }
}

/// Compute the set of body wiki-link targets in an entity. Used by
/// callers of `synthesise_alias_relations` to capture the pre-mutation
/// state once, before any borrow conflicts re-enter the engine's
/// schemas / store maps. Uses the lenient decoder — this snapshot
/// must tolerate on-disk drift on pre-strict entities whose bodies
/// may still contain non-conformant links; the strict gate fires
/// only on the post-mutation `next` state.
pub(super) fn collect_body_link_targets(entity: &Entity) -> std::collections::HashSet<EntityId> {
    entity
        .sections
        .iter()
        .flat_map(|(_, body)| {
            crate::entity::parser::extract_inline_links_lenient(body, &entity.mem)
        })
        .collect()
}

/// Alias-existence invariant validator. Given the post-mutation entity
/// state, scan every section body for wiki-links whose target has no
/// corresponding explicit relation in `entity.relationships`. Returns
/// the list of `(section_key, target_id)` pairs that violate the
/// invariant — empty when the post-mutation state is clean.
///
/// Used by [`Engine::create_entity`] and [`Engine::update_entity`]
/// (and `batch_update`). The validator runs unconditionally — under
/// the alias model body wiki-links are foreign-key references on the
/// `## Relationships` table and every reference must be backed.
///
/// Sections from the auto-managed `## Relationships` heading are
/// not scanned (the engine generates them from the relations list
/// at write time; the parser keeps them out of
/// `entity.sections` so they never reach this function).
///
/// Reuses [`crate::entity::parser::extract_inline_links`] so the
/// lexical discipline (fenced-code masking, inline-code masking,
/// alias handling, cross-mem forms) matches every other validator
/// surface in the engine.
pub(super) fn scan_wikilinks_without_relation(
    next: &Entity,
) -> Result<Vec<(String, EntityId)>, EngineError> {
    let explicit_targets: std::collections::HashSet<EntityId> = next
        .relationships
        .iter()
        .map(|r| r.target.clone())
        .collect();
    let mut missing: Vec<(String, EntityId)> = Vec::new();
    for (section_key, body) in next.sections.iter() {
        let ids = crate::entity::parser::extract_inline_links(body, &next.mem)
            .map_err(|errs| map_wiki_link_errors(section_key, errs))?;
        for target in ids {
            // A self-targeting body link is intentionally unbacked: the
            // alias pass drops its (vacuous) self-edge (F11), so it has no
            // backing relation by design and must not trip the
            // unbacked-link refusal here.
            if target == next.id {
                continue;
            }
            if !explicit_targets.contains(&target)
                && !missing
                    .iter()
                    .any(|(k, t)| k == section_key && t == &target)
            {
                missing.push((section_key.clone(), target));
            }
        }
    }
    Ok(missing)
}

#[cfg(test)]
mod tests {

    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::engine::test_helpers::*;
    use crate::engine::{CreateEntityArgs, Engine, UpdateEntityArgs};

    use crate::storage::FilesystemMemWriter;
    use crate::vcs::CommitContext;

    use indexmap::IndexMap;

    #[test]
    fn with_ctx_wrappers_delegate_to_explicit_forms() {
        // Each *_with_ctx wrapper bundles a CommitContext and
        // routes through the corresponding 4-arg method. Verify
        // create → update → rename → delete via the wrappers
        // observably mutate the store the same way the explicit
        // forms would.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let ctx = CommitContext::internal();

        // create_entity_with_ctx
        let create_args = CreateEntityArgs {
            anchors: Vec::new(),
            mem: "specs".to_string(),
            title: "Seed".to_string(),
            entity_type: "spec".to_string(),
            sections: IndexMap::from_iter([
                ("identity".to_string(), "seed identity".to_string()),
                ("purpose".to_string(), "seed purpose".to_string()),
            ]),
            metadata: IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        };
        let created = engine.create_entity_with_ctx(create_args, &ctx).unwrap();
        assert_eq!(created.title, "Seed");
        assert!(engine.store().get(&created.id).is_some());

        // update_entity_with_ctx
        let update_args = UpdateEntityArgs {
            anchors: Vec::new(),
            id: created.id.clone(),
            expected_hash: Some(created.content_hash.clone()),
            sections: IndexMap::from_iter([("identity".to_string(), "updated".to_string())]),
            append_sections: IndexMap::new(),
            patch_sections: IndexMap::new(),
            metadata: IndexMap::new(),
            metadata_unset: Vec::new(),
            dry_run: false,
            declare_relations: Vec::new(),
            relations_unset: Vec::new(),
        };
        let updated = engine.update_entity_with_ctx(update_args, &ctx).unwrap();
        assert!(
            !updated.commit_sha.is_empty()
                || (updated.modified_sections.replaced.is_empty()
                    && updated.modified_sections.appended.is_empty()
                    && updated.modified_sections.patched.is_empty())
        );

        // rename_entity_with_ctx
        let renamed = engine
            .rename_entity_with_ctx(&created.id, "Renamed", &updated.content_hash, &ctx)
            .unwrap();
        assert_ne!(renamed.old_id, renamed.new_id);
        assert!(engine.store().get(&renamed.new_id).is_some());

        // delete_entity_with_ctx
        let deleted = engine
            .delete_entity_with_ctx(&renamed.new_id, &renamed.content_hash, &ctx)
            .unwrap();
        assert_eq!(deleted.id, renamed.new_id);
        assert!(engine.store().get(&renamed.new_id).is_none());
    }
}
