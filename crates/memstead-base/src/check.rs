//! Check records — the engine-recorded act of verification
//! (agent-trust plan 14).
//!
//! A check is an agent recording "entity E checked, verdict ok |
//! failed, via method M". It is engine state, never entity content:
//! absent from markdown and `content_hash`, and it produces no mem
//! commit — checking-touches-nothing is what makes check-staleness
//! computable. Records are append-only JSONL under the workspace
//! store (`.memstead/state/checks/checks.jsonl`); a newer check
//! supersedes older ones for state derivation but never erases them.
//!
//! Unlike the friction ledger next door, recording here is NOT
//! best-effort: a check the ledger failed to persist must refuse —
//! the caller believes the act was recorded, and a silently dropped
//! check is exactly the self-report dishonesty this tier exists to
//! end. For the same reason there is no rotation cap: check history
//! is the substrate process state derives from, not disposable
//! telemetry.
//!
//! Each record carries plan-13 provenance (actor, client, declared
//! role) plus the entity's `content_hash` at check time. State
//! derivation compares that hash against the current one:
//!
//! - no record            → `never_checked`
//! - hash matches, ok     → `checked_ok`
//! - hash matches, failed → `check_failed`
//! - hash differs         → `check_stale` (whatever the verdict was,
//!   it no longer speaks to the current content — stated, never
//!   silently carried forward)

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The closed verdict vocabulary. Nuance goes in the method note or
/// in process-mem entities — never in new verdict values.
pub const VERDICTS: [&str; 2] = ["ok", "failed"];

/// A check verdict from the closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Failed,
}

impl Verdict {
    /// Parse a wire value; `None` for anything outside the vocabulary.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Self::Ok),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
        }
    }
}

/// One recorded check — the full ledger line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckRecord {
    /// Unix epoch seconds at record time.
    pub ts: u64,
    /// Full entity id (`mem--slug`).
    pub entity: String,
    /// `ok` | `failed`.
    pub verdict: String,
    /// Optional free-text method note ("diffed against source spec",
    /// "re-ran the derivation").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// The entity's `content_hash` at check time — the staleness
    /// baseline.
    pub entity_hash: String,
    /// Recorded actor identity (plan-13 provenance).
    pub actor: String,
    /// Recorded client identity (`name@version`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    /// The caller-declared role, or `"unspecified"` — recorded
    /// honestly; downstream gates treat unspecified as
    /// cannot-confirm, never as any real role.
    pub role: String,
}

/// Derived per-entity check state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    NeverChecked,
    CheckedOk,
    CheckFailed,
    CheckStale,
}

impl CheckState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NeverChecked => "never_checked",
            Self::CheckedOk => "checked_ok",
            Self::CheckFailed => "check_failed",
            Self::CheckStale => "check_stale",
        }
    }
}

/// Derive the state from the newest record (if any) and the entity's
/// current `content_hash`.
pub fn derive_state(latest: Option<&CheckRecord>, current_hash: &str) -> CheckState {
    match latest {
        None => CheckState::NeverChecked,
        Some(rec) if rec.entity_hash != current_hash => CheckState::CheckStale,
        Some(rec) if rec.verdict == "failed" => CheckState::CheckFailed,
        Some(_) => CheckState::CheckedOk,
    }
}

/// The ledger's directory under the workspace store:
/// `<root>/.memstead/state/checks/`.
fn checks_dir(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(crate::workspace_store::WORKSPACE_STORE_DIR)
        .join("state")
        .join("checks")
}

/// The ledger file path for a workspace.
pub fn check_ledger_path(workspace_root: &Path) -> PathBuf {
    checks_dir(workspace_root).join("checks.jsonl")
}

/// Append/read handle for a workspace's check ledger.
#[derive(Debug, Clone)]
pub struct CheckLedger {
    path: PathBuf,
}

impl CheckLedger {
    pub fn for_workspace(workspace_root: &Path) -> Self {
        Self {
            path: check_ledger_path(workspace_root),
        }
    }

    /// Append one record. One `write` syscall of one complete line on
    /// an `O_APPEND` handle — concurrent writers interleave whole
    /// lines, never tear them. Errors propagate: a check that did not
    /// persist must refuse at the surface.
    pub fn record(&self, rec: &CheckRecord) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut line = serde_json::to_string(rec).map_err(std::io::Error::other)?;
        line.push('\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(line.as_bytes())
    }

    /// All records, oldest first. A missing ledger is an empty one;
    /// unparseable lines are skipped (a torn tail must not poison the
    /// readable history).
    pub fn all(&self) -> Vec<CheckRecord> {
        let Ok(content) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        content
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// The newest record for one entity, if any.
    pub fn latest_for(&self, entity: &str) -> Option<CheckRecord> {
        self.all().into_iter().rev().find(|r| r.entity == entity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn rec(entity: &str, verdict: &str, hash: &str) -> CheckRecord {
        CheckRecord {
            ts: 1,
            entity: entity.to_string(),
            verdict: verdict.to_string(),
            method: None,
            entity_hash: hash.to_string(),
            actor: "cli".to_string(),
            client: None,
            role: "checker".to_string(),
        }
    }

    #[test]
    fn state_derivation_covers_all_four_states() {
        assert_eq!(derive_state(None, "h1"), CheckState::NeverChecked);
        let ok = rec("m--e", "ok", "h1");
        assert_eq!(derive_state(Some(&ok), "h1"), CheckState::CheckedOk);
        assert_eq!(derive_state(Some(&ok), "h2"), CheckState::CheckStale);
        let failed = rec("m--e", "failed", "h1");
        assert_eq!(derive_state(Some(&failed), "h1"), CheckState::CheckFailed);
        // A failed check on changed content is stale too — the verdict
        // no longer speaks to current content either way.
        assert_eq!(derive_state(Some(&failed), "h2"), CheckState::CheckStale);
    }

    #[test]
    fn ledger_appends_and_serves_newest_per_entity() {
        let tmp = TempDir::new().unwrap();
        let ledger = CheckLedger::for_workspace(tmp.path());
        assert!(ledger.latest_for("m--a").is_none());
        ledger.record(&rec("m--a", "failed", "h1")).unwrap();
        ledger.record(&rec("m--b", "ok", "h9")).unwrap();
        ledger.record(&rec("m--a", "ok", "h2")).unwrap();
        let latest = ledger.latest_for("m--a").unwrap();
        assert_eq!(latest.verdict, "ok");
        assert_eq!(latest.entity_hash, "h2");
        // Supersession never erases: all three records remain.
        assert_eq!(ledger.all().len(), 3);
    }

    #[test]
    fn verdict_vocabulary_is_closed() {
        assert!(Verdict::from_wire("ok").is_some());
        assert!(Verdict::from_wire("failed").is_some());
        assert!(Verdict::from_wire("passed").is_none());
        assert!(Verdict::from_wire("OK").is_none());
    }
}
