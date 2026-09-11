//! The proof schemas (grounding, plenum) end to end through create,
//! update and health, plus the required-outgoing warnings on create
//! and update.

use super::*;

fn proof_create(
    engine: &mut Engine,
    entity_type: &str,
    title: &str,
    metadata: &[(&str, &str)],
    relations: Vec<crate::ops::RelateArg>,
) -> Result<CreateEntityOutcome, EngineError> {
    let (actor, client) = cli_actor();
    let mut sections = IndexMap::new();
    sections.insert("body".to_string(), format!("{title} body."));
    let mut md = IndexMap::new();
    for (k, v) in metadata {
        md.insert(k.to_string(), v.to_string());
    }
    engine.create_entity(
        CreateEntityArgs {
            anchors: Vec::new(),
            mem: "proof".to_string(),
            title: title.to_string(),
            entity_type: entity_type.to_string(),
            sections,
            metadata: md,
            relations,
            dry_run: false,
        },
        actor,
        Some(&client),
        None,
    )
}

const GROUNDING_MANIFEST: &str = r#"name: grounding
version: 0.1.0
description: anker-shaped grounding proof schema
when_to_use: constraint-proof tests
types:
  - anchor
  - tradeoff
relationships:
  mode: strict
  definitions:
    - name: FOLLOWS_FROM
      description: stands on
      default_weight: 3.0
    - name: SUPPORTS
      description: pro
      default_weight: 1.0
    - name: OPPOSES
      description: contra
      default_weight: 1.0
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;

const GROUNDING_ANCHOR: &str = r#"name: anchor
description: a judgment standing on others
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
    description: lifecycle
    field_type: string
    enum_values: [open, checked, fallen]
  - key: checked_by
    description: who checked
    field_type: string
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
  - status
  - checked_by
health_required_fields:
  - body
staleness_threshold_days: 90
constraints:
  - kind: requires_when
    field: checked_by
    when_field: status
    when_value: checked
  - kind: status_propagation
    field: status
    value: fallen
    rel_type: FOLLOWS_FROM
    direction: incoming
write_rules: []
"#;

const GROUNDING_TRADEOFF: &str = r#"name: tradeoff
description: a claim with two sides
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
  - relationships: [SUPPORTS]
    cardinality: at_least_one
  - relationships: [OPPOSES]
    cardinality: at_least_one
write_rules: []
"#;

/// The anker proof: the grounding-shaped
/// schema answers `pruefe_kette.py`'s check questions 1–3 from
/// health output alone — no project Python.
#[test]
fn anker_proof_grounding_schema_answers_check_questions_from_health() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_proof_schema(
        &tmp,
        "grounding",
        GROUNDING_MANIFEST,
        &[
            ("anchor", GROUNDING_ANCHOR),
            ("tradeoff", GROUNDING_TRADEOFF),
        ],
    );

    // A fallen root, a child standing on it, a grandchild standing
    // on the child (transitive), plus an untainted sibling chain.
    let root = proof_create(
        &mut engine,
        "anchor",
        "Root",
        &[("status", "fallen")],
        vec![],
    )
    .unwrap();
    let child = proof_create(
        &mut engine,
        "anchor",
        "Child",
        &[],
        vec![rel(&root.id.0, "FOLLOWS_FROM")],
    )
    .unwrap();
    let grandchild = proof_create(
        &mut engine,
        "anchor",
        "Grandchild",
        &[],
        vec![rel(&child.id.0, "FOLLOWS_FROM")],
    )
    .unwrap();
    let standing = proof_create(
        &mut engine,
        "anchor",
        "Standing Root",
        &[("status", "open")],
        vec![],
    )
    .unwrap();
    let standing_child = proof_create(
        &mut engine,
        "anchor",
        "Standing Child",
        &[],
        vec![rel(&standing.id.0, "FOLLOWS_FROM")],
    )
    .unwrap();
    // Question 3's subject: checked without a checker.
    let unchecked = proof_create(
        &mut engine,
        "anchor",
        "Checked No Checker",
        &[("status", "checked")],
        vec![],
    )
    .unwrap();
    // Question 2's subject: a one-sided trade-off.
    let onesided = proof_create(
        &mut engine,
        "tradeoff",
        "One Sided",
        &[],
        vec![rel(&standing.id.0, "SUPPORTS")],
    )
    .unwrap();

    // Question 1 — descendants of the fallen anchor are flagged,
    // naming their ancestor; the standing chain is not.
    let findings = crate::ops::health::collect_constraint_findings(
        engine.store(),
        None,
        engine.schemas(),
        None,
    );
    let tainted_of = |id: &crate::entity::EntityId| -> Vec<String> {
        findings
            .iter()
            .filter(|r| &r.id == id)
            .flat_map(|r| &r.violations)
            .filter_map(|v| match v {
                crate::ops::health::UnsatisfiedConstraint::StatusPropagation {
                    tainted_by, ..
                } => Some(tainted_by.clone()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(tainted_of(&child.id), vec![root.id.to_string()]);
    assert_eq!(
        tainted_of(&grandchild.id),
        vec![root.id.to_string()],
        "the taint is transitive and names the terminal ancestor"
    );
    assert!(tainted_of(&standing_child.id).is_empty());
    assert!(
        tainted_of(&root.id).is_empty(),
        "the source is not its own finding"
    );

    // Question 3 — checked-without-checker is flagged.
    assert!(
        findings.iter().any(|r| r.id == unchecked.id
            && r.violations.iter().any(|v| matches!(
                v,
                crate::ops::health::UnsatisfiedConstraint::RequiresWhen { field, .. }
                    if field == "checked_by"
            ))),
        "checked-without-checker must be a health finding"
    );

    // Question 2 — the one-sided trade-off is flagged missing its
    // OPPOSES block (form 4 at warn), from health output alone.
    let missing = crate::ops::health::collect_missing_required_outgoing(
        engine.store(),
        None,
        engine.schemas(),
    );
    let onesided_report = missing
        .iter()
        .find(|r| r.id == onesided.id)
        .expect("one-sided trade-off flagged");
    assert_eq!(onesided_report.missing.len(), 1);
    assert_eq!(onesided_report.missing[0].relationships, vec!["OPPOSES"]);
}

const PLENUM_MANIFEST: &str = r#"name: plenum-proof
version: 0.1.0
description: plenum-shaped uniqueness and vocabulary proof schema
when_to_use: constraint-proof tests
types:
  - rede
  - vocabulary
relationships:
  mode: strict
  definitions:
    - name: REFERENCES
      description: soft ref
      default_weight: 0.5
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;

fn plenum_rede_type(unique_severity: &str) -> String {
    format!(
        r#"name: rede
description: one speech
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: rede_id
    description: source id
    field_type: string
  - key: rede_sha256
    description: content hash
    field_type: string
  - key: kategorie
    description: category from the shared vocabulary
    field_type: string
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
  - rede_id
  - rede_sha256
  - kategorie
health_required_fields:
  - body
staleness_threshold_days: 90
constraints:
  - kind: unique
    fields: [rede_id, rede_sha256]
    severity: {unique_severity}
  - kind: enum_from_neighbour
    field: kategorie
    rel_type: REFERENCES
    section: terms
write_rules: []
"#
    )
}

const PLENUM_VOCABULARY: &str = r#"name: vocabulary
description: the shared term list
when_to_use: tests
sections:
  - key: terms
    heading: Terms
    required: false
    search_weight: 5.0
    catch_all: false
    write_rules: []
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
  - terms
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;

/// The plenum proof, uniqueness half: a
/// second create with the same declared key tuple refuses with a
/// typed code naming the colliding entity — the 37-duplicates
/// scenario bounces at the engine. Health reports a pre-existing
/// violation planted under a warn-tier variant.
#[test]
fn plenum_proof_uniqueness_refuses_duplicates_and_health_reports_planted_ones() {
    // Block tier: the duplicate refuses, naming the collider.
    let tmp = TempDir::new().unwrap();
    let rede = plenum_rede_type("block");
    let mut engine = engine_with_proof_schema(
        &tmp,
        "plenum-proof",
        PLENUM_MANIFEST,
        &[("rede", &rede), ("vocabulary", PLENUM_VOCABULARY)],
    );
    let first = proof_create(
        &mut engine,
        "rede",
        "Speech One",
        &[("rede_id", "19-42"), ("rede_sha256", "abc123")],
        vec![],
    )
    .unwrap();
    let err = proof_create(
        &mut engine,
        "rede",
        "Speech One Duplicate",
        &[("rede_id", "19-42"), ("rede_sha256", "abc123")],
        vec![],
    )
    .unwrap_err();
    assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED");
    assert_eq!(
        err.details()["violations"][0]["colliding"],
        first.id.to_string(),
        "the refusal names the colliding entity"
    );
    // A different tuple passes.
    proof_create(
        &mut engine,
        "rede",
        "Speech Two",
        &[("rede_id", "19-43"), ("rede_sha256", "def456")],
        vec![],
    )
    .unwrap();

    // Warn tier: plant the duplicate, health reports it.
    let tmp = TempDir::new().unwrap();
    let rede = plenum_rede_type("warn");
    let mut engine = engine_with_proof_schema(
        &tmp,
        "plenum-proof",
        PLENUM_MANIFEST,
        &[("rede", &rede), ("vocabulary", PLENUM_VOCABULARY)],
    );
    proof_create(
        &mut engine,
        "rede",
        "Planted A",
        &[("rede_id", "19-42"), ("rede_sha256", "abc123")],
        vec![],
    )
    .unwrap();
    let planted = proof_create(
        &mut engine,
        "rede",
        "Planted B",
        &[("rede_id", "19-42"), ("rede_sha256", "abc123")],
        vec![],
    )
    .unwrap();
    assert!(
        planted
            .warnings
            .iter()
            .any(|w| matches!(w, WarningHint::ConstraintUnsatisfied { .. })),
        "warn tier surfaces the duplicate as a warning and commits"
    );
    let findings = crate::ops::health::collect_constraint_findings(
        engine.store(),
        None,
        engine.schemas(),
        None,
    );
    assert_eq!(
        findings.len(),
        2,
        "both sides of the planted duplicate are findings: {findings:?}"
    );
}

/// The plenum proof, enum-from-neighbour half: renaming a value in the neighbour's section makes
/// every stale holder a health finding.
#[test]
fn plenum_proof_enum_from_neighbour_flags_stale_holders_after_rename() {
    let tmp = TempDir::new().unwrap();
    let rede = plenum_rede_type("warn");
    let mut engine = engine_with_proof_schema(
        &tmp,
        "plenum-proof",
        PLENUM_MANIFEST,
        &[("rede", &rede), ("vocabulary", PLENUM_VOCABULARY)],
    );
    let (actor, client) = cli_actor();

    // The vocabulary entity enumerates the legal categories.
    let mut sections = IndexMap::new();
    sections.insert("body".to_string(), "the term list.".to_string());
    sections.insert("terms".to_string(), "- haushalt\n- verkehr\n".to_string());
    let vocab = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "proof".to_string(),
                title: "Kategorien".to_string(),
                entity_type: "vocabulary".to_string(),
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

    // A holder whose value is backed: clean.
    let holder = proof_create(
        &mut engine,
        "rede",
        "Holder",
        &[("kategorie", "haushalt")],
        vec![rel(&vocab.id.0, "REFERENCES")],
    )
    .unwrap();
    let findings = crate::ops::health::collect_constraint_findings(
        engine.store(),
        None,
        engine.schemas(),
        None,
    );
    assert!(
        findings.iter().all(|r| r.id != holder.id),
        "backed value produces no finding: {findings:?}"
    );

    // Rename the value in the neighbour's section — the holder
    // goes stale and health flags it.
    let current = engine.get_entity(&vocab.id).unwrap().content_hash.clone();
    let mut sections = IndexMap::new();
    sections.insert("terms".to_string(), "- finanzen\n- verkehr\n".to_string());
    engine
        .update_entity(
            crate::engine::UpdateEntityArgs {
                anchors: Vec::new(),
                id: vocab.id.clone(),
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
        .unwrap();
    let findings = crate::ops::health::collect_constraint_findings(
        engine.store(),
        None,
        engine.schemas(),
        None,
    );
    let stale = findings
        .iter()
        .find(|r| r.id == holder.id)
        .expect("stale holder is flagged after the rename");
    assert!(stale.violations.iter().any(|v| matches!(
        v,
        crate::ops::health::UnsatisfiedConstraint::EnumFromNeighbour { value, .. }
            if value == "haushalt"
    )));
}

/// The advertised mutation warning is real: a create leaving a
/// required-outgoing block unsatisfied returns
/// `MISSING_REQUIRED_OUTGOING` naming the block with cardinality —
/// and still commits. Complements: a create satisfying the block
/// via inline `relations` emits no such warning; the health sweep
/// reports exactly the same unsatisfied blocks (shared evaluation).
#[test]
fn create_warns_missing_required_outgoing_and_still_commits() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_required_outgoing_schema(&tmp);
    let (actor, client) = cli_actor();

    let outcome = engine
        .create_entity(
            task_create_args("Orphan Task", vec![]),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert!(
        !outcome.write_id.is_empty(),
        "the warning never blocks the mutation"
    );
    let blocks = missing_outgoing_of(&outcome.warnings);
    assert_eq!(
        blocks,
        vec![(vec!["PART_OF".to_string()], "at_least_one".to_string())],
        "warning names the unsatisfied block with cardinality; warnings = {:?}",
        outcome.warnings
    );

    // Health-path parity: the sweep reports the same entity with
    // the same block — the two surfaces share one evaluation.
    let reports = crate::ops::health::collect_missing_required_outgoing(
        engine.store(),
        None,
        engine.schemas(),
    );
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].id, outcome.id);
    assert_eq!(reports[0].missing.len(), 1);
    assert_eq!(reports[0].missing[0].relationships, vec!["PART_OF"]);
    assert_eq!(reports[0].missing[0].cardinality, "at_least_one");

    // Complement: a create whose inline relation satisfies the
    // block emits no MISSING_REQUIRED_OUTGOING.
    let satisfied = engine
        .create_entity(
            task_create_args(
                "Child Task",
                vec![crate::ops::RelateArg {
                    target: outcome.id.clone(),
                    rel_type: "PART_OF".to_string(),
                    description: None,
                }],
            ),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert!(
        missing_outgoing_of(&satisfied.warnings).is_empty(),
        "satisfied block emits no warning: {:?}",
        satisfied.warnings
    );
}

/// Update-side mirror: a section-only update on an entity with an
/// unsatisfied block warns; declaring the satisfying relation in
/// the same update clears it.
#[test]
fn update_warns_missing_required_outgoing_until_satisfied() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_required_outgoing_schema(&tmp);
    let (actor, client) = cli_actor();
    let a = engine
        .create_entity(
            task_create_args("Task A", vec![]),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let b = engine
        .create_entity(
            task_create_args("Task B", vec![]),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let update =
        |engine: &mut Engine, id: &crate::entity::EntityId, declare: Vec<crate::ops::RelateArg>| {
            let current = engine.get_entity(id).unwrap().content_hash.clone();
            let mut sections = IndexMap::new();
            sections.insert("body".to_string(), format!("edited at {:?}", declare.len()));
            engine
                .update_entity(
                    crate::engine::UpdateEntityArgs {
                        anchors: Vec::new(),
                        id: id.clone(),
                        expected_hash: Some(current),
                        sections,
                        append_sections: IndexMap::new(),
                        patch_sections: IndexMap::new(),
                        sections_unset: Vec::new(),
                        metadata: IndexMap::new(),
                        metadata_unset: Vec::new(),
                        declare_relations: declare,
                        dry_run: false,
                        relations_unset: Vec::new(),
                        anchors_unset: Vec::new(),
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap()
        };

    // Section-only update on an unsatisfied entity: warning fires,
    // mutation commits.
    let outcome = update(&mut engine, &a.id, vec![]);
    assert!(!outcome.write_id.is_empty());
    assert_eq!(
        missing_outgoing_of(&outcome.warnings),
        vec![(vec!["PART_OF".to_string()], "at_least_one".to_string())]
    );

    // Declaring the satisfying relation in the update clears it.
    let outcome = update(
        &mut engine,
        &a.id,
        vec![crate::ops::RelateArg {
            target: b.id.clone(),
            rel_type: "PART_OF".to_string(),
            description: None,
        }],
    );
    assert!(
        missing_outgoing_of(&outcome.warnings).is_empty(),
        "satisfied block emits no warning: {:?}",
        outcome.warnings
    );
}
