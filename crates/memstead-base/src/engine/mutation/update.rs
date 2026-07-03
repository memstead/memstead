//! `Engine::update_entity` and `Engine::batch_update` — rewrite an
//! entity's sections / metadata in place, optimistically-locked.

use std::path::Path;

use crate::engine_fallback_type;
use crate::entity::EntityId;
use crate::entity::generator::generate_markdown;
use crate::entity::parser::parse_markdown;
use crate::entity::store_builder::push_entities_into_store;
use crate::ops::{ModifiedMetadata, ModifiedSections, WarningHint};
use crate::provenance::{Provenance, ProvenanceKind};
use crate::runtime_validator::{
    parse_metadata_value, validate_section_content, validate_section_keys,
    validate_updatable_section, validate_writable_metadata_key,
};
use crate::vcs::{Actor, ClientId, CommitContext};
use crate::workspace::MountCapability;

use super::super::{Engine, EngineError, UpdateEntityArgs, UpdateEntityOutcome};
use super::{
    PATCH_OLD_NOT_FOUND_CONTENT_CAP, make_stub, today_iso, unknown_type_error,
    validate_relation_target_grammar,
};
use crate::engine::outcomes::RelationDeclared;
use crate::entity::{Entity, Relationship};

use std::sync::Arc;

/// Result of [`Engine::prepare_update`] — the validation + markdown
/// step split out of the commit so the batch path can prepare every
/// item before committing the whole set atomically.
enum PrepareOutcome {
    /// No commit is needed: the no-op short-circuit (content unchanged)
    /// and the dry-run preview both return a finished outcome here.
    Done(UpdateEntityOutcome),
    /// A real change whose post-mutation markdown is ready to stage +
    /// commit.
    Prepared(PreparedUpdate),
}

/// Everything the commit step needs to stage one prepared update's
/// disk write and build its outcome. Carries no commit SHA — that's
/// produced when the (single or batched) commit lands.
struct PreparedUpdate {
    mount_idx: usize,
    id: EntityId,
    mem: String,
    type_def: Arc<memstead_schema::TypeDefinition>,
    file_path: String,
    markdown: String,
    /// Body wiki-link targets the entity had *before* this mutation —
    /// the GC sweep scopes orphan-stub detection to these.
    prev_body_targets: std::collections::HashSet<EntityId>,
    modified_date: String,
    modified_sections: ModifiedSections,
    modified_metadata: ModifiedMetadata,
    warnings: Vec<WarningHint>,
    relations_declared: Vec<RelationDeclared>,
}

/// The store-side results of applying a prepared write — filled in
/// after the commit lands by [`Engine::apply_prepared_to_store`].
struct AppliedWrite {
    content_hash: String,
    title: String,
    orphan_stubs_removed: Vec<EntityId>,
}

impl Engine {
    /// Update an entity's sections and/or metadata.
    ///
    /// Same six-concern shape as [`Engine::create_entity`]. Optimistic
    /// locking via `args.expected_hash`: when `Some`, must match the
    /// store's current `content_hash` or returns
    /// [`EngineError::HashMismatch`]. The new engine's MCP-facing
    /// callers should always pass the hash; `None` is the
    /// `--force`-style escape hatch.
    ///
    /// Internally a two-step pipeline: [`Self::prepare_update`] runs
    /// all validation and computes the post-mutation markdown without
    /// committing, then [`Self::commit_prepared_update`] stages and
    /// commits the result. The split lets [`Self::batch_update`]
    /// prepare every item up front and commit the whole batch as one
    /// atomic unit.
    pub fn update_entity(
        &mut self,
        args: UpdateEntityArgs,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<UpdateEntityOutcome, EngineError> {
        // Reload-before-operation: probe the mem ref and reload if a
        // sibling advanced it, so the `expected_hash` compare inside
        // `prepare_update` runs against current truth. A stale hash for
        // the targeted entity then trips a real `HASH_MISMATCH`; an
        // unrelated concurrent write leaves this entity's hash intact
        // and the update proceeds. The drift notice rides the outcome.
        let mut drift_warnings = self.reload_if_stale(Some(args.id.mem()));
        let mut outcome = match self.prepare_update(args)? {
            PrepareOutcome::Done(outcome) => outcome,
            PrepareOutcome::Prepared(prepared) => {
                self.commit_prepared_update(prepared, actor, client, note)?
            }
        };
        drift_warnings.append(&mut outcome.warnings);
        outcome.warnings = drift_warnings;
        Ok(outcome)
    }

    /// Stage the prepared disk write, commit it as one commit, append
    /// provenance, and apply the change to the in-memory store — the
    /// single-update tail of [`Self::update_entity`]. The batch path
    /// drives the same steps but commits once across all items.
    fn commit_prepared_update(
        &mut self,
        prepared: PreparedUpdate,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<UpdateEntityOutcome, EngineError> {
        let backend = self.mounts[prepared.mount_idx].backend.as_ref();
        backend.write_entity(Path::new(&prepared.file_path), prepared.markdown.as_bytes())?;
        let commit_subject = format!("memstead: update {}", prepared.id);
        let ctx = CommitContext {
            actor,
            client: client.cloned(),
            tool: Some("update_entity"),
            note: note.map(String::from),
            logical_operation_id: None,
            entity_ids: None,
        };
        let commit_sha = backend.commit(&commit_subject, &ctx)?;
        backend.append_provenance(&Provenance::new(
            std::time::SystemTime::now(),
            ProvenanceKind::Update,
            Some(prepared.id.to_string()),
            actor,
            client.cloned(),
            note.map(String::from),
        ))?;
        self.record_self_write(prepared.mount_idx, &commit_sha);

        let applied = self.apply_prepared_to_store(&prepared)?;

        self.invalidate_communities();
        self.invalidate_search_indexes();

        // `require_notes` provenance nudge — single engine-level
        // enforcement point. Only reached on the real-commit path; the
        // no-op and dry-run prepare outcomes never demand a note.
        let mut warnings = prepared.warnings;
        if let Some(w) = self.note_missing_warning("update_entity", note) {
            warnings.push(w);
        }

        Ok(UpdateEntityOutcome {
            id: prepared.id.clone(),
            title: applied.title,
            file_path: prepared.file_path,
            content_hash: applied.content_hash,
            commit_sha,
            modified_date: prepared.modified_date,
            orphan_stubs_removed: applied.orphan_stubs_removed,
            modified_sections: prepared.modified_sections,
            modified_metadata: prepared.modified_metadata,
            prospective_hash: None,
            warnings,
            relations_declared: prepared.relations_declared,
        })
    }

    /// Parse the prepared markdown, push it into the in-memory store,
    /// re-map alias-target edge sources, and GC any stub the mutation
    /// orphaned. Shared post-commit store-application step for the
    /// single-update and batch paths — does NOT touch the backend or
    /// commit (the caller has already staged + committed the disk
    /// write).
    fn apply_prepared_to_store(
        &mut self,
        prepared: &PreparedUpdate,
    ) -> Result<AppliedWrite, EngineError> {
        let parse_result = parse_markdown(
            &prepared.markdown,
            &prepared.file_path,
            prepared.type_def.as_ref(),
            &prepared.mem,
        )
        .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
        let content_hash = parse_result.entity.content_hash.clone();
        let title = parse_result.entity.title.clone();
        let fallback = engine_fallback_type();
        push_entities_into_store(&mut self.store, vec![parse_result], fallback.as_ref(), None);
        crate::entity::store_builder::remap_alias_target_edge_sources(
            &mut self.store,
            &self.schemas,
        );
        let orphan_stubs_removed =
            super::gc_orphan_stubs_among(&mut self.store, &prepared.prev_body_targets);
        Ok(AppliedWrite {
            content_hash,
            title,
            orphan_stubs_removed,
        })
    }

    /// Validate an update and compute its post-mutation markdown
    /// *without* committing. Returns [`PrepareOutcome::Done`] for the
    /// no-op / dry-run short-circuits (which never commit) and
    /// [`PrepareOutcome::Prepared`] for a real change whose write the
    /// caller stages + commits. May mutate the store in place via the
    /// alias-synthesis auto-stub upsert; the batch path snapshots the
    /// store before preparing so a refused batch can roll that back.
    fn prepare_update(&mut self, args: UpdateEntityArgs) -> Result<PrepareOutcome, EngineError> {
        let id = &args.id;
        let mem = id.mem().to_string();

        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .ok_or_else(|| EngineError::UnknownMem(mem.clone()))?;
        if self.mounts[mount_idx].mount.capability != MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem));
        }

        let entity = self
            .store
            .get(id)
            .ok_or_else(|| EngineError::NotFound { id: id.to_string() })?;

        // Snapshot the prev entity's body wiki-link set before any
        // subsequent `&mut self` reborrow burns the `entity` borrow.
        // Fed to the alias-synthesis pass so the GC step can compare
        // prev vs. next body links and drop pointer-rel-type relations
        // whose target was a body link before but isn't any more.
        let prev_body_targets = super::collect_body_link_targets(entity);

        // Stub guard — stubs have no body, no metadata, no
        // schema-resolved type to validate against. The recovery is
        // `memstead_create` (stub adoption preserves incoming
        // references). Pre-Item-02 the update path fell through to
        // the `type_def` lookup below and surfaced the cryptic
        // `UnknownType { name: "" }` cascade. Mirrors the
        // `StubCannotRelate` guard on `memstead_relate`.
        if entity.stub {
            return Err(EngineError::StubNotUpdatable { id: id.to_string() });
        }

        // Skip the hash check on dry_run — full's dry_run is the
        // designated stale-hash recovery path. Agents preview a
        // change without holding a fresh hash, get back the current
        // `content_hash` and a `prospective_hash`, then call the
        // real update with `expected_hash = content_hash`.
        if !args.dry_run
            && let Some(expected) = args.expected_hash.as_deref()
            && entity.content_hash != expected
        {
            return Err(EngineError::HashMismatch {
                id: id.to_string(),
                current: entity.content_hash.clone(),
                is_stub: entity.stub,
            });
        }

        // Empty-mutation guard. After existence/stub/hash
        // gates so the more-specific errors fire first. A payload
        // with no recognised mutation content refuses BEFORE any
        // mutation work runs so a misspelled or omitted mutation
        // key (which deserialised to empty defaults under the
        // lenient pre-fix posture) doesn't silently land as
        // `succeeded: N, action: "updated", commit_sha: ""`.
        // Distinct from `UPDATE_NOOP` (a warning that fires when
        // mutation content was provided but matched the current
        // entity state) — the two are different states and ship
        // different envelopes.
        if args.sections.is_empty()
            && args.append_sections.is_empty()
            && args.patch_sections.is_empty()
            && args.metadata.is_empty()
            && args.metadata_unset.is_empty()
            && args.declare_relations.is_empty()
            && args.relations_unset.is_empty()
        {
            return Err(EngineError::EmptyUpdate { id: id.to_string() });
        }

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

        validate_section_keys(
            args.sections
                .keys()
                .chain(args.append_sections.keys())
                .chain(args.patch_sections.keys())
                .map(String::as_str),
            type_def.as_ref(),
        )?;
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
                        .map(|(k, p)| (k.as_str(), p.new.as_str())),
                ),
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
        for key in &args.metadata_unset {
            validate_writable_metadata_key(key.as_str(), type_def.as_ref())?;
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
                entity,
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

        let mut next = entity.clone();

        // Repair-shaped removals, applied before declarations so a
        // repair can drop and re-shape relations in one atomic
        // update. Absent (rel_type, target) pairs are silent no-ops,
        // symmetric with `metadata_unset`. The strict post-state
        // validation below still runs — repair widens accepted
        // inputs, never admissible outputs.
        for unset in &args.relations_unset {
            let canonical = crate::entity::id::validate_rel_type(&unset.rel_type)
                .unwrap_or_else(|_| unset.rel_type.clone());
            next.relationships
                .retain(|r| !(r.rel_type == canonical && r.target == unset.target));
        }

        // Atomic batched relation declarations. Validated and applied
        // before the section/metadata changes so the strict
        // wiki-link/relation validator at the end of this fn sees
        // the freshly-declared relations as part of the post-state.
        // Same vocabulary + shape + grammar gates `memstead_relate`
        // runs; auto-stubs absent Write-mem targets identically
        // to the relate path. Returns the (rel_type, target,
        // target_was_stubbed) triples in `relations_declared` on
        // the outcome so the agent sees what landed.
        let relations_declared = apply_declare_relations(
            self,
            &mut next,
            &args.declare_relations,
            &mem,
            mount_idx,
            type_def.as_ref(),
            schema.as_ref(),
        )?;

        let mut modified_sections: Vec<String> = Vec::new();
        for (key, body) in args.sections {
            modified_sections.push(key.clone());
            next.sections.insert(key, body);
        }

        // Apply append_sections after replace. Empty/absent body
        // is replaced wholesale with the append value; otherwise
        // a `\n` separator joins the two. Mirrors full.
        let mut modified_sections_appended: Vec<String> = Vec::new();
        for (key, value) in args.append_sections {
            let existing = next.sections.get(&key).cloned().unwrap_or_default();
            let new_content = if existing.trim().is_empty() {
                value
            } else {
                format!("{existing}\n{value}")
            };
            next.sections.insert(key.clone(), new_content);
            modified_sections_appended.push(key);
        }

        // Apply patch_sections after append. Find-and-replace the
        // `old` substring with `new`; `all` flips between
        // first-occurrence (replacen 1) and every-occurrence
        // (replace). Empty/absent section is rejected with
        // PatchSectionEmpty; missing-`old` rejected with
        // PatchOldNotFound carrying a UTF-8-safe truncated snapshot
        // of the current body. Mirrors full.
        let mut modified_sections_patched: Vec<String> = Vec::new();
        for (key, patch) in args.patch_sections {
            let existing = next
                .sections
                .get(&key)
                .ok_or_else(|| EngineError::PatchSectionEmpty {
                    section: key.clone(),
                })?
                .clone();
            if !existing.contains(&patch.old) {
                let cap = PATCH_OLD_NOT_FOUND_CONTENT_CAP;
                let truncated = existing.len() > cap;
                // Truncate at a UTF-8 char boundary to avoid
                // splitting a code point.
                let mut cut = cap.min(existing.len());
                while cut > 0 && !existing.is_char_boundary(cut) {
                    cut -= 1;
                }
                let current_content = if truncated {
                    existing[..cut].to_string()
                } else {
                    existing.clone()
                };
                return Err(EngineError::PatchOldNotFound {
                    section: key,
                    current_content,
                    truncated,
                });
            }
            let patched = if patch.all {
                existing.replace(&patch.old, &patch.new)
            } else {
                existing.replacen(&patch.old, &patch.new, 1)
            };
            next.sections.insert(key.clone(), patched);
            modified_sections_patched.push(key);
        }

        let mut modified_metadata_set: Vec<String> = Vec::new();
        for (key, value) in &args.metadata {
            let parsed = parse_metadata_value(key.as_str(), value.as_str(), type_def.as_ref())?;
            modified_metadata_set.push(key.clone());
            next.metadata.insert(key.clone(), parsed);
        }

        let mut modified_metadata_unset: Vec<String> = Vec::new();
        for key in args.metadata_unset {
            // Reject unset on required fields — the pre-remove check
            // carries the recovery payload (field_description,
            // enum_values, type_write_rules) so MCP envelopes surface
            // the full REQUIRED_FIELD_UNSET shape.
            let field_def = type_def.metadata_field(&key);
            let is_required = field_def.map(|f| !f.optional).unwrap_or(false);
            if is_required {
                let (field_description, enum_values) = match field_def {
                    Some(f) => (
                        Some(f.description.clone()),
                        f.enum_values.clone().unwrap_or_default(),
                    ),
                    None => (None, Vec::new()),
                };
                return Err(EngineError::RequiredFieldUnset {
                    field: key,
                    entity_type: type_def.name.clone(),
                    field_description,
                    enum_values,
                    type_write_rules: type_def.write_rules.clone(),
                    // Update path — caller passed
                    // `metadata_unset: ["field"]` against a required
                    // field. The wording ("cannot unset required
                    // field …") is semantically correct for this
                    // path.
                    on_create: false,
                    // The unset path targets one field per call by
                    // definition, so the multi-field accumulator
                    // stays empty here — the singular fields above
                    // are authoritative.
                    missing: Vec::new(),
                });
            }
            if next.metadata.shift_remove(&key).is_some() {
                modified_metadata_unset.push(key);
            }
        }

        // Delay the auto-stamp until AFTER the no-op short-circuit. Pre-fix
        // this branch overwrote `last_modified` with `today_iso()`
        // before the bytes-compare, so the prospective markdown
        // always differed from the on-disk bytes (the schema's
        // `last_modified` was already populated with a full ISO
        // timestamp on the prior write, but `today_iso()` returns
        // date-only), and the no-op compare never matched. Compute
        // `today` for later use but don't stamp `next` yet.
        let today = today_iso();

        // Alias-synthesis pass: for schemas declaring
        // `alias_target_rel_type`, append engine-emitted relations of
        // that rel-type for every body wiki-link not already backed,
        // and GC pointer-rel-type relations whose target was a body
        // wiki-link in the prev state but isn't in next. Cross-mem
        // refusal aborts the update — no partial state.
        //
        // The returned `Vec<Relationship>` is the per-call set of
        // synthesised relations; feed it into the auto-stub warning
        // emission below.
        let (synthesised_relations, self_link_ignored) =
            super::synthesise_alias_relations(self, &prev_body_targets, &mut next)?;

        // Alias-existence invariant: every body wiki-link must be
        // backed by an entry in `entity.relationships`. The validator
        // runs against the *full* post-mutation state (not just the
        // delta), so a mutation that leaves an existing unbacked link
        // in place still fails — forcing cleanup of historical drift.
        let missing = super::scan_wikilinks_without_relation(&next)?;
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

        let file_path = next.file_path.clone();

        // The bytes-compare runs against the pre-stamp markdown so the
        // auto-timestamp doesn't synthesise a false delta. When the
        // user-visible payload (sections, user-set metadata,
        // declared relations) didn't change, the pre-stamp markdown
        // matches the on-disk bytes byte-for-byte; we short-circuit
        // and return with `last_modified` preserved at its pre-call
        // value. Real changes fall through; we stamp + regenerate
        // below.
        let markdown_pre_stamp = generate_markdown(&next, type_def.as_ref());

        // No-op short-circuit. When the pre-stamp markdown's hash
        // matches the entity's current `content_hash`, the
        // post-mutation user-visible state equals the on-disk state.
        // Skip the disk write, the commit, the provenance append,
        // and the store re-parse. Mirrors `relate.rs`'s
        // `NoOpAlreadyPresent` / `NoOpAbsent` and `rename.rs`'s
        // slug-noop short-circuits: empty `commit_sha`, unchanged
        // `content_hash`, preserved `last_modified`, typed
        // `UpdateNoop` warning. Skipped on the dry_run path because
        // the dry_run preview semantics document a separate
        // non-committing shape with `prospective_hash: Some(_)` and
        // the unchanged `content_hash`; conflating the two would
        // lose the prospective-hash channel callers use to chain a
        // follow-up real update with `expected_hash`.
        if !args.dry_run {
            let prospective_hash = crate::entity::parser::compute_hash(&markdown_pre_stamp);
            // `next` was cloned from `entity` at the top of this fn
            // and its `content_hash` field has not been recomputed
            // since — equals the on-disk value. Reading from `next`
            // rather than `entity` avoids extending the `self.store`
            // immutable borrow past the mutable borrow taken by
            // `apply_declare_relations`.
            if prospective_hash == next.content_hash {
                // No-op: report the preserved `last_modified` from
                // the pre-stamp `next` (which still carries the
                // entity's on-disk value because we haven't run the
                // auto-stamp yet on this branch).
                let modified_date = next
                    .metadata
                    .get("last_modified")
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                return Ok(PrepareOutcome::Done(UpdateEntityOutcome {
                    id: id.clone(),
                    title: next.title.clone(),
                    file_path,
                    content_hash: next.content_hash.clone(),
                    commit_sha: String::new(),
                    modified_date,
                    // No-op: the prospective hash equals the on-disk hash,
                    // so nothing landed. `modified_*` report the *applied*
                    // delta (empty), consistent with the empty `commit_sha`
                    // and unchanged hash — not the request-derived keys,
                    // which would claim a change that did not happen
                    // (F1). The request vecs
                    // (`modified_metadata_set` etc.) are intentionally
                    // dropped on this branch.
                    modified_sections: ModifiedSections::default(),
                    modified_metadata: ModifiedMetadata::default(),
                    prospective_hash: None,
                    // No write happened on the no-op path, so nothing
                    // could have orphaned a stub.
                    orphan_stubs_removed: Vec::new(),
                    warnings: vec![WarningHint::UpdateNoop { id: id.clone() }],
                    relations_declared,
                }));
            }
        }

        // Real change: apply the auto-stamp now and regenerate the
        // markdown so the subsequent hash + write reflect it.
        super::auto_stamp_timestamps(&mut next, type_def.as_ref(), &today);
        let markdown = generate_markdown(&next, type_def.as_ref());

        let mut warnings: Vec<WarningHint> = Vec::new();

        // Mirror the create-path emission shape — drive the warning from
        // the synthesised relations the alias pass just emitted, not
        // from a re-parse of the generated markdown. `parse_markdown`
        // filters its `inline_links` against the entity's
        // `relationships` vec (which the synthesis pass has already
        // appended to), so the pre-fix path saw `inline_links: []`
        // and silently dropped the warning the docstring promises.
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
        // F11: surface a dropped self-referential body link (the alias
        // pass omitted the vacuous self-edge).
        if self_link_ignored {
            warnings.push(WarningHint::SelfLinkIgnored { id: id.clone() });
        }

        // Dry-run: compute prospective hash from the in-memory
        // entity and return without touching disk, store, or
        // commits. Mirrors full's `UpdateArgs.dry_run` semantics —
        // `content_hash` carries the unchanged on-disk hash so the
        // caller can use it as `expected_hash` on the follow-up
        // real call (designated stale-hash recovery path).
        if args.dry_run {
            let prospective = crate::entity::parser::compute_hash(&markdown);
            // `next.content_hash` was cloned from the source
            // entity and not modified since; equals the on-disk
            // value.
            let current_hash = next.content_hash.clone();
            let modified_date = if type_def.metadata_fields.iter().any(|f| f.auto_timestamp) {
                today.clone()
            } else {
                String::new()
            };
            return Ok(PrepareOutcome::Done(UpdateEntityOutcome {
                id: id.clone(),
                title: next.title.clone(),
                file_path,
                content_hash: current_hash,
                commit_sha: String::new(),
                modified_date,
                modified_sections: ModifiedSections {
                    replaced: modified_sections,
                    appended: modified_sections_appended,
                    patched: modified_sections_patched,
                },
                modified_metadata: ModifiedMetadata {
                    set: modified_metadata_set,
                    unset: modified_metadata_unset,
                },
                prospective_hash: Some(prospective),
                // Dry-run touches neither store nor disk, so no stub
                // could have been GC'd.
                orphan_stubs_removed: Vec::new(),
                warnings,
                relations_declared: relations_declared.clone(),
            }));
        }

        // Real change prepared. Compute `modified_date` (mirrors full's
        // UpdateResult.modified_date — the `today` the auto-stamp loop
        // used; empty when the schema has no auto_timestamp field), then
        // hand the staged write to the caller to commit. The single
        // path commits immediately; the batch path commits the whole
        // set at once.
        let modified_date = if type_def.metadata_fields.iter().any(|f| f.auto_timestamp) {
            today.clone()
        } else {
            String::new()
        };

        Ok(PrepareOutcome::Prepared(PreparedUpdate {
            mount_idx,
            id: id.clone(),
            mem,
            type_def,
            file_path,
            markdown,
            prev_body_targets,
            modified_date,
            modified_sections: ModifiedSections {
                replaced: modified_sections,
                appended: modified_sections_appended,
                patched: modified_sections_patched,
            },
            modified_metadata: ModifiedMetadata {
                set: modified_metadata_set,
                unset: modified_metadata_unset,
            },
            // F5: `InlineWikiLinkAutoStubbed` rides on the outcome so the
            // update path matches create's contract.
            warnings,
            relations_declared,
        }))
    }

    /// Apply a batch of [`UpdateEntityArgs`] **atomically** — all or
    /// nothing. Surfaces `BatchResult` for `memstead batch-update`
    /// consumers.
    ///
    /// The batch validates and prepares every item first (each with
    /// its own optimistic-lock check), then commits the whole set as
    /// **one** commit per mem. If any item fails — validation error,
    /// `HASH_MISMATCH`, entity-not-found, any per-item refusal —
    /// **nothing is committed**: the on-disk mem and the in-memory
    /// store are restored to exactly their pre-call state, and the
    /// result is marked `applied: false` with the offending item
    /// carrying a typed `{code, message, details}` error envelope and
    /// every other item marked `"not_applied"`. The first failing item
    /// stops preparation (fail-fast); the caller fixes it and
    /// resubmits.
    ///
    /// On success the returned `commit_sha` is the single batch commit
    /// — an honest `memstead_changes_since` cursor / revert handle. Each
    /// item's per-entry note rides into its own provenance record.
    ///
    /// Empty batches return `applied: true` with zero counts and no
    /// commit. A batch where every item is a no-op (content unchanged)
    /// likewise applies with an empty `commit_sha`.
    ///
    /// Atomicity is per-mem: for the common single-mem batch a
    /// commit-time backend failure rolls the whole batch back. A batch
    /// spanning multiple mems commits each mem in turn; if a later
    /// mem's commit fails, already-committed mems stay committed
    /// (true cross-mem two-phase commit is out of scope) — but the
    /// dominant failure mode, a per-item validation/hash refusal, is
    /// always fully atomic because no commit happens until every item
    /// has passed.
    pub fn batch_update(
        &mut self,
        updates: Vec<(UpdateEntityArgs, Option<String>)>,
        actor: Actor,
        client: Option<&ClientId>,
    ) -> Result<crate::ops::BatchResult, EngineError> {
        if updates.is_empty() {
            return Ok(crate::ops::BatchResult {
                applied: true,
                results: Vec::new(),
                succeeded: 0,
                failed: 0,
                commit_sha: String::new(),
            });
        }

        // Reload-before-operation: refresh every mem this batch
        // touches *before* preparing items, so each item's
        // `expected_hash` check runs against current truth (the batch
        // is the one multi-op-per-process path, so a sibling commit
        // between boot and this call is plausible). Notices stash on
        // the engine for the caller to drain.
        let mut touched_mems: Vec<String> = updates
            .iter()
            .map(|(a, _)| a.id.mem().to_string())
            .collect();
        touched_mems.sort();
        touched_mems.dedup();
        for v in &touched_mems {
            self.reload_if_stale(Some(v));
        }

        // Snapshot the in-memory store so a refused batch (or a
        // commit-time backend failure) can roll back any auto-stubs
        // and store pushes that earlier items already applied during
        // preparation. The on-disk side rolls back by discarding each
        // backend's staged-but-uncommitted pending buffer.
        let store_snapshot = self.store.clone();

        // What each item is, in submission order, so the result
        // entries echo the input order. `Prepared` is a real write
        // (its `PreparedUpdate` lives in `prepared`); `Noop` is an
        // applied no-op (content unchanged, no write).
        enum Item {
            Prepared,
            Noop,
        }
        let mut items: Vec<(EntityId, Item)> = Vec::with_capacity(updates.len());
        let mut prepared: Vec<PreparedUpdate> = Vec::new();
        let mut notes: Vec<Option<String>> = Vec::new();

        // --- Phase 1: validate + prepare every item (no commits) ---
        let mut iter = updates.into_iter();
        while let Some((args, note)) = iter.next() {
            let id = args.id.clone();
            match self.prepare_update(args) {
                Ok(PrepareOutcome::Done(_)) => {
                    // No-op (batch never sets dry_run): applied, no write.
                    items.push((id, Item::Noop));
                }
                Ok(PrepareOutcome::Prepared(p)) => {
                    prepared.push(p);
                    notes.push(note);
                    items.push((id, Item::Prepared));
                }
                Err(e) => {
                    // Refuse the whole batch. Roll back store + disk.
                    self.store = store_snapshot;
                    self.discard_all_pending();
                    let mut results: Vec<crate::ops::BatchEntry> = items
                        .into_iter()
                        .map(|(prev_id, _)| crate::ops::BatchEntry {
                            id: prev_id,
                            action: "not_applied".to_string(),
                            error: None,
                        })
                        .collect();
                    results.push(crate::ops::BatchEntry {
                        id,
                        action: "error".to_string(),
                        error: Some(batch_error_envelope(&e)),
                    });
                    // Items after the failure were never prepared.
                    for (rem_args, _) in iter {
                        results.push(crate::ops::BatchEntry {
                            id: rem_args.id,
                            action: "not_applied".to_string(),
                            error: None,
                        });
                    }
                    return Ok(crate::ops::BatchResult {
                        applied: false,
                        results,
                        succeeded: 0,
                        failed: 1,
                        commit_sha: String::new(),
                    });
                }
            }
        }

        // --- Phase 2: stage every prepared write, then commit once
        // per mem. ---
        for p in &prepared {
            if let Err(e) = self.mounts[p.mount_idx]
                .backend
                .write_entity(Path::new(&p.file_path), p.markdown.as_bytes())
            {
                self.store = store_snapshot;
                self.discard_all_pending();
                return Err(e.into());
            }
        }

        // Distinct mount indices in first-seen order — one commit each.
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
            let subject = format!("memstead: batch-update ({count} entities)");
            let ctx = CommitContext {
                actor,
                client: client.cloned(),
                tool: Some("batch_update"),
                note: None,
                logical_operation_id: None,
                // F13: name every entity this batch commit touched so an
                // `--include-notes` reader can recover them from the note
                // record alone — the subject only says `(N entities)`.
                entity_ids: Some(entity_ids),
            };
            match self.mounts[m].backend.commit(&subject, &ctx) {
                Ok(sha) => mount_commits.push((m, sha)),
                Err(e) => {
                    // A commit failed. Roll back the store and any
                    // still-pending backends. Mems already committed
                    // in this loop stay committed (per-mem atomicity).
                    self.store = store_snapshot;
                    self.discard_all_pending();
                    return Err(e.into());
                }
            }
        }

        // Provenance + store application per item, now that the commits
        // landed. `record_self_write` marks the commit as engine-self
        // so drift detection ignores it.
        for (p, note) in prepared.iter().zip(notes.iter()) {
            let commit_sha = mount_commits
                .iter()
                .find(|(m, _)| *m == p.mount_idx)
                .map(|(_, s)| s.clone())
                .unwrap_or_default();
            self.mounts[p.mount_idx]
                .backend
                .append_provenance(&Provenance::new(
                    std::time::SystemTime::now(),
                    ProvenanceKind::Update,
                    Some(p.id.to_string()),
                    actor,
                    client.cloned(),
                    note.clone(),
                ))?;
            self.record_self_write(p.mount_idx, &commit_sha);
            self.apply_prepared_to_store(p)?;
        }

        self.invalidate_communities();
        self.invalidate_search_indexes();

        // Single-mem batches name their one commit; multi-mem names
        // the last mem committed (see the method docstring).
        let commit_sha = mount_commits
            .last()
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        let succeeded = items.len();
        let results: Vec<crate::ops::BatchEntry> = items
            .into_iter()
            .map(|(id, item)| crate::ops::BatchEntry {
                id,
                action: match item {
                    Item::Prepared => "updated".to_string(),
                    Item::Noop => "noop".to_string(),
                },
                error: None,
            })
            .collect();

        Ok(crate::ops::BatchResult {
            applied: true,
            results,
            succeeded,
            failed: 0,
            commit_sha,
        })
    }

    /// Best-effort discard of every backend's staged-but-uncommitted
    /// pending buffer — the disk-side half of an atomic-batch rollback.
    /// Discard errors (a poisoned pending mutex) are swallowed: we are
    /// already unwinding a refused batch and have nothing better to do.
    fn discard_all_pending(&self) {
        for mount in &self.mounts {
            let _ = mount.backend.discard_pending();
        }
    }

    /// CommitContext-bundling wrapper around [`Self::update_entity`].
    /// See [`Self::create_entity_with_ctx`] for the rationale.
    pub fn update_entity_with_ctx(
        &mut self,
        args: UpdateEntityArgs,
        ctx: &CommitContext<'_>,
    ) -> Result<UpdateEntityOutcome, EngineError> {
        self.update_entity(args, ctx.actor, ctx.client.as_ref(), ctx.note.as_deref())
    }
}

/// Build a per-item structured error envelope for [`Engine::batch_update`].
/// Mirrors the `{code, message, details}` shape single-update failures
/// carry on the MCP wire so a mixed-success batch is structurally uniform.
/// Variants without a typed recovery payload (boundary / internal failures
/// like `ParseAfterWrite`, `Backend`) return an empty details object —
/// the code and message channels still discriminate.
fn batch_error_envelope(err: &EngineError) -> crate::ops::BatchError {
    // The per-item envelope reads the centralised
    // `EngineError::details()` helper so every typed variant ships the
    // same recovery payload the singleton MCP/CLI surfaces emit, so
    // agents' "fix from `details` rather than re-fetching" loop works
    // the same in batch mode.
    let code = err.code().to_string();
    let message = err.to_string();
    let details = err.details();
    crate::ops::BatchError {
        code,
        message,
        details,
    }
}

/// Validate, auto-stub, and append a batch of relation declarations
/// onto `next.relationships`. Returns the canonical
/// `RelationDeclared` summary echoed back on the outcome.
///
/// Validates each declared relation against the same gates
/// `memstead_relate` runs (target-id grammar, rel-type vocabulary,
/// schema shape, cross-mem policy, ReadOnly-target rule). On the
/// add path with an absent Write-mem target, the target is
/// auto-stubbed via `make_stub` — matching the
/// `WarningHint::AutoStubCreated` semantics of the relate flow.
///
/// Defined at module scope (rather than a method on `Engine`) so
/// the borrow on `engine.store` for the auto-stub upsert can run
/// alongside the `&mut next` borrow.
fn apply_declare_relations(
    engine: &mut Engine,
    next: &mut Entity,
    declarations: &[crate::ops::RelateArg],
    source_mem: &str,
    source_mount_idx: usize,
    type_def: &memstead_schema::TypeDefinition,
    schema: &memstead_schema::Schema,
) -> Result<Vec<RelationDeclared>, EngineError> {
    let _ = type_def; // Reserved for future per-type policy hooks.
    let _ = source_mount_idx; // Reserved for parity with delete.
    let mut declared: Vec<RelationDeclared> = Vec::with_capacity(declarations.len());
    for rel in declarations {
        // Canonicalise rel_type to UPPER_SNAKE_CASE so the validator
        // and the stored edge see the same wire-contract form.
        let canonical = crate::entity::id::validate_rel_type(&rel.rel_type)
            .unwrap_or_else(|_| rel.rel_type.clone());

        validate_relation_target_grammar(&rel.to)?;

        let target_mem = rel.to.mem().to_string();
        super::validate_cross_mem_add_policy(engine, source_mem, &target_mem)?;
        if target_mem != source_mem
            && let Some(mount) = engine.mount(&target_mem)
            && mount.capability == MountCapability::ReadOnly
            && !engine.store.contains(&rel.to)
        {
            return Err(EngineError::CrossMemTargetNotFound {
                target_id: rel.to.to_string(),
                target_mem: target_mem.clone(),
            });
        }

        // Rel-type + shape validation, routed through the engine's
        // cross-mem-aware edge validator. Cross-different-schema
        // edges check vocabulary + shape against the source schema's
        // `cross_mem_relationships:` entry; same-schema edges fall
        // through to the intra-mem `relationships.definitions`.
        // Open-mode admits unknown rel-types silently (no
        // per-declaration warning surfaced here — symmetry with the
        // pre-cross-mem behaviour).
        let target_type = engine
            .store
            .get(&rel.to)
            .map(|e| e.entity_type.clone())
            .filter(|t| !t.is_empty());
        let _ = schema; // Helper resolves the schema via the engine.
        let _ = super::route_edge_validation(
            engine,
            &canonical,
            next.entity_type.as_str(),
            target_type.as_deref(),
            source_mem,
            &target_mem,
            &next.id,
            &rel.to,
            /* check_shape = */ true,
        )?;

        // Per-edge description posture. Normalise first so
        // empty/whitespace-only inputs collapse to `None` and the
        // posture check sees a canonical input that matches what
        // the renderer will emit.
        let normalised_description =
            crate::entity::normalise_description(rel.description.as_deref());
        super::validate_description_posture(
            engine,
            &canonical,
            normalised_description.as_deref(),
            source_mem,
            &target_mem,
            &next.id,
            &rel.to,
        )?;
        // declare_relations is an explicit-author
        // boundary too — gate on manual_authoring posture.
        super::validate_manual_authoring_posture(
            engine, &canonical, source_mem, &next.id, &rel.to,
        )?;

        // Append to the entity's relationships list. Duplicate
        // declarations are idempotent — same (rel_type, target) pair
        // is a silent no-op so the agent can re-issue the same call
        // without surprise.
        let exists = next
            .relationships
            .iter()
            .any(|r| r.rel_type == canonical && r.target == rel.to);
        if !exists {
            next.relationships.push(Relationship {
                rel_type: canonical.clone(),
                target: rel.to.clone(),
                description: normalised_description,
            });
        }

        // Auto-stub absent Write-mem targets. Same mechanic as
        // `memstead_relate`'s relate path. ReadOnly cross-mem targets
        // were caught above; same-mem and cross-mem-to-Write
        // both fall through here.
        let target_was_stubbed = !engine.store.contains(&rel.to);
        if target_was_stubbed && !exists {
            engine.store.upsert(
                rel.to.clone(),
                make_stub(&rel.to, crate::entity::StubKind::ForwardReference),
            );
        }

        declared.push(RelationDeclared {
            rel_type: canonical,
            target: rel.to.clone(),
            target_was_stubbed,
        });
    }
    Ok(declared)
}

#[cfg(test)]
mod tests {

    use indexmap::IndexMap;
    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::engine::test_helpers::*;
    use crate::engine::{
        CreateEntityArgs, Engine, EngineError, RelateEntityArgs, UpdateEntityArgs,
    };
    use crate::entity::EntityId;

    use crate::storage::{ArchiveBackend, FilesystemMemWriter};
    use crate::vcs::Actor;

    #[test]
    fn batch_update_empty_batch_returns_zero_counts() {
        // No updates → BatchResult with zero counts + empty
        // commit_sha. No engine mutation happens.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        let result = engine.batch_update(Vec::new(), Actor::Cli, None).unwrap();
        assert!(result.applied, "empty batch is a vacuous success");
        assert_eq!(result.results.len(), 0);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 0);
        assert_eq!(result.commit_sha, "");
    }

    #[test]
    fn batch_update_refuses_whole_batch_when_one_item_fails() {
        // Atomic semantics: a 2-item batch where item 1 is valid and
        // item 2 targets a missing id refuses the WHOLE batch. Nothing
        // is committed — item 1 is NOT applied (its section change does
        // not land), `applied` is false, `commit_sha` is empty, the
        // missing item carries the typed ENTITY_NOT_FOUND envelope, and
        // the valid item is marked `not_applied`.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        // Seed: create an entity.
        let create_args = CreateEntityArgs {
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
        let created = engine
            .create_entity(create_args, Actor::Cli, None, None)
            .unwrap();

        // Batch: update the seed entity AND a missing id.
        let valid_update = UpdateEntityArgs {
            id: created.id.clone(),
            expected_hash: Some(created.content_hash.clone()),
            sections: IndexMap::from_iter([("identity".to_string(), "updated body".to_string())]),
            append_sections: IndexMap::new(),
            patch_sections: IndexMap::new(),
            metadata: IndexMap::new(),
            metadata_unset: Vec::new(),
            declare_relations: Vec::new(),
            dry_run: false,
            relations_unset: Vec::new(),
        };
        let missing_update = UpdateEntityArgs {
            id: EntityId("specs--nonexistent".to_string()),
            expected_hash: None,
            sections: IndexMap::new(),
            append_sections: IndexMap::new(),
            patch_sections: IndexMap::new(),
            metadata: IndexMap::new(),
            metadata_unset: Vec::new(),
            declare_relations: Vec::new(),
            dry_run: false,
            relations_unset: Vec::new(),
        };

        let result = engine
            .batch_update(
                vec![(valid_update, None), (missing_update, None)],
                Actor::Cli,
                None,
            )
            .unwrap();
        // Whole batch refused: nothing applied, no commit.
        assert!(!result.applied, "a failing item must refuse the batch");
        assert_eq!(result.results.len(), 2);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 1);
        assert_eq!(result.commit_sha, "", "refused batch must not commit");
        // First entry: the valid item, marked not_applied (the batch
        // was refused before it could land).
        assert_eq!(result.results[0].action, "not_applied");
        assert!(result.results[0].error.is_none());
        // Second entry: the failing item carries the typed envelope.
        assert_eq!(result.results[1].action, "error");
        let err = result.results[1]
            .error
            .as_ref()
            .expect("failed entry must carry a structured error envelope");
        assert_eq!(err.code, "ENTITY_NOT_FOUND");
        assert!(err.message.contains("not found"), "got: {}", err.message);

        // The valid item's section change must NOT have landed — the
        // store is byte-identical to pre-call.
        let seed = engine.get_entity(&created.id).unwrap();
        assert_eq!(
            seed.sections.get("identity").map(String::as_str),
            Some("seed identity"),
            "refused batch must leave the in-memory store untouched",
        );
        assert_eq!(
            seed.content_hash, created.content_hash,
            "refused batch must not change the entity's content hash",
        );
    }

    #[test]
    fn batch_update_applies_all_valid_items_as_one_commit() {
        // A 2-item batch where both items are valid
        // applies both and produces exactly one commit; the response's
        // commit_sha names it and both entries report "updated".
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        let mk = |title: &str| CreateEntityArgs {
            mem: "specs".to_string(),
            title: title.to_string(),
            entity_type: "spec".to_string(),
            sections: IndexMap::from_iter([
                ("identity".to_string(), "id".to_string()),
                ("purpose".to_string(), "purp".to_string()),
            ]),
            metadata: IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        };
        let a = engine
            .create_entity(mk("A"), Actor::Cli, None, None)
            .unwrap();
        let b = engine
            .create_entity(mk("B"), Actor::Cli, None, None)
            .unwrap();

        let upd = |id: EntityId, hash: String, body: &str| UpdateEntityArgs {
            id,
            expected_hash: Some(hash),
            sections: IndexMap::from_iter([("identity".to_string(), body.to_string())]),
            append_sections: IndexMap::new(),
            patch_sections: IndexMap::new(),
            metadata: IndexMap::new(),
            metadata_unset: Vec::new(),
            declare_relations: Vec::new(),
            dry_run: false,
            relations_unset: Vec::new(),
        };

        let result = engine
            .batch_update(
                vec![
                    (upd(a.id.clone(), a.content_hash.clone(), "A body"), None),
                    (upd(b.id.clone(), b.content_hash.clone(), "B body"), None),
                ],
                Actor::Cli,
                None,
            )
            .unwrap();
        assert!(result.applied);
        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 0);
        assert!(
            !result.commit_sha.is_empty(),
            "applied batch carries the commit"
        );
        assert!(result.results.iter().all(|e| e.action == "updated"));
        // Both section changes landed.
        assert_eq!(
            engine
                .get_entity(&a.id)
                .unwrap()
                .sections
                .get("identity")
                .map(String::as_str),
            Some("A body"),
        );
        assert_eq!(
            engine
                .get_entity(&b.id)
                .unwrap()
                .sections
                .get("identity")
                .map(String::as_str),
            Some("B body"),
        );
    }

    #[test]
    fn batch_update_rolls_back_in_memory_store_auto_stub_on_refusal() {
        // The subtle invariant: an earlier item that auto-stubs a
        // relation target during preparation must have that stub rolled
        // OUT of the in-memory store when a later item refuses the
        // batch. Item 1 declares a relation to an absent target (which
        // upserts a forward-reference stub during prepare); item 2
        // targets a missing entity and fails. The refusal must leave no
        // trace of the stub.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        engine.set_workspace_root(mem_dir);
        let (actor, client) = cli_actor();

        let a = engine
            .create_entity(
                empty_create_args("specs", "Anchor"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let stub_target = EntityId::new("specs", "would-be-stub");
        let item1 = UpdateEntityArgs {
            relations_unset: Vec::new(),
            id: a.id.clone(),
            expected_hash: Some(a.content_hash.clone()),
            sections: IndexMap::new(),
            append_sections: IndexMap::new(),
            patch_sections: IndexMap::new(),
            metadata: IndexMap::new(),
            metadata_unset: Vec::new(),
            declare_relations: vec![crate::ops::RelateArg {
                rel_type: "USES".to_string(),
                to: stub_target.clone(),
                description: None,
            }],
            dry_run: false,
        };
        let item2 = UpdateEntityArgs {
            id: EntityId::new("specs", "nonexistent"),
            expected_hash: None,
            sections: IndexMap::from_iter([("identity".to_string(), "x".to_string())]),
            append_sections: IndexMap::new(),
            patch_sections: IndexMap::new(),
            metadata: IndexMap::new(),
            metadata_unset: Vec::new(),
            declare_relations: Vec::new(),
            dry_run: false,
            relations_unset: Vec::new(),
        };

        // Sanity: the would-be stub does not exist before the batch.
        assert!(engine.get_entity(&stub_target).is_none());

        let result = engine
            .batch_update(vec![(item1, None), (item2, None)], actor, Some(&client))
            .unwrap();
        assert!(!result.applied, "missing item 2 must refuse the batch");

        // The auto-stub item 1 created during preparation was rolled
        // back with the store snapshot — no orphaned stub survives.
        assert!(
            engine.get_entity(&stub_target).is_none(),
            "refused batch must roll the in-memory auto-stub back out of the store",
        );
        // The anchor's relation set is unchanged too.
        let anchor = engine.get_entity(&a.id).unwrap();
        assert!(
            !anchor.relationships.iter().any(|r| r.target == stub_target),
            "refused batch must not leave the declared relation on the anchor",
        );
    }

    #[test]
    fn update_entity_replaces_a_section_and_logs_provenance() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Updatable");
        let (actor, client) = cli_actor();

        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "Updated body.".to_string());

        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                Some("section update"),
            )
            .unwrap();

        assert_eq!(
            outcome.modified_sections.replaced,
            vec!["identity".to_string()]
        );
        assert_ne!(
            outcome.content_hash, seeded.content_hash,
            "hash must change"
        );
        // Store carries the new content.
        let entity = engine.get_entity(&seeded.id).unwrap();
        assert!(
            entity
                .sections
                .get("identity")
                .unwrap()
                .contains("Updated body.")
        );
        // Provenance log records the update.
        let log = std::fs::read_to_string(tmp.path().join(".memstead/changes.jsonl")).unwrap();
        assert!(log.contains("\"kind\":\"update\""));
        assert!(log.contains("\"note\":\"section update\""));
    }

    #[test]
    fn update_entity_rejects_hash_mismatch() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Hash Guarded");
        let (actor, client) = cli_actor();
        let err = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some("wrong-hash".to_string()),
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::HashMismatch {
                id,
                current,
                is_stub,
            } => {
                assert_eq!(id, seeded.id.to_string());
                assert_eq!(current, seeded.content_hash);
                assert!(!is_stub, "real entity must not flag as stub");
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn update_entity_rejects_unknown_id() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, _) = engine_with_seed(&tmp, "Anchor");
        let (actor, client) = cli_actor();
        let err = engine
            .update_entity(
                UpdateEntityArgs {
                    id: crate::EntityId::new("specs", "ghost"),
                    expected_hash: None,
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NotFound { .. }));
    }

    #[test]
    fn update_entity_rejects_read_only_mount() {
        let tmp = TempDir::new().unwrap();
        let archive_path = build_archive(
            tmp.path(),
            "ext",
            &[(
                "a.md",
                b"---\ntype: spec\n---\n# A\n\n## Identity\n\nbody.\n",
            )],
        );
        let mut engine = Engine::from_mounts(vec![(
            archive_mount("external", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)),
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        let id = crate::EntityId::new("external", "a");
        let err = engine
            .update_entity(
                UpdateEntityArgs {
                    id,
                    expected_hash: None,
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::ReadOnlyMount(v) if v == "external"));
    }

    #[test]
    fn update_entity_patches_section_with_find_and_replace() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Patch Subject");
        let (actor, client) = cli_actor();

        // Pre-write a known body via the replace path so the patch
        // test has a deterministic substring to target.
        let mut replace = IndexMap::new();
        replace.insert("identity".to_string(), "hello world hello".to_string());
        let replaced = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections: replace,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // First-occurrence patch (all = false).
        let mut patches = IndexMap::new();
        patches.insert(
            "identity".to_string(),
            crate::ops::PatchArg {
                old: "hello".to_string(),
                new: "HI".to_string(),
                all: false,
            },
        );
        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(replaced.content_hash.clone()),
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: patches,
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(outcome.modified_sections.patched, vec!["identity"]);
        let body = engine
            .get_entity(&seeded.id)
            .unwrap()
            .sections
            .get("identity")
            .unwrap()
            .clone();
        assert!(body.contains("HI world hello"), "first-only: {body:?}");
    }

    #[test]
    fn update_entity_patch_rejects_missing_old_substring() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Patch Miss");
        let (actor, client) = cli_actor();
        let mut patches = IndexMap::new();
        patches.insert(
            "identity".to_string(),
            crate::ops::PatchArg {
                old: "this-substring-does-not-exist".to_string(),
                new: "nope".to_string(),
                all: false,
            },
        );
        let err = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: patches,
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::PatchOldNotFound { section, .. } => {
                assert_eq!(section, "identity");
            }
            other => panic!("expected PatchOldNotFound, got {other:?}"),
        }
    }

    #[test]
    fn update_entity_appends_to_existing_section_with_newline_separator() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Append Subject");
        let (actor, client) = cli_actor();

        let mut appends = IndexMap::new();
        appends.insert("identity".to_string(), "appended tail.".to_string());

        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections: IndexMap::new(),
                    append_sections: appends,
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // modified_sections.appended carries the append key;
        // modified_sections.replaced stays empty.
        assert_eq!(outcome.modified_sections.appended, vec!["identity"]);
        assert!(outcome.modified_sections.replaced.is_empty());

        // The section body now contains the appended tail.
        let updated = engine.get_entity(&seeded.id).unwrap();
        let body = updated.sections.get("identity").expect("identity section");
        assert!(
            body.contains("appended tail."),
            "appended body missing: {body:?}"
        );
    }

    /// Item 02: `memstead_update` against a stub must surface a typed
    /// `StubNotUpdatable` envelope rather than the pre-fix
    /// `UnknownType { name: "" }` cascade. Mirrors the
    /// `StubCannotRelate` guard that `memstead_relate` already runs;
    /// before Item 02 the docstring list advertised the
    /// `STUB_NOT_UPDATABLE` code but no engine path emitted it.
    #[test]
    fn update_entity_against_stub_surfaces_typed_stub_not_updatable() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Source");
        let (actor, client) = cli_actor();
        // Materialise a stub by relating from a real entity to an
        // absent target. The relate path upserts the stub.
        let stub_id = crate::EntityId::new("specs", "stub-update-target");
        engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: stub_id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let err = engine
            .update_entity(
                UpdateEntityArgs {
                    id: stub_id.clone(),
                    expected_hash: Some(String::new()),
                    sections: IndexMap::from_iter([("identity".to_string(), "body".to_string())]),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::StubNotUpdatable { id } => assert_eq!(id, stub_id.to_string()),
            other => panic!("expected StubNotUpdatable, got {other:?}"),
        }
    }

    #[test]
    fn update_entity_rejects_conflicting_section_modes() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Conflict");
        let (actor, client) = cli_actor();

        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "replace".to_string());
        let mut appends = IndexMap::new();
        appends.insert("identity".to_string(), "append".to_string());

        let err = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections,
                    append_sections: appends,
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();

        match err {
            EngineError::ConflictingSectionModes { section, modes } => {
                assert_eq!(section, "identity");
                assert_eq!(modes, vec!["sections", "append_sections"]);
            }
            other => panic!("expected ConflictingSectionModes, got {other:?}"),
        }
    }

    #[test]
    fn update_entity_rejects_overlapping_metadata_and_metadata_unset_keys() {
        // Wire contract: setting and unsetting the same key is a hard
        // error. The check runs before the required-field check so the
        // resolution (pick one map) is unambiguous regardless of
        // whether the conflicting key is required.
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Overlap Subject");
        let (actor, client) = cli_actor();

        let mut metadata = IndexMap::new();
        // `tags` is a non-required field on the default `spec` schema —
        // so this conflict is purely about the overlap, not about
        // unsetting-a-required-field.
        metadata.insert("tags".to_string(), "foo".to_string());

        let err = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata,
                    metadata_unset: vec!["tags".to_string()],
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::SetAndUnsetConflict { keys } => {
                assert_eq!(keys, vec!["tags".to_string()]);
            }
            other => panic!("expected SetAndUnsetConflict, got {other:?}"),
        }
    }

    #[test]
    fn update_entity_pointer_schema_auto_synthesises_references_from_body_link() {
        // Under the default schema's `alias_target_rel_type: REFERENCES`
        // pointer, a body wiki-link no longer trips the strict validator
        // — the alias-synthesis pass emits the REFERENCES relation
        // first, the validator finds the link backed, the body lands.
        // (Schemas without the pointer continue to refuse with
        // `WIKILINK_WITHOUT_RELATION`; that path is covered by the
        // dedicated no-pointer fixture test elsewhere.)
        use crate::EntityId;
        use crate::engine::UpdateEntityArgs;
        use indexmap::IndexMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        engine.set_workspace_root(mem_dir.clone());
        let (actor, client) = cli_actor();

        let target = engine
            .create_entity(
                empty_create_args("specs", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let source = engine
            .create_entity(
                empty_create_args("specs", "Source"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert(
            "purpose".to_string(),
            "see [[target]] for context".to_string(),
        );
        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    id: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .expect("auto-synthesis must satisfy the alias-existence invariant");
        // Body landed.
        assert!(
            outcome
                .modified_sections
                .replaced
                .iter()
                .any(|s| s == "purpose"),
        );
        let in_mem = engine.get_entity(&source.id).unwrap();
        assert_eq!(
            in_mem
                .sections
                .get("purpose")
                .map(String::as_str)
                .unwrap_or(""),
            "see [[target]] for context",
        );
        // REFERENCES relation synthesised from the body wiki-link.
        assert!(
            in_mem
                .relationships
                .iter()
                .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
            "synthesis must emit REFERENCES → target; relationships: {:?}",
            in_mem.relationships,
        );
        // Defeat unused-import warnings for the helper imports.
        let _ = EntityId::new("specs", "x");
    }

    #[test]
    fn update_entity_declare_relations_passes_strict_validator_in_one_call() {
        // The agent declares the relation + adds the body wiki-link
        // in a single `memstead_update` call. Without
        // `declare_relations`, the strict validator would refuse
        // (no backing relation yet); with the batched declaration,
        // the relation lands *before* the strict validator runs so
        // the body link passes the gate.
        use crate::engine::UpdateEntityArgs;
        use crate::ops::RelateArg;
        use indexmap::IndexMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        engine.set_workspace_root(mem_dir.clone());
        let (actor, client) = cli_actor();

        let target = engine
            .create_entity(
                empty_create_args("specs", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let source = engine
            .create_entity(
                empty_create_args("specs", "Source"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Atomic declare + body update. USES (not REFERENCES) — under
        // the default schema's `alias_target_rel_type: REFERENCES`
        // pointer, explicit declare_relations type=REFERENCES is
        // refused; the body wiki-link is auto-emitted via synthesis.
        // The test's intent — that declare_relations atomically lands
        // alongside body changes — holds for any rel-type that admits
        // explicit authoring.
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert(
            "purpose".to_string(),
            "see [[target]] for context".to_string(),
        );
        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    relations_unset: Vec::new(),
                    id: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    dry_run: false,
                    declare_relations: vec![RelateArg {
                        rel_type: "USES".to_string(),
                        to: target.id.clone(),
                        description: None,
                    }],
                },
                actor,
                Some(&client),
                None,
            )
            .expect("declare_relations + body update must succeed in one call");

        assert_eq!(outcome.relations_declared.len(), 1);
        assert_eq!(outcome.relations_declared[0].rel_type, "USES");
        assert_eq!(outcome.relations_declared[0].target, target.id);
        assert!(
            !outcome.relations_declared[0].target_was_stubbed,
            "target was already present in store; target_was_stubbed must be false"
        );

        let in_mem = engine.get_entity(&source.id).unwrap();
        assert!(
            in_mem.relationships.iter().any(|r| r.target == target.id),
            "declared relation must land in entity.relationships; got {:?}",
            in_mem.relationships
        );
    }

    #[test]
    fn update_entity_declare_relations_auto_stubs_absent_target() {
        // When the declared target doesn't exist yet, the engine
        // auto-stubs it (same mechanic as `memstead_relate`) and flags
        // `target_was_stubbed: true` in the outcome.
        use crate::EntityId;
        use crate::engine::UpdateEntityArgs;
        use crate::ops::RelateArg;
        use indexmap::IndexMap;

        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Source");
        let (actor, client) = cli_actor();
        let absent_target = EntityId::new("specs", "not-yet-existing");
        assert!(!engine.store().contains(&absent_target));

        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    relations_unset: Vec::new(),
                    id: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    dry_run: false,
                    declare_relations: vec![RelateArg {
                        rel_type: "USES".to_string(),
                        to: absent_target.clone(),
                        description: None,
                    }],
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        assert_eq!(outcome.relations_declared.len(), 1);
        assert!(
            outcome.relations_declared[0].target_was_stubbed,
            "absent target must be auto-stubbed; got target_was_stubbed=false"
        );
        // Stub now exists in the store.
        assert!(engine.store().contains(&absent_target));
        let stub = engine.get_entity(&absent_target).unwrap();
        assert!(stub.stub);
    }

    #[test]
    fn update_entity_alias_synthesis_runs_unconditionally_for_pointer_schemas() {
        // Under the alias model with a pointer-set schema (default
        // schema's `alias_target_rel_type: REFERENCES`), a fresh
        // workspace's first body-wiki-link write triggers the
        // alias-synthesis pass and the mutation lands with the
        // REFERENCES relation auto-emitted.
        use crate::engine::UpdateEntityArgs;
        use indexmap::IndexMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        engine.set_workspace_root(mem_dir.clone());
        let (actor, client) = cli_actor();
        let target = engine
            .create_entity(
                empty_create_args("specs", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let source = engine
            .create_entity(
                empty_create_args("specs", "Source"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert(
            "purpose".to_string(),
            "see [[target]] for context".to_string(),
        );
        engine
            .update_entity(
                UpdateEntityArgs {
                    id: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .expect("synthesis must back the wiki-link and let the body land");
        let in_mem = engine.get_entity(&source.id).unwrap();
        assert!(
            in_mem
                .relationships
                .iter()
                .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
            "synthesis must emit REFERENCES → target; relationships: {:?}",
            in_mem.relationships,
        );
    }

    #[test]
    fn update_entity_dry_run_returns_prospective_hash_without_writing() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Preview Subject");
        let (actor, client) = cli_actor();
        let original_hash = seeded.content_hash.clone();

        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "preview body".to_string());

        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    // Stale-hash recovery path — dry_run skips the
                    // hash check, so a wrong expected_hash is OK.
                    expected_hash: Some("wrong-hash".to_string()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: true,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Wire shape: content_hash = current; prospective_hash =
        // what the write would produce; commit_sha empty.
        assert_eq!(outcome.content_hash, original_hash);
        let prospective = outcome
            .prospective_hash
            .expect("prospective_hash populated on dry_run");
        assert_ne!(prospective, original_hash);
        assert!(outcome.commit_sha.is_empty());
        // Store entity unchanged.
        let store_entity = engine.get_entity(&seeded.id).unwrap();
        assert_eq!(store_entity.content_hash, original_hash);
    }

    /// Edge-count round-trip lock. Captures the REFERENCES-drift
    /// finding:
    /// a mutation cycle (create body wiki-links → relate → update
    /// body → rename → delete) must return both `total_edges` and the
    /// REFERENCES counter to the pre-cycle values exactly. The bug
    /// was in `push_entities_into_store`: `upsert` preserved the
    /// entity's pre-existing out-edges, so `add_edge` (idempotent on
    /// `(from, to, rel_type)`) couldn't remove edges that the new
    /// parse no longer emits. Dropping a wiki-link from a body or
    /// absorbing one into an explicit relationship leaked the stale
    /// REFERENCES edge.
    ///
    /// Under the alias model the leak is structurally impossible —
    /// body wiki-links no longer emit edges, so the cleanup-on-reparse
    /// path the original test exercised has no premise. The test
    /// keeps the CRUD cycle but routes edges through atomic
    /// `relations:` declarations and explicit `memstead_relate`, which is
    /// what the model now treats as the only edge source.
    #[test]
    fn references_edges_round_trip_across_full_crud_cycle() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        // Seed two link targets so the wiki-links inside the
        // probe entity's body resolve to real entities (not auto-
        // stubs we'd then have to GC).
        let foo = engine
            .create_entity(
                empty_create_args("specs", "Foo"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let bar = engine
            .create_entity(
                empty_create_args("specs", "Bar"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let count_references = |engine: &Engine| -> usize {
            engine
                .store()
                .all_ids()
                .flat_map(|id| engine.store().outgoing(id))
                .filter(|e| e.rel_type == "REFERENCES")
                .count()
        };

        let baseline_edges = engine.store().edge_count();
        let baseline_refs = count_references(&engine);

        // Step 1: create entity with body wiki-links — the
        // alias-synthesis pass auto-emits one REFERENCES per body
        // wiki-link (default schema's `alias_target_rel_type` →
        // REFERENCES), so the explicit `relations:` slot stays
        // empty. Net: 2 REFERENCES.
        let mut sections = IndexMap::new();
        sections.insert(
            "identity".to_string(),
            "See [[foo]] and [[bar]] inline.".to_string(),
        );
        sections.insert("purpose".to_string(), "probe purpose".to_string());
        let probe = engine
            .create_entity(
                CreateEntityArgs {
                    mem: "specs".to_string(),
                    title: "Probe".to_string(),
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
        assert_eq!(count_references(&engine), baseline_refs + 2);

        // Step 2: relate INFORMED_BY → foo as a second relation to the
        // same target. The body wiki-link `[[foo]]` aliases the set of
        // relations to foo, so adding INFORMED_BY does not affect the
        // REFERENCES count — both relations coexist.
        let relate1 = engine
            .relate_entity(
                RelateEntityArgs {
                    source: probe.id.clone(),
                    expected_hash: Some(probe.content_hash.clone()),
                    rel_type: "INFORMED_BY".to_string(),
                    target: foo.id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(
            count_references(&engine),
            baseline_refs + 2,
            "set-membership aliasing — adding INFORMED_BY does not \
             absorb the REFERENCES relation"
        );

        // Step 3: drop the [[bar]] body link. The alias-synthesis pass
        // GCs the synthesised REFERENCES → bar atomically with the
        // body update — no second `memstead_relate --remove` needed.
        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "See [[foo]] inline.".to_string());
        let updated = engine
            .update_entity(
                UpdateEntityArgs {
                    id: probe.id.clone(),
                    expected_hash: Some(relate1.content_hash.clone()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(
            count_references(&engine),
            baseline_refs + 1,
            "REFERENCES → bar must be auto-GC'd when its body link drops"
        );

        // Step 4: rename the entity. Edges follow via remove + push.
        let renamed = engine
            .rename_entity(
                crate::engine::RenameEntityArgs {
                    id: probe.id.clone(),
                    expected_hash: Some(updated.content_hash.clone()),
                    new_title: "Probe Renamed".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(count_references(&engine), baseline_refs + 1);

        // Step 5: delete the renamed entity. The INFORMED_BY → foo
        // edge cascades; REFERENCES count unchanged.
        engine
            .delete_entity(
                crate::engine::DeleteEntityArgs {
                    id: renamed.new_id.clone(),
                    expected_hash: Some(renamed.content_hash.clone()),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Final assertion: every counter back to baseline.
        assert_eq!(
            engine.store().edge_count(),
            baseline_edges,
            "total edges must round-trip to baseline"
        );
        assert_eq!(
            count_references(&engine),
            baseline_refs,
            "REFERENCES counter must round-trip to baseline"
        );

        // Cross-check: a full reload of the mem produces the same
        // post-cycle counts. If the in-memory store and the on-disk
        // bytes drift, reload uncovers it.
        engine.reload_one_mem("specs").unwrap();
        assert_eq!(
            engine.store().edge_count(),
            baseline_edges,
            "total edges must match disk after reload"
        );
        assert_eq!(
            count_references(&engine),
            baseline_refs,
            "REFERENCES must match disk after reload"
        );
        // Sanity: foo + bar still in the store (they were not deleted).
        assert!(engine.store().contains(&foo.id));
        assert!(engine.store().contains(&bar.id));
    }

    #[test]
    fn update_entity_returns_commit_sha_title_modified_date_warnings_shape() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Subject");
        let (actor, client) = cli_actor();

        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "edited body".to_string());

        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Folder backend produces a synthetic CommitId.
        assert!(
            !outcome.commit_sha.is_empty(),
            "commit_sha must be populated on a real update"
        );
        // Title echoed from the parsed entity post-write.
        assert_eq!(outcome.title, "Subject");
        // The default `spec` schema declares `modified_date` with
        // `auto_timestamp: true`; the unified update path
        // auto-stamps it. Asserting non-empty pins the
        // wire-shape parity with full's UpdateResult.modified_date.
        assert!(
            !outcome.modified_date.is_empty(),
            "modified_date must be auto-stamped on update for the default spec schema",
        );
        // V1: warnings always empty (typed warning surfaces are
        // separate session work). The vec is present on the outcome
        // so the wire shape parity with full's UpdateResult holds.
        assert!(outcome.warnings.is_empty());
        // Section was modified (existing behaviour, sanity check).
        assert_eq!(
            outcome.modified_sections.replaced,
            vec!["identity".to_string()]
        );
    }

    // ---- Engine::update_entity no-op detection ---------------------

    /// Re-setting a
    /// section to its current on-disk value short-circuits to
    /// `UPDATE_NOOP` and preserves `last_modified` at its pre-call
    /// value. Pre-fix the auto-timestamp stamped `last_modified` to
    /// `today_iso()` before the bytes-compare ran; the stamp
    /// synthesised a delta and the no-op never matched.
    #[test]
    fn update_entity_noop_resetting_section_to_current_value_preserves_last_modified() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Section Resetter");
        let (actor, client) = cli_actor();

        // Read the pre-update `last_modified` so we can assert it
        // survives the no-op.
        let pre_last_modified = engine
            .get_entity(&seeded.id)
            .and_then(|e| e.metadata.get("last_modified"))
            .map(|v| v.to_frontmatter_string())
            .expect("seeded entity has last_modified");

        // Re-set `identity` to its current on-disk body. The seed
        // helper writes "fixture identity body" — passing the same
        // string back must be a no-op.
        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "fixture identity body".to_string());
        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        assert_eq!(outcome.commit_sha, "", "no-op must not commit");
        assert_eq!(
            outcome.content_hash, seeded.content_hash,
            "no-op must not advance content_hash",
        );
        assert!(
            outcome.warnings.iter().any(|w| w.code() == "UPDATE_NOOP"),
            "UPDATE_NOOP must fire on bytes-identical re-set",
        );
        assert_eq!(
            outcome.modified_date, pre_last_modified,
            "no-op must preserve last_modified at the pre-call value",
        );
        // The applied delta is empty on a
        // no-op — `modified_sections` must not claim `identity` was
        // replaced when nothing landed (matching the empty commit_sha
        // and unchanged hash above).
        assert!(
            outcome.modified_sections.replaced.is_empty()
                && outcome.modified_sections.appended.is_empty()
                && outcome.modified_sections.patched.is_empty(),
            "no-op must report an empty section delta, got {:?}",
            outcome.modified_sections,
        );

        // The on-disk entity also still carries the pre-update
        // last_modified — the no-op didn't bump it through some
        // other path.
        let post_last_modified = engine
            .get_entity(&seeded.id)
            .and_then(|e| e.metadata.get("last_modified"))
            .map(|v| v.to_frontmatter_string())
            .expect("entity still in store");
        assert_eq!(post_last_modified, pre_last_modified);
    }

    /// A payload with no
    /// recognised mutation content refuses with `EMPTY_UPDATE` BEFORE
    /// any engine work runs. Previously this same input short-
    /// circuited as a success-with-`UPDATE_NOOP`-warning, which
    /// hid a boundary-discipline failure mode (a
    /// misspelled mutation key deserialises to empty defaults and
    /// looks like a no-op success on the wire). The new refusal
    /// makes "no mutation content provided" structurally distinct
    /// from "mutation content provided but matched current state"
    /// (which still surfaces `UPDATE_NOOP` — see
    /// `update_entity_noop_same_content_surfaces_warning` below).
    #[test]
    fn update_entity_empty_payload_refuses_with_typed_code() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Empty Payload");
        let (actor, client) = cli_actor();

        let err = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::EmptyUpdate { id } => {
                assert_eq!(id, seeded.id.to_string());
            }
            other => panic!("expected EMPTY_UPDATE, got {other:?}"),
        }
        // No provenance row landed — the refusal preempts any write.
        let log_path = tmp.path().join(".memstead/changes.jsonl");
        if let Ok(log) = std::fs::read_to_string(&log_path) {
            let updates = log.matches("\"kind\":\"update\"").count();
            assert_eq!(updates, 0, "EMPTY_UPDATE refusal must not log an update");
        }
    }

    /// Complement: a
    /// payload with mutation content that matches the current entity
    /// state continues to land as success-with-`UPDATE_NOOP`-warning.
    /// The new `EMPTY_UPDATE` refusal applies only when no mutation
    /// content was provided; this path is structurally distinct.
    #[test]
    fn update_entity_noop_same_content_surfaces_warning() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Same Content Noop");
        let (actor, client) = cli_actor();

        // `empty_create_args` seeds `identity` with this exact body.
        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "fixture identity body".to_string());

        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        assert_eq!(outcome.commit_sha, "");
        assert_eq!(outcome.content_hash, seeded.content_hash);
        let codes: Vec<&str> = outcome.warnings.iter().map(|w| w.code()).collect();
        assert!(
            codes.contains(&"UPDATE_NOOP"),
            "same-content update must surface UPDATE_NOOP; got {codes:?}",
        );
    }

    #[test]
    fn update_entity_noop_metadata_unset_on_absent_key() {
        // `metadata_unset=["never-set-key"]`
        // where the key was never set is a no-op — no field actually
        // changed, no commit advances, follow-up calls can chain
        // `expected_hash` without `HASH_MISMATCH`.
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Absent Key Noop");
        let (actor, client) = cli_actor();

        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    // `tags` is declared on the `spec` schema but
                    // unset on the seeded entity. Unsetting it should
                    // be a no-op rather than producing a fresh commit.
                    metadata_unset: vec!["tags".to_string()],
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        assert_eq!(outcome.commit_sha, "");
        assert_eq!(outcome.content_hash, seeded.content_hash);
        assert!(
            outcome.warnings.iter().any(|w| w.code() == "UPDATE_NOOP"),
            "absent-key metadata_unset must surface UPDATE_NOOP",
        );
        // Empty applied delta on the no-op —
        // `unset` must not claim `tags` was removed when nothing landed.
        assert!(
            outcome.modified_metadata.set.is_empty() && outcome.modified_metadata.unset.is_empty(),
            "no-op must report an empty metadata delta, got {:?}",
            outcome.modified_metadata,
        );

        // Follow-up real change against the unchanged hash succeeds —
        // no HASH_MISMATCH cascade from a phantom advance.
        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "real change".to_string());
        let real = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(!real.commit_sha.is_empty());
        assert_ne!(real.content_hash, seeded.content_hash);
    }

    /// The exact MCP repro — re-setting a
    /// metadata key to its current value no-ops, and the response's
    /// `modified_metadata` reports the applied delta (empty), not the
    /// requested key. Pre-fix the no-op short-circuit echoed
    /// `set: ["level"]` while `commit_sha` was empty and the hash
    /// unchanged — a self-contradictory response.
    #[test]
    fn update_entity_noop_setting_metadata_to_current_value_reports_empty_delta() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Stability Resetter");
        let (actor, client) = cli_actor();

        // `level` defaults to "M0" on the spec schema, so the seed
        // carries it. Re-setting it to "M0" changes nothing.
        let mut metadata = IndexMap::new();
        metadata.insert("level".to_string(), "M0".to_string());
        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata,
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        assert_eq!(outcome.commit_sha, "", "no-op must not commit");
        assert_eq!(
            outcome.content_hash, seeded.content_hash,
            "no-op must not advance hash"
        );
        assert!(
            outcome.warnings.iter().any(|w| w.code() == "UPDATE_NOOP"),
            "re-set to current value must surface UPDATE_NOOP",
        );
        assert!(
            outcome.modified_metadata.set.is_empty() && outcome.modified_metadata.unset.is_empty(),
            "no-op must not claim `level` was set — applied delta is empty, got {:?}",
            outcome.modified_metadata,
        );
    }

    #[test]
    fn update_entity_noop_declare_already_related_edge() {
        // Re-declare an already-related edge
        // via `declare_relations` — no field, section, metadata or
        // relations list actually changes, so the bytes are identical
        // and the call no-ops.
        use crate::ops::RelateArg;
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        let target = engine
            .create_entity(
                empty_create_args("specs", "Target Already Related"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let source = engine
            .create_entity(
                empty_create_args("specs", "Source Already Related"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let after_relate = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        // Now re-declare the same edge via update.declare_relations.
        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    relations_unset: Vec::new(),
                    id: source.id.clone(),
                    expected_hash: Some(after_relate.content_hash.clone()),
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: vec![RelateArg {
                        rel_type: "USES".to_string(),
                        to: target.id.clone(),
                        description: None,
                    }],
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        assert_eq!(outcome.commit_sha, "");
        assert_eq!(outcome.content_hash, after_relate.content_hash);
        assert!(
            outcome.warnings.iter().any(|w| w.code() == "UPDATE_NOOP"),
            "duplicate declare must surface UPDATE_NOOP",
        );
        // `relations_declared` still records the entry — the per-
        // relation outcome is part of the surface, even for no-ops.
        assert_eq!(outcome.relations_declared.len(), 1);
        assert_eq!(outcome.relations_declared[0].rel_type, "USES");
        assert_eq!(outcome.relations_declared[0].target, target.id);
        assert!(!outcome.relations_declared[0].target_was_stubbed);
    }

    #[test]
    fn update_entity_real_change_still_commits_and_advances_hash() {
        // Regression: the no-op short-circuit must not short-circuit
        // real changes. A section replacement still produces a
        // non-empty `commit_sha`, advances `content_hash`, and does
        // NOT surface UPDATE_NOOP.
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Real Change Subject");
        let (actor, client) = cli_actor();

        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "definitely new body".to_string());

        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        assert!(!outcome.commit_sha.is_empty(), "real change must commit");
        assert_ne!(
            outcome.content_hash, seeded.content_hash,
            "real change must advance content_hash",
        );
        assert!(
            !outcome.warnings.iter().any(|w| w.code() == "UPDATE_NOOP"),
            "real change must not surface UPDATE_NOOP",
        );
    }

    #[test]
    fn update_entity_noop_preserves_expected_hash_across_chain() {
        // A follow-up update with the original
        // hash after one or more no-ops succeeds because the hash
        // never advanced. Demonstrates the `expected_hash`-caching
        // posture the agent surface relies on.
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Chained Noops Subject");
        let (actor, client) = cli_actor();

        // Two no-op calls in a row — both must return the same hash.
        // Pass same-content mutation
        // so UPDATE_NOOP fires (rather than EMPTY_UPDATE) and the
        // hash-chain invariant is exercised on the warning path.
        let mut noop_sections = IndexMap::new();
        noop_sections.insert("identity".to_string(), "fixture identity body".to_string());
        for _ in 0..2 {
            let outcome = engine
                .update_entity(
                    UpdateEntityArgs {
                        id: seeded.id.clone(),
                        expected_hash: Some(seeded.content_hash.clone()),
                        sections: noop_sections.clone(),
                        append_sections: IndexMap::new(),
                        patch_sections: IndexMap::new(),
                        metadata: IndexMap::new(),
                        metadata_unset: Vec::new(),
                        declare_relations: Vec::new(),
                        dry_run: false,
                        relations_unset: Vec::new(),
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap();
            assert_eq!(outcome.commit_sha, "");
            assert_eq!(outcome.content_hash, seeded.content_hash);
        }

        // Real follow-up with the original hash still works — no
        // HASH_MISMATCH cascade because the hash never advanced.
        let mut sections = IndexMap::new();
        sections.insert(
            "identity".to_string(),
            "third call: real change".to_string(),
        );
        let real = engine
            .update_entity(
                UpdateEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(!real.commit_sha.is_empty());
        assert_ne!(real.content_hash, seeded.content_hash);
    }

    // ---- Engine::delete_entity --------------------------------------

    // ---------------------------------------------------------------------
    // Alias-synthesis pass. Body wiki-links auto-emit
    // relations of the source schema's `alias_target_rel_type` pointer
    // and are garbage-collected when the body wiki-link disappears.
    // ---------------------------------------------------------------------

    #[test]
    fn synthesis_gc_drops_auto_emitted_reference_when_body_link_removed() {
        // Create with `[[target]]` in body → synthesis emits
        // REFERENCES. Update body to drop the wiki-link → GC drops
        // the auto-emitted REFERENCES.
        use crate::engine::{CreateEntityArgs, UpdateEntityArgs};
        use indexmap::IndexMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        engine.set_workspace_root(mem_dir.clone());
        let (actor, client) = cli_actor();

        let target = engine
            .create_entity(
                empty_create_args("specs", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        // Create source with the body wiki-link already present —
        // synthesis fires inside create.
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("identity".to_string(), "source identity".to_string());
        sections.insert(
            "purpose".to_string(),
            "see [[target]] for context".to_string(),
        );
        let source = engine
            .create_entity(
                CreateEntityArgs {
                    mem: "specs".to_string(),
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
        assert!(
            engine
                .get_entity(&source.id)
                .unwrap()
                .relationships
                .iter()
                .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
            "create-time synthesis must emit REFERENCES → target",
        );

        // Update: drop the body wiki-link. GC should remove the
        // synthesised REFERENCES.
        let mut new_sections: IndexMap<String, String> = IndexMap::new();
        new_sections.insert("purpose".to_string(), "no link any more".to_string());
        engine
            .update_entity(
                UpdateEntityArgs {
                    id: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    sections: new_sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .expect("update must succeed; GC drops the now-orphan REFERENCES");
        let in_mem = engine.get_entity(&source.id).unwrap();
        assert!(
            !in_mem
                .relationships
                .iter()
                .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
            "GC must drop the auto-emitted REFERENCES after body link removal; got {:?}",
            in_mem.relationships,
        );
    }

    #[test]
    fn update_gc_removes_orphan_stub_when_last_body_link_dropped() {
        // Create source with `[[ghost]]` body link → alias synthesis
        // auto-stubs `ghost` and emits REFERENCES → ghost. The update
        // drops the link, so the REFERENCES edge (the stub's only
        // referrer) disappears; the orphan-stub GC sweep removes the
        // stub and surfaces it in `orphan_stubs_removed`. A reload from
        // disk shows the same (decremented) stub count — proving the GC
        // was a real store mutation, not a session-local view fix.
        use crate::engine::{CreateEntityArgs, UpdateEntityArgs};
        use indexmap::IndexMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        engine.set_workspace_root(mem_dir.clone());
        let (actor, client) = cli_actor();

        let ghost = crate::EntityId::new("specs", "ghost");
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("identity".to_string(), "source identity".to_string());
        sections.insert(
            "purpose".to_string(),
            "see [[ghost]] for context".to_string(),
        );
        let source = engine
            .create_entity(
                CreateEntityArgs {
                    mem: "specs".to_string(),
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
        assert!(
            engine.store().contains(&ghost) && engine.get_entity(&ghost).unwrap().stub,
            "body wiki-link to an absent target must auto-stub it",
        );
        assert_eq!(
            engine.health().stub_count,
            1,
            "one stub before the link drop"
        );

        let mut new_sections: IndexMap<String, String> = IndexMap::new();
        new_sections.insert("purpose".to_string(), "no link any more".to_string());
        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    id: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    sections: new_sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .expect("update must succeed and GC the now-orphan stub");

        assert_eq!(
            outcome.orphan_stubs_removed,
            vec![ghost.clone()],
            "the update that dropped the last body link must report the GC'd stub",
        );
        assert!(
            !engine.store().contains(&ghost),
            "orphan stub must be gone from the in-memory store",
        );
        assert_eq!(
            engine.health().stub_count,
            0,
            "stub count decremented in-session"
        );

        // Reload from disk: the source's on-disk markdown no longer
        // carries the link, so the parser re-emits no stub. The
        // decremented count holds across the reload — the GC was real.
        engine.reload_each_writable_mem().unwrap();
        assert!(
            !engine.store().contains(&ghost),
            "stub stays gone after reload-from-disk",
        );
        assert_eq!(
            engine.health().stub_count,
            0,
            "reloaded-from-disk store carries the same stub count as the in-session post-update state",
        );
    }

    #[test]
    fn update_gc_noop_when_section_edit_changes_no_body_link() {
        // An update that edits one section while leaving the `[[ghost]]`
        // link standing in another orphans nothing: `orphan_stubs_removed`
        // is present and empty (stable shape, no spurious GC), and the
        // stub survives because its referrer survives.
        use crate::engine::{CreateEntityArgs, UpdateEntityArgs};
        use indexmap::IndexMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        engine.set_workspace_root(mem_dir.clone());
        let (actor, client) = cli_actor();

        let ghost = crate::EntityId::new("specs", "ghost");
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("identity".to_string(), "original identity".to_string());
        sections.insert(
            "purpose".to_string(),
            "see [[ghost]] for context".to_string(),
        );
        let source = engine
            .create_entity(
                CreateEntityArgs {
                    mem: "specs".to_string(),
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
        assert!(engine.store().contains(&ghost), "ghost stub materialised");

        // Replace `identity` only; the `[[ghost]]` link in `purpose`
        // stays, so no edge drops.
        let mut edit: IndexMap<String, String> = IndexMap::new();
        edit.insert("identity".to_string(), "edited identity".to_string());
        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    id: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    sections: edit,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .expect("update must succeed");
        assert!(
            outcome.orphan_stubs_removed.is_empty(),
            "an edit that keeps every body wiki-link orphans nothing; got {:?}",
            outcome.orphan_stubs_removed,
        );
        assert!(
            engine.store().contains(&ghost),
            "the still-referenced stub survives the unrelated section edit",
        );
    }

    #[test]
    fn update_gc_preserves_stub_with_surviving_referrer() {
        // Two sources both body-link `[[ghost]]`. Dropping the link from
        // one leaves `ghost` referenced by the other — set-membership
        // semantics keep the stub alive and `orphan_stubs_removed` empty.
        use crate::engine::{CreateEntityArgs, UpdateEntityArgs};
        use indexmap::IndexMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        engine.set_workspace_root(mem_dir.clone());
        let (actor, client) = cli_actor();

        let ghost = crate::EntityId::new("specs", "ghost");
        let make_with_link = |title: &str| {
            let mut sections: IndexMap<String, String> = IndexMap::new();
            sections.insert("identity".to_string(), format!("{title} identity"));
            sections.insert("purpose".to_string(), "see [[ghost]]".to_string());
            CreateEntityArgs {
                mem: "specs".to_string(),
                title: title.to_string(),
                entity_type: "spec".to_string(),
                sections,
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            }
        };
        let source_a = engine
            .create_entity(make_with_link("Source A"), actor, Some(&client), None)
            .unwrap();
        engine
            .create_entity(make_with_link("Source B"), actor, Some(&client), None)
            .unwrap();
        assert!(engine.store().contains(&ghost), "ghost stub materialised");

        // Drop the link from source A only.
        let mut drop_link: IndexMap<String, String> = IndexMap::new();
        drop_link.insert("purpose".to_string(), "no link here".to_string());
        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    id: source_a.id.clone(),
                    expected_hash: Some(source_a.content_hash.clone()),
                    sections: drop_link,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .expect("update must succeed");
        assert!(
            outcome.orphan_stubs_removed.is_empty(),
            "the stub keeps a referrer (source B), so nothing is GC'd; got {:?}",
            outcome.orphan_stubs_removed,
        );
        assert!(
            engine.store().contains(&ghost),
            "stub survives via the surviving referrer",
        );
    }

    #[test]
    fn synthesis_gc_preserves_non_pointer_explicit_relation_across_body_update() {
        // Explicit USES to target (USES is not the schema's
        // alias_target_rel_type pointer). A subsequent body-changing
        // update must NOT drop the USES edge — GC only touches
        // relations of the pointer rel-type. Under Option C, REFERENCES
        // can't be authored explicitly (`manual_authoring: forbidden`),
        // so the analogous "explicit REFERENCES preserved" scenario is
        // structurally impossible; USES exercises the same invariant
        // from the rel-type-discrimination side.
        use crate::engine::{RelateEntityArgs, UpdateEntityArgs};
        use indexmap::IndexMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        engine.set_workspace_root(mem_dir.clone());
        let (actor, client) = cli_actor();

        let target = engine
            .create_entity(
                empty_create_args("specs", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let source = engine
            .create_entity(
                empty_create_args("specs", "Source"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Explicit relate — no body wiki-link.
        let relate = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Update an unrelated section. The explicit USES must survive
        // — it's not the alias_target_rel_type, GC ignores it.
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("purpose".to_string(), "unrelated edit".to_string());
        engine
            .update_entity(
                UpdateEntityArgs {
                    id: source.id.clone(),
                    expected_hash: Some(relate.content_hash.clone()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .expect("update must succeed");
        let in_mem = engine.get_entity(&source.id).unwrap();
        assert!(
            in_mem
                .relationships
                .iter()
                .any(|r| r.rel_type == "USES" && r.target == target.id),
            "explicit USES must survive an unrelated body update; got {:?}",
            in_mem.relationships,
        );
    }

    #[test]
    fn synthesis_dedupes_repeated_body_links_to_same_target() {
        // Two `[[target]]` wiki-links in one body — synthesis must
        // not double-add. Result: exactly one REFERENCES.
        use crate::engine::UpdateEntityArgs;
        use indexmap::IndexMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        engine.set_workspace_root(mem_dir.clone());
        let (actor, client) = cli_actor();

        let target = engine
            .create_entity(
                empty_create_args("specs", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let source = engine
            .create_entity(
                empty_create_args("specs", "Source"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert(
            "purpose".to_string(),
            "see [[target]] and again [[target]]".to_string(),
        );
        engine
            .update_entity(
                UpdateEntityArgs {
                    id: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let in_mem = engine.get_entity(&source.id).unwrap();
        let count = in_mem
            .relationships
            .iter()
            .filter(|r| r.rel_type == "REFERENCES" && r.target == target.id)
            .count();
        assert_eq!(
            count, 1,
            "dedupe must leave exactly one REFERENCES → target; got {:?}",
            in_mem.relationships,
        );
    }

    #[test]
    fn synthesis_coexists_with_explicit_uses_to_same_target() {
        // Explicit `USES` to target AND body wiki-link to target →
        // entity carries both USES and REFERENCES edges; synthesis
        // dedupes on `(rel_type, target)` so USES never suppresses
        // REFERENCES.
        use crate::engine::{RelateEntityArgs, UpdateEntityArgs};
        use indexmap::IndexMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        engine.set_workspace_root(mem_dir.clone());
        let (actor, client) = cli_actor();

        let target = engine
            .create_entity(
                empty_create_args("specs", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let source = engine
            .create_entity(
                empty_create_args("specs", "Source"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        // Explicit USES.
        let relate = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        // Body wiki-link to the same target — synthesis emits REFERENCES.
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert(
            "purpose".to_string(),
            "we also reference [[target]]".to_string(),
        );
        engine
            .update_entity(
                UpdateEntityArgs {
                    id: source.id.clone(),
                    expected_hash: Some(relate.content_hash.clone()),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let in_mem = engine.get_entity(&source.id).unwrap();
        assert!(
            in_mem
                .relationships
                .iter()
                .any(|r| r.rel_type == "USES" && r.target == target.id),
            "USES must survive — synthesis dedupes on (rel_type, target)",
        );
        assert!(
            in_mem
                .relationships
                .iter()
                .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
            "REFERENCES must be synthesised even though USES already targets the same entity",
        );
    }

    // ---------------------------------------------------------------------
    // Alias-synthesis integration tests against custom-schema engines.
    // The default schema pins `alias_target_rel_type: REFERENCES`; these
    // tests verify the engine doesn't hardcode that name by mounting
    // schemas with a non-REFERENCES pointer (proves name-agnosticism)
    // and schemas with no pointer at all (proves the strict
    // `WIKILINK_WITHOUT_RELATION` refusal still fires for opt-out
    // schemas). Both build the engine via `from_mounts_with_schemas_dir`
    // — the production path for workspace-authored schemas.
    // ---------------------------------------------------------------------

    mod alias_synthesis_custom_schema {
        use std::path::Path;

        use indexmap::IndexMap;
        use memstead_schema::SchemaRef;
        use tempfile::TempDir;

        use crate::backend::MemBackend;
        use crate::engine::test_helpers::*;
        use crate::engine::{CreateEntityArgs, Engine, EngineError, UpdateEntityArgs};
        use crate::storage::FilesystemMemWriter;
        use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

        const TYPE_BODY: &str = r#"description: t
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
hierarchy_relationship: _default
propagating_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;

        fn write_schema_files(root: &Path, name: &str, manifest: &str, types: &[(&str, &str)]) {
            let dir = root.join(name);
            std::fs::create_dir_all(dir.join("types")).unwrap();
            std::fs::write(dir.join("schema.yaml"), manifest).unwrap();
            for (type_name, body) in types {
                std::fs::write(dir.join("types").join(format!("{type_name}.yaml")), body).unwrap();
            }
        }

        fn make_type_yaml(name: &str) -> String {
            format!("name: {name}\n{TYPE_BODY}")
        }

        fn folder_mount_with_pin(mem: &str, path: std::path::PathBuf, pin: SchemaRef) -> Mount {
            Mount {
                mem: mem.to_string(),
                schema: Some(pin),
                storage: MountStorage::Folder { path },
                capability: MountCapability::Write,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            }
        }

        fn engine_with_schema(
            manifest: &str,
            type_yaml_name: &str,
            schema_name: &str,
            schema_version: semver::Version,
        ) -> (Engine, TempDir) {
            let tmp = TempDir::new().unwrap();
            let schemas_dir = tmp.path().join("schemas");
            std::fs::create_dir_all(&schemas_dir).unwrap();
            write_schema_files(
                &schemas_dir,
                schema_name,
                manifest,
                &[(type_yaml_name, &make_type_yaml(type_yaml_name))],
            );
            let mem_dir = tmp.path().join("mem");
            std::fs::create_dir_all(&mem_dir).unwrap();
            let writer = FilesystemMemWriter::new(mem_dir.clone());
            let pin = SchemaRef::new(schema_name, schema_version);
            let mount = folder_mount_with_pin("v", mem_dir, pin);
            let mut engine = Engine::from_mounts_with_schemas_dir(
                vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
                Some(&schemas_dir),
            )
            .expect("engine with custom schema constructs");
            engine.set_workspace_root(tmp.path().to_path_buf());
            (engine, tmp)
        }

        #[test]
        fn non_references_alias_pointer_emits_named_rel_type_from_body_link() {
            // Schema names CITES as the alias pointer — the engine
            // must emit CITES (not REFERENCES) from a body wiki-link.
            // Proves no hard-coded "REFERENCES" string anywhere in
            // the synthesis path.
            let manifest = r#"name: aliased
version: 0.1.0
description: alias-synthesis fixture using a non-REFERENCES pointer
when_to_use: tests prove the engine does not hard-code REFERENCES
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: CITES
      description: Citation — auto-emitted from body wiki-links
      default_weight: 0.5
    - name: PART_OF
      description: Hierarchy
      default_weight: 3.0
      acyclic: true
    - name: _default
      description: Fallback
      default_weight: 1.0
alias_target_rel_type: CITES
community:
  resolution: 1.0
  seed: 42
"#;
            let (mut engine, _tmp) =
                engine_with_schema(manifest, "doc", "aliased", semver::Version::new(0, 1, 0));
            let (actor, client) = cli_actor();

            let target = engine
                .create_entity(
                    CreateEntityArgs {
                        mem: "v".to_string(),
                        title: "Target".to_string(),
                        entity_type: "doc".to_string(),
                        sections: IndexMap::from_iter([(
                            "body".to_string(),
                            "target body".to_string(),
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

            let mut sections: IndexMap<String, String> = IndexMap::new();
            sections.insert("body".to_string(), "see [[target]]".to_string());
            let source = engine
                .create_entity(
                    CreateEntityArgs {
                        mem: "v".to_string(),
                        title: "Source".to_string(),
                        entity_type: "doc".to_string(),
                        sections,
                        metadata: IndexMap::new(),
                        relations: Vec::new(),
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .expect("create must succeed; CITES is auto-emitted by synthesis");

            let in_mem = engine.get_entity(&source.id).unwrap();
            assert!(
                in_mem
                    .relationships
                    .iter()
                    .any(|r| r.rel_type == "CITES" && r.target == target.id),
                "synthesis must emit CITES (the pointer rel-type), not REFERENCES; got {:?}",
                in_mem.relationships,
            );
            assert!(
                !in_mem
                    .relationships
                    .iter()
                    .any(|r| r.rel_type == "REFERENCES"),
                "engine must not hard-code REFERENCES — pointer rel-type is CITES; got {:?}",
                in_mem.relationships,
            );
        }

        #[test]
        fn no_pointer_schema_refuses_unbacked_body_wiki_link() {
            // Schema declares no `alias_target_rel_type`. Body wiki-link
            // without a backing relation must refuse with
            // `WIKILINK_WITHOUT_RELATION` — the strict validator's
            // pre-Option-C semantics, preserved for opt-out schemas.
            let manifest = r#"name: no-alias
version: 0.1.0
description: schema without alias_target_rel_type pointer
when_to_use: tests prove strict validator still fires for opt-out schemas
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: USES
      description: Use
      default_weight: 1.0
    - name: PART_OF
      description: Hierarchy
      default_weight: 3.0
      acyclic: true
    - name: _default
      description: Fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
            let (mut engine, _tmp) =
                engine_with_schema(manifest, "doc", "no-alias", semver::Version::new(0, 1, 0));
            let (actor, client) = cli_actor();

            let target = engine
                .create_entity(
                    CreateEntityArgs {
                        mem: "v".to_string(),
                        title: "Target".to_string(),
                        entity_type: "doc".to_string(),
                        sections: IndexMap::from_iter([(
                            "body".to_string(),
                            "target body".to_string(),
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
            let source = engine
                .create_entity(
                    CreateEntityArgs {
                        mem: "v".to_string(),
                        title: "Source".to_string(),
                        entity_type: "doc".to_string(),
                        sections: IndexMap::from_iter([(
                            "body".to_string(),
                            "source body".to_string(),
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

            // Add a body wiki-link with no backing relation. The
            // synthesis pass is a no-op (no pointer), so the
            // validator surfaces `WIKILINK_WITHOUT_RELATION`.
            let mut sections: IndexMap<String, String> = IndexMap::new();
            sections.insert("body".to_string(), "see [[target]]".to_string());
            let err = engine
                .update_entity(
                    UpdateEntityArgs {
                        id: source.id.clone(),
                        expected_hash: Some(source.content_hash.clone()),
                        sections,
                        append_sections: IndexMap::new(),
                        patch_sections: IndexMap::new(),
                        metadata: IndexMap::new(),
                        metadata_unset: Vec::new(),
                        declare_relations: Vec::new(),
                        dry_run: false,
                        relations_unset: Vec::new(),
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap_err();
            match err {
                EngineError::WikiLinkWithoutRelation { from_id, missing } => {
                    assert_eq!(from_id, source.id.to_string());
                    assert_eq!(missing.len(), 1);
                    assert_eq!(missing[0].section_key, "body");
                    assert_eq!(missing[0].target_id, target.id.to_string());
                }
                other => panic!(
                    "no-pointer schema must refuse with WikiLinkWithoutRelation; got {other:?}"
                ),
            }
        }

        /// Restoring the
        /// pre-alias-synthesis invariant that every body wiki-link
        /// target carries a grammar-valid `EntityId`. Natural-form
        /// `[[Knowledge Graph]]` no longer slips through into a
        /// malformed auto-stub; the engine refuses with the typed
        /// `InvalidWikiLinkTarget` envelope and the
        /// `title_to_slug`-derived suggestion the agent lifts
        /// directly into a retry. Covers F1 of the 2026-05-18 CLI probe.
        #[test]
        fn natural_form_body_wiki_link_refuses_with_typed_envelope() {
            let manifest = r#"name: aliased
version: 0.1.0
description: alias-synthesis fixture
when_to_use: tests prove strict wiki-link grammar at mutation entry
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: REFERENCES
      description: Reference — auto-emitted from body wiki-links
      default_weight: 0.5
    - name: PART_OF
      description: Hierarchy
      default_weight: 3.0
      acyclic: true
    - name: _default
      description: Fallback
      default_weight: 1.0
alias_target_rel_type: REFERENCES
community:
  resolution: 1.0
  seed: 42
"#;
            let (mut engine, _tmp) =
                engine_with_schema(manifest, "doc", "aliased", semver::Version::new(0, 1, 0));
            let (actor, client) = cli_actor();

            let mut sections: IndexMap<String, String> = IndexMap::new();
            sections.insert("body".to_string(), "see [[Knowledge Graph]]".to_string());
            let err = engine
                .create_entity(
                    CreateEntityArgs {
                        mem: "v".to_string(),
                        title: "Source".to_string(),
                        entity_type: "doc".to_string(),
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
                EngineError::InvalidWikiLinkTarget {
                    raw,
                    suggested,
                    section,
                    link_source,
                    ..
                } => {
                    assert_eq!(raw, "Knowledge Graph");
                    assert_eq!(suggested.as_deref(), Some("knowledge-graph"));
                    assert_eq!(section, "body");
                    assert_eq!(link_source, "body_link");
                }
                other => panic!(
                    "natural-form body wiki-link must refuse with InvalidWikiLinkTarget; got {other:?}"
                ),
            }
        }

        /// Tier-2 body wiki-link with a non-conformant
        /// mem prefix refuses with the distinct `InvalidMemName`
        /// (wire code `INVALID_MEM_NAME`) — mems are fixed
        /// identifiers, not free-form text the agent can slugify, so
        /// the recovery path is different from `InvalidWikiLinkTarget`.
        #[test]
        fn tier_two_bad_mem_prefix_refuses_with_distinct_envelope() {
            let manifest = r#"name: aliased
version: 0.1.0
description: alias-synthesis fixture
when_to_use: tests prove strict mem-prefix grammar at mutation entry
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: REFERENCES
      description: Reference
      default_weight: 0.5
    - name: PART_OF
      description: Hierarchy
      default_weight: 3.0
      acyclic: true
    - name: _default
      description: Fallback
      default_weight: 1.0
alias_target_rel_type: REFERENCES
community:
  resolution: 1.0
  seed: 42
"#;
            let (mut engine, _tmp) =
                engine_with_schema(manifest, "doc", "aliased", semver::Version::new(0, 1, 0));
            let (actor, client) = cli_actor();

            let mut sections: IndexMap<String, String> = IndexMap::new();
            sections.insert("body".to_string(), "see [[Other Mem:foo]]".to_string());
            let err = engine
                .create_entity(
                    CreateEntityArgs {
                        mem: "v".to_string(),
                        title: "Source".to_string(),
                        entity_type: "doc".to_string(),
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
                EngineError::InvalidWikiLinkMem { raw, section, .. } => {
                    assert_eq!(raw, "Other Mem");
                    assert_eq!(section, "body");
                }
                other => panic!(
                    "Tier-2 bad mem prefix must refuse with InvalidWikiLinkMem; got {other:?}"
                ),
            }
        }

        /// Body wiki-link
        /// containing the ambiguous `[[<segments>/<segments>--<slug>]]`
        /// form refuses with `InvalidWikiLinkTarget` carrying the
        /// colon-form (`<prefix>:<slug>`) as `suggested`. Pre-fix the
        /// dash form silently produced a same-mem phantom stub at
        /// slug `team/sub-mem--target`, losing the agent's intent.
        #[test]
        fn hierarchical_dash_form_body_link_refuses_with_colon_suggestion() {
            let manifest = r#"name: aliased
version: 0.1.0
description: alias-synthesis fixture
when_to_use: tests prove hierarchical dash-form refusal at mutation entry
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: REFERENCES
      description: Reference — auto-emitted from body wiki-links
      default_weight: 0.5
    - name: PART_OF
      description: Hierarchy
      default_weight: 3.0
      acyclic: true
    - name: _default
      description: Fallback
      default_weight: 1.0
alias_target_rel_type: REFERENCES
community:
  resolution: 1.0
  seed: 42
"#;
            let (mut engine, _tmp) =
                engine_with_schema(manifest, "doc", "aliased", semver::Version::new(0, 1, 0));
            let (actor, client) = cli_actor();

            let mut sections: IndexMap<String, String> = IndexMap::new();
            sections.insert(
                "body".to_string(),
                "see [[team/sub-mem--target]]".to_string(),
            );
            let err = engine
                .create_entity(
                    CreateEntityArgs {
                        mem: "v".to_string(),
                        title: "Source".to_string(),
                        entity_type: "doc".to_string(),
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
                EngineError::InvalidWikiLinkTarget {
                    raw,
                    suggested,
                    section,
                    link_source,
                    ..
                } => {
                    assert_eq!(raw, "team/sub-mem--target");
                    assert_eq!(suggested.as_deref(), Some("team/sub-mem:target"));
                    assert_eq!(section, "body");
                    assert_eq!(link_source, "body_link");
                }
                other => panic!(
                    "hierarchical dash-form body link must refuse with InvalidWikiLinkTarget; got {other:?}"
                ),
            }

            // The entity did not land — no phantom stub for the source,
            // no phantom stub for the would-be target.
            let listed = engine.store().all_entities().collect::<Vec<_>>();
            assert!(
                listed.is_empty(),
                "refused create must not leave any entity behind, got: {listed:?}"
            );
        }
    }

    // ---- relations_unset repair-power gating -------------------------

    /// Markdown for a deliberately non-conformant `spec`: carries an
    /// undeclared metadata field (`zzz_bogus_field`) plus one USES
    /// relation. Written straight to disk before engine construction —
    /// the write path refuses non-conformant entities, so out-of-band
    /// state is the only way to seed one (which is exactly the
    /// repair-power scenario: drift entered outside the engine).
    const DRIFTED_MD: &str = "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nzzz_bogus_field: x\n---\n# Drifted\n\n## Identity\n\nNon-conformant fixture.\n\n## Purpose\n\nRepair-gate tests.\n\n## Relationships\n\n- **USES**: [[anchor]]\n";

    fn repair_engine() -> (TempDir, Engine) {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        std::fs::write(
            mem_dir.join("anchor.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\n---\n# Anchor\n\n## Identity\n\nTarget.\n\n## Purpose\n\nRelation target.\n",
        )
        .unwrap();
        std::fs::write(mem_dir.join("drifted.md"), DRIFTED_MD).unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        (tmp, engine)
    }

    fn repair_args(id: EntityId, hash: Option<String>) -> UpdateEntityArgs {
        UpdateEntityArgs {
            id,
            expected_hash: hash,
            sections: IndexMap::new(),
            append_sections: IndexMap::new(),
            patch_sections: IndexMap::new(),
            metadata: IndexMap::new(),
            metadata_unset: Vec::new(),
            declare_relations: Vec::new(),
            dry_run: false,
            relations_unset: vec![crate::ops::RelationUnsetArg {
                rel_type: "USES".to_string(),
                target: EntityId::new("specs", "anchor"),
            }],
        }
    }

    /// A conformant entity refuses repair-shaped input with
    /// `REPAIR_NOT_NEEDED` and is not modified — even though it has a
    /// relation that `relations_unset` names. The recovery text points
    /// at the focused detach path.
    #[test]
    fn relations_unset_on_conformant_entity_refuses_repair_not_needed() {
        let (_tmp, mut engine) = repair_engine();
        // `anchor` is conformant. Give it a relation first via the
        // ordinary relate path so there is something to (not) remove.
        let anchor = EntityId::new("specs", "anchor");
        let drifted = EntityId::new("specs", "drifted");
        engine
            .relate_entity(
                RelateEntityArgs {
                    source: anchor.clone(),
                    expected_hash: None,
                    rel_type: "USES".to_string(),
                    target: drifted.clone(),
                    remove: false,
                    description: None,
                },
                Actor::Cli,
                None,
                None,
            )
            .expect("relate on conformant entity works");
        let mut args = repair_args(anchor.clone(), None);
        args.relations_unset[0].target = drifted.clone();
        let err = engine
            .update_entity(args, Actor::Cli, None, None)
            .unwrap_err();
        match err {
            EngineError::RepairNotNeeded { id, recovery } => {
                assert_eq!(id, anchor.to_string());
                assert!(
                    recovery.contains("memstead_relate"),
                    "recovery must point at the focused tool; got {recovery}"
                );
            }
            other => panic!("expected RepairNotNeeded, got {other:?}"),
        }
        // Entity unmodified — the relation is still there.
        let entity = engine.store().get(&anchor).unwrap();
        assert!(
            entity.relationships.iter().any(|r| r.target == drifted),
            "gate must not modify the entity"
        );
    }

    /// A non-conformant entity accepts `relations_unset`: the named
    /// relation is removed atomically within the same update that also
    /// repairs the conformance break (`metadata_unset` on the
    /// undeclared field). The post-write entity is integral.
    #[test]
    fn relations_unset_repairs_non_conformant_entity_atomically() {
        let (_tmp, mut engine) = repair_engine();
        let drifted = EntityId::new("specs", "drifted");
        // Pre-state really is non-conformant.
        let pre = engine.conformance_findings("specs", None).unwrap();
        assert!(
            pre.iter().any(|f| f.id == drifted.to_string()),
            "fixture must lint non-conformant; got {pre:?}"
        );
        let mut args = repair_args(drifted.clone(), None);
        args.metadata_unset = vec!["zzz_bogus_field".to_string()];
        engine
            .update_entity(args, Actor::Cli, None, None)
            .expect("repair update lands");
        let entity = engine.store().get(&drifted).unwrap();
        assert!(
            entity.relationships.is_empty(),
            "relation must be removed; got {:?}",
            entity.relationships
        );
        assert!(
            !entity.metadata.contains_key("zzz_bogus_field"),
            "conformance break must be repaired in the same update"
        );
        let post = engine.conformance_findings("specs", None).unwrap();
        assert!(
            post.iter().all(|f| f.id != drifted.to_string()),
            "post-repair entity must be conformant; got {post:?}"
        );
    }

    /// Repair widens accepted inputs, never admissible outputs: a
    /// repair write whose post-state would violate the schema refuses
    /// with the relevant write-time code and nothing lands.
    #[test]
    fn relations_unset_post_state_must_still_validate() {
        let (_tmp, mut engine) = repair_engine();
        let drifted = EntityId::new("specs", "drifted");
        let mut args = repair_args(drifted.clone(), None);
        // Post-state violation: an unknown section alongside the
        // repair input.
        args.sections = IndexMap::from_iter([("nonexistent_section".to_string(), "x".to_string())]);
        let err = engine
            .update_entity(args, Actor::Cli, None, None)
            .unwrap_err();
        assert_eq!(
            err.code(),
            "UNKNOWN_SECTION",
            "strict-write post-condition must hold during repair; got {err:?}"
        );
        // Nothing landed: the relation survives.
        let entity = engine.store().get(&drifted).unwrap();
        assert!(
            !entity.relationships.is_empty(),
            "refused repair must not partially apply"
        );
    }

    /// Absent `(rel_type, target)` pairs are silent no-ops — symmetric
    /// with `metadata_unset` — so a repair retry is idempotent.
    #[test]
    fn relations_unset_absent_pair_is_silent_noop() {
        let (_tmp, mut engine) = repair_engine();
        let drifted = EntityId::new("specs", "drifted");
        let mut args = repair_args(drifted.clone(), None);
        args.relations_unset[0].rel_type = "NEVER_DECLARED".to_string();
        // Also repair the field so the post-state is integral.
        args.metadata_unset = vec!["zzz_bogus_field".to_string()];
        engine
            .update_entity(args, Actor::Cli, None, None)
            .expect("absent pair no-ops, update lands");
        let entity = engine.store().get(&drifted).unwrap();
        assert_eq!(
            entity.relationships.len(),
            1,
            "the USES relation must survive an unmatched unset"
        );
    }
}
