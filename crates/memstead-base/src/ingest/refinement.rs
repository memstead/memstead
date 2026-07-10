//! Rotation / batch-order scheduling — the deterministic substrate the verify
//! sampler (E3b) reuses. The refinement-as-writer *brief* (the scout/writer
//! two-phase flow and its temp findings file) is **deleted** (D1/D9/AC10):
//! `refinement` mode is gone from the vocabulary and no renderer remains. What
//! survives, unrendered, is the rotation machinery — a `batch_size`-at-a-time
//! walk over a source facet's files in a reproducibly-shuffled order that
//! resets each rotation.
//!
//! One rotation of [`next_batch`] walks the source files in `batch_size`
//! batches, covering the whole set once before reshuffling for the next
//! rotation. Deterministic state (rotation, cursor, shuffled file order) lives
//! engine-side under `<workspace>/.memstead.cache/ingest/refinement/` — the
//! same engine-internal cache the mtime memo and backoff use.
//!
//! **Port note.** The original plugin shuffled the file order with
//! `Math.random`; this uses a small rotation-seeded PRNG so the order still
//! varies across rotations but is reproducible (no `rand` dependency). The
//! behaviour preserved is "each rotation covers the whole source set in a
//! different batch order"; the exact permutation is not load-bearing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::cursor::enumerate_facet_files;
use super::resolve::{ResolvedIngest, ResolvedSource};

/// Per-binding rotation batch state (persisted as JSON).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RefinementState {
    #[serde(default)]
    rotation: u64,
    #[serde(default)]
    cursor: usize,
    #[serde(default)]
    file_order: Vec<String>,
}

/// One batch: the files to review plus its position in the rotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    /// The files this batch reviews (a `batch_size` slice of the rotation).
    pub files: Vec<String>,
    /// The current rotation number.
    pub rotation: u64,
    /// This batch's 1-based index within the rotation.
    pub batch_index: usize,
    /// The total number of batches in the rotation.
    pub total_batches: usize,
}

/// The `<workspace>/.memstead.cache/ingest/refinement/` directory.
fn refinement_dir(cache_root: &Path) -> PathBuf {
    cache_root.join("refinement")
}

fn state_path(cache_root: &Path, binding_name: &str) -> PathBuf {
    refinement_dir(cache_root).join(format!("{binding_name}.json"))
}

/// Enumerate the union of every source facet's files (sorted, de-duplicated).
/// Each facet's enumeration applies the binding's `deny_paths` (the same
/// strategy-invariant deny set the git and mtime slices honour), so a denied
/// file never lands in a batch.
fn enumerate_source_files(resolved: &ResolvedIngest, workspace_root: &Path) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for source in &resolved.sources {
        if let ResolvedSource::Primary(p) = source {
            files.extend(enumerate_facet_files(
                p,
                &resolved.deny_paths,
                workspace_root,
            ));
        }
    }
    files.sort();
    files.dedup();
    files
}

/// A small rotation-seeded Fisher-Yates shuffle — reproducible, dependency-free.
fn shuffle(files: &mut [String], seed: u64) {
    let mut state = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    for i in (1..files.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let j = ((state >> 33) as usize) % (i + 1);
        files.swap(i, j);
    }
}

fn load_state(cache_root: &Path, binding_name: &str) -> Option<RefinementState> {
    let bytes = std::fs::read(state_path(cache_root, binding_name)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn save_state(cache_root: &Path, binding_name: &str, state: &RefinementState) {
    let path = state_path(cache_root, binding_name);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut bytes) = serde_json::to_vec_pretty(state) {
        bytes.push(b'\n');
        let _ = std::fs::write(path, bytes);
    }
}

/// Advance to the next batch, starting (and shuffling) a new rotation when the
/// current one is exhausted. `None` when the binding has no source files.
/// Unrendered in E2 — the verify sampler (E3b) is its consumer.
pub fn next_batch(
    resolved: &ResolvedIngest,
    workspace_root: &Path,
    cache_root: &Path,
) -> Option<Batch> {
    let batch_size = (resolved.batch_size as usize).max(1);
    let all_files = enumerate_source_files(resolved, workspace_root);
    if all_files.is_empty() {
        return None;
    }

    let mut state = load_state(cache_root, &resolved.name).unwrap_or_default();
    if state.file_order.is_empty() || state.cursor >= state.file_order.len() {
        // New rotation: bump the counter (only after a completed prior rotation)
        // and reshuffle the whole source set.
        let rotation = state.rotation + u64::from(!state.file_order.is_empty());
        let mut file_order = all_files;
        shuffle(&mut file_order, rotation);
        state = RefinementState {
            rotation,
            cursor: 0,
            file_order,
        };
    }

    let end = (state.cursor + batch_size).min(state.file_order.len());
    let files = state.file_order[state.cursor..end].to_vec();
    let batch_index = state.cursor / batch_size + 1;
    let total_batches = state.file_order.len().div_ceil(batch_size);
    state.cursor += files.len();
    save_state(cache_root, &resolved.name, &state);

    Some(Batch {
        files,
        rotation: state.rotation,
        batch_index,
        total_batches,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::BuildMode;
    use crate::ingest::resolve::ResolvedPrimarySource;
    use crate::pipeline::{IngestTrigger, MediumType, PatternEntry, PatternMode};

    fn resolved(name: &str, batch_size: u32) -> ResolvedIngest {
        ResolvedIngest {
            name: name.to_string(),
            mode: BuildMode::Discovery,
            trigger: IngestTrigger::Loop,
            batch_size,
            deny_paths: vec![],
            projection_ref: format!("{name}/p"),
            projection_mem: name.to_string(),
            projection_name: "p".to_string(),
            intent: None,
            sources: vec![ResolvedSource::Primary(ResolvedPrimarySource {
                facet_ref: "f".to_string(),
                medium: "m".to_string(),
                medium_type: MediumType::Codebase,
                medium_pointer: String::new(),
                declared_change_detection: None,
                scope: vec![PatternEntry {
                    path: "**/*.rs".to_string(),
                    mode: PatternMode::Allow,
                }],
                preparation: None,
            })],
            destination_mem: name.to_string(),
            rules: None,
            post_actions: None,
        }
    }

    /// Batching walks the shuffled file set across a rotation, then reshuffles a
    /// new rotation once exhausted.
    #[test]
    fn next_batch_walks_a_rotation_then_starts_a_new_one() {
        let ws = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let root = ws.path();
        for i in 0..5 {
            std::fs::write(root.join(format!("f{i}.rs")), "").unwrap();
        }
        let r = resolved("ref", 2);

        let b1 = next_batch(&r, root, cache.path()).unwrap();
        assert_eq!(b1.rotation, 0);
        assert_eq!(b1.batch_index, 1);
        assert_eq!(b1.total_batches, 3); // ceil(5/2)
        assert_eq!(b1.files.len(), 2);

        let b2 = next_batch(&r, root, cache.path()).unwrap();
        assert_eq!(b2.batch_index, 2);
        let b3 = next_batch(&r, root, cache.path()).unwrap();
        assert_eq!(b3.batch_index, 3);
        assert_eq!(b3.files.len(), 1); // remainder

        // Rotation exhausted → next batch starts rotation 1.
        let b4 = next_batch(&r, root, cache.path()).unwrap();
        assert_eq!(b4.rotation, 1);
        assert_eq!(b4.batch_index, 1);

        // Every file appears exactly once across a rotation.
        let mut seen: Vec<String> = [b1.files, b2.files, b3.files].concat();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 5, "the rotation covers all files");
    }
}
