//! Validate phase of an update: the payload gates, in the order the
//! wire contract fixes them — the empty-mutation guard, the anchors
//! payload, the schema and the type, section-mode conflicts, the
//! required-section and updatable-section gates, section keys and
//! content, metadata key gates, the set/unset overlap, and the
//! repair-power gate. Refusals leave nothing behind.

use std::collections::HashSet;
use std::sync::Arc;

use memstead_schema::{Schema, TypeDefinition};

use crate::entity::{Entity, EntityId};
use crate::runtime_validator::{
    validate_section_content, validate_section_keys, validate_unsettable_metadata_key,
    validate_updatable_section, validate_writable_metadata_key,
};

use super::super::unknown_type_error;
use super::resolve::ResolvedUpdate;
use super::{Engine, EngineError, UpdateEntityArgs};

/// An update past every payload gate: the schema and the type the
/// compose phase renders against, and the validated anchors payload.
pub(super) struct ValidatedUpdate {
    pub(super) args: UpdateEntityArgs,
    pub(super) mount_idx: usize,
    pub(super) mem: String,
    pub(super) entity: Entity,
    pub(super) prev_body_targets: HashSet<EntityId>,
    pub(super) schema: Arc<Schema>,
    pub(super) type_def: Arc<TypeDefinition>,
    pub(super) anchors: Vec<crate::anchor::Anchor>,
    pub(super) anchor_unsets: Vec<crate::anchor::AnchorUnset>,
}

impl Engine {
    /// Run the payload gates over a resolved update.
    pub(super) fn validate_update(
        &self,
        resolved: ResolvedUpdate,
    ) -> Result<ValidatedUpdate, EngineError> {
        let ResolvedUpdate {
            args,
            mount_idx,
            mem,
            entity,
            prev_body_targets,
        } = resolved;
        let id = &args.id;

        // Empty-mutation guard. After existence/stub/hash
        // gates so the more-specific errors fire first. A payload
        // with no recognised mutation content refuses BEFORE any
        // mutation work runs so a misspelled or omitted mutation
        // key (which deserialised to empty defaults under the
        // lenient pre-fix posture) doesn't silently land as
        // `succeeded: N, action: "updated", write_id: ""`.
        // Distinct from `UPDATE_NOOP` (a warning that fires when
        // mutation content was provided but matched the current
        // entity state) — the two are different states and ship
        // different envelopes.
        if args.sections.is_empty()
            && args.append_sections.is_empty()
            && args.patch_sections.is_empty()
            && args.sections_unset.is_empty()
            && args.metadata.is_empty()
            && args.metadata_unset.is_empty()
            && args.declare_relations.is_empty()
            && args.relations_unset.is_empty()
            && args.anchors.is_empty()
            && args.anchors_unset.is_empty()
        {
            return Err(EngineError::EmptyUpdate { id: id.to_string() });
        }

        // Validate any `anchors[]` / `anchors_unset[]` payload up front —
        // a malformed element refuses the whole update with a typed
        // `INVALID_ANCHOR` envelope before any disk write, on every path
        // (real / dry-run / no-op). Empty payloads → empty vecs (no
        // sidecar write; byte-identical to a pre-anchor update).
        let validated_anchors = self.validate_anchor_inputs(&mem, &args.anchors)?;
        let validated_anchor_unsets = Self::validate_anchor_unsets(&args.anchors_unset)?;

        let schema = self
            .schemas
            .get(&mem)
            .expect("schema present for every registered mount")
            .clone();
        let type_def = schema
            .get_type(&entity.entity_type)
            .ok_or_else(|| unknown_type_error(schema.as_ref(), &entity.entity_type))?;

        // Mode-conflict: the same section key may not appear in
        // more than one of `sections`, `append_sections`,
        // `patch_sections`. Mirrors full's
        // `EngineError::ConflictingSectionModes`. Three-way check:
        // build the conflict list per key and reject when ≥2 modes
        // claim it.
        for key in args.sections.keys() {
            let mut modes = vec!["sections".to_string()];
            if args.append_sections.contains_key(key) {
                modes.push("append_sections".to_string());
            }
            if args.patch_sections.contains_key(key) {
                modes.push("patch_sections".to_string());
            }
            if modes.len() > 1 {
                return Err(EngineError::ConflictingSectionModes {
                    section: key.clone(),
                    modes,
                });
            }
        }
        for key in args.append_sections.keys() {
            if args.patch_sections.contains_key(key) {
                return Err(EngineError::ConflictingSectionModes {
                    section: key.clone(),
                    modes: vec!["append_sections".to_string(), "patch_sections".to_string()],
                });
            }
        }
        // `sections_unset` is a fourth mode: a key both written and
        // removed in one call is a contradiction, not a sequence.
        for key in &args.sections_unset {
            let mut modes = vec!["sections_unset".to_string()];
            if args.sections.contains_key(key) {
                modes.push("sections".to_string());
            }
            if args.append_sections.contains_key(key) {
                modes.push("append_sections".to_string());
            }
            if args.patch_sections.contains_key(key) {
                modes.push("patch_sections".to_string());
            }
            if modes.len() > 1 {
                return Err(EngineError::ConflictingSectionModes {
                    section: key.clone(),
                    modes,
                });
            }
        }
        // Removing a REQUIRED section refuses with the conformance
        // vocabulary: the right repair for a required-but-empty heading
        // is filling it, never removing it. One refusal names every
        // offending key.
        let unset_required: Vec<crate::runtime_validator::MissingRequiredSection> = type_def
            .required_sections()
            .filter(|sec| args.sections_unset.contains(&sec.key))
            .map(|sec| crate::runtime_validator::MissingRequiredSection {
                entity_type: type_def.name.clone(),
                key: sec.key.clone(),
                heading: sec.heading.clone(),
                write_rules: sec.write_rules.clone(),
            })
            .collect();
        if !unset_required.is_empty() {
            let mut type_guidance: std::collections::BTreeMap<String, Vec<String>> =
                std::collections::BTreeMap::new();
            type_guidance.insert(type_def.name.clone(), type_def.write_rules.clone());
            return Err(EngineError::MissingRequiredSection {
                entity_type: type_def.name.clone(),
                missing_count: unset_required.len(),
                sections: unset_required,
                type_guidance,
                pre_announced_missing_fields: Vec::new(),
            });
        }
        // An absent key is a silent no-op (symmetric with
        // `metadata_unset`), so the updatable-section gate applies only
        // to keys the entity actually carries — otherwise unsetting a
        // key the schema never declared would refuse instead of
        // no-opping.
        for key in &args.sections_unset {
            if entity.sections.contains_key(key) || key == "relationships" {
                validate_updatable_section(key.as_str(), type_def.as_ref())?;
            }
        }

        validate_section_keys(
            args.sections
                .keys()
                .chain(args.append_sections.keys())
                .chain(args.patch_sections.keys())
                .map(String::as_str),
            type_def.as_ref(),
        )?;
        let mut heading_buf: Vec<&str> = Vec::new();
        #[allow(unused_assignments)]
        let mut catch_all = None;
        // Refuse embedded `^## ` in section content on every update path
        // that writes section bodies: `sections` (replace) and
        // `append_sections` (append). `patch_sections` replaces a
        // substring — its `new` text feeds into the eventual section
        // body so it gets the same gate.
        validate_section_content(
            args.sections
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .chain(
                    args.append_sections
                        .iter()
                        .map(|(k, v)| (k.as_str(), v.as_str())),
                )
                .chain(
                    args.patch_sections
                        .iter()
                        .flat_map(|(k, ps)| ps.iter().map(move |p| (k.as_str(), p.new.as_str()))),
                ),
            {
                let t: &memstead_schema::TypeDefinition = type_def.as_ref();
                catch_all = crate::runtime_validator::catch_all_context(t, &mut heading_buf);
                catch_all
            },
        )?;
        for key in args.sections.keys() {
            validate_updatable_section(key.as_str(), type_def.as_ref())?;
        }
        for key in args.append_sections.keys() {
            validate_updatable_section(key.as_str(), type_def.as_ref())?;
        }
        for key in args.patch_sections.keys() {
            validate_updatable_section(key.as_str(), type_def.as_ref())?;
        }
        for key in args.metadata.keys() {
            validate_writable_metadata_key(key.as_str(), type_def.as_ref())?;
        }
        // Unset has its own gate: the reserved `mem`/`id`/`type` triple
        // is unset-ALLOWED (the sanctioned repair for entities that
        // acquired a smuggled reserved key before the write gates
        // closed) while engine-stamped timestamp fields stay refused —
        // see `validate_unsettable_metadata_key`.
        for key in &args.metadata_unset {
            validate_unsettable_metadata_key(key.as_str(), type_def.as_ref())?;
        }

        // Reject the same key appearing in `metadata` (set) and
        // `metadata_unset` — the wire contract is that the conflict is
        // a hard error. Caught before any required-field /
        // parse-metadata check so the resolution ("pick one map") is
        // unambiguous regardless of whether the overlapping key is
        // required.
        let mut overlap: Vec<String> = args
            .metadata
            .keys()
            .filter(|k| args.metadata_unset.iter().any(|u| u == k.as_str()))
            .cloned()
            .collect();
        if !overlap.is_empty() {
            overlap.sort();
            overlap.dedup();
            return Err(EngineError::SetAndUnsetConflict { keys: overlap });
        }

        // Repair-power gate:
        // repair-shaped input is accepted only when the entity
        // currently fails the conformance check against the effective
        // schema. Conformance is per-entity-local and cheap, so the
        // gate runs in pre-validation; no agent-settable flag exists —
        // the entity's own state is the evidence. A pure-consistency
        // break does not open the gate (those have ungated repair
        // paths: `memstead_relate(remove)` and the additive params).
        if !args.relations_unset.is_empty() {
            let findings = crate::ops::integrity::entity_conformance_findings(
                &self.store,
                &entity,
                schema.as_ref(),
                &self.schemas,
            );
            if findings.is_empty() {
                return Err(EngineError::RepairNotNeeded {
                    id: id.to_string(),
                    recovery: "use memstead_relate(remove=true) to detach an edge from a                                conformant entity, or the additive memstead_update params                                to evolve it"
                        .to_string(),
                });
            }
        }

        Ok(ValidatedUpdate {
            args,
            mount_idx,
            mem,
            entity,
            prev_body_targets,
            schema,
            type_def,
            anchors: validated_anchors,
            anchor_unsets: validated_anchor_unsets,
        })
    }
}
