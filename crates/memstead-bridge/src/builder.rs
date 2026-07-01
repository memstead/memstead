//! Build [`CommitEnvelope`]s from engine state.
//!
//! Three entry points:
//! - [`build_commit_envelope`] — one envelope for one commit SHA.
//! - [`build_commit_envelopes`] — range `(since, until]` with
//!   delta-size refusal.
//! - [`build_snapshot`] — wraps `Engine::export_vault_to_bytes`
//!   into a `[SnapshotOutput]` carrying both archive bytes and the
//!   resolved HEAD SHA, ready for an HTTP response.
//!
//! All three are read-only — they touch the engine's read surface
//! only. The read-only-by-construction invariant is pinned by the
//! `no_mutating_engine_call_in_bridge_surface` source-scan test.

use std::collections::BTreeMap;
use std::path::Path;

use gix::object::tree::diff::Change;

use crate::error::BridgeError;
use crate::wire::{CommitEnvelope, EntityChange, SearchHit, SearchQuery, SearchResult};

/// Default ceiling for `n_commits` in a range request. Spec threshold
/// for range requests: re-snapshot instead of paginating when the
/// threshold is exceeded.
pub const DEFAULT_DELTA_LIMIT: u32 = 50;

/// Default page size for `/search` when the request omits `limit`.
pub const DEFAULT_SEARCH_LIMIT: usize = 20;

/// Hard ceiling on `/search` `limit` values. Requests above this
/// refuse with `INVALID_SEARCH_QUERY` — keeps a single expensive
/// query from monopolising the engine lock.
pub const DEFAULT_SEARCH_MAX_LIMIT: usize = 100;

/// Caller-tunable build config. Lives on the embedder's app state so
/// the per-request handlers can read it through `axum::extract::State`.
#[derive(Debug, Clone)]
pub struct BuildConfig {
    /// Maximum number of commits a single `/commits?since=...` range
    /// may return. Range builds that would exceed this refuse with
    /// [`BridgeError::DeltaTooLarge`].
    pub delta_limit: u32,
    /// Default page size for `/search` when the request omits
    /// `limit`. Default: [`DEFAULT_SEARCH_LIMIT`].
    pub search_default_limit: usize,
    /// Hard ceiling on `/search` `limit`. Requests above this
    /// refuse with `INVALID_SEARCH_QUERY`. Default:
    /// [`DEFAULT_SEARCH_MAX_LIMIT`].
    pub search_max_limit: usize,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            delta_limit: DEFAULT_DELTA_LIMIT,
            search_default_limit: DEFAULT_SEARCH_LIMIT,
            search_max_limit: DEFAULT_SEARCH_MAX_LIMIT,
        }
    }
}

/// Output of [`build_snapshot`] — both the archive bytes ready to
/// send and the HEAD SHA the snapshot is anchored at.
#[derive(Debug, Clone)]
pub struct SnapshotOutput {
    /// Vault name (echoes the caller's argument).
    pub vault: String,
    /// Archive bytes — `application/zip` content for an HTTP body.
    pub bytes: Vec<u8>,
    /// HEAD SHA of the vault's branch at the time the snapshot was
    /// built. Clients persist this as their last-known-HEAD cursor
    /// for follow-up `/commits?since=<sha>` requests.
    pub head: String,
}

/// Build a single commit's wire envelope. Reads commit metadata via
/// gix, walks the parent-vs-commit tree diff, and pulls full markdown
/// content per touched `.md` blob from the commit's tree.
///
/// `vault` is the vault label baked into the response and the path
/// filter for the tree diff — only changes under the vault's path
/// surface (`.md` files outside any `.memstead/` engine-internal
/// subtree).
pub fn build_commit_envelope(
    engine: &memstead_base::Engine,
    vault: &str,
    sha: &str,
) -> Result<CommitEnvelope, BridgeError> {
    let gitdir = resolve_vault_gitdir(engine, vault)?;
    let repo = gix::open(&gitdir).map_err(|e| BridgeError::Git(format!("gix open: {e}")))?;
    build_envelope_from_repo(&repo, vault, sha)
}

/// Build the sequence of envelopes for `(since, until]`. `since` may
/// be the empty-tree SHA (`4b825dc6...`) for a from-the-start walk;
/// `until` defaults to the vault branch tip when empty.
pub fn build_commit_envelopes(
    engine: &memstead_base::Engine,
    vault: &str,
    since: &str,
    until: Option<&str>,
    config: &BuildConfig,
) -> Result<Vec<CommitEnvelope>, BridgeError> {
    let gitdir = resolve_vault_gitdir(engine, vault)?;
    let repo = gix::open(&gitdir).map_err(|e| BridgeError::Git(format!("gix open: {e}")))?;

    let until_sha = match until {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => format!("refs/heads/{vault}"),
    };
    let head_id = repo
        .rev_parse_single(until_sha.as_str())
        .map_err(|_| BridgeError::UnknownCommit(until_sha.clone()))?
        .detach();

    let hidden: Vec<gix::ObjectId> = if since.is_empty()
        || since == memstead_base::ops::EMPTY_TREE_SHA
    {
        Vec::new()
    } else {
        let id = repo
            .rev_parse_single(since)
            .map_err(|_| BridgeError::UnknownCommit(since.to_string()))?
            .detach();
        vec![id]
    };

    let walk = repo
        .rev_walk([head_id])
        .with_hidden(hidden)
        .all()
        .map_err(|e| BridgeError::Git(format!("rev-walk: {e}")))?;

    // First pass: collect commit ids and count. The order is newest
    // → oldest from `rev_walk`; we reverse for chronological output
    // (oldest → newest) so consumers can apply commits in order.
    let mut ids: Vec<gix::ObjectId> = Vec::new();
    for info in walk {
        let info = info.map_err(|e| BridgeError::Git(format!("rev-walk-step: {e}")))?;
        ids.push(info.id);
        if ids.len() as u32 > config.delta_limit {
            // Continue counting for the typed error's `n_commits`
            // payload — clients use the count to decide whether to
            // re-snapshot or split the range. Stop walking the
            // remainder past 2× the limit to avoid burning time on
            // pathological histories.
            if ids.len() as u32 >= config.delta_limit * 2 {
                break;
            }
        }
    }

    if ids.len() as u32 > config.delta_limit {
        return Err(BridgeError::DeltaTooLarge {
            n_commits: ids.len() as u32,
            limit: config.delta_limit,
        });
    }

    ids.reverse();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(build_envelope_from_repo(&repo, vault, &id.to_string())?);
    }
    Ok(out)
}

/// Snapshot wrapper around `Engine::export_vault_to_bytes`.
/// Returns archive bytes + the HEAD SHA the snapshot was anchored at
/// so clients can persist the SHA as their next polling cursor.
pub fn build_snapshot(
    engine: &memstead_base::Engine,
    vault: &str,
) -> Result<SnapshotOutput, BridgeError> {
    // Resolve HEAD before exporting so the cursor in the response
    // matches the snapshot's content. The snapshot itself does not
    // walk git history; the SHA gives clients a concrete anchor for
    // their follow-up `/commits?since=<sha>` poll.
    let gitdir = resolve_vault_gitdir(engine, vault).ok();
    let head = match &gitdir {
        Some(g) => head_sha_for_vault(g, vault).unwrap_or_default(),
        None => String::new(),
    };
    let bytes = engine.export_vault_to_bytes(vault).map_err(|e| match e {
        memstead_base::EngineError::UnknownVault(name) => BridgeError::UnknownVault(name),
        other => BridgeError::Engine(other.to_string()),
    })?;
    Ok(SnapshotOutput {
        vault: vault.to_string(),
        bytes,
        head,
    })
}

/// Run a search against `vault` and return the wire envelope.
///
/// Framework-agnostic — `search_handler` in
/// [`crate::handlers`] is the axum-wired wrapper, but other
/// embedders (a CLI test driver, a different web framework) can
/// call this directly with their own auth + transport layer.
///
/// Refusal ladder (HTTP-status hints reflect the canonical
/// `search_handler` mapping):
/// - empty `q` (whitespace-only counts) →
///   [`BridgeError::InvalidSearchQuery`] / 400.
/// - `limit` outside `[1, config.search_max_limit]` → same code /
///   400.
/// - `vault` not mounted in `engine` → [`BridgeError::UnknownVault`]
///   / 404.
/// - Underlying `Engine::search` failure → [`BridgeError::Engine`] /
///   500.
///
/// Whitespace inside `q` is split into separate query terms — each
/// becomes one entry in [`memstead_base::ops::Query::any`]; BM25 ranks
/// entities matching more terms higher automatically.
pub fn run_search(
    engine: &memstead_base::Engine,
    vault: &str,
    query: SearchQuery,
    config: &BuildConfig,
) -> Result<SearchResult, BridgeError> {
    // 1. Validate `q`. Empty / whitespace-only is the canonical
    //    "you wanted overview, not search" refuse path — keep
    //    `/search` focused on text predicates.
    let q_trimmed = query.q.trim();
    if q_trimmed.is_empty() {
        return Err(BridgeError::InvalidSearchQuery {
            reason: "`q` is required and must contain at least one non-whitespace character"
                .to_string(),
        });
    }

    // 2. Validate `limit` against the configured ceiling. `None`
    //    falls back to the default — same shape MCP uses for
    //    `memstead_search` without an explicit limit.
    let limit = match query.limit {
        Some(0) => {
            return Err(BridgeError::InvalidSearchQuery {
                reason: "`limit` must be at least 1".to_string(),
            });
        }
        Some(n) if n > config.search_max_limit => {
            return Err(BridgeError::InvalidSearchQuery {
                reason: format!(
                    "`limit` {n} exceeds server max of {}",
                    config.search_max_limit
                ),
            });
        }
        Some(n) => n,
        None => config.search_default_limit,
    };

    // 3. Verify the vault is mounted. Folder, git-branch, archive —
    //    every backend `Engine::search` accepts is fine here.
    if !engine.vault_names().iter().any(|n| *n == vault) {
        return Err(BridgeError::UnknownVault(vault.to_string()));
    }

    // 4. Build the engine-side `SearchScope`. Whitespace-split into
    //    `any` terms; BM25 promotes multi-term matches.
    let terms: Vec<String> = q_trimmed
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    let inner_query = memstead_base::ops::Query {
        any: terms,
        not: Vec::new(),
        phrase: None,
        field: None,
    };
    let scope = memstead_base::ops::SearchScope {
        query: Some(inner_query),
        vault: Some(vault.to_string()),
        entity_type: query.entity_type.clone(),
        limit: Some(limit),
        offset: query.offset,
        filters: std::collections::HashMap::new(),
        range_filters: std::collections::HashMap::new(),
        edge_type: None,
        related_to: None,
        depth: None,
        expand_via: None,
        expand_depth: None,
        stub: None,
        token_budget: None,
    };

    // 5. Dispatch to the engine + project to the wire shape. The
    //    engine's `SearchHit` is re-exported as `wire::SearchHit`
    //    one-to-one — JSON byte-identical to MCP's per-hit shape.
    let result = engine
        .search(&scope)
        .map_err(|e| BridgeError::Engine(e.to_string()))?;

    let truncated = result.total > result.returned + result.offset;
    let hits: Vec<SearchHit> = result.hits.iter().map(SearchHit::from_engine).collect();
    Ok(SearchResult {
        vault: vault.to_string(),
        query: query.q,
        hits,
        total_matched: result.total,
        truncated,
        warnings: result.warnings.iter().map(ToString::to_string).collect(),
    })
}

/// Build a [`CommitEnvelope`] from an already-opened repo + a commit
/// SHA. Reusable across the single-commit and range entry points.
fn build_envelope_from_repo(
    repo: &gix::Repository,
    vault: &str,
    sha: &str,
) -> Result<CommitEnvelope, BridgeError> {
    let id = repo
        .rev_parse_single(sha)
        .map_err(|_| BridgeError::UnknownCommit(sha.to_string()))?;
    let object = id
        .object()
        .map_err(|_| BridgeError::UnknownCommit(sha.to_string()))?;
    let commit = object
        .try_into_commit()
        .map_err(|_| BridgeError::UnknownCommit(format!("{sha} is not a commit")))?;

    let resolved_sha = commit.id.to_hex().to_string();
    let parent_sha = commit
        .parent_ids()
        .next()
        .map(|p| p.detach().to_string())
        .unwrap_or_default();
    let timestamp = commit
        .time()
        .ok()
        .map(format_iso_8601)
        .unwrap_or_default();

    let raw_message = match commit.message_raw() {
        Ok(bstr) => std::str::from_utf8(bstr.as_ref())
            .map(|s| s.to_string())
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    let trailers = parse_trailers(&raw_message);

    let tree = commit
        .tree()
        .map_err(|e| BridgeError::Git(format!("tree({resolved_sha}): {e}")))?;

    // Parent tree for the diff. Empty for the root commit — use gix's
    // synthetic empty tree so `tree.changes()` against it lists every
    // file as an Addition.
    let parent_tree = if let Ok(Some(parent_id)) = commit.parent_ids().next().ok_or(()).map(Some)
    {
        let parent_obj = parent_id
            .object()
            .map_err(|e| BridgeError::Git(format!("parent({resolved_sha}): {e}")))?;
        let parent_commit = parent_obj
            .try_into_commit()
            .map_err(|_| BridgeError::Git(format!("parent of {resolved_sha} is not a commit")))?;
        parent_commit
            .tree()
            .map_err(|e| BridgeError::Git(format!("parent tree({resolved_sha}): {e}")))?
    } else {
        repo.empty_tree()
    };

    let mut platform = parent_tree
        .changes()
        .map_err(|e| BridgeError::Git(format!("diff init({resolved_sha}): {e}")))?;
    let rewrites = gix::diff::Rewrites {
        copies: None,
        percentage: Some(0.6),
        limit: 1000,
        track_empty: false,
    };
    platform.options(|opts| {
        opts.track_rewrites(Some(rewrites));
    });

    let mut changes: Vec<EntityChange> = Vec::new();
    platform
        .for_each_to_obtain_tree(
            &tree,
            |change| -> Result<std::ops::ControlFlow<()>, std::convert::Infallible> {
                match change {
                    Change::Addition { location, .. } => {
                        if let Some(path) = vault_entity_path(location.as_ref()) {
                            let content = read_blob(&tree, &path).unwrap_or_default();
                            changes.push(EntityChange::Added { path, content });
                        }
                    }
                    Change::Deletion { location, .. } => {
                        if let Some(path) = vault_entity_path(location.as_ref()) {
                            changes.push(EntityChange::Deleted { path });
                        }
                    }
                    Change::Modification { location, .. } => {
                        if let Some(path) = vault_entity_path(location.as_ref()) {
                            let content = read_blob(&tree, &path).unwrap_or_default();
                            changes.push(EntityChange::Modified { path, content });
                        }
                    }
                    Change::Rewrite {
                        source_location,
                        location,
                        ..
                    } => {
                        let from = vault_entity_path(source_location.as_ref());
                        let to = vault_entity_path(location.as_ref());
                        if let (Some(from), Some(to)) = (from, to) {
                            let content = read_blob(&tree, &to).unwrap_or_default();
                            changes.push(EntityChange::Renamed { from, to, content });
                        }
                    }
                }
                Ok(std::ops::ControlFlow::Continue(()))
            },
        )
        .map_err(|e| BridgeError::Git(format!("diff({resolved_sha}): {e}")))?;

    changes.sort_by(|a, b| primary_path(a).cmp(&primary_path(b)));

    Ok(CommitEnvelope {
        sha: resolved_sha,
        parent: parent_sha,
        vault: vault.to_string(),
        timestamp,
        trailers,
        changes,
    })
}

/// Filter a tree-diff path down to a vault entity. Skips the engine-
/// internal `.memstead/` subtree and non-`.md` entries; returns the
/// POSIX relative path so consumers can echo it as the wire-format
/// `path` value.
fn vault_entity_path(raw: &gix::bstr::BStr) -> Option<String> {
    let s = std::str::from_utf8(raw.as_ref()).ok()?;
    if s.is_empty() || !s.ends_with(".md") {
        return None;
    }
    if s.starts_with(".memstead/") {
        return None;
    }
    Some(s.to_string())
}

/// Read a `.md` blob from `tree` at the given relative path. Returns
/// `None` when the path is missing on this side (e.g. for a deletion's
/// post-state).
fn read_blob(tree: &gix::Tree<'_>, path: &str) -> Option<String> {
    let entry = tree.lookup_entry_by_path(path).ok()??;
    let object = entry.object().ok()?;
    let blob = object.try_into_blob().ok()?;
    String::from_utf8(blob.data.clone()).ok()
}

/// Sort key for `EntityChange` ordering. `Renamed` keys on `to` (the
/// surviving identity).
fn primary_path(c: &EntityChange) -> &str {
    match c {
        EntityChange::Added { path, .. }
        | EntityChange::Modified { path, .. }
        | EntityChange::Deleted { path } => path,
        EntityChange::Renamed { to, .. } => to,
    }
}

/// Format a gix commit timestamp as a UTC ISO 8601 string with
/// second precision. Falls back to empty when the time is invalid.
fn format_iso_8601(time: gix::date::Time) -> String {
    // gix::date::Time exposes `seconds` (Unix epoch seconds) and
    // `offset` (timezone delta). The bridge wire format wants UTC,
    // so we ignore `offset` for serialisation purposes. The standard
    // `time` crate isn't in the workspace; format manually.
    let secs = time.seconds;
    format_unix_seconds_utc(secs)
}

/// Tiny `Unix epoch seconds → "YYYY-MM-DDTHH:MM:SSZ"` formatter so
/// we don't need a `time` / `chrono` dependency. Handles 1970-2099 —
/// far enough for any reasonable vault history; commits outside that
/// range fall back to a `?` placeholder.
fn format_unix_seconds_utc(secs: i64) -> String {
    if secs < 0 || secs > 4_102_444_800 {
        return "?".to_string();
    }
    let s = secs as u64;
    let (year, month, day, hour, min, sec) = civil_from_unix(s);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn civil_from_unix(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = (secs % 86_400) as u32;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    // Howard Hinnant's days_to_ymd algorithm (public-domain pseudocode).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy as u32 - (153 * mp as u32 + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y } as u32;
    (y, m, d, hour, min, sec)
}

/// Parse commit-trailer lines from a commit message body. Standard
/// `Key: Value` shape at the trailer block (one trailer per line at
/// the end of the message); engine convention adds `Replays:` /
/// `Integration-Run:` etc. on top of git's stock `Co-Authored-By:`,
/// `Tool:`, etc.
fn parse_trailers(message: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    // Trailers live in the last "paragraph" (blank-line-delimited
    // block). Scan from the end backward until a blank line.
    let lines: Vec<&str> = message.lines().collect();
    let mut start = lines.len();
    for (i, line) in lines.iter().enumerate().rev() {
        if line.trim().is_empty() {
            start = i + 1;
            break;
        }
        start = i;
    }
    for line in &lines[start..] {
        if let Some((key, value)) = parse_trailer_line(line) {
            out.insert(key, value);
        }
    }
    out
}

fn parse_trailer_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let colon = trimmed.find(':')?;
    let key = trimmed[..colon].trim();
    let value = trimmed[colon + 1..].trim();
    if key.is_empty() || value.is_empty() {
        return None;
    }
    // Trailer keys are conventionally `Single-Word` or `Multi-Word-Hyphen`,
    // single token, ASCII letters / hyphen. Reject lines that look like
    // free-form prose (e.g. a sentence containing a colon).
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    Some((key.to_string(), value.to_string()))
}

/// Look up the gitdir for `vault`. Returns the path the git-branch
/// mount declares; folder / archive vaults refuse via
/// [`BridgeError::UnknownVault`] because the bridge wire format only
/// applies to git-backed vaults.
fn resolve_vault_gitdir(
    engine: &memstead_base::Engine,
    vault: &str,
) -> Result<std::path::PathBuf, BridgeError> {
    let names: Vec<&str> = engine.vault_names().into_iter().collect();
    if !names.iter().any(|n| *n == vault) {
        return Err(BridgeError::UnknownVault(vault.to_string()));
    }
    engine
        .gitdir_for(vault)
        .map_err(|_| BridgeError::UnknownVault(vault.to_string()))
}

/// Resolve `refs/heads/<vault>` to its SHA. Returns `None` when the
/// branch does not exist (fresh vault) — callers treat that as
/// "empty HEAD" rather than an error.
fn head_sha_for_vault(gitdir: &Path, vault: &str) -> Option<String> {
    let repo = gix::open(gitdir).ok()?;
    let id = repo
        .rev_parse_single(format!("refs/heads/{vault}").as_str())
        .ok()?;
    Some(id.detach().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use memstead_base::storage::VaultWriter;
    use memstead_git_branch::storage::git_tree::GitTreeVaultWriter;
    use memstead_base::vcs::CommitContext;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Read-only-by-construction guard. The bridge serves snapshots,
    /// commit ranges, and search over a shared engine and must never
    /// invoke a mutating engine operation. The builder entry points
    /// take `&Engine`, which blocks mutation at compile time, but the
    /// handlers hold an `Arc<Mutex<Engine>>` whose guard derefs
    /// mutably — so the invariant needs an explicit pin. This scans the
    /// crate's production source (everything before each file's
    /// `#[cfg(test)]` module) and fails if any mutating engine method
    /// call appears.
    #[test]
    fn no_mutating_engine_call_in_bridge_surface() {
        const MUTATING: &[&str] = &[
            ".create_entity(",
            ".update_entity(",
            ".delete_entity(",
            ".relate_entity(",
            ".rename_entity(",
            ".reload_one_vault(",
            ".reload_each_writable_vault(",
            ".reload_each_writable_vault_reports(",
            ".apply_commit(",
        ];
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for name in ["lib.rs", "builder.rs", "handlers.rs", "wire.rs", "error.rs"] {
            let path = src_dir.join(name);
            let file = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {name}: {e}"));
            // Production code only — drop the in-file `#[cfg(test)]`
            // module, which legitimately names mutating methods in
            // fixtures and in this very allowlist.
            let production = file.split("#[cfg(test)]").next().unwrap_or(&file);
            for pat in MUTATING {
                assert!(
                    !production.contains(pat),
                    "{name} calls a mutating engine method ({pat}) — the bridge surface must stay read-only by construction"
                );
            }
        }
    }

    fn init_gitdir(tmp: &TempDir) -> PathBuf {
        let gitdir = tmp.path().join("vault-repo").join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        gix::init_bare(&gitdir).unwrap();
        gitdir
    }

    fn body(title: &str) -> String {
        format!(
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# {title}\n\n## Identity\n\n{title}\n"
        )
    }

    fn commit(gitdir: &Path, branch: &str, file: &str, content: &str, subject: &str) -> String {
        let writer = GitTreeVaultWriter::new(
            gitdir.to_path_buf(),
            format!("refs/heads/{branch}"),
        );
        writer
            .write_entity(Path::new(file), content.as_bytes())
            .unwrap();
        writer.commit(subject, &CommitContext::internal()).unwrap()
    }

    fn engine_with_vault(gitdir: &Path) -> memstead_base::Engine {
        let mount = memstead_base::Mount {
            vault: "specs".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "default",
                semver::Version::new(1, 0, 0),
            )),
            storage: memstead_base::MountStorage::GitBranch {
                gitdir: gitdir.to_path_buf(),
                branch: "specs".to_string(),
            },
            capability: memstead_base::MountCapability::Write,
            lifecycle: memstead_base::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let backend = memstead_git_branch::storage::instantiate_pro_backend(&mount).unwrap();
        let mut engine =
            memstead_base::Engine::from_mounts(vec![(mount, backend)]).unwrap();
        engine.set_git_branch_ops(memstead_git_branch::storage::PRO_GIT_BRANCH_OPS);
        engine
    }

    #[test]
    fn parse_trailers_recovers_standard_key_value_lines() {
        let msg = "subject\n\nbody paragraph\n\nTool: memstead_update\nActor: agent\nClient: claude\n";
        let trailers = parse_trailers(msg);
        assert_eq!(trailers.get("Tool"), Some(&"memstead_update".to_string()));
        assert_eq!(trailers.get("Actor"), Some(&"agent".to_string()));
        assert_eq!(trailers.get("Client"), Some(&"claude".to_string()));
    }

    #[test]
    fn parse_trailers_ignores_prose_with_spaces_in_key() {
        // Free-form prose where the would-be key has a space — the
        // trailer parser rejects it because the alphanumeric+hyphen
        // grammar refuses spaces in the key token.
        let msg = "subject\n\nwell that is a fine result: nice\n";
        let trailers = parse_trailers(msg);
        assert!(trailers.is_empty(), "got: {trailers:?}");
    }

    #[test]
    fn iso8601_formatter_known_epoch_matches_spec() {
        // 2026-05-18T14:23:01Z = 1779114181 (computed externally).
        let s = format_unix_seconds_utc(1779114181);
        assert_eq!(s, "2026-05-18T14:23:01Z");
    }

    #[test]
    fn build_envelope_unknown_vault_returns_typed_error() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let engine = engine_with_vault(&gitdir);
        let err = build_commit_envelope(&engine, "missing", "deadbeef").unwrap_err();
        match err {
            BridgeError::UnknownVault(v) => assert_eq!(v, "missing"),
            other => panic!("expected UnknownVault, got {other:?}"),
        }
    }

    #[test]
    fn build_envelope_unknown_commit_returns_typed_error() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        commit(&gitdir, "specs", "alpha.md", &body("Alpha"), "seed");
        let engine = engine_with_vault(&gitdir);
        let err = build_commit_envelope(&engine, "specs", "0000000000000000000000000000000000000000")
            .unwrap_err();
        match err {
            BridgeError::UnknownCommit(s) => {
                assert!(s.starts_with("0000"), "got: {s}");
            }
            other => panic!("expected UnknownCommit, got {other:?}"),
        }
    }

    #[test]
    fn build_envelope_first_commit_lists_every_file_as_added() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let sha = commit(&gitdir, "specs", "alpha.md", &body("Alpha"), "seed");
        let engine = engine_with_vault(&gitdir);
        let env = build_commit_envelope(&engine, "specs", &sha).unwrap();
        assert_eq!(env.sha, sha);
        assert_eq!(env.parent, "");
        assert_eq!(env.vault, "specs");
        assert_eq!(env.changes.len(), 1);
        match &env.changes[0] {
            EntityChange::Added { path, content } => {
                assert_eq!(path, "alpha.md");
                assert!(content.contains("# Alpha"));
            }
            other => panic!("expected Added, got {other:?}"),
        }
    }

    #[test]
    fn build_envelope_modified_carries_post_state_content() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        commit(&gitdir, "specs", "alpha.md", &body("Alpha-v0"), "seed");
        let sha2 = commit(&gitdir, "specs", "alpha.md", &body("Alpha-v1"), "rev");
        let engine = engine_with_vault(&gitdir);
        let env = build_commit_envelope(&engine, "specs", &sha2).unwrap();
        assert_eq!(env.changes.len(), 1);
        match &env.changes[0] {
            EntityChange::Modified { path, content } => {
                assert_eq!(path, "alpha.md");
                assert!(content.contains("Alpha-v1"));
                assert!(!content.contains("Alpha-v0"));
            }
            other => panic!("expected Modified, got {other:?}"),
        }
    }

    #[test]
    fn build_envelopes_range_returns_chronological_order() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let sha1 = commit(&gitdir, "specs", "a.md", &body("A"), "first");
        let sha2 = commit(&gitdir, "specs", "b.md", &body("B"), "second");
        let sha3 = commit(&gitdir, "specs", "c.md", &body("C"), "third");
        let engine = engine_with_vault(&gitdir);
        let envs = build_commit_envelopes(
            &engine,
            "specs",
            memstead_base::ops::EMPTY_TREE_SHA,
            Some(&sha3),
            &BuildConfig::default(),
        )
        .unwrap();
        let shas: Vec<String> = envs.iter().map(|e| e.sha.clone()).collect();
        assert_eq!(shas, vec![sha1, sha2, sha3]);
    }

    #[test]
    fn build_envelopes_delta_too_large_refuses() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let mut last = String::new();
        for i in 0..5 {
            last = commit(
                &gitdir,
                "specs",
                &format!("entity-{i}.md"),
                &body(&format!("E{i}")),
                &format!("c{i}"),
            );
        }
        let engine = engine_with_vault(&gitdir);
        let config = BuildConfig {
            delta_limit: 2,
            ..BuildConfig::default()
        };
        let err = build_commit_envelopes(
            &engine,
            "specs",
            memstead_base::ops::EMPTY_TREE_SHA,
            Some(&last),
            &config,
        )
        .unwrap_err();
        match err {
            BridgeError::DeltaTooLarge { n_commits, limit } => {
                assert!(n_commits > limit);
                assert_eq!(limit, 2);
            }
            other => panic!("expected DeltaTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn build_snapshot_returns_bytes_and_head_sha() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        // The snapshot path needs a vault config blob — write one
        // through the same writer the engine uses for the vault
        // backend so the export path finds it.
        let writer = GitTreeVaultWriter::new(
            gitdir.clone(),
            "refs/heads/__MEMSTEAD".to_string(),
        );
        writer
            .write_entity(
                Path::new("vaults/specs/config.json"),
                br#"{"schema":"default@1.0.0","version":"1.0.0"}"#,
            )
            .unwrap();
        writer
            .commit("seed config", &CommitContext::internal())
            .unwrap();

        let sha = commit(&gitdir, "specs", "alpha.md", &body("Alpha"), "seed");
        let engine = engine_with_vault(&gitdir);
        let snap = build_snapshot(&engine, "specs").unwrap();
        assert_eq!(snap.vault, "specs");
        assert!(!snap.bytes.is_empty(), "snapshot must produce non-empty archive bytes");
        assert_eq!(snap.head, sha);
    }
}
