//! Backend-neutral entity-level delta surface for `memstead_changes_since`
//! callers.
//!
//! [`ChangeEnvelope`] is the per-entity event shape ("this entity was
//! added / updated / removed / renamed between cursor X and the
//! current state"). [`crate::Engine::changes_since`] dispatches per
//! mount: folder mounts synthesise from `.memstead/changes.jsonl` via
//! [`folder_changes_since`]; git-branch mounts call the registered
//! [`crate::GitBranchOps::changes_since`] dispatcher (real tree-diff
//! with rename detection); archive mounts return an empty report.
//!
//! The cursor format is backend-specific and opaque from the caller's
//! perspective: a commit SHA for git-branch, an RFC-3339 timestamp
//! for folder, ignored for archive. The empty-tree-SHA sentinel
//! ([`EMPTY_TREE_SHA`]) is a convention preserved across backends so
//! "diff against nothing" works without each backend re-inventing
//! the same first-poll shape.
//!
//! `title` and `entity_type` on the envelope variants are populated
//! by the engine wrapper from the in-memory store (best-effort —
//! `Removed` envelopes always leave them `None` because the entity
//! is gone). Backend dispatchers produce id-only envelopes; the
//! [`crate::Engine::changes_since`] wrapper enriches.

use std::path::Path;

use serde::Serialize;

use crate::backend::BackendError;
use crate::entity::EntityId;
use crate::provenance::ProvenanceKind;

/// Canonical git empty-tree hash. Callers without a prior cursor pass
/// this to get "every entity in the current state as added". Both
/// the git-branch backend (special-cased to bypass `rev_parse`) and
/// any future folder-backend implementation honour the same sentinel.
pub const EMPTY_TREE_SHA: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Default content-similarity threshold for rename detection (60%).
/// Callers override per-call via `changes_since`'s `rename_similarity`
/// parameter; the engine wrapper accepts `[0.1, 1.0]` and emits a
/// `LIMIT_CLAMPED` warning for out-of-range values. Higher values
/// miss edited renames; lower values risk false-positive rename
/// pairing.
pub const RENAME_SIMILARITY_DEFAULT: f32 = 0.6;

/// Lower bound for `rename_similarity` — anything below 0.1 produces
/// nearly-random rewrite pairing on a modest diff.
pub const RENAME_SIMILARITY_MIN: f32 = 0.1;

/// Upper bound for `rename_similarity` — 1.0 means "only paired up
/// on a byte-identical match"; above that there is no semantic
/// meaning.
pub const RENAME_SIMILARITY_MAX: f32 = 1.0;

/// Single delta entry between two snapshots. `Renamed` collapses what
/// would otherwise appear as a `Removed` + `Added` pair so agents see
/// one semantic event per filesystem rename.
///
/// `title` and `entity_type` are best-effort enrichment from the
/// engine's in-memory store: present when the backend's diff resolves
/// to an entity the engine still knows about, `None` otherwise.
/// `Removed` envelopes always leave both `None` (the entity is gone
/// by definition); other variants populate when the lookup succeeds.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum ChangeEnvelope {
    Added {
        id: EntityId,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        entity_type: Option<String>,
    },
    Updated {
        id: EntityId,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        entity_type: Option<String>,
    },
    Removed {
        id: EntityId,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        entity_type: Option<String>,
    },
    Renamed {
        from_id: EntityId,
        to_id: EntityId,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        entity_type: Option<String>,
    },
}

impl ChangeEnvelope {
    /// The id this change sorts and renders under — `to_id` for a
    /// rename (the surviving entity), the entity id otherwise.
    pub fn primary_id(&self) -> &str {
        match self {
            ChangeEnvelope::Added { id, .. }
            | ChangeEnvelope::Updated { id, .. }
            | ChangeEnvelope::Removed { id, .. } => id.as_ref(),
            ChangeEnvelope::Renamed { to_id, .. } => to_id.as_ref(),
        }
    }

    /// The wire `action` verb — the same token the serde tag emits and
    /// `memstead_changes_since` reports: `added` | `updated` | `removed` |
    /// `renamed`.
    pub fn action(&self) -> &'static str {
        match self {
            ChangeEnvelope::Added { .. } => "added",
            ChangeEnvelope::Updated { .. } => "updated",
            ChangeEnvelope::Removed { .. } => "removed",
            ChangeEnvelope::Renamed { .. } => "renamed",
        }
    }

    /// Best-effort entity type carried on the envelope (`None` on
    /// `Removed`, or when the store lookup missed).
    pub fn entity_type(&self) -> Option<&str> {
        match self {
            ChangeEnvelope::Added { entity_type, .. }
            | ChangeEnvelope::Updated { entity_type, .. }
            | ChangeEnvelope::Removed { entity_type, .. }
            | ChangeEnvelope::Renamed { entity_type, .. } => entity_type.as_deref(),
        }
    }

    /// The same change with `title` / `entity_type` stripped — the
    /// `ids`-tier projection of a notice entry. The id (and a rename's
    /// `from_id` / `to_id` pair) is preserved; it is identity, not rich
    /// detail.
    fn without_metadata(&self) -> Self {
        match self {
            ChangeEnvelope::Added { id, .. } => ChangeEnvelope::Added {
                id: id.clone(),
                title: None,
                entity_type: None,
            },
            ChangeEnvelope::Updated { id, .. } => ChangeEnvelope::Updated {
                id: id.clone(),
                title: None,
                entity_type: None,
            },
            ChangeEnvelope::Removed { id, .. } => ChangeEnvelope::Removed {
                id: id.clone(),
                title: None,
                entity_type: None,
            },
            ChangeEnvelope::Renamed { from_id, to_id, .. } => ChangeEnvelope::Renamed {
                from_id: from_id.clone(),
                to_id: to_id.clone(),
                title: None,
                entity_type: None,
            },
        }
    }
}

/// Backend-neutral "what changed" report. The engine wrapper
/// ([`crate::Engine::changes_since`], landing in a follow-up session)
/// adds rename-similarity clamping warnings, optional agent-notes
/// piggyback (git-branch only), and the operator-facing
/// `mem: String` field on top.
///
/// `head` echoes the resolved cursor of the current state — agents
/// remember it as the next polling cursor so the next call passes it
/// straight back as `since` without a `memstead_health` round-trip.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BackendChanges {
    /// The cursor the caller passed in, echoed verbatim.
    pub since: String,
    /// The cursor of the current state — opaque to the caller, but
    /// stable across consecutive polls when nothing has changed.
    pub head: String,
    /// Per-entity events. Empty when nothing changed (or when the
    /// backend has no native diff and inherits the trait's default
    /// impl — folder + archive today).
    pub changes: Vec<ChangeEnvelope>,
    /// Per-commit agent-notes parsed from commit trailers. Empty for
    /// backends without commit history (folder, archive). Populated
    /// by the git-branch backend on every `changes_since` call — the
    /// walk lives inside the backend so the engine has a single source
    /// of truth for both the rename map (note-driven) and the
    /// per-commit feed, and the MCP `include_notes` parameter becomes a
    /// renderer-side filter rather than a separate engine-side trigger.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<crate::ops::agent_notes::CommitNote>,
    /// Workspace-level `__MEMSTEAD` ref tip (unified schemas + per-mem
    /// configs). `None` for backends without commit history; `None`
    /// also on git-branch backends where the `__MEMSTEAD` ref does not
    /// (yet) exist — pre-migration workspaces are legitimate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memstead_ref: Option<String>,
}

impl BackendChanges {
    /// Empty report at `since` — the default a backend without a
    /// native diff returns. `head` echoes `since` so the caller's
    /// cursor stays stable across polls.
    pub fn empty_at(since: &str) -> Self {
        Self {
            since: since.to_string(),
            head: since.to_string(),
            changes: Vec::new(),
            notes: Vec::new(),
            memstead_ref: None,
        }
    }
}

/// Synthesise per-entity events for a folder-backed mem by reading
/// `<mem_root>/.memstead/changes.jsonl` and bucketing events by entity.
///
/// Net-effect rules per entity:
/// - Last event = Delete                  → `Removed`
/// - First event = Create                 → `Added`
/// - Anything else                        → `Updated`
///
/// A `rename` row records only the id the entity now carries (the
/// ledger's on-disk shape); with `store` present the replay pairs it
/// with the id that vanished: an id the ledger knows, absent from the
/// store, never deleted, whose creation instant matches the renamed
/// entity's `created_date` (a rename keeps it), falling back to the
/// nearest vanished id before the rename row. A pair inside one window
/// surfaces as one event: `renamed` with `from_id` and `to_id`, or
/// `added` under the final id when the entity was also created in the
/// window. A vanished id that pairs with nothing surfaces as `removed`:
/// an `added` row for an id no reader can fetch is the one shape the
/// feed never serves. Without `store` (a bare replay) a rename row
/// surfaces as `updated` under the new id. `Batch` events have no
/// entity id and don't contribute envelopes. The cursor is an RFC-3339 timestamp; the
/// [`EMPTY_TREE_SHA`] sentinel and the empty string mean "from the
/// beginning". Any other non-parseable cursor **refuses** with the
/// typed `INVALID_TS_CURSOR:` marker (lifted by the engine to the
/// `INVALID_CURSOR` code): the folder ledger's timestamps are compared
/// lexically, and a mutation's `write_id` — fixed-width hex minted
/// from a nanosecond clock — sorts below every timestamp, so the old
/// tolerant reading silently replayed the whole history to exactly
/// the caller who confused the two. `head` echoes the latest
/// timestamp seen, falling back to `since`.
///
/// Envelopes are id-only (`title` / `entity_type` are `None`); the
/// engine wrapper enriches from its in-memory store.
/// The store's view the folder replay pairs renames with: for an id
/// the store serves, its `created_date` as written in the frontmatter;
/// `None` for an id it does not serve.
pub type StoreLookup<'a> = &'a dyn Fn(&EntityId) -> Option<String>;

pub fn folder_changes_since(
    mem_root: &Path,
    mem: &str,
    since: &str,
    store: Option<StoreLookup<'_>>,
) -> Result<BackendChanges, BackendError> {
    if !since.is_empty()
        && since != EMPTY_TREE_SHA
        && crate::filesystem::changelog::parse_rfc3339_utc(since).is_none()
    {
        // Typed marker, same convention as the git backend's
        // `COMMIT_NOT_FOUND:` — the engine wrapper lifts it to
        // `EngineError::InvalidTimestampCursor` (code INVALID_CURSOR).
        return Err(BackendError::Other(format!("INVALID_TS_CURSOR:{since}")));
    }
    // The ledger is a file a commit appends to: while a commit of this
    // mem is cut off, the feed is not read, like every other read that
    // goes to the mem's files.
    crate::storage::filesystem::refuse_cut_off_commit(mem_root)?;
    let log_path = crate::filesystem::changelog::changelog_path(mem_root);
    let raw = match std::fs::read_to_string(&log_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BackendChanges::empty_at(since));
        }
        Err(e) => return Err(BackendError::Io(e)),
    };

    struct Aggregate {
        first_kind: ProvenanceKind,
        last_kind: ProvenanceKind,
    }
    /// What the whole ledger says about one id, window or not: the
    /// instant it was created (when the ledger saw it), the instant of
    /// its last row, and whether a delete row ever closed it.
    #[derive(Default)]
    struct Lifetime {
        created_at: Option<String>,
        last_at: String,
        deleted: bool,
        /// The id's last row is the rename row that named it: it is a
        /// link in a rename chain, and the next rename continues from it.
        named_by_rename: bool,
    }
    let mut by_entity: std::collections::BTreeMap<String, Aggregate> =
        std::collections::BTreeMap::new();
    let mut lifetimes: std::collections::BTreeMap<String, Lifetime> =
        std::collections::BTreeMap::new();
    let mut rename_rows: Vec<(String, String)> = Vec::new();
    let mut max_ts: Option<String> = None;

    let cursor_opt: Option<&str> = if since.is_empty() || since == EMPTY_TREE_SHA {
        None
    } else {
        Some(since)
    };

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let ts_str = value.get("ts").and_then(|v| v.as_str()).unwrap_or("");
        let kind = match value
            .get("kind")
            .and_then(|v| v.as_str())
            .and_then(ProvenanceKind::parse)
        {
            Some(k) => k,
            None => continue,
        };
        let entity_id = match value.get("entity").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        {
            let life = lifetimes.entry(entity_id.clone()).or_default();
            // A create revives an id a delete retired: its lifetime is the
            // newest one, and its creation instant is that create's.
            if matches!(kind, ProvenanceKind::Create) {
                life.created_at = Some(ts_str.to_string());
                life.deleted = false;
            }
            if ts_str >= life.last_at.as_str() {
                life.last_at = ts_str.to_string();
                life.named_by_rename = matches!(kind, ProvenanceKind::Rename);
            }
            if matches!(kind, ProvenanceKind::Delete) {
                life.deleted = true;
            }
        }
        if let Some(c) = cursor_opt
            && ts_str <= c
        {
            continue;
        }
        if matches!(kind, ProvenanceKind::Rename) {
            rename_rows.push((ts_str.to_string(), entity_id.clone()));
        }

        if max_ts.as_deref().is_none_or(|m| ts_str > m) {
            max_ts = Some(ts_str.to_string());
        }

        by_entity
            .entry(entity_id)
            .and_modify(|agg| {
                agg.last_kind = kind;
            })
            .or_insert(Aggregate {
                first_kind: kind,
                last_kind: kind,
            });
    }

    // Pair every rename row in the window with the id that vanished
    // for it, when the store can say which ids still exist, then
    // compose the pairs into chains: a -> b -> c inside one window is
    // one rename from a to c, and b, a name no reader ever held for
    // long, surfaces nowhere.
    let mut links: Vec<(String, String)> = Vec::new(); // (from, to), in row order
    if let Some(lookup) = store {
        let parse = |s: &str| match s.split_once("--") {
            Some((v, slug)) if v == mem => Some(EntityId::new(v, slug)),
            _ => None,
        };
        let exists = |name: &str| parse(name).and_then(|id| lookup(&id));
        let mut taken: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        rename_rows.sort();
        for (rename_ts, new_name) in &rename_rows {
            let vanished: Vec<(&String, &Lifetime)> = lifetimes
                .iter()
                .filter(|(name, life)| {
                    *name != new_name
                        && !life.deleted
                        && !taken.contains(*name)
                        && life.last_at.as_str() < rename_ts.as_str()
                        && parse(name).is_some()
                        && exists(name).is_none()
                })
                .collect();
            // The renamed entity keeps its creation instant: when the
            // store serves the new id, the vanished id created at that
            // instant is the origin, unless the instant is ambiguous.
            let same_instant: Vec<&String> = match exists(new_name) {
                Some(created_date) => vanished
                    .iter()
                    .filter(|(_, life)| {
                        life.created_at
                            .as_deref()
                            .is_some_and(|c| same_instant(c, &created_date))
                    })
                    .map(|(name, _)| *name)
                    .collect(),
                None => Vec::new(),
            };
            let nearest: Option<String> = vanished
                .iter()
                .max_by(|a, b| a.1.last_at.cmp(&b.1.last_at))
                .map(|(name, _)| (*name).clone());
            // A vanished id that a rename row named (inside this window
            // or before it) is a chain link: the row after it continues
            // the chain, whatever the origin's creation instant says.
            let continues_chain = nearest
                .as_deref()
                .is_some_and(|n| lifetimes.get(n).is_some_and(|l| l.named_by_rename));
            let from = if continues_chain {
                nearest
            } else {
                match same_instant.as_slice() {
                    [one] => Some((*one).clone()),
                    _ => nearest,
                }
            };
            if let Some(from) = from {
                taken.insert(from.clone());
                links.push((from, new_name.clone()));
            }
        }
    }
    // Chains: an origin is a `from` no link renames into; follow it
    // to the name the entity carries now.
    let mut paired: Vec<(String, String, Vec<String>)> = Vec::new(); // (origin, final, members)
    for (from, _) in &links {
        if links.iter().any(|(_, to)| to == from) {
            continue;
        }
        let mut members = vec![from.clone()];
        let mut current = from.clone();
        while let Some((_, to)) = links.iter().find(|(f, _)| *f == current) {
            members.push(to.clone());
            current = to.clone();
        }
        paired.push((from.clone(), current, members));
    }

    let mut changes: Vec<ChangeEnvelope> = Vec::with_capacity(by_entity.len());
    for (from, to, members) in &paired {
        let from_agg = by_entity.remove(from);
        for member in members {
            by_entity.remove(member);
        }
        let (Some(from_id), Some(to_id)) = (
            from.split_once("--").map(|(v, s)| EntityId::new(v, s)),
            to.split_once("--").map(|(v, s)| EntityId::new(v, s)),
        ) else {
            continue;
        };
        let born_in_window =
            from_agg.is_some_and(|a| matches!(a.first_kind, ProvenanceKind::Create));
        changes.push(if born_in_window {
            ChangeEnvelope::Added {
                id: to_id,
                title: None,
                entity_type: None,
            }
        } else {
            ChangeEnvelope::Renamed {
                from_id,
                to_id,
                title: None,
                entity_type: None,
            }
        });
    }
    for (entity_str, agg) in by_entity {
        let id = match entity_str.split_once("--") {
            Some((v, slug)) if v == mem => EntityId::new(v, slug),
            _ => continue,
        };
        // An id the store no longer serves and no delete row closed
        // vanished without a pair: it is gone for every reader.
        if let Some(lookup) = store
            && !matches!(agg.last_kind, ProvenanceKind::Delete)
            && lookup(&id).is_none()
        {
            changes.push(ChangeEnvelope::Removed {
                id,
                title: None,
                entity_type: None,
            });
            continue;
        }
        let envelope = match (agg.first_kind, agg.last_kind) {
            (_, ProvenanceKind::Delete) => ChangeEnvelope::Removed {
                id,
                title: None,
                entity_type: None,
            },
            (ProvenanceKind::Create, _) => ChangeEnvelope::Added {
                id,
                title: None,
                entity_type: None,
            },
            _ => ChangeEnvelope::Updated {
                id,
                title: None,
                entity_type: None,
            },
        };
        changes.push(envelope);
    }

    changes.sort_by(|a, b| a.primary_id().cmp(b.primary_id()));

    Ok(BackendChanges {
        since: since.to_string(),
        head: max_ts.unwrap_or_else(|| since.to_string()),
        changes,
        notes: Vec::new(),
        memstead_ref: None,
    })
}

/// The newest write per entity id among `notes`, which run newest
/// first (the order a git-branch backend's change feed returns them
/// in): for each id a note names, the identity and note of the first
/// note that names it. A note names the entity of its subject and
/// every id its multi-entity trailers list. A batch commit records its
/// per-entry notes as `<id>: <note>` lines: each entity gets its own
/// line there and an entity without one gets no note.
pub fn newest_writes_from_notes(
    notes: &[crate::ops::agent_notes::CommitNote],
) -> std::collections::BTreeMap<String, NewestWrite> {
    let mut writes = std::collections::BTreeMap::new();
    for note in notes {
        // The subject of a rename names two ids, `<old> → <new>`: the
        // write is to the entity under either name.
        // (A batch's subject names a count, not an id: only what has
        // the shape of an id is taken.)
        let subject_ids = note
            .entity_id
            .iter()
            .flat_map(|subject| subject.split(" → "))
            .filter(|part| part.contains("--") && !part.contains(' '))
            .map(str::to_string);
        let named: Vec<String> = subject_ids
            .chain(note.entity_ids.iter().cloned())
            .chain(note.created_ids.iter().cloned())
            .collect();
        let batch = note
            .tool_verb
            .as_deref()
            .is_some_and(|verb| verb.starts_with("batch"));
        for id in &named {
            if writes.contains_key(id) {
                continue;
            }
            let text = match (&note.note, batch) {
                (Some(joined), true) => batch_entry_note(joined, id, &named),
                (text, _) => text.clone(),
            };
            writes.insert(
                id.clone(),
                NewestWrite {
                    identity: note.identity.clone(),
                    note: text.map(bounded_note),
                },
            );
        }
    }
    writes
}

/// The note one entity's entry declared in a batch, from the batch
/// commit's joined note ([`crate::vcs::batch_note_line`]): the text
/// after `<id>: ` on the line of that id, with the indented lines that
/// follow it, the indent removed. `None` when the batch has no line for
/// the id. A record written before the join indented its continuation
/// lines is read as it was meant where that is unambiguous: an
/// unindented line that begins no entity's entry continues the entry
/// above it.
fn batch_entry_note(joined: &str, id: &str, ids: &[String]) -> Option<String> {
    let mut mine: Vec<&str> = Vec::new();
    let mut owner: Option<&str> = None;
    for line in joined.lines() {
        // An indented line continues the entry above it, whatever it
        // says.
        if let (Some(current), Some(rest)) = (owner, line.strip_prefix("  ")) {
            if current == id {
                mine.push(rest);
            }
            continue;
        }
        let begins = ids.iter().find_map(|candidate| {
            line.strip_prefix(candidate.as_str())
                .and_then(|rest| rest.strip_prefix(": "))
                .map(|rest| (candidate.as_str(), rest))
        });
        match begins {
            Some((candidate, rest)) => {
                owner = Some(candidate);
                if candidate == id {
                    mine.push(rest);
                }
            }
            None if owner == Some(id) => mine.push(line),
            None => {}
        }
    }
    (!mine.is_empty()).then(|| mine.join("\n"))
}

/// A note as a notice entry carries it: whole up to the length a write
/// may declare ([`crate::mem_management::NOTE_MAX_LEN`]), cut there
/// with an ellipsis when a record holds more, so fifty entries stay a
/// payload of bounded size.
fn bounded_note(note: String) -> String {
    let max = crate::mem_management::NOTE_MAX_LEN;
    if note.chars().count() <= max {
        return note;
    }
    let mut cut: String = note.chars().take(max).collect();
    cut.push('…');
    cut
}

/// The newest write per entity id in a folder mem's change ledger after
/// `since` (a ledger timestamp; empty for the whole ledger): the
/// identity and note of the last row that names the id. A ledger that
/// cannot be read yields nothing.
pub fn folder_newest_writes(
    mem_root: &Path,
    since: &str,
) -> std::collections::BTreeMap<String, NewestWrite> {
    let mut writes = std::collections::BTreeMap::new();
    let log_path = crate::filesystem::changelog::changelog_path(mem_root);
    let Ok(raw) = std::fs::read_to_string(&log_path) else {
        return writes;
    };
    let cursor = (!since.is_empty() && since != EMPTY_TREE_SHA).then_some(since);
    let mut newest_at: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for line in raw.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        let ts = value.get("ts").and_then(|v| v.as_str()).unwrap_or("");
        if cursor.is_some_and(|c| ts <= c) {
            continue;
        }
        let Some(entity) = value
            .get("entity")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if newest_at.get(entity).is_some_and(|at| ts < at.as_str()) {
            continue;
        }
        newest_at.insert(entity.to_string(), ts.to_string());
        let text = |key: &str| {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        writes.insert(
            entity.to_string(),
            NewestWrite {
                identity: text("identity"),
                note: text("note").map(bounded_note),
            },
        );
    }
    writes
}

/// Whether a ledger timestamp and an entity's `created_date` name the
/// same instant at the coarser of the two precisions (the ledger
/// carries milliseconds, a frontmatter date seconds or a bare day).
fn same_instant(ledger_ts: &str, created_date: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches('Z').replace('T', " ");
    let a = norm(ledger_ts);
    let b = norm(created_date);
    let n = a.len().min(b.len());
    n >= 10 && a[..n] == b[..n]
}

/// Engine-wrapper-level "what changed" shape returned by
/// [`crate::Engine::changes_since`].
///
/// Adds the operator-facing `mem: String` and `warnings:
/// Vec<WarningHint>` that the engine layer owns (rename-similarity
/// clamping, etc.) on top of [`BackendChanges`]. Envelope `title` /
/// `entity_type` fields are enriched from the engine's in-memory
/// store (best-effort — `Removed` envelopes always leave them
/// `None`; missing-from-store entities also leave them `None`).
///
/// Optional `notes` and `memstead_ref` carry per-commit agent-notes and
/// the workspace-level `__MEMSTEAD` ref tip when the caller passes
/// `include_notes: true`. Both fields stay `None` on folder + archive
/// mounts (no commit history to read). MCP and CLI handlers populate
/// them for git-branch mounts by pattern-matching on
/// [`crate::workspace::MountStorage::GitBranch`] and calling
/// `memstead_git_branch::ops::agent_notes::agent_notes_since` directly.
#[derive(Debug, Clone, Serialize)]
pub struct ChangesReport {
    pub mem: String,
    pub since: String,
    pub head: String,
    pub changes: Vec<ChangeEnvelope>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<crate::ops::WarningHint>,
    /// Per-commit agent-notes parsed from commit trailers (git-branch
    /// backend only). `None` when `include_notes` is false or the
    /// backend has no commit history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<crate::ops::agent_notes::CommitNote>>,
    /// Workspace-level `__MEMSTEAD` ref tip (unified schemas + per-mem
    /// configs). `None` when `include_notes` is false or the
    /// workspace has not been migrated to the unified layout yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memstead_ref: Option<String>,
}

// ---- mem_changed notice (reload-before-op awareness contract) ----

/// Max changed-entity count rendered with full per-entity detail
/// (id + change-kind + title + type) before the notice degrades to
/// id+kind only. Rich detail is the expensive part of the payload; an
/// id is cheap. Below this threshold the notice is `mode: "detailed"`.
pub(crate) const NOTICE_DETAILED_MAX: usize = 50;

/// Max changed-entity count that still lists every changed id inline
/// (id + change-kind, `mode: "ids"`). The inline id list is exactly
/// what lets the agent run its own relevance check as a local
/// set-intersection against its context in zero round-trips, so it
/// survives well past the rich-detail threshold. Above this, the
/// notice collapses to `mode: "counts"` and points the agent at
/// `memstead_changes_since` for the full delta.
const NOTICE_IDS_MAX: usize = 500;

/// Per-change-kind counts in `mode: "counts"`. Field names track the
/// `memstead_changes_since` action vocabulary (`updated`, not `modified`)
/// so the notice and the recovery surface speak one language.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct NoticeByChange {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub renamed: usize,
}

/// The size-graceful body of a [`MemChangedNotice`]. Internally
/// tagged on `mode` so a caller decodes one stable shape and branches
/// on the discriminator — no request-shape-dependent polymorphism.
///
/// The `detailed` and `ids` tiers carry [`ChangeEnvelope`]s — the exact
/// per-entity shape `memstead_changes_since` emits (same `action`
/// vocabulary, `from_id` / `to_id` on renames). Sharing the type is the
/// point: an agent that follows the notice's `self_inform` to
/// `memstead_changes_since` decodes one shape on both surfaces, and the two
/// delta representations cannot drift apart in vocabulary or richness.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum NoticeChanges {
    /// Small delta — full per-entity [`ChangeEnvelope`]s with `title` /
    /// `entity_type` enrichment, each with who wrote the entity last
    /// inside the notice's range and why ([`NoticeEntry`]).
    Detailed { entries: Vec<NoticeEntry> },
    /// Medium delta — every changed entity inline as a [`ChangeEnvelope`]
    /// with `title` / `entity_type` stripped. The id list (and a
    /// rename's `from_id` / `to_id`) stays complete; only rich detail is
    /// dropped once the delta exceeds [`NOTICE_DETAILED_MAX`].
    Ids { entries: Vec<ChangeEnvelope> },
    /// Mass change — counts by type and by change-kind, plus the
    /// `memstead_changes_since(since=<from_head>)` instruction for the
    /// full delta. `counts` keys are entity types (omitted for
    /// envelopes whose type the store couldn't resolve, e.g. removed).
    Counts {
        counts: std::collections::BTreeMap<String, usize>,
        by_change: NoticeByChange,
        self_inform: String,
    },
}

/// Who wrote an entity last inside a range, and why: the declared
/// identity and the note of the newest write that names the entity.
/// Either is `None` when that write declared none; a value never comes
/// from an older write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewestWrite {
    pub identity: Option<String>,
    pub note: Option<String>,
}

/// One entry of a notice's `detailed` tier: the change as
/// `memstead_changes_since` reports it, plus the writer and the note of
/// the newest write to that entity inside the notice's range, so a
/// reader that is told a mem moved need not make a second call to learn
/// by whom and why. Either key is absent from the wire when that write
/// declared none. The other tiers, and the entries
/// `memstead_changes_since` returns, carry neither: the walk over the
/// range's writes is the cost the tiers exist to avoid.
///
/// Derefs to its [`ChangeEnvelope`], so a reader of the change itself
/// reads an entry as it reads an envelope.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct NoticeEntry {
    #[serde(flatten)]
    pub change: ChangeEnvelope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl std::ops::Deref for NoticeEntry {
    type Target = ChangeEnvelope;
    fn deref(&self) -> &ChangeEnvelope {
        &self.change
    }
}

/// Non-blocking "the mem moved under you" notice, attached to a
/// response only when a reload happened during the operation. The
/// operation's own result/error rides alongside — this is purely the
/// objective "what else changed" delta, scaled by size, for the agent
/// to judge relevance against (the engine does not filter to a
/// per-agent interest model).
///
/// Built by [`MemChangedNotice::from_delta`] from the
/// `from_head → to_head` [`ChangeEnvelope`] list a reload produced.
/// Entries are ordered lexically by id so two notices over the same
/// delta are byte-identical.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MemChangedNotice {
    pub mem: String,
    pub from_head: String,
    pub to_head: String,
    pub changes: NoticeChanges,
}

impl MemChangedNotice {
    /// Build a notice from the per-entity delta between `from_head`
    /// and `to_head`, degrading by size:
    /// `detailed` (≤ [`NOTICE_DETAILED_MAX`]) → `ids`
    /// (≤ [`NOTICE_IDS_MAX`]) → `counts`. Entries are sorted lexically
    /// by primary id (the `to_id` for a rename) so the output is
    /// deterministic regardless of input order.
    pub fn from_delta(
        mem: String,
        from_head: String,
        to_head: String,
        changes: Vec<ChangeEnvelope>,
    ) -> Self {
        Self::from_delta_with_writes(
            mem,
            from_head,
            to_head,
            changes,
            &std::collections::BTreeMap::new(),
        )
    }

    /// [`Self::from_delta`] with the newest write per entity id
    /// ([`NewestWrite`]), which the `detailed` tier carries on each
    /// entry. An id without an entry in `writes`, and every entry of
    /// the other tiers, carries neither key. For a rename the write is
    /// looked up under the id the entity has now, then under the one it
    /// had.
    pub fn from_delta_with_writes(
        mem: String,
        from_head: String,
        to_head: String,
        mut changes: Vec<ChangeEnvelope>,
        writes: &std::collections::BTreeMap<String, NewestWrite>,
    ) -> Self {
        changes.sort_by(|a, b| a.primary_id().cmp(b.primary_id()));
        let n = changes.len();
        let body = if n <= NOTICE_DETAILED_MAX {
            NoticeChanges::Detailed {
                entries: changes
                    .into_iter()
                    .map(|change| {
                        let write = writes.get(change.primary_id()).or_else(|| match &change {
                            ChangeEnvelope::Renamed { from_id, .. } => {
                                writes.get(AsRef::<str>::as_ref(from_id))
                            }
                            _ => None,
                        });
                        NoticeEntry {
                            identity: write.and_then(|w| w.identity.clone()),
                            note: write.and_then(|w| w.note.clone()),
                            change,
                        }
                    })
                    .collect(),
            }
        } else if n <= NOTICE_IDS_MAX {
            NoticeChanges::Ids {
                entries: changes
                    .iter()
                    .map(ChangeEnvelope::without_metadata)
                    .collect(),
            }
        } else {
            let mut counts: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            let mut by_change = NoticeByChange::default();
            for env in &changes {
                match env {
                    ChangeEnvelope::Added { .. } => by_change.added += 1,
                    ChangeEnvelope::Updated { .. } => by_change.updated += 1,
                    ChangeEnvelope::Removed { .. } => by_change.removed += 1,
                    ChangeEnvelope::Renamed { .. } => by_change.renamed += 1,
                }
                if let Some(t) = env.entity_type() {
                    *counts.entry(t.to_string()).or_default() += 1;
                }
            }
            NoticeChanges::Counts {
                counts,
                by_change,
                self_inform: format!("call memstead_changes_since(since={from_head})"),
            }
        };
        Self {
            mem,
            from_head,
            to_head,
            changes: body,
        }
    }

    /// Total changed-entity count this notice describes, across every
    /// degradation tier. The MCP layer uses it to populate the
    /// `entities_loaded` field of a `MemReloaded` warning synthesised
    /// for an error response — a mutation reloads *inside* the engine
    /// and surfaces only the stashed notice, not a `WarningHint`, so the
    /// error-text warning line is reconstructed from the notice itself.
    pub fn entity_count(&self) -> usize {
        match &self.changes {
            NoticeChanges::Detailed { entries } => entries.len(),
            NoticeChanges::Ids { entries } => entries.len(),
            NoticeChanges::Counts { by_change, .. } => {
                by_change.added + by_change.updated + by_change.removed + by_change.renamed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_at_echoes_cursor() {
        let r = BackendChanges::empty_at("abc");
        assert_eq!(r.since, "abc");
        assert_eq!(r.head, "abc");
        assert!(r.changes.is_empty());
    }

    #[test]
    fn changes_report_omits_notes_and_memstead_ref_when_none() {
        // Default-shaped report (no include_notes) — both Optional
        // fields skip-serialize-when-none. Consumers that don't
        // request notes see no notes/memstead_ref keys on the wire.
        let r = ChangesReport {
            mem: "specs".to_string(),
            since: "abc".to_string(),
            head: "def".to_string(),
            changes: Vec::new(),
            warnings: Vec::new(),
            notes: None,
            memstead_ref: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(
            !json.contains("\"notes\""),
            "notes must be omitted when None: {json}"
        );
        assert!(
            !json.contains("\"memstead_ref\""),
            "memstead_ref must be omitted when None: {json}"
        );
    }

    #[test]
    fn changes_report_emits_notes_and_memstead_ref_when_some() {
        // When include_notes populates the fields, wire shape carries
        // both keys nested at the report root. `memstead_ref` is the SHA
        // of `refs/heads/__MEMSTEAD` (unified schemas + per-mem configs).
        let r = ChangesReport {
            mem: "specs".to_string(),
            since: "abc".to_string(),
            head: "def".to_string(),
            changes: Vec::new(),
            warnings: Vec::new(),
            notes: Some(Vec::new()),
            memstead_ref: Some("aabbccdd".to_string()),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(
            json.contains("\"notes\":["),
            "notes must be present: {json}"
        );
        assert!(
            json.contains("\"memstead_ref\":\"aabbccdd\""),
            "memstead_ref must be present and carry the SHA: {json}"
        );
    }

    #[test]
    fn change_envelope_serializes_action_tag() {
        // Confirms the `action: "added" | "updated" | "removed" |
        // "renamed"` discriminator is emitted as the wire shape MCP
        // callers expect — same as full's existing ChangeEnvelope.
        let env = ChangeEnvelope::Added {
            id: EntityId::new("specs", "hello"),
            title: Some("Hello".to_string()),
            entity_type: Some("spec".to_string()),
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains(r#""action":"added""#));
        assert!(json.contains(r#""title":"Hello""#));
        assert!(json.contains(r#""entity_type":"spec""#));
    }

    #[test]
    fn change_envelope_skips_none_metadata_fields() {
        let env = ChangeEnvelope::Removed {
            id: EntityId::new("specs", "gone"),
            title: None,
            entity_type: None,
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains(r#""action":"removed""#));
        assert!(!json.contains("\"title\""));
        assert!(!json.contains("\"entity_type\""));
    }

    #[test]
    fn change_envelope_renamed_carries_both_ids() {
        let env = ChangeEnvelope::Renamed {
            from_id: EntityId::new("specs", "old"),
            to_id: EntityId::new("specs", "new"),
            title: Some("New".to_string()),
            entity_type: Some("spec".to_string()),
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains(r#""action":"renamed""#));
        assert!(json.contains(r#""from_id":"specs--old""#));
        assert!(json.contains(r#""to_id":"specs--new""#));
    }

    // ---- MemChangedNotice degradation + determinism --------------

    /// `n` added envelopes with predictable ids (`e-0000` …) so tests
    /// can assert ordering and inline-id presence.
    fn added_envelopes(n: usize) -> Vec<ChangeEnvelope> {
        (0..n)
            .map(|i| ChangeEnvelope::Added {
                id: EntityId::new("specs", &format!("e-{i:04}")),
                title: Some(format!("Entity {i}")),
                entity_type: Some("spec".to_string()),
            })
            .collect()
    }

    /// The detailed tier carries, per entry, the newest write to its
    /// entity: the identity and note that write declared, each key
    /// absent when it declared none. Two notices over the same delta
    /// are byte-identical, and the larger tiers carry neither key.
    #[test]
    fn a_detailed_entry_names_the_newest_write_and_drops_what_was_not_declared() {
        let changes = vec![
            ChangeEnvelope::Updated {
                id: EntityId::new("specs", "bbb"),
                title: Some("B".to_string()),
                entity_type: Some("spec".to_string()),
            },
            ChangeEnvelope::Added {
                id: EntityId::new("specs", "aaa"),
                title: Some("A".to_string()),
                entity_type: Some("spec".to_string()),
            },
            ChangeEnvelope::Renamed {
                from_id: EntityId::new("specs", "old"),
                to_id: EntityId::new("specs", "new"),
                title: None,
                entity_type: None,
            },
            ChangeEnvelope::Removed {
                id: EntityId::new("specs", "zzz"),
                title: None,
                entity_type: None,
            },
        ];
        let write = |identity: Option<&str>, note: Option<&str>| NewestWrite {
            identity: identity.map(str::to_string),
            note: note.map(str::to_string),
        };
        let writes: std::collections::BTreeMap<String, NewestWrite> = [
            (
                "specs--aaa".to_string(),
                write(Some("xavier"), Some("made it")),
            ),
            ("specs--bbb".to_string(), write(None, Some("no name"))),
            // A rename's write is found under the id the entity had.
            ("specs--old".to_string(), write(Some("yolanda"), None)),
        ]
        .into_iter()
        .collect();
        let notice = MemChangedNotice::from_delta_with_writes(
            "specs".to_string(),
            "H0".to_string(),
            "H1".to_string(),
            changes.clone(),
            &writes,
        );
        let value = serde_json::to_value(&notice).unwrap();
        let entries = value["changes"]["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0]["id"], "specs--aaa");
        assert_eq!(entries[0]["action"], "added");
        assert_eq!(entries[0]["identity"], "xavier");
        assert_eq!(entries[0]["note"], "made it");
        assert_eq!(entries[1]["id"], "specs--bbb");
        assert_eq!(entries[1]["note"], "no name");
        assert!(entries[1].get("identity").is_none(), "{}", entries[1]);
        assert_eq!(entries[2]["to_id"], "specs--new");
        assert_eq!(entries[2]["identity"], "yolanda");
        assert!(entries[2].get("note").is_none(), "{}", entries[2]);
        // No write known for the id: neither key, not an empty string.
        assert_eq!(entries[3]["id"], "specs--zzz");
        assert!(entries[3].get("identity").is_none() && entries[3].get("note").is_none());

        // The same delta in another order: the same bytes.
        let mut reversed = changes;
        reversed.reverse();
        let again = MemChangedNotice::from_delta_with_writes(
            "specs".to_string(),
            "H0".to_string(),
            "H1".to_string(),
            reversed,
            &writes,
        );
        assert_eq!(
            serde_json::to_string(&notice).unwrap(),
            serde_json::to_string(&again).unwrap()
        );

        // Past the detailed tier the writes are not carried.
        let many = added_envelopes(NOTICE_DETAILED_MAX + 1);
        let all: std::collections::BTreeMap<String, NewestWrite> = many
            .iter()
            .map(|e| (e.primary_id().to_string(), write(Some("xavier"), Some("n"))))
            .collect();
        let ids = MemChangedNotice::from_delta_with_writes(
            "specs".to_string(),
            "H0".to_string(),
            "H1".to_string(),
            many,
            &all,
        );
        let json = serde_json::to_string(&ids).unwrap();
        assert!(json.contains(r#""mode":"ids""#));
        assert!(
            !json.contains("identity") && !json.contains("\"note\""),
            "{json}"
        );
    }

    /// The newest write per id among commit notes that run newest
    /// first: the first note that names an id decides, a rename's
    /// subject names both ids, and a multi-entity commit names every
    /// id its trailer lists.
    #[test]
    fn the_newest_note_that_names_an_id_decides_its_write() {
        let note = |subject_id: &str, ids: &[&str], identity: Option<&str>, text: Option<&str>| {
            crate::ops::agent_notes::CommitNote {
                mem: "specs".to_string(),
                sha: "0".repeat(40),
                subject: format!("memstead: write {subject_id}"),
                tool_verb: None,
                entity_id: Some(subject_id.to_string()),
                note: text.map(str::to_string),
                actor: None,
                tool: None,
                client: None,
                logical_operation_id: None,
                role: None,
                identity: identity.map(str::to_string),
                entity_ids: ids.iter().map(|s| s.to_string()).collect(),
                merged_by: None,
                proposal: None,
                created_ids: Vec::new(),
                co_writers: Vec::new(),
                unattributed_writes: Vec::new(),
                merge_renames: Vec::new(),
                timestamp: 0,
            }
        };
        let notes = vec![
            // Newest: a rename that also rewrote a referrer, no note.
            note(
                "specs--bee → specs--bumble",
                &["specs--aye"],
                Some("yolanda"),
                None,
            ),
            // Older: an update of the referrer.
            note("specs--aye", &[], Some("xavier"), Some("older")),
            note("specs--other", &[], None, Some("only a note")),
        ];
        let writes = newest_writes_from_notes(&notes);
        for id in ["specs--bee", "specs--bumble", "specs--aye"] {
            assert_eq!(writes[id].identity.as_deref(), Some("yolanda"), "{id}");
            assert_eq!(writes[id].note, None, "{id}: never the older write's note");
        }
        assert_eq!(writes["specs--other"].identity, None);
        assert_eq!(writes["specs--other"].note.as_deref(), Some("only a note"));
    }

    /// A batch commit joins its per-entry notes as `<id>: <note>` lines.
    /// Each entity's entry names its own note, an entity whose batch
    /// entry declared none gets none, and no entry carries the joined
    /// text. A note longer than a write may declare is cut.
    #[test]
    fn a_batch_gives_each_entity_its_own_note_and_a_note_has_a_bound() {
        let batch = crate::ops::agent_notes::CommitNote {
            mem: "specs".to_string(),
            sha: "0".repeat(40),
            subject: "memstead: batch-update (3 entities)".to_string(),
            tool_verb: Some("batch-update".to_string()),
            entity_id: Some("(3 entities)".to_string()),
            note: Some(
                "specs--one: note for one\nand its second line\nspecs--three: Reason: three"
                    .to_string(),
            ),
            actor: None,
            tool: None,
            client: None,
            logical_operation_id: None,
            role: None,
            identity: Some("yolanda".to_string()),
            entity_ids: vec![
                "specs--one".to_string(),
                "specs--two".to_string(),
                "specs--three".to_string(),
            ],
            merged_by: None,
            proposal: None,
            created_ids: Vec::new(),
            co_writers: Vec::new(),
            unattributed_writes: Vec::new(),
            merge_renames: Vec::new(),
            timestamp: 0,
        };
        let writes = newest_writes_from_notes(std::slice::from_ref(&batch));
        assert_eq!(
            writes["specs--one"].note.as_deref(),
            Some("note for one\nand its second line")
        );
        assert_eq!(writes["specs--two"].note, None, "its entry declared none");
        assert_eq!(
            writes["specs--three"].note.as_deref(),
            Some("Reason: three")
        );
        for id in ["specs--one", "specs--two", "specs--three"] {
            assert_eq!(writes[id].identity.as_deref(), Some("yolanda"));
        }

        // The writer's join and this reader agree, whatever a note's
        // lines say: a line of one entry's note that begins with another
        // entry's id, or with its own, stays where it was written.
        let joined = [
            crate::vcs::batch_note_line(
                "specs--one",
                "first line of one\nspecs--two: planted by one\nspecs--one: and its own id\n\nafter a gap",
            ),
            crate::vcs::batch_note_line("specs--three", "said by three"),
        ]
        .join("\n");
        let mut planted = batch.clone();
        planted.note = Some(joined);
        let writes = newest_writes_from_notes(std::slice::from_ref(&planted));
        assert_eq!(
            writes["specs--one"].note.as_deref(),
            Some(
                "first line of one\nspecs--two: planted by one\nspecs--one: and its own id\n\nafter a gap"
            )
        );
        assert_eq!(writes["specs--two"].note, None, "nothing was planted on it");
        assert_eq!(
            writes["specs--three"].note.as_deref(),
            Some("said by three")
        );

        // A single write's note is carried whole up to the declared
        // limit and cut beyond it.
        let limit = crate::mem_management::NOTE_MAX_LEN;
        let mut single = batch.clone();
        single.tool_verb = Some("update".to_string());
        single.entity_id = Some("specs--one".to_string());
        single.entity_ids = Vec::new();
        single.note = Some("n".repeat(limit));
        let whole = newest_writes_from_notes(std::slice::from_ref(&single));
        assert_eq!(
            whole["specs--one"].note.as_deref().map(str::len),
            Some(limit)
        );
        single.note = Some("ä".repeat(limit + 40));
        let cut = newest_writes_from_notes(std::slice::from_ref(&single));
        let text = cut["specs--one"].note.clone().unwrap();
        assert_eq!(text.chars().count(), limit + 1);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn notice_small_delta_is_detailed_ordered_and_typed() {
        // Out-of-order input; the notice sorts by id and carries full
        // detail as `ChangeEnvelope`s — same `action` vocabulary as
        // `memstead_changes_since` (`updated`, not `modified`).
        let changes = vec![
            ChangeEnvelope::Updated {
                id: EntityId::new("specs", "bbb"),
                title: Some("Bee".to_string()),
                entity_type: Some("spec".to_string()),
            },
            ChangeEnvelope::Added {
                id: EntityId::new("specs", "aaa"),
                title: Some("Ay".to_string()),
                entity_type: Some("spec".to_string()),
            },
        ];
        let notice = MemChangedNotice::from_delta(
            "specs".to_string(),
            "H0".to_string(),
            "H1".to_string(),
            changes,
        );
        match &notice.changes {
            NoticeChanges::Detailed { entries } => {
                assert_eq!(entries.len(), 2);
                // Lexical by id: aaa before bbb.
                assert_eq!(entries[0].primary_id(), "specs--aaa");
                assert_eq!(entries[0].action(), "added");
                assert_eq!(entries[1].primary_id(), "specs--bbb");
                assert_eq!(entries[1].action(), "updated");
                assert_eq!(entries[1].entity_type(), Some("spec"));
            }
            other => panic!("expected detailed, got {other:?}"),
        }
        let json = serde_json::to_string(&notice).unwrap();
        assert!(json.contains(r#""mode":"detailed""#));
        // Notice and changes_since speak one language: `action`/`updated`,
        // and `entity_type` (not the old `change`/`modified`/`type`).
        assert!(json.contains(r#""action":"updated""#));
        assert!(json.contains(r#""entity_type":"spec""#));
        assert!(
            !json.contains(r#""change":"#),
            "no legacy `change` key: {json}"
        );
    }

    #[test]
    fn notice_entry_is_byte_identical_to_changes_since_envelope() {
        // Parity: the notice's detailed-tier entry and the
        // `memstead_changes_since` event are the *same* serialized shape, so
        // an agent decodes one and decodes the other — no translation
        // between `change`/`action` or `modified`/`updated` or
        // `type`/`entity_type`. Verified by reusing one `ChangeEnvelope`
        // on both surfaces and comparing the JSON.
        let env = ChangeEnvelope::Updated {
            id: EntityId::new("specs", "x"),
            title: Some("X".to_string()),
            entity_type: Some("spec".to_string()),
        };
        let changes_since_json = serde_json::to_value(&env).unwrap();
        let notice = MemChangedNotice::from_delta(
            "specs".to_string(),
            "H0".to_string(),
            "H1".to_string(),
            vec![env],
        );
        let notice_json = serde_json::to_value(&notice).unwrap();
        let entry = &notice_json["changes"]["entries"][0];
        assert_eq!(
            entry, &changes_since_json,
            "notice entry must equal the changes_since envelope verbatim",
        );
    }

    #[test]
    fn notice_renamed_carries_both_ids_and_sorts_under_to_id() {
        // A rename in the notice carries both prior and new id —
        // an agent holding the old id can follow it. Parity with
        // `memstead_changes_since` (from_id + to_id, not remove+add).
        let changes = vec![ChangeEnvelope::Renamed {
            from_id: EntityId::new("specs", "old"),
            to_id: EntityId::new("specs", "new"),
            title: Some("New".to_string()),
            entity_type: Some("spec".to_string()),
        }];
        let notice = MemChangedNotice::from_delta(
            "specs".to_string(),
            "H0".to_string(),
            "H1".to_string(),
            changes,
        );
        match &notice.changes {
            NoticeChanges::Detailed { entries } => {
                assert_eq!(entries[0].primary_id(), "specs--new");
                assert_eq!(entries[0].action(), "renamed");
            }
            other => panic!("expected detailed, got {other:?}"),
        }
        let json = serde_json::to_string(&notice).unwrap();
        assert!(
            json.contains(r#""from_id":"specs--old""#),
            "rename carries from_id: {json}"
        );
        assert!(
            json.contains(r#""to_id":"specs--new""#),
            "rename carries to_id: {json}"
        );
    }

    #[test]
    fn notice_ids_tier_preserves_rename_both_ids() {
        // The rename rule holds in the `ids` tier too: rich detail is dropped but a
        // rename still carries both ids (identity, not detail).
        let mut changes = added_envelopes(NOTICE_DETAILED_MAX);
        changes.push(ChangeEnvelope::Renamed {
            from_id: EntityId::new("specs", "zzz-old"),
            to_id: EntityId::new("specs", "zzz-new"),
            title: Some("Z".to_string()),
            entity_type: Some("spec".to_string()),
        });
        let notice = MemChangedNotice::from_delta(
            "specs".to_string(),
            "H0".to_string(),
            "H1".to_string(),
            changes,
        );
        let json = serde_json::to_string(&notice).unwrap();
        assert!(
            json.contains(r#""mode":"ids""#),
            "expected ids tier: {json}"
        );
        assert!(json.contains(r#""from_id":"specs--zzz-old""#));
        assert!(json.contains(r#""to_id":"specs--zzz-new""#));
        // Rich detail still dropped in the ids tier.
        assert!(!json.contains(r#""title""#), "ids tier drops title: {json}");
    }

    #[test]
    fn notice_medium_delta_degrades_to_ids_with_every_id_inline() {
        // 60 > NOTICE_DETAILED_MAX (50) but ≤ NOTICE_IDS_MAX (500):
        // mode drops to "ids" yet every changed id is still listed.
        let notice = MemChangedNotice::from_delta(
            "specs".to_string(),
            "H0".to_string(),
            "H1".to_string(),
            added_envelopes(60),
        );
        match &notice.changes {
            NoticeChanges::Ids { entries } => {
                assert_eq!(entries.len(), 60, "every changed id stays inline");
                assert_eq!(entries[0].primary_id(), "specs--e-0000");
            }
            other => panic!("expected ids, got {other:?}"),
        }
        let json = serde_json::to_string(&notice).unwrap();
        assert!(json.contains(r#""mode":"ids""#));
        // Rich detail dropped — no title/entity_type keys in ids mode.
        assert!(!json.contains(r#""title""#));
        assert!(!json.contains(r#""entity_type""#));
    }

    #[test]
    fn notice_id_list_outlives_rich_detail() {
        // Complement AC: there is a delta size that drops title/type
        // (mode "ids") while still listing every id — i.e. the id list
        // is budgeted on a distinctly higher threshold than the detail.
        let just_over_detail = NOTICE_DETAILED_MAX + 1;
        let notice = MemChangedNotice::from_delta(
            "specs".to_string(),
            "H0".to_string(),
            "H1".to_string(),
            added_envelopes(just_over_detail),
        );
        match &notice.changes {
            NoticeChanges::Ids { entries } => {
                assert_eq!(entries.len(), just_over_detail);
            }
            other => panic!("expected ids at {just_over_detail}, got {other:?}"),
        }
    }

    #[test]
    fn notice_mass_delta_degrades_to_counts_with_self_inform() {
        // > NOTICE_IDS_MAX (500): collapse to counts. by_change sums
        // every event; counts buckets by type; self_inform names
        // changes_since with the from_head cursor; no ids inline.
        let n = NOTICE_IDS_MAX + 1;
        let notice = MemChangedNotice::from_delta(
            "specs".to_string(),
            "H0".to_string(),
            "H1".to_string(),
            added_envelopes(n),
        );
        match &notice.changes {
            NoticeChanges::Counts {
                counts,
                by_change,
                self_inform,
            } => {
                assert_eq!(by_change.added, n);
                assert_eq!(counts.get("spec").copied(), Some(n));
                assert_eq!(self_inform, "call memstead_changes_since(since=H0)");
            }
            other => panic!("expected counts, got {other:?}"),
        }
        let json = serde_json::to_string(&notice).unwrap();
        assert!(json.contains(r#""mode":"counts""#));
        // No per-entity id list at counts scale.
        assert!(!json.contains("specs--e-"));
    }

    #[test]
    fn notice_is_deterministic_regardless_of_input_order() {
        // Same delta, reversed input → byte-identical JSON.
        let forward = added_envelopes(20);
        let mut reversed = forward.clone();
        reversed.reverse();
        let a = MemChangedNotice::from_delta(
            "specs".to_string(),
            "H0".to_string(),
            "H1".to_string(),
            forward,
        );
        let b = MemChangedNotice::from_delta(
            "specs".to_string(),
            "H0".to_string(),
            "H1".to_string(),
            reversed,
        );
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap(),
        );
    }
}
