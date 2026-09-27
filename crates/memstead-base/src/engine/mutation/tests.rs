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

/// A chained `section_map` carries every mark. Retype refuses only
/// collisions, so `{a: b, b: c}` and `{a: b, b: a}` are legal, and a loop
/// that mutated the marks in place read its own write for `b` as `b`'s
/// original hash: it destroyed the real one and left the content at the end
/// of the chain unmarked. Nothing looked wrong at the time, because the
/// entity still read third-party on the surviving mark; it broke the moment
/// the owner rewrote that one section.
#[test]
fn a_chained_section_map_carries_every_mark() {
    use crate::preparation::entity_section_prepared_hash;

    let a_content = "the proposer's first claim";
    let b_content = "the proposer's second claim";
    let marks = std::collections::BTreeMap::from([
        (
            "a".to_string(),
            entity_section_prepared_hash("a", a_content),
        ),
        (
            "b".to_string(),
            entity_section_prepared_hash("b", b_content),
        ),
    ]);
    // After the retype the content has moved a→b and b→c, unchanged.
    let sections = indexmap::IndexMap::from([
        ("b".to_string(), a_content.to_string()),
        ("c".to_string(), b_content.to_string()),
    ]);
    let section_map = indexmap::IndexMap::from([
        ("a".to_string(), "b".to_string()),
        ("b".to_string(), "c".to_string()),
    ]);

    let carried = super::rekey_marks_for_retype(&marks, &section_map, &sections);
    assert_eq!(
        carried.get("b").cloned(),
        Some(entity_section_prepared_hash("b", a_content)),
        "the first section's mark follows to its new key"
    );
    assert_eq!(
        carried.get("c").cloned(),
        Some(entity_section_prepared_hash("c", b_content)),
        "and the second's is not eaten by the first: reading its own write dropped this one"
    );
    assert_eq!(carried.len(), 2, "both marks survive: {carried:?}");
}

/// A retype where nothing holds any more RETIRES the mark instead of writing
/// an empty map. An empty map is a record defect the read path fails the whole
/// mem closed on, so the engine's own retype must not manufacture one: adopt a
/// contribution, rewrite every load-bearing section in your own words, then
/// decide it is really another type is an ordinary owner sequence, and it
/// would have bricked the mem's origin labelling.
#[test]
fn a_retype_after_a_full_rewrite_retires_the_mark_rather_than_emptying_it() {
    use crate::preparation::entity_section_prepared_hash;

    let marks = std::collections::BTreeMap::from([(
        "identity".to_string(),
        entity_section_prepared_hash("identity", "the proposer's claim"),
    )]);
    // The owner has since rewritten the section, so nothing holds.
    let sections =
        indexmap::IndexMap::from([("claim".to_string(), "the owner's own account".to_string())]);
    let section_map = indexmap::IndexMap::from([("identity".to_string(), "claim".to_string())]);

    let carried = super::rekey_marks_for_retype(&marks, &section_map, &sections);
    assert!(
        carried.is_empty(),
        "nothing holds, so nothing is carried: {carried:?}"
    );
    // The caller must turn that into a retirement, never an empty map; the
    // end-to-end assertion is `a_retype_leaves_the_record_readable` on the
    // git-branch side.
}

/// Every engine write that MOVES a marked section's bytes must carry the
/// adopted body's origin marks with it, and that list has now been wrong three
/// times: a rename's referrer rewrite, a mem rename's sweep, and an export's
/// retarget were each found serving a contributor's untouched sentences as the
/// workspace's own after the fact.
///
/// The cause cannot be removed: the rewrite helpers are pure text functions
/// with no backend in hand, so the carry has to live at each call site. So this
/// enumerates them. A new call site fails this test, which is the prompt to
/// carry the marks there or to record why the site cannot move marked bytes.
///
/// Owner: whoever adds the call site. Sunset: the day the rewrite helpers take
/// the record along themselves, at which point the enumeration is dead.
#[test]
fn every_wikilink_rewrite_call_site_is_accounted_for() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found: Vec<String> = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable source dir") {
            let path = entry.expect("readable entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let rel = path
                .strip_prefix(&root)
                .expect("under src")
                .to_string_lossy()
                .replace('\\', "/");
            // The helpers' own module, and test modules, are not write paths.
            if rel.contains("wikilink_rewrite") || rel.contains("tests") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("utf-8 source");
            for (n, line) in text.lines().enumerate() {
                let calls = [
                    "rewrite_mem_prefix(",
                    "rewrite_cross_mem_slug(",
                    "rewrite_bare_slug(",
                ];
                if calls.iter().any(|c| line.contains(c)) {
                    found.push(format!("{rel}:{}", n + 1));
                }
            }
        }
    }
    found.sort();
    // Each of these sites carries the marks: the rename's self-link and
    // referrer rewrites through `stage_proposal_marks_rehash`, the sweep
    // through the same, and the export through `rekey_proposal_marks`.
    let accounted = [
        "engine/mutation/mem_sweep.rs",
        "engine/mutation/rename.rs",
        "ops/export.rs",
    ];
    let unaccounted: Vec<&String> = found
        .iter()
        .filter(|site| !accounted.iter().any(|a| site.starts_with(a)))
        .collect();
    assert!(
        unaccounted.is_empty(),
        "a wiki-link rewrite call site appeared in a file that does not carry an adopted \
         body's origin marks: {unaccounted:?}. Carry them (see `stage_proposal_marks_rehash`) \
         or add the file here with the reason it cannot move marked bytes."
    );
    assert!(
        found.len() >= 6,
        "the enumeration found {} sites, fewer than the known set: the grep or the helpers \
         were renamed, and this guard has stopped guarding",
        found.len()
    );
}

/// Renaming onto a slug the record already mentions moves the marks and
/// leaves the audit trail alone. The target entry's disposition, reason and
/// content hash document the entity that WAS there; only its marks move,
/// because only they describe the bytes that are there now. The source's
/// marks are cleared, so the label moves rather than being copied.
#[test]
fn a_rename_onto_a_recorded_slug_moves_the_marks_and_keeps_the_audit_trail() {
    let source_marks = std::collections::BTreeMap::from([(
        "identity".to_string(),
        crate::preparation::entity_section_prepared_hash("identity", "the proposer's claim"),
    )]);
    let stale_marks = std::collections::BTreeMap::from([(
        "identity".to_string(),
        "the deleted entity's hash".to_string(),
    )]);
    let record = record_of(vec![vec![
        (
            "x",
            "adopt",
            Some("x's reason"),
            Some("hx"),
            Some(source_marks.clone()),
        ),
        (
            "y",
            "adopt",
            Some("y's reason"),
            Some("hy"),
            Some(stale_marks),
        ),
    ]]);

    let (carried, moved) = super::move_marks_on_rename(record, "x", "y");
    assert!(moved, "the marks moved, so the record is written");
    let entry = &carried.proposals[0].entities["y"];
    assert_eq!(
        entry.landed_sections.as_ref(),
        Some(&source_marks),
        "the marks describe the body that is now at this slug"
    );
    assert_eq!(
        entry.reason.as_deref(),
        Some("y's reason"),
        "the audit trail of the entity that was here is untouched"
    );
    assert_eq!(
        entry.content_hash.as_deref(),
        Some("hy"),
        "and so is its hash"
    );
    assert!(
        carried.proposals[0].entities["x"].landed_sections.is_none(),
        "and the source no longer claims the label: this is a move, not a copy"
    );
}

/// A LATER proposal's mark-less entry on the rename target does not retire
/// the marks the rename just moved there. The read flattens every proposal in
/// order, so deciding the rename per proposal left the last word to an
/// unrelated merge: adopt `beta`, later land an `adopt_with_changes` on
/// `alpha`, delete `alpha`, rename `beta` onto the freed slug, and the
/// contributor's untouched body read first-party.
#[test]
fn a_later_proposals_entry_cannot_retire_marks_a_rename_moved() {
    let marks = std::collections::BTreeMap::from([(
        "identity".to_string(),
        crate::preparation::entity_section_prepared_hash("identity", "the proposer's claim"),
    )]);
    let record = record_of(vec![
        vec![("beta", "adopt", None, None, Some(marks.clone()))],
        // Two later merges whose own bodies the owner typed: no marks, and
        // each read as a retirement of whatever sits at that slug. Two, so
        // that writing the moved marks onto the FIRST entry mentioning the
        // target is not enough: the second would retire them again.
        vec![(
            "alpha",
            "adopt_with_changes",
            Some("merged by hand"),
            None,
            None,
        )],
        vec![("alpha", "adopt_with_changes", Some("and again"), None, None)],
    ]);

    let (carried, moved) = super::move_marks_on_rename(record, "beta", "alpha");
    assert!(moved);
    assert_eq!(
        carried.effective_marks("alpha").as_ref(),
        Some(&marks),
        "the flattened outcome for the target is the moved marks, whatever the order"
    );
    assert!(
        carried.effective_marks("beta").is_none(),
        "and the source flattens to nothing"
    );
}

/// A rename of an entity the record marks nothing about changes nothing: no
/// write, so a mark-less entry's slug does not move by accident, which it did
/// whenever some unrelated proposal in the same record happened to carry a
/// mark.
#[test]
fn a_rename_without_marks_leaves_the_record_alone() {
    let record = record_of(vec![vec![
        ("x", "reject", Some("not this time"), Some("hx"), None),
        (
            "z",
            "adopt",
            None,
            None,
            Some(std::collections::BTreeMap::from([(
                "identity".to_string(),
                "zzz".to_string(),
            )])),
        ),
    ]]);
    let (carried, moved) = super::move_marks_on_rename(record.clone(), "x", "y");
    assert!(
        !moved,
        "nothing effective at the source, so nothing is written"
    );
    assert_eq!(carried, record, "and the record is untouched");
}

/// One entry of a test record: slug, disposition, reason, content hash, marks.
type RecordedEntry<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    Option<std::collections::BTreeMap<String, String>>,
);

/// Build a record from a list of proposals, each a list of entries.
fn record_of(proposals: Vec<Vec<RecordedEntry<'_>>>) -> crate::ops::proposal::ProposalRecord {
    use crate::ops::proposal::{ProposalRecord, ProposalRecordEntry, RecordedDisposition};
    ProposalRecord {
        proposals: proposals
            .into_iter()
            .enumerate()
            .map(|(i, entries)| ProposalRecordEntry {
                id: format!("fork@{i}"),
                entities: entries
                    .into_iter()
                    .map(|(slug, disposition, reason, hash, marks)| {
                        (
                            slug.to_string(),
                            RecordedDisposition {
                                disposition: disposition.to_string(),
                                reason: reason.map(str::to_string),
                                content_hash: hash.map(str::to_string),
                                landed_sections: marks,
                            },
                        )
                    })
                    .collect(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

/// A rename refuses a record the READER cannot make sense of, instead of
/// repairing it. An entry with a structurally empty mark map makes the read
/// serve every entity of the mem third-party on purpose; a rename that cleared
/// or overwrote that entry lifted the quarantine and the mem's own entities
/// went back to first-party, with the evidence gone from the file.
#[test]
fn a_rename_refuses_a_record_the_reader_calls_defective() {
    let record = record_of(vec![
        vec![(
            "beta",
            "adopt",
            None,
            None,
            Some(std::collections::BTreeMap::new()),
        )],
        vec![(
            "beta",
            "adopt",
            None,
            None,
            Some(std::collections::BTreeMap::from([(
                "identity".to_string(),
                "real".to_string(),
            )])),
        )],
    ]);
    assert!(
        record.structural_defect().is_some(),
        "the fixture is the defect the read fails closed on"
    );
    let (carried, moved) = super::move_marks_on_rename(record.clone(), "beta", "delta");
    assert!(!moved, "nothing is written");
    assert_eq!(
        carried, record,
        "and the evidence of the defect is left exactly as it was"
    );
}

/// The relocated row is the one whose marks were EFFECTIVE, not the first row
/// that happens to name the slug. Moving the first moved a `reject`'s reason
/// and content hash onto another slug, which takes the re-proposal gate with
/// it: a reworded resubmission at the rejected slug would then land with no
/// mark on the brief, and a slug nothing was ever rejected at would carry one.
#[test]
fn the_relocated_row_is_the_one_that_carried_the_marks() {
    let marks = std::collections::BTreeMap::from([(
        "identity".to_string(),
        crate::preparation::entity_section_prepared_hash("identity", "the proposer's claim"),
    )]);
    let record = record_of(vec![
        vec![("beta", "reject", Some("not this time"), Some("h1"), None)],
        vec![("beta", "adopt", None, None, Some(marks.clone()))],
    ]);

    let (carried, moved) = super::move_marks_on_rename(record, "beta", "gamma");
    assert!(moved);
    assert_eq!(
        carried.proposals[0].entities["beta"].reason.as_deref(),
        Some("not this time"),
        "the rejection stays on the slug it was recorded for"
    );
    assert!(
        !carried.proposals[0].entities.contains_key("gamma"),
        "and no rejection is invented for the new slug"
    );
    assert_eq!(
        carried.effective_marks("gamma").as_ref(),
        Some(&marks),
        "while the adopted marks did move"
    );
}

/// Every write that follows a mark refuses a record the reader calls
/// defective, not only the rename. The doc on `structural_defect` states that
/// as a property of the design, and a doc comment standing in for a guarantee
/// is what produced five rounds of one-position-over defects.
#[test]
fn every_mark_following_writer_refuses_a_defective_record() {
    let defective = record_of(vec![vec![(
        "beta",
        "adopt",
        None,
        None,
        Some(std::collections::BTreeMap::new()),
    )]]);
    assert!(defective.structural_defect().is_some());

    // The rename.
    let (out, moved) = super::move_marks_on_rename(defective.clone(), "beta", "delta");
    assert!(!moved && out == defective, "rename refuses");

    // The retype re-key, at the level it decides: an empty map carries
    // nothing, so the caller's guard is what keeps it from writing.
    let carried = super::rekey_marks_for_retype(
        &std::collections::BTreeMap::new(),
        &indexmap::IndexMap::from([("a".to_string(), "b".to_string())]),
        &indexmap::IndexMap::from([("b".to_string(), "x".to_string())]),
    );
    assert!(carried.is_empty());

    // The export's carry.
    let bytes = defective.to_bytes();
    let out = crate::ops::export::rekey_proposal_marks(
        Some(bytes.clone()),
        &[(
            "beta".to_string(),
            indexmap::IndexMap::from([("identity".to_string(), "a".to_string())]),
            indexmap::IndexMap::from([("identity".to_string(), "b".to_string())]),
        )],
    );
    assert_eq!(out, Some(bytes), "export refuses");
}
