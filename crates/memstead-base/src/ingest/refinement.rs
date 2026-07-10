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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::cursor::enumerate_facet_files;
use super::resolve::{ResolvedIngest, ResolvedSource};

/// The rotation key the verify uncovered-artifact sampler walks under. Named so
/// independent verify samples (uncovered files, anchor spot-checks) each get
/// their own rotation cursor within one binding's state without interfering.
pub const ROTATION_UNCOVERED_FILES: &str = "uncovered-files";

/// The rotation key the verify anchor-adjudication sampler walks under (D2) — a
/// distinct cursor from [`ROTATION_UNCOVERED_FILES`], so the cap-sized
/// adjudication window rotates over the anchor set independently of the
/// uncovered-file sample.
pub const ROTATION_ANCHOR_ADJUDICATION: &str = "anchor-adjudication";

/// One named rotation's cursor over a set: the shuffled item order plus its
/// rotation counter and position. One rotation covers the whole set once before
/// reshuffling.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RotationCursor {
    #[serde(default)]
    rotation: u64,
    #[serde(default)]
    cursor: usize,
    #[serde(default)]
    order: Vec<String>,
}

/// Per-binding verify-scheduling state (persisted as JSON under the engine cache
/// tier). Holds the level-trigger run clock (`verify_runs`, for `full_resync_every`,
/// D3) and the set of named rotation cursors the verify samplers walk (D2).
///
/// The prior flat single-rotation shape (a bare `rotation`/`cursor`/`file_order`
/// triple) is superseded by `rotations`; because this lives under the recomputable
/// `.memstead.cache/` tier, a state file in the old shape simply fails to parse
/// and reseeds — no migration needed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RefinementState {
    /// The verify-run counter — the `full_resync_every` level-trigger clock (D3).
    /// Ticks every verify run, including a run whose source enumerates to nothing
    /// (a non-enumerable medium), so the schedule refuses *on time* rather than
    /// silently never firing.
    #[serde(default)]
    verify_runs: u64,
    /// Named rotation cursors, keyed by sample kind
    /// ([`ROTATION_UNCOVERED_FILES`], [`ROTATION_ANCHOR_ADJUDICATION`]).
    #[serde(default)]
    rotations: BTreeMap<String, RotationCursor>,
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

/// Increment and return the persisted verify-run counter for a binding — the
/// level-trigger clock the `full_resync_every` schedule reads (D3). Independent
/// of the rotation cursors: it ticks every verify run, including a run whose
/// source enumerates to nothing (a non-enumerable medium), so the schedule can
/// **refuse on time** rather than silently never firing. Returns the new
/// (post-increment, 1-based) run count.
pub fn bump_verify_runs(cache_root: &Path, binding_name: &str) -> u64 {
    let mut state = load_state(cache_root, binding_name).unwrap_or_default();
    state.verify_runs = state.verify_runs.saturating_add(1);
    let n = state.verify_runs;
    save_state(cache_root, binding_name, &state);
    n
}

/// Advance one **named** rotation over an arbitrary item set — the generalized
/// rotation core the verify samplers (D2) repurpose. `items` is the full set to
/// cover (sorted + de-duplicated by the caller for determinism); `rotation_key`
/// namespaces this rotation within the binding's state file so independent
/// samples rotate on their own cursor. One rotation walks the whole set once in
/// a reproducibly-shuffled order before reshuffling for the next; same persisted
/// state → same sequence. `None` when `items` is empty.
pub fn next_rotation_batch(
    cache_root: &Path,
    binding_name: &str,
    rotation_key: &str,
    items: Vec<String>,
    batch_size: usize,
) -> Option<Batch> {
    let batch_size = batch_size.max(1);
    if items.is_empty() {
        return None;
    }

    let mut state = load_state(cache_root, binding_name).unwrap_or_default();
    let mut cursor = state.rotations.remove(rotation_key).unwrap_or_default();
    if cursor.order.is_empty() || cursor.cursor >= cursor.order.len() {
        // New rotation: bump the counter (only after a completed prior rotation)
        // and reshuffle the whole set.
        let rotation = cursor.rotation + u64::from(!cursor.order.is_empty());
        let mut order = items;
        shuffle(&mut order, rotation);
        cursor = RotationCursor {
            rotation,
            cursor: 0,
            order,
        };
    }

    let end = (cursor.cursor + batch_size).min(cursor.order.len());
    let files = cursor.order[cursor.cursor..end].to_vec();
    let batch_index = cursor.cursor / batch_size + 1;
    let total_batches = cursor.order.len().div_ceil(batch_size);
    cursor.cursor += files.len();
    let rotation = cursor.rotation;
    state.rotations.insert(rotation_key.to_string(), cursor);
    save_state(cache_root, binding_name, &state);

    Some(Batch {
        files,
        rotation,
        batch_index,
        total_batches,
    })
}

/// Advance the uncovered-artifact file sample (D2) — the retained rotation over
/// a source facet's enumerated files, one `batch_size` window at a time. A thin
/// wrapper over [`next_rotation_batch`] keyed [`ROTATION_UNCOVERED_FILES`].
/// `None` when the binding has no source files (e.g. a non-enumerable medium).
pub fn next_batch(
    resolved: &ResolvedIngest,
    workspace_root: &Path,
    cache_root: &Path,
    batch_size: usize,
) -> Option<Batch> {
    let all_files = enumerate_source_files(resolved, workspace_root);
    next_rotation_batch(
        cache_root,
        &resolved.name,
        ROTATION_UNCOVERED_FILES,
        all_files,
        batch_size,
    )
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

        let b1 = next_batch(&r, root, cache.path(), 2).unwrap();
        assert_eq!(b1.rotation, 0);
        assert_eq!(b1.batch_index, 1);
        assert_eq!(b1.total_batches, 3); // ceil(5/2)
        assert_eq!(b1.files.len(), 2);

        let b2 = next_batch(&r, root, cache.path(), 2).unwrap();
        assert_eq!(b2.batch_index, 2);
        let b3 = next_batch(&r, root, cache.path(), 2).unwrap();
        assert_eq!(b3.batch_index, 3);
        assert_eq!(b3.files.len(), 1); // remainder

        // Rotation exhausted → next batch starts rotation 1.
        let b4 = next_batch(&r, root, cache.path(), 2).unwrap();
        assert_eq!(b4.rotation, 1);
        assert_eq!(b4.batch_index, 1);

        // Every file appears exactly once across a rotation.
        let mut seen: Vec<String> = [b1.files, b2.files, b3.files].concat();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 5, "the rotation covers all files");
    }

    /// D2 — a named rotation over an arbitrary item set is deterministic
    /// (same persisted state → same sequence), covers the whole set over a full
    /// rotation, and reshuffles the next rotation into a different order.
    #[test]
    fn named_rotation_is_deterministic_and_covers_the_whole_set() {
        let cache = tempfile::tempdir().unwrap();
        let items: Vec<String> = (0..6).map(|i| format!("id{i}")).collect();
        let key = ROTATION_ANCHOR_ADJUDICATION;

        // Walk a full rotation of batch 2 → three windows covering all six ids.
        let mut covered: Vec<String> = Vec::new();
        let mut order_r0: Vec<String> = Vec::new();
        for i in 0..3 {
            let b = next_rotation_batch(cache.path(), "m/b", key, items.clone(), 2).unwrap();
            assert_eq!(b.rotation, 0);
            assert_eq!(b.batch_index, i + 1);
            assert_eq!(b.total_batches, 3);
            covered.extend(b.files.clone());
            order_r0.extend(b.files);
        }
        let mut uniq = covered.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), 6, "one rotation covers the whole set");

        // Next rotation reshuffles (different order, same coverage).
        let b = next_rotation_batch(cache.path(), "m/b", key, items.clone(), 2).unwrap();
        assert_eq!(
            b.rotation, 1,
            "a new rotation starts once the prior is done"
        );

        // Reproducibility: re-running from a fresh cache with the same seed
        // (rotation 0) yields the identical first-rotation order.
        let cache2 = tempfile::tempdir().unwrap();
        let mut order_repro: Vec<String> = Vec::new();
        for _ in 0..3 {
            let b = next_rotation_batch(cache2.path(), "m/b", key, items.clone(), 2).unwrap();
            order_repro.extend(b.files);
        }
        assert_eq!(order_r0, order_repro, "same seed/state → same sequence");
    }

    /// D2 — two named rotations under one binding advance on independent cursors:
    /// walking one does not consume the other.
    #[test]
    fn named_rotations_are_independent() {
        let cache = tempfile::tempdir().unwrap();
        let a: Vec<String> = (0..4).map(|i| format!("a{i}")).collect();
        let files =
            next_rotation_batch(cache.path(), "m/b", ROTATION_UNCOVERED_FILES, a.clone(), 2)
                .unwrap();
        let anchors = next_rotation_batch(
            cache.path(),
            "m/b",
            ROTATION_ANCHOR_ADJUDICATION,
            a.clone(),
            2,
        )
        .unwrap();
        // Both are the first window of their own rotation.
        assert_eq!(files.batch_index, 1);
        assert_eq!(anchors.batch_index, 1);
        // Advancing the file rotation again does not touch the anchor cursor.
        let files2 =
            next_rotation_batch(cache.path(), "m/b", ROTATION_UNCOVERED_FILES, a.clone(), 2)
                .unwrap();
        assert_eq!(files2.batch_index, 2);
        let anchors_again =
            next_rotation_batch(cache.path(), "m/b", ROTATION_ANCHOR_ADJUDICATION, a, 2).unwrap();
        assert_eq!(anchors_again.batch_index, 2, "anchor cursor is independent");
    }

    /// D3 — the verify-run counter ticks every call and persists across a fresh
    /// load (the level-trigger clock survives process restarts).
    #[test]
    fn verify_run_counter_ticks_and_persists() {
        let cache = tempfile::tempdir().unwrap();
        assert_eq!(bump_verify_runs(cache.path(), "m/b"), 1);
        assert_eq!(bump_verify_runs(cache.path(), "m/b"), 2);
        assert_eq!(bump_verify_runs(cache.path(), "m/b"), 3);
        // A different binding has its own counter.
        assert_eq!(bump_verify_runs(cache.path(), "m/other"), 1);
    }
}
