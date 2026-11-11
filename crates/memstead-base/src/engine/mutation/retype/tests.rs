use std::collections::BTreeMap;
use std::path::Path;

use indexmap::IndexMap;
use memstead_schema::SchemaRef;
use memstead_schema::workspace_config::CrossLinkValue;
use tempfile::TempDir;

use crate::backend::MemBackend;
use crate::engine::outcomes::{RetypeEdgeDirection, RetypeProblem};
use crate::engine::test_helpers::*;
use crate::engine::{CreateEntityArgs, Engine, EngineError, RelateEntityArgs, RetypeEntityArgs};
use crate::ops::WarningHint;
use crate::storage::FilesystemBackend;
use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage, WorkspaceSettings};

const MAIN_MANIFEST: &str = r#"name: rt
version: 0.1.0
description: retype fixture
when_to_use: tests
types:
  - claim
  - finding
relationships:
  mode: strict
  definitions:
    - name: SUPPORTS
      description: pinned both ends to claim
      default_weight: 1.0
      source_types: [claim]
      target_types: [claim]
    - name: CITES
      description: unpinned
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;

const PEER_MANIFEST: &str = r#"name: rt-peer
version: 0.1.0
description: peer fixture
when_to_use: tests
types:
  - remark
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
cross_mem_relationships:
  - to_schema: rt
    definitions:
      - name: COMMENTS_ON
        description: pinned to claim on the target side
        default_weight: 1.0
        source_types: [remark]
        target_types: [claim]
community:
  resolution: 1.0
  seed: 42
"#;

fn type_yaml(name: &str, main_section: &str, main_heading: &str) -> String {
    format!(
        r#"name: {name}
description: t
when_to_use: Here
sections:
  - key: {main_section}
    heading: {main_heading}
    required: true
    search_weight: 10.0
    write_rules: []
  - key: notes
    heading: Notes
    required: false
    search_weight: 1.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - {main_section}
hierarchy_relationship: _default
no_self_loop_relationships: []
updatable_fields:
  - title
  - {main_section}
  - notes
health_required_fields:
  - {main_section}
staleness_threshold_days: 90
write_rules: []
"#
    )
}

fn write_schema(root: &Path, name: &str, manifest: &str, types: &[(&str, String)]) {
    let dir = root.join(name);
    std::fs::create_dir_all(dir.join("types")).unwrap();
    std::fs::write(dir.join("schema.yaml"), manifest).unwrap();
    for (t, body) in types {
        std::fs::write(dir.join("types").join(format!("{t}.yaml")), body).unwrap();
    }
}

fn mount(mem: &str, path: std::path::PathBuf, pin: SchemaRef, lifecycle: MountLifecycle) -> Mount {
    Mount {
        mem: mem.to_string(),
        schema: Some(pin),
        storage: MountStorage::Folder { path },
        capability: MountCapability::Write,
        lifecycle,
        cross_linkable: true,
        migration_target: None,
    }
}

struct Fixture {
    _tmp: TempDir,
    schemas_dir: std::path::PathBuf,
    main_dir: std::path::PathBuf,
    peer_dir: std::path::PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let schemas_dir = tmp.path().join("schemas");
    std::fs::create_dir_all(&schemas_dir).unwrap();
    write_schema(
        &schemas_dir,
        "rt",
        MAIN_MANIFEST,
        &[
            ("claim", type_yaml("claim", "statement", "Statement")),
            ("finding", type_yaml("finding", "conclusion", "Conclusion")),
        ],
    );
    write_schema(
        &schemas_dir,
        "rt-peer",
        PEER_MANIFEST,
        &[("remark", type_yaml("remark", "body", "Body"))],
    );
    let main_dir = tmp.path().join("mem-main");
    let peer_dir = tmp.path().join("mem-peer");
    std::fs::create_dir_all(&main_dir).unwrap();
    std::fs::create_dir_all(&peer_dir).unwrap();
    Fixture {
        _tmp: tmp,
        schemas_dir,
        main_dir,
        peer_dir,
    }
}

fn boot(f: &Fixture, peer_lifecycle: MountLifecycle) -> Engine {
    let main_pin = SchemaRef::new("rt", semver::Version::new(0, 1, 0));
    let peer_pin = SchemaRef::new("rt-peer", semver::Version::new(0, 1, 0));
    let mut engine = Engine::from_mounts_with_schemas_dir(
        vec![
            (
                mount("main", f.main_dir.clone(), main_pin, MountLifecycle::Eager),
                Box::new(FilesystemBackend::new(f.main_dir.clone())) as Box<dyn MemBackend>,
            ),
            (
                mount("peer", f.peer_dir.clone(), peer_pin, peer_lifecycle),
                Box::new(FilesystemBackend::new(f.peer_dir.clone())) as Box<dyn MemBackend>,
            ),
        ],
        Some(&f.schemas_dir),
    )
    .expect("engine boots");
    let mut settings = WorkspaceSettings::default();
    let mut links: BTreeMap<String, CrossLinkValue> = BTreeMap::new();
    links.insert("peer".to_string(), CrossLinkValue::Wildcard);
    settings.cross_mem_links = links;
    engine.set_settings(settings);
    engine
}

fn create(engine: &mut Engine, mem: &str, title: &str, ty: &str, section: &str) -> crate::EntityId {
    let (actor, client) = cli_actor();
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: mem.to_string(),
                title: title.to_string(),
                entity_type: ty.to_string(),
                sections: IndexMap::from_iter([
                    (section.to_string(), format!("{title} says so.")),
                    ("notes".to_string(), "Some notes.".to_string()),
                ]),
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .expect("creates")
        .id
}

fn relate(engine: &mut Engine, from: &crate::EntityId, rel: &str, to: &crate::EntityId) {
    let (actor, client) = cli_actor();
    engine
        .relate_entity(
            RelateEntityArgs {
                source: from.clone(),
                expected_hash: None,
                rel_type: rel.to_string(),
                target: to.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .expect("relates");
}

fn retype(
    engine: &mut Engine,
    id: &crate::EntityId,
    target: &str,
    map: &[(&str, &str)],
) -> Result<crate::engine::RetypeEntityOutcome, EngineError> {
    let (actor, client) = cli_actor();
    let hash = engine.get_entity(id).unwrap().content_hash.clone();
    engine.retype_entity(
        RetypeEntityArgs {
            id: id.clone(),
            expected_hash: crate::ExpectedHash::Current(hash),
            target_type: target.to_string(),
            section_map: map
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            drop_metadata: Vec::new(),
            dry_run: false,
        },
        actor,
        Some(&client),
        Some("test retype"),
    )
}

fn file_bytes(f: &Fixture, id: &crate::EntityId) -> Vec<u8> {
    std::fs::read(f.main_dir.join(format!("{}.md", id.name()))).unwrap()
}

/// The type changes in place with the mapped section; id, path and
/// incoming edges stay; the response says checks are stale; a fresh
/// boot loads the result with no shape drops and the same edge count.
#[test]
fn retype_keeps_identity_and_edges_and_states_staleness() {
    let f = fixture();
    let mut engine = boot(&f, MountLifecycle::Eager);
    let a = create(&mut engine, "main", "Claim A", "claim", "statement");
    let b = create(&mut engine, "main", "Claim B", "claim", "statement");
    // Unpinned incoming edge onto B, and an unpinned outgoing one.
    relate(&mut engine, &a, "CITES", &b);
    relate(&mut engine, &b, "CITES", &a);
    let edges_before = engine.store().incoming(&b).len() + engine.store().outgoing(&b).len();
    let hash_before = engine.get_entity(&b).unwrap().content_hash.clone();

    let out = retype(&mut engine, &b, "finding", &[("statement", "conclusion")])
        .expect("retype succeeds");
    assert_eq!(out.id, b);
    assert_eq!(out.file_path, "claim-b.md");
    assert_eq!(
        (out.old_type.as_str(), out.new_type.as_str()),
        ("claim", "finding")
    );
    assert_eq!(
        out.sections_renamed,
        vec![("statement".to_string(), "conclusion".to_string())]
    );
    assert_eq!(out.edges_rechecked, 2);
    assert!(out.checks_stale);
    assert!(
        out.staleness_note
            .contains("check records and derivation baselines")
    );
    assert!(out.staleness_note.contains(&hash_before));
    assert!(!out.write_id.is_empty());

    let e = engine.get_entity(&b).unwrap();
    assert_eq!(
        e.entity_type,
        "finding",
        "store entity after retype: {e:?}\nfile: {}",
        String::from_utf8(file_bytes(&f, &b)).unwrap()
    );
    assert_eq!(e.file_path, "claim-b.md");
    assert_eq!(
        e.sections.get("conclusion").map(String::as_str),
        Some("Claim B says so.")
    );
    assert!(!e.sections.contains_key("statement"));
    assert_ne!(e.content_hash, hash_before);
    assert_eq!(engine.store().incoming(&b).len(), 1, "incoming edge stays");
    assert_eq!(engine.store().incoming(&b)[0].from, a);
    let text = String::from_utf8(file_bytes(&f, &b)).unwrap();
    assert!(text.contains("type: finding"), "{text}");
    assert!(text.contains("## Conclusion"), "{text}");

    // Restart: nothing dropped, edge count unchanged.
    let fresh = boot(&f, MountLifecycle::Eager);
    let dropped = fresh
        .load_warnings()
        .iter()
        .any(|w| matches!(w, WarningHint::ParsedRelationInvalid { .. }));
    assert!(!dropped, "{:?}", fresh.load_warnings());
    assert_eq!(
        fresh.store().incoming(&b).len() + fresh.store().outgoing(&b).len(),
        edges_before
    );
    assert_eq!(
        fresh.get_entity(&b).unwrap().entity_type,
        "finding",
        "warnings: {:?}\nfile: {}",
        fresh.load_warnings(),
        String::from_utf8(file_bytes(&f, &b)).unwrap()
    );
}

/// Refusal complement: a map naming a key the target does not
/// declare refuses UNKNOWN_SECTION with the target's declared sections
/// and a proposed map, and the file is byte-identical afterwards.
#[test]
fn unknown_section_refuses_with_declared_sections_and_touches_nothing() {
    let f = fixture();
    let mut engine = boot(&f, MountLifecycle::Eager);
    let b = create(&mut engine, "main", "Claim B", "claim", "statement");
    let before = file_bytes(&f, &b);
    let err = retype(&mut engine, &b, "finding", &[("statement", "bogus")]).unwrap_err();
    assert_eq!(err.code(), "UNKNOWN_SECTION");
    let d = err.details();
    assert_eq!(
        d["target_sections"],
        serde_json::json!(["conclusion", "notes"])
    );
    assert_eq!(d["target_catch_all"], "notes");
    assert_eq!(d["proposed_section_map"]["statement"], "notes", "{d}");
    // Report-all: the unknown key AND the required section it leaves
    // empty arrive together; the envelope code is the map defect.
    let codes: Vec<&str> = d["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, vec!["UNKNOWN_SECTION", "MISSING_REQUIRED_SECTION"]);
    assert_eq!(d["problems"][0]["key"], "bogus");
    assert_eq!(
        file_bytes(&f, &b),
        before,
        "refusal leaves the file untouched"
    );
    assert_eq!(engine.get_entity(&b).unwrap().entity_type, "claim");

    // No map at all: the undeclared `statement` refuses the same way,
    // and the required `conclusion` is reported in the same envelope.
    let err = retype(&mut engine, &b, "finding", &[]).unwrap_err();
    assert_eq!(
        err.code(),
        "UNKNOWN_SECTION",
        "a section-map defect dominates the envelope code"
    );
    let codes: Vec<String> = err.details()["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["code"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(codes, vec!["UNKNOWN_SECTION", "MISSING_REQUIRED_SECTION"]);
    assert_eq!(file_bytes(&f, &b), before);
    assert!(matches!(
        retype(&mut engine, &b, "claim", &[]).unwrap_err(),
        EngineError::RetypeNoOp { .. }
    ));
}

/// An incoming edge whose rel-type pins its target to the current
/// type refuses; so does an outgoing one and a cross-mem one; two
/// violations at once arrive in one envelope; every refusal leaves the
/// file byte-identical.
#[test]
fn edge_shapes_are_rechecked_in_both_directions_and_across_mems() {
    let f = fixture();
    let mut engine = boot(&f, MountLifecycle::Eager);
    let a = create(&mut engine, "main", "Claim A", "claim", "statement");
    let b = create(&mut engine, "main", "Claim B", "claim", "statement");
    relate(&mut engine, &a, "SUPPORTS", &b); // pinned: target must stay claim
    let before_b = file_bytes(&f, &b);

    // Incoming.
    let err = retype(&mut engine, &b, "finding", &[("statement", "conclusion")]).unwrap_err();
    assert_eq!(err.code(), "INVALID_REL_SHAPE", "{err}");
    let EngineError::RetypeRefused { problems, .. } = &err else {
        panic!("{err:?}")
    };
    assert_eq!(problems.len(), 1);
    let RetypeProblem::EdgeShape(edge) = &problems[0] else {
        panic!("{problems:?}")
    };
    assert_eq!(edge.direction, RetypeEdgeDirection::Incoming);
    assert_eq!(
        (edge.from.as_str(), edge.rel_type.as_str()),
        (a.as_ref(), "SUPPORTS")
    );
    assert!(!edge.cross_mem);
    assert_eq!(file_bytes(&f, &b), before_b);

    // Outgoing.
    let before_a = file_bytes(&f, &a);
    let err = retype(&mut engine, &a, "finding", &[("statement", "conclusion")]).unwrap_err();
    assert_eq!(err.code(), "INVALID_REL_SHAPE");
    let EngineError::RetypeRefused { problems, .. } = &err else {
        panic!("{err:?}")
    };
    let RetypeProblem::EdgeShape(edge) = &problems[0] else {
        panic!("{problems:?}")
    };
    assert_eq!(edge.direction, RetypeEdgeDirection::Outgoing);
    assert_eq!(edge.to, b.to_string());
    assert_eq!(file_bytes(&f, &a), before_a);

    // Report-all: B also supports A now — retyping B violates an
    // incoming AND an outgoing pin, both in one envelope.
    relate(&mut engine, &b, "SUPPORTS", &a);
    let before_b = file_bytes(&f, &b);
    let err = retype(&mut engine, &b, "finding", &[("statement", "conclusion")]).unwrap_err();
    assert_eq!(err.code(), "INVALID_REL_SHAPE");
    let d = err.details();
    let dirs: Vec<String> = d["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["direction"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(dirs, vec!["outgoing", "incoming"]);
    assert_eq!(file_bytes(&f, &b), before_b);

    // Cross-mem: a remark in the peer mem comments on a claim; the
    // cross-mem entry pins the target to claim.
    let c = create(&mut engine, "main", "Claim C", "claim", "statement");
    let r = create(&mut engine, "peer", "Remark R", "remark", "body");
    relate(&mut engine, &r, "COMMENTS_ON", &c);
    let before_c = file_bytes(&f, &c);
    let err = retype(&mut engine, &c, "finding", &[("statement", "conclusion")]).unwrap_err();
    assert_eq!(err.code(), "INVALID_REL_SHAPE", "{err}");
    let EngineError::RetypeRefused { problems, .. } = &err else {
        panic!("{err:?}")
    };
    let RetypeProblem::EdgeShape(edge) = &problems[0] else {
        panic!("{problems:?}")
    };
    assert!(edge.cross_mem);
    assert_eq!(edge.from, r.to_string());
    assert_eq!(file_bytes(&f, &c), before_c);
}

/// Two mems pinning the SAME schema: a cross-mem edge is judged by that
/// schema's own pins, exactly as relate and the loader judge it, so a
/// retype that would strand the referrer's edge refuses — the
/// same-schema case that used to be waved through.
#[test]
fn same_schema_cross_mem_edges_are_rechecked() {
    let f = fixture();
    let tmp_twin = f._tmp.path().join("mem-twin");
    std::fs::create_dir_all(&tmp_twin).unwrap();
    let main_pin = SchemaRef::new("rt", semver::Version::new(0, 1, 0));
    let mut engine = Engine::from_mounts_with_schemas_dir(
        vec![
            (
                mount(
                    "main",
                    f.main_dir.clone(),
                    main_pin.clone(),
                    MountLifecycle::Eager,
                ),
                Box::new(FilesystemBackend::new(f.main_dir.clone())) as Box<dyn MemBackend>,
            ),
            (
                mount("twin", tmp_twin.clone(), main_pin, MountLifecycle::Eager),
                Box::new(FilesystemBackend::new(tmp_twin.clone())) as Box<dyn MemBackend>,
            ),
        ],
        Some(&f.schemas_dir),
    )
    .unwrap();
    let mut settings = WorkspaceSettings::default();
    let mut links: BTreeMap<String, CrossLinkValue> = BTreeMap::new();
    links.insert("twin".to_string(), CrossLinkValue::Wildcard);
    links.insert("main".to_string(), CrossLinkValue::Wildcard);
    settings.cross_mem_links = links;
    engine.set_settings(settings);

    let v = create(&mut engine, "main", "Claim V", "claim", "statement");
    let w = create(&mut engine, "twin", "Claim W", "claim", "statement");
    relate(&mut engine, &w, "SUPPORTS", &v); // pinned target: claim
    let before = file_bytes(&f, &v);
    let err = retype(&mut engine, &v, "finding", &[("statement", "conclusion")]).unwrap_err();
    assert_eq!(err.code(), "INVALID_REL_SHAPE", "{err}");
    let EngineError::RetypeRefused { problems, .. } = &err else {
        panic!("{err:?}")
    };
    let RetypeProblem::EdgeShape(edge) = &problems[0] else {
        panic!("{problems:?}")
    };
    assert!(edge.cross_mem);
    assert_eq!(edge.from, w.to_string());
    assert_eq!(file_bytes(&f, &v), before);

    // Outgoing across the twin: W supports V, retyping W refuses too.
    let err = retype(&mut engine, &w, "finding", &[("statement", "conclusion")]).unwrap_err();
    assert_eq!(err.code(), "INVALID_REL_SHAPE", "{err}");
}

/// The referrer lives in a LAZY mem: its edge is not in the store, so
/// the retype probes the mem's storage and finds it; the same graph
/// with the peer loaded eagerly refuses identically, and the deferred
/// probe counts the edge it examined.
#[test]
fn deferred_referrers_are_probed_through_storage() {
    let f = fixture();
    {
        let mut engine = boot(&f, MountLifecycle::Eager);
        let c = create(&mut engine, "main", "Claim C", "claim", "statement");
        let r = create(&mut engine, "peer", "Remark R", "remark", "body");
        relate(&mut engine, &r, "COMMENTS_ON", &c);
    }
    let mut lazy = boot(&f, MountLifecycle::Lazy);
    let c = crate::EntityId::canonical("main--claim-c");
    assert!(
        lazy.mem_is_deferred("peer"),
        "peer stays unloaded until touched"
    );
    assert!(
        lazy.store().incoming(&c).is_empty(),
        "the lazy referrer's edge is not in the store"
    );
    let err = retype(&mut lazy, &c, "finding", &[("statement", "conclusion")]).unwrap_err();
    assert_eq!(err.code(), "INVALID_REL_SHAPE", "{err}");
    let EngineError::RetypeRefused { problems, .. } = &err else {
        panic!("{err:?}")
    };
    let RetypeProblem::EdgeShape(edge) = &problems[0] else {
        panic!("{problems:?}")
    };
    assert!(edge.cross_mem);
    assert_eq!(edge.from, "peer--remark-r");
    assert_eq!(lazy.get_entity(&c).unwrap().entity_type, "claim");

    // Without the pinned referrer, a lazily mounted peer does not
    // block, and the probe reports the edges it examined (none).
    let d = create(&mut lazy, "main", "Claim D", "claim", "statement");
    let out = retype(&mut lazy, &d, "finding", &[("statement", "conclusion")]).unwrap();
    assert_eq!(out.edges_rechecked, 0);
}

/// Dry run: the same validation, the prospective hash, no write.
#[test]
fn dry_run_validates_and_writes_nothing() {
    let f = fixture();
    let mut engine = boot(&f, MountLifecycle::Eager);
    let b = create(&mut engine, "main", "Claim B", "claim", "statement");
    let before = file_bytes(&f, &b);
    let (actor, client) = cli_actor();
    let out = engine
        .retype_entity(
            RetypeEntityArgs {
                id: b.clone(),
                expected_hash: crate::ExpectedHash::Missing,
                target_type: "finding".to_string(),
                section_map: IndexMap::from_iter([(
                    "statement".to_string(),
                    "conclusion".to_string(),
                )]),
                drop_metadata: Vec::new(),
                dry_run: true,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert!(out.write_id.is_empty());
    assert!(out.prospective_hash.is_some());
    assert_ne!(
        out.prospective_hash.as_deref(),
        Some(out.content_hash.as_str())
    );
    assert_eq!(file_bytes(&f, &b), before);
    assert_eq!(engine.get_entity(&b).unwrap().entity_type, "claim");
}

/// A second engine (the second process) adds a pinned
/// relation onto the entity from another mem after the first engine
/// loaded; the first engine's retype has to see it and refuse.
#[test]
fn a_retype_sees_a_pinned_referrer_a_second_engine_added_from_another_mem() {
    let f = fixture();
    let twin_dir = f._tmp.path().join("mem-twin");
    std::fs::create_dir_all(&twin_dir).unwrap();
    let schemas_dir = f.schemas_dir.clone();
    let main_dir = f.main_dir.clone();
    let boot2 = move || {
        let pin = SchemaRef::new("rt", semver::Version::new(0, 1, 0));
        let mut engine = Engine::from_mounts_with_schemas_dir(
            vec![
                (
                    mount("main", main_dir.clone(), pin.clone(), MountLifecycle::Eager),
                    Box::new(FilesystemBackend::new(main_dir.clone())) as Box<dyn MemBackend>,
                ),
                (
                    mount("twin", twin_dir.clone(), pin, MountLifecycle::Eager),
                    Box::new(FilesystemBackend::new(twin_dir.clone())) as Box<dyn MemBackend>,
                ),
            ],
            Some(&schemas_dir),
        )
        .unwrap();
        let mut settings = WorkspaceSettings::default();
        let mut links: BTreeMap<String, CrossLinkValue> = BTreeMap::new();
        links.insert("twin".to_string(), CrossLinkValue::Wildcard);
        links.insert("main".to_string(), CrossLinkValue::Wildcard);
        settings.cross_mem_links = links;
        engine.set_settings(settings);
        engine
    };
    let mut first = boot2();
    let v = create(&mut first, "main", "Claim V", "claim", "statement");

    let (boot3, v2) = (boot2.clone(), v.clone());
    std::thread::spawn(move || {
        let mut second = boot3();
        let w = create(&mut second, "twin", "Claim W", "claim", "statement");
        relate(&mut second, &w, "SUPPORTS", &v2);
    })
    .join()
    .unwrap();

    let result = retype(&mut first, &v, "finding", &[("statement", "conclusion")]);
    let code = result.as_ref().map(|_| "OK").unwrap_or_else(|e| e.code());
    assert_eq!(
        code, "INVALID_REL_SHAPE",
        "the guard sees the pinned referrer"
    );
    let fresh = boot2();
    assert_eq!(fresh.get_entity(&v).unwrap().entity_type, "claim");
    assert_eq!(fresh.store().incoming(&v).len(), 1);
}
