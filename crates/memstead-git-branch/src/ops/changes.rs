//! `memstead_changes_since` — two-tree diff between a caller-provided commit
//! SHA and the mem's current HEAD, with rename detection tunable via
//! `rename_similarity` (default 60%).
//!
//! Agents remember the `commit_sha` returned by every mutation and feed it
//! back through this op to pick up incremental deltas without re-scanning
//! the whole mem. The response is a flat list of [`ChangeEnvelope`]s —
//! one per touched entity — with renames surfaced as a single event rather
//! than a removed + added pair (at the selected similarity threshold).
//!
//! "Diff against nothing" sentinel: callers with no prior SHA pass the
//! canonical git empty-tree hash (`4b825dc642cb6eb9a060e54bf8d69288fbee4904`).
//! The diff then treats HEAD as entirely new content.
//!
//! Non-entity paths (e.g. `.memstead/config.json`, schema files) are
//! filtered out: the surface is entity-level. Only `.md` files outside
//! `.memstead/` ever produce envelopes.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

use crate::entity::EntityId;
use crate::entity::id::file_path_to_id;
use crate::ops::WarningHint;
use crate::ops::agent_notes::CommitNote;
use crate::store::Store;
use crate::vcs::VcsError;

// `ChangeEnvelope`, `EMPTY_TREE_SHA`, and `RENAME_SIMILARITY_*` are
// re-exports of the lifted-to-`memstead-base` originals. Existing
// downstream consumers (MCP server, CLI, tests, the macOS UniFFI
// path) keep their `memstead_git_branch::ChangeEnvelope` import untouched
// — the type is the same one `memstead-base` defines, just reachable
// from both crates.
pub use memstead_base::ops::{
    BackendChanges, ChangeEnvelope, EMPTY_TREE_SHA, RENAME_SIMILARITY_DEFAULT,
    RENAME_SIMILARITY_MAX, RENAME_SIMILARITY_MIN,
};

/// Flat diff between `since` and HEAD for one mem. `head` echoes the
/// resolved HEAD commit SHA so agents can remember it as the next
/// polling cursor — saves a round-trip to `memstead_health`. `warnings`
/// carries typed `{code, message, details}` envelopes (e.g.
/// `LIMIT_CLAMPED` when the caller's `rename_similarity` was out of
/// range); omitted from the wire when empty.
///
/// `notes` and `memstead_ref` are populated only when the caller requests
/// `include_notes` (CLI `--include-notes`, MCP `include_notes: true`).
/// They piggyback on the same response so the auto-commit outer-repo
/// cursor flow gets entity-deltas + per-commit agent-notes + the
/// `__MEMSTEAD` ref tip in one round-trip.
#[derive(Debug, Clone, Serialize)]
pub struct ChangesReport {
    pub mem: String,
    pub since: String,
    pub head: String,
    pub changes: Vec<ChangeEnvelope>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<crate::ops::agent_notes::CommitNote>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memstead_ref: Option<String>,
}

/// Compute [`ChangesReport`] for `mem_name`, whose gix repository lives
/// at `git_dir`. `since` is any gix-resolvable commit spec (commit SHA,
/// ref name, `HEAD~1`, …) plus the empty-tree sentinel.
///
/// Error policy:
/// - Unknown `since` ref → [`VcsError::ObjectNotFound`].
/// - Empty repository (no HEAD) + non-sentinel `since` → same.
/// - Empty repository + sentinel `since` → empty report (`head` echoes
///   the sentinel). Matches the "nothing committed yet, nothing to diff"
///   contract so fresh clients don't crash on a brand-new mem.
pub fn changes_since(
    store: &Store,
    mem_name: &str,
    git_dir: &Path,
    since: &str,
    rename_similarity: f32,
    head_ref: Option<&str>,
) -> Result<ChangesReport, VcsError> {
    let repo = gix::open(git_dir)?;

    // #53: anchor a `HEAD`-based `since` revspec on the mem branch so
    // `HEAD~5` / `^` resolve against the per-mem branch tip, not the
    // gitdir's symbolic HEAD (the dummy default branch). Only for the
    // mem-repo backend (signalled by `head_ref` being `Some`); disk-
    // backed mems keep gitdir-HEAD semantics. The original `since` is
    // echoed in the response and error messages.
    let resolve_since = match head_ref {
        Some(_) => crate::ops::diff::normalise_ref_for_mem(mem_name, since),
        None => since.to_string(),
    };

    // Resolve the head tip. For disk-backed mems `head_ref` is `None`
    // and we read the symbolic HEAD; for mem-repo-backed mems the
    // engine passes `refs/heads/<mem>` so we walk the per-mem branch
    // tip the writer commits to. If the repo has no matching commit yet
    // (fresh repo, fresh ref) the head tree is the empty tree — fresh
    // clients sync against a fresh mem via the sentinel without first
    // forcing a mutation.
    let head_lookup: Result<gix::Commit<'_>, ()> = match head_ref {
        Some(ref_name) => repo
            .rev_parse_single(ref_name)
            .ok()
            .and_then(|id| id.object().ok())
            .and_then(|obj| obj.try_into_commit().ok())
            .ok_or(()),
        None => repo.head_commit().map_err(|_| ()),
    };
    let (head_sha, head_tree) = match head_lookup {
        Ok(c) => {
            let sha = c.id.to_hex().to_string();
            let tree = c
                .tree()
                .map_err(|e| VcsError::Git(format!("head tree: {e}")))?;
            (sha, tree)
        }
        Err(()) => (EMPTY_TREE_SHA.to_string(), repo.empty_tree()),
    };

    // Walk the commit range (since, head] via `agent_notes_since` to
    // collect the authoritative rename map and per-commit notes.
    // The engine's own provenance is the deterministic source for
    // rename pairing — relying on gix's content-similarity scorer alone
    // produces false-positive pairings over wide cursor windows. The
    // walk runs unconditionally; the resulting notes
    // and `__MEMSTEAD` ref ride on `BackendChanges` so MCP `include_notes`
    // becomes a renderer-side filter, not an engine-side trigger.
    let notes_report = crate::ops::agent_notes::agent_notes_since(
        mem_name, git_dir, &resolve_since, head_ref,
    )?;
    let rename_map = build_authoritative_rename_map(&notes_report.notes);

    // Resolve `since`. The canonical empty-tree SHA bypasses rev_parse —
    // git itself special-cases that hash, and gix may not have the tree
    // object physically present in a fresh odb.
    let since_tree = if resolve_since == EMPTY_TREE_SHA {
        repo.empty_tree()
    } else {
        let id = repo
            .rev_parse_single(resolve_since.as_str())
            .map_err(|e| VcsError::ObjectNotFound(format!("{since}: {e}")))?;
        let object = id
            .object()
            .map_err(|e| VcsError::ObjectNotFound(format!("{since}: {e}")))?;
        let commit = object
            .try_into_commit()
            .map_err(|_| VcsError::ObjectNotFound(format!("{since} is not a commit")))?;
        commit
            .tree()
            .map_err(|e| VcsError::Git(format!("since tree: {e}")))?
    };

    // Short-circuit identical trees. Skips the diff pipeline entirely
    // for the common "poll with no changes" case. Notes + memstead_ref
    // still ride along — a poll with no diff but with intermediate
    // commits (e.g. all rewrites cancel) still wants the agent-note
    // feed for provenance reconstruction.
    if since_tree.id == head_tree.id {
        return Ok(ChangesReport {
            mem: mem_name.to_string(),
            since: since.to_string(),
            head: head_sha,
            changes: Vec::new(),
            warnings: Vec::new(),
            notes: Some(notes_report.notes),
            memstead_ref: notes_report.memstead_ref,
        });
    }

    // Two-tree diff with 60% rename similarity. We walk deletions +
    // additions and let gix pair them up via content-similarity into
    // `Rewrite` events as the fallback for renames the engine did
    // not author (external `git mv`, pre-provenance migrations).
    let mut platform = since_tree
        .changes()
        .map_err(|e| VcsError::Git(format!("diff init: {e}")))?;
    let rewrites = gix::diff::Rewrites {
        copies: None,
        percentage: Some(rename_similarity),
        limit: 1000,
        track_empty: false,
    };
    platform.options(|opts| {
        opts.track_rewrites(Some(rewrites));
    });

    // Collect raw diff events first; commit-note-driven rename collapse
    // runs over them in a second pass so authoritative pairs win over
    // gix-similarity coincidences.
    let mut additions: Vec<EntityId> = Vec::new();
    let mut deletions: Vec<EntityId> = Vec::new();
    let mut modifications: Vec<EntityId> = Vec::new();
    let mut gix_rewrites: Vec<(EntityId, EntityId)> = Vec::new();
    platform
        .for_each_to_obtain_tree(
            &head_tree,
            |change| -> Result<std::ops::ControlFlow<()>, std::convert::Infallible> {
                use gix::object::tree::diff::Change;
                match change {
                    Change::Addition { location, .. } => {
                        if let Some(id) = path_to_entity_id(mem_name, location.as_ref()) {
                            additions.push(id);
                        }
                    }
                    Change::Deletion { location, .. } => {
                        if let Some(id) = path_to_entity_id(mem_name, location.as_ref()) {
                            deletions.push(id);
                        }
                    }
                    Change::Modification { location, .. } => {
                        if let Some(id) = path_to_entity_id(mem_name, location.as_ref()) {
                            modifications.push(id);
                        }
                    }
                    Change::Rewrite {
                        source_location,
                        location,
                        ..
                    } => {
                        let from = path_to_entity_id(mem_name, source_location.as_ref());
                        let to = path_to_entity_id(mem_name, location.as_ref());
                        if let (Some(from_id), Some(to_id)) = (from, to) {
                            gix_rewrites.push((from_id, to_id));
                        }
                    }
                }
                Ok(std::ops::ControlFlow::Continue(()))
            },
        )
        .map_err(|e| VcsError::Git(format!("diff: {e}")))?;

    let envelopes = combine_with_rename_map(
        store,
        rename_map,
        additions,
        deletions,
        modifications,
        gix_rewrites,
    );

    Ok(ChangesReport {
        mem: mem_name.to_string(),
        since: since.to_string(),
        head: head_sha,
        changes: envelopes,
        warnings: Vec::new(),
        notes: Some(notes_report.notes),
        memstead_ref: notes_report.memstead_ref,
    })
}

/// Build the authoritative `old_id → new_id` rename map by walking
/// commit notes from oldest to newest and composing transitive
/// rename chains in place.
///
/// `agent_notes_since` returns notes newest-first (git-log order); we
/// reverse to chronological order so a follow-up rename `B → C` after
/// a prior `A → B` collapses to a single `A → C` entry rather than
/// the (logically equivalent but harder to consume) `A → B` plus
/// `B → C` pair.
///
/// Cross-mem peer commits — subject `memstead: rename A → B (cross-mem
/// rewrite in V)` — are filtered out. They modify wiki-link bodies in
/// the peer mem but never add or remove A or B there, so the local
/// diff would never match them; including them is harmless but
/// confusing for downstream readers of the map.
fn build_authoritative_rename_map(notes: &[CommitNote]) -> HashMap<EntityId, EntityId> {
    let mut forward: HashMap<EntityId, EntityId> = HashMap::new();
    let mut reverse: HashMap<EntityId, EntityId> = HashMap::new();
    for note in notes.iter().rev() {
        if note.tool_verb.as_deref() != Some("rename") {
            continue;
        }
        let Some(id_str) = note.entity_id.as_deref() else {
            continue;
        };
        let Some((old_id, new_id)) = parse_rename_entity_field(id_str) else {
            continue;
        };
        // Transitive collapse: if `old_id` is the target of an earlier
        // rename, replay the chain so the map ends up keyed on the
        // original source.
        let origin = reverse.remove(&old_id).unwrap_or_else(|| old_id.clone());
        if origin != old_id {
            forward.remove(&origin);
        }
        forward.insert(origin.clone(), new_id.clone());
        reverse.insert(new_id, origin);
    }
    forward
}

/// Split the `entity_id` field of a parsed rename commit subject into
/// `(old_id, new_id)`. Returns `None` for cross-mem peer rewrites
/// (parenthetical qualifier) and malformed entries.
pub(super) fn parse_rename_entity_field(field: &str) -> Option<(EntityId, EntityId)> {
    if field.contains("(cross-mem rewrite") {
        return None;
    }
    let mut parts = field.splitn(2, " → ");
    let old = parts.next()?.trim();
    let new = parts.next()?.trim();
    if old.is_empty() || new.is_empty() {
        return None;
    }
    Some((EntityId(old.to_string()), EntityId(new.to_string())))
}

/// Combine raw diff events with the authoritative rename map into the
/// final [`ChangeEnvelope`] list. Pairs `Addition(new) + Deletion(old)`
/// driven by the note map first; falls back to gix-similarity
/// `Rewrite` events for renames the engine did not author; emits
/// remaining additions/deletions/modifications as-is.
///
/// Rename-then-delete edge case: a note records `A → B` (transitively
/// `A → C`) but the new id is also deleted in the same window — the
/// gix diff sees only `Deletion(A)`. The map's `A → ?` lookup finds
/// no matching addition; the entity at A is gone; emit
/// `Removed { id: A }` — the rename was undone before the cursor saw it.
fn combine_with_rename_map(
    store: &Store,
    rename_map: HashMap<EntityId, EntityId>,
    additions: Vec<EntityId>,
    deletions: Vec<EntityId>,
    modifications: Vec<EntityId>,
    gix_rewrites: Vec<(EntityId, EntityId)>,
) -> Vec<ChangeEnvelope> {
    use std::collections::HashSet;
    let addition_set: HashSet<&EntityId> = additions.iter().collect();
    let deletion_set: HashSet<&EntityId> = deletions.iter().collect();

    let mut envelopes: Vec<ChangeEnvelope> = Vec::new();
    let mut absorbed_add: HashSet<EntityId> = HashSet::new();
    let mut absorbed_del: HashSet<EntityId> = HashSet::new();

    // Authoritative renames first — the engine's own provenance wins.
    for (old_id, new_id) in &rename_map {
        let has_old_del = deletion_set.contains(old_id);
        let has_new_add = addition_set.contains(new_id);
        if has_old_del && has_new_add {
            envelopes.push(ChangeEnvelope::Renamed {
                from_id: old_id.clone(),
                to_id: new_id.clone(),
                title: title_for(store, new_id),
                entity_type: type_for(store, new_id),
            });
            absorbed_add.insert(new_id.clone());
            absorbed_del.insert(old_id.clone());
        } else if has_old_del {
            // Rename-then-delete: the entity at old_id is gone; the
            // intermediate new id never lands. Emit Removed.
            envelopes.push(ChangeEnvelope::Removed {
                id: old_id.clone(),
                title: None,
                entity_type: None,
            });
            absorbed_del.insert(old_id.clone());
        }
        // Addition without matching Deletion: the new id appeared
        // without the old id being removed — would mean a re-create
        // at the new id outside the engine's rename path. Leave the
        // addition unabsorbed; it falls through as Added.
    }

    // gix-similarity fallback for renames the engine did not author.
    // Skip pairs the authoritative map already absorbed; otherwise
    // emit as Renamed.
    for (from_id, to_id) in gix_rewrites {
        if absorbed_del.contains(&from_id) || absorbed_add.contains(&to_id) {
            continue;
        }
        envelopes.push(ChangeEnvelope::Renamed {
            title: title_for(store, &to_id),
            entity_type: type_for(store, &to_id),
            from_id,
            to_id,
        });
    }

    // Remaining additions / deletions / modifications.
    for id in additions {
        if absorbed_add.contains(&id) {
            continue;
        }
        envelopes.push(ChangeEnvelope::Added {
            title: title_for(store, &id),
            entity_type: type_for(store, &id),
            id,
        });
    }
    for id in deletions {
        if absorbed_del.contains(&id) {
            continue;
        }
        envelopes.push(ChangeEnvelope::Removed {
            id,
            title: None,
            entity_type: None,
        });
    }
    for id in modifications {
        envelopes.push(ChangeEnvelope::Updated {
            title: title_for(store, &id),
            entity_type: type_for(store, &id),
            id,
        });
    }
    envelopes
}

/// Translate a tree-diff path into a mem-qualified `EntityId`. Returns
/// `None` for non-entity paths so engine config / schema edits don't
/// leak into the entity-level delta surface.
fn path_to_entity_id(mem: &str, path: &gix::bstr::BStr) -> Option<EntityId> {
    let s = std::str::from_utf8(path.as_ref()).ok()?;
    if s.is_empty() || !s.ends_with(".md") {
        return None;
    }
    // `.memstead/` is engine-internal; nothing under it maps to an entity.
    if s.starts_with(".memstead/") {
        return None;
    }
    Some(file_path_to_id(s, mem))
}

/// Best-effort title lookup. Stubs resolve to their hollow title (usually
/// the slug); real entities resolve to the authored title. Missing-from-
/// store → `None`.
fn title_for(store: &Store, id: &EntityId) -> Option<String> {
    store.get(id).map(|e| e.title.clone())
}

/// Best-effort entity-type lookup. Mirrors `title_for`. Missing-from-store
/// (including removed-in-this-diff entities) → `None`.
fn type_for(store: &Store, id: &EntityId) -> Option<String> {
    store.get(id).map(|e| e.entity_type.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::agent_notes::CommitNote;

    fn rename_note(old: &str, new: &str) -> CommitNote {
        CommitNote {
            mem: "specs".to_string(),
            sha: "abc".to_string(),
            subject: format!("memstead: rename {old} → {new}"),
            tool_verb: Some("rename".to_string()),
            entity_id: Some(format!("{old} → {new}")),
            note: None,
            actor: None,
            tool: None,
            client: None,
            logical_operation_id: None,
            entity_ids: Vec::new(),
            timestamp: 0,
        }
    }

    #[test]
    fn parse_rename_entity_field_extracts_old_and_new_ids() {
        let parsed =
            parse_rename_entity_field("specs--old-name → specs--new-name").unwrap();
        assert_eq!(parsed.0.0, "specs--old-name");
        assert_eq!(parsed.1.0, "specs--new-name");
    }

    #[test]
    fn parse_rename_entity_field_rejects_cross_mem_rewrite_qualifier() {
        // Cross-mem peer rewrites carry a parenthetical qualifier
        // (rename.rs:433 path); they describe a rename that happened
        // in some other mem and so cannot drive local pairing.
        assert!(
            parse_rename_entity_field(
                "specs--old → specs--new (cross-mem rewrite in `other`)"
            )
            .is_none()
        );
    }

    #[test]
    fn build_authoritative_rename_map_collapses_transitive_chain() {
        // A → B then B → C in
        // the same cursor window collapses to the single edge
        // A → C — the gix diff sees only Addition(C) + Deletion(A)
        // (B is intermediate and never lands on the head tree), so
        // the map must end keyed on the original source.
        // `agent_notes_since` returns notes newest-first, so the
        // input order here mirrors that contract.
        let notes = vec![
            rename_note("specs--b", "specs--c"),
            rename_note("specs--a", "specs--b"),
        ];
        let map = build_authoritative_rename_map(&notes);
        assert_eq!(map.len(), 1, "transitive collapse: {map:?}");
        let final_target = map
            .get(&EntityId("specs--a".to_string()))
            .expect("composed map keyed on original source");
        assert_eq!(final_target.0, "specs--c");
    }

    #[test]
    fn build_authoritative_rename_map_skips_cross_mem_peer_commits() {
        // The renaming entity's mem commit drives pairing; cross-
        // mem peer commits (subject carries the `(cross-mem
        // rewrite in V)` qualifier) only rewrite wiki-link bodies in
        // the peer and don't add/remove the renaming entity there.
        let mut peer = rename_note("specs--a", "specs--b");
        peer.subject = "memstead: rename specs--a → specs--b (cross-mem rewrite in `peers`)"
            .to_string();
        peer.entity_id = Some(
            "specs--a → specs--b (cross-mem rewrite in `peers`)".to_string(),
        );
        let map = build_authoritative_rename_map(&[peer]);
        assert!(map.is_empty(), "cross-mem peer commit must not enter the map: {map:?}");
    }

    #[test]
    fn build_authoritative_rename_map_ignores_non_rename_verbs() {
        let note = CommitNote {
            mem: "specs".to_string(),
            sha: "abc".to_string(),
            subject: "memstead: update specs--foo".to_string(),
            tool_verb: Some("update".to_string()),
            entity_id: Some("specs--foo".to_string()),
            note: None,
            actor: None,
            tool: None,
            client: None,
            logical_operation_id: None,
            entity_ids: Vec::new(),
            timestamp: 0,
        };
        let map = build_authoritative_rename_map(&[note]);
        assert!(map.is_empty(), "non-rename verbs ignored: {map:?}");
    }

    #[test]
    fn combine_with_rename_map_pairs_authoritative_add_and_del() {
        use crate::store::Store;
        let store = Store::new();
        let mut rename_map: HashMap<EntityId, EntityId> = HashMap::new();
        rename_map.insert(
            EntityId("specs--a".to_string()),
            EntityId("specs--b".to_string()),
        );
        let envelopes = combine_with_rename_map(
            &store,
            rename_map,
            vec![EntityId("specs--b".to_string())],
            vec![EntityId("specs--a".to_string())],
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(envelopes.len(), 1);
        match &envelopes[0] {
            ChangeEnvelope::Renamed { from_id, to_id, .. } => {
                assert_eq!(from_id.0, "specs--a");
                assert_eq!(to_id.0, "specs--b");
            }
            other => panic!("expected Renamed, got {other:?}"),
        }
    }

    #[test]
    fn combine_with_rename_map_rename_then_delete_emits_removed() {
        // A rename followed by a delete of the new
        // id in the same window. Note says A → B; gix diff says
        // Deletion(A) only (B was added then removed). Emit
        // Removed { id: A } — the entity is gone.
        use crate::store::Store;
        let store = Store::new();
        let mut rename_map: HashMap<EntityId, EntityId> = HashMap::new();
        rename_map.insert(
            EntityId("specs--a".to_string()),
            EntityId("specs--b".to_string()),
        );
        let envelopes = combine_with_rename_map(
            &store,
            rename_map,
            Vec::new(),
            vec![EntityId("specs--a".to_string())],
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(envelopes.len(), 1);
        match &envelopes[0] {
            ChangeEnvelope::Removed { id, .. } => {
                assert_eq!(id.0, "specs--a");
            }
            other => panic!("expected Removed, got {other:?}"),
        }
    }

    #[test]
    fn combine_with_rename_map_keeps_gix_rewrites_as_fallback() {
        // External rename (e.g. `git mv` outside the engine) — no
        // commit note exists; gix's content-similarity scorer pairs
        // the two file paths and emits a Rewrite event. The combine
        // pass keeps it as Renamed because neither side appears in
        // the authoritative map.
        use crate::store::Store;
        let store = Store::new();
        let envelopes = combine_with_rename_map(
            &store,
            HashMap::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![(
                EntityId("specs--external-old".to_string()),
                EntityId("specs--external-new".to_string()),
            )],
        );
        assert_eq!(envelopes.len(), 1);
        match &envelopes[0] {
            ChangeEnvelope::Renamed { from_id, to_id, .. } => {
                assert_eq!(from_id.0, "specs--external-old");
                assert_eq!(to_id.0, "specs--external-new");
            }
            other => panic!("expected Renamed from gix fallback, got {other:?}"),
        }
    }

    #[test]
    fn combine_with_rename_map_authoritative_wins_over_gix_rewrite() {
        // Both sources fire: the engine wrote a `memstead: rename A → C`
        // note AND gix's similarity scorer paired A↔B (false
        // positive). The note wins; B never appears as a rename
        // target. B falls through to whatever its underlying
        // addition path produced (here we exercise only that the
        // gix pair is suppressed).
        use crate::store::Store;
        let store = Store::new();
        let mut rename_map: HashMap<EntityId, EntityId> = HashMap::new();
        rename_map.insert(
            EntityId("specs--a".to_string()),
            EntityId("specs--c".to_string()),
        );
        let envelopes = combine_with_rename_map(
            &store,
            rename_map,
            vec![EntityId("specs--c".to_string())],
            vec![EntityId("specs--a".to_string())],
            Vec::new(),
            vec![(
                EntityId("specs--a".to_string()),
                EntityId("specs--b".to_string()),
            )],
        );
        // One Renamed (A → C from the note). The gix Rewrite A → B
        // is suppressed because A was absorbed by the authoritative
        // pairing.
        assert_eq!(envelopes.len(), 1);
        match &envelopes[0] {
            ChangeEnvelope::Renamed { from_id, to_id, .. } => {
                assert_eq!(from_id.0, "specs--a");
                assert_eq!(to_id.0, "specs--c");
            }
            other => panic!("expected note-driven Renamed, got {other:?}"),
        }
    }

    #[test]
    fn path_to_entity_id_strips_md_and_prefixes_mem() {
        let bstr = gix::bstr::BString::from("architecture/result.md");
        let id = path_to_entity_id("specs", bstr.as_ref()).unwrap();
        assert_eq!(id.0, "specs--architecture/result");
    }

    #[test]
    fn path_to_entity_id_skips_memstead_internal_files() {
        let bstr = gix::bstr::BString::from(".memstead/config.json");
        assert!(path_to_entity_id("specs", bstr.as_ref()).is_none());
        // A `.md` under `.memstead/` is engine-internal and never an entity.
        let internal_md = gix::bstr::BString::from(".memstead/notes.md");
        assert!(path_to_entity_id("specs", internal_md.as_ref()).is_none());
    }

    #[test]
    fn path_to_entity_id_skips_non_markdown() {
        let bstr = gix::bstr::BString::from("image.png");
        assert!(path_to_entity_id("specs", bstr.as_ref()).is_none());
    }

    #[test]
    fn empty_tree_sentinel_is_canonical_git_hash() {
        assert_eq!(EMPTY_TREE_SHA.len(), 40);
        // Prefix check — this SHA is stable across every git version.
        assert!(EMPTY_TREE_SHA.starts_with("4b825dc6"));
    }
}
