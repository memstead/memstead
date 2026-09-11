//! Section modes: unset, ordered patches, append, the open-fence gate
//! on every regenerating verb, the stub guard, conflicting modes and
//! overlapping metadata keys.

use super::*;

/// Convenience: the update-args fixture for the sections_unset tests
/// (all-empty apart from the caller-set fields).
fn unset_args(id: EntityId, hash: String, unset: &[&str]) -> UpdateEntityArgs {
    UpdateEntityArgs {
        anchors: Vec::new(),
        id,
        expected_hash: Some(hash),
        sections: IndexMap::new(),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: unset.iter().map(|s| s.to_string()).collect(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    }
}

/// `sections_unset` removes a non-required section outright — heading
/// and body — and reports it under `modified_sections.unset`. An
/// absent key no-ops silently (symmetric with `metadata_unset`).
#[test]
fn update_entity_sections_unset_removes_optional_section() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Unset Subject");
    let (actor, client) = cli_actor();
    // Give the entity an optional section first.
    let mut sections = IndexMap::new();
    sections.insert("specifies".to_string(), "temporary content".to_string());
    let with_specifies = engine
        .update_entity(
            UpdateEntityArgs {
                sections,
                ..unset_args(seeded.id.clone(), seeded.content_hash.clone(), &[])
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let outcome = engine
        .update_entity(
            unset_args(
                seeded.id.clone(),
                with_specifies.content_hash.clone(),
                &["specifies", "not-present"],
            ),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(outcome.modified_sections.unset, vec!["specifies"]);
    let entity = engine.store().get(&seeded.id).unwrap();
    assert!(
        !entity.sections.contains_key("specifies"),
        "section removed: {:?}",
        entity.sections.keys().collect::<Vec<_>>()
    );
}

/// Removing a schema-REQUIRED section refuses with the conformance
/// vocabulary — the right repair for a required-but-empty heading is
/// filling it, never removing it (operator condition on this verb).
#[test]
fn update_entity_sections_unset_refuses_required_section() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Unset Required");
    let (actor, client) = cli_actor();
    let err = engine
        .update_entity(
            unset_args(
                seeded.id.clone(),
                seeded.content_hash.clone(),
                &["identity"],
            ),
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::MissingRequiredSection {
            entity_type,
            sections,
            ..
        } => {
            assert_eq!(entity_type, "spec");
            assert_eq!(sections.len(), 1);
            assert_eq!(sections[0].key, "identity");
        }
        other => panic!("expected MissingRequiredSection, got {other:?}"),
    }
}

/// The same key written and unset in one call is a contradiction —
/// refused as a section-mode conflict; and `relationships` is not
/// unsettable, like every other write mode.
#[test]
fn update_entity_sections_unset_conflicts_and_relationships_refuse() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Unset Conflict");
    let (actor, client) = cli_actor();
    let mut sections = IndexMap::new();
    sections.insert("specifies".to_string(), "body".to_string());
    let err = engine
        .update_entity(
            UpdateEntityArgs {
                sections,
                ..unset_args(
                    seeded.id.clone(),
                    seeded.content_hash.clone(),
                    &["specifies"],
                )
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::ConflictingSectionModes { section, modes } => {
            assert_eq!(section, "specifies");
            assert!(modes.contains(&"sections_unset".to_string()), "{modes:?}");
            assert!(modes.contains(&"sections".to_string()), "{modes:?}");
        }
        other => panic!("expected ConflictingSectionModes, got {other:?}"),
    }

    let err = engine
        .update_entity(
            unset_args(
                seeded.id.clone(),
                seeded.content_hash.clone(),
                &["relationships"],
            ),
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "SECTION_NOT_UPDATABLE", "{err:?}");
}

/// Several patches for ONE section apply in order against the
/// evolving body — the batched multi-edit the one-patch-per-section
/// map shape refused (backlog: `duplicate patch` per extra edit,
/// reconfirmed by two campaigns).
#[test]
fn update_entity_applies_multiple_patches_per_section_in_order() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Multi Patch");
    let (actor, client) = cli_actor();
    let mut patches = IndexMap::new();
    patches.insert(
        "identity".to_string(),
        vec![
            crate::ops::PatchArg {
                old: "fixture".to_string(),
                new: "FIRST".to_string(),
                all: false,
            },
            // The second patch matches text the FIRST patch produced —
            // provable in-order application against the evolving body.
            crate::ops::PatchArg {
                old: "FIRST identity".to_string(),
                new: "SECOND".to_string(),
                all: false,
            },
        ],
    );
    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                patch_sections: patches,
                ..unset_args(seeded.id.clone(), seeded.content_hash.clone(), &[])
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(outcome.modified_sections.patched, vec!["identity"]);
    let entity = engine.store().get(&seeded.id).unwrap();
    assert_eq!(entity.sections["identity"], "SECOND body");
}

/// A patch whose `old` lives in a DIFFERENT section gets that section
/// named in the refusal — the one-call recovery for a patch that
/// targeted the wrong section (backlog: a "found in `versioning`
/// instead" hint turns three attempts into one).
#[test]
fn update_entity_patch_names_the_sections_that_do_contain_old() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Patch Wrong Section");
    let (actor, client) = cli_actor();
    let mut patches = IndexMap::new();
    patches.insert(
        "identity".to_string(),
        vec![crate::ops::PatchArg {
            old: "fixture purpose body".to_string(),
            new: "nope".to_string(),
            all: false,
        }],
    );
    let err = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: seeded.id.clone(),
                expected_hash: Some(seeded.content_hash.clone()),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: patches,
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::PatchOldNotFound {
            section,
            found_in_sections,
            ..
        } => {
            assert_eq!(section, "identity");
            assert_eq!(found_in_sections, vec!["purpose".to_string()]);
        }
        other => panic!("expected PatchOldNotFound, got {other:?}"),
    }
}

#[test]
fn update_entity_appends_to_existing_section_with_newline_separator() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Append Subject");
    let (actor, client) = cli_actor();

    let mut appends = IndexMap::new();
    appends.insert("identity".to_string(), "appended tail.".to_string());

    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: seeded.id.clone(),
                expected_hash: Some(seeded.content_hash.clone()),
                sections: IndexMap::new(),
                append_sections: appends,
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // modified_sections.appended carries the append key;
    // modified_sections.replaced stays empty.
    assert_eq!(outcome.modified_sections.appended, vec!["identity"]);
    assert!(outcome.modified_sections.replaced.is_empty());

    // The section body now contains the appended tail.
    let updated = engine.get_entity(&seeded.id).unwrap();
    let body = updated.sections.get("identity").expect("identity section");
    assert!(
        body.contains("appended tail."),
        "appended body missing: {body:?}"
    );
}

/// Criteria 5 and 6. The state is reproduced the way it actually arrives:
/// a hand-edited file on disk, reloaded. The engine cannot author it, so a
/// fixture that went through `update_entity` would prove nothing.
fn engine_with_open_fence(tmp: &TempDir) -> (Engine, crate::EntityId) {
    let (_engine, seeded) = engine_with_seed(tmp, "Fenced");
    let id = seeded.id.clone();
    let path = tmp.path().join(&seeded.file_path);
    let raw = std::fs::read_to_string(&path).expect("seeded file");
    // Open a fence inside `identity`. In CommonMark its range runs to end
    // of text, so `## Purpose` below is masked and absorbed into it.
    let doctored = raw.replace("fixture identity body", "intro\n\n```rust\nfn main() {}");
    assert_ne!(doctored, raw, "the seeded body must be there to doctor");
    std::fs::write(&path, doctored).unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    drop(seeded);
    (engine, id)
}

#[test]
fn a_write_that_does_not_resolve_an_open_fence_is_refused() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, id) = engine_with_open_fence(&tmp);
    let (actor, client) = cli_actor();
    // The absorption is real before the write is attempted, and this is
    // the exact absorption shape: `purpose` is present and EMPTY
    // while its content sits verbatim inside `identity`. A surface that
    // reports "empty section" here is telling the truth about the parse
    // and a lie about the entity.
    let stored = engine.get_entity(&id).expect("entity loads");
    // Since absent-vs-empty became representable (sections_unset), a
    // heading masked inside the fence parses as ABSENT — the honest
    // reading: the document carries no visible `## Purpose` section.
    assert!(
        stored
            .sections
            .get("purpose")
            .is_none_or(|v| v.trim().is_empty()),
        "purpose should read as absent or empty: {:?}",
        stored.sections.get("purpose")
    );
    assert!(
        stored.sections["identity"].contains("## Purpose"),
        "its content is inside identity: {:?}",
        stored.sections.get("identity")
    );
    let hash = stored.content_hash.clone();

    let err = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: id.clone(),
                expected_hash: Some(hash),
                sections: IndexMap::from_iter([(
                    "purpose".to_string(),
                    "a new purpose".to_string(),
                )]),
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
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::UnterminatedFenceInStoredBody {
            ref section,
            ref fence,
            ref swallowed,
            ..
        } => {
            assert_eq!(section, "identity");
            assert_eq!(fence, "```");
            // Every declared section after the open fence — the fence's
            // range reaches end of text. The seeded file carries only
            // the written sections (unwritten optional headings are no
            // longer scaffolded), so `Purpose` is the whole set here.
            assert_eq!(swallowed, &vec!["Purpose".to_string()]);
        }
        other => panic!("expected UnterminatedFenceInStoredBody, got {other:?}"),
    }
    assert_eq!(err.code(), "UNTERMINATED_FENCE_IN_STORED_BODY");
}

#[test]
fn replacing_the_absorbing_section_is_the_way_out() {
    // Criterion 6. The refusal above strands nothing: the caller lifts the
    // swallowed content back out in the same call, and editing the file
    // directly is forbidden by the workspace rule, so this route has to
    // exist through the engine.
    let tmp = TempDir::new().unwrap();
    let (mut engine, id) = engine_with_open_fence(&tmp);
    let (actor, client) = cli_actor();
    let hash = engine.get_entity(&id).unwrap().content_hash.clone();
    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: id.clone(),
                expected_hash: Some(hash),
                sections: IndexMap::from_iter([
                    (
                        "identity".to_string(),
                        "intro\n\n```rust\nfn main() {}\n```".to_string(),
                    ),
                    ("purpose".to_string(), "the recovered purpose".to_string()),
                ]),
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
            actor,
            Some(&client),
            None,
        )
        .expect("a corrected body for the absorbing section is admitted");
    assert!(
        outcome
            .modified_sections
            .replaced
            .contains(&"identity".to_string())
    );
    let fixed = engine.get_entity(&id).unwrap();
    assert_eq!(
        fixed.sections.get("purpose").map(String::as_str),
        Some("the recovered purpose"),
        "the swallowed section is a section again"
    );
    assert!(
        crate::markdown::closing_fence_if_unterminated(fixed.sections.get("identity").unwrap())
            .is_none()
    );
}

/// The bypass the first version of this fix left open. The gate lived in
/// `update_entity`, so every OTHER verb that regenerates the file walked
/// past it and froze the absorption anyway. `relate` is the cheapest
/// witness; `rename` reaches the same render. The gate now sits at the
/// render itself, so this holds for any verb that writes bytes.
#[test]
fn every_verb_that_regenerates_the_file_is_gated_not_only_update() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, id) = engine_with_open_fence(&tmp);
    let (actor, client) = cli_actor();
    let before =
        std::fs::read_to_string(tmp.path().join(&engine.get_entity(&id).unwrap().file_path))
            .unwrap();
    let hash = engine.get_entity(&id).unwrap().content_hash.clone();

    let err = engine
        .relate_entity(
            RelateEntityArgs {
                source: id.clone(),
                expected_hash: Some(hash),
                rel_type: "USES".to_string(),
                target: crate::EntityId::new("specs", "some-target"),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .expect_err("relate must not be able to freeze the absorption");
    assert_eq!(err.code(), "UNTERMINATED_FENCE_IN_STORED_BODY");

    let err = engine
        .rename_entity(
            crate::engine::RenameEntityArgs {
                id: id.clone(),
                new_title: "Renamed Fenced".to_string(),
                expected_hash: Some(engine.get_entity(&id).unwrap().content_hash.clone()),
            },
            actor,
            Some(&client),
            None,
        )
        .expect_err("rename must not be able to freeze the absorption either");
    assert_eq!(err.code(), "UNTERMINATED_FENCE_IN_STORED_BODY");

    // And nothing was written: a refused mutation leaves the file alone.
    let after =
        std::fs::read_to_string(tmp.path().join(&engine.get_entity(&id).unwrap().file_path))
            .unwrap();
    assert_eq!(before, after, "a refused write must not touch the file");
}

#[test]
fn an_entity_with_no_open_fence_updates_exactly_as_before() {
    // Criterion 7 at the write tier: the new gate is invisible to every
    // entity that does not carry the condition.
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Ordinary");
    let (actor, client) = cli_actor();
    engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: seeded.id.clone(),
                expected_hash: Some(seeded.content_hash.clone()),
                sections: IndexMap::from_iter([(
                    "purpose".to_string(),
                    "a new purpose".to_string(),
                )]),
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
            actor,
            Some(&client),
            None,
        )
        .expect("an ordinary update is untouched by the fence gate");
}

/// Item 02: `memstead_update` against a stub must surface a typed
/// `StubNotUpdatable` envelope rather than the pre-fix
/// `UnknownType { name: "" }` cascade. Mirrors the
/// `StubCannotRelate` guard that `memstead_relate` already runs;
/// before Item 02 the docstring list advertised the
/// `STUB_NOT_UPDATABLE` code but no engine path emitted it.
#[test]
fn update_entity_against_stub_surfaces_typed_stub_not_updatable() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Source");
    let (actor, client) = cli_actor();
    // Materialise a stub by relating from a real entity to an
    // absent target. The relate path upserts the stub.
    let stub_id = crate::EntityId::new("specs", "stub-update-target");
    engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: stub_id.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let err = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: stub_id.clone(),
                expected_hash: Some(String::new()),
                sections: IndexMap::from_iter([("identity".to_string(), "body".to_string())]),
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
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::StubNotUpdatable { id } => assert_eq!(id, stub_id.to_string()),
        other => panic!("expected StubNotUpdatable, got {other:?}"),
    }
}

#[test]
fn update_entity_rejects_conflicting_section_modes() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Conflict");
    let (actor, client) = cli_actor();

    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "replace".to_string());
    let mut appends = IndexMap::new();
    appends.insert("identity".to_string(), "append".to_string());

    let err = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: seeded.id.clone(),
                expected_hash: Some(seeded.content_hash.clone()),
                sections,
                append_sections: appends,
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();

    match err {
        EngineError::ConflictingSectionModes { section, modes } => {
            assert_eq!(section, "identity");
            assert_eq!(modes, vec!["sections", "append_sections"]);
        }
        other => panic!("expected ConflictingSectionModes, got {other:?}"),
    }
}

#[test]
fn update_entity_rejects_overlapping_metadata_and_metadata_unset_keys() {
    // Wire contract: setting and unsetting the same key is a hard
    // error. The check runs before the required-field check so the
    // resolution (pick one map) is unambiguous regardless of
    // whether the conflicting key is required.
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Overlap Subject");
    let (actor, client) = cli_actor();

    let mut metadata = IndexMap::new();
    // `tags` is a non-required field on the default `spec` schema —
    // so this conflict is purely about the overlap, not about
    // unsetting-a-required-field.
    metadata.insert("tags".to_string(), "foo".to_string());

    let err = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: seeded.id.clone(),
                expected_hash: Some(seeded.content_hash.clone()),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata,
                metadata_unset: vec!["tags".to_string()],
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::SetAndUnsetConflict { keys } => {
            assert_eq!(keys, vec!["tags".to_string()]);
        }
        other => panic!("expected SetAndUnsetConflict, got {other:?}"),
    }
}
