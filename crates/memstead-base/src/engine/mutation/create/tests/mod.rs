//! Tests of the create path, cut by concern: the schema gates a create
//! enforces, the proof schemas, the single and batch create paths, the
//! refusals, body links and cross-mem policy, anchors, cycles and section
//! formats. The helpers more than one file shares live here.

use crate::backend::MemBackend;
use crate::engine::test_helpers::*;
use crate::engine::{CreateEntityArgs, CreateEntityOutcome, Engine, EngineError, RelateEntityArgs};
use crate::ops::WarningHint;
use crate::storage::{ArchiveBackend, FilesystemBackend};
use indexmap::IndexMap;
use tempfile::TempDir;

mod anchors_cycles_format;
mod create_path;
mod create_refusals;
mod links_and_policy;
mod proof_schemas;
mod schema_gates;

/// Boot an engine whose mem pins a schema with one type (`task`)
/// declaring `required_outgoing: [{relationships: [PART_OF],
/// cardinality: at_least_one}]` — the fixture for the
/// MISSING_REQUIRED_OUTGOING mutation-warning tests.
fn engine_with_required_outgoing_schema(tmp: &TempDir) -> Engine {
    let schemas_dir = tmp.path().join("schemas");
    let pkg = schemas_dir.join("reqout");
    std::fs::create_dir_all(pkg.join("types")).unwrap();
    std::fs::write(
        pkg.join("schema.yaml"),
        r#"name: reqout
version: 0.1.0
description: required-outgoing fixture
when_to_use: tests
types:
  - task
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
"#,
    )
    .unwrap();
    std::fs::write(
        pkg.join("types").join("task.yaml"),
        r#"name: task
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
required_outgoing:
  - relationships: [PART_OF]
    cardinality: at_least_one
write_rules: []
"#,
    )
    .unwrap();
    let mem_dir = tmp.path().join("mem");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mount = crate::workspace::Mount {
        mem: "tasks".to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            "reqout",
            semver::Version::new(0, 1, 0),
        )),
        storage: crate::workspace::MountStorage::Folder { path: mem_dir },
        capability: crate::workspace::MountCapability::Write,
        lifecycle: crate::workspace::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    Engine::from_mounts_with_schemas_dir(
        vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
        Some(&schemas_dir),
    )
    .unwrap()
}

fn task_create_args(title: &str, relations: Vec<crate::ops::RelateArg>) -> CreateEntityArgs {
    let mut sections = IndexMap::new();
    sections.insert("body".to_string(), "a task body.".to_string());
    CreateEntityArgs {
        anchors: Vec::new(),
        mem: "tasks".to_string(),
        title: title.to_string(),
        entity_type: "task".to_string(),
        sections,
        metadata: IndexMap::new(),
        relations,
        dry_run: false,
    }
}

fn missing_outgoing_of(warnings: &[WarningHint]) -> Vec<(Vec<String>, String)> {
    warnings
        .iter()
        .filter_map(|w| match w {
            WarningHint::MissingRequiredOutgoing { missing, .. } => Some(
                missing
                    .iter()
                    .map(|b| (b.relationships.clone(), b.cardinality.clone()))
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect()
}

fn relate(
    engine: &mut Engine,
    from: &str,
    rel: &str,
    to: &str,
) -> Result<crate::engine::RelateEntityOutcome, crate::engine::EngineError> {
    let (actor, client) = cli_actor();
    engine.relate_entity(
        crate::engine::RelateEntityArgs {
            source: crate::entity::EntityId(from.to_string()),
            target: crate::entity::EntityId(to.to_string()),
            rel_type: rel.to_string(),
            description: None,
            remove: false,
            expected_hash: None,
            dry_run: false,
        },
        actor,
        Some(&client),
        None,
    )
}

/// Generic constraint-proof fixture: one folder-mounted mem
/// (`proof`) pinned to a schema built from the given manifest and
/// type YAMLs.
fn engine_with_proof_schema(
    tmp: &TempDir,
    schema_name: &str,
    manifest_yaml: &str,
    types: &[(&str, &str)],
) -> Engine {
    let schemas_dir = tmp.path().join("schemas");
    let pkg = schemas_dir.join(schema_name);
    std::fs::create_dir_all(pkg.join("types")).unwrap();
    std::fs::write(pkg.join("schema.yaml"), manifest_yaml).unwrap();
    for (name, yaml) in types {
        std::fs::write(pkg.join("types").join(format!("{name}.yaml")), yaml).unwrap();
    }
    let mem_dir = tmp.path().join("mem");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mount = crate::workspace::Mount {
        mem: "proof".to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            schema_name,
            semver::Version::new(0, 1, 0),
        )),
        storage: crate::workspace::MountStorage::Folder { path: mem_dir },
        capability: crate::workspace::MountCapability::Write,
        lifecycle: crate::workspace::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    Engine::from_mounts_with_schemas_dir(
        vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
        Some(&schemas_dir),
    )
    .unwrap()
}

fn rel(to: &str, rel_type: &str) -> crate::ops::RelateArg {
    crate::ops::RelateArg {
        target: crate::entity::EntityId(to.to_string()),
        rel_type: rel_type.to_string(),
        description: None,
    }
}

/// Build a folder-mount engine pinned to the `planning` schema, so
/// tests can exercise `decision` — a type with `decided_on` (Date,
/// required, no default / no init_timestamp) — without inventing a
/// synthetic schema.
fn engine_with_planning_schema(tmp: &TempDir) -> Engine {
    use crate::workspace::Mount;
    use crate::workspace::{MountCapability, MountLifecycle, MountStorage};
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mount = Mount {
        mem: "planning".to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            "planning",
            semver::Version::new(0, 1, 0),
        )),
        storage: MountStorage::Folder { path: mem_dir },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)]).unwrap()
}

// ---- Engine::update_entity --------------------------------------

/// Build a folder-mount Engine with one freshly-created entity.
/// Returns the engine + the created outcome so tests have the
/// id and current hash to use as `expected_hash` for the next
/// mutation.
fn engine_with_seed(tmp: &TempDir, title: &str) -> (Engine, CreateEntityOutcome) {
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    let outcome = engine
        .create_entity(
            empty_create_args("specs", title),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    (engine, outcome)
}
