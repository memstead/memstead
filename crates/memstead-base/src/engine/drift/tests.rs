#![cfg(test)]

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::backend::{BackendError, MemBackend};
use crate::engine::test_helpers::*;
use crate::engine::{DeleteEntityArgs, Engine, EngineError};
use crate::entity::EntityId;

use crate::provenance::Provenance;
use crate::storage::ArchiveBackend;
use crate::vcs::CommitContext;
use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

#[test]
fn engine_diff_unknown_mem_returns_typed_error() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    let err = engine.diff("nope", "a", "b", None).unwrap_err();
    assert!(matches!(err, EngineError::UnknownMem { name: v, .. } if v == "nope"));
}

/// A `write_id` from a mutation response passed back as `since`
/// on a folder mem refuses with the typed `INVALID_CURSOR` code
/// and a message naming both the right cursor and the confusion —
/// before this guard it silently replayed the whole history.
#[test]
fn engine_changes_since_folder_refuses_write_token_as_cursor() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    let token = format!("{:032x}{:016x}", 1_766_000_000_000_000_000u128, 7u64);
    let err = engine.changes_since("specs", &token, None).unwrap_err();
    assert_eq!(err.code(), "INVALID_CURSOR");
    match &err {
        EngineError::InvalidTimestampCursor { mem, since } => {
            assert_eq!(mem, "specs");
            assert_eq!(since, &token);
        }
        other => panic!("expected InvalidTimestampCursor, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("RFC3339"),
        "message names the cursor dialect: {msg}"
    );
    assert!(
        msg.contains("`write_id` is an identity, not a cursor"),
        "message names the confusion: {msg}"
    );
    // The sentinel and a real timestamp still read.
    assert!(
        engine
            .changes_since("specs", crate::ops::EMPTY_TREE_SHA, None)
            .is_ok()
    );
    assert!(engine.changes_since("specs", "", None).is_ok());
}

#[test]
fn engine_diff_folder_mount_refuses_with_invalid_input() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    // Folder backend has no git refs — refuse cleanly via the
    // typed `INVALID_INPUT` code rather than collapsing through
    // the backend layer.
    let err = engine.diff("specs", "a", "b", None).unwrap_err();
    assert_eq!(err.code(), "INVALID_INPUT");
    match err {
        EngineError::NotGitBacked { mem, operation } => {
            assert_eq!(mem.as_deref(), Some("specs"));
            assert_eq!(operation, "memstead_diff");
        }
        other => panic!("expected NotGitBacked, got {other:?}"),
    }
}

#[test]
fn engine_diff_rename_similarity_out_of_range_refuses() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    let bad = crate::ops::DiffConfig {
        rename_similarity: 2.0,
        ..Default::default()
    };
    let err = engine.diff("specs", "a", "b", Some(bad)).unwrap_err();
    assert!(matches!(
        err,
        EngineError::RenameSimilarityOutOfRange { .. }
    ));
}

#[test]
fn engine_changes_since_archive_mount_returns_empty_report() {
    // Archive backends have no diff surface; the engine wrapper
    // produces an empty `ChangesReport` with the cursor echoed.
    let tmp = TempDir::new().unwrap();
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);
    let mount = archive_mount("ext", archive_path.clone());
    let engine = Engine::from_mounts(vec![(
        mount,
        Box::new(ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let report = engine.changes_since("ext", "abc", None).expect("known mem");
    assert_eq!(report.mem, "ext");
    assert_eq!(report.since, "abc");
    assert_eq!(report.head, "abc");
    assert!(report.changes.is_empty());
    assert!(report.warnings.is_empty());
}

#[test]
fn engine_changes_since_unknown_mem_returns_typed_error() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    let err = engine
        .changes_since("does-not-exist", "abc", None)
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownMem { .. }));
}

#[test]
fn engine_changes_since_refuses_rename_similarity_below_min() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    // 0.05 is below RENAME_SIMILARITY_MIN (0.1); typed refusal,
    // not a silent clamp.
    let err = engine
        .changes_since("specs", "abc", Some(0.05))
        .expect_err("out-of-range refuses");
    match err {
        EngineError::RenameSimilarityOutOfRange {
            requested,
            allowed_min,
            allowed_max,
        } => {
            assert!((requested - 0.05).abs() < f32::EPSILON);
            assert!((allowed_min - crate::ops::RENAME_SIMILARITY_MIN).abs() < f32::EPSILON);
            assert!((allowed_max - crate::ops::RENAME_SIMILARITY_MAX).abs() < f32::EPSILON);
        }
        other => panic!("expected RenameSimilarityOutOfRange, got {other:?}"),
    }
}

#[test]
fn engine_changes_since_refuses_rename_similarity_above_max() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    // 1.5 is above RENAME_SIMILARITY_MAX (1.0); typed refusal.
    let err = engine
        .changes_since("specs", "abc", Some(1.5))
        .expect_err("out-of-range refuses");
    match err {
        EngineError::RenameSimilarityOutOfRange { requested, .. } => {
            assert!((requested - 1.5).abs() < f32::EPSILON);
        }
        other => panic!("expected RenameSimilarityOutOfRange, got {other:?}"),
    }
}

#[test]
fn engine_changes_since_no_warning_when_rename_similarity_in_range() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    // 0.5 is comfortably inside the valid range; no warning.
    let report = engine
        .changes_since("specs", "", Some(0.5))
        .expect("known mem");
    assert!(report.warnings.is_empty());
}

#[test]
fn engine_changes_since_no_warning_when_rename_similarity_omitted() {
    // Caller passes None → wrapper falls back to the default;
    // no clamping, no warning.
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    let report = engine.changes_since("specs", "", None).expect("known mem");
    assert!(report.warnings.is_empty());
}

#[test]
fn engine_changes_since_enriches_envelope_title_and_type_from_store() {
    // `build_demo_engine` creates three entities via the engine's
    // mutation pipeline, which appends Create events to the folder
    // backend's changelog. `Engine::changes_since` synthesises
    // BackendChanges from the changelog (id-only envelopes), then
    // enriches title / entity_type from the in-memory store.
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    let report = engine
        .changes_since("specs", crate::ops::EMPTY_TREE_SHA, None)
        .expect("known mem");

    // Three Create events → three Added envelopes, each enriched.
    assert_eq!(report.changes.len(), 3);
    for env in &report.changes {
        match env {
            crate::ops::ChangeEnvelope::Added {
                id,
                title,
                entity_type,
            } => {
                assert!(title.is_some(), "title enriched for {id}");
                assert_eq!(entity_type.as_deref(), Some("spec"), "type for {id}");
            }
            other => panic!("expected Added envelope, got {other:?}"),
        }
    }
}

#[test]
fn engine_changes_since_removed_envelope_keeps_title_and_type_none() {
    // Create-then-delete net effect = Removed. Even though the
    // store may still know the entity, the engine wrapper
    // unconditionally strips title / entity_type on Removed.
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let (actor, client) = cli_actor();
    let id = EntityId::new("specs", "lonely-three");
    let hash = engine
        .get_entity(&id)
        .expect("seeded entity present")
        .content_hash
        .clone();
    engine
        .delete_entity(
            DeleteEntityArgs {
                id: id.clone(),
                expected_hash: crate::ExpectedHash::Current(hash),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let report = engine
        .changes_since("specs", crate::ops::EMPTY_TREE_SHA, None)
        .unwrap();
    let removed = report
        .changes
        .iter()
        .find(|e| {
            matches!(e,
                crate::ops::ChangeEnvelope::Removed { id: rid, .. } if rid == &id)
        })
        .expect("removed envelope for lonely-three");
    match removed {
        crate::ops::ChangeEnvelope::Removed {
            title, entity_type, ..
        } => {
            assert!(title.is_none());
            assert!(entity_type.is_none());
        }
        other => panic!("expected Removed, got {other:?}"),
    }
}

// ---- Engine::cross_mem_link_allowed ---------------------------

#[test]
fn reload_if_stale_returns_empty_for_folder_only_engine() {
    // Folder mems now carry a changelog-derived drift cursor, so
    // this pins the QUIET case: no sibling wrote between probes,
    // so repeated checks stay warning-free (the first probe
    // captures the baseline silently, the second sees no advance).
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let warnings = engine.reload_if_stale(None);
    assert!(warnings.is_empty());
    let warnings = engine.reload_if_stale(Some("specs"));
    assert!(warnings.is_empty());
}

#[test]
fn reload_if_stale_short_circuits_for_unknown_mem_filter() {
    // Filtering by an unknown mem produces zero candidates;
    // the method returns an empty Vec without panicking.
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let warnings = engine.reload_if_stale(Some("does-not-exist"));
    assert!(warnings.is_empty());
}

/// Test fixture: a `MemBackend` whose `current_head` and
/// (read-side) entity surface are externally mutable so a test
/// can simulate a sibling writer advancing the head between
/// drift-check probes. Write methods are no-ops; the engine's
/// drift-check path never invokes them.
struct ManualHeadBackend {
    head: std::sync::Mutex<Option<String>>,
    entities: std::sync::Mutex<Vec<(PathBuf, Vec<u8>)>>,
}

impl ManualHeadBackend {
    fn new(initial_head: Option<&str>) -> Self {
        Self {
            head: std::sync::Mutex::new(initial_head.map(String::from)),
            entities: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn set_head(&self, head: Option<&str>) {
        *self.head.lock().unwrap() = head.map(String::from);
    }
}

impl MemBackend for ManualHeadBackend {
    fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
        Ok(self
            .entities
            .lock()
            .unwrap()
            .iter()
            .map(|(p, _)| p.clone())
            .collect())
    }
    fn read_entity(&self, rel: &Path) -> Result<Option<Vec<u8>>, BackendError> {
        Ok(self
            .entities
            .lock()
            .unwrap()
            .iter()
            .find(|(p, _)| p == rel)
            .map(|(_, b)| b.clone()))
    }
    fn write_entity(&self, _: &Path, _: &[u8]) -> Result<(), BackendError> {
        Ok(())
    }
    fn delete_entity(&self, _: &Path) -> Result<(), BackendError> {
        Ok(())
    }
    fn move_entity(&self, _: &Path, _: &Path) -> Result<(), BackendError> {
        Ok(())
    }
    fn commit(
        &self,
        _: &str,
        _: &CommitContext<'_>,
    ) -> Result<crate::storage::CommitId, BackendError> {
        Ok("synthetic".to_string())
    }
    fn stage_provenance(&self, _: &Provenance) -> Result<(), BackendError> {
        Ok(())
    }
    fn append_provenance(&self, _: &Provenance) -> Result<(), BackendError> {
        Ok(())
    }
    fn read_provenance(&self, _: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
        Ok(Vec::new())
    }
    fn current_head(&self) -> Result<Option<String>, BackendError> {
        Ok(self.head.lock().unwrap().clone())
    }
}

/// A `ManualHeadBackend` shared with the test so the head can move
/// after the engine took ownership of the mount.
struct SharedHeadBackend(std::sync::Arc<ManualHeadBackend>);
impl MemBackend for SharedHeadBackend {
    fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
        self.0.list_entities()
    }
    fn read_entity(&self, rel: &Path) -> Result<Option<Vec<u8>>, BackendError> {
        self.0.read_entity(rel)
    }
    fn write_entity(&self, p: &Path, b: &[u8]) -> Result<(), BackendError> {
        self.0.write_entity(p, b)
    }
    fn delete_entity(&self, p: &Path) -> Result<(), BackendError> {
        self.0.delete_entity(p)
    }
    fn move_entity(&self, f: &Path, t: &Path) -> Result<(), BackendError> {
        self.0.move_entity(f, t)
    }
    fn commit(
        &self,
        m: &str,
        c: &CommitContext<'_>,
    ) -> Result<crate::storage::CommitId, BackendError> {
        self.0.commit(m, c)
    }
    fn stage_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
        self.0.stage_provenance(r)
    }
    fn append_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
        self.0.append_provenance(r)
    }
    fn read_provenance(&self, c: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
        self.0.read_provenance(c)
    }
    fn current_head(&self) -> Result<Option<String>, BackendError> {
        self.0.current_head()
    }
}

/// A mount whose branch was missing at boot loads empty and warns
/// `MOUNT_UNBACKED`; when the branch appears (born, fetched, pushed
/// into place by another process) the next probe reloads it, the
/// entities the branch holds are served, the warning is gone, and
/// the reload names the head the branch appeared at.
/// The folder feed through the engine, on a real ledger: an entity
/// created and then renamed arrives once; from a cursor between the
/// two writes it arrives as `renamed` with both ids; either way the
/// event carries the title and type of the entity a reader can
/// fetch, and no event names an id the store cannot serve.
#[test]
fn folder_feed_serves_a_renamed_entity_once_with_its_final_id() {
    use crate::engine::test_helpers::*;
    use crate::engine::{CreateEntityArgs, RenameEntityArgs};
    use crate::storage::FilesystemBackend;
    let tmp = tempfile::TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(mem_dir.clone());
    let (actor, client) = cli_actor();

    let args: CreateEntityArgs = empty_create_args("specs", "Alpha One");
    let created = engine
        .create_entity(args, actor, Some(&client), Some("seed"))
        .unwrap();
    let old_id = created.id.clone();
    let keeper = engine
        .create_entity(
            empty_create_args("specs", "Keeper"),
            actor,
            Some(&client),
            Some("seed"),
        )
        .unwrap();
    let between = engine
        .changes_since("specs", "", None)
        .unwrap()
        .head
        .clone();
    // The ledger's millisecond clock must move past `between`.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let renamed = engine
        .rename_entity(
            RenameEntityArgs {
                id: old_id.clone(),
                expected_hash: crate::ExpectedHash::Current(created.content_hash.clone()),
                new_title: "Alpha Two".to_string(),
            },
            actor,
            Some(&client),
            Some("rename"),
        )
        .unwrap();
    let new_id = renamed.new_id.clone();

    // Whole window: one added under the final id, one for keeper.
    let whole = engine.changes_since("specs", "", None).unwrap();
    let mut ids: Vec<(String, String)> = whole
        .changes
        .iter()
        .map(|c| (c.action().to_string(), c.primary_id().to_string()))
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            ("added".to_string(), new_id.0.clone()),
            ("added".to_string(), keeper.id.0.clone()),
        ],
        "{:?}",
        whole.changes
    );
    for c in &whole.changes {
        assert!(
            engine
                .store()
                .get(&EntityId(c.primary_id().to_string()))
                .is_some(),
            "every event names an id the store serves: {c:?}"
        );
        let titled = matches!(
            c,
            crate::ops::ChangeEnvelope::Added { title: Some(_), .. }
                | crate::ops::ChangeEnvelope::Updated { title: Some(_), .. }
                | crate::ops::ChangeEnvelope::Renamed { title: Some(_), .. }
        );
        assert!(titled && c.entity_type().is_some(), "{c:?}");
    }

    // From between the add and the rename: the renamed event alone.
    let later = engine.changes_since("specs", &between, None).unwrap();
    match later.changes.as_slice() {
        [
            crate::ops::ChangeEnvelope::Renamed {
                from_id,
                to_id,
                title,
                entity_type,
            },
        ] => {
            assert_eq!(from_id, &old_id);
            assert_eq!(to_id, &new_id);
            assert_eq!(title.as_deref(), Some("Alpha Two"));
            assert!(entity_type.is_some());
        }
        other => panic!("expected the renamed event alone, got {other:?}"),
    }
    // The head round-trips to silence.
    let quiet = engine.changes_since("specs", &later.head, None).unwrap();
    assert!(quiet.changes.is_empty(), "{:?}", quiet.changes);
}

#[test]
fn reload_if_stale_reloads_a_mem_whose_branch_appeared() {
    let shared = std::sync::Arc::new(ManualHeadBackend::new(None));
    let mount = Mount {
        mem: "specs".to_string(),
        schema: Some(pin("default")),
        storage: MountStorage::Folder {
            path: PathBuf::from("/dev/null"),
        },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mut engine =
        Engine::from_mounts(vec![(mount, Box::new(SharedHeadBackend(shared.clone())))]).unwrap();
    assert!(
        engine
            .unbacked_mount_warnings()
            .iter()
            .any(|w| w.code() == "MOUNT_UNBACKED"),
        "an entity-less mount reads empty: {:?}",
        engine.unbacked_mount_warnings()
    );
    assert!(engine.reload_if_stale(Some("specs")).is_empty());

    // The branch appears with one entity, written by someone else.
    shared.entities.lock().unwrap().push((
        PathBuf::from("first.md"),
        b"---\ntype: spec\n---\n# First\n\n## Identity\n\nfirst.\n\n## Purpose\n\nexists.\n"
            .to_vec(),
    ));
    shared.set_head(Some("aaa"));

    let warnings = engine.reload_if_stale(Some("specs"));
    match warnings.as_slice() {
        [
            crate::ops::WarningHint::MemReloaded {
                mem,
                old_head,
                new_head,
                entities_loaded,
            },
        ] => {
            assert_eq!(mem, "specs");
            assert!(old_head.is_empty(), "no head was cached: {old_head}");
            assert_eq!(new_head, "aaa");
            assert_eq!(*entities_loaded, 1);
        }
        other => panic!("expected one MemReloaded, got {other:?}"),
    }
    assert!(
        engine
            .store()
            .get(&EntityId::new("specs", "first"))
            .is_some(),
        "the appeared branch's entity is served"
    );
    assert!(
        !engine
            .unbacked_mount_warnings()
            .iter()
            .any(|w| w.code() == "MOUNT_UNBACKED"),
        "a mem holding an entity no longer reads empty: {:?}",
        engine.unbacked_mount_warnings()
    );
    // Cached now; the next probe is quiet.
    assert!(engine.reload_if_stale(Some("specs")).is_empty());
}

#[test]
fn reload_if_stale_emits_mem_reloaded_when_head_advances() {
    // Use an Arc<ManualHeadBackend> so the test retains a handle
    // for mutation after the engine has taken ownership of a
    // Box<dyn MemBackend> wrapper around it.
    struct ArcBackend(std::sync::Arc<ManualHeadBackend>);
    impl MemBackend for ArcBackend {
        fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
            self.0.list_entities()
        }
        fn read_entity(&self, rel: &Path) -> Result<Option<Vec<u8>>, BackendError> {
            self.0.read_entity(rel)
        }
        fn write_entity(&self, p: &Path, b: &[u8]) -> Result<(), BackendError> {
            self.0.write_entity(p, b)
        }
        fn delete_entity(&self, p: &Path) -> Result<(), BackendError> {
            self.0.delete_entity(p)
        }
        fn move_entity(&self, f: &Path, t: &Path) -> Result<(), BackendError> {
            self.0.move_entity(f, t)
        }
        fn commit(
            &self,
            m: &str,
            c: &CommitContext<'_>,
        ) -> Result<crate::storage::CommitId, BackendError> {
            self.0.commit(m, c)
        }
        fn stage_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
            self.0.stage_provenance(r)
        }
        fn append_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
            self.0.append_provenance(r)
        }
        fn read_provenance(&self, c: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
            self.0.read_provenance(c)
        }
        fn current_head(&self) -> Result<Option<String>, BackendError> {
            self.0.current_head()
        }
    }

    let shared = std::sync::Arc::new(ManualHeadBackend::new(Some("aaa")));
    let backend = Box::new(ArcBackend(shared.clone()));
    let mount = Mount {
        mem: "specs".to_string(),
        schema: Some(pin("default")),
        storage: MountStorage::Folder {
            path: PathBuf::from("/dev/null"),
        },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mut engine = Engine::from_mounts(vec![(mount, backend)]).unwrap();

    // No drift on first probe — cached==new.
    let warnings = engine.reload_if_stale(Some("specs"));
    assert!(warnings.is_empty());

    // Sibling writer advances the head.
    shared.set_head(Some("bbb"));

    let warnings = engine.reload_if_stale(Some("specs"));
    assert_eq!(warnings.len(), 1);
    match &warnings[0] {
        crate::ops::WarningHint::MemReloaded {
            mem,
            old_head,
            new_head,
            ..
        } => {
            assert_eq!(mem, "specs");
            assert_eq!(old_head, "aaa");
            assert_eq!(new_head, "bbb");
        }
        other => panic!("expected MemReloaded, got {other:?}"),
    }

    // Drift cleared — the engine's cached head now matches the
    // backend's current head; another probe is a no-op.
    let warnings = engine.reload_if_stale(Some("specs"));
    assert!(warnings.is_empty());
}

#[test]
fn mem_drifted_tracks_sibling_advance_until_reload() {
    // The read-only drift probe (built for the retired macOS app's roster): it reports
    // `true` once a sibling writer advances the backend past the
    // engine's cached head, *without* itself reloading, and clears
    // after the engine re-reads.
    struct ArcBackend(std::sync::Arc<ManualHeadBackend>);
    impl MemBackend for ArcBackend {
        fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
            self.0.list_entities()
        }
        fn read_entity(&self, rel: &Path) -> Result<Option<Vec<u8>>, BackendError> {
            self.0.read_entity(rel)
        }
        fn write_entity(&self, p: &Path, b: &[u8]) -> Result<(), BackendError> {
            self.0.write_entity(p, b)
        }
        fn delete_entity(&self, p: &Path) -> Result<(), BackendError> {
            self.0.delete_entity(p)
        }
        fn move_entity(&self, f: &Path, t: &Path) -> Result<(), BackendError> {
            self.0.move_entity(f, t)
        }
        fn commit(
            &self,
            m: &str,
            c: &CommitContext<'_>,
        ) -> Result<crate::storage::CommitId, BackendError> {
            self.0.commit(m, c)
        }
        fn stage_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
            self.0.stage_provenance(r)
        }
        fn append_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
            self.0.append_provenance(r)
        }
        fn read_provenance(&self, c: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
            self.0.read_provenance(c)
        }
        fn current_head(&self) -> Result<Option<String>, BackendError> {
            self.0.current_head()
        }
    }

    let shared = std::sync::Arc::new(ManualHeadBackend::new(Some("aaa")));
    let backend = Box::new(ArcBackend(shared.clone()));
    let mount = Mount {
        mem: "specs".to_string(),
        schema: Some(pin("default")),
        storage: MountStorage::Folder {
            path: PathBuf::from("/dev/null"),
        },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mut engine = Engine::from_mounts(vec![(mount, backend)]).unwrap();

    // Fresh boot: cached == live, no drift.
    assert!(!engine.mem_drifted("specs").unwrap());

    // Sibling writer advances the head — drift is visible WITHOUT a reload.
    shared.set_head(Some("bbb"));
    assert!(engine.mem_drifted("specs").unwrap());
    // Probing did not reload — still drifted on a second read.
    assert!(engine.mem_drifted("specs").unwrap());

    // Re-reading through the engine clears it.
    let _ = engine.reload_if_stale(Some("specs"));
    assert!(!engine.mem_drifted("specs").unwrap());

    // Unknown mem errors rather than reporting a bogus `false`.
    assert!(matches!(
        engine.mem_drifted("nope"),
        Err(EngineError::UnknownMem { .. })
    ));
}

#[test]
fn reload_one_mem_report_head_before_is_prior_cursor_and_advances() {
    // Regression for the reload→changes_since recipe. `head_before`
    // must report the engine's PRIOR cursor (the SHA it last knew),
    // not the post-drift on-disk tip — otherwise
    // `changes_since(since=head_before)` spans an empty range in
    // exactly the sibling-drift case the recipe targets. The reload
    // must also advance the cursor to the new tip so the next
    // staleness probe is a no-op rather than a spurious reload.
    struct ArcBackend(std::sync::Arc<ManualHeadBackend>);
    impl MemBackend for ArcBackend {
        fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
            self.0.list_entities()
        }
        fn read_entity(&self, rel: &Path) -> Result<Option<Vec<u8>>, BackendError> {
            self.0.read_entity(rel)
        }
        fn write_entity(&self, p: &Path, b: &[u8]) -> Result<(), BackendError> {
            self.0.write_entity(p, b)
        }
        fn delete_entity(&self, p: &Path) -> Result<(), BackendError> {
            self.0.delete_entity(p)
        }
        fn move_entity(&self, f: &Path, t: &Path) -> Result<(), BackendError> {
            self.0.move_entity(f, t)
        }
        fn commit(
            &self,
            m: &str,
            c: &CommitContext<'_>,
        ) -> Result<crate::storage::CommitId, BackendError> {
            self.0.commit(m, c)
        }
        fn stage_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
            self.0.stage_provenance(r)
        }
        fn append_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
            self.0.append_provenance(r)
        }
        fn read_provenance(&self, c: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
            self.0.read_provenance(c)
        }
        fn current_head(&self) -> Result<Option<String>, BackendError> {
            self.0.current_head()
        }
    }

    let shared = std::sync::Arc::new(ManualHeadBackend::new(Some("aaa")));
    let backend = Box::new(ArcBackend(shared.clone()));
    let mount = Mount {
        mem: "specs".to_string(),
        schema: Some(pin("default")),
        storage: MountStorage::Folder {
            path: PathBuf::from("/dev/null"),
        },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mut engine = Engine::from_mounts(vec![(mount, backend)]).unwrap();

    // Sibling writer advances the head past the engine's cursor.
    shared.set_head(Some("bbb"));

    let report = engine.reload_one_mem_report("specs").unwrap();
    // head_before is the prior cursor "aaa", not the drifted tip.
    assert_eq!(report.head_before, "aaa");
    assert_eq!(report.head_after, "bbb");

    // Cursor advanced to "bbb": a follow-up staleness probe is a
    // no-op, not a spurious MEM_RELOADED.
    let warnings = engine.reload_if_stale(Some("specs"));
    assert!(
        warnings.is_empty(),
        "cursor should have advanced to bbb, got {warnings:?}"
    );
}

#[test]
fn reload_if_stale_fires_every_call_no_throttle() {
    // Two back-to-back probes with the head advancing between
    // them: the second must reload and warn. There is no throttle
    // window — the ref check is the correctness floor.
    let shared = std::sync::Arc::new(ManualHeadBackend::new(Some("aaa")));
    struct ArcBackend(std::sync::Arc<ManualHeadBackend>);
    impl MemBackend for ArcBackend {
        fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
            self.0.list_entities()
        }
        fn read_entity(&self, rel: &Path) -> Result<Option<Vec<u8>>, BackendError> {
            self.0.read_entity(rel)
        }
        fn write_entity(&self, p: &Path, b: &[u8]) -> Result<(), BackendError> {
            self.0.write_entity(p, b)
        }
        fn delete_entity(&self, p: &Path) -> Result<(), BackendError> {
            self.0.delete_entity(p)
        }
        fn move_entity(&self, f: &Path, t: &Path) -> Result<(), BackendError> {
            self.0.move_entity(f, t)
        }
        fn commit(
            &self,
            m: &str,
            c: &CommitContext<'_>,
        ) -> Result<crate::storage::CommitId, BackendError> {
            self.0.commit(m, c)
        }
        fn stage_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
            self.0.stage_provenance(r)
        }
        fn append_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
            self.0.append_provenance(r)
        }
        fn read_provenance(&self, c: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
            self.0.read_provenance(c)
        }
        fn current_head(&self) -> Result<Option<String>, BackendError> {
            self.0.current_head()
        }
    }

    let backend = Box::new(ArcBackend(shared.clone()));
    let mount = Mount {
        mem: "specs".to_string(),
        schema: Some(pin("default")),
        storage: MountStorage::Folder {
            path: PathBuf::from("/dev/null"),
        },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mut engine = Engine::from_mounts(vec![(mount, backend)]).unwrap();

    // First probe observes cached==new; no warning.
    let warnings = engine.reload_if_stale(Some("specs"));
    assert!(warnings.is_empty());

    // Sibling advances head — the very next probe reloads and
    // warns, with no throttle window to mask it.
    shared.set_head(Some("bbb"));
    let warnings = engine.reload_if_stale(Some("specs"));
    assert_eq!(
        warnings.len(),
        1,
        "no throttle window — the moved ref reloads on the next probe"
    );
}

/// An id deleted and created again is alive again for the folder feed: a
/// later rename of it pairs, and arrives as one renamed event, where the
/// earlier delete kept the old id marked gone and the rename surfaced as
/// a removed id plus an updated one.
#[test]
fn folder_feed_pairs_a_rename_of_an_id_that_was_deleted_and_created_again() {
    use crate::engine::test_helpers::*;
    use crate::engine::{CreateEntityArgs, DeleteEntityArgs, RenameEntityArgs};
    use crate::storage::FilesystemBackend;
    let tmp = tempfile::TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(mem_dir.clone());
    let (actor, client) = cli_actor();
    let tick = || std::thread::sleep(std::time::Duration::from_millis(5));

    let first: CreateEntityArgs = empty_create_args("specs", "Alpha");
    let first = engine
        .create_entity(first, actor, Some(&client), None)
        .unwrap();
    tick();
    engine
        .delete_entity(
            DeleteEntityArgs {
                id: first.id.clone(),
                expected_hash: crate::ExpectedHash::Current(first.content_hash.clone()),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    tick();
    let again = engine
        .create_entity(
            empty_create_args("specs", "Alpha"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let between = engine
        .changes_since("specs", "", None)
        .unwrap()
        .head
        .clone();
    tick();
    let renamed = engine
        .rename_entity(
            RenameEntityArgs {
                id: again.id.clone(),
                expected_hash: crate::ExpectedHash::Current(again.content_hash.clone()),
                new_title: "Beta".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let later = engine.changes_since("specs", &between, None).unwrap();
    match later.changes.as_slice() {
        [crate::ops::ChangeEnvelope::Renamed { from_id, to_id, .. }] => {
            assert_eq!(from_id, &again.id);
            assert_eq!(to_id, &renamed.new_id);
        }
        other => panic!("expected one renamed event, got {other:?}"),
    }
}

/// Taking a moved head (after a pull or a reset) moves the cursor only
/// with a reload that went through. A reload that cannot read the mem
/// leaves the cursor where it was and marks the view behind, so the next
/// operation reloads again and a write refuses meanwhile; the cursor used
/// to move first, which made the stale view read as current.
#[test]
fn a_moved_head_whose_reload_fails_leaves_the_view_marked_behind() {
    struct Flaky {
        inner: std::sync::Arc<ManualHeadBackend>,
        unreadable: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    impl MemBackend for Flaky {
        fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
            self.inner.list_entities()
        }
        fn read_entity(&self, rel: &Path) -> Result<Option<Vec<u8>>, BackendError> {
            self.inner.read_entity(rel)
        }
        fn read_all_entities(&self) -> Result<Vec<crate::backend::EntityRead>, BackendError> {
            if self.unreadable.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(vec![(
                    PathBuf::from("lost.md"),
                    Err(BackendError::Other("blob unreadable".to_string())),
                )]);
            }
            Ok(Vec::new())
        }
        fn write_entity(&self, p: &Path, b: &[u8]) -> Result<(), BackendError> {
            self.inner.write_entity(p, b)
        }
        fn delete_entity(&self, p: &Path) -> Result<(), BackendError> {
            self.inner.delete_entity(p)
        }
        fn move_entity(&self, f: &Path, t: &Path) -> Result<(), BackendError> {
            self.inner.move_entity(f, t)
        }
        fn commit(
            &self,
            m: &str,
            c: &CommitContext<'_>,
        ) -> Result<crate::storage::CommitId, BackendError> {
            self.inner.commit(m, c)
        }
        fn stage_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
            self.inner.stage_provenance(r)
        }
        fn append_provenance(&self, r: &Provenance) -> Result<(), BackendError> {
            self.inner.append_provenance(r)
        }
        fn read_provenance(&self, c: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
            self.inner.read_provenance(c)
        }
        fn current_head(&self) -> Result<Option<String>, BackendError> {
            self.inner.current_head()
        }
    }
    let inner = std::sync::Arc::new(ManualHeadBackend::new(Some("aaa")));
    let unreadable = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mount = Mount {
        mem: "specs".to_string(),
        schema: Some(pin("default")),
        storage: MountStorage::Folder {
            path: PathBuf::from("/dev/null"),
        },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let backend = Box::new(Flaky {
        inner: inner.clone(),
        unreadable: unreadable.clone(),
    });
    let mut engine = Engine::from_mounts(vec![(mount, backend)]).unwrap();
    let _ = engine.reload_if_stale(Some("specs"));
    let before = engine.mounts[0].last_known_head.clone();

    inner.set_head(Some("bbb"));
    unreadable.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(engine.adopt_moved_head(0, "specs", "bbb").is_err());
    assert_eq!(
        engine.mounts[0].last_known_head, before,
        "the cursor did not move"
    );
    assert!(
        engine.mounts[0].reload_failure.is_some(),
        "the view is marked behind"
    );

    unreadable.store(false, std::sync::atomic::Ordering::SeqCst);
    engine.adopt_moved_head(0, "specs", "bbb").unwrap();
    assert_eq!(engine.mounts[0].last_known_head.as_deref(), Some("bbb"));
    assert!(engine.mounts[0].reload_failure.is_none());
}
