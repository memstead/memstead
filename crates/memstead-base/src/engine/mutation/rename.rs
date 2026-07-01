//! `Engine::rename_entity` — change an entity's slug, move its file
//! on the backend, and rewrite the in-memory store.

use std::path::Path;

use crate::engine_fallback_type;
use crate::entity::generator::generate_markdown;
use crate::entity::id::validate_and_derive_slug;
use crate::entity::parser::parse_markdown;
use crate::entity::store_builder::push_entities_into_store;
use crate::entity::EntityId;
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
    /// **Same-vault referrers and self-references are rewritten
    /// atomically.** The renaming entity is treated as the first
    /// referrer of itself: every entry in its own `relationships`
    /// list whose target equals the old id is updated to point at
    /// the new id, and every `[[<old-slug>]]` token in its own
    /// section bodies is rewritten to the new slug (respecting
    /// fenced-code and inline-code masking). Every other entity in
    /// the same vault that pointed at the old id (via an explicit
    /// relation or an inline body wiki-link) gets the same two-
    /// surface rewrite. All rewrites land in one per-vault commit.
    /// Cross-vault referrers and ReadOnly-mount referrers are not
    /// yet walked — those land with the multi-vault atomicity
    /// machinery and the residual-stub demotion path respectively.
    pub fn rename_entity(
        &mut self,
        args: RenameEntityArgs,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<RenameEntityOutcome, EngineError> {
        let id = &args.id;
        let vault = id.vault().to_string();

        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.vault == vault)
            .ok_or_else(|| EngineError::UnknownVault(vault.clone()))?;
        if self.mounts[mount_idx].mount.capability != MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(vault));
        }

        // Reload-before-operation: reload if a sibling advanced the
        // vault ref so the `expected_hash` compare below runs against
        // current truth. The drift notice rides the outcome's
        // `warnings` (real-rename path).
        let mut drift_warnings = self.reload_if_stale(Some(&vault));

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
            return Err(EngineError::StubNotRenamable {
                id: id.to_string(),
            });
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

        let new_slug = validate_and_derive_slug(&args.new_title)?;
        let new_id = EntityId::new(&vault, &new_slug);
        crate::entity::id::enforce_id_length(new_id.as_ref())?;

        if new_id == *id {
            // Title-with-same-slug: surface a Tier-2 warning rather
            // than touching disk for nothing. Matches pro's
            // RenameResult slug-noop shape so autonomous skills
            // can distinguish the silent no-op from a successful
            // cosmetic rewrite via the typed warning code.
            return Ok(RenameEntityOutcome {
                old_id: id.clone(),
                new_id: new_id.clone(),
                old_path: entity.file_path.clone(),
                new_path: entity.file_path.clone(),
                content_hash: entity.content_hash.clone(),
                commit_sha: String::new(),
                warnings: vec![WarningHint::TitleNormalizedToSlugNoop {
                    requested_title: args.new_title.clone(),
                    current_slug: id.name().to_string(),
                }],
            });
        }
        if self.store.contains(&new_id) {
            return Err(EngineError::AlreadyExists {
                id: new_id.to_string(),
            });
        }

        let schema = self
            .schemas
            .get(&vault)
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
        // full-id form `[[vault--slug]]` (same vault) as references to
        // the same entity. The bare-slug rewriter only catches the
        // first; `rewrite_cross_vault_slug` (which already powers the
        // cross-vault Tier-2 rewrite) covers the second by matching
        // `<vault>--<slug>` and `<vault>:<slug>` forms. Calling both
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
            let (rewritten, count) = crate::entity::wikilink_rewrite::rewrite_cross_vault_slug(
                body,
                &vault,
                &old_slug,
                &new_slug_owned,
            );
            if count > 0 {
                *body = rewritten;
            }
        }

        // Every entity whose on-disk file the rename rewrites
        // (the renaming entity itself plus every Write-vault referrer
        // touched by the body/relationships rewrite cascade) gets
        // `auto_timestamp` metadata fields stamped before
        // `generate_markdown` so the new file carries the stamp.
        // Pre-compute `today` once so all entities rewritten by this
        // logical operation receive the same timestamp.
        let today = super::today_iso();
        super::auto_stamp_timestamps(&mut next, type_def.as_ref(), &today);

        let markdown = generate_markdown(&next, type_def.as_ref());

        // Referrer collection. Walk every incoming edge — explicit
        // relations and `EdgeSource::BodyLink` synthesised mirrors
        // alike — and bucket each unique referrer by its vault's
        // capability. Same-vault referrers are guaranteed Write (the
        // mount-capability gate at the top of this fn refused the
        // rename if the renaming vault itself isn't Write). Cross-
        // vault referrers split into Write (rewriteable, subject to
        // the `cross_vault_links` policy gate below) and ReadOnly
        // (the engine has no write access; their handling lands with
        // the rename-path residual-stub demotion in the next cut and
        // is filtered out here).
        let mut seen: std::collections::HashSet<EntityId> = std::collections::HashSet::new();
        let mut same_vault_ids: Vec<EntityId> = Vec::new();
        let mut cross_vault_ids: Vec<EntityId> = Vec::new();
        for in_edge in self.store.incoming(id) {
            if in_edge.from == *id || !seen.insert(in_edge.from.clone()) {
                continue;
            }
            if in_edge.from.vault() == vault {
                same_vault_ids.push(in_edge.from.clone());
            } else {
                cross_vault_ids.push(in_edge.from.clone());
            }
        }

        // Cross-vault peers are partitioned by mount capability.
        // Write peers feed the cross-vault rewrite plan; ReadOnly
        // peers feed the residual-stub demotion path (the engine
        // can't rewrite their on-disk markdown, so we materialise an
        // in-memory stub at the OLD id that holds the surviving
        // incoming edges from the ReadOnly mount — mirrors the
        // delete-path's same-shaped demotion).
        let mut cross_vault_write_ids: Vec<EntityId> = Vec::new();
        let mut readonly_referrers: Vec<EntityId> = Vec::new();
        for from_id in cross_vault_ids {
            match self
                .mount(from_id.vault())
                .map(|m| m.capability)
                .unwrap_or(MountCapability::Write)
            {
                MountCapability::Write => cross_vault_write_ids.push(from_id),
                MountCapability::ReadOnly => readonly_referrers.push(from_id),
            }
        }
        readonly_referrers.sort_by(|a, b| a.to_string().cmp(&b.to_string()));

        // Pre-flight policy gate. Each propagated referrer rewrite
        // is an edge of the form `referrer ∈ peer_vault → renamed ∈
        // vault` — same direction as the original edge. The gate
        // consults `cross_vault_link_allowed(peer_vault, vault)`,
        // which is the direction the policy gates new edges with
        // (forward-looking add-filter). A blocked peer aborts the
        // rename up-front — no writes have happened yet, so the
        // refusal is clean. This is the edge direction, not its
        // inverse.
        let mut blocked_counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for from_id in &cross_vault_write_ids {
            let peer_vault = from_id.vault().to_string();
            if !self.cross_vault_link_allowed(&peer_vault, &vault) {
                *blocked_counts.entry(peer_vault).or_insert(0) += 1;
            }
        }
        if !blocked_counts.is_empty() {
            let blocked_referrers: Vec<crate::engine::error::BlockedReferrer> = blocked_counts
                .into_iter()
                .map(|(peer_vault, count)| crate::engine::error::BlockedReferrer {
                    from_vault: peer_vault,
                    to_vault: vault.clone(),
                    count,
                })
                .collect();
            return Err(EngineError::RenameBlockedByCrossVaultPolicy {
                from_vault: vault.clone(),
                blocked_referrers,
            });
        }

        // Deterministic iteration order so the resulting per-vault
        // pending-op replay is stable across runs (helpful for test
        // snapshots and human reviewers).
        same_vault_ids.sort_by(|a, b| a.to_string().cmp(&b.to_string()));
        cross_vault_write_ids.sort_by(|a, b| a.to_string().cmp(&b.to_string()));

        // ----- Same-vault rewrite plan -----
        // (rewritten_markdown, file_path, type_def) per same-vault
        // referrer — collected before any backend write so a
        // per-referrer schema or rewrite failure aborts the rename
        // before anything lands.
        let mut same_vault_writes: Vec<(
            String,
            String,
            std::sync::Arc<memstead_schema::TypeDefinition>,
        )> = Vec::with_capacity(same_vault_ids.len());
        for from_id in &same_vault_ids {
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
                // A same-vault referrer may also use
                // full-id form `[[<vault>--<slug>]]` to point at the
                // renaming entity — covered by the cross-vault helper
                // which matches the `--` and `:` separator forms.
                let (rewritten, count) = crate::entity::wikilink_rewrite::rewrite_cross_vault_slug(
                    body,
                    &vault,
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
            let ref_markdown = generate_markdown(&next_ref, referrer_type_def.as_ref());
            same_vault_writes.push((
                ref_markdown,
                next_ref.file_path.clone(),
                referrer_type_def,
            ));
        }

        // ----- Cross-vault rewrite plan -----
        // Group cross-vault Write referrers by their vault so each
        // peer vault's backend gets one commit. Each entry holds the
        // peer mount index, the peer vault's schema, and the list of
        // (markdown, file_path, type_def) for that vault's referrers.
        struct PeerVaultPlan {
            mount_idx: usize,
            vault: String,
            writes: Vec<(String, String, std::sync::Arc<memstead_schema::TypeDefinition>)>,
        }
        let mut peer_plans: std::collections::BTreeMap<String, PeerVaultPlan> =
            std::collections::BTreeMap::new();
        for from_id in &cross_vault_write_ids {
            let peer_vault = from_id.vault().to_string();
            let peer_mount_idx = self
                .mounts
                .iter()
                .position(|m| m.mount.vault == peer_vault)
                .expect("peer mount present for collected referrer id");
            let peer_schema = self
                .schemas
                .get(&peer_vault)
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
            // Cross-vault referrers reference the renaming entity
            // via the cross-vault wiki-link forms (`[[<vault>:<slug>]]`
            // or the legacy `[[<vault>--<slug>]]`). The bare-slug
            // form is reserved for same-vault refs and never appears
            // here.
            for body in next_ref.sections.values_mut() {
                let (rewritten, count) =
                    crate::entity::wikilink_rewrite::rewrite_cross_vault_slug(
                        body,
                        &vault,
                        &old_slug,
                        &new_slug_owned,
                    );
                if count > 0 {
                    *body = rewritten;
                }
            }
            // A cross-vault Write peer is a referrer too — preserve its
            // `last_modified` for the same reason as the same-vault case
            // above (slug rewrite is a foreign-key change, not a semantic
            // edit). Its content hash still changes; the staleness clock
            // does not reset.
            let ref_markdown = generate_markdown(&next_ref, referrer_type_def.as_ref());

            peer_plans
                .entry(peer_vault.clone())
                .or_insert_with(|| PeerVaultPlan {
                    mount_idx: peer_mount_idx,
                    vault: peer_vault.clone(),
                    writes: Vec::new(),
                })
                .writes
                .push((ref_markdown, next_ref.file_path.clone(), referrer_type_def));
        }

        // ----- Apply: renaming entity's own vault first -----
        // Mint a single `logical_operation_id` up front so every
        // commit produced by this rename — source vault + every peer
        // vault — carries the same correlation id in its provenance
        // entry. Single-vault renames also tag with an id (it just
        // maps to one commit); consumers branch on whether the id
        // recurs to identify a multi-commit logical operation.
        let logical_op_id = crate::provenance::mint_logical_operation_id();

        let backend = self.mounts[mount_idx].backend.as_ref();
        backend.write_entity(Path::new(&new_file_path), markdown.as_bytes())?;
        backend.delete_entity(Path::new(&old_file_path))?;
        for (ref_markdown, ref_file_path, _) in &same_vault_writes {
            backend.write_entity(Path::new(ref_file_path), ref_markdown.as_bytes())?;
        }
        let commit_subject = format!("memstead: rename {} → {new_id}", id);
        let ctx = CommitContext {
            actor,
            client: client.cloned(),
            tool: Some("rename_entity"),
            note: note.map(String::from),
            logical_operation_id: Some(logical_op_id.as_str()),
            entity_ids: None,
        };
        let commit_sha = backend.commit(&commit_subject, &ctx)?;

        backend.append_provenance(
            &Provenance::new(
                std::time::SystemTime::now(),
                ProvenanceKind::Rename,
                Some(new_id.to_string()),
                actor,
                client.cloned(),
                note.map(String::from),
            )
            .with_logical_operation_id(logical_op_id.clone()),
        )?;

        self.record_self_write(mount_idx, &commit_sha);

        // ----- Apply: cross-vault peer vaults (parent-pinned) -----
        // Snapshot every peer vault's current head before any peer
        // writes begin. The snapshots are pinned through
        // `commit_with_expected_parent`, so a sibling writer that
        // advances a peer vault's head between snapshot and commit
        // aborts the commit with `BackendError::ParentMismatch`. The
        // engine layer maps this to `RENAME_PARTIAL_FAILURE` (the
        // source vault has already committed by this point — its
        // state is durable; only the failed peer's writes are lost).
        // Folder and archive backends inherit the trait's default
        // `commit_with_expected_parent` (which ignores the parent
        // and delegates to `commit`); the git-branch backend
        // overrides to check the per-vault branch tip.
        let mut peer_snapshots: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();
        for plan in peer_plans.values() {
            let peer_backend = self.mounts[plan.mount_idx].backend.as_ref();
            let snapshot = peer_backend.current_head()?;
            peer_snapshots.insert(plan.vault.clone(), snapshot);
        }

        // Track which vaults have already committed in this logical
        // operation. On a peer-commit failure, the engine surfaces
        // the partial-state envelope so the agent can decide whether
        // to retry, reconcile, or accept.
        let mut committed_vaults: Vec<String> = vec![vault.clone()];
        for plan in peer_plans.values() {
            let peer_backend = self.mounts[plan.mount_idx].backend.as_ref();
            for (ref_markdown, ref_file_path, _) in &plan.writes {
                peer_backend.write_entity(Path::new(ref_file_path), ref_markdown.as_bytes())?;
            }
            let peer_commit_subject =
                format!("memstead: rename {} → {new_id} (cross-vault rewrite in `{}`)", id, plan.vault);
            let peer_ctx = CommitContext {
                actor,
                client: client.cloned(),
                tool: Some("rename_entity"),
                note: note.map(String::from),
                logical_operation_id: Some(logical_op_id.as_str()),
                entity_ids: None,
            };
            let expected = peer_snapshots
                .get(&plan.vault)
                .cloned()
                .unwrap_or(None);
            let peer_commit_result = peer_backend.commit_with_expected_parent(
                &peer_commit_subject,
                &peer_ctx,
                expected.as_deref(),
            );
            let peer_commit_sha = match peer_commit_result {
                Ok(sha) => sha,
                Err(crate::backend::BackendError::ParentMismatch { .. }) => {
                    return Err(EngineError::RenamePartialFailure {
                        committed_vaults: std::mem::take(&mut committed_vaults),
                        failed_vault: plan.vault.clone(),
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
                .with_logical_operation_id(logical_op_id.clone()),
            )?;
            self.record_self_write(plan.mount_idx, &peer_commit_sha);
            committed_vaults.push(plan.vault.clone());
        }

        // ----- Re-parse and push -----
        let parse_result =
            parse_markdown(&markdown, &new_file_path, type_def.as_ref(), &vault)
                .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
        let content_hash = parse_result.entity.content_hash.clone();

        let mut parse_results = vec![parse_result];
        for (ref_markdown, ref_file_path, ref_type_def) in &same_vault_writes {
            let pr = parse_markdown(ref_markdown, ref_file_path, ref_type_def.as_ref(), &vault)
                .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
            parse_results.push(pr);
        }
        for plan in peer_plans.values() {
            for (ref_markdown, ref_file_path, ref_type_def) in &plan.writes {
                let pr = parse_markdown(
                    ref_markdown,
                    ref_file_path,
                    ref_type_def.as_ref(),
                    &plan.vault,
                )
                .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
                parse_results.push(pr);
            }
        }

        // Residual-stub demotion for ReadOnly cross-vault referrers.
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
        // Reload-before-operation drift notice, surfaced first.
        outcome_warnings.append(&mut drift_warnings);
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
                        since_commit: commit_sha.clone(),
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
        push_entities_into_store(
            &mut self.store,
            parse_results,
            fallback.as_ref(),
            None,
        );
        crate::entity::store_builder::remap_alias_target_edge_sources(
            &mut self.store,
            &self.schemas,
        );

        self.invalidate_communities();
        self.invalidate_search_indexes();

        // `require_notes` provenance nudge — single engine-level
        // enforcement point. Only reached on the real-rename path; the
        // slug-noop short-circuit returns early above with an empty
        // `commit_sha` and never demands a note.
        if let Some(w) = self.note_missing_warning("rename_entity", note) {
            outcome_warnings.push(w);
        }

        Ok(RenameEntityOutcome {
            old_id: id.clone(),
            new_id,
            old_path: old_file_path,
            new_path: new_file_path,
            content_hash,
            commit_sha,
            warnings: outcome_warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;

    use crate::backend::VaultBackend;
    use crate::engine::test_helpers::*;
    use crate::engine::{Engine, EngineError, RenameEntityArgs};
    use crate::ops::WarningHint;
    use crate::storage::FilesystemVaultWriter;

    #[test]
    fn rename_entity_renames_file_and_id_persists_across_restart() {
        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().to_path_buf();

        let (old_id, new_id, new_file) = {
            let writer = FilesystemVaultWriter::new(vault_dir.clone());
            let mut engine = Engine::from_mounts(vec![(
                folder_mount("specs", vault_dir.clone()),
                Box::new(writer) as Box<dyn VaultBackend>,
            )])
            .unwrap();
            let (actor, client) = cli_actor();
            let seeded = engine
                .create_entity(empty_create_args("specs", "Old Name"), actor, Some(&client), None)
                .unwrap();
            let outcome = engine
                .rename_entity(
                    RenameEntityArgs {
                        id: seeded.id.clone(),
                        expected_hash: Some(seeded.content_hash.clone()),
                        new_title: "New Name".to_string(),
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap();
            assert_eq!(outcome.old_id.to_string(), "specs--old-name");
            assert_eq!(outcome.new_id.to_string(), "specs--new-name");
            assert_eq!(outcome.new_path, "new-name.md");
            // Old file gone, new file present.
            assert!(!vault_dir.join(&outcome.old_path).exists());
            assert!(vault_dir.join(&outcome.new_path).exists());
            (outcome.old_id, outcome.new_id, outcome.new_path)
        };

        // New engine reading the same vault sees only the new id.
        let writer2 = FilesystemVaultWriter::new(vault_dir.clone());
        let engine2 = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir),
            Box::new(writer2) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        assert!(engine2.get_entity(&old_id).is_none());
        let new_entity = engine2.get_entity(&new_id).expect("new id must persist");
        assert_eq!(new_entity.title, "New Name");
        assert_eq!(new_entity.file_path, new_file);
    }

    #[test]
    fn rename_entity_returns_typed_warning_on_slug_noop() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Same Slug");
        let (actor, client) = cli_actor();
        let outcome = engine
            .rename_entity(
                RenameEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    new_title: "Same  Slug".to_string(), // slugifies to same
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        // Wire-shape parity with pro: slug-noop is Ok+warning, not
        // an error. Old/new IDs are equal; old/new paths are equal;
        // commit_sha is empty (no disk write); warnings carries the
        // typed TitleNormalizedToSlugNoop hint.
        assert_eq!(outcome.old_id, outcome.new_id);
        assert_eq!(outcome.old_path, outcome.new_path);
        assert!(outcome.commit_sha.is_empty());
        assert_eq!(outcome.warnings.len(), 1);
        assert!(matches!(
            outcome.warnings[0],
            WarningHint::TitleNormalizedToSlugNoop { .. }
        ));
    }

    #[test]
    fn rename_entity_returns_commit_sha_on_real_rename() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Old Name");
        let (actor, client) = cli_actor();
        let outcome = engine
            .rename_entity(
                RenameEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    new_title: "Brand New Name".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        // Real rename: commit_sha non-empty (folder backend produces
        // a synthetic CommitId), warnings empty, IDs differ.
        assert_ne!(outcome.old_id, outcome.new_id);
        assert!(
            !outcome.commit_sha.is_empty(),
            "commit_sha must be populated on a real rename"
        );
        assert!(outcome.warnings.is_empty());
    }

    #[test]
    fn rename_entity_rewrites_self_references_in_body_and_relationships() {
        use crate::engine::RelateEntityArgs;
        use crate::entity::EntityId;
        use indexmap::IndexMap;

        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().to_path_buf();
        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir.clone()),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        // Create the entity with a body section that contains a
        // self-reference. The slug is `old-name`; the body literally
        // names `[[old-name]]`. After rename, both surfaces — the
        // file on disk and the in-memory entity — must point at the
        // new slug.
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("identity".to_string(), "the seed identity".to_string());
        sections.insert(
            "purpose".to_string(),
            "see also [[old-name]] for prior context".to_string(),
        );
        // F11: the `[[old-name]]` self-reference body link does NOT
        // synthesise a self-edge (the alias pass drops vacuous self-edges
        // and emits `SELF_LINK_IGNORED`); `scan_wikilinks_without_relation`
        // also skips self-targets, so the unbacked self-link is admitted.
        // The body link still rewrites on rename — this test pins that the
        // body follows the slug while no self-relation is ever created.
        let seeded = engine
            .create_entity(
                crate::engine::CreateEntityArgs {
                    vault: "specs".to_string(),
                    title: "Old Name".to_string(),
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
        assert_eq!(seeded.id.to_string(), "specs--old-name");
        let related = seeded.clone();

        let outcome = engine
            .rename_entity(
                RenameEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(related.content_hash.clone()),
                    new_title: "Brand New Name".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(outcome.new_id.to_string(), "specs--brand-new-name");

        // File on disk reflects the rewrite — old slug must not
        // appear anywhere in the new file's bytes.
        let new_bytes = std::fs::read_to_string(vault_dir.join(&outcome.new_path)).unwrap();
        assert!(
            new_bytes.contains("[[brand-new-name]]"),
            "expected new slug in body, got:\n{new_bytes}"
        );
        assert!(
            !new_bytes.contains("[[old-name]]"),
            "old slug must not survive in the rewritten file, got:\n{new_bytes}"
        );

        // In-memory entity: section body rewritten, relationships
        // list points at the new id.
        let in_mem = engine.get_entity(&outcome.new_id).unwrap();
        assert!(
            in_mem
                .sections
                .get("purpose")
                .map(|s| s.contains("[[brand-new-name]]"))
                .unwrap_or(false),
            "section body must be rewritten in-memory; got {:?}",
            in_mem.sections.get("purpose")
        );
        // F11: no self-relation is ever synthesised — neither to the old
        // id nor (after the body rewrite) to the new id. The body link
        // followed the rename, but it produces no self-edge.
        let new_self_target = EntityId::new("specs", "brand-new-name");
        assert!(
            in_mem
                .relationships
                .iter()
                .all(|r| r.target != seeded.id && r.target != new_self_target),
            "a self-referential body link must produce no self-relation (F11), got: {:?}",
            in_mem.relationships
        );
    }

    /// A
    /// rename rewrites a referrer's body wiki-link to the new slug — a
    /// foreign-key change, not a semantic edit — so the referrer's
    /// `last_modified` staleness clock must NOT reset. Pre-written files
    /// carry an old `last_modified` (2020-01-01) so the assertion is
    /// distinctive: after a same-day rename the clock stays at the old
    /// date (it would jump to today if the re-commit still stamped it),
    /// while the body link is correctly rewritten.
    #[test]
    fn rename_preserves_referrer_last_modified_but_rewrites_link() {
        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().to_path_buf();

        std::fs::write(
            vault_dir.join("target.md"),
            "---\ntype: spec\ncreated_date: 2020-01-01\nlast_modified: 2020-01-01\nlevel: M0\n---\n# Target\n\n## Identity\n\nT\n\n## Purpose\n\nP\n",
        )
        .unwrap();
        std::fs::write(
            vault_dir.join("referrer.md"),
            "---\ntype: spec\ncreated_date: 2020-01-01\nlast_modified: 2020-01-01\nlevel: M0\n---\n# Referrer\n\n## Identity\n\nR\n\n## Purpose\n\nDepends on [[target]] for context.\n\n## Relationships\n\n- **REFERENCES**: [[target]]\n",
        )
        .unwrap();

        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir.clone()),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let target_id = crate::entity::EntityId::new("specs", "target");
        let target_hash = engine
            .store()
            .get(&target_id)
            .expect("target loaded from disk")
            .content_hash
            .clone();

        engine
            .rename_entity(
                RenameEntityArgs {
                    id: target_id,
                    expected_hash: Some(target_hash),
                    new_title: "Target Renamed".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .expect("rename succeeds");

        let referrer_md = std::fs::read_to_string(vault_dir.join("referrer.md")).unwrap();
        // Staleness clock preserved — NOT bumped to today.
        assert!(
            referrer_md.contains("last_modified: 2020-01-01"),
            "referrer's last_modified must be preserved across a rename-driven slug rewrite; got:\n{referrer_md}"
        );
        // Core rename job intact — the body wiki-link points at the new slug.
        assert!(
            referrer_md.contains("[[target-renamed]]"),
            "referrer's body wiki-link must be rewritten to the new slug; got:\n{referrer_md}"
        );
        assert!(
            !referrer_md.contains("[[target]]"),
            "old slug must not survive in the referrer body; got:\n{referrer_md}"
        );
    }

    #[test]
    fn rename_entity_rewrites_same_vault_referrers_atomically() {
        use crate::engine::{CreateEntityArgs, RelateEntityArgs};
        use crate::entity::EntityId;
        use indexmap::IndexMap;

        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().to_path_buf();
        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir.clone()),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        // Target — the entity that will be renamed.
        let target = engine
            .create_entity(empty_create_args("specs", "Target Spec"), actor, Some(&client), None)
            .unwrap();
        assert_eq!(target.id.to_string(), "specs--target-spec");

        // Referrer A — explicit relation declared atomically with the
        // body wiki-link. Both surfaces must be rewritten.
        let mut sections_a: IndexMap<String, String> = IndexMap::new();
        sections_a.insert("identity".to_string(), "referrer alpha identity".to_string());
        sections_a.insert(
            "purpose".to_string(),
            "rationale relies on [[target-spec]] for context".to_string(),
        );
        let referrer_a = engine
            .create_entity(
                CreateEntityArgs {
                    vault: "specs".to_string(),
                    title: "Referrer Alpha".to_string(),
                    entity_type: "spec".to_string(),
                    sections: sections_a,
                    metadata: IndexMap::new(),
                    // REFERENCES is engine-emitted from the body wiki-link
                    // via the alias-synthesis pass; explicit author is
                    // refused under `manual_authoring: forbidden`.
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Referrer B — second referrer with body wiki-link + atomic
        // backing relation. Confirms multi-referrer body rewrites.
        let mut sections_b: IndexMap<String, String> = IndexMap::new();
        sections_b.insert("identity".to_string(), "referrer beta identity".to_string());
        sections_b.insert(
            "purpose".to_string(),
            "consult [[target-spec]] for the canonical phrasing".to_string(),
        );
        let referrer_b = engine
            .create_entity(
                CreateEntityArgs {
                    vault: "specs".to_string(),
                    title: "Referrer Bravo".to_string(),
                    entity_type: "spec".to_string(),
                    sections: sections_b,
                    metadata: IndexMap::new(),
                    // REFERENCES is engine-emitted from the body wiki-link
                    // via the alias-synthesis pass; explicit author is
                    // refused under `manual_authoring: forbidden`.
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Bystander — no reference to the target. Must not be
        // touched on disk (its content_hash must be unchanged).
        let bystander = engine
            .create_entity(empty_create_args("specs", "Bystander"), actor, Some(&client), None)
            .unwrap();
        let bystander_bytes_before =
            std::fs::read_to_string(vault_dir.join(&bystander.file_path)).unwrap();

        let renamed = engine
            .rename_entity(
                RenameEntityArgs {
                    id: target.id.clone(),
                    expected_hash: Some(target.content_hash.clone()),
                    new_title: "Renamed Spec".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(renamed.new_id.to_string(), "specs--renamed-spec");

        // Old slug must not survive in any vault file — grep-clean,
        // scoped to this single-vault workspace.
        for path in std::fs::read_dir(&vault_dir).unwrap().flatten() {
            let p = path.path();
            if p.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let body = std::fs::read_to_string(&p).unwrap();
            assert!(
                !body.contains("[[target-spec]]"),
                "old slug must not survive in {}, got:\n{body}",
                p.display()
            );
        }

        // Referrer A's explicit relation now points at the new id.
        let in_mem_a = engine.get_entity(&referrer_a.id).unwrap();
        assert!(
            in_mem_a
                .relationships
                .iter()
                .any(|r| r.rel_type == "REFERENCES"
                    && r.target == EntityId::new("specs", "renamed-spec")),
            "expected referrer A's relation to point at renamed-spec, got {:?}",
            in_mem_a.relationships
        );
        assert!(
            in_mem_a
                .sections
                .get("purpose")
                .map(|s| s.contains("[[renamed-spec]]"))
                .unwrap_or(false),
            "referrer A's body must be rewritten"
        );

        // Referrer B's body is rewritten; it had no explicit
        // relation, so the relationships list stays empty.
        let in_mem_b = engine.get_entity(&referrer_b.id).unwrap();
        assert!(
            in_mem_b
                .sections
                .get("purpose")
                .map(|s| s.contains("[[renamed-spec]]"))
                .unwrap_or(false),
            "referrer B's body must be rewritten"
        );

        // Bystander untouched — exact byte equality on disk.
        let bystander_bytes_after =
            std::fs::read_to_string(vault_dir.join(&bystander.file_path)).unwrap();
        assert_eq!(
            bystander_bytes_before, bystander_bytes_after,
            "bystander must not be rewritten"
        );
    }

    /// Two-vault test scaffolding: build an engine with `specs` and
    /// `memos` Write mounts, and set `cross_vault_links` so each is
    /// permitted to link into the other. Returns the engine, both
    /// vault directories, and the actor/client tuple. The test then
    /// seeds whatever entities it needs.
    fn engine_with_two_vaults_and_bidirectional_policy(
        specs_dir: PathBuf,
        memos_dir: PathBuf,
    ) -> Engine {
        use memstead_schema::workspace_config::CrossLinkValue;
        let writer_specs = FilesystemVaultWriter::new(specs_dir.clone());
        let writer_memos = FilesystemVaultWriter::new(memos_dir.clone());
        let mut engine = Engine::from_mounts(vec![
            (
                folder_mount("specs", specs_dir),
                Box::new(writer_specs) as Box<dyn VaultBackend>,
            ),
            (
                folder_mount("memos", memos_dir),
                Box::new(writer_memos) as Box<dyn VaultBackend>,
            ),
        ])
        .unwrap();
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_vault_links.insert(
            "memos".to_string(),
            CrossLinkValue::List(vec!["specs".to_string()]),
        );
        settings.cross_vault_links.insert(
            "specs".to_string(),
            CrossLinkValue::List(vec!["memos".to_string()]),
        );
        engine.set_settings(settings);
        engine
    }

    #[test]
    fn rename_entity_rewrites_cross_vault_write_referrer() {
        use crate::engine::{CreateEntityArgs, RelateEntityArgs};
        use crate::entity::EntityId;
        use indexmap::IndexMap;

        let tmp_specs = TempDir::new().unwrap();
        let tmp_memos = TempDir::new().unwrap();
        let specs_dir = tmp_specs.path().to_path_buf();
        let memos_dir = tmp_memos.path().to_path_buf();
        let mut engine = engine_with_two_vaults_and_bidirectional_policy(
            specs_dir.clone(),
            memos_dir.clone(),
        );
        let (actor, client) = cli_actor();

        // Renaming target lives in `specs`.
        let target = engine
            .create_entity(empty_create_args("specs", "Target Spec"), actor, Some(&client), None)
            .unwrap();

        // Cross-vault referrer in `memos` has a body wiki-link in the
        // `:` form atomically backed by an explicit cross-vault relation.
        // The legacy `--` form is a same-vault nested-prefix drift
        // (resolves to `memos--specs--target-spec`); under the alias
        // model it cannot be backed and the engine surfaces it as
        // `SuspiciousNestedPrefix`, so it stays out of fresh fixtures.
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("claim".to_string(), "the claim".to_string());
        sections.insert(
            "context".to_string(),
            "discussion stems from [[specs:target-spec]]"
                .to_string(),
        );
        let referrer = engine
            .create_entity(
                CreateEntityArgs {
                    vault: "memos".to_string(),
                    title: "Cross Note".to_string(),
                    entity_type: "memo".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    // REFERENCES is engine-emitted from the body wiki-link
                    // via the alias-synthesis pass; explicit author is
                    // refused under `manual_authoring: forbidden`.
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Perform the rename.
        let renamed = engine
            .rename_entity(
                RenameEntityArgs {
                    id: target.id.clone(),
                    expected_hash: Some(target.content_hash.clone()),
                    new_title: "Renamed Spec".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(renamed.new_id.to_string(), "specs--renamed-spec");

        // Cross-vault referrer's on-disk file now carries the new
        // slug in the colon form and no `target-spec` remnants survive.
        let referrer_path = memos_dir.join(&referrer.file_path);
        let referrer_bytes = std::fs::read_to_string(&referrer_path).unwrap();
        assert!(
            referrer_bytes.contains("[[specs:renamed-spec]]"),
            "expected colon-form rewrite in referrer body, got:\n{referrer_bytes}"
        );
        assert!(
            !referrer_bytes.contains("target-spec"),
            "old slug must not survive in referrer file, got:\n{referrer_bytes}"
        );

        // In-memory referrer's relationship list points at the new id.
        let in_mem = engine.get_entity(&referrer.id).unwrap();
        assert!(
            in_mem
                .relationships
                .iter()
                .any(|r| r.rel_type == "REFERENCES"
                    && r.target == EntityId::new("specs", "renamed-spec")),
            "expected cross-vault relation to point at renamed-spec, got {:?}",
            in_mem.relationships
        );
        assert!(
            in_mem.relationships.iter().all(|r| r.target != target.id),
            "no relationship may still target the old id, got: {:?}",
            in_mem.relationships
        );
    }

    /// Wraps a real `VaultBackend` and forwards every method
    /// verbatim, except `commit_with_expected_parent` returns
    /// `BackendError::ParentMismatch` whenever the caller passes a
    /// non-`None` `expected_parent`. Models the "sibling writer
    /// advanced the head between snapshot and our commit" case
    /// without needing a real git-branch repository.
    struct DriftingBackend {
        inner: Box<dyn VaultBackend>,
    }
    impl DriftingBackend {
        fn new(inner: Box<dyn VaultBackend>) -> Self {
            Self { inner }
        }
    }
    impl crate::backend::VaultBackend for DriftingBackend {
        fn list_entities(&self) -> Result<Vec<PathBuf>, crate::backend::BackendError> {
            self.inner.list_entities()
        }
        fn read_entity(
            &self,
            rel: &std::path::Path,
        ) -> Result<Option<Vec<u8>>, crate::backend::BackendError> {
            self.inner.read_entity(rel)
        }
        fn write_entity(
            &self,
            rel: &std::path::Path,
            b: &[u8],
        ) -> Result<(), crate::backend::BackendError> {
            self.inner.write_entity(rel, b)
        }
        fn delete_entity(
            &self,
            rel: &std::path::Path,
        ) -> Result<(), crate::backend::BackendError> {
            self.inner.delete_entity(rel)
        }
        fn move_entity(
            &self,
            f: &std::path::Path,
            t: &std::path::Path,
        ) -> Result<(), crate::backend::BackendError> {
            self.inner.move_entity(f, t)
        }
        fn commit(
            &self,
            m: &str,
            c: &crate::vcs::CommitContext<'_>,
        ) -> Result<crate::storage::CommitId, crate::backend::BackendError> {
            self.inner.commit(m, c)
        }
        fn commit_with_expected_parent(
            &self,
            m: &str,
            c: &crate::vcs::CommitContext<'_>,
            expected_parent: Option<&str>,
        ) -> Result<crate::storage::CommitId, crate::backend::BackendError> {
            if expected_parent.is_some() {
                Err(crate::backend::BackendError::ParentMismatch {
                    expected: expected_parent.unwrap().to_string(),
                    actual: "drifted-by-sibling-writer".to_string(),
                })
            } else {
                self.inner.commit(m, c)
            }
        }
        fn append_provenance(
            &self,
            r: &crate::Provenance,
        ) -> Result<(), crate::backend::BackendError> {
            self.inner.append_provenance(r)
        }
        fn read_provenance(
            &self,
            c: Option<&str>,
        ) -> Result<Vec<crate::Provenance>, crate::backend::BackendError> {
            self.inner.read_provenance(c)
        }
        fn current_head(&self) -> Result<Option<String>, crate::backend::BackendError> {
            // Return a non-None head so the rename's snapshot is
            // populated and the parent-pin path is exercised.
            Ok(Some("snapshot-head-sha".to_string()))
        }
    }

    #[test]
    fn rename_entity_surfaces_partial_failure_when_peer_vault_drifts() {
        use crate::engine::{CreateEntityArgs, RelateEntityArgs};
        use indexmap::IndexMap;
        use memstead_schema::workspace_config::CrossLinkValue;

        let tmp_specs = TempDir::new().unwrap();
        let tmp_memos = TempDir::new().unwrap();
        let specs_dir = tmp_specs.path().to_path_buf();
        let memos_dir = tmp_memos.path().to_path_buf();

        // specs uses a plain filesystem backend; memos uses one
        // wrapped in DriftingBackend so its peer-vault commit during
        // rename fails with ParentMismatch (the parent-pin tripped
        // by a hypothetical sibling writer).
        let writer_specs = FilesystemVaultWriter::new(specs_dir.clone());
        let writer_memos_inner: Box<dyn VaultBackend> =
            Box::new(FilesystemVaultWriter::new(memos_dir.clone()));
        let writer_memos = DriftingBackend::new(writer_memos_inner);

        let mut engine = Engine::from_mounts(vec![
            (
                folder_mount("specs", specs_dir.clone()),
                Box::new(writer_specs) as Box<dyn VaultBackend>,
            ),
            (
                folder_mount("memos", memos_dir.clone()),
                Box::new(writer_memos) as Box<dyn VaultBackend>,
            ),
        ])
        .unwrap();
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_vault_links.insert(
            "memos".to_string(),
            CrossLinkValue::List(vec!["specs".to_string()]),
        );
        settings.cross_vault_links.insert(
            "specs".to_string(),
            CrossLinkValue::List(vec!["memos".to_string()]),
        );
        engine.set_settings(settings);

        let (actor, client) = cli_actor();
        let target = engine
            .create_entity(
                empty_create_args("specs", "Target Spec"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("claim".to_string(), "the claim".to_string());
        sections.insert(
            "context".to_string(),
            "see [[specs:target-spec]]".to_string(),
        );
        let referrer = engine
            .create_entity(
                CreateEntityArgs {
                    vault: "memos".to_string(),
                    title: "Cross Note".to_string(),
                    entity_type: "memo".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    // REFERENCES is engine-emitted from the body wiki-link
                    // via the alias-synthesis pass; explicit author is
                    // refused under `manual_authoring: forbidden`.
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let err = engine
            .rename_entity(
                RenameEntityArgs {
                    id: target.id.clone(),
                    expected_hash: Some(target.content_hash.clone()),
                    new_title: "Renamed Spec".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::RenamePartialFailure {
                committed_vaults,
                failed_vault,
                failure_cause,
            } => {
                // The renaming entity's own vault committed before
                // the peer-vault commit was attempted, so it must be
                // listed as already-committed.
                assert_eq!(committed_vaults, vec!["specs".to_string()]);
                assert_eq!(failed_vault, "memos");
                assert_eq!(failure_cause, "drift");
            }
            other => panic!("expected RenamePartialFailure, got {other:?}"),
        }
        // The renaming entity's own vault has the new file (its
        // commit landed) — that's the whole point of the partial-
        // failure envelope: source vault is durable, peer is not.
        assert!(specs_dir.join("renamed-spec.md").exists());
        assert!(!specs_dir.join(&target.file_path).exists());
    }

    #[test]
    fn rename_entity_tags_every_per_vault_commit_with_same_logical_operation_id() {
        use crate::backend::VaultBackend;
        use crate::engine::{CreateEntityArgs, RelateEntityArgs};
        use indexmap::IndexMap;

        let tmp_specs = TempDir::new().unwrap();
        let tmp_memos = TempDir::new().unwrap();
        let specs_dir = tmp_specs.path().to_path_buf();
        let memos_dir = tmp_memos.path().to_path_buf();
        let mut engine = engine_with_two_vaults_and_bidirectional_policy(
            specs_dir.clone(),
            memos_dir.clone(),
        );
        let (actor, client) = cli_actor();

        let target = engine
            .create_entity(empty_create_args("specs", "Target Spec"), actor, Some(&client), None)
            .unwrap();
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("claim".to_string(), "the claim".to_string());
        sections.insert(
            "context".to_string(),
            "discussion stems from [[specs:target-spec]]".to_string(),
        );
        let referrer = engine
            .create_entity(
                CreateEntityArgs {
                    vault: "memos".to_string(),
                    title: "Cross Note".to_string(),
                    entity_type: "memo".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    // REFERENCES is engine-emitted from the body wiki-link
                    // via the alias-synthesis pass; explicit author is
                    // refused under `manual_authoring: forbidden`.
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let _ = engine
            .rename_entity(
                RenameEntityArgs {
                    id: target.id.clone(),
                    expected_hash: Some(target.content_hash.clone()),
                    new_title: "Renamed Spec".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Read provenance from each vault's backend and find the
        // rename entries. Both vaults must record a Rename entry, and
        // both entries must share the same logical_operation_id.
        let specs_backend: Box<dyn VaultBackend> =
            Box::new(FilesystemVaultWriter::new(specs_dir.clone()));
        let memos_backend: Box<dyn VaultBackend> =
            Box::new(FilesystemVaultWriter::new(memos_dir.clone()));
        let specs_provenance = specs_backend.read_provenance(None).unwrap();
        let memos_provenance = memos_backend.read_provenance(None).unwrap();

        let specs_rename = specs_provenance
            .iter()
            .find(|p| matches!(p.kind, crate::provenance::ProvenanceKind::Rename))
            .expect("specs vault must have a rename provenance entry");
        let memos_rename = memos_provenance
            .iter()
            .find(|p| matches!(p.kind, crate::provenance::ProvenanceKind::Rename))
            .expect("memos vault must have a rename provenance entry");

        let specs_id = specs_rename
            .logical_operation_id
            .as_deref()
            .expect("specs rename entry must carry a logical_operation_id");
        let memos_id = memos_rename
            .logical_operation_id
            .as_deref()
            .expect("memos rename entry must carry a logical_operation_id");
        assert_eq!(
            specs_id, memos_id,
            "both per-vault rename commits must share the same logical_operation_id"
        );
        assert!(
            specs_id.starts_with("logop-"),
            "logical_operation_id must use the `logop-` prefix the engine mints; got {specs_id}"
        );
    }

    #[test]
    fn rename_entity_refuses_when_cross_vault_referrer_blocked_by_policy() {
        use crate::engine::{CreateEntityArgs, RelateEntityArgs};
        use indexmap::IndexMap;
        use memstead_schema::workspace_config::CrossLinkValue;

        let tmp_specs = TempDir::new().unwrap();
        let tmp_memos = TempDir::new().unwrap();
        let specs_dir = tmp_specs.path().to_path_buf();
        let memos_dir = tmp_memos.path().to_path_buf();

        // Start with full policy so the create + cross-vault relate
        // succeed during setup.
        let mut engine = engine_with_two_vaults_and_bidirectional_policy(
            specs_dir.clone(),
            memos_dir.clone(),
        );
        let (actor, client) = cli_actor();

        let target = engine
            .create_entity(empty_create_args("specs", "Target Spec"), actor, Some(&client), None)
            .unwrap();
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("claim".to_string(), "the claim".to_string());
        sections.insert(
            "context".to_string(),
            "see [[specs:target-spec]]".to_string(),
        );
        let referrer = engine
            .create_entity(
                CreateEntityArgs {
                    vault: "memos".to_string(),
                    title: "Cross Note".to_string(),
                    entity_type: "memo".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    // REFERENCES is engine-emitted from the body wiki-link
                    // via the alias-synthesis pass; explicit author is
                    // refused under `manual_authoring: forbidden`.
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Tighten policy: revoke `memos → specs`, which is the
        // direction of the existing referrer edge (`memos--cross-note
        // REFERENCES specs--target-spec`). The propagated rewrite
        // preserves that direction, so the rename gate must refuse
        // up-front with the now-blocked direction named.
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_vault_links.insert(
            "specs".to_string(),
            CrossLinkValue::List(vec!["memos".to_string()]),
        );
        // No entry for `memos` → `cross_vault_link_allowed("memos", "specs")` = false.
        engine.set_settings(settings);

        let err = engine
            .rename_entity(
                RenameEntityArgs {
                    id: target.id.clone(),
                    expected_hash: Some(target.content_hash.clone()),
                    new_title: "Renamed Spec".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::RenameBlockedByCrossVaultPolicy {
                from_vault,
                blocked_referrers,
            } => {
                assert_eq!(from_vault, "specs");
                assert_eq!(blocked_referrers.len(), 1);
                assert_eq!(blocked_referrers[0].from_vault, "memos");
                assert_eq!(blocked_referrers[0].to_vault, "specs");
                assert_eq!(blocked_referrers[0].count, 1);
            }
            other => panic!("expected RenameBlockedByCrossVaultPolicy, got {other:?}"),
        }
        // Nothing landed: target file is still at the old path, no
        // new file was created.
        assert!(specs_dir.join(&target.file_path).exists());
        assert!(!specs_dir.join("renamed-spec.md").exists());
    }

    /// Rename target has no Write-vault referrers but is referenced
    /// from a ReadOnly archive. The rename rewrites the renaming
    /// entity's own vault but cannot reach into the archive — the
    /// archive's wiki-link still points at the old slug. To keep
    /// `incoming(<new_id>)` aligned with what a fresh boot would
    /// produce and surface the dangling reference, the OLD-id store
    /// entry is demoted to a stub holding the surviving archive
    /// incoming edges, with a `ResidualStubForReadOnlyReferrers`
    /// warning on the outcome. Mirrors delete-path's same-shaped
    /// demotion.
    #[test]
    fn rename_entity_demotes_to_stub_when_only_readonly_cross_vault_referrers_remain() {
        use crate::engine::test_helpers::{archive_mount, build_archive};
        use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

        let tmp = TempDir::new().unwrap();
        let writable_dir = tmp.path().join("writable");
        std::fs::create_dir_all(&writable_dir).unwrap();
        let writer = FilesystemVaultWriter::new(writable_dir.clone());

        // Archive entity declares an explicit cross-vault relation
        // into the writable vault; under the alias model every edge
        // originates from `## Relationships`.
        let archive_md = "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# Archived Source\n\n## Identity\n\nLinks to [[specs:target]].\n\n## Purpose\n\nFixture for rename residual-stub demotion.\n\n## Relationships\n\n- **REFERENCES**: [[specs:target]]\n";
        let archive_path = build_archive(
            tmp.path(),
            "archive",
            &[("archived-source.md", archive_md.as_bytes())],
        );

        let folder_mount = Mount {
            vault: "specs".to_string(),
            schema: Some(crate::engine::test_helpers::pin("default")),
            storage: MountStorage::Folder {
                path: writable_dir.clone(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let archive_reader = crate::storage::ArchiveBackend::new(archive_path.clone());
        let mut engine = Engine::from_mounts(vec![
            (folder_mount, Box::new(writer) as Box<dyn VaultBackend>),
            (
                archive_mount("archive", archive_path.clone()),
                Box::new(archive_reader) as Box<dyn VaultBackend>,
            ),
        ])
        .unwrap();

        let (actor, client) = cli_actor();
        let target = engine
            .create_entity(empty_create_args("specs", "Target"), actor, Some(&client), None)
            .unwrap();

        // Sanity check: the archive's wiki-link produces an incoming
        // edge on the target.
        let archived_source_id = crate::EntityId::new("archive", "archived-source");
        let incoming_pre: Vec<_> = engine
            .store()
            .incoming(&target.id)
            .iter()
            .map(|e| e.from.clone())
            .collect();
        assert!(
            incoming_pre.contains(&archived_source_id),
            "archive wiki-link must produce an incoming edge on target; got {incoming_pre:?}"
        );

        // Rename. The archive can't be rewritten; engine demotes the
        // OLD-id store entry to a stub and emits the warning.
        let outcome = engine
            .rename_entity(
                RenameEntityArgs {
                    id: target.id.clone(),
                    expected_hash: Some(target.content_hash.clone()),
                    new_title: "Renamed Target".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(outcome.new_id.to_string(), "specs--renamed-target");

        // New file landed.
        assert!(writable_dir.join(&outcome.new_path).exists());
        // Old slug demoted to a stub at the original id.
        let demoted = engine
            .get_entity(&target.id)
            .expect("residual stub must remain at old id");
        assert!(demoted.stub, "demoted entity must be flagged as stub");
        assert!(demoted.entity_type.is_empty());
        // Archive's incoming edge still points at the old id (the
        // archive markdown wasn't rewritten).
        let incoming_old: Vec<_> = engine
            .store()
            .incoming(&target.id)
            .iter()
            .map(|e| e.from.clone())
            .collect();
        assert!(
            incoming_old.contains(&archived_source_id),
            "archive incoming edge must survive demotion at old id; got {incoming_old:?}"
        );
        // New id has no incoming edge from the archive (its wiki-link
        // points at the old slug, not the new one).
        let incoming_new: Vec<_> = engine
            .store()
            .incoming(&outcome.new_id)
            .iter()
            .map(|e| e.from.clone())
            .collect();
        assert!(
            !incoming_new.contains(&archived_source_id),
            "archive must not be wired to new id (markdown still references old slug); got {incoming_new:?}"
        );
        // Warning carries the surviving referrer.
        let referrers = outcome
            .warnings
            .iter()
            .find_map(|w| match w {
                WarningHint::ResidualStubForReadOnlyReferrers { id: warn_id, referrers } => {
                    assert_eq!(warn_id, &target.id);
                    Some(referrers.clone())
                }
                _ => None,
            })
            .expect("ResidualStubForReadOnlyReferrers warning must surface");
        assert_eq!(referrers, vec![archived_source_id]);
    }

    /// A same-vault referrer whose body uses the full-id form
    /// `[[<vault>--<slug>]]` to point at the renaming entity must have
    /// that token retargeted to the new slug. Pre-fix the rewrite
    /// pass only matched short-form `[[<slug>]]`; full-id tokens
    /// survived and pointed at the dead id. The new code calls
    /// `rewrite_cross_vault_slug` on the same-vault path alongside
    /// the bare-slug helper, covering both legal slug-form variants.
    #[test]
    fn rename_entity_rewrites_full_id_form_body_link_on_same_vault_referrer() {
        use crate::engine::CreateEntityArgs;
        use indexmap::IndexMap;
        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().to_path_buf();
        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir.clone()),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        // Target entity to rename.
        let target = engine
            .create_entity(empty_create_args("specs", "Target Spec"), actor, Some(&client), None)
            .unwrap();
        assert_eq!(target.id.to_string(), "specs--target-spec");

        // Referrer body uses the full-id form `[[specs--target-spec]]`.
        // The body parser admits both short and full-id forms; the
        // alias-synthesis pass emits one REFERENCES edge regardless
        // of which form the author wrote.
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("identity".to_string(), "referrer identity".to_string());
        sections.insert(
            "purpose".to_string(),
            "see also [[specs--target-spec]] for context".to_string(),
        );
        let referrer = engine
            .create_entity(
                CreateEntityArgs {
                    vault: "specs".to_string(),
                    title: "Referrer Full".to_string(),
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

        let renamed = engine
            .rename_entity(
                RenameEntityArgs {
                    id: target.id.clone(),
                    expected_hash: Some(target.content_hash.clone()),
                    new_title: "Renamed Spec".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(renamed.new_id.to_string(), "specs--renamed-spec");

        // The full-id form was retargeted (slug part rewritten, full-id
        // form preserved). The old slug must not survive in the
        // referrer's on-disk file.
        let referrer_path = vault_dir.join(&referrer.file_path);
        let body = std::fs::read_to_string(&referrer_path).unwrap();
        assert!(
            body.contains("[[specs--renamed-spec]]"),
            "expected full-id form retargeted to new slug, got:\n{body}"
        );
        assert!(
            !body.contains("target-spec"),
            "old slug must not survive in any form, got:\n{body}"
        );
    }

    #[test]
    fn rename_entity_rejects_collision_with_existing_id() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, first) = engine_with_seed(&tmp, "First");
        let (actor, client) = cli_actor();
        let _ = engine
            .create_entity(empty_create_args("specs", "Second"), actor, Some(&client), None)
            .unwrap();
        // Rename `first` to a title that slugifies to `second`.
        let err = engine
            .rename_entity(
                RenameEntityArgs {
                    id: first.id.clone(),
                    expected_hash: Some(first.content_hash.clone()),
                    new_title: "Second".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::AlreadyExists { id } if id == "specs--second"));
    }

    /// With `test → other` granted, an `IMPLEMENTS` edge from
    /// `test--src` to `other--target` is created. Revoking `test →
    /// other` (the actual edge direction) must block the rename
    /// up-front — pre-fix the gate checked the inverse direction
    /// (`other → test`), which was un-granted in both phases, so the
    /// rename refused for the wrong reason.
    #[test]
    fn rename_propagation_gate_checks_actual_edge_direction() {
        use crate::engine::{CreateEntityArgs, RelateEntityArgs};
        use crate::engine::error::BlockedReferrer;
        use indexmap::IndexMap;
        use memstead_schema::workspace_config::CrossLinkValue;

        let tmp_test = TempDir::new().unwrap();
        let tmp_other = TempDir::new().unwrap();
        let test_dir = tmp_test.path().to_path_buf();
        let other_dir = tmp_other.path().to_path_buf();

        // Pretty-print scaffold: `test` and `other` are the
        // canonical vault names. Reuse the helper by ignoring the
        // returned dirs and overriding the policy explicitly.
        let writer_test = FilesystemVaultWriter::new(test_dir.clone());
        let writer_other = FilesystemVaultWriter::new(other_dir.clone());
        let mut engine = Engine::from_mounts(vec![
            (
                folder_mount("test", test_dir.clone()),
                Box::new(writer_test) as Box<dyn VaultBackend>,
            ),
            (
                folder_mount("other", other_dir.clone()),
                Box::new(writer_other) as Box<dyn VaultBackend>,
            ),
        ])
        .unwrap();
        let (actor, client) = cli_actor();

        // Setup policy: only `test → other` granted. Create the edge
        // `test--src IMPLEMENTS other--target`.
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_vault_links.insert(
            "test".to_string(),
            CrossLinkValue::List(vec!["other".to_string()]),
        );
        engine.set_settings(settings);

        let target = engine
            .create_entity(empty_create_args("other", "Target"), actor, Some(&client), None)
            .unwrap();
        let mut src_sections: IndexMap<String, String> = IndexMap::new();
        src_sections.insert("identity".to_string(), "source identity".to_string());
        src_sections.insert("purpose".to_string(), "source purpose".to_string());
        let src = engine
            .create_entity(
                CreateEntityArgs {
                    vault: "test".to_string(),
                    title: "Src".to_string(),
                    entity_type: "spec".to_string(),
                    sections: src_sections,
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let src = engine
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
            .unwrap();
        let _ = src; // sink — we won't use the post-relate hash again

        // Revoke `test → other`. The post-rename rewrite would
        // re-emit the `test → other` edge, which the gate must now
        // refuse — pre-fix the inverted check passed because nothing
        // ever gated the right direction.
        engine.set_settings(crate::workspace::WorkspaceSettings::default());

        let err = engine
            .rename_entity(
                RenameEntityArgs {
                    id: target.id.clone(),
                    expected_hash: Some(target.content_hash.clone()),
                    new_title: "Renamed".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::RenameBlockedByCrossVaultPolicy {
                from_vault,
                blocked_referrers,
            } => {
                assert_eq!(from_vault, "other");
                assert_eq!(
                    blocked_referrers,
                    vec![BlockedReferrer {
                        from_vault: "test".to_string(),
                        to_vault: "other".to_string(),
                        count: 1,
                    }],
                    "blocked_referrers must name the actual edge direction (test → other)",
                );
            }
            other => panic!("expected RenameBlockedByCrossVaultPolicy, got {other:?}"),
        }
        // No write landed in either vault: target file path
        // unchanged, the new slug file absent.
        assert!(other_dir.join(&target.file_path).exists());
        assert!(!other_dir.join("renamed.md").exists());
    }

    /// Re-granting the actual edge
    /// direction lets the same rename succeed. Verifies the gate
    /// fires only on the *un-granted* direction.
    #[test]
    fn rename_propagation_succeeds_when_actual_edge_direction_granted() {
        use crate::engine::{CreateEntityArgs, RelateEntityArgs};
        use indexmap::IndexMap;
        use memstead_schema::workspace_config::CrossLinkValue;

        let tmp_test = TempDir::new().unwrap();
        let tmp_other = TempDir::new().unwrap();
        let test_dir = tmp_test.path().to_path_buf();
        let other_dir = tmp_other.path().to_path_buf();

        let writer_test = FilesystemVaultWriter::new(test_dir.clone());
        let writer_other = FilesystemVaultWriter::new(other_dir.clone());
        let mut engine = Engine::from_mounts(vec![
            (
                folder_mount("test", test_dir),
                Box::new(writer_test) as Box<dyn VaultBackend>,
            ),
            (
                folder_mount("other", other_dir),
                Box::new(writer_other) as Box<dyn VaultBackend>,
            ),
        ])
        .unwrap();
        let (actor, client) = cli_actor();

        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_vault_links.insert(
            "test".to_string(),
            CrossLinkValue::List(vec!["other".to_string()]),
        );
        engine.set_settings(settings);

        let target = engine
            .create_entity(empty_create_args("other", "Target"), actor, Some(&client), None)
            .unwrap();
        let mut src_sections: IndexMap<String, String> = IndexMap::new();
        src_sections.insert("identity".to_string(), "source identity".to_string());
        src_sections.insert("purpose".to_string(), "source purpose".to_string());
        let src = engine
            .create_entity(
                CreateEntityArgs {
                    vault: "test".to_string(),
                    title: "Src".to_string(),
                    entity_type: "spec".to_string(),
                    sections: src_sections,
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let _ = engine
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
            .unwrap();

        // Policy unchanged from setup (`test → other` still granted)
        // — rename must succeed and rewrite the cross-vault referrer.
        let outcome = engine
            .rename_entity(
                RenameEntityArgs {
                    id: target.id.clone(),
                    expected_hash: Some(target.content_hash.clone()),
                    new_title: "Renamed".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_ne!(outcome.old_id, outcome.new_id);
        let renamed = engine
            .get_entity(&outcome.new_id)
            .expect("renamed entity persists");
        assert_eq!(renamed.title, "Renamed");
        // Referrer rewritten — IMPLEMENTS edge now points at the new id.
        let updated_src = engine.get_entity(&src.id).expect("source persists");
        assert!(
            updated_src
                .relationships
                .iter()
                .any(|r| r.rel_type == "IMPLEMENTS" && r.target == outcome.new_id),
            "referrer's IMPLEMENTS edge must point at the new id after rewrite"
        );
    }

    /// A rename whose target has no
    /// cross-vault referrers bypasses the gate entirely. Even with
    /// a fully-empty cross-link policy, the rename succeeds.
    #[test]
    fn rename_with_no_cross_vault_referrers_succeeds_regardless_of_policy() {
        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().to_path_buf();
        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        // Default settings: no cross-vault links at all. The
        // single-vault rename's referrers (if any) are all same-vault
        // and bypass the gate by construction.
        let target = engine
            .create_entity(empty_create_args("specs", "Target"), actor, Some(&client), None)
            .unwrap();
        let outcome = engine
            .rename_entity(
                RenameEntityArgs {
                    id: target.id.clone(),
                    expected_hash: Some(target.content_hash.clone()),
                    new_title: "Renamed Target".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_ne!(outcome.old_id, outcome.new_id);
    }
}
