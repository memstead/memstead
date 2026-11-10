//! Mem-rename content sweep — the referrer-rewrite half of
//! `memstead mem rename`.
//!
//! Entity ids are derived from `(mount name, file path)`, so renaming a
//! mount re-ids every entity in it for free; what the rename must
//! rewrite by hand is every **textual** `<old>--<slug>` / `<old>:<slug>`
//! reference across the workspace — cross-mem wiki-links and
//! `## Relationships` entries in peer mems, full-id self-references in
//! the renamed mem itself, and the renamed mem's anchors-sidecar keys.
//!
//! The sweep is a raw-text pass over each entity file
//! ([`crate::entity::wikilink_rewrite::rewrite_mem_prefix`], sharing the
//! parser's code-masking discipline) rather than a parse → mutate →
//! regenerate round-trip: formatting outside the rewritten links stays
//! byte-identical, and both link surfaces (bodies and the
//! `## Relationships` section) are covered by the one pass because both
//! are wiki-links.
//!
//! The sweep holds every writable mem's write lock and has reloaded
//! every mem under it before it reads a file: it writes whichever mem
//! holds a reference, and it learns which by reading them all.
//!
//! One commit per affected mem through the engine's one commit call,
//! every commit tagged with a shared `logical_operation_id` and carrying
//! one provenance record per rewritten entity, so a folder mem's ledger
//! tells a sibling process what changed. The peers commit first, in
//! name order, and the renamed mem itself last. Peer commits are
//! parent-pinned (`commit_with_expected_parent`) against a writer that
//! does not take the lock. A commit that fails after another landed
//! surfaces as [`EngineError::RenamePartialFailure`], which states what
//! landed mem by mem; a failure before any commit landed fails the
//! sweep as a whole. Either way the mount still carries the old name,
//! so the same `mem rename` sent again runs the sweep again, finds the
//! landed mems done, and completes. Read-only mounts cannot be
//! rewritten — their stale references degrade to load-time stubs,
//! which health surfaces.
//!
//! Every mem the sweep committed is reloaded before it returns, on
//! success and on failure: the engine serves what storage holds.

use std::path::Path;

use crate::engine::error::{PartialItem, PartialMem};
use crate::engine::{Engine, EngineError};
use crate::entity::EntityId;
use crate::provenance::{Provenance, ProvenanceKind};

/// Outcome of [`Engine::rewrite_mem_references`].
#[derive(Debug, Clone)]
pub struct MemSweepOutcome {
    /// Mems whose entity files were rewritten and committed, in commit
    /// order (peers sorted by name, then the renamed mem itself when it
    /// had self-references or anchors).
    pub rewritten_mems: Vec<String>,
    /// The shared `logical_operation_id` carried by every commit.
    pub logical_operation_id: String,
}

impl Engine {
    /// Rewrite every textual reference to mem `old_mem` so it carries
    /// `new_mem` instead: cross-mem wiki-links and Relationships
    /// entries in every writable peer mem, full-id self-references
    /// inside `old_mem` itself, and `old_mem`'s anchors-sidecar keys
    /// (`<old>--<slug>` → `<new>--<slug>`). One commit per affected
    /// mem; unaffected mems get no commit. Idempotent: a second run
    /// finds nothing left to rewrite and commits nothing — which is
    /// exactly what makes an interrupted `mem rename` completable by
    /// re-issuing it.
    ///
    /// Read-only mounts are skipped (no write access); their stale
    /// references surface as load-time stubs on the next boot.
    pub fn rewrite_mem_references(
        &mut self,
        old_mem: &str,
        new_mem: &str,
        note: Option<&str>,
    ) -> Result<MemSweepOutcome, EngineError> {
        // The sweep's answer scope is the whole workspace — every mem
        // that references `old_mem` must be rewritten — so its lock and
        // its load scope match: every writable mem is locked, every mem
        // reloaded, and a deferred (lazy, unloaded) mem is loaded, or a
        // reference inside it would survive the rename unrewritten.
        let (_write_lock, _reloaded) = self.lock_workspace_for_write_if(true)?;
        let logical_op_id = crate::provenance::mint_logical_operation_id();

        // Peers in name order, the renamed mem itself last: its
        // self-references and anchors move with the mem, and the mem's
        // own part of a rename is the last thing to land.
        let mut mem_order: Vec<usize> = self
            .mounts
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.mount.mem != old_mem
                    && m.mount.capability == crate::workspace::MountCapability::Write
            })
            .map(|(i, _)| i)
            .collect();
        mem_order.sort_by(|a, b| self.mounts[*a].mount.mem.cmp(&self.mounts[*b].mount.mem));
        if let Some(own_idx) = self.mounts.iter().position(|m| {
            m.mount.mem == old_mem && m.mount.capability == crate::workspace::MountCapability::Write
        }) {
            mem_order.push(own_idx);
        }

        // Plan first, commit after: every mem's rewrite is computed
        // before any backend write, so a read failure aborts cleanly.
        let mut plans: Vec<SweepPlan> = Vec::new();
        for mount_idx in mem_order {
            if let Some(plan) = self.plan_mem_sweep(mount_idx, old_mem, new_mem)? {
                plans.push(plan);
            }
        }

        let mut commits: Vec<(usize, String)> = Vec::new();
        for (at, plan) in plans.iter().enumerate() {
            match self.land_mem_sweep(plan, old_mem, new_mem, &logical_op_id, note) {
                Ok(write_id) => {
                    self.record_self_write(plan.mount_idx, &write_id);
                    // Same as the anchor backfill: a sweep has no per-operation
                    // response to carry the notice. The merge still protects the
                    // other writer's fields.
                    let _intervention_has_no_channel_here =
                        self.stamp_mutation_versions(plan.mount_idx);
                    commits.push((plan.mount_idx, write_id));
                }
                Err(cause) => {
                    return Err(self.sweep_failed_at(&plans, at, commits, cause));
                }
            }
        }

        // The engine's view follows what the sweep wrote.
        self.level_committed_mounts(&commits);
        Ok(MemSweepOutcome {
            rewritten_mems: plans.iter().map(|p| p.mem.clone()).collect(),
            logical_operation_id: logical_op_id,
        })
    }

    /// What the sweep writes into one mem; `None` when the mem holds
    /// no reference to `old_mem`. Reads storage and the store, writes
    /// nothing.
    fn plan_mem_sweep(
        &self,
        mount_idx: usize,
        old_mem: &str,
        new_mem: &str,
    ) -> Result<Option<SweepPlan>, EngineError> {
        let mem_name = self.mounts[mount_idx].mount.mem.clone();
        let backend = self.mounts[mount_idx].backend.as_ref();

        // The head the plan is computed from; the commit is pinned to
        // it against a writer that does not take the lock.
        let head = backend.current_head()?;

        let mut changed: Vec<(std::path::PathBuf, String)> = Vec::new();
        for rel_path in backend.list_entities()? {
            let Some(bytes) = backend.read_entity(&rel_path)? else {
                continue;
            };
            let Ok(text) = String::from_utf8(bytes) else {
                // Undecodable file — boot already warns about it;
                // the sweep leaves it alone.
                continue;
            };
            let (rewritten, count) =
                crate::entity::wikilink_rewrite::rewrite_mem_prefix(&text, old_mem, new_mem);
            if count > 0 {
                changed.push((rel_path, rewritten));
            }
        }

        // The renamed mem's anchors sidecar: re-key every
        // `<old>--<slug>` row to `<new>--<slug>` in the same
        // commit as its content rewrites.
        let mut sidecar: Option<Vec<u8>> = None;
        if mem_name == old_mem
            && let Some(raw) = backend.read_anchors_sidecar()?
        {
            let current = crate::anchor::AnchorSidecar::from_bytes(&raw)
                .map_err(|e| EngineError::Mem(format!("anchors sidecar parse: {e}")))?;
            let prefix = format!("{old_mem}--");
            let mut anchors_changed = false;
            let mut next = crate::anchor::AnchorSidecar::default();
            for (entity_id, anchors) in &current.entities {
                let new_key = match entity_id.strip_prefix(&prefix) {
                    Some(rest) => {
                        anchors_changed = true;
                        format!("{new_mem}--{rest}")
                    }
                    None => entity_id.clone(),
                };
                next.set(&new_key, anchors.clone());
            }
            if anchors_changed {
                sidecar = Some(next.to_bytes());
            }
        }

        if changed.is_empty() && sidecar.is_none() {
            return Ok(None);
        }

        // An adopted body's origin marks follow the same rewrite. The mem
        // rename retargets cross-mem wiki-links inside section bodies, and
        // a mark is a hash over those bytes, so an unfollowed mark stops
        // matching and this mem's contributed bodies would be served as the
        // workspace's own prose. Only entities the record marks are
        // touched, and the after-sections come from the same rewrite
        // function applied to the store's own section bodies, so nothing
        // here re-parses a file the sweep deliberately treats as text.
        let mut rehash: Vec<MarkRehash> = Vec::new();
        for entity in self
            .store
            .all_entities()
            .filter(|e| e.mem == mem_name && !e.stub)
        {
            let mut after = entity.sections.clone();
            let mut moved = false;
            for body in after.values_mut() {
                // The SECTION form: the file form trims a leading
                // `---…---` block as frontmatter and leaves it unmasked,
                // which a section body must not have done to it. The two
                // passes have to mask alike, or this one rewrites a link
                // the file pass masked and the mark ends up hashing bytes
                // the file does not hold.
                let (rewritten, count) =
                    crate::entity::wikilink_rewrite::rewrite_mem_prefix_in_section(
                        body, old_mem, new_mem,
                    );
                if count > 0 {
                    *body = rewritten;
                    moved = true;
                }
            }
            if moved {
                rehash.push((entity.id.clone(), entity.sections.clone(), after));
            }
        }

        Ok(Some(SweepPlan {
            mount_idx,
            mem: mem_name,
            head,
            changed,
            sidecar,
            rehash,
        }))
    }

    /// Stage one mem's rewrite and commit it, pinned to the head the
    /// plan was computed from. Returns the commit's id.
    fn land_mem_sweep(
        &self,
        plan: &SweepPlan,
        old_mem: &str,
        new_mem: &str,
        logical_op_id: &str,
        note: Option<&str>,
    ) -> Result<String, EngineError> {
        let backend = self.mounts[plan.mount_idx].backend.as_ref();
        for (rel_path, text) in &plan.changed {
            backend.write_entity(Path::new(rel_path), text.as_bytes())?;
        }
        for (id, before, after) in &plan.rehash {
            super::stage_proposal_marks_rehash(backend, id, before, after)?;
        }
        if let Some(bytes) = &plan.sidecar {
            backend.write_anchors_sidecar(bytes)?;
        }

        let subject = format!(
            "memstead: rename mem `{old_mem}` → `{new_mem}` (reference rewrite in `{}`)",
            plan.mem
        );
        let mut ctx = self.session_commit_context(Some("mem rename"), note.map(String::from));
        ctx.logical_operation_id = Some(logical_op_id);
        // One record per rewritten entity: the ledger of a folder mem is
        // how a sibling process learns what a commit changed.
        let records: Vec<Provenance> = plan
            .entity_ids()
            .into_iter()
            .map(|id| {
                Provenance::new(
                    std::time::SystemTime::now(),
                    ProvenanceKind::Update,
                    Some(id.to_string()),
                    self.current_actor,
                    self.current_client.clone(),
                    note.map(String::from),
                )
                .with_role(self.current_role)
                .with_identity(self.current_identity.clone())
                .with_logical_operation_id(logical_op_id.to_string())
            })
            .collect();
        self.commit_staged_pinned(
            plan.mount_idx,
            &subject,
            &ctx,
            &records,
            plan.head.as_deref(),
        )
        .map(|landed| landed.write_id)
    }

    /// The report for a sweep that failed at `plans[failed]`. With
    /// nothing committed the sweep failed as a whole and `cause` is the
    /// report. With a commit landed the report is the partial-failure
    /// envelope, built after the committed mems were reloaded, so every
    /// entry carries the hash its entity has now.
    fn sweep_failed_at(
        &mut self,
        plans: &[SweepPlan],
        failed: usize,
        commits: Vec<(usize, String)>,
        cause: EngineError,
    ) -> EngineError {
        if commits.is_empty() {
            return cause;
        }
        self.level_committed_mounts(&commits);
        let failure_cause = match &cause {
            EngineError::Backend(crate::backend::BackendError::ParentMismatch { .. }) => "drift",
            _ => "storage",
        };
        let mems = plans
            .iter()
            .enumerate()
            .map(|(at, plan)| {
                let landed = at < failed;
                PartialMem {
                    mem: plan.mem.clone(),
                    landed,
                    error: (at == failed).then(|| cause.to_string()),
                    items: plan
                        .entity_ids()
                        .into_iter()
                        .map(|id| PartialItem {
                            current_hash: self
                                .store
                                .get(&id)
                                .filter(|e| !e.stub)
                                .map(|e| e.content_hash.clone()),
                            id: id.to_string(),
                            landed,
                        })
                        .collect(),
                }
            })
            .collect();
        EngineError::RenamePartialFailure {
            committed_mems: plans[..failed].iter().map(|p| p.mem.clone()).collect(),
            failed_mem: plans[failed].mem.clone(),
            failure_cause: failure_cause.to_string(),
            mems,
        }
    }
}

/// One entity whose marked sections the sweep's rewrite moves: its id,
/// its sections before the rewrite, and after.
type MarkRehash = (
    EntityId,
    indexmap::IndexMap<String, String>,
    indexmap::IndexMap<String, String>,
);

/// One mem's part of a sweep, computed before anything is written.
struct SweepPlan {
    mount_idx: usize,
    mem: String,
    /// The mem's head when the plan was computed.
    head: Option<String>,
    /// Entity files with their rewritten text.
    changed: Vec<(std::path::PathBuf, String)>,
    /// The re-keyed anchors sidecar, on the renamed mem itself.
    sidecar: Option<Vec<u8>>,
    rehash: Vec<MarkRehash>,
}

impl SweepPlan {
    /// The entities whose files this plan rewrites.
    fn entity_ids(&self) -> Vec<EntityId> {
        self.changed
            .iter()
            .map(|(rel_path, _)| {
                crate::entity::id::file_path_to_id(&rel_path.to_string_lossy(), &self.mem)
            })
            .collect()
    }
}
