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
use crate::entity::generator::generate_markdown;
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
    EdgeRouteOutcome, make_stub, route_edge_validation, today_iso, unknown_type_error,
    validate_relation_target_grammar,
};

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
            .ok_or_else(|| EngineError::UnknownMem(args.mem.clone()))?;
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
        let mut drift_warnings = self.reload_if_stale(Some(&args.mem));

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
        // Refuse section content with embedded `^## ` headings — the
        // compose-then-reparse pipeline would split the value at the
        // heading and silently move the trailing content into another
        // section.
        validate_section_content(args.sections.iter().map(|(k, v)| (k.as_str(), v.as_str())))?;

        // 4. Slug + id; reject duplicates against the in-memory store.
        //    Stub adoption: a pre-existing stub at the same id is
        //    *not* a duplicate — the create promotes the stub to a
        //    real entity while preserving its incoming edges (store.
        //    upsert leaves in_edges in place). Mirrors full's
        //    `if let Some(existing) = store.get(&id) && !existing.stub`.
        let slug = validate_and_derive_slug(&args.title)?;
        let id = EntityId::new(&args.mem, &slug);
        crate::entity::id::enforce_id_length(id.as_ref())?;
        if let Some(existing) = self.store.get(&id)
            && !existing.stub
        {
            return Err(EngineError::AlreadyExists { id: id.to_string() });
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
        let today = today_iso();
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
            validate_relation_target_grammar(&rel.to)?;
            let target_mem = rel.to.mem().to_string();
            // Cross-mem policy gate. The funnel
            // sits ahead of the rel-type / shape checks so the policy
            // refusal is identical in shape and ordering to
            // `memstead_relate` and `memstead_update.declare_relations`.
            super::validate_cross_mem_add_policy(self, &args.mem, &rel.to)?;
            // Target-type lookup mirrors the relate path: `None` for
            // not-yet-present targets so the target gate admits the
            // stub-bound case. The cross-mem router below consults
            // it for both intra-mem shape and cross-mem-different
            // shape checks.
            let target_type = self
                .store
                .get(&rel.to)
                .map(|e| e.entity_type.clone())
                .filter(|t| !t.is_empty());
            match route_edge_validation(
                self,
                &rel.rel_type,
                args.entity_type.as_str(),
                target_type.as_deref(),
                &args.mem,
                &target_mem,
                &id,
                &rel.to,
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
                &rel.to,
            )?;
            // Explicit inline-relations path is an
            // explicit-author boundary — gate on the rel-type's
            // `manual_authoring` posture.
            super::validate_manual_authoring_posture(self, &rel.rel_type, &args.mem, &id, &rel.to)?;
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
                target: r.to.clone(),
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
                target: r.to.clone(),
                target_was_stubbed: !self.store.contains(&r.to),
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
        let (synthesised_relations, self_link_ignored) =
            super::synthesise_alias_relations(self, &empty_prev_targets, &mut entity_for_render)?;
        if self_link_ignored {
            warnings.push(WarningHint::SelfLinkIgnored { id: id.clone() });
        }

        // Alias-existence invariant: every body wiki-link must be
        // backed by an entry in `entity.relationships` (the auto-managed
        // `## Relationships` section). Runs unconditionally on every
        // Write-Mem create. See [`scan_wikilinks_without_relation`].
        let missing = super::scan_wikilinks_without_relation(&entity_for_render)?;
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

        let markdown = generate_markdown(&entity_for_render, type_def.as_ref());

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

        // 7b. Dry-run: compute prospective hash from the in-memory
        //     entity and return without touching disk, store, or
        //     edges. Mirrors full's `CreateArgs.dry_run` semantics —
        //     `content_hash` carries the prospective hash since
        //     there's no current to differentiate from. `commit_sha`
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
            return Ok(CreateEntityOutcome {
                id,
                title: args.title,
                mem: args.mem,
                file_path,
                content_hash: prospective_hash,
                commit_sha: String::new(),
                created_date,
                warnings,
                type_guidance,
                incoming_count,
                incoming,
                relations_declared: relations_declared.clone(),
            });
        }

        // 8. Write + commit through the backend. The commit subject
        //    is `memstead: create <id>` so the git-branch backend's
        //    `read_provenance` recovers the kind via the verb. The
        //    folder backend's commit ignores the message; the
        //    canonical form is harmless there.
        let backend = self.mounts[mount_idx].backend.as_ref();
        backend.write_entity(Path::new(&file_path), markdown.as_bytes())?;
        let commit_subject = format!("memstead: create {id}");
        let ctx = CommitContext {
            actor,
            client: client.cloned(),
            tool: Some("create_entity"),
            note: note.map(String::from),
            logical_operation_id: None,
            entity_ids: None,
        };
        let commit_sha = backend.commit(&commit_subject, &ctx)?;

        // 9. Append provenance. Folder writes a JSONL line; git-branch
        //    no-ops (the commit object already carries the data).
        backend.append_provenance(&Provenance::new(
            std::time::SystemTime::now(),
            ProvenanceKind::Create,
            Some(id.to_string()),
            actor,
            client.cloned(),
            note.map(String::from),
        ))?;

        // Self-write bookkeeping: jump `last_known_head` to the SHA
        // we just produced so the next read doesn't surface
        // `MEM_RELOADED` for our own commit.
        self.record_self_write(mount_idx, &commit_sha);

        // 10. Update the in-memory store via re-parse so the store
        //     mirrors the on-disk shape (content_hash, heading_spans).
        let parse_result = parse_markdown(&markdown, &file_path, type_def.as_ref(), &args.mem)
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
        for rel in &args.relations {
            if !self.store.contains(&rel.to) {
                self.store.upsert(
                    rel.to.clone(),
                    make_stub(&rel.to, crate::entity::StubKind::ForwardReference),
                );
            }
        }

        self.invalidate_communities();
        self.invalidate_search_indexes();

        // Stub-adoption visibility: project the incoming edges that
        // survived the upsert. Empty for a fresh create; populated
        // when a pre-existing stub at this id had referrers.
        let incoming = project_incoming(self.store.incoming(&id));
        let incoming_count = (!incoming.is_empty()).then_some(incoming.len());

        // `require_notes` provenance nudge — single engine-level
        // enforcement point (see `Engine::note_missing_warning`). Only
        // reached on the real-write path (commit landed); the dry-run
        // early return above never demands a note.
        if let Some(w) = self.note_missing_warning("create_entity", note) {
            warnings.push(w);
        }

        Ok(CreateEntityOutcome {
            id,
            title: args.title,
            mem: args.mem,
            file_path,
            content_hash,
            commit_sha,
            created_date,
            warnings,
            type_guidance,
            incoming_count,
            incoming,
            relations_declared,
        })
    }

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
    fn create_entity_returns_commit_sha_title_mem_on_real_write() {
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
            !outcome.commit_sha.is_empty(),
            "commit_sha must be populated on a real create"
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

        // Both timestamps should reflect the engine's `today_iso()`,
        // not the caller's `2020-01-01`.
        let today = super::today_iso();
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
                id: outcome.id.clone(),
                metadata,
                metadata_unset: Vec::new(),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                expected_hash: Some(outcome.content_hash.clone()),
                dry_run: false,
                declare_relations: Vec::new(),
                relations_unset: Vec::new(),
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
                to: existing.id.clone(),
                rel_type: "USES".to_string(),
                description: None,
            },
            crate::ops::RelateArg {
                to: absent.clone(),
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
            to: crate::EntityId("specs--bad target with spaces!!".to_string()),
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
                to: target.id.clone(),
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
            to: existing.id.clone(),
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

        // Wire shape: content_hash = prospective hash; commit_sha empty.
        assert_eq!(outcome.id.to_string(), "specs--preview-only");
        assert!(
            !outcome.content_hash.is_empty(),
            "prospective hash populated"
        );
        assert!(outcome.commit_sha.is_empty(), "no commit on dry_run");
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
            EngineError::AlreadyExists { id } => assert_eq!(id, "specs--same-slug"),
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

        // F10 + F19: char-drop titles refuse with reason `invalid_chars`
        // and a `proposed_slug` for mechanical retry.
        let err = engine
            .create_entity(
                empty_create_args("specs", "Hello, World!"),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::InvalidTitle(crate::SlugError::TitleHasInvalidChars {
                invalid_chars,
                proposed_slug,
                ..
            }) => {
                assert!(invalid_chars.contains(&',') && invalid_chars.contains(&'!'));
                assert_eq!(proposed_slug, "hello-world");
            }
            other => panic!("expected InvalidTitle/TitleHasInvalidChars, got {other:?}"),
        }

        // F19: path-traversal-shaped titles fall under the same gate
        // (the `/` and `.` chars are pipeline-dropped).
        let err = engine
            .create_entity(
                empty_create_args("specs", "../etc/passwd"),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::InvalidTitle(slug_err) => {
                assert_eq!(slug_err.reason(), "invalid_chars");
            }
            other => panic!("expected InvalidTitle, got {other:?}"),
        }
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
            to: target.id.clone(),
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
            to: target.id.clone(),
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
            to: target.id.clone(),
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
            to: target.id.clone(),
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
            !created.commit_sha.is_empty(),
            "create still commits (nudge, not block)"
        );

        // --- update, no note: NOTE_MISSING + commit landed ---
        let mut edit: IndexMap<String, String> = IndexMap::new();
        edit.insert("identity".to_string(), "revised".to_string());
        let updated = engine
            .update_entity(
                UpdateEntityArgs {
                    id: created.id.clone(),
                    expected_hash: Some(created.content_hash.clone()),
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
            .unwrap();
        assert_eq!(
            note_missing(&updated.warnings),
            1,
            "update emits NOTE_MISSING"
        );
        assert!(!updated.commit_sha.is_empty(), "update still commits");

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
        assert!(!related.commit_sha.is_empty(), "relate still commits");

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
}
