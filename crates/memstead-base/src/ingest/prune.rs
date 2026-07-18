//! Prune — deletion **proposal** machinery (bundle plan `05-verify-sync-engine`,
//! group F).
//!
//! Prune answers "the source removed this artifact entirely — should the entity
//! describing it be deleted?". It **never** mutates the destination mem: it
//! produces [`PruneProposal`]s the **sync brief** surfaces, and the deletion
//! reaches the mem only when an agent acts on that brief through the normal MCP
//! mutation surface (A5 holds — there is no engine path from here that deletes
//! or writes a mem entity). [`prune_proposals`] takes a shared `&Engine`, so it
//! is structurally incapable of a mem mutation.
//!
//! ## Guarantee (F1) and degradation (F2)
//!
//! A binding requests a [`crate::binding::PruneGuarantee`]. The guarantee a
//! medium can *support* is stated at binding-validation time (a `never-clobber`
//! request over a non-base-retrievable medium is refused there, never at run
//! time). At proposal time prune resolves the **effective** posture per
//! candidate:
//!
//! - **never-clobber** — where the candidate's source **base leg is
//!   retrievable** (a git-pinned anchor: `at_version` is a commit), a three-way
//!   merge can tell a model-side edit apart from a clean removal, so a clean
//!   removal can be proposed as a confident (agent-enacted) delete.
//! - **conflict-flag degradation** — everywhere else (a `conflict-flag`
//!   request, or a candidate with **no** retrievable base leg — a non-git
//!   source): prune presents **both** sides and never proposes a clean delete,
//!   so a model-side edit is never silently clobbered. This is the decided
//!   posture; span-snapshot base legs for non-git sources are out of scope (no
//!   current payer).
//!
//! ## Provenance guards (F3)
//!
//! - an `authored`-provenance entity is **never** a prune target (excluded
//!   entirely — no proposal is produced);
//! - a `derived` entity is **flagged with its inputs**, never auto-proposed for
//!   deletion — its inputs must be re-examined first;
//! - only `anchored` / `informed-by` entities whose whole source basis vanished
//!   become delete proposals, and only conservatively (every anchor orphaned).

use std::collections::BTreeMap;
use std::path::Path;

use crate::Engine;
use crate::anchor::{AnchorProvenanceClass, AnchorState, AnchorVersion};
use crate::binding::{Binding, PruneGuarantee};

use super::resolve::ResolvedIngest;

/// The **effective** prune posture for a candidate (F1/F2) — the requested
/// guarantee resolved against what is actually retrievable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneMode {
    /// Never-clobber three-way merge is in force (a `never-clobber` request).
    /// Whether a *given* candidate can use it still depends on that candidate's
    /// base-leg retrievability — a candidate with no retrievable base degrades
    /// to conflict-flagging.
    NeverClobber,
    /// Conflict-flag degradation is in force (a `conflict-flag` request): both
    /// sides are always presented, a clean delete is never proposed.
    ConflictFlag,
}

impl PruneMode {
    /// The effective posture a binding's requested guarantee selects.
    pub fn from_guarantee(guarantee: PruneGuarantee) -> Self {
        match guarantee {
            PruneGuarantee::NeverClobber => PruneMode::NeverClobber,
            PruneGuarantee::ConflictFlag => PruneMode::ConflictFlag,
        }
    }
}

/// The three-way-merge outcome for a never-clobber candidate whose base leg was
/// retrieved: did the model side diverge from the retrieved base?
///
/// The model-divergence signal (comparing the current entity against the base
/// leg) is not wired this cycle, so [`prune_proposals`] supplies `None` and
/// every candidate conservatively conflict-flags. The [`PruneMerge::Clean`]
/// branch is the reachable, tested seam a future model-divergence check drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneMerge {
    /// Base retrieved, model side unchanged from it — a clean removal.
    Clean,
    /// Base retrieved, model side diverged (a hand edit) — a real conflict.
    Conflict,
}

/// The disposition a prune proposal carries (F2/F3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneDisposition {
    /// Never-clobber, base leg retrieved, three-way merge clean → a confident
    /// (still agent-enacted) delete proposal.
    CleanDelete,
    /// Both sides presented; the agent decides. Never an auto-write — the model
    /// side may carry a deliberate edit. Conflict-flag degradation, or a
    /// never-clobber merge that found (or could not rule out) a divergence.
    ConflictFlag,
    /// A `derived` entity — flagged with its inputs, never auto-proposed for
    /// deletion (F3).
    DerivedFlagged,
}

impl PruneDisposition {
    /// Stable wire form.
    pub fn as_wire(&self) -> &'static str {
        match self {
            PruneDisposition::CleanDelete => "clean-delete",
            PruneDisposition::ConflictFlag => "conflict-flag",
            PruneDisposition::DerivedFlagged => "derived-flagged",
        }
    }
}

/// Classify one candidate entity into a prune disposition, or `None` when it is
/// **excluded entirely** — an `authored`-provenance entity is never a prune
/// target (F3).
///
/// - `authored` → `None` (never targeted);
/// - `derived` → [`PruneDisposition::DerivedFlagged`] (flagged with inputs,
///   never a delete);
/// - `anchored` / `informed-by`:
///   - conflict-flag mode → [`PruneDisposition::ConflictFlag`] (both sides);
///   - never-clobber mode → [`PruneDisposition::CleanDelete`] **only** when the
///     base leg is retrievable **and** the merge is clean; otherwise
///     [`PruneDisposition::ConflictFlag`] (no retrievable base, or a divergent /
///     undetermined merge — never a silent clobber).
pub fn classify_prune_candidate(
    class: AnchorProvenanceClass,
    mode: PruneMode,
    base_retrievable: bool,
    merge: Option<PruneMerge>,
) -> Option<PruneDisposition> {
    match class {
        // F3 — an authored entity is never a prune target.
        AnchorProvenanceClass::Authored => None,
        // F3 — a derived entity is flagged with its inputs, never a delete.
        AnchorProvenanceClass::Derived => Some(PruneDisposition::DerivedFlagged),
        AnchorProvenanceClass::Anchored | AnchorProvenanceClass::InformedBy => match mode {
            PruneMode::ConflictFlag => Some(PruneDisposition::ConflictFlag),
            PruneMode::NeverClobber => {
                if base_retrievable && matches!(merge, Some(PruneMerge::Clean)) {
                    Some(PruneDisposition::CleanDelete)
                } else {
                    // No retrievable base, a divergent merge, or an
                    // undetermined merge — degrade, never clobber.
                    Some(PruneDisposition::ConflictFlag)
                }
            }
        },
    }
}

/// A single prune proposal — a proposed removal the sync brief surfaces. The
/// engine never enacts it: an agent acting on the sync brief deletes (or keeps)
/// the entity through the MCP mutation surface (A5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneProposal {
    /// The destination-mem entity id (`mem--slug`) the proposal concerns.
    pub entity: String,
    /// The now-gone source artifacts the entity's (all-orphaned) anchors
    /// referenced, deduplicated and sorted.
    pub artifacts: Vec<String>,
    /// The entity's dominant provenance class wire string (the class the
    /// disposition was decided from).
    pub class: String,
    /// The disposition (F2/F3).
    pub disposition: PruneDisposition,
    /// Whether the candidate's source base leg is retrievable (a git-pinned
    /// anchor). Drives the never-clobber vs. conflict-flag posture and is
    /// surfaced so the brief can state which one applies.
    pub base_retrievable: bool,
    /// For a `derived` candidate: the input artifact refs to re-examine before
    /// any removal (F3). Empty for every other class.
    pub derived_inputs: Vec<String>,
}

/// Gather the prune proposals for a binding — **read-only** on the destination
/// mem (shared `&Engine`; no mutation is structurally possible, A5). Returns an
/// empty vec when the binding declares no `prune` block (prune disabled).
///
/// A **candidate** is an entity whose *entire* source basis vanished — every one
/// of its anchors resolves [`AnchorState::Orphaned`] against the live source
/// (the conservative "concept removed entirely" signal; an entity with any
/// still-resolving anchor is left to sync's ordinary drift path, not prune). An
/// entity with any *unobserved* anchor is skipped — prune never asserts a
/// removal it could not observe.
pub fn prune_proposals(
    engine: &Engine,
    _workspace_root: &Path,
    binding: &Binding,
    resolved: &ResolvedIngest,
) -> Vec<PruneProposal> {
    // Prune disabled → no proposals.
    let Some(prune) = binding.prune.as_ref() else {
        return Vec::new();
    };
    let mode = PruneMode::from_guarantee(prune.guarantee);

    // Group the destination mem's resolved anchors by entity.
    struct Acc {
        classes: Vec<AnchorProvenanceClass>,
        artifacts: Vec<String>,
        base_retrievable: bool,
        derived_inputs: Vec<String>,
        all_orphaned: bool,
        any: bool,
    }
    let mut by_entity: BTreeMap<String, Acc> = BTreeMap::new();
    for (eid, resolved_anchor) in engine.mem_anchors_resolved(&resolved.destination_mem) {
        let entry = by_entity.entry(eid.as_ref().to_string()).or_insert(Acc {
            classes: Vec::new(),
            artifacts: Vec::new(),
            base_retrievable: false,
            derived_inputs: Vec::new(),
            all_orphaned: true,
            any: false,
        });
        entry.any = true;
        let anchor = &resolved_anchor.anchor;
        entry.classes.push(anchor.class);
        entry.artifacts.push(anchor.artifact.clone());
        // A git-pinned commit is a retrievable base leg for the three-way merge.
        if matches!(anchor.at_version, Some(AnchorVersion::Commit(_))) {
            entry.base_retrievable = true;
        }
        if anchor.class == AnchorProvenanceClass::Derived {
            entry
                .derived_inputs
                .extend(anchor.derived_from.iter().cloned());
        }
        // Every anchor must resolve orphaned for the whole basis to be gone;
        // an unobserved anchor (state None) blocks the candidate — prune never
        // asserts a removal it could not observe.
        match resolved_anchor.state {
            Some(AnchorState::Orphaned) => {}
            _ => entry.all_orphaned = false,
        }
    }

    let mut proposals: Vec<PruneProposal> = Vec::new();
    for (entity, acc) in by_entity {
        if !acc.any || !acc.all_orphaned {
            continue;
        }
        // Dominant class precedence: authored (exclude) > derived (flag) >
        // anchored > informed-by.
        let dominant = if acc.classes.contains(&AnchorProvenanceClass::Authored) {
            AnchorProvenanceClass::Authored
        } else if acc.classes.contains(&AnchorProvenanceClass::Derived) {
            AnchorProvenanceClass::Derived
        } else if acc.classes.contains(&AnchorProvenanceClass::Anchored) {
            AnchorProvenanceClass::Anchored
        } else {
            AnchorProvenanceClass::InformedBy
        };

        // Merge outcome is unwired this cycle → None → conservative conflict-flag.
        let Some(disposition) =
            classify_prune_candidate(dominant, mode, acc.base_retrievable, None)
        else {
            // Authored → excluded, never a prune target (F3).
            continue;
        };

        let mut artifacts = acc.artifacts;
        artifacts.sort();
        artifacts.dedup();
        let mut derived_inputs = acc.derived_inputs;
        derived_inputs.sort();
        derived_inputs.dedup();

        proposals.push(PruneProposal {
            entity,
            artifacts,
            class: dominant.as_wire().to_string(),
            disposition,
            base_retrievable: acc.base_retrievable,
            derived_inputs,
        });
    }
    proposals
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- F2/F3: pure classifier -----------------------------------------

    /// F3 — an authored entity is never a prune target: excluded (no proposal),
    /// in either mode.
    #[test]
    fn authored_is_never_a_prune_target() {
        for mode in [PruneMode::NeverClobber, PruneMode::ConflictFlag] {
            assert_eq!(
                classify_prune_candidate(
                    AnchorProvenanceClass::Authored,
                    mode,
                    true,
                    Some(PruneMerge::Clean),
                ),
                None,
                "authored must never be proposed for deletion"
            );
        }
    }

    /// F3 — a derived entity is flagged (with inputs), never auto-proposed for
    /// deletion, in either mode.
    #[test]
    fn derived_is_flagged_not_deleted() {
        for mode in [PruneMode::NeverClobber, PruneMode::ConflictFlag] {
            assert_eq!(
                classify_prune_candidate(
                    AnchorProvenanceClass::Derived,
                    mode,
                    true,
                    Some(PruneMerge::Clean),
                ),
                Some(PruneDisposition::DerivedFlagged),
                "derived is flagged, never a clean delete"
            );
        }
    }

    /// F2 — conflict-flag mode always presents both sides (never a clean
    /// delete), whatever the base/merge state.
    #[test]
    fn conflict_flag_mode_never_clean_deletes() {
        for base in [true, false] {
            for merge in [None, Some(PruneMerge::Clean), Some(PruneMerge::Conflict)] {
                assert_eq!(
                    classify_prune_candidate(
                        AnchorProvenanceClass::Anchored,
                        PruneMode::ConflictFlag,
                        base,
                        merge,
                    ),
                    Some(PruneDisposition::ConflictFlag),
                    "conflict-flag mode never auto-clean-deletes"
                );
            }
        }
    }

    /// F2 — never-clobber degrades to conflict-flag when the base leg is not
    /// retrievable (a non-git source), or when the merge is divergent /
    /// undetermined; it clean-deletes only with a retrievable base AND a clean
    /// merge.
    #[test]
    fn never_clobber_clean_delete_needs_base_and_clean_merge() {
        let anchored = AnchorProvenanceClass::Anchored;
        // Retrievable base + clean merge → the one clean-delete path.
        assert_eq!(
            classify_prune_candidate(
                anchored,
                PruneMode::NeverClobber,
                true,
                Some(PruneMerge::Clean)
            ),
            Some(PruneDisposition::CleanDelete)
        );
        // No retrievable base (non-git) → conflict-flag degradation.
        assert_eq!(
            classify_prune_candidate(
                anchored,
                PruneMode::NeverClobber,
                false,
                Some(PruneMerge::Clean)
            ),
            Some(PruneDisposition::ConflictFlag),
            "no base leg degrades to conflict-flag"
        );
        // Divergent merge → conflict-flag (never clobber the model edit).
        assert_eq!(
            classify_prune_candidate(
                anchored,
                PruneMode::NeverClobber,
                true,
                Some(PruneMerge::Conflict)
            ),
            Some(PruneDisposition::ConflictFlag),
            "a divergent merge is never a clean delete"
        );
        // Undetermined merge (signal unwired) → conflict-flag (safe default).
        assert_eq!(
            classify_prune_candidate(anchored, PruneMode::NeverClobber, true, None),
            Some(PruneDisposition::ConflictFlag),
            "an undetermined merge conservatively conflict-flags"
        );
    }

    /// `informed-by` is a delete candidate too (a non-hash class that still owns
    /// a concept), following the same mode rules as `anchored`.
    #[test]
    fn informed_by_follows_the_same_mode_rules() {
        assert_eq!(
            classify_prune_candidate(
                AnchorProvenanceClass::InformedBy,
                PruneMode::ConflictFlag,
                false,
                None,
            ),
            Some(PruneDisposition::ConflictFlag)
        );
    }

    // ---- F2/F3: end-to-end over a real engine ----------------------------

    use crate::anchor::{Anchor, AnchorGrain, AnchorHashStability, AnchorSidecar};
    use crate::binding::{
        BINDING_VERSION, BuildMode, BuildOperation, CoverageSemantics, DEFAULT_ADJUDICATION_CAP,
        DEFAULT_FULL_RESYNC_EVERY, Operations, PruneConfig, VerifyOperation,
    };
    use crate::ingest::render::render_sync_brief_for;
    use crate::ingest::resolve::resolve_binding_run;
    use crate::pipeline::{Facet, IngestTrigger, Medium, MediumType, PatternEntry, PatternMode};
    use crate::pipeline_store::{load_pipeline_configs, write_binding, write_facet, write_medium};
    use crate::workspace::{
        Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
    };
    use crate::workspace_store::WorkspaceStoreAdapter;

    /// An orphan-bound anchor of `class` on `artifact`, git-pinned when
    /// `commit` is set (a retrievable base leg).
    fn orphan_anchor(
        artifact: &str,
        class: AnchorProvenanceClass,
        derived_from: Vec<&str>,
        commit: Option<&str>,
    ) -> Anchor {
        Anchor {
            artifact: artifact.to_string(),
            grain: AnchorGrain::File,
            class,
            at_version: commit.map(|c| AnchorVersion::Commit(c.to_string())),
            hash: class.is_hash_bearing().then(|| "recorded".to_string()),
            hash_stability: AnchorHashStability::Stable,
            derived_from: derived_from.into_iter().map(str::to_string).collect(),
            binding: None,
        }
    }

    /// Scaffold a filesystem-medium mem whose anchors reference **absent** source
    /// files (so every anchor resolves orphaned), with a `prune` block at
    /// `guarantee`. Returns the engine, workspace root, binding and resolved run.
    /// The source is deliberately **non-git** (a plain filesystem medium, no
    /// `at_version` unless the fixture pins one) so the base leg is not
    /// retrievable — the F2 degradation case.
    fn setup(
        tmp: &Path,
        guarantee: PruneGuarantee,
        entity_anchors: &[(&str, Vec<Anchor>)],
    ) -> (Engine, std::path::PathBuf, Binding, ResolvedIngest) {
        let root = tmp.to_path_buf();
        let mem_dir = root.join("mem");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        let mount = Mount {
            mem: "engine".to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: MountStorage::Folder {
                path: mem_dir.clone(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        };
        crate::FileWorkspaceStore::new()
            .save_state(
                &root,
                &Workspace {
                    mounts: vec![mount],
                    settings: WorkspaceSettings::default(),
                },
            )
            .unwrap();

        // Seed the anchors sidecar (test fixture — the production write path is
        // the mutation surface, not prune). No source files are created, so every
        // anchor resolves orphaned.
        let mut sidecar = AnchorSidecar::default();
        for (eid, anchors) in entity_anchors {
            sidecar.set(eid, anchors.clone());
        }
        std::fs::write(
            mem_dir.join(crate::anchor::ANCHOR_SIDECAR_PATH),
            sidecar.to_bytes(),
        )
        .unwrap();

        // A filesystem-source binding (namespace `path`, so mem_anchors_resolved
        // observes it) with the requested prune guarantee.
        let binding = Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![crate::pipeline::Source {
                name: "graph".to_string(),
                medium_type: MediumType::Filesystem,
                pointer: String::new(),
                change_detection: None,
                scope: vec![PatternEntry {
                    path: "src/**/*.rs".to_string(),
                    mode: PatternMode::Allow,
                }],
                engagement: None,
                preparation: None,
            }],
            reference_mems: Vec::new(),
            destination_mem: "engine".to_string(),
            deny_paths: Vec::new(),
            coverage_semantics: CoverageSemantics::Exhaustive,
            rules: None,
            prune: Some(PruneConfig { guarantee }),
            operations: Operations {
                build: Some(BuildOperation {
                    mode: BuildMode::Discovery,
                    trigger: IngestTrigger::Loop,
                    batch_size: 20,
                    post_actions: None,
                }),
                sync: Some(crate::binding::SyncOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 20,
                }),
                verify: Some(VerifyOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 20,
                    adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                    full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
                }),
            },
        };
        write_binding(&root, "engine", "graph", &binding).unwrap();

        let engine = Engine::from_workspace_root(&root).unwrap();
        let configs = load_pipeline_configs(&root).unwrap();
        let resolved = resolve_binding_run("engine/graph", &binding).unwrap();
        (engine, root, binding, resolved)
    }

    /// F2 — conflict-flag degradation on a **non-git** source: a model-side
    /// entity whose source artifact was removed surfaces BOTH sides in the sync
    /// brief and is NEVER auto-deleted. Requesting never-clobber over a non-git
    /// anchor (no retrievable base leg) degrades to conflict-flag.
    #[test]
    fn f2_conflict_flag_on_non_git_surfaces_both_sides_no_auto_delete() {
        let tmp = tempfile::tempdir().unwrap();
        // Request never-clobber; the non-git anchor has no base leg → degrades.
        let (engine, root, binding, resolved) = setup(
            tmp.path(),
            PruneGuarantee::NeverClobber,
            &[(
                "engine--removed",
                vec![orphan_anchor(
                    "src/removed.rs",
                    AnchorProvenanceClass::Anchored,
                    vec![],
                    None, // non-git: no retrievable base leg
                )],
            )],
        );

        let proposals = prune_proposals(&engine, &root, &binding, &resolved);
        assert_eq!(proposals.len(), 1, "the orphaned entity is a candidate");
        let p = &proposals[0];
        assert_eq!(p.entity, "engine--removed");
        assert!(
            !p.base_retrievable,
            "non-git anchor has no retrievable base"
        );
        assert_eq!(
            p.disposition,
            PruneDisposition::ConflictFlag,
            "no base leg → conflict-flag degradation, never a clean delete"
        );

        // The rendered sync brief presents BOTH sides and frames it as a proposal.
        let brief = render_sync_brief_for(&engine, &root, "engine/graph").unwrap();
        assert!(brief.contains("Prune — proposed removals"));
        assert!(brief.contains("source side:"), "source side surfaced");
        assert!(brief.contains("model side:"), "model side surfaced");
        assert!(
            brief.contains("never overwrites a model-side edit"),
            "no auto-overwrite is stated"
        );
        // A5: the pass never mutated the mem — the entity's anchor is still there
        // (prune_proposals took a shared &Engine; a delete is structurally
        // impossible). Re-read the sidecar to confirm.
        let after = engine.mem_anchors_resolved("engine");
        assert!(
            after.iter().any(|(e, _)| e.as_ref() == "engine--removed"),
            "prune must not delete the entity's anchors — it only proposes"
        );
    }

    /// F3 — provenance guards: an `authored` entity is NEVER a prune target
    /// (excluded, no proposal); a `derived` entity is flagged with its inputs,
    /// never proposed for deletion; a plain `anchored` entity is proposed.
    #[test]
    fn f3_authored_excluded_and_derived_flagged_not_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, root, binding, resolved) = setup(
            tmp.path(),
            PruneGuarantee::ConflictFlag,
            &[
                (
                    "engine--handwritten",
                    vec![orphan_anchor(
                        "src/authored.rs",
                        AnchorProvenanceClass::Authored,
                        vec![],
                        None,
                    )],
                ),
                (
                    "engine--synthesised",
                    vec![orphan_anchor(
                        "src/derived.rs",
                        AnchorProvenanceClass::Derived,
                        vec!["src/in_a.rs", "src/in_b.rs"],
                        None,
                    )],
                ),
                (
                    "engine--plain",
                    vec![orphan_anchor(
                        "src/plain.rs",
                        AnchorProvenanceClass::Anchored,
                        vec![],
                        None,
                    )],
                ),
            ],
        );

        let proposals = prune_proposals(&engine, &root, &binding, &resolved);

        // F3 — authored is never a prune target: no proposal names it.
        assert!(
            !proposals.iter().any(|p| p.entity == "engine--handwritten"),
            "an authored entity is never proposed for deletion"
        );

        // F3 — derived is flagged with its inputs, not proposed for deletion.
        let derived = proposals
            .iter()
            .find(|p| p.entity == "engine--synthesised")
            .expect("the derived entity is flagged");
        assert_eq!(derived.disposition, PruneDisposition::DerivedFlagged);
        assert_eq!(derived.class, "derived");
        assert_eq!(
            derived.derived_inputs,
            vec!["src/in_a.rs".to_string(), "src/in_b.rs".to_string()],
            "the derived entity carries its inputs to re-examine"
        );

        // The plain anchored entity IS proposed (conflict-flag).
        let plain = proposals
            .iter()
            .find(|p| p.entity == "engine--plain")
            .expect("a plain anchored entity is proposed");
        assert_eq!(plain.disposition, PruneDisposition::ConflictFlag);

        // The rendered sync brief flags the derived entity as NOT-for-deletion,
        // never emits an auto-delete instruction, and never names the authored one.
        let brief = render_sync_brief_for(&engine, &root, "engine/graph").unwrap();
        assert!(brief.contains("flagged, NOT proposed for deletion"));
        assert!(brief.contains("`engine--synthesised`"));
        assert!(
            !brief.contains("engine--handwritten"),
            "the authored entity never appears in a prune proposal"
        );
        assert!(brief.contains("nothing is auto-deleted"));
    }

    /// A `never-clobber` binding whose anchor IS git-pinned has a retrievable
    /// base leg — the proposal reports it (the never-clobber posture), while
    /// still degrading to conflict-flag until the model-divergence merge signal
    /// is wired (the gatherer supplies no merge outcome this cycle).
    #[test]
    fn git_pinned_anchor_reports_a_retrievable_base_leg() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, root, binding, resolved) = setup(
            tmp.path(),
            PruneGuarantee::NeverClobber,
            &[(
                "engine--pinned",
                vec![orphan_anchor(
                    "src/pinned.rs",
                    AnchorProvenanceClass::Anchored,
                    vec![],
                    Some("deadbeef"),
                )],
            )],
        );
        let proposals = prune_proposals(&engine, &root, &binding, &resolved);
        assert_eq!(proposals.len(), 1);
        assert!(
            proposals[0].base_retrievable,
            "a git-pinned anchor exposes a retrievable base leg"
        );
        // Merge outcome unwired → still conflict-flag (never a silent clobber).
        assert_eq!(proposals[0].disposition, PruneDisposition::ConflictFlag);
    }

    /// An entity with a **still-resolving** anchor is NOT a prune candidate —
    /// the whole basis must be gone (conservatism). Here one anchor's file
    /// exists, so the entity is skipped.
    #[test]
    fn entity_with_a_surviving_anchor_is_not_pruned() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, root, binding, resolved) = setup(
            tmp.path(),
            PruneGuarantee::ConflictFlag,
            &[(
                "engine--partly-gone",
                vec![
                    orphan_anchor("src/gone.rs", AnchorProvenanceClass::Anchored, vec![], None),
                    orphan_anchor(
                        "src/present.rs",
                        AnchorProvenanceClass::InformedBy,
                        vec![],
                        None,
                    ),
                ],
            )],
        );
        // Create only the second file so its anchor resolves (not orphaned).
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("present.rs"), "fn a() {}\n").unwrap();

        let proposals = prune_proposals(&engine, &root, &binding, &resolved);
        assert!(
            proposals.is_empty(),
            "an entity whose basis is not entirely gone is not a prune candidate"
        );
    }
}
