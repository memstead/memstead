//! Resolve phase of a rename: find every entity that refers to the
//! renamed one, by relationship row or by a link in its body, and sort
//! the referrers by where the engine may write: the renamed entity's
//! own mem, other writable mems (gated by the cross-mem link policy),
//! and read-only mems, whose referrers stay as they are.

use crate::entity::EntityId;
use crate::workspace::MountCapability;

use super::{Engine, EngineError, RenameReferrers};

impl Engine {
    /// Who refers to `id`, sorted by where the rewrite can land.
    /// Refuses when the link policy forbids a peer mem's referrers to
    /// point into `mem`: nothing is written yet.
    pub(super) fn resolve_rename_referrers(
        &self,
        id: &EntityId,
        mem: &str,
    ) -> Result<RenameReferrers, EngineError> {
        let mem = mem.to_string();
        let old_slug = id.name().to_string();

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

        Ok(RenameReferrers {
            same_mem: same_mem_ids,
            cross_mem_write: cross_mem_write_ids,
            readonly: readonly_referrers,
        })
    }
}
