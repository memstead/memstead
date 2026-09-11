//! Source-cursor driver — assemble a [`SourceCursor`] from live workspace
//! state, so the brief's changed-slice preface can steer a pass at what moved.
//!
//! Engine-side port of the plugin's `computeSourceCursor` (`inject.mjs`). For
//! each of a binding's source facets it resolves the change-detection
//! strategy, reads the durable baseline from the **destination** mem's
//! `sync_state` (keyed `"<binding-id>/<facet-or-refmem>#synced"`, D4), computes
//! the changed slice against the source's current state, and unions the
//! per-facet slices.
//!
//! Strategies:
//!   - **git** — diff the stored commit id against the source tree's current
//!     `HEAD` (subprocess `git rev-parse` / `git diff --name-status`), with
//!     the facet scope + ingest `deny_paths` pushed down as `:(glob)` /
//!     `:(glob,exclude)` pathspecs.
//!   - **graph** — diff the source mem's snapshot token via the engine's own
//!     [`Engine::changes_since`]; reference mems are graph-detected too.
//!   - **mtime** — enumerate the facet's files (minus the facet scope's own
//!     denies *and* the ingest `deny_paths`, applied identically to the git
//!     strategy's exclude pathspecs — see [`enumerate_facet_files`]), compute a
//!     stat-map digest, memoise it under `.memstead.cache/ingest/source-cursor/`,
//!     and diff the current digest against the memoised baseline via the pure
//!     [`super::slice::mtime_slice_outcome`] core (precise, incl. deletions).
//!
//! **Deny invariance.** Ingest `deny_paths` are enforced identically by every
//! strategy that reads a file tree — git, mtime, and refinement's enumeration,
//! plus both token computations (`current_primary_token` / [`source_moved`]).
//! A file matching a `deny_paths` entry appears in no changed slice, no
//! refinement batch, and never influences the mtime digest or the
//! `source_moved` token. The **graph** strategy is exempt *by definition*:
//! `deny_paths` entries are file-path globs, but a graph source's artifacts are
//! entities (entity-granular), so a file-path glob can never select one. This
//! exemption is designed, not an omission.
//!
//! **One deny dialect.** A `deny_paths` entry is a **workspace-relative glob**,
//! resolved by [`memstead_base::check_path::DenyOracle`] — the glob match plus the
//! literal-base directory-prefix rule plus the malformed-entry fallback — the
//! same resolver the path-check command answers with. The git strategy pushes
//! what git's own pathspec dialect can express and filters its diff through
//! the oracle afterwards, so the two strategies agree even on the entries
//! pathspecs cannot express.
//!
//! Note that a `deny_paths` entry's resolution root is the WORKSPACE, while a
//! facet-scope entry's is its source's `pointer` (since 2026-08-27). Those are
//! two namespaces for two questions, not two dialects for one: an ingest deny
//! spans every source in the binding, so it has no single pointer to be
//! relative to. The plugin's
//! PreToolUse deny hook enforces the *identical* dialect against the ingest
//! agent's Read/Glob/Grep by asking the engine itself: `projection
//! check-path` answers through [`memstead_base::check_path::check_deny_paths`], which
//! reads the active binding's record fresh on every call (the pointer channel
//! is [`memstead_base::check_path::write_active_binding_file`], published on
//! consuming brief renders). A deny entry that selects **no file** in
//! the project tree is surfaced as a rendered brief warning
//! ([`SourceCursor::dead_denies`]) rather than silently no-op'ing — catching
//! typos and un-migrated legacy bare names, never a hard error.
//!
//! **One empty-scope semantic.** A facet with **no allow patterns** is
//! *unscoped* — and that is a **typed refusal**, identical on every file-tree
//! strategy: git, mtime, and refinement all decline to diff or enumerate the
//! whole medium (a `facet_unscoped` check gates it). No strategy silently emits an
//! empty slice, enumeration, or batch for an unscoped facet; instead the source
//! contributes [`NoSignalReason::Unscoped`], which renders in the brief. A
//! facet that genuinely wants the whole medium writes `**/*`. This is a
//! different field from the ingest's `deny_paths`: an **empty `deny_paths`**
//! list is valid and means "no denies" — it never trips the unscoped refusal.
//!
//! **Visible no-signal.** Every source contributes a per-source outcome. A
//! genuinely-unchanged source (baseline present, nothing moved) stays silent —
//! the only documented silence, preserving the "brief is byte-identical to a
//! plain roam when nothing moved" property. Every other no-signal condition —
//! unscoped facet, `signal:none`, git failure / unknown baseline, missing graph
//! snapshot — is collected as a [`NoSignalNote`] and rendered distinguishably.
//!
//! Load-bearing invariant: the new baseline `token` is only *collected* here
//! (into `write_commands` / `reseed`); it is recorded by the engine's
//! `set_mem_sync_state` writer when `projection advance` completes a full pass
//!. The driver never writes it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use memstead_base::Engine;
use memstead_base::pipeline::{MediumType, PatternMode};

use super::brief::{DeliveredUnit, DeliverySequence, NoSignalNote, SourceCursor, SyncCommand};
use super::change_detection::{
    StatMap, compute_stat_map, digest_stat_map, parse_digest_token, serialize_digest_token,
};
use super::slice::{
    NoSignalReason, Slice, SliceOutcome, graph_slice_outcome, is_git_token, mtime_slice_outcome,
};
use memstead_base::binding_run::{
    ChangeStrategy, ResolvedIngest, ResolvedSource, find_git_root, resolve_change_strategy,
};
use memstead_base::pipeline::Source;

// The scope-enumeration cluster lives in the kernel (`memstead_base::source_scope`)
// since 2026-09-10; these re-exports keep every `ingest::cursor::` path.
pub use memstead_base::source_scope::{
    EntitySelector, ScopeEnumeration, ScopePatternNote, enumerate_facet_files,
    enumerate_facet_files_reported, enumerate_graph_entities, enumerate_source_artifacts,
    enumerate_source_artifacts_reported, medium_base, parse_entity_selector, scope_migration_notes,
};
#[cfg(test)]
pub(crate) use memstead_base::source_scope::{allow_could_match_under, glob_literal_prefix};
pub(crate) use memstead_base::source_scope::{
    build_glob_set, engine_state_denies, normalize_lexical, relative_path, selector_matches,
};

/// The relative path from `from` to `to`, lexically normalized — public so a
/// caller holding two absolute paths (a workspace root and a source tree, say)
/// can express one as a medium pointer against the other, the exact inverse of
/// [`medium_base`].
pub fn relative_to(from: &Path, to: &Path) -> PathBuf {
    relative_path(from, to)
}

/// The honest caveat for a medium base that resolves outside the workspace
/// root, or `None` when it does not — the single wording every front door
/// that scaffolds a binding prints, so the layout split is named once, at the
/// layout decision, in the same terms everywhere.
///
/// The shape is supported: enumeration, change detection, sync, and anchor
/// resolution all work on it (measured on this project's own out-of-root
/// bindings, where zero anchors orphan). What degrades rides the message —
/// `../…` artifact ids and a layout that must stay fixed — together with the
/// recipe that avoids it. Only path-namespace mediums can be out-of-root;
/// every other medium type yields `None`.
pub fn out_of_root_layout_warning(
    pointer: &str,
    workspace_root: &Path,
    medium_type: memstead_base::pipeline::MediumType,
) -> Option<String> {
    use memstead_base::pipeline::MediumType;
    if !matches!(medium_type, MediumType::Codebase | MediumType::Filesystem) {
        return None;
    }
    let base = medium_base(pointer, workspace_root);
    // Canonicalize both sides when possible so symlinked roots (macOS /tmp)
    // don't false-positive; fall back to the lexical forms for not-yet-existing
    // paths.
    let canon_base = std::fs::canonicalize(&base).unwrap_or(base);
    let canon_root =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    if canon_base.starts_with(&canon_root) {
        return None;
    }
    Some(format!(
        "medium base '{}' resolves outside the workspace root '{}': supported — \
         enumeration, change detection, and anchor resolution all work on this shape — \
         but artifact ids render as workspace-relative '../…' chains and the \
         workspace-to-source relative layout must stay fixed (moving either side \
         breaks the pointer). To avoid the '../…' ids, root the workspace at the \
         common parent directory containing every source tree.",
        canon_base.display(),
        canon_root.display()
    ))
}

/// Whether `sha` names a commit that exists in the repo at `git_root`.
/// `git cat-file -e <sha>^{commit}` — exit 0 iff present and a commit.
fn commit_exists(git_root: &Path, sha: &str) -> bool {
    Command::new("git")
        .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
        .current_dir(git_root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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

/// Translate a pattern into a git pathspec relative to `git_root`, with
/// `:(glob)` magic (or `:(glob,exclude)` for a deny).
///
/// `base` is the namespace the pattern is written in: the medium base for a
/// facet scope pattern (source-relative), the workspace root for an
/// ingest-level deny (workspace-relative). One join, named by the caller,
/// rather than one hardcoded root that silently disagrees with the walk.
///
/// A `**`-prefixed pattern joins at `base` like every other pattern — the
/// pointer join is what confines the diff to the medium subtree. Emitted
/// verbatim (git-root-relative), a scope glob like `**/*.md` matched the
/// WHOLE repository, so a sub-tree-pointed binding's changed slice presented
/// artifacts its enumeration correctly kept out of `S(D)` and its `exclude`
/// gate refused as out-of-scope — the two axes provably disagreed
/// (drift-benchmark runs 03 and 06 on `plugin/graph`). `base` sits inside
/// `git_root` by construction on the allow path (the root is found by walking
/// up from `base`), so the joined form cannot escape the repo there; the
/// escape fallback below keeps the historical repo-wide reading for a deny
/// whose namespace root lies outside this repo, which for an exclude is the
/// conservative direction.
fn to_git_pathspec(pattern: &str, git_root: &Path, base: &Path, exclude: bool) -> String {
    let magic = if exclude {
        ":(glob,exclude)"
    } else {
        ":(glob)"
    };
    let resolved = normalize_lexical(&base.join(pattern));
    let git_rel = relative_path(git_root, &resolved);
    if pattern.starts_with("**")
        && git_rel
            .components()
            .next()
            .is_some_and(|c| c == Component::ParentDir)
    {
        return format!("{magic}{pattern}");
    }
    format!("{magic}{}", git_rel.to_string_lossy())
}

/// Like [`to_git_pathspec`], but `None` when the pattern resolves *outside*
/// `git_root` (its git-relative path escapes with a leading `..`). Git fatals
/// on an out-of-tree pathspec, so a cross-repo deny must be dropped from the
/// diff rather than pushed — it can match nothing in this repo regardless.
fn in_repo_pathspec(pattern: &str, git_root: &Path, base: &Path, exclude: bool) -> Option<String> {
    // Prefix-free glob — same verbatim re-anchoring as `to_git_pathspec`.
    if pattern.starts_with("**") {
        return Some(to_git_pathspec(pattern, git_root, base, exclude));
    }
    let resolved = normalize_lexical(&base.join(pattern));
    let git_rel = relative_path(git_root, &resolved);
    if git_rel
        .components()
        .next()
        .is_some_and(|c| c == Component::ParentDir)
    {
        return None;
    }
    let magic = if exclude {
        ":(glob,exclude)"
    } else {
        ":(glob)"
    };
    Some(format!("{magic}{}", git_rel.to_string_lossy()))
}

/// Whether a primary source's facet declares **no allow patterns** — an
/// *unscoped* facet. This is the single condition behind the uniform
/// empty-scope refusal ([`NoSignalReason::Unscoped`]): neither git nor mtime
/// diffs or enumerates the whole medium for such a facet, and refinement emits
/// no batch for it. It is orthogonal to the ingest's `deny_paths` — an empty
/// deny list is not an unscoped facet.
fn facet_unscoped(source: &Source) -> bool {
    !source.scope.iter().any(|r| r.mode == PatternMode::Allow)
}

/// Compute the git changed slice for one primary source between its stored
/// baseline commit and the tree's current `HEAD`. Mirrors `computeGitSlice`.
fn compute_git_slice(
    source: &Source,
    deny_paths: &[String],
    workspace_root: &Path,
    baseline: Option<&str>,
) -> SliceOutcome {
    let base = medium_base(&source.pointer, workspace_root);
    let Some(git_root) = find_git_root(&base) else {
        return SliceOutcome::NoSignal {
            reason: NoSignalReason::GitUnavailable,
        };
    };
    let Some(head) = git_head(&git_root) else {
        return SliceOutcome::NoSignal {
            reason: NoSignalReason::GitUnavailable,
        };
    };

    let baseline = match baseline {
        Some(b) if is_git_token(b) => b,
        // No usable commit baseline — seed at HEAD, present no slice.
        _ => return SliceOutcome::Reseed { token: head },
    };
    if baseline == head {
        return SliceOutcome::Unchanged { token: head };
    }
    // A git-shaped baseline that THIS repo does not contain is not a usable
    // baseline either — it is foreign (seeded when the pointer resolved to a
    // different repo, e.g. before a source tree moved into a submodule) or
    // gone (gc'd / rewritten away). Diffing against it fatals, which used to
    // degrade every pass into `GitUnavailable` — a baseline that never seats
    // and a binding that never backs off. Reseed at HEAD instead: one honest
    // full re-roam, then normal change detection. `GitUnavailable` below is
    // reserved for transient git failures on a baseline that does exist.
    if !commit_exists(&git_root, baseline) {
        return SliceOutcome::Reseed { token: head };
    }

    // Pathspecs from the facet scope + the ingest's deny_paths.
    let mut allows: Vec<&str> = Vec::new();
    let mut scope_denies: Vec<&str> = Vec::new();
    for rule in &source.scope {
        match rule.mode {
            PatternMode::Allow => allows.push(&rule.path),
            PatternMode::Deny => scope_denies.push(&rule.path),
        }
    }
    if allows.is_empty() {
        // Unscoped facet — the uniform typed refusal (never diff the whole
        // repo); renders in the brief rather than degrading silently.
        return SliceOutcome::NoSignal {
            reason: NoSignalReason::Unscoped,
        };
    }
    let mut ws_denies: Vec<&str> = deny_paths.iter().map(String::as_str).collect();
    // Engine self-exclusion — same forced set the mtime strategy's
    // enumeration applies, pushed down as exclude pathspecs so the
    // slice never names engine state either.
    let forced = engine_state_denies(workspace_root);
    for f in &forced {
        ws_denies.push(f);
    }
    let mut specs: Vec<String> =
        Vec::with_capacity(allows.len() + scope_denies.len() + ws_denies.len());
    // Scope patterns are source-relative, so they re-anchor against the medium
    // base — the same join the mtime enumeration performs. Ingest denies below
    // stay workspace-rooted; the two namespaces are per-source and per-binding
    // respectively, not two dialects for one thing.
    for a in &allows {
        specs.push(to_git_pathspec(a, &git_root, &base, false));
    }
    // A deny may target a path OUTSIDE this medium's git repo — a
    // cross-medium workspace-relative glob such as `../dev/**`, whose tree
    // lives in a sibling repo. Git *fatals* on an out-of-tree pathspec
    // (`'../dev/**' is outside repository`), which would sink the entire
    // diff into a no-signal degrade. Such a deny can exclude nothing here
    // anyway (the files simply aren't in this repo), so drop it: the plugin
    // hook still enforces it agent-side (workspace-relative, cross-repo),
    // and a genuinely-dead entry is still surfaced by the brief warning.
    for d in &scope_denies {
        if let Some(spec) = in_repo_pathspec(d, &git_root, &base, true) {
            specs.push(spec);
        }
    }
    for d in &ws_denies {
        if let Some(spec) = in_repo_pathspec(d, &git_root, workspace_root, true) {
            specs.push(spec);
        }
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
        _ => {
            return SliceOutcome::NoSignal {
                reason: NoSignalReason::GitUnavailable,
            };
        }
    };
    let text = String::from_utf8_lossy(&out.stdout);

    // The exclude pathspecs above are git's own glob dialect and cannot
    // express the two rules that extend the engine's deny resolution: the
    // literal-base directory-prefix block, and the fallback that keeps a
    // MALFORMED entry blocking by its literal prefix rather than dropping it.
    // Pushing what git understands and then filtering the result through the
    // same `DenyOracle` the mtime enumeration and the path-check command use
    // is what makes deny enforcement genuinely strategy-invariant — before
    // this, a `broken[` entry excluded a file from `S(D)` and left it in the
    // git slice.
    let ws_oracle = memstead_base::check_path::DenyOracle::new(
        &ws_denies
            .iter()
            .map(|d| (*d).to_string())
            .collect::<Vec<_>>(),
    );

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
        if ws_oracle.is_denied(&ws_path) {
            continue;
        }
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
/// Restrict a graph changed slice to the facet's scope. Without this the
/// selector was honoured by enumeration and ignored by change detection, so a
/// brief could print `Entities: type:concept` and then hand the agent a
/// changed `memo` two sections below — an artifact its own coverage model
/// says is out of scope, which `advance` would then accept because its gate
/// is the presented slice.
///
/// Added and modified entities are classified from the live store. **Deleted
/// entities are kept unconditionally**: the entity is gone, so its type can no
/// longer be read, and a deletion that cannot be classified must be reported
/// rather than dropped — a missed deletion is the highest-signal drift there
/// is. An `id:` selector still applies to deletions, because an id is all a
/// deletion leaves behind.
fn filter_graph_slice_to_scope(engine: &Engine, source: &Source, slice: &mut Slice) {
    let mut allows: Vec<EntitySelector> = Vec::new();
    let mut denies: Vec<EntitySelector> = Vec::new();
    for rule in &source.scope {
        let Some(sel) = parse_entity_selector(&rule.path) else {
            continue;
        };
        match rule.mode {
            PatternMode::Allow => allows.push(sel),
            PatternMode::Deny => denies.push(sel),
        }
    }
    if allows.is_empty() {
        return;
    }
    let in_scope = |id: &str, known_type: Option<&str>| {
        // A `type:` selector cannot judge an entity whose type is unreadable
        // (a deletion). Treat it as matching so the artifact survives to be
        // reported, rather than silently vanishing from the slice.
        let matches = |s: &EntitySelector| match (s, known_type) {
            (EntitySelector::Type(_), None) => true,
            _ => selector_matches(s, id, known_type.unwrap_or_default()),
        };
        allows.iter().any(&matches) && !denies.iter().any(&matches)
    };
    let type_of = |id: &str| {
        engine
            .store()
            .get(&memstead_base::entity::EntityId::canonical(id))
            .map(|e| e.entity_type.clone())
    };
    slice
        .added
        .retain(|id| in_scope(id, type_of(id).as_deref()));
    slice
        .modified
        .retain(|id| in_scope(id, type_of(id).as_deref()));
    slice.deleted.retain(|id| in_scope(id, None));
}

fn compute_graph_slice(
    engine: &Engine,
    source: Option<&Source>,
    source_mem: &str,
    baseline: Option<&str>,
) -> SliceOutcome {
    let current = match engine.mem_head_sha(source_mem) {
        Ok(Some(sha)) => sha,
        // Source has no snapshot signal, or is unknown — degrade.
        _ => {
            return SliceOutcome::NoSignal {
                reason: NoSignalReason::GraphSnapshotMissing,
            };
        }
    };
    // Fetch the entity delta only when the source actually moved.
    let changed = matches!(baseline, Some(b) if is_git_token(b) && b != current);
    let mut outcome = if changed {
        let baseline = baseline.expect("changed implies a baseline");
        match engine.changes_since(source_mem, baseline, None) {
            Ok(report) => graph_slice_outcome(Some(baseline), &current, &report.changes),
            // Unknown baseline / engine error — degrade.
            Err(_) => SliceOutcome::NoSignal {
                reason: NoSignalReason::GraphSnapshotMissing,
            },
        }
    } else {
        graph_slice_outcome(baseline, &current, &[])
    };
    // The scope narrows the slice exactly as it narrows S(D). Applied after
    // the diff rather than pushed into it, mirroring how the path strategies
    // apply deny pathspecs — one place decides what "in scope" means.
    // A reference mem carries no facet scope — it is read whole by design,
    // so there is nothing to narrow by and `None` is the honest input.
    if let (Some(source), SliceOutcome::Changed { slice, .. }) = (source, &mut outcome) {
        filter_graph_slice_to_scope(engine, source, slice);
    }
    outcome
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

/// Directory names never worth walking for the dead-deny scan — build output,
/// VCS metadata ([`VCS_INTERNAL_DIRS`]), dependency caches, and the engine's
/// own cache.
const DEAD_DENY_SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    ".memstead.cache",
    ".sqlx",
    ".svn",
    ".hg",
];

/// Bounded, pruned walk of `base` collecting every file's **workspace-relative**
/// path (the same string space the deny globs match). Skips heavy directories
/// ([`DEAD_DENY_SKIP_DIRS`]) and gives up (returns `None`) past `cap` files, so
/// the dead-deny scan degrades to "can't tell" rather than warning falsely or
/// walking an unbounded tree. Best-effort: unreadable directories are skipped.
fn walk_tree_bounded(base: &Path, workspace_root: &Path, cap: usize) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![base.to_path_buf()];
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
                let skip = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| DEAD_DENY_SKIP_DIRS.contains(&n));
                if !skip {
                    stack.push(path);
                }
            } else if file_type.is_file() {
                if out.len() >= cap {
                    return None;
                }
                out.push(
                    relative_path(workspace_root, &normalize_lexical(&path))
                        .to_string_lossy()
                        .to_string(),
                );
            }
        }
    }
    Some(out)
}

/// The ingest `deny_paths` entries that select **no file** in the project tree
/// — surfaced as a rendered brief warning (AC 6 refusal leg) so a zero-matching
/// deny is never a silent no-op. Resolution base is the medium's git project
/// root (so a cross-medium workspace-relative deny like `../dev/**`, whose
/// target lives outside a sub-medium, still resolves against real files),
/// falling back to the workspace root. Answers through the *same*
/// [`memstead_base::check_path::DenyOracle`] the strategies and the path-check command
/// use, so "does this deny select anything" is decided by the resolution that
/// actually enforces it — including the literal-base directory-prefix rule and
/// the malformed-entry fallback. Answering with the raw glob alone told an
/// author that a working bare-name entry (`dev`) matched nothing and invited
/// them to delete it, which would have raised the denominator.
/// Best-effort: if the tree can't be enumerated
/// (walk cap hit, no readable base) nothing is reported — a warning is only
/// ever raised on a confirmed zero-match.
///
/// The scaffold's own default hygiene entries
/// ([`memstead_base::binding::DEFAULT_SCAFFOLD_DENY_PATHS`]) are exempt: `projection
/// init` writes them into every codebase/filesystem binding, and most trees
/// carry none of the debris they name. The exemption is a membership check
/// against that constant, not a prediction about what the enumerator walks —
/// enumeration is a filesystem walk even on a git-signalled source, so these
/// entries CAN match (deleting `**/node_modules/**` from a scaffolded record
/// over a repo that gitignores `node_modules/` raises the denominator). The
/// engine never calls its own output a typo; a user-authored entry that
/// matches nothing keeps the loud warning.
fn dead_deny_entries(resolved: &ResolvedIngest, workspace_root: &Path) -> Vec<String> {
    if resolved.deny_paths.is_empty() {
        return Vec::new();
    }
    let base = find_git_root(workspace_root).unwrap_or_else(|| workspace_root.to_path_buf());
    let Some(files) = walk_tree_bounded(&base, workspace_root, 100_000) else {
        return Vec::new();
    };
    let mut dead: Vec<String> = Vec::new();
    for entry in &resolved.deny_paths {
        if memstead_base::binding::DEFAULT_SCAFFOLD_DENY_PATHS.contains(&entry.as_str()) {
            continue;
        }
        let oracle = memstead_base::check_path::DenyOracle::new(std::slice::from_ref(entry));
        if oracle.is_empty() {
            // An empty entry resolves to nothing either way — not a confirmed
            // zero-match, so it is not reported here.
            continue;
        }
        if !files.iter().any(|f| oracle.is_denied(f)) {
            dead.push(entry.clone());
        }
    }
    dead
}

/// Compute the `mtime` changed slice for one primary source: enumerate the
/// facet files, stat them, memoise the current map, and diff against the
/// baseline digest's memoised map (precise) or degrade to a full scan on memo
/// miss. Mirrors the mtime branch of the plugin's `computeSourceCursor`.
fn compute_mtime_slice(
    source: &Source,
    ingest_name: &str,
    deny_paths: &[String],
    workspace_root: &Path,
    cache_root: &Path,
    baseline: Option<&str>,
) -> SliceOutcome {
    if facet_unscoped(source) {
        // Unscoped facet — the same typed refusal git raises, so the mtime
        // strategy never enumerates the whole medium nor emits an empty slice.
        return SliceOutcome::NoSignal {
            reason: NoSignalReason::Unscoped,
        };
    }
    let files = enumerate_facet_files(source, deny_paths, workspace_root);
    let now_map = compute_stat_map(&files, workspace_root);
    let now_digest = digest_stat_map(&now_map);
    write_cursor_memo(
        cache_root,
        ingest_name,
        &source.name,
        &now_digest.aggregate,
        &now_map,
    );
    let prev_map = baseline
        .and_then(parse_digest_token)
        .and_then(|base| read_cursor_memo(cache_root, ingest_name, &source.name, &base.aggregate));
    mtime_slice_outcome(baseline, prev_map.as_ref(), &now_map)
}

/// The current change-detection token for a primary source, per its resolved
/// strategy: git `HEAD`, the graph mem's snapshot, or the freshly-computed
/// mtime digest. `None` when there is no signal.
fn current_primary_token(
    engine: &Engine,
    source: &Source,
    deny_paths: &[String],
    workspace_root: &Path,
) -> Option<String> {
    match resolve_change_strategy(source, workspace_root) {
        ChangeStrategy::Git => git_head(&find_git_root(&medium_base(
            &source.pointer,
            workspace_root,
        ))?),
        ChangeStrategy::Graph => {
            if facet_unscoped(source) {
                // Symmetric with the mtime arm: no signal at all, rather than
                // a whole-mem token posing as a scoped one.
                None
            } else {
                engine.mem_head_sha(&source.pointer).ok().flatten()
            }
        }
        ChangeStrategy::Mtime => {
            if facet_unscoped(source) {
                // Unscoped facet has no signal — not an empty-set digest posing
                // as one, so the source can never register as "moved".
                None
            } else {
                let files = enumerate_facet_files(source, deny_paths, workspace_root);
                Some(serialize_digest_token(&digest_stat_map(&compute_stat_map(
                    &files,
                    workspace_root,
                ))))
            }
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
    source_moved_since(engine, resolved, workspace_root, "synced", false)
}

/// The generalized form of [`source_moved`]: compare each source's current
/// change-detection token against the baseline stored under
/// `"<binding>/<facet>#<state>"` in the destination mem's `sync_state`. The
/// `state` suffix selects the baseline family — `"synced"` (the build/sync
/// baseline [`source_moved`] reads) or `"verified"` (the verify baseline).
///
/// `missing_baseline_is_moved` decides the never-recorded case: `false`
/// preserves [`source_moved`]'s posture (no baseline ⇒ not "moved" — a first
/// sync does not by itself defeat backoff); `true` treats a source with a live
/// current token but no recorded baseline as moved — the verify due-check's
/// posture, where "never verified" means the first verify is due.
pub fn source_moved_since(
    engine: &Engine,
    resolved: &ResolvedIngest,
    workspace_root: &Path,
    state: &str,
    missing_baseline_is_moved: bool,
) -> bool {
    let dest = &resolved.destination_mem;
    let baseline_map = engine
        .mem_config_for(dest)
        .map(|c| c.sync_state.clone())
        .unwrap_or_default();

    for source in &resolved.sources {
        let (facet_ref, current) = match source {
            ResolvedSource::Primary(p) => (
                p.name.clone(),
                current_primary_token(engine, p, &resolved.deny_paths, workspace_root),
            ),
            ResolvedSource::Reference { mem } => {
                (mem.clone(), engine.mem_head_sha(mem).ok().flatten())
            }
        };
        let key = format!("{}/{}#{state}", resolved.name, facet_ref);
        let Some(baseline) = baseline_map.get(&key) else {
            // No baseline recorded for this state family.
            if missing_baseline_is_moved && current.as_deref().is_some_and(|c| !c.is_empty()) {
                return true;
            }
            continue;
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
    let mut no_signal: Vec<NoSignalNote> = Vec::new();
    let mut delivery: Vec<DeliverySequence> = Vec::new();
    let mut degraded = false;
    // Units already disposed in an in-progress pass (touchpoint B): the
    // sequence counts them and presents the next ones in order. Read once,
    // lazily — a binding without a delivery source never touches the store.
    let disposed_units: std::cell::OnceCell<BTreeSet<String>> = std::cell::OnceCell::new();
    let disposed_units = || {
        disposed_units.get_or_init(|| {
            resolved
                .name
                .split_once('/')
                .and_then(|(mem, name)| {
                    super::advance::read_advance_store(workspace_root, mem, name)
                        .ok()
                        .flatten()
                })
                .map(|state| state.dispositions.keys().cloned().collect())
                .unwrap_or_default()
        })
    };

    for source in &resolved.sources {
        // Key: "<ingest>/<facet_ref>" for primaries, "<ingest>/<mem>" for
        // reference sources — matching the plugin's sync_state keying.
        // The note's remedy is medium-shaped, so the medium travels with it.
        let primary_medium = match source {
            ResolvedSource::Primary(p) => Some(p.medium_type),
            ResolvedSource::Reference { .. } => None,
        };
        let (facet_ref, outcome) = match source {
            ResolvedSource::Primary(p) => {
                let key = format!("{}/{}#synced", resolved.name, p.name);
                let baseline = baseline_map.get(&key).map(String::as_str);
                let outcome = match resolve_change_strategy(p, workspace_root) {
                    ChangeStrategy::Git => {
                        compute_git_slice(p, &resolved.deny_paths, workspace_root, baseline)
                    }
                    // A graph-typed primary's medium pointer is the source mem id.
                    // An unscoped graph facet refuses exactly as the git and
                    // mtime arms do: the graph slice alone used to proceed on
                    // an empty scope, which is how a facet could carry scope
                    // nothing interpreted and still look like it was working.
                    ChangeStrategy::Graph if facet_unscoped(p) => SliceOutcome::NoSignal {
                        reason: NoSignalReason::Unscoped,
                    },
                    ChangeStrategy::Graph => {
                        compute_graph_slice(engine, Some(p), &p.pointer, baseline)
                    }
                    ChangeStrategy::Mtime => compute_mtime_slice(
                        p,
                        &resolved.name,
                        &resolved.deny_paths,
                        workspace_root,
                        &cache_root,
                        baseline,
                    ),
                    // `none` is inert — a rendered `signal:none` state, no slice.
                    ChangeStrategy::None => SliceOutcome::NoSignal {
                        reason: NoSignalReason::DetectionNone,
                    },
                };
                // Touchpoint B: a source declaring a delivery preparation
                // delivers units in its own total order instead of files. A
                // source declaring none keeps the file-granularity outcome
                // computed above, byte-for-byte.
                let outcome = match memstead_base::preparation::delivery_preparation(
                    p.preparation.as_deref(),
                ) {
                    Some(prep)
                        if matches!(
                            p.medium_type,
                            MediumType::Codebase | MediumType::Filesystem | MediumType::Git
                        ) =>
                    {
                        let (outcome, sequence) = deliver_units(
                            p,
                            prep.id,
                            &resolved.deny_paths,
                            workspace_root,
                            baseline,
                            resolved.batch_size as usize,
                            disposed_units(),
                            outcome,
                        );
                        delivery.extend(sequence);
                        outcome
                    }
                    _ => outcome,
                };
                (p.name.clone(), outcome)
            }
            ResolvedSource::Reference { mem } => {
                let key = format!("{}/{}#synced", resolved.name, mem);
                let baseline = baseline_map.get(&key).map(String::as_str);
                (
                    mem.clone(),
                    compute_graph_slice(engine, None, mem, baseline),
                )
            }
        };

        let key = format!("{}/{}#synced", resolved.name, facet_ref);
        match outcome {
            // Genuinely unchanged (baseline present, nothing moved) is the only
            // documented silence — it renders nothing, keeping an all-unchanged
            // brief byte-identical to a plain roam.
            SliceOutcome::Unchanged { .. } => {}
            // Every no-signal reason is a visible per-source note.
            SliceOutcome::NoSignal { reason } => no_signal.push(NoSignalNote {
                source: facet_ref.clone(),
                reason,
                medium_type: primary_medium,
            }),
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
        no_signal,
        any_changes,
        degraded,
        dead_denies: dead_deny_entries(resolved, workspace_root),
        delivery,
        dest_mem: dest.clone(),
        // The resolved ingest's `name` is the canonical binding id `<mem>/<stem>`
        // (via `resolve_binding_run`) — the id the `projection advance` line the
        // brief renders (D4/D7) is keyed on.
        binding_id: resolved.name.clone(),
    }
}

fn dedupe_sort(v: &mut Vec<String>) {
    v.sort();
    v.dedup();
}

/// A workspace-relative source file's text (lossy for non-UTF-8 bytes).
fn read_workspace_file(workspace_root: &Path, ws_rel: &str) -> Option<String> {
    std::fs::read(workspace_root.join(ws_rel))
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// The text of a source file as it stood at the git baseline commit —
/// `git show <baseline>:<repo-relative path>` — when the source resolves to
/// the git strategy and the baseline is a commit of its repo. `None`
/// otherwise: the caller then has no old state to diff units against and
/// degrades to whole-file units, saying so.
fn git_baseline_content(
    source: &Source,
    workspace_root: &Path,
    baseline: Option<&str>,
    ws_rel: &str,
) -> Option<String> {
    let baseline = baseline.filter(|b| is_git_token(b))?;
    if !matches!(
        resolve_change_strategy(source, workspace_root),
        ChangeStrategy::Git
    ) {
        return None;
    }
    let git_root = find_git_root(&medium_base(&source.pointer, workspace_root))?;
    let abs = normalize_lexical(&workspace_root.join(ws_rel));
    let rel = relative_path(&git_root, &abs);
    let spec = format!("{baseline}:{}", rel.to_string_lossy().replace('\\', "/"));
    let out = Command::new("git")
        .args(["show", &spec])
        .current_dir(&git_root)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The text of a changed artifact at the binding's sync baseline, whichever
/// primary facet holds it: each git-strategy facet is asked in declaration
/// order with its own `#synced` token, the first retrievable content wins.
/// `None` when no facet can produce one (a non-git facet, no baseline, an
/// added file).
pub(crate) fn baseline_content_for(
    engine: &Engine,
    resolved: &ResolvedIngest,
    workspace_root: &Path,
    ws_rel: &str,
) -> Option<String> {
    let baseline_map = engine
        .mem_config_for(&resolved.destination_mem)
        .map(|c| c.sync_state.clone())
        .unwrap_or_default();
    resolved.sources.iter().find_map(|source| match source {
        ResolvedSource::Primary(p) => {
            let key = format!("{}/{}#synced", resolved.name, p.name);
            let baseline = baseline_map.get(&key).map(String::as_str);
            git_baseline_content(p, workspace_root, baseline, ws_rel)
        }
        ResolvedSource::Reference { .. } => None,
    })
}

/// The symbols a source text defines, read lexically: the name after a
/// definition keyword (`fn`, `struct`, `enum`, `trait`, `type`, `const`,
/// `static`, `mod`, `macro_rules!`, and the `function`, `class`,
/// `interface`, `def` of other languages), plus the variants of every
/// `enum` block (a line inside it that opens with a capitalised identifier).
/// No parser and no language table: a definition is a keyword followed by
/// an identifier, which is what a reader scanning a diff hunk sees too.
pub(crate) fn defined_symbols(text: &str) -> BTreeSet<String> {
    const KEYWORDS: &[&str] = &[
        "fn",
        "struct",
        "enum",
        "trait",
        "type",
        "const",
        "static",
        "mod",
        "function",
        "class",
        "interface",
        "def",
    ];
    fn ident(s: &str) -> Option<&str> {
        let end = s
            .char_indices()
            .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        let name = &s[..end];
        (!name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit())).then_some(name)
    }
    let mut out = BTreeSet::new();
    // (indent of the `enum` line) while inside an enum block.
    let mut enum_indent: Option<usize> = None;
    for raw in text.lines() {
        let indent = raw.len() - raw.trim_start().len();
        let line = raw.trim();
        if let Some(open) = enum_indent {
            if line.starts_with('}') && indent <= open {
                enum_indent = None;
            } else if indent > open
                && line.starts_with(|c: char| c.is_ascii_uppercase())
                && let Some(name) = ident(line)
                && line[name.len()..]
                    .trim_start()
                    .starts_with([',', '{', '(', '='])
            {
                out.insert(name.to_string());
            }
        }
        let mut words = line
            .split(|c: char| c.is_whitespace() || c == '(')
            .filter(|w| !w.is_empty());
        while let Some(word) = words.next() {
            let word = word.trim_end_matches('!');
            if KEYWORDS.contains(&word)
                && let Some(next) = words.next()
                && let Some(name) = ident(next)
            {
                out.insert(name.to_string());
                if word == "enum" && line.ends_with('{') {
                    enum_indent = Some(indent);
                }
                break;
            }
        }
    }
    out
}

/// The symbols a change to one artifact defines or removes: the defined set
/// of the current text against the defined set at the baseline (an added
/// file defines all of its symbols, a deleted file removes all of its
/// baseline symbols). Empty when neither text is readable.
fn changed_symbols(
    engine: &Engine,
    resolved: &ResolvedIngest,
    workspace_root: &Path,
    ws_rel: &str,
) -> BTreeSet<String> {
    let now = read_workspace_file(workspace_root, ws_rel)
        .map(|t| defined_symbols(&t))
        .unwrap_or_default();
    let old = baseline_content_for(engine, resolved, workspace_root, ws_rel)
        .map(|t| defined_symbols(&t))
        .unwrap_or_default();
    now.symmetric_difference(&old).cloned().collect()
}

/// Symbol names too ubiquitous to steer by: the standard trait methods and
/// constructor idioms nearly every Rust file defines, which a body names for
/// its own type and never for the changed file's.
const GENERIC_SYMBOLS: &[&str] = &[
    "new",
    "default",
    "from",
    "into",
    "clone",
    "fmt",
    "drop",
    "hash",
    "eq",
    "cmp",
    "len",
    "iter",
    "next",
    "get",
    "set",
    "push",
    "insert",
    "remove",
    "build",
    "main",
    "run",
    "call",
    "read",
    "write",
    "open",
    "close",
    "init",
    "load",
    "save",
    "parse",
    "render",
    "resolve",
    "apply",
    "with",
    "none",
    "some",
    "self",
    "this",
    "test",
    "tests",
    "error",
    "kind",
    "name",
    "path",
    "value",
    "state",
    "config",
    "engine",
    "entity",
    "id",
    "code",
    "message",
    "text",
    "body",
    "title",
    "section",
    "sections",
    "type",
    "mem",
    "key",
    "keys",
    "as_str",
    "to_string",
    "as_ref",
    "borrow",
    "deref",
    "index",
    "try_from",
    "try_into",
    "partial_cmp",
    "serialize",
    "deserialize",
    "display",
    "debug",
];

/// The destination entities each changed artifact steers (the claims-in-
/// sight move): the entities anchoring it, and the entities naming it by
/// path or by a symbol its change defines or removes without anchoring it.
/// Built from entity bodies at call time — fenced code masked, the path
/// tokens resolved under the binding's source join exactly as the verify
/// pass resolves an unanchored mention, the symbol tokens read from inline
/// code spans. An entity that anchors the artifact is listed once, under
/// anchors. Lexical and deterministic: no embedding, no model call.
pub fn steered_entities(
    engine: &Engine,
    resolved: &ResolvedIngest,
    workspace_root: &Path,
    slice: &Slice,
) -> super::brief::SteeredEntities {
    use super::brief::{MentionKind, MentionRow};
    use super::findings::{code_span_symbols, path_tokens, resolve_mention};

    let mut artifacts: Vec<String> = slice
        .added
        .iter()
        .chain(slice.modified.iter())
        .chain(slice.deleted.iter())
        .map(|a| memstead_base::preparation::split_unit_id(a).0.to_string())
        .collect();
    artifacts.sort();
    artifacts.dedup();
    if artifacts.is_empty() {
        return super::brief::SteeredEntities::new();
    }
    let artifact_set: BTreeSet<String> = artifacts.iter().cloned().collect();
    let pointers: Vec<String> = resolved
        .sources
        .iter()
        .filter_map(|s| match s {
            ResolvedSource::Primary(p) => Some(p.pointer.clone()),
            ResolvedSource::Reference { .. } => None,
        })
        .collect();

    let mut out = super::brief::SteeredEntities::new();
    let dest = resolved.destination_mem.as_str();
    for artifact in &artifacts {
        let mut anchored: Vec<String> = engine
            .anchors_referencing_artifact(artifact)
            .into_iter()
            .filter(|(eid, _)| eid.mem() == dest)
            .map(|(eid, _)| eid.as_ref().to_string())
            .collect();
        anchored.sort();
        anchored.dedup();
        if !anchored.is_empty() {
            out.entry(artifact.clone()).or_default().anchored = anchored;
        }
    }

    // Symbol sets are computed once per artifact, and only when some body
    // carries an inline code span at all (the common case on a prose mem).
    let mut symbols: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for entity in engine.store().all_entities() {
        if entity.mem != dest || entity.stub {
            continue;
        }
        let eid = entity.id.to_string();
        for (section, body) in &entity.sections {
            let masked = memstead_base::markdown::mask_code_blocks(body);
            let mut hits: Vec<(String, MentionKind)> = Vec::new();
            for token in path_tokens(&masked) {
                if let Some(artifact) = resolve_mention(&token, &pointers, &artifact_set) {
                    hits.push((artifact, MentionKind::Path));
                }
            }
            // A symbol worth steering by is one a reader would recognise as
            // this change's: the ubiquitous trait and constructor names
            // (`new`, `default`, `fmt`, …) and anything under four characters
            // name too many things to point at one.
            let spans: Vec<String> = code_span_symbols(body)
                .into_iter()
                .filter(|s| s.len() >= 4 && !GENERIC_SYMBOLS.contains(&s.as_str()))
                .collect();
            if !spans.is_empty() {
                for artifact in &artifacts {
                    let defined = symbols.entry(artifact.as_str()).or_insert_with(|| {
                        changed_symbols(engine, resolved, workspace_root, artifact)
                    });
                    for sym in &spans {
                        if defined.contains(sym) {
                            hits.push((artifact.clone(), MentionKind::Symbol(sym.clone())));
                        }
                    }
                }
            }
            for (artifact, kind) in hits {
                let entry = out.entry(artifact).or_default();
                if entry.anchored.contains(&eid) {
                    continue;
                }
                let row = MentionRow {
                    entity: eid.clone(),
                    kind,
                    section: section.clone(),
                };
                // One row per (entity, kind): the first section wins.
                if !entry
                    .mentioned
                    .iter()
                    .any(|r| r.entity == row.entity && r.kind == row.kind)
                {
                    entry.mentioned.push(row);
                }
            }
        }
    }
    for e in out.values_mut() {
        e.mentioned.sort();
    }
    out.retain(|_, e| !e.anchored.is_empty() || !e.mentioned.is_empty());
    out
}

/// The total order of a delivery sequence: the units' own order keys first,
/// then the path, then the same-stamp ordinal NUMERICALLY (`.2` before
/// `.10`: an unpadded ordinal compared as text would deliver the tenth entry
/// of a day before the second), then the key as text — never the order the
/// files were discovered in. The same set of units sorts identically however
/// it was collected.
pub(crate) fn sequence_units(units: &mut Vec<DeliveredUnit>) {
    fn rank(id: &str) -> (&str, u64, &str) {
        let (path, key) = memstead_base::preparation::split_unit_id(id);
        let key = key.unwrap_or("");
        let ordinal = key
            .rsplit_once('.')
            .and_then(|(_, n)| n.parse::<u64>().ok())
            .unwrap_or(1);
        (path, ordinal, key)
    }
    units.sort_by(|a, b| (&a.order_key, rank(&a.id)).cmp(&(&b.order_key, rank(&b.id))));
    units.dedup_by(|a, b| a.id == b.id);
}

/// Touchpoint B: turn a delivery-prepared source's file-level outcome into
/// its unit sequence. A first run (`Reseed`) delivers every unit of every
/// in-scope file; a change run (`Changed`) delivers the units of added files,
/// the units that differ in modified files (diffed against the git baseline
/// content; without one, every unit of the file, flagged degraded), and the
/// baseline's units of deleted files (a deleted file with no retrievable
/// baseline stays a file-level deletion). The units sort into the total
/// order `(order key, id)`, the same on every pass; units already disposed in
/// the in-progress advance store are marked so the brief presents the next
/// ones. The unit ids replace the file ids in the outcome's slice, so the
/// advance gate accepts every unit of the sequence (the brief lists the next
/// batch of them). Every other outcome passes through untouched.
#[allow(clippy::too_many_arguments)]
fn deliver_units(
    source: &Source,
    preparation: &str,
    deny_paths: &[String],
    workspace_root: &Path,
    baseline: Option<&str>,
    batch: usize,
    disposed: &BTreeSet<String>,
    outcome: SliceOutcome,
) -> (SliceOutcome, Option<DeliverySequence>) {
    use memstead_base::preparation::{DeliveryUnit, UnitChange, diff_units, unit_id, unitize};

    let units_of =
        |text: &str| -> Vec<DeliveryUnit> { unitize(preparation, text).unwrap_or_default() };
    let delivered = |path: &str, u: &DeliveryUnit, change: UnitChange| DeliveredUnit {
        id: unit_id(path, &u.key),
        order_key: u.order_key.clone(),
        change,
        disposed: false,
    };

    let mut units: Vec<DeliveredUnit> = Vec::new();
    let mut file_level_deleted: Vec<String> = Vec::new();
    let mut degraded_units = false;
    let (token, first_run, degraded) = match outcome {
        SliceOutcome::Reseed { token } => {
            for f in enumerate_facet_files(source, deny_paths, workspace_root) {
                if let Some(text) = read_workspace_file(workspace_root, &f) {
                    for u in units_of(&text) {
                        units.push(delivered(&f, &u, UnitChange::Added));
                    }
                }
            }
            if units.is_empty() {
                // Nothing in scope: the plain reseed, exactly as before.
                return (SliceOutcome::Reseed { token }, None);
            }
            (token, true, false)
        }
        SliceOutcome::Changed {
            token,
            slice,
            degraded,
        } => {
            for f in &slice.added {
                if let Some(text) = read_workspace_file(workspace_root, f) {
                    for u in units_of(&text) {
                        units.push(delivered(f, &u, UnitChange::Added));
                    }
                }
            }
            for f in &slice.modified {
                let Some(now) = read_workspace_file(workspace_root, f) else {
                    continue;
                };
                let new_units = units_of(&now);
                match git_baseline_content(source, workspace_root, baseline, f) {
                    Some(old) => {
                        for (u, change) in diff_units(&units_of(&old), &new_units) {
                            units.push(delivered(f, &u, change));
                        }
                    }
                    None => {
                        degraded_units = true;
                        for u in new_units {
                            units.push(delivered(f, &u, UnitChange::Modified));
                        }
                    }
                }
            }
            for f in &slice.deleted {
                match git_baseline_content(source, workspace_root, baseline, f) {
                    Some(old) => {
                        for u in units_of(&old) {
                            units.push(delivered(f, &u, UnitChange::Deleted));
                        }
                    }
                    None => file_level_deleted.push(f.clone()),
                }
            }
            (token, false, degraded)
        }
        other => return (other, None),
    };

    sequence_units(&mut units);
    for u in &mut units {
        u.disposed = disposed.contains(&u.id);
    }

    let mut slice = Slice::default();
    for u in &units {
        match u.change {
            UnitChange::Added => slice.added.push(u.id.clone()),
            UnitChange::Modified => slice.modified.push(u.id.clone()),
            UnitChange::Deleted => slice.deleted.push(u.id.clone()),
        }
    }
    slice.deleted.extend(file_level_deleted);
    dedupe_sort(&mut slice.added);
    dedupe_sort(&mut slice.modified);
    dedupe_sort(&mut slice.deleted);

    let sequence = DeliverySequence {
        source: source.name.clone(),
        preparation: preparation.to_string(),
        first_run,
        degraded: degraded_units,
        batch,
        units,
    };
    (
        SliceOutcome::Changed {
            token,
            slice,
            degraded,
        },
        Some(sequence),
    )
}

#[cfg(test)]
mod tests;
