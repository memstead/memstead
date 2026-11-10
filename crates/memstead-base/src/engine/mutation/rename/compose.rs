//! Compose phase of a rename: render the renamed entity under its new
//! id, with its links to itself retargeted, and render every referrer
//! the engine may write with its relationship rows and body links
//! retargeted. Everything is rendered before anything is staged, so a
//! referrer that cannot be rendered refuses the rename with nothing
//! written.

use crate::entity::Entity;

use super::super::unknown_type_error;
use super::{Engine, EngineError, PeerMemPlan, ReferrerRewrite, RenameReferrers, ValidatedRename};

impl Engine {
    /// The renamed entity as it will be written, and its markdown.
    pub(super) fn compose_renamed_entity(
        &self,
        validated: &ValidatedRename,
    ) -> Result<(Entity, String), EngineError> {
        let (id, new_id, mem) = (&validated.id, &validated.new_id, &validated.mem);
        let (entity, type_def) = (&validated.entity, &validated.type_def);
        let new_slug = validated.derivation.slug.clone();

        let mut next = entity.clone();
        next.id = new_id.clone();
        next.title = validated.new_title.clone();
        next.file_path = format!("{new_slug}.md");

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
                mem,
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
        super::super::auto_stamp_timestamps(&mut next, type_def.as_ref(), &today);

        let markdown = super::super::render_for_write(&next, type_def.as_ref())?;

        Ok((next, markdown))
    }

    /// The rewrite of every referrer in a writable mem: the renamed
    /// entity's own mem first, then each peer mem's, grouped so each
    /// peer gets one commit.
    pub(super) fn compose_referrer_rewrites(
        &self,
        validated: &ValidatedRename,
        referrers: &RenameReferrers,
    ) -> Result<
        (
            Vec<ReferrerRewrite>,
            std::collections::BTreeMap<String, PeerMemPlan>,
        ),
        EngineError,
    > {
        let (id, new_id, mem) = (&validated.id, &validated.new_id, &validated.mem);
        let old_slug = id.name().to_string();
        let new_slug_owned = validated.derivation.slug.clone();
        let (same_mem_ids, cross_mem_write_ids) = (&referrers.same_mem, &referrers.cross_mem_write);
        let schema = self
            .schemas
            .get(mem)
            .expect("schema present for every registered mount");

        // ----- Same-mem rewrite plan -----
        // (rewritten_markdown, file_path, type_def) per same-mem
        // referrer — collected before any backend write so a
        // per-referrer schema or rewrite failure aborts the rename
        // before anything lands.
        let mut same_mem: Vec<ReferrerRewrite> = Vec::with_capacity(same_mem_ids.len());
        for from_id in same_mem_ids {
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
                    mem,
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
            let ref_markdown =
                super::super::render_for_write(&next_ref, referrer_type_def.as_ref())?;
            same_mem.push(ReferrerRewrite {
                id: from_id.clone(),
                markdown: ref_markdown,
                file_path: next_ref.file_path.clone(),
                type_def: referrer_type_def,
                before: referrer.sections.clone(),
                after: next_ref.sections.clone(),
            });
        }

        // ----- Cross-mem rewrite plan -----
        // Group cross-mem Write referrers by their mem so each
        // peer mem's backend gets one commit. Each entry holds the
        // peer mount index, the peer mem's schema, and the list of
        // (markdown, file_path, type_def) for that mem's referrers.
        let mut peer_plans: std::collections::BTreeMap<String, PeerMemPlan> =
            std::collections::BTreeMap::new();
        for from_id in cross_mem_write_ids {
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
            let before_sections = referrer.sections.clone();
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
                    mem,
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
            let ref_markdown =
                super::super::render_for_write(&next_ref, referrer_type_def.as_ref())?;

            let plan_entry = peer_plans
                .entry(peer_mem.clone())
                .or_insert_with(|| PeerMemPlan {
                    mount_idx: peer_mount_idx,
                    mem: peer_mem.clone(),
                    rewrites: Vec::new(),
                });
            plan_entry.rewrites.push(ReferrerRewrite {
                id: from_id.clone(),
                markdown: ref_markdown,
                file_path: next_ref.file_path.clone(),
                type_def: referrer_type_def,
                before: before_sections,
                after: next_ref.sections.clone(),
            });
        }

        Ok((same_mem, peer_plans))
    }
}
