//! Per-vault tantivy search indexes.
//!
//! The engine holds `search_indexes: HashMap<String, VaultIndex>` for
//! its writable vaults, each with a long-lived `IndexWriter`, lazily
//! built on first search and rebuilt from the store after any mutation
//! or reload invalidates the map (whole-map invalidation, not
//! incremental upkeep). [`execute_on_vault`] serves queries against the
//! built indexes.

pub mod query;
pub mod schema;
pub mod snippets;
pub mod tokenizer;
pub mod writer;

pub use query::execute_on_vault;
pub use schema::IndexFields;
pub use snippets::{compute_matched_terms, compute_score_breakdown};
pub use writer::VaultIndex;

use std::collections::HashMap;
use std::sync::Arc;

use memstead_schema::Schema;

use crate::store::Store;

/// Build per-vault indexes for every writable vault. Each vault's index
/// uses its own pinned schema from `writable_schemas`.
pub fn build_all(
    store: &Store,
    writable_schemas: &HashMap<String, Arc<Schema>>,
) -> HashMap<String, VaultIndex> {
    let mut indexes: HashMap<String, VaultIndex> = HashMap::new();

    // Write-vaults keep their writers alive for the engine lifetime.
    for (name, schema) in writable_schemas {
        match VaultIndex::build_in_ram(name.clone(), Some(schema)) {
            Ok(idx) => {
                indexes.insert(name.clone(), idx);
            }
            Err(e) => {
                tracing::warn!(
                    vault = name.as_str(),
                    error = %e,
                    "failed to create search index for writable vault; skipping"
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
        let Some(idx) = indexes.get_mut(&entity.vault) else {
            continue;
        };
        if let Err(e) = idx.index_entity(entity) {
            tracing::warn!(
                vault = entity.vault.as_str(),
                id = entity.id.as_ref(),
                error = %e,
                "failed to index entity during bulk build"
            );
        }
    }

    for idx in indexes.values_mut() {
        if let Err(e) = idx.commit() {
            tracing::warn!(
                vault = idx.vault.as_str(),
                error = %e,
                "failed to commit vault index after bulk build"
            );
        }
    }

    indexes
}
