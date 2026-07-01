//! `Engine::diff(ref_a, ref_b)` implementation for git-branch mounts.
//!
//! Walks the two tree objects pointed to by `ref_a` and `ref_b` inside
//! the workspace's vault-repo gitdir, produces a per-entity diff
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
//!   the same vault's history. A generic two-ref diff (potentially
//!   cross-vault) needs a different walk; v1 emits Added/Deleted pairs
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
use memstead_base::entity::parser::{extract_inline_links_lenient, peek_title_and_type};
use memstead_base::ops::{Diff, DiffConfig, EntityDiff, IncomingRipple};

use crate::EMPTY_TREE_SHA;

/// Normalise a caller-supplied ref against the per-vault branch
/// convention, shared by `memstead_diff` and `memstead_changes_since`.
///
/// Rewrites a leading `HEAD` *token* — the whole ref (`HEAD`) or the base
/// of a revspec (`HEAD~5`, `HEAD^`, `HEAD^{tree}`, `HEAD@{1}`) — to
/// `refs/heads/<vault>`, preserving the suffix, so resolution targets the
/// selected vault's branch tip rather than the vault-repo gitdir's
/// symbolic HEAD (which points at the dummy default branch). gix already
/// parses the revspec suffix; this only re-anchors the `HEAD` base. A ref
/// that merely *starts with* `HEAD` (e.g. `HEADER`, `HEAD-foo`) is left
/// alone — the character after `HEAD` must be a revspec operator
/// (`~ ^ : @`) or end-of-string. Targeted at the per-vault entry point
/// only: vault-less callers (cross-vault diffs naming a peer branch) pass
/// fully-qualified refs that don't begin with the `HEAD` token.
///
/// The empty-tree sentinel is handled inside `resolve_tree` so callers see
/// a single dispatch.
pub(crate) fn normalise_ref_for_vault(vault: &str, raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("HEAD")
        && (rest.is_empty() || rest.starts_with(['~', '^', ':', '@']))
    {
        return format!("refs/heads/{vault}{rest}");
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
fn resolve_tree<'r>(
    repo: &'r gix::Repository,
    raw: &str,
) -> Result<gix::Tree<'r>, BackendError> {
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

/// Translate a tree-diff path to a vault-qualified entity id, returning
/// `None` for engine-internal paths (`.memstead/...`) and non-markdown
/// entries. Mirrors `changes::path_to_entity_id` but is callable from
/// here without exporting the private helper.
fn path_to_entity_id(vault: &str, path: &gix::bstr::BStr) -> Option<EntityId> {
    let s = std::str::from_utf8(path.as_ref()).ok()?;
    if s.is_empty() || !s.ends_with(".md") {
        return None;
    }
    if s.starts_with(".memstead/") {
        return None;
    }
    Some(file_path_to_id(s, vault))
}

/// Read the markdown body for `path` from `tree`. Returns `None` when
/// the path is not present in the tree, or when the lookup fails.
fn read_md_at_path(
    _repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    path: &gix::bstr::BStr,
) -> Option<String> {
    let entry = tree.lookup_entry_by_path(path.to_string().as_str()).ok()??;
    let object = entry.object().ok()?;
    let blob = object.try_into_blob().ok()?;
    String::from_utf8(blob.data.clone()).ok()
}

/// Extract `(title, entity_type)` from an optionally-present markdown
/// blob. `(None, None)` when the blob is absent / non-UTF-8 or carries
/// neither a `# ` heading nor a `type:` frontmatter field.
fn peek_meta(raw: &Option<String>) -> (Option<String>, Option<String>) {
    raw.as_deref().map(peek_title_and_type).unwrap_or((None, None))
}

/// Two-ref structural diff. See module docs for the v1 scope and the
/// known gaps (rename, ripple) that will surface as handover items.
pub fn diff_two_refs(
    gitdir: &Path,
    vault: &str,
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
    // Bare `HEAD` substitutes to `refs/heads/<vault>` so the diff targets
    // the vault's branch tip (not the gitdir's symbolic HEAD on a
    // dummy default branch). Per-vault callers always pass a vault
    // selector; the substitution is unconditional on this entry
    // point.
    let normalised_a = normalise_ref_for_vault(vault, ref_a);
    let normalised_b = normalise_ref_for_vault(vault, ref_b);
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
                        if let Some(id) = path_to_entity_id(vault, location.as_ref()) {
                            // Read the post-state blob unconditionally to
                            // populate `title`/`entity_type` (the docstring's
                            // metadata-only shape); keep the full body as
                            // `content_after` only when content is requested.
                            let raw = read_md_at_path(&repo, &tree_b, location.as_ref());
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
                        if let Some(id) = path_to_entity_id(vault, location.as_ref()) {
                            // The entity still exists on `ref_a`; pull its
                            // metadata from that side so a deleted entry
                            // still carries `title`/`entity_type`.
                            let raw = read_md_at_path(&repo, &tree_a, location.as_ref());
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
                        if let Some(id) = path_to_entity_id(vault, location.as_ref()) {
                            // Post-state (`ref_b`) is the source for the
                            // current metadata, mirroring `memstead_changes_since`.
                            let raw_b = read_md_at_path(&repo, &tree_b, location.as_ref());
                            let (title, entity_type) = peek_meta(&raw_b);
                            let content_before = if config.include_content {
                                read_md_at_path(&repo, &tree_a, location.as_ref())
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
                        let from = path_to_entity_id(vault, source_location.as_ref());
                        let to = path_to_entity_id(vault, location.as_ref());
                        if let (Some(from_id), Some(to_id)) = (from, to) {
                            // Post-state (the `to` side) carries the surviving
                            // metadata.
                            let raw_b = read_md_at_path(&repo, &tree_b, location.as_ref());
                            let (title, entity_type) = peek_meta(&raw_b);
                            let content_before = if config.include_content {
                                read_md_at_path(&repo, &tree_a, source_location.as_ref())
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
    apply_agent_notes_renames(gitdir, vault, ref_a, ref_b, &mut entries, config);

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
    entries.sort_by(|a, b| primary_id(a).cmp(&primary_id(b)));

    if config.include_ripple {
        let affected = collect_affected_ids(&entries);
        if !affected.is_empty() {
            let ripple_a = scan_tree_ripple(&repo, &tree_a, vault, &affected, "ref_a");
            let ripple_b = scan_tree_ripple(&repo, &tree_b, vault, &affected, "ref_b");
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
    vault: &str,
    ref_a: &str,
    ref_b: &str,
    entries: &mut Vec<EntityDiff>,
    config: &DiffConfig,
) {
    let report = match crate::ops::agent_notes::agent_notes_since(
        vault,
        gitdir,
        ref_a,
        Some(ref_b),
    ) {
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
        let Some((old_id, new_id)) = crate::ops::changes::parse_rename_entity_field(id_str)
        else {
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
    for (origin, _terminal) in forward.iter() {
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
            EntityDiff::Added { title, entity_type, .. } => {
                (title.clone(), entity_type.clone())
            }
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
    let trimmed_bom = content.trim_start_matches('\u{FEFF}');
    if !trimmed_bom.starts_with("---") {
        return Some("missing frontmatter (body does not open with `---`)".to_string());
    }
    let after_open = match trimmed_bom.strip_prefix("---") {
        Some(rest) => rest.trim_start_matches('\r').trim_start_matches('\n'),
        None => return Some("missing frontmatter".to_string()),
    };
    // Look for a line that is just "---" closing the frontmatter.
    let mut closed = false;
    for line in after_open.lines() {
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
    }
    if !closed {
        return Some("malformed frontmatter (no closing `---` line)".to_string());
    }
    None
}

/// Rewrite `entries` so any entry whose surviving content fails the
/// minimum-bar parse check classifies as [`EntityDiff::InvalidEntity`]
/// instead. Preserves the surviving content on whichever side is
/// available so consumers can still surface what's there.
fn demote_invalid_entries(entries: &mut Vec<EntityDiff>) {
    for entry in entries.iter_mut() {
        match entry {
            EntityDiff::Added { id, content_after, .. } => {
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
            EntityDiff::Deleted { id, content_before, .. } => {
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
                        (Some(a), Some(b)) => ("both".to_string(), format!("{a} (ref_a); {b} (ref_b)")),
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
    vault: &str,
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
        let referrer_id = file_path_to_id(path, vault);
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
        for target in extract_inline_links_lenient(content, vault) {
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
mod tests {
    use super::*;
    use crate::storage::VaultWriter;
    use crate::storage::git_tree::GitTreeVaultWriter;
    use crate::vcs::CommitContext;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn init_gitdir(tmp: &TempDir) -> PathBuf {
        let gitdir = tmp.path().join("vault-repo").join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        gix::init_bare(&gitdir).unwrap();
        gitdir
    }

    fn body_with_title(title: &str) -> String {
        // Per-title unique padding so gix's similarity-driven rename
        // detection (50% default) does not pair unrelated test
        // entities as a single Rewrite event. Plain `# {title}` bodies
        // were too similar across entities; the repeated title token
        // here pushes each body's hash far enough apart that gix sees
        // distinct adds and deletes.
        let unique = title.repeat(64);
        format!(
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# {title}\n\n## Identity\n\n{unique}\n"
        )
    }

    fn write_and_commit(
        gitdir: &PathBuf,
        vault: &str,
        entries: &[(&str, &str)],
        subject: &str,
    ) -> String {
        let writer = GitTreeVaultWriter::new(gitdir.clone(), format!("refs/heads/{vault}"));
        for (path, content) in entries {
            writer.write_entity(Path::new(path), content.as_bytes()).unwrap();
        }
        writer.commit(subject, &CommitContext::internal()).unwrap()
    }

    #[test]
    fn diff_unknown_ref_returns_unknown_ref_marker() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let err = diff_two_refs(
            &gitdir,
            "specs",
            "no-such-ref",
            "no-such-other",
            &DiffConfig::default(),
        )
        .unwrap_err();
        match err {
            BackendError::Other(msg) => {
                assert!(
                    msg.starts_with("UNKNOWN_REF:"),
                    "expected UNKNOWN_REF marker, got: {msg}",
                );
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn normalise_rewrites_only_the_head_token() {
        // #53: the HEAD base of a revspec re-anchors on the vault branch,
        // suffix preserved.
        assert_eq!(normalise_ref_for_vault("v", "HEAD"), "refs/heads/v");
        assert_eq!(normalise_ref_for_vault("v", "HEAD~5"), "refs/heads/v~5");
        assert_eq!(normalise_ref_for_vault("v", "HEAD^"), "refs/heads/v^");
        assert_eq!(normalise_ref_for_vault("v", "HEAD^{tree}"), "refs/heads/v^{tree}");
        assert_eq!(normalise_ref_for_vault("v", "HEAD@{1}"), "refs/heads/v@{1}");
        // Refusal: a ref that merely starts with "HEAD" is left alone.
        assert_eq!(normalise_ref_for_vault("v", "HEADER"), "HEADER");
        assert_eq!(normalise_ref_for_vault("v", "HEAD-foo"), "HEAD-foo");
        // Refusal: a plain branch / SHA passes through unchanged.
        assert_eq!(normalise_ref_for_vault("v", "main"), "main");
        assert_eq!(normalise_ref_for_vault("v", "deadbeef"), "deadbeef");
    }

    #[test]
    fn diff_head_revspec_anchors_on_vault_branch() {
        // #53: `HEAD~1` / `HEAD` resolve against `refs/heads/<vault>`, not
        // the gitdir's symbolic HEAD (the dummy default branch, which has no
        // commits here — before the fix these revspecs would refuse).
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        write_and_commit(
            &gitdir,
            "specs",
            &[("alpha.md", &body_with_title("Alpha"))],
            "c1",
        );
        write_and_commit(
            &gitdir,
            "specs",
            &[("beta.md", &body_with_title("Beta"))],
            "c2 add beta",
        );

        let diff = diff_two_refs(&gitdir, "specs", "HEAD~1", "HEAD", &DiffConfig::default())
            .expect("HEAD revspec must resolve against the vault branch");
        assert_eq!(diff.entries.len(), 1, "only beta added between c1 and c2");
        assert!(
            matches!(diff.entries[0], EntityDiff::Added { .. }),
            "the one change is beta added: {:?}",
            diff.entries[0]
        );
    }

    #[test]
    fn diff_added_modified_deleted_surface() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);

        // Ref A: alpha + beta with body B0.
        write_and_commit(
            &gitdir,
            "specs",
            &[
                ("alpha.md", &body_with_title("Alpha")),
                ("beta.md", &body_with_title("Beta-v0")),
            ],
            "seed",
        );
        let sha_a = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();

        // Ref B: alpha unchanged, beta body changed, gamma added.
        write_and_commit(
            &gitdir,
            "specs",
            &[
                ("beta.md", &body_with_title("Beta-v1")),
                ("gamma.md", &body_with_title("Gamma")),
            ],
            "update beta + add gamma",
        );
        // Drop alpha in a third commit so it surfaces as a deletion.
        let writer =
            GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/specs".to_string());
        writer.delete_entity(Path::new("alpha.md")).unwrap();
        writer.commit("drop alpha", &CommitContext::internal()).unwrap();

        let diff = diff_two_refs(
            &gitdir,
            "specs",
            &sha_a,
            "refs/heads/specs",
            &DiffConfig::default(),
        )
        .unwrap();
        assert_eq!(diff.entries.len(), 3, "Add+Modify+Delete expected");
        let statuses: Vec<&str> = diff
            .entries
            .iter()
            .map(|e| match e {
                EntityDiff::Added { .. } => "added",
                EntityDiff::Modified { .. } => "modified",
                EntityDiff::Deleted { .. } => "deleted",
                EntityDiff::Renamed { .. } => "renamed",
                EntityDiff::InvalidEntity { .. } => "invalid",
            })
            .collect();
        // Sorted by primary id: alpha (deleted), beta (modified), gamma (added).
        assert_eq!(statuses, vec!["deleted", "modified", "added"]);

        // Content enrichment defaults to on: both sides populated for
        // the modified entry; one-sided for added/deleted.
        let beta = diff
            .entries
            .iter()
            .find(|e| matches!(e, EntityDiff::Modified { .. }))
            .unwrap();
        match beta {
            EntityDiff::Modified {
                content_before,
                content_after,
                ..
            } => {
                assert!(content_before.as_ref().unwrap().contains("Beta-v0"));
                assert!(content_after.as_ref().unwrap().contains("Beta-v1"));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn diff_include_content_false_strips_bodies() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        write_and_commit(
            &gitdir,
            "specs",
            &[("alpha.md", &body_with_title("Alpha"))],
            "seed",
        );
        let sha_seed = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();
        write_and_commit(
            &gitdir,
            "specs",
            &[("alpha.md", &body_with_title("Alpha-v2"))],
            "rev2",
        );

        let cfg = DiffConfig {
            include_content: false,
            ..DiffConfig::default()
        };
        let diff = diff_two_refs(&gitdir, "specs", &sha_seed, "refs/heads/specs", &cfg).unwrap();
        assert_eq!(diff.entries.len(), 1);
        match &diff.entries[0] {
            EntityDiff::Modified {
                title,
                entity_type,
                content_before,
                content_after,
                ..
            } => {
                assert!(content_before.is_none(), "include_content=false elides before");
                assert!(content_after.is_none(), "include_content=false elides after");
                // Metadata is present regardless of the content toggle —
                // the docstring's metadata-only shape promises it and a
                // JSON consumer needs `title`/`entity_type` without
                // parsing bodies. Sourced from the post-state (`ref_b`).
                assert_eq!(
                    title.as_deref(),
                    Some("Alpha-v2"),
                    "title present (from ref_b) even with include_content=false"
                );
                assert_eq!(
                    entity_type.as_deref(),
                    Some("spec"),
                    "entity_type present even with include_content=false"
                );
            }
            other => panic!("expected Modified, got {other:?}"),
        }
    }

    /// With `include_content: true` the metadata fields are additive to
    /// the body fields — `{id, title, entity_type, status}` plus
    /// `content_before`/`content_after`, not a replacement. Also pins
    /// the added-entry shape (post-state metadata from `ref_b`).
    #[test]
    fn diff_populates_title_and_type_with_content_on() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let sha_empty = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"; // empty tree
        write_and_commit(
            &gitdir,
            "specs",
            &[("alpha.md", &body_with_title("Alpha"))],
            "seed",
        );

        let cfg = DiffConfig {
            include_content: true,
            ..DiffConfig::default()
        };
        let diff = diff_two_refs(&gitdir, "specs", sha_empty, "refs/heads/specs", &cfg).unwrap();
        assert_eq!(diff.entries.len(), 1);
        match &diff.entries[0] {
            EntityDiff::Added {
                title,
                entity_type,
                content_after,
                ..
            } => {
                assert_eq!(title.as_deref(), Some("Alpha"), "title populated on Added");
                assert_eq!(entity_type.as_deref(), Some("spec"), "entity_type populated");
                assert!(
                    content_after.is_some(),
                    "content_after present and additive to the metadata fields"
                );
            }
            other => panic!("expected Added, got {other:?}"),
        }
    }

    #[test]
    fn diff_rename_chain_collapses_multi_step_engine_authored_renames() {
        // Seed alpha, rename to beta via an engine-style commit, then
        // rename beta to gamma. Diffing seed → head should surface a
        // single `Renamed { from: alpha, to: gamma, chain: [beta] }`
        // rather than a chain of intermediate edits.
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);

        let body = body_with_title("Title");
        let writer =
            GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/specs".to_string());
        writer.write_entity(Path::new("alpha.md"), body.as_bytes()).unwrap();
        writer.commit("seed", &CommitContext::internal()).unwrap();
        let sha_seed =
            sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();

        // alpha → beta. Move the file (delete + write) and emit the
        // commit subject the engine uses on its rename pipeline.
        let writer =
            GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/specs".to_string());
        writer.delete_entity(Path::new("alpha.md")).unwrap();
        writer.write_entity(Path::new("beta.md"), body.as_bytes()).unwrap();
        writer
            .commit("memstead: rename specs--alpha → specs--beta", &CommitContext::internal())
            .unwrap();

        // beta → gamma. Same shape.
        let writer =
            GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/specs".to_string());
        writer.delete_entity(Path::new("beta.md")).unwrap();
        writer.write_entity(Path::new("gamma.md"), body.as_bytes()).unwrap();
        writer
            .commit("memstead: rename specs--beta → specs--gamma", &CommitContext::internal())
            .unwrap();

        let diff = diff_two_refs(
            &gitdir,
            "specs",
            &sha_seed,
            "refs/heads/specs",
            &DiffConfig::default(),
        )
        .unwrap();

        let renamed = diff
            .entries
            .iter()
            .find(|e| matches!(e, EntityDiff::Renamed { .. }))
            .expect("a Renamed entry must surface");
        match renamed {
            EntityDiff::Renamed {
                from_id,
                to_id,
                rename_chain,
                ..
            } => {
                assert_eq!(from_id.to_string(), "specs--alpha");
                assert_eq!(to_id.to_string(), "specs--gamma");
                assert_eq!(
                    rename_chain
                        .iter()
                        .map(|id| id.to_string())
                        .collect::<Vec<_>>(),
                    vec!["specs--beta".to_string()],
                    "the multi-step rename's intermediate id must surface in rename_chain",
                );
            }
            _ => unreachable!(),
        }
        // No leftover Added/Deleted entries for the chain endpoints.
        let leftover: Vec<_> = diff
            .entries
            .iter()
            .filter(|e| matches!(e, EntityDiff::Added { .. } | EntityDiff::Deleted { .. }))
            .collect();
        assert!(
            leftover.is_empty(),
            "agent-notes promotion must absorb the Added+Deleted pair, got: {leftover:?}",
        );
    }

    #[test]
    fn diff_ripple_lists_incoming_wikilinks_on_each_side() {
        // Build a vault with three entities: alpha, beta, gamma.
        // beta links to alpha on ref_a; gamma links to alpha on ref_b.
        // Modify alpha between the two refs. The diff entry for
        // alpha should surface both referrers in its ripple list,
        // each tagged with the right side.
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);

        let alpha_v1 = body_with_title("Alpha-v1");
        let beta_links_alpha = format!(
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Beta\n\n## Identity\n\nLinks to [[specs--alpha]].\n"
        );
        write_and_commit(
            &gitdir,
            "specs",
            &[
                ("alpha.md", &alpha_v1),
                ("beta.md", &beta_links_alpha),
            ],
            "seed",
        );
        let sha_a = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();

        let alpha_v2 = body_with_title("Alpha-v2");
        let gamma_links_alpha = format!(
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Gamma\n\n## Identity\n\nLinks to [[specs--alpha]].\n"
        );
        // Drop beta to break its outbound link on ref_b. Add gamma
        // with a fresh inbound link to alpha.
        let writer =
            GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/specs".to_string());
        writer.write_entity(Path::new("alpha.md"), alpha_v2.as_bytes()).unwrap();
        writer
            .write_entity(Path::new("gamma.md"), gamma_links_alpha.as_bytes())
            .unwrap();
        writer.delete_entity(Path::new("beta.md")).unwrap();
        writer.commit("rev2", &CommitContext::internal()).unwrap();

        let diff = diff_two_refs(
            &gitdir,
            "specs",
            &sha_a,
            "refs/heads/specs",
            &DiffConfig::default(),
        )
        .unwrap();

        // Find the entry for alpha; both ripple sides must surface.
        let alpha = diff
            .entries
            .iter()
            .find(|e| matches!(e, EntityDiff::Modified { id, .. } if id.to_string() == "specs--alpha"))
            .expect("alpha must show as modified");
        match alpha {
            EntityDiff::Modified { ripple, .. } => {
                let mut sides_seen: Vec<String> = ripple
                    .iter()
                    .map(|r| format!("{}@{}", r.from_id, r.side))
                    .collect();
                sides_seen.sort();
                assert_eq!(
                    sides_seen,
                    vec![
                        "specs--beta@ref_a".to_string(),
                        "specs--gamma@ref_b".to_string(),
                    ],
                    "ripple must list beta on ref_a and gamma on ref_b",
                );
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn diff_include_ripple_false_leaves_ripple_empty() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let beta_links_alpha = "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Beta\n\n## Identity\n\n[[specs--alpha]]\n";
        write_and_commit(
            &gitdir,
            "specs",
            &[
                ("alpha.md", &body_with_title("Alpha-v1")),
                ("beta.md", beta_links_alpha),
            ],
            "seed",
        );
        let sha_a = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();
        write_and_commit(
            &gitdir,
            "specs",
            &[("alpha.md", &body_with_title("Alpha-v2"))],
            "rev2",
        );

        let cfg = DiffConfig {
            include_ripple: false,
            ..DiffConfig::default()
        };
        let diff = diff_two_refs(&gitdir, "specs", &sha_a, "refs/heads/specs", &cfg).unwrap();
        for entry in &diff.entries {
            let ripple = match entry {
                EntityDiff::Added { ripple, .. }
                | EntityDiff::Modified { ripple, .. }
                | EntityDiff::Deleted { ripple, .. }
                | EntityDiff::Renamed { ripple, .. } => ripple.clone(),
                EntityDiff::InvalidEntity { .. } => Vec::new(),
            };
            assert!(
                ripple.is_empty(),
                "include_ripple=false must produce empty ripple lists: {entry:?}",
            );
        }
    }

    #[test]
    fn diff_invalid_entity_surfaces_for_missing_frontmatter() {
        // An entity whose markdown body has no frontmatter block
        // demotes to `InvalidEntity` instead of `Modified` / `Added`.
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        write_and_commit(
            &gitdir,
            "specs",
            &[("alpha.md", &body_with_title("Alpha-v1"))],
            "seed",
        );
        let sha_a = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();
        // Overwrite alpha with a body that has no frontmatter.
        let writer =
            GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/specs".to_string());
        writer
            .write_entity(Path::new("alpha.md"), b"# Alpha but no frontmatter\n\nbody.\n")
            .unwrap();
        writer.commit("break", &CommitContext::internal()).unwrap();

        let diff = diff_two_refs(
            &gitdir,
            "specs",
            &sha_a,
            "refs/heads/specs",
            &DiffConfig::default(),
        )
        .unwrap();
        let alpha = diff.entries.first().expect("alpha should appear");
        match alpha {
            EntityDiff::InvalidEntity { id, side, error, .. } => {
                assert_eq!(id.to_string(), "specs--alpha");
                assert_eq!(side, "ref_b");
                assert!(error.contains("frontmatter"), "unexpected error: {error}");
            }
            other => panic!("expected InvalidEntity, got {other:?}"),
        }
    }

    #[test]
    fn engine_diff_routes_git_branch_mount_through_hook() {
        // End-to-end: build an engine with a git-branch mount that
        // points at our seeded gitdir, install the pro ops bundle so
        // the engine's `diff` dispatcher reaches our `diff_two_refs`
        // implementation, and assert the returned `Diff` is the same
        // one calling the function directly would produce.
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        write_and_commit(
            &gitdir,
            "specs",
            &[("alpha.md", &body_with_title("Alpha"))],
            "seed",
        );
        let sha_seed =
            sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();
        write_and_commit(
            &gitdir,
            "specs",
            &[("alpha.md", &body_with_title("Alpha-v2"))],
            "rev2",
        );

        let mount = memstead_base::Mount {
            vault: "specs".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "default",
                semver::Version::new(1, 0, 0),
            )),
            storage: memstead_base::MountStorage::GitBranch {
                gitdir: gitdir.clone(),
                branch: "specs".to_string(),
            },
            capability: memstead_base::MountCapability::Write,
            lifecycle: memstead_base::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let backend = crate::storage::instantiate_pro_backend(&mount).unwrap();
        let mut engine =
            memstead_base::Engine::from_mounts(vec![(mount, backend)]).unwrap();
        engine.set_git_branch_ops(crate::storage::PRO_GIT_BRANCH_OPS);

        let diff = engine
            .diff(
                "specs",
                &sha_seed,
                "refs/heads/specs",
                None,
            )
            .unwrap();
        assert_eq!(diff.entries.len(), 1);
        assert!(matches!(diff.entries[0], EntityDiff::Modified { .. }));
        assert_eq!(diff.resolved_a_sha, sha_seed);
    }

    #[test]
    fn engine_diff_unknown_ref_surfaces_typed_engine_error() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        write_and_commit(
            &gitdir,
            "specs",
            &[("alpha.md", &body_with_title("Alpha"))],
            "seed",
        );

        let mount = memstead_base::Mount {
            vault: "specs".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "default",
                semver::Version::new(1, 0, 0),
            )),
            storage: memstead_base::MountStorage::GitBranch {
                gitdir: gitdir.clone(),
                branch: "specs".to_string(),
            },
            capability: memstead_base::MountCapability::Write,
            lifecycle: memstead_base::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let backend = crate::storage::instantiate_pro_backend(&mount).unwrap();
        let mut engine =
            memstead_base::Engine::from_mounts(vec![(mount, backend)]).unwrap();
        engine.set_git_branch_ops(crate::storage::PRO_GIT_BRANCH_OPS);

        let err = engine
            .diff("specs", "nope-a", "nope-b", None)
            .unwrap_err();
        match err {
            memstead_base::EngineError::UnknownRef(raw) => {
                assert!(raw.contains("nope"), "unexpected UnknownRef payload: {raw}");
            }
            other => panic!("expected UnknownRef, got {other:?}"),
        }
    }

    #[test]
    fn diff_resolves_refs_to_sha_in_response() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        write_and_commit(
            &gitdir,
            "specs",
            &[("alpha.md", &body_with_title("Alpha"))],
            "seed",
        );
        let sha = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();

        let diff = diff_two_refs(
            &gitdir,
            "specs",
            "refs/heads/specs",
            "refs/heads/specs",
            &DiffConfig::default(),
        )
        .unwrap();
        assert_eq!(diff.resolved_a_sha, sha);
        assert_eq!(diff.resolved_b_sha, sha);
        // Same ref vs. same ref → no entries.
        assert!(diff.entries.is_empty());
    }

    /// The canonical empty-tree SHA is accepted as
    /// `ref_a` and short-circuits to the empty tree, so a first-sync
    /// diff lists every entity in the vault as `added`.
    /// `resolved_a_sha` echoes the sentinel verbatim.
    #[test]
    fn diff_accepts_empty_tree_sentinel_for_ref_a() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        write_and_commit(
            &gitdir,
            "specs",
            &[
                ("alpha.md", &body_with_title("Alpha")),
                ("beta.md", &body_with_title("Beta")),
            ],
            "seed",
        );

        let diff = diff_two_refs(
            &gitdir,
            "specs",
            EMPTY_TREE_SHA,
            "refs/heads/specs",
            &DiffConfig::default(),
        )
        .expect("empty-tree sentinel must be accepted");
        assert_eq!(diff.resolved_a_sha, EMPTY_TREE_SHA);
        assert_eq!(diff.entries.len(), 2, "first-sync lists every entity");
        for entry in &diff.entries {
            assert!(
                matches!(entry, EntityDiff::Added { .. }),
                "first-sync entries must all be `added`, got: {entry:?}",
            );
        }
    }

    /// A real tree-only SHA that
    /// is NOT the canonical empty-tree sentinel continues to refuse
    /// with `UNKNOWN_REF`. The sentinel handling is keyed on the
    /// literal hash, not on "is it a tree".
    #[test]
    fn diff_refuses_arbitrary_tree_sha_with_unknown_ref() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        write_and_commit(
            &gitdir,
            "specs",
            &[("alpha.md", &body_with_title("Alpha"))],
            "seed",
        );
        // Find a real tree SHA (the seed commit's tree) — that SHA
        // is a tree, not a commit, and isn't the canonical empty
        // tree, so resolve_tree's "is it a commit" gate refuses it.
        let repo = gix::open(&gitdir).unwrap();
        let head = repo.rev_parse_single("refs/heads/specs").unwrap();
        let head_commit = head.object().unwrap().try_into_commit().unwrap();
        let tree_sha = head_commit.tree().unwrap().id.to_string();
        assert_ne!(
            tree_sha, EMPTY_TREE_SHA,
            "tree SHA must differ from sentinel for this test to be meaningful"
        );

        let err = diff_two_refs(
            &gitdir,
            "specs",
            &tree_sha,
            "refs/heads/specs",
            &DiffConfig::default(),
        )
        .unwrap_err();
        match err {
            BackendError::Other(msg) => assert!(
                msg.starts_with("UNKNOWN_REF:"),
                "expected UNKNOWN_REF marker, got: {msg}",
            ),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    /// Bare `HEAD` substitutes to `refs/heads/<vault>`
    /// so the diff targets the vault's branch tip, not the gitdir's
    /// symbolic HEAD on a dummy default branch. Compares behaviour
    /// against the explicit `refs/heads/<vault>` form — both calls
    /// produce the same `resolved_b_sha` and same entries.
    #[test]
    fn diff_resolves_bare_head_to_vault_branch_tip() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        // Seed a commit on the vault branch.
        write_and_commit(
            &gitdir,
            "specs",
            &[("alpha.md", &body_with_title("Alpha"))],
            "seed",
        );
        let sha_a = sha_for(&gix::open(&gitdir).unwrap(), "refs/heads/specs").unwrap();

        // Advance the vault branch.
        write_and_commit(
            &gitdir,
            "specs",
            &[("beta.md", &body_with_title("Beta"))],
            "add beta",
        );

        // The gitdir's symbolic HEAD points at `refs/heads/main` (an
        // unrelated default branch with no commits) by default —
        // resolving bare HEAD literally would either refuse or hit
        // the wrong branch. The substitution targets
        // `refs/heads/<vault>` per the vault selector.
        let via_head = diff_two_refs(
            &gitdir,
            "specs",
            &sha_a,
            "HEAD",
            &DiffConfig::default(),
        )
        .expect("bare HEAD must resolve via vault substitution");
        let via_explicit = diff_two_refs(
            &gitdir,
            "specs",
            &sha_a,
            "refs/heads/specs",
            &DiffConfig::default(),
        )
        .expect("explicit refs/heads/<vault> still works");

        // Both calls land on the same commit and produce the same
        // entry set — the bare-HEAD substitution is structurally
        // equivalent to the explicit form.
        assert_eq!(via_head.resolved_b_sha, via_explicit.resolved_b_sha);
        assert_eq!(via_head.entries.len(), via_explicit.entries.len());
    }

    /// First-sync diff against
    /// the vault-branch HEAD using only the canonical sentinel and
    /// bare `HEAD`. Collapses to one call (no need to look up the
    /// explicit ref names).
    #[test]
    fn diff_empty_tree_sentinel_and_bare_head_compose() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        write_and_commit(
            &gitdir,
            "specs",
            &[
                ("alpha.md", &body_with_title("Alpha")),
                ("beta.md", &body_with_title("Beta")),
            ],
            "seed",
        );
        let diff = diff_two_refs(
            &gitdir,
            "specs",
            EMPTY_TREE_SHA,
            "HEAD",
            &DiffConfig::default(),
        )
        .expect("sentinel + HEAD must compose");
        assert_eq!(diff.resolved_a_sha, EMPTY_TREE_SHA);
        assert_eq!(diff.entries.len(), 2);
        assert!(
            diff.entries
                .iter()
                .all(|e| matches!(e, EntityDiff::Added { .. })),
            "every entity surfaces as added on first-sync diff"
        );
    }
}
