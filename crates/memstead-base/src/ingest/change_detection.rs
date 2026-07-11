//! Source-change targeting primitives for the ingest loop's filesystem
//! (`mtime`) strategy.
//!
//! The ingest loop steers a fresh, memoryless iteration at the *changed*
//! slice of a source rather than re-roaming the whole thing. For sources
//! without a git work tree, "what changed" is computed from a per-file
//! `{mtime, size}` stat map: a file is **added** (new key), **modified**
//! (mtime or size differs), or **deleted** (key gone). Deletions are the
//! cheapest, highest-signal drift — a watermark (`max(mtime)`) is blind to
//! them, so a per-file map is used instead.
//!
//! Two artefacts come out of a stat map:
//!
//!   - a small [`Digest`] `{count, watermark, aggregate}` — the durable
//!     token the engine persists per `(ingest, facet)`. Byte-comparing two
//!     digests answers "did anything change since the last sync"; it
//!     survives a skill-cache wipe because it lives in engine mem config.
//!   - the **full map** ([`StatMap`]) — a rebuildable memo keyed by digest,
//!     used to compute *which* files changed. On memo miss the caller
//!     degrades to a one-tick full scan (detection from the digest still
//!     fires; only the precise slice is lost).
//!
//! Everything here is pure (no I/O except [`compute_stat_map`]'s `stat()`),
//! so the digest/diff logic is unit-testable without a workspace.
//!
//! The digest token is opaque to the engine — it stores and returns the
//! string verbatim. [`parse_digest_token`] is deliberately tolerant: an
//! unrecognized shape returns `None` ("no reliable signal"), never panics,
//! so a token produced by a different medium-type strategy (a git commit
//! id, say) degrades gracefully instead of aborting the run.
//!
//! **Port note.** This is a faithful port of the Claude-Code plugin's
//! `skills/ingest/scripts/change-detection.mjs`. The one deliberate change:
//! the `aggregate` hash uses SHA-256 (truncated to 16 hex chars), matching
//! the engine's existing [`crate::entity::parser::compute_hash`] convention,
//! where the plugin used SHA-1. The aggregate is an opaque content digest
//! only ever compared for equality against a token produced by the same
//! producer, so the algorithm is an internal detail — the preserved
//! behaviour is "same files with the same `(mtime, size)` ⇒ same digest;
//! any change ⇒ a different digest". Using SHA-256 keeps the port within
//! the crate's existing dependency closure (no new `sha1` dependency).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Digest token schema version. Tags the serialized token so a future
/// digest shape (or a foreign token — a git commit id, a graph snapshot
/// token) can be told apart by [`parse_digest_token`].
const DIGEST_VERSION: u32 = 1;

/// One file's stat signature: integer-millisecond mtime and byte size.
///
/// `mtime` is rounded to whole milliseconds so the value is stable across
/// JSON round-trips (mirroring the plugin's `Math.round(st.mtimeMs)`), and
/// `size` guards against mtime-preserving content writes (`cp -p`, tar
/// extraction) that leave the timestamp untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatEntry {
    /// Modification time in integer milliseconds since the Unix epoch.
    pub mtime: i64,
    /// File size in bytes.
    pub size: u64,
}

/// A stat map: workspace-relative path → [`StatEntry`]. A [`BTreeMap`] keeps
/// the keys sorted, which both the digest (stable hash over sorted tuples)
/// and the diff (sorted output classes) rely on.
pub type StatMap = BTreeMap<String, StatEntry>;

/// The durable digest of a [`StatMap`]. `count` is the entry count,
/// `watermark` the maximum mtime, `aggregate` a short content hash over the
/// sorted `(path, mtime, size)` tuples. Two maps produce the same digest
/// iff they hold the same files with the same `(mtime, size)`, so a digest
/// change is a reliable "something moved" trigger; `count`/`watermark`
/// shifts make additions and deletions visible even without the full map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    /// Number of files in the map.
    pub count: u64,
    /// Maximum mtime across the map (integer ms), `0` for an empty map.
    pub watermark: i64,
    /// SHA-256 over the sorted `(path, mtime, size)` tuples, first 16 hex.
    pub aggregate: String,
}

/// The classified difference between two stat maps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatDiff {
    /// Paths present only in the newer map.
    pub added: Vec<String>,
    /// Paths in both maps whose `mtime` *or* `size` differs.
    pub modified: Vec<String>,
    /// Paths present only in the older map (vanished files).
    pub deleted: Vec<String>,
}

/// The on-the-wire shape of a serialized digest token. Field order matches
/// the plugin's `serializeDigestToken` output (`v`, `count`, `watermark`,
/// `aggregate`) so a token round-trips through both producers identically.
#[derive(Debug, Serialize, Deserialize)]
struct TokenWire {
    v: u32,
    count: u64,
    watermark: i64,
    aggregate: String,
}

/// Convert a [`SystemTime`] to integer milliseconds since the Unix epoch,
/// rounded to the nearest whole millisecond (matching JS `Math.round`).
/// Times before the epoch are represented as negative milliseconds.
fn system_time_to_millis(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => (d.as_nanos() as f64 / 1_000_000.0).round() as i64,
        Err(e) => -((e.duration().as_nanos() as f64 / 1_000_000.0).round() as i64),
    }
}

/// Stat every relative path under `root` into a [`StatMap`]. Unreadable or
/// vanished paths, and non-file entries (directories, symlinks to nothing),
/// are skipped — a file that disappears between enumeration and stat simply
/// isn't in the map, which is the correct "deleted" signal on the next diff.
pub fn compute_stat_map<S: AsRef<str>>(rel_paths: &[S], root: &Path) -> StatMap {
    let mut map = StatMap::new();
    for rel in rel_paths {
        let rel = rel.as_ref();
        let md = match std::fs::metadata(root.join(rel)) {
            Ok(md) => md,
            // vanished or unreadable — omit; surfaces as a deletion next diff.
            Err(_) => continue,
        };
        if !md.is_file() {
            continue;
        }
        let mtime = md.modified().ok().map(system_time_to_millis).unwrap_or(0);
        map.insert(
            rel.to_string(),
            StatEntry {
                mtime,
                size: md.len(),
            },
        );
    }
    map
}

/// Reduce a [`StatMap`] to its durable [`Digest`].
pub fn digest_stat_map(map: &StatMap) -> Digest {
    let mut hasher = Sha256::new();
    let mut watermark: i64 = 0;
    // BTreeMap iterates in sorted key order, so the hash is order-stable.
    for (path, entry) in map {
        if entry.mtime > watermark {
            watermark = entry.mtime;
        }
        hasher.update(format!("{path}\0{}\0{}\n", entry.mtime, entry.size).as_bytes());
    }
    let aggregate = crate::hex_lower(&hasher.finalize())[..16].to_string();
    Digest {
        count: map.len() as u64,
        watermark,
        aggregate,
    }
}

/// Serialize a [`Digest`] into the opaque token string the engine persists.
pub fn serialize_digest_token(digest: &Digest) -> String {
    serde_json::to_string(&TokenWire {
        v: DIGEST_VERSION,
        count: digest.count,
        watermark: digest.watermark,
        aggregate: digest.aggregate.clone(),
    })
    .expect("digest token always serializes")
}

/// Parse a token back into a [`Digest`], or `None` if it isn't a recognized
/// mtime-digest token. Tolerant by contract: a malformed string, a git
/// commit id, a graph snapshot token, or a future-version digest all return
/// `None`, so the caller treats the source as having no usable baseline
/// (degrade, don't abort). The `&str` type already excludes the plugin's
/// `null`/non-string case at the boundary.
pub fn parse_digest_token(token: &str) -> Option<Digest> {
    if token.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(token).ok()?;
    let obj = value.as_object()?;
    if obj.get("v").and_then(serde_json::Value::as_u64) != Some(u64::from(DIGEST_VERSION)) {
        return None;
    }
    let count = obj.get("count")?.as_u64()?;
    let watermark = obj.get("watermark")?.as_i64()?;
    let aggregate = obj.get("aggregate")?.as_str()?.to_string();
    Some(Digest {
        count,
        watermark,
        aggregate,
    })
}

/// Two digests are equal iff every field matches. A missing digest on
/// either side (no baseline yet) is never equal — mirroring the plugin's
/// `digestsEqual(a, null) === false`.
pub fn digests_equal(a: Option<&Digest>, b: Option<&Digest>) -> bool {
    matches!((a, b), (Some(x), Some(y)) if x == y)
}

/// Diff two stat maps into added / modified / deleted (each a sorted path
/// list). A key only in `now` is added; only in `prev` is deleted; in both
/// with a differing `mtime` *or* `size` is modified.
pub fn diff_stat_maps(prev: &StatMap, now: &StatMap) -> StatDiff {
    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();
    for (path, b) in now {
        match prev.get(path) {
            None => added.push(path.clone()),
            Some(a) => {
                if a.mtime != b.mtime || a.size != b.size {
                    modified.push(path.clone());
                }
            }
        }
    }
    for path in prev.keys() {
        if !now.contains_key(path) {
            deleted.push(path.clone());
        }
    }
    // BTreeMap iteration already yields sorted order; the explicit sorts
    // make the contract independent of the map type and mirror the plugin.
    added.sort();
    modified.sort();
    deleted.sort();
    StatDiff {
        added,
        modified,
        deleted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn entry(mtime: i64, size: u64) -> StatEntry {
        StatEntry { mtime, size }
    }

    fn map(pairs: &[(&str, i64, u64)]) -> StatMap {
        pairs
            .iter()
            .map(|(k, m, s)| ((*k).to_string(), entry(*m, *s)))
            .collect()
    }

    /// A digest round-trips through serialize/parse unchanged.
    #[test]
    fn digest_token_round_trips() {
        let d = Digest {
            count: 3,
            watermark: 1_700_000_000_000,
            aggregate: "abc123def456abcd".to_string(),
        };
        let back = parse_digest_token(&serialize_digest_token(&d));
        assert_eq!(back, Some(d));
    }

    /// Unrecognized token shapes parse to `None` (no reliable signal): a git
    /// commit id, a future-version digest, junk, empty, and wrong types.
    #[test]
    fn unrecognized_tokens_parse_to_none() {
        assert_eq!(parse_digest_token("a1b2c3d4e5f6"), None); // git-oid-ish
        assert_eq!(parse_digest_token(r#"{"v":2,"count":1}"#), None); // future v
        assert_eq!(parse_digest_token("not json"), None);
        assert_eq!(parse_digest_token(""), None);
        assert_eq!(parse_digest_token(r#"{"v":1,"count":"x"}"#), None); // wrong type
    }

    /// `digests_equal` is true only when every field matches, and false when
    /// either side is absent (no baseline).
    #[test]
    fn digests_equal_requires_every_field() {
        let a = Digest {
            count: 1,
            watermark: 10,
            aggregate: "x".to_string(),
        };
        assert!(digests_equal(Some(&a), Some(&a.clone())));
        assert!(!digests_equal(
            Some(&a),
            Some(&Digest {
                count: 2,
                ..a.clone()
            })
        ));
        assert!(!digests_equal(
            Some(&a),
            Some(&Digest {
                watermark: 11,
                ..a.clone()
            })
        ));
        assert!(!digests_equal(
            Some(&a),
            Some(&Digest {
                aggregate: "y".to_string(),
                ..a.clone()
            })
        ));
        assert!(!digests_equal(Some(&a), None));
    }

    /// Identical maps digest identically regardless of insertion order; a
    /// size change or an mtime change each move the digest.
    #[test]
    fn digest_is_stable_and_change_sensitive() {
        let m1 = map(&[("a.rs", 100, 10), ("b.rs", 200, 20)]);
        // A BTreeMap normalizes order, so build the "reordered" map from the
        // reversed slice to prove insertion order is irrelevant.
        let m2 = map(&[("b.rs", 200, 20), ("a.rs", 100, 10)]);
        assert_eq!(
            digest_stat_map(&m1),
            digest_stat_map(&m2),
            "key order must not matter"
        );

        let mut m3 = m1.clone();
        m3.insert("a.rs".to_string(), entry(100, 11));
        assert_ne!(
            digest_stat_map(&m1),
            digest_stat_map(&m3),
            "a size change moves the digest"
        );

        let mut m4 = m1.clone();
        m4.insert("a.rs".to_string(), entry(101, 10));
        assert_ne!(
            digest_stat_map(&m1),
            digest_stat_map(&m4),
            "an mtime change moves the digest"
        );
    }

    /// `watermark` is the max mtime; `count` is the entry count.
    #[test]
    fn digest_watermark_and_count() {
        let d = digest_stat_map(&map(&[("a", 5, 1), ("b", 99, 1)]));
        assert_eq!(d.count, 2);
        assert_eq!(d.watermark, 99);
    }

    /// The diff classifies added / modified / deleted and treats an
    /// mtime-only touch and a size-only growth both as modified.
    #[test]
    fn diff_classifies_added_modified_deleted() {
        let prev = map(&[
            ("keep.rs", 100, 10),
            ("touch.rs", 100, 10),
            ("grow.rs", 100, 10),
            ("gone.rs", 100, 10),
        ]);
        let now = map(&[
            ("keep.rs", 100, 10),  // unchanged
            ("touch.rs", 200, 10), // mtime touched
            ("grow.rs", 100, 99),  // size grew (mtime preserved)
            ("new.rs", 300, 5),    // added
                                   // gone.rs deleted
        ]);
        let StatDiff {
            added,
            modified,
            deleted,
        } = diff_stat_maps(&prev, &now);
        assert_eq!(added, ["new.rs"]);
        assert_eq!(modified, ["grow.rs", "touch.rs"]);
        assert_eq!(deleted, ["gone.rs"]);
        assert!(
            !modified.contains(&"keep.rs".to_string()),
            "identical (mtime,size) is absent from the slice"
        );
    }

    /// Empty-vs-empty yields no changes.
    #[test]
    fn diff_empty_vs_empty() {
        assert_eq!(
            diff_stat_maps(&StatMap::new(), &StatMap::new()),
            StatDiff {
                added: vec![],
                modified: vec![],
                deleted: vec![],
            }
        );
    }

    /// `compute_stat_map` stats listed files, reflects their size, skips a
    /// path that does not exist, and skips a directory (non-file). A freshly
    /// written file carries a populated integer-ms mtime.
    #[test]
    fn compute_stat_map_over_a_real_directory() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path();
        fs::create_dir_all(base.join("sub")).unwrap();
        fs::write(base.join("a.txt"), "hello").unwrap();
        fs::write(base.join("sub/b.txt"), "worldworld").unwrap();

        let paths = ["a.txt", "sub/b.txt", "missing.txt", "sub"];
        let m = compute_stat_map(&paths, base);

        assert!(
            !m.contains_key("missing.txt"),
            "a path that does not exist is omitted"
        );
        assert!(
            !m.contains_key("sub"),
            "a directory is not a file — skipped"
        );
        assert_eq!(m["a.txt"].size, 5);
        assert_eq!(m["sub/b.txt"].size, 10);
        assert!(
            m["a.txt"].mtime > 0,
            "a freshly written file has a populated integer-ms mtime"
        );
    }
}
