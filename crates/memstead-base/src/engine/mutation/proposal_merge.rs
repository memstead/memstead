//! `Engine::proposal_merge`: apply a filled review brief onto the mem a
//! fork was forked from; `Engine::proposal_list`: read the record the
//! merge keeps.
//!
//! The merge re-renders the brief (so the picture it applies is the
//! picture as it stands, never the file's copy), validates the
//! disposition file against it (coverage, the closed vocabulary, the
//! reasons, no `adopt` on a conflict, the pinned tips), reads the
//! proposer of every adopted entity off the fork commit that last
//! touched it, rehearses every adopted body through the target's write
//! gate, and only then lands: one commit on the target branch per
//! proposer identity (in slug order, each parent-pinned to the tip
//! before it) carrying every adopted entity as the fork has it, the
//! record sidecar in the last of them; a second commit under the
//! merger's identity with the owner's final bodies for
//! `adopt_with_changes`; a verification check record per entity the
//! merge created or updated, under the merger's identity; and a
//! whole-store validation of the target read back from its backend. A
//! refusal lands nothing: the store snapshot is restored and every
//! pending buffer discarded, and when an earlier proposer's commit had
//! already landed, the target branch is moved back to the tip the merge
//! was pinned to (every commit above it was this merge's own). A gate
//! refusal is the gate's own error wrapped per entity
//! ([`EngineError::InEntity`]): the code stays the gate's, and
//! `details.entity` and `details.stage` (`landing` for the fork's
//! version, `amend` for the owner's body) say where it failed.
//!
//! Once the last commit is on the branch, nothing refuses any more: a
//! failure of the bookkeeping after it (a check record the ledger
//! refused, the re-read of the target, the validation reading, the tip
//! read back) is a warning on the outcome, and the merge is reported as
//! landed, because it is. An error from this function therefore always
//! means a refusal that landed nothing.
//!
//! Nothing is written to the fork.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use crate::check::{CheckKind, Verdict};
use crate::engine::history::filter_notes_for_entity;
use crate::engine::proposal::{LandingWrite, normalise_to, parse_body, resolve_sha};
use crate::engine::{Engine, EngineError};
use crate::engine_fallback_type;
use crate::entity::parser::parse_markdown;
use crate::entity::store_builder::push_entities_into_store;
use crate::entity::{Entity, EntityId};
use crate::ops::proposal::{
    DISPOSITION_ADOPT, DISPOSITION_ADOPT_WITH_CHANGES, DISPOSITION_REJECT, DISPOSITION_VOCABULARY,
    DispositionBody, MergeCommit, MergeValidation, MergedEntity, PROPOSAL_RECORD_PATH,
    ProposalBrief, ProposalChange, ProposalEntry, ProposalMergeOutcome, ProposalRecord,
    ProposalRecordEntry, RecordedDisposition,
};
use crate::vcs::{Actor, ClientId, CommitContext, ProposalTrailers, Role};
use crate::workspace::{MountCapability, MountStorage};

use super::create::{CreatePrepareOutcome, PreparedCreate};
use super::update::{PrepareOutcome, PreparedUpdate};
use super::{
    deferred_verified_stub_kind, gc_orphan_stubs, make_stub, stage_anchors_removal,
    stage_anchors_replace,
};

/// The tool name on the merge commits' `Tool:` trailer.
const MERGE_TOOL: &str = "proposal_merge";

/// `details.stage` of a gate refusal met while landing the fork's
/// version of an entity.
const STAGE_LANDING: &str = "landing";
/// `details.stage` of a gate refusal met while amending a landed entity
/// with the owner's final body.
const STAGE_AMEND: &str = "amend";

/// One write the merge lands for an adopted entity.
enum Landing {
    Create(PreparedCreate),
    Update(PreparedUpdate),
    /// A deletion in the target: the id, its file path, and the
    /// read-only referrers that demote it to a residual stub.
    Delete {
        id: EntityId,
        file_path: String,
        readonly_referrers: Vec<EntityId>,
    },
}

impl Landing {
    fn id(&self) -> &EntityId {
        match self {
            Landing::Create(p) => &p.id,
            Landing::Update(p) => &p.id,
            Landing::Delete { id, .. } => id,
        }
    }

    fn is_create(&self) -> bool {
        matches!(self, Landing::Create(_))
    }
}

/// One adopted entry's plan: the slug, its proposer, and the writes.
struct Adopted {
    slug: String,
    proposer: String,
    landings: Vec<Landing>,
    /// The owner's final body, for `adopt_with_changes`.
    amend: Option<DispositionBody>,
}

impl Engine {
    /// Apply `file` (the brief's JSON with `dispositions` filled) onto
    /// the mem `fork` was forked from. The merger is the session's
    /// identity ([`Engine::set_identity`]) and is required.
    ///
    /// Refusals, each landing nothing: `INVALID_INPUT` (no merger
    /// identity; the file names another fork; a value outside the
    /// vocabulary; a missing reason or body; `adopt_with_changes` on a
    /// deletion), `PROPOSAL_DISPOSITIONS_INCOMPLETE`, `PROPOSAL_CONFLICT`,
    /// `PROPOSAL_STALE` (a pinned tip moved), `PROPOSAL_UNATTRIBUTED`
    /// (an adopted entity's last fork commit carries no identity), the
    /// write gate's own code for a body it refuses (with
    /// `details.entity` naming the slug and `details.stage` saying
    /// `landing` or `amend`), `HAS_INCOMING_REFS` for a deletion the
    /// target's referrers block, `UNKNOWN_MEM`, `READ_ONLY_MOUNT`, and
    /// the brief's refusals. Once the last commit is on the branch a
    /// failure is a warning on the outcome, never an error.
    pub fn proposal_merge(
        &mut self,
        fork: &str,
        file: &ProposalBrief,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<ProposalMergeOutcome, EngineError> {
        let Some(merger) = self.current_identity().map(str::to_string) else {
            return Err(EngineError::InvalidInput(
                "a proposal merge records the merger's identity beside the proposer's, and \
                 the session declares none: pass `--identity <merger>`"
                    .to_string(),
            ));
        };
        if file.fork != fork {
            return Err(EngineError::InvalidInput(format!(
                "the disposition file was rendered for fork '{}', not '{fork}'",
                file.fork
            )));
        }

        // ---- The picture as it stands, and the file against it ----
        let brief = self.proposal_brief(fork)?;
        let target = brief.target.clone();
        let (gitdir, fork_branch) = match &self.find_mount(fork)?.mount.storage {
            MountStorage::GitBranch { gitdir, branch } => (gitdir.clone(), branch.clone()),
            _ => unreachable!("the brief refused a fork that is not git-backed"),
        };
        let target_mount = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == target)
            .ok_or_else(|| self.unknown_mem_error(&target))?;
        if self.mounts[target_mount].mount.capability != MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(target.clone()));
        }
        for (side, recorded, current) in [
            ("target", &file.target_tip, &brief.target_tip),
            ("fork", &file.fork_tip, &brief.fork_tip),
            ("base", &file.base, &brief.base),
        ] {
            if recorded != current {
                return Err(EngineError::ProposalStale {
                    fork: fork.to_string(),
                    side: side.to_string(),
                    recorded: recorded.clone(),
                    current: current.clone(),
                });
            }
        }
        let listed: BTreeSet<&str> = brief.entries.iter().map(|e| e.slug.as_str()).collect();
        let filed: BTreeSet<&str> = file.dispositions.keys().map(String::as_str).collect();
        let missing: Vec<String> = listed.difference(&filed).map(|s| s.to_string()).collect();
        let unexpected: Vec<String> = filed.difference(&listed).map(|s| s.to_string()).collect();
        if !missing.is_empty() || !unexpected.is_empty() {
            return Err(EngineError::ProposalDispositionsIncomplete {
                fork: fork.to_string(),
                missing,
                unexpected,
            });
        }

        // ---- Every slot, in slug order ----
        struct Slot<'a> {
            entry: &'a ProposalEntry,
            disposition: &'a str,
            reason: Option<String>,
            body: Option<DispositionBody>,
        }
        let mut slots: Vec<Slot<'_>> = Vec::with_capacity(brief.entries.len());
        for entry in &brief.entries {
            let slot = &file.dispositions[&entry.slug];
            let disposition = slot.disposition.trim();
            if !DISPOSITION_VOCABULARY.contains(&disposition) {
                return Err(EngineError::InvalidInput(format!(
                    "dispositions.{}.disposition is '{}': the vocabulary is {}",
                    entry.slug,
                    slot.disposition,
                    DISPOSITION_VOCABULARY.join(", ")
                )));
            }
            let disposition = DISPOSITION_VOCABULARY
                .iter()
                .find(|v| **v == disposition)
                .copied()
                .expect("checked above");
            let reason = crate::vcs::normalise_identity(Some(&slot.reason));
            if matches!(
                disposition,
                DISPOSITION_REJECT | DISPOSITION_ADOPT_WITH_CHANGES
            ) && reason.is_none()
            {
                return Err(EngineError::InvalidInput(format!(
                    "dispositions.{}.reason is empty: `{disposition}` needs a reason",
                    entry.slug
                )));
            }
            if disposition == DISPOSITION_ADOPT
                && let Some(c) = &entry.conflict
            {
                return Err(EngineError::ProposalConflict {
                    fork: fork.to_string(),
                    slug: entry.slug.clone(),
                    kind: c.kind.as_wire().to_string(),
                });
            }
            let body = if disposition == DISPOSITION_ADOPT_WITH_CHANGES {
                if entry.status == ProposalChange::Deleted {
                    return Err(EngineError::InvalidInput(format!(
                        "dispositions.{}: the fork's version is a deletion, which takes no \
                         final body; `adopt` it or `reject` it",
                        entry.slug
                    )));
                }
                let Some(raw) = &slot.body else {
                    return Err(EngineError::InvalidInput(format!(
                        "dispositions.{}.body is absent: `adopt_with_changes` carries the \
                         owner's final body (`sections`, `metadata`, optional `title` and \
                         `relations`)",
                        entry.slug
                    )));
                };
                let body: DispositionBody = serde_json::from_value(raw.clone()).map_err(|e| {
                    EngineError::InvalidInput(format!(
                        "dispositions.{}.body does not parse as a body (`sections`, \
                             `metadata`, optional `title` and `relations`): {e}",
                        entry.slug
                    ))
                })?;
                if body.sections.is_empty() {
                    return Err(EngineError::InvalidInput(format!(
                        "dispositions.{}.body.sections is empty: the final body carries at \
                         least one section",
                        entry.slug
                    )));
                }
                Some(body)
            } else {
                None
            };
            slots.push(Slot {
                entry,
                disposition,
                reason,
                body,
            });
        }

        // ---- The proposer of every adopted entity, off the fork's commits ----
        let notes = {
            let Some(hook) = self.git_branch_ops.as_ref() else {
                return Err(EngineError::InvalidInput(
                    "a proposal merge needs the git-branch ops bundle (full boot only)".to_string(),
                ));
            };
            (hook.changes_since)(
                &gitdir,
                &fork_branch,
                fork,
                crate::ops::EMPTY_TREE_SHA,
                crate::ops::RENAME_SIMILARITY_DEFAULT,
            )
            .map_err(EngineError::Backend)?
            .notes
        };
        let mut proposers: BTreeMap<String, String> = BTreeMap::new();
        for slot in slots.iter().filter(|s| s.disposition != DISPOSITION_REJECT) {
            let fork_id = EntityId::new(fork, &slot.entry.slug);
            let touch = filter_notes_for_entity(fork_id.as_ref(), &notes)
                .into_iter()
                .next();
            match touch.as_ref().and_then(|t| t.identity.clone()) {
                Some(identity) => {
                    proposers.insert(slot.entry.slug.clone(), identity);
                }
                None => {
                    return Err(EngineError::ProposalUnattributed {
                        fork: fork.to_string(),
                        slug: slot.entry.slug.clone(),
                        sha: touch
                            .map(|t| t.reference)
                            .unwrap_or_else(|| "(no engine-recorded commit)".to_string()),
                    });
                }
            }
        }

        // ---- The validation baseline, before anything moves ----
        let findings_before = self.merge_findings(&target)?;
        let load_errors_before = self.load_errors().len();

        // ---- Rehearse every adopted write through the gate ----
        let store_snapshot = self.store.clone();
        let target_schema = self.schema_for(&target);
        let mut adopted: Vec<Adopted> = Vec::new();
        let mut noops: Vec<String> = Vec::new();
        // The fork's version of every adopted entity under the target's
        // name, parsed under the target's schema.
        let mut versions: BTreeMap<String, Entity> = BTreeMap::new();
        for slot in slots
            .iter()
            .filter(|s| s.disposition != DISPOSITION_REJECT)
            .filter(|s| s.entry.status != ProposalChange::Deleted)
        {
            let slug = slot.entry.slug.as_str();
            let target_id = EntityId::new(&target, slug);
            let landing = slot
                .entry
                .fork_body
                .as_deref()
                .map(|b| normalise_to(b, fork, &target))
                .and_then(|b| {
                    parse_body(
                        &b,
                        &crate::entity::id::id_to_file_path(&target_id),
                        &target,
                        target_schema.as_deref(),
                    )
                })
                .ok_or_else(|| {
                    EngineError::InvalidInput(format!(
                        "the fork's body of '{slug}' does not parse under the target's schema \
                         (the brief's precheck names the failure); reject it, or repair it on \
                         the fork"
                    ))
                })?;
            versions.insert(slug.to_string(), landing);
        }
        // Every entity the merge creates is staged as a typed skeleton
        // first, so a sibling's edge to it validates against a real
        // target (the batch create's rule).
        let creates: HashSet<EntityId> = versions
            .keys()
            .map(|slug| EntityId::new(&target, slug))
            .filter(|id| !self.target_holds(id))
            .collect();
        for id in &creates {
            let landing = &versions[id.path()];
            let mut skeleton = make_stub(id, crate::entity::StubKind::ForwardReference);
            skeleton.stub = false;
            skeleton.stub_kind = None;
            skeleton.entity_type = landing.entity_type.clone();
            skeleton.title = landing.title.clone();
            self.store.upsert(id.clone(), skeleton);
        }
        let deletions: HashSet<EntityId> = slots
            .iter()
            .filter(|s| s.disposition == DISPOSITION_ADOPT)
            .filter_map(|s| match s.entry.status {
                ProposalChange::Deleted => Some(EntityId::new(&target, &s.entry.slug)),
                ProposalChange::Renamed => s
                    .entry
                    .renamed_from
                    .as_deref()
                    .map(|from| EntityId::new(&target, from)),
                _ => None,
            })
            .collect();
        let prepared: Result<(), EngineError> = (|| {
            for slot in slots.iter().filter(|s| s.disposition != DISPOSITION_REJECT) {
                let slug = slot.entry.slug.as_str();
                let target_id = EntityId::new(&target, slug);
                let mut landings: Vec<Landing> = Vec::new();
                match slot.entry.status {
                    ProposalChange::Deleted => {
                        if let Some(l) = self.plan_delete(&target_id, &deletions)? {
                            landings.push(l);
                        } else {
                            noops.push(slug.to_string());
                        }
                    }
                    _ => {
                        let landing = &versions[slug];
                        match self.plan_landing(&target_id, landing, &creates)? {
                            Some(l) => landings.push(l),
                            None => noops.push(slug.to_string()),
                        }
                        if slot.entry.status == ProposalChange::Renamed
                            && let Some(from) = slot.entry.renamed_from.as_deref()
                            && let Some(l) =
                                self.plan_delete(&EntityId::new(&target, from), &deletions)?
                        {
                            landings.push(l);
                        }
                    }
                }
                adopted.push(Adopted {
                    slug: slug.to_string(),
                    proposer: proposers[slug].clone(),
                    landings,
                    amend: slot.body.clone(),
                });
            }
            Ok(())
        })();
        if let Err(e) = prepared {
            self.store = store_snapshot;
            self.discard_all_pending();
            return Err(e);
        }

        // ---- The store as it will be, so a deletion's referrers are judged
        //      against the adopted bodies, not the pre-merge ones ----
        let applied: Result<(), EngineError> = (|| {
            let fallback = engine_fallback_type();
            for a in &adopted {
                for l in &a.landings {
                    match l {
                        Landing::Create(p) => {
                            let parsed = parse_markdown(
                                &p.markdown,
                                &p.file_path,
                                p.type_def.as_ref(),
                                &p.mem,
                            )
                            .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
                            push_entities_into_store(
                                &mut self.store,
                                vec![parsed],
                                fallback.as_ref(),
                                None,
                            );
                        }
                        Landing::Update(p) => {
                            self.apply_prepared_to_store(p)?;
                        }
                        Landing::Delete { .. } => {}
                    }
                }
            }
            crate::entity::store_builder::remap_alias_target_edge_sources(
                &mut self.store,
                &self.schemas,
            );
            for a in &adopted {
                for l in &a.landings {
                    if let Landing::Delete { id, .. } = l {
                        let referrers = self.classify_delete_referrers(id);
                        let blocking: Vec<_> = referrers
                            .write_referrers
                            .into_iter()
                            .filter(|r| !deletions.contains(&EntityId(r.from_id.clone())))
                            .collect();
                        if !blocking.is_empty() {
                            return Err(EngineError::HasIncomingRefs {
                                id: id.to_string(),
                                referrers: blocking,
                            });
                        }
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = applied {
            self.store = store_snapshot;
            self.discard_all_pending();
            return Err(e);
        }

        // ---- The owner's final bodies, rehearsed now against the landed
        //      versions the store holds, so a body the gate refuses lands
        //      nothing; staged and committed after the merge commits ----
        let amends: Result<Vec<PreparedUpdate>, EngineError> = (|| {
            let mut prepared: Vec<PreparedUpdate> = Vec::new();
            for a in &adopted {
                if let Some(body) = &a.amend
                    && let Some(p) = self.plan_amend(&EntityId::new(&target, &a.slug), body)?
                {
                    prepared.push(p);
                }
            }
            Ok(prepared)
        })();
        let amends = match amends {
            Ok(p) => p,
            Err(e) => {
                self.store = store_snapshot;
                self.discard_all_pending();
                return Err(e);
            }
        };

        // ---- The record ----
        let mut record = self.read_proposal_record(&target)?.unwrap_or_default();
        let mut distinct_proposers: Vec<String> = Vec::new();
        for a in &adopted {
            if !distinct_proposers.contains(&a.proposer) {
                distinct_proposers.push(a.proposer.clone());
            }
        }
        record.proposals.push(ProposalRecordEntry {
            id: brief.proposal_id.clone(),
            proposer: (!distinct_proposers.is_empty()).then(|| distinct_proposers.join(", ")),
            ancestor: brief.ancestor.clone(),
            base: brief.base.clone(),
            target_tip: brief.target_tip.clone(),
            merged_by: Some(merger.clone()),
            at: self.now_iso(),
            entities: slots
                .iter()
                .map(|s| {
                    (
                        s.entry.slug.clone(),
                        RecordedDisposition {
                            disposition: s.disposition.to_string(),
                            reason: s.reason.clone(),
                            content_hash: s.entry.content_hash.clone(),
                        },
                    )
                })
                .collect(),
        });
        let record_bytes = record.to_bytes();

        // ---- Land: one commit per proposer identity, in slug order ----
        let mut merge_commits: Vec<MergeCommit> = Vec::new();
        let mut parent = brief.target_tip.clone();
        let mut landed_any = false;
        let mut created_ids: HashSet<EntityId> = HashSet::new();
        let mut deleted_ids: Vec<(EntityId, Vec<EntityId>)> = Vec::new();
        for (i, proposer) in distinct_proposers.iter().enumerate() {
            let group: Vec<&Adopted> = adopted.iter().filter(|a| &a.proposer == proposer).collect();
            let last_group = i + 1 == distinct_proposers.len();
            let staged: Result<(Vec<String>, Vec<String>), EngineError> = (|| {
                let backend = self.mounts[target_mount].backend.as_ref();
                let mut ids: Vec<String> = Vec::new();
                let mut created: Vec<String> = Vec::new();
                for a in &group {
                    for l in &a.landings {
                        match l {
                            Landing::Create(p) => {
                                self.stage_prepared_create(p)?;
                                let rows = self.entity_anchors(&EntityId::new(fork, &a.slug));
                                stage_anchors_replace(backend, &p.id, rows)?;
                                created.push(p.id.to_string());
                            }
                            Landing::Update(p) => {
                                self.stage_prepared_update(p)?;
                                let rows = self.entity_anchors(&EntityId::new(fork, &a.slug));
                                stage_anchors_replace(backend, &p.id, rows)?;
                            }
                            Landing::Delete { id, file_path, .. } => {
                                backend.delete_entity(Path::new(file_path))?;
                                stage_anchors_removal(backend, id)?;
                            }
                        }
                        ids.push(l.id().to_string());
                    }
                }
                if last_group {
                    backend.write_entity(Path::new(PROPOSAL_RECORD_PATH), &record_bytes)?;
                }
                Ok((ids, created))
            })();
            let (ids, created) = match staged {
                Ok(v) => v,
                Err(e) => {
                    return Err(self.unwind_merge(
                        &target,
                        &brief.target_tip,
                        landed_any.then_some(parent.as_str()),
                        &store_snapshot,
                        e,
                    ));
                }
            };
            let mut ctx = CommitContext::new(
                Some(MERGE_TOOL),
                actor,
                client.cloned(),
                note.map(String::from),
                Role::Unspecified,
                Some(proposer.clone()),
            );
            ctx.entity_ids = Some(ids.clone());
            ctx.proposal = Some(ProposalTrailers {
                proposal: brief.proposal_id.clone(),
                merged_by: merger.clone(),
                created: created.clone(),
            });
            let subject = format!("memstead: proposal-merge {}", brief.proposal_id);
            let sha = match self.mounts[target_mount]
                .backend
                .commit_with_expected_parent(&subject, &ctx, Some(&parent))
            {
                Ok(sha) => sha,
                Err(e) => {
                    let e = match e {
                        crate::backend::BackendError::ParentMismatch { expected, actual } => {
                            EngineError::ProposalStale {
                                fork: fork.to_string(),
                                side: "target".to_string(),
                                recorded: expected,
                                current: actual,
                            }
                        }
                        other => other.into(),
                    };
                    return Err(self.unwind_merge(
                        &target,
                        &brief.target_tip,
                        landed_any.then_some(parent.as_str()),
                        &store_snapshot,
                        e,
                    ));
                }
            };
            landed_any = true;
            self.record_self_write(target_mount, &sha);
            for a in &group {
                for l in &a.landings {
                    match l {
                        Landing::Delete {
                            id,
                            readonly_referrers,
                            ..
                        } => deleted_ids.push((id.clone(), readonly_referrers.clone())),
                        l if l.is_create() => {
                            created_ids.insert(l.id().clone());
                        }
                        _ => {}
                    }
                }
            }
            parent = sha.clone();
            merge_commits.push(MergeCommit {
                sha,
                identity: proposer.clone(),
                entities: ids,
            });
        }

        // ---- The store after the commits: deletions, stubs, indexes ----
        for (id, readonly_referrers) in &deleted_ids {
            if readonly_referrers.is_empty() {
                self.store.remove(id);
            } else {
                self.store.remove_edges_from(id);
                self.store.upsert(
                    id.clone(),
                    make_stub(
                        id,
                        crate::entity::StubKind::Residual {
                            since_commit: parent.clone(),
                            readonly_referrers: readonly_referrers.clone(),
                        },
                    ),
                );
            }
        }
        let mut out_of_batch: Vec<(EntityId, crate::entity::StubKind)> = Vec::new();
        for a in &adopted {
            for l in &a.landings {
                if let Landing::Create(p) = l {
                    for t in &p.relation_targets {
                        if !self.store.contains(t) {
                            out_of_batch.push((t.clone(), deferred_verified_stub_kind(self, t)?));
                        }
                    }
                }
            }
        }
        for (t, kind) in out_of_batch {
            self.store.upsert(t.clone(), make_stub(&t, kind));
        }
        gc_orphan_stubs(&mut self.store);
        let _ = self.stamp_mutation_versions(target_mount);
        self.invalidate_communities();
        self.invalidate_search_indexes();

        // ---- The owner's final bodies, one commit under the merger ----
        // From the last commit on, a failure is bookkeeping: it goes here
        // and the merge is reported as landed (see the module doc).
        let mut warnings: Vec<String> = Vec::new();
        let mut amend_commit: Option<MergeCommit> = None;
        if !amends.is_empty() {
            let staged: Result<(), EngineError> = (|| {
                for p in &amends {
                    self.stage_prepared_update(p)?;
                }
                Ok(())
            })();
            if let Err(e) = staged {
                return Err(self.unwind_merge(
                    &target,
                    &brief.target_tip,
                    Some(&parent),
                    &store_snapshot,
                    e,
                ));
            }
            let ids: Vec<String> = amends.iter().map(|p| p.id.to_string()).collect();
            let mut ctx = self.commit_context(
                Some(MERGE_TOOL),
                actor,
                client.cloned(),
                note.map(String::from),
            );
            ctx.entity_ids = Some(ids.clone());
            ctx.proposal = Some(ProposalTrailers {
                proposal: brief.proposal_id.clone(),
                merged_by: merger.clone(),
                created: Vec::new(),
            });
            let subject = format!("memstead: proposal-amend {}", brief.proposal_id);
            let sha = match self.mounts[target_mount]
                .backend
                .commit_with_expected_parent(&subject, &ctx, Some(&parent))
            {
                Ok(sha) => sha,
                Err(e) => {
                    return Err(self.unwind_merge(
                        &target,
                        &brief.target_tip,
                        Some(&parent),
                        &store_snapshot,
                        e.into(),
                    ));
                }
            };
            self.record_self_write(target_mount, &sha);
            for p in &amends {
                if let Err(e) = self.apply_prepared_to_store(p) {
                    warnings.push(format!(
                        "{}: the store's copy of the amended entity was not refreshed ({}); \
                         the re-read after the merge serves the landed body",
                        p.id,
                        e.prose_render()
                    ));
                }
            }
            let _ = self.stamp_mutation_versions(target_mount);
            self.invalidate_communities();
            self.invalidate_search_indexes();
            parent = sha.clone();
            amend_commit = Some(MergeCommit {
                sha,
                identity: merger.clone(),
                entities: ids,
            });
        }

        // ---- The merger's check per entity the merge created or updated,
        //      against the landed hash; a no-op or a deletion gets none ----
        let method = format!("proposal {}", brief.proposal_id);
        let mut entities: Vec<MergedEntity> = Vec::with_capacity(slots.len());
        for slot in &slots {
            let slug = slot.entry.slug.as_str();
            let target_id = EntityId::new(&target, slug);
            let plan = adopted.iter().find(|a| a.slug == slug);
            let amended = amends.iter().any(|p| p.id == target_id);
            let (action, proposer) = match plan {
                None => ("none", None),
                Some(a) => {
                    let action = if a.landings.iter().any(|l| l.is_create()) {
                        "created"
                    } else if amended || a.landings.iter().any(|l| matches!(l, Landing::Update(_)))
                    {
                        "updated"
                    } else if a
                        .landings
                        .iter()
                        .any(|l| matches!(l, Landing::Delete { .. }))
                    {
                        "deleted"
                    } else {
                        "noop"
                    };
                    (action, Some(a.proposer.clone()))
                }
            };
            let exists = self.target_holds(&target_id);
            let content_hash = exists.then(|| {
                self.store
                    .get(&target_id)
                    .map(|e| e.content_hash.clone())
                    .unwrap_or_default()
            });
            let check_recorded = exists
                && matches!(action, "created" | "updated")
                && match self.record_check(
                    &target,
                    target_id.as_ref(),
                    Verdict::Ok,
                    CheckKind::Verification,
                    Some(&method),
                    actor,
                    client,
                ) {
                    Ok(_) => true,
                    Err(e) => {
                        warnings.push(format!(
                            "{target_id}: the verification check was not recorded ({}); the \
                             entity reads as never checked until `memstead check` records one",
                            e.prose_render()
                        ));
                        false
                    }
                };
            entities.push(MergedEntity {
                slug: slug.to_string(),
                target_id: target_id.to_string(),
                disposition: slot.disposition.to_string(),
                action: action.to_string(),
                proposer,
                content_hash,
                check_recorded,
            });
        }
        let _ = noops;
        let _ = created_ids;

        // ---- The whole store, read back ----
        let reread = match self.reload_one_mem(&target) {
            Ok(_) => true,
            Err(e) => {
                warnings.push(format!(
                    "the target was not re-read from its backend after the merge ({}); run \
                     `memstead reload --mem {target}`",
                    e.prose_render()
                ));
                false
            }
        };
        let findings_after = match self.merge_findings(&target) {
            Ok(after) => Some(after),
            Err(e) => {
                warnings.push(format!(
                    "the validation reading after the merge failed ({}); `findings_after` \
                     restates the count before the merge and `new_findings` is unknown",
                    e.prose_render()
                ));
                None
            }
        };
        let new_findings: Vec<String> = findings_after
            .as_ref()
            .map(|after| {
                after
                    .difference(&findings_before)
                    .map(|(id, code)| format!("{id}: {code}"))
                    .collect()
            })
            .unwrap_or_default();
        let validation = MergeValidation {
            entities: self
                .store
                .all_entities()
                .filter(|e| e.mem == target && !e.stub)
                .count(),
            findings_before: findings_before.len(),
            findings_after: findings_after
                .as_ref()
                .map_or(findings_before.len(), BTreeSet::len),
            new_findings,
            all_entities_parse: reread && self.load_errors().len() <= load_errors_before,
        };

        let target_tip_after = {
            let Some(ops) = self.git_branch_ops() else {
                unreachable!("checked above");
            };
            let target_branch = match &self.mounts[target_mount].mount.storage {
                MountStorage::GitBranch { branch, .. } => branch.clone(),
                _ => unreachable!("the brief refused a target that is not git-backed"),
            };
            match resolve_sha(&ops, &gitdir, &crate::branch_full_ref(&target_branch)) {
                Ok(sha) => sha,
                Err(e) => {
                    warnings.push(format!(
                        "the target tip was not read back after the last commit ({}); \
                         `target_tip_after` is the sha of the last commit landed",
                        e.prose_render()
                    ));
                    parent.clone()
                }
            }
        };
        debug_assert_eq!(target_tip_after, parent);

        Ok(ProposalMergeOutcome {
            fork: fork.to_string(),
            target,
            proposal_id: brief.proposal_id,
            merged_by: merger,
            target_tip_before: brief.target_tip,
            target_tip_after,
            merge_commits,
            amend_commit,
            entities,
            record_path: PROPOSAL_RECORD_PATH.to_string(),
            validation,
            warnings,
        })
    }

    /// The proposal record of `target`: every proposal merged into it,
    /// with every disposition. An empty record for a mem no proposal
    /// was merged into; `UNKNOWN_MEM` for a mem that is not mounted.
    pub fn proposal_list(&mut self, target: &str) -> Result<ProposalRecord, EngineError> {
        self.find_mount(target)?;
        self.reload_if_stale(Some(target));
        Ok(self.read_proposal_record(target)?.unwrap_or_default())
    }

    /// The integrity and conformance findings of `mem` as a set of
    /// `(id, code)`, the whole-store reading the merge compares before
    /// and after.
    fn merge_findings(&self, mem: &str) -> Result<BTreeSet<(String, String)>, EngineError> {
        let mut out = BTreeSet::new();
        for f in self
            .conformance_findings(mem, None)?
            .into_iter()
            .chain(self.consistency_findings(mem)?)
        {
            out.insert((f.id, f.code));
        }
        Ok(out)
    }

    /// The create or update that lands `landing` in the target, rehearsed
    /// through the gate; `None` when the target already holds exactly
    /// this version. Relations the fork dropped are removed from the
    /// store's copy of the entity before the update composes, so the
    /// composed post-state (which the gate validates: required edges,
    /// acyclicity, shapes) is the fork's version, not a union.
    fn plan_landing(
        &mut self,
        target_id: &EntityId,
        landing: &Entity,
        creates: &HashSet<EntityId>,
    ) -> Result<Option<Landing>, EngineError> {
        let existing = if creates.contains(target_id) {
            None
        } else {
            self.store.get(target_id).filter(|e| !e.stub).cloned()
        };
        match self.landing_write_args(
            target_id.mem(),
            target_id.path(),
            landing,
            existing.as_ref(),
        ) {
            LandingWrite::Create(args) => {
                match self
                    .prepare_create(*args, Some(creates), Vec::new())
                    .map_err(|e| e.in_entity(target_id.path(), STAGE_LANDING))?
                {
                    CreatePrepareOutcome::Prepared(p) => Ok(Some(Landing::Create(p))),
                    CreatePrepareOutcome::Done(_) => Ok(None),
                }
            }
            LandingWrite::Update(args) => {
                let mut args = *args;
                let dropped: Vec<(String, EntityId)> = args
                    .relations_unset
                    .drain(..)
                    .map(|r| (r.rel_type, r.target))
                    .collect();
                if !dropped.is_empty()
                    && let Some(existing) = self.store.get_mut(target_id)
                {
                    existing.relationships.retain(|r| {
                        !dropped
                            .iter()
                            .any(|(t, id)| *t == r.rel_type && *id == r.target)
                    });
                }
                for (rel_type, to) in &dropped {
                    self.store.remove_edge(target_id, to, rel_type);
                }
                match self
                    .prepare_update(args)
                    .map_err(|e| e.in_entity(target_id.path(), STAGE_LANDING))?
                {
                    PrepareOutcome::Prepared(p) => Ok(Some(Landing::Update(p))),
                    PrepareOutcome::Done(_) => Ok(None),
                }
            }
        }
    }

    /// The deletion of `id` in the target, when the target holds it;
    /// the write-mem referrers are judged later, against the adopted
    /// bodies. Read-only referrers demote the id to a residual stub, as
    /// a plain delete does.
    fn plan_delete(
        &self,
        id: &EntityId,
        deletions: &HashSet<EntityId>,
    ) -> Result<Option<Landing>, EngineError> {
        let Some(entity) = self.store.get(id).filter(|e| !e.stub) else {
            return Ok(None);
        };
        let _ = deletions;
        let referrers = self.classify_delete_referrers(id);
        Ok(Some(Landing::Delete {
            id: id.clone(),
            file_path: entity.file_path.clone(),
            readonly_referrers: referrers.readonly_referrers,
        }))
    }

    /// The update that lands the owner's final body over the landed
    /// entity: the body's sections replace the landed ones (a landed
    /// section the body lacks is unset), its metadata sets the fields
    /// it names (a field it omits keeps the landed value, as a create
    /// keeps a default), its relations (when given) replace the landed
    /// ones. A title that differs refuses: a title change is a rename.
    /// `None` when the body restates the landed version.
    fn plan_amend(
        &mut self,
        id: &EntityId,
        body: &DispositionBody,
    ) -> Result<Option<PreparedUpdate>, EngineError> {
        let Some(existing) = self.store.get(id).filter(|e| !e.stub).cloned() else {
            return Err(EngineError::NotFound { id: id.to_string() });
        };
        if let Some(title) = body.title.as_deref().map(str::trim)
            && !title.is_empty()
            && title != existing.title
        {
            return Err(EngineError::InvalidInput(format!(
                "dispositions.{}.body.title is '{title}' and the landed title is '{}': a title \
                 change is a rename (`memstead rename`), not a merge",
                id.path(),
                existing.title
            )));
        }
        let sections_unset: Vec<String> = existing
            .sections
            .keys()
            .filter(|k| k.as_str() != "relationships" && !body.sections.contains_key(*k))
            .cloned()
            .collect();
        let mut declare_relations: Vec<crate::ops::RelateArg> = Vec::new();
        if let Some(relations) = &body.relations {
            let wanted: Vec<(String, EntityId, Option<String>)> = relations
                .iter()
                .map(|r| {
                    let (resolved, _) = self
                        .resolve_entity_id(&EntityId(r.target.clone()))
                        .unwrap_or((EntityId(r.target.clone()), None));
                    (r.rel_type.clone(), resolved, r.description.clone())
                })
                .collect();
            let dropped: Vec<(String, EntityId)> = existing
                .relationships
                .iter()
                .filter(|x| {
                    !wanted
                        .iter()
                        .any(|(t, id, _)| *t == x.rel_type && *id == x.target)
                })
                .map(|x| (x.rel_type.clone(), x.target.clone()))
                .collect();
            if let Some(e) = self.store.get_mut(id) {
                e.relationships.retain(|r| {
                    !dropped
                        .iter()
                        .any(|(t, id)| *t == r.rel_type && *id == r.target)
                });
            }
            for (rel_type, to) in &dropped {
                self.store.remove_edge(id, to, rel_type);
            }
            for (rel_type, target, description) in wanted {
                if !existing
                    .relationships
                    .iter()
                    .any(|x| x.rel_type == rel_type && x.target == target)
                {
                    declare_relations.push(crate::ops::RelateArg {
                        target,
                        rel_type,
                        description,
                    });
                }
            }
        }
        let args = crate::UpdateEntityArgs {
            id: id.clone(),
            expected_hash: None,
            sections: body.sections.clone(),
            append_sections: Default::default(),
            patch_sections: Default::default(),
            sections_unset,
            metadata: body.metadata.clone(),
            metadata_unset: Vec::new(),
            dry_run: false,
            declare_relations,
            anchors: Vec::new(),
            anchors_unset: Vec::new(),
            relations_unset: Vec::new(),
        };
        match self
            .prepare_update(args)
            .map_err(|e| e.in_entity(id.path(), STAGE_AMEND))?
        {
            PrepareOutcome::Prepared(p) => Ok(Some(p)),
            PrepareOutcome::Done(_) => Ok(None),
        }
    }

    /// Roll a failed merge back so nothing of it stands: every pending
    /// buffer discarded and the store snapshot restored; when a commit
    /// of this merge already landed (`landed` is the branch's tip then),
    /// the target branch moved back to `target_tip`, the tip the merge
    /// was pinned to (every commit above it is this merge's own, each
    /// parent-pinned, so no other writer's work sits between), and the
    /// target re-read from its backend. Returns `error`, the failure
    /// that triggered the unwind, unless the branch could not be moved
    /// back: then the landed commits stand and the error names both.
    fn unwind_merge(
        &mut self,
        target: &str,
        target_tip: &str,
        landed: Option<&str>,
        snapshot: &crate::store::Store,
        error: EngineError,
    ) -> EngineError {
        self.discard_all_pending();
        self.store = snapshot.clone();
        let Some(landed) = landed else {
            return error;
        };
        match self.branch_reset(target, target_tip, Some(landed)) {
            Ok(_) => {
                let _ = self.reload_one_mem(target);
                error
            }
            Err(reset) => {
                let _ = self.reload_one_mem(target);
                EngineError::Backend(crate::backend::BackendError::Other(format!(
                    "{}; the merge commits already on '{target}' (tip {landed}) could not be \
                     moved back to {target_tip} and stand: {reset}",
                    error.prose_render()
                )))
            }
        }
    }
}
