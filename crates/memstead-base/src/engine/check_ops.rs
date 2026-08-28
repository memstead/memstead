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

use crate::check::{CheckKind, CheckLedger, CheckRecord, CheckState, Verdict, derive_state};
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
    ///
    /// `kind` selects the closed check-kind vocabulary. A
    /// `conformance` record is bound to the mem's schema pin, stamped
    /// HERE from the mount — never caller-supplied, so a verdict
    /// cannot claim a prose version the caller never read; a mem with
    /// no pin refuses (`INVALID_INPUT`), because a semantic judgment
    /// against no schema binds to nothing.
    #[allow(clippy::too_many_arguments)] // the record's own fields, no natural grouping
    pub fn record_check(
        &mut self,
        mem_name: &str,
        entity_id: &str,
        verdict: Verdict,
        kind: CheckKind,
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
        let schema_ref = match kind {
            CheckKind::Verification => None,
            CheckKind::Conformance => Some(
                self.mounts[mount_idx]
                    .mount
                    .schema
                    .as_ref()
                    .map(|s| s.as_display())
                    .ok_or_else(|| {
                        EngineError::InvalidInput(format!(
                            "a conformance check binds to the mem's schema pin, and mem \
                             `{mem_name}` declares none"
                        ))
                    })?,
            ),
        };
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
            // The caller-declared identity rides engine session state
            // ([`Engine::set_identity`]), same as the role — absence
            // records as absence (plan 15).
            identity: self.current_identity().map(str::to_string),
            // Verification records omit the kind entirely, so a
            // kind-omitted caller's ledger lines stay byte-identical
            // to the pre-kind shape.
            kind: match kind {
                CheckKind::Verification => None,
                CheckKind::Conformance => Some(kind.as_str().to_string()),
            },
            schema_ref,
        };
        ledger
            .record(&record)
            .map_err(|e| EngineError::CheckNotRecorded {
                reason: format!("ledger append failed: {e}"),
            })?;
        Ok(record)
    }

    /// A [`crate::ops::health::CheckStateProvider`]-shaped closure over
    /// this engine's check ledger — the `transition_requires_checks`
    /// constraint's window into derived verification state. One ledger
    /// handle per closure; an engine without a workspace root derives
    /// every entity as `never_checked`, so a declared gate refuses
    /// honestly rather than passing unverified.
    pub(crate) fn check_state_provider(
        &self,
    ) -> impl Fn(&crate::entity::Entity) -> CheckState + '_ {
        let ledger = self.workspace_root().map(CheckLedger::for_workspace);
        move |entity: &crate::entity::Entity| match &ledger {
            None => CheckState::NeverChecked,
            Some(ledger) => derive_state(
                ledger
                    .latest_for_kind(&entity.id.0, CheckKind::Verification)
                    .as_ref(),
                &entity.content_hash,
            ),
        }
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
            .and_then(|l| l.latest_for_kind(entity_id, CheckKind::Verification));
        Ok((derive_state(latest.as_ref(), &current_hash), latest))
    }

    /// Derive one entity's `conformance` state and newest conformance
    /// record: hash staleness plus pin staleness (a re-pinned or
    /// unpinned mem stales the verdict — the prose it judged against
    /// is no longer the prose in force). Same refusals as
    /// [`Self::entity_check_state`].
    pub fn entity_conformance_state(
        &self,
        mem_name: &str,
        entity_id: &str,
    ) -> Result<(CheckState, Option<CheckRecord>), EngineError> {
        let mount = self.find_mount(mem_name)?;
        let current_pin = mount.mount.schema.as_ref().map(|s| s.as_display());
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
            .and_then(|l| l.latest_for_kind(entity_id, CheckKind::Conformance));
        Ok((
            crate::check::derive_state_pinned(
                latest.as_ref(),
                &current_hash,
                current_pin.as_deref(),
            ),
            latest,
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::check::{CheckKind, Verdict};
    use crate::vcs::Actor;
    use crate::workspace::MountCapability;

    /// A conformance check binds to the mem's schema pin; a mem that
    /// declares none cannot accept one. Today that refusal arrives as
    /// the quarantine gate (an unpinned mem quarantines at boot and
    /// serves nothing), which fires before the pin guard inside
    /// `record_check`; the guard's own `INVALID_INPUT` remains as
    /// defense in depth for any future backend that serves unpinned.
    #[test]
    fn conformance_refuses_without_a_schema_pin() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("anything.md"),
            "---\nid: anything\ntitle: Anything\ntype: note\n---\n\nBody.\n",
        )
        .unwrap();
        let mut mount = crate::engine::test_helpers::folder_mount("m", tmp.path().to_path_buf());
        mount.schema = None;
        let mut engine = crate::Engine::from_mounts(vec![(
            mount,
            Box::new(crate::storage::FilesystemMemWriter::new(
                tmp.path().to_path_buf(),
            )) as Box<dyn crate::backend::MemBackend>,
        )])
        .unwrap();
        let err = engine
            .record_check(
                "m",
                "m--anything",
                Verdict::Ok,
                CheckKind::Conformance,
                None,
                Actor::Cli,
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "MEM_QUARANTINED");
    }

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
            Box::new(crate::storage::FilesystemMemWriter::new(
                tmp.path().to_path_buf(),
            )) as Box<dyn crate::backend::MemBackend>,
        )])
        .unwrap();
        let err = engine
            .record_check(
                "ro",
                "ro--anything",
                Verdict::Ok,
                crate::check::CheckKind::Verification,
                None,
                Actor::Cli,
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "READ_ONLY_MOUNT");
    }
}
