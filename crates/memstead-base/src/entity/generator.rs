//! Entity → Markdown generator. Deterministic output for roundtrip stability.
//!
//! Output order:
//! 1. YAML frontmatter (metadata keys in schema metadata_fields order)
//! 2. `# {title}`
//! 3. Required sections (schema order)
//! 4. Relationships section (if any explicit relations)
//! 5. Optional sections (schema order)

use memstead_schema::{FieldType, TypeDefinition, Serialization};

use super::Entity;

/// Sentinel value emitted into the frontmatter for required date
/// fields the caller did not supply and the schema does not
/// auto-fill. Reads cleanly as "obviously a placeholder" to humans
/// and agents (epoch zero in ISO 8601) and never gets mistaken for a
/// real claim.
pub const MISSING_DATE_SENTINEL: &str = "1970-01-01";

/// Generate markdown from a structured entity.
///
/// Invariant: `parse(generate(parse(md))) == parse(md)` — field order, spacing,
/// and metadata serialization are deterministic.
pub fn generate_markdown(entity: &Entity, schema: &TypeDefinition) -> String {
    let mut parts = Vec::new();

    // YAML frontmatter
    let metadata_str = build_metadata(entity, schema);
    parts.push(format!("---\n{metadata_str}\n---"));

    // Title
    parts.push(format!("# {}", entity.title));

    // Required sections (schema order)
    for s in schema.sections.iter().filter(|s| s.required) {
        let content = entity.sections.get(s.key.as_str()).map(|v| v.as_str()).unwrap_or("");
        parts.push(format!("## {}\n{content}", s.heading));
    }

    // Relationships section (between required and optional).
    // Cross-vault relations render as `[[<vault>:<slug>]]` so the
    // wiki-link round-trips through the parser (which interprets
    // bare-slug `[[<slug>]]` as same-vault). Pre-fix the renderer
    // always emitted `[[<slug>]]`, breaking the round-trip for
    // cross-vault relations — the parser would land them in the
    // entity's own vault and the in-memory edge would drift from
    // the on-disk intent.
    if !entity.relationships.is_empty() {
        let rel_lines: Vec<String> = entity
            .relationships
            .iter()
            .map(|r| {
                let link = if r.target.vault() == entity.vault {
                    r.target.path().to_string()
                } else {
                    format!("{}:{}", r.target.vault(), r.target.path())
                };
                // Em-dash form when the relation carries a per-edge
                // description; canonical delimiter is the three-byte
                // ` — ` (space + U+2014 + space). Empty / whitespace-only
                // descriptions are normalised to `None` at every
                // mutation entry, so `Some("")` should never reach the
                // renderer — if it does we still avoid emitting a bare
                // em-dash by treating empty-after-trim as the simple form.
                match r.description.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    Some(text) => format!("- **{}**: [[{link}]] \u{2014} {text}", r.rel_type),
                    None => format!("- **{}**: [[{link}]]", r.rel_type),
                }
            })
            .collect();
        parts.push(format!("## Relationships\n{}", rel_lines.join("\n")));
    }

    // Optional sections (schema order)
    for s in schema.sections.iter().filter(|s| !s.required) {
        let content = entity.sections.get(s.key.as_str()).map(|v| v.as_str()).unwrap_or("");
        // All sections get the same spacing for roundtrip stability
        parts.push(format!("## {}\n\n{content}", s.heading));
    }

    parts.join("\n\n") + "\n"
}

/// Build YAML frontmatter metadata string.
/// Keys are emitted in the order defined by schema.metadata_fields.
fn build_metadata(entity: &Entity, schema: &TypeDefinition) -> String {
    let mut lines = Vec::new();

    for field_def in &schema.metadata_fields {
        let value = entity.metadata.get(field_def.key.as_str());

        // Optional fields: skip if absent
        if field_def.optional && value.is_none() {
            continue;
        }

        // omit-when-falsy: only emit when truthy
        if field_def.serialization == Serialization::OmitWhenFalsy {
            match value {
                Some(v) if !v.is_falsy() => {}
                _ => continue,
            }
        }

        let formatted = match field_def.field_type {
            FieldType::Date => {
                // Schema-managed timestamps (`init_timestamp` /
                // `auto_timestamp`) are written into metadata by the
                // create / update flow before the generator runs, so
                // `value.is_some()` for those. When the value is absent
                // here, the field is required, the caller did not
                // supply it, and the schema does not auto-fill it —
                // `MISSING_REQUIRED_FIELD` already warned. A fallback of
                // today's date would be indistinguishable from a
                // real "set today" claim, so use a clearly-unreal
                // sentinel (`1970-01-01`) so an agent or reviewer
                // reading the frontmatter sees the placeholder
                // immediately. Schema-declared `default_value` (when
                // set) wins over the sentinel — the schema's choice
                // is authoritative.
                let val = value
                    .map(|v| v.to_frontmatter_string())
                    .unwrap_or_else(|| {
                        field_def
                            .default_value
                            .as_deref()
                            .unwrap_or(MISSING_DATE_SENTINEL)
                            .to_string()
                    });
                format!("{}: {val}", field_def.key)
            }
            FieldType::Boolean => {
                let val = value
                    .map(|v| v.to_frontmatter_string())
                    .unwrap_or_else(|| field_def.default_value.as_deref().unwrap_or("false").to_string());
                format!("{}: {val}", field_def.key)
            }
            FieldType::Number => {
                let val = value
                    .map(|v| v.to_frontmatter_string())
                    .unwrap_or_else(|| field_def.default_value.as_deref().unwrap_or("0").to_string());
                format!("{}: {val}", field_def.key)
            }
            FieldType::String => {
                if field_def.serialization == Serialization::CsvArray {
                    // csv-array: emit as comma-separated
                    let val = value.map(|v| v.to_frontmatter_string()).unwrap_or_default();
                    format!("{}: {}", field_def.key, quote_if_ambiguous(&val))
                } else {
                    let val = value
                        .map(|v| v.to_frontmatter_string())
                        .unwrap_or_else(|| field_def.default_value.as_deref().unwrap_or("").to_string());
                    format!("{}: {}", field_def.key, quote_if_ambiguous(&val))
                }
            }
        };

        lines.push(formatted);
    }

    lines.join("\n")
}

/// YAML-quote a string value when emitting it unquoted would make the
/// tolerant parser re-type it (as bool/int/float) on the next read.
///
/// Round-trip protection for `FieldType::String` metadata whose stored
/// value happens to look like another type — e.g. a `temporal_range:
/// "1968"` in source would otherwise be re-emitted as `temporal_range:
/// 1968`, re-parsed as `MetadataValue::Integer(1968)`, and rejected by
/// strict ingress as a String-vs-Integer type mismatch. Quoting forces
/// the tolerant parser's `strip_quotes` branch, which keeps the value
/// in `MetadataValue::String`.
///
/// Prefer double quotes; fall back to single quotes if the value
/// contains `"`. If the value contains both quote styles the tolerant
/// parser's `strip_quotes` cannot escape either, so we leave it
/// unquoted — accepted as a rare lossy corner case, not defended here.
fn quote_if_ambiguous(s: &str) -> String {
    if !crate::entity::parser::would_coerce_from_string(s) {
        return s.to_string();
    }
    if !s.contains('"') {
        format!("\"{s}\"")
    } else if !s.contains('\'') {
        format!("'{s}'")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{EntityId, MetadataValue, Relationship};
    use indexmap::IndexMap;
    use memstead_schema::{builtin_names, type_by_name};

    fn make_entity(title: &str, vault: &str) -> Entity {
        let schema = type_by_name(builtin_names::SPEC).unwrap();
        let mut metadata = IndexMap::new();
        metadata.insert("level".to_string(), MetadataValue::String("M0".to_string()));
        metadata.insert(
            "created_date".to_string(),
            MetadataValue::String("2026-01-15".to_string()),
        );
        metadata.insert(
            "last_modified".to_string(),
            MetadataValue::String("2026-04-12".to_string()),
        );
        metadata.insert(
            "tags".to_string(),
            MetadataValue::String("backend, api".to_string()),
        );
        metadata.insert(
            "type".to_string(),
            MetadataValue::String("spec".to_string()),
        );

        let mut sections = IndexMap::new();
        for s in &schema.sections {
            sections.insert(s.key.clone(), String::new());
        }
        sections.insert("identity".to_string(), "Test identity.".to_string());
        sections.insert("purpose".to_string(), "Test purpose.".to_string());

        let slug = crate::entity::id::title_to_slug(title).unwrap();
        Entity {
            id: EntityId::new(vault, &slug),
            title: title.to_string(),
            entity_type: "spec".to_string(),
            vault: vault.to_string(),
            file_path: format!("{slug}.md"),
            metadata,
            sections,
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    fn make_assertion_scaffold(title: &str) -> Entity {
        let schema = type_by_name(builtin_names::ASSERTION).unwrap();
        let mut metadata = IndexMap::new();
        metadata.insert(
            "created_date".to_string(),
            MetadataValue::String("2026-01-15".to_string()),
        );
        metadata.insert(
            "last_modified".to_string(),
            MetadataValue::String("2026-04-12".to_string()),
        );
        metadata.insert(
            "type".to_string(),
            MetadataValue::String("assertion".to_string()),
        );

        let mut sections = IndexMap::new();
        for s in &schema.sections {
            sections.insert(s.key.clone(), String::new());
        }

        let slug = crate::entity::id::title_to_slug(title).unwrap();
        Entity {
            id: EntityId::new("assertions", &slug),
            title: title.to_string(),
            entity_type: "assertion".to_string(),
            vault: "assertions".to_string(),
            file_path: format!("{slug}.md"),
            metadata,
            sections,
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn generate_assertion_scaffold_has_schema_sections_and_defaults() {
        let schema = type_by_name(builtin_names::ASSERTION).unwrap();
        let entity = make_assertion_scaffold("Sled Outperforms Rocksdb");
        let md = generate_markdown(&entity, &schema);

        // Schema-declared headings, not spec ones
        assert!(md.contains("## Claim"));
        assert!(md.contains("## Evidence"));
        assert!(md.contains("## Conditions"));
        assert!(md.contains("## Counterevidence"));
        assert!(!md.contains("## Identity"));
        assert!(!md.contains("## Purpose"));

        // Required sections before optional ones (schema order)
        let claim_pos = md.find("## Claim").unwrap();
        let evid_pos = md.find("## Evidence").unwrap();
        let cond_pos = md.find("## Conditions").unwrap();
        let counter_pos = md.find("## Counterevidence").unwrap();
        assert!(claim_pos < evid_pos);
        assert!(evid_pos < cond_pos);
        assert!(cond_pos < counter_pos);

        // Metadata defaults from schemas.rs
        assert!(md.contains("type: assertion"));
        assert!(md.contains("confidence: medium"));
        assert!(md.contains("verification_status: unverified"));

        // Optional fields with no value are omitted
        assert!(!md.contains("source_quality:"));
        assert!(!md.contains("last_verified:"));
    }

    #[test]
    fn generate_basic_entity() {
        let schema = type_by_name(builtin_names::SPEC).unwrap();
        let entity = make_entity("Test Entity", "specs");
        let md = generate_markdown(&entity, &schema);

        assert!(md.starts_with("---\n"));
        assert!(md.contains("# Test Entity"));
        assert!(md.contains("## Identity\nTest identity."));
        assert!(md.contains("## Purpose\nTest purpose."));
        assert!(md.contains("type: spec"));
        assert!(md.contains("level: M0"));
    }

    #[test]
    fn generate_with_relationships() {
        let schema = type_by_name(builtin_names::SPEC).unwrap();
        let mut entity = make_entity("Parent", "specs");
        entity.relationships.push(Relationship {
            rel_type: "USES".to_string(),
            target: EntityId::new("specs", "child-entity"),
            description: None,
        });
        let md = generate_markdown(&entity, &schema);

        assert!(md.contains("## Relationships\n- **USES**: [[child-entity]]"));
    }

    #[test]
    fn generate_relationship_with_description_uses_em_dash_delimiter() {
        let schema = type_by_name(builtin_names::SPEC).unwrap();
        let mut entity = make_entity("Parent", "specs");
        entity.relationships.push(Relationship {
            rel_type: "OTHER".to_string(),
            target: EntityId::new("specs", "child-entity"),
            description: Some("replaced by checkout-flow".to_string()),
        });
        let md = generate_markdown(&entity, &schema);

        assert!(
            md.contains(
                "## Relationships\n- **OTHER**: [[child-entity]] \u{2014} replaced by checkout-flow"
            ),
            "expected canonical em-dash delimiter; got:\n{md}"
        );
    }

    #[test]
    fn generate_relationship_without_description_omits_em_dash() {
        let schema = type_by_name(builtin_names::SPEC).unwrap();
        let mut entity = make_entity("Parent", "specs");
        entity.relationships.push(Relationship {
            rel_type: "USES".to_string(),
            target: EntityId::new("specs", "child-entity"),
            description: None,
        });
        let md = generate_markdown(&entity, &schema);
        // Trailing whitespace, em-dash, or stray content must not
        // appear after the closing `]]` when description is None.
        assert!(md.contains("- **USES**: [[child-entity]]\n"));
        assert!(!md.contains("\u{2014}"), "no em-dash should be emitted when description is None");
    }

    #[test]
    fn generate_metadata_order_follows_schema() {
        let schema = type_by_name(builtin_names::SPEC).unwrap();
        let entity = make_entity("Order Test", "specs");
        let md = generate_markdown(&entity, &schema);

        // Canonical base-metadata order: type, created_date, last_modified,
        // <type-specific>, tags. Type-specific level sits between timestamps
        // and tags.
        let type_pos = md.find("type:").unwrap();
        let created_pos = md.find("created_date:").unwrap();
        let modified_pos = md.find("last_modified:").unwrap();
        let level_pos = md.find("level:").unwrap();
        let tags_pos = md.find("tags:").unwrap();

        assert!(type_pos < created_pos);
        assert!(created_pos < modified_pos);
        assert!(modified_pos < level_pos);
        assert!(level_pos < tags_pos);
    }

    #[test]
    fn generate_omits_optional_absent() {
        let schema = type_by_name(builtin_names::NARRATIVE).unwrap();
        let mut metadata = IndexMap::new();
        metadata.insert(
            "reliability".to_string(),
            MetadataValue::String("firsthand".to_string()),
        );
        metadata.insert(
            "created_date".to_string(),
            MetadataValue::String("2026-01-15".to_string()),
        );
        metadata.insert(
            "last_modified".to_string(),
            MetadataValue::String("2026-04-12".to_string()),
        );
        metadata.insert(
            "type".to_string(),
            MetadataValue::String("narrative".to_string()),
        );

        let mut sections = IndexMap::new();
        for s in &schema.sections {
            sections.insert(s.key.clone(), "placeholder.".to_string());
        }

        let entity = Entity {
            id: EntityId::new("narratives", "no-optional"),
            title: "No Optional".to_string(),
            entity_type: "narrative".to_string(),
            vault: "narratives".to_string(),
            file_path: "no-optional.md".to_string(),
            metadata,
            sections,
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        };

        let md = generate_markdown(&entity, &schema);
        // temporal_range is optional on narrative with no value — must be omitted.
        assert!(!md.contains("temporal_range:"));
    }

    #[test]
    fn generate_ends_with_newline() {
        let schema = type_by_name(builtin_names::SPEC).unwrap();
        let entity = make_entity("Newline Test", "specs");
        let md = generate_markdown(&entity, &schema);
        assert!(md.ends_with('\n'));
    }

    #[test]
    fn roundtrip_parse_generate_parse() {
        let schema = type_by_name(builtin_names::SPEC).unwrap();
        let md = "\
---
type: spec
created_date: 2026-01-15
last_modified: 2026-04-12
level: M0
tags: backend, api
---
# Roundtrip Test

## Identity

This is a roundtrip test entity.

## Purpose

Testing parse→generate→parse stability.

## Relationships

- **USES**: [[other-entity]]

## Specifies

Some content with [[inline-link]].

## Constraints



## Rationale

";

        // First parse
        let result1 =
            crate::entity::parser::parse_markdown(md, "roundtrip-test.md", &schema, "specs")
                .unwrap();

        // Generate
        let generated = generate_markdown(&result1.entity, &schema);

        // Second parse
        let result2 =
            crate::entity::parser::parse_markdown(&generated, "roundtrip-test.md", &schema, "specs")
                .unwrap();

        // Compare: parse(generate(parse(md))) == parse(md)
        assert_eq!(result1.entity.title, result2.entity.title);
        assert_eq!(result1.entity.id, result2.entity.id);
        assert_eq!(
            result1.entity.relationships.len(),
            result2.entity.relationships.len()
        );
        for (r1, r2) in result1
            .entity
            .relationships
            .iter()
            .zip(&result2.entity.relationships)
        {
            assert_eq!(r1.rel_type, r2.rel_type);
            assert_eq!(r1.target, r2.target);
        }
        // Compare sections
        for key in result1.entity.sections.keys() {
            assert_eq!(
                result1.entity.sections.get(key).map(|s| s.trim()),
                result2.entity.sections.get(key).map(|s| s.trim()),
                "Section '{key}' differs after roundtrip"
            );
        }
        // Compare metadata
        for (key, val) in &result1.entity.metadata {
            assert_eq!(
                Some(val),
                result2.entity.metadata.get(key),
                "Metadata '{key}' differs after roundtrip"
            );
        }

        // Verify second roundtrip is byte-stable
        let generated2 = generate_markdown(&result2.entity, &schema);
        assert_eq!(
            generated, generated2,
            "Second roundtrip changed the markdown"
        );
    }

    /// A `FieldType::String` value whose text happens to look like a
    /// number/bool must round-trip as String — the generator has to
    /// YAML-quote it, otherwise the tolerant parser re-types it on the
    /// next read and strict ingress rejects the canonical bytes.
    ///
    /// Regression lock for the narrative-schema `temporal_range` bug
    /// surfaced by V3's project-vaults round-trip test.
    #[test]
    fn generate_quotes_number_shaped_string_value() {
        let schema = type_by_name(builtin_names::NARRATIVE).unwrap();
        let mut metadata = IndexMap::new();
        metadata.insert(
            "temporal_range".to_string(),
            MetadataValue::String("1968".to_string()),
        );
        metadata.insert(
            "reliability".to_string(),
            MetadataValue::String("firsthand".to_string()),
        );
        metadata.insert(
            "created_date".to_string(),
            MetadataValue::String("2026-04-12".to_string()),
        );
        metadata.insert(
            "last_modified".to_string(),
            MetadataValue::String("2026-04-12".to_string()),
        );
        metadata.insert(
            "tags".to_string(),
            MetadataValue::String("history".to_string()),
        );
        metadata.insert(
            "type".to_string(),
            MetadataValue::String("narrative".to_string()),
        );

        let mut sections = IndexMap::new();
        for s in &schema.sections {
            sections.insert(s.key.clone(), "placeholder.".to_string());
        }

        let entity = Entity {
            id: EntityId::new("specs", "ambiguous"),
            title: "Ambiguous".to_string(),
            entity_type: "narrative".to_string(),
            vault: "specs".to_string(),
            file_path: "ambiguous.md".to_string(),
            metadata,
            sections,
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        };

        let md = generate_markdown(&entity, &schema);
        assert!(
            md.contains("temporal_range: \"1968\""),
            "number-shaped string value must be emitted quoted, got:\n{md}"
        );

        // Round-trip must preserve the String typing — unquoted would
        // coerce back to Integer and break strict ingress.
        let parsed =
            crate::entity::parser::parse_markdown(&md, "ambiguous.md", &schema, "specs").unwrap();
        assert_eq!(
            parsed.entity.metadata.get("temporal_range"),
            Some(&MetadataValue::String("1968".to_string())),
            "quoted value must round-trip as String"
        );
    }
}
