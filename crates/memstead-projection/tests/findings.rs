//! The findings store at the crate boundary: a verify pass over a real
//! workspace records findings a fresh process reads back, a changed
//! binding supersedes the prior batch, an authored exclusion drops an
//! uncovered finding from the current view without a verify, and a
//! completed verify records the baseline the due-check reads.

mod common;

use std::collections::BTreeMap;

use memstead_base::Engine;
use memstead_base::anchor::AnchorProvenanceClass;
use memstead_projection::{
    FindingClass, FindingTarget, current_findings, read_findings_store, record_exclusions,
    record_verified_baseline, source_moved_since, verify_binding,
};

use common::{FACET, MEM, anchor, workspace, write_anchors, write_entity, write_source};

/// One entity anchored on a present file with a stale recorded hash, on
/// a vanished file, and (informed-by) on the present file again; one
/// source file nothing anchors.
fn seeded() -> common::Fixture {
    let fixture = workspace();
    write_source(fixture.root(), "src/present.rs", "fn a() {}\n");
    write_source(fixture.root(), "src/uncovered.rs", "fn b() {}\n");
    write_entity(&fixture, "e", "E", "Body.");
    write_anchors(
        &fixture,
        "engine--e",
        vec![
            anchor(
                "src/present.rs",
                AnchorProvenanceClass::Anchored,
                Some("stale"),
            ),
            anchor(
                "src/gone.rs",
                AnchorProvenanceClass::Anchored,
                Some("stale"),
            ),
            anchor("src/present.rs", AnchorProvenanceClass::InformedBy, None),
        ],
    );
    fixture
}

fn classes(findings: &[memstead_projection::Finding]) -> Vec<(FindingClass, String)> {
    let mut out: Vec<(FindingClass, String)> = findings
        .iter()
        .map(|f| {
            let target = match &f.target {
                FindingTarget::Anchor { entity, artifact } => format!("{entity}:{artifact}"),
                FindingTarget::Artifact { artifact } => artifact.clone(),
                FindingTarget::Mention {
                    entity, artifact, ..
                } => format!("{entity}~{artifact}"),
            };
            (f.class, target)
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.as_wire().cmp(b.0.as_wire())));
    out
}

#[test]
fn a_verify_pass_records_findings_a_fresh_read_serves_as_current() {
    let fixture = seeded();
    let (binding, resolved) = common::write_graph_binding(&fixture, &["src/**/*.rs"]);
    let engine = Engine::from_workspace_root(fixture.root()).unwrap();

    let outcome = verify_binding(&engine, fixture.root(), &binding, &resolved).unwrap();
    assert_eq!(outcome.binding, common::BINDING);
    assert_eq!(outcome.superseded, 0, "no prior key");
    assert_eq!(outcome.backlog, 0, "nothing queued under the default cap");
    assert_eq!(
        outcome.recorded, 3,
        "drifted, unresolvable, uncovered: one each"
    );

    // A fresh read from disk, the way a later process reads it.
    let store = read_findings_store(fixture.root(), MEM, FACET)
        .unwrap()
        .expect("the pass persisted a store");
    assert_eq!(store.binding, common::BINDING);
    let current = store.current(&outcome.key);
    assert_eq!(
        classes(current),
        vec![
            (
                FindingClass::UnresolvableAnchor,
                "engine--e:src/gone.rs".to_string()
            ),
            (
                FindingClass::Drifted,
                "engine--e:src/present.rs".to_string()
            ),
            (FindingClass::Uncovered, "src/uncovered.rs".to_string()),
        ]
    );
    assert!(store.superseded(&outcome.key).is_empty());
    for f in current {
        assert_eq!(
            f.key, outcome.key,
            "every finding names the key it was recorded under"
        );
        assert_eq!(f.facet, FACET);
    }

    // The read-only current view agrees with the store.
    let (key, served) = current_findings(&engine, fixture.root(), &binding, &resolved).unwrap();
    assert_eq!(key, outcome.key);
    assert_eq!(classes(&served), classes(current));
}

#[test]
fn a_changed_binding_supersedes_the_prior_batch_without_deleting_it() {
    let fixture = seeded();
    let (binding, resolved) = common::write_graph_binding(&fixture, &["src/**/*.rs"]);
    let engine = Engine::from_workspace_root(fixture.root()).unwrap();
    let first = verify_binding(&engine, fixture.root(), &binding, &resolved).unwrap();
    assert_eq!(first.recorded, 3);

    // A narrower scope is a different declaration: hash(D) changes, and
    // the anchor on the vanished file falls outside the scope, so the
    // stale hash on the present file is the only finding left.
    let (narrow, narrow_resolved) = common::write_graph_binding(&fixture, &["src/present.rs"]);
    let second = verify_binding(&engine, fixture.root(), &narrow, &narrow_resolved).unwrap();
    assert_ne!(second.key.binding_hash, first.key.binding_hash);
    assert_eq!(second.superseded, 3, "the whole prior batch is segregated");

    let store = read_findings_store(fixture.root(), MEM, FACET)
        .unwrap()
        .unwrap();
    assert_eq!(
        classes(store.current(&second.key)),
        vec![(
            FindingClass::Drifted,
            "engine--e:src/present.rs".to_string()
        ),]
    );
    let superseded: Vec<_> = store.superseded(&second.key).into_iter().cloned().collect();
    assert_eq!(classes(&superseded), classes(store.current(&first.key)));
    let (_, served) = current_findings(&engine, fixture.root(), &narrow, &narrow_resolved).unwrap();
    assert_eq!(
        served.len(),
        1,
        "the current view never serves a superseded batch"
    );
}

#[test]
fn an_authored_exclusion_drops_the_uncovered_finding_from_the_current_view_at_once() {
    let fixture = seeded();
    let (binding, resolved) = common::write_graph_binding(&fixture, &["src/**/*.rs"]);
    let engine = Engine::from_workspace_root(fixture.root()).unwrap();
    let outcome = verify_binding(&engine, fixture.root(), &binding, &resolved).unwrap();
    assert_eq!(outcome.recorded, 3);

    let mut exclusions = BTreeMap::new();
    exclusions.insert(
        "src/uncovered.rs".to_string(),
        "mined; warrants no entity".to_string(),
    );
    let excluded = record_exclusions(&engine, fixture.root(), &resolved, &exclusions).unwrap();
    assert_eq!(excluded.added, 1);

    // The stored batch is untouched; the current view consults the
    // ledger and drops the finding without a second verify.
    let store = read_findings_store(fixture.root(), MEM, FACET)
        .unwrap()
        .unwrap();
    assert_eq!(store.current(&outcome.key).len(), 3);
    let (_, served) = current_findings(&engine, fixture.root(), &binding, &resolved).unwrap();
    assert_eq!(
        classes(&served),
        vec![
            (
                FindingClass::UnresolvableAnchor,
                "engine--e:src/gone.rs".to_string()
            ),
            (
                FindingClass::Drifted,
                "engine--e:src/present.rs".to_string()
            ),
        ]
    );

    // An artifact outside the source scope refuses the whole call.
    let mut outside = BTreeMap::new();
    outside.insert("docs/nowhere.md".to_string(), "not in scope".to_string());
    assert!(record_exclusions(&engine, fixture.root(), &resolved, &outside).is_err());
}

#[test]
fn a_completed_verify_records_the_baseline_the_due_check_reads() {
    let fixture = workspace();
    write_source(fixture.root(), "src/a.rs", "fn a() {}\n");
    let head = common::commit_all(fixture.root(), "base");
    let (binding, resolved) = common::write_graph_binding(&fixture, &["src/**/*.rs"]);
    let mut engine = Engine::from_workspace_root(fixture.root()).unwrap();

    // Never verified: the first verify is due.
    assert!(source_moved_since(
        &engine,
        &resolved,
        fixture.root(),
        "verified",
        true
    ));

    let outcome = verify_binding(&engine, fixture.root(), &binding, &resolved).unwrap();
    assert_eq!(
        outcome.facet_heads.get(FACET).map(String::as_str),
        Some(head.as_str()),
        "the facet head is the work tree's HEAD"
    );
    assert_eq!(outcome.key.source_head, format!("{FACET}={head}"));
    assert!(
        !engine
            .mem_config_for(MEM)
            .unwrap()
            .sync_state
            .contains_key(&common::verified_key()),
        "the pass itself writes nothing to the mem"
    );

    let written = record_verified_baseline(&mut engine, MEM, &outcome, None).unwrap();
    assert_eq!(written, vec![common::verified_key()]);
    assert_eq!(
        engine
            .mem_config_for(MEM)
            .unwrap()
            .sync_state
            .get(&common::verified_key())
            .map(String::as_str),
        Some(head.as_str())
    );
    assert!(
        !source_moved_since(&engine, &resolved, fixture.root(), "verified", true),
        "verified at HEAD: not due"
    );

    // The baseline is durable: a fresh engine over the same workspace
    // reads it, and a new commit makes the verify due again.
    let engine = Engine::from_workspace_root(fixture.root()).unwrap();
    assert!(!source_moved_since(
        &engine,
        &resolved,
        fixture.root(),
        "verified",
        true
    ));
    write_source(fixture.root(), "src/b.rs", "fn b() {}\n");
    common::commit_all(fixture.root(), "move");
    assert!(source_moved_since(
        &engine,
        &resolved,
        fixture.root(),
        "verified",
        true
    ));
}
