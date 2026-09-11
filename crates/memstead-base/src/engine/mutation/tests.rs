#![cfg(test)]

use tempfile::TempDir;

use crate::backend::MemBackend;
use crate::engine::test_helpers::*;
use crate::engine::{CreateEntityArgs, Engine, UpdateEntityArgs};

use crate::storage::FilesystemBackend;
use crate::vcs::CommitContext;

use indexmap::IndexMap;

#[test]
fn with_ctx_wrappers_delegate_to_explicit_forms() {
    // Each *_with_ctx wrapper bundles a CommitContext and
    // routes through the corresponding 4-arg method. Verify
    // create → update → rename → delete via the wrappers
    // observably mutate the store the same way the explicit
    // forms would.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let ctx = CommitContext::internal();

    // create_entity_with_ctx
    let create_args = CreateEntityArgs {
        anchors: Vec::new(),
        mem: "specs".to_string(),
        title: "Seed".to_string(),
        entity_type: "spec".to_string(),
        sections: IndexMap::from_iter([
            ("identity".to_string(), "seed identity".to_string()),
            ("purpose".to_string(), "seed purpose".to_string()),
        ]),
        metadata: IndexMap::new(),
        relations: Vec::new(),
        dry_run: false,
    };
    let created = engine.create_entity_with_ctx(create_args, &ctx).unwrap();
    assert_eq!(created.title, "Seed");
    assert!(engine.store().get(&created.id).is_some());

    // update_entity_with_ctx
    let update_args = UpdateEntityArgs {
        anchors: Vec::new(),
        id: created.id.clone(),
        expected_hash: Some(created.content_hash.clone()),
        sections: IndexMap::from_iter([("identity".to_string(), "updated".to_string())]),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        dry_run: false,
        declare_relations: Vec::new(),
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    };
    let updated = engine.update_entity_with_ctx(update_args, &ctx).unwrap();
    assert!(
        !updated.write_id.is_empty()
            || (updated.modified_sections.replaced.is_empty()
                && updated.modified_sections.appended.is_empty()
                && updated.modified_sections.patched.is_empty())
    );

    // rename_entity_with_ctx
    let renamed = engine
        .rename_entity_with_ctx(&created.id, "Renamed", &updated.content_hash, &ctx)
        .unwrap();
    assert_ne!(renamed.old_id, renamed.new_id);
    assert!(engine.store().get(&renamed.new_id).is_some());

    // delete_entity_with_ctx
    let deleted = engine
        .delete_entity_with_ctx(&renamed.new_id, &renamed.content_hash, &ctx)
        .unwrap();
    assert_eq!(deleted.id, renamed.new_id);
    assert!(engine.store().get(&renamed.new_id).is_none());
}

/// Minimal on-disk MemConfig pinning `default@1.0.0`, with an
/// optional pre-set mutation stamp — the carrier the stamp path
/// and the boot skew check both read.
fn write_config(dir: &std::path::Path, stamp: Option<memstead_schema::MutationStamp>) {
    let meta = dir.join(memstead_schema::MEM_META_DIR);
    std::fs::create_dir_all(&meta).unwrap();
    let mut config: memstead_schema::MemConfig =
        serde_json::from_str(r#"{"schema": "default@1.0.0"}"#).unwrap();
    config.mutation_stamp = stamp;
    std::fs::write(
        meta.join("config.json"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
}

fn stamped_engine_fixture(mem_dir: std::path::PathBuf) -> Engine {
    Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(FilesystemBackend::new(mem_dir)) as Box<dyn MemBackend>,
    )])
    .unwrap()
}

fn disk_stamp(dir: &std::path::Path) -> Option<memstead_schema::MutationStamp> {
    let bytes = std::fs::read(dir.join(memstead_schema::MEM_META_DIR).join("config.json")).unwrap();
    let config: memstead_schema::MemConfig = serde_json::from_slice(&bytes).unwrap();
    config.mutation_stamp
}

fn spec_create_args(title: &str) -> CreateEntityArgs {
    CreateEntityArgs {
        anchors: Vec::new(),
        mem: "specs".to_string(),
        title: title.to_string(),
        entity_type: "spec".to_string(),
        sections: IndexMap::from_iter([
            ("identity".to_string(), "seed identity".to_string()),
            ("purpose".to_string(), "seed purpose".to_string()),
        ]),
        metadata: IndexMap::new(),
        relations: Vec::new(),
        dry_run: false,
    }
}

/// Criterion 3: a mutation stamps the mem's
/// engine-owned state with the running engine version and resolved
/// schema; a read-only load writes nothing.
#[test]
fn mutation_writes_version_stamp_and_read_only_load_does_not() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    write_config(&mem_dir, None);

    // Read-only session: boot and drop without mutating — the
    // stamp stays absent.
    drop(stamped_engine_fixture(mem_dir.clone()));
    assert!(
        disk_stamp(&mem_dir).is_none(),
        "a read-only load must not write a stamp"
    );

    // A mutation stamps engine version + resolved schema.
    let mut engine = stamped_engine_fixture(mem_dir.clone());
    engine
        .create_entity_with_ctx(spec_create_args("Seed"), &CommitContext::internal())
        .unwrap();
    let stamp = disk_stamp(&mem_dir).expect("mutation must write the stamp");
    assert_eq!(stamp.engine_version, crate::build_info::full_version());
    assert_eq!(stamp.schema, "default@1.0.0");

    // A second mutation under the same binary leaves the stamp at
    // the same value (the write path compares and no-ops).
    let mut engine = stamped_engine_fixture(mem_dir.clone());
    engine
        .create_entity_with_ctx(spec_create_args("Second"), &CommitContext::internal())
        .unwrap();
    let again = disk_stamp(&mem_dir).expect("stamp survives");
    assert_eq!(again, stamp);
}

/// The reported damage, reproduced (04/03, criteria 7 and 8): a
/// long-lived engine boots, a sibling writes the config out of band, and
/// the engine's next ENTITY mutation stamps the version. Before the fix
/// that stamp serialized the boot-time struct and the sibling's write was
/// gone. No lifecycle call is involved anywhere in this test, which is why
/// the loss looked spontaneous to the operator who reported it.
///
/// The divergent stamp is written to disk rather than injected, because
/// the running binary's version is a compile-time constant with no runtime
/// seam; this is how the existing skew coverage reaches the condition too.
#[test]
fn a_sibling_config_write_survives_the_next_entity_mutation() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    // Seed a stamp that disagrees with this binary, so the stamp writer is
    // live rather than dormant: that is the two-binary topology the report
    // came from.
    write_config(
        &mem_dir,
        Some(memstead_schema::MutationStamp {
            engine_version: "0.0.1-other".to_string(),
            schema: "default@1.0.0".to_string(),
        }),
    );

    // The long-lived engine boots and caches the config as it is now.
    let mut engine = stamped_engine_fixture(mem_dir.clone());

    // A sibling process sets a description. The engine never learns:
    // a config-only write advances no entity head and appends no change
    // log line, so the staleness probe cannot see it.
    let path = mem_dir
        .join(memstead_schema::MEM_META_DIR)
        .join("config.json");
    let mut sibling: memstead_schema::MemConfig =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    sibling.description = Some("written by the sibling".to_string());
    std::fs::write(&path, serde_json::to_vec_pretty(&sibling).unwrap()).unwrap();

    // An ordinary entity write. Nothing about it mentions config.
    engine
        .create_entity_with_ctx(spec_create_args("Seed"), &CommitContext::internal())
        .unwrap();

    let after: memstead_schema::MemConfig =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        after.description.as_deref(),
        Some("written by the sibling"),
        "the sibling's description must survive an entity mutation"
    );
    assert_eq!(
        after.mutation_stamp.map(|s| s.engine_version),
        Some(crate::build_info::full_version().to_string()),
        "and the stamp this engine came to write must still land"
    );
}

/// Criterion 3 for the stamp writer: the intervention reaches the ENTITY
/// mutation's own response. The stamp has no response of its own, and an
/// earlier draft discarded the report with `let _`, so an operator whose
/// config moved during an innocuous entity write was told nothing.
#[test]
fn the_stamps_intervention_rides_the_entity_mutations_response() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    write_config(
        &mem_dir,
        Some(memstead_schema::MutationStamp {
            engine_version: "0.0.1-other".to_string(),
            schema: "default@1.0.0".to_string(),
        }),
    );
    let mut engine = stamped_engine_fixture(mem_dir.clone());

    let path = mem_dir
        .join(memstead_schema::MEM_META_DIR)
        .join("config.json");
    let mut sibling: memstead_schema::MemConfig =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    sibling.description = Some("theirs".to_string());
    std::fs::write(&path, serde_json::to_vec_pretty(&sibling).unwrap()).unwrap();

    let outcome = engine
        .create_entity_with_ctx(spec_create_args("Seed"), &CommitContext::internal())
        .unwrap();
    assert!(
        outcome
            .warnings
            .iter()
            .any(|w| w.code() == "CONFIG_WRITE_INTERVENED"),
        "the entity mutation must report the config intervention: {:?}",
        outcome.warnings
    );
}

/// Criterion 5: the folder backend's config write is a compare-and-set,
/// not check-then-write. A write whose `expected` no longer matches the
/// file must refuse rather than overwrite.
#[test]
fn the_folder_config_write_refuses_a_stale_expectation() {
    use crate::backend::MemBackend;
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    write_config(&mem_dir, None);
    let backend = FilesystemBackend::new(mem_dir.clone());
    let observed = backend.read_mem_config().unwrap().expect("config exists");

    // Someone else writes.
    let path = mem_dir
        .join(memstead_schema::MEM_META_DIR)
        .join("config.json");
    std::fs::write(&path, br#"{"schema": "default@1.0.0", "title": "theirs"}"#).unwrap();

    // A write against the stale expectation is refused, not applied.
    let wrote = backend
        .write_mem_config_cas(
            Some(&observed),
            b"{\"schema\": \"default@1.0.0\"}",
            &crate::vcs::CommitContext::new(
                Some("test"),
                crate::vcs::Actor::Cli,
                None,
                None,
                crate::vcs::Role::Unspecified,
                None,
            ),
        )
        .unwrap();
    assert!(!wrote, "a stale expectation must not overwrite");
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        on_disk.contains("theirs"),
        "their write survived: {on_disk}"
    );

    // And against the current bytes it lands.
    let current = backend.read_mem_config().unwrap().unwrap();
    assert!(
        backend
            .write_mem_config_cas(
                Some(&current),
                b"{\"schema\": \"default@1.0.0\"}",
                &crate::vcs::CommitContext::new(
                    Some("test"),
                    crate::vcs::Actor::Cli,
                    None,
                    None,
                    crate::vcs::Role::Unspecified,
                    None
                )
            )
            .unwrap(),
        "a current expectation writes"
    );
}

/// Criterion 7's complement: the stamp does not become a busy writer. With
/// a stamp that already agrees, an entity mutation must not touch the
/// config at all, so a sibling's write is untouched for the boring reason
/// rather than the interesting one.
#[test]
fn a_matching_stamp_still_writes_no_config_at_all() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    write_config(
        &mem_dir,
        Some(memstead_schema::MutationStamp {
            engine_version: crate::build_info::full_version().to_string(),
            schema: "default@1.0.0".to_string(),
        }),
    );
    let mut engine = stamped_engine_fixture(mem_dir.clone());
    let path = mem_dir
        .join(memstead_schema::MEM_META_DIR)
        .join("config.json");
    let before = std::fs::read(&path).unwrap();
    engine
        .create_entity_with_ctx(spec_create_args("Seed"), &CommitContext::internal())
        .unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a mutation whose stamp already matches must write no config"
    );
}

/// 04/04, criteria 9 and 10: skew reaches the write that meets it, before
/// that write's own restamp erases the evidence, and the write still
/// lands.
///
/// Boot-only detection meant a long-lived server started under one binary
/// and written to by another never said so, because the first mutation
/// both revealed and hid the fact.
#[test]
fn skew_is_reported_at_the_write_and_the_write_still_lands() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    write_config(
        &mem_dir,
        Some(memstead_schema::MutationStamp {
            engine_version: "0.0.1".to_string(),
            schema: "default@1.0.0".to_string(),
        }),
    );
    let mut engine = stamped_engine_fixture(mem_dir.clone());
    let outcome = engine
        .create_entity_with_ctx(spec_create_args("Seed"), &CommitContext::internal())
        .unwrap();

    let skew: Vec<_> = outcome
        .warnings
        .iter()
        .filter(|w| w.code() == "ENGINE_VERSION_SKEW")
        .collect();
    assert_eq!(
        skew.len(),
        1,
        "the write that meets the skew must report it: {:?}",
        outcome.warnings
    );
    assert!(
        matches!(
            skew[0],
            crate::ops::WarningHint::EngineVersionSkew {
                direction: crate::build_info::SkewDirection::StampedOlder,
                ..
            }
        ),
        "and say which way: {:?}",
        skew[0]
    );
    // Criterion 10: it landed. An older engine is not prevented from
    // writing; a deliberate downgrade is the operator's business.
    assert!(engine.get_entity(&outcome.id).is_some());
    assert_eq!(
        disk_stamp(&mem_dir).map(|s| s.engine_version),
        Some(crate::build_info::full_version().to_string()),
        "and the restamp still happened"
    );

    // Second write, same binary: nothing left to report.
    let again = engine
        .create_entity_with_ctx(spec_create_args("Second"), &CommitContext::internal())
        .unwrap();
    assert!(
        !again
            .warnings
            .iter()
            .any(|w| w.code() == "ENGINE_VERSION_SKEW"),
        "the skew is resolved once restamped: {:?}",
        again.warnings
    );
}

/// Criterion 8's complement at the write tier: a stamp from the same
/// release with a different build hash is not skew, so a workspace whose
/// binary is rebuilt from source is not told its engine disagrees on
/// every mutation.
#[test]
fn a_rebuild_of_the_same_release_is_not_skew_at_the_write() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    write_config(
        &mem_dir,
        Some(memstead_schema::MutationStamp {
            engine_version: format!("{}+gdeadbee", crate::ENGINE_VERSION),
            schema: "default@1.0.0".to_string(),
        }),
    );
    let mut engine = stamped_engine_fixture(mem_dir.clone());
    let outcome = engine
        .create_entity_with_ctx(spec_create_args("Seed"), &CommitContext::internal())
        .unwrap();
    assert!(
        !outcome
            .warnings
            .iter()
            .any(|w| w.code() == "ENGINE_VERSION_SKEW"),
        "a differing build hash on the same version is not skew: {:?}",
        outcome.warnings
    );
}

/// Criterion 3/4: boot under a different
/// binary version surfaces the warn-tier `ENGINE_VERSION_SKEW`
/// naming both versions, on load warnings AND in `health()`;
/// a stamp-less mem and a matching stamp are silent.
#[test]
fn boot_skew_warning_fires_only_on_disagreeing_stamp() {
    use crate::ops::WarningHint;

    // Disagreeing stamp → warning on boot and in health.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    write_config(
        &mem_dir,
        Some(memstead_schema::MutationStamp {
            engine_version: "0.0.1".to_string(),
            schema: "default@1.0.0".to_string(),
        }),
    );
    let engine = stamped_engine_fixture(mem_dir);
    let skew: Vec<_> = engine
        .load_warnings()
        .iter()
        .filter(|w| matches!(w, WarningHint::EngineVersionSkew { .. }))
        .collect();
    assert_eq!(skew.len(), 1, "one skewed mem, one warning: {skew:?}");
    if let WarningHint::EngineVersionSkew {
        mem,
        stamped_engine,
        running_engine,
        stamped_schema,
        direction,
    } = skew[0]
    {
        assert_eq!(mem, "specs");
        assert_eq!(stamped_engine, "0.0.1");
        assert_eq!(running_engine, crate::build_info::full_version());
        assert_eq!(stamped_schema, "default@1.0.0");
        // 0.0.1 against any shipped version: the mem is behind us.
        assert_eq!(*direction, crate::build_info::SkewDirection::StampedOlder);
    }
    let health = engine.health();
    assert!(
        health
            .warnings
            .iter()
            .any(|w| w.code() == "ENGINE_VERSION_SKEW"),
        "health() must surface the skew without an include gate: {:?}",
        health.warnings,
    );

    // Matching stamp → silent.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    write_config(
        &mem_dir,
        Some(memstead_schema::MutationStamp {
            engine_version: crate::build_info::full_version().to_string(),
            schema: "default@1.0.0".to_string(),
        }),
    );
    let engine = stamped_engine_fixture(mem_dir);
    assert!(
        !engine
            .load_warnings()
            .iter()
            .any(|w| matches!(w, WarningHint::EngineVersionSkew { .. })),
        "a matching stamp is not skew"
    );

    // No stamp → silent (absence of a stamp is not skew).
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    write_config(&mem_dir, None);
    let engine = stamped_engine_fixture(mem_dir);
    assert!(
        !engine
            .load_warnings()
            .iter()
            .any(|w| matches!(w, WarningHint::EngineVersionSkew { .. })),
        "a stamp-less (pre-plan) mem boots without warning noise"
    );
}
