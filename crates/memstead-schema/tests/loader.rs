//! Schema loader tests.
//!
//! Covers structural + semantic validation via `load_schema_from_memory`:
//! - manifest parsing (incl. `deny_unknown_fields`)
//! - semver / name validation
//! - type-file / name consistency
//! - relationship vocabulary rules
//! - per-type validation (catch_all, references, enum defaults)
//! - edge_weight resolution at load time
//! - actionable error messages with closest-match suggestions

use memstead_schema::loader::{SchemaLoadError, load_schema_from_memory};
use memstead_schema::manifest::{Cardinality, RelationshipMode};

fn minimal_manifest() -> String {
    r#"name: example
version: 1.0.0
description: Example schema for tests
when_to_use: In loader tests only
types:
  - sample
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: Hierarchical containment
      default_weight: 3.0
    - name: REFERENCES
      description: Soft reference
      default_weight: 0.5
    - name: _default
      description: Fallback weight for unknown relationships
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#
    .to_string()
}

fn minimal_type() -> String {
    r#"name: sample
description: Sample type for tests
when_to_use: Whenever a minimal type is needed
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules:
      - One sentence describing the body.
metadata_fields:
  - key: status
    description: Lifecycle state
    field_type: string
    default_value: active
    enum_values:
      - active
      - closed
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
propagating_relationships: []
updatable_fields:
  - title
  - body
  - status
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules:
  - Keep it short.
"#
    .to_string()
}

fn load(
    manifest: &str,
    types: &[(&str, &str)],
) -> Result<memstead_schema::schema::Schema, SchemaLoadError> {
    let owned: Vec<(String, String)> = types
        .iter()
        .map(|(n, c)| ((*n).to_string(), (*c).to_string()))
        .collect();
    load_schema_from_memory(manifest, &owned)
}

// ---------------------------------------------------------------------------
// Manifest parsing
// ---------------------------------------------------------------------------

#[test]
fn manifest_parses_minimal() {
    let schema = load(&minimal_manifest(), &[("sample", &minimal_type())]).expect("load ok");
    assert_eq!(schema.manifest.name, "example");
    assert_eq!(schema.version, semver::Version::new(1, 0, 0));
    assert_eq!(schema.mode(), RelationshipMode::Strict);
    assert_eq!(schema.types.len(), 1);
    assert!(schema.get_type("sample").is_some());
}

#[test]
fn manifest_rejects_unknown_field() {
    let mut manifest = minimal_manifest();
    manifest.push_str("\nstray_field: boom\n");
    let err = load(&manifest, &[("sample", &minimal_type())]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::ParseManifest { .. }),
        "got: {err}"
    );
}

#[test]
fn manifest_rejects_invalid_semver() {
    let manifest = minimal_manifest().replace("version: 1.0.0", "version: one-point-oh");
    let err = load(&manifest, &[("sample", &minimal_type())]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidVersion { .. }),
        "got: {err}"
    );
}

#[test]
fn manifest_rejects_invalid_name_uppercase() {
    let manifest = minimal_manifest().replace("name: example", "name: Example");
    let err = load(&manifest, &[("sample", &minimal_type())]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidName { .. }),
        "got: {err}"
    );
}

#[test]
fn manifest_rejects_invalid_name_spaces() {
    let manifest = minimal_manifest().replace("name: example", "name: \"has spaces\"");
    let err = load(&manifest, &[("sample", &minimal_type())]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidName { .. }),
        "got: {err}"
    );
}

#[test]
fn manifest_rejects_invalid_name_empty() {
    let manifest = minimal_manifest().replace("name: example", "name: \"\"");
    let err = load(&manifest, &[("sample", &minimal_type())]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidName { .. }),
        "got: {err}"
    );
}

#[test]
fn manifest_rejects_invalid_name_starts_with_digit() {
    let manifest = minimal_manifest().replace("name: example", "name: 1schema");
    let err = load(&manifest, &[("sample", &minimal_type())]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidName { .. }),
        "got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Type file / name consistency
// ---------------------------------------------------------------------------

#[test]
fn type_file_name_must_match_declaration() {
    let t = minimal_type().replace("name: sample", "name: other");
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::TypeNameMismatch { ref file, ref declared, .. } if file == "sample" && declared == "other"),
        "got: {err}"
    );
}

#[test]
fn type_file_count_must_match_types_list_extras() {
    let err = load(
        &minimal_manifest(),
        &[("sample", &minimal_type()), ("extra", &minimal_type())],
    )
    .expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::TypeFileMismatch { .. }),
        "got: {err}"
    );
}

#[test]
fn type_file_count_must_match_types_list_missing() {
    let manifest = minimal_manifest().replace("  - sample\n", "  - sample\n  - extra\n");
    let err = load(&manifest, &[("sample", &minimal_type())]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::TypeFileMismatch { .. }),
        "got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Relationship vocabulary
// ---------------------------------------------------------------------------

#[test]
fn strict_mode_declared_relationship_accepted() {
    let schema = load(&minimal_manifest(), &[("sample", &minimal_type())]).expect("load ok");
    assert!(schema.relationship_known("PART_OF"));
    assert!(schema.relationship_known("REFERENCES"));
    assert!(schema.relationship_known("_default"));
    assert!(!schema.relationship_known("UNKNOWN"));
}

#[test]
fn default_weight_required() {
    let manifest = minimal_manifest().replace(
        "    - name: _default\n      description: Fallback weight for unknown relationships\n      default_weight: 1.0\n",
        "",
    );
    let err = load(&manifest, &[("sample", &minimal_type())]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::MissingDefaultWeight),
        "got: {err}"
    );
}

#[test]
fn acyclic_defaults_to_false_when_absent() {
    let schema = load(&minimal_manifest(), &[("sample", &minimal_type())]).expect("load ok");
    for def in &schema.manifest.relationships.definitions {
        assert!(!def.acyclic, "{} must default to acyclic=false", def.name);
    }
}

#[test]
fn acyclic_parses_when_true() {
    let manifest = minimal_manifest().replace(
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n",
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n      acyclic: true\n",
    );
    let schema = load(&manifest, &[("sample", &minimal_type())]).expect("load ok");
    let part_of = schema
        .manifest
        .relationships
        .definitions
        .iter()
        .find(|d| d.name == "PART_OF")
        .expect("PART_OF present");
    assert!(part_of.acyclic);
    let refs = schema
        .manifest
        .relationships
        .definitions
        .iter()
        .find(|d| d.name == "REFERENCES")
        .expect("REFERENCES present");
    assert!(!refs.acyclic, "untouched sibling stays permissive");
}

#[test]
fn acyclic_rejects_non_boolean() {
    let manifest = minimal_manifest().replace(
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n",
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n      acyclic: maybe\n",
    );
    let err = load(&manifest, &[("sample", &minimal_type())]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::ParseManifest { .. }),
        "got: {err}"
    );
}

#[test]
fn duplicate_relationship_rejected() {
    let manifest = minimal_manifest().replace(
        "    - name: _default",
        "    - name: PART_OF\n      description: duplicate\n      default_weight: 1.0\n    - name: _default",
    );
    let err = load(&manifest, &[("sample", &minimal_type())]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::DuplicateRelationship { ref name } if name == "PART_OF"),
        "got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Per-type validation
// ---------------------------------------------------------------------------

#[test]
fn catch_all_exactly_one_zero_fails() {
    let t = minimal_type().replace("catch_all: true", "catch_all: false");
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::CatchAllViolation { count: 0, .. }),
        "got: {err}"
    );
}

#[test]
fn catch_all_exactly_one_two_fails() {
    let t = minimal_type().replace(
        "sections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules:\n      - One sentence describing the body.\n",
        "sections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\n  - key: notes\n    heading: Notes\n    required: false\n    search_weight: 1.0\n    catch_all: true\n    write_rules: []\n",
    );
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::CatchAllViolation { count: 2, .. }),
        "got: {err}"
    );
}

#[test]
fn edge_weight_override_validates_against_declared_relationships() {
    let t = minimal_type().replace(
        "propagating_relationships: []",
        "propagating_relationships: []\nedge_weight_overrides:\n  NOT_DECLARED: 2.0",
    );
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::UndeclaredRelationship { field, .. } if field == "edge_weight_overrides"),
        "got: {err}"
    );
}

#[test]
fn default_value_must_be_in_enum_values() {
    let t = minimal_type().replace("default_value: active", "default_value: bogus");
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::DefaultValueNotInEnum { ref field, ref default, .. } if field == "status" && default == "bogus"),
        "got: {err}"
    );
}

#[test]
fn text_field_must_reference_section() {
    let t = minimal_type().replace(
        "text_fields:\n  - body",
        "text_fields:\n  - body\n  - status", // status is metadata, not section
    );
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::UnknownFieldReference { field, ref reference, .. } if field == "text_fields" && reference == "status"),
        "got: {err}"
    );
}

#[test]
fn updatable_field_title_accepted() {
    // `title` is the entity's virtual name — always updatable.
    let schema = load(&minimal_manifest(), &[("sample", &minimal_type())]).expect("load ok");
    let td = schema.get_type("sample").unwrap();
    assert!(td.updatable_fields.iter().any(|f| f == "title"));
}

// ---------------------------------------------------------------------------
// Edge weight resolution
// ---------------------------------------------------------------------------

#[test]
fn edge_weights_resolved_at_load_without_overrides() {
    let schema = load(&minimal_manifest(), &[("sample", &minimal_type())]).expect("load ok");
    let td = schema.get_type("sample").unwrap();
    assert_eq!(td.edge_weight("PART_OF"), 3.0);
    assert_eq!(td.edge_weight("REFERENCES"), 0.5);
    assert_eq!(td.edge_weight("_default"), 1.0);
}

#[test]
fn edge_weights_resolved_at_load_with_overrides() {
    let t = minimal_type().replace(
        "propagating_relationships: []",
        "propagating_relationships: []\nedge_weight_overrides:\n  PART_OF: 9.0",
    );
    let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("load ok");
    let td = schema.get_type("sample").unwrap();
    assert_eq!(td.edge_weight("PART_OF"), 9.0, "override wins");
    assert_eq!(td.edge_weight("REFERENCES"), 0.5, "default preserved");
}

#[test]
fn edge_weight_falls_back_to_default_for_unknown_rel() {
    let schema = load(&minimal_manifest(), &[("sample", &minimal_type())]).expect("load ok");
    let td = schema.get_type("sample").unwrap();
    assert_eq!(td.edge_weight("TOTALLY_UNKNOWN"), 1.0);
}

// ---------------------------------------------------------------------------
// Community config
// ---------------------------------------------------------------------------

#[test]
fn community_config_parsed_from_manifest() {
    let schema = load(&minimal_manifest(), &[("sample", &minimal_type())]).expect("load ok");
    assert_eq!(schema.manifest.community.resolution, 1.0);
    assert_eq!(schema.manifest.community.seed, 42);
}

// ---------------------------------------------------------------------------
// Error messages
// ---------------------------------------------------------------------------

#[test]
fn error_message_includes_closest_match() {
    let t = minimal_type().replace(
        "hierarchy_relationship: PART_OF",
        "hierarchy_relationship: PART_OFF",
    );
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("PART_OFF"),
        "message must mention the offender: {msg}"
    );
    assert!(
        msg.contains("Did you mean 'PART_OF'"),
        "expected closest-match hint, got: {msg}"
    );
    assert!(
        msg.contains("Available:") && msg.contains("PART_OF"),
        "expected available-list in message, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Builtin default bundle
// ---------------------------------------------------------------------------

#[test]
fn builtin_default_loads_ten_types() {
    let s = memstead_schema::schema::Schema::builtin_default();
    assert_eq!(s.manifest.name, "default");
    assert_eq!(s.version, semver::Version::new(1, 0, 0));
    assert!(s.relationship_known("_default"));
    assert!(s.relationship_known("PART_OF"));
    assert_eq!(s.types.len(), 10);
    for name in memstead_schema::builtin_names::ALL {
        assert!(s.get_type(name).is_some(), "missing builtin type: {name}");
    }
}

// ---------------------------------------------------------------------------
// Example schema (authoring-tutorial reference)
// ---------------------------------------------------------------------------

/// The minimal example schema under `examples/minimal/` is linked from
/// the authoring tutorial. If it ever stops loading, the docs are lying.
#[test]
fn example_minimal_schema_loads() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/minimal");
    let schema =
        memstead_schema::load_schema_from_dir(&path).expect("minimal example schema loads");
    assert_eq!(schema.manifest.name, "recipe");
    assert_eq!(schema.version, semver::Version::new(0, 1, 0));
    assert!(schema.get_type("recipe").is_some());
    assert!(schema.get_type("ingredient").is_some());
    assert!(schema.relationship_known("CONTAINS"));
    assert!(schema.relationship_known("SUBSTITUTES_FOR"));
    assert_eq!(schema.mode(), RelationshipMode::Strict);
}

// ---------------------------------------------------------------------------
// Base metadata merge (implicit metadata)
// ---------------------------------------------------------------------------

#[test]
fn base_metadata_fields_injected_in_canonical_order() {
    let manifest = minimal_manifest();
    let type_yaml = minimal_type();
    let schema = load_schema_from_memory(&manifest, &[("sample".into(), type_yaml)]).unwrap();
    let td = schema.get_type("sample").unwrap();

    let keys: Vec<&str> = td.metadata_fields.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(
        keys,
        vec!["type", "created_date", "last_modified", "status", "tags"],
        "base fields must wrap declared fields: type/created/last_modified first, tags last"
    );
}

#[test]
fn base_metadata_carries_engine_flags() {
    let manifest = minimal_manifest();
    let type_yaml = minimal_type();
    let schema = load_schema_from_memory(&manifest, &[("sample".into(), type_yaml)]).unwrap();
    let td = schema.get_type("sample").unwrap();

    let created = td.metadata_field("created_date").unwrap();
    assert!(
        created.init_timestamp,
        "created_date must keep init_timestamp"
    );
    let modified = td.metadata_field("last_modified").unwrap();
    assert!(
        modified.auto_timestamp,
        "last_modified must keep auto_timestamp"
    );
    let tags = td.metadata_field("tags").unwrap();
    assert!(tags.optional, "tags must be optional by default");
}

#[test]
fn redeclaring_base_metadata_key_is_rejected() {
    // `type` is now rejected with `ReservedSchemaKey` (engine-invariant
    // frontmatter discriminator); the rest of the base-metadata keys
    // still surface as `RedeclaredBaseField` (engine-managed conveniences,
    // not reserved). See `reserved_metadata_field_keys` in the loader.
    for redeclared in ["created_date", "last_modified", "tags"] {
        let manifest = minimal_manifest();
        let type_yaml = format!(
            r#"name: sample
description: Sample type for tests
when_to_use: Whenever a minimal type is needed
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules:
      - One sentence describing the body.
metadata_fields:
  - key: {redeclared}
    description: Conflicting declaration
    field_type: string
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
propagating_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules:
  - Keep it short.
"#
        );
        let err = load_schema_from_memory(&manifest, &[("sample".into(), type_yaml)]).unwrap_err();
        match err {
            SchemaLoadError::RedeclaredBaseField { type_name, field } => {
                assert_eq!(type_name, "sample");
                assert_eq!(field, redeclared);
            }
            other => panic!("expected RedeclaredBaseField for '{redeclared}', got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Schema-strictness — edge shape + cardinality on RelationshipDef
// ---------------------------------------------------------------------------

#[test]
fn relationship_shape_round_trips_through_loader() {
    let manifest = minimal_manifest().replace(
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n",
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n      source_types: [sample]\n      target_types: [sample]\n      cardinality_per_source: \"1\"\n",
    );
    let schema = load(&manifest, &[("sample", &minimal_type())]).expect("load ok");
    let part_of = schema
        .relationship_def("PART_OF")
        .expect("PART_OF declared");
    assert_eq!(part_of.source_types, vec!["sample".to_string()]);
    assert_eq!(part_of.target_types, vec!["sample".to_string()]);
    assert_eq!(part_of.cardinality_per_source, Some(Cardinality::One));
}

#[test]
fn per_edge_description_defaults_to_forbidden_when_omitted() {
    use memstead_schema::PerEdgeDescription;
    let schema = load(&minimal_manifest(), &[("sample", &minimal_type())]).expect("load ok");
    for def in &schema.manifest.relationships.definitions {
        assert_eq!(
            def.per_edge_description,
            PerEdgeDescription::Forbidden,
            "{} must default to per_edge_description: forbidden",
            def.name
        );
    }
}

#[test]
fn per_edge_description_round_trips_through_loader() {
    use memstead_schema::PerEdgeDescription;
    let manifest = minimal_manifest().replace(
        "    - name: REFERENCES\n      description: Soft reference\n      default_weight: 0.5\n",
        "    - name: REFERENCES\n      description: Soft reference\n      default_weight: 0.5\n      per_edge_description: optional\n",
    ).replace(
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n",
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n      per_edge_description: required\n",
    );
    let schema = load(&manifest, &[("sample", &minimal_type())]).expect("load ok");
    let part_of = schema
        .relationship_def("PART_OF")
        .expect("PART_OF declared");
    assert_eq!(part_of.per_edge_description, PerEdgeDescription::Required);
    let references = schema
        .relationship_def("REFERENCES")
        .expect("REFERENCES declared");
    assert_eq!(
        references.per_edge_description,
        PerEdgeDescription::Optional
    );
}

/// `manual_authoring`
/// round-trips through the loader for every accepted variant. Default
/// is `Allow` so external user schemas without the field stay
/// permissive.
#[test]
fn manual_authoring_round_trips_through_loader() {
    use memstead_schema::ManualAuthoring;
    let manifest = minimal_manifest()
        .replace(
            "    - name: REFERENCES\n      description: Soft reference\n      default_weight: 0.5\n",
            "    - name: REFERENCES\n      description: Soft reference\n      default_weight: 0.5\n      manual_authoring: forbidden\n",
        )
        .replace(
            "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n",
            "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n      manual_authoring: warn\n",
        );
    let schema = load(&manifest, &[("sample", &minimal_type())]).expect("load ok");
    let references = schema
        .relationship_def("REFERENCES")
        .expect("REFERENCES declared");
    assert_eq!(references.manual_authoring, ManualAuthoring::Forbidden);
    let part_of = schema
        .relationship_def("PART_OF")
        .expect("PART_OF declared");
    assert_eq!(part_of.manual_authoring, ManualAuthoring::Warn);
}

/// Default `manual_authoring` value is `Allow` so external schemas
/// without the field stay permissive — pre-Item-21 behavior preserved
/// for every rel-type that doesn't opt in.
#[test]
fn manual_authoring_defaults_to_allow() {
    use memstead_schema::ManualAuthoring;
    let schema = load(&minimal_manifest(), &[("sample", &minimal_type())]).expect("load ok");
    for def in &schema.manifest.relationships.definitions {
        assert_eq!(
            def.manual_authoring,
            ManualAuthoring::Allow,
            "{} must default to manual_authoring: allow",
            def.name,
        );
    }
}

#[test]
fn per_edge_description_rejects_unknown_value_at_load() {
    let manifest = minimal_manifest().replace(
        "    - name: REFERENCES\n      description: Soft reference\n      default_weight: 0.5\n",
        "    - name: REFERENCES\n      description: Soft reference\n      default_weight: 0.5\n      per_edge_description: maybe\n",
    );
    let err = load(&manifest, &[("sample", &minimal_type())])
        .expect_err("unknown enum value must fail schema load");
    let msg = err.to_string();
    assert!(
        msg.contains("per_edge_description") || msg.contains("maybe"),
        "error must surface the field or invalid value; got: {msg}"
    );
}

#[test]
fn relationship_shape_defaults_to_empty() {
    let schema = load(&minimal_manifest(), &[("sample", &minimal_type())]).expect("load ok");
    for def in &schema.manifest.relationships.definitions {
        assert!(
            def.source_types.is_empty(),
            "{} must default to shape-free source",
            def.name
        );
        assert!(
            def.target_types.is_empty(),
            "{} must default to shape-free target",
            def.name
        );
        assert!(
            def.cardinality_per_source.is_none(),
            "{} must default to no cardinality",
            def.name
        );
    }
}

#[test]
fn unknown_source_type_rejected_at_load() {
    let manifest = minimal_manifest().replace(
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n",
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n      source_types: [smaple]\n",
    );
    let err = load(&manifest, &[("sample", &minimal_type())]).expect_err("must fail");
    match err {
        SchemaLoadError::UndeclaredRelationshipType {
            ref relationship,
            field,
            ref reference,
            ..
        } => {
            assert_eq!(relationship, "PART_OF");
            assert_eq!(field, "source_types");
            assert_eq!(reference, "smaple");
            // Did-you-mean suggestion against declared types — typo close
            // to `sample`, the lone declared type.
            assert!(
                err.to_string().contains("Did you mean 'sample'?"),
                "loader error must surface nearest-match suggestion: {err}"
            );
        }
        other => panic!("expected UndeclaredRelationshipType, got {other:?}"),
    }
}

#[test]
fn unknown_target_type_rejected_at_load() {
    let manifest = minimal_manifest().replace(
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n",
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n      target_types: [missing]\n",
    );
    let err = load(&manifest, &[("sample", &minimal_type())]).expect_err("must fail");
    assert!(
        matches!(
            err,
            SchemaLoadError::UndeclaredRelationshipType { field, .. } if field == "target_types"
        ),
        "got: {err}"
    );
}

#[test]
fn cardinality_typo_rejected_at_load() {
    // serde rejects values outside `1`, `0..1`, `1..N`, `0..N`. Authors
    // who tried `2` (or anything else) get a parse error rather than a
    // silently-ignored field.
    let manifest = minimal_manifest().replace(
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n",
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n      cardinality_per_source: \"2\"\n",
    );
    let err = load(&manifest, &[("sample", &minimal_type())]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::ParseManifest { .. }),
        "got: {err}"
    );
}

#[test]
fn cardinality_admits_predicates() {
    assert!(Cardinality::One.admits(1));
    assert!(!Cardinality::One.admits(0));
    assert!(!Cardinality::One.admits(2));

    assert!(Cardinality::ZeroOrOne.admits(0));
    assert!(Cardinality::ZeroOrOne.admits(1));
    assert!(!Cardinality::ZeroOrOne.admits(2));

    assert!(!Cardinality::OneOrMore.admits(0));
    assert!(Cardinality::OneOrMore.admits(1));
    assert!(Cardinality::OneOrMore.admits(2));

    assert!(Cardinality::ZeroOrMore.admits(0));
    assert!(Cardinality::ZeroOrMore.admits(99));
}

// ---------------------------------------------------------------------------
// Schema-strictness — required_outgoing on TypeDefinition
// ---------------------------------------------------------------------------

#[test]
fn required_outgoing_round_trips_through_loader() {
    let t = minimal_type().replace(
        "write_rules:\n  - Keep it short.\n",
        "write_rules:\n  - Keep it short.\nrequired_outgoing:\n  - relationships: [REFERENCES]\n    cardinality: at_least_one\n",
    );
    let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("load ok");
    let td = schema.get_type("sample").unwrap();
    assert_eq!(td.required_outgoing.len(), 1);
    let block = &td.required_outgoing[0];
    assert_eq!(block.relationships, vec!["REFERENCES".to_string()]);
    assert_eq!(
        block.cardinality,
        memstead_schema::types::RequiredCardinality::AtLeastOne
    );
    assert!(block.admits(1));
    assert!(!block.admits(0));
}

#[test]
fn required_outgoing_defaults_to_empty() {
    let schema = load(&minimal_manifest(), &[("sample", &minimal_type())]).expect("load ok");
    let td = schema.get_type("sample").unwrap();
    assert!(td.required_outgoing.is_empty());
}

#[test]
fn required_outgoing_unknown_relationship_rejected_at_load() {
    let t = minimal_type().replace(
        "write_rules:\n  - Keep it short.\n",
        "write_rules:\n  - Keep it short.\nrequired_outgoing:\n  - relationships: [REFRENCES]\n    cardinality: at_least_one\n",
    );
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    match err {
        SchemaLoadError::UndeclaredRelationship {
            ref relationship,
            field,
            ..
        } => {
            assert_eq!(relationship, "REFRENCES");
            assert_eq!(field, "required_outgoing");
            assert!(
                err.to_string().contains("Did you mean 'REFERENCES'?"),
                "loader must surface nearest-match suggestion: {err}"
            );
        }
        other => panic!("expected UndeclaredRelationship, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Schema-strictness — default_writing_guidance on SchemaManifest
// ---------------------------------------------------------------------------

#[test]
fn default_writing_guidance_round_trips_through_loader() {
    let manifest = minimal_manifest().replace(
        "community:\n  resolution: 1.0\n  seed: 42\n",
        "community:\n  resolution: 1.0\n  seed: 42\ndefault_writing_guidance:\n  avoid: |\n    Schema-default avoid prose.\n  goal: |\n    Schema-default goal prose.\n",
    );
    let schema = load(&manifest, &[("sample", &minimal_type())]).expect("load ok");
    let dwg = schema
        .manifest
        .default_writing_guidance
        .as_ref()
        .expect("default_writing_guidance present");
    assert_eq!(dwg.avoid.as_deref(), Some("Schema-default avoid prose.\n"),);
    assert_eq!(dwg.goal.as_deref(), Some("Schema-default goal prose.\n"));
}

#[test]
fn default_writing_guidance_defaults_to_none() {
    let schema = load(&minimal_manifest(), &[("sample", &minimal_type())]).expect("load ok");
    assert!(schema.manifest.default_writing_guidance.is_none());
}

#[test]
fn default_writing_guidance_avoid_only_accepted() {
    let manifest = minimal_manifest().replace(
        "community:\n  resolution: 1.0\n  seed: 42\n",
        "community:\n  resolution: 1.0\n  seed: 42\ndefault_writing_guidance:\n  avoid: just-avoid\n",
    );
    let schema = load(&manifest, &[("sample", &minimal_type())]).expect("load ok");
    let dwg = schema
        .manifest
        .default_writing_guidance
        .as_ref()
        .expect("dwg present");
    assert_eq!(dwg.avoid.as_deref(), Some("just-avoid"));
    assert!(dwg.goal.is_none());
}

#[test]
fn default_writing_guidance_rejects_unknown_subkey() {
    // serde's `deny_unknown_fields` on DefaultWritingGuidance makes typos hard.
    let manifest = minimal_manifest().replace(
        "community:\n  resolution: 1.0\n  seed: 42\n",
        "community:\n  resolution: 1.0\n  seed: 42\ndefault_writing_guidance:\n  avid: oops\n",
    );
    let err = load(&manifest, &[("sample", &minimal_type())]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::ParseManifest { .. }),
        "got: {err}"
    );
}

#[test]
fn required_outgoing_unknown_cardinality_rejected_at_load() {
    // serde rejects values outside the declared enum variants.
    let t = minimal_type().replace(
        "write_rules:\n  - Keep it short.\n",
        "write_rules:\n  - Keep it short.\nrequired_outgoing:\n  - relationships: [REFERENCES]\n    cardinality: at_least_two\n",
    );
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::ParseType { .. }),
        "got: {err}"
    );
}

// ---------------------------------------------------------------------------
// cross_mem_relationships section
// ---------------------------------------------------------------------------

#[test]
fn cross_mem_relationships_omitted_loads_cleanly() {
    let schema = load(&minimal_manifest(), &[("sample", &minimal_type())]).unwrap();
    assert!(schema.manifest.cross_mem_relationships.is_empty());
}

#[test]
fn cross_mem_relationships_empty_array_loads_cleanly() {
    let m = minimal_manifest().replace("community:", "cross_mem_relationships: []\ncommunity:");
    let schema = load(&m, &[("sample", &minimal_type())]).expect("empty list loads");
    assert!(schema.manifest.cross_mem_relationships.is_empty());
}

#[test]
fn cross_mem_relationships_section_loads_well_formed_entries() {
    let m = minimal_manifest().replace(
        "community:",
        "cross_mem_relationships:\n  - to_schema: other\n    definitions:\n      - name: ADDRESSES\n        description: outbound\n        default_weight: 1.0\n        source_types: [sample]\n        target_types: [foreign_type]\ncommunity:",
    );
    let schema = load(&m, &[("sample", &minimal_type())]).unwrap();
    assert_eq!(schema.manifest.cross_mem_relationships.len(), 1);
    let entry = &schema.manifest.cross_mem_relationships[0];
    assert_eq!(entry.to_schema, "other");
    assert_eq!(entry.definitions.len(), 1);
    assert_eq!(entry.definitions[0].name, "ADDRESSES");
    assert_eq!(
        entry.definitions[0].source_types,
        vec!["sample".to_string()]
    );
    // Target types are opaque — they reference the target schema's
    // namespace, not the source schema's types.
    assert_eq!(
        entry.definitions[0].target_types,
        vec!["foreign_type".to_string()]
    );
}

#[test]
fn cross_mem_relationships_to_schema_versioned_rejected() {
    // `to_schema` is the domain identity — a bare schema name. A
    // version suffix refuses at load so a version component can never
    // re-enter the eligibility path.
    let m = minimal_manifest().replace(
        "community:",
        "cross_mem_relationships:\n  - to_schema: \"other@1.0.0\"\n    definitions: []\ncommunity:",
    );
    let err = load(&m, &[("sample", &minimal_type())]).unwrap_err();
    match err {
        SchemaLoadError::InvalidCrossMemToSchema { value, .. } => {
            assert_eq!(value, "other@1.0.0");
        }
        other => panic!("expected InvalidCrossMemToSchema, got {other:?}"),
    }
    // The message directs the author to the bare-name form.
    let m2 = minimal_manifest().replace(
        "community:",
        "cross_mem_relationships:\n  - to_schema: \"other@1.0.0\"\n    definitions: []\ncommunity:",
    );
    let msg = load(&m2, &[("sample", &minimal_type())])
        .unwrap_err()
        .to_string();
    assert!(
        msg.contains("to_schema") && msg.contains("bare schema name"),
        "error must name the field and the expected bare-name form: {msg}"
    );
}

#[test]
fn cross_mem_relationships_to_schema_range_rejected() {
    // Range syntax is refused like any versioned form.
    let m = minimal_manifest().replace(
        "community:",
        "cross_mem_relationships:\n  - to_schema: other@^1.0.0\n    definitions: []\ncommunity:",
    );
    let err = load(&m, &[("sample", &minimal_type())]).unwrap_err();
    assert!(matches!(
        err,
        SchemaLoadError::InvalidCrossMemToSchema { .. }
    ));
}

#[test]
fn cross_mem_relationships_to_schema_must_be_valid_schema_name() {
    // Bare-name values follow the same shape grammar as schema names.
    let m = minimal_manifest().replace(
        "community:",
        "cross_mem_relationships:\n  - to_schema: Other_Schema\n    definitions: []\ncommunity:",
    );
    let err = load(&m, &[("sample", &minimal_type())]).unwrap_err();
    assert!(matches!(
        err,
        SchemaLoadError::InvalidCrossMemToSchema { .. }
    ));
}

#[test]
fn cross_mem_relationships_duplicate_to_schema_rejected() {
    let m = minimal_manifest().replace(
        "community:",
        "cross_mem_relationships:\n  - to_schema: other\n    definitions: []\n  - to_schema: other\n    definitions: []\ncommunity:",
    );
    let err = load(&m, &[("sample", &minimal_type())]).unwrap_err();
    match err {
        SchemaLoadError::DuplicateCrossMemToSchema { to_schema } => {
            assert_eq!(to_schema, "other");
        }
        other => panic!("expected DuplicateCrossMemToSchema, got {other:?}"),
    }
}

#[test]
fn cross_mem_relationships_source_types_must_belong_to_source() {
    // `source_types` belong to the source schema's namespace — unknown
    // names raise at load.
    let m = minimal_manifest().replace(
        "community:",
        "cross_mem_relationships:\n  - to_schema: other\n    definitions:\n      - name: ADDRESSES\n        description: outbound\n        default_weight: 1.0\n        source_types: [not_declared]\n        target_types: [foreign]\ncommunity:",
    );
    let err = load(&m, &[("sample", &minimal_type())]).unwrap_err();
    match err {
        SchemaLoadError::UndeclaredCrossMemSourceType {
            to_schema,
            relationship,
            reference,
            declared,
        } => {
            assert_eq!(to_schema, "other");
            assert_eq!(relationship, "ADDRESSES");
            assert_eq!(reference, "not_declared");
            assert!(declared.contains(&"sample".to_string()));
        }
        other => panic!("expected UndeclaredCrossMemSourceType, got {other:?}"),
    }
}

#[test]
fn cross_mem_relationships_target_types_are_opaque() {
    // `target_types` may name strings the source schema has never heard
    // of — they belong to the target schema's namespace and are not
    // checked at source-schema load time.
    let m = minimal_manifest().replace(
        "community:",
        "cross_mem_relationships:\n  - to_schema: other\n    definitions:\n      - name: ADDRESSES\n        description: outbound\n        default_weight: 1.0\n        source_types: [sample]\n        target_types: [completely_unknown_foreign_name]\ncommunity:",
    );
    let schema = load(&m, &[("sample", &minimal_type())])
        .expect("target_types are opaque — opaque strings load cleanly");
    assert_eq!(
        schema.manifest.cross_mem_relationships[0].definitions[0].target_types,
        vec!["completely_unknown_foreign_name".to_string()]
    );
}

// ---------------------------------------------------------------------------
// alias_target_rel_type — schema-level pointer
// ---------------------------------------------------------------------------

#[test]
fn alias_target_rel_type_defaults_to_none() {
    let schema = load(&minimal_manifest(), &[("sample", &minimal_type())]).unwrap();
    assert!(schema.manifest.alias_target_rel_type.is_none());
    assert!(schema.alias_target_rel_type().is_none());
}

#[test]
fn alias_target_rel_type_round_trips_through_loader() {
    let m = minimal_manifest().replace(
        "community:",
        "alias_target_rel_type: REFERENCES\ncommunity:",
    );
    let schema = load(&m, &[("sample", &minimal_type())]).expect("load ok");
    assert_eq!(
        schema.manifest.alias_target_rel_type.as_deref(),
        Some("REFERENCES"),
    );
    assert_eq!(schema.alias_target_rel_type(), Some("REFERENCES"));
}

#[test]
fn alias_target_rel_type_accepts_non_references_pointer() {
    // The engine must not hard-code REFERENCES. Any declared rel-type
    // name is a valid pointer.
    let m = minimal_manifest().replace("community:", "alias_target_rel_type: PART_OF\ncommunity:");
    let schema = load(&m, &[("sample", &minimal_type())])
        .expect("non-REFERENCES alias target loads cleanly");
    assert_eq!(schema.alias_target_rel_type(), Some("PART_OF"));
}

#[test]
fn alias_target_rel_type_undeclared_refuses_at_load() {
    let m = minimal_manifest().replace(
        "community:",
        "alias_target_rel_type: NOT_DECLARED\ncommunity:",
    );
    let err = load(&m, &[("sample", &minimal_type())]).expect_err("undeclared pointer must refuse");
    match err {
        SchemaLoadError::AliasTargetRelTypeNotDeclared {
            schema,
            target,
            declared,
        } => {
            assert_eq!(schema, "example");
            assert_eq!(target, "NOT_DECLARED");
            assert!(declared.contains(&"REFERENCES".to_string()));
        }
        other => panic!("expected AliasTargetRelTypeNotDeclared, got {other:?}"),
    }
}

#[test]
fn alias_target_rel_type_undeclared_surfaces_fuzzy_suggestion() {
    let m =
        minimal_manifest().replace("community:", "alias_target_rel_type: REFRENCES\ncommunity:");
    let err = load(&m, &[("sample", &minimal_type())]).expect_err("must fail");
    assert!(
        err.to_string().contains("Did you mean 'REFERENCES'?"),
        "loader must surface nearest-match suggestion: {err}"
    );
}

#[test]
fn alias_target_rel_type_auto_couples_manual_authoring_to_forbidden() {
    // Option C coupling: setting `alias_target_rel_type: REFERENCES`
    // forces the named rel-type's `manual_authoring` to `Forbidden`
    // at load. The pointer rel-type is engine-emitted-only; explicit
    // authoring is refused with `RELATION_MANUAL_AUTHORING_FORBIDDEN`.
    let m = minimal_manifest().replace(
        "community:",
        "alias_target_rel_type: REFERENCES\ncommunity:",
    );
    let schema = load(&m, &[("sample", &minimal_type())]).expect("load ok");
    assert_eq!(
        schema.relationship_manual_authoring("REFERENCES"),
        memstead_schema::ManualAuthoring::Forbidden,
        "pointer rel-type must be auto-coupled to Forbidden at load",
    );
}

#[test]
fn alias_target_rel_type_auto_coupling_overrides_explicit_allow() {
    // A schema that explicitly writes `manual_authoring: allow` on
    // the rel-type named by `alias_target_rel_type` gets silently
    // overridden to `Forbidden` at load. Explicit `allow` is
    // meaningless for a pointer rel-type — the synthesis pass is the
    // only path to such edges by design.
    let m = minimal_manifest()
        .replace(
            "    - name: REFERENCES\n      description: Soft reference\n      default_weight: 0.5\n",
            "    - name: REFERENCES\n      description: Soft reference\n      default_weight: 0.5\n      manual_authoring: allow\n",
        )
        .replace(
            "community:",
            "alias_target_rel_type: REFERENCES\ncommunity:",
        );
    let schema = load(&m, &[("sample", &minimal_type())]).expect("load ok");
    assert_eq!(
        schema.relationship_manual_authoring("REFERENCES"),
        memstead_schema::ManualAuthoring::Forbidden,
        "explicit allow on the pointer rel-type must be overridden to Forbidden",
    );
}

#[test]
fn alias_target_rel_type_non_pointer_rel_types_unaffected_by_coupling() {
    // The coupling only applies to the rel-type named by
    // `alias_target_rel_type`. Other rel-types keep whatever
    // `manual_authoring` posture the schema declared.
    let m = minimal_manifest().replace(
        "community:",
        "alias_target_rel_type: REFERENCES\ncommunity:",
    );
    let schema = load(&m, &[("sample", &minimal_type())]).expect("load ok");
    assert_eq!(
        schema.relationship_manual_authoring("PART_OF"),
        memstead_schema::ManualAuthoring::Allow,
        "non-pointer rel-types must retain their declared posture",
    );
}
