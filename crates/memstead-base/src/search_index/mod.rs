//! Per-mem tantivy search indexes.
//!
//! The engine holds `search_indexes: HashMap<String, MemIndex>` for
//! its writable mems, each with a long-lived `IndexWriter`, lazily
//! built on first search and rebuilt from the store after any mutation
//! or reload invalidates the map (whole-map invalidation, not
//! incremental upkeep). [`execute_on_mem`] serves queries against the
//! built indexes.

pub mod query;
pub mod schema;
pub mod snippets;
pub mod tokenizer;
pub mod writer;

pub use query::execute_on_mem;
pub use schema::IndexFields;
pub use snippets::{compute_matched_terms, compute_score_breakdown};
pub use writer::MemIndex;

use std::collections::HashMap;
use std::sync::Arc;

use memstead_schema::Schema;

use crate::store::Store;

/// Build per-mem indexes for every writable mem. Each mem's index
/// uses its own pinned schema from `writable_schemas`.
pub fn build_all(
    store: &Store,
    writable_schemas: &HashMap<String, Arc<Schema>>,
) -> HashMap<String, MemIndex> {
    let mut indexes: HashMap<String, MemIndex> = HashMap::new();

    // Write-mems keep their writers alive for the engine lifetime.
    for (name, schema) in writable_schemas {
        match MemIndex::build_in_ram(name.clone(), Some(schema)) {
            Ok(idx) => {
                indexes.insert(name.clone(), idx);
            }
            Err(e) => {
                tracing::warn!(
                    mem = name.as_str(),
                    error = %e,
                    "failed to create search index for writable mem; skipping"
                );
            }
        }
    }

    // Populate every index from the store, then commit. Warn-log on error
    // so a broken index doesn't take the engine down — callers can reload
    // to recover.
    for entity in store.all_entities() {
        if entity.stub {
            continue;
        }
        let Some(idx) = indexes.get_mut(&entity.mem) else {
            continue;
        };
        if let Err(e) = idx.index_entity(entity) {
            tracing::warn!(
                mem = entity.mem.as_str(),
                id = entity.id.as_ref(),
                error = %e,
                "failed to index entity during bulk build"
            );
        }
    }

    for idx in indexes.values_mut() {
        if let Err(e) = idx.commit() {
            tracing::warn!(
                mem = idx.mem.as_str(),
                error = %e,
                "failed to commit mem index after bulk build"
            );
        }
    }

    indexes
}
