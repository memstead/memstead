//! Per-entity history — the narrative query behind an inspector's
//! "how did this entity get this way".
//!
//! The engine already records everything a story needs (commit
//! subjects, provenance trailers, agent notes, batch entity lists,
//! rename chains) but exposed it only as branch-wide feeds; every
//! consumer filtered client-side. This module owns the per-entity
//! filter, rename-chain attribution, and pagination — one semantic for
//! every surface.
//!
//! Data sources per backend, all pre-existing:
//! - git-branch: the commit-note walk that already rides
//!   `changes_since` (`BackendChanges::notes`, newest-first) — full
//!   fidelity: sha, subject, trailers, batch `Entities:` lists,
//!   authoritative rename pairs in the subject.
//! - folder / in-memory: `MemBackend::read_provenance` (the
//!   `.memstead/changes.jsonl` line scan / its in-memory analogue,
//!   oldest-first) — same story, coarser: no batch entity lists, and
//!   rename records carry only the post-rename id, so chains are not
//!   stitchable. Both gaps surface as stated `limitations`, never as
//!   silently absent entries.
//! - archive: the seam records no history — typed refusal, never a
//!   fabricated empty story.
//!
//! This query is read-only and mutates nothing; it deliberately stays
//! narrative-only (content-level diffs per touch are the pairwise
//! `diff` op's job, git-branch consumers only).

use serde::Serialize;

use super::{Engine, EngineError};
use crate::workspace::MountStorage;

/// Default page size when the caller passes `None`.
pub const HISTORY_PAGE_DEFAULT: usize = 50;
/// Hard page-size cap; larger requests clamp here.
pub const HISTORY_PAGE_MAX: usize = 200;

/// One recorded touch of an entity, newest-first in the report.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EntityTouch {
    /// Backend-native reference for this touch: the commit SHA on
    /// git-branch mems, the changelog RFC-3339 timestamp on folder /
    /// in-memory mems. Also the basis of the page cursor.
    pub reference: String,
    /// Touch time, seconds since unix epoch.
    pub timestamp: i64,
    /// The entity's id when this touch happened — pre-rename touches
    /// carry their then-current id (criterion: the story starts at the
    /// first appearance under any prior id).
    pub id_at_touch: String,
    /// Mutation verb (`create` / `update` / `delete` / `relate` /
    /// `rename` / `batch_update`…). `None` when the record predates
    /// the subject convention or the commit is external.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verb: Option<String>,
    /// Commit subject line (git-branch only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// The agent's stated intent, where one was written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    /// `Tool:` trailer (git-branch only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// On a rename touch: the id before the rename. `None` on
    /// non-rename touches — and on folder-backend rename records,
    /// which don't carry the pre-rename id (a stated limitation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
    /// On a rename touch: the id after the rename.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renamed_to: Option<String>,
    /// Every id a multi-entity commit touched (batch context — names
    /// the commit's scope without those entities' own stories
    /// appearing here). Empty for single-entity touches.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub batch_entity_ids: Vec<String>,
    /// Correlation id linking commits of one logical operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_op: Option<String>,
}

/// Where the returned story starts — the visible-truncation contract:
/// whatever the record cannot reach is stated, never silent.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StoryStart {
    /// The oldest recorded touch is the entity's creation — the full
    /// recorded story is reachable through the pages.
    Recorded,
    /// The story stops before the entity's first appearance, and here
    /// is why (unstitchable rename, records predating the changelog,
    /// an adopted mem with no history…).
    Truncated { reason: String },
}

/// Result of [`Engine::entity_history`] — one page of the newest-first
/// touch feed plus the explicit bounds of what it omits.
#[derive(Debug, Clone, Serialize)]
pub struct EntityHistoryReport {
    pub mem: String,
    /// The entity's current id (the query key).
    pub entity_id: String,
    /// This page's touches, newest first.
    pub touches: Vec<EntityTouch>,
    /// Total recorded touches across all pages — with `next_cursor`,
    /// this states exactly what a page omits.
    pub total_recorded: usize,
    /// Opaque continuation: pass back as `cursor` for the next page.
    /// `None` = this page ends the recorded story.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Where the recorded story starts (visible-truncation contract).
    pub story_start: StoryStart,
    /// Stated per-backend gaps (folder rename stitching, batch
    /// attribution). Empty on full-fidelity backends.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub limitations: Vec<String>,
}

/// Split a rename record's `entity_id` field (`"old → new"`) into the
/// pair. Cross-mem peer rewrites (parenthetical qualifier) and
/// malformed values return `None` — they modify wiki-link bodies in a
/// peer mem, never the entity itself.
fn parse_rename_pair(field: &str) -> Option<(String, String)> {
    if field.contains("(cross-mem rewrite") {
        return None;
    }
    let mut parts = field.splitn(2, " → ");
    let old = parts.next()?.trim();
    let new = parts.next()?.trim();
    if old.is_empty() || new.is_empty() {
        return None;
    }
    Some((old.to_string(), new.to_string()))
}

impl Engine {
    /// An entity's recorded history: every touch, newest-first, with
    /// rename chains followed so the story starts at the entity's
    /// first appearance under any prior id. Bounded and pageable —
    /// `page_size` clamps to [`HISTORY_PAGE_MAX`], `cursor` continues
    /// a prior page (`INVALID_CURSOR` when it matches no touch).
    ///
    /// Refusals: `UNKNOWN_MEM`, `ENTITY_NOT_FOUND` (an unknown id
    /// never yields an empty story), `INVALID_INPUT` on archive
    /// mounts (their seam records no history — refusing beats
    /// fabricating emptiness).
    pub fn entity_history(
        &self,
        mem: &str,
        entity_id: &str,
        page_size: Option<usize>,
        cursor: Option<&str>,
    ) -> Result<EntityHistoryReport, EngineError> {
        let m = self.find_mount(mem)?;

        // An unknown entity refuses — marklessness-style honesty: the
        // caller learns "no such entity", never an empty history that
        // reads as "exists, untouched".
        let known = self
            .store
            .all_entities()
            .any(|e| e.mem == mem && e.id.0 == entity_id);
        if !known {
            return Err(EngineError::NotFound {
                id: entity_id.to_string(),
            });
        }

        let mut limitations: Vec<String> = Vec::new();

        // ---- Collect the raw record, newest-first, per backend ----
        let touches: Vec<EntityTouch> = match &m.mount.storage {
            MountStorage::GitBranch { gitdir, branch } => match self.git_branch_ops.as_ref() {
                Some(hook) => {
                    let backend_changes = (hook.changes_since)(
                        gitdir,
                        branch,
                        mem,
                        crate::ops::EMPTY_TREE_SHA,
                        crate::ops::RENAME_SIMILARITY_DEFAULT,
                    )
                    .map_err(EngineError::Backend)?;
                    filter_notes_for_entity(entity_id, &backend_changes.notes)
                }
                None => {
                    limitations.push(
                        "this build carries no git-branch history walk — the recorded story \
                         is not reachable"
                            .to_string(),
                    );
                    Vec::new()
                }
            },
            MountStorage::Folder { .. } | MountStorage::InMemory => {
                limitations.push(
                    "changelog-backed history: rename records carry only the post-rename id \
                     (prior-id chains are not stitchable) and batch mutations record no \
                     per-entity attribution on this backend"
                        .to_string(),
                );
                let records = m
                    .backend
                    .read_provenance(None)
                    .map_err(EngineError::Backend)?;
                filter_provenance_for_entity(entity_id, &records)
            }
            MountStorage::Archive { .. } => {
                return Err(EngineError::InvalidInput(format!(
                    "mem '{mem}' is an archive mount — archives record no history at the \
                     engine seam; open the source mem for the entity's story"
                )));
            }
        };

        // ---- Visible-truncation verdict ----
        // The recorded story is complete exactly when its oldest touch
        // is the entity's creation. Anything else — empty record,
        // oldest touch mid-stream — states where and why it stops.
        let story_start = match touches.last() {
            Some(oldest) if oldest.verb.as_deref() == Some("create") => StoryStart::Recorded,
            Some(oldest) => StoryStart::Truncated {
                reason: format!(
                    "oldest recorded touch is a {} of `{}`, not the entity's creation — \
                     earlier history (an unrecorded prior id, or records predating the \
                     provenance log) is not reachable",
                    oldest.verb.as_deref().unwrap_or("touch"),
                    oldest.id_at_touch
                ),
            },
            None => StoryStart::Truncated {
                reason: "no touches recorded — the entity predates this mem's provenance \
                         record (an adopted or migrated mem)"
                    .to_string(),
            },
        };

        // ---- Page ----
        let size = page_size
            .unwrap_or(HISTORY_PAGE_DEFAULT)
            .clamp(1, HISTORY_PAGE_MAX);
        let start_idx = match cursor {
            None => 0,
            Some(c) => {
                let pos = parse_cursor(c).and_then(|(reference, k)| {
                    touches
                        .iter()
                        .enumerate()
                        .filter(|(_, t)| t.reference == reference)
                        .nth(k)
                        .map(|(i, _)| i + 1)
                });
                pos.ok_or_else(|| EngineError::InvalidChangesCursor {
                    mem: mem.to_string(),
                    since: c.to_string(),
                })?
            }
        };
        let total_recorded = touches.len();
        let page: Vec<EntityTouch> = touches[start_idx.min(total_recorded)..]
            .iter()
            .take(size)
            .cloned()
            .collect();
        let next_cursor = if start_idx + page.len() < total_recorded {
            page.last().map(|last| {
                let k = touches[..start_idx + page.len()]
                    .iter()
                    .filter(|t| t.reference == last.reference)
                    .count()
                    - 1;
                format!("{}@{k}", last.reference)
            })
        } else {
            None
        };

        Ok(EntityHistoryReport {
            mem: mem.to_string(),
            entity_id: entity_id.to_string(),
            touches: page,
            total_recorded,
            next_cursor,
            story_start,
            limitations,
        })
    }
}

/// Cursor wire form: `<reference>@<occurrence>` where `occurrence`
/// disambiguates touches sharing a reference (same-millisecond folder
/// changelog lines; impossible for commit SHAs but the format stays
/// uniform). Opaque to callers.
fn parse_cursor(c: &str) -> Option<(String, usize)> {
    let (reference, k) = c.rsplit_once('@')?;
    if reference.is_empty() {
        return None;
    }
    Some((reference.to_string(), k.parse().ok()?))
}

/// Newest→oldest single pass over the commit-note walk, tracking the
/// entity's id backwards through authoritative rename records:
/// `current` starts at the queried id; a rename whose *new* id equals
/// `current` is that entity's rename touch and flips `current` to the
/// old id, so every older touch is matched under its then-current id.
/// A rename whose *old* id equals `current` is a different entity's
/// departure from an id later reused — deliberately not a touch.
fn filter_notes_for_entity(
    entity_id: &str,
    notes: &[crate::ops::agent_notes::CommitNote],
) -> Vec<EntityTouch> {
    let mut current = entity_id.to_string();
    let mut out: Vec<EntityTouch> = Vec::new();
    for n in notes {
        let base = |id_at: &str| EntityTouch {
            reference: n.sha.clone(),
            timestamp: n.timestamp,
            id_at_touch: id_at.to_string(),
            verb: n.tool_verb.clone(),
            subject: Some(n.subject.clone()),
            note: n.note.clone(),
            actor: n.actor.clone(),
            client: n.client.clone(),
            tool: n.tool.clone(),
            renamed_from: None,
            renamed_to: None,
            batch_entity_ids: Vec::new(),
            logical_op: n.logical_operation_id.clone(),
        };
        if n.tool_verb.as_deref() == Some("rename") {
            if let Some((old, new)) = n.entity_id.as_deref().and_then(parse_rename_pair)
                && new == current
            {
                let mut touch = base(&new);
                touch.renamed_from = Some(old.clone());
                touch.renamed_to = Some(new);
                out.push(touch);
                current = old;
            }
            continue;
        }
        if n.entity_id.as_deref() == Some(current.as_str()) {
            out.push(base(&current));
        } else if n.entity_ids.iter().any(|id| id == &current) {
            let mut touch = base(&current);
            touch.batch_entity_ids = n.entity_ids.clone();
            out.push(touch);
        }
    }
    out
}

/// Changelog-backed record (folder / in-memory): filter the
/// oldest-first provenance feed to the entity's id and reverse to
/// newest-first. Rename records match only when they carry this
/// entity's (post-rename) id — the pre-rename chain is not recorded
/// on this backend (stated limitation upstream).
fn filter_provenance_for_entity(
    entity_id: &str,
    records: &[crate::provenance::Provenance],
) -> Vec<EntityTouch> {
    let mut out: Vec<EntityTouch> = records
        .iter()
        .filter(|p| p.entity.as_deref() == Some(entity_id))
        .map(|p| {
            let is_rename = matches!(p.kind, crate::provenance::ProvenanceKind::Rename);
            EntityTouch {
                reference: crate::filesystem::changelog::format_rfc3339_utc(p.timestamp),
                timestamp: p
                    .timestamp
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                id_at_touch: entity_id.to_string(),
                verb: Some(p.kind.as_str().to_string()),
                subject: None,
                note: p.note.clone(),
                actor: Some(p.actor.as_trailer().to_string()),
                client: p
                    .client
                    .as_ref()
                    .map(|c| format!("{}@{}", c.name, c.version)),
                tool: None,
                renamed_from: None,
                renamed_to: is_rename.then(|| entity_id.to_string()),
                batch_entity_ids: Vec::new(),
                logical_op: p.logical_operation_id.clone(),
            }
        })
        .collect();
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use crate::storage::MemWriter;

    const SEED: &str = "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Seed\n\n## Identity\n\nSeed.\n";

    /// Folder-backed engine with one pre-existing entity written
    /// outside the engine (no changelog record) — mirrors the review
    /// module's fixture.
    fn folder_engine(tmp: &tempfile::TempDir) -> crate::Engine {
        let dir = tmp.path().join("specs");
        if !dir.exists() {
            std::fs::create_dir_all(&dir).unwrap();
            let writer = crate::storage::FilesystemMemWriter::new(dir.clone());
            MemWriter::write_entity(&writer, std::path::Path::new("seed.md"), SEED.as_bytes())
                .unwrap();
            MemWriter::commit(&writer, "seed", &crate::vcs::CommitContext::internal()).unwrap();
        }
        let mount = crate::Mount {
            mem: "specs".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "default",
                semver::Version::new(1, 0, 0),
            )),
            storage: crate::MountStorage::Folder { path: dir.clone() },
            capability: crate::MountCapability::Write,
            lifecycle: crate::MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        };
        let backend =
            Box::new(crate::storage::FilesystemMemWriter::new(dir)) as Box<dyn crate::MemBackend>;
        crate::Engine::from_mounts(vec![(mount, backend)]).unwrap()
    }

    fn create(engine: &mut crate::Engine, title: &str, note: &str) -> String {
        let outcome = engine
            .create_entity(
                crate::CreateEntityArgs {
                    mem: "specs".to_string(),
                    title: title.to_string(),
                    entity_type: "spec".to_string(),
                    sections: [
                        ("identity".to_string(), "x".to_string()),
                        ("purpose".to_string(), "y".to_string()),
                    ]
                    .into_iter()
                    .collect(),
                    metadata: Default::default(),
                    relations: Vec::new(),
                    anchors: Vec::new(),
                    dry_run: false,
                },
                crate::vcs::Actor::Cli,
                None,
                Some(note),
            )
            .unwrap();
        outcome.id.0
    }

    fn update(engine: &mut crate::Engine, id: &str, note: &str) {
        engine
            .update_entity(
                crate::UpdateEntityArgs {
                    id: crate::EntityId(id.to_string()),
                    expected_hash: None,
                    sections: [("identity".to_string(), format!("touched: {note}"))]
                        .into_iter()
                        .collect(),
                    append_sections: Default::default(),
                    patch_sections: Default::default(),
                    metadata: Default::default(),
                    metadata_unset: Vec::new(),
                    dry_run: false,
                    declare_relations: Vec::new(),
                    anchors: Vec::new(),
                    relations_unset: Vec::new(),
                },
                crate::vcs::Actor::App,
                None,
                Some(note),
            )
            .unwrap();
    }

    #[test]
    fn folder_history_serves_touches_with_stated_limitations() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = folder_engine(&tmp);
        let id = create(&mut engine, "Story", "born");
        update(&mut engine, &id, "grew");

        let report = engine.entity_history("specs", &id, None, None).unwrap();
        assert_eq!(report.total_recorded, 2);
        assert_eq!(report.touches.len(), 2);
        // Newest first: the update, then the create.
        assert_eq!(report.touches[0].verb.as_deref(), Some("update"));
        assert_eq!(report.touches[0].note.as_deref(), Some("grew"));
        assert_eq!(report.touches[0].actor.as_deref(), Some("app"));
        assert_eq!(report.touches[1].verb.as_deref(), Some("create"));
        assert_eq!(report.touches[1].actor.as_deref(), Some("cli"));
        assert_eq!(report.story_start, super::StoryStart::Recorded);
        assert!(
            report
                .limitations
                .iter()
                .any(|l| l.contains("rename records carry only the post-rename id")),
            "folder limitations must be stated: {:?}",
            report.limitations
        );
        // Touches of the *other* entity (the seed) never appear.
        assert!(report.touches.iter().all(|t| t.id_at_touch == id));
    }

    #[test]
    fn folder_rename_truncates_visibly_not_silently() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = folder_engine(&tmp);
        let id = create(&mut engine, "Before Rename", "born");
        let outcome = engine
            .rename_entity(
                crate::RenameEntityArgs {
                    id: crate::EntityId(id.clone()),
                    expected_hash: None,
                    new_title: "After Rename".to_string(),
                },
                crate::vcs::Actor::Cli,
                None,
                Some("renamed"),
            )
            .unwrap();
        let new_id = outcome.new_id.0;

        // The folder changelog records renames under the post-rename
        // id only — the story under the new id starts at the rename
        // and SAYS so (never an unexplained short history).
        let report = engine.entity_history("specs", &new_id, None, None).unwrap();
        assert_eq!(report.touches[0].verb.as_deref(), Some("rename"));
        assert!(report.touches[0].renamed_from.is_none());
        match &report.story_start {
            super::StoryStart::Truncated { reason } => {
                assert!(
                    reason.contains("not the entity's creation"),
                    "reason must explain the truncation: {reason}"
                );
            }
            other => panic!("expected visible truncation, got {other:?}"),
        }
    }

    #[test]
    fn refusals_are_typed_never_empty_stories() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = folder_engine(&tmp);
        let id = create(&mut engine, "Real", "born");

        // Unknown mem.
        let err = engine.entity_history("ghost", &id, None, None).unwrap_err();
        assert_eq!(err.code(), "UNKNOWN_MEM");
        // Unknown entity — never an empty history.
        let err = engine
            .entity_history("specs", "specs--nope", None, None)
            .unwrap_err();
        assert_eq!(err.code(), "ENTITY_NOT_FOUND");
        // Garbage cursor.
        let err = engine
            .entity_history("specs", &id, None, Some("zzz@0"))
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_CURSOR");
        let err = engine
            .entity_history("specs", &id, None, Some("no-separator"))
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_CURSOR");
    }

    #[test]
    fn pre_changelog_entity_states_the_empty_record() {
        // The seed entity was written outside the engine — no
        // changelog line exists for it. Its story is empty AND says
        // why, distinguishable from "exists, untouched" by the stated
        // truncation.
        let tmp = tempfile::TempDir::new().unwrap();
        let engine = folder_engine(&tmp);
        let report = engine
            .entity_history("specs", "specs--seed", None, None)
            .unwrap();
        assert!(report.touches.is_empty());
        assert!(matches!(
            report.story_start,
            super::StoryStart::Truncated { .. }
        ));
    }

    #[test]
    fn archive_mounts_refuse_rather_than_fabricate_emptiness() {
        // An archive-declared mount whose backend nonetheless serves
        // the store: the storage arm must refuse before the seam's
        // history-free `read_provenance` can masquerade as an empty
        // story.
        let dir = tempfile::TempDir::new().unwrap();
        let backend = crate::storage::InMemoryBackend::new();
        crate::backend::MemBackend::write_entity(
            &backend,
            std::path::Path::new("seed.md"),
            SEED.as_bytes(),
        )
        .unwrap();
        crate::backend::MemBackend::commit(
            &backend,
            "seed",
            &crate::vcs::CommitContext::internal(),
        )
        .unwrap();
        let mount = crate::Mount {
            mem: "specs".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "default",
                semver::Version::new(1, 0, 0),
            )),
            storage: crate::MountStorage::Archive {
                path: dir.path().join("sealed.mem"),
            },
            capability: crate::MountCapability::Write,
            lifecycle: crate::MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        };
        let engine = crate::Engine::from_mounts(vec![(
            mount,
            Box::new(backend) as Box<dyn crate::MemBackend>,
        )])
        .unwrap();
        let err = engine
            .entity_history("specs", "specs--seed", None, None)
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_INPUT");
    }

    #[test]
    fn pages_compose_without_gaps_or_duplicates() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = folder_engine(&tmp);
        let id = create(&mut engine, "Paged", "born");
        for i in 0..5 {
            update(&mut engine, &id, &format!("touch {i}"));
        }

        let full = engine.entity_history("specs", &id, None, None).unwrap();
        assert_eq!(full.total_recorded, 6);
        assert!(full.next_cursor.is_none());

        // Walk in pages of 2 and re-compose.
        let mut collected: Vec<super::EntityTouch> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = engine
                .entity_history("specs", &id, Some(2), cursor.as_deref())
                .unwrap();
            assert!(page.touches.len() <= 2);
            assert_eq!(page.total_recorded, 6, "every page states the whole");
            collected.extend(page.touches.clone());
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        assert_eq!(
            collected, full.touches,
            "paged walk must equal the single-page story exactly"
        );
    }
}
