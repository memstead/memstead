//! Mem membership follows reload-before-operation.
//!
//! Content drift is probed per operation ([`super::Engine::reload_if_stale`]
//! compares each mount's branch tip with the head it last loaded). Membership
//! used to be fixed at boot: a mem registered or unregistered by another
//! process (the CLI, a sibling server) stayed invisible, or kept being served,
//! until a restart — on 2026-09-02 a ui-api served a retired mem for hours.
//! This module is the membership half of the same discipline: before each
//! operation the engine compares a fingerprint of the mount roster
//! (`.memstead/state/mounts.json`) with the one it booted or last reconciled
//! on, and on a change mounts the new entries cold under the boot quarantine
//! rules, unmounts the gone entries atomically, remounts the entries whose
//! capability changed (every row counts, read-only ones included: a read
//! that was withdrawn must stop being served), re-scans the schema sources,
//! and reports the change (`MEM_ROSTER_CHANGED`) so an agent drops the cached
//! hashes of the mems that left.
//!
//! The probe is a `stat` plus, only when size or mtime moved, a hash of the
//! file — the same cost band as the branch-tip probe, never a boot.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{Engine, EngineError};

/// What the engine last saw of the roster file: size and mtime for the
/// cheap comparison, the content hash for the decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterFingerprint {
    len: u64,
    modified: Option<std::time::SystemTime>,
    hash: u64,
}

/// One applied roster reconciliation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterChange {
    /// Mems the roster gained and the engine mounted cold, ready to serve.
    /// A mem whose capability changed is here and under `removed`: it was
    /// unmounted and mounted again under the roster's capability.
    pub added: Vec<String>,
    /// Mems the roster lost and the engine unmounted: their entities,
    /// search index, community partition and derived views are gone.
    pub removed: Vec<String>,
    /// Mems the roster gained that failed to mount and are quarantined
    /// under the boot rules (each with its reason on the quarantine
    /// roster); the other mems keep serving.
    pub quarantined: Vec<String>,
    /// Per-item failures that were not a quarantine: the schema-source
    /// re-scan, or an unmount that could not complete (the roster change
    /// for that mem is not applied and it stays fully served).
    pub failures: Vec<crate::ops::RefreshFailure>,
}

impl RosterChange {
    /// Whether anything at all changed or failed.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.quarantined.is_empty()
            && self.failures.is_empty()
    }
}

/// The event a roster subscriber receives — the applied change.
pub type RosterChangedEvent = RosterChange;

/// Callback shape for roster subscribers, mirroring
/// [`super::events::EventCallback`].
pub type RosterCallback = Arc<dyn Fn(&RosterChangedEvent) + Send + Sync + 'static>;

/// The roster subscriber registry: the next id, then `(id, callback)`.
pub(crate) type RosterSubscribers = std::sync::Mutex<(u64, Vec<(u64, RosterCallback)>)>;

fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

impl Engine {
    /// The roster file this engine's membership is read from, when the
    /// engine knows its workspace root.
    fn roster_path(&self) -> Option<PathBuf> {
        self.workspace_root.as_ref().map(|root| {
            root.join(crate::workspace_store::WORKSPACE_STORE_DIR)
                .join("state")
                .join("mounts.json")
        })
    }

    /// The roster's current fingerprint. `Ok(None)` when the engine has no
    /// workspace root or the file does not exist (an ad-hoc mount list, a
    /// standalone folder mem). Size and mtime unchanged from the cached
    /// fingerprint short-circuits without reading the file.
    fn roster_fingerprint_now(&self) -> Result<Option<RosterFingerprint>, std::io::Error> {
        let Some(path) = self.roster_path() else {
            return Ok(None);
        };
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let len = meta.len();
        let modified = meta.modified().ok();
        if let Some(cached) = &self.roster_fingerprint
            && cached.len == len
            && cached.modified == modified
        {
            return Ok(Some(cached.clone()));
        }
        let bytes = std::fs::read(&path)?;
        Ok(Some(RosterFingerprint {
            len,
            modified,
            hash: hash_bytes(&bytes),
        }))
    }

    /// Capture the roster as it stands as the reconciliation baseline —
    /// what boot and every applied reconcile leave behind.
    pub(crate) fn capture_roster_fingerprint(&mut self) {
        self.roster_fingerprint = self.roster_fingerprint_now().ok().flatten();
    }

    /// Reconcile membership with the roster file: `Ok(None)` when the roster
    /// did not change since the last reconcile; `Ok(Some(change))` after
    /// applying a change; `Err` only when the roster could not be read or
    /// parsed at all (the baseline is kept, so the next operation retries).
    ///
    /// A roster file that did not exist at boot (a workspace whose first
    /// mem is created by another process after this engine started) is a
    /// change like any other: the first observation is applied, never
    /// swallowed as a baseline. Boot captures its own baseline through
    /// [`Engine::capture_roster_fingerprint`], so an engine that saw the
    /// file at boot is not re-mounted here.
    ///
    /// Per mem the change is atomic: an unmount that cannot complete leaves
    /// that mem fully served, is reported under `failures`, and keeps the
    /// baseline where it was so it is retried next time; a cold mount that
    /// fails quarantines the mem exactly as boot would. Every roster row
    /// counts, read-only ones included; a row whose capability changed is
    /// unmounted and mounted again under the new capability.
    pub fn reconcile_roster(&mut self) -> Result<Option<RosterChange>, EngineError> {
        let now = self.roster_fingerprint_now().map_err(|e| {
            EngineError::Backend(crate::backend::BackendError::Other(format!(
                "roster unreadable: {e}"
            )))
        })?;
        let Some(now) = now else {
            return Ok(None);
        };
        let Some(cached) = self.roster_fingerprint.clone() else {
            // No baseline: the file appeared after boot. Mount what it names.
            return self.apply_roster(now).map(Some);
        };
        if cached.hash == now.hash {
            self.roster_fingerprint = Some(now);
            return Ok(None);
        }
        self.apply_roster(now).map(Some)
    }

    /// The reconciliation without the fingerprint gate — what
    /// `full_refresh` runs so its report is authoritative even when the
    /// roster file did not move (a first observation, a probe that was
    /// skipped). An engine without a roster file yields an empty change.
    pub(crate) fn reconcile_roster_forced(&mut self) -> Result<RosterChange, EngineError> {
        let now = self.roster_fingerprint_now().map_err(|e| {
            EngineError::Backend(crate::backend::BackendError::Other(format!(
                "roster unreadable: {e}"
            )))
        })?;
        match now {
            Some(now) => self.apply_roster(now),
            None => Ok(RosterChange::default()),
        }
    }

    fn apply_roster(&mut self, now: RosterFingerprint) -> Result<RosterChange, EngineError> {
        let root = self
            .workspace_root
            .clone()
            .expect("a roster fingerprint implies a workspace root");
        let workspace = crate::workspace_store::WorkspaceStoreAdapter::load(
            &crate::workspace_store::FileWorkspaceStore::new(),
            &root,
        )
        .map_err(|e| {
            EngineError::Backend(crate::backend::BackendError::Other(format!(
                "roster under {} unreadable: {e}",
                root.display()
            )))
        })?;

        let cross_mem_links = workspace.settings.cross_mem_links.clone();
        // Every roster row counts, whatever its capability: a read-only
        // mount another process added or withdrew is membership like a
        // writable one, and a row whose capability changed is a mount to
        // take down and bring up again under the new capability.
        let manifest: BTreeMap<String, crate::workspace::MountCapability> = workspace
            .mounts
            .iter()
            .map(|m| (m.mem.clone(), m.capability))
            .collect();
        let mounted: BTreeMap<String, crate::workspace::MountCapability> = self
            .mounts
            .iter()
            .map(|m| (m.mount.mem.clone(), m.mount.capability))
            .collect();
        let quarantined_now: BTreeSet<String> = self
            .quarantined
            .iter()
            .map(|q| q.mount.mem.clone())
            .collect();

        let mut change = RosterChange::default();
        let mut all_applied = true;

        // The schema catalogue first: a mem that arrived may pin a schema
        // installed since boot, and a cold mount resolves against the
        // catalogue as it stands.
        let mut schema_report = crate::ops::FullRefreshReport::default();
        self.refresh_schema_sources(&mut schema_report);
        change.failures.extend(schema_report.failures);

        // Removals first, each atomic: the mems the roster no longer
        // names, and the ones it names under another capability. The
        // latter are mounted again below, so a capability change is
        // reported the way it happened, under removed and under added.
        let mut remounting: BTreeSet<String> = BTreeSet::new();
        for (name, capability) in &mounted {
            let roster_capability = manifest.get(name);
            if roster_capability == Some(capability) {
                continue;
            }
            match self.unmount_mem(name) {
                Ok(()) => {
                    change.removed.push(name.clone());
                    if roster_capability.is_some() {
                        remounting.insert(name.clone());
                    }
                }
                Err(e) => {
                    all_applied = false;
                    change.failures.push(crate::ops::RefreshFailure {
                        item: format!("unmount:{name}"),
                        error: e.to_string(),
                    });
                }
            }
        }
        // A gone quarantined entry just leaves the quarantine roster: it
        // served nothing. One that stays follows its row's capability, so
        // a later attach never restores an access the roster withdrew.
        let gone_quarantined: Vec<String> = quarantined_now
            .iter()
            .filter(|name| !manifest.contains_key(*name))
            .cloned()
            .collect();
        if !gone_quarantined.is_empty() {
            self.quarantined
                .retain(|q| !gone_quarantined.contains(&q.mount.mem));
            change.removed.extend(gone_quarantined);
        }
        for q in &mut self.quarantined {
            if let Some(capability) = manifest.get(&q.mount.mem) {
                q.mount.capability = *capability;
            }
        }

        // Additions: cold mount under the boot quarantine rules.
        let mut any_mounted = false;
        for mount in workspace.mounts {
            if (mounted.contains_key(&mount.mem) && !remounting.contains(&mount.mem))
                || quarantined_now.contains(&mount.mem)
            {
                continue;
            }
            let name = mount.mem.clone();
            // A mem that is mounted again under another capability was
            // never away for the caller: it must not read as unmounted
            // if the remount ends in quarantine.
            if remounting.contains(&name) {
                self.recently_unmounted.remove(&name);
            }
            let backend = match (self.backend_factory)(&mount) {
                Ok(b) => b,
                Err(e) => {
                    // Boot's reason for a mount that cannot be
                    // instantiated: a missing installed archive says
                    // what to install.
                    let (code, message) =
                        super::boot::instantiate_failure_reason(&mount, e.code(), e.to_string());
                    self.quarantine_mount(mount, &code, message);
                    change.quarantined.push(name);
                    continue;
                }
            };
            // Boot's rule for storage that is gone (a branch the mem-repo
            // lacks, a folder that does not exist): quarantine under
            // MOUNT_UNBACKED rather than serving an empty graph.
            if let Some(crate::ops::WarningHint::MountUnbacked { reason, .. }) =
                super::boot::unbacked_mount_warning(&mount, backend.as_ref())
                && reason != crate::ops::MountUnbackedReason::Empty
            {
                let (code, message) = super::boot::missing_storage_reason(&mount);
                self.quarantine_mount(mount, &code, message);
                change.quarantined.push(name);
                continue;
            }
            match self.register_writable_mem_batched(
                mount.clone(),
                backend,
                crate::mem::MemOrigin::ExplicitToml,
            ) {
                Ok(()) => {
                    any_mounted = true;
                    self.recently_unmounted.remove(&name);
                    change.added.push(name);
                }
                Err(e) => {
                    self.quarantine_mount(mount, e.code(), e.to_string());
                    change.quarantined.push(name);
                }
            }
        }
        if any_mounted {
            self.finish_batched_registrations();
        }

        // The cross-mem grants come from the same load. They are the one
        // policy that names mems, so they move with the roster (a rename
        // rewrites them with the mount) and must govern the next write as
        // the mounts do. The rest of the policy keeps its own refresh path.
        let mut current = self.settings().clone();
        current.cross_mem_links = cross_mem_links;
        self.set_settings(current);
        if all_applied {
            self.roster_fingerprint = Some(now);
        }
        self.invalidate_communities();
        self.invalidate_search_indexes();
        self.emit_roster_changed(&change);
        Ok(change)
    }

    fn quarantine_mount(&mut self, mount: crate::workspace::Mount, code: &str, message: String) {
        self.quarantined.push(super::QuarantinedMem {
            mount,
            reason_code: code.to_string(),
            reason_message: message,
        });
    }

    /// Unmount one mem, writable or read-only, atomically: every derived structure for it
    /// (store slice, schema entry, router slot, load warnings, pending
    /// change notices, community and search memos, its quarantine entry)
    /// is gone afterwards, or nothing is touched and the error names why.
    /// The mem is remembered as recently unmounted so an operation naming
    /// it refuses with `MEM_UNMOUNTED` rather than a bare unknown-mem.
    pub(crate) fn unmount_mem(&mut self, mem: &str) -> Result<(), EngineError> {
        #[cfg(test)]
        if self.inject_unmount_failure.as_deref() == Some(mem) {
            return Err(EngineError::Backend(crate::backend::BackendError::Other(
                format!("injected unmount failure for mem `{mem}`"),
            )));
        }
        // The read-only slot has its own unregistration: it clears the
        // router's read-only entry, which the writable one leaves alone.
        let read_only = self.mounts.iter().any(|m| {
            m.mount.mem == mem && m.mount.capability == crate::workspace::MountCapability::ReadOnly
        });
        let removed = if read_only {
            self.unregister_read_mount(mem)?
        } else {
            self.unregister_writable_mem(mem)?
        };
        if removed.is_none() {
            // Not mounted: a quarantined entry leaving the roster.
            self.quarantined.retain(|q| q.mount.mem != mem);
        }
        self.pending_mem_changed.retain(|n| n.mem != mem);
        self.callers.forget_mem(mem);
        self.labelling_memo = std::cell::OnceCell::new();
        self.recently_unmounted.insert(mem.to_string());
        Ok(())
    }

    /// Resolve `mem` for a write on the roster as it stands now: the
    /// roster is reconciled first, so a verb that judges writability
    /// before it takes the write lock never refuses on a capability the
    /// roster has since changed, and never accepts one it has withdrawn.
    /// Returns the mount index and the roster notice the reconciliation
    /// produced, for the caller's response.
    pub(in crate::engine) fn writable_mount_on_current_roster(
        &mut self,
        mem: &str,
    ) -> Result<(usize, Vec<crate::ops::WarningHint>), EngineError> {
        let notice = self.roster_warnings();
        let idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .ok_or_else(|| self.unknown_mem_error(mem))?;
        if self.mounts[idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem.to_string()));
        }
        Ok((idx, notice))
    }

    /// The capability the roster file gives `mem` at this instant, read
    /// without reconciling: for the one caller that attaches a mount the
    /// reconciliation does not touch (a quarantined mem on its way back
    /// into service) and must not attach it under a capability the roster
    /// has since changed. `None` when there is no roster file, it cannot
    /// be read, or it does not name the mem.
    pub(in crate::engine) fn roster_capability_on_disk(
        &self,
        mem: &str,
    ) -> Option<crate::workspace::MountCapability> {
        let root = self.workspace_root.as_ref()?;
        self.roster_path().filter(|p| p.is_file())?;
        let workspace = crate::workspace_store::WorkspaceStoreAdapter::load(
            &crate::workspace_store::FileWorkspaceStore::new(),
            root,
        )
        .ok()?;
        workspace
            .mounts
            .iter()
            .find(|m| m.mem == mem)
            .map(|m| m.capability)
    }

    /// Whether `mem` left the roster during this engine's lifetime and has
    /// not returned — the typed-refusal memory behind `MEM_UNMOUNTED`.
    pub fn recently_unmounted(&self, mem: &str) -> bool {
        self.recently_unmounted.contains(mem)
    }

    /// Subscribe to applied roster changes. Returns the subscription id;
    /// pass it to [`Self::unsubscribe_roster_changes`] to stop.
    pub fn subscribe_roster_changes(&self, callback: RosterCallback) -> u64 {
        let mut subs = self
            .roster_subscribers
            .lock()
            .expect("roster subscriber registry mutex must not be poisoned");
        let id = subs.0 + 1;
        subs.0 = id;
        subs.1.push((id, callback));
        id
    }

    /// Drop a roster subscription; a no-op for an unknown id.
    pub fn unsubscribe_roster_changes(&self, id: u64) {
        let mut subs = self
            .roster_subscribers
            .lock()
            .expect("roster subscriber registry mutex must not be poisoned");
        subs.1.retain(|(slot, _)| *slot != id);
    }

    fn emit_roster_changed(&self, change: &RosterChange) {
        if change.is_empty() {
            return;
        }
        let callbacks: Vec<RosterCallback> = self
            .roster_subscribers
            .lock()
            .expect("roster subscriber registry mutex must not be poisoned")
            .1
            .iter()
            .map(|(_, cb)| cb.clone())
            .collect();
        for cb in callbacks {
            cb(change);
        }
    }

    /// The set of mems this engine serves as writable, for tests and
    /// consumers that compare rosters.
    pub fn writable_mem_set(&self) -> HashSet<String> {
        self.mounts
            .iter()
            .filter(|m| m.mount.capability == crate::workspace::MountCapability::Write)
            .map(|m| m.mount.mem.clone())
            .collect()
    }
}
