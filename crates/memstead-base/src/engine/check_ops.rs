//! The check operation and derived check state (agent-trust plan 14).
//!
//! `record_check` is the engine-recorded act of verification: it
//! appends one [`crate::check::CheckRecord`] — verdict, method note,
//! the entity's `content_hash` at check time, and plan-13 provenance
//! (actor, client, declared role) — to the workspace check ledger.
//! Checking mutates nothing: no entity write, no mem commit, no
//! `content_hash` change. That non-mutation is load-bearing — it is
//! what makes check-staleness derivable by hash comparison.
//!
//! `entity_check_state` derives never-checked | checked-ok |
//! check-failed | check-stale from the newest record against the
//! entity's current hash; the derivation lives in
//! [`crate::check::derive_state`] so surfaces and health share one
//! implementation.

use crate::check::{CheckLedger, CheckRecord, CheckState, Verdict, derive_state};
use crate::vcs::{Actor, ClientId};

use super::{Engine, error::EngineError};

impl Engine {
    /// Record a check of one entity. Refuses typed on unknown mem
    /// (quarantine included), unknown entity, read-only mounts, and
    /// on any persistence failure (`CHECK_NOT_RECORDED`) — recording
    /// is never best-effort, because a caller who believes an
    /// unrecorded check landed is the exact dishonesty this tier
    /// removes. The declared role rides engine session state
    /// ([`Engine::set_role`]), same as every mutation.
    pub fn record_check(
        &mut self,
        mem_name: &str,
        entity_id: &str,
        verdict: Verdict,
        method: Option<&str>,
        actor: Actor,
        client: Option<&ClientId>,
    ) -> Result<CheckRecord, EngineError> {
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }
        let entity_hash = self
            .store
            .all_entities()
            .find(|e| e.mem == mem_name && e.id.0 == entity_id)
            .map(|e| e.content_hash.clone())
            .ok_or_else(|| EngineError::NotFound {
                id: entity_id.to_string(),
            })?;
        let Some(root) = self.workspace_root() else {
            return Err(EngineError::CheckNotRecorded {
                reason: "engine has no workspace root — no durable check store".to_string(),
            });
        };
        let ledger = CheckLedger::for_workspace(root);
        let record = CheckRecord {
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            entity: entity_id.to_string(),
            verdict: verdict.as_str().to_string(),
            method: method
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_string),
            entity_hash,
            actor: actor.as_trailer().to_string(),
            client: client.map(|c| format!("{}@{}", c.name, c.version)),
            role: self
                .current_role()
                .as_trailer()
                .unwrap_or("unspecified")
                .to_string(),
        };
        ledger
            .record(&record)
            .map_err(|e| EngineError::CheckNotRecorded {
                reason: format!("ledger append failed: {e}"),
            })?;
        Ok(record)
    }

    /// Derive one entity's check state and newest check record.
    /// Refuses typed on unknown mem/entity; an engine with no
    /// workspace root has no check store and honestly derives
    /// `never_checked` (no recorded checks exist).
    pub fn entity_check_state(
        &self,
        mem_name: &str,
        entity_id: &str,
    ) -> Result<(CheckState, Option<CheckRecord>), EngineError> {
        self.find_mount(mem_name)?;
        let current_hash = self
            .store
            .all_entities()
            .find(|e| e.mem == mem_name && e.id.0 == entity_id)
            .map(|e| e.content_hash.clone())
            .ok_or_else(|| EngineError::NotFound {
                id: entity_id.to_string(),
            })?;
        let latest = self
            .workspace_root()
            .map(CheckLedger::for_workspace)
            .and_then(|l| l.latest_for(entity_id));
        Ok((derive_state(latest.as_ref(), &current_hash), latest))
    }
}

#[cfg(test)]
mod tests {
    use crate::check::Verdict;
    use crate::vcs::Actor;
    use crate::workspace::MountCapability;

    /// Criterion 5 complement: a read-only mount refuses a check
    /// typed (`READ_ONLY_MOUNT`) — capability gating runs before the
    /// entity lookup, same as every mutation-shaped guard.
    #[test]
    fn check_refuses_read_only_mounts_typed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut mount = crate::engine::test_helpers::folder_mount("ro", tmp.path().to_path_buf());
        mount.capability = MountCapability::ReadOnly;
        let mut engine = crate::Engine::from_mounts(vec![(
            mount,
            Box::new(crate::storage::FilesystemMemWriter::new(tmp.path().to_path_buf()))
                as Box<dyn crate::backend::MemBackend>,
        )])
        .unwrap();
        let err = engine
            .record_check("ro", "ro--anything", Verdict::Ok, None, Actor::Cli, None)
            .unwrap_err();
        assert_eq!(err.code(), "READ_ONLY_MOUNT");
    }
}
