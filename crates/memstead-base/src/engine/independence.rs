//! The author≠checker independence reading, derived from provenance.
//!
//! A check record confirms an acceptance criterion only when the party
//! that did the work did not record it. Until 2026-09-02 the reading
//! compared a check's identity with the identity that CREATED the
//! criterion entity — and the planning session authors criteria while the
//! executing session checks them, so the executor's own checks read
//! `confirmed_independent` (found twice by the evidence-engine bundle,
//! once on a wrong check). The comparator now (decision basket line 9,
//! option a): a check on a criterion reads `confirmed_independent` only
//! when its identity differs from **every identity that mutated the
//! verified plan, its criteria, or its session-log notes since the
//! criterion was written**; a check under one of those identities reads
//! `self_checked`; a check or a record without an identity stays
//! `unconfirmable`. Identities are the only comparator; roles and the
//! transport pair are recorded context. Nothing is stamped: the reading
//! is computed at read time from the append-only provenance record, so
//! every existing ledger keeps parsing and derives under the new rule.
//!
//! The `transition_requires_checks` gate consumes the same reading: a
//! plan cannot complete on the executor's own checks.

use std::collections::{BTreeSet, HashMap};

use serde::Serialize;

use super::Engine;
use crate::check::{CheckRecord, CheckState};

/// The independence half of a check's standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Independence {
    /// The check's identity mutated nothing in the verified plan's set
    /// since the criterion was written.
    ConfirmedIndependent,
    /// The check's identity is one of the executors.
    SelfChecked,
    /// The check, or every relevant provenance record, carries no
    /// identity — absence is never promoted.
    Unconfirmable,
}

impl Independence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConfirmedIndependent => "confirmed_independent",
            Self::SelfChecked => "self_checked",
            Self::Unconfirmable => "unconfirmable",
        }
    }
}

/// One entity's standing before the gate: its derived check state, and
/// for an ok-checked entity the independence of that check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckStanding {
    pub state: CheckState,
    /// `Some` only for `CheckedOk`.
    pub independence: Option<Independence>,
}

impl CheckStanding {
    /// A standing that confirms when the state is `checked_ok` — the
    /// form store-only callers (and tests) build when no provenance is
    /// in play.
    pub fn assumed_independent(state: CheckState) -> Self {
        Self {
            state,
            independence: (state == CheckState::CheckedOk)
                .then_some(Independence::ConfirmedIndependent),
        }
    }

    /// Whether this standing satisfies the transition gate.
    pub fn confirms(&self) -> bool {
        self.state == CheckState::CheckedOk
            && self.independence == Some(Independence::ConfirmedIndependent)
    }

    /// The label the gate reports for an entity that does not confirm:
    /// the check state, or the independence reading when the state is
    /// `checked_ok` but not independent.
    pub fn label(&self) -> &'static str {
        match (self.state, self.independence) {
            (CheckState::CheckedOk, Some(i)) => i.as_str(),
            (CheckState::CheckedOk, None) => Independence::Unconfirmable.as_str(),
            (s, _) => s.as_str(),
        }
    }
}

/// One mem's mutation touches by entity: `(timestamp, identity)` per
/// touch, from the mem's provenance record (the git-branch note trailers,
/// or the folder ledger). Built once per mem and shared by every
/// derivation in one pass.
#[derive(Debug, Default, Clone)]
pub struct MemTouches {
    by_entity: HashMap<String, Vec<(i64, Option<String>)>>,
}

impl MemTouches {
    /// The oldest touch timestamp of `entity`, if any is recorded.
    fn written_at(&self, entity: &str) -> Option<i64> {
        self.by_entity
            .get(entity)
            .and_then(|t| t.iter().map(|(ts, _)| *ts).min())
    }

    /// Identities that touched `entity` at or after `since`.
    fn identities_since(&self, entity: &str, since: i64, into: &mut BTreeSet<String>) {
        if let Some(touches) = self.by_entity.get(entity) {
            for (ts, id) in touches {
                if *ts >= since
                    && let Some(id) = id
                {
                    into.insert(id.clone());
                }
            }
        }
    }

    /// Whether any touch of `entity` carries an identity.
    fn any_identity(&self, entity: &str) -> bool {
        self.by_entity
            .get(entity)
            .is_some_and(|t| t.iter().any(|(_, id)| id.is_some()))
    }
}

/// The executors of one criterion: the identities the reading compares
/// against, and the plans the set was drawn from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Executors {
    pub identities: Vec<String>,
    pub plans: Vec<String>,
}

impl Engine {
    /// Gather one mem's touches from its provenance record. Git-branch
    /// mems walk the branch's commit notes once; folder and in-memory
    /// mems read their ledger; an archive records no history at the
    /// engine seam and yields no touches (its checks stay
    /// `unconfirmable` unless the ledger's own author identity decides).
    pub fn mem_touches(&self, mem: &str) -> MemTouches {
        let mut out = MemTouches::default();
        let Some(m) = self.mounts.iter().find(|m| m.mount.mem == mem) else {
            return out;
        };
        match &m.mount.storage {
            crate::workspace::MountStorage::GitBranch { gitdir, branch } => {
                if let Some(hook) = self.git_branch_ops.as_ref()
                    && let Ok(changes) = (hook.changes_since)(
                        gitdir,
                        branch,
                        mem,
                        crate::ops::EMPTY_TREE_SHA,
                        crate::ops::RENAME_SIMILARITY_DEFAULT,
                    )
                {
                    for n in &changes.notes {
                        let Some(entity) = n.entity_id.as_deref() else {
                            continue;
                        };
                        // A rename note names `old -> new`; both ids are the
                        // same entity's story.
                        for id in entity.split("->").map(str::trim).filter(|s| !s.is_empty()) {
                            out.by_entity
                                .entry(id.to_string())
                                .or_default()
                                .push((n.timestamp, n.identity.clone()));
                        }
                    }
                }
            }
            crate::workspace::MountStorage::Folder { .. }
            | crate::workspace::MountStorage::InMemory => {
                if let Ok(records) = m.backend.read_provenance(None) {
                    for r in records {
                        let Some(entity) = r.entity.as_deref() else {
                            continue;
                        };
                        let ts = r
                            .timestamp
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        out.by_entity
                            .entry(entity.to_string())
                            .or_default()
                            .push((ts, r.identity.clone()));
                    }
                }
            }
            crate::workspace::MountStorage::Archive { .. } => {}
        }
        out
    }

    /// The executors of `entity` (a criterion): every identity that
    /// mutated the plan(s) it VERIFIES, those plans' other criteria, or the
    /// notes PART_OF those plans, at or after the criterion's first touch.
    /// `None` when the entity verifies no plan — the reading then falls
    /// back to the entity's own author.
    pub fn executors_of(
        &self,
        entity: &crate::entity::Entity,
        touches: &MemTouches,
    ) -> Option<Executors> {
        let plans: Vec<crate::entity::EntityId> = entity
            .relationships
            .iter()
            .filter(|r| r.rel_type == "VERIFIES")
            .map(|r| r.target.clone())
            .collect();
        if plans.is_empty() {
            return None;
        }
        let since = touches.written_at(&entity.id.0).unwrap_or(0);
        let mut set: BTreeSet<String> = BTreeSet::new();
        let mut members: BTreeSet<String> = BTreeSet::new();
        for plan in &plans {
            members.insert(plan.0.clone());
            for other in self.store.all_entities().filter(|o| o.mem == entity.mem) {
                if other.relationships.iter().any(|r| {
                    &r.target == plan && (r.rel_type == "VERIFIES" || r.rel_type == "PART_OF")
                }) {
                    members.insert(other.id.0.clone());
                }
            }
        }
        for member in &members {
            touches.identities_since(member, since, &mut set);
        }
        Some(Executors {
            identities: set.into_iter().collect(),
            plans: plans.into_iter().map(|p| p.0).collect(),
        })
    }

    /// The independence reading of one ok check on `entity`.
    pub fn independence_of(
        &self,
        entity: &crate::entity::Entity,
        check: &CheckRecord,
        touches: &MemTouches,
    ) -> (Independence, Option<Executors>) {
        let Some(checker) = check.identity.as_deref() else {
            return (Independence::Unconfirmable, None);
        };
        match self.executors_of(entity, touches) {
            Some(executors) => {
                let reading = if executors.identities.iter().any(|i| i == checker) {
                    Independence::SelfChecked
                } else if executors.identities.is_empty()
                    && !executors.plans.iter().any(|p| touches.any_identity(p))
                    && !touches.any_identity(&entity.id.0)
                {
                    // Nothing in the plan's set carries an identity: no
                    // comparator exists, the reading cannot be promoted.
                    Independence::Unconfirmable
                } else {
                    Independence::ConfirmedIndependent
                };
                (reading, Some(executors))
            }
            None => {
                // Not a criterion: today's rule, the entity's own author.
                let author = touches
                    .by_entity
                    .get(&entity.id.0)
                    .and_then(|t| t.iter().min_by_key(|(ts, _)| *ts))
                    .and_then(|(_, id)| id.clone());
                let reading = match author {
                    Some(a) if a == checker => Independence::SelfChecked,
                    Some(_) => Independence::ConfirmedIndependent,
                    None => Independence::Unconfirmable,
                };
                (reading, None)
            }
        }
    }

    /// The gate's window into the ledger: derived state plus, for an
    /// ok-checked entity, the independence of that check — with each
    /// mem's touches gathered once per provider.
    pub(crate) fn check_standing_provider(
        &self,
    ) -> impl Fn(&crate::entity::Entity) -> CheckStanding + '_ {
        let ledger = self
            .workspace_root()
            .map(crate::check::CheckLedger::for_workspace);
        let touches: std::cell::RefCell<HashMap<String, MemTouches>> =
            std::cell::RefCell::new(HashMap::new());
        move |entity: &crate::entity::Entity| {
            let Some(ledger) = &ledger else {
                return CheckStanding {
                    state: CheckState::NeverChecked,
                    independence: None,
                };
            };
            let latest =
                ledger.latest_for_kind(&entity.id.0, crate::check::CheckKind::Verification);
            let state = crate::check::derive_state(latest.as_ref(), &entity.content_hash);
            if state != CheckState::CheckedOk {
                return CheckStanding {
                    state,
                    independence: None,
                };
            }
            let check = latest.expect("checked_ok implies a record");
            let mut cache = touches.borrow_mut();
            let mem_touches = cache
                .entry(entity.mem.clone())
                .or_insert_with(|| self.mem_touches(&entity.mem));
            let (independence, _) = self.independence_of(entity, &check, mem_touches);
            CheckStanding {
                state,
                independence: Some(independence),
            }
        }
    }
}
