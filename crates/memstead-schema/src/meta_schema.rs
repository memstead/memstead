//! Embedded JSON Schemas (the "meta-schemas") for authoring validation.
//!
//! `schema-manifest.schema.json` and `type-definition.schema.json` are
//! generated from the Rust schema structs and committed under
//! `generated/`. They are baked into the binary here so the engine can
//! *publish* them into a workspace's `.memstead/meta-schemas/` at boot —
//! a well-known location an authored package's
//! `# yaml-language-server: $schema=…` directive resolves against, giving
//! IDE-side validation as the YAML is edited. `memstead schema install`
//! rewrites a package's directive to the installed-location form so the
//! published copy is what the editor checks against.

use std::path::Path;

/// JSON Schema for a schema package manifest (`schema.yaml`).
pub const META_SCHEMA_MANIFEST: &str = include_str!("../generated/schema-manifest.schema.json");

/// JSON Schema for a type definition (`types/<type>.yaml`).
pub const META_SCHEMA_TYPE_DEFINITION: &str =
    include_str!("../generated/type-definition.schema.json");

/// Directory (under `<workspace>/.memstead/`) the meta-schemas publish to.
pub const META_SCHEMA_DIR: &str = "meta-schemas";

/// Published filename of the manifest meta-schema.
pub const META_SCHEMA_MANIFEST_FILE: &str = "schema-manifest.schema.json";
/// Published filename of the type-definition meta-schema.
pub const META_SCHEMA_TYPE_DEFINITION_FILE: &str = "type-definition.schema.json";

/// Publish the embedded meta-schemas into
/// `<workspace_root>/.memstead/meta-schemas/`. Idempotent — rewrites a
/// file only when its on-disk bytes differ. Best-effort by contract: the
/// engine-boot caller ignores the error so a read-only or
/// permission-restricted workspace still boots; the meta-schemas are an
/// editor convenience, not load-bearing engine state.
pub fn publish_meta_schemas(workspace_root: &Path) -> std::io::Result<()> {
    let dir = workspace_root.join(".memstead").join(META_SCHEMA_DIR);
    write_if_changed(&dir.join(META_SCHEMA_MANIFEST_FILE), META_SCHEMA_MANIFEST)?;
    write_if_changed(
        &dir.join(META_SCHEMA_TYPE_DEFINITION_FILE),
        META_SCHEMA_TYPE_DEFINITION,
    )?;
    Ok(())
}

fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Ok(existing) = std::fs::read_to_string(path)
        && existing == contents
    {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_meta_schemas_are_valid_json_objects() {
        let manifest: serde_json::Value = serde_json::from_str(META_SCHEMA_MANIFEST).unwrap();
        assert!(
            manifest.get("properties").is_some(),
            "manifest meta-schema has properties"
        );
        let type_def: serde_json::Value =
            serde_json::from_str(META_SCHEMA_TYPE_DEFINITION).unwrap();
        assert!(
            type_def.get("properties").is_some(),
            "type meta-schema has properties"
        );
    }

    #[test]
    fn publish_writes_both_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        publish_meta_schemas(tmp.path()).unwrap();
        let mdir = tmp.path().join(".memstead").join("meta-schemas");
        assert_eq!(
            std::fs::read_to_string(mdir.join(META_SCHEMA_MANIFEST_FILE)).unwrap(),
            META_SCHEMA_MANIFEST,
        );
        assert!(mdir.join(META_SCHEMA_TYPE_DEFINITION_FILE).is_file());
        // Second publish is a no-op (bytes unchanged) and still succeeds.
        publish_meta_schemas(tmp.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(mdir.join(META_SCHEMA_MANIFEST_FILE)).unwrap(),
            META_SCHEMA_MANIFEST,
        );
    }
}
