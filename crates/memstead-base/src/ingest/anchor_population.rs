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
//! **The entity end comes first (consistency-sweep 03/02).** An anchor is an
//! edge with two ends and the engine checked one of them: a mem whose entity
//! files are gone, but whose sidecar still names them, verified clean at a
//! hundred percent, because every row resolved against a source that was fine.
//! A row whose entity the mem no longer holds is partitioned out as DANGLING
//! before either membership test below. It is not a fifth anchor state (the
//! four describe the artifact end, and a vanished entity says nothing about
//! the source) and not an exclusion (those are legal authoring this binding
//! does not answer for). It is a sidecar integrity condition, reported and
//! never repaired: the engine's own delete and rename paths keep the sidecar
//! in step, so the row is the trace of a writer that went around the engine,
//! and deleting it would erase the evidence.
//!
//! That test reads a NEGATIVE from the store, which is only evidence when the
//! store holds everything the mem has. Where it does not
//! ([`crate::Engine::entity_set_is_reconcilable`]), no row is called dangling
//! and the population says why instead.
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

use crate::Engine;
use crate::anchor::AnchorGrain;
use crate::engine::query::ResolvedAnchor;
use crate::entity::EntityId;
use crate::ingest::cursor::build_glob_set;
use crate::ingest::resolve::{ResolvedIngest, ResolvedSource};
use crate::pipeline::PatternMode;
use globset::GlobSet;

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

/// One sidecar row whose ENTITY is gone (consistency-sweep 03/02).
///
/// Not an exclusion and not an anchor state. The four states describe the
/// artifact end of the edge, and a vanished entity says nothing about the
/// source; an out-of-scope exclusion is legal authoring, and this is not. It
/// is a sidecar integrity condition: the engine's own delete and rename paths
/// keep the sidecar in step, so a dangling row is the trace of a writer that
/// went around the engine.
#[derive(Debug, Clone)]
pub struct DanglingAnchor {
    /// The id the sidecar is keyed by, which the mem no longer holds.
    pub entity: EntityId,
    pub artifact: String,
}

/// The partition of a mem's anchors for one binding.
#[derive(Debug, Clone, Default)]
pub struct AnchorPopulation {
    /// The anchors this binding answers for.
    pub included: Vec<(EntityId, ResolvedAnchor)>,
    /// The rest, each with the reason it is out, in a stable order.
    pub excluded: Vec<ExcludedAnchor>,
    /// Rows whose entity is gone, named rather than counted. In no binding's
    /// population: the condition belongs to the mem's sidecar, so every
    /// binding on the mem reports the same rows.
    pub dangling: Vec<DanglingAnchor>,
    /// How many included anchors carried no producing binding and were kept by
    /// the fallback. Reported, never inferred: a reader must be able to tell a
    /// population established by provenance from one resting on the fallback.
    pub without_provenance: usize,
    /// Why the entity end could NOT be reconciled this pass, when it could
    /// not. `None` means it was. A surface that reads an empty `dangling`
    /// without reading this would report a clean axis over state it did not
    /// examine.
    pub unreconciled: Option<&'static str>,
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
///
/// Takes no workspace root: scope is the binding's DECLARED patterns, so the
/// partition needs no filesystem access and cannot be perturbed by what
/// happens to exist when it runs.
pub fn population_for(
    engine: &Engine,
    resolved: &ResolvedIngest,
    binding_hash: Option<&str>,
) -> AnchorPopulation {
    let scope = scope_matcher(resolved);
    // The entity end, before either membership test. A dangling row belongs
    // to no binding's population, so asking whose it is first would let a
    // scope or provenance exclusion hide the integrity condition behind a
    // bucket that reads as legal.
    let mut out = AnchorPopulation {
        unreconciled: engine
            .entity_set_is_reconcilable(&resolved.destination_mem)
            .err(),
        ..Default::default()
    };

    for (eid, resolved_anchor) in engine.mem_anchors_resolved(&resolved.destination_mem) {
        let anchor = &resolved_anchor.anchor;

        if out.unreconciled.is_none() && engine.entity_is_absent(&eid) {
            out.dangling.push(DanglingAnchor {
                entity: eid,
                artifact: anchor.artifact.clone(),
            });
            continue;
        }

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
            // NOT counted here: the scope test below can still exclude this
            // anchor, and a count taken at this point would report an anchor
            // as kept by the fallback while the population excluded it. A
            // grade reproduced exactly that: included 0, without_provenance 1.
            _ => {}
        }

        // Scope second. An `entity`-grain anchor is judged by the graph
        // selector rather than by a path glob, and the enumerator already
        // returns entity ids for a graph source, so one membership test
        // serves both. An empty enumeration means the binding declares no
        // artifacts this pass; excluding on it would empty the axis for a
        // reason unrelated to the anchor, so it is treated as no opinion.
        if let Some(matcher) = &scope
            && !in_declared_scope(matcher, anchor)
        {
            out.excluded.push(ExcludedAnchor {
                entity: eid,
                artifact: anchor.artifact.clone(),
                reason: ExclusionReason::OutOfScope,
            });
            continue;
        }

        if anchor.binding.is_none() {
            out.without_provenance += 1;
        }
        out.included.push((eid, resolved_anchor));
    }

    out.excluded.sort_by(|a, b| {
        (a.reason, &a.artifact, &a.entity.0).cmp(&(b.reason, &b.artifact, &b.entity.0))
    });
    out.dangling
        .sort_by(|a, b| (&a.entity.0, &a.artifact).cmp(&(&b.entity.0, &b.artifact)));
    out
}

/// The binding's DECLARED scope as a matcher, or `None` when it declares no
/// allow patterns at all (an unscoped facet has no opinion).
///
/// Declared, not enumerated, and the distinction is the whole point. An
/// earlier version asked the enumerator which artifacts exist right now, which
/// silently excluded every anchor whose file had been DELETED. An orphaned
/// anchor is the single highest-signal finding the axis produces, and that
/// version made it vanish instead: a test that expected orphan plus drifted
/// caught it. A deleted file is still in scope; its absence is the finding.
fn scope_matcher(resolved: &ResolvedIngest) -> Option<ScopeMatcher> {
    // ONE matcher per primary source, each in ITS OWN namespace. A source's
    // scope patterns are source-relative (they join onto its `pointer`), so
    // pooling every source's raw patterns into one set — which is what this
    // did until 2026-08-27 — answers membership in a namespace no source
    // speaks. The observable cost was real: after the dogfood bindings were
    // migrated to the source-relative dialect, `project/graph`'s five anchors
    // moved from in-population to `excluded_out_of_scope` with no report, and
    // the same run's coverage denominator still counted the files those
    // anchors name. Two readings of one scope inside one report.
    let mut per_source: Vec<SourceScope> = Vec::new();
    for source in &resolved.sources {
        if let ResolvedSource::Primary(p) = source {
            let mut allows: Vec<&str> = Vec::new();
            let mut denies: Vec<&str> = Vec::new();
            for rule in &p.scope {
                match rule.mode {
                    PatternMode::Allow => allows.push(rule.path.as_str()),
                    PatternMode::Deny => denies.push(rule.path.as_str()),
                }
            }
            // No allow patterns at all is an UNSCOPED facet, which has no
            // opinion — it contributes no matcher rather than an empty allow
            // set, which would exclude every anchor (the "silently drops" half
            // of the posture rather than the "excludes and names" half).
            if allows.is_empty() {
                continue;
            }
            let Some(allow_set) = build_glob_set(&allows) else {
                continue;
            };
            per_source.push(SourceScope {
                pointer: p.pointer.trim_end_matches('/').to_string(),
                allow: allow_set,
                deny: if denies.is_empty() {
                    None
                } else {
                    build_glob_set(&denies)
                },
                allow_heads: allows.iter().map(|p| literal_head(p)).collect(),
            });
        }
    }
    if per_source.is_empty() {
        return None;
    }
    // Binding-level denies stay in the WORKSPACE namespace and go through the
    // resolver that enforces them, so a path hidden from the ingest agent is
    // also outside the population.
    let ws_deny = super::check_path::DenyOracle::new(&resolved.deny_paths);
    Some(ScopeMatcher {
        per_source,
        ws_deny,
    })
}

/// The declared scope, as the two glob sets plus the literal head of each
/// allow pattern (see [`ScopeMatcher::covers_tree`]).
struct ScopeMatcher {
    per_source: Vec<SourceScope>,
    ws_deny: super::check_path::DenyOracle,
}

/// One primary source's declared scope, in that source's own namespace.
struct SourceScope {
    /// The source's medium pointer, trailing `/` trimmed. Empty for a
    /// pointer-less source, where the two readings coincide.
    pointer: String,
    allow: GlobSet,
    deny: Option<GlobSet>,
    allow_heads: Vec<String>,
}

impl SourceScope {
    /// The forms of `artifact` this source could be speaking about, in ITS
    /// namespace: the path as written (already source-relative), and — when
    /// the path is workspace-relative and lies under this source's pointer —
    /// the same path with the pointer prefix stripped. This is the candidate
    /// ordering the ratified anchor-artifact decision already uses at anchor
    /// resolution; membership must ask the same question resolution does, or
    /// an anchor resolves under one reading and is excluded under another.
    fn candidates(&self, artifact: &str) -> Vec<String> {
        if self.pointer.is_empty() {
            // The two readings coincide; the path as written is the answer.
            return vec![artifact.to_string()];
        }
        let prefix = format!("{}/", self.pointer);
        if let Some(rest) = artifact.strip_prefix(&prefix)
            && !rest.is_empty()
        {
            // Workspace-relative and under this pointer: the stripped form IS
            // the source-relative one, and it alone decides. Keeping the
            // as-written form as a second chance would union two populations —
            // a source-relative `**/*.md` matches any relative path, so an
            // artifact in a sibling tree would slip in.
            return vec![rest.to_string()];
        }
        // Not under the pointer. It may already be source-relative (the form
        // the ratified anchor decision resolves first), which never escapes
        // its own tree — so a path that climbs out with `..` belongs to some
        // other source, not this one.
        if artifact.starts_with("../") || artifact == ".." {
            return Vec::new();
        }
        vec![artifact.to_string()]
    }

    /// Whether this source's declared scope admits `artifact`.
    fn admits(&self, artifact: &str, grain: AnchorGrain) -> bool {
        self.candidates(artifact).iter().any(|c| {
            let path = c.trim_end_matches('/');
            if self.deny.as_ref().is_some_and(|d| d.is_match(path)) {
                return false;
            }
            self.allow.is_match(path) || (grain == AnchorGrain::Tree && self.covers_tree_for(path))
        })
    }

    fn covers_tree_for(&self, dir: &str) -> bool {
        let dir = format!("{}/", dir.trim_end_matches('/'));
        self.allow_heads
            .iter()
            .any(|h| h.starts_with(&dir) || dir.starts_with(h.as_str()))
    }
}

/// A glob pattern's leading literal path, up to the first metacharacter.
fn literal_head(pattern: &str) -> String {
    let cut = pattern.find(['*', '?', '[', '{']).unwrap_or(pattern.len());
    let head = &pattern[..cut];
    match head.rfind('/') {
        Some(i) => head[..=i].to_string(),
        None => String::new(),
    }
}

/// Is this artifact inside the binding's declared scope?
///
/// A `tree`-grain anchor names a directory, which no file glob matches, so it
/// is in scope when the scope could contain something beneath it. An
/// `entity`-grain anchor is a graph id rather than a path and file globs
/// cannot judge it, so it is never excluded on path grounds.
fn in_declared_scope(matcher: &ScopeMatcher, anchor: &crate::anchor::Anchor) -> bool {
    if anchor.grain == AnchorGrain::Entity {
        return true;
    }
    let path = anchor.artifact.trim_end_matches('/');
    // A binding-level deny is workspace-namespaced and hides the path from the
    // ingest agent, so it is outside the population whatever any source says.
    if matcher.ws_deny.is_denied(path) {
        return false;
    }
    // In scope when ANY primary source's own scope admits it. A binding with
    // several sources declares a union, and an anchor belongs to the source
    // whose namespace it reads in.
    matcher
        .per_source
        .iter()
        .any(|s| s.admits(&anchor.artifact, anchor.grain))
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
            span_unvalidated: false,
            hash_source: None,
        }
    }

    /// One mem, real files so the enumerator has a scope, and a seeded anchor
    /// sidecar. Mirrors the harness `prune.rs` proved.
    fn fixture(
        tmp: &std::path::Path,
        scope: &str,
        files: &[&str],
        anchors: Vec<Anchor>,
    ) -> (Engine, ResolvedIngest) {
        fixture_with(tmp, scope, files, anchors, true, MountLifecycle::Eager)
    }

    /// `write_entity: false` builds the condition 03/02 is about: a sidecar
    /// keyed to an entity the mem does not hold. `lifecycle` builds the other
    /// one, a mem whose entities are not in the store at all, where a missing
    /// id proves nothing.
    fn fixture_with(
        tmp: &std::path::Path,
        scope: &str,
        files: &[&str],
        anchors: Vec<Anchor>,
        write_entity: bool,
        lifecycle: MountLifecycle,
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
                        lifecycle,
                        cross_linkable: false,
                        migration_target: None,
                    }],
                    settings: WorkspaceSettings::default(),
                },
            )
            .unwrap();

        // The entity the sidecar is keyed to. Written, because it exists: a
        // sidecar row whose entity does not is a DANGLING row, which the
        // population partitions out on its own (consistency-sweep 03/02).
        // Every fixture here omitted it, so every one of them was quietly a
        // mem whose sidecar had outlived its entities.
        if write_entity {
            std::fs::write(
                mem_dir.join("e.md"),
                "---\ntype: decision\n---\n\n# E\n\n## Decision\n\nBody.\n",
            )
            .unwrap();
        }

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

    /// Membership must ask each source in ITS namespace. A pointer-bearing
    /// source whose scope is source-relative (the dialect since 2026-08-27)
    /// still holds anchors written workspace-relative, which is the fallback
    /// form the ratified anchor decision resolves through. Pooling raw
    /// patterns answered in a namespace no source speaks, so migrating a
    /// binding's scope silently moved its anchors to `excluded_out_of_scope`
    /// while the same report's denominator still counted the files they name.
    #[test]
    fn membership_joins_each_source_scope_onto_its_pointer() {
        let scope_source = |pointer: &str, pattern: &str| Source {
            name: "src".to_string(),
            medium_type: MediumType::Filesystem,
            pointer: pointer.to_string(),
            change_detection: None,
            scope: vec![PatternEntry {
                path: pattern.to_string(),
                mode: PatternMode::Allow,
            }],
            engagement: None,
            preparation: None,
        };
        let resolved_with = |source: Source| {
            let binding = Binding {
                version: BINDING_VERSION,
                intent: None,
                sources: vec![source],
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
            resolve_binding_run("engine/src", &binding).unwrap()
        };

        // The shape the dogfood bindings actually have: pointer `../dev`,
        // scope source-relative, anchor stored workspace-relative.
        let r = resolved_with(scope_source("../dev", "**/*.md"));
        let m = scope_matcher(&r).expect("a scoped source yields a matcher");
        assert!(
            in_declared_scope(
                &m,
                &anchor("../dev/institute/CHARTER.md", None, AnchorGrain::File)
            ),
            "an anchor under the pointer is in scope after the join"
        );
        assert!(
            !in_declared_scope(&m, &anchor("../other/README.md", None, AnchorGrain::File)),
            "a path outside the pointer stays out"
        );

        // A pointer-less source is untouched: the two readings coincide.
        let r0 = resolved_with(scope_source("", "dev/**/*.md"));
        let m0 = scope_matcher(&r0).unwrap();
        assert!(in_declared_scope(
            &m0,
            &anchor("dev/notes.md", None, AnchorGrain::File)
        ));
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
        let a = population_for(&engine, &r, Some("hash-A"));
        let b = population_for(&engine, &r, Some("hash-B"));
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
        let pop = population_for(&engine, &r, Some("h"));
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
        let _ = population_for(&engine, &r, Some("h"));
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
        let pop = population_for(&engine, &r, Some("h"));
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
        let pop = population_for(&engine, &r, Some("h"));
        assert_eq!(pop.included.len(), 1);
        assert_eq!(pop.without_provenance, 1);
    }

    /// Scope is the DECLARED patterns, not what happens to exist. An anchor
    /// whose file was deleted stays in the population, because its absence is
    /// the orphan finding the axis exists to raise. An earlier version asked
    /// the enumerator instead and made every orphan vanish.
    #[test]
    fn a_deleted_artifact_stays_in_scope_so_its_orphaning_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture(
            tmp.path(),
            "src/**/*.rs",
            &["src/present.rs"],
            vec![
                anchor("src/present.rs", Some("h"), AnchorGrain::File),
                anchor("src/gone.rs", Some("h"), AnchorGrain::File),
            ],
        );
        let pop = population_for(&engine, &r, Some("h"));
        assert_eq!(pop.included.len(), 2, "the deleted file is still in scope");
        assert!(pop.excluded.is_empty());
    }

    /// A `tree`-grain anchor names a directory, which no file glob matches, so
    /// it is judged by whether the scope could contain something beneath it.
    #[test]
    fn a_tree_anchor_is_in_scope_when_the_scope_reaches_under_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture(
            tmp.path(),
            "src/**/*.rs",
            &["src/sub/a.rs"],
            vec![
                anchor("src/sub/", Some("h"), AnchorGrain::Tree),
                anchor("docs/", Some("h"), AnchorGrain::Tree),
            ],
        );
        let pop = population_for(&engine, &r, Some("h"));
        assert_eq!(pop.included.len(), 1);
        assert_eq!(pop.included[0].1.anchor.artifact, "src/sub/");
        assert_eq!(pop.excluded.len(), 1);
        assert_eq!(pop.excluded[0].artifact, "docs/");
    }

    /// Criterion 7's asymmetry, at the population level: an anchor another
    /// binding wrote is not this binding's evidence of coverage either. The
    /// report's coverage filter carries the same rule; this pins the shared
    /// premise, that provenance decides membership on both axes rather than
    /// only on the resolution one.
    #[test]
    fn another_bindings_anchor_is_not_this_bindings_evidence() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture(
            tmp.path(),
            "src/**/*.rs",
            &["src/a.rs"],
            vec![anchor("src/a.rs", Some("theirs"), AnchorGrain::File)],
        );
        let pop = population_for(&engine, &r, Some("ours"));
        assert!(
            pop.included.is_empty(),
            "the covering anchor belongs to the other binding"
        );
        assert_eq!(pop.excluded.len(), 1);
        assert_eq!(pop.excluded[0].reason, ExclusionReason::OtherBinding);
    }

    /// Criteria 1, 2 and 3 (03/02). The row's ENTITY is gone. It is neither
    /// in the population nor in an exclusion bucket: the exclusions are legal
    /// authoring the binding does not answer for, and this is a sidecar the
    /// mem's entities have outlived.
    #[test]
    fn a_row_whose_entity_is_gone_is_dangling_and_is_its_own_class() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture_with(
            tmp.path(),
            "src/**/*.rs",
            &["src/a.rs"],
            vec![anchor("src/a.rs", Some("h"), AnchorGrain::File)],
            false,
            MountLifecycle::Eager,
        );
        let pop = population_for(&engine, &r, Some("h"));
        assert_eq!(pop.dangling.len(), 1, "the row is reported");
        assert_eq!(pop.dangling[0].entity.as_ref(), "engine--e");
        assert_eq!(pop.dangling[0].artifact, "src/a.rs");
        assert!(pop.included.is_empty(), "and raises no figure");
        assert!(
            pop.excluded.is_empty(),
            "an exclusion bucket would name it legal, which it is not"
        );
        assert_eq!(pop.unreconciled, None);
    }

    /// Criterion 4: detection reads. A reconciliation that tidied the sidecar
    /// would erase the only evidence that a writer went around the engine.
    #[test]
    fn detecting_a_dangling_row_never_touches_the_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture_with(
            tmp.path(),
            "src/**/*.rs",
            &["src/a.rs"],
            vec![anchor("src/a.rs", Some("h"), AnchorGrain::File)],
            false,
            MountLifecycle::Eager,
        );
        let path = tmp
            .path()
            .join("mem")
            .join(crate::anchor::ANCHOR_SIDECAR_PATH);
        let before = std::fs::read(&path).unwrap();
        let pop = population_for(&engine, &r, Some("h"));
        assert_eq!(pop.dangling.len(), 1);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "the sidecar is evidence, not a mess to tidy"
        );
    }

    /// Criterion 8. A deferred mem holds no entities in the store, so a
    /// missing id proves nothing. Claiming zero dangling rows there would be
    /// the silent-clean this campaign exists to remove: the population says
    /// WHY instead, and keeps the rows it can still adjudicate.
    #[test]
    fn a_mem_whose_entities_are_not_loaded_says_so_instead_of_reporting_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture_with(
            tmp.path(),
            "src/**/*.rs",
            &["src/a.rs"],
            vec![anchor("src/a.rs", Some("h"), AnchorGrain::File)],
            false,
            MountLifecycle::Lazy,
        );
        let pop = population_for(&engine, &r, Some("h"));
        assert!(
            pop.unreconciled.is_some(),
            "the surface must be able to say the entity end was not examined"
        );
        assert!(
            pop.dangling.is_empty(),
            "and must not fabricate dangling rows from an unloaded store"
        );
        assert_eq!(
            pop.included.len(),
            1,
            "the artifact end is still adjudicable and is still reported"
        );
    }

    /// Criterion 7's complement at this layer: an entity that IS there keeps
    /// its anchors, so the fix cannot turn the engine's own correct delete and
    /// rename handling into a finding.
    #[test]
    fn an_entity_that_exists_produces_no_dangling_row() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture(
            tmp.path(),
            "src/**/*.rs",
            &["src/a.rs"],
            vec![anchor("src/a.rs", Some("h"), AnchorGrain::File)],
        );
        let pop = population_for(&engine, &r, Some("h"));
        assert!(pop.dangling.is_empty());
        assert_eq!(pop.included.len(), 1);
    }

    /// The fallback counter reports what was KEPT. Counting at the provenance
    /// branch credited anchors the scope test then excluded, so a report could
    /// say one anchor was included by the fallback over an empty population.
    #[test]
    fn the_fallback_counter_counts_only_what_survived_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture(
            tmp.path(),
            "src/**/*.rs",
            &["src/a.rs"],
            vec![anchor("docs/b.md", None, AnchorGrain::File)],
        );
        let pop = population_for(&engine, &r, Some("h"));
        assert!(pop.included.is_empty());
        assert_eq!(
            pop.without_provenance, 0,
            "an excluded anchor was never kept by the fallback"
        );
    }

    /// Rows and artifacts must stay comparable. The row count is the
    /// population's own size, not a sum of the state buckets: deriving it as
    /// observed plus authored omitted the unobserved rows and printed fewer
    /// rows than artifacts.
    #[test]
    fn the_row_count_is_the_populations_own_size() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, r) = fixture(
            tmp.path(),
            "src/**/*.rs",
            &["src/a.rs"],
            vec![
                anchor("src/a.rs", Some("h"), AnchorGrain::File),
                anchor("src/a.rs", Some("h"), AnchorGrain::Span),
                anchor("src/b.rs", Some("h"), AnchorGrain::File),
            ],
        );
        let pop = population_for(&engine, &r, Some("h"));
        assert_eq!(pop.included.len(), 3);
        assert_eq!(pop.distinct_artifacts(), 2);
        assert!(
            pop.included.len() >= pop.distinct_artifacts(),
            "rows can never be fewer than the artifacts they cover"
        );
    }

    /// A source declaring no allow patterns is unscoped and has no opinion,
    /// rather than an empty scope that would exclude everything.
    #[test]
    fn an_unscoped_source_excludes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, mut r) = fixture(
            tmp.path(),
            "src/**/*.rs",
            &["src/a.rs"],
            vec![anchor("anywhere/x.md", Some("h"), AnchorGrain::File)],
        );
        if let Some(ResolvedSource::Primary(p)) = r.sources.first_mut() {
            p.scope.clear();
        }
        let pop = population_for(&engine, &r, Some("h"));
        assert_eq!(pop.included.len(), 1);
        assert!(pop.excluded.is_empty());
    }
}
