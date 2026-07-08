//! Source-cursor driver — assemble a [`SourceCursor`] from live workspace
//! state, so the brief's changed-slice preface can steer a pass at what moved.
//!
//! Engine-side port of the plugin's `computeSourceCursor` (`inject.mjs`). For
//! each of a projection's source facets it resolves the change-detection
//! strategy, reads the durable baseline from the **destination** mem's
//! `sync_state` (keyed `"<ingest>/<facet-or-refmem>"`), computes the changed
//! slice against the source's current state, and unions the per-facet slices.
//!
//! Strategies:
//!   - **git** — diff the stored commit id against the source tree's current
//!     `HEAD` (subprocess `git rev-parse` / `git diff --name-status`), with
//!     the facet scope + ingest `deny_paths` pushed down as `:(glob)` /
//!     `:(glob,exclude)` pathspecs.
//!   - **graph** — diff the source mem's snapshot token via the engine's own
//!     [`Engine::changes_since`]; reference mems are graph-detected too.
//!   - **mtime** — not yet assembled here (needs facet-file enumeration); a
//!     source resolving to `mtime` currently contributes no slice. The pure
//!     [`super::slice::mtime_slice_outcome`] core is ready for it.
//!
//! Load-bearing invariant: the new baseline `token` is only *collected* here
//! (into `write_commands` / `reseed`); the agent records it via
//! `set-sync-state` as the last step of a full pass. The driver never writes it.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::Engine;
use crate::pipeline::{MediumType, PatternMode};

use super::brief::{SourceCursor, SyncCommand};
use super::change_detection::{
    StatMap, compute_stat_map, digest_stat_map, parse_digest_token, serialize_digest_token,
};
use super::resolve::{
    ChangeStrategy, ResolvedIngest, ResolvedPrimarySource, ResolvedSource, find_git_root,
    resolve_change_strategy,
};
use super::slice::{Slice, SliceOutcome, graph_slice_outcome, is_git_token, mtime_slice_outcome};

/// Lexically normalize a path — resolve `.` and `..` without touching the
/// filesystem (no symlink resolution), matching Node's `path.resolve` on an
/// already-absolute path.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out: Vec<Component> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(comp),
            },
            other => out.push(other),
        }
    }
    out.iter().collect()
}

/// The relative path from `from` to `to` (both normalized), matching Node's
/// `path.relative`.
fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let from = normalize_lexical(from);
    let to = normalize_lexical(to);
    let from_comps: Vec<Component> = from.components().collect();
    let to_comps: Vec<Component> = to.components().collect();
    let mut common = 0;
    while common < from_comps.len()
        && common < to_comps.len()
        && from_comps[common] == to_comps[common]
    {
        common += 1;
    }
    let mut result = PathBuf::new();
    for _ in common..from_comps.len() {
        result.push("..");
    }
    for comp in &to_comps[common..] {
        result.push(comp.as_os_str());
    }
    result
}

/// The medium pointer resolved to an absolute base directory.
fn medium_base(pointer: &str, workspace_root: &Path) -> PathBuf {
    if pointer.is_empty() {
        workspace_root.to_path_buf()
    } else {
        normalize_lexical(&workspace_root.join(pointer))
    }
}

/// `git rev-parse HEAD` in `git_root`, or `None` on any failure.
fn git_head(git_root: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(git_root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Translate a workspace-relative facet pattern into a git pathspec relative
/// to `git_root`, with `:(glob)` magic (or `:(glob,exclude)` for a deny).
fn to_git_pathspec(pattern: &str, git_root: &Path, workspace_root: &Path, exclude: bool) -> String {
    let resolved = normalize_lexical(&workspace_root.join(pattern));
    let git_rel = relative_path(git_root, &resolved);
    let magic = if exclude {
        ":(glob,exclude)"
    } else {
        ":(glob)"
    };
    format!("{magic}{}", git_rel.to_string_lossy())
}

/// Build a [`GlobSet`] from workspace-relative glob patterns, or `None` if
/// any pattern is malformed.
fn build_glob_set(patterns: &[&str]) -> Option<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).ok()?);
    }
    builder.build().ok()
}

/// Enumerate the workspace-relative file paths a primary source's facet scope
/// selects — the `mtime` strategy's input set. Mirrors the plugin's
/// `enumerateFacetFiles`: only `codebase`/`filesystem` mediums; the facet's
/// allow globs minus its deny globs, evaluated over the medium's directory
/// tree; empty when the facet has no allows. Returns a sorted, de-duplicated
/// list.
pub fn enumerate_facet_files(source: &ResolvedPrimarySource, workspace_root: &Path) -> Vec<String> {
    if !matches!(
        source.medium_type,
        MediumType::Codebase | MediumType::Filesystem
    ) {
        return Vec::new();
    }
    let mut allows: Vec<&str> = Vec::new();
    let mut denies: Vec<&str> = Vec::new();
    for rule in &source.scope {
        match rule.mode {
            PatternMode::Allow => allows.push(&rule.path),
            PatternMode::Deny => denies.push(&rule.path),
        }
    }
    if allows.is_empty() {
        return Vec::new();
    }
    let Some(allow_set) = build_glob_set(&allows) else {
        return Vec::new();
    };
    let deny_set = if denies.is_empty() {
        None
    } else {
        build_glob_set(&denies)
    };

    // Walk the medium's directory tree; the facet patterns are
    // workspace-relative, so each candidate is matched by its
    // workspace-relative path.
    let base = medium_base(&source.medium_pointer, workspace_root);
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![base];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                let rel = relative_path(workspace_root, &normalize_lexical(&path))
                    .to_string_lossy()
                    .to_string();
                let denied = deny_set.as_ref().is_some_and(|d| d.is_match(&rel));
                if allow_set.is_match(&rel) && !denied {
                    out.push(rel);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Compute the git changed slice for one primary source between its stored
/// baseline commit and the tree's current `HEAD`. Mirrors `computeGitSlice`.
fn compute_git_slice(
    source: &ResolvedPrimarySource,
    deny_paths: &[String],
    workspace_root: &Path,
    baseline: Option<&str>,
) -> SliceOutcome {
    let base = medium_base(&source.medium_pointer, workspace_root);
    let Some(git_root) = find_git_root(&base) else {
        return SliceOutcome::NoSignal;
    };
    let Some(head) = git_head(&git_root) else {
        return SliceOutcome::NoSignal;
    };

    let baseline = match baseline {
        Some(b) if is_git_token(b) => b,
        // No usable commit baseline — seed at HEAD, present no slice.
        _ => return SliceOutcome::Reseed { token: head },
    };
    if baseline == head {
        return SliceOutcome::Unchanged { token: head };
    }

    // Pathspecs from the facet scope + the ingest's deny_paths.
    let mut allows: Vec<&str> = Vec::new();
    let mut denies: Vec<&str> = Vec::new();
    for rule in &source.scope {
        match rule.mode {
            PatternMode::Allow => allows.push(&rule.path),
            PatternMode::Deny => denies.push(&rule.path),
        }
    }
    if allows.is_empty() {
        // Unscoped facet — refuse to diff the whole repo.
        return SliceOutcome::NoSignal;
    }
    for dp in deny_paths {
        denies.push(dp);
    }
    let mut specs: Vec<String> = Vec::with_capacity(allows.len() + denies.len());
    for a in &allows {
        specs.push(to_git_pathspec(a, &git_root, workspace_root, false));
    }
    for d in &denies {
        specs.push(to_git_pathspec(d, &git_root, workspace_root, true));
    }

    let mut cmd = Command::new("git");
    cmd.args([
        "diff",
        "--no-renames",
        "--name-status",
        baseline,
        &head,
        "--",
    ]);
    cmd.args(&specs);
    cmd.current_dir(&git_root);
    let out = match cmd.output() {
        Ok(o) if o.status.success() => o,
        // Unknown baseline (gc'd / rewritten), an out-of-repo pathspec, or a
        // git failure — degrade to a whole re-roam (the plugin does the same).
        _ => return SliceOutcome::NoSignal,
    };
    let text = String::from_utf8_lossy(&out.stdout);

    let mut slice = Slice::default();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some(tab) = line.find('\t') else { continue };
        let status = line[..tab].trim();
        let git_path = line[tab + 1..].trim();
        let ws_path = relative_path(workspace_root, &normalize_lexical(&git_root.join(git_path)))
            .to_string_lossy()
            .to_string();
        match status.chars().next() {
            Some('A') => slice.added.push(ws_path),
            Some('D') => slice.deleted.push(ws_path),
            // M, T (type change), C, and the rest.
            _ => slice.modified.push(ws_path),
        }
    }
    slice.added.sort();
    slice.modified.sort();
    slice.deleted.sort();
    SliceOutcome::Changed {
        token: head,
        slice,
        degraded: false,
    }
}

/// Compute the graph changed slice for a source mem between its stored
/// baseline snapshot token and the mem's current head. Mirrors
/// `computeGraphSlice`, using the engine's own change history.
fn compute_graph_slice(engine: &Engine, source_mem: &str, baseline: Option<&str>) -> SliceOutcome {
    let current = match engine.mem_head_sha(source_mem) {
        Ok(Some(sha)) => sha,
        // Source has no snapshot signal, or is unknown — degrade.
        _ => return SliceOutcome::NoSignal,
    };
    // Fetch the entity delta only when the source actually moved.
    let changed = matches!(baseline, Some(b) if is_git_token(b) && b != current);
    if changed {
        let baseline = baseline.expect("changed implies a baseline");
        match engine.changes_since(source_mem, baseline, None) {
            Ok(report) => graph_slice_outcome(Some(baseline), &current, &report.changes),
            // Unknown baseline / engine error — degrade.
            Err(_) => SliceOutcome::NoSignal,
        }
    } else {
        graph_slice_outcome(baseline, &current, &[])
    }
}

// ── mtime source-cursor memo ────────────────────────────────────────────────
//
// The `mtime` strategy's durable baseline is a small digest token (in the
// destination mem's `sync_state`), which cannot by itself say *which* files
// changed. The engine keeps a rebuildable memo — the full stat map keyed by
// its digest aggregate — so a run whose baseline matches a memoised aggregate
// diffs precisely (incl. deletions) instead of degrading to a full scan.
//
// The memo lives engine-side under `<workspace>/.memstead.cache/ingest/` in
// the plugin's format (`{aggregate: {relpath: {mtime, size}}}`), so the engine
// and the transition-era skill share it. It is pure engine-internal cache —
// not mem-repo, not the graph — so writing it during brief rendering is not a
// tracked mutation. A write failure only costs the next run's precision.

/// The `<cache_root>/source-cursor/<ingest>/<facet>.json` memo path.
fn cursor_memo_path(cache_root: &Path, ingest_name: &str, facet_ref: &str) -> PathBuf {
    let safe: String = facet_ref
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    cache_root
        .join("source-cursor")
        .join(ingest_name)
        .join(format!("{safe}.json"))
}

/// Read the stat map memoised under `aggregate` for a facet, or `None` on miss.
fn read_cursor_memo(
    cache_root: &Path,
    ingest: &str,
    facet: &str,
    aggregate: &str,
) -> Option<StatMap> {
    let bytes = std::fs::read(cursor_memo_path(cache_root, ingest, facet)).ok()?;
    let memo: BTreeMap<String, StatMap> = serde_json::from_slice(&bytes).ok()?;
    memo.get(aggregate).cloned()
}

/// Memoise the current stat map under its aggregate, bounding the file to the
/// 3 most-recent aggregates. Best-effort.
fn write_cursor_memo(cache_root: &Path, ingest: &str, facet: &str, aggregate: &str, map: &StatMap) {
    let path = cursor_memo_path(cache_root, ingest, facet);
    let mut memo: BTreeMap<String, StatMap> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    memo.insert(aggregate.to_string(), map.clone());
    if memo.len() > 3 {
        // Keep the just-written aggregate plus up to two others.
        let drop: Vec<String> = memo
            .keys()
            .filter(|k| k.as_str() != aggregate)
            .skip(2)
            .cloned()
            .collect();
        for key in drop {
            memo.remove(&key);
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(&memo) {
        let _ = std::fs::write(&path, bytes);
    }
}

/// Compute the `mtime` changed slice for one primary source: enumerate the
/// facet files, stat them, memoise the current map, and diff against the
/// baseline digest's memoised map (precise) or degrade to a full scan on memo
/// miss. Mirrors the mtime branch of the plugin's `computeSourceCursor`.
fn compute_mtime_slice(
    source: &ResolvedPrimarySource,
    ingest_name: &str,
    workspace_root: &Path,
    cache_root: &Path,
    baseline: Option<&str>,
) -> SliceOutcome {
    let files = enumerate_facet_files(source, workspace_root);
    let now_map = compute_stat_map(&files, workspace_root);
    let now_digest = digest_stat_map(&now_map);
    write_cursor_memo(
        cache_root,
        ingest_name,
        &source.facet_ref,
        &now_digest.aggregate,
        &now_map,
    );
    let prev_map = baseline.and_then(parse_digest_token).and_then(|base| {
        read_cursor_memo(cache_root, ingest_name, &source.facet_ref, &base.aggregate)
    });
    mtime_slice_outcome(baseline, prev_map.as_ref(), &now_map)
}

/// The current change-detection token for a primary source, per its resolved
/// strategy: git `HEAD`, the graph mem's snapshot, or the freshly-computed
/// mtime digest. `None` when there is no signal.
fn current_primary_token(
    engine: &Engine,
    source: &ResolvedPrimarySource,
    workspace_root: &Path,
) -> Option<String> {
    match resolve_change_strategy(source, workspace_root) {
        ChangeStrategy::Git => git_head(&find_git_root(&medium_base(
            &source.medium_pointer,
            workspace_root,
        ))?),
        ChangeStrategy::Graph => engine.mem_head_sha(&source.medium_pointer).ok().flatten(),
        ChangeStrategy::Mtime => {
            let files = enumerate_facet_files(source, workspace_root);
            Some(serialize_digest_token(&digest_stat_map(&compute_stat_map(
                &files,
                workspace_root,
            ))))
        }
        ChangeStrategy::None => None,
    }
}

/// Whether any of an ingest's sources moved since its last synced pass — the
/// cheap, slice-free predicate the backoff uses as its additive second
/// trigger. Compares each source's current token to the baseline stored in the
/// destination mem's `sync_state`; a source with no baseline is not "moved"
/// (a first sync does not by itself defeat backoff). Mirrors the plugin's
/// `sourceChangedSince`.
pub fn source_moved(engine: &Engine, resolved: &ResolvedIngest, workspace_root: &Path) -> bool {
    let dest = &resolved.destination_mem;
    let baseline_map = engine
        .mem_config_for(dest)
        .map(|c| c.sync_state.clone())
        .unwrap_or_default();

    for source in &resolved.sources {
        let (facet_ref, current) = match source {
            ResolvedSource::Primary(p) => (
                p.facet_ref.clone(),
                current_primary_token(engine, p, workspace_root),
            ),
            ResolvedSource::Reference { mem } => {
                (mem.clone(), engine.mem_head_sha(mem).ok().flatten())
            }
        };
        let key = format!("{}/{}", resolved.name, facet_ref);
        let Some(baseline) = baseline_map.get(&key) else {
            continue; // no baseline ⇒ not "moved"
        };
        if let Some(current) = current
            && !current.is_empty()
            && current != *baseline
        {
            return true;
        }
    }
    false
}

/// Assemble the combined [`SourceCursor`] for an ingest from live state: the
/// destination mem's `sync_state` baselines and each source's current state.
pub fn compute_source_cursor(
    engine: &Engine,
    resolved: &ResolvedIngest,
    workspace_root: &Path,
) -> SourceCursor {
    let dest = &resolved.destination_mem;
    let baseline_map = engine
        .mem_config_for(dest)
        .map(|c| c.sync_state.clone())
        .unwrap_or_default();

    let cache_root = workspace_root.join(".memstead.cache").join("ingest");
    let mut union = Slice::default();
    let mut write_commands: Vec<SyncCommand> = Vec::new();
    let mut reseed: Vec<SyncCommand> = Vec::new();
    let mut degraded = false;

    for source in &resolved.sources {
        // Key: "<ingest>/<facet_ref>" for primaries, "<ingest>/<mem>" for
        // reference sources — matching the plugin's sync_state keying.
        let (facet_ref, outcome) = match source {
            ResolvedSource::Primary(p) => {
                let key = format!("{}/{}", resolved.name, p.facet_ref);
                let baseline = baseline_map.get(&key).map(String::as_str);
                let outcome = match resolve_change_strategy(p, workspace_root) {
                    ChangeStrategy::Git => {
                        compute_git_slice(p, &resolved.deny_paths, workspace_root, baseline)
                    }
                    // A graph-typed primary's medium pointer is the source mem id.
                    ChangeStrategy::Graph => {
                        compute_graph_slice(engine, &p.medium_pointer, baseline)
                    }
                    ChangeStrategy::Mtime => compute_mtime_slice(
                        p,
                        &resolved.name,
                        workspace_root,
                        &cache_root,
                        baseline,
                    ),
                    // `none` is inert — no slice.
                    ChangeStrategy::None => SliceOutcome::NoSignal,
                };
                (p.facet_ref.clone(), outcome)
            }
            ResolvedSource::Reference { mem } => {
                let key = format!("{}/{}", resolved.name, mem);
                let baseline = baseline_map.get(&key).map(String::as_str);
                (mem.clone(), compute_graph_slice(engine, mem, baseline))
            }
        };

        let key = format!("{}/{}", resolved.name, facet_ref);
        match outcome {
            SliceOutcome::NoSignal | SliceOutcome::Unchanged { .. } => {}
            SliceOutcome::Reseed { token } => reseed.push(SyncCommand { key, token }),
            SliceOutcome::Changed {
                token,
                slice,
                degraded: d,
            } => {
                union.added.extend(slice.added);
                union.modified.extend(slice.modified);
                union.deleted.extend(slice.deleted);
                degraded |= d;
                write_commands.push(SyncCommand { key, token });
            }
        }
    }

    dedupe_sort(&mut union.added);
    dedupe_sort(&mut union.modified);
    dedupe_sort(&mut union.deleted);
    let any_changes =
        !union.added.is_empty() || !union.modified.is_empty() || !union.deleted.is_empty();

    SourceCursor {
        union,
        write_commands,
        reseed,
        any_changes,
        degraded,
        dest_mem: dest.clone(),
    }
}

fn dedupe_sort(v: &mut Vec<String>) {
    v.sort();
    v.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_resolves_dot_and_dotdot() {
        assert_eq!(
            normalize_lexical(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
        assert_eq!(
            normalize_lexical(Path::new("/a/../../b")),
            PathBuf::from("/b"),
            "dotdot past root is clamped"
        );
    }

    #[test]
    fn relative_computes_updowns() {
        assert_eq!(
            relative_path(Path::new("/a/b"), Path::new("/a/b/c/d")),
            PathBuf::from("c/d")
        );
        assert_eq!(
            relative_path(Path::new("/a/b/c"), Path::new("/a/x")),
            PathBuf::from("../../x")
        );
        // A workspace whose medium is a sibling repository.
        assert_eq!(
            relative_path(Path::new("/m/public"), Path::new("/m/public/crates/x.rs")),
            PathBuf::from("crates/x.rs")
        );
        assert_eq!(
            relative_path(Path::new("/m/graph"), Path::new("/m/public/crates/x.rs")),
            PathBuf::from("../public/crates/x.rs")
        );
    }

    #[test]
    fn pathspec_builds_glob_magic_relative_to_git_root() {
        let ws = Path::new("/m/graph");
        let git_root = Path::new("/m/public");
        assert_eq!(
            to_git_pathspec("../public/**/*.rs", git_root, ws, false),
            ":(glob)**/*.rs"
        );
        assert_eq!(
            to_git_pathspec("../public/target/**", git_root, ws, true),
            ":(glob,exclude)target/**"
        );
    }

    use crate::ingest::resolve::ResolvedPrimarySource;
    use crate::pipeline::{MediumType, PatternEntry};

    fn git(repo: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    fn primary(scope: Vec<PatternEntry>) -> ResolvedPrimarySource {
        ResolvedPrimarySource {
            facet_ref: "src".to_string(),
            medium: "m".to_string(),
            medium_type: MediumType::Codebase,
            medium_pointer: String::new(),
            declared_change_detection: Some("git".to_string()),
            scope,
            preparation: None,
        }
    }

    /// A real git diff: baseline commit → HEAD produces the changed slice,
    /// classifying added / modified / deleted and honouring the scope.
    #[test]
    fn git_slice_diffs_baseline_to_head() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        git(root, &["init", "-q"]);
        std::fs::write(root.join("keep.rs"), "one").unwrap();
        std::fs::write(root.join("gone.rs"), "bye").unwrap();
        std::fs::write(root.join("note.md"), "ignored-by-scope").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "base"]);
        let baseline = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(root)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        // Move: modify keep.rs, delete gone.rs, add new.rs, touch note.md.
        std::fs::write(root.join("keep.rs"), "two").unwrap();
        std::fs::remove_file(root.join("gone.rs")).unwrap();
        std::fs::write(root.join("new.rs"), "hi").unwrap();
        std::fs::write(root.join("note.md"), "still ignored").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "move"]);

        // Scope to *.rs only — note.md must not appear.
        let source = primary(vec![PatternEntry {
            path: "**/*.rs".to_string(),
            mode: PatternMode::Allow,
        }]);
        let outcome = compute_git_slice(&source, &[], root, Some(&baseline));
        match outcome {
            SliceOutcome::Changed {
                slice, degraded, ..
            } => {
                assert!(!degraded);
                assert_eq!(slice.added, vec!["new.rs"]);
                assert_eq!(slice.modified, vec!["keep.rs"]);
                assert_eq!(slice.deleted, vec!["gone.rs"]);
            }
            other => panic!("expected Changed, got {other:?}"),
        }

        // Same baseline == HEAD → Unchanged.
        let head = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(root)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        assert!(matches!(
            compute_git_slice(&source, &[], root, Some(&head)),
            SliceOutcome::Unchanged { .. }
        ));

        // A non-commit baseline → Reseed at HEAD.
        assert!(matches!(
            compute_git_slice(&source, &[], root, None),
            SliceOutcome::Reseed { .. }
        ));
    }

    /// Facet-file enumeration honours allow globs, deny globs, and the
    /// codebase/filesystem medium-type gate.
    #[test]
    fn enumerate_honours_allow_and_deny() {
        let ws = tempfile::tempdir().unwrap();
        let root = ws.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.rs"), "").unwrap();
        std::fs::write(root.join("sub/b.rs"), "").unwrap();
        std::fs::write(root.join("c.md"), "").unwrap();

        // medium_pointer "" → base is the workspace root; allow **/*.rs,
        // deny sub/** (so sub/b.rs is excluded, c.md never matched).
        let source = primary(vec![
            PatternEntry {
                path: "**/*.rs".to_string(),
                mode: PatternMode::Allow,
            },
            PatternEntry {
                path: "sub/**".to_string(),
                mode: PatternMode::Deny,
            },
        ]);
        assert_eq!(enumerate_facet_files(&source, root), vec!["a.rs"]);

        // A graph medium enumerates nothing (not a file tree).
        let mut graph_source = source.clone();
        graph_source.medium_type = MediumType::Graph;
        assert!(enumerate_facet_files(&graph_source, root).is_empty());
    }

    /// The mtime driver reseeds on the first pass (writing the memo), then
    /// diffs precisely against the memoised map — including deletions.
    #[test]
    fn mtime_driver_reseeds_then_diffs_precisely() {
        let ws = tempfile::tempdir().unwrap();
        let root = ws.path();
        let cache = root.join(".memstead.cache").join("ingest");
        std::fs::write(root.join("a.rs"), "one").unwrap();
        std::fs::write(root.join("gone.rs"), "bye").unwrap();
        let source = primary(vec![PatternEntry {
            path: "**/*.rs".to_string(),
            mode: PatternMode::Allow,
        }]);

        // First pass: no baseline → reseed at the current digest, memo written.
        let token = match compute_mtime_slice(&source, "ing", root, &cache, None) {
            SliceOutcome::Reseed { token } => token,
            other => panic!("expected Reseed, got {other:?}"),
        };

        // Move the source: modify a.rs (size change), delete gone.rs, add new.rs.
        std::fs::write(root.join("a.rs"), "one-longer").unwrap();
        std::fs::remove_file(root.join("gone.rs")).unwrap();
        std::fs::write(root.join("new.rs"), "x").unwrap();

        // Second pass with the reseed token → precise diff from the memo.
        match compute_mtime_slice(&source, "ing", root, &cache, Some(&token)) {
            SliceOutcome::Changed {
                slice, degraded, ..
            } => {
                assert!(
                    !degraded,
                    "memo present → precise, not a degraded full scan"
                );
                assert_eq!(slice.added, vec!["new.rs"]);
                assert_eq!(slice.modified, vec!["a.rs"]);
                assert_eq!(
                    slice.deleted,
                    vec!["gone.rs"],
                    "deletions come from the memo"
                );
            }
            other => panic!("expected Changed, got {other:?}"),
        }

        // A run whose baseline aggregate is not memoised degrades to a full
        // scan (every current file as added, no deletions).
        let stale = super::super::change_detection::serialize_digest_token(
            &super::super::change_detection::digest_stat_map(&stat_map_for(&["absent.rs"])),
        );
        match compute_mtime_slice(&source, "ing", root, &cache, Some(&stale)) {
            SliceOutcome::Changed { degraded, .. } => assert!(degraded, "memo miss → degraded"),
            other => panic!("expected degraded Changed, got {other:?}"),
        }
    }

    fn stat_map_for(paths: &[&str]) -> super::super::change_detection::StatMap {
        paths
            .iter()
            .map(|p| {
                (
                    (*p).to_string(),
                    super::super::change_detection::StatEntry { mtime: 1, size: 1 },
                )
            })
            .collect()
    }
}
