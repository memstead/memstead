//! Ingest selection + backoff — pick the next *due* ingest in a `--all`
//! rotation, skipping ones whose destination is unchanged and whose sources
//! have not moved. Engine-side port of the plugin's `nextIngest` / `shouldSkip`
//! / backoff state.
//!
//! The deterministic state (round-robin cursor, per-ingest backoff, one-shot
//! ran-set) lives engine-side under `<workspace>/.memstead.cache/ingest/` —
//! the same engine-internal bookkeeping location the mtime memo uses. This is
//! not mem-repo / graph state; selection mutates it as its job.
//!
//! Backoff shape (mirrors the plugin exactly): a **linear-ramp** per-ingest
//! skip counter. A destination-snapshot change *or* a moved source resets it
//! to zero and runs; otherwise each unproductive pass grows the cooldown by
//! one (capped at [`MAX_SKIP_LEVEL`]). A one-shot build never skips.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Engine;
use crate::binding::BuildMode;
use crate::pipeline_store::BindingConfigs;

use super::cursor::source_moved;
use super::resolve::{ResolvedIngest, resolve_binding_run};

/// The backoff cooldown ceiling — after this many consecutive unproductive
/// passes the skip count stops growing. Mirrors the plugin's `MAX_SKIP_LEVEL`.
pub const MAX_SKIP_LEVEL: u32 = 10;

/// Per-ingest destination-snapshot backoff state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackoffEntry {
    /// Passes still to skip before the next run.
    #[serde(default)]
    pub skip_remaining: u32,
    /// Current cooldown level (grows by one per unproductive pass, capped).
    #[serde(default)]
    pub skip_level: u32,
    /// The destination snapshot token this entry was last evaluated against.
    #[serde(default)]
    pub snapshot: String,
}

/// The round-robin cursor — the ingest the last rotation advanced to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// The last ingest the cursor advanced to (`None` before the first pass).
    #[serde(default)]
    pub last: Option<String>,
}

/// Apply the destination-snapshot backoff to `entry`, mutating it, and return
/// whether to **skip** this pass. `current` is the destination mem's current
/// snapshot token (empty when none). Mirrors the backoff block of the plugin's
/// `shouldSkip`:
///
///   - destination moved (`snapshot` set and `current` differs) → reset to
///     zero, store `current`, **run**;
///   - `skip_remaining > 0` → decrement, **skip**;
///   - otherwise → if the snapshot is unchanged ramp the level (capped), set
///     `skip_remaining = skip_level`, store `current`, **run**.
pub fn apply_backoff(entry: &mut BackoffEntry, current: &str) -> bool {
    if !entry.snapshot.is_empty() && current != entry.snapshot {
        entry.skip_remaining = 0;
        entry.skip_level = 0;
        entry.snapshot = current.to_string();
        return false;
    }
    if entry.skip_remaining > 0 {
        entry.skip_remaining -= 1;
        return true;
    }
    if !entry.snapshot.is_empty() && current == entry.snapshot {
        entry.skip_level = (entry.skip_level + 1).min(MAX_SKIP_LEVEL);
        entry.skip_remaining = entry.skip_level;
    }
    entry.snapshot = current.to_string();
    false
}

/// Whether a binding should be skipped this rotation. A one-shot build never
/// skips (one-shots are excluded from the eligible set once run). Discovery: a
/// moved source overrides backoff; otherwise the destination-snapshot
/// [`apply_backoff`].
pub fn should_skip(
    mode: BuildMode,
    source_moved: bool,
    entry: &mut BackoffEntry,
    current: &str,
) -> bool {
    match mode {
        BuildMode::OneShot => return false,
        BuildMode::Discovery => {}
    }
    if source_moved {
        return false;
    }
    apply_backoff(entry, current)
}

// ── state files (engine-internal cache) ─────────────────────────────────────

fn read_json<T: Default + for<'de> Deserialize<'de>>(cache_root: &Path, name: &str) -> T {
    std::fs::read(cache_root.join(name))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(cache_root: &Path, name: &str, value: &T) {
    let _ = std::fs::create_dir_all(cache_root);
    if let Ok(bytes) = serde_json::to_vec(value) {
        let _ = std::fs::write(cache_root.join(name), bytes);
    }
}

/// Read the set of one-shot ingests that have already run.
fn read_one_shot_runs(cache_root: &Path) -> BTreeSet<String> {
    let map: BTreeMap<String, bool> = read_json(cache_root, "ingest-one-shot-runs.json");
    map.into_iter()
        .filter(|(_, v)| *v)
        .map(|(k, _)| k)
        .collect()
}

/// Select the next *due* ingest for a `--all` rotation, advancing the
/// round-robin cursor and the per-ingest backoff state. Returns the selected
/// ingest name, or `None` when every eligible ingest is backing off this pass.
pub fn select_next_due(
    engine: &Engine,
    workspace_root: &Path,
    configs: &BindingConfigs,
) -> Option<String> {
    let cache_root = workspace_root.join(".memstead.cache").join("ingest");

    // Eligible = all resolvable bindings minus one-shots that already ran. The
    // selection cache is keyed off the canonical binding id (`<mem>/<stem>`,
    // D3/D9), which is the resolved run's `name`.
    let one_shot_ran = read_one_shot_runs(&cache_root);
    let mut eligible: Vec<ResolvedIngest> = configs
        .bindings
        .iter()
        .filter_map(|r| {
            resolve_binding_run(configs, &format!("{}/{}", r.mem, r.name), &r.config).ok()
        })
        .filter(|ri| !(ri.mode == BuildMode::OneShot && one_shot_ran.contains(&ri.name)))
        .collect();
    eligible.sort_by(|a, b| a.name.cmp(&b.name));
    let n = eligible.len();
    if n == 0 {
        return None;
    }

    // Advance the round-robin cursor by one from the last-picked position.
    let mut cursor: Cursor = read_json(&cache_root, "ingest-cursor.json");
    let start = cursor
        .last
        .as_ref()
        .and_then(|last| eligible.iter().position(|ri| &ri.name == last))
        .map_or(0, |i| (i + 1) % n);
    cursor.last = Some(eligible[start].name.clone());
    write_json(&cache_root, "ingest-cursor.json", &cursor);

    // From the start, take the first ingest that is not backing off.
    let mut backoff: BTreeMap<String, BackoffEntry> = read_json(&cache_root, "ingest-backoff.json");
    let mut selected = None;
    for offset in 0..n {
        let ingest = &eligible[(start + offset) % n];
        let current = engine
            .mem_head_sha(&ingest.destination_mem)
            .ok()
            .flatten()
            .unwrap_or_default();
        let moved = source_moved(engine, ingest, workspace_root);
        let entry = backoff.entry(ingest.name.clone()).or_default();
        if !should_skip(ingest.mode, moved, entry, &current) {
            selected = Some(ingest.name.clone());
            break;
        }
    }
    write_json(&cache_root, "ingest-backoff.json", &backoff);
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The linear-ramp backoff: first pass at a fresh snapshot runs and stores
    /// it; a repeat (unchanged) ramps the cooldown and skips it down; a
    /// destination change resets to zero and runs immediately.
    #[test]
    fn backoff_ramps_and_resets() {
        let mut e = BackoffEntry::default();

        // First evaluation: empty snapshot → runs, stores current.
        assert!(!apply_backoff(&mut e, "sha1"));
        assert_eq!(e.snapshot, "sha1");
        assert_eq!(e.skip_level, 0);

        // Unchanged again → ramp to level 1, skip_remaining 1, runs this pass.
        assert!(!apply_backoff(&mut e, "sha1"));
        assert_eq!(e.skip_level, 1);
        assert_eq!(e.skip_remaining, 1);

        // Next pass: skip_remaining 1 → skip, decrement to 0.
        assert!(apply_backoff(&mut e, "sha1"));
        assert_eq!(e.skip_remaining, 0);

        // Next: remaining 0, unchanged → ramp to 2, runs.
        assert!(!apply_backoff(&mut e, "sha1"));
        assert_eq!(e.skip_level, 2);
        assert_eq!(e.skip_remaining, 2);

        // A destination change resets everything and runs immediately.
        assert!(!apply_backoff(&mut e, "sha2"));
        assert_eq!(e.skip_level, 0);
        assert_eq!(e.skip_remaining, 0);
        assert_eq!(e.snapshot, "sha2");
    }

    /// The ramp is capped at MAX_SKIP_LEVEL.
    #[test]
    fn backoff_caps_at_max_level() {
        let mut e = BackoffEntry {
            skip_level: MAX_SKIP_LEVEL,
            skip_remaining: 0,
            snapshot: "s".to_string(),
        };
        assert!(!apply_backoff(&mut e, "s")); // unchanged, remaining 0 → ramp
        assert_eq!(e.skip_level, MAX_SKIP_LEVEL, "capped");
        assert_eq!(e.skip_remaining, MAX_SKIP_LEVEL);
    }

    /// A one-shot build never skips; a moved source overrides backoff for
    /// discovery; an unchanged discovery destination backs off.
    #[test]
    fn should_skip_honours_mode_and_source_movement() {
        let mut e = BackoffEntry {
            skip_remaining: 3,
            skip_level: 3,
            snapshot: "s".to_string(),
        };
        // A one-shot build never skips, regardless of backoff.
        assert!(!should_skip(BuildMode::OneShot, false, &mut e.clone(), "s"));
        // Discovery with a moved source → run (backoff untouched).
        let mut e2 = e.clone();
        assert!(!should_skip(BuildMode::Discovery, true, &mut e2, "s"));
        assert_eq!(e2.skip_remaining, 3, "moved source does not touch backoff");
        // Discovery, unchanged, cooling down → skip.
        assert!(should_skip(BuildMode::Discovery, false, &mut e, "s"));
    }
}
