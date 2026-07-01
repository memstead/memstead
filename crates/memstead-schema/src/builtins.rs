//! Embedded builtin schemas, baked into the binary via `include_dir!`.
//!
//! Every directory under `builtins/schemas/` is loaded as a first-class schema
//! and registered in the default `SchemaRegistry`. Ships `default` (the legacy
//! 10-knowledge-type bundle) plus domain-specific schemas (`ingest`,
//! `planning`, `project`, `software`) that vaults may pin via
//! `schema = "<name>@<version>"` in their per-vault config.

use std::sync::Arc;

use include_dir::{Dir, include_dir};

use crate::loader::{self, SchemaLoadError};
use crate::schema::Schema;

static BUILTIN_SCHEMAS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/builtins/schemas");

/// Access to the embedded `builtins/schemas` directory.
///
/// Exposed so the source-collection path
/// (`crate::source::collect_schema_source`) can lift raw YAML bytes out
/// of the binary when a vault pins a builtin schema but has no
/// workspace or cache copy.
pub(crate) fn builtin_schemas_dir() -> &'static Dir<'static> {
    &BUILTIN_SCHEMAS
}

/// Read a built-in schema package's optional `vault-template.json` —
/// the `VaultConfig` starter a client (`memstead vault create`, the
/// planning skills) fills and passes through the `write_guidance`
/// create-parameter. Returns the parsed JSON object when the package
/// ships a template, `None` when it does not (`default`, `ingest`) or
/// when `name` is not a built-in.
///
/// The template is opaque to the engine (schema-strictness D8): it is
/// surfaced verbatim for the client to fill `<REQUIRED: …>`
/// placeholders. Loaded from the embedded `builtins/schemas/` tree so a
/// vault pinning a built-in resolves its template without a workspace
/// or cache copy, mirroring [`builtin_schemas_dir`].
pub fn builtin_vault_template(name: &str) -> Option<serde_json::Value> {
    let file = BUILTIN_SCHEMAS.get_file(format!("{name}/vault-template.json").as_str())?;
    serde_json::from_slice(file.contents()).ok()
}

/// Load every embedded schema into owned `Schema` values.
pub fn load_builtin_schemas() -> Result<Vec<Arc<Schema>>, SchemaLoadError> {
    let mut out = Vec::new();
    for dir in BUILTIN_SCHEMAS.dirs() {
        let schema = load_builtin_dir(dir)?;
        out.push(Arc::new(schema));
    }
    Ok(out)
}

fn load_builtin_dir(dir: &Dir<'_>) -> Result<Schema, SchemaLoadError> {
    let manifest_file = dir.get_file(
        format!("{}/schema.yaml", dir.path().display()).as_str(),
    );
    let manifest_text = manifest_file
        .and_then(|f| f.contents_utf8())
        .ok_or_else(|| SchemaLoadError::Io {
            path: dir.path().join("schema.yaml"),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "embedded schema.yaml missing or not utf-8",
            ),
        })?;

    let mut types: Vec<(String, String)> = Vec::new();
    let types_path = format!("{}/types", dir.path().display());
    if let Some(types_dir) = dir.get_dir(types_path.as_str()) {
        for file in types_dir.files() {
            if file.path().extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            let Some(stem) = file
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            let Some(contents) = file.contents_utf8() else {
                return Err(SchemaLoadError::Io {
                    path: file.path().to_path_buf(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "embedded type yaml is not utf-8",
                    ),
                });
            };
            types.push((stem, contents.to_string()));
        }
    }

    loader::load_schema_from_memory(manifest_text, &types)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three scaffolding-bearing built-ins ship a parseable
    /// `vault-template.json` carrying their instance writeGuidance key;
    /// the deprecated literal `goal`/`avoid` (now in the schema's
    /// `default_writing_guidance`) must NOT be present.
    #[test]
    fn builtin_vault_templates_carry_instance_keys_only() {
        let cases = [
            ("planning", "phase_context"),
            ("project", "scope"),
            ("software", "stack"),
        ];
        for (name, instance_key) in cases {
            let tpl = builtin_vault_template(name)
                .unwrap_or_else(|| panic!("{name} must ship a vault-template.json"));
            assert!(tpl["language"].is_string(), "{name}: template carries language");
            let wg = &tpl["writeGuidance"];
            assert!(
                wg.get(instance_key).is_some(),
                "{name}: template carries instance key {instance_key}",
            );
            assert!(
                wg.get("goal").is_none() && wg.get("avoid").is_none(),
                "{name}: template must not carry the deprecated literal goal/avoid (schema owns those)",
            );
        }
    }

    /// Packages without a template (and unknown names) resolve to None.
    #[test]
    fn builtin_vault_template_absent_is_none() {
        assert!(builtin_vault_template("default").is_none());
        assert!(builtin_vault_template("ingest").is_none());
        assert!(builtin_vault_template("not-a-builtin").is_none());
    }

    /// The added template files are inert to schema loading — every
    /// built-in still loads (the loader reads only schema.yaml + types/).
    #[test]
    fn all_builtins_still_load_with_templates_present() {
        let schemas = load_builtin_schemas().expect("built-ins load");
        assert!(
            schemas.iter().any(|s| s.manifest.name == "planning"),
            "planning still loads alongside its vault-template.json",
        );
    }
}
