//! Emit JSON Schemas for schema-author IDE validation.
//!
//! Run from the repo root:
//!
//! ```text
//! cargo run --bin emit_json_schemas
//! ```
//!
//! Writes two files under `crates/memstead-schema/generated/` that the
//! `json_schemas.rs` drift test compares against what `schemars` would emit
//! today. Regenerate after any change to `TypeDefinition` or `SchemaManifest`.

use memstead_schema::manifest::SchemaManifest;
use memstead_schema::types::TypeDefinition;

fn main() {
    let type_schema = schemars::schema_for!(TypeDefinition);
    let manifest_schema = schemars::schema_for!(SchemaManifest);

    let out_dir = "crates/memstead-schema/generated";
    std::fs::create_dir_all(out_dir).expect("create generated/");

    let type_path = format!("{out_dir}/type-definition.schema.json");
    let manifest_path = format!("{out_dir}/schema-manifest.schema.json");

    std::fs::write(
        &type_path,
        serde_json::to_string_pretty(&type_schema).expect("serialize type schema"),
    )
    .expect("write type schema");
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest_schema).expect("serialize manifest schema"),
    )
    .expect("write manifest schema");

    println!("wrote {type_path}");
    println!("wrote {manifest_path}");
}
