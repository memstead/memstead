//! `Engine::rename_entity` — change an entity's slug, move its file
//! on the backend, and rewrite the in-memory store.

use std::path::Path;

use crate::engine_fallback_type;
use crate::entity::EntityId;
use crate::entity::id::validate_and_derive_slug;
use crate::entity::parser::parse_markdown;
use crate::entity::store_builder::push_entities_into_store;
use crate::ops::WarningHint;
use crate::provenance::{Provenance, ProvenanceKind};
use crate::vcs::{Actor, ClientId, CommitContext};
use crate::workspace::MountCapability;

use super::super::{Engine, EngineError, RenameEntityArgs, RenameEntityOutcome};
use super::{make_stub, unknown_type_error};

impl Engine {
    /// Positional + CommitContext wrapper around
    /// [`Self::rename_entity`].
    pub fn rename_entity_with_ctx(
        &mut self,
        old_id: &EntityId,
        new_title: &str,
        expected_hash: &str,
        ctx: &CommitContext<'_>,
    ) -> Result<RenameEntityOutcome, EngineError> {
        let args = RenameEntityArgs {
            id: old_id.clone(),
            expected_hash: Some(expected_hash.to_string()),
            new_title: new_title.to_string(),
        };
        self.rename_entity(args, ctx.actor, ctx.client.as_ref(), ctx.note.as_deref())
    }

    /// Rename an entity by changing its title — the slug, id, and
    /// on-disk file path follow.
    ///
    /// **Same-mem referrers and self-references are rewritten
    /// atomically.** The renaming entity is treated as the first
    /// referrer of itself: every entry in its own `relationships`
    /// list whose target equals the old id is updated to point at
    /// the new id, and every `[[<old-slug>]]` token in its own
    /// section bodies is rewritten to the new slug (respecting
    /// fenced-code and inline-code masking). Every other entity in
    /// the same mem that pointed at the old id (via an explicit
    /// relation or an inline body wiki-link) gets the same two-
    /// surface rewrite. All rewrites land in one per-mem commit.
    /// Cross-mem referrers and ReadOnly-mount referrers are not
    /// yet walked — those land with the multi-mem atomicity
    /// machinery and the residual-stub demotion path respectively.
    pub fn rename_entity(
        &mut self,
        args: RenameEntityArgs,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<RenameEntityOutcome, EngineError> {
        // A short id resolves (or refuses) before anything reads its mem.
        let mut args = args;
        let (resolved, short_hint) = self.resolve_entity_id(&args.id)?;
        args.id = resolved;
        let id = &args.id;
        let mem = id.mem().to_string();

        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .ok_or_else(|| self.unknown_mem_error(&mem))?;
        if self.mounts[mount_idx].mount.capability != MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem));
        }

        // Reload-before-operation: reload if a sibling advanced the
        // mem ref so the `expected_hash` compare below runs against
        // current truth. The drift notice rides the outcome's
        // `warnings` (real-rename path).
        let mut drift_warnings: Vec<WarningHint> = short_hint.into_iter().collect();
        drift_warnings.extend(self.reload_if_stale(Some(&mem)));

        let entity = self
            .store
            .get(id)
            .ok_or_else(|| EngineError::NotFound { id: id.to_string() })?;

        // Stub guard — stubs derive their title from the id and have
        // no `entity_type` to validate against. Recovery is
        // `memstead_create` (stub adoption). Item 02 realised the
        // `STUB_NOT_RENAMABLE` code the description list had
        // advertised since the strictness work landed.
        if entity.stub {
            return Err(EngineError::StubNotRenamable { id: id.to_string() });
        }

        if let Some(expected) = args.expected_hash.as_deref()
            && entity.content_hash != expected
        {
            return Err(EngineError::HashMismatch {
                id: id.to_string(),
                current: entity.content_hash.clone(),
                is_stub: entity.stub,
            });
        }

        let derivation = validate_and_derive_slug(&args.new_title)?;
        let new_slug = derivation.slug.clone();
        let new_id = EntityId::new(&mem, &new_slug);
        crate::entity::id::enforce_id_length(new_id.as_ref())?;

        if new_id == *id {
            // Title-with-same-slug: surface a Tier-2 warning rather
            // than touching disk for nothing. Matches full's
            // RenameResult slug-noop shape so autonomous skills
            // can distinguish the silent no-op from a successful
            // cosmetic rewrite via the typed warning code.
            return Ok(RenameEntityOutcome {
                old_id: id.clone(),
                new_id: new_id.clone(),
                old_path: entity.file_path.clone(),
                new_path: entity.file_path.clone(),
                content_hash: entity.content_hash.clone(),
                write_id: String::new(),
                warnings: vec![WarningHint::TitleNormalizedToSlugNoop {
                    requested_title: args.new_title.clone(),
                    current_slug: id.name().to_string(),
                }],
            });
        }
        if let Some(existing) = self.store.get(&new_id) {
            return Err(EngineError::AlreadyExists {
                id: new_id.to_string(),
                existing_title: existing.title.clone(),
                existing_is_stub: existing.stub,
            });
        }

        let schema = self
            .schemas
            .get(&mem)
            .expect("schema present for every registered mount");
        let type_def = schema
            .get_type(&entity.entity_type)
            .ok_or_else(|| unknown_type_error(schema, &entity.entity_type))?;

        let old_file_path = entity.file_path.clone();
        let new_file_path = format!("{new_slug}.md");

        let mut next = entity.clone();
        next.id = new_id.clone();
        next.title = args.new_title.clone();
        next.file_path = new_file_path.clone();

        // Self-reference rewrite, surface 1/2 — `relationships` list.
        // Any explicit edge `<this> --<type>--> <old_id>` is repointed
        // to `<this> --<type>--> <new_id>` so the regenerated
        // `## Relationships` section reflects the new id rather than
        // a relation that would re-emit a stub of the old slug at
        // next read.
        for rel in next.relationships.iter_mut() {
            if rel.target == *id {
                rel.target = new_id.clone();
            }
        }

        // Self-reference rewrite, surface 2/2 — section bodies.
        // Every `[[<old-slug>]]` token in the renaming entity's own
        // section bodies becomes `[[<new-slug>]]`. Code-fenced and
        // inline-code matches are preserved (the rewriter shares the
        // masking discipline of `extract_inline_links`).
        //
        // The body parser admits both short form `[[slug]]` and
        // full-id form `[[mem--slug]]` (same mem) as references to
        // the same entity. The bare-slug rewriter only catches the
        // first; `rewrite_cross_mem_slug` (which already powers the
        // cross-mem Tier-2 rewrite) covers the second by matching
        // `<mem>--<slug>` and `<mem>:<slug>` forms. Calling both
        // here preserves the form the author wrote — short stays short,
        // full-id stays full-id — just retargeted to the new slug.
        let old_slug = id.name().to_string();
        let new_slug_owned = new_slug.clone();
        for body in next.sections.values_mut() {
            let (rewritten, count) = crate::entity::wikilink_rewrite::rewrite_bare_slug(
                body,
                &old_slug,
                &new_slug_owned,
            );
            if count > 0 {
                *body = rewritten;
            }
            let (rewritten, count) = crate::entity::wikilink_rewrite::rewrite_cross_mem_slug(
                body,
                &mem,
                &old_slug,
                &new_slug_owned,
            );
            if count > 0 {
                *body = rewritten;
            }
        }

        // Every entity whose on-disk file the rename rewrites
        // (the renaming entity itself plus every Write-mem referrer
        // touched by the body/relationships rewrite cascade) gets
        // `auto_timestamp` metadata fields stamped before
        // `generate_markdown` so the new file carries the stamp.
        // Pre-compute `today` once so all entities rewritten by this
        // logical operation receive the same timestamp.
        let today = self.now_iso();
        super::auto_stamp_timestamps(&mut next, type_def.as_ref(), &today);

        let markdown = super::render_for_write(&next, type_def.as_ref())?;

        // Referrer collection. Walk every incoming edge — explicit
        // relations and `EdgeSource::BodyLink` synthesised mirrors
        // alike — and bucket each unique referrer by its mem's
        // capability. Same-mem referrers are guaranteed Write (the
        // mount-capability gate at the top of this fn refused the
        // rename if the renaming mem itself isn't Write). Cross-
        // mem referrers split into Write (rewriteable, subject to
        // the `cross_mem_links` policy gate below) and ReadOnly
        // (the engine has no write access; their handling lands with
        // the rename-path residual-stub demotion in the next cut and
        // is filtered out here).
        let mut seen: std::collections::HashSet<EntityId> = std::collections::HashSet::new();
        let mut same_mem_ids: Vec<EntityId> = Vec::new();
        let mut cross_mem_ids: Vec<EntityId> = Vec::new();
        for in_edge in self.store.incoming(id) {
            if in_edge.from == *id || !seen.insert(in_edge.from.clone()) {
                continue;
            }
            if in_edge.from.mem() == mem {
                same_mem_ids.push(in_edge.from.clone());
            } else {
                cross_mem_ids.push(in_edge.from.clone());
            }
        }
        // Prose-only referrers. Body wiki-links are not edge sources
        // (every store edge originates from the auto-managed
        // `## Relationships` section), so a hand-authored file carrying
        // an inline `[[old-slug]]` with no relationship row has no
        // incoming edge and the walk above cannot see it — its link
        // would silently go stale. Scan section bodies with the same
        // lenient extractor the load-time drift scan uses (code-fence
        // and inline-code masking included), gated on a cheap substring
        // probe so the store-wide pass stays proportional to actual
        // mentions. Engine-written entities are covered either way:
        // alias synthesis always emits the row on write.
        for entity in self.store.all_entities() {
            if entity.id == *id || entity.stub || seen.contains(&entity.id) {
                continue;
            }
            let mentions = entity.sections.values().any(|body| {
                body.contains(old_slug.as_str())
                    && crate::entity::parser::extract_inline_links_lenient(body, entity.id.mem())
                        .iter()
                        .any(|t| t == id)
            });
            if !mentions {
                continue;
            }
            seen.insert(entity.id.clone());
            if entity.id.mem() == mem {
                same_mem_ids.push(entity.id.clone());
            } else {
                cross_mem_ids.push(entity.id.clone());
            }
        }

        // Cross-mem peers are partitioned by mount capability.
        // Write peers feed the cross-mem rewrite plan; ReadOnly
        // peers feed the residual-stub demotion path (the engine
        // can't rewrite their on-disk markdown, so we materialise an
        // in-memory stub at the OLD id that holds the surviving
        // incoming edges from the ReadOnly mount — mirrors the
        // delete-path's same-shaped demotion).
        let mut cross_mem_write_ids: Vec<EntityId> = Vec::new();
        let mut readonly_referrers: Vec<EntityId> = Vec::new();
        for from_id in cross_mem_ids {
            match self
                .mount(from_id.mem())
                .map(|m| m.capability)
                .unwrap_or(MountCapability::Write)
            {
                MountCapability::Write => cross_mem_write_ids.push(from_id),
                MountCapability::ReadOnly => readonly_referrers.push(from_id),
            }
        }
        readonly_referrers.sort_by_key(|a| a.to_string());

        // Pre-flight policy gate. Each propagated referrer rewrite
        // is an edge of the form `referrer ∈ peer_mem → renamed ∈
        // mem` — same direction as the original edge. The gate
        // consults `cross_mem_link_allowed(peer_mem, mem)`,
        // which is the direction the policy gates new edges with
        // (forward-looking add-filter). A blocked peer aborts the
        // rename up-front — no writes have happened yet, so the
        // refusal is clean. This is the edge direction, not its
        // inverse.
        let mut blocked_counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for from_id in &cross_mem_write_ids {
            let peer_mem = from_id.mem().to_string();
            if !self.cross_mem_link_allowed(&peer_mem, &mem) {
                *blocked_counts.entry(peer_mem).or_insert(0) += 1;
            }
        }
        if !blocked_counts.is_empty() {
            let blocked_referrers: Vec<crate::engine::error::BlockedReferrer> = blocked_counts
                .into_iter()
                .map(|(peer_mem, count)| crate::engine::error::BlockedReferrer {
                    from_mem: peer_mem,
                    to_mem: mem.clone(),
                    count,
                })
                .collect();
            return Err(EngineError::RenameBlockedByCrossMemPolicy {
                from_mem: mem.clone(),
                blocked_referrers,
            });
        }

        // Deterministic iteration order so the resulting per-mem
        // pending-op replay is stable across runs (helpful for test
        // snapshots and human reviewers).
        same_mem_ids.sort_by_key(|a| a.to_string());
        cross_mem_write_ids.sort_by_key(|a| a.to_string());

        // ----- Same-mem rewrite plan -----
        // (rewritten_markdown, file_path, type_def) per same-mem
        // referrer — collected before any backend write so a
        // per-referrer schema or rewrite failure aborts the rename
        // before anything lands.
        let mut same_mem_writes: Vec<(
            String,
            String,
            std::sync::Arc<memstead_schema::TypeDefinition>,
        )> = Vec::with_capacity(same_mem_ids.len());
        for from_id in &same_mem_ids {
            let Some(referrer) = self.store.get(from_id) else {
                continue;
            };
            if referrer.stub {
                continue;
            }
            let referrer_type_def = schema
                .get_type(&referrer.entity_type)
                .ok_or_else(|| unknown_type_error(schema, &referrer.entity_type))?;
            let mut next_ref = referrer.clone();
            for rel in next_ref.relationships.iter_mut() {
                if rel.target == *id {
                    rel.target = new_id.clone();
                }
            }
            for body in next_ref.sections.values_mut() {
                let (rewritten, count) = crate::entity::wikilink_rewrite::rewrite_bare_slug(
                    body,
                    &old_slug,
                    &new_slug_owned,
                );
                if count > 0 {
                    *body = rewritten;
                }
                // A same-mem referrer may also use
                // full-id form `[[<mem>--<slug>]]` to point at the
                // renaming entity — covered by the cross-mem helper
                // which matches the `--` and `:` separator forms.
                let (rewritten, count) = crate::entity::wikilink_rewrite::rewrite_cross_mem_slug(
                    body,
                    &mem,
                    &old_slug,
                    &new_slug_owned,
                );
                if count > 0 {
                    *body = rewritten;
                }
            }
            // Do NOT bump the referrer's `auto_timestamp` fields. A
            // rename rewrites the referrer's body wiki-link to the new
            // slug — a foreign-key change, not a semantic edit — so its
            // `last_modified` staleness clock must NOT reset (a staleness
            // audit would otherwise read every referrer of a renamed
            // entity as freshly-touched). The referrer's content hash
            // still changes (its body now holds the new slug); only the
            // staleness clock is preserved by carrying the prior
            // timestamp through from the cloned referrer. (Pre-fix this
            // stamped the shared `today` for cross-entity consistency.)
            let ref_markdown = super::render_for_write(&next_ref, referrer_type_def.as_ref())?;
            same_mem_writes.push((ref_markdown, next_ref.file_path.clone(), referrer_type_def));
        }

        // ----- Cross-mem rewrite plan -----
        // Group cross-mem Write referrers by their mem so each
        // peer mem's backend gets one commit. Each entry holds the
        // peer mount index, the peer mem's schema, and the list of
        // (markdown, file_path, type_def) for that mem's referrers.
        struct PeerMemPlan {
            mount_idx: usize,
            mem: String,
            writes: Vec<(
                String,
                String,
                std::sync::Arc<memstead_schema::TypeDefinition>,
            )>,
        }
        let mut peer_plans: std::collections::BTreeMap<String, PeerMemPlan> =
            std::collections::BTreeMap::new();
        for from_id in &cross_mem_write_ids {
            let peer_mem = from_id.mem().to_string();
            let peer_mount_idx = self
                .mounts
                .iter()
                .position(|m| m.mount.mem == peer_mem)
                .expect("peer mount present for collected referrer id");
            let peer_schema = self
                .schemas
                .get(&peer_mem)
                .expect("schema present for every registered mount");

            let Some(referrer) = self.store.get(from_id) else {
                continue;
            };
            if referrer.stub {
                continue;
            }
            let referrer_type_def = peer_schema
                .get_type(&referrer.entity_type)
                .ok_or_else(|| unknown_type_error(peer_schema, &referrer.entity_type))?;
            let mut next_ref = referrer.clone();
            for rel in next_ref.relationships.iter_mut() {
                if rel.target == *id {
                    rel.target = new_id.clone();
                }
            }
            // Cross-mem referrers reference the renaming entity
            // via the cross-mem wiki-link forms (`[[<mem>:<slug>]]`
            // or the legacy `[[<mem>--<slug>]]`). The bare-slug
            // form is reserved for same-mem refs and never appears
            // here.
            for body in next_ref.sections.values_mut() {
                let (rewritten, count) = crate::entity::wikilink_rewrite::rewrite_cross_mem_slug(
                    body,
                    &mem,
                    &old_slug,
                    &new_slug_owned,
                );
                if count > 0 {
                    *body = rewritten;
                }
            }
            // A cross-mem Write peer is a referrer too — preserve its
            // `last_modified` for the same reason as the same-mem case
            // above (slug rewrite is a foreign-key change, not a semantic
            // edit). Its content hash still changes; the staleness clock
            // does not reset.
            let ref_markdown = super::render_for_write(&next_ref, referrer_type_def.as_ref())?;

            peer_plans
                .entry(peer_mem.clone())
                .or_insert_with(|| PeerMemPlan {
                    mount_idx: peer_mount_idx,
                    mem: peer_mem.clone(),
                    writes: Vec::new(),
                })
                .writes
                .push((ref_markdown, next_ref.file_path.clone(), referrer_type_def));
        }

        // ----- Apply: renaming entity's own mem first -----
        // Mint a single `logical_operation_id` up front so every
        // commit produced by this rename — source mem + every peer
        // mem — carries the same correlation id in its provenance
        // entry. Single-mem renames also tag with an id (it just
        // maps to one commit); consumers branch on whether the id
        // recurs to identify a multi-commit logical operation.
        let logical_op_id = crate::provenance::mint_logical_operation_id();

        let backend = self.mounts[mount_idx].backend.as_ref();
        backend.write_entity(Path::new(&new_file_path), markdown.as_bytes())?;
        backend.delete_entity(Path::new(&old_file_path))?;
        for (ref_markdown, ref_file_path, _) in &same_mem_writes {
            backend.write_entity(Path::new(ref_file_path), ref_markdown.as_bytes())?;
        }
        // Move the renamed entity's anchor row old_id → new_id in the SAME
        // commit as the file move so entity + anchors rewind together and
        // resolution finds every anchor under the new id (zero under the
        // old). A no-op when the entity had no anchors (byte-identical).
        super::stage_anchors_rename(backend, id, &new_id)?;
        let commit_subject = format!("memstead: rename {} → {new_id}", id);
        let mut ctx = self.commit_context(
            Some("rename_entity"),
            actor,
            client.cloned(),
            note.map(String::from),
        );
        ctx.logical_operation_id = Some(logical_op_id.as_str());
        let write_id = backend.commit(&commit_subject, &ctx)?;

        backend.append_provenance(
            &Provenance::new(
                std::time::SystemTime::now(),
                ProvenanceKind::Rename,
                Some(new_id.to_string()),
                actor,
                client.cloned(),
                note.map(String::from),
            )
            .with_role(self.current_role)
            .with_identity(self.current_identity.clone())
            .with_logical_operation_id(logical_op_id.clone()),
        )?;

        self.record_self_write(mount_idx, &write_id);
        let stamp_warnings = self.stamp_mutation_versions(mount_idx);
        let mut peer_stamp_warnings: Vec<WarningHint> = Vec::new();

        // ----- Apply: cross-mem peer mems (parent-pinned) -----
        // Snapshot every peer mem's current head before any peer
        // writes begin. The snapshots are pinned through
        // `commit_with_expected_parent`, so a sibling writer that
        // advances a peer mem's head between snapshot and commit
        // aborts the commit with `BackendError::ParentMismatch`. The
        // engine layer maps this to `RENAME_PARTIAL_FAILURE` (the
        // source mem has already committed by this point — its
        // state is durable; only the failed peer's writes are lost).
        // Folder and archive backends inherit the trait's default
        // `commit_with_expected_parent` (which ignores the parent
        // and delegates to `commit`); the git-branch backend
        // overrides to check the per-mem branch tip.
        let mut peer_snapshots: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();
        for plan in peer_plans.values() {
            let peer_backend = self.mounts[plan.mount_idx].backend.as_ref();
            let snapshot = peer_backend.current_head()?;
            peer_snapshots.insert(plan.mem.clone(), snapshot);
        }

        // Track which mems have already committed in this logical
        // operation. On a peer-commit failure, the engine surfaces
        // the partial-state envelope so the agent can decide whether
        // to retry, reconcile, or accept.
        let mut committed_mems: Vec<String> = vec![mem.clone()];
        for plan in peer_plans.values() {
            let peer_backend = self.mounts[plan.mount_idx].backend.as_ref();
            for (ref_markdown, ref_file_path, _) in &plan.writes {
                peer_backend.write_entity(Path::new(ref_file_path), ref_markdown.as_bytes())?;
            }
            let peer_commit_subject = format!(
                "memstead: rename {} → {new_id} (cross-mem rewrite in `{}`)",
                id, plan.mem
            );
            let mut peer_ctx = self.commit_context(
                Some("rename_entity"),
                actor,
                client.cloned(),
                note.map(String::from),
            );
            peer_ctx.logical_operation_id = Some(logical_op_id.as_str());
            let expected = peer_snapshots.get(&plan.mem).cloned().unwrap_or(None);
            let peer_commit_result = peer_backend.commit_with_expected_parent(
                &peer_commit_subject,
                &peer_ctx,
                expected.as_deref(),
            );
            let peer_write_id = match peer_commit_result {
                Ok(sha) => sha,
                Err(crate::backend::BackendError::ParentMismatch { .. }) => {
                    return Err(EngineError::RenamePartialFailure {
                        committed_mems: std::mem::take(&mut committed_mems),
                        failed_mem: plan.mem.clone(),
                        failure_cause: "drift".to_string(),
                    });
                }
                Err(e) => return Err(e.into()),
            };
            peer_backend.append_provenance(
                &Provenance::new(
                    std::time::SystemTime::now(),
                    ProvenanceKind::Rename,
                    Some(new_id.to_string()),
                    actor,
                    client.cloned(),
                    note.map(String::from),
                )
                .with_role(self.current_role)
                .with_identity(self.current_identity.clone())
                .with_logical_operation_id(logical_op_id.clone()),
            )?;
            self.record_self_write(plan.mount_idx, &peer_write_id);
            // Per peer mount, not only the source mem: a rename touches every
            // pinned peer, and each one's config write can meet its own
            // intervening writer.
            peer_stamp_warnings.extend(self.stamp_mutation_versions(plan.mount_idx));
            committed_mems.push(plan.mem.clone());
        }

        // ----- Re-parse and push -----
        let parse_result = parse_markdown(&markdown, &new_file_path, type_def.as_ref(), &mem)
            .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
        let content_hash = parse_result.entity.content_hash.clone();

        let mut parse_results = vec![parse_result];
        for (ref_markdown, ref_file_path, ref_type_def) in &same_mem_writes {
            let pr = parse_markdown(ref_markdown, ref_file_path, ref_type_def.as_ref(), &mem)
                .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
            parse_results.push(pr);
        }
        for plan in peer_plans.values() {
            for (ref_markdown, ref_file_path, ref_type_def) in &plan.writes {
                let pr = parse_markdown(
                    ref_markdown,
                    ref_file_path,
                    ref_type_def.as_ref(),
                    &plan.mem,
                )
                .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
                parse_results.push(pr);
            }
        }

        // Residual-stub demotion for ReadOnly cross-mem referrers.
        // The engine can't rewrite ReadOnly-mount markdown, so the
        // wiki-links there still point at the OLD slug after the
        // rename. To keep `incoming(<new_id>)` aligned with what a
        // fresh boot would produce (and to surface the dangling
        // reference to the agent), we demote the OLD-id store entry
        // to a stub instead of removing it outright. Its surviving
        // `in_edges` from the ReadOnly mount remain valid — they
        // point at the now-stub at the old id.
        //
        // When no ReadOnly referrers exist, the old entry is
        // removed cleanly (the existing Write-path behaviour).
        let mut outcome_warnings: Vec<WarningHint> = Vec::new();
        outcome_warnings.extend(stamp_warnings);
        outcome_warnings.extend(peer_stamp_warnings);
        // Reload-before-operation drift notice, surfaced first.
        outcome_warnings.append(&mut drift_warnings);
        // Title↔slug divergence of the NEW title — same visibility
        // contract as create's.
        if !derivation.dropped_chars.is_empty() {
            outcome_warnings.push(WarningHint::TitleCharsDroppedFromSlug {
                title: args.new_title.trim().to_string(),
                dropped_chars: derivation.dropped_chars.clone(),
                slug: new_slug.clone(),
            });
        }
        if readonly_referrers.is_empty() {
            self.store.remove(id);
        } else {
            // Sever outgoing edges from the old id (the entity is
            // gone — its body and relations live at the new id now)
            // and replace the node with a stub at the same id. The
            // in_edges from the ReadOnly mount survive untouched.
            self.store.remove_edges_from(id);
            self.store.upsert(
                id.clone(),
                make_stub(
                    id,
                    crate::entity::StubKind::Residual {
                        since_commit: write_id.clone(),
                        readonly_referrers: readonly_referrers.clone(),
                    },
                ),
            );
            outcome_warnings.push(WarningHint::ResidualStubForReadOnlyReferrers {
                id: id.clone(),
                referrers: readonly_referrers,
            });
        }

        let fallback = engine_fallback_type();
        // Incremental (flywheel W8/01): the touched set is the OLD id
        // (its document must leave the index) plus every re-parsed
        // entity — the renamed entity at its new id and each rewritten
        // referrer.
        let mut touched: Vec<crate::EntityId> = parse_results
            .iter()
            .map(|pr| pr.entity.id.clone())
            .collect();
        touched.push(id.clone());
        push_entities_into_store(&mut self.store, parse_results, fallback.as_ref(), None);
        crate::entity::store_builder::remap_alias_target_edge_sources(
            &mut self.store,
            &self.schemas,
        );

        self.invalidate_communities();
        self.maintain_search_indexes(&touched);

        // `require_notes` provenance nudge — single engine-level
        // enforcement point. Only reached on the real-rename path; the
        // slug-noop short-circuit returns early above with an empty
        // `write_id` and never demands a note.
        if let Some(w) = self.note_missing_warning("rename_entity", note) {
            outcome_warnings.push(w);
        }

        Ok(RenameEntityOutcome {
            old_id: id.clone(),
            new_id,
            old_path: old_file_path,
            new_path: new_file_path,
            content_hash,
            write_id,
            warnings: outcome_warnings,
        })
    }
}

#[cfg(test)]
mod tests;
