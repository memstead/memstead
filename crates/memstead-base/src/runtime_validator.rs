//! Runtime CRUD validators consumed by the unified [`crate::Engine`]
//! and `memstead-git-branch`'s mem-repo engine.
//!
//! Distinct concern from [`crate::validator`], which validates sealed
//! archive bytes at the registry / read-mem ingress boundary. This
//! module sits *inside* both runtime engines and gates per-mutation
//! payloads (section keys, metadata keys, enum values) against the
//! pinned schema. The wire-format error codes
//! (`UNKNOWN_SECTION`, `UNKNOWN_METADATA`, `INVALID_ENUM_VALUE`,
//! `MISSING_REQUIRED_SECTION`) are stable across both engines so MCP
//! callers see the same envelope shape regardless of workspace flavour.
//!
//! Returns a typed [`ValidationError`] (or a list of
//! [`MissingRequiredSection`] for the warning surface) — the engine
//! layer above wraps these into its own per-flavour error/Result type.

use std::sync::OnceLock;

use indexmap::IndexMap;
use memstead_schema::{
    CrossMemRelationshipEntry, FieldType, RelationshipDef, RelationshipMode, Schema, TypeDefinition,
};
use regex::Regex;

use crate::entity::MetadataValue;

/// Compact relationship-vocabulary entry — `name` plus optional
/// `when_to_use` prose. Surfaces inside [`ValidationError::InvalidRelationshipType`]
/// recovery payloads so an agent reads the canonical vocabulary in
/// the same response that rejected the call. Mirrors the public
/// `RelationshipHint` shape in `memstead-git-branch`; the engine adapter
/// there converts between the two with a 1:1 field copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationshipHint {
    pub name: String,
    pub when_to_use: Option<String>,
}

/// Inline rendering on the text mirror is the relationship name —
/// `when_to_use` stays on `details.allowed[].when_to_use` for callers
/// that branch on the typed shape.
impl std::fmt::Display for RelationshipHint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// A typed CRUD-time validation failure. Mirrors the wire-format error
/// codes the MCP layer surfaces; the engine adapters convert each
/// variant into their own error type.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ValidationError {
    /// `UNKNOWN_SECTION`: section key not declared on the type and
    /// not absorbed by a `catch_all` section.
    #[error("unknown section '{key}' for type '{entity_type}'")]
    UnknownSection {
        key: String,
        entity_type: String,
        declared: Vec<String>,
        suggestion: Option<String>,
    },
    /// `UNKNOWN_METADATA`: metadata key not declared on the type.
    #[error("unknown metadata field '{key}' for type '{entity_type}'")]
    UnknownMetadata {
        key: String,
        entity_type: String,
        declared: Vec<String>,
        suggestion: Option<String>,
    },
    /// `INVALID_ENUM_VALUE`: enum-typed metadata field rejected the
    /// supplied value.
    #[error("invalid value '{value}' for field '{field}' on type '{entity_type}'")]
    InvalidEnumValue {
        field: String,
        value: String,
        allowed: Vec<String>,
        field_description: Option<String>,
        suggestion: Option<String>,
        type_write_rules: Vec<String>,
        entity_type: String,
    },
    /// `READ_ONLY_FIELD`: caller tried to set or unset a read-only
    /// metadata key (`mem`, `id`, `type`) on update.
    #[error("cannot change read-only field '{field}' via update")]
    ReadOnlyField { field: String },
    /// `SECTION_NOT_UPDATABLE`: the section is not in the type's
    /// `updatable_fields` allowlist, or it is the virtual
    /// `relationships` surface (which is managed by `memstead_relate`,
    /// not `memstead_update`).
    #[error("section '{section}' is not updatable for type '{entity_type}'")]
    SectionNotUpdatable {
        section: String,
        entity_type: String,
    },
    /// `INVALID_REL_TYPE`: the relationship name is not declared in
    /// the active schema and the schema runs in strict mode. Open-mode
    /// schemas admit unknown names with a warning instead — see
    /// [`check_relationship_strict_or_open`].
    #[error("invalid relationship type '{input}'")]
    InvalidRelationshipType {
        input: String,
        allowed: Vec<RelationshipHint>,
        suggestion: Option<String>,
    },
    /// `INVALID_REL_SHAPE`: the edge's `(from_type, to_type)` pair
    /// violates the schema's declared `source_types` / `target_types`
    /// for this relationship. Only fires for shape-pinned edges; an
    /// edge with empty constraint lists admits any pair.
    #[error(
        "relationship '{rel_type}' from type '{from_type}' to type '{to_type}' violates declared shape"
    )]
    InvalidRelationshipShape {
        rel_type: String,
        from_type: String,
        to_type: String,
        allowed_source_types: Vec<String>,
        allowed_target_types: Vec<String>,
        suggestion: Option<RelationshipHint>,
    },
    /// `SECTION_CONTENT_INVALID`: a section body contains a `^## `
    /// line (level-2 heading) which the entity's compose-then-reparse
    /// pipeline would interpret as a section delimiter. Without this
    /// guard a caller can inject content into a different section by
    /// embedding a heading in another section's body. Deeper headings
    /// (`### ` and below) are allowed — the parser anchors only on
    /// level 2.
    #[error(
        "section '{section}' content contains an embedded `^## ` heading line '{embedded_heading}' — \
         the compose-then-reparse pipeline would split the value at that heading; use `### ` or \
         deeper for sub-headings"
    )]
    SectionContentInvalid {
        section: String,
        embedded_heading: String,
    },
    /// `SECTION_CONTENT_INVALID` (control-byte sub-case): a section body
    /// contains a control character other than tab (`\t`) or newline
    /// (`\n`). A NUL especially makes git classify the `.md` blob as
    /// binary, defeating the diffable-markdown invariant the storage
    /// model rests on, and downstream text tooling truncates at it.
    /// Shares the wire code with the heading-injection case (both are
    /// `SECTION_CONTENT_INVALID`) — the `control_char`/`byte_offset`
    /// recovery fields discriminate it from `embedded_heading`. Mirrors
    /// the title control-char guard (refuse with an actionable hint, not
    /// silent strip). `\t` and `\n` stay legal; the verbatim-escape
    /// contract is untouched (this screens a byte class, it does not
    /// de-escape).
    #[error(
        "section '{section}' content contains a disallowed control character U+{codepoint:04X} \
         at byte offset {byte_offset} — only tab and newline are permitted in section bodies"
    )]
    SectionContentControlByte {
        section: String,
        /// The offending control character (a `char`, since the body is
        /// already valid UTF-8 — a NUL is `U+0000`).
        control_char: char,
        /// Its Unicode scalar value, surfaced as a number for
        /// unambiguous machine reading (the string form JSON-escapes).
        codepoint: u32,
        /// Byte offset into the section body — matches the `od -c` view a
        /// caller uses to locate the byte.
        byte_offset: usize,
    },
    /// `INVALID_FIELD_VALUE`: a non-enum typed metadata field received a
    /// value that does not parse as its declared type — a `Date` field
    /// given `"not-a-real-date"` or `""`, or a `Number` field given
    /// non-numeric text. Distinct from `INVALID_ENUM_VALUE` (the key and
    /// type are valid but the value is out of an enum's vocabulary) and
    /// `UNKNOWN_METADATA_FIELD` (the key is not declared): here the key
    /// and field-type are valid but the *value* is malformed for the
    /// type. Without this check the value round-trips raw and corrupts
    /// range-filter results (a non-date string sorts lexically against
    /// real dates, so `*_after` matches it).
    #[error(
        "invalid value '{value}' for field '{field}' on type '{entity_type}' — expected {expected_type}"
    )]
    InvalidFieldValue {
        field: String,
        value: String,
        expected_type: String,
        expected_format: Option<String>,
        field_description: Option<String>,
        entity_type: String,
    },
}

impl ValidationError {
    /// Stable `UPPER_SNAKE_CASE` wire code for this validation sub-variant.
    /// Single source of truth — both `EngineError::code()` (via the
    /// `Validation(_)` arm) and the MCP `validation_envelope` mapper read
    /// from here, so the wire code cannot drift between channels.
    pub fn code(&self) -> &'static str {
        match self {
            ValidationError::UnknownSection { .. } => "UNKNOWN_SECTION",
            ValidationError::UnknownMetadata { .. } => "UNKNOWN_METADATA_FIELD",
            ValidationError::InvalidEnumValue { .. } => "INVALID_ENUM_VALUE",
            ValidationError::ReadOnlyField { .. } => "READ_ONLY_FIELD",
            ValidationError::SectionNotUpdatable { .. } => "SECTION_NOT_UPDATABLE",
            ValidationError::InvalidRelationshipType { .. } => "INVALID_REL_TYPE",
            ValidationError::InvalidRelationshipShape { .. } => "INVALID_REL_SHAPE",
            ValidationError::SectionContentInvalid { .. } => "SECTION_CONTENT_INVALID",
            ValidationError::SectionContentControlByte { .. } => "SECTION_CONTENT_INVALID",
            ValidationError::InvalidFieldValue { .. } => "INVALID_FIELD_VALUE",
        }
    }

    /// Structured recovery payload for this validation sub-variant. The
    /// payload mirrors the variant's declared fields so callers branching
    /// on `code` can read the same shape `validation_envelope` ships on
    /// the MCP wire. Returned shape is documented in the MCP tool
    /// descriptions — see `Errors:` blocks on `memstead_create` / `memstead_update`
    /// / `memstead_relate`.
    pub fn details(&self) -> serde_json::Value {
        match self {
            ValidationError::UnknownSection {
                key,
                entity_type,
                declared,
                suggestion,
            } => serde_json::json!({
                "key": key,
                "entity_type": entity_type,
                "declared": declared,
                "suggestion": suggestion,
            }),
            ValidationError::UnknownMetadata {
                key,
                entity_type,
                declared,
                suggestion,
            } => serde_json::json!({
                "key": key,
                "entity_type": entity_type,
                "declared": declared,
                "suggestion": suggestion,
            }),
            ValidationError::InvalidEnumValue {
                field,
                value,
                allowed,
                field_description,
                suggestion,
                type_write_rules,
                entity_type,
            } => serde_json::json!({
                "field": field,
                "value": value,
                "allowed": allowed,
                "field_description": field_description,
                "suggestion": suggestion,
                "type_write_rules": type_write_rules,
                "entity_type": entity_type,
            }),
            ValidationError::ReadOnlyField { field } => serde_json::json!({
                "field": field,
            }),
            ValidationError::SectionNotUpdatable {
                section,
                entity_type,
            } => serde_json::json!({
                "section": section,
                "entity_type": entity_type,
            }),
            ValidationError::InvalidRelationshipType {
                input,
                allowed,
                suggestion,
            } => {
                let allowed_json: Vec<serde_json::Value> = allowed
                    .iter()
                    .map(|h| {
                        serde_json::json!({
                            "name": h.name,
                            "when_to_use": h.when_to_use,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "input": input,
                    "allowed": allowed_json,
                    "suggestion": suggestion,
                })
            }
            ValidationError::InvalidRelationshipShape {
                rel_type,
                from_type,
                to_type,
                allowed_source_types,
                allowed_target_types,
                suggestion,
            } => {
                let suggestion_json = suggestion.as_ref().map(|h| {
                    serde_json::json!({
                        "name": h.name,
                        "when_to_use": h.when_to_use,
                    })
                });
                let mut details = serde_json::Map::new();
                details.insert(
                    "rel_type".into(),
                    serde_json::Value::String(rel_type.clone()),
                );
                details.insert(
                    "from_type".into(),
                    serde_json::Value::String(from_type.clone()),
                );
                details.insert("to_type".into(), serde_json::Value::String(to_type.clone()));
                // Empty source/target_types in the schema mean shape-free
                // (any type admitted). Surface that as an omitted field on
                // the structured payload — presence implies a constraint,
                // absence implies "any". Disambiguates "no targets allowed"
                // (which the engine cannot produce — empty is never a
                // forbid-all signal) from "any target allowed".
                if !allowed_source_types.is_empty() {
                    details.insert(
                        "allowed_source_types".into(),
                        serde_json::json!(allowed_source_types),
                    );
                }
                if !allowed_target_types.is_empty() {
                    details.insert(
                        "allowed_target_types".into(),
                        serde_json::json!(allowed_target_types),
                    );
                }
                details.insert("suggestion".into(), serde_json::json!(suggestion_json));
                serde_json::Value::Object(details)
            }
            ValidationError::SectionContentInvalid {
                section,
                embedded_heading,
            } => serde_json::json!({
                "section": section,
                "embedded_heading": embedded_heading,
            }),
            ValidationError::SectionContentControlByte {
                section,
                control_char,
                codepoint,
                byte_offset,
            } => serde_json::json!({
                "section": section,
                "control_char": control_char.to_string(),
                "codepoint": codepoint,
                "byte_offset": byte_offset,
            }),
            ValidationError::InvalidFieldValue {
                field,
                value,
                expected_type,
                expected_format,
                field_description,
                entity_type,
            } => serde_json::json!({
                "field": field,
                "value": value,
                "expected_type": expected_type,
                "expected_format": expected_format,
                "field_description": field_description,
                "entity_type": entity_type,
            }),
        }
    }

    /// Render rich, fully-inlined recovery prose for the agent-visible
    /// text channel. Closes the asymmetry where warnings render their
    /// structured `details` inline but errors collapse `details.X`
    /// references to a "+N more — see details.X" pointer pointing at a
    /// channel the agent's MCP client doesn't surface to the model. The
    /// structured
    /// `details()` channel is unchanged — this method only governs
    /// `result.content[0].text`. `Display` stays terse for logs and
    /// `tracing::warn!` consumers.
    pub fn prose_render(&self) -> String {
        match self {
            ValidationError::UnknownSection {
                key,
                entity_type,
                declared,
                suggestion,
            } => {
                let declared_inline = if declared.is_empty() {
                    "(none)".to_string()
                } else {
                    declared.join(", ")
                };
                let suggestion_clause = suggestion
                    .as_deref()
                    .map(|s| format!(" Did you mean '{s}'?"))
                    .unwrap_or_default();
                format!(
                    "unknown section '{key}' for type '{entity_type}' — declared sections: {declared_inline}.{suggestion_clause}"
                )
            }
            ValidationError::UnknownMetadata {
                key,
                entity_type,
                declared,
                suggestion,
            } => {
                let declared_inline = if declared.is_empty() {
                    "(none)".to_string()
                } else {
                    declared.join(", ")
                };
                let suggestion_clause = suggestion
                    .as_deref()
                    .map(|s| format!(" Did you mean '{s}'?"))
                    .unwrap_or_default();
                format!(
                    "unknown metadata field '{key}' for type '{entity_type}' — declared fields: {declared_inline}.{suggestion_clause}"
                )
            }
            ValidationError::InvalidEnumValue {
                field,
                value,
                allowed,
                field_description,
                suggestion,
                type_write_rules,
                entity_type,
            } => {
                let allowed_inline = if allowed.is_empty() {
                    "(none)".to_string()
                } else {
                    allowed.join(", ")
                };
                let desc_clause = field_description
                    .as_deref()
                    .map(|d| format!(" Field purpose: {d}."))
                    .unwrap_or_default();
                let suggestion_clause = suggestion
                    .as_deref()
                    .map(|s| format!(" Did you mean '{s}'?"))
                    .unwrap_or_default();
                let rules_clause = if type_write_rules.is_empty() {
                    String::new()
                } else {
                    format!(" Type-level write_rules: {}.", type_write_rules.join("; "))
                };
                format!(
                    "invalid value '{value}' for field '{field}' on type '{entity_type}' — allowed: {allowed_inline}.{desc_clause}{suggestion_clause}{rules_clause}"
                )
            }
            ValidationError::ReadOnlyField { field } => {
                format!("cannot change read-only field '{field}' via update")
            }
            ValidationError::SectionNotUpdatable {
                section,
                entity_type,
            } => format!("section '{section}' is not updatable for type '{entity_type}'"),
            ValidationError::InvalidRelationshipType {
                input,
                allowed,
                suggestion,
            } => {
                let allowed_inline = if allowed.is_empty() {
                    "(none)".to_string()
                } else {
                    allowed
                        .iter()
                        .map(|h| h.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let suggestion_clause = suggestion
                    .as_deref()
                    .map(|s| format!(" Did you mean '{s}'?"))
                    .unwrap_or_default();
                format!(
                    "invalid relationship type '{input}' — must be one of the schema's declared types: {allowed_inline}.{suggestion_clause}"
                )
            }
            ValidationError::InvalidRelationshipShape {
                rel_type,
                from_type,
                to_type,
                allowed_source_types,
                allowed_target_types,
                suggestion,
            } => {
                let sources_inline = if allowed_source_types.is_empty() {
                    "any".to_string()
                } else {
                    allowed_source_types.join(", ")
                };
                let targets_inline = if allowed_target_types.is_empty() {
                    "any".to_string()
                } else {
                    allowed_target_types.join(", ")
                };
                let suggestion_clause = suggestion
                    .as_ref()
                    .map(|h| format!(" Suggested rel-type: '{}'.", h.name))
                    .unwrap_or_default();
                format!(
                    "relationship '{rel_type}' from type '{from_type}' to type '{to_type}' violates declared shape — allowed sources: {sources_inline}; allowed targets: {targets_inline}.{suggestion_clause}"
                )
            }
            ValidationError::SectionContentInvalid {
                section,
                embedded_heading,
            } => format!(
                "section '{section}' content contains an embedded `## ` heading line '{embedded_heading}' — use `### ` or deeper for sub-headings"
            ),
            ValidationError::SectionContentControlByte {
                section,
                codepoint,
                byte_offset,
                ..
            } => format!(
                "section '{section}' content contains a disallowed control character U+{codepoint:04X} at byte offset {byte_offset} — \
                 only tab (U+0009) and newline (U+000A) are permitted in section bodies. Remove the control character: it would break \
                 the diffable-markdown invariant (a NUL makes git treat the file as binary and text tooling truncates at it)."
            ),
            ValidationError::InvalidFieldValue {
                field,
                value,
                expected_type,
                expected_format,
                field_description,
                entity_type,
            } => {
                let format_clause = expected_format
                    .as_deref()
                    .map(|f| format!(" Expected format: {f}."))
                    .unwrap_or_default();
                let desc_clause = field_description
                    .as_deref()
                    .map(|d| format!(" Field purpose: {d}."))
                    .unwrap_or_default();
                format!(
                    "invalid value '{value}' for field '{field}' on type '{entity_type}' — \
                     not a valid {expected_type}.{format_clause}{desc_clause}"
                )
            }
        }
    }
}

/// Read-only metadata keys the `memstead_update` path must never accept on
/// either set or unset. Keeping these tied to the schema's identity
/// fields would silently drift the entity-id contract; the engine
/// surface treats them as immutable across both flavours.
pub const READ_ONLY_METADATA_KEYS: &[&str] = &["mem", "id", "type"];

/// Reject any attempt to set or unset a read-only metadata key. Both
/// engine flavours call this from their `update_entity` paths; the
/// shared list ensures the `mem` / `id` / `type` triple stays
/// authoritative across both, and the schema's `init_timestamp` /
/// `auto_timestamp` annotations are honoured on write — the engine
/// owns those values on create (`init_timestamp`, set once) and on
/// every update (`auto_timestamp`, re-stamped). Returns
/// [`ValidationError::ReadOnlyField`] on rejection.
pub fn validate_writable_metadata_key(
    key: &str,
    schema: &TypeDefinition,
) -> Result<(), ValidationError> {
    if READ_ONLY_METADATA_KEYS.contains(&key) {
        return Err(ValidationError::ReadOnlyField {
            field: key.to_string(),
        });
    }
    if let Some(field) = schema.metadata_field(key)
        && (field.init_timestamp || field.auto_timestamp)
    {
        return Err(ValidationError::ReadOnlyField {
            field: key.to_string(),
        });
    }
    Ok(())
}

/// Reject an `memstead_update` attempt to write a section that is either
/// the virtual `relationships` surface (managed by `memstead_relate`) or
/// not part of the type's `updatable_fields` allowlist. When the
/// allowlist is empty the section passes — types that opt out of the
/// allowlist accept any declared section.
pub fn validate_updatable_section(
    section: &str,
    schema: &TypeDefinition,
) -> Result<(), ValidationError> {
    if section == "relationships" {
        return Err(ValidationError::SectionNotUpdatable {
            section: section.to_string(),
            entity_type: schema.name.clone(),
        });
    }
    if !schema.updatable_fields.is_empty() && !schema.updatable_fields.iter().any(|f| f == section)
    {
        return Err(ValidationError::SectionNotUpdatable {
            section: section.to_string(),
            entity_type: schema.name.clone(),
        });
    }
    Ok(())
}

/// Tier-2 warning shape — the create / update path emits one entry per
/// required section that is missing or empty. Same payload the MCP
/// layer surfaces as `MISSING_REQUIRED_SECTION` warnings. Type-level
/// `write_rules` no longer ride per warning — they ship once at the
/// mutation-response top level on `type_guidance` keyed by
/// `entity_type` (F9).
#[derive(Debug, Clone)]
pub struct MissingRequiredSection {
    pub entity_type: String,
    pub key: String,
    pub heading: String,
    pub write_rules: Vec<String>,
}

/// Refuse section content that would round-trip through the compose
/// pipeline as a section delimiter. The compose-then-reparse loop's
/// parser anchors on `(?m)^## (.+)$`, so a section body containing a
/// `^## ` line gets split at that heading on the next read — content
/// after the heading lands under a different section key (or a
/// fabricated one). Deeper headings (`### ` and below) are safe — the
/// parser only matches level 2.
pub fn validate_section_content<'a>(
    sections: impl Iterator<Item = (&'a str, &'a str)>,
) -> Result<(), ValidationError> {
    for (key, value) in sections {
        // Refuse control characters other than tab/newline before the
        // heading check. A NUL (and other C0/C1/DEL controls) persists
        // verbatim today and breaks the diffable-markdown invariant — a
        // NUL makes git classify the blob as binary and downstream text
        // tooling truncates at it. Mirrors the title control-char guard
        // (`char::is_control`, refuse-with-actionable-hint) but keeps
        // `\t`/`\n` legal, which titles disallow. We refuse rather than
        // strip — silently mutating caller-sent content is the
        // no-silent-data-loss anti-pattern the title fix already flagged.
        // The verbatim-escape contract is untouched: this screens a byte
        // class, it does not interpret or de-escape content.
        if let Some((byte_offset, ch)) = value
            .char_indices()
            .find(|(_, c)| c.is_control() && *c != '\t' && *c != '\n')
        {
            return Err(ValidationError::SectionContentControlByte {
                section: key.to_string(),
                control_char: ch,
                codepoint: ch as u32,
                byte_offset,
            });
        }
        for line in value.lines() {
            // Match the parser's regex shape: `^## ` (two hashes, one
            // space, at least one trailing char). The trailing space
            // requirement excludes bare `##` (which the parser does
            // not match either) and `###`+ headings.
            if line.starts_with("## ") && line.len() > 3 {
                return Err(ValidationError::SectionContentInvalid {
                    section: key.to_string(),
                    embedded_heading: line.to_string(),
                });
            }
        }
    }
    Ok(())
}

/// Validate that every section key in `provided` is either schema-declared
/// for `schema`, or — if the schema has a catch-all section — admitted by
/// it. Unknown keys return [`ValidationError::UnknownSection`] carrying
/// the declared list plus a Levenshtein suggestion (or the catch-all key
/// when no close match exists).
///
/// Pure function: no I/O, no allocation outside the eventual error
/// payload. The `"relationships"` section is allowed through here — the
/// engine layer above gates it via its own SectionNotUpdatable check.
pub fn validate_section_keys<'a>(
    provided: impl Iterator<Item = &'a str>,
    schema: &TypeDefinition,
) -> Result<(), ValidationError> {
    let mut declared: Vec<String> = schema.sections.iter().map(|s| s.key.clone()).collect();
    declared.sort();
    let declared_set: std::collections::HashSet<&str> =
        schema.sections.iter().map(|s| s.key.as_str()).collect();
    let catch_all_key = schema.catch_all_section().map(|s| s.key.clone());

    for key in provided {
        if key == "relationships" {
            continue;
        }
        if declared_set.contains(key) {
            continue;
        }
        let suggestion = schema
            .suggest_section(key)
            .or_else(|| catch_all_key.clone());
        return Err(ValidationError::UnknownSection {
            key: key.to_string(),
            entity_type: schema.name.clone(),
            declared: declared.clone(),
            suggestion,
        });
    }
    Ok(())
}

/// Parse a metadata value string into the appropriate
/// [`MetadataValue`] type, consulting the schema for field-type
/// information. Validates enum constraints when the field definition
/// specifies `enum_values`.
///
/// Unknown keys are a hard error — engine code that builds metadata
/// only emits schema-declared fields, so a lenient insert would
/// silently drop the value at write time and the agent would read a
/// success response while losing data.
pub fn parse_metadata_value(
    key: &str,
    value: &str,
    schema: &TypeDefinition,
) -> Result<MetadataValue, ValidationError> {
    let Some(field_def) = schema.metadata_field(key) else {
        let mut declared: Vec<String> = schema
            .metadata_fields
            .iter()
            .map(|f| f.key.clone())
            .collect();
        declared.sort();
        return Err(ValidationError::UnknownMetadata {
            key: key.to_string(),
            entity_type: schema.name.clone(),
            declared,
            suggestion: schema.suggest_metadata_field(key),
        });
    };

    if let Some(ref allowed) = field_def.enum_values
        && !allowed.iter().any(|v| v == value)
    {
        let suggestion = nearest_str_match(value, allowed);
        return Err(ValidationError::InvalidEnumValue {
            field: key.to_string(),
            value: value.to_string(),
            allowed: allowed.clone(),
            field_description: Some(field_def.description.clone()),
            suggestion,
            type_write_rules: schema.write_rules.clone(),
            entity_type: schema.name.clone(),
        });
    }

    Ok(match field_def.field_type {
        FieldType::Boolean => MetadataValue::Bool(value == "true" || value == "1"),
        FieldType::Number => {
            if let Ok(n) = value.parse::<i64>() {
                MetadataValue::Integer(n)
            } else if let Ok(f) = value.parse::<f64>() {
                MetadataValue::Float(f)
            } else {
                // Pre-fix this fell back to `String`, silently storing
                // non-numeric text in a Number field. Reject so the
                // value never reaches the store (and never corrupts a
                // range filter on the field).
                return Err(ValidationError::InvalidFieldValue {
                    field: key.to_string(),
                    value: value.to_string(),
                    expected_type: "Number".to_string(),
                    expected_format: Some("an integer or decimal number".to_string()),
                    field_description: Some(field_def.description.clone()),
                    entity_type: schema.name.clone(),
                });
            }
        }
        FieldType::Date => {
            // The field's declared shape is `YYYY-MM-DD` (or the ISO
            // datetime form). Pre-fix any string — including `""` and
            // arbitrary text — fell through to the `String` arm and was
            // stored raw; a non-date value then sorts lexically against
            // real dates and produces false `*_after` / `*_before`
            // range-filter matches. Validate at the write boundary so
            // the corruption can never land.
            if !is_date_shaped(value) {
                return Err(ValidationError::InvalidFieldValue {
                    field: key.to_string(),
                    value: value.to_string(),
                    expected_type: "Date".to_string(),
                    expected_format: Some("YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ".to_string()),
                    field_description: Some(field_def.description.clone()),
                    entity_type: schema.name.clone(),
                });
            }
            MetadataValue::String(value.to_string())
        }
        _ => MetadataValue::String(value.to_string()),
    })
}

/// Does `s` match the shape a `Date`-typed metadata value must have —
/// `YYYY-MM-DD` or the ISO-8601 datetime form `YYYY-MM-DDTHH:MM:SSZ`?
///
/// Single source of truth for the date-shape check, shared by the CRUD
/// write path ([`parse_metadata_value`]) and the archive-ingress strict
/// validator (`crate::validator::strict::value_matches_type`). Keeping
/// one regex means the value a `memstead_create` accepts and the value an
/// import re-accepts cannot drift apart.
pub fn is_date_shaped(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}:\d{2}Z)?$").unwrap())
        .is_match(s)
}

/// Tier-2 warning shape — the create path emits one entry per required
/// metadata field that is not auto-filled by the schema (no
/// `default_value`, no `init_timestamp`, no `auto_timestamp`) and was
/// not supplied by the caller. Same payload the MCP layer surfaces as
/// `MISSING_REQUIRED_FIELD` warnings — mirrors the
/// `REQUIRED_FIELD_UNSET` error envelope so a single decoder handles
/// both surfaces.
#[derive(Debug, Clone)]
pub struct MissingRequiredField {
    pub entity_type: String,
    pub key: String,
    pub description: String,
    pub enum_values: Vec<String>,
}

/// Return one [`MissingRequiredField`] per required metadata field that
/// the caller did not supply and the schema does not auto-fill. A field
/// is "auto-filled" when it carries `default_value`, `init_timestamp`,
/// or `auto_timestamp` — the engine writes a non-trivial value without
/// caller input. Optional fields and supplied fields are skipped.
///
/// Caller-side intent: the warning fires when the entity would land in
/// a stuck state (placeholder today's-date / empty string) that the
/// agent did not opt into. Surfaced from the create path so dry-run
/// and real-write preview the same set of warnings.
pub fn missing_required_fields(
    schema: &TypeDefinition,
    supplied: &IndexMap<String, String>,
) -> Vec<MissingRequiredField> {
    schema
        .metadata_fields
        .iter()
        .filter(|f| {
            // Engine-managed fields (`type`, `id`, `mem`) are seeded
            // independently of caller input; not the agent's
            // responsibility to supply.
            !READ_ONLY_METADATA_KEYS.contains(&f.key.as_str())
                && !f.optional
                && f.default_value.is_none()
                && !f.init_timestamp
                && !f.auto_timestamp
                && !supplied.contains_key(f.key.as_str())
        })
        .map(|f| MissingRequiredField {
            entity_type: schema.name.clone(),
            key: f.key.clone(),
            description: f.description.clone(),
            enum_values: f.enum_values.clone().unwrap_or_default(),
        })
        .collect()
}

/// Return one [`MissingRequiredSection`] per required section that is
/// absent or empty in `sections`. Empty (whitespace-only) bodies count
/// as missing — same predicate as the health report uses.
pub fn missing_required_sections(
    schema: &TypeDefinition,
    sections: &IndexMap<String, String>,
) -> Vec<MissingRequiredSection> {
    schema
        .required_sections()
        .filter_map(|sec| {
            let is_empty = sections
                .get(sec.key.as_str())
                .is_none_or(|c| c.trim().is_empty());
            is_empty.then(|| MissingRequiredSection {
                entity_type: schema.name.clone(),
                key: sec.key.clone(),
                heading: sec.heading.clone(),
                write_rules: sec.write_rules.clone(),
            })
        })
        .collect()
}

/// Outcome of running a relationship name against a schema. The
/// engine adapter above decides whether to ride the warning out on
/// the response (open mode) or convert the error into its own type
/// (strict mode).
#[derive(Debug, Clone)]
pub enum RelationshipCheck {
    /// Name is declared in the schema's relationship vocabulary.
    Ok,
    /// Schema runs in open mode and admits the name with a warning
    /// the engine layer can surface to the agent.
    OpenWarning(String),
}

/// Validate a relationship name against a mem schema's vocabulary.
/// Strict-mode schemas reject undeclared names with
/// [`ValidationError::InvalidRelationshipType`]; open-mode schemas
/// admit unknown names and return a warning string for the engine to
/// surface.
///
/// Both engine flavours call this from their `memstead_relate` paths so
/// the wire shape (`INVALID_REL_TYPE`, `allowed[]`, `suggestion`) is
/// stable across mem-repo and filesystem-mem.
pub fn validate_rel_type(
    rel_type: &str,
    schema: &Schema,
) -> Result<RelationshipCheck, ValidationError> {
    if schema.relationship_known(rel_type) {
        return Ok(RelationshipCheck::Ok);
    }
    match schema.mode() {
        RelationshipMode::Strict => {
            let allowed = declared_relationship_hints(schema);
            let candidate_names: Vec<String> = allowed.iter().map(|h| h.name.clone()).collect();
            let suggestion = nearest_str_match(rel_type, &candidate_names);
            Err(ValidationError::InvalidRelationshipType {
                input: rel_type.to_string(),
                allowed,
                suggestion,
            })
        }
        RelationshipMode::Open => {
            let declared: Vec<String> = declared_relationship_hints(schema)
                .into_iter()
                .map(|h| h.name)
                .collect();
            let suggestion = schema
                .suggest_relationship(rel_type)
                .map(|s| format!(" Did you mean '{s}'?"))
                .unwrap_or_default();
            let (schema_name, schema_version) = schema.id();
            Ok(RelationshipCheck::OpenWarning(format!(
                "relationship '{rel_type}' is not declared in schema \
                 '{schema_name}@{schema_version}' (mode: open). \
                 Accepted with default weight. Declared: [{}].{suggestion}",
                declared.join(", "),
            )))
        }
    }
}

/// Reject an edge whose `(from_type, to_type)` pair violates the
/// schema's declared `source_types` / `target_types` for this
/// relationship. No-op when both constraint lists are empty
/// (shape-free edges) or when the relationship name is unknown
/// (callers run this only after [`validate_rel_type`] succeeds, so
/// this branch is defensive). The target-type check is skipped when
/// `to_type` is `None` — happens for auto-stubbed targets that have
/// no type yet; once the stub is authored as a real entity, future
/// edges land under the strict check.
///
/// Suggestion: nearest-match edge in the schema whose declared shape
/// would admit `(from_type, to_type)`. Tiebreaker is declaration
/// order in the YAML (deterministic).
pub fn validate_rel_shape(
    rel_type: &str,
    from_type: &str,
    to_type: Option<&str>,
    schema: &Schema,
) -> Result<(), ValidationError> {
    let Some(def) = schema.relationship_def(rel_type) else {
        return Ok(());
    };
    let source_ok = def.source_types.is_empty() || def.source_types.iter().any(|t| t == from_type);
    let target_ok = def.target_types.is_empty()
        || to_type.is_none_or(|t| def.target_types.iter().any(|d| d == t));
    if source_ok && target_ok {
        return Ok(());
    }
    let to_for_err = to_type.unwrap_or("<unknown>").to_string();
    let suggestion = suggest_shape_admitting(from_type, to_type, schema);
    Err(ValidationError::InvalidRelationshipShape {
        rel_type: rel_type.to_string(),
        from_type: from_type.to_string(),
        to_type: to_for_err,
        allowed_source_types: def.source_types.clone(),
        allowed_target_types: def.target_types.clone(),
        suggestion,
    })
}

/// Outcome of looking up a rel-type against a cross-mem entry in
/// the source schema's `cross_mem_relationships:` vocabulary.
/// `EdgeNotDeclared` carries the recovery payload the engine layer
/// wraps into [`crate::EngineError::CrossMemEdgeNotDeclared`]; the
/// other variants reuse the existing `ValidationError` shapes so
/// agents reading the wire shape decode `INVALID_REL_TYPE` /
/// `INVALID_REL_SHAPE` identically in both intra- and cross-mem
/// flows.
#[derive(Debug, Clone)]
pub enum CrossMemRelCheck {
    /// `(rel_type, from_type, to_type)` are admitted by the matched
    /// cross-mem entry's declared vocabulary and shape. The engine
    /// proceeds with the relate write.
    Ok,
    /// The source schema declares no cross-mem entry whose
    /// `to_schema:` matches the target schema. Carries the recovery
    /// payload for `CROSS_MEM_EDGE_NOT_DECLARED`.
    EdgeNotDeclared,
    /// Validation tripped the matched cross-mem entry's own
    /// vocabulary / shape — reuses the existing `INVALID_REL_TYPE` /
    /// `INVALID_REL_SHAPE` envelopes (carried as the wrapped
    /// `ValidationError`) so wire-shape decoders stay flat.
    Invalid(ValidationError),
}

/// Validate a cross-mem edge whose source and target mems pin
/// schemas with *different names* against the source schema's
/// outbound `cross_mem_relationships:` vocabulary.
///
/// Caller responsibility: only invoke when the source and target
/// schema *names* differ — same-name mems (any version pair) fall
/// through to the intra-mem path ([`validate_rel_type`] +
/// [`validate_rel_shape`]); same-name is same domain.
///
/// The lookup goes through [`Schema::cross_mem_entry`], which
/// matches by target schema name only — eligibility is name-based,
/// so the target mem's pinned version never participates and a
/// version bump on the target side cannot invalidate a declaration.
///
/// On a match, the cross-mem entry's `definitions` list is the sole
/// vocabulary for this edge: the source schema's intra-mem
/// `relationships.definitions` is NOT consulted in this regime (per
/// AC #6 / #9). A rel-type present intra-mem but absent cross-mem
/// surfaces here as `INVALID_REL_TYPE`; a shape violation surfaces
/// here as `INVALID_REL_SHAPE` with the cross-mem entry's shape
/// (not the intra-mem entry's, if both exist).
pub fn validate_cross_mem_edge(
    rel_type: &str,
    from_type: &str,
    to_type: Option<&str>,
    source_schema: &Schema,
    target_schema_ref: &memstead_schema::SchemaRef,
) -> CrossMemRelCheck {
    let Some(entry) = source_schema.cross_mem_entry(&target_schema_ref.name) else {
        return CrossMemRelCheck::EdgeNotDeclared;
    };

    let Some(def) = entry.definitions.iter().find(|d| d.name == rel_type) else {
        let allowed: Vec<RelationshipHint> = cross_mem_entry_hints(entry);
        let candidate_names: Vec<String> = allowed.iter().map(|h| h.name.clone()).collect();
        let suggestion = nearest_str_match(rel_type, &candidate_names);
        return CrossMemRelCheck::Invalid(ValidationError::InvalidRelationshipType {
            input: rel_type.to_string(),
            allowed,
            suggestion,
        });
    };

    let source_ok = def.source_types.is_empty() || def.source_types.iter().any(|t| t == from_type);
    let target_ok = def.target_types.is_empty()
        || to_type.is_none_or(|t| def.target_types.iter().any(|d| d == t));
    if source_ok && target_ok {
        return CrossMemRelCheck::Ok;
    }
    let to_for_err = to_type.unwrap_or("<unknown>").to_string();
    let suggestion = cross_mem_suggest_shape(entry, from_type, to_type);
    CrossMemRelCheck::Invalid(ValidationError::InvalidRelationshipShape {
        rel_type: rel_type.to_string(),
        from_type: from_type.to_string(),
        to_type: to_for_err,
        allowed_source_types: def.source_types.clone(),
        allowed_target_types: def.target_types.clone(),
        suggestion,
    })
}

/// Sorted vocabulary hints for one cross-mem entry, excluding the
/// `_default` sentinel — same shape as
/// [`declared_relationship_hints`] but scoped to a single cross-mem
/// declaration.
fn cross_mem_entry_hints(entry: &CrossMemRelationshipEntry) -> Vec<RelationshipHint> {
    let mut out: Vec<RelationshipHint> = entry
        .definitions
        .iter()
        .filter(|d| d.name != "_default")
        .map(|d| RelationshipHint {
            name: d.name.clone(),
            when_to_use: d.when_to_use.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// First cross-mem `definition` in declaration order whose declared
/// shape would admit `(from_type, to_type)`. Mirrors
/// [`suggest_shape_admitting`] but scoped to a single cross-mem
/// entry.
fn cross_mem_suggest_shape(
    entry: &CrossMemRelationshipEntry,
    from_type: &str,
    to_type: Option<&str>,
) -> Option<RelationshipHint> {
    entry
        .definitions
        .iter()
        .filter(|d| d.name != "_default")
        .find(|d| cross_mem_def_admits(d, from_type, to_type))
        .map(|d| RelationshipHint {
            name: d.name.clone(),
            when_to_use: d.when_to_use.clone(),
        })
}

fn cross_mem_def_admits(d: &RelationshipDef, from_type: &str, to_type: Option<&str>) -> bool {
    let src_ok = d.source_types.is_empty() || d.source_types.iter().any(|t| t == from_type);
    let tgt_ok =
        d.target_types.is_empty() || to_type.is_none_or(|t| d.target_types.iter().any(|x| x == t));
    src_ok && tgt_ok
}

/// First edge in declaration order whose declared shape would admit
/// the `(from_type, to_type)` pair. Empty `source_types` /
/// `target_types` admit anything. Returns `None` when no such edge
/// exists. The `_default` sentinel is excluded — it carries no shape
/// and is never a real edge's rel_type.
fn suggest_shape_admitting(
    from_type: &str,
    to_type: Option<&str>,
    schema: &Schema,
) -> Option<RelationshipHint> {
    schema
        .manifest
        .relationships
        .definitions
        .iter()
        .filter(|d| d.name != "_default")
        .find(|d| {
            let src_ok = d.source_types.is_empty() || d.source_types.iter().any(|t| t == from_type);
            let tgt_ok = d.target_types.is_empty()
                || to_type.is_none_or(|t| d.target_types.iter().any(|x| x == t));
            src_ok && tgt_ok
        })
        .map(|d| RelationshipHint {
            name: d.name.clone(),
            when_to_use: d.when_to_use.clone(),
        })
}

/// Sorted relationship vocabulary as `RelationshipHint`s, excluding
/// the internal `_default` catch-all. Used inside
/// [`validate_rel_type`] to populate the `INVALID_REL_TYPE` recovery
/// payload's `allowed[]` list.
fn declared_relationship_hints(schema: &Schema) -> Vec<RelationshipHint> {
    let mut out: Vec<RelationshipHint> = schema
        .manifest
        .relationships
        .definitions
        .iter()
        .filter(|d| d.name != "_default")
        .map(|d| RelationshipHint {
            name: d.name.clone(),
            when_to_use: d.when_to_use.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Levenshtein-nearest match against a candidate set, with a noise
/// floor of `chars/2` (beyond that the input shares almost nothing with
/// the schema vocabulary, so a "did you mean" suggestion does not
/// help). Returns `None` when no candidate is close enough.
fn nearest_str_match(needle: &str, candidates: &[String]) -> Option<String> {
    let noise_floor = (needle.chars().count() / 2).max(1);
    let mut best: Option<(usize, String)> = None;
    for cand in candidates {
        let d = strsim::levenshtein(needle, cand);
        if d == 0 || d > noise_floor {
            continue;
        }
        match &best {
            Some((bd, _)) if *bd <= d => {}
            _ => best = Some((d, cand.clone())),
        }
    }
    best.map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
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
propagating_relationships: []
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
propagating_relationships: []
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
propagating_relationships: []
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
        let err = validate_section_content([("body", "line1\u{0}line2")].into_iter())
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
            let err = validate_section_content([("s", body.as_str())].into_iter())
                .expect_err("control char must be refused");
            assert_eq!(err.code(), "SECTION_CONTENT_INVALID", "char {:?}", bad);
        }
    }

    #[test]
    fn section_content_allows_tab_and_newline() {
        // The two legitimate whitespace controls round-trip; multi-line
        // and tabbed bodies are unaffected.
        validate_section_content([("body", "line1\nline2\n\tindented\tcols\n")].into_iter())
            .expect("tab and newline must stay legal in section bodies");
    }

    #[test]
    fn section_content_keeps_backslashes_verbatim() {
        // The fix screens a byte class — it must not interpret or
        // de-escape content. Literal backslashes (incl. ones that look
        // like escapes) pass through untouched.
        validate_section_content(
            [("body", r"a literal \n and \t and \0 and \\ backslash")].into_iter(),
        )
        .expect("backslashes are literal content, not control bytes");
    }

    #[test]
    fn section_content_still_refuses_heading_injection() {
        // The pre-existing heading-injection guard is unchanged and
        // shares the wire code.
        let err = validate_section_content([("body", "intro\n## Injected\ntail")].into_iter())
            .expect_err("embedded `## ` heading must still be refused");
        assert_eq!(err.code(), "SECTION_CONTENT_INVALID");
        assert!(matches!(err, ValidationError::SectionContentInvalid { .. }));
    }
}
