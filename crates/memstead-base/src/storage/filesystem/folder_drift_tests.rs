#![cfg(test)]

use super::*;

const ENTITY: &str = "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Seed\n\n## Identity\n\nSeed.\n";
const SIBLING_ENTITY: &str = "---\ntype: spec\ncreated_date: 2026-01-02\nlast_modified: 2026-01-02\nlevel: M0\n---\n# Sibling\n\n## Identity\n\nWritten out-of-band.\n";

fn folder_engine(dir: std::path::PathBuf) -> crate::Engine {
    let mount = crate::Mount {
        mem: "specs".to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            "default",
            semver::Version::new(1, 0, 0),
        )),
        storage: crate::MountStorage::Folder { path: dir.clone() },
        capability: crate::MountCapability::Write,
        lifecycle: crate::MountLifecycle::Eager,
        cross_linkable: false,
        migration_target: None,
    };
    let backend = Box::new(FilesystemBackend::new(dir)) as Box<dyn crate::MemBackend>;
    crate::Engine::from_mounts(vec![(mount, backend)]).unwrap()
}

/// A sibling process's folder commit is drift: the changelog-ts
/// cursor advances, `reload_if_stale` reloads, surfaces
/// MEM_RELOADED, stashes the structured notice — and the engine's
/// own writes never masquerade as drift (the recorded head is
/// probe-corrected to the cursor dialect).
#[test]
fn sibling_folder_write_is_drift_and_self_write_is_not() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("specs");
    std::fs::create_dir_all(&dir).unwrap();
    // Seed through a writer WITH provenance so a baseline cursor exists.
    let seeder = FilesystemBackend::new(dir.clone());
    MemBackend::write_entity(&seeder, std::path::Path::new("seed.md"), ENTITY.as_bytes()).unwrap();
    MemBackend::commit(&seeder, "seed", &CommitContext::internal()).unwrap();
    crate::backend::MemBackend::append_provenance(
        &seeder,
        &Provenance::new(
            std::time::SystemTime::now(),
            ProvenanceKind::Create,
            Some("specs--seed".into()),
            Actor::Cli,
            None,
            None,
        ),
    )
    .unwrap();

    let mut engine = folder_engine(dir.clone());
    // First probe captures the baseline silently.
    assert!(engine.reload_if_stale(None).is_empty());

    // Self-write through the engine: no spurious drift afterwards.
    engine
        .create_entity(
            crate::CreateEntityArgs {
                mem: "specs".to_string(),
                title: "Self Made".to_string(),
                entity_type: "spec".to_string(),
                sections: [
                    ("identity".to_string(), "self".to_string()),
                    ("purpose".to_string(), "prove no self-drift".to_string()),
                ]
                .into_iter()
                .collect(),
                metadata: Default::default(),
                relations: Vec::new(),
                anchors: Vec::new(),
                dry_run: false,
            },
            crate::vcs::Actor::Cli,
            None,
            None,
        )
        .unwrap();
    let mut op = crate::OperationScope::begin(&mut engine);
    assert!(
        op.reload_if_stale(None).is_empty(),
        "the engine's own write must not read as sibling drift"
    );
    let (_, notices) = op.finish();
    assert!(notices.is_empty());

    // Sibling write: a separate writer instance (a stand-in for a
    // second process) commits + appends provenance out-of-band.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let sibling = FilesystemBackend::new(dir);
    MemBackend::write_entity(
        &sibling,
        std::path::Path::new("sibling.md"),
        SIBLING_ENTITY.as_bytes(),
    )
    .unwrap();
    MemBackend::commit(&sibling, "sibling", &CommitContext::internal()).unwrap();
    crate::backend::MemBackend::append_provenance(
        &sibling,
        &Provenance::new(
            std::time::SystemTime::now(),
            ProvenanceKind::Create,
            Some("specs--sibling".into()),
            Actor::Cli,
            None,
            None,
        ),
    )
    .unwrap();

    let mut op = crate::OperationScope::begin(&mut engine);
    let warnings = op.reload_if_stale(None);
    assert_eq!(
        warnings.len(),
        1,
        "sibling drift must surface: {warnings:?}"
    );
    match &warnings[0] {
        crate::ops::WarningHint::MemReloaded { mem, .. } => assert_eq!(mem, "specs"),
        other => panic!("expected MemReloaded, got {other:?}"),
    }
    let (_, notices) = op.finish();
    assert_eq!(notices.len(), 1);
    // Post-reload the sibling entity is visible.
    assert!(
        engine
            .get_entity(&crate::EntityId("specs--sibling".to_string()))
            .is_some(),
        "reload must surface the sibling's entity"
    );
    // Idempotent probe: no repeat notice, and a second scope over
    // the same engine starts empty — the first operation's notice
    // cannot be inherited.
    let mut second = crate::OperationScope::begin(&mut engine);
    assert!(second.reload_if_stale(None).is_empty());
    let (_, notices) = second.finish();
    assert!(notices.is_empty(), "a later scope inherits nothing");

    // A notice recorded OUTSIDE any scope (a caller that reloaded
    // without opening one) is discarded when the next scope opens:
    // the scope is the only channel, and it never hands out what
    // another operation produced.
    std::thread::sleep(std::time::Duration::from_millis(5));
    MemBackend::write_entity(
        &sibling,
        std::path::Path::new("sibling-two.md"),
        SIBLING_ENTITY
            .replace("specs--sibling", "specs--sibling-two")
            .replace("# Sibling", "# Sibling two")
            .as_bytes(),
    )
    .unwrap();
    MemBackend::commit(&sibling, "sibling two", &CommitContext::internal()).unwrap();
    crate::backend::MemBackend::append_provenance(
        &sibling,
        &Provenance::new(
            std::time::SystemTime::now(),
            ProvenanceKind::Create,
            Some("specs--sibling-two".into()),
            Actor::Cli,
            None,
            None,
        ),
    )
    .unwrap();
    let unscoped = engine.reload_if_stale(None);
    assert_eq!(unscoped.len(), 1, "the unscoped reload still reports drift");
    let (_, notices) = crate::OperationScope::begin(&mut engine).finish();
    assert!(
        notices.is_empty(),
        "the next scope discards what an unscoped reload recorded: {notices:?}"
    );
}

/// On a folder mem the detailed notice names the newest writer and its
/// note from the change ledger: the last row that names the entity
/// inside the notice's range. A write that declared neither yields an
/// entry with neither key.
#[test]
fn a_folder_notice_names_the_newest_writer_and_its_note() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("specs");
    std::fs::create_dir_all(&dir).unwrap();
    let seeder = FilesystemBackend::new(dir.clone());
    MemBackend::write_entity(&seeder, std::path::Path::new("seed.md"), ENTITY.as_bytes()).unwrap();
    MemBackend::commit(&seeder, "seed", &CommitContext::internal()).unwrap();
    crate::backend::MemBackend::append_provenance(
        &seeder,
        &Provenance::new(
            std::time::SystemTime::now(),
            ProvenanceKind::Create,
            Some("specs--seed".into()),
            Actor::Cli,
            None,
            None,
        ),
    )
    .unwrap();

    let mut engine = folder_engine(dir.clone());
    assert!(engine.reload_if_stale(None).is_empty());
    let mut writer = folder_engine(dir);
    let create = |writer: &mut crate::Engine, title: &str, note: Option<&str>| {
        std::thread::sleep(std::time::Duration::from_millis(5));
        writer
            .create_entity(
                crate::CreateEntityArgs {
                    mem: "specs".to_string(),
                    title: title.to_string(),
                    entity_type: "spec".to_string(),
                    sections: [
                        ("identity".to_string(), "i".to_string()),
                        ("purpose".to_string(), "p".to_string()),
                    ]
                    .into_iter()
                    .collect(),
                    metadata: Default::default(),
                    relations: Vec::new(),
                    anchors: Vec::new(),
                    dry_run: false,
                },
                crate::vcs::Actor::Cli,
                None,
                note,
            )
            .unwrap();
    };
    writer.reload_if_stale(None);
    writer.set_identity(Some("xavier".to_string()));
    create(&mut writer, "With Both", Some("a folder note"));
    writer.set_identity(None);
    create(&mut writer, "With Neither", None);

    let mut op = crate::OperationScope::begin(&mut engine);
    op.reload_if_stale(None);
    let (_, notices) = op.finish();
    assert_eq!(notices.len(), 1, "{notices:?}");
    let value = serde_json::to_value(&notices[0]).unwrap();
    assert_eq!(value["changes"]["mode"], "detailed", "{value}");
    let entry = |id: &str| {
        value["changes"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap_or_else(|| panic!("an entry for {id}: {value}"))
            .clone()
    };
    let both = entry("specs--with-both");
    assert_eq!(both["identity"], "xavier");
    assert_eq!(both["note"], "a folder note");
    let neither = entry("specs--with-neither");
    assert!(neither.get("identity").is_none(), "{neither}");
    assert!(neither.get("note").is_none(), "{neither}");
}
