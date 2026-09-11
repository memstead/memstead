//! Compose phase of an update: apply the delta to a copy of the entity
//! (repair-shaped removals, declared relations, section replace /
//! append / patch / unset, metadata set / unset), synthesise relations
//! from body wiki-links, enforce the alias-existence invariant, decide
//! the no-op against the on-disk bytes, stamp and render, and judge
//! the composed state — heading divergence, declared section formats,
//! required outgoing edges, declared constraints, the auto-stub and
//! dropped-link warnings. A no-op or a dry-run completes here; a real
//! change leaves as the staged material the commit phase lands.

use crate::engine::outcomes::RelationDeclared;
use crate::entity::{Entity, EntityId, Relationship};
use crate::ops::{ModifiedMetadata, ModifiedSections, WarningHint};
use crate::runtime_validator::parse_metadata_value;

use super::super::{PATCH_OLD_NOT_FOUND_CONTENT_CAP, make_stub, validate_relation_target_grammar};
use super::outcome::{dry_run_outcome, noop_outcome};
use super::validate::ValidatedUpdate;
use super::{Engine, EngineError, PrepareOutcome, PreparedUpdate};

impl Engine {
    /// Apply the delta, synthesise, render and judge the composed
    /// entity; return the no-op or dry-run outcome, or the prepared
    /// write.
    pub(super) fn compose_update(
        &mut self,
        validated: ValidatedUpdate,
    ) -> Result<PrepareOutcome, EngineError> {
        let ValidatedUpdate {
            args,
            mount_idx,
            mem,
            entity,
            prev_body_targets,
            schema,
            type_def,
            anchors: validated_anchors,
            anchor_unsets: validated_anchor_unsets,
        } = validated;
        let id = &args.id;

        let mut next = entity;

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

        // Keys this update touches, captured before the args maps are
        // consumed — the section-format evaluation below judges
        // exactly these on their composed bodies.
        let format_touched: std::collections::HashSet<String> = args
            .sections
            .keys()
            .chain(args.append_sections.keys())
            .chain(args.patch_sections.keys())
            .cloned()
            .collect();

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
        for (key, patches) in args.patch_sections {
            // Patches for one section apply in order against the evolving
            // body, so a batched multi-edit lands in one call (the old
            // one-patch-per-section-per-call shape cost one refused call
            // per extra edit; two campaigns hit it).
            for patch in patches {
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
                    // Where the substring DOES occur — the one-call recovery
                    // when the patch targeted the wrong section (a "found in
                    // `versioning` instead" hint turns three attempts into one).
                    let found_in_sections: Vec<String> = next
                        .sections
                        .iter()
                        .filter(|(k, body)| k.as_str() != key && body.contains(&patch.old))
                        .map(|(k, _)| k.clone())
                        .collect();
                    return Err(EngineError::PatchOldNotFound {
                        section: key,
                        current_content,
                        truncated,
                        found_in_sections,
                    });
                }
                let patched = if patch.all {
                    existing.replace(&patch.old, &patch.new)
                } else {
                    existing.replacen(&patch.old, &patch.new, 1)
                };
                next.sections.insert(key.clone(), patched);
            }
            modified_sections_patched.push(key);
        }

        // Apply `sections_unset` after the write modes (same-key overlap
        // is already refused above, so ordering carries no semantics):
        // heading and body leave the entity. Absent keys no-op silently,
        // symmetric with `metadata_unset`.
        let mut modified_sections_unset: Vec<String> = Vec::new();
        for key in &args.sections_unset {
            if next.sections.shift_remove(key).is_some() {
                modified_sections_unset.push(key.clone());
            }
        }

        let mut modified_metadata_set: Vec<String> = Vec::new();
        for (key, value) in &args.metadata {
            let parsed = parse_metadata_value(key.as_str(), value.as_str(), type_def.as_ref())?;
            modified_metadata_set.push(key.clone());
            next.metadata.insert(key.clone(), parsed);
        }

        let mut modified_metadata_unset: Vec<String> = Vec::new();
        for key in args.metadata_unset {
            // Reserved identity/discriminator keys bypass the
            // required-field gate below: unsetting one is the
            // sanctioned repair for a historically smuggled key, and
            // it can only move the entity toward the invariant. For
            // `type` the engine immediately re-seeds the authoritative
            // discriminator from the entity's own type — the
            // frontmatter can never go typeless (a missing `type:`
            // would silently re-type the entity to the mem's default
            // on the next parse), so on a healthy entity the unset is
            // a no-op. `mem`/`id` are never engine-seeded in the map;
            // removing a smuggled one is a real (recorded) removal.
            if crate::runtime_validator::READ_ONLY_METADATA_KEYS.contains(&key.as_str()) {
                if key == "type" {
                    let authoritative =
                        crate::entity::MetadataValue::String(next.entity_type.clone());
                    if next
                        .metadata
                        .shift_remove("type")
                        .is_some_and(|removed| removed != authoritative)
                    {
                        modified_metadata_unset.push(key);
                    }
                    next.metadata.insert("type".to_string(), authoritative);
                } else if next.metadata.shift_remove(&key).is_some() {
                    modified_metadata_unset.push(key);
                }
                continue;
            }
            // Reject unset on required fields — the pre-remove check
            // carries the recovery payload (field_description,
            // enum_values, type_write_rules) so MCP envelopes surface
            // the full REQUIRED_FIELD_UNSET shape.
            let field_def = type_def.metadata_field(&key);
            let is_required = field_def.map(|f| f.is_required()).unwrap_or(false);
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
        let today = self.now_iso();

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
        let alias_outcome =
            super::super::synthesise_alias_relations(self, &prev_body_targets, &mut next)?;
        let synthesised_relations = alias_outcome.emitted;
        let self_link_ignored = alias_outcome.self_link_ignored;
        let undeclared_targets: std::collections::HashSet<crate::entity::EntityId> = alias_outcome
            .undeclared_dropped
            .iter()
            .map(|d| d.target.clone())
            .collect();
        let undeclared_dropped = alias_outcome.undeclared_dropped;

        // Alias-existence invariant: every body wiki-link must be
        // backed by an entry in `entity.relationships`. The validator
        // runs against the *full* post-mutation state (not just the
        // delta), so a mutation that leaves an existing unbacked link
        // in place still fails — forcing cleanup of historical drift.
        let missing = super::super::scan_wikilinks_without_relation(&next, &undeclared_targets)?;
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
        let markdown_pre_stamp = super::super::render_for_write(&next, type_def.as_ref())?;

        // Whether the user-visible content (sections, user-set metadata,
        // declared relations) is byte-identical to the on-disk entity —
        // the pre-stamp markdown's hash matches the current
        // `content_hash`. When this holds past the no-op guard below, the
        // *only* remaining delta is the anchors sidecar (an anchor-only
        // update). `_hash` excludes anchors, so an anchor-only commit
        // produces zero entity deltas; the distinct `anchor` verb is the
        // compensating signal that makes it observable. `next.content_hash`
        // is the on-disk value (cloned from the source entity and never
        // recomputed since — same rationale as the no-op branch reads it).
        let content_unchanged =
            crate::entity::parser::compute_hash(&markdown_pre_stamp) == next.content_hash;

        // No-op short-circuit. When the pre-stamp markdown's hash
        // matches the entity's current `content_hash`, the
        // post-mutation user-visible state equals the on-disk state.
        // Skip the disk write, the commit, the provenance append,
        // and the store re-parse. Mirrors `relate.rs`'s
        // `NoOpAlreadyPresent` / `NoOpAbsent` and `rename.rs`'s
        // slug-noop short-circuits: empty `write_id`, unchanged
        // `content_hash`, preserved `last_modified`, typed
        // `UpdateNoop` warning. Skipped on the dry_run path because
        // the dry_run preview semantics document a separate
        // non-committing shape with `prospective_hash: Some(_)` and
        // the unchanged `content_hash`; conflating the two would
        // lose the prospective-hash channel callers use to chain a
        // follow-up real update with `expected_hash`.
        // Whether the anchors merge would change the sidecar, decided on a
        // copy before any write so the answer can steer the no-op guard:
        // an anchors-only update that restates the stored rows is a no-op
        // like any other.
        let anchors_changed: Option<bool> =
            if validated_anchors.is_empty() && validated_anchor_unsets.is_empty() {
                None
            } else {
                Some(super::super::anchors_would_change(
                    self.mounts[mount_idx].backend.as_ref(),
                    id,
                    &validated_anchor_unsets,
                    &validated_anchors,
                    !content_unchanged,
                )?)
            };
        if !args.dry_run {
            // An update carrying anchors or unsets that change the sidecar
            // is never a no-op even when the entity content is unchanged;
            // one whose rows restate what is stored is.
            if content_unchanged && anchors_changed != Some(true) {
                return Ok(PrepareOutcome::Done(noop_outcome(
                    &next,
                    file_path,
                    relations_declared,
                    anchors_changed,
                )));
            }
        }

        // Real change: apply the auto-stamp now and regenerate the
        // markdown so the subsequent hash + write reflect it.
        // Exception — the anchor-only leg (`content_unchanged` true,
        // reachable only because anchors/unsets are present): the
        // sidecar is the sole delta and `_hash` excludes it, so the
        // entity bytes must stay untouched. Stamping here would move
        // `last_modified` (and with it `_hash`) whenever the update
        // lands in a different second than the previous write —
        // breaking the "anchors never move `_hash`" contract.
        if !content_unchanged {
            super::super::auto_stamp_timestamps(&mut next, type_def.as_ref(), &today);
        }
        let markdown = super::super::render_for_write(&next, type_def.as_ref())?;

        let mut warnings: Vec<WarningHint> = Vec::new();

        // Heading-divergence check: when a written section's declared
        // heading differs from a heading the file already carried for
        // the same key (derives to it), warn — the write commits and
        // the regenerated file replaces the old heading text, which
        // the caller should see rather than discover on the next read.
        // `next.raw_section_headings` is the pre-mutation parse
        // artefact (cloned from the store entity; nothing in this
        // pipeline rewrites it).
        for key in modified_sections
            .iter()
            .chain(modified_sections_appended.iter())
            .chain(modified_sections_patched.iter())
        {
            let Some(def) = type_def.section(key) else {
                continue;
            };
            if let Some(existing) = next.raw_section_headings.iter().find(|h| {
                h.as_str() != def.heading && memstead_schema::derive_section_key(h) == *key
            }) {
                warnings.push(WarningHint::SectionHeadingDivergence {
                    entity_id: id.clone(),
                    section_key: key.clone(),
                    writing_heading: def.heading.clone(),
                    existing_heading: existing.clone(),
                });
            }
        }

        // Required-outgoing evaluation — the warning the tool
        // descriptions have promised all along. `next` carries this
        // update's final edge set (declared relations applied,
        // alias-synthesis run), evaluated through the same function
        // the health sweep uses (one implementation; the two surfaces
        // cannot disagree). A warning, never a refusal.
        // Section-format evaluation (plan 08), composed-body rule: a
        // section touched by this update (replace, append, or patch)
        // is judged on its COMPOSED final body — the delta-only
        // byte-class guard keeps its scope, shape needs the
        // composition point. Untouched sections stay lenient (their
        // pre-existing violations are health findings; the next write
        // is the sanctioned repair point).
        for def in &type_def.sections {
            if def.format_severity != memstead_schema::ConstraintSeverity::Block {
                continue;
            }
            if !format_touched.contains(def.key.as_str()) {
                continue;
            }
            let Some(body) = next.sections.get(def.key.as_str()) else {
                continue;
            };
            if let Some(first) = crate::section_format::check_section_format(def, body)
                .into_iter()
                .next()
            {
                return Err(EngineError::SectionFormatRefused {
                    entity_type: next.entity_type.clone(),
                    entity_id: id.to_string(),
                    violation: first,
                });
            }
        }

        let unsatisfied =
            crate::ops::health::unsatisfied_required_outgoing(&next, type_def.as_ref());
        if !unsatisfied.is_empty() {
            // `severity: block` promotes the warning to a refusal —
            // evaluated in the prepare step, before any disk write or
            // commit, so a refused update leaves nothing behind.
            let blocked: Vec<_> = unsatisfied
                .iter()
                .filter(|b| b.severity == memstead_schema::ConstraintSeverity::Block)
                .cloned()
                .collect();
            if !blocked.is_empty() {
                return Err(EngineError::RequiredOutgoingUnsatisfied {
                    entity_type: next.entity_type.clone(),
                    entity_id: id.to_string(),
                    missing: blocked,
                });
            }
            warnings.push(WarningHint::MissingRequiredOutgoing {
                entity_type: next.entity_type.clone(),
                entity_id: id.clone(),
                missing: unsatisfied,
            });
        }

        // Declared-constraints evaluation — same single evaluation the
        // health `constraints` include runs, against this update's
        // final state. Block-tier violations refuse; warn-tier warn.
        let check_provider = self.check_standing_provider();
        let violated = crate::ops::health::unsatisfied_constraints(
            &self.store,
            &next,
            type_def.as_ref(),
            Some(id),
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
                    entity_type: next.entity_type.clone(),
                    entity_id: id.to_string(),
                    violations: blocked,
                });
            }
            warnings.push(WarningHint::ConstraintUnsatisfied {
                entity_type: next.entity_type.clone(),
                entity_id: id.clone(),
                violations: violated,
            });
        }

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
        // A cross-schema body link the alias pass declined for lack of a
        // cross_mem_relationships declaration: the write succeeded, the
        // link stays prose — say so, typed.
        for dropped in undeclared_dropped {
            warnings.push(WarningHint::CrossSchemaLinkUndeclared {
                from: id.clone(),
                target: dropped.target,
                source_schema: dropped.source_schema,
                target_schema: dropped.target_schema,
            });
        }

        // Dry-run: compute prospective hash from the in-memory
        // entity and return without touching disk, store, or
        // commits. Mirrors full's `UpdateArgs.dry_run` semantics —
        // `content_hash` carries the unchanged on-disk hash so the
        // caller can use it as `expected_hash` on the follow-up
        // real call (designated stale-hash recovery path).
        // `next.content_hash` was cloned from the source entity and
        // not modified since; equals the on-disk value.
        let current_hash = next.content_hash.clone();
        let title = next.title.clone();
        let dry_run_modified_date = if type_def.metadata_fields.iter().any(|f| f.auto_timestamp) {
            today.clone()
        } else {
            String::new()
        };

        // Real change prepared. Compute `modified_date` (mirrors full's
        // UpdateResult.modified_date — the `today` the auto-stamp loop
        // used; empty when the schema has no auto_timestamp field), then
        // hand the staged write to the caller to commit. The single
        // path commits immediately; the batch path commits the whole
        // set at once.
        let modified_date = if content_unchanged {
            // Anchor-only: nothing was stamped; report the preserved
            // on-entity value, mirroring the no-op branch.
            next.metadata
                .get("last_modified")
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        } else if type_def.metadata_fields.iter().any(|f| f.auto_timestamp) {
            today.clone()
        } else {
            String::new()
        };

        let prepared = PreparedUpdate {
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
                unset: modified_sections_unset,
            },
            modified_metadata: ModifiedMetadata {
                set: modified_metadata_set,
                unset: modified_metadata_unset,
            },
            // F5: `InlineWikiLinkAutoStubbed` rides on the outcome so the
            // update path matches create's contract.
            warnings,
            relations_declared,
            // Anchor-only: content byte-identical to disk, so the sidecar
            // is the sole delta. Reachable here only past the no-op guard,
            // which already returned when content is unchanged AND no
            // anchors or unsets — so `content_unchanged` here implies
            // anchor work is present. The explicit `!is_empty()` keeps the
            // predicate self-evidently correct without leaning on that
            // invariant.
            anchor_only: content_unchanged && anchors_changed == Some(true),
            anchors_changed,
            content_changed: !content_unchanged,
            anchors: validated_anchors,
            anchor_unsets: validated_anchor_unsets,
        };

        if args.dry_run {
            return Ok(PrepareOutcome::Done(dry_run_outcome(
                prepared,
                title,
                current_hash,
                dry_run_modified_date,
            )));
        }

        Ok(PrepareOutcome::Prepared(prepared))
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

        validate_relation_target_grammar(&rel.target)?;

        let target_mem = rel.target.mem().to_string();
        // Grant + ReadOnly-missing-target checks both live in the
        // shared add-path funnel.
        super::super::validate_cross_mem_add_policy(engine, source_mem, &rel.target)?;

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
            .get(&rel.target)
            .map(|e| e.entity_type.clone())
            .filter(|t| !t.is_empty());
        // Deferred-mem target (flywheel W7/02): the real type comes
        // from the one resolved blob, never from loading the mem.
        let target_type = match target_type {
            Some(t) => Some(t),
            None => super::super::peek_deferred_target_type(engine, &rel.target)?,
        };
        let _ = super::super::route_edge_validation(
            engine,
            &canonical,
            next.entity_type.as_str(),
            target_type.as_deref(),
            source_mem,
            &target_mem,
            &next.id,
            &rel.target,
            /* check_shape = */ true,
        )?;

        // Per-edge description posture. Normalise first so
        // empty/whitespace-only inputs collapse to `None` and the
        // posture check sees a canonical input that matches what
        // the renderer will emit.
        let normalised_description =
            crate::entity::normalise_description(rel.description.as_deref());
        super::super::validate_description_posture(
            engine,
            &canonical,
            normalised_description.as_deref(),
            source_mem,
            &target_mem,
            &next.id,
            &rel.target,
        )?;
        // declare_relations is an explicit-author
        // boundary too — gate on manual_authoring posture.
        super::super::validate_manual_authoring_posture(
            engine,
            &canonical,
            source_mem,
            &next.id,
            &rel.target,
        )?;

        // Cycle family — the same shared gate `memstead_relate` runs
        // (self-loop on listed no-self-loop rel-types, long cycle on acyclic
        // types), against the current store state.
        super::super::validate_edge_acyclicity(
            &engine.store,
            schema,
            &next.id,
            next.entity_type.as_str(),
            &rel.target,
            &canonical,
        )?;

        // Append to the entity's relationships list. Duplicate
        // declarations are idempotent — same (rel_type, target) pair
        // is a silent no-op so the agent can re-issue the same call
        // without surprise.
        let exists = next
            .relationships
            .iter()
            .any(|r| r.rel_type == canonical && r.target == rel.target);
        if !exists {
            next.relationships.push(Relationship {
                rel_type: canonical.clone(),
                target: rel.target.clone(),
                description: normalised_description,
            });
        }

        // Auto-stub absent Write-mem targets. Same mechanic as
        // `memstead_relate`'s relate path. ReadOnly cross-mem targets
        // were caught above; same-mem and cross-mem-to-Write
        // both fall through here.
        let target_was_stubbed = !engine.store.contains(&rel.target);
        if target_was_stubbed && !exists {
            let kind = super::super::deferred_verified_stub_kind(engine, &rel.target)?;
            engine
                .store
                .upsert(rel.target.clone(), make_stub(&rel.target, kind));
        }

        declared.push(RelationDeclared {
            rel_type: canonical,
            target: rel.target.clone(),
            target_was_stubbed,
        });
    }
    Ok(declared)
}
