//! The population the anchor axis answers for.
//!
//! WHY: the mem-wide anchor query ([`crate::Engine::mem_anchors_resolved`])
//! scopes by mem and nothing else, so a binding's fidelity report scored every
//! anchor in its destination mem, including anchors another binding wrote and
//! anchors pointing at artifacts its own scope excludes. Narrowing a binding's
//! scope until it matched nothing left its anchors still scoring. Three
//! separately filed findings were this one defect.
//!
//! THE POSTURE (consistency-sweep bundle 03, copied from its README): the axis
//! answers for exactly one population, the anchors the binding under
//! verification is responsible for. Every anchor outside it is excluded AND
//! NAMED; nothing is silently included and nothing silently dropped.
//!
//! Two signals decide membership, in this order.
//!
//! **Provenance.** An anchor recording a producing binding belongs to that
//! binding and to no other. This is the primary signal and it is exact.
//!
//! **Scope.** An anchor pointing at an artifact the binding's own scope
//! excludes is not this binding's responsibility even when it wrote it: the
//! binding no longer covers that artifact, so reporting fidelity over it
//! attributes drift to a binding that has disclaimed the file.
//!
//! **The pre-provenance fallback, and why it is inclusion rather than
//! exclusion.** The `binding` field is optional and absent on every anchor
//! written before it existed. Filtering strictly on it would empty the axis for
//! every mem already on disk, turning a clean report into an empty one on
//! upgrade. So an anchor with no recorded binding is INCLUDED, and the report
//! states that it did so. That is the conservative direction: a figure that
//! over-reports is visible and correctable, a figure that silently becomes
//! empty reads as success.

use std::collections::BTreeSet;
use std::path::Path;

use crate::Engine;
use crate::anchor::AnchorGrain;
use crate::engine::query::ResolvedAnchor;
use crate::entity::EntityId;
use crate::ingest::cursor::enumerate_source_artifacts;
use crate::ingest::resolve::{ResolvedIngest, ResolvedSource};

/// Why an anchor is not in the population. Every exclusion carries one, and
/// the report names them, because a number a reader cannot act on reproduces
/// the original defect one level up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExclusionReason {
    /// Written by a different binding, which reports on it instead.
    OtherBinding,
    /// Points at an artifact this binding's scope does not cover.
    OutOfScope,
}

impl ExclusionReason {
    pub fn as_wire(self) -> &'static str {
        match self {
            ExclusionReason::OtherBinding => "other-binding",
            ExclusionReason::OutOfScope => "out-of-scope",
        }
    }
}

/// One excluded anchor, named rather than merely counted.
#[derive(Debug, Clone)]
pub struct ExcludedAnchor {
    pub entity: EntityId,
    pub artifact: String,
    pub reason: ExclusionReason,
}

/// The partition of a mem's anchors for one binding.
#[derive(Debug, Clone, Default)]
pub struct AnchorPopulation {
    /// The anchors this binding answers for.
    pub included: Vec<(EntityId, ResolvedAnchor)>,
    /// The rest, each with the reason it is out, in a stable order.
    pub excluded: Vec<ExcludedAnchor>,
    /// How many included anchors carried no producing binding and were kept by
    /// the fallback. Reported, never inferred: a reader must be able to tell a
    /// population established by provenance from one resting on the fallback.
    pub without_provenance: usize,
}

impl AnchorPopulation {
    /// Distinct artifacts among the included anchors. The row count and this
    /// differ whenever one artifact carries several legitimate rows at
    /// different grains or classes, and a reader reads the figure as being
    /// about artifacts, so both are reported.
    pub fn distinct_artifacts(&self) -> usize {
        self.included
            .iter()
            .map(|(_, r)| r.anchor.artifact.as_str())
            .collect::<BTreeSet<_>>()
            .len()
    }

    pub fn excluded_count(&self, reason: ExclusionReason) -> usize {
        self.excluded.iter().filter(|e| e.reason == reason).count()
    }
}

/// Partition `mem`'s anchors into the population `resolved` answers for and
/// the rest.
///
/// `binding_hash` is `hash(D)` of the binding under verification, the same
/// value the findings store keys on. Pass `None` where the caller genuinely
/// has no binding identity, which keeps every anchor and excludes only by
/// scope.
pub fn population_for(
    engine: &Engine,
    resolved: &ResolvedIngest,
    binding_hash: Option<&str>,
    workspace_root: &Path,
) -> AnchorPopulation {
    let in_scope = scope_artifacts(engine, resolved, workspace_root);
    let mut out = AnchorPopulation::default();

    for (eid, resolved_anchor) in engine.mem_anchors_resolved(&resolved.destination_mem) {
        let anchor = &resolved_anchor.anchor;

        // Provenance first, because it is exact where it is present.
        match (anchor.binding.as_deref(), binding_hash) {
            (Some(theirs), Some(ours)) if theirs != ours => {
                out.excluded.push(ExcludedAnchor {
                    entity: eid,
                    artifact: anchor.artifact.clone(),
                    reason: ExclusionReason::OtherBinding,
                });
                continue;
            }
            (None, _) => out.without_provenance += 1,
            _ => {}
        }

        // Scope second. An `entity`-grain anchor is judged by the graph
        // selector rather than by a path glob, and the enumerator already
        // returns entity ids for a graph source, so one membership test
        // serves both. An empty enumeration means the binding declares no
        // artifacts this pass; excluding on it would empty the axis for a
        // reason unrelated to the anchor, so it is treated as no opinion.
        if let Some(known) = &in_scope
            && !known.contains(anchor.artifact.as_str())
            && !covers_prefix(known, anchor)
        {
            out.excluded.push(ExcludedAnchor {
                entity: eid,
                artifact: anchor.artifact.clone(),
                reason: ExclusionReason::OutOfScope,
            });
            continue;
        }

        out.included.push((eid, resolved_anchor));
    }

    out.excluded.sort_by(|a, b| {
        (a.reason, &a.artifact, &a.entity.0).cmp(&(b.reason, &b.artifact, &b.entity.0))
    });
    out
}

/// A `tree`-grain anchor names a directory, and the enumeration lists the
/// files under it. Membership for such an anchor is "the scope contains
/// something beneath this artifact", not equality.
fn covers_prefix(known: &BTreeSet<String>, anchor: &crate::anchor::Anchor) -> bool {
    if anchor.grain != AnchorGrain::Tree {
        return false;
    }
    let prefix = format!("{}/", anchor.artifact.trim_end_matches('/'));
    known.iter().any(|k| k.starts_with(&prefix))
}

/// Every artifact this binding's sources currently enumerate, or `None` when
/// the binding enumerates nothing at all (see the call site for why that is
/// treated as no opinion rather than as an empty scope).
fn scope_artifacts(
    engine: &Engine,
    resolved: &ResolvedIngest,
    workspace_root: &Path,
) -> Option<BTreeSet<String>> {
    let mut known = BTreeSet::new();
    for source in &resolved.sources {
        if let ResolvedSource::Primary(p) = source {
            for artifact in
                enumerate_source_artifacts(engine, p, &resolved.deny_paths, workspace_root)
            {
                known.insert(artifact);
            }
        }
    }
    if known.is_empty() { None } else { Some(known) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use crate::anchor::{Anchor, AnchorHashStability, AnchorProvenanceClass, AnchorSidecar};
    use crate::binding::BuildMode;
    use crate::binding::{
        BINDING_VERSION, Binding, BuildOperation, DEFAULT_ADJUDICATION_CAP,
        DEFAULT_FULL_RESYNC_EVERY, Operations, VerifyOperation,
    };
    use crate::ingest::resolve::resolve_binding_run;
    use crate::pipeline::{IngestTrigger, MediumType};
    use crate::pipeline::{PatternEntry, PatternMode, Source};
    use crate::workspace::{
        Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
    };
    use crate::workspace_store::WorkspaceStoreAdapter;

    fn anchor(artifact: &str, binding: Option<&str>, grain: AnchorGrain) -> Anchor {
        Anchor {
            artifact: artifact.to_string(),
            grain,
            class: AnchorProvenanceClass::Anchored,
            at_version: None,
            hash: None,
            hash_stability: AnchorHashStability::Stable,
            derived_from: vec![],
            binding: binding.map(str::to_string),
            source: Some("src".to_string()),
        }
    }

    /// One mem, real files so the enumerator has a scope, and a seeded anchor
    /// sidecar. Mirrors the harness `prune.rs` proved.
    fn fixture(
        tmp: &Path,
        scope: &str,
        files: &[&str],
        anchors: Vec<Anchor>,
    ) -> (Engine, ResolvedIngest) {
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
        for f in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "x\n").unwrap();
        }
        crate::FileWorkspaceStore::new()
            .save_state(
                &root,
                &Workspace {
                    mounts: vec![Mount {
                        mem: "engine".to_string(),
                        schema: Some("default@1.0.0".parse().unwrap()),
                        storage: MountStorage::Folder {
                            path: mem_dir.clone(),
                        },
                        capability: MountCapability::Write,
                        lifecycle: MountLifecycle::Eager,
                        cross_linkable: false,
                        migration_target: None,
                    }],
                    settings: WorkspaceSettings::default(),
                },
            )
            .unwrap();

        let mut sidecar = AnchorSidecar::default();
        sidecar.set("engine--e", anchors);
        std::fs::write(
            mem_dir.join(crate::anchor::ANCHOR_SIDECAR_PATH),
            sidecar.to_bytes(),
        )
        .unwrap();

        let binding = Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![Source {
                name: "src".to_string(),
                medium_type: MediumType::Filesystem,
                pointer: String::new(),
                change_detection: None,
                scope: vec![PatternEntry {
                    path: scope.to_string(),
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
            prune: None,
            operations: Operations {
                build: Some(BuildOperation {
                    mode: BuildMode::Discovery,
                    trigger: IngestTrigger::Loop,
                    batch_size: 20,
                    post_actions: None,
                }),
                sync: None,
                verify: Some(VerifyOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 20,
                    adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                    full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
                }),
            },
        };
        let engine = Engine::from_workspace_root(&root).unwrap();
        let resolved = resolve_binding_run("engine/src", &binding).unwrap();
        (engine, resolved)
    }

    /// Criterion 1 and 3: two bindings on one mem see different populations,
    /// and each names the other's anchor rather than dropping it.
    #[test]
    fn each_binding_answers_for_its_own_anchors() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture(
            tmp.path(),
            "src/**/*.rs",
            &["src/a.rs", "src/b.rs"],
            vec![
                anchor("src/a.rs", Some("hash-A"), AnchorGrain::File),
                anchor("src/b.rs", Some("hash-B"), AnchorGrain::File),
            ],
        );
        let a = population_for(&engine, &r, Some("hash-A"), tmp.path());
        let b = population_for(&engine, &r, Some("hash-B"), tmp.path());
        assert_eq!(a.included.len(), 1);
        assert_eq!(b.included.len(), 1);
        assert_ne!(
            a.included[0].1.anchor.artifact, b.included[0].1.anchor.artifact,
            "the two populations differ"
        );
        assert_eq!(a.excluded.len(), 1);
        assert_eq!(a.excluded[0].reason, ExclusionReason::OtherBinding);
        assert_eq!(a.excluded[0].artifact, "src/b.rs");
    }

    /// Criteria 2 and 3: an out-of-scope anchor leaves the figures and appears
    /// by name.
    #[test]
    fn an_out_of_scope_anchor_is_excluded_and_named() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture(
            tmp.path(),
            "src/**/*.rs",
            &["src/a.rs", "docs/b.md"],
            vec![
                anchor("src/a.rs", Some("h"), AnchorGrain::File),
                anchor("docs/b.md", Some("h"), AnchorGrain::File),
            ],
        );
        let pop = population_for(&engine, &r, Some("h"), tmp.path());
        assert_eq!(pop.included.len(), 1);
        assert_eq!(pop.included[0].1.anchor.artifact, "src/a.rs");
        assert_eq!(pop.excluded.len(), 1, "named, not dropped");
        assert_eq!(pop.excluded[0].reason, ExclusionReason::OutOfScope);
    }

    /// Criterion 4: exclusion is a reporting decision, never a mutation.
    #[test]
    fn exclusion_never_mutates_the_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture(
            tmp.path(),
            "src/**/*.rs",
            &["src/a.rs"],
            vec![anchor("docs/b.md", Some("h"), AnchorGrain::File)],
        );
        let before = engine.mem_anchors_resolved("engine").len();
        let _ = population_for(&engine, &r, Some("h"), tmp.path());
        assert_eq!(engine.mem_anchors_resolved("engine").len(), before);
    }

    /// Criteria 5 and 6: several rows on one artifact stay several rows, and
    /// the distinct-artifact count sits beside the row count.
    #[test]
    fn the_distinct_artifact_count_sits_beside_the_row_count() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture(
            tmp.path(),
            "src/**/*.rs",
            &["src/a.rs"],
            vec![
                anchor("src/a.rs", Some("h"), AnchorGrain::File),
                anchor("src/a.rs", Some("h"), AnchorGrain::Span),
            ],
        );
        let pop = population_for(&engine, &r, Some("h"), tmp.path());
        assert_eq!(pop.included.len(), 2, "two rows, not merged");
        assert_eq!(pop.distinct_artifacts(), 1, "one artifact");
    }

    /// Criterion 8: an upgrade must not empty the axis, and the fallback is
    /// counted rather than left to be inferred.
    #[test]
    fn anchors_without_provenance_are_kept_and_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture(
            tmp.path(),
            "src/**/*.rs",
            &["src/a.rs"],
            vec![anchor("src/a.rs", None, AnchorGrain::File)],
        );
        let pop = population_for(&engine, &r, Some("h"), tmp.path());
        assert_eq!(pop.included.len(), 1);
        assert_eq!(pop.without_provenance, 1);
    }

    /// A binding enumerating nothing has no opinion on scope rather than an
    /// empty one, so a momentarily-unmatched source cannot empty the axis for
    /// a reason unrelated to the anchors.
    #[test]
    fn an_empty_enumeration_is_no_opinion() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture(
            tmp.path(),
            "nothing/**/*.rs",
            &[],
            vec![anchor("src/a.rs", Some("h"), AnchorGrain::File)],
        );
        let pop = population_for(&engine, &r, Some("h"), tmp.path());
        assert_eq!(pop.included.len(), 1);
        assert!(pop.excluded.is_empty());
    }
}
