//! Runtime CRUD validators consumed by the unified [`crate::Engine`] —
//! the single mutation engine, whatever storage backend (mem-repo git
//! branch, plain folder, archive) sits behind a mount.
//!
//! Distinct concern from [`crate::validator`], which validates sealed
//! archive bytes at the registry / read-mem ingress boundary. This
//! module sits *inside* the mutation engine and gates per-mutation
//! payloads (section keys, metadata keys, enum values) against the
//! pinned schema. The wire-format error codes
//! (`UNKNOWN_SECTION`, `UNKNOWN_METADATA`, `INVALID_ENUM_VALUE`,
//! `MISSING_REQUIRED_SECTION`) are stable regardless of workspace
//! storage, so MCP callers always see the same envelope shape.
//!
//! Returns a typed [`ValidationError`] (or a list of
//! [`MissingRequiredSection`] for the warning surface) — the engine
//! layer above wraps these into its error/Result type.

use std::sync::OnceLock;

use indexmap::IndexMap;
use memstead_schema::{
    CrossMemRelationshipEntry, FieldType, RelationshipDef, RelationshipMode, Schema, Serialization,
    TypeDefinition,
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
        "section '{section}' content contains an embedded reserved (`# ` / `## `) heading line '{embedded_heading}' — \
         the compose-then-reparse pipeline would split the value at that heading; use `### ` or \
         deeper for sub-headings"
    )]
    SectionContentInvalid {
        section: String,
        embedded_heading: String,
    },
    /// `EMPTY_UNDECLARED_HEADING`: caller-supplied content carries a heading
    /// the type does not declare with NO body under it.
    /// The catch-all builder skips empty content, so such
    /// a heading is dropped on the write that accepts it: the caller is told
    /// now rather than discovering the loss afterwards.
    ///
    /// The boundary is exactly the complement of the catch-all exemption
    /// [`validate_section_content`] grants: an undeclared heading WITH a body
    /// survives verbatim and is accepted, one without a body does not survive
    /// and is refused. The exemption and this refusal are the same rule read
    /// from its two sides.
    #[error(
        "section '{section}' carries heading '{heading}', which type '{entity_type}' does not \
         declare and which has no body under it. The catch-all keeps absorbed content but skips \
         empty content, so this heading would be dropped by the write. Give it a body, or \
         remove the heading."
    )]
    EmptyUndeclaredHeading {
        section: String,
        heading: String,
        entity_type: String,
    },
    /// `UNTERMINATED_FENCE`: caller-supplied section content ends inside a
    /// fenced code block that never closes.
    /// Nothing about the parse is wrong: an open fence's range
    /// runs to end of text in CommonMark, so every `## ` line the generator
    /// writes after this section lands inside the fence, is masked, and is
    /// absorbed into this section's body on the next read. The sections then
    /// render as empty and the entity reads as healthy.
    ///
    /// The sibling guard [`ValidationError::SectionContentInvalid`] refuses
    /// content that would BECOME a delimiter; this one refuses content that
    /// would HIDE one. Both are about the same seam from opposite sides, and
    /// neither can see the other's case: a fence opener is not a heading line,
    /// and the heading guard's own mask is what renders the hidden heading
    /// invisible to it.
    ///
    /// The oracle is `markdown::closing_fence_if_unterminated`, the referee's
    /// existing sentinel probe. No second fence model is written here.
    #[error(
        "section '{section}' ends inside an unterminated `{fence}` code fence — every section \
         heading written after it would be swallowed into this body and read back as empty. \
         Close the fence with a line of `{fence}`."
    )]
    UnterminatedFence { section: String, fence: String },
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
            ValidationError::EmptyUndeclaredHeading { .. } => "EMPTY_UNDECLARED_HEADING",
            ValidationError::UnterminatedFence { .. } => "UNTERMINATED_FENCE",
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
            ValidationError::UnterminatedFence { section, fence } => serde_json::json!({
                "section": section,
                "fence": fence,
                "expected": format!(
                    "close the fence with a line of `{fence}`, or remove the opener"
                ),
            }),
            ValidationError::EmptyUndeclaredHeading {
                section,
                heading,
                entity_type,
            } => serde_json::json!({
                "section": section,
                "heading": heading,
                "entity_type": entity_type,
                "expected": "give the heading a body, or remove it: the catch-all keeps \
                             absorbed content but skips empty content",
            }),
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
            ValidationError::EmptyUndeclaredHeading {
                section,
                heading,
                entity_type,
            } => format!(
                "section '{section}' carries heading '{heading}', which type '{entity_type}' does \
                 not declare and which has no body under it. An undeclared heading WITH content \
                 is kept byte-verbatim in the catch-all; an empty one is skipped, so this write \
                 would drop it. Give the heading a body, or remove it."
            ),
            ValidationError::UnterminatedFence { section, fence } => format!(
                "section '{section}' ends inside an unterminated `{fence}` code fence. In \
                 CommonMark an open fence runs to the end of the text, so every section heading \
                 written after this one would land inside the fence, be masked, and be read back \
                 as part of THIS section's body: the sections after it would render empty and \
                 the entity would still read as healthy. Close the fence with a line of \
                 `{fence}`, or remove the opener."
            ),
            ValidationError::SectionContentInvalid {
                section,
                embedded_heading,
            } => format!(
                "section '{section}' content contains an embedded reserved (`# ` / `## `) heading line '{embedded_heading}' — use `### ` or deeper for sub-headings"
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

/// Reserved metadata keys — the entity's identity/discriminator triple.
/// No write path accepts them as caller-supplied metadata (create and
/// update both refuse a *set*); letting them ride the schema's declared
/// fields would silently drift the entity-id contract. Unset is the one
/// sanctioned exception: `metadata_unset` may name a reserved key to
/// repair an entity that acquired a smuggled one before the write gates
/// closed — removing a reserved key can only move the entity toward the
/// invariant (the `type` discriminator is re-seeded by the engine, never
/// left absent).
pub const READ_ONLY_METADATA_KEYS: &[&str] = &["mem", "id", "type"];

/// Reject a caller-supplied reserved identity/discriminator key
/// (`mem` / `id` / `type`) as metadata — the create-path half of the
/// reservation, deliberate and typed
/// ([`ValidationError::ReadOnlyField`]) rather than the incidental
/// `UNKNOWN_METADATA_FIELD` a reserved key would otherwise trip
/// (no installable schema can declare one). Timestamp fields are NOT
/// checked here: create's posture for `init_timestamp` /
/// `auto_timestamp` fields is stamp-and-proceed with an
/// `IGNORED_READONLY_FIELD` warning, deliberately.
///
/// The `_` prefix is refused as a namespace, not as a key list: every
/// underscore-prefixed frontmatter key is a computed read-channel slot
/// (`_hash`, `_tokens`, `_signals`, ...), so a stored metadata key in
/// that namespace would render as a second, stale copy of a computed
/// field — frontmatter copied out of a read response and pasted into a
/// write is the observed ingress. Unset stays permissive (see
/// [`validate_unsettable_metadata_key`]): removing a smuggled `_` key
/// is the same sanctioned repair as removing a smuggled reserved key.
pub fn validate_reserved_metadata_key(key: &str) -> Result<(), ValidationError> {
    if READ_ONLY_METADATA_KEYS.contains(&key) || key.starts_with('_') {
        return Err(ValidationError::ReadOnlyField {
            field: key.to_string(),
        });
    }
    Ok(())
}

/// Reject any attempt to **set** a read-only metadata key. The single
/// mutation engine (`memstead-base`) calls this from its `update_entity`
/// path over the `metadata` map: the `mem` / `id` / `type` triple stays
/// engine-authoritative, and the schema's `init_timestamp` /
/// `auto_timestamp` annotations are honoured on write — the engine
/// owns those values on create (`init_timestamp`, set once) and on
/// every update (`auto_timestamp`, re-stamped). Returns
/// [`ValidationError::ReadOnlyField`] on rejection. The unset path has
/// its own gate ([`validate_unsettable_metadata_key`]) because the
/// reserved triple is unset-allowed there as the sanctioned repair.
pub fn validate_writable_metadata_key(
    key: &str,
    schema: &TypeDefinition,
) -> Result<(), ValidationError> {
    validate_reserved_metadata_key(key)?;
    if let Some(field) = schema.metadata_field(key)
        && (field.init_timestamp || field.auto_timestamp)
    {
        return Err(ValidationError::ReadOnlyField {
            field: key.to_string(),
        });
    }
    Ok(())
}

/// Gate for `metadata_unset` keys. Unlike the set path
/// ([`validate_writable_metadata_key`]), the reserved
/// identity/discriminator triple (`mem` / `id` / `type`) IS
/// unsettable: removing one can only move an entity toward the
/// invariant, and it is the sanctioned repair for entities that
/// acquired a smuggled reserved key before the write gates closed
/// (delete-and-recreate would destroy provenance and edges).
/// Engine-stamped timestamp fields (`init_timestamp` /
/// `auto_timestamp`) stay refused on unset — the engine owns their
/// values and re-stamps them; unsetting one is caller confusion, not
/// repair.
pub fn validate_unsettable_metadata_key(
    key: &str,
    schema: &TypeDefinition,
) -> Result<(), ValidationError> {
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

/// What a type's catch-all absorbs, for [`validate_section_content`].
#[derive(Debug, Clone, Copy)]
pub struct CatchAllContext<'a> {
    /// The catch-all section's key.
    pub key: &'a str,
    /// The entity type, so a refusal can name what does not declare the
    /// heading rather than leaving the caller to work it out.
    pub entity_type: &'a str,
    /// Every heading the type declares. A `## ` line matching one of these
    /// forks the entity even inside the catch-all, so it stays refused.
    pub declared_headings: &'a [&'a str],
}

/// Build a [`CatchAllContext`] for a type, when it has a catch-all section.
/// The borrowed heading list lives in `buf`, which the caller owns.
pub fn catch_all_context<'a>(
    type_def: &'a memstead_schema::TypeDefinition,
    buf: &'a mut Vec<&'a str>,
) -> Option<CatchAllContext<'a>> {
    let key = type_def.catch_all_section()?.key.as_str();
    buf.extend(type_def.sections.iter().map(|s| s.heading.as_str()));
    Some(CatchAllContext {
        key,
        entity_type: type_def.name.as_str(),
        declared_headings: buf,
    })
}

/// Whether the lines after `heading` in `body`, up to the next `## ` line,
/// are all blank. That is what the catch-all builder skips.
fn heading_body_is_empty(body: &str, heading_line: &str) -> bool {
    let mut lines = body.lines().skip_while(|l| *l != heading_line);
    lines.next();
    for line in lines {
        if line.starts_with("## ") {
            return true;
        }
        if !line.trim().is_empty() {
            return false;
        }
    }
    true
}

/// Refuse section content that would round-trip through the compose
/// pipeline as a section delimiter. The compose-then-reparse loop's
/// parser anchors on `(?m)^## (.+)$` over the *masked* body, so a
/// section body that shows a `^## ` line to that scan gets split at
/// that heading on the next read — content after the heading lands
/// under a different section key (or a fabricated one). Deeper
/// headings (`### ` and below) are safe — the parser only matches
/// level 2.
///
/// The guard is applied to the content **as the reparse will see it**,
/// which is what makes it exact rather than approximate:
///
/// - the content is trimmed first, because the splitter stores the
///   trimmed body — an indented block opening a section loses its
///   indent on write-back, so `    ## Not A Heading` becomes a real
///   column-0 delimiter on the next parse. Checking the still-indented
///   provided content missed that fork entirely;
/// - code blocks are masked first, by the same CommonMark referee the
///   splitter uses ([`crate::markdown`]) — a `## ` inside a fenced or
///   indented code block never splits anything, so refusing it was the
///   write path disagreeing with the read path about what a code block
///   is.
///
/// `catch_all` names the type's catch-all section and its declared headings,
/// when the caller knows them. Inside
/// the CATCH-ALL body only, a `## ` line whose heading the type does not
/// declare is accepted, because the reparse absorbs it straight back into the
/// catch-all: the content does not land under a different key, which is the
/// whole basis of this guard. That case is not hypothetical — it is what the
/// engine itself emits, since the catch-all builder re-emits absorbed content
/// under its original heading line, and an agent that read an entity and wrote
/// that section back in replace mode was refused its own value.
///
/// This does NOT weaken the guard. A DECLARED heading inside the catch-all
/// still refuses, because that one really does fork: the reparse would move
/// the content to the declared key. Every other section is unchanged, and a
/// caller who passes `None` gets exactly the old behaviour.
pub fn validate_section_content<'a>(
    sections: impl Iterator<Item = (&'a str, &'a str)>,
    catch_all: Option<CatchAllContext<'_>>,
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
        // Before the heading walk, because the two guards diagnose the same
        // seam and this one has the better answer when both apply: a body
        // that leaves a fence open AND carries a heading is swallowed, not
        // forked, so naming the fence tells the caller what to fix.
        if let Some(fence) = crate::markdown::closing_fence_if_unterminated(value.trim()) {
            return Err(ValidationError::UnterminatedFence {
                section: key.to_string(),
                fence,
            });
        }
        // What the splitter will store, and what the splitter will
        // see in it. `stored` and `masked` share byte offsets and line
        // count, so the two line sequences correspond one-to-one and
        // the refusal can quote the real line.
        let stored = value.trim();
        let masked = crate::markdown::mask_code_blocks(stored);
        for (line, masked_line) in stored.lines().zip(masked.lines()) {
            // Match the parser's regex shape: `^## ` (two hashes, one
            // space, at least one trailing char). The trailing space
            // requirement excludes bare `##` (which the parser does
            // not match either) and `###`+ headings. `^# ` joins the
            // guard (plan 08): h1 and h2 are the entity's own levels
            // — the title and the section delimiters — so neither may
            // be embedded in a section body.
            let is_h2 = masked_line.starts_with("## ") && masked_line.len() > 3;
            let is_h1 = masked_line.starts_with("# ") && masked_line.len() > 2;
            // The one exemption, and it is exact: the catch-all re-absorbs an
            // undeclared h2 rather than forking on it — but only when there is
            // something under it. An undeclared heading WITH a body survives
            // the round trip verbatim; one with NO body is skipped by the
            // catch-all builder and silently dropped by the write, so it
            // refuses instead. The exemption and the refusal are the same rule
            // read from its two sides (04/01, criteria 6 and 7).
            if is_h2
                && catch_all.is_some_and(|c| {
                    c.key == key && !c.declared_headings.contains(&&masked_line[3..])
                })
            {
                if heading_body_is_empty(stored, line) {
                    return Err(ValidationError::EmptyUndeclaredHeading {
                        section: key.to_string(),
                        heading: masked_line[3..].to_string(),
                        entity_type: catch_all
                            .map(|c| c.entity_type.to_string())
                            .unwrap_or_default(),
                    });
                }
                continue;
            }
            if is_h2 || is_h1 {
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

    // A declared `value_pattern` binds every written value in full; a
    // csv-array field is checked member by member so the refusal names
    // the one malformed entry, not the whole list.
    if let Some(pattern) = field_def.value_pattern.as_ref()
        && let Ok(re) = regex::Regex::new(&format!("^(?:{pattern})$"))
    {
        let members: Vec<&str> = if field_def.serialization == Serialization::CsvArray {
            value.split(',').map(str::trim).collect()
        } else {
            vec![value]
        };
        if let Some(bad) = members.iter().find(|m| !re.is_match(m)) {
            return Err(ValidationError::InvalidFieldValue {
                field: key.to_string(),
                value: (*bad).to_string(),
                expected_type: "String".to_string(),
                expected_format: Some(format!("matching the pattern `{pattern}`")),
                field_description: Some(field_def.description.clone()),
                entity_type: schema.name.clone(),
            });
        }
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
                && f.is_required()
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
/// The mutation engine calls this from its `memstead_relate` path; the
/// wire shape (`INVALID_REL_TYPE`, `allowed[]`, `suggestion`) is stable
/// regardless of workspace storage.
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
    // Priority-ordered entries: exact-name declaration first, then the
    // `to_schema: "*"` wildcard (loader-bound to the schema's alias
    // target rel-type). First rel-type hit across the entries wins, so
    // structural declarations for a destination never shadow the
    // wildcarded alias links into it.
    let entries = source_schema.cross_mem_entries(&target_schema_ref.name);
    if entries.is_empty() {
        return CrossMemRelCheck::EdgeNotDeclared;
    }

    let Some(def) = entries
        .iter()
        .find_map(|entry| entry.definitions.iter().find(|d| d.name == rel_type))
    else {
        // Only the wildcard matched and it doesn't carry this
        // rel-type: for THIS destination schema the rel-type has
        // genuinely no declaration — the historical
        // `CROSS_MEM_EDGE_NOT_DECLARED` refusal, so a structural edge
        // into an undeclared schema reads the same with or without a
        // wildcard present (the wildcard only ever admits the alias
        // rel-type).
        if !entries.iter().any(|e| e.to_schema != "*") {
            return CrossMemRelCheck::EdgeNotDeclared;
        }
        let allowed: Vec<RelationshipHint> = cross_mem_entries_hints(&entries);
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
    let suggestion = entries
        .iter()
        .find_map(|entry| cross_mem_suggest_shape(entry, from_type, to_type));
    CrossMemRelCheck::Invalid(ValidationError::InvalidRelationshipShape {
        rel_type: rel_type.to_string(),
        from_type: from_type.to_string(),
        to_type: to_for_err,
        allowed_source_types: def.source_types.clone(),
        allowed_target_types: def.target_types.clone(),
        suggestion,
    })
}

/// Union of [`cross_mem_entry_hints`] across priority-ordered entries,
/// de-duplicated by rel-type name (first entry's hint wins) and
/// re-sorted.
fn cross_mem_entries_hints(entries: &[&CrossMemRelationshipEntry]) -> Vec<RelationshipHint> {
    let mut out: Vec<RelationshipHint> = Vec::new();
    for entry in entries {
        for hint in cross_mem_entry_hints(entry) {
            if !out.iter().any(|h| h.name == hint.name) {
                out.push(hint);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
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
mod tests;
