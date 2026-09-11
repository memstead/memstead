//! Schema gates on the write path: declared constraints, required
//! outgoing edges (plain and conditional), acyclic sets, labelling,
//! signals, and the no-self-loop refusal.

use super::*;

/// Fixture for the declared-constraints vertical: type `task`
/// declares `requires_when` (checked → checked_by) at the given
/// severity, plus a `required_outgoing` block at the given
/// severity — so one schema exercises form 1 and form 4 at either
/// tier.
fn engine_with_constraints_schema(
    tmp: &TempDir,
    requires_when_severity: &str,
    required_outgoing_severity: &str,
) -> Engine {
    let schemas_dir = tmp.path().join("schemas");
    let pkg = schemas_dir.join("constr");
    std::fs::create_dir_all(pkg.join("types")).unwrap();
    std::fs::write(
        pkg.join("schema.yaml"),
        r#"name: constr
version: 0.1.0
description: constraint fixture
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
        format!(
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
metadata_fields:
  - key: status
    description: workflow state
    field_type: string
    enum_values: [open, checked]
  - key: checked_by
    description: who checked
    field_type: string
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: [PART_OF]
updatable_fields:
  - title
  - body
  - status
  - checked_by
health_required_fields:
  - body
staleness_threshold_days: 90
required_outgoing:
  - relationships: [PART_OF]
    cardinality: at_least_one
    severity: {required_outgoing_severity}
constraints:
  - kind: requires_when
    field: checked_by
    when_field: status
    when_value: checked
    severity: {requires_when_severity}
write_rules: []
"#
        ),
    )
    .unwrap();
    let mem_dir = tmp.path().join("mem");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mount = crate::workspace::Mount {
        mem: "tasks".to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            "constr",
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

fn checked_task_args(title: &str, relations: Vec<crate::ops::RelateArg>) -> CreateEntityArgs {
    let mut args = task_create_args(title, relations);
    args.metadata
        .insert("status".to_string(), "checked".to_string());
    args
}

/// Form 1 at warn: a create violating `requires_when` warns
/// `CONSTRAINT_UNSATISFIED` and still commits; the health sweep
/// reports the same violation (shared evaluation); a create
/// satisfying the constraint emits neither.
#[test]
fn create_warns_requires_when_and_still_commits() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_constraints_schema(&tmp, "warn", "warn");
    let (actor, client) = cli_actor();

    let outcome = engine
        .create_entity(
            checked_task_args("Unbacked Judgment", vec![]),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert!(!outcome.write_id.is_empty(), "warn tier never blocks");
    let violation = outcome
        .warnings
        .iter()
        .find_map(|w| match w {
            WarningHint::ConstraintUnsatisfied { violations, .. } => Some(violations.clone()),
            _ => None,
        })
        .expect("CONSTRAINT_UNSATISFIED warning present");
    assert_eq!(violation.len(), 1);
    let crate::ops::health::UnsatisfiedConstraint::RequiresWhen {
        field,
        when_field,
        when_value,
        ..
    } = &violation[0]
    else {
        panic!("expected requires_when violation");
    };
    assert_eq!(field, "checked_by");
    assert_eq!(when_field, "status");
    assert_eq!(when_value, "checked");

    // Health parity — same single evaluation.
    let reports = crate::ops::health::collect_constraint_findings(
        engine.store(),
        None,
        engine.schemas(),
        None,
    );
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].id, outcome.id);
    assert_eq!(reports[0].violations.len(), 1);
    let crate::ops::health::UnsatisfiedConstraint::RequiresWhen { field, .. } =
        &reports[0].violations[0]
    else {
        panic!("expected requires_when finding");
    };
    assert_eq!(field, "checked_by");

    // Complement 1: satisfying the constraint in the same create
    // emits no warning and no finding.
    let mut satisfied_args = checked_task_args("Backed Judgment", vec![]);
    satisfied_args
        .metadata
        .insert("checked_by".to_string(), "reviewer-a".to_string());
    let satisfied = engine
        .create_entity(satisfied_args, actor, Some(&client), None)
        .unwrap();
    assert!(
        !satisfied
            .warnings
            .iter()
            .any(|w| matches!(w, WarningHint::ConstraintUnsatisfied { .. })),
        "satisfied constraint emits no warning: {:?}",
        satisfied.warnings
    );

    // Complement 2: an untriggered constraint (status != checked)
    // emits nothing even with checked_by unset.
    let untriggered = engine
        .create_entity(
            task_create_args("Open Task", vec![]),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert!(
        !untriggered
            .warnings
            .iter()
            .any(|w| matches!(w, WarningHint::ConstraintUnsatisfied { .. })),
        "untriggered constraint emits no warning"
    );
}

/// Form 1 at block: the same violation refuses the create with
/// `CONSTRAINT_UNSATISFIED`, leaves nothing behind, and the
/// refusal payload restates the declaration.
#[test]
fn create_refuses_block_tier_requires_when() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_constraints_schema(&tmp, "block", "warn");
    let (actor, client) = cli_actor();

    let err = engine
        .create_entity(
            checked_task_args("Unbacked Judgment", vec![]),
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED");
    let details = err.details();
    assert_eq!(details["violations"][0]["field"], "checked_by");
    assert_eq!(details["violations"][0]["severity"], "block");
    assert_eq!(
        engine.store().all_entities().count(),
        0,
        "refused create leaves nothing behind"
    );

    // The satisfying create passes under the same schema.
    let mut ok_args = checked_task_args("Backed Judgment", vec![]);
    ok_args
        .metadata
        .insert("checked_by".to_string(), "reviewer-a".to_string());
    engine
        .create_entity(ok_args, actor, Some(&client), None)
        .unwrap();
}

/// Form 4 at block: a create leaving a `severity: block`
/// `required_outgoing` block unsatisfied refuses with
/// `MISSING_REQUIRED_OUTGOING` (the same code the warn tier
/// warns with — one condition, one vocabulary); an inline
/// relation satisfying the block lets the create pass.
#[test]
fn create_refuses_block_tier_required_outgoing() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_constraints_schema(&tmp, "warn", "block");
    let (actor, client) = cli_actor();

    let err = engine
        .create_entity(
            task_create_args("Orphan Task", vec![]),
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "MISSING_REQUIRED_OUTGOING");
    let details = err.details();
    assert_eq!(details["missing"][0]["relationships"][0], "PART_OF");
    assert_eq!(details["missing"][0]["severity"], "block");
    assert_eq!(engine.store().all_entities().count(), 0);

    // A create satisfying the block via an inline relation to an
    // auto-stubbed target passes — the stub itself has no type
    // definition under this schema's `task`-only vocabulary, so
    // wire the edge from the real entity.
    let outcome = engine.create_entity(
        task_create_args(
            "Child Task",
            vec![crate::ops::RelateArg {
                target: crate::entity::EntityId("tasks--parent".to_string()),
                rel_type: "PART_OF".to_string(),
                description: None,
            }],
        ),
        actor,
        Some(&client),
        None,
    );
    assert!(
        outcome.is_ok(),
        "satisfied block-tier create passes: {:?}",
        outcome.err()
    );
}

/// Update-side severity mirror for form 1: at warn, an update
/// that makes the constraint trigger warns and commits; at block,
/// the same update refuses and the entity keeps its prior state.
#[test]
fn update_enforces_requires_when_by_severity() {
    let (actor, client) = cli_actor();
    let set_checked = |engine: &mut Engine, id: &crate::entity::EntityId| {
        let current = engine.get_entity(id).unwrap().content_hash.clone();
        let mut metadata = IndexMap::new();
        metadata.insert("status".to_string(), "checked".to_string());
        engine.update_entity(
            crate::engine::UpdateEntityArgs {
                anchors: Vec::new(),
                id: id.clone(),
                expected_hash: Some(current),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata,
                metadata_unset: Vec::new(),
                declare_relations: vec![],
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
    };

    // Warn tier: the update commits with the typed warning.
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_constraints_schema(&tmp, "warn", "warn");
    let a = engine
        .create_entity(
            task_create_args("Task A", vec![]),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let outcome = set_checked(&mut engine, &a.id).unwrap();
    assert!(!outcome.write_id.is_empty());
    assert!(
        outcome
            .warnings
            .iter()
            .any(|w| matches!(w, WarningHint::ConstraintUnsatisfied { .. })),
        "warn-tier update carries the warning: {:?}",
        outcome.warnings
    );

    // Block tier: the same update refuses; the entity keeps its
    // prior metadata.
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_constraints_schema(&tmp, "block", "warn");
    let b = engine
        .create_entity(
            task_create_args("Task B", vec![]),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let err = set_checked(&mut engine, &b.id).unwrap_err();
    assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED");
    assert!(
        !engine
            .get_entity(&b.id)
            .unwrap()
            .metadata
            .contains_key("status"),
        "refused update leaves the entity unchanged"
    );
}

/// Form 4 at block on the relate surface: removing the edge that
/// satisfies a `severity: block` `required_outgoing` block refuses
/// with `MISSING_REQUIRED_OUTGOING`; the edge survives.
#[test]
fn relate_remove_refuses_block_tier_required_outgoing() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_constraints_schema(&tmp, "warn", "block");
    let (actor, client) = cli_actor();
    let parent_id = crate::entity::EntityId("tasks--parent".to_string());
    let child = engine
        .create_entity(
            task_create_args(
                "Child Task",
                vec![crate::ops::RelateArg {
                    target: parent_id.clone(),
                    rel_type: "PART_OF".to_string(),
                    description: None,
                }],
            ),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let err = engine
        .relate_entity(
            crate::engine::RelateEntityArgs {
                source: child.id.clone(),
                target: parent_id.clone(),
                rel_type: "PART_OF".to_string(),
                description: None,
                remove: true,
                expected_hash: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "MISSING_REQUIRED_OUTGOING");
    assert!(
        engine
            .get_entity(&child.id)
            .unwrap()
            .relationships
            .iter()
            .any(|r| r.rel_type == "PART_OF" && r.target == parent_id),
        "refused remove leaves the edge in place"
    );
}

/// Fixture for the conditional `required_outgoing` form: type
/// `task` requires a PART_OF edge only while `status` holds
/// `checked`, at the given severity. No unconditional blocks, no
/// `constraints` — the conditional block is the only obligation.
fn engine_with_conditional_ro_schema(tmp: &TempDir, severity: &str) -> Engine {
    let schemas_dir = tmp.path().join("schemas");
    let pkg = schemas_dir.join("condro");
    std::fs::create_dir_all(pkg.join("types")).unwrap();
    std::fs::write(
        pkg.join("schema.yaml"),
        r#"name: condro
version: 0.1.0
description: conditional required_outgoing fixture
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
        format!(
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
metadata_fields:
  - key: status
    description: workflow state
    field_type: string
    enum_values: [open, checked]
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
  - status
health_required_fields:
  - body
staleness_threshold_days: 90
required_outgoing:
  - relationships: [PART_OF]
    cardinality: at_least_one
    severity: {severity}
    when_field: status
    when_value: checked
write_rules: []
"#
        ),
    )
    .unwrap();
    let mem_dir = tmp.path().join("mem");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mount = crate::workspace::Mount {
        mem: "tasks".to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            "condro",
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

/// An unarmed conditional block never fires: entities whose
/// trigger field is unset or holds another enum value create
/// cleanly without the edge, with no `MISSING_REQUIRED_OUTGOING`
/// warning, even at block tier.
#[test]
fn create_ignores_conditional_required_outgoing_when_unarmed() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_conditional_ro_schema(&tmp, "block");
    let (actor, client) = cli_actor();

    let unset = engine
        .create_entity(
            task_create_args("Unset Status", vec![]),
            actor,
            Some(&client),
            None,
        )
        .expect("unset trigger field must create");
    let mut open_args = task_create_args("Open Task", vec![]);
    open_args
        .metadata
        .insert("status".to_string(), "open".to_string());
    let open = engine
        .create_entity(open_args, actor, Some(&client), None)
        .expect("non-trigger value must create");
    for outcome in [&unset, &open] {
        assert!(
            !outcome
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::MissingRequiredOutgoing { .. })),
            "unarmed block emits no warning: {:?}",
            outcome.warnings
        );
    }
}

/// Armed at block tier: a create whose trigger field holds the
/// trigger value and lacks the edge refuses with
/// `MISSING_REQUIRED_OUTGOING`, and the payload names the trigger
/// (`when_field` / `when_value`); an inline relation satisfying
/// the armed block lets the same create pass.
#[test]
fn create_refuses_block_tier_conditional_required_outgoing() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_conditional_ro_schema(&tmp, "block");
    let (actor, client) = cli_actor();

    let err = engine
        .create_entity(
            checked_task_args("Checked Orphan", vec![]),
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "MISSING_REQUIRED_OUTGOING");
    let details = err.details();
    assert_eq!(details["missing"][0]["relationships"][0], "PART_OF");
    assert_eq!(details["missing"][0]["when_field"], "status");
    assert_eq!(details["missing"][0]["when_value"], "checked");
    assert_eq!(engine.store().all_entities().count(), 0);

    let outcome = engine.create_entity(
        checked_task_args(
            "Checked Child",
            vec![crate::ops::RelateArg {
                target: crate::entity::EntityId("tasks--parent".to_string()),
                rel_type: "PART_OF".to_string(),
                description: None,
            }],
        ),
        actor,
        Some(&client),
        None,
    );
    assert!(
        outcome.is_ok(),
        "satisfied armed block passes: {:?}",
        outcome.err()
    );
}

/// Armed at warn tier: the create lands and carries the
/// `MISSING_REQUIRED_OUTGOING` warning whose block entry names
/// the trigger.
#[test]
fn create_warns_conditional_required_outgoing_at_warn_tier() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_conditional_ro_schema(&tmp, "warn");
    let (actor, client) = cli_actor();

    let outcome = engine
        .create_entity(
            checked_task_args("Checked Orphan", vec![]),
            actor,
            Some(&client),
            None,
        )
        .expect("warn tier lands the write");
    assert!(!outcome.write_id.is_empty());
    let block = outcome
        .warnings
        .iter()
        .find_map(|w| match w {
            WarningHint::MissingRequiredOutgoing { missing, .. } => missing.first(),
            _ => None,
        })
        .expect("warning carries the unsatisfied block");
    assert_eq!(block.relationships, vec!["PART_OF".to_string()]);
    assert_eq!(block.when_field.as_deref(), Some("status"));
    assert_eq!(block.when_value.as_deref(), Some("checked"));
}

/// The metadata flip that arms the block is caught on update: at
/// block tier the update refuses and the entity keeps its prior
/// value; at warn tier the same flip commits with the warning.
#[test]
fn update_flip_to_trigger_value_enforces_conditional_block() {
    let (actor, client) = cli_actor();
    let flip_to_checked = |engine: &mut Engine, id: &crate::entity::EntityId| {
        let current = engine.get_entity(id).unwrap().content_hash.clone();
        let mut metadata = IndexMap::new();
        metadata.insert("status".to_string(), "checked".to_string());
        engine.update_entity(
            crate::engine::UpdateEntityArgs {
                anchors: Vec::new(),
                id: id.clone(),
                expected_hash: Some(current),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata,
                metadata_unset: Vec::new(),
                declare_relations: vec![],
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
    };

    // Block tier: the flip refuses; the entity keeps `open`.
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_conditional_ro_schema(&tmp, "block");
    let mut open_args = task_create_args("Task A", vec![]);
    open_args
        .metadata
        .insert("status".to_string(), "open".to_string());
    let a = engine
        .create_entity(open_args, actor, Some(&client), None)
        .unwrap();
    let err = flip_to_checked(&mut engine, &a.id).unwrap_err();
    assert_eq!(err.code(), "MISSING_REQUIRED_OUTGOING");
    assert_eq!(
        engine.get_entity(&a.id).unwrap().metadata["status"].to_frontmatter_string(),
        "open",
        "refused update leaves the entity unchanged"
    );

    // Warn tier: the same flip commits with the typed warning.
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_conditional_ro_schema(&tmp, "warn");
    let b = engine
        .create_entity(
            task_create_args("Task B", vec![]),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let outcome = flip_to_checked(&mut engine, &b.id).unwrap();
    assert!(!outcome.write_id.is_empty());
    assert!(
        outcome
            .warnings
            .iter()
            .any(|w| matches!(w, WarningHint::MissingRequiredOutgoing { .. })),
        "warn-tier flip carries the warning: {:?}",
        outcome.warnings
    );
}

/// Fixture for declared acyclicity sets: one `claim` type over
/// GROUNDS / CONCLUDES, with `acyclic_sets: [[GROUNDS, CONCLUDES]]`
/// when `with_set` (neither rel-type carries the per-definition
/// `acyclic` flag, so without the set every cycle is legal).
fn engine_with_acyclic_set_schema(tmp: &TempDir, with_set: bool) -> Engine {
    let schemas_dir = tmp.path().join("schemas");
    let pkg = schemas_dir.join("argchain");
    std::fs::create_dir_all(pkg.join("types")).unwrap();
    let sets = if with_set {
        "  acyclic_sets:\n    - [GROUNDS, CONCLUDES]\n"
    } else {
        ""
    };
    std::fs::write(
        pkg.join("schema.yaml"),
        format!(
            r#"name: argchain
version: 0.1.0
description: acyclicity-set fixture
when_to_use: tests
types:
  - claim
relationships:
  mode: strict
{sets}  definitions:
    - name: GROUNDS
      description: g
      default_weight: 3.0
    - name: CONCLUDES
      description: c
      default_weight: 3.0
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#
        ),
    )
    .unwrap();
    std::fs::write(
        pkg.join("types").join("claim.yaml"),
        r#"name: claim
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
write_rules: []
"#,
    )
    .unwrap();
    let mem_dir = tmp.path().join("mem");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mount = crate::workspace::Mount {
        mem: "arg".to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            "argchain",
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

fn claim_args(title: &str, relations: Vec<crate::ops::RelateArg>) -> CreateEntityArgs {
    let mut sections = IndexMap::new();
    sections.insert("body".to_string(), "a claim body.".to_string());
    CreateEntityArgs {
        anchors: Vec::new(),
        mem: "arg".to_string(),
        title: title.to_string(),
        entity_type: "claim".to_string(),
        sections,
        metadata: IndexMap::new(),
        relations,
        dry_run: false,
    }
}

/// The experiment's alternating cycle: with `[GROUNDS, CONCLUDES]`
/// declared as one acyclicity set, the relate that closes a cycle
/// mixing both rel-types refuses with `RELATIONSHIP_CYCLE`; the
/// payload echoes the set and names each hop's rel-type. The same
/// graph WITHOUT the set declaration accepts the cycle (no
/// implicit derivation from anything else).
#[test]
fn relate_refuses_mixed_type_cycle_in_declared_set() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_acyclic_set_schema(&tmp, true);
    let (actor, client) = cli_actor();
    for t in ["A", "B", "C"] {
        engine
            .create_entity(claim_args(t, vec![]), actor, Some(&client), None)
            .unwrap();
    }
    relate(&mut engine, "arg--a", "GROUNDS", "arg--b").unwrap();
    relate(&mut engine, "arg--b", "CONCLUDES", "arg--c").unwrap();

    let err = relate(&mut engine, "arg--c", "GROUNDS", "arg--a").unwrap_err();
    assert_eq!(err.code(), "RELATIONSHIP_CYCLE", "{err:?}");
    let details = err.details();
    assert_eq!(
        details["acyclic_set"],
        serde_json::json!(["GROUNDS", "CONCLUDES"])
    );
    assert_eq!(
        details["existing_path"],
        serde_json::json!(["arg--a", "arg--b", "arg--c"])
    );
    assert_eq!(
        details["existing_path_rel_types"],
        serde_json::json!(["GROUNDS", "CONCLUDES"]),
        "one rel-type per hop, mixing both members"
    );

    // Complement: the identical graph without the declaration
    // accepts the cycle.
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_acyclic_set_schema(&tmp, false);
    for t in ["A", "B", "C"] {
        engine
            .create_entity(claim_args(t, vec![]), actor, Some(&client), None)
            .unwrap();
    }
    relate(&mut engine, "arg--a", "GROUNDS", "arg--b").unwrap();
    relate(&mut engine, "arg--b", "CONCLUDES", "arg--c").unwrap();
    relate(&mut engine, "arg--c", "GROUNDS", "arg--a")
        .expect("without the set declaration the cycle is legal");
}

/// The set refusal fires identically for inline relations on
/// create (through a promoted stub) and declared relations on
/// update.
#[test]
fn create_inline_and_update_declared_refuse_set_cycle() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_acyclic_set_schema(&tmp, true);
    let (actor, client) = cli_actor();

    // create.relations[]: A → GROUNDS → ghost auto-stubs `ghost`;
    // promoting the stub with a CONCLUDES back-edge closes a
    // mixed-type cycle.
    engine
        .create_entity(
            claim_args(
                "Alpha",
                vec![crate::ops::RelateArg {
                    target: crate::entity::EntityId("arg--ghost".to_string()),
                    rel_type: "GROUNDS".to_string(),
                    description: None,
                }],
            ),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let err = engine
        .create_entity(
            claim_args(
                "Ghost",
                vec![crate::ops::RelateArg {
                    target: crate::entity::EntityId("arg--alpha".to_string()),
                    rel_type: "CONCLUDES".to_string(),
                    description: None,
                }],
            ),
            actor,
            Some(&client),
            None,
        )
        .expect_err("cycle-closing create.relations[] must refuse");
    assert_eq!(err.code(), "RELATIONSHIP_CYCLE", "{err:?}");
    assert_eq!(
        err.details()["acyclic_set"],
        serde_json::json!(["GROUNDS", "CONCLUDES"])
    );

    // update.declare_relations: D → GROUNDS → E exists; updating E
    // with CONCLUDES → D closes the mixed cycle.
    for t in ["D", "E"] {
        engine
            .create_entity(claim_args(t, vec![]), actor, Some(&client), None)
            .unwrap();
    }
    relate(&mut engine, "arg--d", "GROUNDS", "arg--e").unwrap();
    let e_id = crate::entity::EntityId("arg--e".to_string());
    let current = engine.get_entity(&e_id).unwrap().content_hash.clone();
    let err = engine
        .update_entity(
            crate::engine::UpdateEntityArgs {
                anchors: Vec::new(),
                id: e_id,
                expected_hash: Some(current),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: vec![crate::ops::RelateArg {
                    target: crate::entity::EntityId("arg--d".to_string()),
                    rel_type: "CONCLUDES".to_string(),
                    description: None,
                }],
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .expect_err("cycle-closing declare_relations must refuse");
    assert_eq!(err.code(), "RELATIONSHIP_CYCLE", "{err:?}");
    assert_eq!(
        err.details()["acyclic_set"],
        serde_json::json!(["GROUNDS", "CONCLUDES"])
    );
}

/// A package whose on-disk edges already close a mixed-type cycle
/// boots with one cycle-closing edge dropped and warned, exactly
/// as the single-type sweep does.
#[test]
fn boot_drops_cycle_closing_edge_in_declared_acyclic_set() {
    let tmp = TempDir::new().unwrap();
    // Write the schema and two claim files closing a GROUNDS /
    // CONCLUDES cycle BEFORE boot, then reuse the fixture builder
    // (same schemas dir and mem dir layout).
    let mem_dir = tmp.path().join("mem");
    std::fs::create_dir_all(&mem_dir).unwrap();
    std::fs::write(
            mem_dir.join("a.md"),
            "---\ntype: claim\n---\n# A\n\n## Body\n\nfirst.\n\n## Relationships\n\n- **GROUNDS**: [[arg--b]]\n",
        )
        .unwrap();
    std::fs::write(
            mem_dir.join("b.md"),
            "---\ntype: claim\n---\n# B\n\n## Body\n\nsecond.\n\n## Relationships\n\n- **CONCLUDES**: [[arg--a]]\n",
        )
        .unwrap();
    let engine = engine_with_acyclic_set_schema(&tmp, true);

    let surviving: usize = engine
        .store()
        .all_entities()
        .map(|e| {
            engine
                .store()
                .outgoing(&e.id)
                .iter()
                .filter(|edge| edge.rel_type == "GROUNDS" || edge.rel_type == "CONCLUDES")
                .count()
        })
        .sum();
    assert_eq!(surviving, 1, "exactly one edge survives the cycle break");
    let cycle_warnings: Vec<_> = engine
        .load_warnings()
        .iter()
        .filter(|w| {
            matches!(
                w,
                WarningHint::ParsedRelationInvalid { reason, .. } if reason == "cycle"
            )
        })
        .cloned()
        .collect();
    assert_eq!(
        cycle_warnings.len(),
        1,
        "exactly one cycle warning fires: {cycle_warnings:?}"
    );
}

/// Fixture for aggregate signals: `claim` declares `attack_load`
/// (in-REBUTS count, notice at 1, warn at 3) and
/// `open_objections` (same set, counterpart `state: open` only,
/// notice at 1); `objection` declares the `state` enum.
/// Fixture for the grounded labelling: `arglab` declares
/// `labelling.attack: [REBUTS]` and a support walk over GROUNDS
/// (direction out, terminal `evidence`); mem `arg` (and, when the
/// test mounts it, mem `other`) pin it.
fn engine_with_labelling_schema(tmp: &TempDir, with_other_mem: bool) -> Engine {
    engine_with_labelling_schema_support(tmp, with_other_mem, true)
}

fn engine_with_labelling_schema_support(
    tmp: &TempDir,
    with_other_mem: bool,
    with_support: bool,
) -> Engine {
    let schemas_dir = tmp.path().join("schemas");
    let pkg = schemas_dir.join("arglab");
    std::fs::create_dir_all(pkg.join("types")).unwrap();
    std::fs::write(
            pkg.join("schema.yaml"),
            format!(
                r#"name: arglab
version: 0.1.0
description: grounded-labelling fixture
when_to_use: tests
types:
  - claim
  - evidence
relationships:
  mode: strict
  labelling:
    attack: [REBUTS]
{support}  definitions:
    - name: REBUTS
      description: attack
      default_weight: 3.0
    - name: GROUNDS
      description: support
      default_weight: 3.0
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
cross_mem_relationships:
  - to_schema: arglab
    definitions:
      - name: REBUTS
        description: cross-mem attack
        default_weight: 3.0
"#,
                support = if with_support {
                    "    support:\n      relationships: [GROUNDS]\n      direction: out\n      terminal_types: [evidence]\n"
                } else {
                    ""
                },
            ),
        )
        .unwrap();
    let body = "sections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n";
    for t in ["claim", "evidence"] {
        std::fs::write(
            pkg.join("types").join(format!("{t}.yaml")),
            format!("name: {t}\ndescription: t\nwhen_to_use: tests\n{body}"),
        )
        .unwrap();
    }
    let mut mounts: Vec<(crate::workspace::Mount, Box<dyn MemBackend>)> = Vec::new();
    for mem in std::iter::once("arg").chain(with_other_mem.then_some("other")) {
        let mem_dir = tmp.path().join(format!("mem-{mem}"));
        std::fs::create_dir_all(&mem_dir).unwrap();
        let writer = FilesystemBackend::new(mem_dir.clone());
        mounts.push((
            crate::workspace::Mount {
                mem: mem.to_string(),
                schema: Some(memstead_schema::SchemaRef::new(
                    "arglab",
                    semver::Version::new(0, 1, 0),
                )),
                storage: crate::workspace::MountStorage::Folder { path: mem_dir },
                capability: crate::workspace::MountCapability::Write,
                lifecycle: crate::workspace::MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            },
            Box::new(writer) as Box<dyn MemBackend>,
        ));
    }
    Engine::from_mounts_with_schemas_dir(mounts, Some(&schemas_dir)).unwrap()
}

fn lab_create(engine: &mut Engine, mem: &str, title: &str, entity_type: &str) {
    let (actor, client) = cli_actor();
    let mut sections = IndexMap::new();
    sections.insert("body".to_string(), "a body.".to_string());
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: mem.to_string(),
                title: title.to_string(),
                entity_type: entity_type.to_string(),
                sections,
                metadata: IndexMap::new(),
                relations: vec![],
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
}

fn lab_relate(engine: &mut Engine, from: &str, rel: &str, to: &str) {
    let (actor, client) = cli_actor();
    engine
        .relate_entity(
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
        .unwrap();
}

fn label_of(engine: &Engine, id: &str) -> (String, Vec<String>, Vec<String>) {
    let entity = engine
        .get_entity(&crate::entity::EntityId(id.to_string()))
        .unwrap()
        .clone();
    let view = engine.computed_labelling(&entity).unwrap();
    (
        view.label.wire().to_string(),
        view.defeated_by.clone(),
        view.undecided_by.clone(),
    )
}

/// The hand-computed grounded extension: an unattacked claim is
/// accepted, a chain defeats, a defeated attacker reinstates, a
/// cycle stays undecided (and keeps its victims undecided); the
/// evidence names the accepted / undecided direct attackers; two
/// instances serve identical labels; the memo invalidates on the
/// in-process mutation path AND the reload path; labels never
/// gate writes; the health axis serves counts with evidence.
#[test]
fn grounded_labelling_extension_evidence_and_invalidation() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_labelling_schema(&tmp, false);
    for t in ["A", "B", "C", "D", "E", "F"] {
        lab_create(&mut engine, "arg", t, "claim");
    }
    lab_relate(&mut engine, "arg--a", "REBUTS", "arg--b");
    lab_relate(&mut engine, "arg--b", "REBUTS", "arg--c");
    lab_relate(&mut engine, "arg--d", "REBUTS", "arg--e");
    lab_relate(&mut engine, "arg--e", "REBUTS", "arg--d");
    lab_relate(&mut engine, "arg--d", "REBUTS", "arg--f");

    assert_eq!(label_of(&engine, "arg--a").0, "accepted", "unattacked");
    let (label, defeated_by, _) = label_of(&engine, "arg--b");
    assert_eq!(label, "defeated");
    assert_eq!(defeated_by, vec!["arg--a".to_string()], "evidence ships");
    assert_eq!(
        label_of(&engine, "arg--c").0,
        "accepted",
        "reinstatement: the only attacker is itself defeated"
    );
    let (label, _, undecided_by) = label_of(&engine, "arg--d");
    assert_eq!(label, "undecided", "cycle member");
    assert_eq!(undecided_by, vec!["arg--e".to_string()]);
    let (label, _, undecided_by) = label_of(&engine, "arg--f");
    assert_eq!(label, "undecided", "victim of an undecided attacker");
    assert_eq!(undecided_by, vec!["arg--d".to_string()]);

    // Determinism: a second instance over the same on-disk state.
    let engine_b = engine_with_labelling_schema(&tmp, false);
    for id in ["arg--a", "arg--b", "arg--c", "arg--d", "arg--e", "arg--f"] {
        assert_eq!(label_of(&engine, id), label_of(&engine_b, id));
    }

    // Health axis: counts per label, evidence on the lists.
    let axis = engine.health_labelling_axis(None);
    assert_eq!(axis["arg"]["counts"]["accepted"], 2);
    assert_eq!(axis["arg"]["counts"]["defeated"], 1);
    assert_eq!(axis["arg"]["counts"]["undecided"], 3);
    assert_eq!(axis["arg"]["defeated"][0]["id"], "arg--b");
    assert_eq!(axis["arg"]["defeated"][0]["defeated_by"][0], "arg--a");

    // Labels never gate writes: updating a defeated entity and
    // adding an edge into it both succeed with the normal shapes.
    let (actor, client) = cli_actor();
    let b_id = crate::entity::EntityId("arg--b".to_string());
    let current = engine.get_entity(&b_id).unwrap().content_hash.clone();
    let mut sections = IndexMap::new();
    sections.insert("body".to_string(), "updated body.".to_string());
    let outcome = engine
        .update_entity(
            crate::engine::UpdateEntityArgs {
                anchors: Vec::new(),
                id: b_id,
                expected_hash: Some(current),
                sections,
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: vec![],
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .expect("updating a defeated entity succeeds");
    assert!(!outcome.write_id.is_empty());
    lab_relate(&mut engine, "arg--f", "REBUTS", "arg--b");

    // In-process invalidation: a fresh unattacked attacker flips
    // A on the next read.
    lab_create(&mut engine, "arg", "H", "claim");
    lab_relate(&mut engine, "arg--h", "REBUTS", "arg--a");
    let (label, defeated_by, _) = label_of(&engine, "arg--a");
    assert_eq!(
        label, "defeated",
        "memo invalidated by the in-process mutation"
    );
    assert_eq!(defeated_by, vec!["arg--h".to_string()]);

    // Reload-path invalidation: an out-of-band file change plus an
    // explicit mem reload serves the new labelling.
    let g_path = tmp.path().join("mem-arg").join("g.md");
    std::fs::write(
            &g_path,
            "---\ntype: claim\n---\n# G\n\n## Body\n\nout of band.\n\n## Relationships\n\n- **REBUTS**: [[arg--c]]\n",
        )
        .unwrap();
    engine.reload_one_mem("arg").expect("reload succeeds");
    let (label, defeated_by, _) = label_of(&engine, "arg--c");
    assert_eq!(label, "defeated", "reload invalidated the memo");
    assert!(defeated_by.contains(&"arg--g".to_string()));
}

/// Support-blindness, chain shape, cross-mem exclusion, and the
/// no-declaration complement.
#[test]
fn labelling_support_blindness_shape_and_cross_mem() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_labelling_schema(&tmp, true);
    // Support chain: conclusion → inference → evidence; an
    // undercutter defeats the inference.
    for (t, ty) in [
        ("Conclusion", "claim"),
        ("Inference", "claim"),
        ("Undercutter", "claim"),
    ] {
        lab_create(&mut engine, "arg", t, ty);
    }
    lab_create(&mut engine, "arg", "Ev One", "evidence");
    lab_relate(&mut engine, "arg--conclusion", "GROUNDS", "arg--inference");
    lab_relate(&mut engine, "arg--inference", "GROUNDS", "arg--ev-one");
    lab_relate(&mut engine, "arg--undercutter", "REBUTS", "arg--inference");

    // Support-blindness: the defeated inference leaves its
    // conclusion accepted; the defeat shows in the shape count.
    let conclusion = engine
        .get_entity(&crate::entity::EntityId("arg--conclusion".to_string()))
        .unwrap()
        .clone();
    let view = engine.computed_labelling(&conclusion).unwrap();
    assert_eq!(view.label.wire(), "accepted", "support-blind by design");
    let shape = view.shape.expect("support declared, shape served");
    assert_eq!(shape.depth, 2);
    assert!((shape.branching - 1.0).abs() < 1e-9);
    assert_eq!(shape.terminal_share, Some(1.0));
    assert_eq!(shape.defeated_in_support, 1, "the defeated inference");
    assert_eq!(shape.undecided_in_support, 0);

    // Isolated entity: zeros and a null share.
    lab_create(&mut engine, "arg", "Loner", "claim");
    let loner = engine
        .get_entity(&crate::entity::EntityId("arg--loner".to_string()))
        .unwrap()
        .clone();
    let view = engine.computed_labelling(&loner).unwrap();
    let shape = view.shape.unwrap();
    assert_eq!(
        (shape.depth, shape.branching, shape.terminal_share),
        (0, 0.0, None)
    );
    assert_eq!(
        (shape.defeated_in_support, shape.undecided_in_support),
        (0, 0)
    );

    // Cross-mem: an attack edge from `other` into `arg` (granted
    // by workspace policy) is excluded from the computation and
    // counted; the target stays accepted.
    let mut settings = engine.settings().clone();
    settings.cross_mem_links.insert(
        "other".to_string(),
        memstead_schema::workspace_config::CrossLinkValue::Wildcard,
    );
    engine.set_settings(settings);
    lab_create(&mut engine, "other", "Foreign", "claim");
    lab_relate(&mut engine, "other--foreign", "REBUTS", "arg--conclusion");
    let conclusion = engine
        .get_entity(&crate::entity::EntityId("arg--conclusion".to_string()))
        .unwrap()
        .clone();
    let view = engine.computed_labelling(&conclusion).unwrap();
    assert_eq!(
        view.label.wire(),
        "accepted",
        "the cross-mem attack is excluded, never guessed"
    );
    let axis = engine.health_labelling_axis(Some("arg"));
    assert_eq!(axis["arg"]["cross_mem_edges_excluded"], 1);
    assert!(axis.get("other").is_none(), "mem filter narrows");

    // Serving channels: envelope `_labelling` and the text
    // channel's `_label` + `## Labelling`; the canonical form
    // stays projection-free.
    let inference = engine
        .get_entity(&crate::entity::EntityId("arg--inference".to_string()))
        .unwrap()
        .clone();
    let view = engine.computed_labelling(&inference).unwrap();
    let md =
        crate::render::render_entity_markdown_with_signals(&inference, None, None, Some(&view));
    assert!(md.contains("_label: defeated"), "{md}");
    assert!(md.contains("## Labelling"), "{md}");
    assert!(md.contains("defeated_by: arg--undercutter"), "{md}");
    let env = crate::render::build_entity_envelope(
        &inference,
        0,
        None,
        None,
        None,
        crate::render::OriginClass::FirstParty,
        engine
            .store()
            .outgoing(&crate::entity::EntityId("arg--inference".to_string())),
        None,
        None,
        Some(&view),
    );
    assert_eq!(env["_labelling"]["label"], "defeated");
    assert_eq!(env["_labelling"]["defeated_by"][0], "arg--undercutter");
    assert!(env["_labelling"]["shape"].is_object());
    let canonical = crate::render::render_entity_markdown(&inference, None);
    assert!(
        !canonical.contains("_label") && !canonical.contains("## Labelling"),
        "canonical form is projection-free"
    );

    // No-declaration complement: a signals-fixture entity (schema
    // without labelling) serves no labelling view at all.
    let tmp2 = TempDir::new().unwrap();
    let mut plain = engine_with_signals_schema(&tmp2);
    let (actor, client) = cli_actor();
    let mut sections = IndexMap::new();
    sections.insert("body".to_string(), "a claim body.".to_string());
    plain
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "arg".to_string(),
                title: "Plain".to_string(),
                entity_type: "claim".to_string(),
                sections,
                metadata: IndexMap::new(),
                relations: vec![],
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let plain_entity = plain
        .get_entity(&crate::entity::EntityId("arg--plain".to_string()))
        .unwrap()
        .clone();
    assert!(plain.computed_labelling(&plain_entity).is_none());
}

/// An attack-only declaration (no `support` walk) serves labels
/// but nothing shape-shaped on any channel.
#[test]
fn labelling_without_support_serves_no_shape() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_labelling_schema_support(&tmp, false, false);
    lab_create(&mut engine, "arg", "Solo", "claim");
    let entity = engine
        .get_entity(&crate::entity::EntityId("arg--solo".to_string()))
        .unwrap()
        .clone();
    let view = engine.computed_labelling(&entity).expect("labels served");
    assert_eq!(view.label.wire(), "accepted");
    assert!(view.shape.is_none(), "no support declaration, no shape");
    assert!(view.to_json().get("shape").is_none());
    let md = crate::render::render_entity_markdown_with_signals(&entity, None, None, Some(&view));
    assert!(!md.contains("- shape:"), "{md}");
}

fn engine_with_signals_schema(tmp: &TempDir) -> Engine {
    let schemas_dir = tmp.path().join("schemas");
    let pkg = schemas_dir.join("argsig");
    std::fs::create_dir_all(pkg.join("types")).unwrap();
    std::fs::write(
        pkg.join("schema.yaml"),
        r#"name: argsig
version: 0.1.0
description: aggregate-signal fixture
when_to_use: tests
types:
  - claim
  - objection
relationships:
  mode: strict
  definitions:
    - name: REBUTS
      description: r
      default_weight: 3.0
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#,
    )
    .unwrap();
    let body = "sections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n";
    std::fs::write(
            pkg.join("types").join("claim.yaml"),
            format!(
                "name: claim\ndescription: t\nwhen_to_use: tests\nmetadata_fields: []\nupdatable_fields:\n  - title\n  - body\n{body}signals:\n  - name: attack_load\n    kind: edge_load\n    relationships: [REBUTS]\n    direction: in\n    thresholds:\n      - at_least: 1\n        level: notice\n      - at_least: 3\n        level: warn\n  - name: open_objections\n    kind: edge_load\n    relationships: [REBUTS]\n    direction: in\n    neighbour_field: state\n    neighbour_value: open\n    thresholds:\n      - at_least: 1\n        level: notice\n"
            ),
        )
        .unwrap();
    std::fs::write(
            pkg.join("types").join("objection.yaml"),
            format!(
                "name: objection\ndescription: t\nwhen_to_use: tests\nmetadata_fields:\n  - key: state\n    description: objection lifecycle\n    field_type: string\n    enum_values: [open, closed]\nupdatable_fields:\n  - title\n  - body\n  - state\n{body}"
            ),
        )
        .unwrap();
    let mem_dir = tmp.path().join("mem");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mount = crate::workspace::Mount {
        mem: "arg".to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            "argsig",
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

fn objection_args(title: &str, state: Option<&str>, rebuts: Option<&str>) -> CreateEntityArgs {
    let mut sections = IndexMap::new();
    sections.insert("body".to_string(), "an objection body.".to_string());
    let mut metadata = IndexMap::new();
    if let Some(s) = state {
        metadata.insert("state".to_string(), s.to_string());
    }
    let relations = rebuts
        .map(|to| {
            vec![crate::ops::RelateArg {
                target: crate::entity::EntityId(to.to_string()),
                rel_type: "REBUTS".to_string(),
                description: None,
            }]
        })
        .unwrap_or_default();
    CreateEntityArgs {
        anchors: Vec::new(),
        mem: "arg".to_string(),
        title: title.to_string(),
        entity_type: "objection".to_string(),
        sections,
        metadata,
        relations,
        dry_run: false,
    }
}

fn crossing_warnings_of(warnings: &[WarningHint]) -> Vec<(String, String, u64, String, String)> {
    warnings
        .iter()
        .filter_map(|w| match w {
            WarningHint::SignalThresholdCrossed {
                entity_id,
                signal,
                value,
                old_level,
                new_level,
            } => Some((
                entity_id.to_string(),
                signal.clone(),
                *value,
                old_level.clone(),
                new_level.clone(),
            )),
            _ => None,
        })
        .collect()
}

/// Signals on reads: thresholds map boundary counts to levels, the
/// neighbour filter counts only qualifying counterparts, the two
/// serving channels carry headline + contributors, a flip of the
/// counterpart's field changes the count on the next read, and two
/// engine instances over the same on-disk state serve identical
/// payloads.
#[test]
fn signals_reads_thresholds_neighbour_filter_and_determinism() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_signals_schema(&tmp);
    let (actor, client) = cli_actor();

    let mut claim_sections = IndexMap::new();
    claim_sections.insert("body".to_string(), "a claim body.".to_string());
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "arg".to_string(),
                title: "Claim".to_string(),
                entity_type: "claim".to_string(),
                sections: claim_sections,
                metadata: IndexMap::new(),
                relations: vec![],
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let claim_id = crate::entity::EntityId("arg--claim".to_string());
    let sig_of = |engine: &Engine, name: &str| {
        let entity = engine.get_entity(&claim_id).unwrap().clone();
        engine
            .computed_signals(&entity)
            .unwrap()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap()
    };

    // Below the first threshold: value 0, level none.
    let s = sig_of(&engine, "attack_load");
    assert_eq!((s.value, s.level_wire()), (0, "none"));

    // First objection (open, inline REBUTS): the create's crossing
    // warnings name BOTH signals moving none → notice on the claim.
    let outcome = engine
        .create_entity(
            objection_args("Obj One", Some("open"), Some("arg--claim")),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let crossings = crossing_warnings_of(&outcome.warnings);
    assert!(
        crossings.contains(&(
            "arg--claim".to_string(),
            "attack_load".to_string(),
            1,
            "none".to_string(),
            "notice".to_string()
        )),
        "create with inline relation crosses attack_load: {crossings:?}"
    );
    assert!(
        crossings.iter().any(|c| c.1 == "open_objections"),
        "open counterpart crosses open_objections too: {crossings:?}"
    );

    // Closed and field-less counterparts count for attack_load,
    // never for open_objections.
    engine
        .create_entity(
            objection_args("Obj Two", Some("closed"), Some("arg--claim")),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    engine
        .create_entity(
            objection_args("Obj Three", None, Some("arg--claim")),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let attack = sig_of(&engine, "attack_load");
    assert_eq!((attack.value, attack.level_wire()), (3, "warn"));
    assert_eq!(attack.contributors.len(), 3);
    let open = sig_of(&engine, "open_objections");
    assert_eq!((open.value, open.level_wire()), (1, "notice"));
    assert_eq!(open.contributors[0].0, "arg--obj-one");

    // Both serving channels: frontmatter headline + `## Signals`
    // contributors on the text channel; `_signals` on the envelope.
    let entity = engine.get_entity(&claim_id).unwrap().clone();
    let signals = engine.computed_signals(&entity).unwrap();
    let md =
        crate::render::render_entity_markdown_with_signals(&entity, None, Some(&signals), None);
    assert!(
        md.contains("_signals: [attack_load: 3 (warn), open_objections: 1 (notice)]"),
        "frontmatter headline: {md}"
    );
    assert!(md.contains("## Signals"), "contributors section: {md}");
    assert!(md.contains("arg--obj-one"), "evidence ships: {md}");
    let env = crate::render::build_entity_envelope(
        &entity,
        0,
        None,
        None,
        None,
        crate::render::OriginClass::FirstParty,
        engine.store().outgoing(&claim_id),
        None,
        Some(&signals),
        None,
    );
    assert_eq!(env["_signals"][0]["name"], "attack_load");
    assert_eq!(env["_signals"][0]["value"], 3);
    assert_eq!(env["_signals"][0]["level"], "warn");
    assert_eq!(
        env["_signals"][0]["contributors"].as_array().unwrap().len(),
        3
    );
    // The canonical form stays signal-free.
    let canonical = crate::render::render_entity_markdown(&entity, None);
    assert!(
        !canonical.contains("_signals"),
        "canonical form is signal-free"
    );

    // Flipping the counterpart's field changes the count on the
    // next read, and the update carries the crossing for the CLAIM.
    let obj_one = crate::entity::EntityId("arg--obj-one".to_string());
    let current = engine.get_entity(&obj_one).unwrap().content_hash.clone();
    let mut metadata = IndexMap::new();
    metadata.insert("state".to_string(), "closed".to_string());
    let outcome = engine
        .update_entity(
            crate::engine::UpdateEntityArgs {
                anchors: Vec::new(),
                id: obj_one,
                expected_hash: Some(current),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata,
                metadata_unset: Vec::new(),
                declare_relations: vec![],
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert!(!outcome.write_id.is_empty(), "success shape kept");
    let crossings = crossing_warnings_of(&outcome.warnings);
    assert!(
        crossings.contains(&(
            "arg--claim".to_string(),
            "open_objections".to_string(),
            0,
            "notice".to_string(),
            "none".to_string()
        )),
        "the neighbour flip crosses the claim's filtered signal: {crossings:?}"
    );
    let open = sig_of(&engine, "open_objections");
    assert_eq!((open.value, open.level_wire()), (0, "none"));

    // Determinism: a second instance over the same on-disk state
    // serves an identical payload.
    let engine_b = engine_with_signals_schema(&tmp);
    let entity_b = engine_b.get_entity(&claim_id).unwrap().clone();
    let signals_b = engine_b.computed_signals(&entity_b).unwrap();
    let entity_a = engine.get_entity(&claim_id).unwrap().clone();
    let signals_a = engine.computed_signals(&entity_a).unwrap();
    assert_eq!(
        crate::ops::signals::signals_json(&signals_a),
        crate::ops::signals::signals_json(&signals_b),
        "two instances over the same mem state serve identical signals"
    );
}

/// Crossings on the relate surface: upward and downward crossings
/// warn with the full detail set; a write that crosses nothing
/// carries nothing signal-shaped; the mutation stays the success
/// shape throughout. The `signals` health axis serves only
/// above-`none` entities, with per-level counts and a mem filter.
#[test]
fn signal_crossings_on_relate_and_health_axis() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_signals_schema(&tmp);
    let (actor, client) = cli_actor();

    let mut claim_sections = IndexMap::new();
    claim_sections.insert("body".to_string(), "a claim body.".to_string());
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "arg".to_string(),
                title: "Claim".to_string(),
                entity_type: "claim".to_string(),
                sections: claim_sections,
                metadata: IndexMap::new(),
                relations: vec![],
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    for (t, s) in [("Obj A", "open"), ("Obj B", "closed"), ("Obj C", "closed")] {
        engine
            .create_entity(objection_args(t, Some(s), None), actor, Some(&client), None)
            .unwrap();
    }
    let relate = |engine: &mut Engine, from: &str, remove: bool| {
        engine
            .relate_entity(
                crate::engine::RelateEntityArgs {
                    source: crate::entity::EntityId(from.to_string()),
                    target: crate::entity::EntityId("arg--claim".to_string()),
                    rel_type: "REBUTS".to_string(),
                    description: None,
                    remove,
                    expected_hash: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap()
    };

    // 0 → 1: none → notice (upward), on the TARGET of the edge.
    let outcome = relate(&mut engine, "arg--obj-a", false);
    assert!(!outcome.write_id.is_empty());
    let crossings = crossing_warnings_of(&outcome.warnings);
    assert!(
        crossings.contains(&(
            "arg--claim".to_string(),
            "attack_load".to_string(),
            1,
            "none".to_string(),
            "notice".to_string()
        )),
        "{crossings:?}"
    );

    // 1 → 2: notice → notice — nothing signal-shaped rides.
    let outcome = relate(&mut engine, "arg--obj-b", false);
    assert!(
        crossing_warnings_of(&outcome.warnings).is_empty(),
        "no threshold crossed, no warning: {:?}",
        outcome.warnings
    );

    // 2 → 3: notice → warn.
    let outcome = relate(&mut engine, "arg--obj-c", false);
    let crossings = crossing_warnings_of(&outcome.warnings);
    assert!(
        crossings.contains(&(
            "arg--claim".to_string(),
            "attack_load".to_string(),
            3,
            "notice".to_string(),
            "warn".to_string()
        )),
        "{crossings:?}"
    );

    // Health axis at warn: the claim is the one above-`none`
    // entity; counts split per level; a mem filter narrows.
    let axis = engine.health_signals_axis(None);
    assert_eq!(axis["entities"].as_array().unwrap().len(), 1);
    assert_eq!(axis["entities"][0]["id"], "arg--claim");
    assert_eq!(axis["counts"]["warn"], 1, "attack_load at warn: {axis}");
    assert_eq!(axis["counts"]["notice"], 1, "open_objections at notice");
    let filtered = engine.health_signals_axis(Some("other"));
    assert!(filtered["entities"].as_array().unwrap().is_empty());

    // 3 → 2: warn → notice (downward crossing warns too).
    let outcome = relate(&mut engine, "arg--obj-c", true);
    let crossings = crossing_warnings_of(&outcome.warnings);
    assert!(
        crossings.contains(&(
            "arg--claim".to_string(),
            "attack_load".to_string(),
            2,
            "warn".to_string(),
            "notice".to_string()
        )),
        "{crossings:?}"
    );
}

/// Regression pin for `no_self_loop_relationships`' single
/// functional behavior: a self-loop (`from == to`) on a rel-type
/// the source type lists there refuses with `RELATIONSHIP_CYCLE`.
/// The constraint vocabulary settles the field's semantics — the
/// new propagation declaration gets a distinct name, and this pin
/// guards that the old field keeps exactly this effect.
#[test]
fn no_self_loop_rel_type_self_loop_refusal_is_pinned() {
    let tmp = TempDir::new().unwrap();
    // The `constr` fixture declares `no_self_loop_relationships:
    // [PART_OF]` on `task`.
    let mut engine = engine_with_constraints_schema(&tmp, "warn", "warn");
    let (actor, client) = cli_actor();
    let a = engine
        .create_entity(
            task_create_args("Task A", vec![]),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let err = engine
        .relate_entity(
            crate::engine::RelateEntityArgs {
                source: a.id.clone(),
                target: a.id.clone(),
                rel_type: "PART_OF".to_string(),
                description: None,
                remove: false,
                expected_hash: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "RELATIONSHIP_CYCLE");
}
