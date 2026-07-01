//! Deterministic re-packing of a validated archive.
//!
//! LF line endings, canonical-form markdown (via
//! `entity::generator::generate_markdown`), custom canonical JSON for
//! the config (sorted keys, two-space indent), and a zip with sorted
//! entries, fixed mtime (1980-01-01), DEFLATE level 6. Pinned by
//! golden-file tests so any serialization change breaks CI.

use std::io::{Cursor, Write};
use std::sync::Arc;

use memstead_schema::{ARCHIVE_CONFIG_PATH, PublishedVaultConfig, Schema, type_by_name};
use zip::CompressionMethod;
use zip::DateTime;
use zip::write::SimpleFileOptions;

use super::ValidationError;
use super::archive::SchemaFile;
use crate::entity::generator::generate_markdown;
use crate::entity::{Entity, id::id_to_file_path};

/// Take the validated entities + config and produce a deterministic
/// zip. Markdown is regenerated via `generate_markdown` (already
/// schema-ordered and parse-stable); config is serialized via the
/// custom canonical JSON formatter in this module; embedded schema
/// files are written back verbatim (they were CRLF-normalized at
/// archive-extract time). Every entry is emitted in sorted path order
/// with fixed mtime so two semantically equal archives produce
/// identical bytes.
pub fn canonical_bytes(
    config: &PublishedVaultConfig,
    entities: &[Entity],
    schema_files: &[SchemaFile],
    embedded_schema: Option<&Arc<Schema>>,
    provenance_bytes: Option<&[u8]>,
) -> Result<Vec<u8>, ValidationError> {
    let mut files: Vec<(String, Vec<u8>)> =
        Vec::with_capacity(entities.len() + schema_files.len() + 2);

    let config_text = canonical_json(config)?;
    files.push((ARCHIVE_CONFIG_PATH.to_string(), config_text.into_bytes()));

    // Preserve the optional authoring-provenance payload verbatim so a
    // normalized archive round-trips it (normalize must not silently drop
    // provenance). Carried as raw bytes — canonical re-pack does not
    // reinterpret the payload; the producer wrote it canonically.
    if let Some(prov) = provenance_bytes {
        files.push((
            memstead_schema::ARCHIVE_PROVENANCE_PATH.to_string(),
            prov.to_vec(),
        ));
    }

    // Schema files ride along verbatim. The on-extract CRLF→LF pass
    // guarantees idempotency: a canonical archive re-validated produces
    // byte-identical canonical bytes. Resorting happens below.
    for sf in schema_files {
        files.push((sf.archive_path.clone(), sf.content.clone().into_bytes()));
    }

    for entity in entities {
        // Prefer the archive-embedded schema so user-defined types
        // regenerate against the publisher's own rules, not the
        // builtin-default table.
        let schema = embedded_schema
            .and_then(|s| s.get_type(&entity.entity_type))
            .or_else(|| type_by_name(&entity.entity_type))
            .ok_or_else(|| {
                ValidationError::GraphConstructionFailed(format!(
                    "entity {} references unresolved type {:?}",
                    entity.id.as_ref(),
                    entity.entity_type
                ))
            })?;
        let md = generate_markdown(entity, &schema);
        let lf = normalize_lf(&md);
        let path = id_to_file_path(&entity.id);
        files.push((path, lf.into_bytes()));
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut buf = Vec::new();
    {
        let cursor = Cursor::new(&mut buf);
        let mut writer = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(6))
            .last_modified_time(DateTime::default());

        for (path, content) in files {
            writer
                .start_file(&path, options)
                .map_err(|e| ValidationError::GraphConstructionFailed(e.to_string()))?;
            writer
                .write_all(&content)
                .map_err(|e| ValidationError::GraphConstructionFailed(e.to_string()))?;
        }
        writer
            .finish()
            .map_err(|e| ValidationError::GraphConstructionFailed(e.to_string()))?;
    }

    Ok(buf)
}

fn normalize_lf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// Serialize a `PublishedVaultConfig` as canonical JSON: sorted keys,
/// two-space indent, LF line endings, trailing `\n`. Uses
/// serde_json's default string-escape rules (so behavior matches the
/// parser's tolerance).
pub fn canonical_json(config: &PublishedVaultConfig) -> Result<String, ValidationError> {
    let value = serde_json::to_value(config)
        .map_err(|e| ValidationError::InvalidConfig { reason: e.to_string() })?;
    let mut out = String::new();
    write_canonical(&value, &mut out, 0);
    out.push('\n');
    Ok(out)
}

fn write_canonical(value: &serde_json::Value, out: &mut String, indent: usize) {
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        serde_json::Value::Number(n) => out.push_str(&n.to_string()),
        serde_json::Value::String(s) => {
            // Delegate string escaping to serde_json for consistency
            // with every other JSON reader. serde_json::to_string wraps
            // the string in double quotes, which is what we want.
            let escaped = serde_json::to_string(s).expect("string escape infallible");
            out.push_str(&escaped);
        }
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                out.push('\n');
                push_indent(out, indent + 1);
                write_canonical(item, out, indent + 1);
                if i < items.len() - 1 {
                    out.push(',');
                }
            }
            out.push('\n');
            push_indent(out, indent);
            out.push(']');
        }
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                out.push('\n');
                push_indent(out, indent + 1);
                let key_escaped =
                    serde_json::to_string(*key).expect("string escape infallible");
                out.push_str(&key_escaped);
                out.push_str(": ");
                write_canonical(&map[*key], out, indent + 1);
                if i < keys.len() - 1 {
                    out.push(',');
                }
            }
            out.push('\n');
            push_indent(out, indent);
            out.push('}');
        }
    }
}

fn push_indent(out: &mut String, indent: usize) {
    for _ in 0..indent * 2 {
        out.push(' ');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;

    fn config() -> PublishedVaultConfig {
        PublishedVaultConfig {
            format: memstead_schema::PUBLISHED_VAULT_FORMAT,
            name: "example".to_string(),
            version: Version::parse("0.1.0").unwrap(),
            description: Some("a test vault".to_string()),
            authors: Some(vec!["Alice".to_string(), "Bob".to_string()]),
            schema: "default@1.0.0".parse().unwrap(),
        }
    }

    #[test]
    fn canonical_json_is_alpha_sorted() {
        let json = canonical_json(&config()).unwrap();
        // Top-level keys in sorted order: authors, description, format, name, schema, version
        let expected = "{\n  \"authors\": [\n    \"Alice\",\n    \"Bob\"\n  ],\n  \"description\": \"a test vault\",\n  \"format\": 3,\n  \"name\": \"example\",\n  \"schema\": \"default@1.0.0\",\n  \"version\": \"0.1.0\"\n}\n";
        assert_eq!(json, expected);
    }

    #[test]
    fn canonical_json_handles_empty_array_inline() {
        let mut c = config();
        c.authors = Some(Vec::new());
        let json = canonical_json(&c).unwrap();
        assert!(json.contains("\"authors\": []"));
    }

    #[test]
    fn canonical_json_ends_with_lf() {
        let json = canonical_json(&config()).unwrap();
        assert!(json.ends_with('\n'));
        assert!(!json.ends_with("\r\n"));
    }

    #[test]
    fn canonical_json_escapes_quotes_in_strings() {
        let mut c = config();
        c.description = Some("he said \"hi\"".to_string());
        let json = canonical_json(&c).unwrap();
        assert!(json.contains("\"he said \\\"hi\\\"\""));
    }

    #[test]
    fn canonical_bytes_produces_deterministic_output() {
        // Same config + empty entities + empty schema files twice → identical
        // bytes.
        let c = config();
        let a = canonical_bytes(&c, &[], &[], None, None).unwrap();
        let b = canonical_bytes(&c, &[], &[], None, None).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn normalize_lf_converts_crlf() {
        assert_eq!(normalize_lf("a\r\nb\r\nc"), "a\nb\nc");
        assert_eq!(normalize_lf("a\nb\nc"), "a\nb\nc");
        // Lone CR stays; not our concern today.
        assert_eq!(normalize_lf("a\rb"), "a\rb");
    }
}
