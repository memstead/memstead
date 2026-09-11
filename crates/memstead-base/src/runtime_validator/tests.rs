#![cfg(test)]

use super::*;

/// Tiny in-memory schema for the rel-shape shape-test fixture:
/// `EXECUTES: step → decision`, plus a shape-free `USES` and
/// `PART_OF`. Used by the rel-shape unit tests; `_default` is
/// preserved for parity with the loader's invariants.
fn shape_test_schema() -> std::sync::Arc<Schema> {
    let manifest_yaml = r#"name: tests-rel-shape
version: 0.1.0
description: rel-shape test schema
when_to_use: tests
types:
  - step
  - decision
  - note
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: parent containment
      default_weight: 3.0
      acyclic: true
    - name: USES
      description: shape-free reference
      default_weight: 1.0
    - name: EXECUTES
      description: step carries out decision
      default_weight: 2.5
      source_types: [step]
      target_types: [decision]
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let body_section = r#"sections:
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
"#;
    let make_type =
        |name: &str| format!("name: {name}\ndescription: t\nwhen_to_use: Here\n{body_section}");
    std::sync::Arc::new(
        memstead_schema::load_schema_from_memory(
            manifest_yaml,
            &[
                ("step".to_string(), make_type("step")),
                ("decision".to_string(), make_type("decision")),
                ("note".to_string(), make_type("note")),
            ],
        )
        .expect("test schema must load"),
    )
}

#[test]
fn reserved_metadata_gate_refuses_the_underscore_namespace_on_set() {
    // The `_` prefix is the computed read-channel namespace (`_hash`,
    // `_tokens`, ...); a stored key there would render as a second,
    // stale copy of a computed field. Refused as a namespace on every
    // set path; ordinary keys pass unchanged.
    for key in ["_hash", "_tokens", "_anything"] {
        assert!(matches!(
            validate_reserved_metadata_key(key),
            Err(ValidationError::ReadOnlyField { .. })
        ));
    }
    assert!(validate_reserved_metadata_key("level").is_ok());
    assert!(matches!(
        validate_reserved_metadata_key("type"),
        Err(ValidationError::ReadOnlyField { .. })
    ));
}

#[test]
fn rel_shape_admits_pair_in_declared_source_target() {
    let schema = shape_test_schema();
    // step → decision is the declared shape; admits cleanly.
    assert!(validate_rel_shape("EXECUTES", "step", Some("decision"), &schema).is_ok());
}

#[test]
fn rel_shape_rejects_violating_source() {
    let schema = shape_test_schema();
    // EXECUTES is shape-pinned to source=step; note → decision violates source.
    let err = validate_rel_shape("EXECUTES", "note", Some("decision"), &schema).unwrap_err();
    match err {
        ValidationError::InvalidRelationshipShape {
            rel_type,
            from_type,
            to_type,
            allowed_source_types,
            allowed_target_types,
            ..
        } => {
            assert_eq!(rel_type, "EXECUTES");
            assert_eq!(from_type, "note");
            assert_eq!(to_type, "decision");
            assert_eq!(allowed_source_types, vec!["step".to_string()]);
            assert_eq!(allowed_target_types, vec!["decision".to_string()]);
        }
        other => panic!("expected InvalidRelationshipShape, got {other:?}"),
    }
}

#[test]
fn rel_shape_rejects_violating_target() {
    let schema = shape_test_schema();
    // step → note violates target: EXECUTES requires target=decision.
    let err = validate_rel_shape("EXECUTES", "step", Some("note"), &schema).unwrap_err();
    assert!(matches!(
        err,
        ValidationError::InvalidRelationshipShape { .. }
    ));
}

#[test]
fn rel_shape_admits_shape_free_relationship() {
    let schema = shape_test_schema();
    // USES has empty source_types/target_types — admits anything.
    assert!(validate_rel_shape("USES", "note", Some("step"), &schema).is_ok());
}

#[test]
fn rel_shape_skips_target_check_when_target_type_unknown() {
    let schema = shape_test_schema();
    // Target stub has no resolved type — target-side check skipped.
    // Source still checked: step is the declared source, so this admits.
    assert!(validate_rel_shape("EXECUTES", "step", None, &schema).is_ok());
}

#[test]
fn rel_shape_no_op_for_unknown_rel_name() {
    let schema = shape_test_schema();
    // Defensive branch: callers run validate_rel_type first, but
    // an unknown name here returns Ok rather than panicking.
    assert!(validate_rel_shape("MADE_UP", "step", Some("decision"), &schema).is_ok());
}

// ---------------------------------------------------------------
// validate_cross_mem_edge — covers the pure-function layer. The
// engine relate path's routing wraps these outcomes into
// `CROSS_MEM_EDGE_NOT_DECLARED` / `INVALID_REL_TYPE` /
// `INVALID_REL_SHAPE` envelopes.
// ---------------------------------------------------------------

/// Cross-mem-aware source schema: declares one outbound entry
/// to the `other` domain with `ADDRESSES: step → requirement` and
/// a shape-free `MENTIONS`. Intra-mem `relationships` carries a
/// disjoint `IMPLEMENTS` rel-type so the "intra-mem-only is
/// invisible cross-mem" AC is exercisable.
fn cross_mem_source_schema() -> std::sync::Arc<Schema> {
    let manifest_yaml = r#"name: source-cv
version: 0.1.0
description: cross-mem source schema
when_to_use: tests
types:
  - step
  - decision
relationships:
  mode: strict
  definitions:
    - name: IMPLEMENTS
      description: intra-mem only
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
cross_mem_relationships:
  - to_schema: other
    definitions:
      - name: ADDRESSES
        description: outbound shape-pinned
        default_weight: 1.0
        source_types: [step]
        target_types: [requirement]
      - name: MENTIONS
        description: outbound shape-free
        default_weight: 0.5
community:
  resolution: 1.0
  seed: 42
"#;
    let body_section = r#"sections:
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
    let make_type =
        |name: &str| format!("name: {name}\ndescription: t\nwhen_to_use: Here\n{body_section}");
    std::sync::Arc::new(
        memstead_schema::load_schema_from_memory(
            manifest_yaml,
            &[
                ("step".to_string(), make_type("step")),
                ("decision".to_string(), make_type("decision")),
            ],
        )
        .expect("cross-mem source schema must load"),
    )
}

fn other_target_ref() -> memstead_schema::SchemaRef {
    memstead_schema::SchemaRef::new("other", semver::Version::new(1, 0, 0))
}

#[test]
fn cross_mem_admits_declared_shape() {
    let src = cross_mem_source_schema();
    let target = other_target_ref();
    match validate_cross_mem_edge("ADDRESSES", "step", Some("requirement"), &src, &target) {
        CrossMemRelCheck::Ok => {}
        other => panic!("expected Ok, got {other:?}"),
    }
}

#[test]
fn cross_mem_no_matching_entry_returns_edge_not_declared() {
    let src = cross_mem_source_schema();
    // Target schema not present in source schema's
    // cross_mem_relationships — source only declares the
    // `other` domain.
    let target = memstead_schema::SchemaRef::new("docs", semver::Version::new(0, 1, 0));
    match validate_cross_mem_edge("ADDRESSES", "step", Some("page"), &src, &target) {
        CrossMemRelCheck::EdgeNotDeclared => {}
        other => panic!("expected EdgeNotDeclared, got {other:?}"),
    }
}

#[test]
fn cross_mem_entry_matches_any_target_version() {
    // Eligibility is name-based: the `other` declaration is
    // satisfied by a target mem pinning *any* version of
    // `other` — a target-side version bump cannot invalidate it.
    let src = cross_mem_source_schema();
    for version in [
        semver::Version::new(1, 0, 0),
        semver::Version::new(1, 1, 0),
        semver::Version::new(2, 5, 0),
    ] {
        let target = memstead_schema::SchemaRef::new("other", version.clone());
        match validate_cross_mem_edge("ADDRESSES", "step", Some("requirement"), &src, &target) {
            CrossMemRelCheck::Ok => {}
            other => panic!("expected Ok against other@{version}, got {other:?}"),
        }
    }
}

#[test]
fn cross_mem_unknown_rel_type_returns_invalid_rel_type() {
    let src = cross_mem_source_schema();
    let target = other_target_ref();
    // `IMPLEMENTS` is declared intra-mem only — invisible to
    // the cross-mem entry and refused with INVALID_REL_TYPE.
    match validate_cross_mem_edge("IMPLEMENTS", "step", Some("requirement"), &src, &target) {
        CrossMemRelCheck::Invalid(ValidationError::InvalidRelationshipType {
            input,
            allowed,
            ..
        }) => {
            assert_eq!(input, "IMPLEMENTS");
            // Cross-mem entry's vocabulary surfaces: ADDRESSES + MENTIONS.
            let names: Vec<String> = allowed.into_iter().map(|h| h.name).collect();
            assert!(names.iter().any(|n| n == "ADDRESSES"));
            assert!(names.iter().any(|n| n == "MENTIONS"));
            // Intra-mem-only rel-type must not leak into the cross-mem list.
            assert!(!names.iter().any(|n| n == "IMPLEMENTS"));
        }
        other => panic!("expected Invalid(InvalidRelationshipType), got {other:?}"),
    }
}

#[test]
fn cross_mem_shape_mismatch_returns_invalid_rel_shape() {
    let src = cross_mem_source_schema();
    let target = other_target_ref();
    // ADDRESSES is shape-pinned to step → requirement. `decision`
    // is a declared source type in source-cv but not admitted by
    // this cross-mem entry; the shape check refuses with the
    // cross-mem entry's shape (not intra-mem's).
    match validate_cross_mem_edge("ADDRESSES", "decision", Some("requirement"), &src, &target) {
        CrossMemRelCheck::Invalid(ValidationError::InvalidRelationshipShape {
            rel_type,
            from_type,
            allowed_source_types,
            allowed_target_types,
            ..
        }) => {
            assert_eq!(rel_type, "ADDRESSES");
            assert_eq!(from_type, "decision");
            assert_eq!(allowed_source_types, vec!["step".to_string()]);
            assert_eq!(allowed_target_types, vec!["requirement".to_string()]);
        }
        other => panic!("expected Invalid(InvalidRelationshipShape), got {other:?}"),
    }
}

#[test]
fn cross_mem_shape_free_rel_type_admits_any_pair() {
    let src = cross_mem_source_schema();
    let target = other_target_ref();
    // MENTIONS has empty source_types/target_types — admits any pair.
    assert!(matches!(
        validate_cross_mem_edge("MENTIONS", "decision", Some("page"), &src, &target),
        CrossMemRelCheck::Ok
    ));
}

/// A schema with a `to_schema: "*"` entry (loader-bound to
/// its alias rel-type) plus an exact per-schema entry for
/// structural edges.
fn wildcard_source_schema() -> std::sync::Arc<Schema> {
    let manifest_yaml = r#"name: source-wc
version: 0.1.0
description: wildcard cross-mem source schema
when_to_use: tests
types:
  - step
  - decision
relationships:
  mode: strict
  definitions:
    - name: SOFT_REF
      description: alias-emitted soft reference
      default_weight: 0.5
    - name: ADDRESSES
      description: structural
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
alias_target_rel_type: SOFT_REF
cross_mem_relationships:
  - to_schema: other
    definitions:
      - name: ADDRESSES
        description: structural, per-schema
        default_weight: 1.0
        source_types: [step]
        target_types: [requirement]
  - to_schema: "*"
    definitions:
      - name: SOFT_REF
        description: soft reference anywhere
        default_weight: 0.5
        source_types: [step]
community:
  resolution: 1.0
  seed: 42
"#;
    let body_section = r#"description: t
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
    let types = vec![
        ("step".to_string(), format!("name: step\n{body_section}")),
        (
            "decision".to_string(),
            format!("name: decision\n{body_section}"),
        ),
    ];
    std::sync::Arc::new(
        memstead_schema::load_schema_from_memory(manifest_yaml, &types)
            .expect("wildcard schema loads"),
    )
}

/// The wildcard admits the alias rel-type into ANY destination
/// schema — including one carrying its own exact structural entry
/// (coexistence: the exact entry must not shadow the wildcard).
#[test]
fn cross_mem_wildcard_admits_alias_edge_to_any_schema() {
    let src = wildcard_source_schema();
    // Arbitrary user-written destination schema, arbitrary type.
    let user = memstead_schema::SchemaRef::new("debate", semver::Version::new(0, 1, 0));
    assert!(matches!(
        validate_cross_mem_edge("SOFT_REF", "step", Some("argument"), &src, &user),
        CrossMemRelCheck::Ok
    ));
    // Destination with an exact structural entry: BOTH work.
    let other = memstead_schema::SchemaRef::new("other", semver::Version::new(1, 0, 0));
    assert!(matches!(
        validate_cross_mem_edge("SOFT_REF", "step", Some("requirement"), &src, &other),
        CrossMemRelCheck::Ok
    ));
    assert!(matches!(
        validate_cross_mem_edge("ADDRESSES", "step", Some("requirement"), &src, &other),
        CrossMemRelCheck::Ok
    ));
}

/// Refusal complements around the wildcard: the source-type list on
/// the wildcard declaration still gates; a structural rel-type into
/// a destination with no per-schema declaration is still the
/// historical `CROSS_MEM_EDGE_NOT_DECLARED` refusal.
#[test]
fn cross_mem_wildcard_keeps_source_type_gate_and_structural_refusal() {
    let src = wildcard_source_schema();
    let user = memstead_schema::SchemaRef::new("debate", semver::Version::new(0, 1, 0));
    // `decision` is not in the wildcard declaration's source_types.
    match validate_cross_mem_edge("SOFT_REF", "decision", Some("argument"), &src, &user) {
        CrossMemRelCheck::Invalid(ValidationError::InvalidRelationshipShape {
            from_type,
            allowed_source_types,
            ..
        }) => {
            assert_eq!(from_type, "decision");
            assert_eq!(allowed_source_types, vec!["step".to_string()]);
        }
        other => panic!("expected shape refusal on source-type gate, got {other:?}"),
    }
    // Structural rel-type into an undeclared destination: the
    // wildcard (alias-only) does not admit it — same refusal as
    // before the wildcard existed.
    assert!(matches!(
        validate_cross_mem_edge("ADDRESSES", "step", Some("argument"), &src, &user),
        CrossMemRelCheck::EdgeNotDeclared
    ));
}

// --- prose_render -----------------------------------------------
// The text channel inlines every recovery field instead of pointing
// at the structured channel. These tests pin that contract.

#[test]
fn prose_render_unknown_section_inlines_all_declared_and_suggestion() {
    let err = ValidationError::UnknownSection {
        key: "implimentation".to_string(),
        entity_type: "spec".to_string(),
        declared: (0..8).map(|i| format!("sec{i}")).collect(),
        suggestion: Some("sec0".to_string()),
    };
    let prose = err.prose_render();
    for d in (0..8).map(|i| format!("sec{i}")) {
        assert!(prose.contains(&d), "missing {d} in: {prose}");
    }
    assert!(prose.contains("Did you mean 'sec0'?"), "got: {prose}");
    assert!(!prose.contains("see details"), "got: {prose}");
}

#[test]
fn prose_render_invalid_enum_value_inlines_field_description_and_rules() {
    let err = ValidationError::InvalidEnumValue {
        field: "level".to_string(),
        value: "M7".to_string(),
        allowed: (0..7).map(|i| format!("M{i}")).collect(),
        field_description: Some("maturity rung (M0=draft … M6=stable)".to_string()),
        suggestion: Some("M6".to_string()),
        type_write_rules: vec!["specs land at M0 unless promoted by a decision".to_string()],
        entity_type: "spec".to_string(),
    };
    let prose = err.prose_render();
    assert!(prose.contains("M0"), "got: {prose}");
    assert!(prose.contains("M6"), "got: {prose}");
    assert!(
        prose.contains("maturity rung"),
        "field_description missing: {prose}"
    );
    assert!(prose.contains("Did you mean 'M6'?"), "got: {prose}");
    assert!(
        prose.contains("specs land at M0"),
        "type_write_rules missing: {prose}"
    );
    assert!(!prose.contains("see details"), "got: {prose}");
}

#[test]
fn prose_render_invalid_rel_shape_renders_any_when_unconstrained() {
    let err = ValidationError::InvalidRelationshipShape {
        rel_type: "OWNS".to_string(),
        from_type: "spec".to_string(),
        to_type: "spec".to_string(),
        allowed_source_types: vec!["actor".to_string()],
        allowed_target_types: vec![],
        suggestion: None,
    };
    let prose = err.prose_render();
    // The shape-free target axis renders as `any` (no brackets,
    // matching the existing convention pinned by
    // `relate_shape_violation_surfaces_typed_envelope`).
    assert!(prose.contains("allowed sources: actor"), "got: {prose}");
    assert!(prose.contains("allowed targets: any"), "got: {prose}");
    assert!(!prose.contains("see details"), "got: {prose}");
}

// ---------------------------------------------------------------
// parse_metadata_value typed-value validation. A Date / Number
// field's value is validated against its declared type at the write
// boundary, so a malformed value cannot land (and cannot corrupt
// range filters).
// ---------------------------------------------------------------

/// In-memory schema with one type carrying a `Date` field
/// (`verified_on`), a `Number` field (`order`), and a free-form
/// `String` field (`note`) — the three arms the value check
/// distinguishes.
fn typed_field_type() -> std::sync::Arc<TypeDefinition> {
    let manifest_yaml = r#"name: tests-typed-fields
version: 0.1.0
description: typed-field test schema
when_to_use: tests
types:
  - widget
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let type_yaml = r#"name: widget
description: t
when_to_use: Here
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: verified_on
    description: ISO YYYY-MM-DD date the widget was verified
    field_type: date
    optional: true
  - key: order
    description: numeric ordering within a plan
    field_type: number
    optional: true
  - key: note
    description: free-form note
    field_type: string
    optional: true
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
    let schema = memstead_schema::load_schema_from_memory(
        manifest_yaml,
        &[("widget".to_string(), type_yaml.to_string())],
    )
    .expect("typed-field test schema must load");
    schema.get_type("widget").expect("widget type present")
}

#[test]
fn date_field_rejects_non_date_value() {
    let ty = typed_field_type();
    let err = parse_metadata_value("verified_on", "not-a-real-date", &ty).unwrap_err();
    assert_eq!(err.code(), "INVALID_FIELD_VALUE");
    match err {
        ValidationError::InvalidFieldValue {
            field,
            value,
            expected_type,
            entity_type,
            ..
        } => {
            assert_eq!(field, "verified_on");
            assert_eq!(value, "not-a-real-date");
            assert_eq!(expected_type, "Date");
            assert_eq!(entity_type, "widget");
        }
        other => panic!("expected InvalidFieldValue, got {other:?}"),
    }
}

#[test]
fn date_field_rejects_empty_string() {
    let ty = typed_field_type();
    let err = parse_metadata_value("verified_on", "", &ty).unwrap_err();
    assert!(matches!(err, ValidationError::InvalidFieldValue { .. }));
}

#[test]
fn date_field_accepts_iso_date_and_datetime() {
    let ty = typed_field_type();
    match parse_metadata_value("verified_on", "2024-06-01", &ty).unwrap() {
        MetadataValue::String(s) => assert_eq!(s, "2024-06-01"),
        other => panic!("expected String, got {other:?}"),
    }
    // ISO-8601 datetime form is also accepted.
    assert!(parse_metadata_value("verified_on", "2024-06-01T12:30:00Z", &ty).is_ok());
}

#[test]
fn number_field_rejects_non_numeric_value() {
    let ty = typed_field_type();
    let err = parse_metadata_value("order", "soon", &ty).unwrap_err();
    match err {
        ValidationError::InvalidFieldValue {
            field,
            expected_type,
            ..
        } => {
            assert_eq!(field, "order");
            assert_eq!(expected_type, "Number");
        }
        other => panic!("expected InvalidFieldValue, got {other:?}"),
    }
}

#[test]
fn number_field_accepts_integer_and_float() {
    let ty = typed_field_type();
    assert!(matches!(
        parse_metadata_value("order", "3", &ty).unwrap(),
        MetadataValue::Integer(3)
    ));
    assert!(matches!(
        parse_metadata_value("order", "2.5", &ty).unwrap(),
        MetadataValue::Float(_)
    ));
}

#[test]
fn string_field_accepts_any_value() {
    let ty = typed_field_type();
    // The free-form String arm is untouched — arbitrary text lands.
    assert!(parse_metadata_value("note", "not-a-real-date", &ty).is_ok());
}

#[test]
fn invalid_field_value_prose_inlines_format_and_purpose() {
    let err = ValidationError::InvalidFieldValue {
        field: "verified_on".to_string(),
        value: "not-a-real-date".to_string(),
        expected_type: "Date".to_string(),
        expected_format: Some("YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ".to_string()),
        field_description: Some("date the widget was verified".to_string()),
        entity_type: "widget".to_string(),
    };
    let prose = err.prose_render();
    assert!(prose.contains("not-a-real-date"), "got: {prose}");
    assert!(prose.contains("YYYY-MM-DD"), "format missing: {prose}");
    assert!(
        prose.contains("date the widget was verified"),
        "purpose missing: {prose}"
    );
    assert!(!prose.contains("see details"), "got: {prose}");
}

#[test]
fn is_date_shaped_matches_strict_validator_contract() {
    assert!(is_date_shaped("2024-06-01"));
    assert!(is_date_shaped("2024-06-01T12:30:00Z"));
    assert!(!is_date_shaped(""));
    assert!(!is_date_shaped("not-a-real-date"));
    assert!(!is_date_shaped("2024-6-1"));
    assert!(!is_date_shaped("2024-06-01 extra"));
}

#[test]
fn section_content_refuses_nul_byte() {
    let err = validate_section_content([("body", "line1\u{0}line2")].into_iter(), None)
        .expect_err("NUL in a section body must be refused");
    assert_eq!(err.code(), "SECTION_CONTENT_INVALID");
    match &err {
        ValidationError::SectionContentControlByte {
            section,
            control_char,
            codepoint,
            byte_offset,
        } => {
            assert_eq!(section, "body");
            assert_eq!(*control_char, '\u{0}');
            assert_eq!(*codepoint, 0);
            // "line1" is 5 bytes — the NUL sits at offset 5.
            assert_eq!(*byte_offset, 5);
        }
        other => panic!("expected SectionContentControlByte, got {other:?}"),
    }
    // Recovery payload names the offending char + offset.
    let details = err.details();
    assert_eq!(details["codepoint"], 0);
    assert_eq!(details["byte_offset"], 5);
    assert_eq!(details["section"], "body");
}

#[test]
fn section_content_refuses_other_c0_controls_and_cr() {
    // Bell, vertical tab, form feed, carriage return — all C0
    // controls outside the tab/newline allow-list.
    for bad in ['\u{7}', '\u{b}', '\u{c}', '\r'] {
        let body = format!("ok{bad}more");
        let err = validate_section_content([("s", body.as_str())].into_iter(), None)
            .expect_err("control char must be refused");
        assert_eq!(err.code(), "SECTION_CONTENT_INVALID", "char {:?}", bad);
    }
}

#[test]
fn section_content_allows_tab_and_newline() {
    // The two legitimate whitespace controls round-trip; multi-line
    // and tabbed bodies are unaffected.
    validate_section_content(
        [("body", "line1\nline2\n\tindented\tcols\n")].into_iter(),
        None,
    )
    .expect("tab and newline must stay legal in section bodies");
}

/// Criterion 6 (consistency-sweep 04/01): the engine must accept back a
/// value it emitted. The catch-all re-emits absorbed content under its
/// original heading line, so an agent that read an entity and wrote that
/// section back in replace mode was refused its own value.
#[test]
fn the_catch_all_accepts_back_the_value_the_engine_emits() {
    let declared = ["Body", "Notes"];
    let ctx = CatchAllContext {
        key: "notes",
        entity_type: "doc",
        declared_headings: &declared,
    };
    // What the engine hands out: the absorbed heading, verbatim.
    validate_section_content(
        [("notes", "## Field Notes\n\nsomething useful\n")].into_iter(),
        Some(ctx),
    )
    .expect("the catch-all re-absorbs an undeclared heading, so writing it back is safe");
}

/// The refusal complement, and the reason the exemption is exact rather
/// than a loosening: a DECLARED heading inside the catch-all really does
/// fork the entity, because the reparse moves that content to the declared
/// key. It stays refused, and so does every other section.
#[test]
fn the_catch_all_exemption_does_not_weaken_the_guard() {
    let declared = ["Body", "Notes"];
    let ctx = CatchAllContext {
        key: "notes",
        entity_type: "doc",
        declared_headings: &declared,
    };
    let err = validate_section_content(
        [("notes", "## Body\n\nthis would move to `body` on reparse\n")].into_iter(),
        Some(ctx),
    )
    .expect_err("a declared heading inside the catch-all forks the entity");
    assert_eq!(err.code(), "SECTION_CONTENT_INVALID");

    // A non-catch-all section is untouched by the exemption.
    let err = validate_section_content([("body", "## Anything\n")].into_iter(), Some(ctx))
        .expect_err("only the catch-all absorbs; every other section still forks");
    assert_eq!(err.code(), "SECTION_CONTENT_INVALID");

    // And an h1 is refused inside the catch-all too: it is the entity's
    // own title level, not something the catch-all absorbs.
    let err = validate_section_content([("notes", "# A Title\n")].into_iter(), Some(ctx))
        .expect_err("h1 is the entity's title level");
    assert_eq!(err.code(), "SECTION_CONTENT_INVALID");
}

/// Criterion 7 (consistency-sweep 04/01): caller-supplied content carrying
/// an undeclared heading with NO body refuses, naming what was rejected.
/// This is the exact complement of the criterion-6 exemption: the catch-all
/// keeps absorbed content but SKIPS empty content, so accepting this write
/// would drop the heading and tell the caller nothing.
#[test]
fn content_that_would_hide_a_delimiter_is_refused() {
    // Criterion 1. An open fence's range runs to end of text, so every
    // heading the generator writes after this section would be masked and
    // absorbed into it.
    let err = validate_section_content([("body", "```rust\nfn main() {}")].into_iter(), None)
        .expect_err("an unterminated fence must be refused");
    assert_eq!(err.code(), "UNTERMINATED_FENCE");
    assert_eq!(err.details()["section"], "body");
    assert_eq!(err.details()["fence"], "```");
    // Tilde fences and longer runs report the closer that actually works.
    let err = validate_section_content([("body", "~~~\nopen")].into_iter(), None)
        .expect_err("tilde fences too");
    assert_eq!(err.details()["fence"], "~~~");
    let err = validate_section_content([("body", "````\n```\nstill inside")].into_iter(), None)
        .expect_err("a longer opener needs a longer closer");
    assert_eq!(err.details()["fence"], "````");
}

#[test]
fn a_closed_fence_around_headings_is_admitted_unchanged() {
    // Criterion 2, both halves. A guard that refuses every fenced `##`
    // fails, and so does one that refuses content whose fence closes
    // later in the same body.
    validate_section_content(
        [(
            "body",
            "prose\n\n```md\n## Not A Section\n# Nor This\n```\n\nmore prose",
        )]
        .into_iter(),
        None,
    )
    .expect("a closed fence containing heading lines is ordinary content");
    // The closer arriving late in the body is still a closer.
    validate_section_content([("body", "```\n## Hidden\n```")].into_iter(), None)
        .expect("closed is closed, wherever the closer sits");
    // A fence inside a container the next column-0 line closes implicitly.
    validate_section_content([("body", "> ```\n> quoted")].into_iter(), None)
        .expect("a blockquote's fence cannot reach past the quote");
}

#[test]
fn an_ordinary_body_is_untouched_by_the_fence_guard() {
    // Criterion 7 at this tier: no fence characters, no new refusal.
    validate_section_content(
        [("body", "plain prose\nwith lines\n\nand a paragraph")].into_iter(),
        None,
    )
    .expect("content with no fence at all cannot trip a fence guard");
}

#[test]
fn an_empty_undeclared_heading_is_refused_at_the_write() {
    let declared = ["Body", "Notes"];
    let ctx = CatchAllContext {
        key: "notes",
        entity_type: "doc",
        declared_headings: &declared,
    };
    for (body, label) in [
        ("## Scratch\n", "bare heading, nothing after it"),
        (
            "## Scratch\n\n   \n",
            "heading followed only by blank lines",
        ),
        (
            "## Scratch\n\n## Other\n\nreal content\n",
            "heading with the next heading under it",
        ),
    ] {
        let err =
            validate_section_content([("notes", body)].into_iter(), Some(ctx)).expect_err(label);
        assert_eq!(err.code(), "EMPTY_UNDECLARED_HEADING", "{label}");
        let d = err.details();
        assert_eq!(d["heading"], "Scratch", "{label}");
        assert_eq!(d["entity_type"], "doc", "{label}");
    }
}

/// The refusal complement, which is what keeps it from
/// stranding every read-modify-write: a heading WITH a body is accepted,
/// because it survives.
#[test]
fn an_undeclared_heading_with_a_body_is_not_refused() {
    let declared = ["Body", "Notes"];
    let ctx = CatchAllContext {
        key: "notes",
        entity_type: "doc",
        declared_headings: &declared,
    };
    validate_section_content(
        [("notes", "## Scratch\n\nsomething\n")].into_iter(),
        Some(ctx),
    )
    .expect("content under the heading survives, so the write is accepted");
}

#[test]
fn section_content_keeps_backslashes_verbatim() {
    // The fix screens a byte class — it must not interpret or
    // de-escape content. Literal backslashes (incl. ones that look
    // like escapes) pass through untouched.
    validate_section_content(
        [("body", r"a literal \n and \t and \0 and \\ backslash")].into_iter(),
        None,
    )
    .expect("backslashes are literal content, not control bytes");
}

#[test]
fn section_content_still_refuses_heading_injection() {
    // The pre-existing heading-injection guard is unchanged and
    // shares the wire code.
    let err = validate_section_content([("body", "intro\n## Injected\ntail")].into_iter(), None)
        .expect_err("embedded `## ` heading must still be refused");
    assert_eq!(err.code(), "SECTION_CONTENT_INVALID");
    assert!(matches!(err, ValidationError::SectionContentInvalid { .. }));
}

/// The write guard classifies code blocks the way the splitter
/// does: a `## ` line inside a code block splits nothing on
/// reparse, so refusing it was the write path disagreeing with the
/// read path about what a code block is.
#[test]
fn section_content_admits_a_heading_inside_a_code_block() {
    for body in [
        "intro\n\n```\n## Not A Heading\n```\n",
        "intro\n\n~~~\n## Not A Heading\n~~~\n",
        "intro\n\n> ```\n> ## Not A Heading\n> ```\n",
        "intro\n\n    ## Not A Heading\n",
    ] {
        validate_section_content([("body", body)].into_iter(), None)
            .unwrap_or_else(|e| panic!("code-block content must be admitted: {body:?} -> {e}"));
    }
}

/// The trim-fork class: the splitter stores the *trimmed* body, so
/// an indented code block that opens a section loses its indent on
/// write-back and its `## ` line lands at column 0 on the next
/// parse. The guard sees what the reparse will see.
#[test]
fn section_content_refuses_the_trim_fork() {
    let err = validate_section_content(
        [("body", "    ## Not A Heading\n    more\n")].into_iter(),
        None,
    )
    .expect_err("content whose trim exposes a column-0 heading must be refused");
    assert_eq!(err.code(), "SECTION_CONTENT_INVALID");
    match err {
        ValidationError::SectionContentInvalid {
            embedded_heading, ..
        } => assert_eq!(
            embedded_heading, "## Not A Heading",
            "the refusal quotes the line the reparse will see"
        ),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn section_content_refuses_the_trim_fork_for_h1_too() {
    let err = validate_section_content([("body", "  # Not A Title\n")].into_iter(), None)
        .expect_err("h1 exposed by the trim must be refused");
    assert_eq!(err.code(), "SECTION_CONTENT_INVALID");
}
