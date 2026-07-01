//! Metadata fields every entity carries regardless of type.
//!
//! The loader prepends `type`, `created_date`, `last_modified` and appends
//! `tags` to each type's declared `metadata_fields`. Authors never write
//! these in YAML — redeclaring a base key is a load-time error.
//!
//! Canonical frontmatter order on write:
//! `type, created_date, last_modified, <type-specific>, tags`.

use crate::types::{FieldType, Filterable, MetadataFieldDef, Serialization};

/// Fields that appear before any type-specific metadata on disk.
pub fn prefix_fields() -> Vec<MetadataFieldDef> {
    vec![
        MetadataFieldDef {
            key: "type".to_string(),
            description: "Entity type discriminator — matches the schema type name.".to_string(),
            field_type: FieldType::String,
            default_value: None,
            enum_values: None,
            optional: false,
            init_timestamp: false,
            auto_timestamp: false,
            serialization: Serialization::Default,
            filterable: Filterable::None,
        },
        MetadataFieldDef {
            key: "created_date".to_string(),
            description: "Date the entity was created — filled by the engine on create."
                .to_string(),
            field_type: FieldType::Date,
            default_value: None,
            enum_values: None,
            optional: false,
            init_timestamp: true,
            auto_timestamp: false,
            serialization: Serialization::Default,
            // Engine-stamped, always a valid ISO date — the canonical
            // "entities created since X" axis. Range-filterable to match
            // `last_modified` and the `memstead_search` range_filters example.
            filterable: Filterable::Range,
        },
        MetadataFieldDef {
            key: "last_modified".to_string(),
            description: "Date of the most recent edit — filled by the engine on write."
                .to_string(),
            field_type: FieldType::Date,
            default_value: None,
            enum_values: None,
            optional: false,
            init_timestamp: false,
            auto_timestamp: true,
            serialization: Serialization::Default,
            filterable: Filterable::Range,
        },
    ]
}

/// Fields that appear after type-specific metadata on disk.
pub fn suffix_fields() -> Vec<MetadataFieldDef> {
    vec![MetadataFieldDef {
        key: "tags".to_string(),
        description: "Free-form categorization tags — comma-separated on disk.".to_string(),
        field_type: FieldType::String,
        default_value: None,
        enum_values: None,
        optional: true,
        init_timestamp: false,
        auto_timestamp: false,
        serialization: Serialization::CsvArray,
        filterable: Filterable::Equality,
    }]
}

/// Every base metadata key. Used by the loader to reject YAML redeclarations
/// and by JSON-Schema generation to surface the implicit fields.
pub const BASE_KEYS: &[&str] = &["type", "created_date", "last_modified", "tags"];

pub fn is_base_key(key: &str) -> bool {
    BASE_KEYS.contains(&key)
}
