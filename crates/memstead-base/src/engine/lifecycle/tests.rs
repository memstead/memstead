#![cfg(test)]

use tempfile::TempDir;

use crate::backend::{BackendError, MemBackend};
use crate::engine::test_helpers::*;
use crate::engine::{Engine, EngineError};
use crate::mem::MemOrigin;
use crate::ops::WarningHint;
use crate::storage::{ArchiveBackend, FilesystemBackend};

fn schema_package_files(heading: &str, manifest_name: &str) -> Vec<(String, Vec<u8>)> {
    let manifest = format!(
        r#"name: {manifest_name}
version: 1.0.0
description: Install-gate test schema
when_to_use: Tests
types:
  - sample
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#
    );
    let type_yaml = format!(
        r#"name: sample
description: t
when_to_use: tests
sections:
  - key: body
    heading: {heading}
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
"#
    );
    vec![
        ("schema.yaml".to_string(), manifest.into_bytes()),
        ("types/sample.yaml".to_string(), type_yaml.into_bytes()),
    ]
}

/// The install gate accepts a conforming package and refuses one
/// whose section heading cannot round-trip to its key — the last
/// moment the author can act, since sealed schemas keep loading.
#[test]
fn install_gate_refuses_non_roundtrip_heading() {
    let ok =
        Engine::validate_schema_package("gate", "1.0.0", &schema_package_files("Body", "gate"));
    assert!(ok.is_ok(), "conforming package passes: {ok:?}");

    let err = Engine::validate_schema_package(
        "gate",
        "1.0.0",
        &schema_package_files("Body Text", "gate"),
    )
    .expect_err("non-deriving heading must refuse install");
    match &err {
        EngineError::SchemaPackageInvalid { name, message, .. } => {
            assert_eq!(name, "gate");
            assert!(
                message.contains("'body'") && message.contains("'Body Text'"),
                "message names the offending tuple: {message}"
            );
        }
        other => panic!("expected SchemaPackageInvalid, got {other:?}"),
    }
}

/// Build a package whose single type carries an exemplar assembled
/// from the given pieces — the fixture for the exemplar-gate
/// tests. The type declares a required `body` section, a `status`
/// enum field, PART_OF (unpinned), and REFINES pinned to
/// `source_types: [other]` so a REFINES exemplar edge from
/// `sample` violates shape.
fn exemplar_package_files(
    section_key: &str,
    status_value: &str,
    rel_type: &str,
) -> Vec<(String, Vec<u8>)> {
    let manifest = r#"name: gate
version: 1.0.0
description: exemplar gate fixture
when_to_use: tests
types:
  - sample
  - other
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: REFINES
      description: pinned
      default_weight: 1.0
      source_types: [other]
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#
    .to_string();
    let type_yaml = format!(
        r#"name: sample
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: status
    description: workflow state
    field_type: string
    enum_values: [draft, final]
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
exemplar:
  title: A Conforming Sample
  metadata:
    status: "{status_value}"
  sections:
    {section_key}: "One canonical body paragraph."
  relations:
    - to: parent-placeholder
      type: {rel_type}
"#
    );
    let other_yaml = r#"name: other
description: shape-pin partner
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
"#
    .to_string();
    vec![
        ("schema.yaml".to_string(), manifest.into_bytes()),
        ("types/sample.yaml".to_string(), type_yaml.into_bytes()),
        ("types/other.yaml".to_string(), other_yaml.into_bytes()),
    ]
}

/// The exemplar gate: a package whose type
/// carries a CONFORMANT exemplar installs; the same package broken
/// three ways — wrong section key, illegal enum value, relationship
/// shape violation — refuses with a typed error naming the type
/// and the defect. No warn-and-carry path exists: the refusal is
/// `SchemaPackageInvalid`, same as every other install-gate class.
#[test]
fn install_gate_validates_exemplars_through_the_real_create_path() {
    // Conformant exemplar → the package installs.
    let ok = Engine::validate_schema_package(
        "gate",
        "1.0.0",
        &exemplar_package_files("body", "draft", "PART_OF"),
    );
    assert!(ok.is_ok(), "conformant exemplar passes: {ok:?}");

    // Variant 1 — wrong section key.
    let err = Engine::validate_schema_package(
        "gate",
        "1.0.0",
        &exemplar_package_files("bogus_section", "draft", "PART_OF"),
    )
    .expect_err("wrong section key must refuse");
    match &err {
        EngineError::SchemaPackageInvalid { message, .. } => {
            assert!(
                message.contains("'sample'") && message.contains("exemplar"),
                "names type and calls out the exemplar: {message}"
            );
            assert!(
                message.contains("UNKNOWN_SECTION") || message.contains("MISSING_REQUIRED_SECTION"),
                "carries the typed defect code: {message}"
            );
        }
        other => panic!("expected SchemaPackageInvalid, got {other:?}"),
    }

    // Variant 2 — illegal enum value.
    let err = Engine::validate_schema_package(
        "gate",
        "1.0.0",
        &exemplar_package_files("body", "not-a-legal-status", "PART_OF"),
    )
    .expect_err("illegal enum value must refuse");
    assert!(
        matches!(&err, EngineError::SchemaPackageInvalid { message, .. }
                if message.contains("'sample'") && message.contains("INVALID_ENUM_VALUE")),
        "got {err:?}"
    );

    // Variant 3 — relationship shape violation (REFINES is pinned
    // to source_types [other]; the exemplar's type is `sample`).
    let err = Engine::validate_schema_package(
        "gate",
        "1.0.0",
        &exemplar_package_files("body", "draft", "REFINES"),
    )
    .expect_err("relationship shape violation must refuse");
    assert!(
        matches!(&err, EngineError::SchemaPackageInvalid { message, .. }
                if message.contains("'sample'") && message.contains("INVALID_REL_SHAPE")),
        "got {err:?}"
    );
}

/// The worked-example teaching package (`memstead-schema/examples/
/// minimal`) models the exemplar practice — its exemplars validate
/// through the same gate, so the material that teaches schema
/// authoring can never itself teach a non-conformant shape.
#[test]
fn worked_example_package_exemplars_validate() {
    let pkg = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../memstead-schema/examples/minimal");
    let schema = std::sync::Arc::new(
        memstead_schema::load_schema_from_dir(&pkg).expect("worked example loads"),
    );
    assert!(
        schema.types.values().all(|td| td.exemplar.is_some()),
        "every worked-example type models the exemplar practice"
    );
    Engine::validate_schema_exemplars(&schema).expect("worked-example exemplars conform");
}

/// Every built-in schema's exemplars validate through the SAME
/// gate the install path runs — a built-in exemplar broken by a
/// future edit fails CI here. Completeness rides the same walk:
/// the NEWEST version of every built-in name carries an exemplar
/// on every type (older versions are sealed as shipped and may
/// predate the field).
#[test]
fn builtin_exemplars_validate_through_the_install_gate() {
    let schemas =
        memstead_schema::builtins::load_builtin_schemas().expect("built-in schemas always load");
    // Validity: every exemplar anywhere in the catalogue conforms.
    for schema in &schemas {
        if let Err(defect) = Engine::validate_schema_exemplars(schema) {
            let (name, version) = schema.id();
            panic!("built-in {name}@{version}: {defect}");
        }
    }
    // Completeness: the newest version per name is exemplar-complete.
    let mut newest: std::collections::HashMap<String, &std::sync::Arc<memstead_schema::Schema>> =
        std::collections::HashMap::new();
    for schema in &schemas {
        let name = schema.manifest.name.clone();
        match newest.get(&name) {
            Some(cur) if cur.version >= schema.version => {}
            _ => {
                newest.insert(name, schema);
            }
        }
    }
    for (name, schema) in &newest {
        for (type_name, td) in &schema.types {
            assert!(
                td.exemplar.is_some(),
                "built-in {name}@{} type '{type_name}' has no exemplar — the \
                     reference schemas model the practice completely",
                schema.version
            );
        }
    }
}

/// Exemplar relation targets are PLACEHOLDERS: a bare slug is
/// legal (target existence is never checked — the absent target
/// is the would-be-stub path), while a mem-prefixed target
/// refuses with the placeholder rule named.
#[test]
fn exemplar_relation_targets_are_bare_placeholder_slugs() {
    let mut files = exemplar_package_files("body", "draft", "PART_OF");
    let patched = String::from_utf8(files[1].1.clone())
        .unwrap()
        .replace("to: parent-placeholder", "to: other--real-entity");
    files[1].1 = patched.into_bytes();
    let err = Engine::validate_schema_package("gate", "1.0.0", &files)
        .expect_err("mem-prefixed exemplar target must refuse");
    assert!(
        matches!(&err, EngineError::SchemaPackageInvalid { message, .. }
                if message.contains("bare") && message.contains("'sample'")),
        "got {err:?}"
    );
}

/// A manifest whose declared identity contradicts the install ref
/// is refused — the schema would otherwise seal under a ref its
/// own manifest disagrees with.
#[test]
fn install_gate_refuses_manifest_identity_mismatch() {
    let err =
        Engine::validate_schema_package("gate", "1.0.0", &schema_package_files("Body", "other"))
            .expect_err("identity mismatch must refuse install");
    assert!(
        matches!(&err, EngineError::SchemaPackageInvalid { message, .. }
                if message.contains("other@1.0.0")),
        "got {err:?}"
    );
}

#[test]
fn reload_each_writable_mem_repopulates_load_warnings() {
    // Boot with a clean mem, then mid-flight write a file
    // with a duplicate heading, then call reload_each_writable_mem.
    // The accumulator should pick up the new typed warning.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    // Newest default generation so the clean-boot baseline isn't
    // tripped by the SCHEMA_GENERATIONS_BEHIND hint.
    let mut mount = folder_mount("specs", mem_dir.clone());
    mount.schema = Some("default@1.3.0".parse().unwrap());
    let mut engine =
        Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)]).unwrap();
    // The only thing a clean, entity-less mount says about itself
    // is that it is empty (`MOUNT_UNBACKED` / `empty`).
    assert!(
        engine
            .load_warnings()
            .iter()
            .all(|w| w.code() == "MOUNT_UNBACKED"),
        "clean boot has no warnings beyond the empty-mount one: {:?}",
        engine.load_warnings()
    );

    // Drop a markdown file with two `## Identity` headings.
    let body = "---\ntype: spec\n---\n# Dup\n\n## Identity\n\nfirst.\n\n## Identity\n\nsecond.\n";
    std::fs::write(mem_dir.join("dup.md"), body).unwrap();

    engine.reload_each_writable_mem().unwrap();
    let warnings = engine.load_warnings();
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, WarningHint::DuplicateSectionHeading { .. })),
        "workspace-wide reload must repopulate load_warnings: {warnings:?}",
    );
}

/// `validate_loaded_relations` runs on the reload path too — a
/// sibling-writer commit that injects a markdown file carrying a
/// schema-undeclared rel-type must surface as a typed
/// `PARSED_RELATION_INVALID` warning after `reload_each_writable_mem`.
/// Without the reload-path wiring this drift would slip past the
/// validator (boot only catches what existed at startup).
#[test]
fn reload_picks_up_parse_time_relation_drift_from_sibling_writer() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    // Seed a clean target entity at boot.
    let target_body = "---\ntype: spec\n---\n# Target\n\n## Identity\n\nThe target.\n";
    std::fs::write(mem_dir.join("target.md"), target_body).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    // Clean boot — no parse-time relation warnings yet.
    assert!(
        !engine
            .load_warnings()
            .iter()
            .any(|w| matches!(w, WarningHint::ParsedRelationInvalid { .. })),
        "clean boot must not emit ParsedRelationInvalid; got: {:?}",
        engine.load_warnings()
    );

    // Sibling-writer drops a new file with an unknown rel-type.
    let drift_body = "---\ntype: spec\n---\n# Source\n\n## Identity\n\nThe source.\n\n## Relationships\n\n- **MADE_UP_TYPE**: [[specs--target]]\n";
    std::fs::write(mem_dir.join("source.md"), drift_body).unwrap();

    engine.reload_each_writable_mem().unwrap();

    let invalid: Vec<_> = engine
        .load_warnings()
        .iter()
        .filter_map(|w| match w {
            WarningHint::ParsedRelationInvalid {
                rel_type,
                reason,
                origin,
                ..
            } => Some((rel_type.clone(), reason.clone(), origin.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        invalid.len(),
        1,
        "reload must surface the parse-time drift, got: {invalid:?}",
    );
    assert_eq!(invalid[0].0, "MADE_UP_TYPE");
    assert_eq!(invalid[0].1, "unknown_rel_type");
    assert_eq!(invalid[0].2, "writable");
}

#[test]
fn reload_one_mem_refreshes_own_slice_and_keeps_other_mems() {
    // Boot two mems, each with a duplicate-heading file, so the
    // accumulator carries one warning per mem. Fix alpha's file on
    // disk, reload ONLY alpha: alpha's stale warning must drop
    // (reload heals drift — health() must stop reporting it) while
    // beta's untouched warning survives (per-mem reload never
    // clears other mems' slices).
    let tmp = TempDir::new().unwrap();
    let dup_body = "---\ntype: spec\n---\n# Dup\n\n## Identity\n\na.\n\n## Identity\n\nb.\n";
    let a_dir = tmp.path().join("a");
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::write(a_dir.join("dup.md"), dup_body).unwrap();
    let b_dir = tmp.path().join("b");
    std::fs::create_dir_all(&b_dir).unwrap();
    std::fs::write(b_dir.join("dup.md"), dup_body).unwrap();
    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("alpha", a_dir.clone()),
            Box::new(FilesystemBackend::new(a_dir.clone())) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("beta", b_dir.clone()),
            Box::new(FilesystemBackend::new(b_dir.clone())) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    let mem_of = |w: &WarningHint| w.source_mem().map(str::to_string);
    let pre: Vec<_> = engine.load_warnings().iter().filter_map(mem_of).collect();
    assert!(
        pre.contains(&"alpha".to_string()) && pre.contains(&"beta".to_string()),
        "boot must populate one warning per mem: {pre:?}"
    );

    // Heal alpha's file on disk, then reload only alpha.
    let clean_body = "---\ntype: spec\n---\n# Dup\n\n## Identity\n\na.\n";
    std::fs::write(a_dir.join("dup.md"), clean_body).unwrap();
    engine.reload_one_mem("alpha").unwrap();

    let post: Vec<_> = engine.load_warnings().iter().filter_map(mem_of).collect();
    assert!(
        !post.contains(&"alpha".to_string()),
        "reload must drop the healed mem's stale warning: {post:?}"
    );
    assert!(
        post.contains(&"beta".to_string()),
        "reload of alpha must not clear beta's slice: {post:?}"
    );
}

#[test]
fn unregister_writable_mem_purges_load_warnings_for_that_mem_only() {
    // Two mems, each contributing a boot-time warning. Deleting
    // alpha must purge alpha's warnings from the accumulator
    // (health() merges it unconditionally — leftovers would cite
    // entities the store no longer holds) while beta's survive.
    let tmp = TempDir::new().unwrap();
    let dup_body = "---\ntype: spec\n---\n# Dup\n\n## Identity\n\na.\n\n## Identity\n\nb.\n";
    let a_dir = tmp.path().join("a");
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::write(a_dir.join("dup.md"), dup_body).unwrap();
    let b_dir = tmp.path().join("b");
    std::fs::create_dir_all(&b_dir).unwrap();
    std::fs::write(b_dir.join("dup.md"), dup_body).unwrap();
    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("alpha", a_dir.clone()),
            Box::new(FilesystemBackend::new(a_dir)) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("beta", b_dir.clone()),
            Box::new(FilesystemBackend::new(b_dir)) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    assert!(
        engine
            .load_warnings()
            .iter()
            .any(|w| w.source_mem() == Some("alpha")),
        "boot must carry alpha-sourced warnings"
    );

    engine.unregister_writable_mem("alpha").unwrap();

    let post = engine.load_warnings();
    assert!(
        !post.iter().any(|w| w.source_mem() == Some("alpha")),
        "delete must purge the removed mem's warnings: {post:?}"
    );
    assert!(
        post.iter().any(|w| w.source_mem() == Some("beta")),
        "delete of alpha must keep beta's warnings: {post:?}"
    );
}

#[test]
fn unregister_writable_mem_keeps_warnings_sourced_in_surviving_mems() {
    // Complement to the purge: a warning SOURCED in a surviving
    // mem whose TARGET pointed into the deleted mem must survive.
    // The invalid row still exists in the survivor's markdown —
    // it is live drift (recover-worthy), not stale state, so
    // purging by target would hide a real finding.
    let tmp = TempDir::new().unwrap();
    let a_dir = tmp.path().join("a");
    std::fs::create_dir_all(&a_dir).unwrap();
    let source_body = "---\ntype: spec\n---\n# Source\n\n## Identity\n\nThe source.\n\n## Relationships\n\n- **MADE_UP_TYPE**: [[beta--b1]]\n";
    std::fs::write(a_dir.join("source.md"), source_body).unwrap();
    let b_dir = tmp.path().join("b");
    std::fs::create_dir_all(&b_dir).unwrap();
    let target_body = "---\ntype: spec\n---\n# B1\n\n## Identity\n\nThe target.\n";
    std::fs::write(b_dir.join("b1.md"), target_body).unwrap();
    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("alpha", a_dir.clone()),
            Box::new(FilesystemBackend::new(a_dir)) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("beta", b_dir.clone()),
            Box::new(FilesystemBackend::new(b_dir)) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    let alpha_sourced = |engine: &Engine| {
        engine
                .load_warnings()
                .iter()
                .any(|w| matches!(w, WarningHint::ParsedRelationInvalid { entity_id, .. } if entity_id.mem() == "alpha"))
    };
    assert!(
        alpha_sourced(&engine),
        "boot must flag alpha's invalid row: {:?}",
        engine.load_warnings()
    );

    engine.unregister_writable_mem("beta").unwrap();

    assert!(
        alpha_sourced(&engine),
        "deleting the TARGET mem must not purge the survivor-sourced warning: {:?}",
        engine.load_warnings()
    );
}

/// The `memstead_reload` MCP tool's no-mem path consumes the
/// reports variant — it must refresh `load_warnings` like its slim
/// counterpart, not discard the sweep's warnings. Regression for
/// the split-brain where the slim variant repopulated and the
/// reports variant silently kept the boot-time snapshot forever
/// (observed live 2026-07-11: warnings for deleted mems survived a
/// workspace-wide MCP reload).
#[test]
fn reload_each_writable_mem_reports_refreshes_load_warnings() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let dup_body = "---\ntype: spec\n---\n# Dup\n\n## Identity\n\na.\n\n## Identity\n\nb.\n";
    std::fs::write(mem_dir.join("dup.md"), dup_body).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    assert!(
        !engine.load_warnings().is_empty(),
        "boot must populate load_warnings"
    );

    // Heal the file on disk; the reports sweep must clear the
    // stale warning.
    let clean_body = "---\ntype: spec\n---\n# Dup\n\n## Identity\n\na.\n";
    std::fs::write(mem_dir.join("dup.md"), clean_body).unwrap();
    engine.reload_each_writable_mem_reports().unwrap();
    assert!(
        engine.load_warnings().is_empty(),
        "reports sweep must drop healed warnings: {:?}",
        engine.load_warnings()
    );

    // And the inverse: fresh drift surfaces through the same sweep.
    std::fs::write(mem_dir.join("dup.md"), dup_body).unwrap();
    engine.reload_each_writable_mem_reports().unwrap();
    assert!(
        engine
            .load_warnings()
            .iter()
            .any(|w| matches!(w, WarningHint::DuplicateSectionHeading { .. })),
        "reports sweep must surface fresh drift: {:?}",
        engine.load_warnings()
    );
}

/// A cross-mem edge `A→B` must survive a
/// per-mem reload of the TARGET mem B. The removal cascade drops
/// B's incoming mirrors (including the cross-mem one sourced from A)
/// and the re-push only rebuilds edges authored by B, so without the
/// reconstruction pass the edge silently vanishes from the in-memory
/// index while staying intact in A's record and on disk — under-
/// reporting topology until a workspace-wide reload heals it.
#[test]
fn per_mem_reload_of_target_preserves_incoming_cross_mem_edge() {
    let tmp = TempDir::new().unwrap();
    let a_dir = tmp.path().join("a");
    let b_dir = tmp.path().join("b");
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::create_dir_all(&b_dir).unwrap();
    let a_writer = FilesystemBackend::new(a_dir.clone());
    let b_writer = FilesystemBackend::new(b_dir.clone());
    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("specs", a_dir),
            Box::new(a_writer) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("memos", b_dir),
            Box::new(b_writer) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();

    // Grant the cross-mem link specs → memos so the relate lands.
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "specs".to_string(),
        memstead_schema::workspace_config::CrossLinkValue::Wildcard,
    );
    engine.set_settings(settings);

    let (actor, client) = cli_actor();
    let source = engine
        .create_entity(
            empty_create_args("specs", "Source"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let target = engine
        .create_entity(
            empty_create_args("memos", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    engine
        .relate_entity(
            crate::engine::RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: target.id.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // (outgoing-present, incoming-present) for the A→B edge.
    let has_edge = |e: &Engine| {
        let out = e
            .store()
            .outgoing(&source.id)
            .iter()
            .any(|edge| edge.target == target.id);
        let inc = e
            .store()
            .incoming(&target.id)
            .iter()
            .any(|edge| edge.from == source.id);
        (out, inc)
    };

    assert_eq!(
        has_edge(&engine),
        (true, true),
        "edge must be indexed in both directions after relate",
    );

    // Per-mem reload of the TARGET mem — the bug trigger.
    engine.reload_one_mem("memos").unwrap();
    assert_eq!(
        has_edge(&engine),
        (true, true),
        "cross-mem edge into B must survive a per-mem reload of B",
    );

    // Convergence: a workspace-wide reload yields the same incoming
    // adjacency for the target — no path-dependent difference.
    engine.reload_each_writable_mem().unwrap();
    assert_eq!(
        has_edge(&engine),
        (true, true),
        "per-mem and workspace reload converge on the same edge",
    );

    // Complement: the edge stayed in the source record throughout —
    // the bug and the fix are about the index, not the records.
    assert!(
        engine
            .store()
            .get(&source.id)
            .unwrap()
            .relationships
            .iter()
            .any(|r| r.target == target.id),
        "source record must retain the relationship throughout",
    );
}

/// A per-mem reload of the SOURCE
/// mem leaves the cross-mem edge intact too — the source's own
/// outgoing edges are rebuilt by the re-push, and the reconstruction
/// pass for the OTHER mem is not needed here. Guards against a fix
/// that fixates on the target case and perturbs the source case.
#[test]
fn per_mem_reload_of_source_preserves_outgoing_cross_mem_edge() {
    let tmp = TempDir::new().unwrap();
    let a_dir = tmp.path().join("a");
    let b_dir = tmp.path().join("b");
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::create_dir_all(&b_dir).unwrap();
    let a_writer = FilesystemBackend::new(a_dir.clone());
    let b_writer = FilesystemBackend::new(b_dir.clone());
    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("specs", a_dir),
            Box::new(a_writer) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("memos", b_dir),
            Box::new(b_writer) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "specs".to_string(),
        memstead_schema::workspace_config::CrossLinkValue::Wildcard,
    );
    engine.set_settings(settings);

    let (actor, client) = cli_actor();
    let source = engine
        .create_entity(
            empty_create_args("specs", "Source"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let target = engine
        .create_entity(
            empty_create_args("memos", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    engine
        .relate_entity(
            crate::engine::RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: target.id.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    engine.reload_one_mem("specs").unwrap();

    let out = engine
        .store()
        .outgoing(&source.id)
        .iter()
        .any(|edge| edge.target == target.id);
    let inc = engine
        .store()
        .incoming(&target.id)
        .iter()
        .any(|edge| edge.from == source.id);
    assert!(
        out && inc,
        "outgoing cross-mem edge must survive a source-mem reload"
    );
}

#[test]
fn workspace_root_setter_round_trips() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let root = tmp.path().to_path_buf();
    engine.set_workspace_root(root.clone());
    assert_eq!(engine.workspace_root(), Some(root.as_path()));
}

#[test]
fn export_mem_folder_backend_produces_archive() {
    // Folder-backed mem with config + one entity. The
    // export_mem dispatcher routes to the folder backend's
    // override which produces a deterministic .memstead archive.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    let config_body = r#"{
            "format": 1,
            "schema": "default@1.0.0",
            "version": "1.0.0"
        }"#;
    std::fs::write(mem_dir.join(".memstead").join("config.json"), config_body).unwrap();

    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let archive_path = tmp.path().join("specs.mem");
    let result = engine.export_mem("specs", &archive_path).unwrap();
    assert!(archive_path.exists(), "archive must exist on disk");
    assert!(result.size_bytes > 0);
    // entity_count is 0 here (no .md files seeded); the function
    // still produces an archive carrying the config + schema.
    assert_eq!(result.entity_count, 0);
}

#[test]
fn export_mem_unknown_mem_returns_unknown_mem() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let output = tmp.path().join("out.mem");
    let err = engine.export_mem("missing", &output).unwrap_err();
    assert!(matches!(err, EngineError::UnknownMem(v) if v == "missing"));
}

#[test]
fn export_mem_missing_config_returns_invalid_input() {
    // Folder mount with no .memstead/config.json — `mem_config_for`
    // returns None and `export_mem` surfaces InvalidInput
    // rather than reaching the backend.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let output = tmp.path().join("out.mem");
    let err = engine.export_mem("specs", &output).unwrap_err();
    assert!(matches!(err, EngineError::InvalidInput(_)));
}

#[test]
fn export_mem_archive_backend_returns_sealed() {
    // Archive backends are already-an-archive — re-export is
    // intentionally rejected via BackendError::Sealed.
    let tmp = TempDir::new().unwrap();
    let archive_path = build_archive(
        tmp.path(),
        "ext",
        &[(
            ".memstead/config.json",
            b"{\"format\":1,\"schema\":\"default@1.0.0\",\"version\":\"1.0.0\"}",
        )],
    );
    let engine = Engine::from_mounts(vec![(
        archive_mount("ext", archive_path.clone()),
        Box::new(ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let output = tmp.path().join("out.mem");
    let err = engine.export_mem("ext", &output).unwrap_err();
    assert!(matches!(err, EngineError::Backend(BackendError::Sealed)));
}

#[test]
fn export_markdown_writes_unchanged_files_zero_writes() {
    // Seed a folder-backed mem with one entity, then call
    // export_markdown. The entity's file already matches the
    // generated content (engine wrote it via create_entity), so
    // export reports `unchanged: 1, written: 0`.
    let tmp = TempDir::new().unwrap();
    let (engine, _seeded) = engine_with_seed(&tmp, "Sample");
    let result = engine.export_markdown(None, None).unwrap();
    assert_eq!(
        result.written, 0,
        "freshly-created entity's file already matches generated markdown"
    );
    assert_eq!(
        result.unchanged, 1,
        "the one seeded entity counts as unchanged"
    );
    assert!(
        result.skipped_mounts.is_empty(),
        "folder-only workspace has no skipped mounts"
    );
}

#[test]
fn export_markdown_skips_non_folder_mounts() {
    // Archive-mounted mem has no working tree — workspace-wide
    // export records it under skipped_mounts and reports zero
    // writes / zero unchanged for the rest.
    let tmp = TempDir::new().unwrap();
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"# Title: Foo\n")]);
    let engine = Engine::from_mounts(vec![(
        archive_mount("ext", archive_path.clone()),
        Box::new(ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let result = engine.export_markdown(None, None).unwrap();
    assert_eq!(result.written, 0);
    assert_eq!(result.unchanged, 0);
    assert_eq!(
        result.skipped_mounts.len(),
        1,
        "archive mount is in the skipped list"
    );
    let entry = &result.skipped_mounts[0];
    assert_eq!(entry.mem, "ext");
    assert_eq!(entry.active_backend, "archive");
    assert_eq!(entry.reason, "backend_does_not_support_markdown_export");
}

#[test]
fn export_markdown_per_mem_refuses_on_incompatible_backend() {
    // Per-mem export against an archive-backed mem returns
    // the typed `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` refusal
    // naming the active backend and the supported-backend list.
    let tmp = TempDir::new().unwrap();
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"# Title: Foo\n")]);
    let engine = Engine::from_mounts(vec![(
        archive_mount("ext", archive_path.clone()),
        Box::new(ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let err = engine.export_markdown(Some("ext"), None).unwrap_err();
    assert_eq!(err.code(), "MARKDOWN_EXPORT_UNSUPPORTED_BACKEND");
    let details = err.details();
    assert_eq!(details["mem"], "ext");
    assert_eq!(details["active_backend"], "archive");
    assert_eq!(details["supported_backends"], serde_json::json!(["folder"]));
}

#[test]
fn register_writable_mem_adds_mount_and_router_entry() {
    // Start with one mem; register a second at runtime. Both
    // should be visible afterwards.
    let tmp = TempDir::new().unwrap();
    let mem_a = tmp.path().join("a");
    std::fs::create_dir_all(&mem_a).unwrap();
    let writer_a = FilesystemBackend::new(mem_a.clone());

    let mut engine = Engine::from_mounts(vec![(
        folder_mount("alpha", mem_a),
        Box::new(writer_a) as Box<dyn MemBackend>,
    )])
    .unwrap();
    assert!(engine.mem_router().is_writable("alpha"));

    let mem_b = tmp.path().join("b");
    std::fs::create_dir_all(&mem_b).unwrap();
    let writer_b = FilesystemBackend::new(mem_b.clone());

    engine
        .register_writable_mem(
            folder_mount("beta", mem_b.clone()),
            Box::new(writer_b) as Box<dyn MemBackend>,
            MemOrigin::ExplicitToml,
        )
        .unwrap();

    // Both mems are now writable + visible.
    assert!(engine.mem_router().is_writable("alpha"));
    assert!(engine.mem_router().is_writable("beta"));
    assert!(engine.mem_router().is_visible("beta"));

    // Mount + schema lookups resolve.
    assert!(engine.mount("beta").is_some());
    assert!(engine.schemas().contains_key("beta"));

    // Folder path surfaces via mem_router.
    assert_eq!(
        engine.mem_router().dir_for_mem("beta"),
        Some(mem_b.as_path()),
    );
}

/// Schema-pin authority on the runtime-register path (symmetric with
/// the boot path): a mem registered at runtime resolves its schema
/// from its own config (`software@0.1.0`) even though the mount
/// expects an unresolvable pin — register succeeds, and the
/// disagreement surfaces a `SchemaPinMismatch` warning.
#[test]
fn register_writable_mem_resolves_schema_from_mem_config() {
    let tmp = TempDir::new().unwrap();
    let mem_a = tmp.path().join("a");
    std::fs::create_dir_all(&mem_a).unwrap();
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("alpha", mem_a.clone()),
        Box::new(FilesystemBackend::new(mem_a)) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let mem_b = tmp.path().join("b");
    std::fs::create_dir_all(mem_b.join(".memstead")).unwrap();
    std::fs::write(
        mem_b.join(".memstead").join("config.json"),
        r#"{"schema":"software@0.1.0"}"#,
    )
    .unwrap();
    let mount_b = crate::workspace::Mount {
        mem: "beta".to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            "totally-not-a-schema",
            semver::Version::new(9, 9, 9),
        )),
        storage: crate::workspace::MountStorage::Folder {
            path: mem_b.clone(),
        },
        capability: crate::workspace::MountCapability::Write,
        lifecycle: crate::workspace::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    engine
            .register_writable_mem(
                mount_b,
                Box::new(FilesystemBackend::new(mem_b)) as Box<dyn MemBackend>,
                MemOrigin::ExplicitToml,
            )
            .expect("config pin software@0.1.0 is authoritative — register must succeed despite the unresolvable mount pin");

    assert!(engine.schemas().contains_key("beta"));
    let surfaced = engine.load_warnings().iter().any(|w| {
        matches!(
            w,
            WarningHint::SchemaPinMismatch { mem, config_pin, mount_pin }
                if mem == "beta"
                    && config_pin == "software@0.1.0"
                    && mount_pin == "totally-not-a-schema@9.9.9"
        )
    });
    assert!(
        surfaced,
        "SchemaPinMismatch must surface for beta: {:?}",
        engine.load_warnings(),
    );
}

#[test]
fn register_writable_mem_rejects_existing_name() {
    // Re-registering an already-writable mem must fail with
    // MemNameCollision and not mutate the engine.
    let tmp = TempDir::new().unwrap();
    let mem_a = tmp.path().join("a");
    std::fs::create_dir_all(&mem_a).unwrap();
    let writer_a = FilesystemBackend::new(mem_a.clone());

    let mut engine = Engine::from_mounts(vec![(
        folder_mount("alpha", mem_a),
        Box::new(writer_a) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let mount_count_pre = engine.mounts().len();

    let mem_collide = tmp.path().join("alpha-2");
    std::fs::create_dir_all(&mem_collide).unwrap();
    let writer_collide = FilesystemBackend::new(mem_collide.clone());

    let err = engine
        .register_writable_mem(
            folder_mount("alpha", mem_collide),
            Box::new(writer_collide) as Box<dyn MemBackend>,
            MemOrigin::ExplicitToml,
        )
        .unwrap_err();
    match err {
        EngineError::MemNameCollision {
            name,
            source_origin,
        } => {
            assert_eq!(name, "alpha");
            // post-restructure source_origin references
            // `.memstead/workspace.toml`; the assertion stays
            // permissive (substring OR non-empty) so the test
            // doesn't lock the exact wording.
            assert!(
                source_origin.contains(".memstead/workspace.toml") || !source_origin.is_empty()
            );
        }
        other => panic!("expected MemNameCollision, got {other:?}"),
    }

    // Engine state unchanged.
    assert_eq!(engine.mounts().len(), mount_count_pre);
}

#[test]
fn register_writable_mem_loads_entities_into_store() {
    // The newly-registered mem's entities should surface in
    // the engine's store after registration.
    let tmp = TempDir::new().unwrap();
    let mem_a = tmp.path().join("a");
    std::fs::create_dir_all(&mem_a).unwrap();
    let writer_a = FilesystemBackend::new(mem_a.clone());

    let mut engine = Engine::from_mounts(vec![(
        folder_mount("alpha", mem_a),
        Box::new(writer_a) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let pre_count = engine.store().all_entities().count();

    // Build mem_b with a markdown entity on disk.
    let mem_b = tmp.path().join("b");
    std::fs::create_dir_all(&mem_b).unwrap();
    std::fs::write(
        mem_b.join("b1.md"),
        "---\ntype: spec\n---\n# B1\n\n## Identity\n\nseed.\n",
    )
    .unwrap();
    let writer_b = FilesystemBackend::new(mem_b.clone());

    engine
        .register_writable_mem(
            folder_mount("beta", mem_b),
            Box::new(writer_b) as Box<dyn MemBackend>,
            MemOrigin::ExplicitToml,
        )
        .unwrap();

    let post_count = engine.store().all_entities().count();
    assert!(post_count > pre_count, "register must load entities");
    let beta_count = engine
        .store()
        .all_entities()
        .filter(|e| e.mem == "beta")
        .count();
    assert_eq!(beta_count, 1);
}

#[test]
fn register_then_unregister_round_trips() {
    // End-to-end check: register a mem, then unregister it,
    // and confirm the engine returns to the pre-registration
    // state.
    let tmp = TempDir::new().unwrap();
    let mem_a = tmp.path().join("a");
    std::fs::create_dir_all(&mem_a).unwrap();
    let writer_a = FilesystemBackend::new(mem_a.clone());

    let mut engine = Engine::from_mounts(vec![(
        folder_mount("alpha", mem_a),
        Box::new(writer_a) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let pre_mounts = engine.mounts().len();

    let mem_b = tmp.path().join("b");
    std::fs::create_dir_all(&mem_b).unwrap();
    let writer_b = FilesystemBackend::new(mem_b);

    engine
        .register_writable_mem(
            folder_mount("beta", tmp.path().join("b")),
            Box::new(writer_b) as Box<dyn MemBackend>,
            MemOrigin::ExplicitToml,
        )
        .unwrap();
    assert_eq!(engine.mounts().len(), pre_mounts + 1);

    let removed = engine.unregister_writable_mem("beta").unwrap();
    assert!(removed.is_some());
    assert_eq!(engine.mounts().len(), pre_mounts);
    assert!(!engine.mem_router().is_writable("beta"));
}

#[test]
fn unregister_writable_mem_returns_false_for_unknown_name() {
    // Idempotent contract: repeated calls / unknown names are
    // not errors — return false so callers can branch without
    // a typed error envelope for the common "already gone" case.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let removed = engine.unregister_writable_mem("missing").unwrap();
    assert!(removed.is_none(), "unknown mem returns Ok(None)");
    // The original mem is still present and readable.
    assert!(engine.mem_router().is_writable("specs"));
}

#[test]
fn unregister_writable_mem_drops_mount_and_router_entry() {
    // Heterogeneous engine: two mounts. Unregister one and
    // assert (a) it's gone from the mount list, (b) gone from
    // the mem_router's writable set, (c) the OTHER mount is
    // untouched.
    let tmp = TempDir::new().unwrap();
    let mem_a = tmp.path().join("a");
    std::fs::create_dir_all(&mem_a).unwrap();
    let writer_a = FilesystemBackend::new(mem_a.clone());
    let mem_b = tmp.path().join("b");
    std::fs::create_dir_all(&mem_b).unwrap();
    let writer_b = FilesystemBackend::new(mem_b.clone());

    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("alpha", mem_a),
            Box::new(writer_a) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("beta", mem_b),
            Box::new(writer_b) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();

    let removed = engine.unregister_writable_mem("alpha").unwrap();
    assert!(removed.is_some());

    // alpha is gone from every surface.
    assert!(!engine.mem_router().is_writable("alpha"));
    assert!(!engine.mem_router().is_visible("alpha"));
    assert!(engine.mount("alpha").is_none());

    // beta survives unchanged.
    assert!(engine.mem_router().is_writable("beta"));
    assert!(engine.mount("beta").is_some());
}

#[test]
fn unregister_writable_mem_drops_entities_for_that_mem_only() {
    // Build an engine with two mems, write one entity to each
    // backend, build the engine (loads both), unregister one,
    // assert the store still has the other mem's entity.
    let tmp = TempDir::new().unwrap();
    let mem_a = tmp.path().join("a");
    std::fs::create_dir_all(&mem_a).unwrap();
    std::fs::write(
        mem_a.join("a1.md"),
        "---\ntype: spec\n---\n# A1\n\n## Identity\n\nseed.\n",
    )
    .unwrap();
    let writer_a = FilesystemBackend::new(mem_a.clone());

    let mem_b = tmp.path().join("b");
    std::fs::create_dir_all(&mem_b).unwrap();
    std::fs::write(
        mem_b.join("b1.md"),
        "---\ntype: spec\n---\n# B1\n\n## Identity\n\nseed.\n",
    )
    .unwrap();
    let writer_b = FilesystemBackend::new(mem_b.clone());

    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("alpha", mem_a),
            Box::new(writer_a) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("beta", mem_b),
            Box::new(writer_b) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();

    let pre_total = engine.store().all_entities().count();
    assert!(pre_total >= 2, "both mems must load entities");

    engine.unregister_writable_mem("alpha").unwrap();

    // alpha's entities are gone.
    let alpha_remaining = engine
        .store()
        .all_entities()
        .filter(|e| e.mem == "alpha")
        .count();
    assert_eq!(alpha_remaining, 0);

    // beta's entities survive.
    let beta_remaining = engine
        .store()
        .all_entities()
        .filter(|e| e.mem == "beta")
        .count();
    assert!(beta_remaining > 0, "beta entities must survive");
}
#[test]
fn reload_one_mem_returns_empty_diff_when_disk_is_unchanged() {
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let result = engine
        .reload_one_mem("specs")
        .expect("reload on stable disk must succeed");
    assert!(result.added.is_empty(), "added: {:?}", result.added);
    assert!(result.changed.is_empty(), "changed: {:?}", result.changed);
    assert!(result.removed.is_empty(), "removed: {:?}", result.removed);
}

#[test]
fn reload_one_mem_picks_up_external_addition() {
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    // Simulate an external writer dropping a new entity on disk
    // without going through the engine.
    std::fs::write(
        tmp.path().join("external.md"),
        "---\ntype: spec\n---\n# External\n\n## Identity\n\nE.\n",
    )
    .unwrap();
    let result = engine.reload_one_mem("specs").unwrap();
    assert_eq!(
        result.added.iter().map(|i| i.as_ref()).collect::<Vec<_>>(),
        vec!["specs--external"]
    );
    assert!(result.changed.is_empty());
    assert!(result.removed.is_empty());
    // The new entity is now reachable through the engine.
    assert!(
        engine
            .get_entity(&crate::EntityId::new("specs", "external"))
            .is_some()
    );
}

#[test]
fn reload_one_mem_picks_up_external_removal() {
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    // Lonely Three exists from the demo fixture; remove it
    // off-engine and reload.
    std::fs::remove_file(tmp.path().join("lonely-three.md")).unwrap();
    let result = engine.reload_one_mem("specs").unwrap();
    assert!(result.added.is_empty());
    assert!(result.changed.is_empty());
    assert_eq!(
        result
            .removed
            .iter()
            .map(|i| i.as_ref())
            .collect::<Vec<_>>(),
        vec!["specs--lonely-three"]
    );
}

#[test]
fn reload_one_mem_picks_up_external_change() {
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    // Overwrite an existing entity's content; the new
    // `content_hash` must surface in the `changed` diff.
    std::fs::write(
        tmp.path().join("source-one.md"),
        "---\ntype: spec\n---\n# Source One Edited\n\n## Identity\n\nNew body.\n",
    )
    .unwrap();
    let result = engine.reload_one_mem("specs").unwrap();
    assert!(result.added.is_empty());
    assert_eq!(
        result
            .changed
            .iter()
            .map(|i| i.as_ref())
            .collect::<Vec<_>>(),
        vec!["specs--source-one"]
    );
    assert!(result.removed.is_empty());
}

#[test]
fn reload_one_mem_rejects_unknown_mem() {
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let err = engine.reload_one_mem("nope").unwrap_err();
    match err {
        EngineError::UnknownMem(name) => assert_eq!(name, "nope"),
        other => panic!("expected UnknownMem, got {other:?}"),
    }
}

#[test]
fn reload_each_writable_mem_returns_one_entry_per_mount() {
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let reports = engine
        .reload_each_writable_mem()
        .expect("batch reload on stable disk must succeed");
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].0, "specs");
    assert!(reports[0].1.added.is_empty());
    assert!(reports[0].1.changed.is_empty());
    assert!(reports[0].1.removed.is_empty());
}

// ---- Engine::settings -------------------------------------------

#[test]
fn settings_default_to_empty_on_fresh_engine() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    let s = engine.settings();
    assert!(s.mem_create_rules.is_empty());
    assert!(s.mem_delete_rules.is_empty());
    assert!(s.cross_mem_links.is_empty());
}

#[test]
fn set_settings_replaces_workspace_policy() {
    use crate::workspace::{CreateRuleSetting, DeleteRuleSetting, WorkspaceSettings};
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let mut settings = WorkspaceSettings::default();
    settings.mem_create_rules.push(CreateRuleSetting {
        pattern: "exec-*".to_string(),
        schemas: vec!["default@1.0.0".to_string()],
        default_cross_links: None,
    });
    settings.mem_delete_rules.push(DeleteRuleSetting {
        pattern: "exec-*".to_string(),
    });
    engine.set_settings(settings);
    assert_eq!(engine.settings().mem_create_rules.len(), 1);
    assert_eq!(engine.settings().mem_create_rules[0].pattern, "exec-*");
    assert_eq!(engine.settings().mem_delete_rules.len(), 1);
    assert_eq!(engine.settings().mem_delete_rules[0].pattern, "exec-*");
}

// ---- Engine::reload_each_writable_mem (continued) -------------

#[test]
fn reload_each_writable_mem_picks_up_external_changes_per_mem() {
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    // Mutate disk: add one entity, remove another, change a third.
    std::fs::write(
        tmp.path().join("new-via-disk.md"),
        "---\ntype: spec\n---\n# New Via Disk\n\n## Identity\n\nN.\n",
    )
    .unwrap();
    std::fs::remove_file(tmp.path().join("lonely-three.md")).unwrap();
    std::fs::write(
        tmp.path().join("source-one.md"),
        "---\ntype: spec\n---\n# Source One\n\n## Identity\n\nDifferent body.\n",
    )
    .unwrap();

    let reports = engine.reload_each_writable_mem().unwrap();
    assert_eq!(reports.len(), 1);
    let (mem, result) = &reports[0];
    assert_eq!(mem, "specs");
    assert_eq!(
        result.added.iter().map(|i| i.as_ref()).collect::<Vec<_>>(),
        vec!["specs--new-via-disk"]
    );
    assert_eq!(
        result
            .removed
            .iter()
            .map(|i| i.as_ref())
            .collect::<Vec<_>>(),
        vec!["specs--lonely-three"]
    );
    assert_eq!(
        result
            .changed
            .iter()
            .map(|i| i.as_ref())
            .collect::<Vec<_>>(),
        vec!["specs--source-one"]
    );
}

// ---- Engine::reload_one_mem_report (rich-shape wrapper) -------

#[test]
fn reload_one_mem_report_returns_rich_shape_for_folder_default() {
    // The folder backend's drift cursor is the changelog's
    // last-line timestamp (RFC3339-millis) — the same dialect
    // `folder_changes_since` accepts. With the demo engine's
    // creates already logged, both heads carry that cursor and,
    // with the disk unchanged between init and reload, they are
    // equal. entities_loaded reflects the post-reload count;
    // changed_entity_ids is empty when the disk is unchanged.
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let report = engine.reload_one_mem_report("specs").unwrap();
    assert_eq!(report.mem, "specs");
    assert_eq!(
        report.head_before, report.head_after,
        "unchanged disk → stable cursor"
    );
    assert!(
        crate::filesystem::changelog::parse_rfc3339_utc(&report.head_after).is_some(),
        "folder heads carry the changelog-ts cursor, got {}",
        report.head_after
    );
    // build_demo_engine seeds 3 entities (Source One, Target Two,
    // Lonely Three) — all real, no stubs from those creates.
    assert_eq!(report.entities_loaded, 3);
    // No external disk changes between init and reload → empty diff.
    assert!(report.changed_entity_ids.is_empty());
}

#[test]
fn reload_one_mem_report_unions_added_changed_removed_into_one_list() {
    // Mutate disk: add one, remove one, change one. The report's
    // changed_entity_ids unions the slim ReloadResult's three
    // diff lists into a single sorted vec — matches full's
    // wire contract.
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    std::fs::write(
        tmp.path().join("new-via-disk.md"),
        "---\ntype: spec\n---\n# New Via Disk\n\n## Identity\n\nN.\n",
    )
    .unwrap();
    std::fs::remove_file(tmp.path().join("lonely-three.md")).unwrap();
    std::fs::write(
        tmp.path().join("source-one.md"),
        "---\ntype: spec\n---\n# Source One\n\n## Identity\n\nDifferent body.\n",
    )
    .unwrap();

    let report = engine.reload_one_mem_report("specs").unwrap();
    assert_eq!(report.mem, "specs");
    let ids: Vec<&str> = report
        .changed_entity_ids
        .iter()
        .map(|id| id.as_ref())
        .collect();
    // Sorted lexicographically: lonely-three < new-via-disk < source-one
    assert_eq!(
        ids,
        vec![
            "specs--lonely-three",
            "specs--new-via-disk",
            "specs--source-one",
        ]
    );
}

#[test]
fn reload_one_mem_report_rejects_unknown_mem() {
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let err = engine.reload_one_mem_report("missing").unwrap_err();
    assert!(matches!(err, EngineError::UnknownMem(_)));
}

#[test]
fn reload_each_writable_mem_reports_returns_one_entry_per_mount() {
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let reports = engine.reload_each_writable_mem_reports().unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].mem, "specs");
    assert_eq!(reports[0].entities_loaded, 3);
}

/// Workspace-wide reload re-reads `.memstead/workspace.toml` and
/// refreshes [`WorkspaceSettings`]. This is the pairing with the
/// CLI's `memstead workspace allow-create / grant-cross-link /
/// set-mutations` family — without it, a CLI write lands on disk
/// but the running engine keeps serving the boot-time policy
/// snapshot until process restart.
#[test]
fn reload_each_writable_mem_reports_refreshes_workspace_settings() {
    let tmp = TempDir::new().unwrap();

    // Minimum-viable workspace.toml (no rules) + one writable
    // folder-backed mem.
    let memstead_dir = tmp.path().join(".memstead");
    std::fs::create_dir_all(&memstead_dir).unwrap();
    let workspace_toml = memstead_dir.join("workspace.toml");
    std::fs::write(
        &workspace_toml,
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    let mounts_json = memstead_dir.join("state").join("mounts.json");
    std::fs::create_dir_all(mounts_json.parent().unwrap()).unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let mounts_body = format!(
        r#"{{ "format": "memstead-mounts-3", "mounts": [{{ "mem": "specs", "schema": "default@1.0.0", "storage": {{ "type": "folder", "path": "{}" }}, "capability": "write", "lifecycle": "eager", "cross_linkable": true }}] }}"#,
        mem_dir.display(),
    );
    std::fs::write(&mounts_json, mounts_body).unwrap();

    let mut engine = Engine::from_workspace_root(tmp.path()).unwrap();
    assert!(
        engine.settings().mem_create_rules.is_empty(),
        "boot-time settings carry no create rules"
    );

    // Simulate an out-of-band CLI write to workspace.toml.
    std::fs::write(
            &workspace_toml,
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n\n[[mem_management.create]]\npattern = \"exec-*\"\nschemas = [\"default@1.0.0\"]\n",
        )
        .unwrap();

    engine.reload_each_writable_mem_reports().unwrap();

    let rules = &engine.settings().mem_create_rules;
    assert_eq!(
        rules.len(),
        1,
        "workspace-wide reload must refresh the policy"
    );
    assert_eq!(rules[0].pattern, "exec-*");
}

// ---- Engine::reload_if_stale ------------------------------

// ---- set_mem_schema / dual-pin migration ----

const MIG_TYPE_TAIL: &str = r#"sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: _default
no_self_loop_relationships: []
updatable_fields: []
health_required_fields: []
staleness_threshold_days: 90
write_rules: []
"#;

/// Schema manifest for the migration tests: `name@version` with a
/// `doc` type. `with_status = true` adds a required, no-default
/// enum field `status` — entities created without it are
/// non-conformant against that schema.
fn mig_manifest(name: &str, version: &str) -> String {
    format!(
        r#"name: {name}
version: {version}
description: migration test schema
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: USES
      description: link
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#
    )
}

fn mig_type_yaml(with_status: bool) -> String {
    let metadata = if with_status {
        "metadata_fields:\n  - key: status\n    description: Lifecycle state\n    field_type: string\n    required: true\n    enum_values:\n      - open\n      - closed\n"
    } else {
        "metadata_fields: []\n"
    };
    format!("name: doc\ndescription: t\nwhen_to_use: tests\n{metadata}{MIG_TYPE_TAIL}")
}

fn write_mig_schema(
    root: &std::path::Path,
    dir: &str,
    name: &str,
    version: &str,
    with_status: bool,
) {
    let d = root.join(dir);
    std::fs::create_dir_all(d.join("types")).unwrap();
    std::fs::write(d.join("schema.yaml"), mig_manifest(name, version)).unwrap();
    std::fs::write(d.join("types").join("doc.yaml"), mig_type_yaml(with_status)).unwrap();
}

/// Engine with one mem pinned `mig-a@0.1.0` (no required
/// metadata) plus loadable `mig-a@0.2.0` (identical shape) and
/// `mig-b@0.1.0` (required enum `status`) in the workspace
/// schemas dir. Two conformant-under-A entities are created.
/// The criterion-4 property test (flywheel W8/01): a
/// deterministic, seeded, hand-rolled generator (xorshift64 — the
/// house discipline, no dependency) drives mutation sequences
/// across EVERY kind — create, update, relate, delete, rename,
/// batch update (applied AND refused/rolled-back), reload, and a
/// schema switch — asserting at checkpoints and at sequence end
/// that the maintained derived structures are identical to a
/// from-scratch rebuild over the current store. Coverage is
/// guaranteed by construction (the first pass cycles every kind
/// once before the random tail), and asserted, so a silently
/// narrowed generator fails the suite. A failing sequence
/// reproduces from the seed printed in the panic message alone.
#[test]
fn derived_structures_match_rebuild_across_random_mutation_sequences() {
    for seed in [0x5eed_0001_u64, 0x5eed_0002, 0x5eed_0003] {
        run_mutation_sequence(seed);
    }
}

struct Xorshift(u64);
impl Xorshift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn assert_derived_oracles(engine: &Engine, seed: u64, label: &str) {
    // Search oracle: the maintained per-mem index holds exactly
    // the ids a from-scratch build over the current store holds.
    let fresh = crate::search_index::build_all(engine.store(), &engine.schemas);
    let live = engine.search_indexes();
    let mut live_mems: Vec<&String> = live.keys().collect();
    let mut fresh_mems: Vec<&String> = fresh.keys().collect();
    live_mems.sort();
    fresh_mems.sort();
    assert_eq!(
        live_mems, fresh_mems,
        "seed {seed:#x} @ {label}: index mem set diverged from rebuild"
    );
    for (mem, idx) in live {
        let mut got = idx.stored_ids().unwrap();
        let mut want = fresh[mem].stored_ids().unwrap();
        got.sort();
        want.sort();
        assert_eq!(
            got, want,
            "seed {seed:#x} @ {label}: mem `{mem}` index contents diverged from rebuild"
        );
    }
    // Community oracle: the memoised partition equals a fresh
    // detection with the same parameter source (smallest mem name).
    let schema = engine
        .schemas
        .iter()
        .min_by(|a, b| a.0.cmp(b.0))
        .map(|(_, s)| s.clone())
        .expect("schema present");
    let weights_schema = schema.clone();
    let fresh_partition = crate::graph::community::detect_communities(
        engine.store(),
        schema.manifest.community.resolution,
        schema.manifest.community.seed,
        move |rel_type| {
            weights_schema
                .manifest
                .relationships
                .definitions
                .iter()
                .find(|d| d.name == rel_type)
                .map(|d| d.default_weight as f64)
                .unwrap_or(1.0)
        },
    );
    assert_eq!(
        engine.communities().entity_cluster_map,
        fresh_partition.entity_cluster_map,
        "seed {seed:#x} @ {label}: partition diverged from a fresh detection"
    );
}

fn run_mutation_sequence(seed: u64) {
    use indexmap::IndexMap;

    let (_tmp, mut engine) = migration_engine();
    let mut rng = Xorshift(seed);
    let mut live: Vec<crate::EntityId> = vec![
        crate::EntityId::new("specs", "one"),
        crate::EntityId::new("specs", "two"),
    ];
    let mut counter = 0usize;
    let mut kinds_hit: std::collections::HashSet<&'static str> = std::collections::HashSet::new();

    const KINDS: [&str; 7] = [
        "create",
        "update",
        "relate",
        "delete",
        "rename",
        "batch_applied",
        "batch_refused",
    ];

    let bare_update = |id: crate::EntityId| crate::engine::UpdateEntityArgs {
        anchors: Vec::new(),
        anchors_unset: Vec::new(),
        id,
        expected_hash: None,
        sections: IndexMap::new(),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
    };

    for op_i in 0..30usize {
        // First pass cycles every kind once (coverage by
        // construction); the tail is seed-driven.
        let kind = *KINDS
            .get(op_i)
            .unwrap_or_else(|| &KINDS[rng.pick(KINDS.len())]);
        match kind {
            "create" => {
                counter += 1;
                let mut args = empty_create_args("specs", &format!("Gen {counter}"));
                args.entity_type = "doc".to_string();
                args.sections = IndexMap::from_iter([(
                    "body".to_string(),
                    format!("generated body {counter}"),
                )]);
                let out = engine
                    .create_entity(args, crate::vcs::Actor::Cli, None, None)
                    .expect("generated create is conformant");
                live.push(out.id);
                kinds_hit.insert("create");
            }
            "update" => {
                let id = live[rng.pick(live.len())].clone();
                let mut args = bare_update(id);
                args.append_sections
                    .insert("body".to_string(), format!("appended at op {op_i}"));
                engine
                    .update_entity(args, crate::vcs::Actor::Cli, None, None)
                    .expect("append update is conformant");
                kinds_hit.insert("update");
            }
            "relate" => {
                if live.len() >= 2 {
                    let a = rng.pick(live.len());
                    let mut b = rng.pick(live.len());
                    if a == b {
                        b = (b + 1) % live.len();
                    }
                    engine
                        .relate_entity(
                            crate::engine::RelateEntityArgs {
                                source: live[a].clone(),
                                expected_hash: None,
                                rel_type: "USES".to_string(),
                                target: live[b].clone(),
                                remove: false,
                                description: None,
                                dry_run: false,
                            },
                            crate::vcs::Actor::Cli,
                            None,
                            None,
                        )
                        .expect("USES relate is legal under mig-a");
                    kinds_hit.insert("relate");
                }
            }
            "delete" => {
                // Only reference-free entities delete cleanly; keep
                // at least two so relate stays possible.
                if live.len() > 2
                    && let Some(pos) = (0..live.len()).find(|&i| {
                        engine.store().incoming(&live[i]).is_empty()
                            && engine
                                .store()
                                .get(&live[i])
                                .is_some_and(|e| e.relationships.is_empty())
                    })
                {
                    let id = live.remove(pos);
                    engine
                        .delete_entity(
                            crate::engine::DeleteEntityArgs {
                                id: id.clone(),
                                expected_hash: None,
                            },
                            crate::vcs::Actor::Cli,
                            None,
                            None,
                        )
                        .expect("reference-free delete lands");
                    kinds_hit.insert("delete");
                }
            }
            "rename" => {
                counter += 1;
                let pos = rng.pick(live.len());
                let old = live[pos].clone();
                let out = engine
                    .rename_entity(
                        crate::engine::RenameEntityArgs {
                            id: old,
                            new_title: format!("Renamed {counter}"),
                            expected_hash: None,
                        },
                        crate::vcs::Actor::Cli,
                        None,
                        None,
                    )
                    .expect("fresh-slug rename lands");
                live[pos] = out.new_id;
                kinds_hit.insert("rename");
            }
            "batch_applied" => {
                let id_a = live[rng.pick(live.len())].clone();
                let mut a = bare_update(id_a);
                a.append_sections
                    .insert("body".to_string(), format!("batch line {op_i}"));
                let result = engine
                    .batch_update(vec![(a, None)], crate::vcs::Actor::Cli, None, false)
                    .expect("batch envelope");
                assert!(result.applied, "single-entry append batch applies");
                kinds_hit.insert("batch_applied");
            }
            "batch_refused" => {
                let id_a = live[rng.pick(live.len())].clone();
                let mut a = bare_update(id_a);
                a.append_sections
                    .insert("body".to_string(), "doomed".to_string());
                let missing = bare_update(crate::EntityId::new("specs", "no-such-entity"));
                let result = engine
                    .batch_update(
                        vec![(a, None), (missing, None)],
                        crate::vcs::Actor::Cli,
                        None,
                        false,
                    )
                    .expect("refused batch returns a report-all envelope");
                assert!(!result.applied, "the missing target refuses the batch");
                kinds_hit.insert("batch_refused");
            }
            _ => unreachable!(),
        }

        if op_i == 14 {
            // The at-least-one schema switch the criterion demands
            // (identical field shape, so the index rebuild is
            // exercised through the epoch path).
            engine
                .set_mem_schema("specs", &sref("mig-a@0.2.0"))
                .expect("integral switch");
            kinds_hit.insert("schema_switch");
        }
        if op_i == 19 {
            engine.reload_one_mem("specs").expect("reload lands");
            kinds_hit.insert("reload");
        }

        if op_i % 10 == 9 {
            assert_derived_oracles(&engine, seed, &format!("checkpoint op {op_i}"));
        }
    }

    assert_derived_oracles(&engine, seed, "sequence end");

    for kind in KINDS.iter().copied().chain(["schema_switch", "reload"]) {
        assert!(
            kinds_hit.contains(kind),
            "seed {seed:#x}: generator coverage narrowed — kind `{kind}` never executed"
        );
    }
}

fn migration_engine() -> (tempfile::TempDir, Engine) {
    let tmp = tempfile::TempDir::new().unwrap();
    let schemas_dir = tmp.path().join("schemas");
    write_mig_schema(&schemas_dir, "mig-a-1", "mig-a", "0.1.0", false);
    write_mig_schema(&schemas_dir, "mig-a-2", "mig-a", "0.2.0", false);
    write_mig_schema(&schemas_dir, "mig-b-1", "mig-b", "0.1.0", true);
    let mem_dir = tmp.path().join("mem");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = crate::storage::FilesystemBackend::new(mem_dir.clone());
    let mut mount = folder_mount("specs", mem_dir);
    mount.schema = Some("mig-a@0.1.0".parse().unwrap());
    let mut engine = Engine::from_mounts_with_schemas_dir(
        vec![(
            mount,
            Box::new(writer) as Box<dyn crate::backend::MemBackend>,
        )],
        Some(&schemas_dir),
    )
    .unwrap();
    for title in ["One", "Two"] {
        let mut args = empty_create_args("specs", title);
        args.entity_type = "doc".to_string();
        args.sections =
            indexmap::IndexMap::from_iter([("body".to_string(), "content".to_string())]);
        engine
            .create_entity(args, crate::vcs::Actor::Cli, None, None)
            .expect("conformant create under mig-a");
    }
    (tmp, engine)
}

fn sref(s: &str) -> memstead_schema::SchemaRef {
    s.parse().unwrap()
}

/// A `SCHEMA_PIN_MISMATCH` state (the mount expects the target, the
/// served config pins an older generation) is exactly what
/// `set-schema` must repair: the switch persists the config pin and
/// reports `switched`, never `noop`. Complement: with both in
/// agreement the same call is a noop.
#[test]
fn set_schema_repairs_a_mount_expectation_ahead_of_the_served_pin() {
    let (_tmp, mut engine) = migration_engine();
    // Fabricate the mismatch: the mount expectation says mig-b while
    // the engine still serves mig-a from the config.
    let idx = engine
        .mounts
        .iter()
        .position(|m| m.mount.mem == "specs")
        .unwrap();
    engine.mounts[idx].mount.schema = Some(sref("mig-b@0.1.0"));
    assert_eq!(engine.schemas.get("specs").unwrap().id().0, "mig-a");

    let out = engine
        .set_mem_schema("specs", &sref("mig-b@0.1.0"))
        .unwrap();
    // The fixture's entities are not integral against mig-b, so the
    // honest answer is a started migration; the point is that it is
    // NOT the noop the stale mount expectation used to produce.
    assert_eq!(
        out.outcome,
        crate::engine::SetSchemaResult::MigrationStarted,
        "a served pin behind the target enters the switch path, never a noop: {out:?}"
    );
    assert!(!out.findings.is_empty());
    assert_eq!(
        engine.schemas.get("specs").unwrap().id().0,
        "mig-b",
        "writes now validate against the target"
    );

    // Complement: served pin and expectation both at the target (no
    // migration in flight) is a noop.
    let (_tmp2, mut clean) = migration_engine();
    let again = clean.set_mem_schema("specs", &sref("mig-a@0.1.0")).unwrap();
    assert_eq!(again.outcome, crate::engine::SetSchemaResult::Noop);
}

#[test]
fn set_schema_noop_on_current_pin() {
    let (_tmp, mut engine) = migration_engine();
    let out = engine
        .set_mem_schema("specs", &sref("mig-a@0.1.0"))
        .unwrap();
    assert_eq!(out.outcome, crate::engine::SetSchemaResult::Noop);
    assert_eq!(out.schema_pin, "mig-a@0.1.0");
    assert_eq!(out.migration_target, None);
    assert!(out.findings.is_empty());
}

/// Schema-switch invalidation: a
/// schema switch changes NO store content — the store generation
/// stays put — yet both derived memos depend on the schema
/// (community weights, the index field set), so the switch must
/// clear them. The schemas EPOCH is what carries that dependency
/// into the memo key; without it the generation-checked hooks
/// would keep both memos and serve results computed against the
/// old schema (the staleness the whole-map drop used to mask).
#[test]
fn schema_switch_invalidates_both_memos_despite_unchanged_store() {
    let (_tmp, mut engine) = migration_engine();

    let _ = engine.communities();
    let _ = engine.search_indexes();
    assert!(engine.community_memo.get().is_some());
    assert!(engine.search_indexes_memo.get().is_some());
    let store_gen_before = engine.store().generation();

    engine
        .set_mem_schema("specs", &sref("mig-a@0.2.0"))
        .unwrap();

    assert_eq!(
        engine.store().generation(),
        store_gen_before,
        "a schema switch mutates no store content"
    );
    assert!(
        engine.community_memo.get().is_none(),
        "the community memo must clear on a schema switch (weights derive from the schema)"
    );
    assert!(
        engine.search_indexes_memo.get().is_none(),
        "the search memo must clear on a schema switch (the field set derives from the schema)"
    );
}

#[test]
fn set_schema_switches_immediately_when_integral() {
    // Version bump within the same domain; entities conform to
    // the identical-shape 0.2.0, so the switch is immediate.
    let (_tmp, mut engine) = migration_engine();
    let out = engine
        .set_mem_schema("specs", &sref("mig-a@0.2.0"))
        .unwrap();
    assert_eq!(out.outcome, crate::engine::SetSchemaResult::Switched);
    assert_eq!(out.schema_pin, "mig-a@0.2.0");
    assert_eq!(out.migration_target, None);
    assert!(out.findings.is_empty());
    assert_eq!(
        engine.schema_pin("specs").unwrap().as_display(),
        "mig-a@0.2.0"
    );
    assert!(engine.migration_target("specs").is_none());
}

/// Regression: an atomic switch must persist the new pin into the
/// **authoritative** backend config, not just `mounts.json`. Boot
/// resolution prefers the backend config's pin over `Mount.schema`,
/// so before this fix the switch evaporated on the next process boot
/// for any config-present mem (every `create_mem`-made mem).
#[test]
fn set_schema_switch_persists_pin_into_backend_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let schemas_dir = tmp.path().join("schemas");
    write_mig_schema(&schemas_dir, "mig-a-1", "mig-a", "0.1.0", false);
    write_mig_schema(&schemas_dir, "mig-a-2", "mig-a", "0.2.0", false);
    let mem_dir = tmp.path().join("mem");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    // Config-present mem: the authoritative pin lives here.
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        br#"{"schema":"mig-a@0.1.0"}"#,
    )
    .unwrap();
    let writer = crate::storage::FilesystemBackend::new(mem_dir.clone());
    let mut mount = folder_mount("specs", mem_dir.clone());
    mount.schema = Some("mig-a@0.1.0".parse().unwrap());
    let mut engine = Engine::from_mounts_with_schemas_dir(
        vec![(
            mount,
            Box::new(writer) as Box<dyn crate::backend::MemBackend>,
        )],
        Some(&schemas_dir),
    )
    .unwrap();

    let out = engine
        .set_mem_schema("specs", &sref("mig-a@0.2.0"))
        .unwrap();
    assert_eq!(out.outcome, crate::engine::SetSchemaResult::Switched);

    // The authoritative backend config now carries the new pin —
    // otherwise the switch would evaporate on reboot.
    let cfg_bytes = std::fs::read(mem_dir.join(".memstead").join("config.json")).unwrap();
    let cfg: serde_json::Value = serde_json::from_slice(&cfg_bytes).unwrap();
    assert_eq!(
        cfg["schema"], "mig-a@0.2.0",
        "atomic switch must update the authoritative backend config"
    );
}

/// A completed migration re-stamps the mutation stamp (the marker
/// `ENGINE_VERSION_SKEW` reads) with the target; a dual-pin entry
/// leaves it on the old generation and says so; a set-schema to
/// the pin the mem already carries changes no byte of the config.
#[test]
fn set_schema_completed_switch_restamps_marker_and_dual_pin_leaves_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let schemas_dir = tmp.path().join("schemas");
    write_mig_schema(&schemas_dir, "mig-a-1", "mig-a", "0.1.0", false);
    write_mig_schema(&schemas_dir, "mig-a-2", "mig-a", "0.2.0", false);
    write_mig_schema(&schemas_dir, "mig-b-1", "mig-b", "0.1.0", true);
    let mem_dir = tmp.path().join("mem");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    let config_path = mem_dir.join(".memstead").join("config.json");
    // A stamp from an earlier mutation on the old generation.
    std::fs::write(
            &config_path,
            br#"{"schema":"mig-a@0.1.0","mutationStamp":{"engineVersion":"0.0.1","schema":"mig-a@0.1.0"}}"#,
        )
        .unwrap();
    let writer = crate::storage::FilesystemBackend::new(mem_dir.clone());
    let mut mount = folder_mount("specs", mem_dir.clone());
    mount.schema = Some("mig-a@0.1.0".parse().unwrap());
    let mut engine = Engine::from_mounts_with_schemas_dir(
        vec![(
            mount,
            Box::new(writer) as Box<dyn crate::backend::MemBackend>,
        )],
        Some(&schemas_dir),
    )
    .unwrap();
    let stamp_of = |path: &std::path::Path| -> serde_json::Value {
        let cfg: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        cfg["mutationStamp"].clone()
    };

    // Refusal complement: the same pin moves no byte.
    let before = std::fs::read(&config_path).unwrap();
    let out = engine
        .set_mem_schema("specs", &sref("mig-a@0.1.0"))
        .unwrap();
    assert_eq!(out.outcome, crate::engine::SetSchemaResult::Noop);
    assert_eq!(out.stamped_schema.as_deref(), Some("mig-a@0.1.0"));
    assert_eq!(
        std::fs::read(&config_path).unwrap(),
        before,
        "a noop set-schema changes no byte of the mem config"
    );

    // Dual-pin entry (mig-b requires a section the entities lack):
    // the marker stays on the old generation and the outcome says so.
    for title in ["One", "Two"] {
        let mut args = empty_create_args("specs", title);
        args.entity_type = "doc".to_string();
        args.sections =
            indexmap::IndexMap::from_iter([("body".to_string(), "content".to_string())]);
        engine
            .create_entity(args, crate::vcs::Actor::Cli, None, None)
            .expect("conformant create under mig-a");
    }
    let out = engine
        .set_mem_schema("specs", &sref("mig-b@0.1.0"))
        .unwrap();
    assert_eq!(
        out.outcome,
        crate::engine::SetSchemaResult::MigrationStarted
    );
    assert!(!out.findings.is_empty());
    assert_eq!(out.stamped_schema.as_deref(), Some("mig-a@0.1.0"));
    assert_eq!(stamp_of(&config_path)["schema"], "mig-a@0.1.0");

    // Completed switch (mig-a@0.2.0 is shape-identical, so the
    // in-flight mig-b target is replaced by an integral switch): the
    // marker names the new generation, stamped by this engine.
    let out = engine
        .set_mem_schema("specs", &sref("mig-a@0.2.0"))
        .unwrap();
    assert_eq!(out.outcome, crate::engine::SetSchemaResult::Switched);
    assert_eq!(out.stamped_schema.as_deref(), Some("mig-a@0.2.0"));
    let stamp = stamp_of(&config_path);
    assert_eq!(stamp["schema"], "mig-a@0.2.0");
    assert_eq!(stamp["engineVersion"], crate::build_info::full_version());
}

#[test]
fn set_schema_unknown_target_refuses_schema_not_found() {
    let (_tmp, mut engine) = migration_engine();
    let err = engine
        .set_mem_schema("specs", &sref("nope@9.9.9"))
        .unwrap_err();
    assert_eq!(err.code(), "SCHEMA_NOT_FOUND");
    // No state change.
    assert!(engine.migration_target("specs").is_none());
}

#[test]
fn set_schema_migration_lifecycle_end_to_end() {
    let (_tmp, mut engine) = migration_engine();
    let target = sref("mig-b@0.1.0");

    // 1. Non-integral target → migration starts; pin unchanged.
    let out = engine.set_mem_schema("specs", &target).unwrap();
    assert_eq!(
        out.outcome,
        crate::engine::SetSchemaResult::MigrationStarted
    );
    assert_eq!(out.schema_pin, "mig-a@0.1.0");
    assert_eq!(out.migration_target.as_deref(), Some("mig-b@0.1.0"));
    assert_eq!(out.findings.len(), 2, "both entities lack `status`");
    assert!(
        out.findings
            .iter()
            .all(|f| f.code == "REQUIRED_FIELD_UNSET")
    );

    // 2. Reads of not-yet-repaired entities stay permissive.
    let one = crate::entity::EntityId::new("specs", "one");
    assert!(engine.store().get(&one).is_some());

    // 3. Re-issue while unrepaired → pending, full remaining set.
    let out = engine.set_mem_schema("specs", &target).unwrap();
    assert_eq!(
        out.outcome,
        crate::engine::SetSchemaResult::MigrationPending
    );
    assert_eq!(out.findings.len(), 2);

    // 4. Writes validate against the TARGET: `status` is unknown
    //    to the pinned mig-a but declared by mig-b — setting it
    //    must commit; an invalid enum value must refuse.
    let mut bad = crate::engine::UpdateEntityArgs {
        anchors: Vec::new(),
        id: one.clone(),
        expected_hash: None,
        sections: indexmap::IndexMap::new(),
        append_sections: indexmap::IndexMap::new(),
        patch_sections: indexmap::IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: indexmap::IndexMap::from_iter([("status".to_string(), "banana".to_string())]),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    };
    let err = engine
        .update_entity(bad.clone(), crate::vcs::Actor::Cli, None, None)
        .unwrap_err();
    assert_eq!(err.code(), "INVALID_ENUM_VALUE", "strict against target");
    bad.metadata = indexmap::IndexMap::from_iter([("status".to_string(), "open".to_string())]);
    engine
        .update_entity(bad, crate::vcs::Actor::Cli, None, None)
        .expect("repair write validated against the migration target");

    // 5. One entity repaired → still pending, findings shrink.
    let out = engine.set_mem_schema("specs", &target).unwrap();
    assert_eq!(
        out.outcome,
        crate::engine::SetSchemaResult::MigrationPending
    );
    assert_eq!(out.findings.len(), 1, "only `two` remains non-integral");

    // 6. Repair the second entity, re-issue → atomic switch.
    let two = crate::entity::EntityId::new("specs", "two");
    let repair = crate::engine::UpdateEntityArgs {
        anchors: Vec::new(),
        id: two.clone(),
        expected_hash: None,
        sections: indexmap::IndexMap::new(),
        append_sections: indexmap::IndexMap::new(),
        patch_sections: indexmap::IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: indexmap::IndexMap::from_iter([("status".to_string(), "closed".to_string())]),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    };
    engine
        .update_entity(repair, crate::vcs::Actor::Cli, None, None)
        .unwrap();
    let out = engine.set_mem_schema("specs", &target).unwrap();
    assert_eq!(out.outcome, crate::engine::SetSchemaResult::Switched);
    assert_eq!(out.schema_pin, "mig-b@0.1.0");
    assert_eq!(out.migration_target, None);
    assert!(out.findings.is_empty());
    assert_eq!(
        engine.schema_pin("specs").unwrap().as_display(),
        "mig-b@0.1.0"
    );
    assert!(engine.migration_target("specs").is_none());
}

/// During migration every not-yet-repaired entity is
/// non-conformant against the target, so `relations_unset` works
/// on exactly those entities with no mode flag — and the same
/// update can complete the entity's repair.
#[test]
fn relations_unset_works_during_migration_without_mode_flag() {
    let (_tmp, mut engine) = migration_engine();
    let one = crate::entity::EntityId::new("specs", "one");
    let two = crate::entity::EntityId::new("specs", "two");
    engine
        .relate_entity(
            crate::engine::RelateEntityArgs {
                source: one.clone(),
                expected_hash: None,
                rel_type: "USES".to_string(),
                target: two.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            crate::vcs::Actor::Cli,
            None,
            None,
        )
        .unwrap();
    // Conformant under the pin → the repair gate is shut.
    let shut = engine
        .update_entity(
            crate::engine::UpdateEntityArgs {
                anchors: Vec::new(),
                id: one.clone(),
                expected_hash: None,
                sections: indexmap::IndexMap::new(),
                append_sections: indexmap::IndexMap::new(),
                patch_sections: indexmap::IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: indexmap::IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: vec![crate::ops::RelationUnsetArg {
                    rel_type: "USES".to_string(),
                    target: two.clone(),
                }],
                anchors_unset: Vec::new(),
            },
            crate::vcs::Actor::Cli,
            None,
            None,
        )
        .unwrap_err();
    assert_eq!(shut.code(), "REPAIR_NOT_NEEDED");

    // Enter migration → `one` is now non-conformant against the
    // target; the same call opens, removes the relation, and the
    // bundled `status` set makes the entity integral-against-target.
    engine
        .set_mem_schema("specs", &sref("mig-b@0.1.0"))
        .unwrap();
    engine
        .update_entity(
            crate::engine::UpdateEntityArgs {
                anchors: Vec::new(),
                id: one.clone(),
                expected_hash: None,
                sections: indexmap::IndexMap::new(),
                append_sections: indexmap::IndexMap::new(),
                patch_sections: indexmap::IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: indexmap::IndexMap::from_iter([(
                    "status".to_string(),
                    "open".to_string(),
                )]),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: vec![crate::ops::RelationUnsetArg {
                    rel_type: "USES".to_string(),
                    target: two.clone(),
                }],
                anchors_unset: Vec::new(),
            },
            crate::vcs::Actor::Cli,
            None,
            None,
        )
        .expect("repair-shaped update lands during migration without a flag");
    let entity = engine.store().get(&one).unwrap();
    assert!(entity.relationships.is_empty());
}

/// Boot honors a persisted in-flight migration: a mount carrying
/// `migration_target` validates writes against the target from
/// the first call of the new process — the resumability half of
/// the dual-pin contract.
#[test]
fn boot_resumes_dual_pin_validation_against_target() {
    let (tmp, engine) = migration_engine();
    drop(engine);
    let schemas_dir = tmp.path().join("schemas");
    let mem_dir = tmp.path().join("mem");
    let writer = crate::storage::FilesystemBackend::new(mem_dir.clone());
    let mut mount = folder_mount("specs", mem_dir);
    mount.schema = Some("mig-a@0.1.0".parse().unwrap());
    mount.migration_target = Some("mig-b@0.1.0".parse().unwrap());
    let engine = Engine::from_mounts_with_schemas_dir(
        vec![(
            mount,
            Box::new(writer) as Box<dyn crate::backend::MemBackend>,
        )],
        Some(&schemas_dir),
    )
    .unwrap();
    // Effective validation schema is the target...
    let (name, version) = {
        let s = engine.schema_for("specs").unwrap();
        let (n, v) = s.id();
        (n.to_string(), v.to_string())
    };
    assert_eq!((name.as_str(), version.as_str()), ("mig-b", "0.1.0"));
    // ...while the settled pin and the in-flight target read back
    // distinctly.
    assert_eq!(
        engine.schema_pin("specs").unwrap().as_display(),
        "mig-a@0.1.0"
    );
    assert_eq!(
        engine.migration_target("specs").unwrap().as_display(),
        "mig-b@0.1.0"
    );
}

/// Every lifecycle setter refuses `READ_ONLY_MOUNT` on a read-only
/// mount — the family, not an instance. `set_mem_schema` was the
/// one ungated sibling (a schema-pin change starts a migration —
/// the last mutation a sealed mount should accept); this test
/// enumerates all seven current setters — extend it when adding an
/// eighth (the enumeration is manual, not reflective). Refusal complement: the same calls succeed (or fail
/// for their own non-capability reasons) against a writable mount —
/// covered by the existing per-setter tests; `set_mem_schema`'s
/// writable-mount behaviour is pinned by the migration tests above.
#[test]
fn every_lifecycle_setter_refuses_on_read_only_mount() {
    let tmp = TempDir::new().unwrap();
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"# Title: Foo\n")]);
    let mut engine = Engine::from_mounts(vec![(
        archive_mount("ext", archive_path.clone()),
        Box::new(ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let default_pin: memstead_schema::SchemaRef = "default@1.0.0".parse().unwrap();
    let attempts: Vec<(&str, EngineError)> = vec![
        (
            "set_mem_schema",
            engine.set_mem_schema("ext", &default_pin).unwrap_err(),
        ),
        (
            "set_mem_version",
            engine
                .set_mem_version("ext", semver::Version::new(9, 9, 9), None)
                .unwrap_err(),
        ),
        (
            "set_mem_description",
            engine
                .set_mem_description("ext", Some("x".into()), None)
                .unwrap_err(),
        ),
        (
            "set_mem_title",
            engine
                .set_mem_title("ext", Some("x".into()), None)
                .unwrap_err(),
        ),
        (
            "set_mem_subject",
            engine.set_mem_subject("ext", None, None).unwrap_err(),
        ),
        (
            "set_mem_internal",
            engine.set_mem_internal("ext", true, None).unwrap_err(),
        ),
        (
            "set_mem_sync_state",
            engine
                .set_mem_sync_state("ext", "k", "t", None)
                .unwrap_err(),
        ),
    ];
    for (setter, err) in attempts {
        match err {
            EngineError::ReadOnlyMount(v) => {
                assert_eq!(v, "ext", "{setter} must name the refused mem")
            }
            other => panic!("{setter} must refuse ReadOnlyMount, got {other:?}"),
        }
    }
}

/// 04/03, criteria 1 and 2, on the folder backend. Every one of the
/// lifecycle setters, each against a config a sibling moved after boot.
/// The loop is the point: the criterion is the whole set behind one
/// implementation, so a test that exercised one setter would pass while
/// the other six stayed broken.
#[test]
fn no_config_setter_reverts_a_siblings_write() {
    type Setter = fn(&mut Engine) -> Result<(), EngineError>;
    let setters: Vec<(&str, Setter)> = vec![
        ("version", |e| {
            e.set_mem_version("specs", semver::Version::new(9, 0, 0), None)
                .map(|_| ())
        }),
        ("description", |e| {
            e.set_mem_description("specs", Some("mine".into()), None)
                .map(|_| ())
        }),
        ("title", |e| {
            e.set_mem_title("specs", Some("Mine".into()), None)
                .map(|_| ())
        }),
        ("internal", |e| {
            e.set_mem_internal("specs", true, None).map(|_| ())
        }),
        ("sync_state", |e| {
            e.set_mem_sync_state("specs", "src/facet", "tok", None)
                .map(|_| ())
        }),
        // Cleared rather than set: the mark validates against a real
        // commit cursor, and the clear path writes config just the same,
        // which is what this test is about.
        ("review_mark", |e| {
            e.set_review_mark("specs", None, None).map(|_| ())
        }),
    ];

    for (name, set) in setters {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let meta = mem_dir.join(memstead_schema::MEM_META_DIR);
        std::fs::create_dir_all(&meta).unwrap();
        let path = meta.join("config.json");
        std::fs::write(
            &path,
            br#"{"schema": "default@1.0.0", "version": "0.1.0"}"#.as_slice(),
        )
        .unwrap();

        let writer = FilesystemBackend::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        // The review-mark setter validates its cursor against a real
        // entity, so seed one before the sibling write.
        engine
            .create_entity(
                crate::engine::test_helpers::empty_create_args("specs", "Seed"),
                crate::vcs::Actor::Cli,
                None,
                None,
            )
            .unwrap();

        // A sibling writes a field this engine has never seen.
        let mut sibling: memstead_schema::MemConfig =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        sibling
            .extra
            .insert("siblingMark".into(), serde_json::json!("kept"));
        std::fs::write(&path, serde_json::to_vec_pretty(&sibling).unwrap()).unwrap();

        set(&mut engine).unwrap_or_else(|e| panic!("{name} setter failed: {e}"));

        let after: memstead_schema::MemConfig =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            after.extra.get("siblingMark"),
            Some(&serde_json::json!("kept")),
            "the {name} setter reverted a field it never set"
        );
    }
}

/// Criterion 3, and its complement 4: the intervention is reported on the
/// operation's own response, and only when there was one.
#[test]
fn intervention_is_reported_on_the_response_and_only_when_real() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let meta = mem_dir.join(memstead_schema::MEM_META_DIR);
    std::fs::create_dir_all(&meta).unwrap();
    let path = meta.join("config.json");
    std::fs::write(&path, br#"{"schema": "default@1.0.0"}"#.as_slice()).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    // Single writer: no report. This is the ordinary path, and a fix that
    // cried intervention here would be worse than the bug.
    let quiet = engine
        .set_mem_description("specs", Some("first".into()), None)
        .unwrap();
    assert!(
        !quiet
            .warnings
            .iter()
            .any(|w| w.code() == "CONFIG_WRITE_INTERVENED"),
        "single-writer workspace must stay silent: {:?}",
        quiet.warnings
    );

    // A sibling intervenes; the next write says so, naming the field.
    let mut sibling: memstead_schema::MemConfig =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    sibling.title = Some("theirs".into());
    std::fs::write(&path, serde_json::to_vec_pretty(&sibling).unwrap()).unwrap();

    let loud = engine
        .set_mem_description("specs", Some("second".into()), None)
        .unwrap();
    let hint = loud
        .warnings
        .iter()
        .find(|w| w.code() == "CONFIG_WRITE_INTERVENED")
        .expect("intervention must be reported on the response");
    assert!(
        format!("{hint}").contains("title"),
        "the report names what they changed: {hint}"
    );
    // And theirs survived, which is the point of reporting rather than refusing.
    let after: memstead_schema::MemConfig =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(after.title.as_deref(), Some("theirs"));
    assert_eq!(after.description.as_deref(), Some("second"));
}
