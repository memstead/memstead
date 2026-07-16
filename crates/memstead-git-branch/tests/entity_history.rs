//! Per-entity history on a real git-branch workspace — the plan's goal
//! fixture: an entity created, batch-updated, renamed, and updated
//! again, by two different writers. The query must return all four
//! touches newest-first under the current id, with pre-rename touches
//! attributed to their then-current id, batch context visible, and
//! pages composing exactly.

use memstead_base::vcs::{Actor, ClientId};
use memstead_base::{CreateEntityArgs, EntityId, RenameEntityArgs, UpdateEntityArgs};
use memstead_git_branch::test_support::init_real_mem_repo;
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use tempfile::TempDir;

fn seed_sections() -> indexmap::IndexMap<String, String> {
    let mut sections = indexmap::IndexMap::new();
    sections.insert("identity".to_string(), "seed identity".to_string());
    sections.insert("purpose".to_string(), "seed purpose".to_string());
    sections
}

fn create(engine: &mut memstead_base::Engine, title: &str, note: &str) -> String {
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
                title: title.to_string(),
                entity_type: "spec".to_string(),
                sections: seed_sections(),
                metadata: Default::default(),
                relations: Vec::new(),
                dry_run: false,
            },
            Actor::Cli,
            None,
            Some(note),
        )
        .unwrap()
        .id
        .0
}

fn update_args(id: &str, body: &str) -> UpdateEntityArgs {
    UpdateEntityArgs {
        id: EntityId(id.to_string()),
        expected_hash: None,
        sections: [("identity".to_string(), body.to_string())]
            .into_iter()
            .collect(),
        append_sections: Default::default(),
        patch_sections: Default::default(),
        metadata: Default::default(),
        metadata_unset: Vec::new(),
        dry_run: false,
        declare_relations: Vec::new(),
        anchors: Vec::new(),
        relations_unset: Vec::new(),
    }
}

#[test]
fn goal_fixture_full_story_under_the_current_id() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");

    // Writer 1 (CLI): birth of the protagonist and a bystander.
    let story = create(&mut engine, "Story", "born");
    let bystander = create(&mut engine, "Bystander", "also born");

    // Writer 1: one batch commit touching both.
    let batch = engine
        .batch_update(
            vec![
                (
                    update_args(&story, "batch touch"),
                    Some("batch pass".to_string()),
                ),
                (update_args(&bystander, "batch touch"), None),
            ],
            Actor::Agent,
            Some(&ClientId {
                name: "claude-code".to_string(),
                version: "1.0.0".to_string(),
            }),
        )
        .unwrap();
    assert!(batch.applied, "{batch:?}");

    // Rename the protagonist.
    let renamed = engine
        .rename_entity(
            RenameEntityArgs {
                id: EntityId(story.clone()),
                expected_hash: None,
                new_title: "Story Renamed".to_string(),
            },
            Actor::Cli,
            None,
            Some("clearer title"),
        )
        .unwrap()
        .new_id
        .0;
    assert_ne!(renamed, story);

    // Writer 2 (App): the final touch under the new id.
    engine
        .update_entity(
            update_args(&renamed, "final touch"),
            Actor::App,
            Some(&ClientId {
                name: "memstead-app".to_string(),
                version: "0.1.0".to_string(),
            }),
            Some("final touch"),
        )
        .unwrap();

    // ---- The story, queried under the CURRENT id ----
    let report = engine
        .entity_history("specs", &renamed, None, None)
        .unwrap();
    assert_eq!(report.entity_id, renamed);
    assert_eq!(
        report.total_recorded, 4,
        "create + batch + rename + update: {report:#?}"
    );
    let verbs: Vec<Option<&str>> = report.touches.iter().map(|t| t.verb.as_deref()).collect();
    assert_eq!(
        verbs,
        vec![
            Some("update"),
            Some("rename"),
            Some("batch-update"),
            Some("create")
        ],
        "newest-first narrative: {report:#?}"
    );

    // Attribution: post-rename touches under the new id, pre-rename
    // under the old — the story starts at the first appearance.
    assert_eq!(report.touches[0].id_at_touch, renamed);
    assert_eq!(
        report.touches[1].renamed_from.as_deref(),
        Some(story.as_str())
    );
    assert_eq!(
        report.touches[1].renamed_to.as_deref(),
        Some(renamed.as_str())
    );
    assert_eq!(report.touches[2].id_at_touch, story);
    assert_eq!(report.touches[3].id_at_touch, story);
    assert!(matches!(
        report.story_start,
        memstead_base::StoryStart::Recorded
    ));
    assert!(report.limitations.is_empty(), "{:?}", report.limitations);

    // Two writers, distinguishable per touch.
    assert_eq!(report.touches[0].actor.as_deref(), Some("app"));
    assert_eq!(
        report.touches[0].client.as_deref(),
        Some("memstead-app@0.1.0")
    );
    assert_eq!(report.touches[3].actor.as_deref(), Some("cli"));

    // The stated intent rides each touch where written.
    assert_eq!(report.touches[0].note.as_deref(), Some("final touch"));
    assert_eq!(report.touches[3].note.as_deref(), Some("born"));

    // Batch context is visible without polluting: the batch touch
    // names both ids…
    assert!(
        report.touches[2]
            .batch_entity_ids
            .iter()
            .any(|id| id == &bystander),
        "{report:#?}"
    );

    // …but the bystander's own story carries only ITS touches: its
    // create and the shared batch commit — none of the protagonist's.
    let bystander_report = engine
        .entity_history("specs", &bystander, None, None)
        .unwrap();
    assert_eq!(bystander_report.total_recorded, 2, "{bystander_report:#?}");
    assert!(
        bystander_report
            .touches
            .iter()
            .all(|t| t.id_at_touch == bystander)
    );

    // ---- Pages compose without gaps or duplicates ----
    let mut collected = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = engine
            .entity_history("specs", &renamed, Some(2), cursor.as_deref())
            .unwrap();
        assert_eq!(page.total_recorded, 4, "every page states the whole");
        collected.extend(page.touches.clone());
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(collected, report.touches);

    // ---- Refusals ----
    let err = engine
        .entity_history("ghost", &renamed, None, None)
        .unwrap_err();
    assert_eq!(err.code(), "UNKNOWN_MEM");
    let err = engine
        .entity_history("specs", "specs--never-existed", None, None)
        .unwrap_err();
    assert_eq!(err.code(), "ENTITY_NOT_FOUND");
    let err = engine
        .entity_history("specs", &renamed, None, Some("0000dead@0"))
        .unwrap_err();
    assert_eq!(err.code(), "INVALID_CURSOR");

    // The old id no longer names a live entity — its story refuses
    // rather than serving a half-history under a dead id.
    let err = engine
        .entity_history("specs", &story, None, None)
        .unwrap_err();
    assert_eq!(err.code(), "ENTITY_NOT_FOUND");
}
