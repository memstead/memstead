//! Drift test: the committed JSON Schemas under `generated/` must match what
//! `schemars` currently produces from `TypeDefinition` and `SchemaManifest`.
//!
//! If this test fails, re-run:
//!     cargo run -p memstead-schema --bin emit_json_schemas
//! and commit the updated files.

use memstead_schema::manifest::SchemaManifest;
use memstead_schema::types::TypeDefinition;

#[test]
fn emitted_json_schemas_match_committed() {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/generated/");

    let type_schema = schemars::schema_for!(TypeDefinition);
    let manifest_schema = schemars::schema_for!(SchemaManifest);

    let on_disk_type = std::fs::read_to_string(format!("{base}type-definition.schema.json"))
        .expect("generated/type-definition.schema.json missing — run emit_json_schemas");
    let on_disk_manifest = std::fs::read_to_string(format!("{base}schema-manifest.schema.json"))
        .expect("generated/schema-manifest.schema.json missing — run emit_json_schemas");

    assert_eq!(
        serde_json::to_string_pretty(&type_schema).unwrap().trim(),
        on_disk_type.trim(),
        "type-definition.schema.json drifted — regenerate via emit_json_schemas"
    );
    assert_eq!(
        serde_json::to_string_pretty(&manifest_schema).unwrap().trim(),
        on_disk_manifest.trim(),
        "schema-manifest.schema.json drifted — regenerate via emit_json_schemas"
    );
}
