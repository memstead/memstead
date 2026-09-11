#![cfg(test)]

use indexmap::IndexMap;
use tempfile::TempDir;

use crate::backend::MemBackend;
use crate::engine::{CreateEntityArgs, Engine, RelateAction, RelateEntityArgs, UpdateEntityArgs};
use crate::ops::WarningHint;
use crate::storage::FilesystemBackend;
use crate::vcs::Actor;
use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

/// A schema whose `DERIVED_FROM` declares `derivation: true` and
/// whose `SUPPORTS` does not — the paired fixture every assertion
/// here contrasts.
fn deriv_schema() -> std::sync::Arc<memstead_schema::Schema> {
    let manifest = r#"name: deriv
version: 0.1.0
description: derivation fixture
when_to_use: tests
types:
  - note
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: DERIVED_FROM
      description: source derives from target
      default_weight: 2.0
      derivation: true
    - name: SUPPORTS
      description: plain edge
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let type_yaml = r#"name: note
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;
    std::sync::Arc::new(
        memstead_schema::loader::load_schema_from_memory(
            manifest,
            &[("note".to_string(), type_yaml.to_string())],
        )
        .expect("fixture schema loads"),
    )
}

fn engine_at(tmp: &TempDir) -> Engine {
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mount = Mount {
        mem: "m".to_string(),
        schema: Some("deriv@0.1.0".parse().unwrap()),
        storage: MountStorage::Folder { path: mem_dir },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    Engine::from_mounts_with_schemas_dir_and_extra(
        vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
        None,
        vec![deriv_schema()],
    )
    .unwrap()
}

fn note(title: &str, body: &str) -> CreateEntityArgs {
    CreateEntityArgs {
        anchors: Vec::new(),
        mem: "m".to_string(),
        title: title.to_string(),
        entity_type: "note".to_string(),
        sections: IndexMap::from_iter([("body".to_string(), body.to_string())]),
        metadata: IndexMap::new(),
        relations: Vec::new(),
        dry_run: false,
    }
}

fn relate_args(from: &crate::EntityId, rel: &str, to: &crate::EntityId) -> RelateEntityArgs {
    RelateEntityArgs {
        source: from.clone(),
        expected_hash: None,
        rel_type: rel.to_string(),
        target: to.clone(),
        remove: false,
        description: None,
        dry_run: false,
    }
}

/// Criterion 1's fixture, one flow: write → edit target → report
/// stale → re-assert → clear, with the refresh STATED. Plus the
/// undeclared-rel-type no-op complement and the
/// source-edit / markdown-invisibility complements.
#[test]
fn derivation_write_edit_report_reassert_clear() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_at(&tmp);
    let target = engine
        .create_entity(note("Target", "v1"), Actor::Cli, None, None)
        .unwrap();
    let source = engine
        .create_entity(note("Source", "conclusion"), Actor::Cli, None, None)
        .unwrap();

    // Write the derivation edge — baseline recorded, report clean.
    let added = engine
        .relate_entity(
            relate_args(&source.id, "DERIVED_FROM", &target.id),
            Actor::Cli,
            None,
            None,
        )
        .unwrap();
    assert_eq!(added.action, RelateAction::Added);
    assert!(
        engine.derivation_report("m").unwrap().is_empty(),
        "freshly baselined edge must not report"
    );

    // Edit the TARGET → the edge reports stale, naming all three.
    let t_now = engine.get_entity(&target.id).unwrap().content_hash.clone();
    engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: target.id.clone(),
                expected_hash: Some(t_now),
                sections: IndexMap::from_iter([("body".to_string(), "v2".to_string())]),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            Actor::Cli,
            None,
            None,
        )
        .unwrap();
    let report = engine.derivation_report("m").unwrap();
    assert_eq!(report.len(), 1, "{report:?}");
    assert_eq!(report[0].source, source.id);
    assert_eq!(report[0].rel_type, "DERIVED_FROM");
    assert_eq!(report[0].target, target.id);
    assert_eq!(report[0].state, "stale");
    assert!(report[0].baseline.is_some());

    // Editing the SOURCE does not mark its own derivation stale
    // (already covered: the edit above touched only the target;
    // now touch the source and assert the report is unchanged in
    // meaning — still exactly the one stale edge).
    let s_now = engine.get_entity(&source.id).unwrap().content_hash.clone();
    engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: source.id.clone(),
                expected_hash: Some(s_now),
                sections: IndexMap::from_iter([("body".to_string(), "conclusion v2".to_string())]),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            Actor::Cli,
            None,
            None,
        )
        .unwrap();
    let report = engine.derivation_report("m").unwrap();
    assert_eq!(report.len(), 1, "source edit adds nothing: {report:?}");
    assert_eq!(report[0].state, "stale");

    // Re-assert: duplicate-add refreshes the baseline as its ONE
    // effect — action noop, `_hash` unchanged, markdown
    // byte-identical, the refresh STATED, a real commit sha.
    let md_before = std::fs::read(tmp.path().join("source.md")).unwrap();
    let hash_before = engine.get_entity(&source.id).unwrap().content_hash.clone();
    let refreshed = engine
        .relate_entity(
            relate_args(&source.id, "DERIVED_FROM", &target.id),
            Actor::Cli,
            None,
            None,
        )
        .unwrap();
    assert_eq!(refreshed.action, RelateAction::NoOpAlreadyPresent);
    assert_eq!(refreshed.content_hash, hash_before, "_hash unchanged");
    assert!(
        !refreshed.write_id.is_empty(),
        "the sidecar refresh persists via a real commit"
    );
    assert!(
        refreshed
            .warnings
            .iter()
            .any(|w| matches!(w, WarningHint::DerivationBaselineRefreshed { .. })),
        "the refresh is STATED, never a bare no-op: {:?}",
        refreshed.warnings
    );
    let md_after = std::fs::read(tmp.path().join("source.md")).unwrap();
    assert_eq!(md_before, md_after, "baselines never touch the markdown");
    assert!(
        engine.derivation_report("m").unwrap().is_empty(),
        "re-assert clears the staleness"
    );

    // Undeclared rel-type: duplicate-add keeps today's EXACT
    // no-op — empty write_id, no refresh warning.
    engine
        .relate_entity(
            relate_args(&source.id, "SUPPORTS", &target.id),
            Actor::Cli,
            None,
            None,
        )
        .unwrap();
    let noop = engine
        .relate_entity(
            relate_args(&source.id, "SUPPORTS", &target.id),
            Actor::Cli,
            None,
            None,
        )
        .unwrap();
    assert_eq!(noop.action, RelateAction::NoOpAlreadyPresent);
    assert!(noop.write_id.is_empty(), "undeclared no-op stays bare");
    assert!(
        !noop
            .warnings
            .iter()
            .any(|w| matches!(w, WarningHint::DerivationBaselineRefreshed { .. })),
    );
    // And undeclared edges never enter the axis: still empty.
    assert!(engine.derivation_report("m").unwrap().is_empty());
}

/// A derivation edge with NO recorded baseline (pre-declaration
/// legacy, simulated by removing the sidecar out of band) reports
/// `unbaselined` — distinct from both fresh and stale, never
/// fabricated. The sidecar file itself never lists as an entity.
#[test]
fn missing_baseline_reports_unbaselined_never_fabricated() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_at(&tmp);
    let target = engine
        .create_entity(note("Target", "v1"), Actor::Cli, None, None)
        .unwrap();
    let source = engine
        .create_entity(note("Source", "conclusion"), Actor::Cli, None, None)
        .unwrap();
    engine
        .relate_entity(
            relate_args(&source.id, "DERIVED_FROM", &target.id),
            Actor::Cli,
            None,
            None,
        )
        .unwrap();
    // The sidecar exists on disk but is not an entity.
    let sidecar = tmp.path().join(".memstead").join("derivations.json");
    assert!(sidecar.exists(), "baseline persisted to the sidecar");
    assert!(
        engine
            .get_entity(&crate::EntityId::new("m", "derivations"))
            .is_none()
    );

    // Simulate a pre-declaration edge: drop the sidecar out of
    // band (fixture surgery, the mounts.json precedent).
    std::fs::remove_file(&sidecar).unwrap();
    let report = engine.derivation_report("m").unwrap();
    assert_eq!(report.len(), 1, "{report:?}");
    assert_eq!(report[0].state, "unbaselined");
    assert!(report[0].baseline.is_none());
}
