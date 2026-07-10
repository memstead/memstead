//! Changed-slice computation — the deterministic core of the ingest
//! source-cursor: given a source's stored baseline and its current state,
//! classify the pass as reseed / unchanged / changed, and for a changed
//! pass compute the added / modified / deleted [`Slice`].
//!
//! This is the engine-side port of the plugin's per-facet
//! `computeSourceCursor` return contract (`inject.mjs`). It covers the two
//! strategies whose slice computation is pure once the inputs are in hand:
//!
//!   - **graph** — map a source mem's [`ChangeEnvelope`]s into a slice of
//!     entity ids ([`graph_changes_to_slice`] / [`graph_slice_outcome`]).
//!   - **mtime** — classify against a stored digest token and diff the
//!     stat maps ([`mtime_slice_outcome`], over the [`super::change_detection`]
//!     primitives).
//!
//! The **git** strategy's slice (parse `git diff --name-status`, build
//! facet-scope pathspecs) lands with its subprocess glue and the path
//! normalization it needs — kept together rather than split here.
//!
//! Load-bearing invariant preserved from the plugin: the new baseline
//! `token` is only ever *returned* here, never written. The agent records
//! it (via `set-sync-state`) as the **last** step of a full pass, so an
//! aborted pass leaves the baseline untouched and the next run re-presents
//! the identical slice.

use crate::ops::ChangeEnvelope;

use super::change_detection::{
    Digest, StatDiff, StatMap, diff_stat_maps, digest_stat_map, digests_equal, parse_digest_token,
    serialize_digest_token,
};

/// The classified set of changed source artifacts in one pass. For the git
/// and mtime strategies the entries are workspace-relative paths; for the
/// graph strategy they are entity ids. Each class is kept sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Slice {
    /// Newly present artifacts.
    pub added: Vec<String>,
    /// Artifacts present before and after, but changed.
    pub modified: Vec<String>,
    /// Artifacts that vanished (the cheapest, highest-signal drift).
    pub deleted: Vec<String>,
}

impl Slice {
    fn sort(&mut self) {
        self.added.sort();
        self.modified.sort();
        self.deleted.sort();
    }
}

impl From<StatDiff> for Slice {
    fn from(d: StatDiff) -> Self {
        Slice {
            added: d.added,
            modified: d.modified,
            deleted: d.deleted,
        }
    }
}

/// Why a source produced no usable change signal — the classified reason a
/// [`SliceOutcome::NoSignal`] carries so the brief can render it
/// *distinguishably* from a genuinely-unchanged source (which stays silent).
/// Every variant is a rendered state; none is dropped silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoSignalReason {
    /// The facet has no allow patterns — it is **unscoped**, so no file-tree
    /// strategy will diff or enumerate the whole medium. A typed refusal,
    /// uniform across git / mtime / refinement. A facet that truly wants the
    /// whole medium writes `**/*`.
    Unscoped,
    /// Change detection is declared `none` — the source is inert by design
    /// ([`super::resolve::ChangeStrategy::None`]), re-roamed whole with no
    /// slice.
    DetectionNone,
    /// A git strategy could not read a signal: no work tree over the medium
    /// pointer, `HEAD` unreadable, the stored baseline is unknown (gc'd /
    /// rewritten / out-of-repo pathspec), or the diff subprocess failed.
    GitUnavailable,
    /// A graph strategy could not read a signal: the source mem has no
    /// snapshot token, is unknown to the engine, or its change history could
    /// not be fetched against the stored baseline.
    GraphSnapshotMissing,
}

/// The outcome of computing a source's changed slice against its baseline —
/// the engine-side shape of the plugin's per-facet `computeSourceCursor`
/// return. The `token` in `Reseed` / `Unchanged` / `Changed` is the new
/// baseline the agent records **only after a full pass** (never written
/// here). `NoSignal` carries a [`NoSignalReason`] and renders in the brief; it
/// advances no baseline (a whole re-roam or a typed refusal, per reason).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SliceOutcome {
    /// No usable change signal, classified by [`NoSignalReason`] — rendered in
    /// the brief, never silently dropped. Advances no baseline.
    NoSignal {
        /// Why detection produced no signal.
        reason: NoSignalReason,
    },
    /// No prior baseline — seed at the current token, present no slice.
    Reseed {
        /// The token to seed the baseline at.
        token: String,
    },
    /// Baseline equals the current token — nothing moved.
    Unchanged {
        /// The (unchanged) current token.
        token: String,
    },
    /// The source moved: the changed slice plus the new baseline token.
    Changed {
        /// The new baseline token to record after a full pass.
        token: String,
        /// The changed artifacts.
        slice: Slice,
        /// The precise slice was unavailable (mtime memo miss) and this is
        /// a full-scan stand-in — detection still fired, precision was lost.
        degraded: bool,
    },
}

/// A git commit id / graph snapshot token: 7–64 hex characters. Mirrors the
/// plugin's `isGitToken`, distinguishing a usable baseline from a foreign
/// token (an mtime digest JSON, an empty string, junk).
pub fn is_git_token(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Map a graph mem's change envelopes into a [`Slice`] of entity ids:
/// `Removed` → deleted; `Renamed` → the new id added **and** the old id
/// deleted; `Added` → added; `Updated` → modified.
pub fn graph_changes_to_slice(changes: &[ChangeEnvelope]) -> Slice {
    let mut slice = Slice::default();
    for change in changes {
        match change {
            ChangeEnvelope::Added { id, .. } => slice.added.push(id.as_ref().to_string()),
            ChangeEnvelope::Removed { id, .. } => slice.deleted.push(id.as_ref().to_string()),
            ChangeEnvelope::Renamed { from_id, to_id, .. } => {
                slice.added.push(to_id.as_ref().to_string());
                slice.deleted.push(from_id.as_ref().to_string());
            }
            ChangeEnvelope::Updated { id, .. } => slice.modified.push(id.as_ref().to_string()),
        }
    }
    slice.sort();
    slice
}

/// Classify a graph source against its baseline. `baseline` is the token
/// stored for this `(ingest, facet)`; `current` is the source mem's current
/// snapshot token; `changes` are the source mem's changes since `baseline`
/// (only consulted when the source actually moved). Mirrors the plugin's
/// `computeGraphSlice`:
///
///   - `current` not a usable snapshot token → [`NoSignal`] (degrade).
///   - `baseline` absent / not a usable token → [`Reseed`] at `current`.
///   - `baseline == current` → [`Unchanged`].
///   - otherwise → [`Changed`] with the mapped `changes`.
///
/// [`NoSignal`]: SliceOutcome::NoSignal
/// [`Reseed`]: SliceOutcome::Reseed
/// [`Unchanged`]: SliceOutcome::Unchanged
/// [`Changed`]: SliceOutcome::Changed
pub fn graph_slice_outcome(
    baseline: Option<&str>,
    current: &str,
    changes: &[ChangeEnvelope],
) -> SliceOutcome {
    if !is_git_token(current) {
        return SliceOutcome::NoSignal {
            reason: NoSignalReason::GraphSnapshotMissing,
        };
    }
    match baseline {
        Some(b) if is_git_token(b) => {
            if b == current {
                SliceOutcome::Unchanged {
                    token: current.to_string(),
                }
            } else {
                SliceOutcome::Changed {
                    token: current.to_string(),
                    slice: graph_changes_to_slice(changes),
                    degraded: false,
                }
            }
        }
        _ => SliceOutcome::Reseed {
            token: current.to_string(),
        },
    }
}

/// Classify an mtime source against its baseline digest token. `baseline` is
/// the token stored for this `(ingest, facet)`; `current_map` is the freshly
/// stat'd map; `prev_map` is the memo of the baseline's stat map, if the
/// skill cache still holds it. Mirrors the plugin's mtime branch of
/// `computeSourceCursor`:
///
///   - `baseline` not a parseable digest token → [`Reseed`] at the current
///     digest.
///   - baseline digest equals the current digest → [`Unchanged`].
///   - otherwise → [`Changed`]: the precise [`diff_stat_maps`] when `prev_map`
///     is present, or a **degraded** full scan (every current file as added)
///     on a memo miss — detection still fired from the digest.
///
/// [`Reseed`]: SliceOutcome::Reseed
/// [`Unchanged`]: SliceOutcome::Unchanged
/// [`Changed`]: SliceOutcome::Changed
pub fn mtime_slice_outcome(
    baseline: Option<&str>,
    prev_map: Option<&StatMap>,
    current_map: &StatMap,
) -> SliceOutcome {
    let current_digest: Digest = digest_stat_map(current_map);
    let token = serialize_digest_token(&current_digest);

    let baseline_digest = baseline.and_then(parse_digest_token);
    let Some(baseline_digest) = baseline_digest else {
        return SliceOutcome::Reseed { token };
    };

    if digests_equal(Some(&baseline_digest), Some(&current_digest)) {
        return SliceOutcome::Unchanged { token };
    }

    match prev_map {
        Some(prev) => SliceOutcome::Changed {
            token,
            slice: diff_stat_maps(prev, current_map).into(),
            degraded: false,
        },
        None => {
            // Memo miss: the digest proved the source moved, but the precise
            // slice is gone — present every current file as added (one-tick
            // full scan), flagged degraded.
            let mut added: Vec<String> = current_map.keys().cloned().collect();
            added.sort();
            SliceOutcome::Changed {
                token,
                slice: Slice {
                    added,
                    modified: Vec::new(),
                    deleted: Vec::new(),
                },
                degraded: true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityId;

    fn added(mem: &str, slug: &str) -> ChangeEnvelope {
        ChangeEnvelope::Added {
            id: EntityId::new(mem, slug),
            title: None,
            entity_type: None,
        }
    }

    fn updated(mem: &str, slug: &str) -> ChangeEnvelope {
        ChangeEnvelope::Updated {
            id: EntityId::new(mem, slug),
            title: None,
            entity_type: None,
        }
    }

    fn removed(mem: &str, slug: &str) -> ChangeEnvelope {
        ChangeEnvelope::Removed {
            id: EntityId::new(mem, slug),
            title: None,
            entity_type: None,
        }
    }

    fn renamed(mem: &str, from: &str, to: &str) -> ChangeEnvelope {
        ChangeEnvelope::Renamed {
            from_id: EntityId::new(mem, from),
            to_id: EntityId::new(mem, to),
            title: None,
            entity_type: None,
        }
    }

    fn entry(mtime: i64, size: u64) -> super::super::change_detection::StatEntry {
        super::super::change_detection::StatEntry { mtime, size }
    }

    fn stat_map(pairs: &[(&str, i64, u64)]) -> StatMap {
        pairs
            .iter()
            .map(|(k, m, s)| ((*k).to_string(), entry(*m, *s)))
            .collect()
    }

    /// A 7–64 hex string is a git token; too short, too long, and non-hex
    /// (including an mtime digest JSON) are not.
    #[test]
    fn is_git_token_recognizes_hex_shas() {
        assert!(is_git_token("a1b2c3d")); // 7 hex
        assert!(is_git_token(&"a".repeat(40))); // canonical sha1 length
        assert!(is_git_token("ABCDEF0")); // case-insensitive
        assert!(!is_git_token("a1b2c3")); // 6 — too short
        assert!(!is_git_token(&"a".repeat(65))); // too long
        assert!(!is_git_token("not-hex")); // non-hex
        assert!(!is_git_token(
            r#"{"v":1,"count":2,"watermark":9,"aggregate":"x"}"#
        ));
        assert!(!is_git_token(""));
    }

    /// The graph mapping routes each action: removed → deleted, added →
    /// added, updated → modified, and a rename → new id added + old deleted.
    #[test]
    fn graph_mapping_routes_each_action() {
        let changes = vec![
            added("m", "new-a"),
            updated("m", "changed-b"),
            removed("m", "gone-c"),
            renamed("m", "old-d", "new-d"),
        ];
        let slice = graph_changes_to_slice(&changes);
        assert_eq!(
            slice.added,
            vec![
                EntityId::new("m", "new-a").as_ref().to_string(),
                EntityId::new("m", "new-d").as_ref().to_string(),
            ]
        );
        assert_eq!(
            slice.modified,
            vec![EntityId::new("m", "changed-b").as_ref().to_string()]
        );
        assert_eq!(
            slice.deleted,
            vec![
                EntityId::new("m", "gone-c").as_ref().to_string(),
                EntityId::new("m", "old-d").as_ref().to_string(),
            ]
        );
    }

    /// The graph outcome contract: invalid current → NoSignal; absent/foreign
    /// baseline → Reseed; equal → Unchanged; moved → Changed with the slice.
    #[test]
    fn graph_outcome_classifies_against_baseline() {
        let cur = "a".repeat(40);

        // current not a snapshot token → degrade.
        assert_eq!(
            graph_slice_outcome(Some(&cur), "not-a-sha", &[]),
            SliceOutcome::NoSignal {
                reason: NoSignalReason::GraphSnapshotMissing
            }
        );

        // no baseline → reseed at current.
        assert_eq!(
            graph_slice_outcome(None, &cur, &[]),
            SliceOutcome::Reseed { token: cur.clone() }
        );
        // foreign baseline token (mtime digest) → reseed.
        assert_eq!(
            graph_slice_outcome(Some("{\"v\":1}"), &cur, &[]),
            SliceOutcome::Reseed { token: cur.clone() }
        );

        // baseline == current → unchanged.
        assert_eq!(
            graph_slice_outcome(Some(&cur), &cur, &[]),
            SliceOutcome::Unchanged { token: cur.clone() }
        );

        // moved → changed with the mapped slice.
        let base = "b".repeat(40);
        match graph_slice_outcome(Some(&base), &cur, &[added("m", "x")]) {
            SliceOutcome::Changed {
                token,
                slice,
                degraded,
            } => {
                assert_eq!(token, cur);
                assert!(!degraded);
                assert_eq!(
                    slice.added,
                    vec![EntityId::new("m", "x").as_ref().to_string()]
                );
            }
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    /// The mtime outcome: unparseable baseline → reseed; equal digest →
    /// unchanged; moved with a memo → precise diff; moved without a memo →
    /// a degraded full scan.
    #[test]
    fn mtime_outcome_classifies_and_degrades() {
        let now = stat_map(&[("a.rs", 100, 10), ("b.rs", 200, 20)]);
        let token_now = serialize_digest_token(&digest_stat_map(&now));

        // No baseline → reseed at the current digest token.
        assert_eq!(
            mtime_slice_outcome(None, None, &now),
            SliceOutcome::Reseed {
                token: token_now.clone()
            }
        );
        // A git-sha baseline is not a digest token → reseed.
        assert_eq!(
            mtime_slice_outcome(Some(&"a".repeat(40)), None, &now),
            SliceOutcome::Reseed {
                token: token_now.clone()
            }
        );

        // Baseline digest equals current → unchanged.
        assert_eq!(
            mtime_slice_outcome(Some(&token_now), None, &now),
            SliceOutcome::Unchanged {
                token: token_now.clone()
            }
        );

        // Moved, with the prior map in the memo → precise diff.
        let prev = stat_map(&[("a.rs", 100, 10), ("gone.rs", 5, 5)]);
        let prev_token = serialize_digest_token(&digest_stat_map(&prev));
        match mtime_slice_outcome(Some(&prev_token), Some(&prev), &now) {
            SliceOutcome::Changed {
                token,
                slice,
                degraded,
            } => {
                assert_eq!(token, token_now);
                assert!(!degraded);
                assert_eq!(slice.added, vec!["b.rs"]);
                assert_eq!(slice.deleted, vec!["gone.rs"]);
                assert!(slice.modified.is_empty());
            }
            other => panic!("expected precise Changed, got {other:?}"),
        }

        // Moved, memo miss → degraded full scan (every current file added).
        match mtime_slice_outcome(Some(&prev_token), None, &now) {
            SliceOutcome::Changed {
                token,
                slice,
                degraded,
            } => {
                assert_eq!(token, token_now);
                assert!(degraded, "memo miss is a degraded full scan");
                assert_eq!(slice.added, vec!["a.rs", "b.rs"]);
                assert!(slice.modified.is_empty() && slice.deleted.is_empty());
            }
            other => panic!("expected degraded Changed, got {other:?}"),
        }
    }
}
