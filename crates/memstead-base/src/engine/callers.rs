//! Change notices are relative to the caller.
//!
//! An engine may serve many callers: several agents behind one server,
//! each of which must be told what changed in a mem since *that
//! caller* last saw it, including what another caller wrote through
//! this same engine. The engine therefore keeps, per caller key and
//! mem, the head that caller last saw. The notice for a caller is the
//! delta from its last-seen head to the head the engine holds when the
//! caller's operation has reloaded and before it writes; afterwards its
//! last-seen head is the head the operation left, so a caller is never
//! told about its own write.
//!
//! A caller seen for the first time starts from what storage holds, so
//! nothing that landed before it came is news to it; where storage was
//! ahead of the engine then, the engine's head is kept beside the
//! caller's, so the gap is no move. And before a notice is built for a
//! mem, the engine catches up with storage for that mem, so a notice's
//! two ends agree with its entries and nothing is told twice.
//!
//! There is one mechanism. A surface with a single caller (the CLI, a
//! stdio server) uses the default key, which the engine knows from its
//! first reload on; what such a surface is told is what it was told
//! when the notice was relative to the engine itself.
//!
//! The key is handed in by the surface that opens the operation
//! ([`super::drift::OperationScope::begin_as`]). It is not an identity:
//! two callers may declare the same identity and are still told
//! separately.

use std::collections::HashMap;

use super::Engine;

/// Who an operation is for, as far as change notices go. The default
/// key is the single caller of a CLI process or a stdio server.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct CallerKey(String);

impl CallerKey {
    /// A caller named by the surface: a session handle, a connection
    /// id, any string the surface keeps stable for one caller.
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// Whether this is the default key of a single-caller surface.
    pub fn is_default(&self) -> bool {
        self.0.is_empty()
    }
}

/// How many callers besides the default one the engine tracks. Past
/// this, the least recently used caller is forgotten and reads as a
/// first contact when it returns.
pub const MAX_TRACKED_CALLERS: usize = 1024;

/// One caller's last-seen head per mem. The empty string stands for
/// "saw the mem with no head".
#[derive(Debug, Default)]
struct CallerState {
    seen: HashMap<String, String>,
    /// Mems whose last-seen head is ahead of the engine: the caller
    /// came while storage held more than the engine had reloaded. The
    /// value is the head the engine held then. While the engine still
    /// stands there, the difference between the two heads is no move
    /// and gives no reason to look at that mem's storage; once the
    /// engine moves, the caller is told what lies beyond its own head.
    ahead: HashMap<String, String>,
    last_used: u64,
}

impl CallerState {
    /// Whether the caller's head for `mem` is ahead of an engine that
    /// still stands at `head`.
    fn is_ahead_of(&self, mem: &str, head: &str) -> bool {
        self.ahead.get(mem).is_some_and(|held| held == head)
    }

    /// The engine stands at `head` for `mem` and the caller has seen
    /// it, unless its own head is ahead of an engine that has not moved.
    fn stands_at(&mut self, mem: &str, head: &str) {
        if self.is_ahead_of(mem, head) {
            return;
        }
        self.ahead.remove(mem);
        self.seen.insert(mem.to_string(), head.to_string());
    }
}

/// The engine's memory of what each caller last saw.
#[derive(Debug, Default)]
pub(crate) struct CallerCursors {
    callers: HashMap<CallerKey, CallerState>,
    tick: u64,
    /// The caller of the operation in progress; the default key between
    /// operations and for a caller that opens no scope.
    current: CallerKey,
    /// Set while the engine reloads a mem on behalf of a notice it is
    /// about to build: that reload is not an operation of the caller's
    /// and runs no per-caller step of its own.
    pub(in crate::engine) catching_up: bool,
}

impl CallerCursors {
    fn touch(&mut self, key: &CallerKey) {
        self.tick += 1;
        if let Some(state) = self.callers.get_mut(key) {
            state.last_used = self.tick;
        }
    }

    /// Forget the least recently used callers past the cap. The default
    /// caller and the current one are never forgotten.
    fn evict(&mut self) {
        while self.callers.keys().filter(|k| !k.is_default()).count() > MAX_TRACKED_CALLERS {
            let victim = self
                .callers
                .iter()
                .filter(|(k, _)| !k.is_default() && **k != self.current)
                .min_by_key(|(_, s)| s.last_used)
                .map(|(k, _)| k.clone());
            match victim {
                Some(key) => {
                    self.callers.remove(&key);
                }
                None => break,
            }
        }
    }

    /// Drop a mem from every caller's memory: it left the roster.
    pub(crate) fn forget_mem(&mut self, mem: &str) {
        for state in self.callers.values_mut() {
            state.seen.remove(mem);
            state.ahead.remove(mem);
        }
    }

    /// How many callers are tracked, the default one included.
    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.callers.len()
    }
}

impl Engine {
    /// The head each mount stands at, as the engine holds it: no probe,
    /// the empty string for a mount with no head.
    fn heads_now(&self) -> Vec<(String, String)> {
        self.mounts
            .iter()
            .map(|m| {
                (
                    m.mount.mem.clone(),
                    m.last_known_head.clone().unwrap_or_default(),
                )
            })
            .collect()
    }

    /// The head each mount's storage holds at this instant, probed: a
    /// head the engine has not reloaded to yet counts. A probe that
    /// fails falls back to the head the engine holds. This is the
    /// baseline of a first contact: a caller that has seen nothing yet
    /// starts from what is there, so a change that landed before it
    /// came, and that the engine itself has yet to reload, is never
    /// news to it.
    fn heads_in_storage(&self) -> Vec<(String, String)> {
        self.mounts
            .iter()
            .map(|m| {
                let head = m
                    .backend
                    .current_head()
                    .ok()
                    .flatten()
                    .or_else(|| m.last_known_head.clone())
                    .unwrap_or_default();
                (m.mount.mem.clone(), head)
            })
            .collect()
    }

    /// Name the caller of the operation that begins. Called by the
    /// operation scope; an engine used without a scope stays on the
    /// default caller.
    pub(in crate::engine) fn set_current_caller(&mut self, caller: CallerKey) {
        self.callers.current = caller;
        // A new operation: nothing has brought this caller up to date
        // yet, whatever an unscoped call left behind.
        self.caller_synced = false;
        self.caller_first_contact = false;
    }

    /// Whether the operation in progress is the default caller's.
    pub(in crate::engine) fn current_caller_is_default(&self) -> bool {
        self.callers.current.is_default()
    }

    /// Before a reload: make sure the default caller is known, with the
    /// heads as they stand before anything is reloaded. A single-caller
    /// surface has been there since boot, so what a reload brings in is
    /// news to it. Any other caller becomes known after its first
    /// reload instead ([`Self::note_changes_for_caller`]) and is told
    /// nothing on that first contact.
    pub(in crate::engine) fn know_default_caller(&mut self) {
        let key = CallerKey::default();
        if self.callers.callers.contains_key(&key) {
            return;
        }
        let seen = self.heads_now().into_iter().collect();
        self.callers.callers.insert(
            key,
            CallerState {
                seen,
                ..CallerState::default()
            },
        );
    }

    /// A first contact: the caller starts from what storage holds, so a
    /// change that landed before it came is never news to it, whether
    /// the engine has reloaded that change yet or not. Where storage is
    /// ahead of the engine, the engine's head is kept beside the
    /// caller's, so the gap neither reads as a move nor makes a later
    /// operation of this caller look at a mem outside its scope.
    fn baseline_from_storage(&mut self, key: &CallerKey) {
        let engine: HashMap<String, String> = self.heads_now().into_iter().collect();
        let seen: HashMap<String, String> = self.heads_in_storage().into_iter().collect();
        let ahead = seen
            .iter()
            .filter_map(|(mem, head)| {
                let held = engine.get(mem)?;
                (held != head).then(|| (mem.clone(), held.clone()))
            })
            .collect();
        let last_used = self
            .callers
            .callers
            .get(key)
            .map(|s| s.last_used)
            .unwrap_or_default();
        self.callers.callers.insert(
            key.clone(),
            CallerState {
                seen,
                ahead,
                last_used,
            },
        );
    }

    /// After a reload and before the operation writes: tell the current
    /// caller what moved since it last looked. For each mem whose head
    /// differs from the head this caller last saw, a notice describing
    /// that delta is recorded for the operation's response and the
    /// caller's memory moves to the head now. A caller seen for the
    /// first time, and a mem a caller sees for the first time, set the
    /// baseline and record nothing.
    ///
    /// Returns one `MEM_RELOADED` warning per notice, for the mems this
    /// call's own reload did not already warn about: a caller told
    /// about another caller's write through the same engine saw no
    /// reload, yet its cached hashes are as stale as after one, and the
    /// text channel of a response speaks through that warning.
    pub(in crate::engine) fn note_changes_for_caller(&mut self) -> Vec<crate::ops::WarningHint> {
        if self.callers.catching_up {
            return Vec::new();
        }
        self.caller_synced = true;
        let key = self.callers.current.clone();
        // A first contact: the caller starts from what storage holds,
        // and its whole first operation hands out nothing. An operation
        // reloads mem by mem, so a later reload of the same operation
        // must not read as news to a caller that came with it.
        if !self.callers.callers.contains_key(&key) || self.caller_first_contact {
            self.caller_first_contact = true;
            self.baseline_from_storage(&key);
            self.callers.touch(&key);
            self.callers.evict();
            return Vec::new();
        }
        // A notice runs from the head the caller last saw to the head
        // the engine holds, and its entries are what storage holds
        // beyond the former. Where the two heads differ, the engine
        // first catches up with storage for that mem, so the ends of
        // the notice agree with its entries and nothing that lies
        // beyond is told a second time when the engine reloads it
        // later. A caller that started from a storage head the engine
        // has not reached is not behind, and its operation looks at no
        // mem on that account.
        let behind: Vec<String> = {
            let state = self.callers.callers.get(&key);
            self.heads_now()
                .into_iter()
                .filter(|(mem, head)| {
                    state.is_some_and(|s| {
                        s.seen.get(mem).is_some_and(|prev| prev != head)
                            && !s.is_ahead_of(mem, head)
                    })
                })
                .map(|(mem, _)| mem)
                .collect()
        };
        if !behind.is_empty() {
            self.callers.catching_up = true;
            for mem in &behind {
                let _ = self.reload_if_stale_content_only(Some(mem));
            }
            self.callers.catching_up = false;
        }
        let heads = self.heads_now();
        self.callers.touch(&key);
        let mut moved: Vec<(String, String, String)> = Vec::new();
        {
            let state = self
                .callers
                .callers
                .get_mut(&key)
                .expect("the caller was just found");
            for (mem, head) in &heads {
                match state.seen.get(mem) {
                    // Ahead of an engine that has not moved: nothing
                    // happened.
                    Some(_) if state.is_ahead_of(mem, head) => {}
                    Some(prev) if prev != head => {
                        moved.push((mem.clone(), prev.clone(), head.clone()));
                    }
                    Some(_) => {
                        state.ahead.remove(mem);
                    }
                    None => {
                        state.seen.insert(mem.clone(), head.clone());
                    }
                }
            }
            // A mem that is no longer mounted is no longer remembered.
            state
                .seen
                .retain(|mem, _| heads.iter().any(|(m, _)| m == mem));
            state
                .ahead
                .retain(|mem, _| heads.iter().any(|(m, _)| m == mem));
        }
        let mut warnings = Vec::new();
        for (mem, prev, head) in moved {
            let notice = if prev.is_empty() {
                // The mem had no head when this caller last saw it (its
                // branch was born, fetched or pushed into place since, or
                // its first write landed). What is new to the caller: the
                // entities the reload in progress brought in, when this
                // reload is where the head appeared; else whatever the
                // backend can tell of its history from the start.
                let changes: Vec<crate::ops::ChangeEnvelope> = match self.appeared_heads.get(&mem) {
                    Some(added) => added
                        .iter()
                        .map(|id| crate::ops::ChangeEnvelope::Added {
                            id: id.clone(),
                            title: None,
                            entity_type: None,
                        })
                        .collect(),
                    None => self
                        .changes_since(&mem, "", None)
                        .map(|r| r.changes)
                        .unwrap_or_default(),
                };
                crate::ops::MemChangedNotice::from_delta(
                    mem.clone(),
                    String::new(),
                    head.clone(),
                    changes,
                )
            } else {
                self.mem_changed_notice(&mem, &prev, &head)
            };
            warnings.push(crate::ops::WarningHint::MemReloaded {
                mem: mem.clone(),
                old_head: prev,
                new_head: head.clone(),
                entities_loaded: notice.entity_count(),
            });
            self.pending_mem_changed.push(notice);
            if let Some(state) = self.callers.callers.get_mut(&key) {
                state.ahead.remove(&mem);
                state.seen.insert(mem, head);
            }
        }
        warnings
    }

    /// Bring the current caller up to date when the operation has not
    /// done so yet: an operation that reloads nothing before it acts (an
    /// explicit reload, a settings write, a read of engine state) still
    /// tells its caller what other callers wrote through this engine,
    /// and still counts as a first contact. A no-op once the operation's
    /// own reload ran the step.
    pub(in crate::engine) fn bring_caller_up_to_date(&mut self) {
        if self.caller_synced {
            return;
        }
        self.know_default_caller();
        // The warnings are the text form of the notices just recorded;
        // a surface that speaks through them rebuilds them from the
        // notices the operation hands out.
        let _ = self.note_changes_for_caller();
    }

    /// The operation is over: whatever head the engine holds now is one
    /// the current caller has seen, because everything that moved since
    /// [`Self::note_changes_for_caller`] ran is the caller's own write.
    /// A caller the operation never brought up to date keeps its memory
    /// as it was, so it is still told later. Then the engine is back on
    /// the default caller.
    pub(in crate::engine) fn close_caller_scope(&mut self) {
        let key = std::mem::take(&mut self.callers.current);
        let first_contact = std::mem::take(&mut self.caller_first_contact);
        if !self.caller_synced {
            return;
        }
        self.caller_synced = false;
        // After a first contact the caller stands at what storage holds
        // (its own writes of this operation included); otherwise at the
        // heads the engine holds, which moved only by the caller's own
        // writes since it was brought up to date.
        // A first contact keeps the baseline its last reload took, moved
        // on by its own writes: what another process writes between that
        // reload and here is told to it later.
        if first_contact {
            return;
        }
        let heads = self.heads_now();
        if let Some(state) = self.callers.callers.get_mut(&key) {
            for (mem, head) in heads {
                state.stands_at(&mem, &head);
            }
        }
    }

    /// The engine's own write moved a mem's head from `previous` to
    /// `recorded`: the current caller, if it stood at `previous`, has
    /// seen the new head, since the write is its own. A later reload in
    /// the same operation then finds nothing to tell it.
    pub(in crate::engine) fn caller_saw_own_write(
        &mut self,
        mem: &str,
        previous: &str,
        recorded: &str,
    ) {
        let key = self.callers.current.clone();
        if let Some(state) = self.callers.callers.get_mut(&key)
            && (state.seen.get(mem).is_some_and(|seen| seen == previous)
                || state.is_ahead_of(mem, previous))
        {
            state.ahead.remove(mem);
            state.seen.insert(mem.to_string(), recorded.to_string());
        }
    }
}
