//! `Engine::diff(ref_a, ref_b)` implementation for git-branch mounts.
//!
//! Walks the two tree objects pointed to by `ref_a` and `ref_b` inside
//! the workspace's mem-repo gitdir, produces a per-entity diff
//! ([`Diff`]) that downstream replay / review / preview / audit
//! tooling consumes. Same backbone the [`changes_since`](super::changes)
//! op uses — two-tree diff with rename detection — but expanded into a
//! two-endpoint shape with optional content enrichment.
//!
//!
//! ## V1 scope
//!
//! - Added / Modified / Deleted entries from a vanilla gix tree-diff.
//! - Content enrichment per `DiffConfig::include_content`.
//! - `InvalidEntity` entries for paths that fail UTF-8 / parse.
//!
//! ## V1 gaps (handover candidates)
//!
//! - **Rename detection**: deferred. The agent-notes-driven rename
//!   collapse `changes_since` performs requires a `since` cursor in
//!   the same mem's history. A generic two-ref diff (potentially
//!   cross-mem) needs a different walk; v1 emits Added/Deleted pairs
//!   instead of `Renamed` and leaves rename-chain unfilled.
//! - **Cross-entity ripple**: `IncomingRipple` lists stay empty in
//!   the entries. Populating them requires a per-side wiki-link graph
//!   reconstruction that the engine's current in-memory store does
//!   not maintain for arbitrary refs.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use gix::object::tree::diff::Change;

use memstead_base::backend::BackendError;
use memstead_base::entity::EntityId;
use memstead_base::entity::id::file_path_to_id;
use memstead_base::entity::parser::{
    body_after_frontmatter, extract_inline_links_lenient, peek_title_and_type,
};
use memstead_base::ops::{Diff, DiffConfig, EntityDiff, IncomingRipple};

use crate::EMPTY_TREE_SHA;

/// Normalise a caller-supplied ref against the mount's declared
/// branch, shared by `memstead_diff` and `memstead_changes_since`.
///
/// Rewrites a leading `HEAD` *token* — the whole ref (`HEAD`) or the base
/// of a revspec (`HEAD~5`, `HEAD^`, `HEAD^{tree}`, `HEAD@{1}`) — to the
/// mount's declared branch ref, preserving the suffix, so resolution
/// targets the selected mem's branch tip rather than the mem-repo
/// gitdir's symbolic HEAD (which points at the dummy default branch).
/// The anchor is the DECLARED branch, never a ref derived from the mem
/// name — the two differ on namespaced mounts. gix already parses the
/// revspec suffix; this only re-anchors the `HEAD` base. A ref that
/// merely *starts with* `HEAD` (e.g. `HEADER`, `HEAD-foo`) is left
/// alone — the character after `HEAD` must be a revspec operator
/// (`~ ^ : @`) or end-of-string. Targeted at the per-mem entry point
/// only: mem-less callers (cross-mem diffs naming a peer branch) pass
/// fully-qualified refs that don't begin with the `HEAD` token.
///
/// The empty-tree sentinel is handled inside `resolve_tree` so callers see
/// a single dispatch.
pub(crate) fn normalise_ref_for_branch(branch: &str, raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("HEAD")
        && (rest.is_empty() || rest.starts_with(['~', '^', ':', '@']))
    {
        return format!("{}{rest}", memstead_base::branch_full_ref(branch));
    }
    raw.to_string()
}

/// Resolve a ref to its tree, returning a typed-marker error
/// (`UNKNOWN_REF:<raw>`) when `rev_parse` refuses.
///
/// The canonical empty-tree SHA (`4b825dc642cb6eb9a060e54bf8d69288fbee4904`)
/// short-circuits to `repo.empty_tree()`. Matches
/// `memstead_changes_since`'s sentinel handling so callers who learned
/// the sentinel from the sibling tool find it works here too. Real
/// tree-only SHAs that are not the canonical sentinel continue to
/// refuse with `UNKNOWN_REF` — the sentinel handling is keyed on
/// the literal hash, not on "is it a tree".
fn resolve_tree<'r>(repo: &'r gix::Repository, raw: &str) -> Result<gix::Tree<'r>, BackendError> {
    if raw == EMPTY_TREE_SHA {
        return Ok(repo.empty_tree());
    }
    let id = repo
        .rev_parse_single(raw)
        .map_err(|_| BackendError::Other(format!("UNKNOWN_REF: {raw}")))?;
    let object = id
        .object()
        .map_err(|_| BackendError::Other(format!("UNKNOWN_REF: {raw}")))?;
    let commit = object
        .try_into_commit()
        .map_err(|_| BackendError::Other(format!("UNKNOWN_REF: {raw} is not a commit")))?;
    commit
        .tree()
        .map_err(|e| BackendError::Other(format!("tree({raw}): {e}")))
}

fn sha_for(repo: &gix::Repository, raw: &str) -> Result<String, BackendError> {
    if raw == EMPTY_TREE_SHA {
        return Ok(EMPTY_TREE_SHA.to_string());
    }
    let id = repo
        .rev_parse_single(raw)
        .map_err(|_| BackendError::Other(format!("UNKNOWN_REF: {raw}")))?;
    Ok(id.detach().to_string())
}

/// Translate a tree-diff path to a mem-qualified entity id, returning
/// `None` for engine-internal paths (`.memstead/...`) and non-markdown
/// entries. Mirrors `changes::path_to_entity_id` but is callable from
/// here without exporting the private helper.
fn path_to_entity_id(mem: &str, path: &gix::bstr::BStr) -> Option<EntityId> {
    let s = std::str::from_utf8(path.as_ref()).ok()?;
    if s.is_empty() || !s.ends_with(".md") {
        return None;
    }
    if s.starts_with(".memstead/") {
        return None;
    }
    Some(file_path_to_id(s, mem))
}

/// Read the markdown body for `path` from `tree`. Returns `None` when
/// the path is not present in the tree, or when the lookup fails.
fn read_md_at_path(
    _repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    path: &gix::bstr::BStr,
) -> Option<String> {
    let entry = tree
        .lookup_entry_by_path(path.to_string().as_str())
        .ok()??;
    let object = entry.object().ok()?;
    let blob = object.try_into_blob().ok()?;
    String::from_utf8(blob.data.clone()).ok()
}

/// Extract `(title, entity_type)` from an optionally-present markdown
/// blob. `(None, None)` when the blob is absent / non-UTF-8 or carries
/// neither a `# ` heading nor a `type:` frontmatter field.
fn peek_meta(raw: &Option<String>) -> (Option<String>, Option<String>) {
    raw.as_deref()
        .map(peek_title_and_type)
        .unwrap_or((None, None))
}

/// Two-ref structural diff. See module docs for the v1 scope and the
/// known gaps (rename, ripple) that will surface as handover items.
/// `branch` is the mount's declared branch; a bare `HEAD` token in
/// either ref re-anchors onto it.
pub fn diff_two_refs(
    gitdir: &Path,
    branch: &str,
    mem: &str,
    ref_a: &str,
    ref_b: &str,
    config: &DiffConfig,
) -> Result<Diff, BackendError> {
    if !gitdir.is_dir() {
        return Err(BackendError::Other(format!(
            "gitdir not found: {}",
            gitdir.display()
        )));
    }
    let repo = gix::open(gitdir).map_err(|e| BackendError::Other(format!("gix open: {e}")))?;
    // Bare `HEAD` substitutes to the mount's declared branch so the
    // diff targets the mem's branch tip (not the gitdir's symbolic
    // HEAD on a dummy default branch). Per-mem callers always pass
    // the mount's branch; the substitution is unconditional on this
    // entry point.
    let normalised_a = normalise_ref_for_branch(branch, ref_a);
    let normalised_b = normalise_ref_for_branch(branch, ref_b);
    let tree_a = resolve_tree(&repo, &normalised_a)?;
    let tree_b = resolve_tree(&repo, &normalised_b)?;
    let resolved_a_sha = sha_for(&repo, &normalised_a)?;
    let resolved_b_sha = sha_for(&repo, &normalised_b)?;

    let mut platform = tree_a
        .changes()
        .map_err(|e| BackendError::Other(format!("diff init: {e}")))?;
    let rewrites = gix::diff::Rewrites {
        copies: None,
        percentage: Some(config.rename_similarity),
        limit: 1000,
        track_empty: false,
    };
    platform.options(|opts| {
        opts.track_rewrites(Some(rewrites));
    });

    let mut entries: Vec<EntityDiff> = Vec::new();
    platform
        .for_each_to_obtain_tree(
            &tree_b,
            |change| -> Result<std::ops::ControlFlow<()>, std::convert::Infallible> {
                match change {
                    Change::Addition { location, .. } => {
                        if let Some(id) = path_to_entity_id(mem, location) {
                            // Read the post-state blob unconditionally to
                            // populate `title`/`entity_type` (the docstring's
                            // metadata-only shape); keep the full body as
                            // `content_after` only when content is requested.
                            let raw = read_md_at_path(&repo, &tree_b, location);
                            let (title, entity_type) = peek_meta(&raw);
                            let content_after = if config.include_content { raw } else { None };
                            entries.push(EntityDiff::Added {
                                id,
                                title,
                                entity_type,
                                content_after,
                                ripple: Vec::new(),
                            });
                        }
                    }
                    Change::Deletion { location, .. } => {
                        if let Some(id) = path_to_entity_id(mem, location) {
                            // The entity still exists on `ref_a`; pull its
                            // metadata from that side so a deleted entry
                            // still carries `title`/`entity_type`.
                            let raw = read_md_at_path(&repo, &tree_a, location);
                            let (title, entity_type) = peek_meta(&raw);
                            let content_before = if config.include_content { raw } else { None };
                            entries.push(EntityDiff::Deleted {
                                id,
                                title,
                                entity_type,
                                content_before,
                                ripple: Vec::new(),
                            });
                        }
                    }
                    Change::Modification { location, .. } => {
                        if let Some(id) = path_to_entity_id(mem, location) {
                            // Post-state (`ref_b`) is the source for the
                            // current metadata, mirroring `memstead_changes_since`.
                            let raw_b = read_md_at_path(&repo, &tree_b, location);
                            let (title, entity_type) = peek_meta(&raw_b);
                            let content_before = if config.include_content {
                                read_md_at_path(&repo, &tree_a, location)
                            } else {
                                None
                            };
                            let content_after = if config.include_content { raw_b } else { None };
                            entries.push(EntityDiff::Modified {
                                id,
                                title,
                                entity_type,
                                content_before,
                                content_after,
                                ripple: Vec::new(),
                            });
                        }
                    }
                    Change::Rewrite {
                        source_location,
                        location,
                        ..
                    } => {
                        let from = path_to_entity_id(mem, source_location);
                        let to = path_to_entity_id(mem, location);
                        if let (Some(from_id), Some(to_id)) = (from, to) {
                            // Post-state (the `to` side) carries the surviving
                            // metadata.
                            let raw_b = read_md_at_path(&repo, &tree_b, location);
                            let (title, entity_type) = peek_meta(&raw_b);
                            let content_before = if config.include_content {
                                read_md_at_path(&repo, &tree_a, source_location)
                            } else {
                                None
                            };
                            let content_after = if config.include_content { raw_b } else { None };
                            entries.push(EntityDiff::Renamed {
                                from_id,
                                to_id,
                                rename_chain: Vec::new(),
                                title,
                                entity_type,
                                content_before,
                                content_after,
                                ripple: Vec::new(),
                            });
                        }
                    }
                }
                Ok(std::ops::ControlFlow::Continue(()))
            },
        )
        .map_err(|e| BackendError::Other(format!("diff: {e}")))?;

    // Agent-notes-driven rename collapse + chain trace. Walks the
    // commit history between `ref_a` and `ref_b` (when `ref_a` is an
    // ancestor of `ref_b` the walk is exact; for unrelated refs the
    // notes set may be empty / partial — gix-similarity rewrites
    // still win as a fallback). For each engine-authored rename note
    // pair `old → new`, pair surviving `Added(new) + Deleted(old)`
    // entries into `Renamed`; for any `Renamed` entry where the notes
    // record a multi-step chain, fill `rename_chain` with the
    // intermediates.
    apply_agent_notes_renames(gitdir, mem, ref_a, ref_b, &mut entries, config);

    // Schema-strictness pass: entries whose markdown body fails the
    // cheap parse check (missing / malformed frontmatter) get demoted
    // to `InvalidEntity` with the surviving bytes attached. Runs only
    // when content is included; the no-content shape carries no body
    // to evaluate. Renamed pairs are exempt — see
    // `demote_invalid_entries` for the rationale.
    if config.include_content {
        demote_invalid_entries(&mut entries);
    }

    // Stable ordering: sort by primary entity id so consumers can
    // structurally compare diff outputs across runs.
    entries.sort_by_key(primary_id);

    if config.include_ripple {
        let affected = collect_affected_ids(&entries);
        if !affected.is_empty() {
            let ripple_a = scan_tree_ripple(&repo, &tree_a, mem, &affected, "ref_a");
            let ripple_b = scan_tree_ripple(&repo, &tree_b, mem, &affected, "ref_b");
            attach_ripple(&mut entries, &ripple_a, &ripple_b);
        }
    }

    Ok(Diff {
        ref_a: ref_a.to_string(),
        ref_b: ref_b.to_string(),
        resolved_a_sha,
        resolved_b_sha,
        config: config.clone(),
        entries,
    })
}

/// Walk commit notes between `ref_a` and `ref_b`, derive the
/// engine-authored rename graph, and fold the result into `entries`:
/// `Added(new) + Deleted(old)` pairs where the notes show `old → new`
/// promote to `Renamed`; existing `Renamed` entries fill in their
/// `rename_chain` with intermediate ids when the notes record a
/// multi-step chain. No-op when the notes lookup fails or returns
/// nothing — gix-similarity stays the fallback rename signal.
fn apply_agent_notes_renames(
    gitdir: &Path,
    mem: &str,
    ref_a: &str,
    ref_b: &str,
    entries: &mut Vec<EntityDiff>,
    config: &DiffConfig,
) {
    let report = match crate::ops::agent_notes::agent_notes_since(mem, gitdir, ref_a, Some(ref_b)) {
        Ok(r) => r,
        // The notes walker may refuse with `ObjectNotFound` for
        // unrelated refs — that's expected and not fatal. The diff
        // already includes the rev_parse errors via the resolve_tree
        // call above, so a notes-walk refusal here is just "no chain
        // data" and the gix-similarity result stands.
        Err(_) => return,
    };

    // Build the per-step rename map in chronological order (notes come
    // newest-first; reverse to walk oldest → newest so multi-step
    // chains compose left-to-right).
    let mut forward: HashMap<EntityId, EntityId> = HashMap::new();
    for note in report.notes.iter().rev() {
        if note.tool_verb.as_deref() != Some("rename") {
            continue;
        }
        let Some(id_str) = note.entity_id.as_deref() else {
            continue;
        };
        let Some((old_id, new_id)) = crate::ops::changes::parse_rename_entity_field(id_str) else {
            continue;
        };
        forward.insert(old_id, new_id);
    }
    if forward.is_empty() {
        return;
    }

    // Pair Added + Deleted entries that the notes flag as a rename.
    // First pass: index entries by id so we can replace them in place.
    let mut deleted_idx: HashMap<EntityId, usize> = HashMap::new();
    let mut added_idx: HashMap<EntityId, usize> = HashMap::new();
    for (idx, entry) in entries.iter().enumerate() {
        match entry {
            EntityDiff::Deleted { id, .. } => {
                deleted_idx.insert(id.clone(), idx);
            }
            EntityDiff::Added { id, .. } => {
                added_idx.insert(id.clone(), idx);
            }
            _ => {}
        }
    }

    let mut to_remove: Vec<usize> = Vec::new();
    let mut promotions: Vec<(usize, EntityDiff)> = Vec::new();
    for origin in forward.keys() {
        let Some(&del_idx) = deleted_idx.get(origin) else {
            continue;
        };
        // Walk forward from origin to find the surviving terminal id
        // among the additions.
        let mut chain: Vec<EntityId> = Vec::new();
        let mut current = origin.clone();
        let final_id: Option<EntityId> = loop {
            let Some(next) = forward.get(&current) else {
                break None;
            };
            if added_idx.contains_key(next) {
                break Some(next.clone());
            }
            chain.push(next.clone());
            current = next.clone();
        };
        let Some(terminal) = final_id else { continue };
        let Some(&add_idx) = added_idx.get(&terminal) else {
            continue;
        };
        let (content_before, content_after) = if config.include_content {
            let cb = match &entries[del_idx] {
                EntityDiff::Deleted { content_before, .. } => content_before.clone(),
                _ => None,
            };
            let ca = match &entries[add_idx] {
                EntityDiff::Added { content_after, .. } => content_after.clone(),
                _ => None,
            };
            (cb, ca)
        } else {
            (None, None)
        };
        // Carry the post-state metadata from the surviving Added entry
        // into the promoted Renamed — the collapse must not drop the
        // `title`/`entity_type` the addition already resolved.
        let (title, entity_type) = match &entries[add_idx] {
            EntityDiff::Added {
                title, entity_type, ..
            } => (title.clone(), entity_type.clone()),
            _ => (None, None),
        };
        let promoted = EntityDiff::Renamed {
            from_id: origin.clone(),
            to_id: terminal.clone(),
            rename_chain: chain,
            title,
            entity_type,
            content_before,
            content_after,
            ripple: Vec::new(),
        };
        promotions.push((add_idx, promoted));
        to_remove.push(del_idx);
    }

    // Apply promotions (replace Added entries) and remove the paired
    // Deletions. Sort indices descending so removals don't shift the
    // others.
    for (idx, promoted) in promotions {
        entries[idx] = promoted;
    }
    to_remove.sort_by(|a, b| b.cmp(a));
    for idx in to_remove {
        entries.remove(idx);
    }

    // Fill rename_chain on entries that came in as Renamed (from gix
    // similarity) when the notes record a multi-step chain.
    for entry in entries.iter_mut() {
        if let EntityDiff::Renamed {
            from_id,
            to_id,
            rename_chain,
            ..
        } = entry
            && rename_chain.is_empty()
        {
            let mut chain: Vec<EntityId> = Vec::new();
            let mut current = from_id.clone();
            while let Some(next) = forward.get(&current) {
                if next == to_id {
                    break;
                }
                chain.push(next.clone());
                current = next.clone();
            }
            // Only attach the chain if it actually leads to `to_id` —
            // otherwise the notes describe a different rename graph
            // than the gix-similarity pairing, and falsely attributing
            // intermediates would mislead consumers.
            if forward.get(&current).is_some_and(|n| n == to_id) {
                *rename_chain = chain;
            }
        }
    }
}

/// Classify a markdown body as well-formed or parse-failing. v1
/// covers the cheapest, most common failure (missing or malformed
/// frontmatter): a body that does not start with `---\n` plus a
/// matching `\n---` line is reported as `InvalidEntity`. Deeper
/// schema-reparse validation (type-bound section / field checks) is a
/// follow-up — the surface admits future expansion without changing
/// the wire shape.
fn classify_parse_failure(content: &str) -> Option<String> {
    // The core's verdict, never a second reader of the fence: a document
    // whose first line is not an opening fence (`---` followed by a line
    // break) has no frontmatter; one that opens and never closes is
    // malformed. The same contract the loader applies.
    match memstead_base::split_frontmatter_core(content).1 {
        memstead_base::Frontmatter::Present { .. } => None,
        memstead_base::Frontmatter::NoOpeningDelimiter => {
            Some("missing frontmatter (body does not open with a `---` line)".to_string())
        }
        memstead_base::Frontmatter::Unclosed => {
            Some("malformed frontmatter (no closing `---` line)".to_string())
        }
    }
}

/// Rewrite `entries` so any entry whose surviving content fails the
/// minimum-bar parse check classifies as [`EntityDiff::InvalidEntity`]
/// instead. Preserves the surviving content on whichever side is
/// available so consumers can still surface what's there.
fn demote_invalid_entries(entries: &mut [EntityDiff]) {
    for entry in entries.iter_mut() {
        match entry {
            EntityDiff::Added {
                id, content_after, ..
            } => {
                if let Some(c) = content_after.as_ref()
                    && let Some(err) = classify_parse_failure(c)
                {
                    *entry = EntityDiff::InvalidEntity {
                        id: id.clone(),
                        side: "ref_b".to_string(),
                        error: err,
                        content_before: None,
                        content_after: content_after.clone(),
                    };
                }
            }
            EntityDiff::Deleted {
                id, content_before, ..
            } => {
                if let Some(c) = content_before.as_ref()
                    && let Some(err) = classify_parse_failure(c)
                {
                    *entry = EntityDiff::InvalidEntity {
                        id: id.clone(),
                        side: "ref_a".to_string(),
                        error: err,
                        content_before: content_before.clone(),
                        content_after: None,
                    };
                }
            }
            EntityDiff::Modified {
                id,
                content_before,
                content_after,
                ..
            } => {
                let err_a = content_before
                    .as_ref()
                    .and_then(|c| classify_parse_failure(c));
                let err_b = content_after
                    .as_ref()
                    .and_then(|c| classify_parse_failure(c));
                if err_a.is_some() || err_b.is_some() {
                    let (side, error) = match (err_a, err_b) {
                        (Some(a), Some(b)) => {
                            ("both".to_string(), format!("{a} (ref_a); {b} (ref_b)"))
                        }
                        (Some(a), None) => ("ref_a".to_string(), a),
                        (None, Some(b)) => ("ref_b".to_string(), b),
                        (None, None) => unreachable!(),
                    };
                    *entry = EntityDiff::InvalidEntity {
                        id: id.clone(),
                        side,
                        error,
                        content_before: content_before.clone(),
                        content_after: content_after.clone(),
                    };
                }
            }
            // Renamed entries skip the demote — a successful rename
            // pairing implies the engine still understood both
            // versions. InvalidEntity is reserved for the simpler
            // Added / Modified / Deleted shapes.
            EntityDiff::Renamed { .. } | EntityDiff::InvalidEntity { .. } => {}
        }
    }
}

/// Collect every entity id that an `EntityDiff` entry concerns. For
/// `Renamed` both `from_id` and `to_id` are affected so the ripple
/// scan can find inbound links to either name (the pre-rename id on
/// the `ref_a` side, the post-rename id on the `ref_b` side).
fn collect_affected_ids(entries: &[EntityDiff]) -> HashSet<EntityId> {
    let mut out = HashSet::new();
    for e in entries {
        match e {
            EntityDiff::Added { id, .. }
            | EntityDiff::Modified { id, .. }
            | EntityDiff::Deleted { id, .. }
            | EntityDiff::InvalidEntity { id, .. } => {
                out.insert(id.clone());
            }
            EntityDiff::Renamed { from_id, to_id, .. } => {
                out.insert(from_id.clone());
                out.insert(to_id.clone());
            }
        }
    }
    out
}

/// Walk a tree's `.md` blobs, scan each body for `[[…]]` wiki-links,
/// and collect every (referrer → affected target, side) triple. Used
/// twice — once per side — to produce both halves of the ripple list.
fn scan_tree_ripple(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    mem: &str,
    affected: &HashSet<EntityId>,
    side: &str,
) -> HashMap<EntityId, Vec<IncomingRipple>> {
    let mut out: HashMap<EntityId, Vec<IncomingRipple>> = HashMap::new();
    let entries = match tree.traverse().breadthfirst.files() {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries {
        if !entry.mode.is_blob() {
            continue;
        }
        let path = match std::str::from_utf8(entry.filepath.as_slice()) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if !path.ends_with(".md") || path.starts_with(".memstead/") {
            continue;
        }
        let referrer_id = file_path_to_id(path, mem);
        // Don't list an entity as its own referrer — a wiki-link in
        // the entity's own body pointing at itself is not "ripple".
        let object = match repo.find_object(entry.oid) {
            Ok(o) => o,
            Err(_) => continue,
        };
        let blob = match object.try_into_blob() {
            Ok(b) => b,
            Err(_) => continue,
        };
        let content = match std::str::from_utf8(&blob.data) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // The BODY, not the whole blob: frontmatter is not markdown,
        // and a YAML value that reads as a fence opener would mask the
        // body away and drop every link in it from the ripple list.
        for target in extract_inline_links_lenient(body_after_frontmatter(content), mem) {
            if target == referrer_id {
                continue;
            }
            if affected.contains(&target) {
                out.entry(target).or_default().push(IncomingRipple {
                    from_id: referrer_id.clone(),
                    side: side.to_string(),
                    section: None,
                });
            }
        }
    }
    out
}

/// Splice ripple lists into the entries. For each entry, the ripple
/// payload combines the pre-state (`ref_a`) and post-state (`ref_b`)
/// referrers. `Renamed` entries pull both halves: pre-state lookups
/// key on `from_id`, post-state on `to_id`.
fn attach_ripple(
    entries: &mut [EntityDiff],
    ripple_a: &HashMap<EntityId, Vec<IncomingRipple>>,
    ripple_b: &HashMap<EntityId, Vec<IncomingRipple>>,
) {
    for entry in entries.iter_mut() {
        match entry {
            EntityDiff::Added { id, ripple, .. }
            | EntityDiff::Modified { id, ripple, .. }
            | EntityDiff::Deleted { id, ripple, .. } => {
                if let Some(list) = ripple_a.get(id) {
                    ripple.extend(list.iter().cloned());
                }
                if let Some(list) = ripple_b.get(id) {
                    ripple.extend(list.iter().cloned());
                }
            }
            EntityDiff::Renamed {
                from_id,
                to_id,
                ripple,
                ..
            } => {
                if let Some(list) = ripple_a.get(from_id) {
                    ripple.extend(list.iter().cloned());
                }
                if let Some(list) = ripple_b.get(to_id) {
                    ripple.extend(list.iter().cloned());
                }
            }
            EntityDiff::InvalidEntity { .. } => {}
        }
    }
}

/// Sort key for the per-entry stable ordering. `Renamed` uses
/// `to_id` (the surviving id); `InvalidEntity` carries `id`.
fn primary_id(entry: &EntityDiff) -> String {
    match entry {
        EntityDiff::Added { id, .. }
        | EntityDiff::Modified { id, .. }
        | EntityDiff::Deleted { id, .. }
        | EntityDiff::InvalidEntity { id, .. } => id.to_string(),
        EntityDiff::Renamed { to_id, .. } => to_id.to_string(),
    }
}

#[cfg(test)]
mod tests;
