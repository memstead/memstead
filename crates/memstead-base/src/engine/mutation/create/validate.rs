//! Validate phase of a create: the pre-write gates, in the order the
//! wire contract fixes them — section keys, reserved metadata keys,
//! anchors, section content, identity (slug, id, duplicates),
//! metadata values and engine-managed timestamps, required sections
//! and fields, then every inline relation through the gates
//! `memstead_relate` runs. Refusals leave nothing behind; the
//! warnings the gates raise ride the accumulator this phase opens.

use std::collections::BTreeMap;
use std::sync::Arc;

use indexmap::IndexMap;

use memstead_schema::TypeDefinition;

use crate::entity::id::validate_and_derive_slug;
use crate::entity::{EntityId, MetadataValue, normalise_description};
use crate::ops::WarningHint;
use crate::runtime_validator::{
    missing_required_fields, missing_required_sections, parse_metadata_value,
    validate_section_content, validate_section_keys,
};

use super::super::{EdgeRouteOutcome, route_edge_validation, validate_relation_target_grammar};
use super::resolve::ResolvedCreate;
use super::{CreateEntityArgs, Engine, EngineError};

/// A create past every input gate: the identity, the parsed
/// metadata, the validated anchors, and the warnings and guidance
/// gathered so far, ready for the compose phase to synthesise and
/// render the entity.
pub(super) struct ValidatedCreate {
    pub(super) args: CreateEntityArgs,
    pub(super) mount_idx: usize,
    pub(super) type_def: Arc<TypeDefinition>,
    pub(super) id: EntityId,
    pub(super) file_path: String,
    pub(super) metadata: IndexMap<String, MetadataValue>,
    pub(super) anchors: Vec<crate::anchor::Anchor>,
    pub(super) warnings: Vec<WarningHint>,
    pub(super) type_guidance: BTreeMap<String, Vec<String>>,
}

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

impl Engine {
    /// Run the input gates over a resolved create. `batch_skeleton_ids`
    /// and `drift_warnings` are what [`Engine::prepare_create`]
    /// documents.
    pub(super) fn validate_create(
        &self,
        resolved: ResolvedCreate,
        batch_skeleton_ids: Option<&std::collections::HashSet<EntityId>>,
        mut drift_warnings: Vec<WarningHint>,
    ) -> Result<ValidatedCreate, EngineError> {
        let ResolvedCreate {
            args,
            mount_idx,
            schema,
            type_def,
            mut title_trimmed_warning,
        } = resolved;

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
            super::super::validate_cross_mem_add_policy(self, &args.mem, &rel.target)?;
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
                None => super::super::peek_deferred_target_type(self, &rel.target)?,
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
            super::super::validate_description_posture(
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
            super::super::validate_manual_authoring_posture(
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
            super::super::validate_edge_acyclicity(
                &self.store,
                schema.as_ref(),
                &id,
                args.entity_type.as_str(),
                &rel.target,
                &canonical,
            )?;
        }

        Ok(ValidatedCreate {
            args,
            mount_idx,
            type_def,
            id,
            file_path,
            metadata,
            anchors: validated_anchors,
            warnings,
            type_guidance,
        })
    }
}
