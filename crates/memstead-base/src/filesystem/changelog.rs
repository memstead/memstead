//! JSONL changelog at `.memstead/changes.jsonl` for filesystem mems.
//!
//! filesystem mems have no commit history, so the agent's `note` parameter and
//! the per-mutation provenance trail (timestamp, mutation type, entity
//! id, actor) lands here instead. Append-only; one line per mutation;
//! the file is created lazily on first append.
//!
//! ## Line shape
//!
//! Each line is a JSON object terminated by `\n`:
//!
//! ```json
//! {"ts":"2026-05-08T15:42:13.001Z","kind":"create","entity":"mem:slug","actor":"agent","note":"first draft"}
//! ```
//!
//! - `ts` — RFC 3339 timestamp with millisecond precision, always UTC
//!   (`Z` suffix). Sortable lexicographically.
//! - `kind` — mutation kind: `create`, `update`, `delete`, `relate`,
//!   `rename`, or `batch`. Future kinds extend the set; readers should
//!   tolerate unknown values.
//! - `entity` — mem-relative entity id (`mem:slug` form), or
//!   `null` for batch mutations that span multiple entities.
//! - `actor` — caller category from
//!   [`memstead_base::vcs::Actor::as_trailer`]: `agent`, `cli`, `external`,
//!   `unknown`.
//! - `note` — agent-authored provenance note when present. Omitted
//!   from the JSON object when absent or whitespace-only.
//! - `client` — optional caller identity (`name@version`). Omitted
//!   when absent.
//!
//! ## Atomicity
//!
//! Writes use `OpenOptions::append(true)` with a single `write_all`
//! call against the buffered line. POSIX guarantees `O_APPEND` writes
//! ≤ `PIPE_BUF` (4 KiB on Linux/macOS) appear atomically; the line
//! shape stays well under that. Concurrent writers from a single
//! process are serialised by the file's append-mode kernel lock.
//! Cross-process concurrency is out of scope (filesystem mems are single-writer
//! per the plan).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::provenance::ProvenanceKind;
use crate::vcs::{Actor, ClientId};

/// Conventional path of the changelog inside a workspace root.
pub fn changelog_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(crate::mem::MEM_META_DIR)
        .join("changes.jsonl")
}

/// Mutation kind written to the `kind` field of each line. The set
/// covers today's MCP mutating tools; new mutations extend the enum.
/// Readers branch on the string form (stable wire shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationKind {
    Create,
    Update,
    Delete,
    Relate,
    Rename,
    Batch,
}

impl MutationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MutationKind::Create => "create",
            MutationKind::Update => "update",
            MutationKind::Delete => "delete",
            MutationKind::Relate => "relate",
            MutationKind::Rename => "rename",
            MutationKind::Batch => "batch",
        }
    }
}

/// Two enums name the same six mutation classes — one is the legacy
/// folder-backend on-disk encoder, the other is the backend-neutral
/// shape consumed by [`crate::backend::MemBackend`]. Bridge here so
/// callers crossing between the two surfaces don't drift.
impl From<ProvenanceKind> for MutationKind {
    fn from(k: ProvenanceKind) -> Self {
        match k {
            ProvenanceKind::Create => MutationKind::Create,
            ProvenanceKind::Update => MutationKind::Update,
            ProvenanceKind::Delete => MutationKind::Delete,
            ProvenanceKind::Relate => MutationKind::Relate,
            ProvenanceKind::Rename => MutationKind::Rename,
            ProvenanceKind::Batch => MutationKind::Batch,
        }
    }
}

impl From<MutationKind> for ProvenanceKind {
    fn from(k: MutationKind) -> Self {
        match k {
            MutationKind::Create => ProvenanceKind::Create,
            MutationKind::Update => ProvenanceKind::Update,
            MutationKind::Delete => ProvenanceKind::Delete,
            MutationKind::Relate => ProvenanceKind::Relate,
            MutationKind::Rename => ProvenanceKind::Rename,
            MutationKind::Batch => ProvenanceKind::Batch,
        }
    }
}

/// A single mutation event. Built at the call site in the filesystem
/// engine (criterion 1's wiring) and passed to [`append_change`].
pub struct ChangeEntry<'a> {
    pub kind: MutationKind,
    /// Mem-relative entity id, or `None` for batch mutations.
    pub entity: Option<&'a str>,
    pub actor: Actor,
    /// Optional caller identity (e.g. `claude-code@2.1.0`).
    pub client: Option<&'a ClientId>,
    /// Optional agent-authored provenance note. Whitespace-only values
    /// are treated as absent.
    pub note: Option<&'a str>,
    /// Correlation id linking every commit produced by a single
    /// logical operation (notably multi-mem `memstead_rename`). `None`
    /// for single-call mutations that don't participate in
    /// correlation. Round-trips through the JSONL wire shape; reader
    /// reconstructs the same value on `read_provenance`.
    pub logical_operation_id: Option<&'a str>,
}

/// Errors surfaced by [`append_change`].
#[derive(Debug, thiserror::Error)]
pub enum ChangelogError {
    #[error("changelog io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("changelog serialisation: {0}")]
    Serialise(#[from] serde_json::Error),
}

/// Append a single change to `<workspace_root>/.memstead/changes.jsonl`.
/// Creates the `.memstead/` parent directory and the file if absent.
pub fn append_change(workspace_root: &Path, entry: &ChangeEntry<'_>) -> Result<(), ChangelogError> {
    let now = std::time::SystemTime::now();
    append_change_at(workspace_root, entry, now)
}

/// Variant of [`append_change`] that takes the timestamp explicitly.
/// Used by tests that need deterministic line ordering; production
/// callers go through [`append_change`].
pub fn append_change_at(
    workspace_root: &Path,
    entry: &ChangeEntry<'_>,
    now: std::time::SystemTime,
) -> Result<(), ChangelogError> {
    let target = changelog_path(workspace_root);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ChangelogError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }

    let ts = format_rfc3339_utc(now);
    let note = entry
        .note
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(|s| s.to_string());
    let client = entry.client.map(|c| format!("{}@{}", c.name, c.version));

    #[derive(Serialize)]
    struct Wire<'a> {
        ts: &'a str,
        kind: &'a str,
        entity: Option<&'a str>,
        actor: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        client: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", rename = "logical_op")]
        logical_operation_id: Option<&'a str>,
    }

    let mut line = serde_json::to_string(&Wire {
        ts: &ts,
        kind: entry.kind.as_str(),
        entity: entry.entity,
        actor: entry.actor.as_trailer(),
        note,
        client,
        logical_operation_id: entry.logical_operation_id,
    })?;
    line.push('\n');

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&target)
        .map_err(|e| ChangelogError::Io {
            path: target.clone(),
            source: e,
        })?;
    file.write_all(line.as_bytes())
        .map_err(|e| ChangelogError::Io {
            path: target,
            source: e,
        })?;
    Ok(())
}

/// Format a `SystemTime` as RFC 3339 with millisecond precision and
/// the `Z` suffix. Always UTC. Hand-rolled because the project does
/// not pull in `chrono` and the underlying `std::time::SystemTime` is
/// epoch-based.
///
/// Public so the folder-backend [`crate::backend::MemBackend`] impl
/// can reuse the same encoder when it constructs cursor strings.
pub fn format_rfc3339_utc(now: std::time::SystemTime) -> String {
    let dur = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = dur.as_secs();
    let millis = dur.subsec_millis();
    let (y, m, d, hh, mm, ss) = decompose_unix_seconds(total_secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{millis:03}Z")
}

/// Inverse of [`format_rfc3339_utc`]. Returns `None` when `s` is not
/// the exact 24-character `YYYY-MM-DDTHH:MM:SS.mmmZ` shape this
/// crate emits — round-trip precision is the contract; lenient
/// parsing is not. Used by the folder-backend `MemBackend::read_provenance`
/// impl when reconstructing [`crate::provenance::Provenance`] records
/// from on-disk JSONL lines.
pub fn parse_rfc3339_utc(s: &str) -> Option<std::time::SystemTime> {
    let bytes = s.as_bytes();
    if bytes.len() != 24
        || bytes[23] != b'Z'
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'.'
    {
        return None;
    }
    let year: i32 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let hour: u32 = s.get(11..13)?.parse().ok()?;
    let minute: u32 = s.get(14..16)?.parse().ok()?;
    let second: u32 = s.get(17..19)?.parse().ok()?;
    let millis: u32 = s.get(20..23)?.parse().ok()?;
    if month == 0
        || month > 12
        || day == 0
        || day > 31
        || hour > 23
        || minute > 59
        || second > 60
        || millis > 999
    {
        return None;
    }
    let days = ymd_to_days(year, month, day)?;
    let total_secs = days
        .checked_mul(86_400)?
        .checked_add(hour as i64 * 3_600)?
        .checked_add(minute as i64 * 60)?
        .checked_add(second as i64)?;
    if total_secs < 0 {
        return None;
    }
    Some(std::time::UNIX_EPOCH + std::time::Duration::new(total_secs as u64, millis * 1_000_000))
}

/// Civil-to-days inverse of the algorithm in [`decompose_unix_seconds`].
/// Returns days since 1970-01-01. Hinnant's algorithm.
fn ymd_to_days(year: i32, month: u32, day: u32) -> Option<i64> {
    let y = if month <= 2 {
        year as i64 - 1
    } else {
        year as i64
    };
    let m = month as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    if !(0..=399).contains(&yoe) {
        return None;
    }
    let doy_offset = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * doy_offset + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// Decompose a UNIX-epoch second count into (year, month, day, hour,
/// minute, second) in UTC. Algorithm from Howard Hinnant's
/// [chrono-Compatible Low-Level Date Algorithms]
/// (https://howardhinnant.github.io/date_algorithms.html).
///
/// Valid for any seconds-since-epoch value the caller is likely to
/// observe (`SystemTime` on POSIX is bounded by `time_t`, well within
/// the algorithm's i64-domain).
fn decompose_unix_seconds(total_secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    const SECONDS_PER_DAY: u64 = 86_400;
    let days = (total_secs / SECONDS_PER_DAY) as i64;
    let secs_of_day = (total_secs % SECONDS_PER_DAY) as u32;
    let hh = secs_of_day / 3_600;
    let mm = (secs_of_day / 60) % 60;
    let ss = secs_of_day % 60;

    // Civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe + era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 {
        (mp + 3) as u32
    } else {
        (mp - 9) as u32
    };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, hh, mm, ss)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::{Actor, ClientId};
    use tempfile::TempDir;

    fn read_lines(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|s| s.to_string())
            .collect()
    }

    fn ts(seconds: u64, millis: u32) -> std::time::SystemTime {
        std::time::UNIX_EPOCH + std::time::Duration::new(seconds, millis * 1_000_000)
    }

    #[test]
    fn appends_a_create_line_with_all_fields() {
        let tmp = TempDir::new().unwrap();
        let client = ClientId {
            name: "claude-code".into(),
            version: "2.1.0".into(),
        };
        append_change_at(
            tmp.path(),
            &ChangeEntry {
                kind: MutationKind::Create,
                entity: Some("spec:hello"),
                actor: Actor::Agent,
                client: Some(&client),
                note: Some("first draft"),
                logical_operation_id: None,
            },
            ts(1_715_000_000, 1),
        )
        .unwrap();

        let lines = read_lines(&changelog_path(tmp.path()));
        assert_eq!(lines.len(), 1);
        let value: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(value["kind"], "create");
        assert_eq!(value["entity"], "spec:hello");
        assert_eq!(value["actor"], "agent");
        assert_eq!(value["client"], "claude-code@2.1.0");
        assert_eq!(value["note"], "first draft");
        // Timestamp shape is RFC 3339 UTC, ms precision, sortable.
        let ts_str = value["ts"].as_str().unwrap();
        assert!(ts_str.ends_with("Z"));
        assert!(ts_str.contains("T"));
        // 2024-05-06T12:53:20 UTC + 1ms == seconds=1_715_000_000, ms=1.
        assert_eq!(ts_str, "2024-05-06T12:53:20.001Z");
    }

    #[test]
    fn omits_optional_fields_when_absent() {
        let tmp = TempDir::new().unwrap();
        append_change_at(
            tmp.path(),
            &ChangeEntry {
                kind: MutationKind::Update,
                entity: Some("spec:foo"),
                actor: Actor::Cli,
                client: None,
                note: None,
                logical_operation_id: None,
            },
            ts(0, 0),
        )
        .unwrap();
        let lines = read_lines(&changelog_path(tmp.path()));
        let value: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert!(value.get("note").is_none());
        assert!(value.get("client").is_none());
    }

    #[test]
    fn whitespace_only_note_is_treated_as_absent() {
        let tmp = TempDir::new().unwrap();
        append_change_at(
            tmp.path(),
            &ChangeEntry {
                kind: MutationKind::Update,
                entity: Some("spec:foo"),
                actor: Actor::Cli,
                client: None,
                note: Some("   \t   "),
                logical_operation_id: None,
            },
            ts(0, 0),
        )
        .unwrap();
        let lines = read_lines(&changelog_path(tmp.path()));
        let value: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert!(value.get("note").is_none());
    }

    #[test]
    fn batch_mutation_writes_null_entity() {
        let tmp = TempDir::new().unwrap();
        append_change_at(
            tmp.path(),
            &ChangeEntry {
                kind: MutationKind::Batch,
                entity: None,
                actor: Actor::Agent,
                client: None,
                note: Some("multi-entity refactor"),
                logical_operation_id: None,
            },
            ts(0, 0),
        )
        .unwrap();
        let lines = read_lines(&changelog_path(tmp.path()));
        let value: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert!(value["entity"].is_null());
        assert_eq!(value["kind"], "batch");
    }

    #[test]
    fn appends_create_then_update_in_order() {
        let tmp = TempDir::new().unwrap();
        for (kind, ent, t) in [
            (MutationKind::Create, "a", 0),
            (MutationKind::Update, "a", 1),
            (MutationKind::Delete, "a", 2),
        ] {
            append_change_at(
                tmp.path(),
                &ChangeEntry {
                    kind,
                    entity: Some(ent),
                    actor: Actor::Cli,
                    client: None,
                    note: None,
                    logical_operation_id: None,
                },
                ts(t, 0),
            )
            .unwrap();
        }

        let lines = read_lines(&changelog_path(tmp.path()));
        assert_eq!(lines.len(), 3);
        let kinds: Vec<String> = lines
            .iter()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line).unwrap()["kind"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(kinds, vec!["create", "update", "delete"]);
    }

    #[test]
    fn creates_memstead_parent_lazily() {
        let tmp = TempDir::new().unwrap();
        assert!(!tmp.path().join(".memstead").exists());
        append_change_at(
            tmp.path(),
            &ChangeEntry {
                kind: MutationKind::Create,
                entity: Some("a"),
                actor: Actor::Cli,
                client: None,
                note: None,
                logical_operation_id: None,
            },
            ts(0, 0),
        )
        .unwrap();
        assert!(tmp.path().join(".memstead").is_dir());
        assert!(tmp.path().join(".memstead").join("changes.jsonl").is_file());
    }

    #[test]
    fn does_not_create_memstead_until_first_change() {
        let tmp = TempDir::new().unwrap();
        // The `memstead init` step is responsible for creating `.memstead/`,
        // but the changelog must not require an extra setup call —
        // the helper creates the parent on demand. Verify that just
        // reading `changelog_path` does not touch disk.
        let _ = changelog_path(tmp.path());
        assert!(!tmp.path().join(".memstead").exists());
    }

    #[test]
    fn timestamp_handles_year_2026() {
        // Sanity-check the civil-from-days helper at a recent epoch
        // that does not sit on a friendly boundary. seconds=1_777_077_296
        // is 2026-04-25 00:34:56 UTC.
        let s = format_rfc3339_utc(ts(1_777_077_296, 0));
        assert_eq!(s, "2026-04-25T00:34:56.000Z");
    }

    #[test]
    fn timestamp_round_trips_a_known_2026_date() {
        // 2026-05-08T12:34:56 UTC. Verifies the helper holds across
        // months — leap-year handling at year boundaries lives in the
        // epoch and 2024 tests above.
        // Days since 1970-01-01:
        //   56 full years (14 leap) + day-of-year 127 = 20581 days
        // 20581 * 86400 + 12*3600 + 34*60 + 56 = 1_778_243_696
        let s = format_rfc3339_utc(ts(1_778_243_696, 0));
        assert_eq!(s, "2026-05-08T12:34:56.000Z");
    }

    #[test]
    fn timestamp_handles_epoch() {
        let s = format_rfc3339_utc(ts(0, 0));
        assert_eq!(s, "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn rfc3339_parser_round_trips_known_dates() {
        for &(secs, ms) in &[
            (0u64, 0u32),
            (1_715_000_000, 1),
            (1_777_077_296, 0),
            (1_778_243_696, 0),
            (1_778_243_696, 999),
        ] {
            let t = ts(secs, ms);
            let s = format_rfc3339_utc(t);
            let parsed = parse_rfc3339_utc(&s).expect("parse round-trip");
            assert_eq!(parsed, t, "round-trip failed for {s}");
        }
    }

    #[test]
    fn rfc3339_parser_rejects_malformed_input() {
        assert!(parse_rfc3339_utc("").is_none());
        assert!(parse_rfc3339_utc("2026-05-08T12:34:56Z").is_none()); // missing ms
        assert!(parse_rfc3339_utc("2026-05-08T12:34:56.000+02:00").is_none()); // wrong tz
        assert!(parse_rfc3339_utc("2026-13-08T12:34:56.000Z").is_none()); // bad month
        assert!(parse_rfc3339_utc("not a date at all aaaaa").is_none());
    }

    #[test]
    fn mutation_kind_str_is_stable() {
        // The string form is the wire shape — these values are read by
        // external tools (jq, grep). Lock them.
        assert_eq!(MutationKind::Create.as_str(), "create");
        assert_eq!(MutationKind::Update.as_str(), "update");
        assert_eq!(MutationKind::Delete.as_str(), "delete");
        assert_eq!(MutationKind::Relate.as_str(), "relate");
        assert_eq!(MutationKind::Rename.as_str(), "rename");
        assert_eq!(MutationKind::Batch.as_str(), "batch");
    }
}
