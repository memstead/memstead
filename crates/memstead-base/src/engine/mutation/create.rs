//! `Engine::create_entity` — write a new entity into a mount's
//! backend and update the in-memory store.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use indexmap::IndexMap;

use memstead_schema::TypeDefinition;

/// Build the per-mutation `type_guidance` map from the warnings that
/// would otherwise carry the same type-level `write_rules` per entry.
/// Each distinct `entity_type` named on a section / field warning
/// contributes one entry holding the type's `write_rules`. Returns an
/// empty map when no section/field warnings fire — the stable empty
/// shape ships on the wire so consumers don't branch on field
/// presence (F9).
fn build_type_guidance(
    warnings: &[WarningHint],
    type_def: &TypeDefinition,
) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for w in warnings {
        let entity_type = match w {
            WarningHint::MissingRequiredSection { entity_type, .. }
            | WarningHint::MissingRequiredField { entity_type, .. } => entity_type.as_str(),
            _ => continue,
        };
        if !out.contains_key(entity_type) && entity_type == type_def.name {
            out.insert(entity_type.to_string(), type_def.write_rules.clone());
        }
    }
    out
}

use crate::engine_fallback_type;
use crate::entity::id::validate_and_derive_slug;
use crate::entity::parser::parse_markdown;
use crate::entity::store_builder::push_entities_into_store;
use crate::entity::{Entity, EntityId, MetadataValue, Relationship, normalise_description};
use crate::ops::{WarningHint, project_incoming};
use crate::provenance::{Provenance, ProvenanceKind};
use crate::runtime_validator::{
    missing_required_fields, missing_required_sections, parse_metadata_value,
    validate_section_content, validate_section_keys,
};
use crate::vcs::{Actor, ClientId, CommitContext};
use crate::workspace::MountCapability;

use super::super::{CreateEntityArgs, CreateEntityOutcome, Engine, EngineError};
use super::{
    EdgeRouteOutcome, make_stub, route_edge_validation, unknown_type_error,
    validate_relation_target_grammar,
};

/// Everything a validated create needs to hit disk — the create-side
/// twin of `PreparedUpdate`. Produced by `Engine::prepare_create`,
/// consumed by `Engine::commit_prepared_create` (single item) and by
/// `Engine::batch_create` (staged all-first, one commit per mem).
struct PreparedCreate {
    mount_idx: usize,
    id: EntityId,
    title: String,
    mem: String,
    file_path: String,
    markdown: String,
    anchors: Vec<crate::anchor::Anchor>,
    warnings: Vec<WarningHint>,
    type_guidance: std::collections::BTreeMap<String, Vec<String>>,
    relations_declared: Vec<crate::engine::outcomes::RelationDeclared>,
    /// Inline-relation targets — the commit tail materialises
    /// forward-reference stubs for the ones the store still lacks.
    relation_targets: Vec<EntityId>,
    type_def: std::sync::Arc<memstead_schema::TypeDefinition>,
}

/// Outcome of `Engine::prepare_create`: a dry-run completes at prepare
/// time; a real write returns the staged material.
enum CreatePrepareOutcome {
    Done(CreateEntityOutcome),
    Prepared(PreparedCreate),
}

impl Engine {
    /// Create a new entity in `args.mem`. Six concerns wired here
    /// in one shape regardless of which backend serves the mount:
    ///
    /// 1. **Capability gating** — rejects mounts with `ReadOnly`
    ///    capability before reaching the backend.
    /// 2. **Validator pipeline** — `validate_section_keys` +
    ///    `parse_metadata_value` enforce the pinned schema's strictness;
    ///    typed `ValidationError` lifts to `EngineError::Validation`.
    /// 3. **Provenance** — a `Provenance` record routes through
    ///    `backend.append_provenance` (folder writes JSONL, git-branch
    ///    no-ops since the commit subject + trailers carry the same
    ///    fields).
    /// 4. **Write + commit atomicity** — `backend.write_entity` then
    ///    `backend.commit` with the canonical `memstead: create <id>`
    ///    subject so the git-branch backend's `read_provenance` can
    ///    recover the kind.
    /// 5. **Store update** — re-parse the freshly-generated markdown
    ///    so the in-memory `Store` mirrors disk (including
    ///    generator-determined `content_hash`).
    /// 6. **Error envelope** — `BackendError::Sealed` lifts via the
    ///    `Backend` variant so MCP callers see the typed payload
    ///    intact; `HashMismatch` propagates likewise.
    pub fn create_entity(
        &mut self,
        args: CreateEntityArgs,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<CreateEntityOutcome, EngineError> {
        let drift_warnings = self.reload_if_stale(Some(&args.mem));
        // Declared relations on an ACYCLIC rel-type (or one in an
        // `acyclic_sets` set, whose guard walks the set's UNION
        // subgraph) run the same whole-subgraph cycle guard relate
        // runs — full load first, or a cycle through a deferred mem's
        // edge is invisible (see the relate path's comment for the
        // demonstrated failure). Declared signals on the new entity's
        // schema or any relation target's schema need the full load
        // too: the threshold-crossing diff counts edges that can
        // originate in any mem.
        if args.relations.iter().any(|r| {
            self.schemas.get(&args.mem).is_some_and(|s| {
                s.relationship_acyclic(&r.rel_type)
                    || s.acyclic_set_containing(&r.rel_type).is_some()
            }) || self
                .schemas
                .get(r.target.mem())
                .is_some_and(|s| s.types.values().any(|td| !td.signals.is_empty()))
        }) || self
            .schemas
            .get(&args.mem)
            .is_some_and(|s| s.types.values().any(|td| !td.signals.is_empty()))
        {
            self.ensure_mems_loaded(None);
        }
        match self.prepare_create(args, None, drift_warnings)? {
            CreatePrepareOutcome::Done(outcome) => Ok(outcome),
            CreatePrepareOutcome::Prepared(prepared) => {
                self.commit_prepared_create(prepared, actor, client, note)
            }
        }
    }

    /// Validate a create and compute everything up to (but not
    /// including) the disk write — the create-side prepare of the
    /// prepare-all-then-commit split `batch_update` established.
    /// Returns [`CreatePrepareOutcome::Done`] for a dry-run (its
    /// outcome is complete), [`CreatePrepareOutcome::Prepared`] for a
    /// real write the caller commits via
    /// [`Self::commit_prepared_create`].
    ///
    /// `batch_skeleton_ids` is the batch path's staging set: ids the
    /// current batch has pre-inserted as skeleton entities so
    /// intra-batch references validate as REAL targets. A create whose
    /// id is in the set skips the already-exists refusal (the skeleton
    /// is this very entry's placeholder — batch-side identity checks
    /// have already refused genuine duplicates). Single-item callers
    /// pass `None`.
    /// NOTE: the reload-before-operation drift probe is the CALLER's
    /// job (single-item: `create_entity` probes its one mem; batch:
    /// `batch_create` probes every touched mem once, up front). A probe
    /// inside prepare would reload mid-batch and wipe the staged
    /// skeletons.
    fn prepare_create(
        &mut self,
        args: CreateEntityArgs,
        batch_skeleton_ids: Option<&std::collections::HashSet<EntityId>>,
        mut drift_warnings: Vec<WarningHint>,
    ) -> Result<CreatePrepareOutcome, EngineError> {
        let mut args = args;
        // Canonicalise rel_type on every inline relation — same contract
        // as `relate_entity`: input is case-insensitive, storage and
        // response are UPPER_SNAKE_CASE. Syntax errors fall through to
        // the schema check, which surfaces them as INVALID_REL_TYPE.
        for rel in &mut args.relations {
            if let Ok(canonical) = crate::entity::id::validate_rel_type(&rel.rel_type) {
                rel.rel_type = canonical;
            }
        }

        // Trim surrounding whitespace from the title before slug
        // derivation + storage. Internal whitespace is preserved.
        // Fully-whitespace titles collapse to empty and fall through to
        // the validator below (which already refuses empty). Without
        // trimming, a caller-supplied
        // `"   Foo   "` renders with leading/trailing spaces despite the
        // slug being correct. We emit `TITLE_TRIMMED` whenever trimming
        // changed the value so the audit trail records the drift.
        let mut title_trimmed_warning: Option<crate::ops::WarningHint> = None;
        let trimmed_title = args.title.trim();
        if trimmed_title.len() != args.title.len() {
            title_trimmed_warning = Some(crate::ops::WarningHint::TitleTrimmed {
                original: args.title.clone(),
                trimmed: trimmed_title.to_string(),
            });
            args.title = trimmed_title.to_string();
        }

        // 1. Resolve the mount and gate on capability.
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == args.mem)
            .ok_or_else(|| self.unknown_mem_error(&args.mem))?;
        if self.mounts[mount_idx].mount.capability != MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(args.mem));
        }

        // 1a. Reload-before-operation. Probe the mem ref and reload
        //     if a sibling writer advanced it past our cached head, so
        //     the duplicate-id check below and the eventual commit both
        //     run against current truth. Any `MemReloaded` warning
        //     rides the outcome's `warnings` (merged at the accumulator
        //     below). This is what makes a create at an id a sibling
        //     just created refuse as already-exists rather than
        //     silently rebasing onto an unobserved commit.
        // 2. Resolve schema + type. The schema map is populated for
        //    every mount during `from_mounts`, so the lookup is total.
        let schema = self
            .schemas
            .get(&args.mem)
            .expect("schema present for every registered mount");
        let type_def = schema
            .get_type(&args.entity_type)
            .ok_or_else(|| unknown_type_error(schema, &args.entity_type))?;

        // 3. Pre-write validators: section keys and metadata values.
        validate_section_keys(args.sections.keys().map(String::as_str), type_def.as_ref())?;
        // Reserved identity/discriminator keys (`mem`/`id`/`type`)
        // refuse deliberately (`READ_ONLY_FIELD`) before the metadata
        // parse loop can refuse them incidentally as
        // `UNKNOWN_METADATA_FIELD` — symmetric with the update path's
        // set gate, so the two paths agree and the refusal names the
        // real reason. Timestamp fields keep create's documented
        // stamp-and-proceed posture (`IGNORED_READONLY_FIELD` warning,
        // step 5a) — only the triple is checked here.
        for key in args.metadata.keys() {
            crate::runtime_validator::validate_reserved_metadata_key(key.as_str())?;
        }
        // 3a. Validate any `anchors[]` payload up front — a malformed
        //     element (unknown class/grain, missing artifact, hash on a
        //     non-hash class, grain unsupported by the resolving medium's
        //     namespace) refuses the WHOLE create with a typed
        //     `INVALID_ANCHOR` envelope BEFORE any disk write, so the
        //     entity is never written. Empty payload → empty vec (no
        //     sidecar write; byte-identical to a pre-anchor create). Runs
        //     even on the dry_run path so validity agrees across preview
        //     and real write.
        let validated_anchors = self.validate_anchor_inputs(&args.mem, &args.anchors)?;
        // Refuse section content with embedded `^## ` headings — the
        // compose-then-reparse pipeline would split the value at the
        // heading and silently move the trailing content into another
        // section.
        let mut heading_buf: Vec<&str> = Vec::new();
        let catch_all = crate::runtime_validator::catch_all_context(&type_def, &mut heading_buf);
        validate_section_content(
            args.sections.iter().map(|(k, v)| (k.as_str(), v.as_str())),
            catch_all,
        )?;

        // 4. Slug + id; reject duplicates against the in-memory store.
        //    Stub adoption: a pre-existing stub at the same id is
        //    *not* a duplicate — the create promotes the stub to a
        //    real entity while preserving its incoming edges (store.
        //    upsert leaves in_edges in place). Mirrors full's
        //    `if let Some(existing) = store.get(&id) && !existing.stub`.
        let derivation = validate_and_derive_slug(&args.title)?;
        let slug = derivation.slug.clone();
        let id = EntityId::new(&args.mem, &slug);
        crate::entity::id::enforce_id_length(id.as_ref())?;
        if let Some(existing) = self.store.get(&id)
            && !existing.stub
            && !batch_skeleton_ids.is_some_and(|set| set.contains(&id))
        {
            return Err(EngineError::AlreadyExists {
                id: id.to_string(),
                existing_title: existing.title.clone(),
                existing_is_stub: false,
            });
        }
        let file_path = format!("{slug}.md");

        // 5. Build metadata. `type` is seeded so the generator emits
        //    the canonical frontmatter; caller-provided overrides go
        //    through `parse_metadata_value` for enum / type checks.
        let mut metadata: IndexMap<String, MetadataValue> = IndexMap::new();
        metadata.insert(
            "type".to_string(),
            MetadataValue::String(args.entity_type.clone()),
        );
        for (k, v) in &args.metadata {
            let parsed = parse_metadata_value(k.as_str(), v.as_str(), type_def.as_ref())?;
            metadata.insert(k.clone(), parsed);
        }

        // 5a. Engine-managed timestamps: schema-declared `init_timestamp`
        //     and `auto_timestamp` fields take the engine value
        //     regardless of any caller-supplied override. Symmetric with
        //     the update path's `auto_timestamp` loop — both flags carry
        //     a schema-promised meaning the user cannot override.
        //     `init_timestamp` is create-only (set once, then stable);
        //     `auto_timestamp` re-stamps on every update.
        let today = self.now_iso();
        // Accumulate `IGNORED_READONLY_FIELD` warnings: when the caller
        // supplied a value for an auto-managed field, the engine value
        // overwrites it below — surface that the input was discarded
        // rather than swallowing it silently (the update path refuses
        // these keys with `READ_ONLY_FIELD`; create's posture is
        // stamp-and-proceed, so it warns). Built here, merged into the
        // response `warnings` accumulator once that exists.
        let mut ignored_readonly: Vec<WarningHint> = Vec::new();
        for field_def in &type_def.metadata_fields {
            if field_def.init_timestamp || field_def.auto_timestamp {
                if let Some(supplied) = args.metadata.get(field_def.key.as_str()) {
                    ignored_readonly.push(WarningHint::IgnoredReadonlyField {
                        field: field_def.key.clone(),
                        supplied: supplied.clone(),
                    });
                }
                metadata.insert(field_def.key.clone(), MetadataValue::String(today.clone()));
            }
        }

        // 6. Refuse — not warn — when required sections are absent or
        //    empty. Pre-fix this branch emitted a `WarningHint` per
        //    missing section and let the entity land with empty
        //    placeholders; the resulting on-disk state then failed the
        //    install-time strict validator, breaking the export-then-
        //    install round-trip. The refusal carries every missing
        //    section plus the type-level `type_guidance` map so the
        //    agent recovers in a single round-trip via re-call with
        //    the missing content filled in. Iterative authoring stays
        //    available — the agent creates the entity with whatever
        //    sections they have, then fills in the rest via
        //    `memstead_update` (which retains its permissive posture on
        //    `MISSING_REQUIRED_SECTION`).
        let missing_sections = missing_required_sections(type_def.as_ref(), &args.sections);
        if !missing_sections.is_empty() {
            let mut type_guidance: BTreeMap<String, Vec<String>> = BTreeMap::new();
            if missing_sections
                .iter()
                .any(|m| m.entity_type == type_def.name)
            {
                type_guidance.insert(type_def.name.clone(), type_def.write_rules.clone());
            }
            return Err(EngineError::MissingRequiredSection {
                entity_type: type_def.name.clone(),
                missing_count: missing_sections.len(),
                sections: missing_sections,
                type_guidance,
                // Cross-gate pre-announcement: step 6a's demand set
                // depends only on the type definition and the supplied
                // metadata keys — both fully knowable here — so the
                // refusal announces it now and the fixed-everything
                // retry clears both gates in one round-trip. The same
                // computation runs again at 6a when the sections pass,
                // which is what keeps the announcement true rather
                // than merely plausible.
                pre_announced_missing_fields: missing_required_fields(
                    type_def.as_ref(),
                    &args.metadata,
                ),
            });
        }

        // 6a. Parallel for metadata fields: refuse on the first
        //     missing required field the schema does not auto-fill.
        //     Same trust-boundary reasoning as the sections case —
        //     pre-fix the generator silently wrote today's-date / ""
        //     placeholders that the strict validator at install time
        //     can refuse. The agent fixes one field per round-trip
        //     (schema-declaration order); the recovery shape mirrors
        //     the existing `RequiredFieldUnset` envelope on the update
        //     path so a single decoder handles both surfaces.
        let missing_fields = missing_required_fields(type_def.as_ref(), &args.metadata);
        if !missing_fields.is_empty() {
            // Surface the
            // full accumulator (`details.missing[]`) so the agent
            // fixes every required-no-default field unset in one
            // retry. The singular `field` / `field_description` /
            // `enum_values` echo the first entry for back-compat
            // with consumers reading the singular shape.
            let first = missing_fields[0].clone();
            return Err(EngineError::RequiredFieldUnset {
                field: first.key,
                entity_type: first.entity_type,
                field_description: Some(first.description),
                enum_values: first.enum_values,
                type_write_rules: type_def.write_rules.clone(),
                // Create path — the caller never
                // supplied this field. Display / prose_render flip to
                // "not provided" wording so the prose matches the
                // semantic. Recovery is unchanged; the typed code
                // stays `REQUIRED_FIELD_UNSET`.
                on_create: true,
                missing: missing_fields,
            });
        }

        let mut warnings: Vec<WarningHint> = Vec::new();

        // Reload-before-operation drift notice (probed at the top, after
        // the capability gate). Surfaced first so the agent sees the
        // world moved before reading the rest of the outcome.
        warnings.append(&mut drift_warnings);

        // Auto-managed fields the caller tried to set (computed during
        // the stamp loop above) — the supplied values were discarded.
        warnings.append(&mut ignored_readonly);

        // Title↔slug divergence: the widened title grammar admits
        // characters the slug alphabet drops — visible, not fatal.
        if !derivation.dropped_chars.is_empty() {
            warnings.push(WarningHint::TitleCharsDroppedFromSlug {
                title: args.title.trim().to_string(),
                dropped_chars: derivation.dropped_chars.clone(),
                slug: slug.clone(),
            });
        }

        // Surface the title-trim drift (computed pre-validation) so the
        // audit trail records what the caller sent.
        if let Some(w) = title_trimmed_warning.take() {
            warnings.push(w);
        }

        // 6c. Build `type_guidance` map for the response — one entry
        //     per distinct entity_type referenced by warnings carrying
        //     entity-type context (currently
        //     `UndeclaredRelationshipOpen` etc). Empty when no such
        //     warnings fire — the section/field cases now refuse
        //     above. The stable empty shape always ships so callers
        //     don't branch on field presence.
        let type_guidance = build_type_guidance(&warnings, type_def.as_ref());

        // 6b. Validate inline relationship inputs through the same
        //     gates `memstead_relate` runs (Item 02): target-id grammar,
        //     rel-type vocabulary, schema shape. Pre-fix the create
        //     path ran only the rel-type check, so an agent could
        //     sneak a malformed target id (auto-stub at
        //     `bad@chars$here`) or a shape-violating
        //     `(rel_type, source_type, target_type)` triple through
        //     `memstead_create.relations[]` even though `memstead_relate`
        //     rejected the same input. Strict-mode schemas reject
        //     unknown rel-types with `INVALID_REL_TYPE`; open-mode
        //     schemas admit them and surface a typed
        //     `UndeclaredRelationshipOpen` warning. Stub-as-source
        //     is impossible here — the source is the newly-created
        //     entity, always real-by-construction.
        for rel in &args.relations {
            validate_relation_target_grammar(&rel.target)?;
            let target_mem = rel.target.mem().to_string();
            // Cross-mem policy gate. The funnel
            // sits ahead of the rel-type / shape checks so the policy
            // refusal is identical in shape and ordering to
            // `memstead_relate` and `memstead_update.declare_relations`.
            super::validate_cross_mem_add_policy(self, &args.mem, &rel.target)?;
            // Target-type lookup mirrors the relate path: `None` for
            // not-yet-present targets so the target gate admits the
            // stub-bound case. The cross-mem router below consults
            // it for both intra-mem shape and cross-mem-different
            // shape checks.
            let target_type = self
                .store
                .get(&rel.target)
                .map(|e| e.entity_type.clone())
                .filter(|t| !t.is_empty());
            // Deferred-mem target (flywheel W7/02): the store cannot
            // answer for an unloaded mem — the real type comes from
            // the one resolved blob, without loading the mem. `None`
            // for non-deferred or absent targets, unchanged posture.
            let target_type = match target_type {
                Some(t) => Some(t),
                None => super::peek_deferred_target_type(self, &rel.target)?,
            };
            match route_edge_validation(
                self,
                &rel.rel_type,
                args.entity_type.as_str(),
                target_type.as_deref(),
                &args.mem,
                &target_mem,
                &id,
                &rel.target,
                /* check_shape = */ true,
            )? {
                EdgeRouteOutcome::Ok => {}
                EdgeRouteOutcome::OpenModeWarning(w) => warnings.push(*w),
            }
            // Per-edge description posture. Normalise first so empty
            // strings collapse to `None` before the gate.
            let normalised = normalise_description(rel.description.as_deref());
            super::validate_description_posture(
                self,
                &rel.rel_type,
                normalised.as_deref(),
                &args.mem,
                &target_mem,
                &id,
                &rel.target,
            )?;
            // Explicit inline-relations path is an
            // explicit-author boundary — gate on the rel-type's
            // `manual_authoring` posture.
            super::validate_manual_authoring_posture(
                self,
                &rel.rel_type,
                &args.mem,
                &id,
                &rel.target,
            )?;
            // Cycle family — the same shared gate `memstead_relate` runs
            // (self-loop on listed no-self-loop rel-types, long cycle
            // on acyclic ones), against the current store. A stub being promoted
            // by this create already carries its incoming edges, so a
            // back-path through the new id is visible; on the batch
            // path prior items' edges are staged into the store, so an
            // intra-batch cycle refuses here too. Canonicalise the
            // rel-type first (same derivation as
            // `update.declare_relations`) so the schema lookups see
            // the wire-contract form.
            let canonical = crate::entity::id::validate_rel_type(&rel.rel_type)
                .unwrap_or_else(|_| rel.rel_type.clone());
            super::validate_edge_acyclicity(
                &self.store,
                schema.as_ref(),
                &id,
                args.entity_type.as_str(),
                &rel.target,
                &canonical,
            )?;
        }

        // 7. Synthesise the in-memory entity for the generator. The
        //    `content_hash` and `heading_spans` are derived; left
        //    blank because we re-parse the generated bytes below.
        //    Inline relations land in `relationships` so the
        //    generator emits them and the post-parse re-ingest
        //    rebuilds the edges in the store.
        let relationships: Vec<Relationship> = args
            .relations
            .iter()
            .map(|r| Relationship {
                rel_type: r.rel_type.clone(),
                target: r.target.clone(),
                description: normalise_description(r.description.as_deref()),
            })
            .collect();
        // Pre-compute the `relations_declared` outcome echo. Read
        // `target_was_stubbed` against the pre-mutation store state
        // (the post-parse `push_entities_into_store` step will
        // auto-stub absent targets). Shape matches
        // `memstead_update.relations_declared` so callers see a uniform
        // wire shape across the two tools.
        let relations_declared: Vec<crate::engine::outcomes::RelationDeclared> = args
            .relations
            .iter()
            .map(|r| crate::engine::outcomes::RelationDeclared {
                rel_type: r.rel_type.clone(),
                target: r.target.clone(),
                target_was_stubbed: !self.store.contains(&r.target),
            })
            .collect();
        let mut entity_for_render = Entity {
            id: id.clone(),
            title: args.title.clone(),
            entity_type: args.entity_type.clone(),
            mem: args.mem.clone(),
            file_path: file_path.clone(),
            metadata,
            sections: args.sections,
            relationships,
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: HashMap::new(),
            raw_section_headings: Vec::new(),
        };
        // Alias-synthesis pass: for schemas declaring
        // `alias_target_rel_type`, append engine-emitted relations of
        // that rel-type for every body wiki-link not already backed.
        // Cross-mem refusal aborts the create — no partial state.
        // Schemas without the pointer fall through unchanged and the
        // validator below catches the missing relations.
        //
        // The returned `Vec<Relationship>` is the per-call set of
        // relations the pass just emitted (in body iteration order).
        // It feeds the `InlineWikiLinkAutoStubbed` emission below —
        // using the post-mutation `entity.relationships` as the source
        // via `parse_markdown` filters out the body-link targets
        // because the parser-side `relationships`-coverage filter has
        // already absorbed them.
        let empty_prev_targets = std::collections::HashSet::new();
        let alias_outcome =
            super::synthesise_alias_relations(self, &empty_prev_targets, &mut entity_for_render)?;
        let synthesised_relations = alias_outcome.emitted;
        if alias_outcome.self_link_ignored {
            warnings.push(WarningHint::SelfLinkIgnored { id: id.clone() });
        }
        let undeclared_targets: std::collections::HashSet<crate::entity::EntityId> = alias_outcome
            .undeclared_dropped
            .iter()
            .map(|d| d.target.clone())
            .collect();
        for dropped in alias_outcome.undeclared_dropped {
            warnings.push(WarningHint::CrossSchemaLinkUndeclared {
                from: id.clone(),
                target: dropped.target,
                source_schema: dropped.source_schema,
                target_schema: dropped.target_schema,
            });
        }

        // Alias-existence invariant: every body wiki-link must be
        // backed by an entry in `entity.relationships` (the auto-managed
        // `## Relationships` section). Runs unconditionally on every
        // Write-Mem create. See [`scan_wikilinks_without_relation`].
        let missing =
            super::scan_wikilinks_without_relation(&entity_for_render, &undeclared_targets)?;
        if !missing.is_empty() {
            return Err(EngineError::WikiLinkWithoutRelation {
                from_id: id.to_string(),
                missing: missing
                    .into_iter()
                    .map(|(section_key, target)| crate::engine::MissingWikiLink {
                        section_key,
                        target_id: target.to_string(),
                    })
                    .collect(),
            });
        }

        let markdown = super::render_for_write(&entity_for_render, type_def.as_ref())?;

        // 7a. Inline `[[wiki-link]]` patterns in section bodies that
        //     point at non-existent targets get auto-stubbed by the
        //     loader on re-ingest. Surface the would-be stubs as a
        //     warning so prose-induced ghosts are reviewable. Mirrors
        //     `memstead_relate`'s `AUTO_STUB_CREATED` observation
        //     discipline.
        //
        //     The input set is the relations the alias-synthesis pass
        //     emitted on this call — NOT a re-parse of the generated
        //     markdown. `parse_markdown` filters its `inline_links`
        //     against the entity's `relationships` vec (which the
        //     synthesis pass has already appended to), so the
        //     pre-fix path saw `inline_links: []` and never fired
        //     the warning. The synthesised vec is the authoritative
        //     per-call source.
        let auto_stubbed: Vec<EntityId> = synthesised_relations
            .iter()
            .filter_map(|rel| {
                if !self.store.contains(&rel.target) {
                    Some(rel.target.clone())
                } else {
                    None
                }
            })
            .collect();
        if !auto_stubbed.is_empty() {
            warnings.push(WarningHint::InlineWikiLinkAutoStubbed {
                from: id.clone(),
                stubs: auto_stubbed,
            });
        }

        // 7a-bis. Required-outgoing evaluation — the warning the tool
        // descriptions have promised all along. Runs now that every
        // edge this create carries (declared + alias-synthesised) is
        // known, through the same evaluation the health sweep uses
        // (one implementation; the two surfaces cannot disagree). A
        // warning, never a refusal: entities are legitimately built up
        // over several calls.
        // Section-format evaluation (plan 08): each written section
        // body against its declared markdown shape, judged by the
        // real CommonMark reduction. Block-tier refuses with the
        // first violation, pre-commit; warn-tier surfaces via the
        // health sweep, never at write time.
        for def in &type_def.sections {
            if def.format_severity != memstead_schema::ConstraintSeverity::Block {
                continue;
            }
            // Absent-as-empty: the generator renders every declared
            // section heading (empty body when omitted), so the
            // format judges the state that actually lands on disk —
            // an expression that does not admit the empty sequence
            // makes its section effectively required (declare a `?`
            // or `*` form to admit omission). Without this, an
            // omitting create passes while health flags the same
            // on-disk state — write path and health must agree.
            let body = entity_for_render
                .sections
                .get(def.key.as_str())
                .map(String::as_str)
                .unwrap_or("");
            if let Some(first) = crate::section_format::check_section_format(def, body)
                .into_iter()
                .next()
            {
                return Err(EngineError::SectionFormatRefused {
                    entity_type: entity_for_render.entity_type.clone(),
                    entity_id: id.to_string(),
                    violation: first,
                });
            }
        }

        let unsatisfied = crate::ops::health::unsatisfied_required_outgoing(
            &entity_for_render,
            type_def.as_ref(),
        );
        if !unsatisfied.is_empty() {
            // A block declared `severity: block` promotes the warning
            // to a refusal — evaluated here, before any disk or store
            // effect, so a refused create leaves nothing behind.
            let blocked: Vec<_> = unsatisfied
                .iter()
                .filter(|b| b.severity == memstead_schema::ConstraintSeverity::Block)
                .cloned()
                .collect();
            if !blocked.is_empty() {
                return Err(EngineError::RequiredOutgoingUnsatisfied {
                    entity_type: entity_for_render.entity_type.clone(),
                    entity_id: id.to_string(),
                    missing: blocked,
                });
            }
            warnings.push(WarningHint::MissingRequiredOutgoing {
                entity_type: entity_for_render.entity_type.clone(),
                entity_id: id.clone(),
                missing: unsatisfied,
            });
        }

        // Declared-constraints evaluation (`requires_when`, …) — the
        // same single evaluation the health `constraints` include
        // runs. Block-tier violations refuse; warn-tier violations
        // warn and the write proceeds.
        let check_provider = self.check_standing_provider();
        let violated = crate::ops::health::unsatisfied_constraints(
            &self.store,
            &entity_for_render,
            type_def.as_ref(),
            None,
            Some(&check_provider),
        );
        if !violated.is_empty() {
            let blocked: Vec<_> = violated
                .iter()
                .filter(|v| v.severity() == memstead_schema::ConstraintSeverity::Block)
                .cloned()
                .collect();
            if !blocked.is_empty() {
                return Err(EngineError::ConstraintUnsatisfied {
                    entity_type: entity_for_render.entity_type.clone(),
                    entity_id: id.to_string(),
                    violations: blocked,
                });
            }
            warnings.push(WarningHint::ConstraintUnsatisfied {
                entity_type: entity_for_render.entity_type.clone(),
                entity_id: id.clone(),
                violations: violated,
            });
        }

        // 7b. Dry-run: compute prospective hash from the in-memory
        //     entity and return without touching disk, store, or
        //     edges. Mirrors full's `CreateArgs.dry_run` semantics —
        //     `content_hash` carries the prospective hash since
        //     there's no current to differentiate from. `write_id`
        //     is empty. Stub creation is also skipped (no
        //     in-memory side effects).
        if args.dry_run {
            let prospective_hash = crate::entity::parser::compute_hash(&markdown);
            // `created_date` from the in-memory entity (the
            // metadata-construction loop already set the
            // init_timestamp default to `today_iso`-equivalent).
            let created_date = entity_for_render
                .metadata
                .get("created_date")
                .map(|v| v.to_frontmatter_string())
                .unwrap_or_default();
            // Full's dry_run computes incoming from the existing
            // store state (the refs that *would* be adopted if a
            // stub exists at this id). Read before any mutation.
            let incoming = project_incoming(self.store.incoming(&id));
            let incoming_count = (!incoming.is_empty()).then_some(incoming.len());
            return Ok(CreatePrepareOutcome::Done(CreateEntityOutcome {
                id,
                title: args.title,
                mem: args.mem,
                file_path,
                content_hash: prospective_hash,
                write_id: String::new(),
                created_date,
                warnings,
                type_guidance,
                incoming_count,
                incoming,
                relations_declared: relations_declared.clone(),
            }));
        }

        Ok(CreatePrepareOutcome::Prepared(PreparedCreate {
            mount_idx,
            id,
            title: args.title,
            mem: args.mem,
            file_path,
            markdown,
            anchors: validated_anchors,
            warnings,
            type_guidance,
            relations_declared,
            relation_targets: args.relations.iter().map(|r| r.target.clone()).collect(),
            type_def,
        }))
    }

    /// Stage the prepared disk write, commit it, append provenance, and
    /// apply the change to the in-memory store — the single-create tail
    /// of [`Self::create_entity`]. The batch path drives the same
    /// steps but stages every item first and commits once per mem.
    fn commit_prepared_create(
        &mut self,
        prepared: PreparedCreate,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<CreateEntityOutcome, EngineError> {
        let PreparedCreate {
            mount_idx,
            id,
            title,
            mem,
            file_path,
            markdown,
            anchors: validated_anchors,
            mut warnings,
            type_guidance,
            relations_declared,
            relation_targets,
            type_def,
        } = prepared;

        // Aggregate signals: the entities a create can move are the
        // new entity itself (baseline all-`none`) and the targets of
        // its inline relations. Captured before the store mutates,
        // diffed after the push below.
        let signal_snapshot = {
            let mut candidates: Vec<&EntityId> = vec![&id];
            candidates.extend(relation_targets.iter());
            crate::ops::signals::snapshot_levels(&self.store, &self.schemas, candidates)
        };

        // 8. Write + commit through the backend. The commit subject
        //    is `memstead: create <id>` so the git-branch backend's
        //    `read_provenance` recovers the kind via the verb. The
        //    folder backend's commit ignores the message; the
        //    canonical form is harmless there.
        let backend = self.mounts[mount_idx].backend.as_ref();
        backend.write_entity(Path::new(&file_path), markdown.as_bytes())?;
        // Stage the anchors sidecar into the SAME pending buffer so it
        // rides the entity's commit atomically. Only when the create
        // carried anchors — an anchorless create writes no sidecar and is
        // byte-identical to a pre-anchor create.
        if !validated_anchors.is_empty() {
            super::stage_anchors_sidecar(backend, &id, &[], validated_anchors, true)?;
        }
        // Derivation baselines (agent-trust plan 12): each explicitly
        // declared relation on a derivation rel-type records the
        // target's current hash ("" for an absent/stubbed target),
        // staged so baseline and entity ride one commit.
        if let Some(schema) = self.schemas.get(&mem) {
            for r in relations_declared
                .iter()
                .filter(|r| super::rel_type_declares_derivation(schema, &r.rel_type))
            {
                let hash = self
                    .store
                    .get(&r.target)
                    .map(|e| e.content_hash.clone())
                    .unwrap_or_default();
                let (from, rel, to) = (id.to_string(), r.rel_type.clone(), r.target.to_string());
                super::stage_derivation_sidecar(backend, |s| s.set(&from, &rel, &to, &hash))?;
            }
        }
        let commit_subject = format!("memstead: create {id}");
        let ctx = CommitContext {
            actor,
            client: client.cloned(),
            tool: Some("create_entity"),
            note: note.map(String::from),
            role: self.current_role,
            identity: self.current_identity.clone(),
            logical_operation_id: None,
            entity_ids: None,
        };
        let write_id = backend.commit(&commit_subject, &ctx)?;

        // 9. Append provenance. Folder writes a JSONL line; git-branch
        //    no-ops (the commit object already carries the data).
        backend.append_provenance(
            &Provenance::new(
                std::time::SystemTime::now(),
                ProvenanceKind::Create,
                Some(id.to_string()),
                actor,
                client.cloned(),
                note.map(String::from),
            )
            .with_role(self.current_role)
            .with_identity(self.current_identity.clone()),
        )?;

        // Self-write bookkeeping: jump `last_known_head` to the SHA
        // we just produced so the next read doesn't surface
        // `MEM_RELOADED` for our own commit.
        self.record_self_write(mount_idx, &write_id);
        let stamp_warnings = self.stamp_mutation_versions(mount_idx);

        // 10. Update the in-memory store via re-parse so the store
        //     mirrors the on-disk shape (content_hash, heading_spans).
        let parse_result = parse_markdown(&markdown, &file_path, type_def.as_ref(), &mem)
            .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
        let content_hash = parse_result.entity.content_hash.clone();

        // Extract `created_date` from the parsed entity's metadata
        // before pushing into the store (after push, the entity is
        // borrowed by the store and re-fetching costs a lookup).
        // The default schema's auto-timestamp fills `created_date`
        // with today's ISO date; the field is empty for schemas
        // that don't declare it.
        let created_date = parse_result
            .entity
            .metadata
            .get("created_date")
            .map(|v| v.to_frontmatter_string())
            .unwrap_or_default();

        let fallback = engine_fallback_type();
        push_entities_into_store(&mut self.store, vec![parse_result], fallback.as_ref(), None);
        crate::entity::store_builder::remap_alias_target_edge_sources(
            &mut self.store,
            &self.schemas,
        );

        // Materialise stubs for any inline-relation targets that
        // weren't already in the store. Mirrors the relate path's
        // ensure_target — full's create relies on the
        // loader stubbing unresolved targets, but the unified
        // store doesn't auto-stub on push, so the engine does it
        // explicitly. Skipped when no relations were declared
        // (the args.relations vec is empty).
        for target in &relation_targets {
            if !self.store.contains(target) {
                let kind = super::deferred_verified_stub_kind(self, target)?;
                self.store.upsert(target.clone(), make_stub(target, kind));
            }
        }

        self.invalidate_communities();
        // Incremental (flywheel W8/01): the new entity is the whole
        // touched set — its stub targets are never indexed.
        self.maintain_search_indexes(std::slice::from_ref(&id));

        // Stub-adoption visibility: project the incoming edges that
        // survived the upsert. Empty for a fresh create; populated
        // when a pre-existing stub at this id had referrers.
        let incoming = project_incoming(self.store.incoming(&id));
        let incoming_count = (!incoming.is_empty()).then_some(incoming.len());

        // Signal crossings — out-of-band beside the success payload,
        // never error-shaped.
        warnings.extend(stamp_warnings);
        warnings.extend(crate::ops::signals::crossing_warnings(
            &self.store,
            &self.schemas,
            &signal_snapshot,
        ));

        // `require_notes` provenance nudge — single engine-level
        // enforcement point (see `Engine::note_missing_warning`). Only
        // reached on the real-write path (commit landed); the dry-run
        // early return above never demands a note.
        if let Some(w) = self.note_missing_warning("create_entity", note) {
            warnings.push(w);
        }

        Ok(CreateEntityOutcome {
            id,
            title,
            mem,
            file_path,
            content_hash,
            write_id,
            created_date,
            warnings,
            type_guidance,
            incoming_count,
            incoming,
            relations_declared,
        })
    }

    /// Atomic batch create — the create-side sibling of
    /// [`Self::batch_update`], with one upgrade and one addition:
    ///
    /// - **Report-all refusal.** Every failing entry is identified with
    ///   its index and typed `{code, message, details}` envelope (the
    ///   family's upgraded contract) — bounded at
    ///   [`Self::BATCH_ERROR_REPORT_CAP`] detailed envelopes, with
    ///   `errors_suppressed` counting the rest. A refused batch writes
    ///   NOTHING: no entity, no edge, no head movement.
    /// - **Intra-batch references resolve as REAL targets.** Every
    ///   entity in the batch is staged (a skeleton store entry carrying
    ///   its declared type) before per-entry validation runs, so an
    ///   edge to a sibling created in the same batch gets full
    ///   target-type shape validation, no transient stub, and no stub
    ///   warning — the batch validates as one graph state, cycles
    ///   included where the schema permits them. Duplicates within the
    ///   batch are refused in the identity pass.
    ///
    /// One workspace load (the caller's), one commit per touched mem
    /// (subject `memstead: batch-create (N entities)`), per-entry
    /// provenance notes exactly like `batch_update`.
    ///
    /// **Rehearsal** (`dry_run: true`): the FULL validation pass runs —
    /// identity, skeleton staging (so intra-batch references resolve as
    /// real targets, cycles included), per-entry prepare, report-all
    /// refusals — then the batch stops before any write. A legal batch
    /// returns the would-be receipt (`applied: true`, per-entry
    /// `"created"` with the prospective ids) with the marker form's
    /// empty `write_id`; an illegal one returns the same refusal a
    /// real call would. Nothing is written, committed, or stubbed.
    pub fn batch_create(
        &mut self,
        creates: Vec<(CreateEntityArgs, Option<String>)>,
        actor: Actor,
        client: Option<&ClientId>,
        dry_run: bool,
    ) -> Result<crate::ops::BatchResult, EngineError> {
        use std::collections::HashSet;

        if creates.is_empty() {
            return Ok(crate::ops::BatchResult {
                warnings: Vec::new(),
                orphan_stubs_removed: Vec::new(),
                errors_suppressed: 0,
                applied: true,
                results: Vec::new(),
                succeeded: 0,
                failed: 0,
                write_id: String::new(),
            });
        }

        // Reload every touched mem once, up front.
        let mut touched_mems: Vec<String> = creates.iter().map(|(a, _)| a.mem.clone()).collect();
        touched_mems.sort();
        touched_mems.dedup();
        for m in &touched_mems {
            self.reload_if_stale(Some(m));
        }
        // Same acyclic-guard rule as the single-item path: declared
        // relations on an ACYCLIC rel-type (or one in an
        // `acyclic_sets` set) walk the whole subgraph, so the walk
        // must see every mem — deferred ones included (see the
        // batch_relate comment). Declared signals on any involved
        // schema need the full load too (see the single-item path).
        if creates.iter().any(|(a, _)| {
            a.relations.iter().any(|r| {
                self.schemas.get(&a.mem).is_some_and(|s| {
                    s.relationship_acyclic(&r.rel_type)
                        || s.acyclic_set_containing(&r.rel_type).is_some()
                }) || self
                    .schemas
                    .get(r.target.mem())
                    .is_some_and(|s| s.types.values().any(|td| !td.signals.is_empty()))
            }) || self
                .schemas
                .get(&a.mem)
                .is_some_and(|s| s.types.values().any(|td| !td.signals.is_empty()))
        }) {
            self.ensure_mems_loaded(None);
        }

        let store_snapshot = self.store.clone();

        // --- Identity pass: derive every entry's id, refusing
        // duplicates against the pre-batch store AND within the batch.
        // Collect EVERY failure (report-all), never just the first.
        struct IdentityRow {
            id: Option<EntityId>,
            error: Option<EngineError>,
        }
        let mut rows: Vec<IdentityRow> = Vec::with_capacity(creates.len());
        // id → title of the batch entry that claimed it, so a
        // within-batch duplicate can name the occupying title.
        let mut batch_ids: HashMap<EntityId, String> = HashMap::new();
        for (args, _) in &creates {
            let identity = (|| -> Result<EntityId, EngineError> {
                let title = args.title.trim();
                // Divergence warnings ride the per-entry prepare pass
                // below, which re-derives; this pass only needs the id.
                let slug = validate_and_derive_slug(title)?.slug;
                let id = EntityId::new(&args.mem, &slug);
                crate::entity::id::enforce_id_length(id.as_ref())?;
                if let Some(existing) = self.store.get(&id)
                    && !existing.stub
                {
                    return Err(EngineError::AlreadyExists {
                        id: id.to_string(),
                        existing_title: existing.title.clone(),
                        existing_is_stub: false,
                    });
                }
                if let Some(prior_title) = batch_ids.get(&id) {
                    // Duplicate WITHIN the batch — same typed code as
                    // the store collision; the index in the report
                    // localises it.
                    return Err(EngineError::AlreadyExists {
                        id: id.to_string(),
                        existing_title: prior_title.clone(),
                        existing_is_stub: false,
                    });
                }
                Ok(id)
            })();
            match identity {
                Ok(id) => {
                    batch_ids.insert(id.clone(), args.title.trim().to_string());
                    rows.push(IdentityRow {
                        id: Some(id),
                        error: None,
                    });
                }
                Err(e) => rows.push(IdentityRow {
                    id: None,
                    error: Some(e),
                }),
            }
        }

        // --- Skeleton staging: make every batch id a REAL, typed store
        // entry so sibling references validate against present targets.
        // A pre-existing stub at a batch id is replaced (its incoming
        // edges survive the upsert — the same adoption the single-item
        // create performs).
        for ((args, _), row) in creates.iter().zip(rows.iter()) {
            if let Some(id) = &row.id {
                let mut skeleton = make_stub(id, crate::entity::StubKind::ForwardReference);
                skeleton.stub = false;
                skeleton.stub_kind = None;
                skeleton.entity_type = args.entity_type.clone();
                skeleton.title = args.title.trim().to_string();
                self.store.upsert(id.clone(), skeleton);
            }
        }

        // --- Full prepare pass, report-all. Skeletons make intra-batch
        // targets real; each entry's own skeleton is exempted from the
        // duplicate check via `batch_skeleton_ids`.
        let mut prepared: Vec<PreparedCreate> = Vec::new();
        let mut notes: Vec<Option<String>> = Vec::new();
        let mut errors: Vec<(usize, EngineError)> = Vec::new();
        let mut ids_in_order: Vec<EntityId> = Vec::new();
        let skeleton_ids: HashSet<EntityId> = batch_ids.keys().cloned().collect();
        for (i, ((args, note), row)) in creates.into_iter().zip(rows).enumerate() {
            let fallback_id = row
                .id
                .clone()
                .unwrap_or_else(|| EntityId::new(&args.mem, "invalid-entry"));
            ids_in_order.push(fallback_id);
            if let Some(e) = row.error {
                errors.push((i, e));
                continue;
            }
            // Rehearsal is batch-level (the `dry_run` parameter) —
            // per-entry dry-run stays forced off so the prepare pass
            // below never short-circuits into a per-entry preview.
            let mut args = args;
            args.dry_run = false;
            match self.prepare_create(args, Some(&skeleton_ids), Vec::new()) {
                Ok(CreatePrepareOutcome::Prepared(p)) => {
                    // Stage this item's declared edges onto its skeleton
                    // so later items validate against the batch's own
                    // graph state — an intra-batch cycle on an acyclic
                    // rel-type refuses exactly like a stored one
                    // (`validate_edge_acyclicity` walks the store). The
                    // snapshot rollback discards these on refusal; the
                    // apply pass replaces them with the parsed truth.
                    for r in &p.relations_declared {
                        self.store.add_edge(
                            p.id.clone(),
                            crate::store::Edge {
                                rel_type: r.rel_type.clone(),
                                target: r.target.clone(),
                                source: crate::store::EdgeSource::Explicit,
                            },
                        );
                    }
                    ids_in_order[i] = p.id.clone();
                    prepared.push(p);
                    notes.push(note);
                }
                Ok(CreatePrepareOutcome::Done(_)) => unreachable!("dry_run forced off"),
                Err(e) => errors.push((i, e)),
            }
        }

        if !errors.is_empty() {
            // Refuse the whole batch; nothing was committed and the
            // store snapshot rolls back the skeletons.
            self.store = store_snapshot;
            self.discard_all_pending();
            let failed = errors.len();
            let mut error_map: std::collections::HashMap<usize, EngineError> =
                errors.into_iter().collect();
            let mut reported = 0usize;
            let mut suppressed = 0usize;
            let results: Vec<crate::ops::BatchEntry> = ids_in_order
                .into_iter()
                .enumerate()
                .map(|(i, id)| match error_map.remove(&i) {
                    Some(e) => {
                        if reported < Self::BATCH_ERROR_REPORT_CAP {
                            reported += 1;
                            crate::ops::BatchEntry {
                                id,
                                action: "error".to_string(),
                                error: Some(super::update::batch_error_envelope(&e)),
                            }
                        } else {
                            suppressed += 1;
                            crate::ops::BatchEntry {
                                id,
                                action: "error".to_string(),
                                error: None,
                            }
                        }
                    }
                    None => crate::ops::BatchEntry {
                        id,
                        action: "not_applied".to_string(),
                        error: None,
                    },
                })
                .collect();
            return Ok(crate::ops::BatchResult {
                warnings: Vec::new(),
                orphan_stubs_removed: Vec::new(),
                errors_suppressed: suppressed,
                applied: false,
                results,
                succeeded: 0,
                failed,
                write_id: String::new(),
            });
        }

        // Rehearsal: every entry validated against the batch's own
        // graph state (skeletons made intra-batch targets real) and
        // nothing failed — stop before any write. Roll back the
        // skeleton staging and return the would-be receipt with the
        // marker form's empty `write_id`.
        if dry_run {
            self.store = store_snapshot;
            self.discard_all_pending();
            let succeeded = prepared.len();
            let results: Vec<crate::ops::BatchEntry> = prepared
                .into_iter()
                .map(|p| crate::ops::BatchEntry {
                    id: p.id,
                    action: "created".to_string(),
                    error: None,
                })
                .collect();
            return Ok(crate::ops::BatchResult {
                warnings: Vec::new(),
                orphan_stubs_removed: Vec::new(),
                errors_suppressed: 0,
                applied: true,
                results,
                succeeded,
                failed: 0,
                write_id: String::new(),
            });
        }

        // --- Stage every write + anchors, then commit once per mem.
        for p in &prepared {
            if let Err(e) = self.mounts[p.mount_idx]
                .backend
                .write_entity(Path::new(&p.file_path), p.markdown.as_bytes())
            {
                self.store = store_snapshot;
                self.discard_all_pending();
                return Err(e.into());
            }
            if !p.anchors.is_empty()
                && let Err(e) = super::stage_anchors_sidecar(
                    self.mounts[p.mount_idx].backend.as_ref(),
                    &p.id,
                    &[],
                    p.anchors.clone(),
                    true,
                )
            {
                self.store = store_snapshot;
                self.discard_all_pending();
                return Err(e);
            }
            // Derivation baselines (plan 12) — same predicate and
            // staging as the single create; rides the batch commit.
            if let Some(schema) = self.schemas.get(&p.mem) {
                for r in p
                    .relations_declared
                    .iter()
                    .filter(|r| super::rel_type_declares_derivation(schema, &r.rel_type))
                {
                    let hash = self
                        .store
                        .get(&r.target)
                        .map(|e| e.content_hash.clone())
                        .unwrap_or_default();
                    let (from, rel, to) =
                        (p.id.to_string(), r.rel_type.clone(), r.target.to_string());
                    if let Err(e) = super::stage_derivation_sidecar(
                        self.mounts[p.mount_idx].backend.as_ref(),
                        |s| s.set(&from, &rel, &to, &hash),
                    ) {
                        self.store = store_snapshot;
                        self.discard_all_pending();
                        return Err(e);
                    }
                }
            }
        }
        let mut distinct_mounts: Vec<usize> = Vec::new();
        for p in &prepared {
            if !distinct_mounts.contains(&p.mount_idx) {
                distinct_mounts.push(p.mount_idx);
            }
        }
        let mut mount_commits: Vec<(usize, String)> = Vec::with_capacity(distinct_mounts.len());
        for &m in &distinct_mounts {
            let entity_ids: Vec<String> = prepared
                .iter()
                .filter(|p| p.mount_idx == m)
                .map(|p| p.id.to_string())
                .collect();
            let count = entity_ids.len();
            let subject = format!("memstead: batch-create ({count} entities)");
            // Per-entry notes ride the ONE batch commit's note record as
            // `<id>: <note>` lines (decision 3, backlog-sweep plan 05):
            // `append_provenance` below is a documented no-op on the
            // git-branch backend, so without this the notes survived
            // nowhere exactly where most writes happen. A batch with no
            // notes carries no note record at all.
            let note_lines: Vec<String> = prepared
                .iter()
                .zip(notes.iter())
                .filter(|(p, _)| p.mount_idx == m)
                .filter_map(|(p, n)| n.as_ref().map(|n| format!("{}: {n}", p.id)))
                .collect();
            let ctx = CommitContext {
                actor,
                client: client.cloned(),
                tool: Some("batch_create"),
                note: if note_lines.is_empty() {
                    None
                } else {
                    Some(note_lines.join("\n"))
                },
                role: self.current_role,
                identity: self.current_identity.clone(),
                logical_operation_id: None,
                entity_ids: Some(entity_ids),
            };
            match self.mounts[m].backend.commit(&subject, &ctx) {
                Ok(sha) => mount_commits.push((m, sha)),
                Err(e) => {
                    self.store = store_snapshot;
                    self.discard_all_pending();
                    return Err(e.into());
                }
            }
        }

        // Provenance + store application (parse the generated bytes so
        // the store mirrors disk, replacing the skeletons).
        let fallback = engine_fallback_type();
        let mut batch_warnings: Vec<WarningHint> = Vec::new();
        for (p, note) in prepared.iter().zip(notes.iter()) {
            let write_id = mount_commits
                .iter()
                .find(|(m, _)| *m == p.mount_idx)
                .map(|(_, s)| s.clone())
                .unwrap_or_default();
            self.mounts[p.mount_idx].backend.append_provenance(
                &Provenance::new(
                    std::time::SystemTime::now(),
                    ProvenanceKind::Create,
                    Some(p.id.to_string()),
                    actor,
                    client.cloned(),
                    note.clone(),
                )
                .with_role(self.current_role)
                .with_identity(self.current_identity.clone()),
            )?;
            self.record_self_write(p.mount_idx, &write_id);
            batch_warnings.extend(self.stamp_mutation_versions(p.mount_idx));
            let parse_result =
                parse_markdown(&p.markdown, &p.file_path, p.type_def.as_ref(), &p.mem)
                    .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
            push_entities_into_store(&mut self.store, vec![parse_result], fallback.as_ref(), None);
        }
        crate::entity::store_builder::remap_alias_target_edge_sources(
            &mut self.store,
            &self.schemas,
        );
        // Forward-reference stubs for OUT-OF-BATCH targets only —
        // in-batch targets are real entities now.
        let mut out_of_batch_stubs: Vec<(EntityId, crate::entity::StubKind)> = Vec::new();
        for p in &prepared {
            for target in &p.relation_targets {
                if !self.store.contains(target) {
                    let kind = super::deferred_verified_stub_kind(self, target)?;
                    out_of_batch_stubs.push((target.clone(), kind));
                }
            }
        }
        for (target, kind) in out_of_batch_stubs {
            self.store.upsert(target.clone(), make_stub(&target, kind));
        }
        self.invalidate_communities();
        self.invalidate_search_indexes();

        let write_id = mount_commits
            .last()
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        let succeeded = prepared.len();
        let results: Vec<crate::ops::BatchEntry> = prepared
            .into_iter()
            .map(|p| crate::ops::BatchEntry {
                id: p.id,
                action: "created".to_string(),
                error: None,
            })
            .collect();
        Ok(crate::ops::BatchResult {
            warnings: batch_warnings,
            orphan_stubs_removed: Vec::new(),
            errors_suppressed: 0,
            applied: true,
            results,
            succeeded,
            failed: 0,
            write_id,
        })
    }

    /// Cap on fully-detailed error envelopes in a refused batch's
    /// report — bounded reporting for very large failing batches.
    /// Entries beyond the cap still carry `action: "error"`; the
    /// result's `errors_suppressed` counts them. Never a silent
    /// truncation.
    pub const BATCH_ERROR_REPORT_CAP: usize = 50;

    /// CommitContext-bundling wrapper around [`Self::create_entity`].
    /// Destructures `CommitContext` into `(actor, client, note)`
    /// and delegates.
    pub fn create_entity_with_ctx(
        &mut self,
        args: CreateEntityArgs,
        ctx: &CommitContext<'_>,
    ) -> Result<CreateEntityOutcome, EngineError> {
        self.create_entity(args, ctx.actor, ctx.client.as_ref(), ctx.note.as_deref())
    }
}

#[cfg(test)]
mod tests {

    use indexmap::IndexMap;
    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::engine::test_helpers::*;
    use crate::engine::{
        CreateEntityArgs, CreateEntityOutcome, Engine, EngineError, RelateEntityArgs,
    };
    use crate::ops::WarningHint;
    use crate::storage::{ArchiveBackend, FilesystemMemWriter};

    /// Boot an engine whose mem pins a schema with one type (`task`)
    /// declaring `required_outgoing: [{relationships: [PART_OF],
    /// cardinality: at_least_one}]` — the fixture for the
    /// MISSING_REQUIRED_OUTGOING mutation-warning tests.
    fn engine_with_required_outgoing_schema(tmp: &TempDir) -> Engine {
        let schemas_dir = tmp.path().join("schemas");
        let pkg = schemas_dir.join("reqout");
        std::fs::create_dir_all(pkg.join("types")).unwrap();
        std::fs::write(
            pkg.join("schema.yaml"),
            r#"name: reqout
version: 0.1.0
description: required-outgoing fixture
when_to_use: tests
types:
  - task
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#,
        )
        .unwrap();
        std::fs::write(
            pkg.join("types").join("task.yaml"),
            r#"name: task
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
required_outgoing:
  - relationships: [PART_OF]
    cardinality: at_least_one
write_rules: []
"#,
        )
        .unwrap();
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mount = crate::workspace::Mount {
            mem: "tasks".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "reqout",
                semver::Version::new(0, 1, 0),
            )),
            storage: crate::workspace::MountStorage::Folder { path: mem_dir },
            capability: crate::workspace::MountCapability::Write,
            lifecycle: crate::workspace::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        Engine::from_mounts_with_schemas_dir(
            vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
            Some(&schemas_dir),
        )
        .unwrap()
    }

    fn task_create_args(title: &str, relations: Vec<crate::ops::RelateArg>) -> CreateEntityArgs {
        let mut sections = IndexMap::new();
        sections.insert("body".to_string(), "a task body.".to_string());
        CreateEntityArgs {
            anchors: Vec::new(),
            mem: "tasks".to_string(),
            title: title.to_string(),
            entity_type: "task".to_string(),
            sections,
            metadata: IndexMap::new(),
            relations,
            dry_run: false,
        }
    }

    fn missing_outgoing_of(warnings: &[WarningHint]) -> Vec<(Vec<String>, String)> {
        warnings
            .iter()
            .filter_map(|w| match w {
                WarningHint::MissingRequiredOutgoing { missing, .. } => Some(
                    missing
                        .iter()
                        .map(|b| (b.relationships.clone(), b.cardinality.clone()))
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// Fixture for the declared-constraints vertical: type `task`
    /// declares `requires_when` (checked → checked_by) at the given
    /// severity, plus a `required_outgoing` block at the given
    /// severity — so one schema exercises form 1 and form 4 at either
    /// tier.
    fn engine_with_constraints_schema(
        tmp: &TempDir,
        requires_when_severity: &str,
        required_outgoing_severity: &str,
    ) -> Engine {
        let schemas_dir = tmp.path().join("schemas");
        let pkg = schemas_dir.join("constr");
        std::fs::create_dir_all(pkg.join("types")).unwrap();
        std::fs::write(
            pkg.join("schema.yaml"),
            r#"name: constr
version: 0.1.0
description: constraint fixture
when_to_use: tests
types:
  - task
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#,
        )
        .unwrap();
        std::fs::write(
            pkg.join("types").join("task.yaml"),
            format!(
                r#"name: task
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: status
    description: workflow state
    field_type: string
    enum_values: [open, checked]
  - key: checked_by
    description: who checked
    field_type: string
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: [PART_OF]
updatable_fields:
  - title
  - body
  - status
  - checked_by
health_required_fields:
  - body
staleness_threshold_days: 90
required_outgoing:
  - relationships: [PART_OF]
    cardinality: at_least_one
    severity: {required_outgoing_severity}
constraints:
  - kind: requires_when
    field: checked_by
    when_field: status
    when_value: checked
    severity: {requires_when_severity}
write_rules: []
"#
            ),
        )
        .unwrap();
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mount = crate::workspace::Mount {
            mem: "tasks".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "constr",
                semver::Version::new(0, 1, 0),
            )),
            storage: crate::workspace::MountStorage::Folder { path: mem_dir },
            capability: crate::workspace::MountCapability::Write,
            lifecycle: crate::workspace::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        Engine::from_mounts_with_schemas_dir(
            vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
            Some(&schemas_dir),
        )
        .unwrap()
    }

    fn checked_task_args(title: &str, relations: Vec<crate::ops::RelateArg>) -> CreateEntityArgs {
        let mut args = task_create_args(title, relations);
        args.metadata
            .insert("status".to_string(), "checked".to_string());
        args
    }

    /// Form 1 at warn: a create violating `requires_when` warns
    /// `CONSTRAINT_UNSATISFIED` and still commits; the health sweep
    /// reports the same violation (shared evaluation); a create
    /// satisfying the constraint emits neither.
    #[test]
    fn create_warns_requires_when_and_still_commits() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_constraints_schema(&tmp, "warn", "warn");
        let (actor, client) = cli_actor();

        let outcome = engine
            .create_entity(
                checked_task_args("Unbacked Judgment", vec![]),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(!outcome.write_id.is_empty(), "warn tier never blocks");
        let violation = outcome
            .warnings
            .iter()
            .find_map(|w| match w {
                WarningHint::ConstraintUnsatisfied { violations, .. } => Some(violations.clone()),
                _ => None,
            })
            .expect("CONSTRAINT_UNSATISFIED warning present");
        assert_eq!(violation.len(), 1);
        let crate::ops::health::UnsatisfiedConstraint::RequiresWhen {
            field,
            when_field,
            when_value,
            ..
        } = &violation[0]
        else {
            panic!("expected requires_when violation");
        };
        assert_eq!(field, "checked_by");
        assert_eq!(when_field, "status");
        assert_eq!(when_value, "checked");

        // Health parity — same single evaluation.
        let reports = crate::ops::health::collect_constraint_findings(
            engine.store(),
            None,
            engine.schemas(),
            None,
        );
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].id, outcome.id);
        assert_eq!(reports[0].violations.len(), 1);
        let crate::ops::health::UnsatisfiedConstraint::RequiresWhen { field, .. } =
            &reports[0].violations[0]
        else {
            panic!("expected requires_when finding");
        };
        assert_eq!(field, "checked_by");

        // Complement 1: satisfying the constraint in the same create
        // emits no warning and no finding.
        let mut satisfied_args = checked_task_args("Backed Judgment", vec![]);
        satisfied_args
            .metadata
            .insert("checked_by".to_string(), "reviewer-a".to_string());
        let satisfied = engine
            .create_entity(satisfied_args, actor, Some(&client), None)
            .unwrap();
        assert!(
            !satisfied
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::ConstraintUnsatisfied { .. })),
            "satisfied constraint emits no warning: {:?}",
            satisfied.warnings
        );

        // Complement 2: an untriggered constraint (status != checked)
        // emits nothing even with checked_by unset.
        let untriggered = engine
            .create_entity(
                task_create_args("Open Task", vec![]),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(
            !untriggered
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::ConstraintUnsatisfied { .. })),
            "untriggered constraint emits no warning"
        );
    }

    /// Form 1 at block: the same violation refuses the create with
    /// `CONSTRAINT_UNSATISFIED`, leaves nothing behind, and the
    /// refusal payload restates the declaration.
    #[test]
    fn create_refuses_block_tier_requires_when() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_constraints_schema(&tmp, "block", "warn");
        let (actor, client) = cli_actor();

        let err = engine
            .create_entity(
                checked_task_args("Unbacked Judgment", vec![]),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED");
        let details = err.details();
        assert_eq!(details["violations"][0]["field"], "checked_by");
        assert_eq!(details["violations"][0]["severity"], "block");
        assert_eq!(
            engine.store().all_entities().count(),
            0,
            "refused create leaves nothing behind"
        );

        // The satisfying create passes under the same schema.
        let mut ok_args = checked_task_args("Backed Judgment", vec![]);
        ok_args
            .metadata
            .insert("checked_by".to_string(), "reviewer-a".to_string());
        engine
            .create_entity(ok_args, actor, Some(&client), None)
            .unwrap();
    }

    /// Form 4 at block: a create leaving a `severity: block`
    /// `required_outgoing` block unsatisfied refuses with
    /// `MISSING_REQUIRED_OUTGOING` (the same code the warn tier
    /// warns with — one condition, one vocabulary); an inline
    /// relation satisfying the block lets the create pass.
    #[test]
    fn create_refuses_block_tier_required_outgoing() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_constraints_schema(&tmp, "warn", "block");
        let (actor, client) = cli_actor();

        let err = engine
            .create_entity(
                task_create_args("Orphan Task", vec![]),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "MISSING_REQUIRED_OUTGOING");
        let details = err.details();
        assert_eq!(details["missing"][0]["relationships"][0], "PART_OF");
        assert_eq!(details["missing"][0]["severity"], "block");
        assert_eq!(engine.store().all_entities().count(), 0);

        // A create satisfying the block via an inline relation to an
        // auto-stubbed target passes — the stub itself has no type
        // definition under this schema's `task`-only vocabulary, so
        // wire the edge from the real entity.
        let outcome = engine.create_entity(
            task_create_args(
                "Child Task",
                vec![crate::ops::RelateArg {
                    target: crate::entity::EntityId("tasks--parent".to_string()),
                    rel_type: "PART_OF".to_string(),
                    description: None,
                }],
            ),
            actor,
            Some(&client),
            None,
        );
        assert!(
            outcome.is_ok(),
            "satisfied block-tier create passes: {:?}",
            outcome.err()
        );
    }

    /// Update-side severity mirror for form 1: at warn, an update
    /// that makes the constraint trigger warns and commits; at block,
    /// the same update refuses and the entity keeps its prior state.
    #[test]
    fn update_enforces_requires_when_by_severity() {
        let (actor, client) = cli_actor();
        let set_checked = |engine: &mut Engine, id: &crate::entity::EntityId| {
            let current = engine.get_entity(id).unwrap().content_hash.clone();
            let mut metadata = IndexMap::new();
            metadata.insert("status".to_string(), "checked".to_string());
            engine.update_entity(
                crate::engine::UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: id.clone(),
                    expected_hash: Some(current),
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    sections_unset: Vec::new(),
                    metadata,
                    metadata_unset: Vec::new(),
                    declare_relations: vec![],
                    dry_run: false,
                    relations_unset: Vec::new(),
                    anchors_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
        };

        // Warn tier: the update commits with the typed warning.
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_constraints_schema(&tmp, "warn", "warn");
        let a = engine
            .create_entity(
                task_create_args("Task A", vec![]),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let outcome = set_checked(&mut engine, &a.id).unwrap();
        assert!(!outcome.write_id.is_empty());
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::ConstraintUnsatisfied { .. })),
            "warn-tier update carries the warning: {:?}",
            outcome.warnings
        );

        // Block tier: the same update refuses; the entity keeps its
        // prior metadata.
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_constraints_schema(&tmp, "block", "warn");
        let b = engine
            .create_entity(
                task_create_args("Task B", vec![]),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let err = set_checked(&mut engine, &b.id).unwrap_err();
        assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED");
        assert!(
            !engine
                .get_entity(&b.id)
                .unwrap()
                .metadata
                .contains_key("status"),
            "refused update leaves the entity unchanged"
        );
    }

    /// Form 4 at block on the relate surface: removing the edge that
    /// satisfies a `severity: block` `required_outgoing` block refuses
    /// with `MISSING_REQUIRED_OUTGOING`; the edge survives.
    #[test]
    fn relate_remove_refuses_block_tier_required_outgoing() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_constraints_schema(&tmp, "warn", "block");
        let (actor, client) = cli_actor();
        let parent_id = crate::entity::EntityId("tasks--parent".to_string());
        let child = engine
            .create_entity(
                task_create_args(
                    "Child Task",
                    vec![crate::ops::RelateArg {
                        target: parent_id.clone(),
                        rel_type: "PART_OF".to_string(),
                        description: None,
                    }],
                ),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let err = engine
            .relate_entity(
                crate::engine::RelateEntityArgs {
                    source: child.id.clone(),
                    target: parent_id.clone(),
                    rel_type: "PART_OF".to_string(),
                    description: None,
                    remove: true,
                    expected_hash: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "MISSING_REQUIRED_OUTGOING");
        assert!(
            engine
                .get_entity(&child.id)
                .unwrap()
                .relationships
                .iter()
                .any(|r| r.rel_type == "PART_OF" && r.target == parent_id),
            "refused remove leaves the edge in place"
        );
    }

    /// Fixture for the conditional `required_outgoing` form: type
    /// `task` requires a PART_OF edge only while `status` holds
    /// `checked`, at the given severity. No unconditional blocks, no
    /// `constraints` — the conditional block is the only obligation.
    fn engine_with_conditional_ro_schema(tmp: &TempDir, severity: &str) -> Engine {
        let schemas_dir = tmp.path().join("schemas");
        let pkg = schemas_dir.join("condro");
        std::fs::create_dir_all(pkg.join("types")).unwrap();
        std::fs::write(
            pkg.join("schema.yaml"),
            r#"name: condro
version: 0.1.0
description: conditional required_outgoing fixture
when_to_use: tests
types:
  - task
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#,
        )
        .unwrap();
        std::fs::write(
            pkg.join("types").join("task.yaml"),
            format!(
                r#"name: task
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: status
    description: workflow state
    field_type: string
    enum_values: [open, checked]
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
  - status
health_required_fields:
  - body
staleness_threshold_days: 90
required_outgoing:
  - relationships: [PART_OF]
    cardinality: at_least_one
    severity: {severity}
    when_field: status
    when_value: checked
write_rules: []
"#
            ),
        )
        .unwrap();
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mount = crate::workspace::Mount {
            mem: "tasks".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "condro",
                semver::Version::new(0, 1, 0),
            )),
            storage: crate::workspace::MountStorage::Folder { path: mem_dir },
            capability: crate::workspace::MountCapability::Write,
            lifecycle: crate::workspace::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        Engine::from_mounts_with_schemas_dir(
            vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
            Some(&schemas_dir),
        )
        .unwrap()
    }

    /// An unarmed conditional block never fires: entities whose
    /// trigger field is unset or holds another enum value create
    /// cleanly without the edge, with no `MISSING_REQUIRED_OUTGOING`
    /// warning, even at block tier.
    #[test]
    fn create_ignores_conditional_required_outgoing_when_unarmed() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_conditional_ro_schema(&tmp, "block");
        let (actor, client) = cli_actor();

        let unset = engine
            .create_entity(
                task_create_args("Unset Status", vec![]),
                actor,
                Some(&client),
                None,
            )
            .expect("unset trigger field must create");
        let mut open_args = task_create_args("Open Task", vec![]);
        open_args
            .metadata
            .insert("status".to_string(), "open".to_string());
        let open = engine
            .create_entity(open_args, actor, Some(&client), None)
            .expect("non-trigger value must create");
        for outcome in [&unset, &open] {
            assert!(
                !outcome
                    .warnings
                    .iter()
                    .any(|w| matches!(w, WarningHint::MissingRequiredOutgoing { .. })),
                "unarmed block emits no warning: {:?}",
                outcome.warnings
            );
        }
    }

    /// Armed at block tier: a create whose trigger field holds the
    /// trigger value and lacks the edge refuses with
    /// `MISSING_REQUIRED_OUTGOING`, and the payload names the trigger
    /// (`when_field` / `when_value`); an inline relation satisfying
    /// the armed block lets the same create pass.
    #[test]
    fn create_refuses_block_tier_conditional_required_outgoing() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_conditional_ro_schema(&tmp, "block");
        let (actor, client) = cli_actor();

        let err = engine
            .create_entity(
                checked_task_args("Checked Orphan", vec![]),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "MISSING_REQUIRED_OUTGOING");
        let details = err.details();
        assert_eq!(details["missing"][0]["relationships"][0], "PART_OF");
        assert_eq!(details["missing"][0]["when_field"], "status");
        assert_eq!(details["missing"][0]["when_value"], "checked");
        assert_eq!(engine.store().all_entities().count(), 0);

        let outcome = engine.create_entity(
            checked_task_args(
                "Checked Child",
                vec![crate::ops::RelateArg {
                    target: crate::entity::EntityId("tasks--parent".to_string()),
                    rel_type: "PART_OF".to_string(),
                    description: None,
                }],
            ),
            actor,
            Some(&client),
            None,
        );
        assert!(
            outcome.is_ok(),
            "satisfied armed block passes: {:?}",
            outcome.err()
        );
    }

    /// Armed at warn tier: the create lands and carries the
    /// `MISSING_REQUIRED_OUTGOING` warning whose block entry names
    /// the trigger.
    #[test]
    fn create_warns_conditional_required_outgoing_at_warn_tier() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_conditional_ro_schema(&tmp, "warn");
        let (actor, client) = cli_actor();

        let outcome = engine
            .create_entity(
                checked_task_args("Checked Orphan", vec![]),
                actor,
                Some(&client),
                None,
            )
            .expect("warn tier lands the write");
        assert!(!outcome.write_id.is_empty());
        let block = outcome
            .warnings
            .iter()
            .find_map(|w| match w {
                WarningHint::MissingRequiredOutgoing { missing, .. } => missing.first(),
                _ => None,
            })
            .expect("warning carries the unsatisfied block");
        assert_eq!(block.relationships, vec!["PART_OF".to_string()]);
        assert_eq!(block.when_field.as_deref(), Some("status"));
        assert_eq!(block.when_value.as_deref(), Some("checked"));
    }

    /// The metadata flip that arms the block is caught on update: at
    /// block tier the update refuses and the entity keeps its prior
    /// value; at warn tier the same flip commits with the warning.
    #[test]
    fn update_flip_to_trigger_value_enforces_conditional_block() {
        let (actor, client) = cli_actor();
        let flip_to_checked = |engine: &mut Engine, id: &crate::entity::EntityId| {
            let current = engine.get_entity(id).unwrap().content_hash.clone();
            let mut metadata = IndexMap::new();
            metadata.insert("status".to_string(), "checked".to_string());
            engine.update_entity(
                crate::engine::UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: id.clone(),
                    expected_hash: Some(current),
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    sections_unset: Vec::new(),
                    metadata,
                    metadata_unset: Vec::new(),
                    declare_relations: vec![],
                    dry_run: false,
                    relations_unset: Vec::new(),
                    anchors_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
        };

        // Block tier: the flip refuses; the entity keeps `open`.
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_conditional_ro_schema(&tmp, "block");
        let mut open_args = task_create_args("Task A", vec![]);
        open_args
            .metadata
            .insert("status".to_string(), "open".to_string());
        let a = engine
            .create_entity(open_args, actor, Some(&client), None)
            .unwrap();
        let err = flip_to_checked(&mut engine, &a.id).unwrap_err();
        assert_eq!(err.code(), "MISSING_REQUIRED_OUTGOING");
        assert_eq!(
            engine.get_entity(&a.id).unwrap().metadata["status"].to_frontmatter_string(),
            "open",
            "refused update leaves the entity unchanged"
        );

        // Warn tier: the same flip commits with the typed warning.
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_conditional_ro_schema(&tmp, "warn");
        let b = engine
            .create_entity(
                task_create_args("Task B", vec![]),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let outcome = flip_to_checked(&mut engine, &b.id).unwrap();
        assert!(!outcome.write_id.is_empty());
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::MissingRequiredOutgoing { .. })),
            "warn-tier flip carries the warning: {:?}",
            outcome.warnings
        );
    }

    /// Fixture for declared acyclicity sets: one `claim` type over
    /// GROUNDS / CONCLUDES, with `acyclic_sets: [[GROUNDS, CONCLUDES]]`
    /// when `with_set` (neither rel-type carries the per-definition
    /// `acyclic` flag, so without the set every cycle is legal).
    fn engine_with_acyclic_set_schema(tmp: &TempDir, with_set: bool) -> Engine {
        let schemas_dir = tmp.path().join("schemas");
        let pkg = schemas_dir.join("argchain");
        std::fs::create_dir_all(pkg.join("types")).unwrap();
        let sets = if with_set {
            "  acyclic_sets:\n    - [GROUNDS, CONCLUDES]\n"
        } else {
            ""
        };
        std::fs::write(
            pkg.join("schema.yaml"),
            format!(
                r#"name: argchain
version: 0.1.0
description: acyclicity-set fixture
when_to_use: tests
types:
  - claim
relationships:
  mode: strict
{sets}  definitions:
    - name: GROUNDS
      description: g
      default_weight: 3.0
    - name: CONCLUDES
      description: c
      default_weight: 3.0
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#
            ),
        )
        .unwrap();
        std::fs::write(
            pkg.join("types").join("claim.yaml"),
            r#"name: claim
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#,
        )
        .unwrap();
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mount = crate::workspace::Mount {
            mem: "arg".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "argchain",
                semver::Version::new(0, 1, 0),
            )),
            storage: crate::workspace::MountStorage::Folder { path: mem_dir },
            capability: crate::workspace::MountCapability::Write,
            lifecycle: crate::workspace::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        Engine::from_mounts_with_schemas_dir(
            vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
            Some(&schemas_dir),
        )
        .unwrap()
    }

    fn claim_args(title: &str, relations: Vec<crate::ops::RelateArg>) -> CreateEntityArgs {
        let mut sections = IndexMap::new();
        sections.insert("body".to_string(), "a claim body.".to_string());
        CreateEntityArgs {
            anchors: Vec::new(),
            mem: "arg".to_string(),
            title: title.to_string(),
            entity_type: "claim".to_string(),
            sections,
            metadata: IndexMap::new(),
            relations,
            dry_run: false,
        }
    }

    fn relate(
        engine: &mut Engine,
        from: &str,
        rel: &str,
        to: &str,
    ) -> Result<crate::engine::RelateEntityOutcome, crate::engine::EngineError> {
        let (actor, client) = cli_actor();
        engine.relate_entity(
            crate::engine::RelateEntityArgs {
                source: crate::entity::EntityId(from.to_string()),
                target: crate::entity::EntityId(to.to_string()),
                rel_type: rel.to_string(),
                description: None,
                remove: false,
                expected_hash: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
    }

    /// The experiment's alternating cycle: with `[GROUNDS, CONCLUDES]`
    /// declared as one acyclicity set, the relate that closes a cycle
    /// mixing both rel-types refuses with `RELATIONSHIP_CYCLE`; the
    /// payload echoes the set and names each hop's rel-type. The same
    /// graph WITHOUT the set declaration accepts the cycle (no
    /// implicit derivation from anything else).
    #[test]
    fn relate_refuses_mixed_type_cycle_in_declared_set() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_acyclic_set_schema(&tmp, true);
        let (actor, client) = cli_actor();
        for t in ["A", "B", "C"] {
            engine
                .create_entity(claim_args(t, vec![]), actor, Some(&client), None)
                .unwrap();
        }
        relate(&mut engine, "arg--a", "GROUNDS", "arg--b").unwrap();
        relate(&mut engine, "arg--b", "CONCLUDES", "arg--c").unwrap();

        let err = relate(&mut engine, "arg--c", "GROUNDS", "arg--a").unwrap_err();
        assert_eq!(err.code(), "RELATIONSHIP_CYCLE", "{err:?}");
        let details = err.details();
        assert_eq!(
            details["acyclic_set"],
            serde_json::json!(["GROUNDS", "CONCLUDES"])
        );
        assert_eq!(
            details["existing_path"],
            serde_json::json!(["arg--a", "arg--b", "arg--c"])
        );
        assert_eq!(
            details["existing_path_rel_types"],
            serde_json::json!(["GROUNDS", "CONCLUDES"]),
            "one rel-type per hop, mixing both members"
        );

        // Complement: the identical graph without the declaration
        // accepts the cycle.
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_acyclic_set_schema(&tmp, false);
        for t in ["A", "B", "C"] {
            engine
                .create_entity(claim_args(t, vec![]), actor, Some(&client), None)
                .unwrap();
        }
        relate(&mut engine, "arg--a", "GROUNDS", "arg--b").unwrap();
        relate(&mut engine, "arg--b", "CONCLUDES", "arg--c").unwrap();
        relate(&mut engine, "arg--c", "GROUNDS", "arg--a")
            .expect("without the set declaration the cycle is legal");
    }

    /// The set refusal fires identically for inline relations on
    /// create (through a promoted stub) and declared relations on
    /// update.
    #[test]
    fn create_inline_and_update_declared_refuse_set_cycle() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_acyclic_set_schema(&tmp, true);
        let (actor, client) = cli_actor();

        // create.relations[]: A → GROUNDS → ghost auto-stubs `ghost`;
        // promoting the stub with a CONCLUDES back-edge closes a
        // mixed-type cycle.
        engine
            .create_entity(
                claim_args(
                    "Alpha",
                    vec![crate::ops::RelateArg {
                        target: crate::entity::EntityId("arg--ghost".to_string()),
                        rel_type: "GROUNDS".to_string(),
                        description: None,
                    }],
                ),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let err = engine
            .create_entity(
                claim_args(
                    "Ghost",
                    vec![crate::ops::RelateArg {
                        target: crate::entity::EntityId("arg--alpha".to_string()),
                        rel_type: "CONCLUDES".to_string(),
                        description: None,
                    }],
                ),
                actor,
                Some(&client),
                None,
            )
            .expect_err("cycle-closing create.relations[] must refuse");
        assert_eq!(err.code(), "RELATIONSHIP_CYCLE", "{err:?}");
        assert_eq!(
            err.details()["acyclic_set"],
            serde_json::json!(["GROUNDS", "CONCLUDES"])
        );

        // update.declare_relations: D → GROUNDS → E exists; updating E
        // with CONCLUDES → D closes the mixed cycle.
        for t in ["D", "E"] {
            engine
                .create_entity(claim_args(t, vec![]), actor, Some(&client), None)
                .unwrap();
        }
        relate(&mut engine, "arg--d", "GROUNDS", "arg--e").unwrap();
        let e_id = crate::entity::EntityId("arg--e".to_string());
        let current = engine.get_entity(&e_id).unwrap().content_hash.clone();
        let err = engine
            .update_entity(
                crate::engine::UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: e_id,
                    expected_hash: Some(current),
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    sections_unset: Vec::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: vec![crate::ops::RelateArg {
                        target: crate::entity::EntityId("arg--d".to_string()),
                        rel_type: "CONCLUDES".to_string(),
                        description: None,
                    }],
                    dry_run: false,
                    relations_unset: Vec::new(),
                    anchors_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .expect_err("cycle-closing declare_relations must refuse");
        assert_eq!(err.code(), "RELATIONSHIP_CYCLE", "{err:?}");
        assert_eq!(
            err.details()["acyclic_set"],
            serde_json::json!(["GROUNDS", "CONCLUDES"])
        );
    }

    /// A package whose on-disk edges already close a mixed-type cycle
    /// boots with one cycle-closing edge dropped and warned, exactly
    /// as the single-type sweep does.
    #[test]
    fn boot_drops_cycle_closing_edge_in_declared_acyclic_set() {
        let tmp = TempDir::new().unwrap();
        // Write the schema and two claim files closing a GROUNDS /
        // CONCLUDES cycle BEFORE boot, then reuse the fixture builder
        // (same schemas dir and mem dir layout).
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(
            mem_dir.join("a.md"),
            "---\ntype: claim\n---\n# A\n\n## Body\n\nfirst.\n\n## Relationships\n\n- **GROUNDS**: [[arg--b]]\n",
        )
        .unwrap();
        std::fs::write(
            mem_dir.join("b.md"),
            "---\ntype: claim\n---\n# B\n\n## Body\n\nsecond.\n\n## Relationships\n\n- **CONCLUDES**: [[arg--a]]\n",
        )
        .unwrap();
        let engine = engine_with_acyclic_set_schema(&tmp, true);

        let surviving: usize = engine
            .store()
            .all_entities()
            .map(|e| {
                engine
                    .store()
                    .outgoing(&e.id)
                    .iter()
                    .filter(|edge| edge.rel_type == "GROUNDS" || edge.rel_type == "CONCLUDES")
                    .count()
            })
            .sum();
        assert_eq!(surviving, 1, "exactly one edge survives the cycle break");
        let cycle_warnings: Vec<_> = engine
            .load_warnings()
            .iter()
            .filter(|w| {
                matches!(
                    w,
                    WarningHint::ParsedRelationInvalid { reason, .. } if reason == "cycle"
                )
            })
            .cloned()
            .collect();
        assert_eq!(
            cycle_warnings.len(),
            1,
            "exactly one cycle warning fires: {cycle_warnings:?}"
        );
    }

    /// Fixture for aggregate signals: `claim` declares `attack_load`
    /// (in-REBUTS count, notice at 1, warn at 3) and
    /// `open_objections` (same set, counterpart `state: open` only,
    /// notice at 1); `objection` declares the `state` enum.
    /// Fixture for the grounded labelling: `arglab` declares
    /// `labelling.attack: [REBUTS]` and a support walk over GROUNDS
    /// (direction out, terminal `evidence`); mem `arg` (and, when the
    /// test mounts it, mem `other`) pin it.
    fn engine_with_labelling_schema(tmp: &TempDir, with_other_mem: bool) -> Engine {
        engine_with_labelling_schema_support(tmp, with_other_mem, true)
    }

    fn engine_with_labelling_schema_support(
        tmp: &TempDir,
        with_other_mem: bool,
        with_support: bool,
    ) -> Engine {
        let schemas_dir = tmp.path().join("schemas");
        let pkg = schemas_dir.join("arglab");
        std::fs::create_dir_all(pkg.join("types")).unwrap();
        std::fs::write(
            pkg.join("schema.yaml"),
            format!(
                r#"name: arglab
version: 0.1.0
description: grounded-labelling fixture
when_to_use: tests
types:
  - claim
  - evidence
relationships:
  mode: strict
  labelling:
    attack: [REBUTS]
{support}  definitions:
    - name: REBUTS
      description: attack
      default_weight: 3.0
    - name: GROUNDS
      description: support
      default_weight: 3.0
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
cross_mem_relationships:
  - to_schema: arglab
    definitions:
      - name: REBUTS
        description: cross-mem attack
        default_weight: 3.0
"#,
                support = if with_support {
                    "    support:\n      relationships: [GROUNDS]\n      direction: out\n      terminal_types: [evidence]\n"
                } else {
                    ""
                },
            ),
        )
        .unwrap();
        let body = "sections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n";
        for t in ["claim", "evidence"] {
            std::fs::write(
                pkg.join("types").join(format!("{t}.yaml")),
                format!("name: {t}\ndescription: t\nwhen_to_use: tests\n{body}"),
            )
            .unwrap();
        }
        let mut mounts: Vec<(crate::workspace::Mount, Box<dyn MemBackend>)> = Vec::new();
        for mem in std::iter::once("arg").chain(with_other_mem.then_some("other")) {
            let mem_dir = tmp.path().join(format!("mem-{mem}"));
            std::fs::create_dir_all(&mem_dir).unwrap();
            let writer = FilesystemMemWriter::new(mem_dir.clone());
            mounts.push((
                crate::workspace::Mount {
                    mem: mem.to_string(),
                    schema: Some(memstead_schema::SchemaRef::new(
                        "arglab",
                        semver::Version::new(0, 1, 0),
                    )),
                    storage: crate::workspace::MountStorage::Folder { path: mem_dir },
                    capability: crate::workspace::MountCapability::Write,
                    lifecycle: crate::workspace::MountLifecycle::Eager,
                    cross_linkable: true,
                    migration_target: None,
                },
                Box::new(writer) as Box<dyn MemBackend>,
            ));
        }
        Engine::from_mounts_with_schemas_dir(mounts, Some(&schemas_dir)).unwrap()
    }

    fn lab_create(engine: &mut Engine, mem: &str, title: &str, entity_type: &str) {
        let (actor, client) = cli_actor();
        let mut sections = IndexMap::new();
        sections.insert("body".to_string(), "a body.".to_string());
        engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: mem.to_string(),
                    title: title.to_string(),
                    entity_type: entity_type.to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: vec![],
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
    }

    fn lab_relate(engine: &mut Engine, from: &str, rel: &str, to: &str) {
        let (actor, client) = cli_actor();
        engine
            .relate_entity(
                crate::engine::RelateEntityArgs {
                    source: crate::entity::EntityId(from.to_string()),
                    target: crate::entity::EntityId(to.to_string()),
                    rel_type: rel.to_string(),
                    description: None,
                    remove: false,
                    expected_hash: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
    }

    fn label_of(engine: &Engine, id: &str) -> (String, Vec<String>, Vec<String>) {
        let entity = engine
            .get_entity(&crate::entity::EntityId(id.to_string()))
            .unwrap()
            .clone();
        let view = engine.computed_labelling(&entity).unwrap();
        (
            view.label.wire().to_string(),
            view.defeated_by.clone(),
            view.undecided_by.clone(),
        )
    }

    /// The hand-computed grounded extension: an unattacked claim is
    /// accepted, a chain defeats, a defeated attacker reinstates, a
    /// cycle stays undecided (and keeps its victims undecided); the
    /// evidence names the accepted / undecided direct attackers; two
    /// instances serve identical labels; the memo invalidates on the
    /// in-process mutation path AND the reload path; labels never
    /// gate writes; the health axis serves counts with evidence.
    #[test]
    fn grounded_labelling_extension_evidence_and_invalidation() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_labelling_schema(&tmp, false);
        for t in ["A", "B", "C", "D", "E", "F"] {
            lab_create(&mut engine, "arg", t, "claim");
        }
        lab_relate(&mut engine, "arg--a", "REBUTS", "arg--b");
        lab_relate(&mut engine, "arg--b", "REBUTS", "arg--c");
        lab_relate(&mut engine, "arg--d", "REBUTS", "arg--e");
        lab_relate(&mut engine, "arg--e", "REBUTS", "arg--d");
        lab_relate(&mut engine, "arg--d", "REBUTS", "arg--f");

        assert_eq!(label_of(&engine, "arg--a").0, "accepted", "unattacked");
        let (label, defeated_by, _) = label_of(&engine, "arg--b");
        assert_eq!(label, "defeated");
        assert_eq!(defeated_by, vec!["arg--a".to_string()], "evidence ships");
        assert_eq!(
            label_of(&engine, "arg--c").0,
            "accepted",
            "reinstatement: the only attacker is itself defeated"
        );
        let (label, _, undecided_by) = label_of(&engine, "arg--d");
        assert_eq!(label, "undecided", "cycle member");
        assert_eq!(undecided_by, vec!["arg--e".to_string()]);
        let (label, _, undecided_by) = label_of(&engine, "arg--f");
        assert_eq!(label, "undecided", "victim of an undecided attacker");
        assert_eq!(undecided_by, vec!["arg--d".to_string()]);

        // Determinism: a second instance over the same on-disk state.
        let engine_b = engine_with_labelling_schema(&tmp, false);
        for id in ["arg--a", "arg--b", "arg--c", "arg--d", "arg--e", "arg--f"] {
            assert_eq!(label_of(&engine, id), label_of(&engine_b, id));
        }

        // Health axis: counts per label, evidence on the lists.
        let axis = engine.health_labelling_axis(None);
        assert_eq!(axis["arg"]["counts"]["accepted"], 2);
        assert_eq!(axis["arg"]["counts"]["defeated"], 1);
        assert_eq!(axis["arg"]["counts"]["undecided"], 3);
        assert_eq!(axis["arg"]["defeated"][0]["id"], "arg--b");
        assert_eq!(axis["arg"]["defeated"][0]["defeated_by"][0], "arg--a");

        // Labels never gate writes: updating a defeated entity and
        // adding an edge into it both succeed with the normal shapes.
        let (actor, client) = cli_actor();
        let b_id = crate::entity::EntityId("arg--b".to_string());
        let current = engine.get_entity(&b_id).unwrap().content_hash.clone();
        let mut sections = IndexMap::new();
        sections.insert("body".to_string(), "updated body.".to_string());
        let outcome = engine
            .update_entity(
                crate::engine::UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: b_id,
                    expected_hash: Some(current),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    sections_unset: Vec::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: vec![],
                    dry_run: false,
                    relations_unset: Vec::new(),
                    anchors_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .expect("updating a defeated entity succeeds");
        assert!(!outcome.write_id.is_empty());
        lab_relate(&mut engine, "arg--f", "REBUTS", "arg--b");

        // In-process invalidation: a fresh unattacked attacker flips
        // A on the next read.
        lab_create(&mut engine, "arg", "H", "claim");
        lab_relate(&mut engine, "arg--h", "REBUTS", "arg--a");
        let (label, defeated_by, _) = label_of(&engine, "arg--a");
        assert_eq!(
            label, "defeated",
            "memo invalidated by the in-process mutation"
        );
        assert_eq!(defeated_by, vec!["arg--h".to_string()]);

        // Reload-path invalidation: an out-of-band file change plus an
        // explicit mem reload serves the new labelling.
        let g_path = tmp.path().join("mem-arg").join("g.md");
        std::fs::write(
            &g_path,
            "---\ntype: claim\n---\n# G\n\n## Body\n\nout of band.\n\n## Relationships\n\n- **REBUTS**: [[arg--c]]\n",
        )
        .unwrap();
        engine.reload_one_mem("arg").expect("reload succeeds");
        let (label, defeated_by, _) = label_of(&engine, "arg--c");
        assert_eq!(label, "defeated", "reload invalidated the memo");
        assert!(defeated_by.contains(&"arg--g".to_string()));
    }

    /// Support-blindness, chain shape, cross-mem exclusion, and the
    /// no-declaration complement.
    #[test]
    fn labelling_support_blindness_shape_and_cross_mem() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_labelling_schema(&tmp, true);
        // Support chain: conclusion → inference → evidence; an
        // undercutter defeats the inference.
        for (t, ty) in [
            ("Conclusion", "claim"),
            ("Inference", "claim"),
            ("Undercutter", "claim"),
        ] {
            lab_create(&mut engine, "arg", t, ty);
        }
        lab_create(&mut engine, "arg", "Ev One", "evidence");
        lab_relate(&mut engine, "arg--conclusion", "GROUNDS", "arg--inference");
        lab_relate(&mut engine, "arg--inference", "GROUNDS", "arg--ev-one");
        lab_relate(&mut engine, "arg--undercutter", "REBUTS", "arg--inference");

        // Support-blindness: the defeated inference leaves its
        // conclusion accepted; the defeat shows in the shape count.
        let conclusion = engine
            .get_entity(&crate::entity::EntityId("arg--conclusion".to_string()))
            .unwrap()
            .clone();
        let view = engine.computed_labelling(&conclusion).unwrap();
        assert_eq!(view.label.wire(), "accepted", "support-blind by design");
        let shape = view.shape.expect("support declared, shape served");
        assert_eq!(shape.depth, 2);
        assert!((shape.branching - 1.0).abs() < 1e-9);
        assert_eq!(shape.terminal_share, Some(1.0));
        assert_eq!(shape.defeated_in_support, 1, "the defeated inference");
        assert_eq!(shape.undecided_in_support, 0);

        // Isolated entity: zeros and a null share.
        lab_create(&mut engine, "arg", "Loner", "claim");
        let loner = engine
            .get_entity(&crate::entity::EntityId("arg--loner".to_string()))
            .unwrap()
            .clone();
        let view = engine.computed_labelling(&loner).unwrap();
        let shape = view.shape.unwrap();
        assert_eq!(
            (shape.depth, shape.branching, shape.terminal_share),
            (0, 0.0, None)
        );
        assert_eq!(
            (shape.defeated_in_support, shape.undecided_in_support),
            (0, 0)
        );

        // Cross-mem: an attack edge from `other` into `arg` (granted
        // by workspace policy) is excluded from the computation and
        // counted; the target stays accepted.
        let mut settings = engine.settings().clone();
        settings.cross_mem_links.insert(
            "other".to_string(),
            memstead_schema::workspace_config::CrossLinkValue::Wildcard,
        );
        engine.set_settings(settings);
        lab_create(&mut engine, "other", "Foreign", "claim");
        lab_relate(&mut engine, "other--foreign", "REBUTS", "arg--conclusion");
        let conclusion = engine
            .get_entity(&crate::entity::EntityId("arg--conclusion".to_string()))
            .unwrap()
            .clone();
        let view = engine.computed_labelling(&conclusion).unwrap();
        assert_eq!(
            view.label.wire(),
            "accepted",
            "the cross-mem attack is excluded, never guessed"
        );
        let axis = engine.health_labelling_axis(Some("arg"));
        assert_eq!(axis["arg"]["cross_mem_edges_excluded"], 1);
        assert!(axis.get("other").is_none(), "mem filter narrows");

        // Serving channels: envelope `_labelling` and the text
        // channel's `_label` + `## Labelling`; the canonical form
        // stays projection-free.
        let inference = engine
            .get_entity(&crate::entity::EntityId("arg--inference".to_string()))
            .unwrap()
            .clone();
        let view = engine.computed_labelling(&inference).unwrap();
        let md =
            crate::render::render_entity_markdown_with_signals(&inference, None, None, Some(&view));
        assert!(md.contains("_label: defeated"), "{md}");
        assert!(md.contains("## Labelling"), "{md}");
        assert!(md.contains("defeated_by: arg--undercutter"), "{md}");
        let env = crate::render::build_entity_envelope(
            &inference,
            0,
            None,
            None,
            None,
            crate::render::OriginClass::FirstParty,
            engine
                .store()
                .outgoing(&crate::entity::EntityId("arg--inference".to_string())),
            None,
            None,
            Some(&view),
        );
        assert_eq!(env["_labelling"]["label"], "defeated");
        assert_eq!(env["_labelling"]["defeated_by"][0], "arg--undercutter");
        assert!(env["_labelling"]["shape"].is_object());
        let canonical = crate::render::render_entity_markdown(&inference, None);
        assert!(
            !canonical.contains("_label") && !canonical.contains("## Labelling"),
            "canonical form is projection-free"
        );

        // No-declaration complement: a signals-fixture entity (schema
        // without labelling) serves no labelling view at all.
        let tmp2 = TempDir::new().unwrap();
        let mut plain = engine_with_signals_schema(&tmp2);
        let (actor, client) = cli_actor();
        let mut sections = IndexMap::new();
        sections.insert("body".to_string(), "a claim body.".to_string());
        plain
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "arg".to_string(),
                    title: "Plain".to_string(),
                    entity_type: "claim".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: vec![],
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let plain_entity = plain
            .get_entity(&crate::entity::EntityId("arg--plain".to_string()))
            .unwrap()
            .clone();
        assert!(plain.computed_labelling(&plain_entity).is_none());
    }

    /// An attack-only declaration (no `support` walk) serves labels
    /// but nothing shape-shaped on any channel.
    #[test]
    fn labelling_without_support_serves_no_shape() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_labelling_schema_support(&tmp, false, false);
        lab_create(&mut engine, "arg", "Solo", "claim");
        let entity = engine
            .get_entity(&crate::entity::EntityId("arg--solo".to_string()))
            .unwrap()
            .clone();
        let view = engine.computed_labelling(&entity).expect("labels served");
        assert_eq!(view.label.wire(), "accepted");
        assert!(view.shape.is_none(), "no support declaration, no shape");
        assert!(view.to_json().get("shape").is_none());
        let md =
            crate::render::render_entity_markdown_with_signals(&entity, None, None, Some(&view));
        assert!(!md.contains("- shape:"), "{md}");
    }

    fn engine_with_signals_schema(tmp: &TempDir) -> Engine {
        let schemas_dir = tmp.path().join("schemas");
        let pkg = schemas_dir.join("argsig");
        std::fs::create_dir_all(pkg.join("types")).unwrap();
        std::fs::write(
            pkg.join("schema.yaml"),
            r#"name: argsig
version: 0.1.0
description: aggregate-signal fixture
when_to_use: tests
types:
  - claim
  - objection
relationships:
  mode: strict
  definitions:
    - name: REBUTS
      description: r
      default_weight: 3.0
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#,
        )
        .unwrap();
        let body = "sections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n";
        std::fs::write(
            pkg.join("types").join("claim.yaml"),
            format!(
                "name: claim\ndescription: t\nwhen_to_use: tests\nmetadata_fields: []\nupdatable_fields:\n  - title\n  - body\n{body}signals:\n  - name: attack_load\n    kind: edge_load\n    relationships: [REBUTS]\n    direction: in\n    thresholds:\n      - at_least: 1\n        level: notice\n      - at_least: 3\n        level: warn\n  - name: open_objections\n    kind: edge_load\n    relationships: [REBUTS]\n    direction: in\n    neighbour_field: state\n    neighbour_value: open\n    thresholds:\n      - at_least: 1\n        level: notice\n"
            ),
        )
        .unwrap();
        std::fs::write(
            pkg.join("types").join("objection.yaml"),
            format!(
                "name: objection\ndescription: t\nwhen_to_use: tests\nmetadata_fields:\n  - key: state\n    description: objection lifecycle\n    field_type: string\n    enum_values: [open, closed]\nupdatable_fields:\n  - title\n  - body\n  - state\n{body}"
            ),
        )
        .unwrap();
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mount = crate::workspace::Mount {
            mem: "arg".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "argsig",
                semver::Version::new(0, 1, 0),
            )),
            storage: crate::workspace::MountStorage::Folder { path: mem_dir },
            capability: crate::workspace::MountCapability::Write,
            lifecycle: crate::workspace::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        Engine::from_mounts_with_schemas_dir(
            vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
            Some(&schemas_dir),
        )
        .unwrap()
    }

    fn objection_args(title: &str, state: Option<&str>, rebuts: Option<&str>) -> CreateEntityArgs {
        let mut sections = IndexMap::new();
        sections.insert("body".to_string(), "an objection body.".to_string());
        let mut metadata = IndexMap::new();
        if let Some(s) = state {
            metadata.insert("state".to_string(), s.to_string());
        }
        let relations = rebuts
            .map(|to| {
                vec![crate::ops::RelateArg {
                    target: crate::entity::EntityId(to.to_string()),
                    rel_type: "REBUTS".to_string(),
                    description: None,
                }]
            })
            .unwrap_or_default();
        CreateEntityArgs {
            anchors: Vec::new(),
            mem: "arg".to_string(),
            title: title.to_string(),
            entity_type: "objection".to_string(),
            sections,
            metadata,
            relations,
            dry_run: false,
        }
    }

    fn crossing_warnings_of(
        warnings: &[WarningHint],
    ) -> Vec<(String, String, u64, String, String)> {
        warnings
            .iter()
            .filter_map(|w| match w {
                WarningHint::SignalThresholdCrossed {
                    entity_id,
                    signal,
                    value,
                    old_level,
                    new_level,
                } => Some((
                    entity_id.to_string(),
                    signal.clone(),
                    *value,
                    old_level.clone(),
                    new_level.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    /// Signals on reads: thresholds map boundary counts to levels, the
    /// neighbour filter counts only qualifying counterparts, the two
    /// serving channels carry headline + contributors, a flip of the
    /// counterpart's field changes the count on the next read, and two
    /// engine instances over the same on-disk state serve identical
    /// payloads.
    #[test]
    fn signals_reads_thresholds_neighbour_filter_and_determinism() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_signals_schema(&tmp);
        let (actor, client) = cli_actor();

        let mut claim_sections = IndexMap::new();
        claim_sections.insert("body".to_string(), "a claim body.".to_string());
        engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "arg".to_string(),
                    title: "Claim".to_string(),
                    entity_type: "claim".to_string(),
                    sections: claim_sections,
                    metadata: IndexMap::new(),
                    relations: vec![],
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let claim_id = crate::entity::EntityId("arg--claim".to_string());
        let sig_of = |engine: &Engine, name: &str| {
            let entity = engine.get_entity(&claim_id).unwrap().clone();
            engine
                .computed_signals(&entity)
                .unwrap()
                .into_iter()
                .find(|s| s.name == name)
                .unwrap()
        };

        // Below the first threshold: value 0, level none.
        let s = sig_of(&engine, "attack_load");
        assert_eq!((s.value, s.level_wire()), (0, "none"));

        // First objection (open, inline REBUTS): the create's crossing
        // warnings name BOTH signals moving none → notice on the claim.
        let outcome = engine
            .create_entity(
                objection_args("Obj One", Some("open"), Some("arg--claim")),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let crossings = crossing_warnings_of(&outcome.warnings);
        assert!(
            crossings.contains(&(
                "arg--claim".to_string(),
                "attack_load".to_string(),
                1,
                "none".to_string(),
                "notice".to_string()
            )),
            "create with inline relation crosses attack_load: {crossings:?}"
        );
        assert!(
            crossings.iter().any(|c| c.1 == "open_objections"),
            "open counterpart crosses open_objections too: {crossings:?}"
        );

        // Closed and field-less counterparts count for attack_load,
        // never for open_objections.
        engine
            .create_entity(
                objection_args("Obj Two", Some("closed"), Some("arg--claim")),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        engine
            .create_entity(
                objection_args("Obj Three", None, Some("arg--claim")),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let attack = sig_of(&engine, "attack_load");
        assert_eq!((attack.value, attack.level_wire()), (3, "warn"));
        assert_eq!(attack.contributors.len(), 3);
        let open = sig_of(&engine, "open_objections");
        assert_eq!((open.value, open.level_wire()), (1, "notice"));
        assert_eq!(open.contributors[0].0, "arg--obj-one");

        // Both serving channels: frontmatter headline + `## Signals`
        // contributors on the text channel; `_signals` on the envelope.
        let entity = engine.get_entity(&claim_id).unwrap().clone();
        let signals = engine.computed_signals(&entity).unwrap();
        let md =
            crate::render::render_entity_markdown_with_signals(&entity, None, Some(&signals), None);
        assert!(
            md.contains("_signals: [attack_load: 3 (warn), open_objections: 1 (notice)]"),
            "frontmatter headline: {md}"
        );
        assert!(md.contains("## Signals"), "contributors section: {md}");
        assert!(md.contains("arg--obj-one"), "evidence ships: {md}");
        let env = crate::render::build_entity_envelope(
            &entity,
            0,
            None,
            None,
            None,
            crate::render::OriginClass::FirstParty,
            engine.store().outgoing(&claim_id),
            None,
            Some(&signals),
            None,
        );
        assert_eq!(env["_signals"][0]["name"], "attack_load");
        assert_eq!(env["_signals"][0]["value"], 3);
        assert_eq!(env["_signals"][0]["level"], "warn");
        assert_eq!(
            env["_signals"][0]["contributors"].as_array().unwrap().len(),
            3
        );
        // The canonical form stays signal-free.
        let canonical = crate::render::render_entity_markdown(&entity, None);
        assert!(
            !canonical.contains("_signals"),
            "canonical form is signal-free"
        );

        // Flipping the counterpart's field changes the count on the
        // next read, and the update carries the crossing for the CLAIM.
        let obj_one = crate::entity::EntityId("arg--obj-one".to_string());
        let current = engine.get_entity(&obj_one).unwrap().content_hash.clone();
        let mut metadata = IndexMap::new();
        metadata.insert("state".to_string(), "closed".to_string());
        let outcome = engine
            .update_entity(
                crate::engine::UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: obj_one,
                    expected_hash: Some(current),
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    sections_unset: Vec::new(),
                    metadata,
                    metadata_unset: Vec::new(),
                    declare_relations: vec![],
                    dry_run: false,
                    relations_unset: Vec::new(),
                    anchors_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(!outcome.write_id.is_empty(), "success shape kept");
        let crossings = crossing_warnings_of(&outcome.warnings);
        assert!(
            crossings.contains(&(
                "arg--claim".to_string(),
                "open_objections".to_string(),
                0,
                "notice".to_string(),
                "none".to_string()
            )),
            "the neighbour flip crosses the claim's filtered signal: {crossings:?}"
        );
        let open = sig_of(&engine, "open_objections");
        assert_eq!((open.value, open.level_wire()), (0, "none"));

        // Determinism: a second instance over the same on-disk state
        // serves an identical payload.
        let engine_b = engine_with_signals_schema(&tmp);
        let entity_b = engine_b.get_entity(&claim_id).unwrap().clone();
        let signals_b = engine_b.computed_signals(&entity_b).unwrap();
        let entity_a = engine.get_entity(&claim_id).unwrap().clone();
        let signals_a = engine.computed_signals(&entity_a).unwrap();
        assert_eq!(
            crate::ops::signals::signals_json(&signals_a),
            crate::ops::signals::signals_json(&signals_b),
            "two instances over the same mem state serve identical signals"
        );
    }

    /// Crossings on the relate surface: upward and downward crossings
    /// warn with the full detail set; a write that crosses nothing
    /// carries nothing signal-shaped; the mutation stays the success
    /// shape throughout. The `signals` health axis serves only
    /// above-`none` entities, with per-level counts and a mem filter.
    #[test]
    fn signal_crossings_on_relate_and_health_axis() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_signals_schema(&tmp);
        let (actor, client) = cli_actor();

        let mut claim_sections = IndexMap::new();
        claim_sections.insert("body".to_string(), "a claim body.".to_string());
        engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "arg".to_string(),
                    title: "Claim".to_string(),
                    entity_type: "claim".to_string(),
                    sections: claim_sections,
                    metadata: IndexMap::new(),
                    relations: vec![],
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        for (t, s) in [("Obj A", "open"), ("Obj B", "closed"), ("Obj C", "closed")] {
            engine
                .create_entity(objection_args(t, Some(s), None), actor, Some(&client), None)
                .unwrap();
        }
        let relate = |engine: &mut Engine, from: &str, remove: bool| {
            engine
                .relate_entity(
                    crate::engine::RelateEntityArgs {
                        source: crate::entity::EntityId(from.to_string()),
                        target: crate::entity::EntityId("arg--claim".to_string()),
                        rel_type: "REBUTS".to_string(),
                        description: None,
                        remove,
                        expected_hash: None,
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap()
        };

        // 0 → 1: none → notice (upward), on the TARGET of the edge.
        let outcome = relate(&mut engine, "arg--obj-a", false);
        assert!(!outcome.write_id.is_empty());
        let crossings = crossing_warnings_of(&outcome.warnings);
        assert!(
            crossings.contains(&(
                "arg--claim".to_string(),
                "attack_load".to_string(),
                1,
                "none".to_string(),
                "notice".to_string()
            )),
            "{crossings:?}"
        );

        // 1 → 2: notice → notice — nothing signal-shaped rides.
        let outcome = relate(&mut engine, "arg--obj-b", false);
        assert!(
            crossing_warnings_of(&outcome.warnings).is_empty(),
            "no threshold crossed, no warning: {:?}",
            outcome.warnings
        );

        // 2 → 3: notice → warn.
        let outcome = relate(&mut engine, "arg--obj-c", false);
        let crossings = crossing_warnings_of(&outcome.warnings);
        assert!(
            crossings.contains(&(
                "arg--claim".to_string(),
                "attack_load".to_string(),
                3,
                "notice".to_string(),
                "warn".to_string()
            )),
            "{crossings:?}"
        );

        // Health axis at warn: the claim is the one above-`none`
        // entity; counts split per level; a mem filter narrows.
        let axis = engine.health_signals_axis(None);
        assert_eq!(axis["entities"].as_array().unwrap().len(), 1);
        assert_eq!(axis["entities"][0]["id"], "arg--claim");
        assert_eq!(axis["counts"]["warn"], 1, "attack_load at warn: {axis}");
        assert_eq!(axis["counts"]["notice"], 1, "open_objections at notice");
        let filtered = engine.health_signals_axis(Some("other"));
        assert!(filtered["entities"].as_array().unwrap().is_empty());

        // 3 → 2: warn → notice (downward crossing warns too).
        let outcome = relate(&mut engine, "arg--obj-c", true);
        let crossings = crossing_warnings_of(&outcome.warnings);
        assert!(
            crossings.contains(&(
                "arg--claim".to_string(),
                "attack_load".to_string(),
                2,
                "warn".to_string(),
                "notice".to_string()
            )),
            "{crossings:?}"
        );
    }

    /// Regression pin for `no_self_loop_relationships`' single
    /// functional behavior: a self-loop (`from == to`) on a rel-type
    /// the source type lists there refuses with `RELATIONSHIP_CYCLE`.
    /// The constraint vocabulary settles the field's semantics — the
    /// new propagation declaration gets a distinct name, and this pin
    /// guards that the old field keeps exactly this effect.
    #[test]
    fn no_self_loop_rel_type_self_loop_refusal_is_pinned() {
        let tmp = TempDir::new().unwrap();
        // The `constr` fixture declares `no_self_loop_relationships:
        // [PART_OF]` on `task`.
        let mut engine = engine_with_constraints_schema(&tmp, "warn", "warn");
        let (actor, client) = cli_actor();
        let a = engine
            .create_entity(
                task_create_args("Task A", vec![]),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let err = engine
            .relate_entity(
                crate::engine::RelateEntityArgs {
                    source: a.id.clone(),
                    target: a.id.clone(),
                    rel_type: "PART_OF".to_string(),
                    description: None,
                    remove: false,
                    expected_hash: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "RELATIONSHIP_CYCLE");
    }

    /// Generic constraint-proof fixture: one folder-mounted mem
    /// (`proof`) pinned to a schema built from the given manifest and
    /// type YAMLs.
    fn engine_with_proof_schema(
        tmp: &TempDir,
        schema_name: &str,
        manifest_yaml: &str,
        types: &[(&str, &str)],
    ) -> Engine {
        let schemas_dir = tmp.path().join("schemas");
        let pkg = schemas_dir.join(schema_name);
        std::fs::create_dir_all(pkg.join("types")).unwrap();
        std::fs::write(pkg.join("schema.yaml"), manifest_yaml).unwrap();
        for (name, yaml) in types {
            std::fs::write(pkg.join("types").join(format!("{name}.yaml")), yaml).unwrap();
        }
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mount = crate::workspace::Mount {
            mem: "proof".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                schema_name,
                semver::Version::new(0, 1, 0),
            )),
            storage: crate::workspace::MountStorage::Folder { path: mem_dir },
            capability: crate::workspace::MountCapability::Write,
            lifecycle: crate::workspace::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        Engine::from_mounts_with_schemas_dir(
            vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
            Some(&schemas_dir),
        )
        .unwrap()
    }

    fn proof_create(
        engine: &mut Engine,
        entity_type: &str,
        title: &str,
        metadata: &[(&str, &str)],
        relations: Vec<crate::ops::RelateArg>,
    ) -> Result<CreateEntityOutcome, EngineError> {
        let (actor, client) = cli_actor();
        let mut sections = IndexMap::new();
        sections.insert("body".to_string(), format!("{title} body."));
        let mut md = IndexMap::new();
        for (k, v) in metadata {
            md.insert(k.to_string(), v.to_string());
        }
        engine.create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "proof".to_string(),
                title: title.to_string(),
                entity_type: entity_type.to_string(),
                sections,
                metadata: md,
                relations,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
    }

    fn rel(to: &str, rel_type: &str) -> crate::ops::RelateArg {
        crate::ops::RelateArg {
            target: crate::entity::EntityId(to.to_string()),
            rel_type: rel_type.to_string(),
            description: None,
        }
    }

    const GROUNDING_MANIFEST: &str = r#"name: grounding
version: 0.1.0
description: anker-shaped grounding proof schema
when_to_use: constraint-proof tests
types:
  - anchor
  - tradeoff
relationships:
  mode: strict
  definitions:
    - name: FOLLOWS_FROM
      description: stands on
      default_weight: 3.0
    - name: SUPPORTS
      description: pro
      default_weight: 1.0
    - name: OPPOSES
      description: contra
      default_weight: 1.0
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;

    const GROUNDING_ANCHOR: &str = r#"name: anchor
description: a judgment standing on others
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: status
    description: lifecycle
    field_type: string
    enum_values: [open, checked, fallen]
  - key: checked_by
    description: who checked
    field_type: string
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
  - status
  - checked_by
health_required_fields:
  - body
staleness_threshold_days: 90
constraints:
  - kind: requires_when
    field: checked_by
    when_field: status
    when_value: checked
  - kind: status_propagation
    field: status
    value: fallen
    rel_type: FOLLOWS_FROM
    direction: incoming
write_rules: []
"#;

    const GROUNDING_TRADEOFF: &str = r#"name: tradeoff
description: a claim with two sides
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
required_outgoing:
  - relationships: [SUPPORTS]
    cardinality: at_least_one
  - relationships: [OPPOSES]
    cardinality: at_least_one
write_rules: []
"#;

    /// The anker proof (plan 07, criterion 2): the grounding-shaped
    /// schema answers `pruefe_kette.py`'s check questions 1–3 from
    /// health output alone — no project Python.
    #[test]
    fn anker_proof_grounding_schema_answers_check_questions_from_health() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_proof_schema(
            &tmp,
            "grounding",
            GROUNDING_MANIFEST,
            &[
                ("anchor", GROUNDING_ANCHOR),
                ("tradeoff", GROUNDING_TRADEOFF),
            ],
        );

        // A fallen root, a child standing on it, a grandchild standing
        // on the child (transitive), plus an untainted sibling chain.
        let root = proof_create(
            &mut engine,
            "anchor",
            "Root",
            &[("status", "fallen")],
            vec![],
        )
        .unwrap();
        let child = proof_create(
            &mut engine,
            "anchor",
            "Child",
            &[],
            vec![rel(&root.id.0, "FOLLOWS_FROM")],
        )
        .unwrap();
        let grandchild = proof_create(
            &mut engine,
            "anchor",
            "Grandchild",
            &[],
            vec![rel(&child.id.0, "FOLLOWS_FROM")],
        )
        .unwrap();
        let standing = proof_create(
            &mut engine,
            "anchor",
            "Standing Root",
            &[("status", "open")],
            vec![],
        )
        .unwrap();
        let standing_child = proof_create(
            &mut engine,
            "anchor",
            "Standing Child",
            &[],
            vec![rel(&standing.id.0, "FOLLOWS_FROM")],
        )
        .unwrap();
        // Question 3's subject: checked without a checker.
        let unchecked = proof_create(
            &mut engine,
            "anchor",
            "Checked No Checker",
            &[("status", "checked")],
            vec![],
        )
        .unwrap();
        // Question 2's subject: a one-sided trade-off.
        let onesided = proof_create(
            &mut engine,
            "tradeoff",
            "One Sided",
            &[],
            vec![rel(&standing.id.0, "SUPPORTS")],
        )
        .unwrap();

        // Question 1 — descendants of the fallen anchor are flagged,
        // naming their ancestor; the standing chain is not.
        let findings = crate::ops::health::collect_constraint_findings(
            engine.store(),
            None,
            engine.schemas(),
            None,
        );
        let tainted_of = |id: &crate::entity::EntityId| -> Vec<String> {
            findings
                .iter()
                .filter(|r| &r.id == id)
                .flat_map(|r| &r.violations)
                .filter_map(|v| match v {
                    crate::ops::health::UnsatisfiedConstraint::StatusPropagation {
                        tainted_by,
                        ..
                    } => Some(tainted_by.clone()),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(tainted_of(&child.id), vec![root.id.to_string()]);
        assert_eq!(
            tainted_of(&grandchild.id),
            vec![root.id.to_string()],
            "the taint is transitive and names the terminal ancestor"
        );
        assert!(tainted_of(&standing_child.id).is_empty());
        assert!(
            tainted_of(&root.id).is_empty(),
            "the source is not its own finding"
        );

        // Question 3 — checked-without-checker is flagged.
        assert!(
            findings.iter().any(|r| r.id == unchecked.id
                && r.violations.iter().any(|v| matches!(
                    v,
                    crate::ops::health::UnsatisfiedConstraint::RequiresWhen { field, .. }
                        if field == "checked_by"
                ))),
            "checked-without-checker must be a health finding"
        );

        // Question 2 — the one-sided trade-off is flagged missing its
        // OPPOSES block (form 4 at warn), from health output alone.
        let missing = crate::ops::health::collect_missing_required_outgoing(
            engine.store(),
            None,
            engine.schemas(),
        );
        let onesided_report = missing
            .iter()
            .find(|r| r.id == onesided.id)
            .expect("one-sided trade-off flagged");
        assert_eq!(onesided_report.missing.len(), 1);
        assert_eq!(onesided_report.missing[0].relationships, vec!["OPPOSES"]);
    }

    const PLENUM_MANIFEST: &str = r#"name: plenum-proof
version: 0.1.0
description: plenum-shaped uniqueness and vocabulary proof schema
when_to_use: constraint-proof tests
types:
  - rede
  - vocabulary
relationships:
  mode: strict
  definitions:
    - name: REFERENCES
      description: soft ref
      default_weight: 0.5
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;

    fn plenum_rede_type(unique_severity: &str) -> String {
        format!(
            r#"name: rede
description: one speech
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: rede_id
    description: source id
    field_type: string
  - key: rede_sha256
    description: content hash
    field_type: string
  - key: kategorie
    description: category from the shared vocabulary
    field_type: string
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
  - rede_id
  - rede_sha256
  - kategorie
health_required_fields:
  - body
staleness_threshold_days: 90
constraints:
  - kind: unique
    fields: [rede_id, rede_sha256]
    severity: {unique_severity}
  - kind: enum_from_neighbour
    field: kategorie
    rel_type: REFERENCES
    section: terms
write_rules: []
"#
        )
    }

    const PLENUM_VOCABULARY: &str = r#"name: vocabulary
description: the shared term list
when_to_use: tests
sections:
  - key: terms
    heading: Terms
    required: false
    search_weight: 5.0
    catch_all: false
    write_rules: []
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
  - terms
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;

    /// The plenum proof, uniqueness half (plan 07, criterion 3): a
    /// second create with the same declared key tuple refuses with a
    /// typed code naming the colliding entity — the 37-duplicates
    /// scenario bounces at the engine. Health reports a pre-existing
    /// violation planted under a warn-tier variant.
    #[test]
    fn plenum_proof_uniqueness_refuses_duplicates_and_health_reports_planted_ones() {
        // Block tier: the duplicate refuses, naming the collider.
        let tmp = TempDir::new().unwrap();
        let rede = plenum_rede_type("block");
        let mut engine = engine_with_proof_schema(
            &tmp,
            "plenum-proof",
            PLENUM_MANIFEST,
            &[("rede", &rede), ("vocabulary", PLENUM_VOCABULARY)],
        );
        let first = proof_create(
            &mut engine,
            "rede",
            "Speech One",
            &[("rede_id", "19-42"), ("rede_sha256", "abc123")],
            vec![],
        )
        .unwrap();
        let err = proof_create(
            &mut engine,
            "rede",
            "Speech One Duplicate",
            &[("rede_id", "19-42"), ("rede_sha256", "abc123")],
            vec![],
        )
        .unwrap_err();
        assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED");
        assert_eq!(
            err.details()["violations"][0]["colliding"],
            first.id.to_string(),
            "the refusal names the colliding entity"
        );
        // A different tuple passes.
        proof_create(
            &mut engine,
            "rede",
            "Speech Two",
            &[("rede_id", "19-43"), ("rede_sha256", "def456")],
            vec![],
        )
        .unwrap();

        // Warn tier: plant the duplicate, health reports it.
        let tmp = TempDir::new().unwrap();
        let rede = plenum_rede_type("warn");
        let mut engine = engine_with_proof_schema(
            &tmp,
            "plenum-proof",
            PLENUM_MANIFEST,
            &[("rede", &rede), ("vocabulary", PLENUM_VOCABULARY)],
        );
        proof_create(
            &mut engine,
            "rede",
            "Planted A",
            &[("rede_id", "19-42"), ("rede_sha256", "abc123")],
            vec![],
        )
        .unwrap();
        let planted = proof_create(
            &mut engine,
            "rede",
            "Planted B",
            &[("rede_id", "19-42"), ("rede_sha256", "abc123")],
            vec![],
        )
        .unwrap();
        assert!(
            planted
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::ConstraintUnsatisfied { .. })),
            "warn tier surfaces the duplicate as a warning and commits"
        );
        let findings = crate::ops::health::collect_constraint_findings(
            engine.store(),
            None,
            engine.schemas(),
            None,
        );
        assert_eq!(
            findings.len(),
            2,
            "both sides of the planted duplicate are findings: {findings:?}"
        );
    }

    /// The plenum proof, enum-from-neighbour half (plan 07,
    /// criterion 3): renaming a value in the neighbour's section makes
    /// every stale holder a health finding.
    #[test]
    fn plenum_proof_enum_from_neighbour_flags_stale_holders_after_rename() {
        let tmp = TempDir::new().unwrap();
        let rede = plenum_rede_type("warn");
        let mut engine = engine_with_proof_schema(
            &tmp,
            "plenum-proof",
            PLENUM_MANIFEST,
            &[("rede", &rede), ("vocabulary", PLENUM_VOCABULARY)],
        );
        let (actor, client) = cli_actor();

        // The vocabulary entity enumerates the legal categories.
        let mut sections = IndexMap::new();
        sections.insert("body".to_string(), "the term list.".to_string());
        sections.insert("terms".to_string(), "- haushalt\n- verkehr\n".to_string());
        let vocab = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "proof".to_string(),
                    title: "Kategorien".to_string(),
                    entity_type: "vocabulary".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: vec![],
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // A holder whose value is backed: clean.
        let holder = proof_create(
            &mut engine,
            "rede",
            "Holder",
            &[("kategorie", "haushalt")],
            vec![rel(&vocab.id.0, "REFERENCES")],
        )
        .unwrap();
        let findings = crate::ops::health::collect_constraint_findings(
            engine.store(),
            None,
            engine.schemas(),
            None,
        );
        assert!(
            findings.iter().all(|r| r.id != holder.id),
            "backed value produces no finding: {findings:?}"
        );

        // Rename the value in the neighbour's section — the holder
        // goes stale and health flags it.
        let current = engine.get_entity(&vocab.id).unwrap().content_hash.clone();
        let mut sections = IndexMap::new();
        sections.insert("terms".to_string(), "- finanzen\n- verkehr\n".to_string());
        engine
            .update_entity(
                crate::engine::UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: vocab.id.clone(),
                    expected_hash: Some(current),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    sections_unset: Vec::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: vec![],
                    dry_run: false,
                    relations_unset: Vec::new(),
                    anchors_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let findings = crate::ops::health::collect_constraint_findings(
            engine.store(),
            None,
            engine.schemas(),
            None,
        );
        let stale = findings
            .iter()
            .find(|r| r.id == holder.id)
            .expect("stale holder is flagged after the rename");
        assert!(stale.violations.iter().any(|v| matches!(
            v,
            crate::ops::health::UnsatisfiedConstraint::EnumFromNeighbour { value, .. }
                if value == "haushalt"
        )));
    }

    /// The advertised mutation warning is real: a create leaving a
    /// required-outgoing block unsatisfied returns
    /// `MISSING_REQUIRED_OUTGOING` naming the block with cardinality —
    /// and still commits. Complements: a create satisfying the block
    /// via inline `relations` emits no such warning; the health sweep
    /// reports exactly the same unsatisfied blocks (shared evaluation).
    #[test]
    fn create_warns_missing_required_outgoing_and_still_commits() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_required_outgoing_schema(&tmp);
        let (actor, client) = cli_actor();

        let outcome = engine
            .create_entity(
                task_create_args("Orphan Task", vec![]),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(
            !outcome.write_id.is_empty(),
            "the warning never blocks the mutation"
        );
        let blocks = missing_outgoing_of(&outcome.warnings);
        assert_eq!(
            blocks,
            vec![(vec!["PART_OF".to_string()], "at_least_one".to_string())],
            "warning names the unsatisfied block with cardinality; warnings = {:?}",
            outcome.warnings
        );

        // Health-path parity: the sweep reports the same entity with
        // the same block — the two surfaces share one evaluation.
        let reports = crate::ops::health::collect_missing_required_outgoing(
            engine.store(),
            None,
            engine.schemas(),
        );
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].id, outcome.id);
        assert_eq!(reports[0].missing.len(), 1);
        assert_eq!(reports[0].missing[0].relationships, vec!["PART_OF"]);
        assert_eq!(reports[0].missing[0].cardinality, "at_least_one");

        // Complement: a create whose inline relation satisfies the
        // block emits no MISSING_REQUIRED_OUTGOING.
        let satisfied = engine
            .create_entity(
                task_create_args(
                    "Child Task",
                    vec![crate::ops::RelateArg {
                        target: outcome.id.clone(),
                        rel_type: "PART_OF".to_string(),
                        description: None,
                    }],
                ),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(
            missing_outgoing_of(&satisfied.warnings).is_empty(),
            "satisfied block emits no warning: {:?}",
            satisfied.warnings
        );
    }

    /// Update-side mirror: a section-only update on an entity with an
    /// unsatisfied block warns; declaring the satisfying relation in
    /// the same update clears it.
    #[test]
    fn update_warns_missing_required_outgoing_until_satisfied() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_required_outgoing_schema(&tmp);
        let (actor, client) = cli_actor();
        let a = engine
            .create_entity(
                task_create_args("Task A", vec![]),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let b = engine
            .create_entity(
                task_create_args("Task B", vec![]),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let update = |engine: &mut Engine,
                      id: &crate::entity::EntityId,
                      declare: Vec<crate::ops::RelateArg>| {
            let current = engine.get_entity(id).unwrap().content_hash.clone();
            let mut sections = IndexMap::new();
            sections.insert("body".to_string(), format!("edited at {:?}", declare.len()));
            engine
                .update_entity(
                    crate::engine::UpdateEntityArgs {
                        anchors: Vec::new(),
                        id: id.clone(),
                        expected_hash: Some(current),
                        sections,
                        append_sections: IndexMap::new(),
                        patch_sections: IndexMap::new(),
                        sections_unset: Vec::new(),
                        metadata: IndexMap::new(),
                        metadata_unset: Vec::new(),
                        declare_relations: declare,
                        dry_run: false,
                        relations_unset: Vec::new(),
                        anchors_unset: Vec::new(),
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap()
        };

        // Section-only update on an unsatisfied entity: warning fires,
        // mutation commits.
        let outcome = update(&mut engine, &a.id, vec![]);
        assert!(!outcome.write_id.is_empty());
        assert_eq!(
            missing_outgoing_of(&outcome.warnings),
            vec![(vec!["PART_OF".to_string()], "at_least_one".to_string())]
        );

        // Declaring the satisfying relation in the update clears it.
        let outcome = update(
            &mut engine,
            &a.id,
            vec![crate::ops::RelateArg {
                target: b.id.clone(),
                rel_type: "PART_OF".to_string(),
                description: None,
            }],
        );
        assert!(
            missing_outgoing_of(&outcome.warnings).is_empty(),
            "satisfied block emits no warning: {:?}",
            outcome.warnings
        );
    }

    /// The source-vs-binding check at the engine seam: an anchor naming
    /// BOTH a producing binding (by hash) and a `source` refuses when
    /// the binding resolves in this workspace but does not declare the
    /// name — with the declared names in the recovery payload. A
    /// declared name is accepted; an unresolvable binding hash accepts
    /// any non-empty name (validation never requires resolution).
    #[test]
    fn anchor_source_validated_against_resolvable_binding() {
        use crate::binding::{
            BINDING_VERSION, Binding, BuildMode, BuildOperation, Operations, hash_binding,
        };
        use crate::pipeline::{IngestTrigger, PatternEntry, PatternMode, Source};

        let tmp = TempDir::new().unwrap();
        let (mut engine, _seed) = engine_with_seed(&tmp, "Seed");
        let (actor, client) = cli_actor();

        // A workspace root carrying one binding with two declared sources.
        let ws = TempDir::new().unwrap();
        let binding = Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: ["api-docs", "guides"]
                .into_iter()
                .map(|n| Source {
                    name: n.to_string(),
                    medium_type: crate::pipeline::MediumType::Codebase,
                    pointer: "../src".to_string(),
                    change_detection: None,
                    scope: vec![PatternEntry {
                        path: "**/*".to_string(),
                        mode: PatternMode::Allow,
                    }],
                    engagement: None,
                    preparation: None,
                })
                .collect(),
            reference_mems: Vec::new(),
            destination_mem: "specs".to_string(),
            deny_paths: Vec::new(),
            coverage_semantics: None,
            rules: None,
            prune: None,
            operations: Operations {
                build: Some(BuildOperation {
                    mode: BuildMode::Discovery,
                    trigger: IngestTrigger::Loop,
                    batch_size: 20,
                    post_actions: None,
                }),
                sync: None,
                verify: None,
            },
        };
        let dir = ws
            .path()
            .join(".memstead")
            .join("projections")
            .join("specs");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("docs.json"),
            serde_json::to_string_pretty(&binding).unwrap(),
        )
        .unwrap();
        engine.set_workspace_root(ws.path().to_path_buf());
        let binding_hash = hash_binding(&binding);

        // The artifact must resolve (workspace-relative fallback) — the
        // write gate refuses dead references; this test's subject is the
        // source-NAME validation, not the path join.
        std::fs::create_dir_all(ws.path().join("src")).unwrap();
        std::fs::write(ws.path().join("src").join("x.rs"), "fn x() {}").unwrap();

        let anchor = |source: &str, binding: &str| crate::anchor::AnchorInput {
            artifact: Some("src/x.rs".into()),
            grain: Some("file".into()),
            class: Some("anchored".into()),
            binding: Some(binding.into()),
            source: Some(source.into()),
            ..Default::default()
        };
        let make_args = |title: &str, a: crate::anchor::AnchorInput| {
            let mut args = empty_create_args("specs", title);
            args.anchors = vec![a];
            args
        };

        // Undeclared name against the RESOLVING binding: refuses with the
        // declared names in the payload.
        let err = engine
            .create_entity(
                make_args("Bad Source", anchor("front-page", &binding_hash)),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_ANCHOR", "got {err:?}");
        let details = err.details();
        assert_eq!(details["field"], "source");
        assert_eq!(details["got"], "front-page");
        assert_eq!(
            details["declared"],
            serde_json::json!(["api-docs", "guides"])
        );

        // A declared name is accepted, and the anchor round-trips with it.
        let ok = engine
            .create_entity(
                make_args("Good Source", anchor("api-docs", &binding_hash)),
                actor,
                Some(&client),
                None,
            )
            .expect("declared source name accepted");
        let anchors = engine.mem_anchors_resolved("specs");
        let stored = anchors
            .iter()
            .find(|(id, _)| id == &ok.id)
            .map(|(_, a)| &a.anchor)
            .expect("anchor stored for the new entity");
        assert_eq!(stored.source.as_deref(), Some("api-docs"));

        // An unresolvable binding hash accepts any non-empty name.
        engine
            .create_entity(
                make_args("Orphaned Binding", anchor("whatever", "deadbeef")),
                actor,
                Some(&client),
                None,
            )
            .expect("unresolvable binding accepts any non-empty name");
    }

    /// Batch create: N mutually-referencing entities (cycle included —
    /// USES is not acyclic in the default schema) land in ONE
    /// invocation with every reference resolving to a REAL typed
    /// entity, never a stub, and no stub warnings.
    #[test]
    fn batch_create_intra_batch_references_resolve_real() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let with_rel = |title: &str, to: &str| {
            let mut args = empty_create_args("specs", title);
            args.relations = vec![crate::ops::RelateArg {
                target: crate::entity::EntityId::new("specs", to),
                rel_type: "USES".to_string(),
                description: None,
            }];
            (args, Some(format!("note for {title}")))
        };
        // A → B → C → A: a cycle the schema permits.
        let result = engine
            .batch_create(
                vec![
                    with_rel("Alpha", "beta"),
                    with_rel("Beta", "gamma"),
                    with_rel("Gamma", "alpha"),
                ],
                actor,
                Some(&client),
                false,
            )
            .unwrap();
        assert!(result.applied, "{result:?}");
        assert_eq!(result.succeeded, 3);
        assert!(!result.write_id.is_empty(), "one real commit");
        assert!(
            result.results.iter().all(|r| r.action == "created"),
            "{result:?}"
        );

        // Every reference resolves to a REAL entity of the right type.
        for name in ["alpha", "beta", "gamma"] {
            let e = engine
                .get_entity(&crate::entity::EntityId::new("specs", name))
                .unwrap();
            assert!(!e.stub, "{name} must be real, not a stub");
            assert_eq!(e.entity_type, "spec");
            assert_eq!(e.relationships.len(), 1, "{name} carries its edge");
        }
        // No stub warnings anywhere in the outcome (in-batch targets
        // never transit through the stub machinery).
        // (BatchResult carries no warnings channel; absence of stubs in
        // the store is the observable.)
    }

    /// Rehearsal contract (agent-trust plan 07): `batch_create` with
    /// `dry_run: true` validates the whole batch — intra-batch
    /// references included — and reports the would-be receipt with the
    /// marker form's empty `write_id`, writing NOTHING. The
    /// follow-up real call on the unchanged mem succeeds.
    #[test]
    fn batch_create_dry_run_reports_receipt_and_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let with_rel = |title: &str, to: &str| {
            let mut args = empty_create_args("specs", title);
            args.relations = vec![crate::ops::RelateArg {
                target: crate::entity::EntityId::new("specs", to),
                rel_type: "USES".to_string(),
                description: None,
            }];
            (args, None)
        };
        let batch = || {
            vec![
                with_rel("Alpha", "beta"),
                with_rel("Beta", "gamma"),
                with_rel("Gamma", "alpha"),
            ]
        };

        let rehearsed = engine
            .batch_create(batch(), actor, Some(&client), true)
            .unwrap();
        assert!(rehearsed.applied, "{rehearsed:?}");
        assert_eq!(rehearsed.succeeded, 3);
        assert!(rehearsed.write_id.is_empty(), "marker form: empty write_id");
        assert!(rehearsed.results.iter().all(|r| r.action == "created"));
        // The receipt names the prospective ids; nothing landed.
        for name in ["alpha", "beta", "gamma"] {
            let id = crate::entity::EntityId::new("specs", name);
            assert!(
                rehearsed.results.iter().any(|r| r.id == id),
                "receipt must name {id}: {rehearsed:?}"
            );
            assert!(
                !engine.store().contains(&id),
                "rehearsal must create nothing"
            );
        }
        assert_eq!(engine.store().all_entities().count(), 0);

        // Identical validation: the real call on the unchanged mem lands.
        let real = engine
            .batch_create(batch(), actor, Some(&client), false)
            .unwrap();
        assert!(real.applied, "{real:?}");
        assert!(!real.write_id.is_empty(), "the real batch commits");
        assert_eq!(real.succeeded, 3);
    }

    /// Rehearsal refusal parity: a batch with failing entries refuses
    /// under `dry_run: true` with the SAME per-entry report-all
    /// envelope the real call returns — and both perform nothing, so
    /// the paired invocations are directly comparable.
    #[test]
    fn batch_create_dry_run_refuses_identically_to_real() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, _seeded) = engine_with_seed(&tmp, "Existing");
        let (actor, client) = cli_actor();
        let plain = |title: &str| (empty_create_args("specs", title), None);
        let batch = || {
            vec![
                plain("Fine One"),
                plain("Existing"),  // duplicate vs pre-batch store
                plain("Bad/Title"), // invalid title character
            ]
        };

        let rehearsed = engine
            .batch_create(batch(), actor, Some(&client), true)
            .unwrap();
        let real = engine
            .batch_create(batch(), actor, Some(&client), false)
            .unwrap();
        assert!(!rehearsed.applied && !real.applied);
        assert_eq!(rehearsed.failed, real.failed);
        assert_eq!(rehearsed.errors_suppressed, real.errors_suppressed);
        let envelope = |r: &crate::ops::BatchResult| {
            r.results
                .iter()
                .map(|e| {
                    (
                        e.id.to_string(),
                        e.action.clone(),
                        e.error.as_ref().map(|err| {
                            (err.code.clone(), err.message.clone(), err.details.clone())
                        }),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(envelope(&rehearsed), envelope(&real), "identical refusals");
        assert!(
            !engine
                .store()
                .contains(&crate::entity::EntityId::new("specs", "fine-one"))
        );
    }

    /// Atomicity + report-all: a batch with several invalid entries
    /// writes NOTHING (no entity, no head movement) and names EVERY
    /// failing entry with its typed code — not only the first.
    #[test]
    fn batch_create_refuses_whole_batch_reporting_every_failure() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Existing");
        let (actor, client) = cli_actor();
        let head_before = engine
            .mem_head_sha("specs")
            .ok()
            .flatten()
            .unwrap_or_default();
        let count_before = engine.store().all_entities().count();

        let plain = |title: &str| (empty_create_args("specs", title), None);
        let result = engine
            .batch_create(
                vec![
                    plain("Fine One"),
                    plain("Existing"),   // duplicate vs pre-batch store
                    plain("Bad\nTitle"), // control character in title
                    plain("Fine Two"),
                    plain("Fine Two"), // duplicate WITHIN the batch
                ],
                actor,
                Some(&client),
                false,
            )
            .unwrap();
        assert!(!result.applied);
        assert_eq!(result.failed, 3, "{result:?}");
        assert!(result.write_id.is_empty());
        let codes: Vec<(usize, &str)> = result
            .results
            .iter()
            .enumerate()
            .filter(|(_, r)| r.action == "error")
            .map(|(i, r)| (i, r.error.as_ref().map(|e| e.code.as_str()).unwrap_or("")))
            .collect();
        assert_eq!(
            codes,
            vec![
                (1, "ENTITY_ALREADY_EXISTS"),
                (2, "INVALID_TITLE"),
                (4, "ENTITY_ALREADY_EXISTS"),
            ],
            "every failing entry named with index + typed code: {result:?}"
        );
        // Valid entries are marked not_applied, and NOTHING was written.
        assert_eq!(result.results[0].action, "not_applied");
        assert_eq!(result.results[3].action, "not_applied");
        let head_after = engine
            .mem_head_sha("specs")
            .ok()
            .flatten()
            .unwrap_or_default();
        assert_eq!(head_before, head_after, "mem head unmoved");
        assert_eq!(
            engine.store().all_entities().count(),
            count_before,
            "no entity created, no skeleton left behind"
        );
        let _ = seeded;
    }

    /// Bounded reporting: with more failing entries than the cap, the
    /// report carries the cap's worth of detailed envelopes and counts
    /// the suppressed remainder — never a silent truncation.
    #[test]
    fn batch_create_bounds_the_failure_report() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        let n = Engine::BATCH_ERROR_REPORT_CAP + 10;
        let batch: Vec<_> = (0..n)
            .map(|i| (empty_create_args("specs", &format!("Bad\nTitle {i}")), None))
            .collect();
        let result = engine
            .batch_create(batch, actor, Some(&client), false)
            .unwrap();
        assert!(!result.applied);
        assert_eq!(result.failed, n);
        let detailed = result
            .results
            .iter()
            .filter(|r| r.action == "error" && r.error.is_some())
            .count();
        let bare = result
            .results
            .iter()
            .filter(|r| r.action == "error" && r.error.is_none())
            .count();
        assert_eq!(detailed, Engine::BATCH_ERROR_REPORT_CAP);
        assert_eq!(bare, 10);
        assert_eq!(
            result.errors_suppressed, 10,
            "suppression is counted, never silent"
        );
    }

    #[test]
    fn create_entity_writes_through_folder_backend_and_updates_store() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let outcome = engine
            .create_entity(
                empty_create_args("specs", "Hello World"),
                actor,
                Some(&client),
                Some("first draft"),
            )
            .unwrap();

        // Outcome reports a real id, real file path, real hash.
        assert_eq!(outcome.id.to_string(), "specs--hello-world");
        assert_eq!(outcome.file_path, "hello-world.md");
        assert!(!outcome.content_hash.is_empty());

        // Store has the new entity.
        let entity = engine
            .get_entity(&crate::EntityId::new("specs", "hello-world"))
            .expect("entity must be in the store after create");
        assert_eq!(entity.title, "Hello World");
        assert_eq!(entity.entity_type, "spec");
        assert_eq!(entity.content_hash, outcome.content_hash);

        // On-disk markdown exists at the expected path.
        let on_disk = std::fs::read_to_string(mem_dir.join("hello-world.md")).unwrap();
        assert!(on_disk.contains("# Hello World"));
        assert!(on_disk.contains("type: spec"));

        // Provenance log has the create record.
        let log_path = mem_dir.join(".memstead").join("changes.jsonl");
        let log = std::fs::read_to_string(&log_path).unwrap();
        assert!(log.contains("\"kind\":\"create\""));
        assert!(log.contains("\"entity\":\"specs--hello-world\""));
        assert!(log.contains("\"actor\":\"cli\""));
        assert!(log.contains("\"note\":\"first draft\""));
    }

    /// Supplying a
    /// value for an auto-managed field (`created_date`) on create no
    /// longer silently discards it — the response carries an
    /// `IGNORED_READONLY_FIELD` warning, and the stored value is the
    /// engine-stamped one, not the supplied `2020-01-01`.
    #[test]
    fn create_entity_warns_on_supplied_auto_managed_field() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let mut args = empty_create_args("specs", "Dated Entity");
        args.metadata
            .insert("created_date".to_string(), "2020-01-01".to_string());

        let outcome = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap();

        let warned = outcome.warnings.iter().any(|w| {
            w.code() == "IGNORED_READONLY_FIELD"
                && matches!(w, WarningHint::IgnoredReadonlyField { field, supplied }
                    if field == "created_date" && supplied == "2020-01-01")
        });
        assert!(
            warned,
            "expected IGNORED_READONLY_FIELD; got {:?}",
            outcome.warnings
        );

        // The engine value was stamped, not the supplied 2020 date.
        assert_ne!(outcome.created_date, "2020-01-01");
    }

    /// Complement: a create with no auto-managed field supplied emits no
    /// `IGNORED_READONLY_FIELD` warning.
    #[test]
    fn create_entity_no_warning_when_auto_managed_field_absent() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let outcome = engine
            .create_entity(
                empty_create_args("specs", "Plain Entity"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(
            !outcome
                .warnings
                .iter()
                .any(|w| w.code() == "IGNORED_READONLY_FIELD"),
            "no auto-managed field supplied — no warning expected; got {:?}",
            outcome.warnings
        );
    }

    #[test]
    fn create_entity_returns_write_id_title_mem_on_real_write() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let outcome = engine
            .create_entity(
                empty_create_args("specs", "Rich Shape"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Folder backend produces a synthetic CommitId — wire-equiv
        // to full's commit SHA.
        assert!(
            !outcome.write_id.is_empty(),
            "write_id must be populated on a real create"
        );
        // title + mem echoed from args (full CreateResult parity).
        assert_eq!(outcome.title, "Rich Shape");
        assert_eq!(outcome.mem, "specs");
        // The create path refuses on missing required sections, so
        // `empty_create_args` seeds identity + purpose and the
        // success path's warnings vec carries no
        // `MissingRequiredSection` entries. The dedicated refusal
        // tests below exercise the gate directly.
        assert!(
            !outcome
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::MissingRequiredSection { .. })),
            "success path must not carry MissingRequiredSection warnings — those refuse on create now",
        );
    }

    /// Missing
    /// required sections refuse on create. The error envelope names
    /// every missing key (in schema-declaration order), carries each
    /// section's `write_rules`, and surfaces the type-level
    /// `type_guidance` map keyed by `entity_type`.
    #[test]
    fn create_entity_refuses_missing_required_sections_with_typed_envelope() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        // `spec` requires `identity` + `purpose`. Supply neither.
        let args = CreateEntityArgs {
            anchors: Vec::new(),
            mem: "specs".to_string(),
            title: "Half Done".to_string(),
            entity_type: "spec".to_string(),
            sections: IndexMap::new(),
            metadata: IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        };
        let err = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap_err();
        match err {
            EngineError::MissingRequiredSection {
                entity_type,
                missing_count,
                sections,
                type_guidance,
                pre_announced_missing_fields: _,
            } => {
                assert_eq!(entity_type, "spec");
                assert_eq!(missing_count, sections.len());
                assert!(
                    missing_count >= 2,
                    "expected ≥2 missing sections, got {missing_count}"
                );
                let keys: Vec<String> = sections.iter().map(|s| s.key.clone()).collect();
                assert!(
                    keys.contains(&"identity".to_string()),
                    "missing keys: {keys:?}"
                );
                assert!(
                    keys.contains(&"purpose".to_string()),
                    "missing keys: {keys:?}"
                );
                assert!(
                    type_guidance.contains_key("spec"),
                    "type_guidance must include `spec` entry, got: {type_guidance:?}",
                );
            }
            other => panic!("expected MissingRequiredSection, got {other:?}"),
        }

        // No entity landed in the store.
        let id = crate::EntityId::new("specs", "half-done");
        assert!(
            engine.store().get(&id).is_none(),
            "refused create must not persist any entity"
        );
    }

    /// Cross-gate pre-announcement (backlog-sweep/09): a first write
    /// failing BOTH the section gate and the metadata gate learns both
    /// demands in the one `MISSING_REQUIRED_SECTION` refusal — the
    /// pre-announced set names exactly what `REQUIRED_FIELD_UNSET`
    /// would demand next — and a second submission fixing everything
    /// announced succeeds. The cold write that took three round-trips
    /// takes two.
    #[test]
    fn create_refusing_sections_pre_announces_metadata_gate_and_fixed_retry_succeeds() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_planning_schema(&tmp);
        let (actor, client) = cli_actor();

        // Round-trip 1: neither gate satisfied — no sections, no
        // metadata. `planning.decision` requires sections decision/
        // context/consequences and no-default fields decided_on +
        // deciders.
        let bare = CreateEntityArgs {
            anchors: Vec::new(),
            mem: "planning".to_string(),
            title: "Cold Write".to_string(),
            entity_type: "decision".to_string(),
            sections: IndexMap::new(),
            metadata: IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        };
        let err = engine
            .create_entity(bare, actor, Some(&client), None)
            .unwrap_err();
        let (sections, announced) = match err {
            EngineError::MissingRequiredSection {
                sections,
                pre_announced_missing_fields,
                ..
            } => (sections, pre_announced_missing_fields),
            other => panic!("expected MissingRequiredSection, got {other:?}"),
        };
        let section_keys: Vec<&str> = sections.iter().map(|s| s.key.as_str()).collect();
        for key in ["decision", "context", "consequences"] {
            assert!(section_keys.contains(&key), "sections: {section_keys:?}");
        }
        // The announcement is exactly the metadata gate's demand set —
        // nothing speculative, nothing withheld that is knowable.
        let announced_keys: Vec<&str> = announced.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(
            announced_keys,
            vec!["decided_on", "deciders"],
            "pre-announced set must equal the metadata gate's demand in declaration order"
        );
        // The wire payload carries the block under `pre_announced`, in
        // REQUIRED_FIELD_UNSET's established `missing[]` element shape.
        let rebuilt = EngineError::MissingRequiredSection {
            entity_type: "decision".to_string(),
            missing_count: sections.len(),
            sections,
            type_guidance: Default::default(),
            pre_announced_missing_fields: announced,
        };
        let details = rebuilt.details();
        let wire_missing = details["pre_announced"]["required_field_unset"]["missing"]
            .as_array()
            .expect("pre_announced.required_field_unset.missing[] present");
        assert_eq!(wire_missing[0]["field"], "decided_on");
        assert!(wire_missing[0].get("description").is_some());
        assert!(wire_missing[0].get("enum_values").is_some());

        // Round-trip 2: fix everything the one refusal announced —
        // and nothing else. Succeeds: no third round-trip exists.
        let mut metadata = IndexMap::new();
        metadata.insert("decided_on".to_string(), "2026-08-19".to_string());
        metadata.insert("deciders".to_string(), "alice".to_string());
        engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "planning".to_string(),
                    title: "Cold Write".to_string(),
                    entity_type: "decision".to_string(),
                    sections: IndexMap::from_iter([
                        ("decision".to_string(), "x".to_string()),
                        ("context".to_string(), "y".to_string()),
                        ("consequences".to_string(), "z".to_string()),
                    ]),
                    metadata,
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("fixing everything announced must succeed in the second round-trip");
    }

    /// Complement (backlog-sweep/09 criterion 3): a body failing ONLY
    /// the section gate — metadata complete — refuses with an empty
    /// pre-announcement, and its `details` payload carries no
    /// `pre_announced` key at all: byte-compatible with the
    /// pre-announcement-free shape.
    #[test]
    fn section_only_refusal_omits_the_pre_announced_block() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_planning_schema(&tmp);
        let (actor, client) = cli_actor();

        let mut metadata = IndexMap::new();
        metadata.insert("decided_on".to_string(), "2026-08-19".to_string());
        metadata.insert("deciders".to_string(), "alice".to_string());
        let err = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "planning".to_string(),
                    title: "Sections Only".to_string(),
                    entity_type: "decision".to_string(),
                    sections: IndexMap::new(),
                    metadata,
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match &err {
            EngineError::MissingRequiredSection {
                pre_announced_missing_fields,
                ..
            } => assert!(
                pre_announced_missing_fields.is_empty(),
                "metadata gate is satisfied — nothing to pre-announce"
            ),
            other => panic!("expected MissingRequiredSection, got {other:?}"),
        }
        assert!(
            err.details().get("pre_announced").is_none(),
            "single-gate refusal must stay byte-compatible: no pre_announced key"
        );
    }

    /// `dry_run: true` returns the same refusal envelope
    /// the real call would. The preview surface doesn't admit content
    /// the real call would refuse.
    #[test]
    fn create_entity_dry_run_returns_same_refusal_envelope_as_real_call() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let args = CreateEntityArgs {
            anchors: Vec::new(),
            mem: "specs".to_string(),
            title: "Half Done Dry".to_string(),
            entity_type: "spec".to_string(),
            sections: IndexMap::new(),
            metadata: IndexMap::new(),
            relations: Vec::new(),
            dry_run: true,
        };
        let err = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap_err();
        assert!(
            matches!(err, EngineError::MissingRequiredSection { .. }),
            "dry_run must surface the same refusal envelope, got {err:?}"
        );
    }

    /// A follow-up call with the missing sections filled
    /// in succeeds. The refusal carries enough recovery information
    /// that the agent's next attempt resolves in one round-trip.
    #[test]
    fn create_entity_succeeds_after_filling_in_required_sections() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "the identity body".to_string());
        sections.insert("purpose".to_string(), "the purpose body".to_string());
        let args = CreateEntityArgs {
            anchors: Vec::new(),
            mem: "specs".to_string(),
            title: "Complete".to_string(),
            entity_type: "spec".to_string(),
            sections,
            metadata: IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        };
        let outcome = engine
            .create_entity(args, actor, Some(&client), None)
            .expect("complete create succeeds");
        assert_eq!(outcome.title, "Complete");
    }

    #[test]
    fn create_entity_promotes_existing_stub_and_preserves_incoming_edges() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Source");
        let (actor, client) = cli_actor();

        // Step 1: relate source → "ghost-target" — creates a stub
        // entity at `specs--ghost-target` with one incoming edge.
        let stub_target = crate::EntityId::new("specs", "ghost-target");
        engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: stub_target.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let stub = engine
            .store()
            .get(&stub_target)
            .expect("stub must be in store");
        assert!(stub.stub);
        assert_eq!(engine.store().incoming(&stub_target).len(), 1);

        // Step 2: create a real entity with the same title — should
        // promote the stub and preserve the incoming edge.
        let outcome = engine
            .create_entity(
                empty_create_args("specs", "Ghost Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // No error: stub adoption proceeded.
        assert_eq!(outcome.id, stub_target);
        // Entity is now a real entity, not a stub.
        let real = engine
            .store()
            .get(&stub_target)
            .expect("entity must still be in store");
        assert!(!real.stub);
        // Incoming edge survived the upsert.
        assert_eq!(engine.store().incoming(&stub_target).len(), 1);
        // Outcome surfaces stub adoption.
        assert_eq!(outcome.incoming_count, Some(1));
        assert_eq!(outcome.incoming.len(), 1);
        assert_eq!(outcome.incoming[0].from, source.id);
        assert_eq!(outcome.incoming[0].rel_type, "USES");
    }

    #[test]
    fn create_entity_reports_no_incoming_on_greenfield_create() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let outcome = engine
            .create_entity(
                empty_create_args("specs", "Greenfield"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        // No pre-existing stub → incoming_count is None, incoming vec
        // is empty. Full's wire shape skip-serialises both.
        assert!(outcome.incoming_count.is_none());
        assert!(outcome.incoming.is_empty());
    }

    #[test]
    fn create_entity_populates_created_date_from_schema_auto_stamp() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let outcome = engine
            .create_entity(
                empty_create_args("specs", "Has Date"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        // The default `spec` schema declares `created_date` with
        // an init_timestamp default. The parsed entity carries the
        // auto-stamped value; the outcome surfaces it for callers
        // who need it without a follow-up read.
        assert!(
            !outcome.created_date.is_empty(),
            "created_date must be populated when the schema auto-stamps it"
        );
    }

    #[test]
    fn create_overrides_user_supplied_timestamps_update_rejects_them() {
        // Schema-declared `init_timestamp` (set on create) and
        // `auto_timestamp` (re-stamped on every update) fields are
        // engine-managed. On create the engine still silently
        // overrides any caller-supplied value (the entity must be
        // stampable in one shot from the user's perspective). On
        // update the writable-metadata validator rejects the write
        // up-front with `READ_ONLY_FIELD` — the agent gets a
        // structured rejection instead of a "set" response whose
        // value the auto-stamp pass silently discards (per the F13
        // / F14 contract).
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        // Pin the mutation clock. The auto-stamp is second-resolution,
        // and the assertions below compare it against a separately
        // computed "now" — so an unpinned run fails whenever a second
        // ticks between the create and the comparison. That is a real
        // flake, not a theoretical one: it fired on a suite run that
        // straddled midnight. The engine's injectable clock exists for
        // exactly this, and pinning it here also makes the expected
        // string a constant rather than a second read of the wall clock.
        const FROZEN_SECS: u64 = 1_754_000_000;
        let frozen = std::time::UNIX_EPOCH + std::time::Duration::from_secs(FROZEN_SECS);
        engine.set_mutation_clock(std::sync::Arc::new(move || frozen));
        let (actor, client) = cli_actor();

        // Caller supplies a past value for the init_timestamp field
        // and the auto_timestamp field. The engine ignores both on
        // create.
        let mut args = empty_create_args("specs", "Stamped Today");
        args.metadata
            .insert("created_date".to_string(), "2020-01-01".to_string());
        args.metadata
            .insert("last_modified".to_string(), "2020-01-01".to_string());

        let outcome = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap();

        // Both timestamps should reflect the engine's own clock, not
        // the caller's `2020-01-01`.
        let today = crate::engine::mutation::iso_from_system_time(frozen);
        assert_eq!(outcome.created_date, today);
        let entity = engine
            .get_entity(&outcome.id)
            .expect("entity must be in store after create");
        assert_eq!(
            entity
                .metadata
                .get("created_date")
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
            today,
            "init_timestamp field must be engine-determined on create, not user-supplied"
        );
        assert_eq!(
            entity
                .metadata
                .get("last_modified")
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
            today,
            "auto_timestamp field must be engine-determined on create, not user-supplied"
        );

        // F13/F14: update rejects a user-supplied value for either
        // init_timestamp or auto_timestamp metadata fields with
        // `READ_ONLY_FIELD`. Test both fields in turn.
        let attempt_update = |key: &str, value: &str| {
            let mut metadata = IndexMap::new();
            metadata.insert(key.to_string(), value.to_string());
            crate::engine::UpdateEntityArgs {
                anchors: Vec::new(),
                id: outcome.id.clone(),
                metadata,
                metadata_unset: Vec::new(),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                expected_hash: Some(outcome.content_hash.clone()),
                dry_run: false,
                declare_relations: Vec::new(),
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            }
        };
        for key in ["created_date", "last_modified"] {
            let err = engine
                .update_entity(
                    attempt_update(key, "2019-12-31"),
                    actor,
                    Some(&client),
                    None,
                )
                .expect_err("schema-managed timestamp must be rejected on update");
            assert_eq!(err.code(), "READ_ONLY_FIELD", "got: {err:?}");
        }
        // Stored value is unchanged after a rejected attempt.
        let entity = engine
            .get_entity(&outcome.id)
            .expect("entity must remain in store after rejected update");
        assert_eq!(
            entity
                .metadata
                .get("last_modified")
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
            today,
            "rejected update must not mutate the auto_timestamp field"
        );
    }

    #[test]
    fn create_entity_wires_inline_relations_and_stubs_absent_targets() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, existing) = engine_with_seed(&tmp, "Existing Target");
        let (actor, client) = cli_actor();
        let absent = crate::EntityId::new("specs", "future-target");
        assert!(!engine.store().contains(&absent));

        let mut args = empty_create_args("specs", "Source With Relations");
        args.relations = vec![
            crate::ops::RelateArg {
                target: existing.id.clone(),
                rel_type: "USES".to_string(),
                description: None,
            },
            crate::ops::RelateArg {
                target: absent.clone(),
                rel_type: "USES".to_string(),
                description: None,
            },
        ];

        let outcome = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap();

        // New entity in store with both edges materialised.
        let source = engine
            .store()
            .get(&outcome.id)
            .expect("source must be in store");
        assert_eq!(source.relationships.len(), 2);
        assert!(
            source
                .relationships
                .iter()
                .any(|r| r.target == existing.id && r.rel_type == "USES")
        );
        assert!(
            source
                .relationships
                .iter()
                .any(|r| r.target == absent && r.rel_type == "USES")
        );

        // Absent target was auto-stubbed (mirrors the relate path's
        // ensure_target).
        let stub = engine
            .store()
            .get(&absent)
            .expect("absent relation target must be auto-stubbed");
        assert!(stub.stub);
        // Existing target unchanged.
        let existing_after = engine.store().get(&existing.id).unwrap();
        assert!(!existing_after.stub);
    }

    /// Build a folder-mount engine pinned to the `planning` schema, so
    /// tests can exercise `decision` — a type with `decided_on` (Date,
    /// required, no default / no init_timestamp) — without inventing a
    /// synthetic schema.
    fn engine_with_planning_schema(tmp: &TempDir) -> Engine {
        use crate::workspace::Mount;
        use crate::workspace::{MountCapability, MountLifecycle, MountStorage};
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mount = Mount {
            mem: "planning".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "planning",
                semver::Version::new(0, 1, 0),
            )),
            storage: MountStorage::Folder { path: mem_dir },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)]).unwrap()
    }

    /// A
    /// required metadata field the schema does not auto-fill
    /// (`default_value` / `init_timestamp` / `auto_timestamp` all
    /// absent) now triggers `REQUIRED_FIELD_UNSET` refusal on the
    /// create path. Pre-fix this surfaced as a `MissingRequiredField`
    /// warning and the generator silently wrote placeholder values
    /// that the install-time strict validator could later refuse,
    /// breaking the export-then-install round-trip.
    #[test]
    fn create_entity_refuses_unsupplied_no_default_required_field() {
        // The `planning.decision` schema declares `decided_on`
        // (Date, required, no default_value, no init_timestamp) and
        // `deciders` (String csv_array, required, no default).
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_planning_schema(&tmp);
        let (actor, client) = cli_actor();

        let mut args = CreateEntityArgs {
            anchors: Vec::new(),
            mem: "planning".to_string(),
            title: "Skip Postgres".to_string(),
            entity_type: "decision".to_string(),
            sections: IndexMap::from_iter([
                ("decision".to_string(), "Use SQLite locally.".to_string()),
                ("context".to_string(), "Single-user dev.".to_string()),
                ("consequences".to_string(), "Lose multi-writer.".to_string()),
            ]),
            metadata: IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        };

        // Real-write path: refuse on the first missing field
        // (declaration order).
        let err = engine
            .create_entity(args.clone(), actor, Some(&client), None)
            .unwrap_err();
        match err {
            EngineError::RequiredFieldUnset {
                field, entity_type, ..
            } => {
                assert!(
                    field == "decided_on" || field == "deciders",
                    "expected first missing field, got {field:?}"
                );
                assert_eq!(entity_type, "decision");
            }
            other => panic!("expected RequiredFieldUnset, got {other:?}"),
        }

        // Dry-run path on the same shape (different title to avoid the
        // already-exists check). Must surface the same refusal — the
        // create dry-run is the agent's preview surface.
        args.title = "Different Title".to_string();
        args.dry_run = true;
        let dry_err = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap_err();
        assert!(
            matches!(dry_err, EngineError::RequiredFieldUnset { .. }),
            "dry_run must surface the same refusal envelope, got {dry_err:?}"
        );
    }

    /// A follow-up call with all required-no-default
    /// fields supplied succeeds. The refusal recovery is a single
    /// round-trip.
    #[test]
    fn create_entity_succeeds_when_all_required_no_default_fields_supplied() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_with_planning_schema(&tmp);
        let (actor, client) = cli_actor();

        let mut metadata = IndexMap::new();
        metadata.insert("decided_on".to_string(), "2026-05-13".to_string());
        metadata.insert("deciders".to_string(), "alice, bob".to_string());

        let outcome = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "planning".to_string(),
                    title: "Complete Decision".to_string(),
                    entity_type: "decision".to_string(),
                    sections: IndexMap::from_iter([
                        ("decision".to_string(), "x".to_string()),
                        ("context".to_string(), "y".to_string()),
                        ("consequences".to_string(), "z".to_string()),
                    ]),
                    metadata,
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("complete decision create succeeds");
        // No MissingRequiredField warnings on the success path —
        // refusal swallows the case before any warning could fire.
        let missing_field_warnings: Vec<&WarningHint> = outcome
            .warnings
            .iter()
            .filter(|w| matches!(w, WarningHint::MissingRequiredField { .. }))
            .collect();
        assert!(
            missing_field_warnings.is_empty(),
            "success path must not carry MissingRequiredField warnings, got: {missing_field_warnings:?}"
        );
    }

    /// Item 02: `memstead_create.relations[]` runs the same target-id
    /// grammar gate as `memstead_relate`. Pre-fix the create path
    /// admitted malformed ids (auto-stub at `bad@chars$here`) even
    /// though `memstead_relate` rejected them.
    #[test]
    fn create_entity_rejects_inline_relation_with_malformed_target_id() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let mut args = empty_create_args("specs", "Source");
        args.relations = vec![crate::ops::RelateArg {
            target: crate::EntityId("specs--bad target with spaces!!".to_string()),
            rel_type: "USES".to_string(),
            description: None,
        }];
        let err = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidEntityId { .. }),
            "malformed target id must trip INVALID_ENTITY_ID on the create path; got {err:?}",
        );
    }

    /// Item 02: `memstead_create.relations[]` runs the same schema-shape
    /// gate as `memstead_relate`. The relate-path shape gate is already
    /// pinned by `memstead-mcp::tool_surface::INVALID_REL_SHAPE` and the
    /// schema-loader tests; the cross-path lock here exercises the
    /// `software` schema's `VIOLATES` rel-type, which declares
    /// `source_types: [incident]` — an inline create from a `spec`
    /// must trip the shape gate even though the rel-type itself is
    /// valid vocabulary.
    #[test]
    fn create_entity_rejects_inline_relation_with_shape_violation() {
        use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mount = Mount {
            mem: "code".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "software",
                semver::Version::new(0, 1, 0),
            )),
            storage: MountStorage::Folder { path: mem_dir },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let mut engine =
            Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)]).unwrap();
        let (actor, client) = cli_actor();

        // Seed an existing target so the shape gate evaluates the
        // real target type (not `None`, which the gate admits as the
        // stub-bound case). The `requirement` type requires `statement` +
        // `rationale` sections plus `verified_on` + `source` metadata
        // (the schema lists these without `default_value` or
        // `optional: true`, so the strict-on-create gate refuses
        // unless supplied).
        let target = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "code".to_string(),
                    title: "Target Requirement".to_string(),
                    entity_type: "requirement".to_string(),
                    sections: IndexMap::from_iter([
                        ("statement".to_string(), "MUST hold.".to_string()),
                        ("rationale".to_string(), "Because tests.".to_string()),
                    ]),
                    metadata: IndexMap::from_iter([
                        ("verified_on".to_string(), "2026-05-19".to_string()),
                        ("source".to_string(), "test fixture".to_string()),
                    ]),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // `VIOLATES` declares `source_types: [incident]`. A `spec`
        // create with `VIOLATES` violates the shape. The `spec` type
        // in the software schema requires `identity` + `purpose`;
        // supply both so the shape gate (not the missing-sections
        // gate) is what fires.
        let args = CreateEntityArgs {
            anchors: Vec::new(),
            mem: "code".to_string(),
            title: "Misshape Source".to_string(),
            entity_type: "spec".to_string(),
            sections: IndexMap::from_iter([
                ("identity".to_string(), "this spec".to_string()),
                (
                    "purpose".to_string(),
                    "exercising the shape gate".to_string(),
                ),
            ]),
            metadata: IndexMap::new(),
            relations: vec![crate::ops::RelateArg {
                target: target.id.clone(),
                rel_type: "VIOLATES".to_string(),
                description: None,
            }],
            dry_run: false,
        };
        let err = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap_err();
        assert!(
            matches!(err, EngineError::Validation(_)),
            "shape violation must trip Validation(InvalidRelationshipShape); got {err:?}",
        );
    }

    #[test]
    fn create_entity_canonicalises_inline_relation_rel_types_to_upper_snake_case() {
        // Wire-level contract: rel_type on inline relations is
        // case-insensitive. The engine stores the relationship as
        // UPPER_SNAKE_CASE regardless of input case.
        let tmp = TempDir::new().unwrap();
        let (mut engine, existing) = engine_with_seed(&tmp, "Existing Target");
        let (actor, client) = cli_actor();

        let mut args = empty_create_args("specs", "Source With Mixed Case Rel");
        args.relations = vec![crate::ops::RelateArg {
            target: existing.id.clone(),
            rel_type: "uses".to_string(),
            description: None,
        }];

        let outcome = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap();

        let source = engine
            .store()
            .get(&outcome.id)
            .expect("source must be in store");
        assert_eq!(source.relationships.len(), 1);
        assert_eq!(
            source.relationships[0].rel_type, "USES",
            "inline relation rel_type must be stored UPPER_SNAKE_CASE",
        );
    }

    #[test]
    fn create_entity_dry_run_skips_disk_and_store_yet_returns_hash() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let mut args = empty_create_args("specs", "Preview Only");
        args.dry_run = true;

        let outcome = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap();

        // Wire shape: content_hash = prospective hash; write_id empty.
        assert_eq!(outcome.id.to_string(), "specs--preview-only");
        assert!(
            !outcome.content_hash.is_empty(),
            "prospective hash populated"
        );
        assert!(outcome.write_id.is_empty(), "no commit on dry_run");
        // No store entry — the engine didn't push.
        assert!(
            engine.store().get(&outcome.id).is_none(),
            "dry_run must not mutate the store",
        );
        // No file on disk.
        assert!(
            !mem_dir.join("preview-only.md").exists(),
            "dry_run must not touch disk",
        );
        // No provenance line.
        let log = mem_dir.join(".memstead").join("changes.jsonl");
        assert!(
            !log.exists()
                || !std::fs::read_to_string(&log)
                    .unwrap()
                    .contains("preview-only"),
            "dry_run must not append provenance",
        );
    }

    #[test]
    fn create_entity_rejects_read_only_mount_before_backend() {
        let tmp = TempDir::new().unwrap();
        let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"# a")]);
        let mut engine = Engine::from_mounts(vec![(
            archive_mount("external", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)),
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let err = engine
            .create_entity(
                empty_create_args("external", "Should Fail"),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::ReadOnlyMount(v) => assert_eq!(v, "external"),
            other => panic!("expected ReadOnlyMount, got {other:?}"),
        }
        // Capability gating runs before the backend → the typed
        // BackendError::Sealed variant never surfaces here. That's
        // the intended ordering.
    }

    #[test]
    fn create_entity_rejects_unknown_mem() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", tmp.path().to_path_buf()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let err = engine
            .create_entity(
                empty_create_args("does-not-exist", "Anything"),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::UnknownMem(v) if v == "does-not-exist"));
    }

    #[test]
    fn create_entity_rejects_unknown_type_against_pinned_schema() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let mut args = empty_create_args("specs", "Anything");
        args.entity_type = "definitely-not-a-real-type".to_string();
        let err = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap_err();
        match err {
            EngineError::UnknownType { name, declared, .. } => {
                assert_eq!(name, "definitely-not-a-real-type");
                assert!(!declared.is_empty(), "declared types must be listed");
            }
            other => panic!("expected UnknownType, got {other:?}"),
        }
    }

    #[test]
    fn create_entity_rejects_duplicate_id() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        engine
            .create_entity(
                empty_create_args("specs", "Same Slug"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let err = engine
            .create_entity(
                empty_create_args("specs", "Same Slug"),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::AlreadyExists {
                id,
                existing_title,
                existing_is_stub,
            } => {
                assert_eq!(id, "specs--same-slug");
                // The refusal names the occupying title so the caller
                // sees which existing title derived the colliding slug.
                assert!(!existing_title.is_empty());
                assert!(!existing_is_stub);
            }
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
    }

    #[test]
    fn create_entity_rejects_invalid_title() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        // F4: empty/whitespace-only titles now refuse with
        // `INVALID_TITLE` / reason `empty`. The earlier hash-fallback
        // behaviour applies only to the loader path (pre-gate
        // entities); the strict mutation gate rejects so the
        // structured-content envelope can carry actionable details.
        let err = engine
            .create_entity(empty_create_args("specs", "  "), actor, Some(&client), None)
            .unwrap_err();
        match err {
            EngineError::InvalidTitle(slug_err) => {
                assert_eq!(slug_err.reason(), "empty", "expected empty reason");
            }
            other => panic!("expected InvalidTitle/TitleEmpty, got {other:?}"),
        }

        // Widened grammar: char-drop titles land, with the divergence
        // reported as the typed warning naming the dropped characters
        // and the derived slug.
        let outcome = engine
            .create_entity(
                empty_create_args("specs", "Hello, World!"),
                actor,
                Some(&client),
                None,
            )
            .expect("char-drop title lands under the widened grammar");
        assert_eq!(outcome.id.as_ref(), "specs--hello-world");
        let dropped = outcome
            .warnings
            .iter()
            .find_map(|w| match w {
                WarningHint::TitleCharsDroppedFromSlug {
                    dropped_chars,
                    slug,
                    ..
                } => Some((dropped_chars.clone(), slug.clone())),
                _ => None,
            })
            .expect("divergence warning rides the outcome");
        assert!(dropped.0.contains(&',') && dropped.0.contains(&'!'));
        assert_eq!(dropped.1, "hello-world");

        // Path-traversal-shaped titles are display text too — the
        // dropped `/` and `.` never reach the slug, so the id stays
        // sanitised (no traversal), and the divergence is reported.
        let outcome = engine
            .create_entity(
                empty_create_args("specs", "../etc/passwd"),
                actor,
                Some(&client),
                None,
            )
            .expect("traversal-shaped title lands with a sanitised slug");
        assert_eq!(outcome.id.as_ref(), "specs--etcpasswd");
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::TitleCharsDroppedFromSlug { .. }))
        );
    }

    #[test]
    fn create_entity_rejects_unknown_section_key() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let mut args = empty_create_args("specs", "Bad Sections");
        args.sections
            .insert("not-a-real-section-key".to_string(), "body".to_string());
        let err = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap_err();
        assert!(matches!(err, EngineError::Validation(_)));
    }

    #[test]
    fn create_entity_persists_across_engine_restart() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        {
            let writer = FilesystemMemWriter::new(mem_dir.clone());
            let mut engine = Engine::from_mounts(vec![(
                folder_mount("specs", mem_dir.clone()),
                Box::new(writer) as Box<dyn MemBackend>,
            )])
            .unwrap();
            let (actor, client) = cli_actor();
            engine
                .create_entity(
                    empty_create_args("specs", "Survives Restart"),
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap();
        }
        // New engine reading the same mem must see the entity.
        let writer2 = FilesystemMemWriter::new(mem_dir.clone());
        let engine2 = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer2) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let entity = engine2
            .get_entity(&crate::EntityId::new("specs", "survives-restart"))
            .expect("entity must persist across engine restart");
        assert_eq!(entity.title, "Survives Restart");
    }

    // ---- Engine::update_entity --------------------------------------

    /// Build a folder-mount Engine with one freshly-created entity.
    /// Returns the engine + the created outcome so tests have the
    /// id and current hash to use as `expected_hash` for the next
    /// mutation.
    fn engine_with_seed(tmp: &TempDir, title: &str) -> (Engine, CreateEntityOutcome) {
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        let outcome = engine
            .create_entity(
                empty_create_args("specs", title),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        (engine, outcome)
    }

    /// Create with
    /// a body wiki-link to a non-existent target emits
    /// `INLINE_WIKI_LINK_AUTO_STUBBED` with the stubbed target id in
    /// `details.stubs`. Pre-fix the warning never fired because the
    /// emission walked `parse_markdown(generated_markdown).inline_links`,
    /// which the parser-side coverage filter had already emptied for
    /// the alias-synthesised body link.
    #[test]
    fn create_entity_emits_inline_wiki_link_auto_stubbed_for_new_stub_target() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let ghost = crate::EntityId::new("specs", "ghost-target");
        assert!(!engine.store().contains(&ghost), "ghost must not pre-exist");

        let mut args = empty_create_args("specs", "Source With Body Link");
        args.sections.insert(
            "identity".to_string(),
            "ref [[ghost-target]] for context".to_string(),
        );

        let outcome = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap();
        let stubbed: Vec<&crate::EntityId> = outcome
            .warnings
            .iter()
            .filter_map(|w| match w {
                WarningHint::InlineWikiLinkAutoStubbed { stubs, .. } => Some(stubs),
                _ => None,
            })
            .flatten()
            .collect();
        assert!(
            stubbed.contains(&&ghost),
            "INLINE_WIKI_LINK_AUTO_STUBBED warning must name the ghost target; got: {:?}",
            outcome.warnings,
        );
        // The stub also lands in the store and the REFERENCES edge exists.
        assert!(
            engine.store().contains(&ghost),
            "ghost stub must materialise"
        );
    }

    /// CLI F11: a body wiki-link to the entity's own slug is dropped (no
    /// vacuous self-edge) with a `SELF_LINK_IGNORED` warning, while a body
    /// link to a *different* target in the same entity still synthesises
    /// its REFERENCES edge normally — only the self-target is dropped.
    #[test]
    fn create_entity_drops_self_link_keeps_other_links_and_warns() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        // Title "Selfie" → slug "selfie" → id "specs--selfie". The body
        // links its own slug AND a different target.
        let mut args = empty_create_args("specs", "Selfie");
        args.sections.insert(
            "identity".to_string(),
            "see [[selfie]] itself and also [[other-ref]]".to_string(),
        );
        let outcome = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap();
        let self_id = outcome.id.clone();
        assert_eq!(self_id.to_string(), "specs--selfie");
        let other_id = crate::EntityId::new("specs", "other-ref");

        // SELF_LINK_IGNORED warning names the self-linking entity.
        assert!(
            outcome.warnings.iter().any(|w| matches!(
                w, WarningHint::SelfLinkIgnored { id } if *id == self_id
            )),
            "self-link must emit SELF_LINK_IGNORED; got: {:?}",
            outcome.warnings,
        );

        // No self-edge: not in relationships, not Outgoing, not Incoming.
        let ent = engine.get_entity(&self_id).unwrap();
        assert!(
            ent.relationships.iter().all(|r| r.target != self_id),
            "no self-relation may be synthesised; got: {:?}",
            ent.relationships,
        );
        assert!(
            engine
                .store()
                .outgoing(&self_id)
                .iter()
                .all(|e| e.target != self_id),
            "self must not be its own Outgoing neighbour",
        );
        assert!(
            engine
                .store()
                .incoming(&self_id)
                .iter()
                .all(|e| e.from != self_id),
            "self must not be its own Incoming neighbour",
        );

        // Complement: the link to a *different* target synthesised its
        // REFERENCES edge normally.
        assert!(
            ent.relationships
                .iter()
                .any(|r| r.rel_type == "REFERENCES" && r.target == other_id),
            "non-self body link must still synthesise its edge; got: {:?}",
            ent.relationships,
        );
    }

    /// dry_run preview matches real-write outcome.
    #[test]
    fn create_entity_dry_run_emits_same_auto_stub_warning() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let mut args = empty_create_args("specs", "Dry Run Body Link");
        args.dry_run = true;
        args.sections
            .insert("identity".to_string(), "see [[dry-run-ghost]]".to_string());

        let outcome = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap();
        let has_warning = outcome.warnings.iter().any(|w| {
            matches!(
                w,
                WarningHint::InlineWikiLinkAutoStubbed { stubs, .. }
                    if stubs.iter().any(|t| t.to_string() == "specs--dry-run-ghost")
            )
        });
        assert!(
            has_warning,
            "dry_run must emit the same warning as real write: {:?}",
            outcome.warnings
        );
    }

    /// Body wiki-link to a target that already exists
    /// in the store does NOT fire the warning — no stub was created.
    #[test]
    fn create_entity_no_auto_stub_warning_when_target_exists() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, existing) = engine_with_seed(&tmp, "Existing Target");
        let (actor, client) = cli_actor();

        let mut args = empty_create_args("specs", "Source Linking Existing");
        let body = format!("ref [[{}]]", existing.id.path());
        args.sections.insert("identity".to_string(), body);

        let outcome = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap();
        let has_warning = outcome
            .warnings
            .iter()
            .any(|w| matches!(w, WarningHint::InlineWikiLinkAutoStubbed { .. }));
        assert!(
            !has_warning,
            "no auto-stub warning when target pre-exists; got: {:?}",
            outcome.warnings
        );
    }

    /// Obligation-schema counterpart of the ingest wildcard
    /// (first-author-path plan 09, criterion 5): an obligation mem
    /// body-links into a NON-SOFTWARE user-schema destination; the
    /// wildcard alias grant admits the auto-emitted REFERENCES edge.
    #[test]
    fn obligation_wildcard_links_into_arbitrary_destination_schema() {
        use crate::engine::test_helpers::write_schema_files_with_default_type;
        use memstead_schema::workspace_config::CrossLinkValue;

        let tmp = TempDir::new().unwrap();
        let dest_dir = tmp.path().join("dest");
        let duties_dir = tmp.path().join("duties");
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::create_dir_all(&duties_dir).unwrap();
        let schemas_dir = tmp.path().join("schemas");
        let user_manifest = r#"name: casefiles
version: 0.1.0
description: a user-written, non-software destination schema
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        write_schema_files_with_default_type(
            &schemas_dir,
            "casefiles@0.1.0",
            user_manifest,
            &["doc"],
        );

        let mount = |mem: &str, dir: &std::path::Path, schema: &str| crate::workspace::Mount {
            mem: mem.to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                schema,
                semver::Version::new(0, 1, 0),
            )),
            storage: crate::workspace::MountStorage::Folder {
                path: dir.to_path_buf(),
            },
            capability: crate::workspace::MountCapability::Write,
            lifecycle: crate::workspace::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let mounts = vec![
            (
                mount("dest", &dest_dir, "casefiles"),
                Box::new(FilesystemMemWriter::new(dest_dir.clone())) as Box<dyn MemBackend>,
            ),
            (
                mount("duties", &duties_dir, "obligation"),
                Box::new(FilesystemMemWriter::new(duties_dir.clone())) as Box<dyn MemBackend>,
            ),
        ];
        let mut engine = Engine::from_mounts_with_schemas_dir(mounts, Some(schemas_dir.as_path()))
            .expect("obligation + user schema boot");
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_mem_links.insert(
            "duties".to_string(),
            CrossLinkValue::List(vec!["dest".to_string()]),
        );
        engine.set_settings(settings);
        let (actor, client) = cli_actor();

        let target = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "dest".to_string(),
                    title: "Case File 17".to_string(),
                    entity_type: "doc".to_string(),
                    sections: IndexMap::from_iter([(
                        "body".to_string(),
                        "destination content".to_string(),
                    )]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let entry = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "duties".to_string(),
                    title: "File Annual Report & Notice".to_string(),
                    entity_type: "obligation".to_string(),
                    sections: IndexMap::from_iter([
                        (
                            "duty".to_string(),
                            "File the report cited in [[dest--case-file-17]].".to_string(),
                        ),
                        (
                            "consequence".to_string(),
                            "Standing lapses at the deadline.".to_string(),
                        ),
                    ]),
                    metadata: IndexMap::from_iter([
                        ("due_date".to_string(), "2026-12-31".to_string()),
                        ("status".to_string(), "open".to_string()),
                    ]),
                    relations: vec![crate::ops::RelateArg {
                        target: crate::entity::EntityId::new("duties", "subject"),
                        rel_type: "CONCERNS".to_string(),
                        description: None,
                    }],
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("wildcard admits the alias link into the non-software destination");
        let stored = engine.get_entity(&entry.id).unwrap();
        assert!(
            stored
                .relationships
                .iter()
                .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
            "alias REFERENCES edge must emit cross-mem: {:?}",
            stored.relationships
        );
    }

    /// Plan 11 end-to-end: an `ingest`-schema process mem body-links
    /// into a destination pinning an ARBITRARY user-written schema.
    /// The wildcard (bound to `alias_target_rel_type: REFERENCES`)
    /// admits the auto-emitted alias edge; the edge survives a fresh
    /// boot (the load path routes through the same matcher); explicit
    /// authoring of the alias type still refuses
    /// RELATION_MANUAL_AUTHORING_FORBIDDEN; a structural rel-type into
    /// the undeclared destination still refuses
    /// A body wiki-link into a destination schema the source schema
    /// declares NO cross-mem entry for (and no wildcard): the schema
    /// legitimately declines the edge — but the author must be TOLD.
    /// default@1.3.0 declares REFERENCES only toward `software`, so a
    /// default-mem entity citing a planning-mem entity got inert prose
    /// where the author expects a citation edge, silently (graph-plans
    /// 02 grading, 2026-08-28). The write knows it dropped the link, so
    /// it warns, typed, naming the target and the declaration gap.
    #[test]
    fn undeclared_cross_schema_body_link_warns_instead_of_vanishing() {
        use memstead_schema::workspace_config::CrossLinkValue;

        let tmp = TempDir::new().unwrap();
        let scratch_dir = tmp.path().join("scratch");
        let plans_dir = tmp.path().join("plans");
        std::fs::create_dir_all(&scratch_dir).unwrap();
        std::fs::create_dir_all(&plans_dir).unwrap();

        let mount = |mem: &str, dir: &std::path::Path, schema: &str, version: (u64, u64, u64)| {
            crate::workspace::Mount {
                mem: mem.to_string(),
                schema: Some(memstead_schema::SchemaRef::new(
                    schema,
                    semver::Version::new(version.0, version.1, version.2),
                )),
                storage: crate::workspace::MountStorage::Folder {
                    path: dir.to_path_buf(),
                },
                capability: crate::workspace::MountCapability::Write,
                lifecycle: crate::workspace::MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            }
        };
        let mut engine = Engine::from_mounts(vec![
            (
                mount("scratch", &scratch_dir, "default", (1, 3, 0)),
                Box::new(FilesystemMemWriter::new(scratch_dir.clone())) as Box<dyn MemBackend>,
            ),
            (
                mount("plans", &plans_dir, "planning", (0, 4, 0)),
                Box::new(FilesystemMemWriter::new(plans_dir.clone())) as Box<dyn MemBackend>,
            ),
        ])
        .unwrap();
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_mem_links.insert(
            "scratch".to_string(),
            CrossLinkValue::List(vec!["plans".to_string()]),
        );
        engine.set_settings(settings);
        let (actor, client) = cli_actor();

        let target = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "plans".to_string(),
                    title: "Target Plan Note".to_string(),
                    entity_type: "goal".to_string(),
                    sections: IndexMap::from_iter([
                        (
                            "statement".to_string(),
                            "the plan goal being cited".to_string(),
                        ),
                        (
                            "rationale".to_string(),
                            "cited from a scratch mem in the repro".to_string(),
                        ),
                        (
                            "success_criteria".to_string(),
                            "the citation edge exists or the drop is warned".to_string(),
                        ),
                    ]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let entry = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "scratch".to_string(),
                    title: "Session Scratch".to_string(),
                    entity_type: "memo".to_string(),
                    sections: IndexMap::from_iter([
                        (
                            "claim".to_string(),
                            "the pilot follows [[plans--target-plan-note]] step by step"
                                .to_string(),
                        ),
                        ("context".to_string(), "exec session scratch".to_string()),
                    ]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("prose citing an undeclared destination schema must not refuse the write");

        // The schema declines the edge — that stays.
        let stored = engine.get_entity(&entry.id).unwrap();
        assert!(
            !stored.relationships.iter().any(|r| r.target == target.id),
            "default→planning declares no entry, so no edge is emitted: {:?}",
            stored.relationships
        );
        // But the drop is TOLD, typed, naming target and gap.
        assert!(
            entry.warnings.iter().any(|w| matches!(
                w,
                WarningHint::CrossSchemaLinkUndeclared { target, .. }
                    if target.as_ref() == "plans--target-plan-note"
            )),
            "the dropped body link must surface as a typed warning: {:?}",
            entry.warnings
        );
    }

    /// CROSS_MEM_EDGE_NOT_DECLARED; and the workspace policy gate
    /// still fires when the direction is not granted.
    #[test]
    fn ingest_wildcard_links_into_arbitrary_destination_schema() {
        use crate::engine::test_helpers::write_schema_files_with_default_type;
        use memstead_schema::workspace_config::CrossLinkValue;

        let tmp = TempDir::new().unwrap();
        let dest_dir = tmp.path().join("dest");
        let proc_dir = tmp.path().join("proc");
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::create_dir_all(&proc_dir).unwrap();

        // A user-written schema the engine has never shipped.
        let schemas_dir = tmp.path().join("schemas");
        let user_manifest = r#"name: debate
version: 0.1.0
description: a user-written destination schema
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        write_schema_files_with_default_type(&schemas_dir, "debate@0.1.0", user_manifest, &["doc"]);

        let mount = |mem: &str, dir: &std::path::Path, schema: &str, version: (u64, u64, u64)| {
            crate::workspace::Mount {
                mem: mem.to_string(),
                schema: Some(memstead_schema::SchemaRef::new(
                    schema,
                    semver::Version::new(version.0, version.1, version.2),
                )),
                storage: crate::workspace::MountStorage::Folder {
                    path: dir.to_path_buf(),
                },
                capability: crate::workspace::MountCapability::Write,
                lifecycle: crate::workspace::MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            }
        };
        let boot = |grant: bool| -> Engine {
            let mounts = vec![
                (
                    mount("dest", &dest_dir, "debate", (0, 1, 0)),
                    Box::new(FilesystemMemWriter::new(dest_dir.clone())) as Box<dyn MemBackend>,
                ),
                (
                    mount("proc", &proc_dir, "ingest", (0, 2, 0)),
                    Box::new(FilesystemMemWriter::new(proc_dir.clone())) as Box<dyn MemBackend>,
                ),
            ];
            let mut engine =
                Engine::from_mounts_with_schemas_dir(mounts, Some(schemas_dir.as_path()))
                    .expect("ingest + user schema boot");
            let mut settings = crate::workspace::WorkspaceSettings::default();
            if grant {
                settings.cross_mem_links.insert(
                    "proc".to_string(),
                    CrossLinkValue::List(vec!["dest".to_string()]),
                );
            }
            engine.set_settings(settings);
            engine
        };
        let (actor, client) = cli_actor();

        let mut engine = boot(true);
        // Destination entity in the user-schema mem.
        let target = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "dest".to_string(),
                    title: "Target Doc".to_string(),
                    entity_type: "doc".to_string(),
                    sections: IndexMap::from_iter([(
                        "body".to_string(),
                        "destination content".to_string(),
                    )]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Process-mem entry body-linking the destination entity.
        let entry = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "proc".to_string(),
                    title: "Check The Claim".to_string(),
                    entity_type: "verification_target".to_string(),
                    sections: IndexMap::from_iter([
                        (
                            "claim".to_string(),
                            "the claim under suspicion lives in [[dest--target-doc]]".to_string(),
                        ),
                        ("source_to_check".to_string(), "dest mem".to_string()),
                        (
                            "verifiable_when".to_string(),
                            "the linked entity still says so".to_string(),
                        ),
                    ]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("wildcard admits the alias link into the user-schema destination");
        let stored = engine.get_entity(&entry.id).unwrap();
        assert!(
            stored
                .relationships
                .iter()
                .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
            "alias REFERENCES edge must emit: {:?}",
            stored.relationships
        );

        // Explicit authoring of the alias rel-type: still forbidden.
        let err = engine
            .relate_entity(
                RelateEntityArgs {
                    source: entry.id.clone(),
                    expected_hash: None,
                    rel_type: "REFERENCES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "RELATION_MANUAL_AUTHORING_FORBIDDEN", "{err:?}");

        // Structural rel-type into the undeclared destination: the
        // historical refusal, wildcard notwithstanding.
        let err = engine
            .relate_entity(
                RelateEntityArgs {
                    source: entry.id.clone(),
                    expected_hash: None,
                    rel_type: "PART_OF".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "CROSS_MEM_EDGE_NOT_DECLARED", "{err:?}");

        // Load-path survival: a FRESH boot over the same folders (the
        // store-builder path that previously dropped undeclared
        // cross-mem edges) keeps the alias edge.
        drop(engine);
        let rebooted = boot(true);
        let reloaded = rebooted.get_entity(&entry.id).unwrap();
        assert!(
            reloaded
                .relationships
                .iter()
                .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
            "alias edge must survive reload: {:?}",
            reloaded.relationships
        );

        // Policy gate intact: without the grant, the same wildcarded
        // link refuses CROSS_MEM_LINK_NOT_ALLOWED.
        let mut denied = boot(false);
        let err = denied
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "proc".to_string(),
                    title: "Denied Entry".to_string(),
                    entity_type: "verification_target".to_string(),
                    sections: IndexMap::from_iter([
                        (
                            "claim".to_string(),
                            "points at [[dest--target-doc]]".to_string(),
                        ),
                        ("source_to_check".to_string(), "dest mem".to_string()),
                        ("verifiable_when".to_string(), "never".to_string()),
                    ]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "CROSS_MEM_LINK_NOT_ALLOWED", "{err:?}");
    }

    /// Two-mem Write-Write scaffold —
    /// `test` and `other` both pin the default schema, no
    /// `cross_mem_links` policy set yet (default deny-all). The
    /// caller installs the policy that matches each scenario.
    fn engine_with_two_default_mems() -> (TempDir, TempDir, Engine) {
        let tmp_test = TempDir::new().unwrap();
        let tmp_other = TempDir::new().unwrap();
        let test_dir = tmp_test.path().to_path_buf();
        let other_dir = tmp_other.path().to_path_buf();
        let writer_test = FilesystemMemWriter::new(test_dir.clone());
        let writer_other = FilesystemMemWriter::new(other_dir.clone());
        let engine = Engine::from_mounts(vec![
            (
                folder_mount("test", test_dir),
                Box::new(writer_test) as Box<dyn MemBackend>,
            ),
            (
                folder_mount("other", other_dir),
                Box::new(writer_other) as Box<dyn MemBackend>,
            ),
        ])
        .unwrap();
        (tmp_test, tmp_other, engine)
    }

    /// `memstead_create` with an inline cross-mem relation refuses
    /// with `CROSS_MEM_LINK_NOT_ALLOWED` when policy denies the
    /// direction. The entity does not persist; the would-be id reads
    /// as `NotFound`.
    #[test]
    fn create_entity_refuses_inline_cross_mem_relation_when_policy_denies() {
        use crate::entity::EntityId;
        use crate::ops::RelateArg;
        use memstead_schema::workspace_config::CrossLinkValue;

        let (_tmp_test, _tmp_other, mut engine) = engine_with_two_default_mems();
        let (actor, client) = cli_actor();

        // Policy: `test → other` granted only. The inline create
        // request below is `other → test`, which must refuse.
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_mem_links.insert(
            "test".to_string(),
            CrossLinkValue::List(vec!["other".to_string()]),
        );
        engine.set_settings(settings);

        // Seed a target in the `test` mem so the inline relation
        // names a real id (the policy gate fires before target
        // resolution regardless, but a real target removes any
        // ambiguity from the assertion).
        let target = engine
            .create_entity(
                empty_create_args("test", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let mut args = empty_create_args("other", "Source");
        args.relations = vec![RelateArg {
            rel_type: "IMPLEMENTS".to_string(),
            target: target.id.clone(),
            description: None,
        }];
        let err = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap_err();
        match err {
            EngineError::CrossMemLinkNotAllowed { from_mem, to_mem } => {
                assert_eq!(from_mem, "other");
                assert_eq!(to_mem, "test");
            }
            other => panic!("expected CROSS_MEM_LINK_NOT_ALLOWED, got {other:?}"),
        }

        // No entity landed: the would-be id is absent.
        let would_be = EntityId::new("other", "source");
        assert!(
            engine.get_entity(&would_be).is_none(),
            "entity must not persist when inline relation refuses"
        );
    }

    /// With the granted direction, the
    /// inline cross-mem relation succeeds and the edge persists.
    #[test]
    fn create_entity_allows_inline_cross_mem_relation_when_policy_grants() {
        use crate::ops::RelateArg;
        use memstead_schema::workspace_config::CrossLinkValue;

        let (_tmp_test, _tmp_other, mut engine) = engine_with_two_default_mems();
        let (actor, client) = cli_actor();

        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_mem_links.insert(
            "other".to_string(),
            CrossLinkValue::List(vec!["test".to_string()]),
        );
        engine.set_settings(settings);

        let target = engine
            .create_entity(
                empty_create_args("test", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let mut args = empty_create_args("other", "Source");
        args.relations = vec![RelateArg {
            rel_type: "IMPLEMENTS".to_string(),
            target: target.id.clone(),
            description: None,
        }];
        let outcome = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap();
        let stored = engine.get_entity(&outcome.id).expect("entity persists");
        assert!(
            stored
                .relationships
                .iter()
                .any(|r| r.rel_type == "IMPLEMENTS" && r.target == target.id),
            "IMPLEMENTS edge must persist on the source's relationships",
        );
    }

    /// A same-mem inline relation
    /// bypasses the policy gate entirely. Even with an empty policy
    /// (default deny-all for cross-mem), the create succeeds.
    #[test]
    fn create_entity_admits_same_mem_inline_relation_regardless_of_policy() {
        use crate::ops::RelateArg;

        let (_tmp_test, _tmp_other, mut engine) = engine_with_two_default_mems();
        let (actor, client) = cli_actor();
        // No cross_mem_links set; same-mem writes must still work.

        let target = engine
            .create_entity(
                empty_create_args("test", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let mut args = empty_create_args("test", "Source");
        args.relations = vec![RelateArg {
            rel_type: "USES".to_string(),
            target: target.id.clone(),
            description: None,
        }];
        let outcome = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap();
        let stored = engine.get_entity(&outcome.id).expect("entity persists");
        assert!(
            stored
                .relationships
                .iter()
                .any(|r| r.rel_type == "USES" && r.target == target.id),
            "same-mem USES edge must persist",
        );
    }

    /// The existing `memstead_relate` path
    /// refuses the same scenario with the same typed code and
    /// payload shape — the two surfaces' refusals are
    /// indistinguishable to an agent.
    #[test]
    fn relate_and_create_refuse_cross_mem_policy_with_identical_envelope() {
        use crate::ops::RelateArg;
        use memstead_schema::workspace_config::CrossLinkValue;

        let (_tmp_test, _tmp_other, mut engine) = engine_with_two_default_mems();
        let (actor, client) = cli_actor();

        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_mem_links.insert(
            "test".to_string(),
            CrossLinkValue::List(vec!["other".to_string()]),
        );
        engine.set_settings(settings);

        let target = engine
            .create_entity(
                empty_create_args("test", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let src = engine
            .create_entity(
                empty_create_args("other", "Source"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // memstead_relate refusal.
        let relate_err = engine
            .relate_entity(
                RelateEntityArgs {
                    source: src.id.clone(),
                    rel_type: "IMPLEMENTS".to_string(),
                    target: target.id.clone(),
                    expected_hash: Some(src.content_hash.clone()),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();

        // memstead_create.relations[] refusal — fresh title so the create
        // attempt hasn't already landed.
        let mut create_args = empty_create_args("other", "Source Two");
        create_args.relations = vec![RelateArg {
            rel_type: "IMPLEMENTS".to_string(),
            target: target.id.clone(),
            description: None,
        }];
        let create_err = engine
            .create_entity(create_args, actor, Some(&client), None)
            .unwrap_err();

        // Both refusals share the typed code, the payload shape, and
        // the (from_mem, to_mem) values.
        match (relate_err, create_err) {
            (
                EngineError::CrossMemLinkNotAllowed {
                    from_mem: rfv,
                    to_mem: rtv,
                },
                EngineError::CrossMemLinkNotAllowed {
                    from_mem: cfv,
                    to_mem: ctv,
                },
            ) => {
                assert_eq!(rfv, "other");
                assert_eq!(rtv, "test");
                assert_eq!(cfv, "other");
                assert_eq!(ctv, "test");
            }
            (a, b) => panic!(
                "expected matching CROSS_MEM_LINK_NOT_ALLOWED on both surfaces; got relate={a:?}, create={b:?}"
            ),
        }
    }

    /// Body wiki-link `[[other--target]]` in mem `test` (with
    /// `test → other` granted) creates the entity, auto-stubs at
    /// `other--target` (NOT `test--other--target` — that was the
    /// pre-fix phantom-stub bug), and emits one REFERENCES edge via
    /// the alias-synthesis path.
    #[test]
    fn create_entity_body_link_cross_mem_dash_form_routes_correctly() {
        use crate::entity::EntityId;
        use indexmap::IndexMap;
        use memstead_schema::workspace_config::CrossLinkValue;

        let (_tmp_test, _tmp_other, mut engine) = engine_with_two_default_mems();
        let (actor, client) = cli_actor();

        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_mem_links.insert(
            "test".to_string(),
            CrossLinkValue::List(vec!["other".to_string()]),
        );
        engine.set_settings(settings);

        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert(
            "identity".to_string(),
            "see [[other--target]] for details".to_string(),
        );
        sections.insert("purpose".to_string(), "source purpose".to_string());
        let outcome = engine
            .create_entity(
                crate::engine::CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "test".to_string(),
                    title: "Source".to_string(),
                    entity_type: "spec".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Auto-stub landed at `other--target`, NOT `test--other--target`.
        let canonical = EntityId::new("other", "target");
        assert!(
            engine.get_entity(&canonical).is_some(),
            "auto-stub must land at the canonical cross-mem id"
        );
        let phantom = EntityId::new("test", "other--target");
        assert!(
            engine.get_entity(&phantom).is_none(),
            "no double-prefixed phantom stub"
        );

        // Exactly one REFERENCES edge to the cross-mem target.
        let source = engine.get_entity(&outcome.id).unwrap();
        let references_count = source
            .relationships
            .iter()
            .filter(|r| r.rel_type == "REFERENCES" && r.target == canonical)
            .count();
        assert_eq!(
            references_count, 1,
            "alias-synthesis must emit exactly one REFERENCES edge per cross-mem body link",
        );
    }

    /// Complement: body wiki-link cross-mem refusal when policy
    /// denies the direction. The auto-stub never lands, the entity
    /// never persists.
    #[test]
    fn create_entity_body_link_cross_mem_refused_when_policy_denies() {
        use indexmap::IndexMap;

        let (_tmp_test, _tmp_other, mut engine) = engine_with_two_default_mems();
        let (actor, client) = cli_actor();
        // Empty cross-link policy — `test → other` denied.

        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert(
            "identity".to_string(),
            "see [[other--target]] for details".to_string(),
        );
        sections.insert("purpose".to_string(), "source purpose".to_string());
        let err = engine
            .create_entity(
                crate::engine::CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "test".to_string(),
                    title: "Source".to_string(),
                    entity_type: "spec".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::CrossMemLinkNotAllowed { from_mem, to_mem } => {
                assert_eq!(from_mem, "test");
                assert_eq!(to_mem, "other");
            }
            other => panic!("expected CROSS_MEM_LINK_NOT_ALLOWED, got {other:?}"),
        }
    }

    /// `[mutations].require_notes = true` drives a single `NOTE_MISSING`
    /// warning out of the engine mutation pipeline on every noteless
    /// mutation — the single enforcement point both the CLI and the MCP
    /// transport inherit. The mutation still commits (the policy nudges,
    /// it never blocks). Supplying a note suppresses it; turning the
    /// policy off silences it entirely. Covers create / update / relate
    /// in one engine instance.
    #[test]
    fn require_notes_drives_single_note_missing_warning_per_noteless_mutation() {
        use crate::engine::UpdateEntityArgs;
        use crate::workspace::{MutationsSection, WorkspaceSettings};
        use indexmap::IndexMap;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        engine.set_workspace_root(mem_dir.clone());
        engine.set_settings(WorkspaceSettings {
            mutations: MutationsSection {
                require_notes: Some(true),
            },
            ..Default::default()
        });
        let (actor, client) = cli_actor();

        let note_missing = |ws: &[WarningHint]| -> usize {
            ws.iter()
                .filter(|w| matches!(w, WarningHint::NoteMissing { tool: _ }))
                .count()
        };

        // --- create, no note: exactly one NOTE_MISSING, commit landed ---
        let created = engine
            .create_entity(
                empty_create_args("specs", "Noteless"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(
            note_missing(&created.warnings),
            1,
            "create under require_notes must emit exactly one NOTE_MISSING; got {:?}",
            created.warnings,
        );
        assert!(
            matches!(
                created.warnings.iter().find(|w| matches!(w, WarningHint::NoteMissing { .. })),
                Some(WarningHint::NoteMissing { tool }) if tool == "create_entity"
            ),
            "the warning names the engine-level verb",
        );
        assert!(
            !created.write_id.is_empty(),
            "create still commits (nudge, not block)"
        );

        // --- update, no note: NOTE_MISSING + commit landed ---
        let mut edit: IndexMap<String, String> = IndexMap::new();
        edit.insert("identity".to_string(), "revised".to_string());
        let updated = engine
            .update_entity(
                UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: created.id.clone(),
                    expected_hash: Some(created.content_hash.clone()),
                    sections: edit,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    sections_unset: Vec::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                    anchors_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(
            note_missing(&updated.warnings),
            1,
            "update emits NOTE_MISSING"
        );
        assert!(!updated.write_id.is_empty(), "update still commits");

        // --- relate, no note: NOTE_MISSING + commit landed ---
        let target = engine
            .create_entity(
                empty_create_args("specs", "Target"),
                actor,
                Some(&client),
                Some("seed"),
            )
            .unwrap();
        let related = engine
            .relate_entity(
                RelateEntityArgs {
                    source: updated.id.clone(),
                    expected_hash: Some(updated.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(
            note_missing(&related.warnings),
            1,
            "relate emits NOTE_MISSING"
        );
        assert!(!related.write_id.is_empty(), "relate still commits");

        // --- with a note: suppressed ---
        let with_note = engine
            .create_entity(
                empty_create_args("specs", "Documented"),
                actor,
                Some(&client),
                Some("a real provenance note"),
            )
            .unwrap();
        assert_eq!(
            note_missing(&with_note.warnings),
            0,
            "a supplied note suppresses the warning",
        );

        // --- policy off: silent even without a note ---
        engine.set_settings(WorkspaceSettings::default());
        let after_off = engine
            .create_entity(
                empty_create_args("specs", "Quiet"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(
            note_missing(&after_off.warnings),
            0,
            "no NOTE_MISSING when require_notes is unset",
        );
    }

    // ---- E3a anchors: create/persist/reload/isolation ------------------

    fn file_anchor(artifact: &str, hash: &str) -> crate::anchor::AnchorInput {
        crate::anchor::AnchorInput {
            artifact: Some(artifact.to_string()),
            grain: Some("file".to_string()),
            class: Some("anchored".to_string()),
            hash: Some(hash.to_string()),
            hash_stability: Some("stable".to_string()),
            ..Default::default()
        }
    }

    fn folder_engine(mem: &str) -> (Engine, TempDir) {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount(mem, dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        (engine, tmp)
    }

    #[test]
    fn create_with_anchors_persists_and_survives_reload() {
        let (mut engine, tmp) = folder_engine("specs");
        let dir = tmp.path().to_path_buf();
        let (actor, client) = cli_actor();
        let mut args = empty_create_args("specs", "Anchored Entity");
        args.anchors = vec![file_anchor("src/lib.rs", "h1")];
        engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap();

        let id = crate::EntityId::new("specs", "anchored-entity");
        let anchors = engine.entity_anchors(&id);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].artifact, "src/lib.rs");
        assert_eq!(
            anchors[0].class,
            crate::anchor::AnchorProvenanceClass::Anchored
        );

        // Survives a fresh boot from the same on-disk mem.
        let writer = FilesystemMemWriter::new(dir.clone());
        let reloaded = Engine::from_mounts(vec![(
            folder_mount("specs", dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        assert_eq!(reloaded.entity_anchors(&id).len(), 1);
        // Reverse lookup finds it by artifact path.
        assert_eq!(reloaded.anchors_referencing_artifact("src/lib.rs").len(), 1);
    }

    #[test]
    fn malformed_anchor_refuses_and_entity_not_written() {
        let (mut engine, tmp) = folder_engine("specs");
        let (actor, client) = cli_actor();
        let mut args = empty_create_args("specs", "Bad Anchor");
        args.anchors = vec![crate::anchor::AnchorInput {
            artifact: Some("x".into()),
            grain: Some("paragraph".into()), // unknown grain
            class: Some("anchored".into()),
            ..Default::default()
        }];
        let err = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap_err();
        assert_eq!(err.code(), crate::anchor::INVALID_ANCHOR_CODE);
        // Entity was not written (refusal fires before the disk write).
        assert!(
            engine
                .get_entity(&crate::EntityId::new("specs", "bad-anchor"))
                .is_none()
        );
        assert!(!tmp.path().join("bad-anchor.md").exists());
    }

    #[test]
    fn anchors_are_not_folded_into_content_hash() {
        // Two identical creates — one anchored, one not — produce the same
        // `_hash`: the anchors sidecar lives under `.memstead/` and never
        // enters content hashing.
        //
        // Both engines run on ONE frozen clock. The schema auto-stamps
        // `created_date` / `last_modified` at second granularity, so without
        // this the assertion also silently depended on both creates landing
        // inside the same second — true on an idle machine, false under a
        // loaded one, where the two entities differ in frontmatter and the
        // hashes diverge for a reason that has nothing to do with anchors.
        let (mut anchored, _t1) = folder_engine("specs");
        let (mut plain, _t2) = folder_engine("specs");
        let frozen = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_754_000_000);
        anchored.set_mutation_clock(std::sync::Arc::new(move || frozen));
        plain.set_mutation_clock(std::sync::Arc::new(move || frozen));
        let (actor, client) = cli_actor();

        let mut a = empty_create_args("specs", "Same Title");
        a.anchors = vec![file_anchor("src/lib.rs", "h1")];
        let with = anchored
            .create_entity(a, actor, Some(&client), None)
            .unwrap();

        let p = empty_create_args("specs", "Same Title");
        let without = plain.create_entity(p, actor, Some(&client), None).unwrap();

        assert_eq!(
            with.content_hash, without.content_hash,
            "anchors must not change the entity content hash"
        );
    }

    #[test]
    fn anchorless_create_writes_no_sidecar() {
        let (mut engine, _tmp) = folder_engine("specs");
        let (actor, client) = cli_actor();
        engine
            .create_entity(
                empty_create_args("specs", "No Anchors"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(
            engine
                .entity_anchors(&crate::EntityId::new("specs", "no-anchors"))
                .is_empty()
        );
    }

    // ---- reserved metadata keys on create --------------------------------

    /// A create carrying a reserved identity/discriminator metadata key
    /// (`type` / `mem` / `id`) refuses with the same deliberate
    /// `READ_ONLY_FIELD` the update path uses — not the incidental
    /// `UNKNOWN_METADATA_FIELD` — and the entity is not written.
    /// Refusal complement: a create with only declared, non-reserved
    /// keys lands exactly as today (covered pervasively by every other
    /// create test; the explicit control below re-asserts it beside
    /// the refusals).
    #[test]
    fn create_refuses_reserved_metadata_keys_deliberately() {
        let (mut engine, _tmp) = folder_engine("specs");
        let (actor, client) = cli_actor();
        for reserved in ["type", "mem", "id"] {
            let mut args = empty_create_args("specs", "Smuggler");
            args.metadata
                .insert(reserved.to_string(), "bogus".to_string());
            let err = engine
                .create_entity(args, actor, Some(&client), None)
                .expect_err("reserved key must refuse on create");
            assert_eq!(err.code(), "READ_ONLY_FIELD", "key '{reserved}': {err:?}");
            assert!(
                engine
                    .get_entity(&crate::EntityId::new("specs", "smuggler"))
                    .is_none(),
                "entity must not be written after the '{reserved}' refusal"
            );
        }
        // Control: the same create without the smuggled key lands.
        engine
            .create_entity(
                empty_create_args("specs", "Smuggler"),
                actor,
                Some(&client),
                None,
            )
            .expect("a clean create is untouched by the reserved-key gate");
    }

    // ---- cycle family on the create paths --------------------------------

    fn create_with_relation(mem: &str, title: &str, rel_type: &str, to: &str) -> CreateEntityArgs {
        let mut args = empty_create_args(mem, title);
        args.relations = vec![crate::ops::RelateArg {
            target: crate::EntityId(to.to_string()),
            rel_type: rel_type.to_string(),
            description: None,
        }];
        args
    }

    /// `create.relations[]` runs the same cycle family as
    /// `memstead_relate`: an edge closing a cycle through a promoted
    /// stub refuses `RELATIONSHIP_CYCLE` (acyclic rel-type), a
    /// self-loop on a listed no-self-loop rel-type refuses
    /// identically, and —
    /// refusal complement — a non-cycle edge on the acyclic type lands
    /// exactly as today.
    #[test]
    fn create_relations_refuse_cycle_and_self_loop_like_relate() {
        let (mut engine, _tmp) = folder_engine("specs");
        let (actor, client) = cli_actor();

        // A PART_OF→ghost auto-stubs `ghost` with an incoming edge.
        engine
            .create_entity(
                create_with_relation("specs", "Alpha", "PART_OF", "specs--ghost"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Promoting the stub with a back-edge closes alpha→ghost→alpha.
        let err = engine
            .create_entity(
                create_with_relation("specs", "Ghost", "PART_OF", "specs--alpha"),
                actor,
                Some(&client),
                None,
            )
            .expect_err("cycle-closing create.relations[] must refuse");
        assert_eq!(err.code(), "RELATIONSHIP_CYCLE", "{err:?}");
        // Recovery detail matches the relate path's shape.
        let details = err.details();
        assert_eq!(details["rel_type"], "PART_OF");
        assert!(details["existing_path"].is_array());
        assert!(
            engine
                .get_entity(&crate::EntityId::new("specs", "ghost"))
                .is_none_or(|e| e.stub),
            "the refused entity must not be written"
        );

        // Self-loop on a listed no-self-loop rel-type (spec lists USES).
        let err = engine
            .create_entity(
                create_with_relation("specs", "Selfy", "USES", "specs--selfy"),
                actor,
                Some(&client),
                None,
            )
            .expect_err("self-loop create.relations[] must refuse");
        assert_eq!(err.code(), "RELATIONSHIP_CYCLE", "{err:?}");

        // Refusal complement: a non-cycle edge on the acyclic type
        // lands (fresh chain link, no back-path).
        engine
            .create_entity(
                create_with_relation("specs", "Beta", "PART_OF", "specs--alpha"),
                actor,
                Some(&client),
                None,
            )
            .expect("a non-cycle PART_OF edge must land as today");
    }

    /// An intra-batch cycle on an acyclic rel-type refuses the whole
    /// batch — the staged state IS the graph state the batch validates
    /// against. Refusal complement: an acyclic intra-batch chain lands.
    #[test]
    fn batch_create_refuses_intra_batch_cycle() {
        let (mut engine, _tmp) = folder_engine("specs");
        let (actor, client) = cli_actor();

        let result = engine
            .batch_create(
                vec![
                    (
                        create_with_relation("specs", "Ping", "PART_OF", "specs--pong"),
                        None,
                    ),
                    (
                        create_with_relation("specs", "Pong", "PART_OF", "specs--ping"),
                        None,
                    ),
                ],
                actor,
                Some(&client),
                false,
            )
            .expect("batch returns a result envelope");
        assert!(!result.applied, "intra-batch cycle must refuse the batch");
        assert!(
            result.results.iter().any(|r| r
                .error
                .as_ref()
                .is_some_and(|e| e.code == "RELATIONSHIP_CYCLE")),
            "the refusal must carry RELATIONSHIP_CYCLE: {:?}",
            result.results
        );
        assert!(
            engine
                .get_entity(&crate::EntityId::new("specs", "ping"))
                .is_none(),
            "nothing lands from a refused batch"
        );

        // Refusal complement: an acyclic intra-batch chain lands.
        let result = engine
            .batch_create(
                vec![
                    (
                        create_with_relation("specs", "Chain One", "PART_OF", "specs--chain-two"),
                        None,
                    ),
                    (empty_create_args("specs", "Chain Two"), None),
                ],
                actor,
                Some(&client),
                false,
            )
            .expect("acyclic batch lands");
        assert!(result.applied, "{:?}", result.results);
        assert_eq!(result.succeeded, 2);
    }

    /// Refusal complement at depth: a deep-but-acyclic PART_OF chain
    /// past the cycle path cap is accepted on the create path — the cap
    /// bounds the *reported* path on refusal, never the legality of a
    /// long acyclic chain — and one closing edge at the far end still
    /// refuses.
    #[test]
    fn deep_acyclic_chain_near_path_cap_is_accepted() {
        let (mut engine, _tmp) = folder_engine("specs");
        let (actor, client) = cli_actor();
        let depth = crate::engine::mutation::RELATIONSHIP_CYCLE_PATH_CAP + 2;

        // link-0 ← link-1 ← … each new entity PART_OF the previous.
        engine
            .create_entity(
                empty_create_args("specs", "Link 0"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        for i in 1..depth {
            engine
                .create_entity(
                    create_with_relation(
                        "specs",
                        &format!("Link {i}"),
                        "PART_OF",
                        &format!("specs--link-{}", i - 1),
                    ),
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap_or_else(|e| panic!("deep acyclic link {i} must land: {e:?}"));
        }

        // Closing the loop end-to-end still refuses, with the reported
        // path truncated at the cap.
        let last = depth - 1;
        let err = engine
            .update_entity(
                {
                    let id = crate::EntityId::new("specs", "link-0");
                    let hash = engine.get_entity(&id).unwrap().content_hash.clone();
                    crate::engine::UpdateEntityArgs {
                        anchors: Vec::new(),
                        anchors_unset: Vec::new(),
                        id,
                        expected_hash: Some(hash),
                        sections: IndexMap::new(),
                        append_sections: IndexMap::new(),
                        patch_sections: IndexMap::new(),
                        sections_unset: Vec::new(),
                        metadata: IndexMap::new(),
                        metadata_unset: Vec::new(),
                        declare_relations: vec![crate::ops::RelateArg {
                            target: crate::EntityId::new("specs", &format!("link-{last}")),
                            rel_type: "PART_OF".to_string(),
                            description: None,
                        }],
                        dry_run: false,
                        relations_unset: Vec::new(),
                    }
                },
                actor,
                Some(&client),
                None,
            )
            .expect_err("closing the deep chain must refuse");
        assert_eq!(err.code(), "RELATIONSHIP_CYCLE");
        let details = err.details();
        assert_eq!(details["path_truncated"], true);
        assert_eq!(
            details["existing_path"].as_array().unwrap().len(),
            crate::engine::mutation::RELATIONSHIP_CYCLE_PATH_CAP
        );
    }

    const FORMAT_MANIFEST: &str = r#"name: formatproof
version: 0.1.0
description: section-format proof schema
when_to_use: format tests
types:
  - plan
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;

    const FORMAT_PLAN_TYPE: &str = r#"name: plan
description: a plan with formatted milestones
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
  - key: meilensteine
    heading: Meilensteine
    required: false
    search_weight: 5.0
    catch_all: false
    write_rules: []
    content: "(heading(3) list(bullet))+"
    item_pattern: '\*\*(?<name>[^*]+)\*\* — (?<datum>\d{4}-\d{2}-\d{2})'
    example: |
      ### Phase 1
      - **Kickoff** — 2026-09-01
  - key: notizen
    heading: Notizen
    required: false
    search_weight: 5.0
    catch_all: false
    write_rules: []
    content: "list(bullet)"
    format_severity: warn
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
  - meilensteine
  - notizen
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;

    fn format_engine(tmp: &TempDir) -> Engine {
        engine_with_proof_schema(
            tmp,
            "formatproof",
            FORMAT_MANIFEST,
            &[("plan", FORMAT_PLAN_TYPE)],
        )
    }

    fn plan_create_args(
        title: &str,
        meilensteine: Option<&str>,
        notizen: Option<&str>,
    ) -> CreateEntityArgs {
        let mut sections = IndexMap::new();
        sections.insert("body".to_string(), "a plan body.".to_string());
        if let Some(m) = meilensteine {
            sections.insert("meilensteine".to_string(), m.to_string());
        }
        if let Some(n) = notizen {
            sections.insert("notizen".to_string(), n.to_string());
        }
        CreateEntityArgs {
            anchors: Vec::new(),
            mem: "proof".to_string(),
            title: title.to_string(),
            entity_type: "plan".to_string(),
            sections,
            metadata: IndexMap::new(),
            relations: vec![],
            dry_run: false,
        }
    }

    /// Block-tier format enforcement on create: a nonconforming
    /// section refuses with the format code and the echoed example;
    /// the conforming write passes; a warn-tier section never refuses.
    #[test]
    fn create_enforces_declared_section_format() {
        let tmp = TempDir::new().unwrap();
        let mut engine = format_engine(&tmp);
        let (actor, client) = cli_actor();

        let err = engine
            .create_entity(
                plan_create_args("Plan A", Some("### Phase 1\n\nprose statt liste\n"), None),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "SECTION_CONTENT_MISMATCH");
        let details = err.details();
        assert_eq!(details["section"], "meilensteine");
        assert!(
            details["example"].as_str().unwrap().contains("Kickoff"),
            "the conforming example is echoed: {details}"
        );
        assert_eq!(details["expected_next"][0], "list(bullet)");

        // Item-pattern violation gets its own code.
        let err = engine
            .create_entity(
                plan_create_args("Plan B", Some("### Phase 1\n- kein format\n"), None),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "SECTION_ITEM_PATTERN_MISMATCH");

        // Conforming write passes.
        engine
            .create_entity(
                plan_create_args(
                    "Plan C",
                    Some("### Phase 1\n- **Kickoff** — 2026-09-01\n"),
                    None,
                ),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Warn-tier section: nonconforming content commits.
        let outcome = engine
            .create_entity(
                plan_create_args(
                    "Plan D",
                    Some("### Phase 1\n- **Kickoff** — 2026-09-01\n"),
                    Some("kein listenpunkt\n"),
                ),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(!outcome.write_id.is_empty(), "warn tier never refuses");

        // Absent-as-empty: omitting the block-tier section refuses
        // exactly like an explicit empty body — the generator renders
        // the empty heading either way, and write path and health
        // must agree about that on-disk state. `+` does not admit the
        // empty sequence, so the section is effectively required.
        let err = engine
            .create_entity(
                plan_create_args("Plan E", None, None),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "SECTION_CONTENT_MISMATCH");
    }

    /// Composed-body rule on update: an append whose delta is
    /// harmless refuses when the COMPOSED body violates; the
    /// conforming replacement passes.
    #[test]
    fn update_judges_format_on_composed_body() {
        let tmp = TempDir::new().unwrap();
        let mut engine = format_engine(&tmp);
        let (actor, client) = cli_actor();
        let created = engine
            .create_entity(
                plan_create_args(
                    "Plan A",
                    Some("### Phase 1\n- **Kickoff** — 2026-09-01\n"),
                    None,
                ),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Append a trailing paragraph: the delta alone is legal
        // markdown, the composed body no longer matches the shape.
        let current = engine.get_entity(&created.id).unwrap().content_hash.clone();
        let mut append = IndexMap::new();
        append.insert(
            "meilensteine".to_string(),
            "\n\nnachtrag als absatz\n".to_string(),
        );
        let err = engine
            .update_entity(
                crate::engine::UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: created.id.clone(),
                    expected_hash: Some(current.clone()),
                    sections: IndexMap::new(),
                    append_sections: append,
                    patch_sections: IndexMap::new(),
                    sections_unset: Vec::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: vec![],
                    dry_run: false,
                    relations_unset: Vec::new(),
                    anchors_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "SECTION_CONTENT_MISMATCH");

        // A conforming append (another phase) passes.
        let mut append = IndexMap::new();
        append.insert(
            "meilensteine".to_string(),
            "\n\n### Phase 2\n- **Go-Live** — 2026-10-01\n".to_string(),
        );
        engine
            .update_entity(
                crate::engine::UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: created.id.clone(),
                    expected_hash: Some(current),
                    sections: IndexMap::new(),
                    append_sections: append,
                    patch_sections: IndexMap::new(),
                    sections_unset: Vec::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: vec![],
                    dry_run: false,
                    relations_unset: Vec::new(),
                    anchors_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
    }

    /// Reserved-heading extension (criterion 4): `^# ` now refuses in
    /// any section body, exactly like `^## ` — free-form sections
    /// included, via the byte-class line guard.
    #[test]
    fn embedded_h1_refuses_in_any_section() {
        let tmp = TempDir::new().unwrap();
        let mut engine = format_engine(&tmp);
        let (actor, client) = cli_actor();
        let mut args = plan_create_args(
            "Plan H",
            Some("### Phase 1\n- **Kickoff** — 2026-09-01\n"),
            None,
        );
        args.sections.insert(
            "body".to_string(),
            "intro\n# Injected Title\ntail".to_string(),
        );
        let err = engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap_err();
        assert_eq!(err.code(), "SECTION_CONTENT_INVALID");
    }
}
