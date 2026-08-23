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
no_self_loop_relationships: []
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
        "no_self_loop_relationships: []",
        "no_self_loop_relationships: []\nedge_weight_overrides:\n  NOT_DECLARED: 2.0",
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
        "no_self_loop_relationships: []",
        "no_self_loop_relationships: []\nedge_weight_overrides:\n  PART_OF: 9.0",
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

/// The `engineering@0.1.0` builtin: knowledge-only vocabulary. The
/// three types load, the current-state types are ABSENT (write-time
/// class-boundary enforcement — a `spec` in a mem pinned to this
/// schema refuses `UNKNOWN_ENTITY_TYPE`), and the relationship
/// vocabulary admits every edge type live standing-knowledge content
/// carries (census-driven: REFERENCES, GOVERNS, MOTIVATED_BY,
/// SUPERSEDES, IMPLEMENTS, GENERALIZES) plus the type files' own
/// references (PART_OF hierarchy, DERIVED_FROM overrides, the
/// GOVERNS/CONSTRAINS no-self-loop declarations).
#[test]
fn builtin_engineering_is_knowledge_only() {
    let all = memstead_schema::builtins::load_builtin_schemas().expect("builtins load");
    let s = all
        .iter()
        .find(|s| s.manifest.name == "engineering")
        .expect("engineering builtin present");
    assert_eq!(s.version, semver::Version::new(0, 1, 0));
    assert_eq!(s.mode(), RelationshipMode::Strict);

    for name in ["decision", "principle", "memo"] {
        assert!(s.get_type(name).is_some(), "missing knowledge type: {name}");
    }
    assert_eq!(
        s.types.len(),
        3,
        "knowledge-only catalogue: exactly three types"
    );
    for absent in [
        "spec",
        "contract",
        "requirement",
        "actor",
        "incident",
        "concept",
    ] {
        assert!(
            s.get_type(absent).is_none(),
            "current-state type {absent} must be absent — the class boundary is a gate"
        );
    }

    for rel in [
        "REFERENCES",
        "GOVERNS",
        "MOTIVATED_BY",
        "SUPERSEDES",
        "IMPLEMENTS",
        "GENERALIZES",
        "PART_OF",
        "DERIVED_FROM",
        "CONSTRAINS",
        "_default",
    ] {
        assert!(
            s.relationship_known(rel),
            "census-required relationship {rel} must be in the vocabulary"
        );
    }
}

/// The `project@0.1.0` knowledge extension: `decision` and `memo`
/// exist with field shapes STRUCTURALLY IDENTICAL to their
/// `software@0.1.0` namesakes (sections: key/heading/required/
/// catch_all; metadata: key/type/enums/default/optional/serialization)
/// so entities migrate between the schemas with metadata verbatim.
/// `principle` gains optional `justification` + `authority`/
/// `universality` (engineering lineage) without changing any existing
/// section or field; the vocabulary admits the migrated content's
/// edge census.
#[test]
fn builtin_project_knowledge_extension_is_migration_compatible() {
    let all = memstead_schema::builtins::load_builtin_schemas().expect("builtins load");
    let project = all
        .iter()
        .find(|s| s.manifest.name == "project")
        .expect("project builtin present");
    let software = all
        .iter()
        .find(|s| s.manifest.name == "software")
        .expect("software builtin present");

    assert_eq!(project.types.len(), 12, "ten original + decision + memo");

    // decision/memo shapes match software's, structurally.
    for ty in ["decision", "memo"] {
        let p = project
            .get_type(ty)
            .unwrap_or_else(|| panic!("project lacks {ty}"));
        let s = software
            .get_type(ty)
            .unwrap_or_else(|| panic!("software lacks {ty}"));
        let sec = |t: &memstead_schema::TypeDefinition| {
            t.sections
                .iter()
                .map(|d| (d.key.clone(), d.heading.clone(), d.required, d.catch_all))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sec(&p),
            sec(&s),
            "{ty}: section shapes must match software's"
        );
        let meta = |t: &memstead_schema::TypeDefinition| {
            t.metadata_fields
                .iter()
                .map(|f| {
                    (
                        f.key.clone(),
                        format!("{:?}", f.field_type),
                        f.enum_values.clone(),
                        f.default_value.clone(),
                        f.is_required(),
                        format!("{:?}", f.serialization),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            meta(&p),
            meta(&s),
            "{ty}: metadata shapes must match software's"
        );
    }

    // principle: additive extension only — every software-lineage
    // field/section present, existing project fields untouched.
    let principle = project.get_type("principle").expect("principle");
    assert!(
        principle
            .sections
            .iter()
            .any(|s| s.key == "justification" && !s.required),
        "principle gains optional justification"
    );
    for (key, required_absent_default) in [("authority", true), ("universality", true)] {
        let f = principle
            .metadata_fields
            .iter()
            .find(|f| f.key == key)
            .unwrap_or_else(|| panic!("principle lacks {key}"));
        assert!(f.is_required() != required_absent_default && f.default_value.is_none());
    }
    for (key, still) in [("status", "active"), ("category", "")] {
        let f = principle.metadata_fields.iter().find(|f| f.key == key);
        assert!(
            f.is_some(),
            "existing project principle field {key} must survive ({still})"
        );
    }

    // Vocabulary: the migrated content's census edges resolve.
    for rel in [
        "DERIVED_FROM",
        "SPECIALIZES",
        "GENERALIZES",
        "DEFINES",
        "SUPERSEDES",
        "MOTIVATED_BY",
        "GOVERNS",
    ] {
        assert!(
            project.relationship_known(rel),
            "project vocabulary must admit {rel}"
        );
    }

    // Cross-mem: knowledge sources into software mems; the
    // engineering block exists for split-crossing lineage edges.
    let sw = project
        .cross_mem_entry("software")
        .expect("to_schema: software");
    let refs = sw
        .definitions
        .iter()
        .find(|d| d.name == "REFERENCES")
        .expect("REFERENCES");
    for src in ["decision", "principle", "memo", "pillar", "evidence"] {
        assert!(
            refs.source_types.iter().any(|t| t == src),
            "cross-mem REFERENCES must admit source {src}"
        );
    }
    for rel in [
        "GOVERNS",
        "MOTIVATED_BY",
        "MOTIVATES",
        "CONSTRAINS",
        "DEFINES",
    ] {
        assert!(
            sw.definitions.iter().any(|d| d.name == rel),
            "software block lacks {rel}"
        );
    }
    let eng = project
        .cross_mem_entry("engineering")
        .expect("to_schema: engineering");
    for rel in ["REFERENCES", "DERIVED_FROM", "MOTIVATED_BY", "SPECIALIZES"] {
        assert!(
            eng.definitions.iter().any(|d| d.name == rel),
            "engineering block lacks {rel}"
        );
    }

    // The engineering builtin declares its own outbound software
    // vocabulary (the migrated public content's census).
    let engineering = all
        .iter()
        .find(|s| s.manifest.name == "engineering")
        .expect("engineering builtin present");
    let eng_sw = engineering
        .cross_mem_entry("software")
        .expect("engineering → software block");
    for rel in ["REFERENCES", "GOVERNS", "MOTIVATED_BY", "IMPLEMENTS"] {
        assert!(
            eng_sw.definitions.iter().any(|d| d.name == rel),
            "engineering→software lacks {rel}"
        );
    }

    // software declares its outbound knowledge-side vocabulary —
    // census-driven from live paired-mem content (the KEEP /
    // REPOINT edge sets of the knowledge-home split).
    let sw_eng = software
        .cross_mem_entry("engineering")
        .expect("software → engineering block");
    for rel in ["REFERENCES", "MOTIVATED_BY", "DERIVED_FROM", "VALIDATES"] {
        assert!(
            sw_eng.definitions.iter().any(|d| d.name == rel),
            "software→engineering lacks {rel}"
        );
    }
    let sw_pr = software
        .cross_mem_entry("project")
        .expect("software → project block");
    for rel in [
        "REFERENCES",
        "MOTIVATED_BY",
        "DEPENDS_ON",
        "IMPLEMENTS",
        "SUPERSEDES",
        "OWNS",
    ] {
        assert!(
            sw_pr.definitions.iter().any(|d| d.name == rel),
            "software→project lacks {rel}"
        );
    }
    let owns = sw_pr.definitions.iter().find(|d| d.name == "OWNS").unwrap();
    assert_eq!(
        owns.source_types,
        vec!["actor"],
        "cross-mem OWNS stays actor-sourced"
    );
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
    assert!(!tags.is_required(), "tags must be optional by default");
}

/// A schema declaring a reserved identity/discriminator metadata key
/// (`type` / `mem` / `id`) LOADS — boot and sealed-schema reads share
/// this loader, and a schema sealed before the reservation widened must
/// keep booting — but the install-path gate
/// (`check_reserved_metadata_keys`) refuses it with the typed error
/// naming the key. Refusal complement: a schema declaring none of them
/// passes the gate untouched.
#[test]
fn reserved_metadata_keys_load_but_refuse_at_install_gate() {
    for reserved in ["type", "mem", "id"] {
        let type_yaml = minimal_type().replace(
            "metadata_fields:\n",
            &format!(
                "metadata_fields:\n  - key: {reserved}\n    description: Smuggled reserved key\n    field_type: string\n"
            ),
        );
        // Sealed/boot posture: the load itself succeeds.
        let schema = load(&minimal_manifest(), &[("sample", &type_yaml)]).unwrap_or_else(|e| {
            panic!("schema declaring '{reserved}' must still load (sealed posture), got {e:?}")
        });
        assert!(
            schema
                .get_type("sample")
                .unwrap()
                .declared_metadata_keys
                .iter()
                .any(|k| k == reserved),
            "loader must record the raw declared key '{reserved}'"
        );
        // Authoring/install posture: the gate refuses, naming the key.
        let err = memstead_schema::check_reserved_metadata_keys(&schema)
            .expect_err("install gate must refuse the reserved key");
        match err {
            SchemaLoadError::ReservedSchemaKey {
                type_name,
                kind,
                offending_key,
                reserved_keys,
            } => {
                assert_eq!(type_name, "sample");
                assert_eq!(kind, "metadata_field");
                assert_eq!(offending_key, reserved);
                assert_eq!(reserved_keys, vec!["type", "mem", "id"]);
            }
            other => panic!("expected ReservedSchemaKey for '{reserved}', got {other:?}"),
        }
    }

    // Complement: a clean schema passes the gate.
    let clean = load(&minimal_manifest(), &[("sample", &minimal_type())]).expect("load ok");
    memstead_schema::check_reserved_metadata_keys(&clean)
        .expect("a schema declaring no reserved key passes the install gate");
}

#[test]
fn redeclaring_base_metadata_key_is_rejected() {
    // `type` refuses with `ReservedSchemaKey` on the install path
    // (engine-invariant frontmatter discriminator; see
    // `check_reserved_metadata_keys`); the rest of the base-metadata
    // keys still surface as `RedeclaredBaseField` at load
    // (engine-managed conveniences, not reserved). See
    // `reserved_metadata_field_keys` in the loader.
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
no_self_loop_relationships: []
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

/// Plan 11: `to_schema: "*"` loads when bound to the schema's
/// `alias_target_rel_type`, and the priority-ordered matcher resolves
/// it for arbitrary destination names — alongside exact entries.
#[test]
fn cross_mem_wildcard_bound_to_alias_target_loads_and_matches_any_name() {
    let m = minimal_manifest().replace(
        "community:",
        "cross_mem_relationships:\n  - to_schema: \"*\"\n    definitions:\n      - name: REFERENCES\n        description: soft link anywhere\n        default_weight: 0.5\n        source_types: [sample]\n  - to_schema: other\n    definitions:\n      - name: PART_OF\n        description: structural, per-schema\n        default_weight: 3.0\nalias_target_rel_type: REFERENCES\ncommunity:",
    );
    let schema = load(&m, &[("sample", &minimal_type())]).expect("wildcard bound to alias loads");
    // Arbitrary never-seen destination: only the wildcard applies.
    let entries = schema.cross_mem_entries("user-written-schema");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].to_schema, "*");
    // Declared destination: exact entry FIRST, wildcard still present —
    // a structural declaration must not shadow the wildcarded alias.
    let entries = schema.cross_mem_entries("other");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].to_schema, "other");
    assert_eq!(entries[1].to_schema, "*");
    // No-entry schema name without a wildcard: empty (exact-only form).
    let m2 = minimal_manifest().replace(
        "community:",
        "cross_mem_relationships:\n  - to_schema: other\n    definitions: []\ncommunity:",
    );
    let schema2 = load(&m2, &[("sample", &minimal_type())]).unwrap();
    assert!(schema2.cross_mem_entries("stranger").is_empty());
}

/// Plan 11 refusal complements: a wildcard for a NON-alias rel-type is
/// refused naming both rel-types; a schema with no
/// `alias_target_rel_type` cannot use a wildcard at all.
#[test]
fn cross_mem_wildcard_refuses_non_alias_rel_type_and_missing_alias_target() {
    // Wildcard declaring a structural rel-type: refused, both names in
    // the message.
    let m = minimal_manifest().replace(
        "community:",
        "cross_mem_relationships:\n  - to_schema: \"*\"\n    definitions:\n      - name: PART_OF\n        description: structural\n        default_weight: 3.0\nalias_target_rel_type: REFERENCES\ncommunity:",
    );
    let err = load(&m, &[("sample", &minimal_type())]).unwrap_err();
    match &err {
        SchemaLoadError::CrossMemWildcardNonAliasRelType {
            rel_type,
            alias_target,
        } => {
            assert_eq!(rel_type, "PART_OF");
            assert_eq!(alias_target, "REFERENCES");
        }
        other => panic!("expected CrossMemWildcardNonAliasRelType, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("PART_OF") && msg.contains("REFERENCES"),
        "message names both rel-types: {msg}"
    );

    // No alias_target_rel_type declared: no wildcard at all.
    let m2 = minimal_manifest().replace(
        "community:",
        "cross_mem_relationships:\n  - to_schema: \"*\"\n    definitions:\n      - name: REFERENCES\n        description: soft\n        default_weight: 0.5\ncommunity:",
    );
    let err2 = load(&m2, &[("sample", &minimal_type())]).unwrap_err();
    assert!(
        matches!(err2, SchemaLoadError::CrossMemWildcardWithoutAliasTarget),
        "got {err2:?}"
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

// ---------------------------------------------------------------------------
// Section-heading round-trip (derive_section_key + install-time check)
// ---------------------------------------------------------------------------

/// A type whose section headings all derive back to their keys, across
/// the shapes a schema author would write: single-word, multi-word
/// matching the key, and non-ASCII whose lowercase equals the key.
fn roundtrip_clean_type() -> String {
    r#"name: sample
description: Sample type for round-trip tests
when_to_use: Round-trip tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
  - key: current_state
    heading: Current State
    required: false
    search_weight: 5.0
    write_rules: []
  - key: begründung
    heading: Begründung
    required: false
    search_weight: 5.0
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
"#
    .to_string()
}

#[test]
fn derive_section_key_shapes() {
    use memstead_schema::derive_section_key;
    assert_eq!(derive_section_key("Body"), "body");
    assert_eq!(derive_section_key("Current State"), "current_state");
    assert_eq!(derive_section_key("Begründung"), "begründung");
    assert_eq!(derive_section_key("Answers argued"), "answers_argued");
    assert_eq!(derive_section_key("Out of Scope"), "out_of_scope");
    assert_eq!(derive_section_key("A  B"), "a__b");
}

#[test]
fn heading_roundtrip_check_accepts_conforming_schema() {
    let schema = load(&minimal_manifest(), &[("sample", &roundtrip_clean_type())])
        .expect("clean schema loads");
    memstead_schema::check_section_heading_roundtrip(&schema)
        .expect("all headings derive to their keys");
}

#[test]
fn heading_roundtrip_check_refuses_and_names_every_tuple() {
    // Two violations in one type — plus one good section, which must
    // not rescue the schema (refused whole, not partially accepted).
    let bad = roundtrip_clean_type()
        .replace("    heading: Current State\n", "    heading: In Scope\n")
        .replace("    heading: Begründung\n", "    heading: Answers argued\n");
    let schema = load(&minimal_manifest(), &[("sample", &bad)])
        .expect("violating schema still LOADS — the check is a separate gate");
    let err = memstead_schema::check_section_heading_roundtrip(&schema)
        .expect_err("non-deriving headings must be refused");
    match &err {
        SchemaLoadError::SectionHeadingMismatch { violations } => {
            assert_eq!(violations.len(), 2, "every offending pair is listed");
            let mut pairs: Vec<(&str, &str, &str)> = violations
                .iter()
                .map(|v| (v.key.as_str(), v.heading.as_str(), v.derived_key.as_str()))
                .collect();
            pairs.sort();
            assert_eq!(
                pairs,
                vec![
                    ("begründung", "Answers argued", "answers_argued"),
                    ("current_state", "In Scope", "in_scope"),
                ]
            );
            assert!(
                violations.iter().all(|v| v.type_name == "sample"),
                "tuples name the offending type"
            );
        }
        other => panic!("expected SectionHeadingMismatch, got {other:?}"),
    }
    // The message names both offending pairs and states the fix.
    let msg = err.to_string();
    assert!(msg.contains("'current_state'") && msg.contains("'In Scope'"));
    assert!(msg.contains("'begründung'") && msg.contains("'Answers argued'"));
    assert!(msg.contains("Fix:"), "message states the fix: {msg}");
}

#[test]
fn heading_roundtrip_violating_schema_still_loads() {
    // Sealed-schema guarantee: the loader itself accepts a violating
    // schema — only the explicit installation-path check refuses.
    // load_schema_from_memory is the same function every boot path
    // uses, so this locks "no boot path refuses on this condition"
    // at the loader level.
    let bad =
        roundtrip_clean_type().replace("    heading: Current State\n", "    heading: In Scope\n");
    load(&minimal_manifest(), &[("sample", &bad)])
        .expect("violating schema must keep loading (sealed schemas must not brick)");
}

#[test]
fn every_builtin_schema_passes_heading_roundtrip() {
    // The refusal cannot land while a shipped built-in would be
    // refused — the planning goal type's scope sections were fixed in
    // the same change (scope_in/In Scope → in_scope/In Scope,
    // scope_out/Out of Scope → out_of_scope/Out of Scope).
    for schema in memstead_schema::builtins::load_builtin_schemas().expect("builtins load") {
        memstead_schema::check_section_heading_roundtrip(&schema).unwrap_or_else(|e| {
            panic!(
                "built-in schema '{}' violates heading round-trip: {e}",
                schema.manifest.name
            )
        });
    }
}

/// The shipped built-ins must pass the widened reserved-metadata-key
/// gate — the install-path refusal cannot land while a built-in would
/// be refused by it.
#[test]
fn every_builtin_schema_passes_reserved_metadata_keys() {
    for schema in memstead_schema::builtins::load_builtin_schemas().expect("builtins load") {
        memstead_schema::check_reserved_metadata_keys(&schema).unwrap_or_else(|e| {
            panic!(
                "built-in schema '{}' declares a reserved metadata key: {e}",
                schema.manifest.name
            )
        });
    }
}

// ---------------------------------------------------------------------------
// Constraint vocabulary (plan: schemas declare what is unhealthy to keep)
// ---------------------------------------------------------------------------

/// Valid `requires_when` declarations load; severity defaults to warn
/// and parses when declared as block. `field` may name a section
/// (`body`) or a metadata field (`status`).
#[test]
fn constraint_requires_when_accepted_with_severity() {
    use memstead_schema::{ConstraintDef, ConstraintSeverity};
    let t = minimal_type()
        + r#"constraints:
  - kind: requires_when
    field: body
    when_field: status
    when_value: closed
  - kind: requires_when
    field: status
    when_field: status
    when_value: active
    severity: block
"#;
    let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("must load");
    let td = schema.types.get("sample").unwrap();
    assert_eq!(td.constraints.len(), 2);
    let ConstraintDef::RequiresWhen { severity, .. } = &td.constraints[0] else {
        panic!("expected requires_when");
    };
    assert_eq!(*severity, ConstraintSeverity::Warn, "default is warn");
    let ConstraintDef::RequiresWhen { severity, .. } = &td.constraints[1] else {
        panic!("expected requires_when");
    };
    assert_eq!(*severity, ConstraintSeverity::Block);
}

#[test]
fn constraint_requires_when_unknown_field_rejected() {
    let t = minimal_type()
        + r#"constraints:
  - kind: requires_when
    field: nonexistent
    when_field: status
    when_value: closed
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { ref offender, .. } if offender == "nonexistent"),
        "got: {err}"
    );
}

#[test]
fn constraint_requires_when_unknown_when_field_rejected() {
    let t = minimal_type()
        + r#"constraints:
  - kind: requires_when
    field: body
    when_field: phase
    when_value: closed
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { ref offender, .. } if offender == "phase"),
        "got: {err}"
    );
}

#[test]
fn constraint_requires_when_value_outside_enum_rejected() {
    let t = minimal_type()
        + r#"constraints:
  - kind: requires_when
    field: body
    when_field: status
    when_value: archived
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { ref offender, .. } if offender == "archived"),
        "got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Reachability obligations (`must_reach`)
// ---------------------------------------------------------------------------

/// A valid obligation loads and round-trips; severity defaults to
/// warn; `max_depth` is optional.
#[test]
fn must_reach_accepted_round_trips() {
    use memstead_schema::{ConstraintSeverity, ReachDirection};
    let t = minimal_type()
        + r#"must_reach:
  - relationships: [PART_OF, REFERENCES]
    direction: out
    terminal_types: [sample]
    max_depth: 3
  - relationships: [REFERENCES]
    direction: in
    terminal_types: [sample]
"#;
    let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("must load");
    let td = schema.types.get("sample").unwrap();
    assert_eq!(td.must_reach.len(), 2);
    assert_eq!(
        td.must_reach[0].relationships,
        vec!["PART_OF", "REFERENCES"]
    );
    assert_eq!(td.must_reach[0].direction, ReachDirection::Out);
    assert_eq!(td.must_reach[0].terminal_types, vec!["sample"]);
    assert_eq!(td.must_reach[0].max_depth, Some(3));
    assert_eq!(td.must_reach[0].severity, ConstraintSeverity::Warn);
    assert_eq!(td.must_reach[1].direction, ReachDirection::In);
    assert_eq!(td.must_reach[1].max_depth, None);
}

/// `block` is a promise the engine will not keep on a transitive
/// property — the loader refuses it.
#[test]
fn must_reach_block_severity_rejected() {
    let t = minimal_type()
        + r#"must_reach:
  - relationships: [PART_OF]
    direction: out
    terminal_types: [sample]
    severity: block
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { kind: "must_reach", ref offender, .. } if offender == "block"),
        "got: {err}"
    );
}

#[test]
fn must_reach_undeclared_relationship_rejected() {
    let t = minimal_type()
        + r#"must_reach:
  - relationships: [FLOATS]
    direction: out
    terminal_types: [sample]
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(format!("{err}").contains("FLOATS"), "got: {err}");
}

#[test]
fn must_reach_unknown_terminal_type_rejected() {
    let t = minimal_type()
        + r#"must_reach:
  - relationships: [PART_OF]
    direction: out
    terminal_types: [ghost]
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { kind: "must_reach", ref offender, .. } if offender == "ghost"),
        "got: {err}"
    );
}

/// The direction vocabulary is closed (`out` / `in`) — anything else
/// fails deserialization, so no declaration loads half-understood.
#[test]
fn must_reach_unknown_direction_rejected() {
    let t = minimal_type()
        + r#"must_reach:
  - relationships: [PART_OF]
    direction: sideways
    terminal_types: [sample]
"#;
    load(&minimal_manifest(), &[("sample", &t)]).expect_err("unknown direction must fail");
}

#[test]
fn must_reach_zero_depth_rejected() {
    let t = minimal_type()
        + r#"must_reach:
  - relationships: [PART_OF]
    direction: out
    terminal_types: [sample]
    max_depth: 0
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { kind: "must_reach", ref offender, .. } if offender == "0"),
        "got: {err}"
    );
}

#[test]
fn must_reach_empty_lists_rejected() {
    let t = minimal_type()
        + r#"must_reach:
  - relationships: []
    direction: out
    terminal_types: [sample]
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(
            err,
            SchemaLoadError::InvalidConstraint {
                kind: "must_reach",
                ..
            }
        ),
        "got: {err}"
    );
    let t = minimal_type()
        + r#"must_reach:
  - relationships: [PART_OF]
    direction: out
    terminal_types: []
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(
            err,
            SchemaLoadError::InvalidConstraint {
                kind: "must_reach",
                ..
            }
        ),
        "got: {err}"
    );
}

/// The `kind` tag is closed: a constraint form the engine does not
/// evaluate fails deserialization — no declaration can load and be
/// silently ignored (the `no_self_loop_relationships` lesson).
#[test]
fn constraint_unknown_kind_rejected() {
    let t = minimal_type()
        + r#"constraints:
  - kind: uniqueness
    fields: [status]
"#;
    load(&minimal_manifest(), &[("sample", &t)]).expect_err("unknown kind must fail");
}

/// `required_outgoing` blocks parse a declared severity; an unknown
/// severity literal fails deserialization.
#[test]
fn required_outgoing_severity_parses_and_rejects_unknown() {
    let t = minimal_type()
        + r#"required_outgoing:
  - relationships: [PART_OF]
    cardinality: at_least_one
    severity: block
"#;
    let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("must load");
    let td = schema.types.get("sample").unwrap();
    assert_eq!(
        td.required_outgoing[0].severity,
        memstead_schema::ConstraintSeverity::Block
    );

    let bad = minimal_type()
        + r#"required_outgoing:
  - relationships: [PART_OF]
    cardinality: at_least_one
    severity: fatal
"#;
    load(&minimal_manifest(), &[("sample", &bad)]).expect_err("unknown severity must fail");
}

// ---------------------------------------------------------------------------
// Conditional required_outgoing (`when_field` / `when_value`)
// ---------------------------------------------------------------------------

/// A conditional block loads alongside an unconditional one; the
/// condition pair round-trips, and the unconditional block keeps
/// `None` for both keys.
#[test]
fn required_outgoing_conditional_block_accepted() {
    let t = minimal_type()
        + r#"required_outgoing:
  - relationships: [REFERENCES]
    cardinality: at_least_one
  - relationships: [PART_OF]
    cardinality: at_least_one
    severity: block
    when_field: status
    when_value: closed
"#;
    let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("must load");
    let td = schema.types.get("sample").unwrap();
    assert_eq!(td.required_outgoing.len(), 2);
    assert_eq!(td.required_outgoing[0].when_field, None);
    assert_eq!(td.required_outgoing[0].when_value, None);
    assert_eq!(
        td.required_outgoing[1].when_field.as_deref(),
        Some("status")
    );
    assert_eq!(
        td.required_outgoing[1].when_value.as_deref(),
        Some("closed")
    );
}

#[test]
fn required_outgoing_when_field_without_when_value_rejected() {
    let t = minimal_type()
        + r#"required_outgoing:
  - relationships: [PART_OF]
    cardinality: at_least_one
    when_field: status
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { kind: "required_outgoing", ref offender, .. } if offender == "status"),
        "got: {err}"
    );
}

#[test]
fn required_outgoing_when_value_without_when_field_rejected() {
    let t = minimal_type()
        + r#"required_outgoing:
  - relationships: [PART_OF]
    cardinality: at_least_one
    when_value: closed
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { kind: "required_outgoing", ref offender, .. } if offender == "closed"),
        "got: {err}"
    );
}

#[test]
fn required_outgoing_when_field_undeclared_rejected() {
    let t = minimal_type()
        + r#"required_outgoing:
  - relationships: [PART_OF]
    cardinality: at_least_one
    when_field: phase
    when_value: closed
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { kind: "required_outgoing", ref offender, .. } if offender == "phase"),
        "got: {err}"
    );
}

/// Stricter than `requires_when`: a trigger field without
/// `enum_values` refuses — a free-text-armed edge obligation would
/// never fire predictably.
#[test]
fn required_outgoing_when_field_non_enum_rejected() {
    let t = minimal_type().replace(
        "metadata_fields:\n",
        "metadata_fields:\n  - key: owner\n    description: Free-text owner\n    field_type: string\n",
    ) + r#"required_outgoing:
  - relationships: [PART_OF]
    cardinality: at_least_one
    when_field: owner
    when_value: someone
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { kind: "required_outgoing", ref offender, .. } if offender == "owner"),
        "got: {err}"
    );
}

#[test]
fn required_outgoing_when_value_outside_enum_rejected() {
    let t = minimal_type()
        + r#"required_outgoing:
  - relationships: [PART_OF]
    cardinality: at_least_one
    when_field: status
    when_value: archived
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { kind: "required_outgoing", ref offender, .. } if offender == "archived"),
        "got: {err}"
    );
}

/// Forms 2/3/5 accept valid declarations; uniqueness defaults to
/// block (its whole point is preventing the duplicate).
#[test]
fn constraint_forms_two_three_five_accepted() {
    use memstead_schema::{ConstraintDef, ConstraintSeverity};
    let t = minimal_type()
        + r#"constraints:
  - kind: unique
    fields: [status]
  - kind: enum_from_neighbour
    field: status
    rel_type: REFERENCES
    section: body
  - kind: status_propagation
    field: status
    value: closed
    rel_type: PART_OF
    direction: incoming
"#;
    let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("must load");
    let td = schema.types.get("sample").unwrap();
    assert_eq!(td.constraints.len(), 3);
    let ConstraintDef::Unique { severity, .. } = &td.constraints[0] else {
        panic!("expected unique");
    };
    assert_eq!(
        *severity,
        ConstraintSeverity::Block,
        "uniqueness defaults to block"
    );
}

#[test]
fn constraint_unique_unknown_field_rejected() {
    let t = minimal_type()
        + r#"constraints:
  - kind: unique
    fields: [status, rede_sha256]
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { ref offender, .. } if offender == "rede_sha256"),
        "got: {err}"
    );
}

#[test]
fn constraint_unique_empty_fields_rejected() {
    let t = minimal_type()
        + r#"constraints:
  - kind: unique
    fields: []
"#;
    load(&minimal_manifest(), &[("sample", &t)]).expect_err("empty tuple must fail");
}

#[test]
fn constraint_enum_from_neighbour_unknown_rel_type_rejected() {
    let t = minimal_type()
        + r#"constraints:
  - kind: enum_from_neighbour
    field: status
    rel_type: ENUMERATES
    section: body
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { ref offender, .. } if offender == "ENUMERATES"),
        "got: {err}"
    );
}

#[test]
fn constraint_enum_from_neighbour_unknown_section_rejected() {
    let t = minimal_type()
        + r#"constraints:
  - kind: enum_from_neighbour
    field: status
    rel_type: REFERENCES
    section: vocabulary
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { ref offender, .. } if offender == "vocabulary"),
        "got: {err}"
    );
}

#[test]
fn constraint_status_propagation_value_outside_enum_rejected() {
    let t = minimal_type()
        + r#"constraints:
  - kind: status_propagation
    field: status
    value: fallen
    rel_type: PART_OF
    direction: incoming
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { ref offender, .. } if offender == "fallen"),
        "got: {err}"
    );
}

/// `status_propagation` can never refuse a write, so a `block`
/// declaration is refused at load rather than accepted as a promise
/// the engine will not keep.
#[test]
fn constraint_status_propagation_block_severity_rejected() {
    let t = minimal_type()
        + r#"constraints:
  - kind: status_propagation
    field: status
    value: closed
    rel_type: PART_OF
    direction: incoming
    severity: block
"#;
    let err = load(&minimal_manifest(), &[("sample", &t)]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::InvalidConstraint { ref offender, .. } if offender == "block"),
        "got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Section-format declarations (plan 08)
// ---------------------------------------------------------------------------

/// A full declaration (content + item_pattern + example + severity)
/// loads, compiles, and defaults severity to block.
#[test]
fn section_format_declaration_accepted_and_compiled() {
    use memstead_schema::ConstraintSeverity;
    let t = minimal_type().replace(
        "  - key: body\n    heading: Body\n    required: true\n",
        "  - key: body\n    heading: Body\n    required: true\n    content: \"(heading(3) list(bullet))+\"\n    item_pattern: '\\*\\*(?<name>[^*]+)\\*\\* — (?<datum>\\d{4}-\\d{2}-\\d{2})'\n    example: |\n      ### Phase 1\n      - **Kickoff** — 2026-09-01\n",
    );
    let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("must load");
    let td = schema.types.get("sample").unwrap();
    let section = td.sections.iter().find(|s| s.key == "body").unwrap();
    assert_eq!(
        section.content.as_deref(),
        Some("(heading(3) list(bullet))+")
    );
    assert!(section.compiled_content.is_some(), "compiled at load");
    assert_eq!(
        section.format_severity,
        ConstraintSeverity::Block,
        "default block"
    );

    let warn = t.replace(
        "    example: |",
        "    format_severity: warn\n    example: |",
    );
    let schema = load(&minimal_manifest(), &[("sample", &warn)]).expect("must load");
    let td = schema.types.get("sample").unwrap();
    let section = td.sections.iter().find(|s| s.key == "body").unwrap();
    assert_eq!(section.format_severity, ConstraintSeverity::Warn);
}

/// Loader honesty: EVERY problem of a malformed declaration is named,
/// never the first only.
#[test]
fn section_format_malformed_declaration_names_all_offenders() {
    let t = minimal_type().replace(
        "  - key: body\n    heading: Body\n    required: true\n",
        "  - key: body\n    heading: Body\n    required: true\n    content: \"bulletList\"\n    item_pattern: '([unclosed'\n    table:\n      columns: []\n",
    );
    // Lenient at load (a sealed schema keeps booting) — the refusal
    // fires on the install / strict-validation path.
    let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("boot-lenient load");
    let err = memstead_schema::check_section_formats(&schema).expect_err("install must refuse");
    let SchemaLoadError::InvalidSectionFormat {
        section, problems, ..
    } = &err
    else {
        panic!("expected InvalidSectionFormat, got: {err}");
    };
    assert_eq!(section, "body");
    assert!(problems.len() >= 3, "all offenders named: {problems:?}");
    assert!(problems.iter().any(|p| p.contains("bulletList")));
    assert!(problems.iter().any(|p| p.contains("item_pattern")));
    assert!(problems.iter().any(|p| p.contains("columns")));
}

/// `item_pattern` needs exactly one of list/paragraph in the content
/// expression; a table block needs `table` in the expression; format
/// fields without `content` refuse.
#[test]
fn section_format_cross_field_legality() {
    // Both list and paragraph → refuse.
    let both = minimal_type().replace(
        "  - key: body\n    heading: Body\n    required: true\n",
        "  - key: body\n    heading: Body\n    required: true\n    content: \"paragraph list\"\n    item_pattern: '.+'\n",
    );
    let schema = load(&minimal_manifest(), &[("sample", &both)]).expect("lenient load");
    memstead_schema::check_section_formats(&schema).expect_err("both kinds must fail");

    // Neither → refuse.
    let neither = minimal_type().replace(
        "  - key: body\n    heading: Body\n    required: true\n",
        "  - key: body\n    heading: Body\n    required: true\n    content: \"table\"\n    item_pattern: '.+'\n",
    );
    let schema = load(&minimal_manifest(), &[("sample", &neither)]).expect("lenient load");
    memstead_schema::check_section_formats(&schema).expect_err("neither kind must fail");

    // table block without `table` in content → refuse.
    let t = minimal_type().replace(
        "  - key: body\n    heading: Body\n    required: true\n",
        "  - key: body\n    heading: Body\n    required: true\n    content: \"paragraph\"\n    table:\n      columns: [Name]\n",
    );
    let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("lenient load");
    memstead_schema::check_section_formats(&schema).expect_err("table without table must fail");

    // item_pattern without content → refuse.
    let t = minimal_type().replace(
        "  - key: body\n    heading: Body\n    required: true\n",
        "  - key: body\n    heading: Body\n    required: true\n    item_pattern: '.+'\n",
    );
    let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("lenient load");
    memstead_schema::check_section_formats(&schema).expect_err("pattern without content must fail");

    // column_patterns naming an undeclared column → refuse.
    let t = minimal_type().replace(
        "  - key: body\n    heading: Body\n    required: true\n",
        "  - key: body\n    heading: Body\n    required: true\n    content: \"table\"\n    table:\n      columns: [Name]\n      column_patterns:\n        Datum: '\\d+'\n",
    );
    let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("lenient load");
    memstead_schema::check_section_formats(&schema).expect_err("unknown column must fail");
}

/// Reserved heading depths refuse at load through the expression
/// parser (criterion 4's load half).
#[test]
fn section_format_heading_depth_one_and_two_refuse_at_load() {
    for depth in ["1", "2"] {
        let t = minimal_type().replace(
            "  - key: body\n    heading: Body\n    required: true\n",
            &format!(
                "  - key: body\n    heading: Body\n    required: true\n    content: \"heading({depth}) paragraph\"\n"
            ),
        );
        let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("lenient load");
        let err = memstead_schema::check_section_formats(&schema).expect_err("install must refuse");
        assert!(
            err.to_string().contains("3–6"),
            "names the reserved range: {err}"
        );
    }
}

/// The builtin `planning` bump (plan 08, criterion 8): 0.1.0 stays
/// untouched; 0.2.0 declares `content: "list(bullet)"` on the
/// bullet-prescribing sections, compiled at load.
#[test]
fn builtin_planning_bump_declares_bullet_lists() {
    let schemas = memstead_schema::builtins::load_builtin_schemas().expect("builtins load");
    let planning: Vec<_> = schemas
        .iter()
        .filter(|s| s.manifest.name == "planning")
        .collect();
    let versions: Vec<String> = planning.iter().map(|s| s.version.to_string()).collect();
    assert!(versions.contains(&"0.1.0".to_string()), "{versions:?}");
    assert!(versions.contains(&"0.2.0".to_string()), "{versions:?}");

    let v1 = planning
        .iter()
        .find(|s| s.version.to_string() == "0.1.0")
        .unwrap();
    let v2 = planning
        .iter()
        .find(|s| s.version.to_string() == "0.2.0")
        .unwrap();
    let pros_v1 = &v1.types["option"]
        .sections
        .iter()
        .find(|s| s.key == "pros")
        .unwrap();
    assert!(pros_v1.content.is_none(), "0.1.0 stays undeclared");
    // Required sections demand the list outright; optional sections
    // declare the `?` form so omission stays legal (absent-as-empty:
    // a bare `list(bullet)` on a `required: false` section would
    // contradict the flag by omission-refusing).
    for (ty, key, expected) in [
        ("option", "pros", "list(bullet)"),
        ("option", "cons", "list(bullet)"),
        ("goal", "in_scope", "list(bullet)?"),
        ("goal", "out_of_scope", "list(bullet)?"),
        ("decision", "consequences", "list(bullet)"),
        ("risk", "mitigations", "list(bullet)?"),
    ] {
        let section = v2.types[ty].sections.iter().find(|s| s.key == key).unwrap();
        assert_eq!(section.content.as_deref(), Some(expected), "{ty}.{key}");
        assert!(section.compiled_content.is_some(), "{ty}.{key} compiled");
        assert!(section.format_problems.is_empty());
        assert_eq!(
            section.required,
            expected == "list(bullet)",
            "{ty}.{key}: omission-admitting form iff optional"
        );
    }
}

/// Gap fixes after the plan-08 grading round: a lone
/// `format_severity: warn` (no `content`) is a declaration and must
/// not load silently ignored; and the install refusal aggregates
/// EVERY defective section across types, not the first only.
#[test]
fn section_format_lone_severity_and_cross_section_aggregation_refuse() {
    // Lone warn severity — detectable (block alone equals the default).
    let t = minimal_type().replace(
        "  - key: body\n    heading: Body\n    required: true\n",
        "  - key: body\n    heading: Body\n    required: true\n    format_severity: warn\n",
    );
    let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("lenient load");
    memstead_schema::check_section_formats(&schema).expect_err("lone severity must refuse");

    // Two defective sections: both named in ONE refusal.
    let t = minimal_type().replace(
        "  - key: body\n    heading: Body\n    required: true\n",
        "  - key: extra\n    heading: Extra\n    required: false\n    search_weight: 1.0\n    catch_all: false\n    write_rules: []\n    content: \"nope\"\n  - key: body\n    heading: Body\n    required: true\n    content: \"alsonope\"\n",
    );
    let schema = load(&minimal_manifest(), &[("sample", &t)]).expect("lenient load");
    let err = memstead_schema::check_section_formats(&schema).expect_err("must refuse");
    let msg = err.to_string();
    assert!(msg.contains("nope"), "{msg}");
    assert!(
        msg.contains("alsonope"),
        "second section's problem named too: {msg}"
    );
}

/// Agent-trust plan 06: the retired `propagating_relationships` key
/// refuses at AUTHORING load (directory context) with the typed rename
/// error naming the new key; sealed content (in-memory: built-ins,
/// installed refs) loads with the old key translated so shipped
/// versions keep serving — install-time strict, sealed-tolerant.
#[test]
fn retired_propagating_relationships_key_refuses_authoring_and_translates_sealed() {
    use memstead_schema::SchemaLoadError;

    let manifest = r#"name: oldkey
version: 0.1.0
description: retired-key test schema
when_to_use: tests
types:
  - thing
relationships:
  mode: strict
  definitions:
    - name: SUPERSEDES
      description: s
      default_weight: 1.0
    - name: PART_OF
      description: h
      default_weight: 1.0
      acyclic: true
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let type_yaml = "name: thing\ndescription: t\nwhen_to_use: h\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\npropagating_relationships: [SUPERSEDES]\nupdatable_fields:\n  - title\nhealth_required_fields: []\nstaleness_threshold_days: 90\nwrite_rules: []\n";

    // Sealed context (in-memory): loads, value TRANSLATED to the new
    // field.
    let sealed = memstead_schema::load_schema_from_memory(
        manifest,
        &[("thing".to_string(), type_yaml.to_string())],
    )
    .expect("sealed content keeps loading with the old key translated");
    assert_eq!(
        sealed
            .types
            .get("thing")
            .unwrap()
            .no_self_loop_relationships,
        vec!["SUPERSEDES".to_string()],
        "the old key's value survives translation"
    );

    // Authoring context (directory): refuses with the rename error
    // naming the new key.
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("types")).unwrap();
    std::fs::write(tmp.path().join("schema.yaml"), manifest).unwrap();
    std::fs::write(tmp.path().join("types").join("thing.yaml"), type_yaml).unwrap();
    let err = memstead_schema::load_schema_from_dir(tmp.path())
        .expect_err("authoring load refuses the retired key");
    match &err {
        SchemaLoadError::PropagatingRelationshipsRenamed { type_name } => {
            assert_eq!(type_name, "thing");
        }
        other => panic!("expected the rename refusal, got {other:?}"),
    }
    assert!(
        err.to_string().contains("no_self_loop_relationships"),
        "refusal names the new key: {err}"
    );
}

/// Sealed built-ins that ship the old key serve payload values under
/// the NEW name — the translation preserves the declared lists (the
/// engineering@0.1.0 decision type is the live example).
#[test]
fn sealed_builtin_old_key_values_survive_translation() {
    let reg = memstead_schema::SchemaRegistry::builtin();
    let engineering = reg
        .get("engineering", &semver::Version::new(0, 1, 0))
        .expect("sealed engineering@0.1.0 keeps loading");
    let decision = engineering.types.get("decision").expect("decision type");
    assert!(
        decision
            .no_self_loop_relationships
            .contains(&"SUPERSEDES".to_string()),
        "sealed old-key values reach the renamed field: {:?}",
        decision.no_self_loop_relationships
    );
}

// ---------------------------------------------------------------------------
// Report-all accumulation — every semantic violation in one refusal
// ---------------------------------------------------------------------------

fn two_type_manifest() -> String {
    minimal_manifest().replace("types:\n  - sample\n", "types:\n  - sample\n  - extra\n")
}

/// `hierarchy_relationship` names an undeclared rel-type.
fn broken_sample() -> String {
    minimal_type().replace(
        "hierarchy_relationship: PART_OF",
        "hierarchy_relationship: BELONGS_TO",
    )
}

/// Two independent violations: `text_fields` and
/// `health_required_fields` each reference a key that does not exist.
fn broken_extra() -> String {
    minimal_type()
        .replace("name: sample", "name: extra")
        .replace(
            "text_fields:\n  - body",
            "text_fields:\n  - missing_section",
        )
        .replace(
            "health_required_fields:\n  - body",
            "health_required_fields:\n  - absent_field",
        )
}

#[test]
fn three_violations_across_two_type_files_refuse_once_naming_all() {
    let err = load(
        &two_type_manifest(),
        &[("sample", &broken_sample()), ("extra", &broken_extra())],
    )
    .expect_err("must fail");
    let SchemaLoadError::Multiple { errors } = &err else {
        panic!("expected Multiple, got: {err}");
    };
    assert_eq!(errors.len(), 3, "got: {err}");
    assert!(
        errors.iter().any(|e| matches!(e,
            SchemaLoadError::UndeclaredRelationship { relationship, .. } if relationship == "BELONGS_TO")),
        "got: {err}"
    );
    assert!(
        errors.iter().any(|e| matches!(e,
            SchemaLoadError::UnknownFieldReference { field: "text_fields", reference, .. } if reference == "missing_section")),
        "got: {err}"
    );
    assert!(
        errors.iter().any(|e| matches!(e,
            SchemaLoadError::UnknownFieldReference { field: "health_required_fields", reference, .. } if reference == "absent_field")),
        "got: {err}"
    );
    // The single rendered refusal names all three offenders.
    let msg = err.to_string();
    assert!(msg.contains("BELONGS_TO"), "got: {msg}");
    assert!(msg.contains("missing_section"), "got: {msg}");
    assert!(msg.contains("absent_field"), "got: {msg}");
}

#[test]
fn accumulated_violations_keep_recovery_material() {
    let err = load(
        &two_type_manifest(),
        &[("sample", &broken_sample()), ("extra", &broken_extra())],
    )
    .expect_err("must fail");
    let msg = err.to_string();
    // The undeclared-relationship entry keeps the declared set and the
    // nearest-match suggestion it carries in the single-violation form.
    assert!(msg.contains("Available: ["), "got: {msg}");
    assert!(msg.contains("Did you mean"), "got: {msg}");
}

#[test]
fn single_violation_stays_bare_not_a_one_element_list() {
    let err = load(&minimal_manifest(), &[("sample", &broken_sample())]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::UndeclaredRelationship { .. }),
        "got: {err}"
    );
    assert!(
        !err.to_string().contains("violations:"),
        "single violation must not render as a list: {err}"
    );
}

#[test]
fn unparseable_manifest_short_circuits_before_semantic_checks() {
    // Both the manifest and a type file carry problems — only the
    // manifest parse error reports; nothing derives from a half-read
    // structure.
    let err = load("name: [unclosed", &[("sample", &broken_sample())]).expect_err("must fail");
    assert!(
        matches!(err, SchemaLoadError::ParseManifest { .. }),
        "got: {err}"
    );
}

#[test]
fn type_parse_failure_reports_alongside_other_files_violations() {
    let err = load(
        &two_type_manifest(),
        &[("sample", &broken_sample()), ("extra", "name: [unclosed")],
    )
    .expect_err("must fail");
    let SchemaLoadError::Multiple { errors } = &err else {
        panic!("expected Multiple, got: {err}");
    };
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaLoadError::ParseType { .. })),
        "got: {err}"
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaLoadError::UndeclaredRelationship { .. })),
        "got: {err}"
    );
}

#[test]
fn manifest_and_type_violations_accumulate_together() {
    // A manifest-level violation (undeclared source type on a rel
    // definition) and a type-level violation report in one refusal.
    let manifest = minimal_manifest().replace(
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n",
        "    - name: PART_OF\n      description: Hierarchical containment\n      default_weight: 3.0\n      source_types: [ghost]\n",
    );
    let err = load(&manifest, &[("sample", &broken_sample())]).expect_err("must fail");
    let SchemaLoadError::Multiple { errors } = &err else {
        panic!("expected Multiple, got: {err}");
    };
    assert!(
        errors.iter().any(|e| matches!(e,
            SchemaLoadError::UndeclaredRelationshipType { reference, .. } if reference == "ghost")),
        "got: {err}"
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaLoadError::UndeclaredRelationship { .. })),
        "got: {err}"
    );
}

/// Accumulated refusals must render identically across runs on the
/// same input — the CI schema check and built-in schema tests compare
/// output. Exercised through `load_schema_from_dir` (the entry the
/// `validate` and `install` surfaces share), whose directory scan is
/// explicitly sorted.
#[test]
fn accumulated_order_is_deterministic_from_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir(root.join("types")).unwrap();
    std::fs::write(root.join("schema.yaml"), two_type_manifest()).unwrap();
    std::fs::write(root.join("types/sample.yaml"), broken_sample()).unwrap();
    std::fs::write(root.join("types/extra.yaml"), broken_extra()).unwrap();

    let first = memstead_schema::loader::load_schema_from_dir(root)
        .expect_err("must fail")
        .to_string();
    let second = memstead_schema::loader::load_schema_from_dir(root)
        .expect_err("must fail")
        .to_string();
    assert_eq!(first, second);
    assert!(first.contains("3 violations"), "got: {first}");
}

// ---------------------------------------------------------------------------
// Metadata-required polarity (first-author-path plan 07)
// ---------------------------------------------------------------------------

/// Old-format sealed fixture — written in the retired language, loaded
/// through the legacy memory path (no format marker). `optional:
/// false` still refuses absence (required), `optional: true` still
/// admits it, and a field carrying NEITHER key reads as required —
/// its written meaning under the old rule.
#[test]
fn sealed_old_format_fixture_loads_with_inverted_equivalent_semantics() {
    let manifest = minimal_manifest();
    let old_format_type = r#"name: sample
description: Sealed old-format fixture
when_to_use: Polarity tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: explicit_mandatory
    description: optional false meant required
    field_type: string
    optional: false
  - key: explicit_optional
    description: optional true meant not required
    field_type: string
    optional: true
  - key: absent_key
    description: absence meant required under the old rule
    field_type: string
title_weight: 100.0
text_fields: [body]
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields: [title, body]
health_required_fields: []
staleness_threshold_days: 90
write_rules: []
"#;
    let schema = load_schema_from_memory(&manifest, &[("sample".into(), old_format_type.into())])
        .expect("sealed old-format fixture keeps loading");
    let td = schema.get_type("sample").unwrap();
    let req = |key: &str| td.metadata_field(key).unwrap().is_required();
    assert!(req("explicit_mandatory"), "optional: false stays required");
    assert!(!req("explicit_optional"), "optional: true stays optional");
    assert!(
        req("absent_key"),
        "unmarked sealed package: absence keeps its legacy written meaning (required)"
    );
}

/// The format marker decides the absent-key reading — the identical
/// declaration in a marked (new-format) package reads as optional,
/// never a heuristic over the document body.
#[test]
fn format_marker_decides_absent_key_reading() {
    use memstead_schema::loader::{MetadataPolarityFormat, load_schema_from_memory_with_format};
    let manifest = minimal_manifest();
    let bare_field_type = r#"name: sample
description: Marker-decided fixture
when_to_use: Polarity tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: bare
    description: no required key
    field_type: string
title_weight: 100.0
text_fields: [body]
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields: [title, body]
health_required_fields: []
staleness_threshold_days: 90
write_rules: []
"#;
    let types = [("sample".to_string(), bare_field_type.to_string())];
    let legacy =
        load_schema_from_memory_with_format(&manifest, &types, MetadataPolarityFormat::Legacy)
            .unwrap();
    let marked = load_schema_from_memory_with_format(
        &manifest,
        &types,
        MetadataPolarityFormat::RequiredOptIn,
    )
    .unwrap();
    assert!(
        legacy
            .get_type("sample")
            .unwrap()
            .metadata_field("bare")
            .unwrap()
            .is_required(),
        "legacy: absence means required"
    );
    assert!(
        !marked
            .get_type("sample")
            .unwrap()
            .metadata_field("bare")
            .unwrap()
            .is_required(),
        "marked: absence means optional"
    );
}

/// Authoring/directory loads refuse the retired `optional:` key with
/// the inversion instructions, and mixing several offenders reports
/// them all through the shared report-all accumulation.
#[test]
fn authoring_refuses_retired_optional_key_naming_the_inversion() {
    let manifest = minimal_manifest();
    let mixed_type = minimal_type().replace(
        "metadata_fields:\n  - key: status",
        "metadata_fields:\n  - key: extra\n    description: retired key user\n    field_type: string\n    optional: true\n  - key: extra2\n    description: second offender\n    field_type: string\n    optional: false\n  - key: status",
    );
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("types")).unwrap();
    std::fs::write(root.join("schema.yaml"), &manifest).unwrap();
    std::fs::write(root.join("types/sample.yaml"), &mixed_type).unwrap();
    let err = memstead_schema::loader::load_schema_from_dir(root).expect_err("must refuse");
    let msg = err.to_string();
    assert!(
        msg.contains("retired `optional:` key"),
        "names the retirement: {msg}"
    );
    assert!(
        msg.contains("`required: true`") && msg.contains("delete `optional: true`"),
        "states the executable inversion: {msg}"
    );
    // Both offenders in one refusal — the shared report-all path.
    assert!(
        msg.contains("'extra'") && msg.contains("'extra2'"),
        "report-all accumulation names every offender: {msg}"
    );
}

/// A schema declaring a metadata field with no `required` key
/// validates on the authoring path, and a section without `required`
/// is legal and optional — the uniform absence-means-optional rule.
#[test]
fn absence_means_optional_for_fields_and_sections_on_authoring_path() {
    let manifest = minimal_manifest();
    let bare = minimal_type()
        .replace(
            "metadata_fields:\n  - key: status",
            "metadata_fields:\n  - key: note_source\n    description: bare field\n    field_type: string\n  - key: status",
        )
        .replace(
            "  - key: body\n    heading: Body\n    required: true",
            "  - key: extra_notes\n    heading: Extra Notes\n    search_weight: 1.0\n    write_rules: []\n  - key: body\n    heading: Body\n    required: true",
        );
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("types")).unwrap();
    std::fs::write(root.join("schema.yaml"), &manifest).unwrap();
    std::fs::write(root.join("types/sample.yaml"), &bare).unwrap();
    let schema = memstead_schema::loader::load_schema_from_dir(root).expect("bare keys validate");
    let td = schema.get_type("sample").unwrap();
    assert!(!td.metadata_field("note_source").unwrap().is_required());
    let extra = td.sections.iter().find(|s| s.key == "extra_notes").unwrap();
    assert!(!extra.required, "section without required key is optional");
}

// ---------------------------------------------------------------------------
// Due-axis declaration validation (first-author-path plan 08)
// ---------------------------------------------------------------------------

fn due_type(due_block: &str) -> String {
    format!(
        "name: sample\ndescription: t\nwhen_to_use: due tests\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\n  - key: vorlauf\n    heading: Vorlauf\n    search_weight: 1.0\n    write_rules: []\nmetadata_fields:\n  - key: faellig_am\n    description: due date\n    field_type: date\n  - key: status\n    description: state\n    field_type: string\n    enum_values: [offen, erledigt]\n  - key: freitext\n    description: plain string\n    field_type: string\n{due_block}title_weight: 100.0\ntext_fields: [body]\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields: [title, body]\nhealth_required_fields: []\nstaleness_threshold_days: 90\nwrite_rules: []\n"
    )
}

/// A well-formed declaration loads; a schema without `due:` loads and
/// carries no axis — behaves exactly as today.
#[test]
fn due_axis_well_formed_loads_and_absence_is_inert() {
    let ok = due_type(
        "due:\n  date_field: faellig_am\n  status_field: status\n  open_values: [offen]\n  lead_section: vorlauf\n",
    );
    let schema = load(&minimal_manifest(), &[("sample", &ok)]).expect("well-formed due loads");
    let td = schema.get_type("sample").unwrap();
    let due = td.due.as_ref().unwrap();
    assert_eq!(due.date_field, "faellig_am");
    assert_eq!(due.lead_section.as_deref(), Some("vorlauf"));

    let none = due_type("");
    let schema = load(&minimal_manifest(), &[("sample", &none)]).expect("no due loads");
    assert!(schema.get_type("sample").unwrap().due.is_none());
}

/// Every malformed reference refuses at load with the recovery
/// quality, and several defects report together through the shared
/// accumulation.
#[test]
fn due_axis_malformed_references_refuse_with_accumulation() {
    // Missing date field + non-enum status + undeclared open value +
    // bad lead section, all at once.
    let bad = due_type(
        "due:\n  date_field: nonexistent\n  status_field: freitext\n  open_values: [offen, unbekannt]\n  lead_section: nosuch\n",
    );
    let err = load(&minimal_manifest(), &[("sample", &bad)]).expect_err("must refuse");
    let msg = err.to_string();
    assert!(msg.contains("violations"), "accumulated: {msg}");
    assert!(msg.contains("'nonexistent'"), "{msg}");
    assert!(
        msg.contains("`status_field` must name an enum-typed metadata field"),
        "{msg}"
    );
    assert!(msg.contains("'nosuch'"), "{msg}");

    // Non-date date_field refuses alone with the shape rule.
    let non_date =
        due_type("due:\n  date_field: freitext\n  status_field: status\n  open_values: [offen]\n");
    let err = load(&minimal_manifest(), &[("sample", &non_date)]).expect_err("must refuse");
    assert!(
        err.to_string()
            .contains("`date_field` must name a date-typed metadata field"),
        "{err}"
    );

    // Undeclared open value names the enum.
    let bad_value = due_type(
        "due:\n  date_field: faellig_am\n  status_field: status\n  open_values: [unbekannt]\n",
    );
    let err = load(&minimal_manifest(), &[("sample", &bad_value)]).expect_err("must refuse");
    let msg = err.to_string();
    assert!(
        msg.contains("`open_values` entry is not in `status`'s enum_values [offen, erledigt]"),
        "{msg}"
    );
}
