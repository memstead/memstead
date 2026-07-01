//! Per-mem tantivy index wrapper with write-through semantics.
//!
//! `MemIndex` owns one `tantivy::Index` and — for write-mems — a long-
//! lived `IndexWriter`. Read-mem archives are immutable: their indexes
//! are populated once at engine init and then drop the writer, so any
//! accidental mutation attempt surfaces as an error rather than silently
//! stale data.

use std::sync::Arc;

use memstead_schema::Schema;
use tantivy::{Index, IndexWriter, TantivyDocument, Term};

use crate::entity::{Entity, EntityId};

use super::schema::IndexFields;
use super::tokenizer;

/// 50 MiB writer heap — tantivy's smallest practical size (its own floor is
/// ~3 MiB, but 50 MiB amortizes to one segment for corpora up to ~10k
/// entities without forcing mid-build flushes).
const WRITER_HEAP_BYTES: usize = 50_000_000;

/// One mem's index. Drop the writer (`finalize_read_only`) after the
/// initial build for read-mem archives; write-mems keep it for the
/// lifetime of the engine.
pub struct MemIndex {
    pub mem: String,
    pub fields: IndexFields,
    pub index: Index,
    writer: Option<IndexWriter>,
}

impl MemIndex {
    /// Build an in-RAM index for a mem. `schema` may be `None` when the
    /// mem's pinned schema failed to resolve; fixed fields still index.
    pub fn build_in_ram(mem: String, schema: Option<&Arc<Schema>>) -> tantivy::Result<Self> {
        let fields = IndexFields::build(schema);
        let index = Index::create_in_ram(fields.schema.clone());
        tokenizer::register(index.tokenizers());
        let writer = index.writer(WRITER_HEAP_BYTES)?;
        Ok(Self {
            mem,
            fields,
            index,
            writer: Some(writer),
        })
    }

    /// Drop the writer and convert this index to read-only. Any subsequent
    /// `index_entity` / `remove_entity` call returns an error.
    pub fn finalize_read_only(&mut self) {
        self.writer = None;
    }

    pub fn is_writable(&self) -> bool {
        self.writer.is_some()
    }

    /// Count of currently-indexed documents. Opens a short-lived reader to
    /// avoid holding one across mutations.
    pub fn doc_count(&self) -> tantivy::Result<usize> {
        let reader = self.index.reader()?;
        let searcher = reader.searcher();
        Ok(searcher.num_docs() as usize)
    }

    /// Index or re-index one entity. Removes any existing doc with the
    /// same id first so updates replace rather than duplicate.
    pub fn index_entity(&mut self, entity: &Entity) -> tantivy::Result<()> {
        if entity.stub {
            // Stubs have no title/content worth indexing and must not be
            // surfaced by search anyway — skip silently.
            return Ok(());
        }
        let Some(writer) = self.writer.as_mut() else {
            return Err(tantivy::TantivyError::InvalidArgument(format!(
                "mem '{}' index is read-only",
                self.mem
            )));
        };
        let id_term = Term::from_field_text(self.fields.id, entity.id.as_ref());
        writer.delete_term(id_term);

        let mut doc = TantivyDocument::new();
        doc.add_text(self.fields.id, entity.id.as_ref());
        doc.add_text(self.fields.mem, &entity.mem);
        doc.add_text(self.fields.entity_type, &entity.entity_type);
        doc.add_text(self.fields.title, &entity.title);

        for (key, content) in &entity.sections {
            if let Some(&field) = self.fields.sections.get(key) {
                doc.add_text(field, content);
            }
        }

        for (key, value) in &entity.metadata {
            if let Some(&field) = self.fields.metadata.get(key) {
                doc.add_text(field, value.to_frontmatter_string());
            }
        }

        writer.add_document(doc)?;
        Ok(())
    }

    /// Remove the doc with this id. No-op if it isn't indexed.
    pub fn remove_entity(&mut self, id: &EntityId) -> tantivy::Result<()> {
        let Some(writer) = self.writer.as_mut() else {
            return Err(tantivy::TantivyError::InvalidArgument(format!(
                "mem '{}' index is read-only",
                self.mem
            )));
        };
        let id_term = Term::from_field_text(self.fields.id, id.as_ref());
        writer.delete_term(id_term);
        Ok(())
    }

    /// Flush pending writes. Read-only indexes succeed as a no-op so
    /// callers can commit across all mems uniformly during bulk builds.
    pub fn commit(&mut self) -> tantivy::Result<()> {
        if let Some(writer) = self.writer.as_mut() {
            writer.commit()?;
        }
        Ok(())
    }

    /// Peek the stored `id` field of every doc — available to integration
    /// tests that need to verify index contents without going through the
    /// query path.
    pub fn stored_ids(&self) -> tantivy::Result<Vec<String>> {
        use tantivy::schema::Value as _;
        let reader = self.index.reader()?;
        let searcher = reader.searcher();
        let mut out = Vec::new();
        for segment in searcher.segment_readers() {
            let store = segment.get_store_reader(10)?;
            for doc_id in 0..segment.num_docs() {
                if segment.is_deleted(doc_id) {
                    continue;
                }
                let doc: TantivyDocument = store.get(doc_id)?;
                if let Some(v) = doc.get_first(self.fields.id)
                    && let Some(s) = v.as_str()
                {
                    out.push(s.to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Entity;
    use indexmap::IndexMap;

    fn make_entity(name: &str, mem: &str) -> Entity {
        let mut sections = IndexMap::new();
        sections.insert("identity".into(), format!("Identity of {name}."));
        sections.insert("purpose".into(), format!("Purpose of {name}."));
        Entity {
            id: EntityId::new(mem, name),
            title: name.to_string(),
            entity_type: "spec".into(),
            mem: mem.into(),
            file_path: format!("{name}.md"),
            metadata: IndexMap::new(),
            sections,
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn index_then_remove_roundtrips() {
        let schema = memstead_schema::Schema::builtin_default();
        let mut idx = MemIndex::build_in_ram("specs".into(), Some(&schema)).unwrap();
        let entity = make_entity("alpha", "specs");
        idx.index_entity(&entity).unwrap();
        idx.commit().unwrap();
        assert_eq!(idx.doc_count().unwrap(), 1);
        assert_eq!(idx.stored_ids().unwrap(), vec!["specs--alpha"]);

        idx.remove_entity(&entity.id).unwrap();
        idx.commit().unwrap();
        assert_eq!(idx.doc_count().unwrap(), 0);
    }

    #[test]
    fn read_only_index_rejects_mutations() {
        let schema = memstead_schema::Schema::builtin_default();
        let mut idx = MemIndex::build_in_ram("archive".into(), Some(&schema)).unwrap();
        let entity = make_entity("fixed", "archive");
        idx.index_entity(&entity).unwrap();
        idx.commit().unwrap();
        idx.finalize_read_only();

        assert!(!idx.is_writable());
        assert!(idx.index_entity(&entity).is_err());
        assert!(idx.remove_entity(&entity.id).is_err());
        // commit on a finalized index must not error.
        assert!(idx.commit().is_ok());
    }

    #[test]
    fn reindexing_same_id_replaces_not_duplicates() {
        let schema = memstead_schema::Schema::builtin_default();
        let mut idx = MemIndex::build_in_ram("specs".into(), Some(&schema)).unwrap();
        let mut entity = make_entity("beta", "specs");
        idx.index_entity(&entity).unwrap();
        entity.title = "Beta renamed".into();
        idx.index_entity(&entity).unwrap();
        idx.commit().unwrap();
        assert_eq!(idx.doc_count().unwrap(), 1);
    }

    #[test]
    fn stubs_are_skipped() {
        let schema = memstead_schema::Schema::builtin_default();
        let mut idx = MemIndex::build_in_ram("specs".into(), Some(&schema)).unwrap();
        let mut entity = make_entity("ghost", "specs");
        entity.stub = true;
        idx.index_entity(&entity).unwrap();
        idx.commit().unwrap();
        assert_eq!(idx.doc_count().unwrap(), 0);
    }
}
