//! Prune — deletion **proposal** machinery.
//!
//! Prune answers "the source removed this artifact entirely — should the entity
//! describing it be deleted?". It **never** mutates the destination mem: it
//! produces [`PruneProposal`]s the **sync brief** surfaces, and the deletion
//! reaches the mem only when an agent acts on that brief through the normal MCP
//! mutation surface (A5 holds — there is no engine path from here that deletes
//! or writes a mem entity). [`prune_proposals`] takes a shared `&Engine`, so it
//! is structurally incapable of a mem mutation.
//!
//! ## Why prune proposes and never decides
//!
//! A source-derived mem is a rebuildable mirror of its source, and every
//! entity in it is agent-authored prose *about* a source artifact, not a copy
//! of it. The artifact and the entity share no common ancestor a three-way
//! merge could compare, and the sync loop edits anchored entities as its
//! ordinary work, so "has the model side changed since the build" has no
//! mechanical answer and no meaning as a guard. The engine therefore states
//! what it observed — every anchor of the entity resolves orphaned — and
//! leaves the disposition to the agent reading the brief, whose rule the
//! engineering mem records: delete, unless a knowledge mem cites the subject,
//! then keep the entity as a frozen historical record. (The never-clobber /
//! conflict-flag merge vocabulary that once framed this was retired on
//! 2026-09-10; the operator's decision of 2026-08-19 had already fixed the
//! posture.)
//!
//! ## Provenance guards (F3)
//!
//! - an `authored`-provenance entity is **never** a prune target (excluded
//!   entirely — no proposal is produced);
//! - a `derived` entity is **flagged with its inputs**, never proposed for
//!   deletion — its inputs must be re-examined first;
//! - only `anchored` / `informed-by` entities whose whole source basis vanished
//!   become proposals, and only conservatively (every anchor orphaned).

use std::collections::BTreeMap;
use std::path::Path;

use memstead_base::Engine;
use memstead_base::anchor::{AnchorProvenanceClass, AnchorState};
use memstead_base::binding::Binding;

use memstead_base::binding_run::ResolvedIngest;

/// The disposition a prune proposal carries (F3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneDisposition {
    /// The entity's whole source basis is gone; both sides are presented and
    /// the agent decides. Never an auto-write.
    Proposed,
    /// A `derived` entity — flagged with its inputs, never proposed for
    /// deletion (F3).
    DerivedFlagged,
}

impl PruneDisposition {
    /// Stable wire form.
    pub fn as_wire(&self) -> &'static str {
        match self {
            PruneDisposition::Proposed => "proposed",
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
/// - `anchored` / `informed-by` → [`PruneDisposition::Proposed`] (both sides
///   presented, the agent decides).
pub fn classify_prune_candidate(class: AnchorProvenanceClass) -> Option<PruneDisposition> {
    match class {
        // F3 — an authored entity is never a prune target.
        AnchorProvenanceClass::Authored => None,
        // F3 — a derived entity is flagged with its inputs, never a delete.
        AnchorProvenanceClass::Derived => Some(PruneDisposition::DerivedFlagged),
        AnchorProvenanceClass::Anchored | AnchorProvenanceClass::InformedBy => {
            Some(PruneDisposition::Proposed)
        }
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
    /// The disposition (F3).
    pub disposition: PruneDisposition,
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
    if binding.prune.is_none() {
        return Vec::new();
    }

    // Group THIS BINDING'S anchors by entity (consistency-sweep 03/01).
    // Proposing deletion over another binding's anchors, or over artifacts
    // this binding's scope excludes, would act on a population it does not
    // answer for, and prune acts rather than merely reports.
    struct Acc {
        classes: Vec<AnchorProvenanceClass>,
        artifacts: Vec<String>,
        derived_inputs: Vec<String>,
        all_orphaned: bool,
        any: bool,
    }
    let mut by_entity: BTreeMap<String, Acc> = BTreeMap::new();
    let population = crate::anchor_population::population_for(
        engine,
        resolved,
        Some(memstead_base::binding::hash_binding(binding).as_str()),
    );
    for (eid, resolved_anchor) in population.included {
        let entry = by_entity.entry(eid.as_ref().to_string()).or_insert(Acc {
            classes: Vec::new(),
            artifacts: Vec::new(),
            derived_inputs: Vec::new(),
            all_orphaned: true,
            any: false,
        });
        entry.any = true;
        let anchor = &resolved_anchor.anchor;
        entry.classes.push(anchor.class);
        entry.artifacts.push(anchor.artifact.clone());
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

        let Some(disposition) = classify_prune_candidate(dominant) else {
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
            derived_inputs,
        });
    }
    proposals
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- F3: pure classifier ------------------------------------------------

    /// F3 — an authored entity is never a prune target: excluded (no proposal).
    #[test]
    fn authored_is_never_a_prune_target() {
        assert_eq!(
            classify_prune_candidate(AnchorProvenanceClass::Authored),
            None
        );
    }

    /// F3 — a derived entity is flagged with its inputs, never proposed.
    #[test]
    fn derived_is_flagged_not_deleted() {
        assert_eq!(
            classify_prune_candidate(AnchorProvenanceClass::Derived),
            Some(PruneDisposition::DerivedFlagged)
        );
    }

    /// An anchored or informed-by entity whose basis is gone is proposed —
    /// both sides shown, the agent decides — never anything stronger.
    #[test]
    fn anchored_and_informed_by_are_proposed() {
        for class in [
            AnchorProvenanceClass::Anchored,
            AnchorProvenanceClass::InformedBy,
        ] {
            assert_eq!(
                classify_prune_candidate(class),
                Some(PruneDisposition::Proposed),
                "{class:?} is proposed, never auto-deleted"
            );
        }
    }

    // ---- F2/F3: end-to-end over a real engine ----------------------------

    use crate::render::render_sync_brief_for;
    use memstead_base::anchor::{
        Anchor, AnchorGrain, AnchorHashStability, AnchorSidecar, AnchorVersion,
    };
    use memstead_base::binding::{
        BINDING_VERSION, BuildMode, BuildOperation, DEFAULT_ADJUDICATION_CAP,
        DEFAULT_FULL_RESYNC_EVERY, Operations, PruneConfig, VerifyOperation,
    };
    use memstead_base::binding_run::resolve_binding_run;
    use memstead_base::pipeline::{IngestTrigger, MediumType, PatternEntry, PatternMode};
    use memstead_base::pipeline_store::write_binding;
    use memstead_base::workspace::{
        Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
    };
    use memstead_base::workspace_store::WorkspaceStoreAdapter;

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
            source: None,
            span_unvalidated: false,
            hash_source: None,
            last_observed: None,
        }
    }

    /// Scaffold a filesystem-medium mem whose anchors reference **absent** source
    /// files (so every anchor resolves orphaned), with prune enabled. Returns
    /// the engine, workspace root, binding and resolved run.
    fn setup(
        tmp: &Path,
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
        memstead_base::FileWorkspaceStore::new()
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
            // The entity each row is keyed to. Written, because it exists: a
            // row whose entity does not is DANGLING and is partitioned out of
            // the population before prune sees it (consistency-sweep 03/02),
            // which is exactly the phantom-entity proposal prune bans.
            // A `!` prefix on the id means "seed the row but NOT the entity",
            // which is the phantom-entity condition the ban is about.
            let (write_entity, eid) = match eid.strip_prefix('!') {
                Some(rest) => (false, rest),
                None => (true, *eid),
            };
            if write_entity {
                let slug = eid.split_once("--").map_or(eid, |(_, s)| s);
                std::fs::write(
                    mem_dir.join(format!("{slug}.md")),
                    "---\ntype: decision\n---\n\n# E\n\n## Decision\n\nBody.\n",
                )
                .unwrap();
            }
            sidecar.set(eid, anchors.clone());
        }
        std::fs::write(
            mem_dir.join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
            sidecar.to_bytes(),
        )
        .unwrap();

        // A filesystem-source binding (namespace `path`, so mem_anchors_resolved
        // observes it) with prune enabled.
        let binding = Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![memstead_base::pipeline::Source {
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
            coverage_semantics: None,
            rules: None,
            prune: Some(PruneConfig::default()),
            operations: Operations {
                build: Some(BuildOperation {
                    mode: BuildMode::Discovery,
                    trigger: IngestTrigger::Loop,
                    batch_size: 20,
                    post_actions: None,
                }),
                sync: Some(memstead_base::binding::SyncOperation {
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
        let resolved = resolve_binding_run("engine/graph", &binding).unwrap();
        (engine, root, binding, resolved)
    }

    /// Criterion 6 (consistency-sweep 03/02): prune must not propose deleting
    /// an entity that is already gone. Its anchor is orphaned, which is
    /// precisely the shape that made a phantom entity a candidate: prune
    /// walked the sidecar by key and never asked whether the key still names
    /// anything.
    #[test]
    fn prune_never_proposes_an_entity_that_does_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, root, binding, resolved) = setup(
            tmp.path(),
            &[
                (
                    "!engine--phantom",
                    vec![orphan_anchor(
                        "src/phantom.rs",
                        AnchorProvenanceClass::Anchored,
                        vec![],
                        None,
                    )],
                ),
                (
                    "engine--real",
                    vec![orphan_anchor(
                        "src/real.rs",
                        AnchorProvenanceClass::Anchored,
                        vec![],
                        None,
                    )],
                ),
            ],
        );

        let proposals = prune_proposals(&engine, &root, &binding, &resolved);
        assert_eq!(
            proposals
                .iter()
                .map(|p| p.entity.as_str())
                .collect::<Vec<_>>(),
            vec!["engine--real"],
            "the entity that exists is still a candidate; the phantom is not proposed"
        );
    }

    /// An entity whose source artifact was removed surfaces BOTH sides in the
    /// sync brief as a proposal and is NEVER auto-deleted.
    #[test]
    fn proposal_presents_both_sides_and_never_deletes() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, root, binding, resolved) = setup(
            tmp.path(),
            &[(
                "engine--removed",
                vec![orphan_anchor(
                    "src/removed.rs",
                    AnchorProvenanceClass::Anchored,
                    vec![],
                    None,
                )],
            )],
        );

        let proposals = prune_proposals(&engine, &root, &binding, &resolved);
        assert_eq!(proposals.len(), 1, "the orphaned entity is a candidate");
        let p = &proposals[0];
        assert_eq!(p.entity, "engine--removed");
        assert_eq!(
            p.disposition,
            PruneDisposition::Proposed,
            "an orphaned anchored entity is proposed, never auto-deleted"
        );

        // The rendered sync brief presents BOTH sides and frames it as a proposal.
        let brief = render_sync_brief_for(&engine, &root, "engine/graph").unwrap();
        assert!(brief.contains("Prune — proposed removals"));
        assert!(brief.contains("source side:"), "source side surfaced");
        assert!(brief.contains("model side:"), "model side surfaced");
        assert!(
            brief.contains("the call is yours"),
            "the brief leaves the decision to the agent"
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

        // The plain anchored entity IS proposed.
        let plain = proposals
            .iter()
            .find(|p| p.entity == "engine--plain")
            .expect("a plain anchored entity is proposed");
        assert_eq!(plain.disposition, PruneDisposition::Proposed);

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

    /// A git-pinned anchor is proposed exactly like an unpinned one: the pin
    /// names a source version, not a merge base the engine could decide from.
    #[test]
    fn git_pinned_anchor_is_proposed_like_any_other() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, root, binding, resolved) = setup(
            tmp.path(),
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
        assert_eq!(proposals[0].disposition, PruneDisposition::Proposed);
    }

    /// An entity with a **still-resolving** anchor is NOT a prune candidate —
    /// the whole basis must be gone (conservatism). Here one anchor's file
    /// exists, so the entity is skipped.
    #[test]
    fn entity_with_a_surviving_anchor_is_not_pruned() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, root, binding, resolved) = setup(
            tmp.path(),
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
