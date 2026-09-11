//! Alias synthesis and its garbage collection: dropped body links,
//! orphaned stubs, surviving referrers, dedupe, coexistence with
//! explicit relations, and the custom pointer schema.

use super::*;

// ---- Engine::delete_entity --------------------------------------

// ---------------------------------------------------------------------
// Alias-synthesis pass. Body wiki-links auto-emit
// relations of the source schema's `alias_target_rel_type` pointer
// and are garbage-collected when the body wiki-link disappears.
// ---------------------------------------------------------------------

#[test]
fn synthesis_gc_drops_auto_emitted_reference_when_body_link_removed() {
    // Create with `[[target]]` in body → synthesis emits
    // REFERENCES. Update body to drop the wiki-link → GC drops
    // the auto-emitted REFERENCES.
    use crate::engine::{CreateEntityArgs, UpdateEntityArgs};
    use indexmap::IndexMap;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(mem_dir.clone());
    let (actor, client) = cli_actor();

    let target = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    // Create source with the body wiki-link already present —
    // synthesis fires inside create.
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert("identity".to_string(), "source identity".to_string());
    sections.insert(
        "purpose".to_string(),
        "see [[target]] for context".to_string(),
    );
    let source = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
                title: "Source".to_string(),
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
    assert!(
        engine
            .get_entity(&source.id)
            .unwrap()
            .relationships
            .iter()
            .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
        "create-time synthesis must emit REFERENCES → target",
    );

    // Update: drop the body wiki-link. GC should remove the
    // synthesised REFERENCES.
    let mut new_sections: IndexMap<String, String> = IndexMap::new();
    new_sections.insert("purpose".to_string(), "no link any more".to_string());
    engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                sections: new_sections,
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
        .expect("update must succeed; GC drops the now-orphan REFERENCES");
    let in_mem = engine.get_entity(&source.id).unwrap();
    assert!(
        !in_mem
            .relationships
            .iter()
            .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
        "GC must drop the auto-emitted REFERENCES after body link removal; got {:?}",
        in_mem.relationships,
    );
}

#[test]
fn update_gc_removes_orphan_stub_when_last_body_link_dropped() {
    // Create source with `[[ghost]]` body link → alias synthesis
    // auto-stubs `ghost` and emits REFERENCES → ghost. The update
    // drops the link, so the REFERENCES edge (the stub's only
    // referrer) disappears; the orphan-stub GC sweep removes the
    // stub and surfaces it in `orphan_stubs_removed`. A reload from
    // disk shows the same (decremented) stub count — proving the GC
    // was a real store mutation, not a session-local view fix.
    use crate::engine::{CreateEntityArgs, UpdateEntityArgs};
    use indexmap::IndexMap;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(mem_dir.clone());
    let (actor, client) = cli_actor();

    let ghost = crate::EntityId::new("specs", "ghost");
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert("identity".to_string(), "source identity".to_string());
    sections.insert(
        "purpose".to_string(),
        "see [[ghost]] for context".to_string(),
    );
    let source = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
                title: "Source".to_string(),
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
    assert!(
        engine.store().contains(&ghost) && engine.get_entity(&ghost).unwrap().stub,
        "body wiki-link to an absent target must auto-stub it",
    );
    assert_eq!(
        engine.health().stub_count,
        1,
        "one stub before the link drop"
    );

    let mut new_sections: IndexMap<String, String> = IndexMap::new();
    new_sections.insert("purpose".to_string(), "no link any more".to_string());
    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                sections: new_sections,
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
        .expect("update must succeed and GC the now-orphan stub");

    assert_eq!(
        outcome.orphan_stubs_removed,
        vec![ghost.clone()],
        "the update that dropped the last body link must report the GC'd stub",
    );
    assert!(
        !engine.store().contains(&ghost),
        "orphan stub must be gone from the in-memory store",
    );
    assert_eq!(
        engine.health().stub_count,
        0,
        "stub count decremented in-session"
    );

    // Reload from disk: the source's on-disk markdown no longer
    // carries the link, so the parser re-emits no stub. The
    // decremented count holds across the reload — the GC was real.
    engine.reload_each_writable_mem().unwrap();
    assert!(
        !engine.store().contains(&ghost),
        "stub stays gone after reload-from-disk",
    );
    assert_eq!(
        engine.health().stub_count,
        0,
        "reloaded-from-disk store carries the same stub count as the in-session post-update state",
    );
}

#[test]
fn update_gc_noop_when_section_edit_changes_no_body_link() {
    // An update that edits one section while leaving the `[[ghost]]`
    // link standing in another orphans nothing: `orphan_stubs_removed`
    // is present and empty (stable shape, no spurious GC), and the
    // stub survives because its referrer survives.
    use crate::engine::{CreateEntityArgs, UpdateEntityArgs};
    use indexmap::IndexMap;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(mem_dir.clone());
    let (actor, client) = cli_actor();

    let ghost = crate::EntityId::new("specs", "ghost");
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert("identity".to_string(), "original identity".to_string());
    sections.insert(
        "purpose".to_string(),
        "see [[ghost]] for context".to_string(),
    );
    let source = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
                title: "Source".to_string(),
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
    assert!(engine.store().contains(&ghost), "ghost stub materialised");

    // Replace `identity` only; the `[[ghost]]` link in `purpose`
    // stays, so no edge drops.
    let mut edit: IndexMap<String, String> = IndexMap::new();
    edit.insert("identity".to_string(), "edited identity".to_string());
    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                sections: edit,
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
        .expect("update must succeed");
    assert!(
        outcome.orphan_stubs_removed.is_empty(),
        "an edit that keeps every body wiki-link orphans nothing; got {:?}",
        outcome.orphan_stubs_removed,
    );
    assert!(
        engine.store().contains(&ghost),
        "the still-referenced stub survives the unrelated section edit",
    );
}

#[test]
fn update_gc_preserves_stub_with_surviving_referrer() {
    // Two sources both body-link `[[ghost]]`. Dropping the link from
    // one leaves `ghost` referenced by the other — set-membership
    // semantics keep the stub alive and `orphan_stubs_removed` empty.
    use crate::engine::{CreateEntityArgs, UpdateEntityArgs};
    use indexmap::IndexMap;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(mem_dir.clone());
    let (actor, client) = cli_actor();

    let ghost = crate::EntityId::new("specs", "ghost");
    let make_with_link = |title: &str| {
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("identity".to_string(), format!("{title} identity"));
        sections.insert("purpose".to_string(), "see [[ghost]]".to_string());
        CreateEntityArgs {
            anchors: Vec::new(),
            mem: "specs".to_string(),
            title: title.to_string(),
            entity_type: "spec".to_string(),
            sections,
            metadata: IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        }
    };
    let source_a = engine
        .create_entity(make_with_link("Source A"), actor, Some(&client), None)
        .unwrap();
    engine
        .create_entity(make_with_link("Source B"), actor, Some(&client), None)
        .unwrap();
    assert!(engine.store().contains(&ghost), "ghost stub materialised");

    // Drop the link from source A only.
    let mut drop_link: IndexMap<String, String> = IndexMap::new();
    drop_link.insert("purpose".to_string(), "no link here".to_string());
    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: source_a.id.clone(),
                expected_hash: Some(source_a.content_hash.clone()),
                sections: drop_link,
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
        .expect("update must succeed");
    assert!(
        outcome.orphan_stubs_removed.is_empty(),
        "the stub keeps a referrer (source B), so nothing is GC'd; got {:?}",
        outcome.orphan_stubs_removed,
    );
    assert!(
        engine.store().contains(&ghost),
        "stub survives via the surviving referrer",
    );
}

#[test]
fn synthesis_gc_preserves_non_pointer_explicit_relation_across_body_update() {
    // Explicit USES to target (USES is not the schema's
    // alias_target_rel_type pointer). A subsequent body-changing
    // update must NOT drop the USES edge — GC only touches
    // relations of the pointer rel-type. Under Option C, REFERENCES
    // can't be authored explicitly (`manual_authoring: forbidden`),
    // so the analogous "explicit REFERENCES preserved" scenario is
    // structurally impossible; USES exercises the same invariant
    // from the rel-type-discrimination side.
    use crate::engine::{RelateEntityArgs, UpdateEntityArgs};
    use indexmap::IndexMap;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(mem_dir.clone());
    let (actor, client) = cli_actor();

    let target = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let source = engine
        .create_entity(
            empty_create_args("specs", "Source"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Explicit relate — no body wiki-link.
    let relate = engine
        .relate_entity(
            RelateEntityArgs {
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

    // Update an unrelated section. The explicit USES must survive
    // — it's not the alias_target_rel_type, GC ignores it.
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert("purpose".to_string(), "unrelated edit".to_string());
    engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: source.id.clone(),
                expected_hash: Some(relate.content_hash.clone()),
                sections,
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
        .expect("update must succeed");
    let in_mem = engine.get_entity(&source.id).unwrap();
    assert!(
        in_mem
            .relationships
            .iter()
            .any(|r| r.rel_type == "USES" && r.target == target.id),
        "explicit USES must survive an unrelated body update; got {:?}",
        in_mem.relationships,
    );
}

#[test]
fn synthesis_dedupes_repeated_body_links_to_same_target() {
    // Two `[[target]]` wiki-links in one body — synthesis must
    // not double-add. Result: exactly one REFERENCES.
    use crate::engine::UpdateEntityArgs;
    use indexmap::IndexMap;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(mem_dir.clone());
    let (actor, client) = cli_actor();

    let target = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let source = engine
        .create_entity(
            empty_create_args("specs", "Source"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert(
        "purpose".to_string(),
        "see [[target]] and again [[target]]".to_string(),
    );
    engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                sections,
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
        .unwrap();
    let in_mem = engine.get_entity(&source.id).unwrap();
    let count = in_mem
        .relationships
        .iter()
        .filter(|r| r.rel_type == "REFERENCES" && r.target == target.id)
        .count();
    assert_eq!(
        count, 1,
        "dedupe must leave exactly one REFERENCES → target; got {:?}",
        in_mem.relationships,
    );
}

#[test]
fn synthesis_coexists_with_explicit_uses_to_same_target() {
    // Explicit `USES` to target AND body wiki-link to target →
    // entity carries both USES and REFERENCES edges; synthesis
    // dedupes on `(rel_type, target)` so USES never suppresses
    // REFERENCES.
    use crate::engine::{RelateEntityArgs, UpdateEntityArgs};
    use indexmap::IndexMap;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(mem_dir.clone());
    let (actor, client) = cli_actor();

    let target = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let source = engine
        .create_entity(
            empty_create_args("specs", "Source"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    // Explicit USES.
    let relate = engine
        .relate_entity(
            RelateEntityArgs {
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
    // Body wiki-link to the same target — synthesis emits REFERENCES.
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert(
        "purpose".to_string(),
        "we also reference [[target]]".to_string(),
    );
    engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: source.id.clone(),
                expected_hash: Some(relate.content_hash.clone()),
                sections,
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
        .unwrap();
    let in_mem = engine.get_entity(&source.id).unwrap();
    assert!(
        in_mem
            .relationships
            .iter()
            .any(|r| r.rel_type == "USES" && r.target == target.id),
        "USES must survive — synthesis dedupes on (rel_type, target)",
    );
    assert!(
        in_mem
            .relationships
            .iter()
            .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
        "REFERENCES must be synthesised even though USES already targets the same entity",
    );
}

// ---------------------------------------------------------------------
// Alias-synthesis integration tests against custom-schema engines.
// The default schema pins `alias_target_rel_type: REFERENCES`; these
// tests verify the engine doesn't hardcode that name by mounting
// schemas with a non-REFERENCES pointer (proves name-agnosticism)
// and schemas with no pointer at all (proves the strict
// `WIKILINK_WITHOUT_RELATION` refusal still fires for opt-out
// schemas). Both build the engine via `from_mounts_with_schemas_dir`
// — the production path for workspace-authored schemas.
// ---------------------------------------------------------------------

mod alias_synthesis_custom_schema {
    use std::path::Path;

    use indexmap::IndexMap;
    use memstead_schema::SchemaRef;
    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::engine::test_helpers::*;
    use crate::engine::{CreateEntityArgs, Engine, EngineError, UpdateEntityArgs};
    use crate::storage::FilesystemBackend;
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    const TYPE_BODY: &str = r#"description: t
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
hierarchy_relationship: _default
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;

    fn write_schema_files(root: &Path, name: &str, manifest: &str, types: &[(&str, &str)]) {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("types")).unwrap();
        std::fs::write(dir.join("schema.yaml"), manifest).unwrap();
        for (type_name, body) in types {
            std::fs::write(dir.join("types").join(format!("{type_name}.yaml")), body).unwrap();
        }
    }

    fn make_type_yaml(name: &str) -> String {
        format!("name: {name}\n{TYPE_BODY}")
    }

    fn folder_mount_with_pin(mem: &str, path: std::path::PathBuf, pin: SchemaRef) -> Mount {
        Mount {
            mem: mem.to_string(),
            schema: Some(pin),
            storage: MountStorage::Folder { path },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        }
    }

    fn engine_with_schema(
        manifest: &str,
        type_yaml_name: &str,
        schema_name: &str,
        schema_version: semver::Version,
    ) -> (Engine, TempDir) {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();
        write_schema_files(
            &schemas_dir,
            schema_name,
            manifest,
            &[(type_yaml_name, &make_type_yaml(type_yaml_name))],
        );
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let writer = FilesystemBackend::new(mem_dir.clone());
        let pin = SchemaRef::new(schema_name, schema_version);
        let mount = folder_mount_with_pin("v", mem_dir, pin);
        let mut engine = Engine::from_mounts_with_schemas_dir(
            vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
            Some(&schemas_dir),
        )
        .expect("engine with custom schema constructs");
        engine.set_workspace_root(tmp.path().to_path_buf());
        (engine, tmp)
    }

    #[test]
    fn non_references_alias_pointer_emits_named_rel_type_from_body_link() {
        // Schema names CITES as the alias pointer — the engine
        // must emit CITES (not REFERENCES) from a body wiki-link.
        // Proves no hard-coded "REFERENCES" string anywhere in
        // the synthesis path.
        let manifest = r#"name: aliased
version: 0.1.0
description: alias-synthesis fixture using a non-REFERENCES pointer
when_to_use: tests prove the engine does not hard-code REFERENCES
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: CITES
      description: Citation — auto-emitted from body wiki-links
      default_weight: 0.5
    - name: PART_OF
      description: Hierarchy
      default_weight: 3.0
      acyclic: true
    - name: _default
      description: Fallback
      default_weight: 1.0
alias_target_rel_type: CITES
community:
  resolution: 1.0
  seed: 42
"#;
        let (mut engine, _tmp) =
            engine_with_schema(manifest, "doc", "aliased", semver::Version::new(0, 1, 0));
        let (actor, client) = cli_actor();

        let target = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "v".to_string(),
                    title: "Target".to_string(),
                    entity_type: "doc".to_string(),
                    sections: IndexMap::from_iter([(
                        "body".to_string(),
                        "target body".to_string(),
                    )]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("body".to_string(), "see [[target]]".to_string());
        let source = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "v".to_string(),
                    title: "Source".to_string(),
                    entity_type: "doc".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("create must succeed; CITES is auto-emitted by synthesis");

        let in_mem = engine.get_entity(&source.id).unwrap();
        assert!(
            in_mem
                .relationships
                .iter()
                .any(|r| r.rel_type == "CITES" && r.target == target.id),
            "synthesis must emit CITES (the pointer rel-type), not REFERENCES; got {:?}",
            in_mem.relationships,
        );
        assert!(
            !in_mem
                .relationships
                .iter()
                .any(|r| r.rel_type == "REFERENCES"),
            "engine must not hard-code REFERENCES — pointer rel-type is CITES; got {:?}",
            in_mem.relationships,
        );
    }

    #[test]
    fn no_pointer_schema_refuses_unbacked_body_wiki_link() {
        // Schema declares no `alias_target_rel_type`. Body wiki-link
        // without a backing relation must refuse with
        // `WIKILINK_WITHOUT_RELATION` — the strict validator's
        // pre-Option-C semantics, preserved for opt-out schemas.
        let manifest = r#"name: no-alias
version: 0.1.0
description: schema without alias_target_rel_type pointer
when_to_use: tests prove strict validator still fires for opt-out schemas
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: USES
      description: Use
      default_weight: 1.0
    - name: PART_OF
      description: Hierarchy
      default_weight: 3.0
      acyclic: true
    - name: _default
      description: Fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        let (mut engine, _tmp) =
            engine_with_schema(manifest, "doc", "no-alias", semver::Version::new(0, 1, 0));
        let (actor, client) = cli_actor();

        let target = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "v".to_string(),
                    title: "Target".to_string(),
                    entity_type: "doc".to_string(),
                    sections: IndexMap::from_iter([(
                        "body".to_string(),
                        "target body".to_string(),
                    )]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let source = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "v".to_string(),
                    title: "Source".to_string(),
                    entity_type: "doc".to_string(),
                    sections: IndexMap::from_iter([(
                        "body".to_string(),
                        "source body".to_string(),
                    )]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Add a body wiki-link with no backing relation. The
        // synthesis pass is a no-op (no pointer), so the
        // validator surfaces `WIKILINK_WITHOUT_RELATION`.
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("body".to_string(), "see [[target]]".to_string());
        let err = engine
            .update_entity(
                UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    sections,
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
            EngineError::WikiLinkWithoutRelation { from_id, missing } => {
                assert_eq!(from_id, source.id.to_string());
                assert_eq!(missing.len(), 1);
                assert_eq!(missing[0].section_key, "body");
                assert_eq!(missing[0].target_id, target.id.to_string());
            }
            other => {
                panic!("no-pointer schema must refuse with WikiLinkWithoutRelation; got {other:?}")
            }
        }
    }

    /// Restoring the
    /// pre-alias-synthesis invariant that every body wiki-link
    /// target carries a grammar-valid `EntityId`. Natural-form
    /// `[[Knowledge Graph]]` no longer slips through into a
    /// malformed auto-stub; the engine refuses with the typed
    /// `InvalidWikiLinkTarget` envelope and the
    /// `title_to_slug`-derived suggestion the agent lifts
    /// directly into a retry. Covers F1 of the 2026-05-18 CLI probe.
    #[test]
    fn natural_form_body_wiki_link_refuses_with_typed_envelope() {
        let manifest = r#"name: aliased
version: 0.1.0
description: alias-synthesis fixture
when_to_use: tests prove strict wiki-link grammar at mutation entry
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: REFERENCES
      description: Reference — auto-emitted from body wiki-links
      default_weight: 0.5
    - name: PART_OF
      description: Hierarchy
      default_weight: 3.0
      acyclic: true
    - name: _default
      description: Fallback
      default_weight: 1.0
alias_target_rel_type: REFERENCES
community:
  resolution: 1.0
  seed: 42
"#;
        let (mut engine, _tmp) =
            engine_with_schema(manifest, "doc", "aliased", semver::Version::new(0, 1, 0));
        let (actor, client) = cli_actor();

        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("body".to_string(), "see [[Knowledge Graph]]".to_string());
        let err = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "v".to_string(),
                    title: "Source".to_string(),
                    entity_type: "doc".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::InvalidWikiLinkTarget {
                raw,
                suggested,
                section,
                link_source,
                ..
            } => {
                assert_eq!(raw, "Knowledge Graph");
                assert_eq!(suggested.as_deref(), Some("knowledge-graph"));
                assert_eq!(section, "body");
                assert_eq!(link_source, "body_link");
            }
            other => panic!(
                "natural-form body wiki-link must refuse with InvalidWikiLinkTarget; got {other:?}"
            ),
        }
    }

    /// Tier-2 body wiki-link with a non-conformant
    /// mem prefix refuses with the distinct `InvalidMemName`
    /// (wire code `INVALID_MEM_NAME`) — mems are fixed
    /// identifiers, not free-form text the agent can slugify, so
    /// the recovery path is different from `InvalidWikiLinkTarget`.
    #[test]
    fn tier_two_bad_mem_prefix_refuses_with_distinct_envelope() {
        let manifest = r#"name: aliased
version: 0.1.0
description: alias-synthesis fixture
when_to_use: tests prove strict mem-prefix grammar at mutation entry
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: REFERENCES
      description: Reference
      default_weight: 0.5
    - name: PART_OF
      description: Hierarchy
      default_weight: 3.0
      acyclic: true
    - name: _default
      description: Fallback
      default_weight: 1.0
alias_target_rel_type: REFERENCES
community:
  resolution: 1.0
  seed: 42
"#;
        let (mut engine, _tmp) =
            engine_with_schema(manifest, "doc", "aliased", semver::Version::new(0, 1, 0));
        let (actor, client) = cli_actor();

        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("body".to_string(), "see [[Other Mem:foo]]".to_string());
        let err = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "v".to_string(),
                    title: "Source".to_string(),
                    entity_type: "doc".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::InvalidWikiLinkMem { raw, section, .. } => {
                assert_eq!(raw, "Other Mem");
                assert_eq!(section, "body");
            }
            other => {
                panic!("Tier-2 bad mem prefix must refuse with InvalidWikiLinkMem; got {other:?}")
            }
        }
    }

    /// Body wiki-link
    /// containing the ambiguous `[[<segments>/<segments>--<slug>]]`
    /// form refuses with `InvalidWikiLinkTarget` carrying the
    /// colon-form (`<prefix>:<slug>`) as `suggested`. Pre-fix the
    /// dash form silently produced a same-mem phantom stub at
    /// slug `team/sub-mem--target`, losing the agent's intent.
    #[test]
    fn hierarchical_dash_form_body_link_refuses_with_colon_suggestion() {
        let manifest = r#"name: aliased
version: 0.1.0
description: alias-synthesis fixture
when_to_use: tests prove hierarchical dash-form refusal at mutation entry
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: REFERENCES
      description: Reference — auto-emitted from body wiki-links
      default_weight: 0.5
    - name: PART_OF
      description: Hierarchy
      default_weight: 3.0
      acyclic: true
    - name: _default
      description: Fallback
      default_weight: 1.0
alias_target_rel_type: REFERENCES
community:
  resolution: 1.0
  seed: 42
"#;
        let (mut engine, _tmp) =
            engine_with_schema(manifest, "doc", "aliased", semver::Version::new(0, 1, 0));
        let (actor, client) = cli_actor();

        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert(
            "body".to_string(),
            "see [[team/sub-mem--target]]".to_string(),
        );
        let err = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "v".to_string(),
                    title: "Source".to_string(),
                    entity_type: "doc".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::InvalidWikiLinkTarget {
                raw,
                suggested,
                section,
                link_source,
                ..
            } => {
                assert_eq!(raw, "team/sub-mem--target");
                assert_eq!(suggested.as_deref(), Some("team/sub-mem:target"));
                assert_eq!(section, "body");
                assert_eq!(link_source, "body_link");
            }
            other => panic!(
                "hierarchical dash-form body link must refuse with InvalidWikiLinkTarget; got {other:?}"
            ),
        }

        // The entity did not land — no phantom stub for the source,
        // no phantom stub for the would-be target.
        let listed = engine.store().all_entities().collect::<Vec<_>>();
        assert!(
            listed.is_empty(),
            "refused create must not leave any entity behind, got: {listed:?}"
        );
    }
}
