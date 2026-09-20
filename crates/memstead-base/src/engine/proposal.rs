//! `Engine::proposal_brief`: the three-way review of a fork against
//! its source mem. Types and the markdown renderer live in
//! [`crate::ops::proposal`]; this module reads.
//!
//! The picture is built from two two-tree walks of the mem-repo (the
//! git-branch diff hook, the same walk `memstead_diff` uses): the
//! fork's base to the fork's tip says what the fork did, the ancestor
//! to the target's tip says what the target did meanwhile. Entities
//! match by slug across the two mems. Bodies compare with mem-qualified
//! self-links normalised to the fork's name on every side, so a link
//! the fork commit retargeted is never a difference. The mechanics
//! precheck rehearses the create or update the merge would run in the
//! target, through the write gate's own dry run, over a store snapshot
//! that is restored afterwards. Anchor rows come from the fork's
//! sidecar under the fork's ids, as stored; nothing is observed.
//! Re-proposal marks come from the target's proposal record when the
//! target branch carries one.
//!
//! Nothing here writes: not the fork, not the target, not the
//! workspace state.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::backend::BackendError;
use crate::engine::{Engine, EngineError};
use crate::entity::{Entity, EntityId};
use crate::ops::proposal::{
    AnchorRow, Conflict, ConflictKind, DispositionSlot, PROPOSAL_RECORD_PATH, Precheck,
    PrecheckFailure, ProposalBrief, ProposalChange, ProposalEntry, ProposalRecord, ProposalSummary,
    ReProposalMark, Referrer, VersionDelta, proposal_id,
};
use crate::ops::{DiffConfig, EntityDiff};
use crate::workspace::MountStorage;

/// Metadata keys the engine stamps; never a difference between two
/// versions, never part of a rehearsed write.
const STAMPED_METADATA: &[&str] = &["type", "created_date", "last_modified"];

/// What one side (the fork, or the target) did to one slug, read off a
/// two-tree diff.
#[derive(Debug, Clone)]
enum SideChange {
    Added {
        after: Option<String>,
    },
    Modified {
        before: Option<String>,
        after: Option<String>,
    },
    Deleted {
        before: Option<String>,
    },
    /// Keyed by the new slug; `from` is the old one.
    Renamed {
        from: String,
        before: Option<String>,
        after: Option<String>,
    },
    /// A side that does not parse; the surviving bytes ride along.
    Invalid {
        error: String,
        before: Option<String>,
        after: Option<String>,
    },
}

impl Engine {
    /// The review brief of `fork` against the mem it was forked from.
    ///
    /// Refusals: `UNKNOWN_MEM` (the fork is not mounted, or its source
    /// is not), `INVALID_INPUT` (the mem records no `forkedFrom`; the
    /// fork or its source is not a git-branch mem of this mem-repo),
    /// `UNKNOWN_REF` (a recorded sha the mem-repo no longer resolves).
    /// The brief writes nothing.
    pub fn proposal_brief(&mut self, fork: &str) -> Result<ProposalBrief, EngineError> {
        let (gitdir, fork_branch) = match &self.find_mount(fork)?.mount.storage {
            MountStorage::GitBranch { gitdir, branch } => (gitdir.clone(), branch.clone()),
            _ => {
                return Err(EngineError::InvalidInput(format!(
                    "mem '{fork}' is not git-backed: a proposal brief reads a fork's branch \
                     against its source's branch"
                )));
            }
        };
        let forked_from = self
            .mem_config_for(fork)
            .and_then(|c| c.forked_from.clone())
            .ok_or_else(|| {
                EngineError::InvalidInput(format!(
                    "mem '{fork}' records no forkedFrom: it is not a fork, so there is no \
                     ancestor to compare against (`memstead mem fork` records one)"
                ))
            })?;
        let target = forked_from.mem.clone();
        let target_branch = match &self.find_mount(&target)?.mount.storage {
            MountStorage::GitBranch {
                gitdir: target_gitdir,
                branch,
            } => {
                if target_gitdir
                    .canonicalize()
                    .unwrap_or(target_gitdir.clone())
                    != gitdir.canonicalize().unwrap_or(gitdir.clone())
                {
                    return Err(EngineError::InvalidInput(format!(
                        "the source '{target}' of fork '{fork}' lives in another mem-repo \
                         ({}): a brief compares two branches of one mem-repo",
                        target_gitdir.display()
                    )));
                }
                branch.clone()
            }
            _ => {
                return Err(EngineError::InvalidInput(format!(
                    "the source '{target}' of fork '{fork}' is not git-backed: a proposal \
                     brief reads the source's branch"
                )));
            }
        };
        let Some(ops) = self.git_branch_ops() else {
            return Err(EngineError::Backend(BackendError::Other(
                "git-branch ops not installed (git-branch ops not wired)".to_string(),
            )));
        };

        // Fresh state on both sides, and the whole store loaded: the
        // target's referrers can sit in any mem.
        self.reload_if_stale(Some(fork));
        self.reload_if_stale(Some(&target));
        self.ensure_mems_loaded(None);

        let fork_ref = crate::branch_full_ref(&fork_branch);
        let target_ref = crate::branch_full_ref(&target_branch);
        let fork_tip = resolve_sha(&ops, &gitdir, &fork_ref)?;
        let target_tip = resolve_sha(&ops, &gitdir, &target_ref)?;
        let ancestor = forked_from.sha.clone();
        let base = forked_from.base_sha().to_string();
        let base_is_ancestor = forked_from.base.is_none();
        resolve_sha(&ops, &gitdir, &base)?;
        resolve_sha(&ops, &gitdir, &ancestor)?;

        // A rename in the brief is a recorded act (the engine's rename
        // note on the fork's branch, which the walk pairs exactly) or a
        // byte-identical move; content similarity never guesses one, so
        // a deletion beside an unrelated addition reads as both.
        let config = DiffConfig {
            rename_similarity: crate::ops::RENAME_SIMILARITY_MAX,
            include_content: true,
            include_ripple: false,
        };
        let fork_diff = run_diff(&ops, &gitdir, &fork_branch, fork, &base, &fork_tip, &config)?;
        let target_diff = run_diff(
            &ops,
            &gitdir,
            &target_branch,
            &target,
            &ancestor,
            &target_tip,
            &config,
        )?;
        let fork_changes = side_changes(&fork_diff.entries);
        let target_changes = side_changes(&target_diff.entries);

        let record = self.read_proposal_record(&target)?;
        let proposal = proposal_id(fork, &base);

        let mut entries: Vec<ProposalEntry> = Vec::with_capacity(fork_changes.len());
        for (slug, change) in &fork_changes {
            // A modification that is only the fork commit's retargeting
            // (a fork read against its ancestor) is no change at all:
            // the bodies agree once their mem qualifiers are normalised.
            if let SideChange::Modified {
                before: Some(before),
                after: Some(after),
            } = change
                && normalise_to(before, &target, fork) == normalise_to(after, &target, fork)
            {
                continue;
            }
            let entry = self.build_entry(
                fork,
                &target,
                slug,
                change,
                &target_changes,
                record.as_ref(),
            );
            entries.push(entry);
        }
        entries.sort_by(|a, b| a.slug.cmp(&b.slug));

        let mut summary = ProposalSummary::default();
        let mut dispositions = BTreeMap::new();
        for e in &entries {
            match e.status {
                ProposalChange::Added => summary.added += 1,
                ProposalChange::Modified => summary.modified += 1,
                ProposalChange::Deleted => summary.deleted += 1,
                ProposalChange::Renamed => summary.renamed += 1,
            }
            if e.conflict.is_some() {
                summary.conflicts += 1;
            }
            if e.precheck.outcome == crate::ops::proposal::PrecheckOutcome::Failed {
                summary.precheck_failures += 1;
            }
            if !e.re_proposal.is_empty() {
                summary.re_proposals += 1;
            }
            dispositions.insert(
                e.slug.clone(),
                DispositionSlot::skeleton(e.conflict.is_some()),
            );
        }

        Ok(ProposalBrief {
            fork: fork.to_string(),
            target,
            proposal_id: proposal,
            ancestor,
            base,
            base_is_ancestor,
            fork_tip,
            target_tip,
            summary,
            entries,
            dispositions,
        })
    }

    /// The target's proposal record, `None` when the branch carries
    /// none; a record that does not parse refuses, named.
    fn read_proposal_record(&self, target: &str) -> Result<Option<ProposalRecord>, EngineError> {
        let mount = self.find_mount(target)?;
        let bytes = mount
            .backend
            .read_entity(Path::new(PROPOSAL_RECORD_PATH))
            .map_err(EngineError::Backend)?;
        match bytes {
            None => Ok(None),
            Some(bytes) => ProposalRecord::from_bytes(&bytes).map(Some).map_err(|e| {
                EngineError::Mem(format!(
                    "the proposal record of '{target}' ({PROPOSAL_RECORD_PATH}) does not parse: {e}"
                ))
            }),
        }
    }

    /// One entry: the fork's change, the conflict against the target's
    /// change on the same slug, the target's referrers, the fork's
    /// anchor rows, the precheck and the re-proposal marks.
    fn build_entry(
        &mut self,
        fork: &str,
        target: &str,
        slug: &str,
        change: &SideChange,
        target_changes: &BTreeMap<String, SideChange>,
        record: Option<&ProposalRecord>,
    ) -> ProposalEntry {
        let fork_schema = self.schema_for(fork);
        let target_schema = self.schema_for(target);
        let normalise = |body: &Option<String>| -> Option<String> {
            body.as_deref().map(|b| normalise_to(b, target, fork))
        };
        let rel_path = crate::entity::id::id_to_file_path(&EntityId::new(fork, slug));

        let (status, renamed_from, base_body, fork_body, parse_failure) = match change {
            SideChange::Added { after } => {
                (ProposalChange::Added, None, None, normalise(after), None)
            }
            SideChange::Modified { before, after } => (
                ProposalChange::Modified,
                None,
                normalise(before),
                normalise(after),
                None,
            ),
            SideChange::Deleted { before } => {
                (ProposalChange::Deleted, None, normalise(before), None, None)
            }
            SideChange::Renamed {
                from,
                before,
                after,
            } => (
                ProposalChange::Renamed,
                Some(from.clone()),
                normalise(before),
                normalise(after),
                None,
            ),
            SideChange::Invalid {
                error,
                before,
                after,
            } => {
                let status = match (before, after) {
                    (None, _) => ProposalChange::Added,
                    (_, None) => ProposalChange::Deleted,
                    _ => ProposalChange::Modified,
                };
                (
                    status,
                    None,
                    normalise(before),
                    normalise(after),
                    Some(error.clone()),
                )
            }
        };

        let base_entity = base_body
            .as_deref()
            .and_then(|b| parse_body(b, &rel_path, fork, fork_schema.as_deref()));
        let fork_entity = fork_body
            .as_deref()
            .and_then(|b| parse_body(b, &rel_path, fork, fork_schema.as_deref()));
        let (title, entity_type) = match (&fork_entity, &base_entity) {
            (Some(e), _) | (None, Some(e)) => (Some(e.title.clone()), Some(e.entity_type.clone())),
            (None, None) => fork_body
                .as_deref()
                .or(base_body.as_deref())
                .map(crate::entity::parser::peek_title_and_type)
                .unwrap_or((None, None)),
        };
        let fork_delta = match (&base_entity, &fork_entity) {
            (Some(b), Some(f)) => Some(version_delta(b, f)),
            _ => None,
        };

        // The proposed version as it would land in the target: the
        // fork's body with its self-links under the target's name.
        let landing_body = fork_body.as_deref().map(|b| normalise_to(b, fork, target));
        let landing_entity = landing_body.as_deref().and_then(|b| {
            parse_body(
                b,
                &crate::entity::id::id_to_file_path(&EntityId::new(target, slug)),
                target,
                target_schema.as_deref(),
            )
        });
        let content_hash = landing_entity.as_ref().map(|e| e.content_hash.clone());

        // The conflict: the target moved the same slug (or, for a
        // rename, the slug the entity had) since the ancestor.
        let conflict = conflict_for(status, slug, renamed_from.as_deref(), target_changes).map(
            |(kind, target_body)| {
                let target_body = target_body.map(|b| normalise_to(&b, target, fork));
                let target_entity = target_body
                    .as_deref()
                    .and_then(|b| parse_body(b, &rel_path, fork, fork_schema.as_deref()));
                let target_delta = match (&base_entity, &target_entity) {
                    (Some(b), Some(t)) => Some(version_delta(b, t)),
                    _ => None,
                };
                Conflict {
                    kind,
                    target_delta,
                    target_body,
                }
            },
        );

        // The target's referrers of the entity the target holds: the
        // slug itself, or for a rename the slug it still has.
        let held_slug = renamed_from.as_deref().unwrap_or(slug);
        let held_id = EntityId::new(target, held_slug);
        let mut referrers: Vec<Referrer> = self
            .store
            .incoming(&held_id)
            .iter()
            .filter(|e| e.from.mem() != fork)
            .map(|e| Referrer {
                id: e.from.to_string(),
                rel_type: e.rel_type.clone(),
            })
            .collect();
        referrers.sort_by(|a, b| (&a.id, &a.rel_type).cmp(&(&b.id, &b.rel_type)));
        referrers.dedup();

        // The fork's anchor rows, as stored, under the fork's ids.
        let anchors: Vec<AnchorRow> = self
            .entity_anchors(&EntityId::new(fork, slug))
            .into_iter()
            .map(|a| AnchorRow {
                artifact: a.artifact.clone(),
                grain: a.grain.as_wire().to_string(),
                class: a.class.as_wire().to_string(),
                span: a.span.clone(),
                last_state: a
                    .last_observed
                    .as_ref()
                    .map(|o| o.state.as_wire().to_string()),
                last_observed_at: a.last_observed.as_ref().map(|o| o.at.clone()),
            })
            .collect();

        let precheck = match parse_failure {
            Some(error) => Precheck::failed(
                "parse",
                vec![PrecheckFailure {
                    code: "PARSE_ERROR".to_string(),
                    message: error,
                }],
            ),
            None => self.precheck(target, slug, status, &conflict, landing_entity.as_ref()),
        };

        let re_proposal: Vec<ReProposalMark> = record
            .map(|r| r.rejections_matching(slug, content_hash.as_deref()))
            .unwrap_or_default();

        ProposalEntry {
            slug: slug.to_string(),
            fork_id: EntityId::new(fork, slug).to_string(),
            target_id: EntityId::new(target, slug).to_string(),
            status,
            renamed_from,
            title,
            entity_type,
            content_hash,
            fork_delta,
            referrers,
            anchors,
            precheck,
            conflict,
            re_proposal,
            base_body,
            fork_body,
        }
    }

    /// Rehearse the write the merge would run in the target for this
    /// entry, through the write gate's dry run, over a store snapshot
    /// restored afterwards (a dry run may stage auto-stubs in memory).
    /// Never a content judgement: the gate refuses mechanics only.
    fn precheck(
        &mut self,
        target: &str,
        slug: &str,
        status: ProposalChange,
        conflict: &Option<Conflict>,
        landing: Option<&Entity>,
    ) -> Precheck {
        let Some(landing) = landing else {
            return match status {
                ProposalChange::Deleted => Precheck::not_run(
                    "a deletion is judged against the target's referrers, listed above",
                ),
                _ => Precheck::not_run("the fork's body does not parse under the target's schema"),
            };
        };
        if status == ProposalChange::Deleted {
            return Precheck::not_run(
                "a deletion is judged against the target's referrers, listed above",
            );
        }
        let target_id = EntityId::new(target, slug);
        let target_has = self.store.get(&target_id).is_some_and(|e| !e.stub);
        if let Some(c) = conflict {
            match c.kind {
                ConflictKind::TargetDeleted => {
                    return Precheck::not_run(
                        "the target no longer holds this entity; the owner resolves the conflict first",
                    );
                }
                ConflictKind::TargetAdded => {
                    return Precheck::not_run(
                        "the target holds its own entity under this slug; the owner resolves the conflict first",
                    );
                }
                ConflictKind::TargetModified => {}
            }
        }
        let sections: indexmap::IndexMap<String, String> = landing
            .sections
            .iter()
            .filter(|(k, _)| k.as_str() != "relationships")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let metadata: indexmap::IndexMap<String, String> = landing
            .metadata
            .iter()
            .filter(|(k, _)| !STAMPED_METADATA.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.to_frontmatter_string()))
            .collect();
        let snapshot = self.store.clone();
        let outcome: Result<(), EngineError> = if target_has {
            let existing = self.store.get(&target_id).cloned();
            let (sections_unset, metadata_unset, declare_relations) = match existing {
                Some(existing) => {
                    let sections_unset: Vec<String> = existing
                        .sections
                        .keys()
                        .filter(|k| k.as_str() != "relationships" && !sections.contains_key(*k))
                        .cloned()
                        .collect();
                    let metadata_unset: Vec<String> = existing
                        .metadata
                        .keys()
                        .filter(|k| {
                            !STAMPED_METADATA.contains(&k.as_str()) && !metadata.contains_key(*k)
                        })
                        .cloned()
                        .collect();
                    let declare_relations: Vec<crate::ops::RelateArg> = landing
                        .relationships
                        .iter()
                        .filter(|r| {
                            !existing
                                .relationships
                                .iter()
                                .any(|x| x.rel_type == r.rel_type && x.target == r.target)
                        })
                        .map(|r| crate::ops::RelateArg {
                            target: r.target.clone(),
                            rel_type: r.rel_type.clone(),
                            description: r.description.clone(),
                        })
                        .collect();
                    (sections_unset, metadata_unset, declare_relations)
                }
                None => (Vec::new(), Vec::new(), Vec::new()),
            };
            self.update_entity(
                crate::UpdateEntityArgs {
                    id: target_id.clone(),
                    expected_hash: None,
                    sections,
                    append_sections: Default::default(),
                    patch_sections: Default::default(),
                    sections_unset,
                    metadata,
                    metadata_unset,
                    dry_run: true,
                    declare_relations,
                    anchors: Vec::new(),
                    anchors_unset: Vec::new(),
                    relations_unset: Vec::new(),
                },
                crate::vcs::Actor::Cli,
                None,
                None,
            )
            .map(|_| ())
        } else {
            self.create_entity(
                crate::CreateEntityArgs {
                    mem: target.to_string(),
                    title: landing.title.clone(),
                    entity_type: landing.entity_type.clone(),
                    sections,
                    metadata,
                    relations: landing
                        .relationships
                        .iter()
                        .map(|r| crate::ops::RelateArg {
                            target: r.target.clone(),
                            rel_type: r.rel_type.clone(),
                            description: r.description.clone(),
                        })
                        .collect(),
                    anchors: Vec::new(),
                    dry_run: true,
                },
                crate::vcs::Actor::Cli,
                None,
                None,
            )
            .map(|_| ())
        };
        self.store = snapshot;
        let operation = if target_has { "update" } else { "create" };
        match outcome {
            Ok(()) => Precheck::clean(operation),
            Err(e) => Precheck::failed(
                operation,
                vec![PrecheckFailure {
                    code: e.code().to_string(),
                    message: e.prose_render(),
                }],
            ),
        }
    }
}

/// The sha a ref resolves to; `UNKNOWN_REF` when it does not.
fn resolve_sha(
    ops: &crate::engine::GitBranchOps,
    gitdir: &Path,
    ref_name: &str,
) -> Result<String, EngineError> {
    match (ops.resolve_ref)(gitdir, ref_name) {
        Ok(Some(sha)) => Ok(sha),
        Ok(None) => Err(EngineError::UnknownRef(ref_name.to_string())),
        Err(e) => Err(EngineError::Backend(e)),
    }
}

/// The two-tree walk between two refs of one branch's mem, ids under
/// `mem`.
fn run_diff(
    ops: &crate::engine::GitBranchOps,
    gitdir: &Path,
    branch: &str,
    mem: &str,
    ref_a: &str,
    ref_b: &str,
    config: &DiffConfig,
) -> Result<crate::ops::Diff, EngineError> {
    (ops.diff)(gitdir, branch, mem, ref_a, ref_b, config).map_err(|e| match e {
        BackendError::Other(msg) if msg.starts_with("UNKNOWN_REF:") => {
            EngineError::UnknownRef(msg.trim_start_matches("UNKNOWN_REF:").trim().to_string())
        }
        other => EngineError::Backend(other),
    })
}

/// A diff's entries keyed by slug (a rename by its new slug).
fn side_changes(entries: &[EntityDiff]) -> BTreeMap<String, SideChange> {
    let mut out = BTreeMap::new();
    for entry in entries {
        match entry {
            EntityDiff::Added {
                id, content_after, ..
            } => {
                out.insert(
                    id.path().to_string(),
                    SideChange::Added {
                        after: content_after.clone(),
                    },
                );
            }
            EntityDiff::Modified {
                id,
                content_before,
                content_after,
                ..
            } => {
                out.insert(
                    id.path().to_string(),
                    SideChange::Modified {
                        before: content_before.clone(),
                        after: content_after.clone(),
                    },
                );
            }
            EntityDiff::Deleted {
                id, content_before, ..
            } => {
                out.insert(
                    id.path().to_string(),
                    SideChange::Deleted {
                        before: content_before.clone(),
                    },
                );
            }
            EntityDiff::Renamed {
                from_id,
                to_id,
                content_before,
                content_after,
                ..
            } => {
                out.insert(
                    to_id.path().to_string(),
                    SideChange::Renamed {
                        from: from_id.path().to_string(),
                        before: content_before.clone(),
                        after: content_after.clone(),
                    },
                );
            }
            EntityDiff::InvalidEntity {
                id,
                error,
                content_before,
                content_after,
                ..
            } => {
                out.insert(
                    id.path().to_string(),
                    SideChange::Invalid {
                        error: error.clone(),
                        before: content_before.clone(),
                        after: content_after.clone(),
                    },
                );
            }
        }
    }
    out
}

/// A body with the mem-qualified self-links of `from` rewritten to
/// `to` (`[[from--slug]]`, `[[from:slug]]`), code spans and links
/// naming any other mem untouched. The export retargeting rule.
fn normalise_to(body: &str, from: &str, to: &str) -> String {
    let bytes = crate::ops::export::retarget_mem_links(body.as_bytes().to_vec(), from, to);
    String::from_utf8(bytes).unwrap_or_else(|_| body.to_string())
}

/// Parse a body under `mem` against the schema; `None` when the type
/// is unknown or the body does not parse.
fn parse_body(
    body: &str,
    rel_path: &str,
    mem: &str,
    schema: Option<&memstead_schema::Schema>,
) -> Option<Entity> {
    let schema = schema?;
    let (_, entity_type) = crate::entity::parser::peek_title_and_type(body);
    let type_def = schema.get_type(&entity_type?)?;
    crate::entity::parser::parse_markdown(body, rel_path, type_def.as_ref(), mem)
        .ok()
        .map(|r| r.entity)
}

/// How `side` differs from `base`.
fn version_delta(base: &Entity, side: &Entity) -> VersionDelta {
    let mut sections: Vec<String> = Vec::new();
    for (key, value) in &side.sections {
        if key == "relationships" {
            continue;
        }
        if base.sections.get(key) != Some(value) {
            sections.push(key.clone());
        }
    }
    for key in base.sections.keys() {
        if key != "relationships" && !side.sections.contains_key(key) {
            sections.push(key.clone());
        }
    }
    let mut metadata: Vec<String> = Vec::new();
    for (key, value) in &side.metadata {
        if STAMPED_METADATA.contains(&key.as_str()) {
            continue;
        }
        if base.metadata.get(key) != Some(value) {
            metadata.push(key.clone());
        }
    }
    for key in base.metadata.keys() {
        if !STAMPED_METADATA.contains(&key.as_str()) && !side.metadata.contains_key(key) {
            metadata.push(key.clone());
        }
    }
    let rels = |e: &Entity| -> Vec<(String, String, Option<String>)> {
        let mut v: Vec<_> = e
            .relationships
            .iter()
            .map(|r| {
                (
                    r.rel_type.clone(),
                    r.target.to_string(),
                    r.description.clone(),
                )
            })
            .collect();
        v.sort();
        v
    };
    VersionDelta {
        title_differs: base.title != side.title,
        entity_type_differs: base.entity_type != side.entity_type,
        sections,
        metadata,
        relationships_differ: rels(base) != rels(side),
    }
}

/// The conflict for one fork change against the target's changes:
/// the kind and the target's body (when the target still holds one).
fn conflict_for(
    status: ProposalChange,
    slug: &str,
    renamed_from: Option<&str>,
    target_changes: &BTreeMap<String, SideChange>,
) -> Option<(ConflictKind, Option<String>)> {
    // The target's changes read as: a slug it created, a slug it
    // modified, a slug it deleted (a rename is a deletion of the old
    // slug and a creation of the new one).
    let mut created: HashMap<&str, Option<String>> = HashMap::new();
    let mut modified: HashMap<&str, Option<String>> = HashMap::new();
    let mut deleted: Vec<&str> = Vec::new();
    for (target_slug, change) in target_changes {
        match change {
            SideChange::Added { after } => {
                created.insert(target_slug.as_str(), after.clone());
            }
            SideChange::Modified { after, .. } => {
                modified.insert(target_slug.as_str(), after.clone());
            }
            SideChange::Deleted { .. } => deleted.push(target_slug.as_str()),
            SideChange::Renamed { from, after, .. } => {
                deleted.push(from.as_str());
                created.insert(target_slug.as_str(), after.clone());
            }
            SideChange::Invalid { before, after, .. } => match (before, after) {
                (None, _) => {
                    created.insert(target_slug.as_str(), after.clone());
                }
                (_, None) => deleted.push(target_slug.as_str()),
                _ => {
                    modified.insert(target_slug.as_str(), after.clone());
                }
            },
        }
    }
    match status {
        ProposalChange::Added => created
            .get(slug)
            .map(|body| (ConflictKind::TargetAdded, body.clone())),
        ProposalChange::Modified | ProposalChange::Deleted => {
            if let Some(body) = modified.get(slug) {
                Some((ConflictKind::TargetModified, body.clone()))
            } else if deleted.contains(&slug) {
                Some((ConflictKind::TargetDeleted, None))
            } else {
                None
            }
        }
        ProposalChange::Renamed => {
            let from = renamed_from.unwrap_or(slug);
            if let Some(body) = created.get(slug) {
                Some((ConflictKind::TargetAdded, body.clone()))
            } else if let Some(body) = modified.get(from) {
                Some((ConflictKind::TargetModified, body.clone()))
            } else if deleted.contains(&from) {
                Some((ConflictKind::TargetDeleted, None))
            } else {
                None
            }
        }
    }
}
