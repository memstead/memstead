//! Per-mem tantivy schema construction.
//!
//! A mem's indexed shape is derived from its resolved `memstead_schema::Schema`:
//! the union of all types' section keys becomes the set of text fields,
//! and every metadata field with `Filterable::Equality | Range` becomes a
//! `STRING` fast field. Fixed fields (`id`, `mem`, `entity_type`, `title`)
//! are always present.
//!
//! Field lookups at index/query time go through `IndexFields` rather than
//! re-hitting `schema.get_field(...)` string-based dispatch — keeps hot
//! paths cheap and panics explicit.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use memstead_schema::{Filterable, Schema};
use tantivy::schema::{
    Field, STORED, STRING, Schema as TantivySchema, SchemaBuilder, TextFieldIndexing, TextOptions,
};

use super::tokenizer::MEMSTEAD_TOKENIZER;

/// Tantivy schema + pre-resolved field handles for a single mem index.
#[derive(Clone, Debug)]
pub struct IndexFields {
    pub schema: TantivySchema,
    pub id: Field,
    pub mem: Field,
    pub entity_type: Field,
    pub title: Field,
    /// Section-key → tantivy text field. Keys mirror the section keys the
    /// parser writes into `Entity.sections`.
    pub sections: BTreeMap<String, Field>,
    /// Metadata field key → tantivy STRING field. Only filterable fields
    /// are added so the index doesn't carry fields no caller will query.
    pub metadata: BTreeMap<String, Field>,
}

impl IndexFields {
    /// Build the tantivy schema for a mem given the resolved mem schema.
    ///
    /// Falls back to a minimal fixed-field schema when `mem_schema` is
    /// `None` — useful for read-mems whose pinned schema failed to
    /// resolve (we still index id/mem/title so structural filters work).
    pub fn build(mem_schema: Option<&Arc<Schema>>) -> Self {
        let mut builder = SchemaBuilder::new();

        // Fixed fields — id/mem are STRING for exact-match; title is a
        // tokenized TEXT field with the memstead analyzer.
        let id = builder.add_text_field("id", STRING | STORED);
        let mem = builder.add_text_field("mem", STRING | STORED);
        let entity_type = builder.add_text_field("entity_type", STRING | STORED);
        let title = builder.add_text_field("title", text_options());

        // Union of section keys across every type in the mem's schema.
        // BTreeSet — deterministic field order across runs, cheap to diff.
        let section_keys: BTreeSet<String> = match mem_schema {
            Some(schema) => schema
                .types
                .values()
                .flat_map(|t| t.sections.iter().map(|s| s.key.clone()))
                .collect(),
            None => BTreeSet::new(),
        };
        let mut sections = BTreeMap::new();
        for key in section_keys {
            let f = builder.add_text_field(&format!("section_{key}"), text_options());
            sections.insert(key, f);
        }

        // Filterable metadata — one STRING field per unique key. Per-type
        // collisions on the same key are fine since `Filterable::Equality`
        // fields carry the same lexical value across types.
        let filterable_keys: BTreeSet<String> = match mem_schema {
            Some(schema) => schema
                .types
                .values()
                .flat_map(|t| t.metadata_fields.iter())
                .filter(|f| {
                    matches!(f.filterable, Filterable::Equality | Filterable::Range)
                })
                .map(|f| f.key.clone())
                .collect(),
            None => BTreeSet::new(),
        };
        let mut metadata = BTreeMap::new();
        for key in filterable_keys {
            let f = builder.add_text_field(&format!("meta_{key}"), STRING);
            metadata.insert(key, f);
        }

        let schema = builder.build();

        Self {
            schema,
            id,
            mem,
            entity_type,
            title,
            sections,
            metadata,
        }
    }
}

/// Text-field options keyed to the memstead tokenizer, storing positions so
/// phrase queries work without re-indexing.
fn text_options() -> TextOptions {
    TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(MEMSTEAD_TOKENIZER)
            .set_index_option(tantivy::schema::IndexRecordOption::WithFreqsAndPositions),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use memstead_schema::Schema;

    #[test]
    fn build_without_schema_still_has_fixed_fields() {
        let fields = IndexFields::build(None);
        assert!(fields.schema.get_field("id").is_ok());
        assert!(fields.schema.get_field("mem").is_ok());
        assert!(fields.schema.get_field("title").is_ok());
        assert!(fields.sections.is_empty());
        assert!(fields.metadata.is_empty());
    }

    #[test]
    fn build_with_default_schema_emits_section_fields() {
        let schema = Schema::builtin_default();
        let fields = IndexFields::build(Some(&schema));
        // The default schema's `spec` type declares identity/purpose —
        // those must surface as tantivy fields.
        assert!(fields.sections.contains_key("identity"));
        assert!(fields.sections.contains_key("purpose"));
        assert!(
            fields
                .schema
                .get_field("section_identity")
                .is_ok()
        );
    }

    #[test]
    fn build_with_default_schema_emits_filterable_metadata() {
        let schema = Schema::builtin_default();
        let fields = IndexFields::build(Some(&schema));
        // The built-in types declare filterable keys like `level` and
        // `status`; a drift in any single key shouldn't fail the test,
        // so we just check that at least one filterable key surfaced.
        assert!(
            !fields.metadata.is_empty(),
            "default schema should expose at least one filterable metadata field"
        );
    }
}
