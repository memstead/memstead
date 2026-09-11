#![cfg(test)]

use super::*;
use memstead_base::anchor::{Anchor, AnchorGrain, AnchorHashStability, AnchorProvenanceClass};

fn key(hash: &str, head: &str) -> FindingKey {
    FindingKey {
        binding_hash: hash.to_string(),
        source_head: head.to_string(),
    }
}

fn anchor(class: AnchorProvenanceClass) -> Anchor {
    Anchor {
        artifact: "src/lib.rs".to_string(),
        grain: AnchorGrain::File,
        class,
        at_version: None,
        hash: if class.is_hash_bearing() {
            Some("h1".to_string())
        } else {
            None
        },
        hash_stability: AnchorHashStability::Stable,
        derived_from: Vec::new(),
        binding: None,
        source: None,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    }
}

/// The store round-trips through serde and survives a write/read cycle on
/// disk — the durability A1 rests on.
#[test]
fn store_round_trips_on_disk_and_delete_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert!(
        read_findings_store(root, "engine", "graph")
            .unwrap()
            .is_none()
    );

    let mut store = FindingsStore {
        binding: "engine/graph".to_string(),
        ..Default::default()
    };
    let k = key("hashA", "head1");
    store.record(
        k.clone(),
        "1".to_string(),
        vec![Finding {
            key: k.clone(),
            facet: "src".to_string(),
            target: FindingTarget::Artifact {
                artifact: "src/a.rs".to_string(),
            },
            class: FindingClass::Uncovered,
            detail: "d".to_string(),
            created_at: "1".to_string(),
        }],
    );
    write_findings_store(root, "engine", "graph", &store).unwrap();
    assert!(findings_store_path(root, "engine", "graph").exists());

    // The store subtree self-ignores: per-checkout engine state must
    // not surface as untracked noise in a tracked workspace.
    let ignore = root
        .join(WORKSPACE_STORE_DIR)
        .join(STATE_DIR)
        .join(FINDINGS_DIR)
        .join(".gitignore");
    assert_eq!(std::fs::read_to_string(&ignore).unwrap(), "*\n");

    // Fresh read from disk (a later process) sees the findings (A1).
    let back = read_findings_store(root, "engine", "graph")
        .unwrap()
        .unwrap();
    assert_eq!(back, store);
    assert_eq!(back.current(&k).len(), 1);

    delete_findings_store(root, "engine", "graph").unwrap();
    assert!(
        read_findings_store(root, "engine", "graph")
            .unwrap()
            .is_none()
    );
    // Idempotent.
    delete_findings_store(root, "engine", "graph").unwrap();
}

/// A3 — a changed `hash(D)` segregates the prior batch: findings under the
/// old hash are never `current` under the new key, only `superseded`.
#[test]
fn changed_binding_hash_supersedes_prior_findings() {
    let mut store = FindingsStore::default();
    let old = key("hashOLD", "head1");
    let new = key("hashNEW", "head1");
    let f_old = Finding {
        key: old.clone(),
        facet: "src".to_string(),
        target: FindingTarget::Artifact {
            artifact: "src/old.rs".to_string(),
        },
        class: FindingClass::Uncovered,
        detail: "old".to_string(),
        created_at: "1".to_string(),
    };
    store.record(old.clone(), "1".to_string(), vec![f_old.clone()]);

    // Recording under the new key must not touch the old batch.
    store.record(new.clone(), "2".to_string(), Vec::new());
    assert!(store.current(&new).is_empty(), "new key has its own view");
    let superseded = store.superseded(&new);
    assert_eq!(superseded.len(), 1, "old batch is segregated as superseded");
    assert_eq!(superseded[0], &f_old);
    // The old findings are never presented as current under the new key.
    assert!(!store.current(&new).contains(&f_old));
}

/// The impl-version bump's documented invalidation-by-construction: a
/// finding recorded under the `hash(D)` a prior engine generation
/// computed (`PREPARATION_IMPL_VERSION` 0, for a binding declaring no
/// preparation at all) is not current under the live hash — segregated
/// as superseded, never presented — because the impl version is hashed
/// into every binding's identity. The old key still reads its own batch,
/// so nothing is deleted, only retired from the current view.
#[test]
fn impl_version_bump_invalidates_findings_by_construction() {
    use memstead_base::binding::{
        PREPARATION_IMPL_VERSION, ScaffoldParams, hash_binding, hash_binding_at_impl_version,
        scaffold_binding,
    };
    let binding = scaffold_binding(ScaffoldParams {
        destination_mem: "plugin",
        source_name: "source-tree",
        pointer: "../public",
        medium_type: memstead_base::pipeline::MediumType::Codebase,
        intent: None,
        additional_deny_paths: Vec::new(),
    })
    .binding;
    assert!(binding.sources[0].preparation.is_none());
    // The live constant is whatever the latest landed implementation set
    // it to; the pin is that the version-0 hash (the pre-registry
    // generation) is not the live one.
    let _ = PREPARATION_IMPL_VERSION;
    let old = key(&hash_binding_at_impl_version(&binding, 0), "head1");
    let live = key(&hash_binding(&binding), "head1");
    assert_ne!(old.binding_hash, live.binding_hash);

    let mut store = FindingsStore::default();
    let f_old = Finding {
        key: old.clone(),
        facet: "source-tree".to_string(),
        target: FindingTarget::Artifact {
            artifact: "src/old.rs".to_string(),
        },
        class: FindingClass::Uncovered,
        detail: "recorded before the bump".to_string(),
        created_at: "1".to_string(),
    };
    store.record(old.clone(), "1".to_string(), vec![f_old.clone()]);

    assert!(
        store.current(&live).is_empty(),
        "a finding keyed on the pre-bump hash is invalid under the live hash"
    );
    assert_eq!(store.superseded(&live), vec![&f_old]);
    assert_eq!(
        store.current(&old),
        &[f_old.clone()][..],
        "nothing is deleted"
    );
}

/// Criterion — findings survive head movement: the store keys on `hash(D)`
/// alone, so a finding recorded at head1 stays `current` when read at
/// head2 (the sync brief's read is head-agnostic), still carrying the head
/// it was observed at as metadata. REFUSAL half: recording the hash's next
/// batch (verify's post-merge write) replaces it — a finding absent from
/// that batch (resolved) never re-presents, at any head.
#[test]
fn moved_source_head_keeps_findings_current_until_superseded() {
    let mut store = FindingsStore::default();
    let before = key("hashA", "head1");
    let after = key("hashA", "head2");
    let f = Finding {
        key: before.clone(),
        facet: "src".to_string(),
        target: FindingTarget::Anchor {
            entity: "engine--e".to_string(),
            artifact: "src/x.rs".to_string(),
        },
        class: FindingClass::UnresolvableAnchor,
        detail: "gone".to_string(),
        created_at: "1".to_string(),
    };
    store.record(before.clone(), "1".to_string(), vec![f.clone()]);

    // The head moved; the finding is still presented, with its observed
    // head intact, and it is not "superseded".
    assert_eq!(store.current(&after), std::slice::from_ref(&f));
    assert_eq!(store.current(&after)[0].key.source_head, "head1");
    assert!(store.superseded(&after).is_empty());

    // A verify at head2 records the hash's next batch WITHOUT the finding
    // (its target observed clean) → resolved, never re-presented.
    store.record(after.clone(), "2".to_string(), Vec::new());
    assert!(store.current(&after).is_empty());
    assert!(store.current(&before).is_empty(), "at the old head too");
    assert_eq!(store.batches.len(), 1, "one batch per hash(D)");
}

/// Migration/compat — a store written by the pre-re-key engine (batches
/// keyed `(hash(D), source_head)`; the exact on-disk shape this project's own live
/// workspaces carry) loads without loss: the other-hash batch stays
/// segregated as superseded, the current-hash batch presents at ANY head,
/// and a legacy same-hash pair collapses to its latest-recorded batch —
/// never resurrecting the older (superseded-at-write-time) one. The next
/// `record` folds the same-hash siblings into one batch.
#[test]
fn legacy_per_head_store_loads_and_presents_head_agnostically() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let path = findings_store_path(root, "engine", "graph");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Trimmed replica of the live on-disk format: `{binding, batches:[{key:
    // {binding_hash, source_head}, recorded_at, findings:[{key, facet,
    // target:{kind,...}, class, detail, created_at}]}]}` — one batch under
    // an old hash, two batches under the current hash at different heads.
    std::fs::write(
        &path,
        r#"{
              "binding": "engine/graph",
              "batches": [
                {
                  "key": { "binding_hash": "hashOLD", "source_head": "src=aaa" },
                  "recorded_at": "100",
                  "findings": [
                    {
                      "key": { "binding_hash": "hashOLD", "source_head": "src=aaa" },
                      "facet": "src",
                      "target": { "kind": "artifact", "artifact": "src/old.rs" },
                      "class": "uncovered",
                      "detail": "old declaration",
                      "created_at": "100"
                    }
                  ]
                },
                {
                  "key": { "binding_hash": "hashCUR", "source_head": "src=bbb" },
                  "recorded_at": "200",
                  "findings": [
                    {
                      "key": { "binding_hash": "hashCUR", "source_head": "src=bbb" },
                      "facet": "src",
                      "target": { "kind": "artifact", "artifact": "src/resolved-at-ccc.rs" },
                      "class": "uncovered",
                      "detail": "was open at bbb, absent from the ccc batch",
                      "created_at": "200"
                    }
                  ]
                },
                {
                  "key": { "binding_hash": "hashCUR", "source_head": "src=ccc" },
                  "recorded_at": "300",
                  "findings": [
                    {
                      "key": { "binding_hash": "hashCUR", "source_head": "src=ccc" },
                      "facet": "src",
                      "target": { "kind": "anchor", "entity": "engine--e", "artifact": "src/x.rs" },
                      "class": "unresolvable-anchor",
                      "detail": "gone",
                      "created_at": "300"
                    }
                  ]
                }
              ]
            }"#,
    )
    .unwrap();

    let mut store = read_findings_store(root, "engine", "graph")
        .unwrap()
        .expect("the legacy on-disk format loads as-is");
    assert_eq!(store.binding, "engine/graph");
    assert_eq!(store.batches.len(), 3, "loaded without loss");

    // Head-agnostic current view: reading at a NEWLY moved head (ddd —
    // recorded nowhere) presents the latest current-hash batch.
    let now = key("hashCUR", "src=ddd");
    let current = store.current(&now);
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].detail, "gone");
    assert_eq!(
        current[0].key.source_head, "src=ccc",
        "the finding keeps the head it was observed at"
    );
    // The pre-re-key superseded batches (old hash + the older same-hash
    // head) stay segregated — never mixed into the current view.
    let superseded = store.superseded(&now);
    assert_eq!(superseded.len(), 2);
    assert!(
        !current.iter().any(|f| f.detail.contains("was open at bbb")),
        "the older same-hash batch was superseded at write time and is not resurrected"
    );

    // The next record under the current hash collapses the legacy
    // same-hash pair into one batch; the old-hash batch is untouched.
    store.record(now.clone(), "400".to_string(), Vec::new());
    assert_eq!(store.batches.len(), 2, "hashCUR collapsed, hashOLD kept");
    assert_eq!(store.superseded(&now).len(), 1);
}

/// An authored exclusion supersedes a standing `uncovered` finding in
/// the STORE, not only in the presentation filter: the merge's
/// accounting closure folds the exclusion ledger in, so an unsampled
/// artifact that gained an exclusion since its finding was recorded
/// closes instead of carrying forward — the verdict count can no longer
/// contradict the coverage section's "0 unaccounted".
#[test]
fn merge_closes_uncovered_findings_for_ledger_excluded_artifacts() {
    let k_old = key("h", "head1");
    let uncovered = |artifact: &str| Finding {
        key: k_old.clone(),
        facet: "src".to_string(),
        target: FindingTarget::Artifact {
            artifact: artifact.to_string(),
        },
        class: FindingClass::Uncovered,
        detail: "no anchor".to_string(),
        created_at: "1".to_string(),
    };
    let prior = vec![uncovered("src/excluded.rs"), uncovered("src/open.rs")];
    let obs = PassObservation {
        anchors_observed: BTreeSet::new(),
        anchors_existing: BTreeSet::new(),
        files_observed: BTreeSet::new(), // neither sampled this pass
        s_d: ["src/excluded.rs".to_string(), "src/open.rs".to_string()].into(),
    };
    let excluded: BTreeSet<String> = ["src/excluded.rs".to_string()].into();
    let merged = merge_with_prior(Vec::new(), &prior, &obs, |artifact: &str| {
        excluded.contains(artifact)
    });
    assert_eq!(
        merged.len(),
        1,
        "the excluded finding closes, the open one carries: {merged:?}"
    );
    assert_eq!(
        merged[0].target,
        FindingTarget::Artifact {
            artifact: "src/open.rs".to_string()
        }
    );
}

/// The head-durable merge: an unobserved-but-still-open prior finding
/// carries forward (original observed head intact); a prior finding whose
/// artifact left `S(D)`, gained coverage, or whose anchor vanished closes;
/// a re-observed target takes this pass's outcome (clean → closed).
#[test]
fn merge_carries_unobserved_open_findings_and_closes_departed() {
    let k_old = key("h", "head1");
    let mk_artifact = |artifact: &str, detail: &str| Finding {
        key: k_old.clone(),
        facet: "src".to_string(),
        target: FindingTarget::Artifact {
            artifact: artifact.to_string(),
        },
        class: FindingClass::Uncovered,
        detail: detail.to_string(),
        created_at: "1".to_string(),
    };
    let anchor_finding = Finding {
        key: k_old.clone(),
        facet: "src".to_string(),
        target: FindingTarget::Anchor {
            entity: "engine--gone".to_string(),
            artifact: "src/gone.rs".to_string(),
        },
        class: FindingClass::UnresolvableAnchor,
        detail: "anchor since removed from the mem".to_string(),
        created_at: "1".to_string(),
    };
    let prior = vec![
        mk_artifact("src/unsampled.rs", "still open, not in this window"),
        mk_artifact("src/departed.rs", "left S(D)"),
        mk_artifact("src/now-covered.rs", "gained an anchor since"),
        mk_artifact("src/observed-clean.rs", "re-sampled and now covered"),
        anchor_finding,
    ];
    let obs = PassObservation {
        anchors_observed: BTreeSet::new(),
        anchors_existing: BTreeSet::new(), // the anchor vanished
        files_observed: ["src/observed-clean.rs".to_string()].into(),
        s_d: [
            "src/unsampled.rs".to_string(),
            "src/now-covered.rs".to_string(),
            "src/observed-clean.rs".to_string(),
        ]
        .into(),
    };
    let merged = merge_with_prior(Vec::new(), &prior, &obs, |artifact| {
        artifact == "src/now-covered.rs" || artifact == "src/observed-clean.rs"
    });
    assert_eq!(merged.len(), 1, "only the still-open unsampled one carries");
    assert_eq!(
        merged[0].target,
        FindingTarget::Artifact {
            artifact: "src/unsampled.rs".to_string()
        }
    );
    assert_eq!(
        merged[0].key.source_head, "head1",
        "a carried finding keeps the head it was observed at"
    );
}

/// Supersession honesty: a fresh `queued-for-adjudication` entry is a
/// scheduling deferral, not an observation — it never downgrades a prior
/// substantive `drifted` verdict for the same target. A fresh substantive
/// outcome (or a clean observation) still supersedes normally.
#[test]
fn merge_deferral_never_downgrades_prior_adjudication() {
    let k_old = key("h", "head1");
    let k_new = key("h", "head2");
    let target = FindingTarget::Anchor {
        entity: "engine--e".to_string(),
        artifact: "src/x.rs".to_string(),
    };
    let prior_drifted = Finding {
        key: k_old.clone(),
        facet: "src".to_string(),
        target: target.clone(),
        class: FindingClass::Drifted,
        detail: "adjudicated drifted at head1".to_string(),
        created_at: "1".to_string(),
    };
    let fresh_queued = Finding {
        key: k_new.clone(),
        facet: "src".to_string(),
        target: target.clone(),
        class: FindingClass::QueuedForAdjudication,
        detail: "deferred by the cap this run".to_string(),
        created_at: "2".to_string(),
    };
    let obs = PassObservation {
        anchors_observed: [target_key(&target)].into(),
        anchors_existing: [target_key(&target)].into(),
        files_observed: BTreeSet::new(),
        s_d: BTreeSet::new(),
    };
    let merged = merge_with_prior(
        vec![fresh_queued],
        std::slice::from_ref(&prior_drifted),
        &obs,
        |_| true,
    );
    assert_eq!(merged.len(), 1);
    assert_eq!(
        merged[0].class,
        FindingClass::Drifted,
        "the prior verdict stands over a deferral"
    );
    assert_eq!(merged[0].key.source_head, "head1");
}

/// A2 — hash-drift adjudication is excluded for `informed-by` (and every
/// non-hash-bearing class): a drifted/recheck state yields NO finding.
#[test]
fn informed_by_anchor_never_drifts() {
    let k = key("h", "s");
    for class in [
        AnchorProvenanceClass::InformedBy,
        AnchorProvenanceClass::Authored,
    ] {
        let a = anchor(class);
        assert!(
            adjudicate_anchor(&k, "f", "engine--e", &a, AnchorState::Drifted, "1").is_none(),
            "{class:?} must not produce a drift finding"
        );
        assert!(
            adjudicate_anchor(&k, "f", "engine--e", &a, AnchorState::Recheck, "1").is_none(),
            "{class:?} must not produce a queued finding"
        );
    }
}

/// A2 — hash-bearing classes DO produce drift/recheck findings, and every
/// class produces an existence (`unresolvable-anchor`) finding when orphaned.
#[test]
fn hash_bearing_drifts_and_orphan_is_class_independent() {
    let k = key("h", "s");
    let anchored = anchor(AnchorProvenanceClass::Anchored);
    let drifted =
        adjudicate_anchor(&k, "f", "engine--e", &anchored, AnchorState::Drifted, "1").unwrap();
    assert_eq!(drifted.class, FindingClass::Drifted);
    assert_eq!(drifted.key, k, "the finding carries its recording key (A2)");

    let queued =
        adjudicate_anchor(&k, "f", "engine--e", &anchored, AnchorState::Recheck, "1").unwrap();
    assert_eq!(queued.class, FindingClass::QueuedForAdjudication);

    // Orphaned is existence, not hash-drift — reported for informed-by too.
    let informed = anchor(AnchorProvenanceClass::InformedBy);
    let orphan =
        adjudicate_anchor(&k, "f", "engine--e", &informed, AnchorState::Orphaned, "1").unwrap();
    assert_eq!(orphan.class, FindingClass::UnresolvableAnchor);

    // Resolves yields nothing.
    assert!(
        adjudicate_anchor(&k, "f", "engine--e", &anchored, AnchorState::Resolves, "1").is_none()
    );
}

/// The finding class vocabulary round-trips through its wire form.
#[test]
fn finding_class_wire_round_trips() {
    for w in FindingClass::WIRE_VALUES {
        let c = FindingClass::from_wire(w).expect("known wire value");
        assert_eq!(c.as_wire(), *w);
    }
    assert!(FindingClass::from_wire("nonsense").is_none());
}

/// A malformed binding id refuses before touching the store tier.
#[test]
fn malformed_binding_id_refuses() {
    assert!(matches!(
        split_binding_id("../escape"),
        Err(FindingsError::MalformedId(_))
    ));
    assert!(matches!(
        split_binding_id("no-slash"),
        Err(FindingsError::MalformedId(_))
    ));
    assert_eq!(
        split_binding_id("engine/graph").unwrap(),
        ("engine".to_string(), "graph".to_string())
    );
}

// ---- A1/A5 end-to-end: verify writes durable findings, no entity write --

use memstead_base::anchor::AnchorSidecar;
use memstead_base::binding::{
    BINDING_VERSION, BuildMode, BuildOperation, CoverageSemantics, DEFAULT_ADJUDICATION_CAP,
    DEFAULT_FULL_RESYNC_EVERY, Operations, VerifyOperation,
};
use memstead_base::binding_run::resolve_binding_run;
use memstead_base::pipeline::{IngestTrigger, MediumType, PatternEntry, PatternMode};
use memstead_base::pipeline_store::{load_pipeline_configs, write_binding};
use memstead_base::workspace::{
    Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
};
use memstead_base::workspace_store::WorkspaceStoreAdapter;

/// A full verify pass over a folder mem: it adjudicates the mem's anchors
/// against the live source (orphaned → unresolvable-anchor; present
/// hash-bearing whose recorded hash mismatches the observed prepared form
/// → deterministic `drifted`; informed-by → no finding, A2) and flags an
/// uncovered source file, then persists the findings to the durable state
/// tier. A **fresh** read from disk (a later process) sees them (A1). The
/// pass runs on a shared `&Engine` — structurally read-only on the mem (A5).
#[test]
fn verify_persists_findings_readable_fresh() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mem_dir = root.join("mem");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();

    // Workspace state so `from_workspace_root` sets `workspace_root` (which
    // the anchor observation and cursor need) and mounts the `engine` mem.
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
            root,
            &Workspace {
                mounts: vec![mount],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();

    // A git work tree at the workspace root so the codebase medium's `git`
    // change strategy resolves; source files: one anchored+present, one
    // uncovered.
    let out = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(out.status.success());
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("present.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("src").join("uncovered.rs"), "fn b() {}\n").unwrap();

    // Seed the engine-owned anchors sidecar directly (test fixture — the
    // production write path is the mutation surface, not this verify code).
    let mk = |artifact: &str, class: AnchorProvenanceClass| Anchor {
        artifact: artifact.to_string(),
        grain: AnchorGrain::File,
        class,
        at_version: None,
        hash: class.is_hash_bearing().then(|| "recorded".to_string()),
        hash_stability: AnchorHashStability::Stable,
        derived_from: Vec::new(),
        binding: None,
        source: None,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    };
    // The entity the sidecar is keyed to. Written, because it exists:
    // a row whose entity does not is DANGLING (consistency-sweep 03/02)
    // and leaves the population before any figure counts it.
    std::fs::write(
        mem_dir.join("e.md"),
        "---\ntype: decision\n---\n\n# E\n\n## Decision\n\nBody.\n",
    )
    .unwrap();
    let mut sidecar = AnchorSidecar::default();
    sidecar.set(
        "engine--e",
        vec![
            mk("src/present.rs", AnchorProvenanceClass::Anchored), // recorded hash mismatches prepared form → drifted
            mk("src/gone.rs", AnchorProvenanceClass::Anchored),    // absent → unresolvable-anchor
            mk("src/present.rs", AnchorProvenanceClass::InformedBy), // present, non-hash → no finding (A2)
        ],
    );
    std::fs::write(
        mem_dir.join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
        sidecar.to_bytes(),
    )
    .unwrap();

    // Binding engine/graph over a codebase facet (medium root = workspace).
    write_binding(
        root,
        "engine",
        "graph",
        &Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![memstead_base::pipeline::Source {
                name: "graph".to_string(),
                medium_type: MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
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
        },
    )
    .unwrap();

    let engine = Engine::from_workspace_root(root).unwrap();

    let configs = load_pipeline_configs(root).unwrap();
    let binding = &configs.bindings[0].config;
    let resolved = resolve_binding_run("engine/graph", binding).unwrap();

    // `&engine` — shared borrow, structurally cannot mutate the mem (A5).
    let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
    assert!(
        outcome.recorded >= 3,
        "orphan + drifted + uncovered at least"
    );
    assert_eq!(outcome.superseded, 0, "no prior key yet");
    assert_eq!(
        outcome.backlog, 0,
        "the mismatching hash adjudicated deterministically — nothing queued"
    );
    assert!(
        outcome.hash_backfill.is_empty(),
        "every hash-bearing anchor already carries a recorded hash — nothing to backfill"
    );

    // Fresh read from disk — a later process / sync-brief render (A1).
    let store = read_findings_store(root, "engine", "graph")
        .unwrap()
        .unwrap();
    let current = store.current(&outcome.key);
    assert_eq!(current.len(), outcome.recorded);

    let has = |c: FindingClass, art: &str| {
        current.iter().any(|f| {
            f.class == c
                && match &f.target {
                    FindingTarget::Anchor { artifact, .. } => artifact == art,
                    FindingTarget::Artifact { artifact } => artifact == art,
                    FindingTarget::Mention { artifact, .. } => artifact == art,
                }
        })
    };
    assert!(has(FindingClass::UnresolvableAnchor, "src/gone.rs"));
    assert!(
        has(FindingClass::Drifted, "src/present.rs"),
        "recorded-hash mismatch on a stable medium adjudicates drifted deterministically"
    );
    assert!(has(FindingClass::Uncovered, "src/uncovered.rs"));
    // A2: the informed-by anchor on the present file produced no finding —
    // the one drifted finding above belongs to the anchored (hash-bearing)
    // anchor, and nothing queued.
    assert!(
        !current
            .iter()
            .any(|f| f.class == FindingClass::QueuedForAdjudication
                || f.class == FindingClass::Wrong),
        "deterministic adjudication leaves nothing queued"
    );
    // The covered file is not flagged uncovered.
    assert!(!has(FindingClass::Uncovered, "src/present.rs"));
}

/// A scratch workspace for the mention walk: a folder mem `engine` with a
/// codebase binding `engine/graph` whose facet points at `pointer` (empty:
/// the workspace root) with scope `**/*.rs`. Source files, entities and
/// the anchors sidecar are the caller's to write.
fn mention_workspace(root: &Path, pointer: &str) {
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
            root,
            &Workspace {
                mounts: vec![mount],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();
    let out = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(out.status.success());
    write_binding(
        root,
        "engine",
        "graph",
        &Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![memstead_base::pipeline::Source {
                name: "graph".to_string(),
                medium_type: MediumType::Codebase,
                pointer: pointer.to_string(),
                change_detection: Some("git".to_string()),
                scope: vec![PatternEntry {
                    path: "**/*.rs".to_string(),
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
                build: None,
                sync: None,
                verify: Some(VerifyOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 20,
                    adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                    full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
                }),
            },
        },
    )
    .unwrap();
}

fn decision_entity(root: &Path, slug: &str, body: &str) {
    std::fs::write(
        root.join("mem").join(format!("{slug}.md")),
        format!("---\ntype: decision\n---\n\n# {slug}\n\n## Decision\n\n{body}\n"),
    )
    .unwrap();
}

fn informed_by(artifact: &str) -> Anchor {
    Anchor {
        artifact: artifact.to_string(),
        grain: AnchorGrain::File,
        class: AnchorProvenanceClass::InformedBy,
        at_version: None,
        hash: None,
        hash_stability: AnchorHashStability::Stable,
        derived_from: Vec::new(),
        binding: None,
        source: None,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    }
}

fn mentions_of<'a>(findings: &'a [Finding], entity: &str) -> Vec<(&'a str, &'a str)> {
    let mut out: Vec<(&str, &str)> = findings
        .iter()
        .filter(|f| f.class == FindingClass::UnanchoredMention)
        .filter_map(|f| match &f.target {
            FindingTarget::Mention {
                entity: e,
                artifact,
                section,
            } if e == entity => Some((artifact.as_str(), section.as_str())),
            _ => None,
        })
        .collect();
    out.sort();
    out
}

/// Criterion "verify records an unanchored mention per entity and
/// artifact", with its refusal complement: a full verify over a scratch
/// binding records one `unanchored-mention` finding for the entity whose
/// prose names an in-scope file it does not anchor (naming the section),
/// none for the entity that anchors the file, none for one that names it
/// inside a fenced code block only, and none for one that names nothing;
/// once the entity anchors the file, the next verify records no mention
/// for it. The fidelity report carries the count beside `uncovered` with
/// the anchors remedy, and health lists each finding as an
/// `UNANCHORED_MENTION` warning.
#[test]
fn verify_records_an_unanchored_mention_per_entity_and_artifact() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    mention_workspace(root, "");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("present.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("src").join("other.rs"), "fn b() {}\n").unwrap();

    // A anchors F; B names F (and a second file) without anchoring; C
    // names F inside a fence only; D names nothing in scope.
    decision_entity(root, "a", "Anchored: `src/present.rs` is watched.");
    decision_entity(
        root,
        "b",
        "The reader in src/present.rs refuses empty input, and `src/other.rs` \
             mirrors it. See also e.g. the docs.",
    );
    decision_entity(root, "c", "```rust\n// src/present.rs\nfn a() {}\n```");
    decision_entity(root, "d", "Nothing named here.");
    let mut sidecar = AnchorSidecar::default();
    sidecar.set("engine--a", vec![informed_by("src/present.rs")]);
    std::fs::write(
        root.join("mem")
            .join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
        sidecar.to_bytes(),
    )
    .unwrap();

    let engine = Engine::from_workspace_root(root).unwrap();
    let configs = load_pipeline_configs(root).unwrap();
    let binding = &configs.bindings[0].config;
    let resolved = resolve_binding_run("engine/graph", binding).unwrap();

    let outcome = verify_binding_full(&engine, root, binding, &resolved).unwrap();
    let store = read_findings_store(root, "engine", "graph")
        .unwrap()
        .unwrap();
    let current = store.current(&outcome.key).to_vec();
    assert_eq!(
        mentions_of(&current, "engine--b"),
        vec![
            ("src/other.rs", "specifies"),
            ("src/present.rs", "specifies")
        ],
        "one finding per (entity, artifact), naming the section"
    );
    assert!(
        mentions_of(&current, "engine--a").is_empty(),
        "anchored: no mention"
    );
    assert!(
        mentions_of(&current, "engine--c").is_empty(),
        "a fenced code block is not a claim"
    );
    assert!(mentions_of(&current, "engine--d").is_empty());
    let detail = current
        .iter()
        .find(|f| f.class == FindingClass::UnanchoredMention)
        .map(|f| f.detail.clone())
        .unwrap();
    assert!(detail.contains("`specifies`"), "{detail}");

    // The report: the count beside `uncovered`, with the anchors remedy,
    // and the heavy list naming each mention.
    let report = super::super::report::compute_fidelity_report(
        &engine,
        root,
        binding,
        &resolved,
        &outcome.key,
    );
    assert_eq!(report.coverage.unanchored_mentions.len(), 2);
    let md = super::super::report::render_fidelity_report(
        &report,
        super::super::report::DEFAULT_REPORT_BUDGET,
        &["unanchored_mentions".to_string()],
    )
    .markdown;
    assert!(
        md.contains(
            "- uncovered (no anchor): 1\n- unanchored mentions (an entity names an \
                         in-scope artifact it does not anchor): 2 — remedy: add the anchor \
                         (`memstead_update` with `anchors`)"
        ),
        "{md}"
    );
    assert!(
        md.contains("## Unanchored mentions\n\n- `engine--b` names `src/other.rs` in `specifies`"),
        "{md}"
    );
    assert_eq!(report.findings_by_class.get("unanchored-mention"), Some(&2));
    let rollup = report.rollup();
    assert!(
        rollup
            .actions
            .iter()
            .any(|a| a.starts_with("2 claim(s) name an in-scope artifact")),
        "{:?}",
        rollup.actions
    );

    // Health: one `UNANCHORED_MENTION` warning per finding, naming the
    // entity, the artifact and the section — the loop's axis, handed
    // to the kernel composer by `ingest::health::compose_health`.
    let loop_warnings = unanchored_mention_warnings(&engine);
    let mentions: Vec<&memstead_base::ops::WarningHint> = loop_warnings
        .iter()
        .filter(|w| w.code() == "UNANCHORED_MENTION")
        .collect();
    assert_eq!(mentions.len(), 2, "{:?}", loop_warnings);
    assert!(mentions.iter().all(|w| {
        let msg = w.message();
        msg.contains("`engine--b` names `src/") && msg.contains("in section `specifies`")
    }));

    // Refusal complement: B anchors F and the other file is excluded with
    // a rationale — the next verify records no mention for B, and the
    // report counts the excluded file under `excluded`.
    sidecar.set("engine--b", vec![informed_by("src/present.rs")]);
    std::fs::write(
        root.join("mem")
            .join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
        sidecar.to_bytes(),
    )
    .unwrap();
    let engine = Engine::from_workspace_root(root).unwrap();
    let exclusions: BTreeMap<String, String> = [(
        "src/other.rs".to_string(),
        "a mirror of present.rs, warrants no entity".to_string(),
    )]
    .into_iter()
    .collect();
    super::super::advance::record_exclusions(&engine, root, &resolved, &exclusions).unwrap();
    let outcome = verify_binding_full(&engine, root, binding, &resolved).unwrap();
    let store = read_findings_store(root, "engine", "graph")
        .unwrap()
        .unwrap();
    let current = store.current(&outcome.key).to_vec();
    assert!(
        mentions_of(&current, "engine--b").is_empty(),
        "anchored or excluded: no mention stands, and none is carried forward: {current:?}"
    );
    let report = super::super::report::compute_fidelity_report(
        &engine,
        root,
        binding,
        &resolved,
        &outcome.key,
    );
    assert!(report.coverage.unanchored_mentions.is_empty());
    assert_eq!(
        report.coverage.excluded, 1,
        "the excluded file counts under `excluded`"
    );
    assert!(
        engine
            .health()
            .warnings
            .iter()
            .all(|w| w.code() != "UNANCHORED_MENTION")
    );
}

/// A git-backed scratch binding whose baseline is the first commit: `F`
/// (`src/f.rs`) defines `old_name` at the baseline and `new_name` at head,
/// `G` (`src/g.rs`) is added at head. The `#synced` token pins the
/// baseline, so the slice is `modified [F]`, `added [G]`.
fn moved_source(root: &Path) -> (String, String) {
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("f.rs"), "pub fn old_name() {}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "baseline"]);
    let baseline = git(&["rev-parse", "HEAD"]);
    {
        let mut engine = Engine::from_workspace_root(root).unwrap();
        engine
            .set_mem_sync_state("engine", "engine/graph/graph#synced", &baseline, None)
            .unwrap();
    }
    std::fs::write(
        root.join("src").join("f.rs"),
        "pub fn new_name() {}\npub struct Kept;\n",
    )
    .unwrap();
    std::fs::write(root.join("src").join("g.rs"), "pub fn g_only() {}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "head"]);
    (baseline, git(&["rev-parse", "HEAD"]))
}

/// Criterion "a changed file's brief lists the entities that name it
/// without anchoring it", with its refusal complement: over a moved
/// source, the sync brief lists A (anchors F) under the anchored line
/// and B (names F's path) and C (names `new_name`, which F's change
/// defines) under the mention lines for F; D (names neither) appears
/// nowhere; `projection advance` leaves F pending until an explicit
/// disposition (its anchor no longer auto-disposes it) and then accepts
/// F's id in one call with the rest of the slice.
#[test]
fn a_changed_files_brief_lists_the_entities_that_name_it_without_anchoring_it() {
    use crate::advance::{DispositionInput, advance_baseline};
    use crate::render::render_sync_brief_for;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    mention_workspace(root, "");
    decision_entity(root, "a", "Anchored on F.");
    decision_entity(root, "b", "The reader in `src/f.rs` refuses empty input.");
    decision_entity(root, "c", "Callers reach it through `new_name()` only.");
    decision_entity(
        root,
        "d",
        "Names nothing of F: `unrelated_symbol` and lib/z.rs.",
    );
    let mut sidecar = AnchorSidecar::default();
    sidecar.set("engine--a", vec![informed_by("src/f.rs")]);
    std::fs::write(
        root.join("mem")
            .join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
        sidecar.to_bytes(),
    )
    .unwrap();
    moved_source(root);

    let engine = Engine::from_workspace_root(root).unwrap();
    let brief = render_sync_brief_for(&engine, root, "engine/graph").unwrap();
    let block = brief
        .split("### Entities to walk for each changed artifact")
        .nth(1)
        .expect("the steered block renders on a moved source: {brief}")
        .split("### Recording your dispositions")
        .next()
        .unwrap()
        .to_string();
    assert!(
        block.contains(
            "- `src/f.rs`\n  - anchored by: `engine--a`\n  - mentioned by:\n    - `engine--b` \
                 (path, in `specifies`)\n    - `engine--c` (`new_name`, in `specifies`)\n"
        ),
        "{block}"
    );
    assert!(
        !block.contains("engine--d") && !block.contains("src/g.rs"),
        "an entity naming neither the path nor a defined symbol is absent, and an \
             artifact steering nothing is absent: {block}"
    );

    // Advance: F is not auto-disposed by A's anchor while B and C are
    // steered at it; G (no entity at all) stays pending too.
    let configs = load_pipeline_configs(root).unwrap();
    let binding = &configs.bindings[0].config;
    let resolved = resolve_binding_run("engine/graph", binding).unwrap();
    let mut engine = Engine::from_workspace_root(root).unwrap();
    let out = advance_baseline(&mut engine, root, &resolved, &BTreeMap::new()).unwrap();
    assert!(
        out.remainder.modified == vec!["src/f.rs".to_string()],
        "F stays pending until the mention-steered entities are judged: {out:?}"
    );
    assert!(!out.completed);
    let dispositions: BTreeMap<String, DispositionInput> =
        [("src/f.rs", "worked"), ("src/g.rs", "skipped")]
            .into_iter()
            .map(|(a, d)| (a.to_string(), DispositionInput::Verdict(d.to_string())))
            .collect();
    let out = advance_baseline(&mut engine, root, &resolved, &dispositions).unwrap();
    assert!(out.completed, "F's id is accepted like any other: {out:?}");
}

/// Criterion "the mention block degrades to a count under the budget,
/// never to silence", with its refusal complement: under a budget too
/// small for the mention lines the brief keeps the anchored line and
/// states the number of mention-steered entities with the include hint;
/// with `--include mentions` the full lines return; and a slice no entity
/// names renders neither a mention line nor a count line.
#[test]
fn the_mention_block_degrades_to_a_count_under_the_budget_never_to_silence() {
    use crate::render::render_sync_brief_budgeted;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    mention_workspace(root, "");
    decision_entity(root, "a", "Anchored on F.");
    decision_entity(root, "b", "The reader in `src/f.rs` refuses empty input.");
    decision_entity(root, "c", "Callers reach it through `new_name()` only.");
    let mut sidecar = AnchorSidecar::default();
    sidecar.set("engine--a", vec![informed_by("src/f.rs")]);
    std::fs::write(
        root.join("mem")
            .join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
        sidecar.to_bytes(),
    )
    .unwrap();
    moved_source(root);
    let engine = Engine::from_workspace_root(root).unwrap();

    let tight = render_sync_brief_budgeted(&engine, root, "engine/graph", 1, &[]).unwrap();
    assert!(
        tight.contains("- `src/f.rs`\n  - anchored by: `engine--a`\n"),
        "the anchored line is unchanged under the budget: {tight}"
    );
    assert!(!tight.contains("  - mentioned by:"), "{tight}");
    assert!(
        tight.contains(
            "- _2 mention-steered entities over 1 changed artifact not listed under the \
                 token budget ("
        ) && tight.contains("re-render with `--include mentions`"),
        "{tight}"
    );

    let full =
        render_sync_brief_budgeted(&engine, root, "engine/graph", 1, &["mentions".to_string()])
            .unwrap();
    assert!(
        full.contains("  - mentioned by:\n    - `engine--b` (path, in `specifies`)"),
        "{full}"
    );
    assert!(!full.contains("mention-steered entities over"), "{full}");

    // Refusal complement: a slice no entity names renders no mention line
    // and no count line. The same source, a mem whose only entity names
    // nothing in it.
    let tmp2 = tempfile::tempdir().unwrap();
    let root2 = tmp2.path();
    mention_workspace(root2, "");
    decision_entity(root2, "d", "Nothing here names the source.");
    std::fs::write(
        root2
            .join("mem")
            .join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
        AnchorSidecar::default().to_bytes(),
    )
    .unwrap();
    moved_source(root2);
    let engine2 = Engine::from_workspace_root(root2).unwrap();
    let quiet = render_sync_brief_budgeted(&engine2, root2, "engine/graph", 1, &[]).unwrap();
    assert!(quiet.contains("**Modified:**\n- `src/f.rs`"), "{quiet}");
    assert!(
        !quiet.contains("  - mentioned by:")
            && !quiet.contains("not listed under the token budget")
            && !quiet.contains("### Entities to walk"),
        "{quiet}"
    );
}

/// The symbol scanners are lexical: definitions by keyword and enum
/// variants on one side, inline code spans split into their segments on
/// the other.
#[test]
fn symbol_scanners_are_lexical() {
    use crate::cursor::defined_symbols;
    let defined = defined_symbols(
        "pub fn alpha(x: u8) {}\nstruct Beta;\npub enum Gamma {\n    First,\n    Second { \
             n: u8 },\n    Third(u8),\n}\nconst DELTA: u8 = 1;\nfn not_enum() {}\nclass Eps:\n    \
             def eta(self): pass\n",
    );
    for s in [
        "alpha", "Beta", "Gamma", "First", "Second", "Third", "DELTA", "not_enum", "Eps", "eta",
    ] {
        assert!(defined.contains(s), "{s} in {defined:?}");
    }
    assert!(!defined.contains("self") && !defined.contains("pass"));
    let spans = code_span_symbols(
        "Use `advance_baseline()` and `AdvanceError::UnknownArtifact`, not `` or `7up`; \
             ```\n`fenced_symbol`\n```",
    );
    assert_eq!(
        spans,
        vec!["advance_baseline", "AdvanceError", "UnknownArtifact"]
    );
}

/// Path matching follows the binding's source join: under a facet whose
/// pointer is `sub`, the artifact `sub/x.rs` is one file whether the
/// entity spells it source-relative (`x.rs`) or workspace-relative
/// (`sub/x.rs`) — one finding, not two, and a spelling that resolves
/// to nothing in `S(D)` raises none.
#[test]
fn mention_matching_follows_the_source_join() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    mention_workspace(root, "sub");
    std::fs::create_dir_all(root.join("sub").join("inner")).unwrap();
    std::fs::write(root.join("sub").join("inner").join("x.rs"), "fn a() {}\n").unwrap();
    decision_entity(
        root,
        "b",
        "Spelt both ways: inner/x.rs and sub/inner/x.rs. Not in scope: lib/x.rs, x.rs.",
    );
    std::fs::write(
        root.join("mem")
            .join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
        AnchorSidecar::default().to_bytes(),
    )
    .unwrap();
    let engine = Engine::from_workspace_root(root).unwrap();
    let configs = load_pipeline_configs(root).unwrap();
    let binding = &configs.bindings[0].config;
    let resolved = resolve_binding_run("engine/graph", binding).unwrap();
    let outcome = verify_binding_full(&engine, root, binding, &resolved).unwrap();
    let store = read_findings_store(root, "engine", "graph")
        .unwrap()
        .unwrap();
    let current = store.current(&outcome.key).to_vec();
    assert_eq!(
        mentions_of(&current, "engine--b"),
        vec![("sub/inner/x.rs", "specifies")],
        "one artifact under either spelling: {current:?}"
    );
}

/// The token scanner: paths survive sentence punctuation and inline code
/// spans, a leading `./` is dropped, and words that are not path-shaped
/// never reach the lookup.
#[test]
fn path_tokens_are_lexical_and_punctuation_tolerant() {
    let toks = path_tokens(
        "See `src/a.rs`, then (src/b.rs). Also ./src/c.rs; and src/d.rs: line 3. \
             Not paths: e.g. and hello and v1.",
    );
    assert_eq!(
        toks,
        vec!["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs", "e.g"]
    );
}

/// Criterion, end-to-end — **findings survive head movement**: a finding
/// recorded at head H keeps presenting through the sync brief's read
/// (`current_findings` / `render_sync_brief_for`) after the source
/// advances to H′, until a verify observes its subject clean — and once
/// resolved it never re-presents, at any head.
#[test]
fn finding_recorded_at_old_head_presents_in_brief_at_new_head() {
    use crate::render::render_sync_brief_for;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
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
            root,
            &Workspace {
                mounts: vec![mount],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();

    // Git source tree at head A: src/present.rs committed.
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("present.rs"), "fn a() {}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "head-a"]);

    // Anchors: `informed-by` on the present file (clean, non-hash — no
    // finding) and on the ABSENT src/gone.rs (orphaned → the finding).
    let mk = |artifact: &str| Anchor {
        artifact: artifact.to_string(),
        grain: AnchorGrain::File,
        class: AnchorProvenanceClass::InformedBy,
        at_version: None,
        hash: None,
        hash_stability: AnchorHashStability::Stable,
        derived_from: Vec::new(),
        binding: None,
        source: None,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    };
    // The entity the sidecar is keyed to. Written, because it exists:
    // a row whose entity does not is DANGLING (consistency-sweep 03/02)
    // and leaves the population before any figure counts it.
    std::fs::write(
        mem_dir.join("e.md"),
        "---\ntype: decision\n---\n\n# E\n\n## Decision\n\nBody.\n",
    )
    .unwrap();
    let mut sidecar = AnchorSidecar::default();
    sidecar.set("engine--e", vec![mk("src/present.rs"), mk("src/gone.rs")]);
    std::fs::write(
        mem_dir.join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
        sidecar.to_bytes(),
    )
    .unwrap();

    write_binding(
        root,
        "engine",
        "graph",
        &Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![memstead_base::pipeline::Source {
                name: "graph".to_string(),
                medium_type: MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
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
            prune: None,
            operations: Operations {
                build: None,
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
        },
    )
    .unwrap();

    // Verify at head A — records the orphaned-anchor finding.
    let configs = load_pipeline_configs(root).unwrap();
    let binding = &configs.bindings[0].config;
    let resolved = resolve_binding_run("engine/graph", binding).unwrap();
    let head_a_outcome = {
        let engine = Engine::from_workspace_root(root).unwrap();
        verify_binding(&engine, root, binding, &resolved).unwrap()
    };
    assert!(
        head_a_outcome.key.source_head.contains("graph="),
        "the run observed a facet head"
    );

    // The source moves to head B.
    std::fs::write(root.join("src").join("present.rs"), "fn a() {} // more\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "head-b"]);

    // A fresh process at head B: the finding recorded at head A is still
    // presented — by the brief's read AND in the rendered sync brief.
    {
        let engine = Engine::from_workspace_root(root).unwrap();
        let (key_b, findings) = current_findings(&engine, root, binding, &resolved).unwrap();
        assert_ne!(
            key_b.source_head, head_a_outcome.key.source_head,
            "the head really moved"
        );
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].class, FindingClass::UnresolvableAnchor);
        assert_eq!(
            findings[0].key.source_head, head_a_outcome.key.source_head,
            "the finding still records the head it was observed at"
        );

        let brief = render_sync_brief_for(&engine, root, "engine/graph").unwrap();
        assert!(brief.contains("## Open findings to repair"));
        assert!(brief.contains("src/gone.rs"));
    }

    // The repair lands: src/gone.rs exists again (head C). A verify
    // observes the anchor clean → the finding closes…
    std::fs::write(root.join("src").join("gone.rs"), "fn g() {}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "head-c"]);
    {
        let engine = Engine::from_workspace_root(root).unwrap();
        verify_binding(&engine, root, binding, &resolved).unwrap();
    }
    // …and never re-presents (REFUSAL: resolved findings stay resolved).
    {
        let engine = Engine::from_workspace_root(root).unwrap();
        let (_key, findings) = current_findings(&engine, root, binding, &resolved).unwrap();
        assert!(
            findings
                .iter()
                .all(|f| f.class != FindingClass::UnresolvableAnchor),
            "the resolved orphan finding must not re-present: {findings:?}"
        );
    }
}

/// Prepared-hash backfill + deterministic drift, end-to-end over real git
/// heads and fresh engines:
///
/// 1. a hash-less `anchored`/`derived` anchor on a resolvable artifact is
///    backfilled by the first verify (once — a re-verify observes an empty
///    worklist and the recorded hash is never overwritten);
/// 2. after a source change, a subsequent verify adjudicates `drifted`
///    deterministically — no LLM sampling, no queued deferral;
/// 3. the tier-3 recheck queue for such anchors drains: post-backfill
///    clean passes queue nothing, instead of re-queueing forever.
///
/// The plain-TREE sibling of this lifecycle is
/// [`plain_tree_anchor_backfills_then_adjudicates_deterministically`].
///
/// REFUSAL half: `authored` / `informed-by` anchors never gain hashes and
/// never adjudicate `drifted`; an `unstable` hash-stability medium
/// resolves `recheck` (queued), never `drifted`.
#[test]
fn hashless_anchor_backfills_once_then_drift_adjudicates_deterministically() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
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
            root,
            &Workspace {
                mounts: vec![mount],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();

    // Git source tree at head A: two committed source files.
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("present.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("src").join("other.rs"), "fn o() {}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "head-a"]);

    // Anchors, all HASH-LESS: `anchored` (stable) + `derived` (stable) on
    // present.rs, `anchored` but UNSTABLE on other.rs, and the two
    // non-hash classes that must never gain a hash.
    let mk = |artifact: &str, class: AnchorProvenanceClass, stab: AnchorHashStability| Anchor {
        artifact: artifact.to_string(),
        grain: AnchorGrain::File,
        class,
        at_version: None,
        hash: None,
        hash_stability: stab,
        derived_from: if class == AnchorProvenanceClass::Derived {
            vec!["src/present.rs".to_string()]
        } else {
            Vec::new()
        },
        binding: None,
        source: None,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    };
    use AnchorHashStability::{Stable, Unstable};
    // The entity the sidecar is keyed to. Written, because it exists:
    // a row whose entity does not is DANGLING (consistency-sweep 03/02)
    // and leaves the population before any figure counts it.
    std::fs::write(
        mem_dir.join("e.md"),
        "---\ntype: decision\n---\n\n# E\n\n## Decision\n\nBody.\n",
    )
    .unwrap();
    let mut sidecar = AnchorSidecar::default();
    sidecar.set(
        "engine--e",
        vec![
            mk("src/present.rs", AnchorProvenanceClass::Anchored, Stable),
            mk("src/present.rs", AnchorProvenanceClass::Derived, Stable),
            mk("src/other.rs", AnchorProvenanceClass::Anchored, Unstable),
            mk("src/present.rs", AnchorProvenanceClass::Authored, Stable),
            mk("src/present.rs", AnchorProvenanceClass::InformedBy, Stable),
        ],
    );
    std::fs::write(
        mem_dir.join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
        sidecar.to_bytes(),
    )
    .unwrap();

    write_binding(
        root,
        "engine",
        "graph",
        &Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![memstead_base::pipeline::Source {
                name: "graph".to_string(),
                medium_type: MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
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
            prune: None,
            operations: Operations {
                build: None,
                sync: None,
                verify: Some(VerifyOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 20,
                    adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                    full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
                }),
            },
        },
    )
    .unwrap();

    let configs = load_pipeline_configs(root).unwrap();
    let binding = &configs.bindings[0].config;
    let resolved = resolve_binding_run("engine/graph", binding).unwrap();

    // --- Pass 1: first observation backfills, once. ---
    {
        let mut engine = Engine::from_workspace_root(root).unwrap();
        let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
        // Every hash-less hash-bearing anchor is on the worklist —
        // including the unstable one; the non-hash classes are not.
        let mut backfilled: Vec<(&str, &str)> = outcome
            .hash_backfill
            .iter()
            .map(|b| (b.entity.as_str(), b.artifact.as_str()))
            .collect();
        backfilled.sort();
        backfilled.dedup();
        assert_eq!(
            backfilled,
            vec![
                ("engine--e", "src/other.rs"),
                ("engine--e", "src/present.rs"),
            ],
            "hash-bearing anchors backfill; authored/informed-by never appear"
        );
        // Backfill candidates are clean-by-construction this pass —
        // nothing queued, nothing drifted (the recheck queue drains).
        assert_eq!(
            outcome.backlog, 0,
            "no recheck queue for backfilled anchors"
        );
        let store = read_findings_store(root, "engine", "graph")
            .unwrap()
            .unwrap();
        assert!(
            store
                .current(&outcome.key)
                .iter()
                .all(|f| !matches!(f.target, FindingTarget::Anchor { .. })),
            "no anchor finding on the backfill pass: {:?}",
            store.current(&outcome.key)
        );

        // The sanctioned post-run write records the hashes.
        let written = record_anchor_hash_backfill(&mut engine, "engine", &outcome, None).unwrap();
        assert_eq!(
            written, 3,
            "anchored + derived + unstable-anchored gain hashes"
        );
    }

    // The sidecar now carries the observed prepared-form hashes — and the
    // non-hash classes still carry none (class semantics preserved).
    let expected_present = memstead_base::anchor::prepared_content_hash(
        &std::fs::read(root.join("src").join("present.rs")).unwrap(),
    );
    {
        let sc = AnchorSidecar::from_bytes(
            &std::fs::read(mem_dir.join(memstead_base::anchor::ANCHOR_SIDECAR_PATH)).unwrap(),
        )
        .unwrap();
        for a in sc.get("engine--e") {
            if a.class.is_hash_bearing() {
                assert!(a.hash.is_some(), "hash-bearing anchor backfilled: {a:?}");
            } else {
                assert!(a.hash.is_none(), "non-hash class never gains a hash: {a:?}");
            }
            if a.artifact == "src/present.rs" && a.class.is_hash_bearing() {
                assert_eq!(a.hash.as_deref(), Some(expected_present.as_str()));
            }
        }
    }

    // --- Pass 2 (fresh engine): idempotent — nothing to backfill, clean. ---
    {
        let mut engine = Engine::from_workspace_root(root).unwrap();
        let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
        assert!(
            outcome.hash_backfill.is_empty(),
            "backfill happens once — a re-verify observes an empty worklist"
        );
        assert_eq!(outcome.backlog, 0, "steady state: nothing re-queues");
        let store = read_findings_store(root, "engine", "graph")
            .unwrap()
            .unwrap();
        assert!(
            store
                .current(&outcome.key)
                .iter()
                .all(|f| !matches!(f.target, FindingTarget::Anchor { .. })),
            "recorded hashes match the source — no anchor finding"
        );
        let written = record_anchor_hash_backfill(&mut engine, "engine", &outcome, None).unwrap();
        assert_eq!(written, 0, "no write, no commit on the idempotent pass");
    }

    // --- Source change: both anchored artifacts move (head B). ---
    std::fs::write(
        root.join("src").join("present.rs"),
        "fn a() { /* changed */ }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src").join("other.rs"),
        "fn o() { /* changed */ }\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "head-b"]);

    // --- Pass 3: deterministic adjudication — stable drifts, unstable
    //     rechecks, non-hash classes stay silent. ---
    {
        let engine = Engine::from_workspace_root(root).unwrap();
        let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
        assert!(
            outcome.hash_backfill.is_empty(),
            "recorded hashes are never overwritten by observation"
        );
        let store = read_findings_store(root, "engine", "graph")
            .unwrap()
            .unwrap();
        let current = store.current(&outcome.key);
        let drifted: Vec<&Finding> = current
            .iter()
            .filter(|f| f.class == FindingClass::Drifted)
            .collect();
        // The stable `anchored` + `derived` anchors on present.rs drift —
        // deterministically, from the hash comparison alone.
        assert_eq!(
            drifted.len(),
            2,
            "stable-medium mismatch → drifted: {current:?}"
        );
        assert!(drifted.iter().all(|f| matches!(
            &f.target,
            FindingTarget::Anchor { artifact, .. } if artifact == "src/present.rs"
        )));
        // REFUSAL: the unstable anchor on other.rs resolves recheck →
        // queued, never drifted.
        assert!(
            current
                .iter()
                .any(|f| f.class == FindingClass::QueuedForAdjudication
                    && matches!(
                        &f.target,
                        FindingTarget::Anchor { artifact, .. } if artifact == "src/other.rs"
                    )),
            "unstable medium resolves recheck (queued), not drifted: {current:?}"
        );
        assert!(
            !current.iter().any(|f| f.class == FindingClass::Drifted
                && matches!(
                    &f.target,
                    FindingTarget::Anchor { artifact, .. } if artifact == "src/other.rs"
                )),
            "an unstable hash break must never assert drift"
        );
    }
}

/// The plain-TREE sibling of the backfill lifecycle: a hash-less `tree`
/// anchor under NO preparation observes the plain per-file digest of its
/// scoped files, backfills once, and thereafter adjudicates
/// deterministically — `drifted` on any scoped-file byte change or a
/// file joining the tree, `resolves` when nothing moved. Before the
/// plain tree digest existed, such an anchor observed no hash at all and
/// re-issued `recheck` (queued-for-adjudication) on every pass, forever
/// — the loop this test seals shut.
#[test]
fn plain_tree_anchor_backfills_then_adjudicates_deterministically() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
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
    memstead_base::FileWorkspaceStore::new()
        .save_state(
            root,
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

    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("src").join("b.rs"), "fn b() {}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "head-a"]);

    std::fs::write(
        mem_dir.join("e.md"),
        "---\ntype: decision\n---\n\n# E\n\n## Decision\n\nBody.\n",
    )
    .unwrap();
    // One hash-less TREE anchor over `src`, carrying the declaring
    // source's NAME — the join is what scopes the enumeration, so a
    // tree anchor without a resolvable source stays honest `recheck`.
    let mut sidecar = AnchorSidecar::default();
    sidecar.set(
        "engine--e",
        vec![Anchor {
            artifact: "src".to_string(),
            grain: AnchorGrain::Tree,
            class: AnchorProvenanceClass::Anchored,
            at_version: None,
            hash: None,
            hash_stability: AnchorHashStability::Stable,
            derived_from: Vec::new(),
            binding: None,
            source: Some("graph".to_string()),
            span_unvalidated: false,
            hash_source: None,
            last_observed: None,
        }],
    );
    std::fs::write(
        mem_dir.join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
        sidecar.to_bytes(),
    )
    .unwrap();

    write_binding(
        root,
        "engine",
        "graph",
        &Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![memstead_base::pipeline::Source {
                name: "graph".to_string(),
                medium_type: MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
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
            prune: None,
            operations: Operations {
                build: None,
                sync: None,
                verify: Some(VerifyOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 20,
                    adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                    full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
                }),
            },
        },
    )
    .unwrap();

    let configs = load_pipeline_configs(root).unwrap();
    let binding = &configs.bindings[0].config;
    let resolved = resolve_binding_run("engine/graph", binding).unwrap();

    // --- Pass 1: the tree observes the plain digest and backfills. ---
    let expected_digest = memstead_base::anchor::prepared_content_hash(
        memstead_base::preparation::plain_tree_digest(&[
            ("src/a.rs".to_string(), b"fn a() {}\n".to_vec()),
            ("src/b.rs".to_string(), b"fn b() {}\n".to_vec()),
        ])
        .as_bytes(),
    );
    {
        let mut engine = Engine::from_workspace_root(root).unwrap();
        let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
        let backfilled: Vec<(&str, &str, &str)> = outcome
            .hash_backfill
            .iter()
            .map(|b| (b.entity.as_str(), b.artifact.as_str(), b.hash.as_str()))
            .collect();
        assert_eq!(
            backfilled,
            vec![("engine--e", "src", expected_digest.as_str())],
            "the tree anchor observes the plain digest and backfills"
        );
        assert_eq!(outcome.backlog, 0, "no recheck queue: the digest exists");
        let written = record_anchor_hash_backfill(&mut engine, "engine", &outcome, None).unwrap();
        assert_eq!(written, 1);
    }

    // --- Pass 2: idempotent and clean. ---
    {
        let engine = Engine::from_workspace_root(root).unwrap();
        let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
        assert!(outcome.hash_backfill.is_empty(), "backfill happens once");
        assert_eq!(outcome.backlog, 0, "steady state: nothing re-queues");
        let store = read_findings_store(root, "engine", "graph")
            .unwrap()
            .unwrap();
        assert!(
            store
                .current(&outcome.key)
                .iter()
                .all(|f| !matches!(f.target, FindingTarget::Anchor { .. })),
            "unchanged tree resolves clean: {:?}",
            store.current(&outcome.key)
        );
    }

    // --- A file JOINS the tree: the digest moves. ---
    std::fs::write(root.join("src").join("c.rs"), "fn c() {}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "head-b"]);

    // --- Pass 3: deterministic drift, no queued deferral. ---
    {
        let engine = Engine::from_workspace_root(root).unwrap();
        let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
        assert!(outcome.hash_backfill.is_empty());
        assert_eq!(outcome.backlog, 0, "drift is asserted, never queued");
        let store = read_findings_store(root, "engine", "graph")
            .unwrap()
            .unwrap();
        let current = store.current(&outcome.key);
        assert!(
            current.iter().any(|f| f.class == FindingClass::Drifted
                && matches!(
                    &f.target,
                    FindingTarget::Anchor { artifact, .. } if artifact == "src"
                )),
            "a joined file drifts the tree anchor deterministically: {current:?}"
        );
        assert!(
            !current
                .iter()
                .any(|f| f.class == FindingClass::QueuedForAdjudication),
            "the perpetual recheck loop is sealed: {current:?}"
        );
    }
}

/// The engine's backfill writer enforces the class guard at the write
/// seam: an `authored` / `informed-by` anchor never gains a hash even if
/// a (buggy or malicious) caller hands one in, and a recorded hash is
/// never overwritten.
#[test]
fn backfill_writer_refuses_non_hash_classes_and_never_overwrites() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
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
    memstead_base::FileWorkspaceStore::new()
        .save_state(
            root,
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

    let anchor = |class: AnchorProvenanceClass, hash: Option<&str>| Anchor {
        artifact: "src/a.rs".to_string(),
        grain: AnchorGrain::File,
        class,
        at_version: None,
        hash: hash.map(str::to_string),
        hash_stability: AnchorHashStability::Stable,
        derived_from: Vec::new(),
        binding: None,
        source: None,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    };
    // The entity the sidecar is keyed to. Written, because it exists:
    // a row whose entity does not is DANGLING (consistency-sweep 03/02)
    // and leaves the population before any figure counts it.
    std::fs::write(
        mem_dir.join("e.md"),
        "---\ntype: decision\n---\n\n# E\n\n## Decision\n\nBody.\n",
    )
    .unwrap();
    let mut sidecar = AnchorSidecar::default();
    sidecar.set(
        "engine--e",
        vec![
            anchor(AnchorProvenanceClass::Authored, None),
            anchor(AnchorProvenanceClass::InformedBy, None),
            anchor(AnchorProvenanceClass::Anchored, Some("recorded")),
        ],
    );
    std::fs::write(
        mem_dir.join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
        sidecar.to_bytes(),
    )
    .unwrap();

    let mut engine = Engine::from_workspace_root(root).unwrap();
    let written = engine
        .record_anchor_observed_hashes(
            "engine",
            &[memstead_base::anchor::ObservedArtifactHash {
                entity: "engine--e".to_string(),
                artifact: "src/a.rs".to_string(),
                hash: "observed".to_string(),
            }],
            None,
        )
        .unwrap();
    assert_eq!(
        written, 0,
        "non-hash classes refuse the hash; a recorded hash is never overwritten"
    );
    let sc = AnchorSidecar::from_bytes(
        &std::fs::read(mem_dir.join(memstead_base::anchor::ANCHOR_SIDECAR_PATH)).unwrap(),
    )
    .unwrap();
    for a in sc.get("engine--e") {
        match a.class {
            AnchorProvenanceClass::Anchored => {
                assert_eq!(a.hash.as_deref(), Some("recorded"), "baseline stands")
            }
            _ => assert!(a.hash.is_none(), "non-hash class stays hash-less: {a:?}"),
        }
    }
}

/// The completed-run `#verified` writer (backlog 2026-07-11): a verify
/// pass surfaces its observed facet heads on the outcome (the per-facet
/// decomposition of `key.source_head`), and [`record_verified_baseline`]
/// records them as `<binding>/<facet>#verified` through the engine's
/// sync-state writer — durable on disk, visible to the same config read
/// `report`/`status` consume. A failed pass returns
/// `Err` before any caller reaches the writer, so the token never
/// advances on an aborted run.
/// A vanished source directory must refuse verify with the typed
/// `SourceUnreachable` error instead of degrading to an empty
/// enumeration: pre-fix, the missing tree produced an empty stat map
/// whose aggregate (the digest of nothing) completed the run and let
/// the caller overwrite a genuine `#verified` baseline with fake
/// state. The engine mem itself stays loadable — only the binding's
/// source is gone.
#[test]
fn verify_refuses_unreachable_source_with_typed_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
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
            root,
            &Workspace {
                mounts: vec![mount],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();

    // The medium points at a subdirectory that does NOT exist — the
    // vanished-source case (`git` declared, so pre-fix the strategy
    // layer silently degraded instead of refusing).
    write_binding(
        root,
        "engine",
        "gone",
        &Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![memstead_base::pipeline::Source {
                name: "gone".to_string(),
                medium_type: MediumType::Codebase,
                pointer: "vanished-src".to_string(),
                change_detection: Some("git".to_string()),
                scope: vec![PatternEntry {
                    path: "**/*.rs".to_string(),
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
                build: None,
                sync: None,
                verify: Some(VerifyOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 20,
                    adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                    full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
                }),
            },
        },
    )
    .unwrap();

    let engine = Engine::from_workspace_root(root).unwrap();
    let configs = load_pipeline_configs(root).unwrap();
    let binding = &configs.bindings[0].config;
    let resolved = resolve_binding_run("engine/gone", binding).unwrap();

    match verify_binding(&engine, root, binding, &resolved) {
        Err(FindingsError::SourceUnreachable { source_name, path }) => {
            assert_eq!(source_name, "gone");
            assert!(
                path.ends_with("vanished-src"),
                "refusal must name the resolved missing path, got `{path}`",
            );
        }
        other => panic!("expected SourceUnreachable refusal, got {other:?}"),
    }

    // Nothing was observed → no `#verified` token exists (the caller
    // never reaches its baseline write on an Err).
    assert!(
        !engine
            .mem_config_for("engine")
            .unwrap()
            .sync_state
            .keys()
            .any(|k| k.ends_with("#verified")),
        "a refused verify must not leave any #verified token",
    );
}

#[test]
fn completed_verify_records_the_verified_baseline() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
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
            root,
            &Workspace {
                mounts: vec![mount],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();
    let out = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(out.status.success());

    write_binding(
        root,
        "engine",
        "graph",
        &Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![memstead_base::pipeline::Source {
                name: "graph".to_string(),
                medium_type: MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
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
        },
    )
    .unwrap();

    let mut engine = Engine::from_workspace_root(root).unwrap();
    // A recorded `#synced` baseline is this facet's current head (the git
    // work tree has no commits, so the cursor contributes no newer token).
    engine
        .set_mem_sync_state("engine", "engine/graph/graph#synced", "deadbeef", None)
        .unwrap();

    let configs = load_pipeline_configs(root).unwrap();
    let binding = &configs.bindings[0].config;
    let resolved = resolve_binding_run("engine/graph", binding).unwrap();

    let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
    // The outcome decomposes its own key: joined facet heads == source_head.
    assert_eq!(
        outcome.facet_heads.get("graph").map(String::as_str),
        Some("deadbeef")
    );
    assert_eq!(outcome.key.source_head, "graph=deadbeef");
    assert_eq!(
        join_facet_heads(&outcome.facet_heads),
        outcome.key.source_head
    );

    // No `#verified` token exists before the writer runs.
    assert!(
        !engine
            .mem_config_for("engine")
            .unwrap()
            .sync_state
            .contains_key("engine/graph/graph#verified")
    );

    let written = record_verified_baseline(&mut engine, "engine", &outcome, None).unwrap();
    assert_eq!(written, vec!["engine/graph/graph#verified".to_string()]);

    // Visible to the engine's config read (the app's sync_state source)…
    assert_eq!(
        engine
            .mem_config_for("engine")
            .unwrap()
            .sync_state
            .get("engine/graph/graph#verified")
            .map(String::as_str),
        Some("deadbeef")
    );
    // …and durable on disk (what a fresh CLI process reads).
    let disk: serde_json::Value = serde_json::from_slice(
        &std::fs::read(mem_dir.join(".memstead").join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        disk["syncState"]["engine/graph/graph#verified"],
        serde_json::json!("deadbeef")
    );
}

// ---- D1: per-run adjudication cap -----------------------------------

/// D1 — the per-run cap queues the remainder. A rotation window covering
/// only a subset of drift candidates adjudicates the in-window ones and
/// QUEUES every out-of-window candidate as `queued-for-adjudication` (the
/// tier-3 backlog). Uncapped (`window = None`) adjudicates every candidate.
#[test]
fn adjudication_cap_queues_the_remainder() {
    let k = key("h", "s");
    let mk = |art: &str| {
        let mut a = anchor(AnchorProvenanceClass::Anchored);
        a.artifact = art.to_string();
        a
    };
    let candidates = vec![
        (
            "engine--a".to_string(),
            mk("src/a.rs"),
            AnchorState::Drifted,
        ),
        (
            "engine--b".to_string(),
            mk("src/b.rs"),
            AnchorState::Drifted,
        ),
        (
            "engine--c".to_string(),
            mk("src/c.rs"),
            AnchorState::Drifted,
        ),
    ];
    // A cap-1 window selects only src/a.rs.
    let window: BTreeSet<String> = [candidate_key("engine--a", &mk("src/a.rs"))]
        .into_iter()
        .collect();
    let out = adjudicate_candidates(&k, "f", &candidates, Some(&window), "1");
    let drifted = out
        .iter()
        .filter(|f| f.class == FindingClass::Drifted)
        .count();
    let queued = out
        .iter()
        .filter(|f| f.class == FindingClass::QueuedForAdjudication)
        .count();
    assert_eq!(drifted, 1, "only the in-window candidate is adjudicated");
    assert_eq!(queued, 2, "the remainder is queued as the tier-3 backlog");
    // A queued remainder finding carries the queued detail, not a drift claim.
    assert!(
        out.iter()
            .any(|f| f.class == FindingClass::QueuedForAdjudication
                && f.detail.contains("cap reached")),
        "capped remainder states it was deferred by the cap"
    );

    // Uncapped: every candidate adjudicated, none queued.
    let uncapped = adjudicate_candidates(&k, "f", &candidates, None, "1");
    assert_eq!(
        uncapped
            .iter()
            .filter(|f| f.class == FindingClass::Drifted)
            .count(),
        3,
        "uncapped adjudicates every candidate"
    );
    assert_eq!(
        uncapped
            .iter()
            .filter(|f| f.class == FindingClass::QueuedForAdjudication)
            .count(),
        0
    );
}

// ---- D3: full_resync scheduling + non-enumerable refusal ------------

/// D3 — `schedule_full_resync`: disabled at cadence 0; not-due off-cadence
/// (with a countdown); due on-cadence for an enumerable facet (walked, no
/// refusal).
#[test]
fn full_resync_schedule_disabled_notdue_due() {
    let codebase = FacetEnumerability {
        facet: "src".to_string(),
        medium_type: "codebase".to_string(),
        enumerable: true,
    };
    assert_eq!(
        schedule_full_resync(0, 5, std::slice::from_ref(&codebase)),
        FullResyncDecision::Disabled
    );
    match schedule_full_resync(3, 2, std::slice::from_ref(&codebase)) {
        FullResyncDecision::NotDue { runs_until_due, .. } => assert_eq!(runs_until_due, 1),
        other => panic!("expected NotDue, got {other:?}"),
    }
    match schedule_full_resync(3, 3, std::slice::from_ref(&codebase)) {
        FullResyncDecision::Due {
            walked_facets,
            refused,
            ..
        } => {
            assert_eq!(walked_facets, vec!["src".to_string()]);
            assert!(refused.is_empty(), "enumerable facet is not refused");
        }
        other => panic!("expected Due, got {other:?}"),
    }
}

/// D3 REFUSAL — a scheduled full walk over a NON-enumerable medium refuses
/// with a typed signal: it never claims coverage and is never a silent skip.
#[test]
fn full_resync_refuses_non_enumerable_medium() {
    let web = FacetEnumerability {
        facet: "manual".to_string(),
        medium_type: "web".to_string(),
        enumerable: false,
    };
    let d = schedule_full_resync(1, 1, &[web]);
    assert!(
        d.is_full_walk(),
        "a due sweep is a full walk even when refused"
    );
    match d {
        FullResyncDecision::Due {
            walked_facets,
            refused,
            ..
        } => {
            assert!(walked_facets.is_empty(), "nothing enumerable to walk");
            assert_eq!(refused.len(), 1, "the non-enumerable facet is refused");
            assert_eq!(refused[0].facet, "manual");
            assert_eq!(refused[0].medium_type, "web");
            assert!(
                refused[0].reason.contains("non-enumerable"),
                "the refusal is typed and states why"
            );
        }
        other => panic!("expected Due with a refusal, got {other:?}"),
    }
}

/// D3 — a scheduled full walk fires the WHOLE-source enumeration this run:
/// with `full_resync_every = 1` (due every run) and a sample `batch_size` of
/// 1, all three uncovered source files are flagged, not just one — the full
/// walk overrides the bounded rotating sample for an enumerable medium.
#[test]
fn full_resync_full_walk_covers_whole_source() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
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
            root,
            &Workspace {
                mounts: vec![mount],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();
    let out = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(out.status.success());
    std::fs::create_dir_all(root.join("src")).unwrap();
    for f in ["a.rs", "b.rs", "c.rs"] {
        std::fs::write(root.join("src").join(f), "fn x() {}\n").unwrap();
    }

    write_binding(
        root,
        "engine",
        "graph",
        &Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![memstead_base::pipeline::Source {
                name: "graph".to_string(),
                medium_type: MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
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
                    batch_size: 1, // a tiny rotating sample …
                    adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                    full_resync_every: 1, // … but a full walk fires EVERY run
                }),
            },
        },
    )
    .unwrap();

    let engine = Engine::from_workspace_root(root).unwrap();
    let configs = load_pipeline_configs(root).unwrap();
    let binding = &configs.bindings[0].config;
    let resolved = resolve_binding_run("engine/graph", binding).unwrap();

    let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
    // The full walk is due on run 1 and covers the enumerable facet.
    match &outcome.full_resync {
        FullResyncDecision::Due {
            walked_facets,
            refused,
            run_count,
            ..
        } => {
            assert_eq!(*run_count, 1);
            assert_eq!(walked_facets, &vec!["graph".to_string()]);
            assert!(refused.is_empty());
        }
        other => panic!("expected a due full walk, got {other:?}"),
    }
    // All three uncovered files flagged despite the batch_size-1 sample.
    let store = read_findings_store(root, "engine", "graph")
        .unwrap()
        .unwrap();
    let uncovered = store
        .current(&outcome.key)
        .iter()
        .filter(|f| f.class == FindingClass::Uncovered)
        .count();
    assert_eq!(
        uncovered, 3,
        "the scheduled full walk covers the whole source, not a batch of one"
    );
}

/// A SCHEDULED full walk consults partiality the way `--full` does: a facet
/// whose enumeration is known-incomplete (here: a scope pattern still in
/// the retired workspace-relative dialect) is demoted into the typed
/// refusal list instead of being walked and announced as full. Without the
/// demotion one report carries both "full-enumeration walk fired" and
/// "`S(D)` is partial, no percentage".
#[test]
fn scheduled_full_walk_demotes_partial_facet_to_refusal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
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
            root,
            &Workspace {
                mounts: vec![mount],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();
    let out = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(out.status.success());
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("a.rs"), "fn x() {}\n").unwrap();

    write_binding(
        root,
        "engine",
        "graph",
        &Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![memstead_base::pipeline::Source {
                name: "graph".to_string(),
                medium_type: MediumType::Codebase,
                pointer: "src".to_string(),
                change_detection: Some("git".to_string()),
                // A MIXED scope: the prefix-free pattern still enumerates,
                // so the facet is non-empty and looks like a population —
                // while the retired-dialect pattern's share is absent.
                scope: vec![
                    PatternEntry {
                        path: "**/*.rs".to_string(),
                        mode: PatternMode::Allow,
                    },
                    PatternEntry {
                        path: "src/nested.rs".to_string(),
                        mode: PatternMode::Allow,
                    },
                ],
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
                    batch_size: 1,
                    adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                    full_resync_every: 1, // a full walk fires EVERY run …
                }),
            },
        },
    )
    .unwrap();

    let engine = Engine::from_workspace_root(root).unwrap();
    let configs = load_pipeline_configs(root).unwrap();
    let binding = &configs.bindings[0].config;
    let resolved = resolve_binding_run("engine/graph", binding).unwrap();

    let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
    match &outcome.full_resync {
        FullResyncDecision::Due {
            walked_facets,
            refused,
            ..
        } => {
            assert!(
                walked_facets.is_empty(),
                "a partial facet must not be announced as walked-in-full: {walked_facets:?}"
            );
            assert_eq!(refused.len(), 1, "the partial facet is refused, typed");
            assert_eq!(refused[0].facet, "graph");
            assert!(
                refused[0].reason.contains("incomplete"),
                "the refusal names the partiality: {}",
                refused[0].reason
            );
        }
        other => panic!("expected a due full walk decision, got {other:?}"),
    }
}

// ---- explicit full measurement (`verify_binding_full`) ----------------

/// An explicit full measurement walks the whole `S(D)` and treats the
/// adjudication cap as unlimited — every drift candidate adjudicates and
/// every uncovered artifact is flagged in ONE run, with nothing deferred
/// to a cap or a rotating sample, and the decision reports `Forced`.
/// REFUSAL half (byte-compat): a no-flag run over the same binding keeps
/// today's capped/sampled behavior exactly — cap-1 adjudicates one
/// candidate and queues the remainder with the cap-reached detail, and
/// the batch-1 sample flags at most one uncovered file.
#[test]
fn full_verify_uncaps_adjudication_and_walks_whole_source() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
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
    memstead_base::FileWorkspaceStore::new()
        .save_state(
            root,
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
    let out = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(out.status.success());
    std::fs::create_dir_all(root.join("src")).unwrap();
    // Three anchored (drift-candidate) files + three uncovered files.
    for f in ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs", "f.rs"] {
        std::fs::write(root.join("src").join(f), "fn x() {}\n").unwrap();
    }
    let mk = |art: &str| Anchor {
        artifact: art.to_string(),
        grain: AnchorGrain::File,
        class: AnchorProvenanceClass::Anchored,
        at_version: None,
        hash: Some("stale-recorded-hash".to_string()), // mismatches → drift candidate
        hash_stability: AnchorHashStability::Stable,
        derived_from: Vec::new(),
        binding: None,
        source: None,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    };
    // The entity the sidecar is keyed to. Written, because it exists:
    // a row whose entity does not is DANGLING (consistency-sweep 03/02)
    // and leaves the population before any figure counts it.
    std::fs::write(
        mem_dir.join("e.md"),
        "---\ntype: decision\n---\n\n# E\n\n## Decision\n\nBody.\n",
    )
    .unwrap();
    let mut sidecar = AnchorSidecar::default();
    sidecar.set(
        "engine--e",
        vec![mk("src/a.rs"), mk("src/b.rs"), mk("src/c.rs")],
    );
    std::fs::write(
        mem_dir.join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
        sidecar.to_bytes(),
    )
    .unwrap();

    write_binding(
        root,
        "engine",
        "graph",
        &Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![memstead_base::pipeline::Source {
                name: "graph".to_string(),
                medium_type: MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
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
            prune: None,
            operations: Operations {
                build: None,
                sync: None,
                verify: Some(VerifyOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 1,        // tiny rotating sample …
                    adjudication_cap: 1,  // … and a tiny cap
                    full_resync_every: 0, // scheduled walks disabled
                }),
            },
        },
    )
    .unwrap();

    let engine = Engine::from_workspace_root(root).unwrap();
    let configs = load_pipeline_configs(root).unwrap();
    let binding = &configs.bindings[0].config;
    let resolved = resolve_binding_run("engine/graph", binding).unwrap();

    // Byte-compat leg — the no-flag run keeps today's capped/sampled
    // economics: one candidate adjudicated, two queued by the cap, at
    // most one uncovered file from the batch-1 sample, no full walk.
    let sampled = verify_binding(&engine, root, binding, &resolved).unwrap();
    assert_eq!(sampled.full_resync, FullResyncDecision::Disabled);
    let store = read_findings_store(root, "engine", "graph")
        .unwrap()
        .unwrap();
    let current = store.current(&sampled.key);
    let count = |c: FindingClass| current.iter().filter(|f| f.class == c).count();
    assert_eq!(count(FindingClass::Drifted), 1, "cap-1 adjudicates one");
    assert_eq!(
        count(FindingClass::QueuedForAdjudication),
        2,
        "the remainder queues"
    );
    assert!(
        current
            .iter()
            .any(|f| f.class == FindingClass::QueuedForAdjudication
                && f.detail.contains("cap reached")),
        "the sampled deferral states the cap"
    );
    assert!(
        count(FindingClass::Uncovered) <= 1,
        "batch-1 sample looks at one artifact"
    );

    // Full measurement: everything adjudicates, everything is walked,
    // nothing deferred — no sampling/truncation residue anywhere.
    let full = verify_binding_full(&engine, root, binding, &resolved).unwrap();
    assert_eq!(
        full.full_resync,
        FullResyncDecision::Forced {
            walked_facets: vec!["graph".to_string()]
        }
    );
    assert_eq!(full.backlog, 0, "cap treated as unlimited — no backlog");
    let store = read_findings_store(root, "engine", "graph")
        .unwrap()
        .unwrap();
    let current = store.current(&full.key);
    let count = |c: FindingClass| current.iter().filter(|f| f.class == c).count();
    assert_eq!(
        count(FindingClass::Drifted),
        3,
        "every candidate adjudicated"
    );
    assert_eq!(count(FindingClass::QueuedForAdjudication), 0);
    assert_eq!(
        count(FindingClass::Uncovered),
        3,
        "the whole S(D) walked — every uncovered file flagged"
    );
    assert!(
        current.iter().all(|f| !f.detail.contains("cap reached")),
        "a full run's findings carry no cap-deferral caveat"
    );
}

/// REFUSAL — an explicit full measurement over a non-enumerable medium
/// refuses the whole run with the typed capability error (nothing
/// observed, nothing recorded — never a fabricated-complete report),
/// while the no-flag sampled verify over the same binding still runs.
#[test]
fn full_verify_refuses_non_enumerable_medium_typed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
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
    memstead_base::FileWorkspaceStore::new()
        .save_state(
            root,
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

    // A web medium — the capability matrix marks it non-enumerable.
    write_binding(
        root,
        "engine",
        "manual",
        &Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![memstead_base::pipeline::Source {
                name: "manual".to_string(),
                medium_type: MediumType::Web,
                pointer: "https://example.com/docs".to_string(),
                change_detection: None,
                scope: Vec::new(),
                engagement: None,
                preparation: None,
            }],
            reference_mems: Vec::new(),
            destination_mem: "engine".to_string(),
            deny_paths: Vec::new(),
            coverage_semantics: Some(CoverageSemantics::Curated),
            rules: None,
            prune: None,
            operations: Operations {
                build: None,
                sync: None,
                verify: Some(VerifyOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 20,
                    adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                    full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
                }),
            },
        },
    )
    .unwrap();

    let engine = Engine::from_workspace_root(root).unwrap();
    let configs = load_pipeline_configs(root).unwrap();
    let binding = &configs.bindings[0].config;
    let resolved = resolve_binding_run("engine/manual", binding).unwrap();

    // Full: typed refusal naming the facet and medium type; nothing recorded.
    let err = verify_binding_full(&engine, root, binding, &resolved).unwrap_err();
    match &err {
        FindingsError::FullWalkNonEnumerable(refusal) => {
            assert_eq!(refusal.facet, "manual");
            assert_eq!(refusal.medium_type, "web");
            assert!(refusal.reason.contains("non-enumerable"));
        }
        other => panic!("expected FullWalkNonEnumerable, got {other:?}"),
    }
    assert!(
        read_findings_store(root, "engine", "manual")
            .unwrap()
            .is_none(),
        "a refused full run records nothing"
    );

    // No-flag: the sampled verify over the same binding still runs.
    let sampled = verify_binding(&engine, root, binding, &resolved).unwrap();
    assert_eq!(sampled.binding, "engine/manual");
}

fn sourceless_binding() -> memstead_base::binding::Binding {
    memstead_base::binding::Binding {
        version: memstead_base::binding::BINDING_VERSION,
        intent: None,
        sources: Vec::new(),
        reference_mems: Vec::new(),
        destination_mem: "m".to_string(),
        deny_paths: Vec::new(),
        coverage_semantics: None,
        rules: None,
        prune: None,
        operations: memstead_base::binding::Operations {
            build: None,
            sync: None,
            verify: None,
        },
    }
}

fn uncovered(key: &FindingKey, artifact: &str) -> Finding {
    Finding {
        key: key.clone(),
        facet: "src".to_string(),
        target: FindingTarget::Artifact {
            artifact: artifact.to_string(),
        },
        class: FindingClass::Uncovered,
        detail: "source artifact in scope has no anchor in the destination mem".to_string(),
        created_at: "1".to_string(),
    }
}

/// An exclusion `projection exclude` just accepted takes effect on the
/// VERY NEXT brief read, with no verify pass in between: the stored batch
/// still carries the uncovered finding, and `current_findings` drops it
/// against the durable exclusion ledger. Non-uncovered findings and
/// uncovered artifacts the ledger does not name are untouched.
#[test]
fn current_findings_drops_ledger_excluded_uncovered_without_a_verify() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    let engine = memstead_base::engine::Engine::from_mounts(Vec::new()).unwrap();
    let binding = sourceless_binding();
    let resolved = resolve_binding_run("m/s", &binding).unwrap();

    let key = FindingKey {
        binding_hash: memstead_base::binding::hash_binding(&binding),
        source_head: String::new(),
    };
    let mut store = FindingsStore {
        binding: "m/s".to_string(),
        ..Default::default()
    };
    store.record(
        key.clone(),
        "1".to_string(),
        vec![uncovered(&key, "docs/a.md"), uncovered(&key, "docs/b.md")],
    );
    write_findings_store(root, "m", "s", &store).unwrap();

    // Before the exclusion: both present.
    let (_, before) = current_findings(&engine, root, &binding, &resolved).unwrap();
    assert_eq!(before.len(), 2);

    // The exclusion lands in the durable ledger (as `projection exclude`
    // records it) — no verify rewrites the batch.
    let state = crate::advance::AdvanceState {
        binding: "m/s".to_string(),
        exclusions: [("docs/a.md".to_string(), "generated; no entity".to_string())]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    crate::advance::write_advance_store(root, "m", "s", &state).unwrap();

    let (_, after) = current_findings(&engine, root, &binding, &resolved).unwrap();
    assert_eq!(after.len(), 1);
    assert!(matches!(
        &after[0].target,
        FindingTarget::Artifact { artifact } if artifact == "docs/b.md"
    ));
}

/// Findings recorded under a prior `hash(D)` are superseded and never
/// surface through `current_findings` — the brief renders the current
/// batch alone.
#[test]
fn current_findings_never_serves_superseded_batches() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    let engine = memstead_base::engine::Engine::from_mounts(Vec::new()).unwrap();
    let binding = sourceless_binding();
    let resolved = resolve_binding_run("m/s", &binding).unwrap();

    let old_key = key("a-prior-binding-hash", "head0");
    let cur_key = FindingKey {
        binding_hash: memstead_base::binding::hash_binding(&binding),
        source_head: String::new(),
    };
    let mut store = FindingsStore {
        binding: "m/s".to_string(),
        ..Default::default()
    };
    store.record(
        old_key.clone(),
        "1".to_string(),
        vec![uncovered(&old_key, "docs/stale.md")],
    );
    store.record(
        cur_key.clone(),
        "2".to_string(),
        vec![uncovered(&cur_key, "docs/live.md")],
    );
    write_findings_store(root, "m", "s", &store).unwrap();

    let (_, current) = current_findings(&engine, root, &binding, &resolved).unwrap();
    assert_eq!(current.len(), 1);
    assert!(matches!(
        &current[0].target,
        FindingTarget::Artifact { artifact } if artifact == "docs/live.md"
    ));
}

/// A workspace with one folder mem `engine` (default@1.0.0), a git source
/// tree at the root, and `files` written relative to the root. Returns the
/// root; the caller writes the binding.
fn a3_workspace(root: &std::path::Path, files: &[&str]) {
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
    let mount = memstead_base::workspace::Mount {
        mem: "engine".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::workspace::MountStorage::Folder {
            path: mem_dir.clone(),
        },
        capability: memstead_base::workspace::MountCapability::Write,
        lifecycle: memstead_base::workspace::MountLifecycle::Eager,
        cross_linkable: false,
        migration_target: None,
    };
    memstead_base::workspace_store::WorkspaceStoreAdapter::save_state(
        &memstead_base::FileWorkspaceStore::new(),
        root,
        &memstead_base::workspace::Workspace {
            mounts: vec![mount],
            settings: memstead_base::workspace::WorkspaceSettings::default(),
        },
    )
    .unwrap();
    let out = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(out.status.success());
    for f in files {
        let p = root.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "fn x() {}\n").unwrap();
    }
}

fn a3_binding(
    sources: &[(&str, &str)],
    deny: &[&str],
    batch_size: u32,
) -> memstead_base::binding::Binding {
    memstead_base::binding::Binding {
        version: memstead_base::binding::BINDING_VERSION,
        intent: None,
        sources: sources
            .iter()
            .map(|(name, glob)| memstead_base::pipeline::Source {
                name: name.to_string(),
                medium_type: memstead_base::pipeline::MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
                scope: vec![memstead_base::pipeline::PatternEntry {
                    path: glob.to_string(),
                    mode: memstead_base::pipeline::PatternMode::Allow,
                }],
                engagement: None,
                preparation: None,
            })
            .collect(),
        reference_mems: Vec::new(),
        destination_mem: "engine".to_string(),
        deny_paths: deny.iter().map(|d| d.to_string()).collect(),
        coverage_semantics: None,
        rules: None,
        prune: None,
        operations: memstead_base::binding::Operations {
            build: Some(memstead_base::binding::BuildOperation {
                mode: memstead_base::binding::BuildMode::Discovery,
                trigger: memstead_base::pipeline::IngestTrigger::Loop,
                batch_size,
                post_actions: None,
            }),
            sync: None,
            verify: Some(memstead_base::binding::VerifyOperation {
                trigger: memstead_base::pipeline::IngestTrigger::Manual,
                batch_size,
                adjudication_cap: memstead_base::binding::DEFAULT_ADJUDICATION_CAP,
                // Scheduled full walks off: the sampled path is under test.
                full_resync_every: 0,
            }),
        },
    }
}

/// A3 AC1: after `projection edit` adds a deny, every sampled verify
/// records zero `uncovered` findings naming a denied file across five
/// rotation windows while `S(D)` reads thirty; lifting the deny makes the
/// denied files present again (the filter follows the binding, not the
/// rotation cache).
#[test]
fn sampled_verify_records_nothing_for_denied_artifacts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut files: Vec<String> = (0..30).map(|i| format!("src/keep/k{i:02}.rs")).collect();
    files.extend((0..10).map(|i| format!("src/deny/d{i:02}.rs")));
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    a3_workspace(root, &refs);
    let engine = Engine::from_workspace_root(root).unwrap();

    let run = |binding: &Binding| -> (VerifyOutcome, Vec<String>) {
        let resolved = resolve_binding_run("engine/graph", binding).unwrap();
        let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
        let store = read_findings_store(root, "engine", "graph")
            .unwrap()
            .expect("store written");
        let uncovered: Vec<String> = store
            .current(&outcome.key)
            .iter()
            .filter(|f| f.class == FindingClass::Uncovered)
            .filter_map(|f| match &f.target {
                FindingTarget::Artifact { artifact } => Some(artifact.clone()),
                _ => None,
            })
            .collect();
        (outcome, uncovered)
    };

    // One sampled run over the whole set seeds the rotation (40 files,
    // window 8).
    let open = a3_binding(&[("graph", "src/**/*.rs")], &[], 8);
    write_binding(root, "engine", "graph", &open).unwrap();
    let (_, first) = run(&open);
    assert!(!first.is_empty(), "the sample records uncovered files");

    // The edit: deny the ten files.
    let denied = a3_binding(&[("graph", "src/**/*.rs")], &["src/deny/**"], 8);
    write_binding(root, "engine", "graph", &denied).unwrap();
    let resolved = resolve_binding_run("engine/graph", &denied).unwrap();
    let ResolvedSource::Primary(p) = &resolved.sources[0] else {
        panic!("primary source")
    };
    assert_eq!(
        enumerate_source_artifacts(&engine, p, &resolved.deny_paths, root).len(),
        30,
        "the denominator honours the deny"
    );
    for pass in 0..5 {
        let (_, uncovered) = run(&denied);
        assert!(
            uncovered.iter().all(|a| !a.starts_with("src/deny/")),
            "pass {pass} recorded a denied artifact: {uncovered:?}"
        );
    }

    // Lifting the deny: the files come back into the sample.
    write_binding(root, "engine", "graph", &open).unwrap();
    let mut seen_denied = false;
    for _ in 0..8 {
        let (_, uncovered) = run(&open);
        if uncovered.iter().any(|a| a.starts_with("src/deny/")) {
            seen_denied = true;
            break;
        }
    }
    assert!(
        seen_denied,
        "with the deny lifted the files are sampled again"
    );
}
