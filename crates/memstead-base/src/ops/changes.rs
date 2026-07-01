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
/// `Rename` events surface as `Updated` (folder rename doesn't carry
/// from→to metadata). `Batch` events have no entity id and don't
/// contribute envelopes. The cursor is an RFC-3339 timestamp; the
/// [`EMPTY_TREE_SHA`] sentinel and any non-parseable cursor are
/// treated as "from the beginning". `head` echoes the latest
/// timestamp seen, falling back to `since`.
///
/// Envelopes are id-only (`title` / `entity_type` are `None`); the
/// engine wrapper enriches from its in-memory store.
pub fn folder_changes_since(
    mem_root: &Path,
    mem: &str,
    since: &str,
) -> Result<BackendChanges, BackendError> {
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
    let mut by_entity: std::collections::BTreeMap<String, Aggregate> =
        std::collections::BTreeMap::new();
    let mut max_ts: Option<String> = None;

    let cursor_opt: Option<&str> =
        if since.is_empty() || since == EMPTY_TREE_SHA {
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
        if let Some(c) = cursor_opt {
            if ts_str <= c {
                continue;
            }
        }
        let kind = match value
            .get("kind")
            .and_then(|v| v.as_str())
            .and_then(ProvenanceKind::from_str)
        {
            Some(k) => k,
            None => continue,
        };
        let entity_id = match value.get("entity").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };

        if max_ts.as_deref().map_or(true, |m| ts_str > m) {
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

    let mut changes: Vec<ChangeEnvelope> = Vec::with_capacity(by_entity.len());
    for (entity_str, agg) in by_entity {
        let id = match entity_str.split_once("--") {
            Some((v, slug)) if v == mem => EntityId::new(v, slug),
            _ => continue,
        };
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

    Ok(BackendChanges {
        since: since.to_string(),
        head: max_ts.unwrap_or_else(|| since.to_string()),
        changes,
        notes: Vec::new(),
        memstead_ref: None,
    })
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
const NOTICE_DETAILED_MAX: usize = 50;

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
    /// `entity_type` enrichment.
    Detailed { entries: Vec<ChangeEnvelope> },
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
        mut changes: Vec<ChangeEnvelope>,
    ) -> Self {
        changes.sort_by(|a, b| a.primary_id().cmp(b.primary_id()));
        let n = changes.len();
        let body = if n <= NOTICE_DETAILED_MAX {
            NoticeChanges::Detailed { entries: changes }
        } else if n <= NOTICE_IDS_MAX {
            NoticeChanges::Ids {
                entries: changes.iter().map(ChangeEnvelope::without_metadata).collect(),
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
            NoticeChanges::Detailed { entries } | NoticeChanges::Ids { entries } => {
                entries.len()
            }
            NoticeChanges::Counts { by_change, .. } => {
                by_change.added
                    + by_change.updated
                    + by_change.removed
                    + by_change.renamed
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
        assert!(json.contains("\"notes\":["), "notes must be present: {json}");
        assert!(
            json.contains("\"memstead_ref\":\"aabbccdd\""),
            "memstead_ref must be present and carry the SHA: {json}"
        );
    }

    #[test]
    fn change_envelope_serializes_action_tag() {
        // Confirms the `action: "added" | "updated" | "removed" |
        // "renamed"` discriminator is emitted as the wire shape MCP
        // callers expect — same as pro's existing ChangeEnvelope.
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
        assert!(!json.contains(r#""change":"#), "no legacy `change` key: {json}");
    }

    #[test]
    fn notice_entry_is_byte_identical_to_changes_since_envelope() {
        // F3 parity: the notice's detailed-tier entry and the
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
        // F2: a rename in the notice carries both prior and new id —
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
        assert!(json.contains(r#""from_id":"specs--old""#), "rename carries from_id: {json}");
        assert!(json.contains(r#""to_id":"specs--new""#), "rename carries to_id: {json}");
    }

    #[test]
    fn notice_ids_tier_preserves_rename_both_ids() {
        // F2 holds in the `ids` tier too: rich detail is dropped but a
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
        assert!(json.contains(r#""mode":"ids""#), "expected ids tier: {json}");
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
