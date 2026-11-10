//! Cross-process write serialisation per mem.
//!
//! Every mutation runs reload-before-operation, compares the caller's
//! `expected_hash`, then commits. Those three steps must be one atomic
//! unit across processes: if a sibling process commits between this
//! process's reload and its commit, the compare runs against a snapshot
//! that is already behind, and a stale write lands over the sibling's
//! (a lost update reported as success). The guard below holds an
//! exclusive OS file lock on the mem's [`MemBackend::write_lock_path`]
//! for the whole unit, so a second writer blocks until the first has
//! committed and then reloads, sees the new content, and gets its honest
//! `HASH_MISMATCH`.
//!
//! The lock is reentrant per thread ([`crate::storage::path_lock`]): a
//! mutation that calls another mutation entry point (a batch over
//! single-item paths, a merge over creates) takes the same lock again
//! without blocking on itself. Two threads, or two engines on two
//! threads, still exclude each other.
//!
//! The guard is also the one exit every mutation shares for its staged
//! writes. A mutation stages bytes into the backend and commits them
//! later; an error between the two (a refused sidecar write, a failed
//! commit, a `?` in any verb) used to leave the staged bytes in the
//! backend, where the next read served them as content that never
//! landed. When the outermost guard of a thread drops, it discards
//! whatever is still staged on the mems it locked. After a commit
//! nothing is staged and the discard is a no-op, so a call site has no
//! failure path to remember: holding the lock is the cleanup.
//!
//! [`MemBackend::write_lock_path`]: crate::backend::MemBackend::write_lock_path

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::backend::{BackendError, MemBackend};
use crate::storage::path_lock;

use super::{Engine, EngineError};

/// Held for the duration of one mutation; dropping it discards the
/// writes still staged on the mems it covers and releases every lock
/// it took (both when the last nested holder drops).
#[must_use = "the write lock is released as soon as the guard drops"]
pub struct MemWriteGuard {
    /// Each covered mem's backend with its lock-file path. A backend
    /// with no shared on-disk state has no path and takes no OS lock,
    /// but its staged writes are discarded like any other's.
    held: Vec<Held>,
}

/// One mem a guard covers.
struct Held {
    path: Option<PathBuf>,
    backend: Arc<dyn MemBackend>,
    /// The mount's record of a discard that failed
    /// ([`super::MountedBackend::strays`]). The guard cannot reach the
    /// engine when it drops, so it shares this flag with the mount.
    strays: Arc<std::sync::atomic::AtomicBool>,
}

thread_local! {
    /// How many guards of this thread cover a backend, by its address.
    /// Only the outermost one discards: an inner guard dropping must
    /// not empty a staged set the outer mutation is still building.
    static COVERED: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());
}

fn backend_key(backend: &Arc<dyn MemBackend>) -> usize {
    Arc::as_ptr(backend) as *const () as usize
}

impl MemWriteGuard {
    fn empty() -> Self {
        Self { held: Vec::new() }
    }

    fn cover(&mut self, held: Held) {
        COVERED.with(|c| {
            *c.borrow_mut()
                .entry(backend_key(&held.backend))
                .or_insert(0) += 1
        });
        self.held.push(held);
    }
}

impl Drop for MemWriteGuard {
    fn drop(&mut self) {
        for Held {
            path,
            backend,
            strays,
        } in self.held.iter().rev()
        {
            let outermost = COVERED.with(|c| {
                let mut covered = c.borrow_mut();
                let key = backend_key(backend);
                match covered.get_mut(&key) {
                    Some(count) if *count > 1 => {
                        *count -= 1;
                        false
                    }
                    _ => {
                        covered.remove(&key);
                        true
                    }
                }
            });
            // Discard before the lock goes, so no other writer can
            // stage between the two. A discard that fails leaves the
            // staged writes where a read or the next commit would pick
            // them up, so the backend is remembered and the engine
            // discards again before its next operation.
            if outermost {
                strays.store(
                    backend.discard_pending().is_err(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }
            if let Some(path) = path {
                path_lock::release(path);
            }
        }
    }
}

impl Engine {
    /// Discard what a failed mutation left staged on `mem`'s backend,
    /// if an earlier discard failed. Run before every operation, ahead
    /// of anything that reads the backend: a staged write is served in
    /// place of the committed bytes, and the next commit would land it.
    ///
    /// When the discard fails again, the backend is replaced by a fresh
    /// one over the same storage: what is staged lives in the backend
    /// value and nowhere on disk, so a new value has none of it. Only a
    /// mount whose storage is on disk is replaced (an in-memory mem is
    /// its backend), and not while a mutation of this thread holds the
    /// backend. An error means the staged writes are still there.
    pub(super) fn discard_strays(&mut self, mem: &str) -> Result<(), BackendError> {
        use std::sync::atomic::Ordering;
        let Some(idx) = self.mounts.iter().position(|m| m.mount.mem == mem) else {
            return Ok(());
        };
        let state = &self.mounts[idx];
        if !state.strays.load(Ordering::SeqCst) {
            return Ok(());
        }
        let failure = match state.backend.discard_pending() {
            Ok(()) => {
                state.strays.store(false, Ordering::SeqCst);
                return Ok(());
            }
            Err(e) => e,
        };
        let on_disk = matches!(
            state.mount.storage,
            crate::workspace::MountStorage::Folder { .. }
                | crate::workspace::MountStorage::GitBranch { .. }
        );
        let held_by_a_mutation = COVERED.with(|c| {
            c.borrow()
                .get(&backend_key(&state.backend))
                .is_some_and(|count| *count > 0)
        });
        if on_disk
            && !held_by_a_mutation
            && let Ok(fresh) = (self.backend_factory)(&state.mount)
        {
            self.mounts[idx].backend = Arc::from(fresh);
            self.mounts[idx].strays = Default::default();
            return Ok(());
        }
        Err(failure)
    }

    /// The lock and the view for a verb that learns what it writes, or
    /// what its guard reads, only by reading the workspace (a delete's
    /// and a retype's referrer check, a rename's referrers). Such a verb
    /// cannot name its mems up front, and a view of a mem taken before
    /// that mem's lock is held can miss what a sibling wrote into it. So
    /// it takes every writable mount's lock, in the one sorted order the
    /// lock defines, after the roster is reconciled (a mem attached
    /// since is locked too), and only then reloads every mem and loads
    /// the ones whose loading was deferred. No lock is taken after
    /// planning began, so there is nothing to restart.
    ///
    /// A mem that cannot be reloaded refuses the verb with
    /// `MEM_VIEW_STALE`: its view is from before the lock, and a guard
    /// that reads every mem would be evaluated on it. So does a mem
    /// that is out of service (quarantined), which is in no view.
    ///
    /// `writes` false (a rehearsal) takes no lock and still brings the
    /// view up to date.
    pub fn lock_workspace_for_write_if(
        &mut self,
        writes: bool,
    ) -> Result<(MemWriteGuard, Vec<crate::ops::WarningHint>), EngineError> {
        let mut warnings = self.roster_before_locking();
        let (guard, reloaded) = self.lock_whole_workspace(writes)?;
        warnings.extend(reloaded);
        Ok((guard, warnings))
    }

    fn lock_whole_workspace(
        &mut self,
        writes: bool,
    ) -> Result<(MemWriteGuard, Vec<crate::ops::WarningHint>), EngineError> {
        // Every mount that has a write lock, the ones this engine mounts
        // read-only included: another process may mount the same
        // storage writable, and a guard that reads it needs its writer
        // to wait like any other.
        let (guard, mut warnings) = self.lock_on_a_settled_roster(writes, |engine| {
            engine
                .mounts
                .iter()
                .filter(|m| {
                    m.mount.capability == crate::workspace::MountCapability::Write
                        || m.backend.write_lock_path().is_some()
                })
                .map(|m| m.mount.mem.clone())
                .collect()
        })?;
        warnings.extend(self.reload_if_stale_content_only(None));
        if let Some(stale) = self.mounts.iter().find(|m| m.reload_failure.is_some()) {
            return Err(EngineError::StaleView {
                mem: stale.mount.mem.clone(),
                detail: stale.reload_failure.clone().unwrap_or_default(),
            });
        }
        // A mem that is out of service is in no view at all: one whose
        // deferred load just failed under this lock, or one that never
        // attached. Its referrers are as unread as a stale mem's.
        if let Some(out) = self.quarantined.first() {
            return Err(EngineError::StaleView {
                mem: out.mount.mem.clone(),
                detail: format!("the mem is out of service: {}", out.reason_code),
            });
        }
        Ok((guard, warnings))
    }

    /// Take the locks `choose` names, and hold them over a roster that
    /// did not change while this process waited for them. The process
    /// that held a lock may have changed the roster before it let go (a
    /// mem rename takes the mem away under the lock a writer into it
    /// is waiting on; a mem created meanwhile is one more writable
    /// mount). So the roster is reconciled once more under the locks;
    /// when that changes the mounts, the locks are given back and
    /// chosen again. Nothing is planned before this returns.
    fn lock_on_a_settled_roster(
        &mut self,
        writes: bool,
        choose: impl Fn(&Self) -> Vec<String>,
    ) -> Result<(MemWriteGuard, Vec<crate::ops::WarningHint>), EngineError> {
        let nested = COVERED.with(|c| !c.borrow().is_empty());
        if nested || !writes {
            return Ok((
                self.lock_mems_for_write_if(writes, &choose(self))?,
                Vec::new(),
            ));
        }
        let mut warnings = Vec::new();
        // A roster that keeps changing is not waited out for ever, and
        // no lock set is kept over a roster that changed under it: the
        // call is refused, and sent again it starts from the roster as
        // it is then.
        for _ in 0..3 {
            let guard = self.lock_mems_for_write(&choose(self))?;
            let changed = self.roster_warnings();
            if changed.is_empty() {
                return Ok((guard, warnings));
            }
            drop(guard);
            warnings.extend(changed);
        }
        Err(EngineError::Backend(BackendError::Other(
            "the workspace's mount roster changed each time this operation had taken its write \
             locks; nothing was written, send the call again"
                .to_string(),
        )))
    }

    /// The lock and the view of a verb that names the mems it writes.
    /// `locked` are the mems whose lock it takes, `written` the ones
    /// it writes (reloaded, and loaded when deferred); every other
    /// loaded mem is reloaded too ([`Self::level_for_write`]).
    ///
    /// When `reads_workspace` says so, the verb is one that learns its
    /// write set or its guard's read set by reading the workspace after
    /// all, and gets [`Self::lock_workspace_for_write_if`] instead:
    /// a call that names an entity by a bare slug (which mem carries
    /// the slug is a fact about every mem), and a call that adds an
    /// edge on an acyclic relationship (the cycle guard walks edges in
    /// every mem, and two writers that each hold one mem's lock could
    /// each add the edge that closes the other's cycle). The question
    /// is asked after the roster is reconciled and before any lock is
    /// taken.
    ///
    /// A mutation running inside another one keeps the outer one's
    /// locks: taking further locks there would take them out of order.
    pub(in crate::engine) fn lock_for_verb<S: AsRef<str>>(
        &mut self,
        writes: bool,
        locked: &[S],
        written: &[S],
        reads_workspace: impl Fn(&Self) -> bool,
    ) -> Result<(MemWriteGuard, Vec<crate::ops::WarningHint>), EngineError> {
        let nested = COVERED.with(|c| !c.borrow().is_empty());
        let mut warnings = self.roster_before_locking();
        if !nested && reads_workspace(self) {
            let (guard, reloaded) = self.lock_whole_workspace(writes)?;
            warnings.extend(reloaded);
            return Ok((guard, warnings));
        }
        let names: Vec<String> = locked.iter().map(|m| m.as_ref().to_string()).collect();
        let (guard, settled) = self.lock_on_a_settled_roster(writes, |_| names.clone())?;
        warnings.extend(settled);
        warnings.extend(self.level_for_write(written));
        // The question again, on the view taken under the locks: what
        // it reads (a schema's relationships) may have changed since
        // it was asked. A yes now gives the locks back and takes every
        // mount, before anything is planned.
        if writes && !nested && reads_workspace(self) {
            drop(guard);
            let (guard, reloaded) = self.lock_whole_workspace(writes)?;
            warnings.extend(reloaded);
            return Ok((guard, warnings));
        }
        Ok((guard, warnings))
    }

    /// [`Self::lock_for_verb`] for a verb whose references depend on
    /// what is in storage: an update that appends to a section or
    /// patches one adds whatever link the resulting body holds, and
    /// that body is the stored text with the call's text worked in.
    /// `refer` names the mems the verb's result refers to, from the
    /// engine's view. It is asked before the locks are taken and again
    /// under them, on the reloaded view; when the second answer names
    /// a mem the first did not, the locks are given back and taken
    /// again with it. Nothing is planned before the two agree.
    pub(in crate::engine) fn lock_for_verb_referring(
        &mut self,
        writes: bool,
        written: &[String],
        refer: impl Fn(&Self) -> Vec<String>,
        reads_workspace: impl Fn(&Self) -> bool,
    ) -> Result<(MemWriteGuard, Vec<crate::ops::WarningHint>), EngineError> {
        let mut warnings = Vec::new();
        let mut referred = refer(self);
        for _ in 0..3 {
            let mut locked = referred.clone();
            locked.extend(written.iter().cloned());
            let (guard, more) =
                self.lock_for_verb(writes, &locked, written, |engine| reads_workspace(engine))?;
            warnings.extend(more);
            let under_lock = refer(self);
            if !writes || under_lock.iter().all(|mem| referred.contains(mem)) {
                return Ok((guard, warnings));
            }
            drop(guard);
            for mem in under_lock {
                if !referred.contains(&mem) {
                    referred.push(mem);
                }
            }
        }
        Err(EngineError::Backend(BackendError::Other(
            "what this write refers to changed each time it had taken its write locks; nothing \
             was written, send the call again"
                .to_string(),
        )))
    }

    /// Whether an edge of `rel_type` out of `mem` is guarded against
    /// cycles: the relationship is acyclic, or it is in an acyclic set.
    ///
    /// The name is judged in the form the verb will write it in: a
    /// caller may send `depends_on`, and the lock is decided before the
    /// verb normalises its arguments. A name that does not normalise is
    /// refused by the verb, and no edge comes of it.
    pub(in crate::engine) fn rel_type_is_acyclic(&self, mem: &str, rel_type: &str) -> bool {
        let Ok(rel_type) = crate::entity::id::validate_rel_type(rel_type) else {
            return false;
        };
        self.schemas.get(mem).is_some_and(|s| {
            s.relationship_acyclic(&rel_type) || s.acyclic_set_containing(&rel_type).is_some()
        })
    }

    /// The lock of a verb that names its mems: the roster is reconciled
    /// first, so a mem attached to the workspace since this engine
    /// started is mounted, and therefore locked, before the first write
    /// into it. Returns what the reconcile reported; the caller takes
    /// its view under the lock ([`Self::level_for_write`]) and looks
    /// its mounts up after this call (a reconcile moves them).
    pub fn lock_for_write<S: AsRef<str>>(
        &mut self,
        writes: bool,
        mems: &[S],
    ) -> Result<(MemWriteGuard, Vec<crate::ops::WarningHint>), EngineError> {
        let mut warnings = self.roster_before_locking();
        let names: Vec<String> = mems.iter().map(|m| m.as_ref().to_string()).collect();
        let (guard, settled) = self.lock_on_a_settled_roster(writes, |_| names.clone())?;
        warnings.extend(settled);
        Ok((guard, warnings))
    }

    /// The view a verb plans from, taken under its locks: the mems it
    /// writes are reloaded (a deferred one is loaded: a write into a
    /// mem needs the mem), and so is every other mem that is loaded.
    /// The mems the write refers to are among its locks
    /// ([`referred_mems`]), so what it checks of a relation target
    /// (that it exists, what type it has) is read from a view taken
    /// under the target's mem's lock and stays true until the commit. A
    /// check that reads further than the mems a call names, the cycle
    /// guard, does not run under this view: its verb takes the whole
    /// workspace ([`Self::lock_for_verb`]). A deferred mem that is not
    /// written stays unloaded; a target in it is checked against its
    /// storage, under its lock.
    pub(in crate::engine) fn level_for_write<S: AsRef<str>>(
        &mut self,
        written: &[S],
    ) -> Vec<crate::ops::WarningHint> {
        let mut mems: Vec<String> = written.iter().map(|m| m.as_ref().to_string()).collect();
        for m in &self.mounts {
            if !m.deferred && !mems.contains(&m.mount.mem) {
                mems.push(m.mount.mem.clone());
            }
        }
        let mut warnings = Vec::new();
        for mem in &mems {
            warnings.extend(self.reload_if_stale_content_only(Some(mem)));
        }
        warnings
    }

    /// The mount a write into `mem` goes to. Looked up after
    /// [`Self::lock_for_write`], whose roster reconcile can move the
    /// mounts.
    pub(in crate::engine) fn writable_mount_index(&self, mem: &str) -> Result<usize, EngineError> {
        let idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .ok_or_else(|| self.unknown_mem_error(mem))?;
        if self.mounts[idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem.to_string()));
        }
        Ok(idx)
    }

    /// Reconcile the roster ahead of a lock. A mutation that runs
    /// inside another one (a batch over the single-item path) holds its
    /// locks already and has begun planning: the membership it planned
    /// against stays as it is until the outer mutation is done.
    fn roster_before_locking(&mut self) -> Vec<crate::ops::WarningHint> {
        if COVERED.with(|c| c.borrow().is_empty()) {
            self.roster_warnings()
        } else {
            Vec::new()
        }
    }

    /// [`Self::lock_mems_for_write`] when `writes` is true; an empty
    /// guard otherwise. A rehearsal (`dry_run`) commits nothing, so it
    /// takes no lock and leaves the workspace byte-identical, lock files
    /// included.
    pub fn lock_mems_for_write_if<S: AsRef<str>>(
        &self,
        writes: bool,
        mems: &[S],
    ) -> Result<MemWriteGuard, EngineError> {
        if writes {
            self.lock_mems_for_write(mems)
        } else {
            Ok(MemWriteGuard::empty())
        }
    }

    /// Take the cross-process write lock of every named mem, in sorted
    /// order (so two multi-mem writers can never deadlock), and hold it
    /// until the returned guard drops. Call it before the
    /// reload-before-operation probe of a mutation and keep the guard
    /// alive past the commit. Mems that are not mounted are skipped:
    /// the mutation's own lookup reports an unknown mem with its usual
    /// error. A backend with no shared state takes no OS lock and is
    /// still covered for the staged-write discard.
    pub fn lock_mems_for_write<S: AsRef<str>>(
        &self,
        mems: &[S],
    ) -> Result<MemWriteGuard, EngineError> {
        let mut locks: Vec<(Held, bool)> = mems
            .iter()
            .filter_map(|name| {
                let m = self.mounts.iter().find(|m| m.mount.mem == name.as_ref())?;
                let writable = m.mount.capability == crate::workspace::MountCapability::Write;
                Some((
                    Held {
                        path: m.backend.write_lock_path(),
                        backend: Arc::clone(&m.backend),
                        strays: Arc::clone(&m.strays),
                    },
                    writable,
                ))
            })
            .collect();
        // Sorted by lock path so two multi-mem writers take their locks
        // in one order; pathless backends take no lock and sort first.
        locks.sort_by(|a, b| a.0.path.cmp(&b.0.path));
        locks.dedup_by(|a, b| backend_key(&a.0.backend) == backend_key(&b.0.backend));
        let mut guard = MemWriteGuard::empty();
        for (held, writable) in locks {
            if let Some(path) = &held.path {
                match path_lock::acquire(path) {
                    Ok(()) => {}
                    // A mem mounted read-only may sit where this
                    // process cannot create a lock file. It is locked
                    // when it can be; no write of this engine goes
                    // there either way.
                    Err(_) if !writable => continue,
                    Err(e) => return Err(EngineError::Backend(BackendError::Io(e))),
                }
            }
            guard.cover(held);
        }
        Ok(guard)
    }
}

/// The mems a create or an update refers to besides the one it writes:
/// the mems of its declared relation targets and of the wiki-links in
/// the text the call brings. The write takes their locks with its own.
/// A delete, a rename or a retype of a target holds the target's mem,
/// so a reference cannot be written while one is under way, whenever
/// the writing mem was attached; the write waits, reloads, and is
/// computed against the target as that verb left it.
pub(in crate::engine) fn referred_mems<'a>(
    own: &str,
    targets: impl IntoIterator<Item = &'a crate::entity::EntityId>,
    texts: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut mems: Vec<String> = targets.into_iter().map(|t| t.mem().to_string()).collect();
    for text in texts {
        // Cheap gate: a text without a wiki-link names no mem.
        if text.contains("[[") {
            mems.extend(
                crate::entity::parser::extract_inline_links_lenient(text, own)
                    .iter()
                    .map(|t| t.mem().to_string()),
            );
        }
    }
    mems.retain(|m| !m.is_empty() && m != own);
    mems.sort();
    mems.dedup();
    mems
}
