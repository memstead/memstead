#![cfg(test)]

use std::path::PathBuf;

use tempfile::TempDir;

use crate::backend::MemBackend;
use crate::engine::test_helpers::*;
use crate::engine::{Engine, EngineError, RenameEntityArgs};
use crate::ops::WarningHint;
use crate::storage::FilesystemBackend;

#[test]
fn rename_moves_entity_anchors_to_new_id() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    let mut args = empty_create_args("specs", "Old Anchored");
    args.anchors = vec![crate::anchor::AnchorInput {
        artifact: Some("src/lib.rs".into()),
        grain: Some("file".into()),
        class: Some("anchored".into()),
        hash: Some("h1".into()),
        hash_stability: Some("stable".into()),
        ..Default::default()
    }];
    let seeded = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();
    let old_id = seeded.id.clone();

    let outcome = engine
        .rename_entity(
            RenameEntityArgs {
                id: old_id.clone(),
                expected_hash: Some(seeded.content_hash.clone()),
                new_title: "New Anchored".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Zero rows under the old id; all anchors resolve under the new id.
    assert!(engine.entity_anchors(&old_id).is_empty());
    assert_eq!(engine.entity_anchors(&outcome.new_id).len(), 1);
    assert_eq!(
        engine.anchors_referencing_artifact("src/lib.rs"),
        vec![(
            outcome.new_id.clone(),
            engine.entity_anchors(&outcome.new_id)[0].clone()
        )]
    );
}

#[test]
fn rename_entity_renames_file_and_id_persists_across_restart() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();

    let (old_id, new_id, new_file) = {
        let writer = FilesystemBackend::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        let seeded = engine
            .create_entity(
                empty_create_args("specs", "Old Name"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let outcome = engine
            .rename_entity(
                RenameEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    new_title: "New Name".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(outcome.old_id.to_string(), "specs--old-name");
        assert_eq!(outcome.new_id.to_string(), "specs--new-name");
        assert_eq!(outcome.new_path, "new-name.md");
        // Old file gone, new file present.
        assert!(!mem_dir.join(&outcome.old_path).exists());
        assert!(mem_dir.join(&outcome.new_path).exists());
        (outcome.old_id, outcome.new_id, outcome.new_path)
    };

    // New engine reading the same mem sees only the new id.
    let writer2 = FilesystemBackend::new(mem_dir.clone());
    let engine2 = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer2) as Box<dyn MemBackend>,
    )])
    .unwrap();
    assert!(engine2.get_entity(&old_id).is_none());
    let new_entity = engine2.get_entity(&new_id).expect("new id must persist");
    assert_eq!(new_entity.title, "New Name");
    assert_eq!(new_entity.file_path, new_file);
}

#[test]
fn rename_entity_returns_typed_warning_on_slug_noop() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Same Slug");
    let (actor, client) = cli_actor();
    let outcome = engine
        .rename_entity(
            RenameEntityArgs {
                id: seeded.id.clone(),
                expected_hash: Some(seeded.content_hash.clone()),
                new_title: "Same  Slug".to_string(), // slugifies to same
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    // Wire-shape parity with full: slug-noop is Ok+warning, not
    // an error. Old/new IDs are equal; old/new paths are equal;
    // write_id is empty (no disk write); warnings carries the
    // typed TitleNormalizedToSlugNoop hint.
    assert_eq!(outcome.old_id, outcome.new_id);
    assert_eq!(outcome.old_path, outcome.new_path);
    assert!(outcome.write_id.is_empty());
    assert_eq!(outcome.warnings.len(), 1);
    assert!(matches!(
        outcome.warnings[0],
        WarningHint::TitleNormalizedToSlugNoop { .. }
    ));
}

#[test]
fn rename_entity_returns_write_id_on_real_rename() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Old Name");
    let (actor, client) = cli_actor();
    let outcome = engine
        .rename_entity(
            RenameEntityArgs {
                id: seeded.id.clone(),
                expected_hash: Some(seeded.content_hash.clone()),
                new_title: "Brand New Name".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    // Real rename: write_id non-empty (folder backend produces
    // a synthetic CommitId), warnings empty, IDs differ.
    assert_ne!(outcome.old_id, outcome.new_id);
    assert!(
        !outcome.write_id.is_empty(),
        "write_id must be populated on a real rename"
    );
    assert!(outcome.warnings.is_empty());
}

#[test]
fn rename_entity_rewrites_self_references_in_body_and_relationships() {
    use crate::entity::EntityId;
    use indexmap::IndexMap;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    // Create the entity with a body section that contains a
    // self-reference. The slug is `old-name`; the body literally
    // names `[[old-name]]`. After rename, both surfaces — the
    // file on disk and the in-memory entity — must point at the
    // new slug.
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert("identity".to_string(), "the seed identity".to_string());
    sections.insert(
        "purpose".to_string(),
        "see also [[old-name]] for prior context".to_string(),
    );
    // F11: the `[[old-name]]` self-reference body link does NOT
    // synthesise a self-edge (the alias pass drops vacuous self-edges
    // and emits `SELF_LINK_IGNORED`); `scan_wikilinks_without_relation`
    // also skips self-targets, so the unbacked self-link is admitted.
    // The body link still rewrites on rename — this test pins that the
    // body follows the slug while no self-relation is ever created.
    let seeded = engine
        .create_entity(
            crate::engine::CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
                title: "Old Name".to_string(),
                entity_type: "spec".to_string(),
                sections,
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(seeded.id.to_string(), "specs--old-name");
    let related = seeded.clone();

    let outcome = engine
        .rename_entity(
            RenameEntityArgs {
                id: seeded.id.clone(),
                expected_hash: Some(related.content_hash.clone()),
                new_title: "Brand New Name".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(outcome.new_id.to_string(), "specs--brand-new-name");

    // File on disk reflects the rewrite — old slug must not
    // appear anywhere in the new file's bytes.
    let new_bytes = std::fs::read_to_string(mem_dir.join(&outcome.new_path)).unwrap();
    assert!(
        new_bytes.contains("[[brand-new-name]]"),
        "expected new slug in body, got:\n{new_bytes}"
    );
    assert!(
        !new_bytes.contains("[[old-name]]"),
        "old slug must not survive in the rewritten file, got:\n{new_bytes}"
    );

    // In-memory entity: section body rewritten, relationships
    // list points at the new id.
    let in_mem = engine.get_entity(&outcome.new_id).unwrap();
    assert!(
        in_mem
            .sections
            .get("purpose")
            .map(|s| s.contains("[[brand-new-name]]"))
            .unwrap_or(false),
        "section body must be rewritten in-memory; got {:?}",
        in_mem.sections.get("purpose")
    );
    // F11: no self-relation is ever synthesised — neither to the old
    // id nor (after the body rewrite) to the new id. The body link
    // followed the rename, but it produces no self-edge.
    let new_self_target = EntityId::new("specs", "brand-new-name");
    assert!(
        in_mem
            .relationships
            .iter()
            .all(|r| r.target != seeded.id && r.target != new_self_target),
        "a self-referential body link must produce no self-relation (F11), got: {:?}",
        in_mem.relationships
    );
}

/// A
/// rename rewrites a referrer's body wiki-link to the new slug — a
/// foreign-key change, not a semantic edit — so the referrer's
/// `last_modified` staleness clock must NOT reset. Pre-written files
/// carry an old `last_modified` (2020-01-01) so the assertion is
/// distinctive: after a same-day rename the clock stays at the old
/// date (it would jump to today if the re-commit still stamped it),
/// while the body link is correctly rewritten.
#[test]
fn rename_preserves_referrer_last_modified_but_rewrites_link() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();

    std::fs::write(
            mem_dir.join("target.md"),
            "---\ntype: spec\ncreated_date: 2020-01-01\nlast_modified: 2020-01-01\nlevel: M0\n---\n# Target\n\n## Identity\n\nT\n\n## Purpose\n\nP\n",
        )
        .unwrap();
    std::fs::write(
            mem_dir.join("referrer.md"),
            "---\ntype: spec\ncreated_date: 2020-01-01\nlast_modified: 2020-01-01\nlevel: M0\n---\n# Referrer\n\n## Identity\n\nR\n\n## Purpose\n\nDepends on [[target]] for context.\n\n## Relationships\n\n- **REFERENCES**: [[target]]\n",
        )
        .unwrap();

    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let target_id = crate::entity::EntityId::new("specs", "target");
    let target_hash = engine
        .store()
        .get(&target_id)
        .expect("target loaded from disk")
        .content_hash
        .clone();

    engine
        .rename_entity(
            RenameEntityArgs {
                id: target_id,
                expected_hash: Some(target_hash),
                new_title: "Target Renamed".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .expect("rename succeeds");

    let referrer_md = std::fs::read_to_string(mem_dir.join("referrer.md")).unwrap();
    // Staleness clock preserved — NOT bumped to today.
    assert!(
        referrer_md.contains("last_modified: 2020-01-01"),
        "referrer's last_modified must be preserved across a rename-driven slug rewrite; got:\n{referrer_md}"
    );
    // Core rename job intact — the body wiki-link points at the new slug.
    assert!(
        referrer_md.contains("[[target-renamed]]"),
        "referrer's body wiki-link must be rewritten to the new slug; got:\n{referrer_md}"
    );
    assert!(
        !referrer_md.contains("[[target]]"),
        "old slug must not survive in the referrer body; got:\n{referrer_md}"
    );
}

/// A hand-authored folder-mem file may carry a prose `[[target]]` with NO
/// `## Relationships` row — body wiki-links are not edge sources, so the
/// store holds no incoming edge for it and the edge-driven referrer walk
/// cannot see it. The rename must rewrite it anyway: the referrer
/// collection scans section bodies for links resolving to the renamed id,
/// not only the store's incoming edges. (Engine-written entities are
/// covered either way — alias synthesis always emits the row on write —
/// so this bites the hand-commit folder-mem model specifically.)
#[test]
fn rename_rewrites_prose_only_referrer_without_relationships_row() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();

    std::fs::write(
            mem_dir.join("target.md"),
            "---\ntype: spec\ncreated_date: 2020-01-01\nlast_modified: 2020-01-01\nlevel: M0\n---\n# Target\n\n## Identity\n\nT\n\n## Purpose\n\nP\n",
        )
        .unwrap();
    // Prose link only — deliberately NO `## Relationships` section.
    std::fs::write(
            mem_dir.join("referrer.md"),
            "---\ntype: spec\ncreated_date: 2020-01-01\nlast_modified: 2020-01-01\nlevel: M0\n---\n# Referrer\n\n## Identity\n\nR\n\n## Purpose\n\nDepends on [[target]] for context.\n",
        )
        .unwrap();

    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let target_id = crate::entity::EntityId::new("specs", "target");
    let target_hash = engine
        .store()
        .get(&target_id)
        .expect("target loaded from disk")
        .content_hash
        .clone();

    engine
        .rename_entity(
            RenameEntityArgs {
                id: target_id,
                expected_hash: Some(target_hash),
                new_title: "Target Renamed".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .expect("rename succeeds");

    let referrer_md = std::fs::read_to_string(mem_dir.join("referrer.md")).unwrap();
    assert!(
        referrer_md.contains("[[target-renamed]]"),
        "the prose-only wiki-link must be rewritten to the new slug; got:\n{referrer_md}"
    );
    assert!(
        !referrer_md.contains("[[target]]"),
        "the old slug must not survive in the prose-only referrer; got:\n{referrer_md}"
    );
}

#[test]
fn rename_entity_rewrites_same_mem_referrers_atomically() {
    use crate::engine::CreateEntityArgs;
    use crate::entity::EntityId;
    use indexmap::IndexMap;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    // Target — the entity that will be renamed.
    let target = engine
        .create_entity(
            empty_create_args("specs", "Target Spec"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(target.id.to_string(), "specs--target-spec");

    // Referrer A — explicit relation declared atomically with the
    // body wiki-link. Both surfaces must be rewritten.
    let mut sections_a: IndexMap<String, String> = IndexMap::new();
    sections_a.insert(
        "identity".to_string(),
        "referrer alpha identity".to_string(),
    );
    sections_a.insert(
        "purpose".to_string(),
        "rationale relies on [[target-spec]] for context".to_string(),
    );
    let referrer_a = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
                title: "Referrer Alpha".to_string(),
                entity_type: "spec".to_string(),
                sections: sections_a,
                metadata: IndexMap::new(),
                // REFERENCES is engine-emitted from the body wiki-link
                // via the alias-synthesis pass; explicit author is
                // refused under `manual_authoring: forbidden`.
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Referrer B — second referrer with body wiki-link + atomic
    // backing relation. Confirms multi-referrer body rewrites.
    let mut sections_b: IndexMap<String, String> = IndexMap::new();
    sections_b.insert("identity".to_string(), "referrer beta identity".to_string());
    sections_b.insert(
        "purpose".to_string(),
        "consult [[target-spec]] for the canonical phrasing".to_string(),
    );
    let referrer_b = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
                title: "Referrer Bravo".to_string(),
                entity_type: "spec".to_string(),
                sections: sections_b,
                metadata: IndexMap::new(),
                // REFERENCES is engine-emitted from the body wiki-link
                // via the alias-synthesis pass; explicit author is
                // refused under `manual_authoring: forbidden`.
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Bystander — no reference to the target. Must not be
    // touched on disk (its content_hash must be unchanged).
    let bystander = engine
        .create_entity(
            empty_create_args("specs", "Bystander"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let bystander_bytes_before =
        std::fs::read_to_string(mem_dir.join(&bystander.file_path)).unwrap();

    let renamed = engine
        .rename_entity(
            RenameEntityArgs {
                id: target.id.clone(),
                expected_hash: Some(target.content_hash.clone()),
                new_title: "Renamed Spec".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(renamed.new_id.to_string(), "specs--renamed-spec");

    // Old slug must not survive in any mem file — grep-clean,
    // scoped to this single-mem workspace.
    for path in std::fs::read_dir(&mem_dir).unwrap().flatten() {
        let p = path.path();
        if p.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(
            !body.contains("[[target-spec]]"),
            "old slug must not survive in {}, got:\n{body}",
            p.display()
        );
    }

    // Referrer A's explicit relation now points at the new id.
    let in_mem_a = engine.get_entity(&referrer_a.id).unwrap();
    assert!(
        in_mem_a
            .relationships
            .iter()
            .any(|r| r.rel_type == "REFERENCES"
                && r.target == EntityId::new("specs", "renamed-spec")),
        "expected referrer A's relation to point at renamed-spec, got {:?}",
        in_mem_a.relationships
    );
    assert!(
        in_mem_a
            .sections
            .get("purpose")
            .map(|s| s.contains("[[renamed-spec]]"))
            .unwrap_or(false),
        "referrer A's body must be rewritten"
    );

    // Referrer B's body is rewritten; it had no explicit
    // relation, so the relationships list stays empty.
    let in_mem_b = engine.get_entity(&referrer_b.id).unwrap();
    assert!(
        in_mem_b
            .sections
            .get("purpose")
            .map(|s| s.contains("[[renamed-spec]]"))
            .unwrap_or(false),
        "referrer B's body must be rewritten"
    );

    // Bystander untouched — exact byte equality on disk.
    let bystander_bytes_after =
        std::fs::read_to_string(mem_dir.join(&bystander.file_path)).unwrap();
    assert_eq!(
        bystander_bytes_before, bystander_bytes_after,
        "bystander must not be rewritten"
    );
}

/// Two-mem test scaffolding: build an engine with `specs` and
/// `memos` Write mounts, and set `cross_mem_links` so each is
/// permitted to link into the other. Returns the engine, both
/// mem directories, and the actor/client tuple. The test then
/// seeds whatever entities it needs.
fn engine_with_two_mems_and_bidirectional_policy(specs_dir: PathBuf, memos_dir: PathBuf) -> Engine {
    use memstead_schema::workspace_config::CrossLinkValue;
    let writer_specs = FilesystemBackend::new(specs_dir.clone());
    let writer_memos = FilesystemBackend::new(memos_dir.clone());
    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("specs", specs_dir),
            Box::new(writer_specs) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("memos", memos_dir),
            Box::new(writer_memos) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "memos".to_string(),
        CrossLinkValue::List(vec!["specs".to_string()]),
    );
    settings.cross_mem_links.insert(
        "specs".to_string(),
        CrossLinkValue::List(vec!["memos".to_string()]),
    );
    engine.set_settings(settings);
    engine
}

#[test]
fn rename_entity_rewrites_cross_mem_write_referrer() {
    use crate::engine::CreateEntityArgs;
    use crate::entity::EntityId;
    use indexmap::IndexMap;

    let tmp_specs = TempDir::new().unwrap();
    let tmp_memos = TempDir::new().unwrap();
    let specs_dir = tmp_specs.path().to_path_buf();
    let memos_dir = tmp_memos.path().to_path_buf();
    let mut engine =
        engine_with_two_mems_and_bidirectional_policy(specs_dir.clone(), memos_dir.clone());
    let (actor, client) = cli_actor();

    // Renaming target lives in `specs`.
    let target = engine
        .create_entity(
            empty_create_args("specs", "Target Spec"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Cross-mem referrer in `memos` has a body wiki-link in the
    // `:` form atomically backed by an explicit cross-mem relation.
    // The legacy `--` form is a same-mem nested-prefix drift
    // (resolves to `memos--specs--target-spec`); under the alias
    // model it cannot be backed and the engine surfaces it as
    // `SuspiciousNestedPrefix`, so it stays out of fresh fixtures.
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert("claim".to_string(), "the claim".to_string());
    sections.insert(
        "context".to_string(),
        "discussion stems from [[specs:target-spec]]".to_string(),
    );
    let referrer = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "memos".to_string(),
                title: "Cross Note".to_string(),
                entity_type: "memo".to_string(),
                sections,
                metadata: IndexMap::new(),
                // REFERENCES is engine-emitted from the body wiki-link
                // via the alias-synthesis pass; explicit author is
                // refused under `manual_authoring: forbidden`.
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Perform the rename.
    let renamed = engine
        .rename_entity(
            RenameEntityArgs {
                id: target.id.clone(),
                expected_hash: Some(target.content_hash.clone()),
                new_title: "Renamed Spec".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(renamed.new_id.to_string(), "specs--renamed-spec");

    // Cross-mem referrer's on-disk file now carries the new
    // slug in the colon form and no `target-spec` remnants survive.
    let referrer_path = memos_dir.join(&referrer.file_path);
    let referrer_bytes = std::fs::read_to_string(&referrer_path).unwrap();
    assert!(
        referrer_bytes.contains("[[specs:renamed-spec]]"),
        "expected colon-form rewrite in referrer body, got:\n{referrer_bytes}"
    );
    assert!(
        !referrer_bytes.contains("target-spec"),
        "old slug must not survive in referrer file, got:\n{referrer_bytes}"
    );

    // In-memory referrer's relationship list points at the new id.
    let in_mem = engine.get_entity(&referrer.id).unwrap();
    assert!(
        in_mem
            .relationships
            .iter()
            .any(|r| r.rel_type == "REFERENCES"
                && r.target == EntityId::new("specs", "renamed-spec")),
        "expected cross-mem relation to point at renamed-spec, got {:?}",
        in_mem.relationships
    );
    assert!(
        in_mem.relationships.iter().all(|r| r.target != target.id),
        "no relationship may still target the old id, got: {:?}",
        in_mem.relationships
    );
}

/// Wraps a real `MemBackend` and forwards every method
/// verbatim, except `commit_with_expected_parent` returns
/// `BackendError::ParentMismatch` whenever the caller passes a
/// non-`None` `expected_parent`. Models the "sibling writer
/// advanced the head between snapshot and our commit" case
/// without needing a real git-branch repository.
struct DriftingBackend {
    inner: Box<dyn MemBackend>,
}
impl DriftingBackend {
    fn new(inner: Box<dyn MemBackend>) -> Self {
        Self { inner }
    }
}
impl crate::backend::MemBackend for DriftingBackend {
    fn list_entities(&self) -> Result<Vec<PathBuf>, crate::backend::BackendError> {
        self.inner.list_entities()
    }
    fn read_entity(
        &self,
        rel: &std::path::Path,
    ) -> Result<Option<Vec<u8>>, crate::backend::BackendError> {
        self.inner.read_entity(rel)
    }
    fn write_entity(
        &self,
        rel: &std::path::Path,
        b: &[u8],
    ) -> Result<(), crate::backend::BackendError> {
        self.inner.write_entity(rel, b)
    }
    fn delete_entity(&self, rel: &std::path::Path) -> Result<(), crate::backend::BackendError> {
        self.inner.delete_entity(rel)
    }
    fn move_entity(
        &self,
        f: &std::path::Path,
        t: &std::path::Path,
    ) -> Result<(), crate::backend::BackendError> {
        self.inner.move_entity(f, t)
    }
    fn commit(
        &self,
        m: &str,
        c: &crate::vcs::CommitContext<'_>,
    ) -> Result<crate::storage::CommitId, crate::backend::BackendError> {
        self.inner.commit(m, c)
    }
    fn commit_with_expected_parent(
        &self,
        m: &str,
        c: &crate::vcs::CommitContext<'_>,
        expected_parent: Option<&str>,
    ) -> Result<crate::storage::CommitId, crate::backend::BackendError> {
        if let Some(expected) = expected_parent {
            Err(crate::backend::BackendError::ParentMismatch {
                expected: expected.to_string(),
                actual: "drifted-by-sibling-writer".to_string(),
            })
        } else {
            self.inner.commit(m, c)
        }
    }
    fn append_provenance(&self, r: &crate::Provenance) -> Result<(), crate::backend::BackendError> {
        self.inner.append_provenance(r)
    }
    fn read_provenance(
        &self,
        c: Option<&str>,
    ) -> Result<Vec<crate::Provenance>, crate::backend::BackendError> {
        self.inner.read_provenance(c)
    }
    fn current_head(&self) -> Result<Option<String>, crate::backend::BackendError> {
        // Return a non-None head so the rename's snapshot is
        // populated and the parent-pin path is exercised.
        Ok(Some("snapshot-head-sha".to_string()))
    }
}

#[test]
fn rename_entity_surfaces_partial_failure_when_peer_mem_drifts() {
    use crate::engine::CreateEntityArgs;
    use indexmap::IndexMap;
    use memstead_schema::workspace_config::CrossLinkValue;

    let tmp_specs = TempDir::new().unwrap();
    let tmp_memos = TempDir::new().unwrap();
    let specs_dir = tmp_specs.path().to_path_buf();
    let memos_dir = tmp_memos.path().to_path_buf();

    // specs uses a plain filesystem backend; memos uses one
    // wrapped in DriftingBackend so its peer-mem commit during
    // rename fails with ParentMismatch (the parent-pin tripped
    // by a hypothetical sibling writer).
    let writer_specs = FilesystemBackend::new(specs_dir.clone());
    let writer_memos_inner: Box<dyn MemBackend> =
        Box::new(FilesystemBackend::new(memos_dir.clone()));
    let writer_memos = DriftingBackend::new(writer_memos_inner);

    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("specs", specs_dir.clone()),
            Box::new(writer_specs) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("memos", memos_dir.clone()),
            Box::new(writer_memos) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "memos".to_string(),
        CrossLinkValue::List(vec!["specs".to_string()]),
    );
    settings.cross_mem_links.insert(
        "specs".to_string(),
        CrossLinkValue::List(vec!["memos".to_string()]),
    );
    engine.set_settings(settings);

    let (actor, client) = cli_actor();
    let target = engine
        .create_entity(
            empty_create_args("specs", "Target Spec"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert("claim".to_string(), "the claim".to_string());
    sections.insert(
        "context".to_string(),
        "see [[specs:target-spec]]".to_string(),
    );
    let _referrer = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "memos".to_string(),
                title: "Cross Note".to_string(),
                entity_type: "memo".to_string(),
                sections,
                metadata: IndexMap::new(),
                // REFERENCES is engine-emitted from the body wiki-link
                // via the alias-synthesis pass; explicit author is
                // refused under `manual_authoring: forbidden`.
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let err = engine
        .rename_entity(
            RenameEntityArgs {
                id: target.id.clone(),
                expected_hash: Some(target.content_hash.clone()),
                new_title: "Renamed Spec".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::RenamePartialFailure {
            committed_mems,
            failed_mem,
            failure_cause,
        } => {
            // The renaming entity's own mem committed before
            // the peer-mem commit was attempted, so it must be
            // listed as already-committed.
            assert_eq!(committed_mems, vec!["specs".to_string()]);
            assert_eq!(failed_mem, "memos");
            assert_eq!(failure_cause, "drift");
        }
        other => panic!("expected RenamePartialFailure, got {other:?}"),
    }
    // The renaming entity's own mem has the new file (its
    // commit landed) — that's the whole point of the partial-
    // failure envelope: source mem is durable, peer is not.
    assert!(specs_dir.join("renamed-spec.md").exists());
    assert!(!specs_dir.join(&target.file_path).exists());
}

#[test]
fn rename_entity_tags_every_per_mem_commit_with_same_logical_operation_id() {
    use crate::backend::MemBackend;
    use crate::engine::CreateEntityArgs;
    use indexmap::IndexMap;

    let tmp_specs = TempDir::new().unwrap();
    let tmp_memos = TempDir::new().unwrap();
    let specs_dir = tmp_specs.path().to_path_buf();
    let memos_dir = tmp_memos.path().to_path_buf();
    let mut engine =
        engine_with_two_mems_and_bidirectional_policy(specs_dir.clone(), memos_dir.clone());
    let (actor, client) = cli_actor();

    let target = engine
        .create_entity(
            empty_create_args("specs", "Target Spec"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert("claim".to_string(), "the claim".to_string());
    sections.insert(
        "context".to_string(),
        "discussion stems from [[specs:target-spec]]".to_string(),
    );
    let _referrer = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "memos".to_string(),
                title: "Cross Note".to_string(),
                entity_type: "memo".to_string(),
                sections,
                metadata: IndexMap::new(),
                // REFERENCES is engine-emitted from the body wiki-link
                // via the alias-synthesis pass; explicit author is
                // refused under `manual_authoring: forbidden`.
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let _ = engine
        .rename_entity(
            RenameEntityArgs {
                id: target.id.clone(),
                expected_hash: Some(target.content_hash.clone()),
                new_title: "Renamed Spec".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Read provenance from each mem's backend and find the
    // rename entries. Both mems must record a Rename entry, and
    // both entries must share the same logical_operation_id.
    let specs_backend: Box<dyn MemBackend> = Box::new(FilesystemBackend::new(specs_dir.clone()));
    let memos_backend: Box<dyn MemBackend> = Box::new(FilesystemBackend::new(memos_dir.clone()));
    let specs_provenance = specs_backend.read_provenance(None).unwrap();
    let memos_provenance = memos_backend.read_provenance(None).unwrap();

    let specs_rename = specs_provenance
        .iter()
        .find(|p| matches!(p.kind, crate::provenance::ProvenanceKind::Rename))
        .expect("specs mem must have a rename provenance entry");
    let memos_rename = memos_provenance
        .iter()
        .find(|p| matches!(p.kind, crate::provenance::ProvenanceKind::Rename))
        .expect("memos mem must have a rename provenance entry");

    let specs_id = specs_rename
        .logical_operation_id
        .as_deref()
        .expect("specs rename entry must carry a logical_operation_id");
    let memos_id = memos_rename
        .logical_operation_id
        .as_deref()
        .expect("memos rename entry must carry a logical_operation_id");
    assert_eq!(
        specs_id, memos_id,
        "both per-mem rename commits must share the same logical_operation_id"
    );
    assert!(
        specs_id.starts_with("logop-"),
        "logical_operation_id must use the `logop-` prefix the engine mints; got {specs_id}"
    );
}

#[test]
fn rename_entity_refuses_when_cross_mem_referrer_blocked_by_policy() {
    use crate::engine::CreateEntityArgs;
    use indexmap::IndexMap;
    use memstead_schema::workspace_config::CrossLinkValue;

    let tmp_specs = TempDir::new().unwrap();
    let tmp_memos = TempDir::new().unwrap();
    let specs_dir = tmp_specs.path().to_path_buf();
    let memos_dir = tmp_memos.path().to_path_buf();

    // Start with full policy so the create + cross-mem relate
    // succeed during setup.
    let mut engine =
        engine_with_two_mems_and_bidirectional_policy(specs_dir.clone(), memos_dir.clone());
    let (actor, client) = cli_actor();

    let target = engine
        .create_entity(
            empty_create_args("specs", "Target Spec"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert("claim".to_string(), "the claim".to_string());
    sections.insert(
        "context".to_string(),
        "see [[specs:target-spec]]".to_string(),
    );
    let _referrer = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "memos".to_string(),
                title: "Cross Note".to_string(),
                entity_type: "memo".to_string(),
                sections,
                metadata: IndexMap::new(),
                // REFERENCES is engine-emitted from the body wiki-link
                // via the alias-synthesis pass; explicit author is
                // refused under `manual_authoring: forbidden`.
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Tighten policy: revoke `memos → specs`, which is the
    // direction of the existing referrer edge (`memos--cross-note
    // REFERENCES specs--target-spec`). The propagated rewrite
    // preserves that direction, so the rename gate must refuse
    // up-front with the now-blocked direction named.
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "specs".to_string(),
        CrossLinkValue::List(vec!["memos".to_string()]),
    );
    // No entry for `memos` → `cross_mem_link_allowed("memos", "specs")` = false.
    engine.set_settings(settings);

    let err = engine
        .rename_entity(
            RenameEntityArgs {
                id: target.id.clone(),
                expected_hash: Some(target.content_hash.clone()),
                new_title: "Renamed Spec".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::RenameBlockedByCrossMemPolicy {
            from_mem,
            blocked_referrers,
        } => {
            assert_eq!(from_mem, "specs");
            assert_eq!(blocked_referrers.len(), 1);
            assert_eq!(blocked_referrers[0].from_mem, "memos");
            assert_eq!(blocked_referrers[0].to_mem, "specs");
            assert_eq!(blocked_referrers[0].count, 1);
        }
        other => panic!("expected RenameBlockedByCrossMemPolicy, got {other:?}"),
    }
    // Nothing landed: target file is still at the old path, no
    // new file was created.
    assert!(specs_dir.join(&target.file_path).exists());
    assert!(!specs_dir.join("renamed-spec.md").exists());
}

/// Rename target has no Write-mem referrers but is referenced
/// from a ReadOnly archive. The rename rewrites the renaming
/// entity's own mem but cannot reach into the archive — the
/// archive's wiki-link still points at the old slug. To keep
/// `incoming(<new_id>)` aligned with what a fresh boot would
/// produce and surface the dangling reference, the OLD-id store
/// entry is demoted to a stub holding the surviving archive
/// incoming edges, with a `ResidualStubForReadOnlyReferrers`
/// warning on the outcome. Mirrors delete-path's same-shaped
/// demotion.
#[test]
fn rename_entity_demotes_to_stub_when_only_readonly_cross_mem_referrers_remain() {
    use crate::engine::test_helpers::{archive_mount, build_archive};
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    let tmp = TempDir::new().unwrap();
    let writable_dir = tmp.path().join("writable");
    std::fs::create_dir_all(&writable_dir).unwrap();
    let writer = FilesystemBackend::new(writable_dir.clone());

    // Archive entity declares an explicit cross-mem relation
    // into the writable mem; under the alias model every edge
    // originates from `## Relationships`.
    let archive_md = "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# Archived Source\n\n## Identity\n\nLinks to [[specs:target]].\n\n## Purpose\n\nFixture for rename residual-stub demotion.\n\n## Relationships\n\n- **REFERENCES**: [[specs:target]]\n";
    let archive_path = build_archive(
        tmp.path(),
        "archive",
        &[("archived-source.md", archive_md.as_bytes())],
    );

    let folder_mount = Mount {
        mem: "specs".to_string(),
        schema: Some(crate::engine::test_helpers::pin("default")),
        storage: MountStorage::Folder {
            path: writable_dir.clone(),
        },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let archive_reader = crate::storage::ArchiveBackend::new(archive_path.clone());
    let mut engine = Engine::from_mounts(vec![
        (folder_mount, Box::new(writer) as Box<dyn MemBackend>),
        (
            archive_mount("archive", archive_path.clone()),
            Box::new(archive_reader) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();

    let (actor, client) = cli_actor();
    let target = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Sanity check: the archive's wiki-link produces an incoming
    // edge on the target.
    let archived_source_id = crate::EntityId::new("archive", "archived-source");
    let incoming_pre: Vec<_> = engine
        .store()
        .incoming(&target.id)
        .iter()
        .map(|e| e.from.clone())
        .collect();
    assert!(
        incoming_pre.contains(&archived_source_id),
        "archive wiki-link must produce an incoming edge on target; got {incoming_pre:?}"
    );

    // Rename. The archive can't be rewritten; engine demotes the
    // OLD-id store entry to a stub and emits the warning.
    let outcome = engine
        .rename_entity(
            RenameEntityArgs {
                id: target.id.clone(),
                expected_hash: Some(target.content_hash.clone()),
                new_title: "Renamed Target".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(outcome.new_id.to_string(), "specs--renamed-target");

    // New file landed.
    assert!(writable_dir.join(&outcome.new_path).exists());
    // Old slug demoted to a stub at the original id.
    let demoted = engine
        .get_entity(&target.id)
        .expect("residual stub must remain at old id");
    assert!(demoted.stub, "demoted entity must be flagged as stub");
    assert!(demoted.entity_type.is_empty());
    // Archive's incoming edge still points at the old id (the
    // archive markdown wasn't rewritten).
    let incoming_old: Vec<_> = engine
        .store()
        .incoming(&target.id)
        .iter()
        .map(|e| e.from.clone())
        .collect();
    assert!(
        incoming_old.contains(&archived_source_id),
        "archive incoming edge must survive demotion at old id; got {incoming_old:?}"
    );
    // New id has no incoming edge from the archive (its wiki-link
    // points at the old slug, not the new one).
    let incoming_new: Vec<_> = engine
        .store()
        .incoming(&outcome.new_id)
        .iter()
        .map(|e| e.from.clone())
        .collect();
    assert!(
        !incoming_new.contains(&archived_source_id),
        "archive must not be wired to new id (markdown still references old slug); got {incoming_new:?}"
    );
    // Warning carries the surviving referrer.
    let referrers = outcome
        .warnings
        .iter()
        .find_map(|w| match w {
            WarningHint::ResidualStubForReadOnlyReferrers {
                id: warn_id,
                referrers,
            } => {
                assert_eq!(warn_id, &target.id);
                Some(referrers.clone())
            }
            _ => None,
        })
        .expect("ResidualStubForReadOnlyReferrers warning must surface");
    assert_eq!(referrers, vec![archived_source_id]);
}

/// A same-mem referrer whose body uses the full-id form
/// `[[<mem>--<slug>]]` to point at the renaming entity must have
/// that token retargeted to the new slug. Pre-fix the rewrite
/// pass only matched short-form `[[<slug>]]`; full-id tokens
/// survived and pointed at the dead id. The new code calls
/// `rewrite_cross_mem_slug` on the same-mem path alongside
/// the bare-slug helper, covering both legal slug-form variants.
#[test]
fn rename_entity_rewrites_full_id_form_body_link_on_same_mem_referrer() {
    use crate::engine::CreateEntityArgs;
    use indexmap::IndexMap;
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    // Target entity to rename.
    let target = engine
        .create_entity(
            empty_create_args("specs", "Target Spec"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(target.id.to_string(), "specs--target-spec");

    // Referrer body uses the full-id form `[[specs--target-spec]]`.
    // The body parser admits both short and full-id forms; the
    // alias-synthesis pass emits one REFERENCES edge regardless
    // of which form the author wrote.
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert("identity".to_string(), "referrer identity".to_string());
    sections.insert(
        "purpose".to_string(),
        "see also [[specs--target-spec]] for context".to_string(),
    );
    let referrer = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
                title: "Referrer Full".to_string(),
                entity_type: "spec".to_string(),
                sections,
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let renamed = engine
        .rename_entity(
            RenameEntityArgs {
                id: target.id.clone(),
                expected_hash: Some(target.content_hash.clone()),
                new_title: "Renamed Spec".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(renamed.new_id.to_string(), "specs--renamed-spec");

    // The full-id form was retargeted (slug part rewritten, full-id
    // form preserved). The old slug must not survive in the
    // referrer's on-disk file.
    let referrer_path = mem_dir.join(&referrer.file_path);
    let body = std::fs::read_to_string(&referrer_path).unwrap();
    assert!(
        body.contains("[[specs--renamed-spec]]"),
        "expected full-id form retargeted to new slug, got:\n{body}"
    );
    assert!(
        !body.contains("target-spec"),
        "old slug must not survive in any form, got:\n{body}"
    );
}

#[test]
fn rename_entity_rejects_collision_with_existing_id() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, first) = engine_with_seed(&tmp, "First");
    let (actor, client) = cli_actor();
    let _ = engine
        .create_entity(
            empty_create_args("specs", "Second"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    // Rename `first` to a title that slugifies to `second`.
    let err = engine
        .rename_entity(
            RenameEntityArgs {
                id: first.id.clone(),
                expected_hash: Some(first.content_hash.clone()),
                new_title: "Second".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert!(matches!(
        err,
        EngineError::AlreadyExists { ref id, ref existing_title, existing_is_stub: false }
            if id == "specs--second" && existing_title == "Second"
    ));
}

/// With `test → other` granted, an `IMPLEMENTS` edge from
/// `test--src` to `other--target` is created. Revoking `test →
/// other` (the actual edge direction) must block the rename
/// up-front — pre-fix the gate checked the inverse direction
/// (`other → test`), which was un-granted in both phases, so the
/// rename refused for the wrong reason.
#[test]
fn rename_propagation_gate_checks_actual_edge_direction() {
    use crate::engine::error::BlockedReferrer;
    use crate::engine::{CreateEntityArgs, RelateEntityArgs};
    use indexmap::IndexMap;
    use memstead_schema::workspace_config::CrossLinkValue;

    let tmp_test = TempDir::new().unwrap();
    let tmp_other = TempDir::new().unwrap();
    let test_dir = tmp_test.path().to_path_buf();
    let other_dir = tmp_other.path().to_path_buf();

    // Pretty-print scaffold: `test` and `other` are the
    // canonical mem names. Reuse the helper by ignoring the
    // returned dirs and overriding the policy explicitly.
    let writer_test = FilesystemBackend::new(test_dir.clone());
    let writer_other = FilesystemBackend::new(other_dir.clone());
    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("test", test_dir.clone()),
            Box::new(writer_test) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("other", other_dir.clone()),
            Box::new(writer_other) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    let (actor, client) = cli_actor();

    // Setup policy: only `test → other` granted. Create the edge
    // `test--src IMPLEMENTS other--target`.
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "test".to_string(),
        CrossLinkValue::List(vec!["other".to_string()]),
    );
    engine.set_settings(settings);

    let target = engine
        .create_entity(
            empty_create_args("other", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let mut src_sections: IndexMap<String, String> = IndexMap::new();
    src_sections.insert("identity".to_string(), "source identity".to_string());
    src_sections.insert("purpose".to_string(), "source purpose".to_string());
    let src = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "test".to_string(),
                title: "Src".to_string(),
                entity_type: "spec".to_string(),
                sections: src_sections,
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let src = engine
        .relate_entity(
            RelateEntityArgs {
                source: src.id.clone(),
                rel_type: "IMPLEMENTS".to_string(),
                target: target.id.clone(),
                expected_hash: Some(src.content_hash.clone()),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let _ = src; // sink — we won't use the post-relate hash again

    // Revoke `test → other`. The post-rename rewrite would
    // re-emit the `test → other` edge, which the gate must now
    // refuse — pre-fix the inverted check passed because nothing
    // ever gated the right direction.
    engine.set_settings(crate::workspace::WorkspaceSettings::default());

    let err = engine
        .rename_entity(
            RenameEntityArgs {
                id: target.id.clone(),
                expected_hash: Some(target.content_hash.clone()),
                new_title: "Renamed".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::RenameBlockedByCrossMemPolicy {
            from_mem,
            blocked_referrers,
        } => {
            assert_eq!(from_mem, "other");
            assert_eq!(
                blocked_referrers,
                vec![BlockedReferrer {
                    from_mem: "test".to_string(),
                    to_mem: "other".to_string(),
                    count: 1,
                }],
                "blocked_referrers must name the actual edge direction (test → other)",
            );
        }
        other => panic!("expected RenameBlockedByCrossMemPolicy, got {other:?}"),
    }
    // No write landed in either mem: target file path
    // unchanged, the new slug file absent.
    assert!(other_dir.join(&target.file_path).exists());
    assert!(!other_dir.join("renamed.md").exists());
}

/// Re-granting the actual edge
/// direction lets the same rename succeed. Verifies the gate
/// fires only on the *un-granted* direction.
#[test]
fn rename_propagation_succeeds_when_actual_edge_direction_granted() {
    use crate::engine::{CreateEntityArgs, RelateEntityArgs};
    use indexmap::IndexMap;
    use memstead_schema::workspace_config::CrossLinkValue;

    let tmp_test = TempDir::new().unwrap();
    let tmp_other = TempDir::new().unwrap();
    let test_dir = tmp_test.path().to_path_buf();
    let other_dir = tmp_other.path().to_path_buf();

    let writer_test = FilesystemBackend::new(test_dir.clone());
    let writer_other = FilesystemBackend::new(other_dir.clone());
    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("test", test_dir),
            Box::new(writer_test) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("other", other_dir),
            Box::new(writer_other) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    let (actor, client) = cli_actor();

    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "test".to_string(),
        CrossLinkValue::List(vec!["other".to_string()]),
    );
    engine.set_settings(settings);

    let target = engine
        .create_entity(
            empty_create_args("other", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let mut src_sections: IndexMap<String, String> = IndexMap::new();
    src_sections.insert("identity".to_string(), "source identity".to_string());
    src_sections.insert("purpose".to_string(), "source purpose".to_string());
    let src = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "test".to_string(),
                title: "Src".to_string(),
                entity_type: "spec".to_string(),
                sections: src_sections,
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let _ = engine
        .relate_entity(
            RelateEntityArgs {
                source: src.id.clone(),
                rel_type: "IMPLEMENTS".to_string(),
                target: target.id.clone(),
                expected_hash: Some(src.content_hash.clone()),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Policy unchanged from setup (`test → other` still granted)
    // — rename must succeed and rewrite the cross-mem referrer.
    let outcome = engine
        .rename_entity(
            RenameEntityArgs {
                id: target.id.clone(),
                expected_hash: Some(target.content_hash.clone()),
                new_title: "Renamed".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_ne!(outcome.old_id, outcome.new_id);
    let renamed = engine
        .get_entity(&outcome.new_id)
        .expect("renamed entity persists");
    assert_eq!(renamed.title, "Renamed");
    // Referrer rewritten — IMPLEMENTS edge now points at the new id.
    let updated_src = engine.get_entity(&src.id).expect("source persists");
    assert!(
        updated_src
            .relationships
            .iter()
            .any(|r| r.rel_type == "IMPLEMENTS" && r.target == outcome.new_id),
        "referrer's IMPLEMENTS edge must point at the new id after rewrite"
    );
}

/// A rename whose target has no
/// cross-mem referrers bypasses the gate entirely. Even with
/// a fully-empty cross-link policy, the rename succeeds.
#[test]
fn rename_with_no_cross_mem_referrers_succeeds_regardless_of_policy() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    // Default settings: no cross-mem links at all. The
    // single-mem rename's referrers (if any) are all same-mem
    // and bypass the gate by construction.
    let target = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let outcome = engine
        .rename_entity(
            RenameEntityArgs {
                id: target.id.clone(),
                expected_hash: Some(target.content_hash.clone()),
                new_title: "Renamed Target".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_ne!(outcome.old_id, outcome.new_id);
}
