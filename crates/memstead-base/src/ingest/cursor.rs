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
//! **One deny dialect.** A `deny_paths` entry is a **workspace-relative glob**
//! — the exact grammar and resolution root as a facet-scope entry, resolved by
//! the same [`build_glob_set`] / `:(glob,exclude)` machinery. The plugin's
//! PreToolUse deny hook enforces the *identical* dialect against the ingest
//! agent's Read/Glob/Grep by asking the engine itself: `projection
//! check-path` answers through [`super::check_path::check_deny_paths`], which
//! reads the active binding's record fresh on every call (the pointer channel
//! is [`super::check_path::write_active_binding_file`], published on
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
//! (D7). The driver never writes it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::Engine;
use crate::pipeline::{MediumType, PatternMode};

use super::brief::{DeliveredUnit, DeliverySequence, NoSignalNote, SourceCursor, SyncCommand};
use super::change_detection::{
    StatMap, compute_stat_map, digest_stat_map, parse_digest_token, serialize_digest_token,
};
use super::resolve::{
    ChangeStrategy, ResolvedIngest, ResolvedSource, find_git_root, resolve_change_strategy,
};
use super::slice::{
    NoSignalReason, Slice, SliceOutcome, graph_slice_outcome, is_git_token, mtime_slice_outcome,
};
use crate::pipeline::Source;

/// Lexically normalize a path — resolve `.` and `..` without touching the
/// filesystem (no symlink resolution), matching Node's `path.resolve` on an
/// already-absolute path.
pub(super) fn normalize_lexical(path: &Path) -> PathBuf {
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
pub(super) fn relative_path(from: &Path, to: &Path) -> PathBuf {
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

/// The medium pointer resolved to an absolute base directory. Public
/// so init-time surfaces (CLI `projection init`) can resolve a medium
/// base exactly as the strategies do — e.g. to warn when it falls
/// outside the workspace root.
pub fn medium_base(pointer: &str, workspace_root: &Path) -> PathBuf {
    if pointer.is_empty() {
        workspace_root.to_path_buf()
    } else {
        normalize_lexical(&workspace_root.join(pointer))
    }
}

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
/// resolution all work on it (measured on the dogfood's own out-of-root
/// bindings, where zero anchors orphan). What degrades rides the message —
/// `../…` artifact ids and a layout that must stay fixed — together with the
/// recipe that avoids it. Only path-namespace mediums can be out-of-root;
/// every other medium type yields `None`.
pub fn out_of_root_layout_warning(
    pointer: &str,
    workspace_root: &Path,
    medium_type: crate::pipeline::MediumType,
) -> Option<String> {
    use crate::pipeline::MediumType;
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

/// Workspace-relative deny globs excluding the engine's own state from
/// every strategy's input set. Unconditional and non-configurable: a
/// binding can never legitimately model `.memstead/`,
/// `.memstead.cache/`, or a mount's resolved storage location as
/// source artifacts — an allow glob covering them does not admit them.
/// The dot-directories key on their *names* (the names are the
/// contract, and a foreign workspace's `.memstead/` is still engine
/// state); the mount storage locations key on their *resolved* paths
/// because their directory names are configurable. Fail-open on an
/// unreadable mount list: the name-based excludes stay in force.
fn engine_state_denies(workspace_root: &Path) -> Vec<String> {
    use crate::workspace_store::{FileWorkspaceStore, WorkspaceStoreAdapter};

    let mut denies: Vec<String> = vec![
        ".memstead/**".to_string(),
        ".memstead.cache/**".to_string(),
        "**/.memstead/**".to_string(),
        "**/.memstead.cache/**".to_string(),
    ];
    if let Ok(ws) = FileWorkspaceStore.load(workspace_root) {
        for mount in &ws.mounts {
            let dir: Option<PathBuf> = match &mount.storage {
                crate::workspace::MountStorage::GitBranch { gitdir, .. } => {
                    gitdir.parent().map(Path::to_path_buf)
                }
                crate::workspace::MountStorage::Folder { path } => Some(path.clone()),
                crate::workspace::MountStorage::Archive { path, .. } => {
                    // A sealed archive is one file, not a tree.
                    let rel = relative_path(workspace_root, &normalize_lexical(path));
                    denies.push(rel.to_string_lossy().to_string());
                    None
                }
                // No on-disk footprint to exclude.
                crate::workspace::MountStorage::InMemory => None,
            };
            if let Some(dir) = dir {
                let rel = relative_path(workspace_root, &normalize_lexical(&dir));
                // A collapsed single-mem folder workspace stores the mem
                // AT the workspace root — excluding `**` there would
                // empty every denominator; skip it.
                if !rel.as_os_str().is_empty() {
                    denies.push(format!("{}/**", rel.to_string_lossy()));
                }
            }
        }
    }
    denies
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

/// Translate a workspace-relative facet pattern into a git pathspec relative
/// to `git_root`, with `:(glob)` magic (or `:(glob,exclude)` for a deny).
///
/// A `**`-prefixed pattern is prefix-free — it matches under any directory,
/// in particular the medium subtree — so it is emitted verbatim as a
/// git-root-relative glob. Lexically re-rooting it (join + relativize) would
/// produce `../**/…` for any non-root medium pointer, and git *fatals* on an
/// out-of-tree pathspec, sinking the whole diff into a no-signal degrade.
fn to_git_pathspec(pattern: &str, git_root: &Path, workspace_root: &Path, exclude: bool) -> String {
    let magic = if exclude {
        ":(glob,exclude)"
    } else {
        ":(glob)"
    };
    if pattern.starts_with("**") {
        return format!("{magic}{pattern}");
    }
    let resolved = normalize_lexical(&workspace_root.join(pattern));
    let git_rel = relative_path(git_root, &resolved);
    format!("{magic}{}", git_rel.to_string_lossy())
}

/// Like [`to_git_pathspec`], but `None` when the pattern resolves *outside*
/// `git_root` (its git-relative path escapes with a leading `..`). Git fatals
/// on an out-of-tree pathspec, so a cross-repo deny must be dropped from the
/// diff rather than pushed — it can match nothing in this repo regardless.
fn in_repo_pathspec(
    pattern: &str,
    git_root: &Path,
    workspace_root: &Path,
    exclude: bool,
) -> Option<String> {
    // Prefix-free glob — same verbatim re-anchoring as `to_git_pathspec`.
    if pattern.starts_with("**") {
        return Some(to_git_pathspec(pattern, git_root, workspace_root, exclude));
    }
    let resolved = normalize_lexical(&workspace_root.join(pattern));
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

/// Build a [`GlobSet`] from workspace-relative glob patterns, or `None` if
/// any pattern is malformed.
fn build_glob_set(patterns: &[&str]) -> Option<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).ok()?);
    }
    builder.build().ok()
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

/// Enumerate the workspace-relative file paths a primary source's facet scope
/// selects — the `mtime` strategy's input set. Mirrors the plugin's
/// `enumerateFacetFiles`: the path-shaped mediums (`codebase` / `filesystem` /
/// `git` — a git source's artifacts are paths pinned at a commit, so the walk
/// is identical and only the anchor namespace differs); the facet's
/// allow globs minus its deny globs, evaluated over the medium's directory
/// tree. Returns a sorted, de-duplicated list. An unscoped facet (no allows)
/// yields an empty list here — but callers must not treat that as signal: the
/// strategy layer (`compute_mtime_slice` / `current_primary_token`) refuses
/// an unscoped facet via `facet_unscoped` *before* enumerating, so the empty
/// list is only ever reached for a genuinely-empty scoped enumeration.
///
/// `deny_paths` are the ingest-level denies (`ResolvedIngest::deny_paths`),
/// applied on top of the facet's own scope denies with the *same*
/// workspace-relative glob grammar the git strategy pushes down as
/// `:(glob,exclude)` pathspecs — so a denied file is excluded from the mtime
/// input set exactly as it is from the git diff. Passing `&[]` yields the
/// facet-scope-only behaviour.
pub fn enumerate_facet_files(
    source: &Source,
    deny_paths: &[String],
    workspace_root: &Path,
) -> Vec<String> {
    if !matches!(
        source.medium_type,
        MediumType::Codebase | MediumType::Filesystem | MediumType::Git
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
    // Ingest deny_paths deny on top of the facet's own denies, sharing the
    // facet-scope glob grammar (workspace-relative, matched against each
    // candidate's workspace-relative path) — the same entries the git strategy
    // resolves as exclude pathspecs, so deny enforcement is strategy-invariant.
    for dp in deny_paths {
        denies.push(dp);
    }
    // Engine self-exclusion — unconditional, below configuration; the
    // git strategy pushes the same set as exclude pathspecs so the
    // denominator stays strategy-invariant.
    let forced = engine_state_denies(workspace_root);
    for f in &forced {
        denies.push(f);
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
    // workspace-relative path. VCS internals are never source artifacts —
    // they are pruned here so `.git/**` plumbing cannot enter `S(D)`,
    // matching the git strategy (whose diffs never name `.git` files).
    let base = medium_base(&source.pointer, workspace_root);
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
                let skip = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    // VCS internals and engine state are never source
                    // artifacts — pruning here saves the walk; the
                    // forced deny globs enforce the same exclusion for
                    // anything that still slips into a candidate list.
                    VCS_INTERNAL_DIRS.contains(&n) || n == ".memstead" || n == ".memstead.cache"
                });
                if !skip {
                    stack.push(path);
                }
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
    let mut denies: Vec<&str> = Vec::new();
    for rule in &source.scope {
        match rule.mode {
            PatternMode::Allow => allows.push(&rule.path),
            PatternMode::Deny => denies.push(&rule.path),
        }
    }
    if allows.is_empty() {
        // Unscoped facet — the uniform typed refusal (never diff the whole
        // repo); renders in the brief rather than degrading silently.
        return SliceOutcome::NoSignal {
            reason: NoSignalReason::Unscoped,
        };
    }
    for dp in deny_paths {
        denies.push(dp);
    }
    // Engine self-exclusion — same forced set the mtime strategy's
    // enumeration applies, pushed down as exclude pathspecs so the
    // slice never names engine state either.
    let forced = engine_state_denies(workspace_root);
    for f in &forced {
        denies.push(f);
    }
    let mut specs: Vec<String> = Vec::with_capacity(allows.len() + denies.len());
    for a in &allows {
        specs.push(to_git_pathspec(a, &git_root, workspace_root, false));
    }
    for d in &denies {
        // A deny may target a path OUTSIDE this medium's git repo — a
        // cross-medium workspace-relative glob such as `../dev/**`, whose tree
        // lives in a sibling repo. Git *fatals* on an out-of-tree pathspec
        // (`'../dev/**' is outside repository`), which would sink the entire
        // diff into a no-signal degrade. Such a deny can exclude nothing here
        // anyway (the files simply aren't in this repo), so drop it: the plugin
        // hook still enforces it agent-side (workspace-relative, cross-repo),
        // and a genuinely-dead entry is still surfaced by the brief warning.
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

/// One parsed entry of a **graph** facet's scope — the entity-namespace
/// counterpart of a path glob. A graph source selects entities, and an entity
/// is not a path: matching id-shaped globs against `mem--slug` invites the
/// "looks scoped, selects nothing" failure the dead-deny lint exists to catch
/// on paths, so the vocabulary is explicit about which axis it selects on.
///
/// Grammar (the whole of it):
///
/// - `*` — every entity in the source mem
/// - `type:<entity_type>` — entities of exactly that type
/// - `id:<glob>` — entities whose full `mem--slug` id matches the glob
///
/// Anything else is refused at binding validation
/// ([`crate::binding::validate_binding`]) rather than silently selecting
/// nothing: a scope nothing interprets is the defect, not a permissible form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitySelector {
    /// `*` — every entity in the mem.
    All,
    /// `type:<entity_type>` — exact type match.
    Type(String),
    /// `id:<glob>` — glob over the full entity id.
    Id(String),
}

/// Parse one graph scope pattern. `None` for an unrecognised form — the
/// caller decides whether that is a validation refusal (declaration time) or
/// a skipped rule (run time, already refused at declaration).
pub fn parse_entity_selector(pattern: &str) -> Option<EntitySelector> {
    let pattern = pattern.trim();
    if pattern == "*" {
        return Some(EntitySelector::All);
    }
    if let Some(rest) = pattern.strip_prefix("type:") {
        let rest = rest.trim();
        if rest.is_empty() {
            return None;
        }
        return Some(EntitySelector::Type(rest.to_string()));
    }
    if let Some(rest) = pattern.strip_prefix("id:") {
        let rest = rest.trim();
        if rest.is_empty() {
            return None;
        }
        // A malformed glob is a refusal, not a rule that matches nothing.
        Glob::new(rest).ok()?;
        return Some(EntitySelector::Id(rest.to_string()));
    }
    None
}

/// Does `selector` select this entity?
fn selector_matches(selector: &EntitySelector, id: &str, entity_type: &str) -> bool {
    match selector {
        EntitySelector::All => true,
        EntitySelector::Type(t) => entity_type == t,
        EntitySelector::Id(g) => Glob::new(g)
            .ok()
            .map(|glob| glob.compile_matcher().is_match(id))
            .unwrap_or(false),
    }
}

/// Enumerate the entity ids a **graph** source's facet scope selects — the
/// graph medium's `S(D)`, the exact counterpart of [`enumerate_facet_files`]
/// for a path medium. The source's `pointer` names the source mem; the store
/// already holds every mounted mem's entities, so this is a filter over
/// memory rather than any kind of walk.
///
/// Stubs are excluded: a stub is a placeholder the engine created for an
/// unresolved reference, not an authored source artifact. Counting them would
/// inflate the denominator with entities the source never wrote, making
/// coverage look worse than it is for a reason no author can act on.
///
/// An unscoped facet (no allow rules) yields an empty list here — callers must
/// not read that as "nothing in scope"; the strategy layer refuses an unscoped
/// facet before reaching this, exactly as it does for the path mediums.
pub fn enumerate_graph_entities(engine: &Engine, source: &Source) -> Vec<String> {
    if source.medium_type != MediumType::Graph {
        return Vec::new();
    }
    let mut allows: Vec<EntitySelector> = Vec::new();
    let mut denies: Vec<EntitySelector> = Vec::new();
    for rule in &source.scope {
        // An unparseable rule is already a validation refusal; at run time it
        // selects nothing rather than everything — a scope the engine cannot
        // read must never widen reach.
        let Some(sel) = parse_entity_selector(&rule.path) else {
            continue;
        };
        match rule.mode {
            PatternMode::Allow => allows.push(sel),
            PatternMode::Deny => denies.push(sel),
        }
    }
    if allows.is_empty() {
        return Vec::new();
    }
    let mem = source.pointer.as_str();
    let mut out: Vec<String> = Vec::new();
    for entity in engine.store().all_entities() {
        if entity.mem != mem || entity.stub {
            continue;
        }
        let id = entity.id.0.as_str();
        let ty = entity.entity_type.as_str();
        if !allows.iter().any(|s| selector_matches(s, id, ty)) {
            continue;
        }
        if denies.iter().any(|s| selector_matches(s, id, ty)) {
            continue;
        }
        out.push(id.to_string());
    }
    out.sort();
    out.dedup();
    out
}

/// Enumerate one primary source's in-scope artifacts, whatever its medium —
/// the single entry point every `S(D)` consumer uses. Path-shaped mediums
/// (codebase / filesystem / git) walk the file tree; a graph source filters
/// the source mem's entities. A medium the matrix marks non-enumerable
/// yields nothing, and its callers render the non-enumerable basis rather
/// than a denominator.
///
/// This exists because the enumeration bail was never in one place: five call
/// sites each repeated the same loop over `enumerate_facet_files`, so teaching
/// only the report about a new medium left the findings store, the refinement
/// rotation, and the exclude membership gate empty-handed.
pub fn enumerate_source_artifacts(
    engine: &Engine,
    source: &Source,
    deny_paths: &[String],
    workspace_root: &Path,
) -> Vec<String> {
    match source.medium_type {
        MediumType::Codebase | MediumType::Filesystem | MediumType::Git => {
            enumerate_facet_files(source, deny_paths, workspace_root)
        }
        MediumType::Graph => enumerate_graph_entities(engine, source),
        MediumType::Web => Vec::new(),
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
            .get(&crate::entity::EntityId::canonical(id))
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

/// VCS metadata directories — never source artifacts. Pruned from source
/// enumeration (`S(D)`, mtime slices, advance) and from the dead-deny scan.
const VCS_INTERNAL_DIRS: &[&str] = &[".git", ".svn", ".hg"];

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
/// falling back to the workspace root. Uses the *same* [`build_glob_set`]
/// matcher the strategies use, so "does this deny select anything" is answered
/// with the identical dialect. Best-effort: if the tree can't be enumerated
/// (walk cap hit, no readable base) nothing is reported — a warning is only
/// ever raised on a confirmed zero-match.
///
/// The scaffold's own default hygiene entries
/// ([`crate::binding::DEFAULT_SCAFFOLD_DENY_PATHS`]) are exempt: `projection
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
        if crate::binding::DEFAULT_SCAFFOLD_DENY_PATHS.contains(&entry.as_str()) {
            continue;
        }
        let Some(set) = build_glob_set(&[entry.as_str()]) else {
            // A malformed glob can't be resolved either way — not a confirmed
            // zero-match, so it is not reported here.
            continue;
        };
        if !files.iter().any(|f| set.is_match(f)) {
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
                let outcome =
                    match crate::preparation::delivery_preparation(p.preparation.as_deref()) {
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

/// The total order of a delivery sequence: the units' own order keys first,
/// then the path, then the same-stamp ordinal NUMERICALLY (`.2` before
/// `.10`: an unpadded ordinal compared as text would deliver the tenth entry
/// of a day before the second), then the key as text — never the order the
/// files were discovered in. The same set of units sorts identically however
/// it was collected.
pub(crate) fn sequence_units(units: &mut Vec<DeliveredUnit>) {
    fn rank(id: &str) -> (&str, u64, &str) {
        let (path, key) = crate::preparation::split_unit_id(id);
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
    use crate::preparation::{DeliveryUnit, UnitChange, diff_units, unit_id, unitize};

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

    /// A `**`-prefixed pattern (the scaffolded facet default `**/*`) is
    /// prefix-free and re-anchors verbatim onto the git root. Lexical
    /// re-rooting would yield `:(glob)../**/*` for any sub-medium — an
    /// out-of-tree pathspec git fatals on, degrading every diff to
    /// no-signal.
    #[test]
    fn wildcard_prefixed_pathspec_reanchors_verbatim() {
        let ws = Path::new("/m/ws");
        let git_root = Path::new("/m/ws/src");
        assert_eq!(to_git_pathspec("**/*", git_root, ws, false), ":(glob)**/*");
        assert_eq!(
            in_repo_pathspec("**/__pycache__/**", git_root, ws, true).as_deref(),
            Some(":(glob,exclude)**/__pycache__/**")
        );
    }

    use crate::ingest::resolve::Source;
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

    fn primary(scope: Vec<PatternEntry>) -> Source {
        Source {
            name: "src".to_string(),
            medium_type: MediumType::Codebase,
            pointer: String::new(),
            change_detection: Some("git".to_string()),
            scope,
            engagement: None,
            preparation: None,
        }
    }

    /// One dialect, one implementation: the SAME entry list must exclude the
    /// SAME files from an engine slice as [`super::check_path::check_deny_paths`]
    /// denies. Successor to the retired cross-boundary fixture test that
    /// pinned the engine against the plugin's JS dialect clone — both callers
    /// now run engine code, and this test keeps the two engine consumers
    /// (enumeration, path check) agreeing on shared data. Proven by
    /// materialising every path into a temp workspace, scoping a facet to
    /// `**` (everything), applying the entries as the ingest `deny_paths`,
    /// and asserting `enumerate_facet_files` yields exactly `allowed`.
    #[test]
    fn deny_dialect_agrees_between_slice_and_check() {
        let strs =
            |items: &[&str]| -> Vec<String> { items.iter().map(|s| s.to_string()).collect() };
        let entries = strs(&["dev/**", "**/VISION.md", "docs/meta/CLAUDE.md"]);
        let blocked = strs(&[
            "dev/notes/a.md",
            "dev/x.rs",
            "dev/deep/nested/y.txt",
            "VISION.md",
            "crates/foo/VISION.md",
            "docs/meta/CLAUDE.md",
        ]);
        let allowed = strs(&[
            "src/lib.rs",
            "dev-tools/x.rs",
            "VISION-draft.md",
            "docs/meta/README.md",
            "other/CLAUDE.md",
            "crates/foo/mod.rs",
        ]);

        let ws = tempfile::tempdir().unwrap();
        for rel in blocked.iter().chain(allowed.iter()) {
            let path = ws.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "x").unwrap();
        }

        // Scope = everything; the ONLY exclusions are the ingest deny_paths.
        let source = primary(vec![PatternEntry {
            path: "**".to_string(),
            mode: PatternMode::Allow,
        }]);
        let mut got = enumerate_facet_files(&source, &entries, ws.path());
        got.sort();
        let mut want = allowed.clone();
        want.sort();
        assert_eq!(
            got, want,
            "engine slice must equal the fixture `allowed` set"
        );

        for b in &blocked {
            assert!(
                !got.contains(b),
                "denied `{b}` leaked into the engine slice"
            );
        }
        // The path check agrees on every case — the two engine consumers of
        // the dialect can never drift apart silently.
        let all: Vec<String> = blocked.iter().chain(allowed.iter()).cloned().collect();
        let checks =
            super::super::check_path::check_deny_paths(&entries, &all, ws.path(), ws.path());
        for c in &checks {
            let expect = blocked.contains(&c.path);
            assert_eq!(
                c.denied, expect,
                "check_deny_paths disagrees with the slice on `{}`",
                c.path
            );
        }
    }

    /// A cross-repo deny (its target sibling to the medium's git repo)
    /// resolves outside `git_root` and is dropped from the pathspecs — pushing
    /// it would make git fatal on the whole diff. An in-repo deny is kept.
    #[test]
    fn out_of_repo_deny_pathspec_is_dropped() {
        let ws = Path::new("/m/graph");
        let git_root = Path::new("/m/public");
        // `../dev/**` (workspace-relative) → /m/dev/** — outside /m/public.
        assert_eq!(in_repo_pathspec("../dev/**", git_root, ws, true), None);
        assert_eq!(in_repo_pathspec("../CLAUDE.md", git_root, ws, true), None);
        // An in-repo deny is preserved as a normal exclude pathspec.
        assert_eq!(
            in_repo_pathspec("../public/target/**", git_root, ws, true),
            Some(":(glob,exclude)target/**".to_string())
        );
    }

    /// A git-shaped baseline the repo does NOT contain reseeds at HEAD
    /// instead of degrading to `GitUnavailable` forever. Regression for the
    /// dogfood plugin/graph binding, whose stored baseline was a commit of a
    /// *different* repo (seeded before the source moved into the submodule):
    /// every pass diffed against a foreign sha, fataled, and the baseline
    /// never seated.
    #[test]
    fn foreign_baseline_reseeds_instead_of_degrading() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        std::fs::write(root.join("keep.rs"), "one").unwrap();
        git(root, &["init", "-q"]);
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "seed"]);

        let source = primary(vec![PatternEntry {
            path: "**/*.rs".to_string(),
            mode: PatternMode::Allow,
        }]);
        // Git-token-shaped, but no such commit exists in this repo.
        let foreign = "46ce8add0fe87250527b6fa21fcfdc2d943d51f0";
        match compute_git_slice(&source, &[], root, Some(foreign)) {
            SliceOutcome::Reseed { token } => {
                // Reseeds at the repo's actual HEAD — the baseline seats.
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
                assert_eq!(token, head);
            }
            other => panic!("foreign baseline must reseed, got {other:?}"),
        }
    }

    /// A real git diff with a cross-repo deny present must still succeed (the
    /// out-of-repo pathspec is dropped, not fataled), and the in-repo scope is
    /// honoured. Regression for the dogfood dialect (`../dev/**` under a
    /// sub-medium): git must not degrade the whole slice.
    #[test]
    fn git_slice_survives_cross_repo_deny() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        std::fs::write(root.join("keep.rs"), "one").unwrap();
        git(root, &["init", "-q"]);
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "seed"]);
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
        std::fs::write(root.join("keep.rs"), "two").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "move"]);

        let source = primary(vec![PatternEntry {
            path: "**/*.rs".to_string(),
            mode: PatternMode::Allow,
        }]);
        // `../dev/**` resolves outside this repo — must be dropped, not fatal.
        let outcome = compute_git_slice(&source, &["../dev/**".to_string()], root, Some(&baseline));
        match outcome {
            SliceOutcome::Changed { slice, .. } => {
                assert_eq!(slice.modified, vec!["keep.rs"]);
            }
            other => panic!("expected Changed (deny dropped), got {other:?}"),
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
        assert_eq!(enumerate_facet_files(&source, &[], root), vec!["a.rs"]);

        // A graph medium is not a file tree, so the FILE walk yields nothing
        // for it — but that is a statement about this function, not about
        // graph enumerability. `enumerate_graph_entities` is the graph arm,
        // and `enumerate_source_artifacts` is what every S(D) consumer calls.
        let mut graph_source = source.clone();
        graph_source.medium_type = MediumType::Graph;
        assert!(enumerate_facet_files(&graph_source, &[], root).is_empty());
    }

    /// Graph enumeration is real: a graph source's `S(D)` is the source mem's
    /// in-scope entity set, selected by the entity vocabulary. This is the
    /// bail the S1b pilot hit — enumeration returned empty for every graph
    /// source, so coverage was vacuously 0/0 and `--full` passed over a
    /// measurement that never happened.
    #[test]
    fn graph_enumeration_selects_the_source_mems_entities() {
        use crate::workspace::{
            Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
        };
        use crate::workspace_store::WorkspaceStoreAdapter;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mem_dir = root.join("srcmem");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();

        let entity = |slug: &str, ty: &str, title: &str| {
            std::fs::write(
                mem_dir.join(format!("{slug}.md")),
                format!("---\ntype: {ty}\n---\n\n# {title}\n\n## Decision\n\nBody.\n"),
            )
            .unwrap();
        };
        entity("alpha-choice", "decision", "Alpha choice");
        entity("beta-choice", "decision", "Beta choice");
        entity("gamma-note", "memo", "Gamma note");

        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        crate::FileWorkspaceStore::new()
            .save_state(
                root,
                &Workspace {
                    mounts: vec![Mount {
                        mem: "srcmem".to_string(),
                        schema: Some("default@1.0.0".parse().unwrap()),
                        storage: MountStorage::Folder {
                            path: mem_dir.clone(),
                        },
                        capability: MountCapability::Write,
                        lifecycle: MountLifecycle::Eager,
                        cross_linkable: false,
                        migration_target: None,
                    }],
                    settings: WorkspaceSettings::default(),
                },
            )
            .unwrap();

        let engine = crate::Engine::from_workspace_root(root).unwrap();

        let graph_source = |patterns: Vec<(&str, PatternMode)>| Source {
            name: "g".to_string(),
            medium_type: MediumType::Graph,
            pointer: "srcmem".to_string(),
            change_detection: None,
            scope: patterns
                .into_iter()
                .map(|(p, mode)| crate::pipeline::PatternEntry {
                    path: p.to_string(),
                    mode,
                })
                .collect(),
            engagement: None,
            preparation: None,
        };

        // `*` — the whole mem. A real denominator, not an empty walk.
        let all = enumerate_graph_entities(&engine, &graph_source(vec![("*", PatternMode::Allow)]));
        assert_eq!(
            all,
            vec![
                "srcmem--alpha-choice".to_string(),
                "srcmem--beta-choice".to_string(),
                "srcmem--gamma-note".to_string(),
            ],
            "the whole-mem selector enumerates every real entity"
        );

        // `type:` selects on the type axis.
        let decisions = enumerate_graph_entities(
            &engine,
            &graph_source(vec![("type:decision", PatternMode::Allow)]),
        );
        assert_eq!(
            decisions,
            vec![
                "srcmem--alpha-choice".to_string(),
                "srcmem--beta-choice".to_string()
            ],
            "type selector excludes the memo"
        );

        // `id:` globs the id, and a deny subtracts from an allow.
        let globbed = enumerate_graph_entities(
            &engine,
            &graph_source(vec![
                ("id:srcmem--*-choice", PatternMode::Allow),
                ("id:srcmem--beta-*", PatternMode::Deny),
            ]),
        );
        assert_eq!(
            globbed,
            vec!["srcmem--alpha-choice".to_string()],
            "deny subtracts from allow in the entity namespace too"
        );

        // An unscoped graph facet enumerates nothing — the same posture the
        // path mediums have always had, and the reason the strategy layer
        // refuses it before ever reaching here.
        assert!(
            enumerate_graph_entities(&engine, &graph_source(vec![])).is_empty(),
            "an unscoped graph facet is never silently 'everything'"
        );

        // The dispatching entry point every S(D) consumer calls agrees.
        assert_eq!(
            enumerate_source_artifacts(
                &engine,
                &graph_source(vec![("*", PatternMode::Allow)]),
                &[],
                root
            ),
            all,
            "enumerate_source_artifacts routes a graph source to the graph arm"
        );
    }

    /// The S1b pilot's headline failure, encoded as a permanent regression
    /// test: a stale-pinned entity anchor over a source entity that changed
    /// since it was pinned must be flagged `drifted`. It used to go unflagged
    /// — anchor resolution was 0/0 for every graph source, so drift was
    /// structurally undetectable while the matrix claimed full parity.
    #[test]
    fn a_stale_entity_anchor_over_a_changed_entity_is_drifted() {
        use crate::anchor::{
            Anchor, AnchorGrain, AnchorProvenanceClass, AnchorSidecar, AnchorState,
        };
        use crate::entity::EntityId;
        use crate::workspace::{
            Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
        };
        use crate::workspace_store::WorkspaceStoreAdapter;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mem_dir = root.join("mem");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            mem_dir.join("pinned.md"),
            "---\ntype: decision\n---\n\n# Pinned\n\n## Decision\n\nOriginal body.\n",
        )
        .unwrap();
        std::fs::write(
            mem_dir.join("steady.md"),
            "---\ntype: decision\n---\n\n# Steady\n\n## Decision\n\nUnchanged body.\n",
        )
        .unwrap();

        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        crate::FileWorkspaceStore::new()
            .save_state(
                root,
                &Workspace {
                    mounts: vec![Mount {
                        mem: "mem".to_string(),
                        schema: Some("default@1.0.0".parse().unwrap()),
                        storage: MountStorage::Folder {
                            path: mem_dir.clone(),
                        },
                        capability: MountCapability::Write,
                        lifecycle: MountLifecycle::Eager,
                        cross_linkable: false,
                        migration_target: None,
                    }],
                    settings: WorkspaceSettings::default(),
                },
            )
            .unwrap();

        // Hash the entities as they stand, so the anchors start out honest.
        let engine = crate::Engine::from_workspace_root(root).unwrap();
        let hash_of = |engine: &crate::Engine, id: &str| {
            let e = engine.store().get(&EntityId::canonical(id)).unwrap();
            crate::anchor::prepared_content_hash(
                crate::render::render_entity_markdown(e, None).as_bytes(),
            )
        };
        let pinned_hash = hash_of(&engine, "mem--pinned");
        let steady_hash = hash_of(&engine, "mem--steady");

        let entity_anchor = |artifact: &str, hash: &str| Anchor {
            artifact: artifact.to_string(),
            grain: AnchorGrain::Entity,
            class: AnchorProvenanceClass::Anchored,
            hash: Some(hash.to_string()),
            source: None,
            binding: None,
            at_version: None,
            derived_from: Vec::new(),
            hash_stability: crate::anchor::AnchorHashStability::Stable,
        };

        let mut sidecar = AnchorSidecar::default();
        sidecar.set(
            "mem--holder",
            vec![
                entity_anchor("mem--pinned", &pinned_hash),
                entity_anchor("mem--steady", &steady_hash),
                // An anchor over an entity that does not exist at all.
                entity_anchor("mem--vanished", "deadbeefdeadbeef"),
            ],
        );
        std::fs::write(
            mem_dir.join(".memstead").join("anchors.json"),
            sidecar.to_bytes(),
        )
        .unwrap();
        std::fs::write(
            mem_dir.join("holder.md"),
            "---\ntype: decision\n---\n\n# Holder\n\n## Decision\n\nHolds anchors.\n",
        )
        .unwrap();

        // Now change ONE source entity — the pilot's move.
        std::fs::write(
            mem_dir.join("pinned.md"),
            "---\ntype: decision\n---\n\n# Pinned\n\n## Decision\n\nBody rewritten.\n",
        )
        .unwrap();

        let engine = crate::Engine::from_workspace_root(root).unwrap();
        let resolved = engine.entity_anchors_resolved(&EntityId::canonical("mem--holder"));
        let state_of = |artifact: &str| {
            resolved
                .iter()
                .find(|r| r.anchor.artifact == artifact)
                .unwrap_or_else(|| panic!("no resolved anchor for {artifact}"))
                .state
        };

        assert_eq!(
            state_of("mem--pinned"),
            Some(AnchorState::Drifted),
            "a stale-pinned anchor over a CHANGED entity must be drifted — \
             this is the pilot failure that went unflagged"
        );
        assert_eq!(
            state_of("mem--steady"),
            Some(AnchorState::Resolves),
            "an anchor over an unchanged entity still resolves"
        );
        assert_eq!(
            state_of("mem--vanished"),
            Some(AnchorState::Orphaned),
            "an anchor over an entity that is not there is orphaned, not unobserved"
        );

        // The complement: a `url` grain genuinely cannot be observed, and must
        // stay unobserved rather than being swept up by the widened arm.
        let mut sc2 = AnchorSidecar::default();
        sc2.set(
            "mem--holder",
            vec![Anchor {
                artifact: "https://example.invalid/doc".to_string(),
                grain: AnchorGrain::Url,
                class: AnchorProvenanceClass::InformedBy,
                hash: None,
                source: None,
                binding: None,
                at_version: None,
                derived_from: Vec::new(),
                hash_stability: crate::anchor::AnchorHashStability::Stable,
            }],
        );
        std::fs::write(
            mem_dir.join(".memstead").join("anchors.json"),
            sc2.to_bytes(),
        )
        .unwrap();
        let engine = crate::Engine::from_workspace_root(root).unwrap();
        let url_state =
            engine.entity_anchors_resolved(&EntityId::canonical("mem--holder"))[0].state;
        assert_eq!(
            url_state, None,
            "url anchors stay unobserved — the fix widens observation, never the \
             scoring of non-observation"
        );
    }

    /// Touchpoint A of the preparation registry, end to end: a graph source
    /// declaring `entity-load-bearing` makes its entity anchors hash the
    /// type's load-bearing sections. A notes-only edit (an optional section)
    /// keeps the prepared anchor resolving while an anchor over the default
    /// form (no source, hence no preparation) drifts — today's behaviour,
    /// untouched for it; a load-bearing edit drifts both. The standalone
    /// `verify_mem_anchors` walks the same observation and inherits the
    /// preparation unchanged. An unregistered identifier reaching a record
    /// by hand computes no form: its anchors stay unobserved, never scored.
    #[test]
    fn entity_load_bearing_preparation_ignores_notes_edits_and_catches_claim_edits() {
        use crate::anchor::{
            Anchor, AnchorGrain, AnchorProvenanceClass, AnchorSidecar, AnchorState,
        };
        use crate::binding::{
            BINDING_VERSION, Binding, BuildMode, BuildOperation, Operations, VerifyOperation,
        };
        use crate::entity::EntityId;
        use crate::pipeline::{IngestTrigger, MediumType, PatternEntry, PatternMode, Source};
        use crate::workspace::{
            Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
        };
        use crate::workspace_store::WorkspaceStoreAdapter;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mem_dir = root.join("home");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();
        // `assertion` in default@1.0.0: `claim` and `evidence` are required
        // (the load-bearing set), `conditions` is optional (notes-class).
        let write_pinned = |claim: &str, conditions: &str| {
            std::fs::write(
                mem_dir.join("pinned.md"),
                format!(
                    "---\ntype: assertion\n---\n\n# Pinned\n\n## Claim\n\n{claim}\n\n\
                     ## Evidence\n\nMeasured.\n\n## Conditions\n\n{conditions}\n"
                ),
            )
            .unwrap();
        };
        write_pinned("The sky is blue.", "daylight");
        std::fs::write(
            mem_dir.join("holder.md"),
            "---\ntype: assertion\n---\n\n# Holder\n\n## Claim\n\nDepends on pinned.\n\n\
             ## Evidence\n\nSee pinned.\n",
        )
        .unwrap();

        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        crate::FileWorkspaceStore::new()
            .save_state(
                root,
                &Workspace {
                    mounts: vec![Mount {
                        mem: "home".to_string(),
                        schema: Some("default@1.0.0".parse().unwrap()),
                        storage: MountStorage::Folder {
                            path: mem_dir.clone(),
                        },
                        capability: MountCapability::Write,
                        lifecycle: MountLifecycle::Eager,
                        cross_linkable: false,
                        migration_target: None,
                    }],
                    settings: WorkspaceSettings::default(),
                },
            )
            .unwrap();

        // The binding: one graph source named `claims`, declaring the
        // registered preparation. Written straight to the store — the shape
        // every edit path validates (`validate_binding`) accepts it.
        let binding_with = |preparation: Option<&str>| Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![Source {
                name: "claims".to_string(),
                medium_type: MediumType::Graph,
                pointer: "home".to_string(),
                change_detection: None,
                scope: vec![PatternEntry {
                    path: "*".to_string(),
                    mode: PatternMode::Allow,
                }],
                engagement: None,
                preparation: preparation.map(str::to_string),
            }],
            reference_mems: vec![],
            destination_mem: "home".to_string(),
            deny_paths: vec![],
            coverage_semantics: None,
            rules: None,
            prune: None,
            operations: Operations {
                build: Some(BuildOperation {
                    mode: BuildMode::Discovery,
                    trigger: IngestTrigger::Manual,
                    batch_size: 5,
                    post_actions: None,
                }),
                sync: None,
                verify: Some(VerifyOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 5,
                    adjudication_cap: 0,
                    full_resync_every: 0,
                }),
            },
        };
        let prepared_binding = binding_with(Some(crate::preparation::ENTITY_LOAD_BEARING));
        assert!(crate::binding::validate_binding(&prepared_binding).is_ok());
        crate::pipeline_store::write_binding(root, "home", "claims", &prepared_binding).unwrap();

        // Record both anchors honestly against the entity as it stands: one
        // produced by the `claims` source (prepared form), one hand-authored
        // (no source: the default form, the canonical rendered markdown).
        let engine = crate::Engine::from_workspace_root(root).unwrap();
        let pinned = engine
            .store()
            .get(&EntityId::canonical("home--pinned"))
            .unwrap();
        let type_def = engine
            .schema_for("home")
            .and_then(|s| s.get_type("assertion"))
            .expect("default@1.0.0 declares assertion");
        assert!(
            crate::preparation::load_bearing_sections(&type_def)
                .iter()
                .map(|s| s.key.as_str())
                .eq(["claim", "evidence"]),
            "the required sections are the load-bearing set"
        );
        let prepared_hash = crate::preparation::entity_prepared_hash(
            pinned,
            Some(&type_def),
            Some(crate::preparation::ENTITY_LOAD_BEARING),
        )
        .unwrap();
        let default_hash =
            crate::preparation::entity_prepared_hash(pinned, Some(&type_def), None).unwrap();
        assert_ne!(prepared_hash, default_hash);

        let anchor = |source: Option<&str>, hash: &str| Anchor {
            artifact: "home--pinned".to_string(),
            grain: AnchorGrain::Entity,
            class: AnchorProvenanceClass::Anchored,
            hash: Some(hash.to_string()),
            source: source.map(str::to_string),
            binding: None,
            at_version: None,
            derived_from: Vec::new(),
            hash_stability: crate::anchor::AnchorHashStability::Stable,
        };
        // Two holders so the two anchors over one artifact stay distinct rows.
        std::fs::write(
            mem_dir.join("holder2.md"),
            "---\ntype: assertion\n---\n\n# Holder2\n\n## Claim\n\nAlso depends.\n\n\
             ## Evidence\n\nSee pinned.\n",
        )
        .unwrap();
        let mut sidecar = AnchorSidecar::default();
        sidecar.set("home--holder", vec![anchor(Some("claims"), &prepared_hash)]);
        sidecar.set("home--holder2", vec![anchor(None, &default_hash)]);
        std::fs::write(
            mem_dir.join(".memstead").join("anchors.json"),
            sidecar.to_bytes(),
        )
        .unwrap();

        let states = |root: &std::path::Path| {
            let engine = crate::Engine::from_workspace_root(root).unwrap();
            let state_of = |holder: &str| {
                engine.entity_anchors_resolved(&EntityId::canonical(holder))[0].state
            };
            let standalone = engine.verify_mem_anchors("home").unwrap();
            (
                state_of("home--holder"),
                state_of("home--holder2"),
                standalone,
            )
        };

        // Unchanged: both resolve.
        let (prepared, plain, report) = states(root);
        assert_eq!(prepared, Some(AnchorState::Resolves));
        assert_eq!(plain, Some(AnchorState::Resolves));
        assert_eq!((report.resolved, report.drifted), (2, 0));

        // A notes-only edit (`conditions` is not load-bearing): the prepared
        // anchor holds, the default-form anchor drifts — today's behaviour,
        // byte-for-byte, for a source that declares nothing.
        write_pinned("The sky is blue.", "daylight, clear weather");
        let (prepared, plain, report) = states(root);
        assert_eq!(
            prepared,
            Some(AnchorState::Resolves),
            "a comma in the notes must not break a load-bearing anchor"
        );
        assert_eq!(plain, Some(AnchorState::Drifted));
        assert_eq!((report.resolved, report.drifted), (1, 1));

        // A load-bearing edit: both drift.
        write_pinned("The sky is green.", "daylight, clear weather");
        let (prepared, plain, report) = states(root);
        assert_eq!(prepared, Some(AnchorState::Drifted));
        assert_eq!(plain, Some(AnchorState::Drifted));
        assert_eq!((report.resolved, report.drifted), (0, 2));

        // Complement: a hand-edited record naming an identifier the registry
        // does not know computes no form — its anchors are unobserved, never
        // scored as drift or resolution; the source-less anchor is unaffected.
        crate::pipeline_store::write_binding(
            root,
            "home",
            "claims",
            &binding_with(Some("pdf-to-markdown")),
        )
        .unwrap();
        let (prepared, plain, report) = states(root);
        assert_eq!(
            prepared, None,
            "an unknown preparation yields no observation"
        );
        assert_eq!(plain, Some(AnchorState::Drifted));
        assert_eq!((report.unresolvable, report.drifted), (1, 1));
    }

    /// Touchpoint B's order is a property of the units, not of discovery: a
    /// shuffled collection sorts into the identical sequence.
    #[test]
    fn shuffled_discovery_sequences_identically() {
        use crate::preparation::UnitChange;
        let unit = |id: &str, order: &str| DeliveredUnit {
            id: id.to_string(),
            order_key: order.to_string(),
            change: UnitChange::Added,
            disposed: false,
        };
        let ordered = vec![
            unit("corpus/notes.md#whole", ""),
            unit("corpus/b.md#2026-08-20T00:00:00", "2026-08-20T00:00:00"),
            unit("corpus/a.md#2026-08-21T00:00:00", "2026-08-21T00:00:00"),
            unit("corpus/a.md#2026-08-21T00:00:00.2", "2026-08-21T00:00:00"),
            unit("corpus/c.md#2026-08-21T00:00:00", "2026-08-21T00:00:00"),
            unit("corpus/b.md#2026-08-22T00:00:00", "2026-08-22T00:00:00"),
        ];
        for shuffle in [
            vec![5, 3, 0, 4, 1, 2],
            vec![2, 1, 0, 5, 4, 3],
            vec![4, 0, 5, 2, 3, 1],
        ] {
            let mut units: Vec<DeliveredUnit> =
                shuffle.iter().map(|i| ordered[*i].clone()).collect();
            sequence_units(&mut units);
            assert_eq!(units, ordered, "discovery order {shuffle:?} must not leak");
        }

        // A date-only day with twelve entries: the same-stamp ordinal orders
        // numerically, never as text (`.10` after `.9`, not before `.2`).
        let day = "2026-08-24T00:00:00";
        let expected: Vec<String> = (1..=12)
            .map(|n| {
                if n == 1 {
                    format!("journal.md#{day}")
                } else {
                    format!("journal.md#{day}.{n}")
                }
            })
            .collect();
        let mut units: Vec<DeliveredUnit> = expected.iter().rev().map(|id| unit(id, day)).collect();
        sequence_units(&mut units);
        assert_eq!(
            units.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
            expected.iter().map(String::as_str).collect::<Vec<_>>()
        );
    }

    /// Touchpoint B end to end over a git corpus whose path order is not its
    /// chronological order: the first run delivers every unit in stamp order
    /// interleaved across files; the sibling source without a preparation
    /// keeps file-granularity delivery; disposing advances through the
    /// sequence, an anchor over exactly a unit auto-disposes it while a
    /// file-level anchor does not; the change run delivers only the new,
    /// changed and removed units at their ordered positions (keys stable
    /// under growth); and a span anchor over a unit observes the unit, not
    /// the file.
    #[test]
    fn dated_entries_deliver_in_a_total_order_across_first_and_change_runs() {
        use crate::anchor::{
            Anchor, AnchorGrain, AnchorProvenanceClass, AnchorSidecar, AnchorState,
        };
        use crate::binding::{
            BINDING_VERSION, Binding, BuildMode, BuildOperation, Operations, VerifyOperation,
        };
        use crate::entity::EntityId;
        use crate::ingest::advance::{DispositionInput, advance_baseline};
        use crate::ingest::brief::render_changed_slice;
        use crate::ingest::resolve::resolve_binding_run;
        use crate::pipeline::{IngestTrigger, PatternMode};
        use crate::preparation::{DATED_ENTRIES, UnitChange, unitize};
        use crate::workspace::{
            Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
        };
        use crate::workspace_store::WorkspaceStoreAdapter;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Destination: a folder mem `home` with one holder entity for anchors.
        let mem_dir = root.join("home");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            mem_dir.join("holder.md"),
            "---\ntype: assertion\n---\n\n# Holder\n\n## Claim\n\nHolds anchors.\n\n\
             ## Evidence\n\nSee corpus.\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        crate::FileWorkspaceStore::new()
            .save_state(
                root,
                &Workspace {
                    mounts: vec![Mount {
                        mem: "home".to_string(),
                        schema: Some("default@1.0.0".parse().unwrap()),
                        storage: MountStorage::Folder {
                            path: mem_dir.clone(),
                        },
                        capability: MountCapability::Write,
                        lifecycle: MountLifecycle::Eager,
                        cross_linkable: false,
                        migration_target: None,
                    }],
                    settings: WorkspaceSettings::default(),
                },
            )
            .unwrap();

        // Source: a git corpus whose lexical path order (a, b, notes) is not
        // its chronological order.
        let corpus = root.join("corpus");
        std::fs::create_dir_all(corpus.join("plain")).unwrap();
        git(&corpus, &["init", "-q"]);
        let write = |name: &str, text: &str| std::fs::write(corpus.join(name), text).unwrap();
        write(
            "a.md",
            "2026-08-21 alpha one\nbody a1\n2026-08-23 alpha two\nbody a2\n",
        );
        write(
            "b.md",
            "2026-08-20 beta one\nbody b1\n2026-08-22 beta two\nbody b2\n",
        );
        write("notes.md", "undated notes\n");
        write("plain/readme.txt", "plain source, file granularity\n");
        git(&corpus, &["add", "."]);
        git(&corpus, &["commit", "-q", "-m", "corpus"]);

        let source = |name: &str, scope: &str, preparation: Option<&str>| Source {
            name: name.to_string(),
            medium_type: MediumType::Filesystem,
            pointer: "corpus".to_string(),
            change_detection: Some("git".to_string()),
            scope: vec![PatternEntry {
                path: scope.to_string(),
                mode: PatternMode::Allow,
            }],
            engagement: None,
            preparation: preparation.map(str::to_string),
        };
        let binding = Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![
                source("logs", "corpus/*.md", Some(DATED_ENTRIES)),
                source("plain", "corpus/plain/**", None),
            ],
            reference_mems: vec![],
            destination_mem: "home".to_string(),
            deny_paths: vec![],
            coverage_semantics: None,
            rules: None,
            prune: None,
            operations: Operations {
                build: Some(BuildOperation {
                    mode: BuildMode::Discovery,
                    trigger: IngestTrigger::Manual,
                    batch_size: 3,
                    post_actions: None,
                }),
                sync: None,
                verify: Some(VerifyOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 5,
                    adjudication_cap: 0,
                    full_resync_every: 0,
                }),
            },
        };
        assert!(
            crate::binding::validate_binding(&binding).is_ok(),
            "{:?}",
            crate::binding::validate_binding(&binding)
        );
        crate::pipeline_store::write_binding(root, "home", "corpus", &binding).unwrap();
        let resolved = resolve_binding_run("home/corpus", &binding).unwrap();

        // ---- First run: every unit, in stamp order, interleaved across files.
        let mut engine = crate::Engine::from_workspace_root(root).unwrap();
        let cursor = compute_source_cursor(&engine, &resolved, root);
        let expected: Vec<&str> = vec![
            "corpus/notes.md#whole",
            "corpus/b.md#2026-08-20T00:00:00",
            "corpus/a.md#2026-08-21T00:00:00",
            "corpus/b.md#2026-08-22T00:00:00",
            "corpus/a.md#2026-08-23T00:00:00",
        ];
        assert_eq!(
            cursor.delivery.len(),
            1,
            "one sequence, for the prepared source only"
        );
        let seq = &cursor.delivery[0];
        assert_eq!(
            (seq.source.as_str(), seq.preparation.as_str()),
            ("logs", DATED_ENTRIES)
        );
        assert!(seq.first_run && !seq.degraded && seq.batch == 3);
        assert_eq!(
            seq.units.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
            expected
        );
        assert!(
            seq.units
                .iter()
                .all(|u| u.change == UnitChange::Added && !u.disposed)
        );
        let mut expected_sorted: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        expected_sorted.sort();
        assert_eq!(
            cursor.union.added, expected_sorted,
            "the advance gate accepts the unit ids"
        );
        // The sibling source without a preparation keeps file granularity:
        // a plain first-run reseed, no units, no sequence.
        assert!(
            cursor
                .reseed
                .iter()
                .any(|c| c.key == "home/corpus/plain#synced")
        );
        assert!(
            cursor
                .write_commands
                .iter()
                .any(|c| c.key == "home/corpus/logs#synced")
        );
        assert!(
            !cursor
                .union
                .added
                .iter()
                .any(|a| a.starts_with("corpus/plain"))
        );
        // Recomputing yields the identical sequence.
        assert_eq!(compute_source_cursor(&engine, &resolved, root), cursor);

        let brief = render_changed_slice(&cursor);
        assert!(
            brief.contains("### Delivery sequence: `logs` (`dated-entries`)"),
            "{brief}"
        );
        assert!(brief.contains("First delivery of this source"));
        let listed: Vec<&str> = brief
            .lines()
            .filter(|l| l.starts_with(|c: char| c.is_ascii_digit()) && l.contains("`corpus/"))
            .collect();
        assert_eq!(
            listed,
            vec![
                "1. `corpus/notes.md#whole` (new)",
                "2. `corpus/b.md#2026-08-20T00:00:00` (new)",
                "3. `corpus/a.md#2026-08-21T00:00:00` (new)",
            ],
            "the batch presents the first three in order"
        );
        assert!(brief.contains("…and 2 more, presented in order once these are disposed"));
        assert!(
            !brief.contains("**Added:**"),
            "unit ids never repeat in a class list: {brief}"
        );

        // ---- Advance through the sequence.
        let dispositions: BTreeMap<String, DispositionInput> =
            [(expected[0], "skipped"), (expected[1], "worked")]
                .into_iter()
                .map(|(a, d)| (a.to_string(), DispositionInput::Verdict(d.to_string())))
                .collect();
        let outcome = advance_baseline(&mut engine, root, &resolved, &dispositions).unwrap();
        assert_eq!((outcome.pending, outcome.completed), (3, false));
        let cursor = compute_source_cursor(&engine, &resolved, root);
        assert!(cursor.delivery[0].units[0].disposed && cursor.delivery[0].units[1].disposed);
        let brief = render_changed_slice(&cursor);
        assert!(
            brief.contains("3. `corpus/a.md#2026-08-21T00:00:00` (new)"),
            "{brief}"
        );
        assert!(
            !brief.contains("1. `corpus/notes.md#whole`"),
            "disposed units are not re-presented"
        );
        assert!(brief.contains("2 units of this sequence already disposed"));

        // An anchor over exactly a unit disposes it; a file-level anchor over
        // `b.md` disposes none of b's units.
        let a21_text = std::fs::read_to_string(corpus.join("a.md")).unwrap();
        let a21_unit = unitize(DATED_ENTRIES, &a21_text)
            .unwrap()
            .into_iter()
            .find(|u| u.key == "2026-08-21T00:00:00")
            .unwrap();
        let anchor = |artifact: &str, grain: AnchorGrain, hash: &str| Anchor {
            artifact: artifact.to_string(),
            grain,
            class: AnchorProvenanceClass::Anchored,
            hash: Some(hash.to_string()),
            source: Some("logs".to_string()),
            binding: None,
            at_version: None,
            derived_from: Vec::new(),
            hash_stability: crate::anchor::AnchorHashStability::Stable,
        };
        let b_file_hash =
            crate::anchor::prepared_content_hash(&std::fs::read(corpus.join("b.md")).unwrap());
        let mut sidecar = AnchorSidecar::default();
        sidecar.set(
            "home--holder",
            vec![
                anchor(expected[2], AnchorGrain::Span, &a21_unit.hash),
                anchor("corpus/b.md", AnchorGrain::File, &b_file_hash),
            ],
        );
        std::fs::write(
            mem_dir.join(".memstead").join("anchors.json"),
            sidecar.to_bytes(),
        )
        .unwrap();
        let mut engine = crate::Engine::from_workspace_root(root).unwrap();
        let outcome = advance_baseline(&mut engine, root, &resolved, &BTreeMap::new()).unwrap();
        assert_eq!(
            outcome.pending, 2,
            "the unit anchor auto-disposed its unit, the file anchor nothing"
        );
        assert!(outcome.remainder.added.contains(&expected[3].to_string()));
        assert!(outcome.remainder.added.contains(&expected[4].to_string()));
        let rest: BTreeMap<String, DispositionInput> = [expected[3], expected[4]]
            .into_iter()
            .map(|a| {
                (
                    a.to_string(),
                    DispositionInput::Verdict("worked".to_string()),
                )
            })
            .collect();
        let outcome = advance_baseline(&mut engine, root, &resolved, &rest).unwrap();
        assert!(outcome.completed, "{outcome:?}");
        assert!(
            outcome
                .tokens_written
                .contains(&"home/corpus/logs#synced".to_string())
        );

        // ---- Change run: one earlier entry appended to a.md, b's second
        // entry edited, b's first entry removed. Only those three units are
        // delivered, at their ordered positions; every other key survives.
        write(
            "a.md",
            "2026-08-21 alpha one\nbody a1\n2026-08-23 alpha two\nbody a2\n2026-08-19 alpha zero\nbody a0\n",
        );
        write("b.md", "2026-08-22 beta two\nbody b2, revised\n");
        git(&corpus, &["add", "."]);
        git(&corpus, &["commit", "-q", "-m", "grow, edit, remove"]);
        let engine = crate::Engine::from_workspace_root(root).unwrap();
        let cursor = compute_source_cursor(&engine, &resolved, root);
        let seq = &cursor.delivery[0];
        assert!(!seq.first_run && !seq.degraded);
        assert_eq!(
            seq.units
                .iter()
                .map(|u| (u.id.as_str(), u.change))
                .collect::<Vec<_>>(),
            vec![
                ("corpus/a.md#2026-08-19T00:00:00", UnitChange::Added),
                ("corpus/b.md#2026-08-20T00:00:00", UnitChange::Deleted),
                ("corpus/b.md#2026-08-22T00:00:00", UnitChange::Modified),
            ]
        );
        let brief = render_changed_slice(&cursor);
        assert!(brief.contains("The units that changed since the last pass"));
        assert!(
            brief.contains("1. `corpus/a.md#2026-08-19T00:00:00` (new)"),
            "{brief}"
        );
        assert!(brief.contains("3. `corpus/b.md#2026-08-22T00:00:00` (changed)"));

        // ---- Touchpoint A over units: the span anchor on a.md's unchanged
        // unit still resolves although its file changed; an anchor on the
        // removed unit orphans; one on the edited unit drifts.
        let b22_old_text = "2026-08-22 beta two\nbody b2\n";
        let b22_old = unitize(DATED_ENTRIES, b22_old_text).unwrap()[0]
            .hash
            .clone();
        let mut sidecar = AnchorSidecar::default();
        sidecar.set(
            "home--holder",
            vec![
                anchor(expected[2], AnchorGrain::Span, &a21_unit.hash),
                anchor(expected[1], AnchorGrain::Span, "deadbeefdeadbeef"),
                anchor(expected[3], AnchorGrain::Span, &b22_old),
            ],
        );
        std::fs::write(
            mem_dir.join(".memstead").join("anchors.json"),
            sidecar.to_bytes(),
        )
        .unwrap();
        let engine = crate::Engine::from_workspace_root(root).unwrap();
        let resolved_anchors = engine.entity_anchors_resolved(&EntityId::canonical("home--holder"));
        let state_of = |artifact: &str| {
            resolved_anchors
                .iter()
                .find(|r| r.anchor.artifact == artifact)
                .unwrap()
                .state
        };
        assert_eq!(
            state_of(expected[2]),
            Some(AnchorState::Resolves),
            "unit unchanged, file changed"
        );
        assert_eq!(
            state_of(expected[1]),
            Some(AnchorState::Orphaned),
            "unit removed"
        );
        assert_eq!(
            state_of(expected[3]),
            Some(AnchorState::Drifted),
            "unit edited"
        );
    }

    /// Criterion 6's regression pin: the graph change-detection half is
    /// untouched except for the deliberate unscoped gate. A SCOPED graph
    /// facet still routes to the graph strategy and reports the same
    /// no-signal reason it always did when the source mem exposes no
    /// snapshot token (a folder mem tracks no head); an UNSCOPED one now
    /// refuses as `Unscoped`, exactly as the git and mtime arms have always
    /// done. Distinguishing the two is the whole point — before this, an
    /// unscoped graph facet silently proceeded.
    #[test]
    fn graph_scoping_changes_only_the_unscoped_arm() {
        use crate::binding::BuildMode;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mem_dir = root.join("srcmem");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            mem_dir.join("one.md"),
            "---\ntype: decision\n---\n\n# One\n\n## Decision\n\nBody.\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        {
            use crate::workspace::{
                Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
            };
            use crate::workspace_store::WorkspaceStoreAdapter;
            crate::FileWorkspaceStore::new()
                .save_state(
                    root,
                    &Workspace {
                        mounts: vec![Mount {
                            mem: "srcmem".to_string(),
                            schema: Some("default@1.0.0".parse().unwrap()),
                            storage: MountStorage::Folder {
                                path: mem_dir.clone(),
                            },
                            capability: MountCapability::Write,
                            lifecycle: MountLifecycle::Eager,
                            cross_linkable: false,
                            migration_target: None,
                        }],
                        settings: WorkspaceSettings::default(),
                    },
                )
                .unwrap();
        }
        let engine = crate::Engine::from_workspace_root(root).unwrap();

        let resolved_with = |scope: Vec<crate::pipeline::PatternEntry>| ResolvedIngest {
            name: "srcmem/p".to_string(),
            mode: BuildMode::Discovery,
            trigger: crate::pipeline::IngestTrigger::Manual,
            batch_size: 20,
            deny_paths: Vec::new(),
            projection_ref: "srcmem/p".to_string(),
            projection_mem: "srcmem".to_string(),
            projection_name: "p".to_string(),
            intent: None,
            sources: vec![ResolvedSource::Primary(Source {
                name: "g".to_string(),
                medium_type: MediumType::Graph,
                pointer: "srcmem".to_string(),
                change_detection: None,
                scope,
                engagement: None,
                preparation: None,
            })],
            destination_mem: "srcmem".to_string(),
            rules: None,
            post_actions: None,
        };

        let scoped = compute_source_cursor(
            &engine,
            &resolved_with(vec![crate::pipeline::PatternEntry {
                path: "*".to_string(),
                mode: PatternMode::Allow,
            }]),
            root,
        );
        let unscoped = compute_source_cursor(&engine, &resolved_with(Vec::new()), root);

        let reason_of = |c: &SourceCursor| c.no_signal.first().map(|n| n.reason);
        assert_eq!(
            reason_of(&scoped),
            Some(NoSignalReason::GraphSnapshotMissing),
            "a scoped graph facet still routes to the graph strategy and reports \
             its own no-signal reason — the change-detection half is untouched"
        );
        assert_eq!(
            reason_of(&unscoped),
            Some(NoSignalReason::Unscoped),
            "an unscoped graph facet refuses like every other medium's, instead of \
             silently proceeding"
        );
    }

    /// The git medium enumerates through the same path walk as codebase and
    /// filesystem — its artifacts are paths pinned at a commit, so the walk is
    /// identical and only the anchor namespace differs. It was excluded from
    /// that arm for no reason beyond the arm's shape, which made its
    /// `enumerable: true` row a claim nothing delivered. Pinned so a refactor
    /// cannot quietly drop it back out.
    #[test]
    fn git_medium_enumerates_through_the_path_walk() {
        let ws = tempfile::tempdir().unwrap();
        let root = ws.path();
        std::fs::write(root.join("a.rs"), "").unwrap();
        std::fs::write(root.join("b.rs"), "").unwrap();

        let source = |medium: MediumType| Source {
            name: "s".to_string(),
            medium_type: medium,
            pointer: ".".to_string(),
            change_detection: None,
            scope: vec![crate::pipeline::PatternEntry {
                path: "**/*.rs".to_string(),
                mode: PatternMode::Allow,
            }],
            engagement: None,
            preparation: None,
        };

        let want = vec!["a.rs".to_string(), "b.rs".to_string()];
        for medium in [
            MediumType::Codebase,
            MediumType::Filesystem,
            MediumType::Git,
        ] {
            assert_eq!(
                enumerate_facet_files(&source(medium), &[], root),
                want,
                "{medium:?} walks the file tree — every medium the matrix marks \
                 enumerable with a path namespace must actually enumerate"
            );
            assert!(
                crate::binding::medium_capabilities(medium).enumerable,
                "{medium:?} claims enumerability, and now delivers it"
            );
        }
    }

    /// A narrowing selector must bound the CHANGED SLICE, not only `S(D)`.
    /// It used to bound only enumeration, so a brief could print
    /// `Entities: type:concept` and then present a changed `memo` two
    /// sections below — an artifact its own coverage model calls out of
    /// scope, which `advance` would accept because its gate is the presented
    /// slice. Scope interpreted in one place and decorative in the other is
    /// the defect this pins closed.
    #[test]
    fn a_narrowing_selector_bounds_the_changed_slice_too() {
        use crate::workspace::{
            Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
        };
        use crate::workspace_store::WorkspaceStoreAdapter;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mem_dir = root.join("srcmem");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();
        let write = |slug: &str, ty: &str| {
            std::fs::write(
                mem_dir.join(format!("{slug}.md")),
                format!("---\ntype: {ty}\n---\n\n# {slug}\n\n## Decision\n\nBody.\n"),
            )
            .unwrap();
        };
        write("kept", "decision");
        write("other", "memo");
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        crate::FileWorkspaceStore::new()
            .save_state(
                root,
                &Workspace {
                    mounts: vec![Mount {
                        mem: "srcmem".to_string(),
                        schema: Some("default@1.0.0".parse().unwrap()),
                        storage: MountStorage::Folder {
                            path: mem_dir.clone(),
                        },
                        capability: MountCapability::Write,
                        lifecycle: MountLifecycle::Eager,
                        cross_linkable: false,
                        migration_target: None,
                    }],
                    settings: WorkspaceSettings::default(),
                },
            )
            .unwrap();
        let engine = crate::Engine::from_workspace_root(root).unwrap();

        let source = Source {
            name: "g".to_string(),
            medium_type: MediumType::Graph,
            pointer: "srcmem".to_string(),
            change_detection: None,
            scope: vec![crate::pipeline::PatternEntry {
                path: "type:decision".to_string(),
                mode: PatternMode::Allow,
            }],
            engagement: None,
            preparation: None,
        };

        let mut slice = Slice {
            added: vec!["srcmem--other".to_string()],
            modified: vec!["srcmem--kept".to_string(), "srcmem--other".to_string()],
            deleted: vec!["srcmem--vanished".to_string()],
        };
        filter_graph_slice_to_scope(&engine, &source, &mut slice);

        assert_eq!(
            slice.modified,
            vec!["srcmem--kept".to_string()],
            "the out-of-scope memo is dropped from the slice the brief presents"
        );
        assert!(
            slice.added.is_empty(),
            "an added out-of-scope entity is out of scope too"
        );
        assert_eq!(
            slice.deleted,
            vec!["srcmem--vanished".to_string()],
            "a DELETED entity is kept even though its type can no longer be \
             read — a deletion that cannot be classified must be reported, \
             never silently dropped"
        );

        // The complement: the whole-mem selector narrows nothing.
        let mut wide = Slice {
            added: Vec::new(),
            modified: vec!["srcmem--kept".to_string(), "srcmem--other".to_string()],
            deleted: Vec::new(),
        };
        let mut all = source.clone();
        all.scope = vec![crate::pipeline::PatternEntry {
            path: "*".to_string(),
            mode: PatternMode::Allow,
        }];
        filter_graph_slice_to_scope(&engine, &all, &mut wide);
        assert_eq!(wide.modified.len(), 2, "`*` selects the whole mem");
    }

    /// The entity-selector grammar is closed: three legal forms, everything
    /// else refused. A pattern that parses to `None` is a validation refusal
    /// at declaration — never a rule that silently selects nothing.
    #[test]
    fn entity_selector_grammar_is_closed() {
        use super::EntitySelector;
        assert_eq!(parse_entity_selector("*"), Some(EntitySelector::All));
        assert_eq!(
            parse_entity_selector("type:decision"),
            Some(EntitySelector::Type("decision".to_string()))
        );
        assert_eq!(
            parse_entity_selector("id:engine--*"),
            Some(EntitySelector::Id("engine--*".to_string()))
        );
        // The path glob `projection init` used to scaffold for graph sources:
        // it looks like scope and selects nothing. Refused, not accepted.
        assert_eq!(parse_entity_selector("**/*"), None);
        assert_eq!(parse_entity_selector("src/**"), None);
        assert_eq!(parse_entity_selector("type:"), None);
        assert_eq!(parse_entity_selector("id:"), None);
        assert_eq!(parse_entity_selector(""), None);
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
        let token = match compute_mtime_slice(&source, "ing", &[], root, &cache, None) {
            SliceOutcome::Reseed { token } => token,
            other => panic!("expected Reseed, got {other:?}"),
        };

        // Move the source: modify a.rs (size change), delete gone.rs, add new.rs.
        std::fs::write(root.join("a.rs"), "one-longer").unwrap();
        std::fs::remove_file(root.join("gone.rs")).unwrap();
        std::fs::write(root.join("new.rs"), "x").unwrap();

        // Second pass with the reseed token → precise diff from the memo.
        match compute_mtime_slice(&source, "ing", &[], root, &cache, Some(&token)) {
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
        match compute_mtime_slice(&source, "ing", &[], root, &cache, Some(&stale)) {
            SliceOutcome::Changed { degraded, .. } => assert!(degraded, "memo miss → degraded"),
            other => panic!("expected degraded Changed, got {other:?}"),
        }
    }

    fn head_sha(repo: &Path) -> String {
        String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    }

    fn slice_contains(slice: &Slice, path: &str) -> bool {
        let p = path.to_string();
        slice.added.contains(&p) || slice.modified.contains(&p) || slice.deleted.contains(&p)
    }

    /// The mtime `source_moved` / `current_primary_token` value: the digest
    /// token over the deny-filtered enumeration — exactly what the mtime branch
    /// of `current_primary_token` computes.
    fn mtime_token(source: &Source, deny: &[String], root: &Path) -> String {
        let files = enumerate_facet_files(source, deny, root);
        serialize_digest_token(&digest_stat_map(&compute_stat_map(&files, root)))
    }

    /// AC1 (deny invariance): a file matching an ingest `deny_paths` entry
    /// appears in **no** changed slice (git, mtime), **no** refinement batch,
    /// and does **not** influence the mtime digest / `source_moved` token —
    /// exercising the *same* denied file across every strategy that reads a
    /// file tree.
    #[test]
    fn deny_paths_excluded_from_every_strategy_and_token() {
        use crate::binding::BuildMode;
        use crate::ingest::refinement::next_batch;
        use crate::pipeline::IngestTrigger;

        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        let cache = root.join(".memstead.cache").join("ingest");

        // One tree that is both the git work tree and the mtime/refinement
        // workspace root (medium_pointer "" → base == root).
        git(root, &["init", "-q"]);
        std::fs::write(root.join("keep.rs"), "one").unwrap();
        std::fs::write(root.join("denied.rs"), "secret-one").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "base"]);
        let baseline = head_sha(root);

        // Both files genuinely move — denied.rs must never surface anywhere.
        std::fs::write(root.join("keep.rs"), "two").unwrap();
        std::fs::write(root.join("denied.rs"), "secret-two").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "move"]);

        // Scope allows every .rs; the ingest denies denied.rs by the same
        // workspace-relative glob grammar the git strategy uses.
        let source = primary(vec![PatternEntry {
            path: "**/*.rs".to_string(),
            mode: PatternMode::Allow,
        }]);
        let deny = vec!["denied.rs".to_string()];

        // (1) git slice — with the deny, only keep.rs.
        match compute_git_slice(&source, &deny, root, Some(&baseline)) {
            SliceOutcome::Changed { slice, .. } => {
                assert_eq!(slice.modified, vec!["keep.rs"]);
                assert!(!slice_contains(&slice, "denied.rs"), "git deny leak");
            }
            other => panic!("git: expected Changed, got {other:?}"),
        }
        // Control: without the deny, denied.rs *is* a real change — proving the
        // deny (not the scope) is what excludes it above.
        match compute_git_slice(&source, &[], root, Some(&baseline)) {
            SliceOutcome::Changed { slice, .. } => {
                assert!(
                    slice_contains(&slice, "denied.rs"),
                    "un-denied, denied.rs is a genuine git change"
                );
            }
            other => panic!("git(no-deny): expected Changed, got {other:?}"),
        }

        // (2) enumeration (mtime input set + refinement source set).
        assert_eq!(enumerate_facet_files(&source, &deny, root), vec!["keep.rs"]);
        assert!(
            enumerate_facet_files(&source, &[], root).contains(&"denied.rs".to_string()),
            "un-denied, denied.rs is enumerated"
        );

        // (2b) mtime slice — reseed, then move both files; only keep.rs surfaces.
        let token = match compute_mtime_slice(&source, "ing", &deny, root, &cache, None) {
            SliceOutcome::Reseed { token } => token,
            other => panic!("mtime reseed expected, got {other:?}"),
        };
        std::fs::write(root.join("keep.rs"), "three-longer").unwrap();
        std::fs::write(root.join("denied.rs"), "secret-three-longer").unwrap();
        match compute_mtime_slice(&source, "ing", &deny, root, &cache, Some(&token)) {
            SliceOutcome::Changed { slice, .. } => {
                assert_eq!(slice.modified, vec!["keep.rs"]);
                assert!(!slice_contains(&slice, "denied.rs"), "mtime deny leak");
            }
            other => panic!("mtime: expected Changed, got {other:?}"),
        }

        // (3) mtime digest / source_moved token — invariant to denied.rs, since
        // the token is the digest over the deny-filtered enumeration. Removing
        // denied.rs from disk leaves the token unchanged; a leak would show it
        // as a deletion and shift the digest.
        let token_present = mtime_token(&source, &deny, root);
        std::fs::remove_file(root.join("denied.rs")).unwrap();
        let token_absent = mtime_token(&source, &deny, root);
        assert_eq!(
            token_present, token_absent,
            "denied.rs must not influence the mtime digest / source_moved token"
        );
        std::fs::write(root.join("denied.rs"), "secret-restored").unwrap();

        // (4) refinement batch — the denied file is never batched.
        let resolved = ResolvedIngest {
            name: "ing".to_string(),
            mode: BuildMode::Discovery,
            trigger: IngestTrigger::Loop,
            batch_size: 50,
            deny_paths: deny.clone(),
            projection_ref: "m/p".to_string(),
            projection_mem: "m".to_string(),
            projection_name: "p".to_string(),
            intent: None,
            sources: vec![ResolvedSource::Primary(source.clone())],
            destination_mem: "m".to_string(),
            rules: None,
            post_actions: None,
        };
        let engine = crate::Engine::from_mounts(Vec::new()).unwrap();
        let batch = next_batch(&engine, &resolved, root, &cache, 20).unwrap();
        assert!(
            batch.files.contains(&"keep.rs".to_string()),
            "keep.rs batched"
        );
        assert!(
            !batch.files.contains(&"denied.rs".to_string()),
            "denied.rs must never enter a refinement batch"
        );
    }

    /// AC2 (one empty-scope semantic): an **unscoped** facet (no allow
    /// patterns) is the same typed refusal — `NoSignal { Unscoped }` — on git
    /// AND mtime, never a silent empty slice. AC2 complement: an empty
    /// `deny_paths` list does NOT trip that refusal — a *scoped* facet still
    /// classifies normally (empty scope and empty deny_paths are different
    /// fields with different semantics).
    #[test]
    fn unscoped_facet_refuses_uniformly_and_empty_deny_is_distinct() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        let cache = root.join(".memstead.cache").join("ingest");
        git(root, &["init", "-q"]);
        std::fs::write(root.join("a.rs"), "one").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "base"]);
        let baseline = head_sha(root);
        std::fs::write(root.join("a.rs"), "two").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "move"]);

        // Unscoped: a deny pattern but no allow. `deny_paths` is empty here —
        // so the refusal comes from the empty *scope*, not from denies.
        let unscoped = primary(vec![PatternEntry {
            path: "target/**".to_string(),
            mode: PatternMode::Deny,
        }]);
        assert_eq!(
            compute_git_slice(&unscoped, &[], root, Some(&baseline)),
            SliceOutcome::NoSignal {
                reason: NoSignalReason::Unscoped
            },
            "git refuses an unscoped facet"
        );
        assert_eq!(
            compute_mtime_slice(&unscoped, "ing", &[], root, &cache, None),
            SliceOutcome::NoSignal {
                reason: NoSignalReason::Unscoped
            },
            "mtime refuses an unscoped facet identically"
        );
        // A fully empty scope is unscoped too.
        let empty_scope = primary(vec![]);
        assert_eq!(
            compute_git_slice(&empty_scope, &[], root, Some(&baseline)),
            SliceOutcome::NoSignal {
                reason: NoSignalReason::Unscoped
            }
        );

        // Complement: a SCOPED facet with an empty `deny_paths` classifies
        // normally — empty deny_paths (no denies) must not trip the refusal.
        let scoped = primary(vec![PatternEntry {
            path: "**/*.rs".to_string(),
            mode: PatternMode::Allow,
        }]);
        assert!(
            matches!(
                compute_git_slice(&scoped, &[], root, Some(&baseline)),
                SliceOutcome::Changed { .. }
            ),
            "scoped facet + empty deny_paths → normal git slice, not a refusal"
        );
        assert!(
            matches!(
                compute_mtime_slice(&scoped, "ing", &[], root, &cache, None),
                SliceOutcome::Reseed { .. }
            ),
            "scoped facet + empty deny_paths → normal mtime reseed, not a refusal"
        );
    }

    /// AC2 refinement leg: an ingest whose only source is unscoped emits no
    /// refinement batch — the refusal, not a silent empty batch.
    #[test]
    fn unscoped_facet_emits_no_refinement_batch() {
        use crate::binding::BuildMode;
        use crate::ingest::refinement::next_batch;
        use crate::pipeline::IngestTrigger;

        let ws = tempfile::tempdir().unwrap();
        let root = ws.path();
        let cache = root.join(".memstead.cache").join("ingest");
        std::fs::write(root.join("a.rs"), "x").unwrap();

        let resolved = ResolvedIngest {
            name: "ing".to_string(),
            mode: BuildMode::Discovery,
            trigger: IngestTrigger::Loop,
            batch_size: 50,
            deny_paths: vec![],
            projection_ref: "m/p".to_string(),
            projection_mem: "m".to_string(),
            projection_name: "p".to_string(),
            intent: None,
            // Only source: an unscoped facet (no allow patterns).
            sources: vec![ResolvedSource::Primary(primary(vec![]))],
            destination_mem: "m".to_string(),
            rules: None,
            post_actions: None,
        };
        assert!(
            next_batch(
                &crate::Engine::from_mounts(Vec::new()).unwrap(),
                &resolved,
                root,
                &cache,
                20
            )
            .is_none(),
            "an all-unscoped ingest emits no refinement batch"
        );
    }

    /// AC3 (visible NoSignal) end-to-end through the cursor: a `signal:none`
    /// source and an unscoped source each contribute a distinct no-signal note;
    /// a first-seen (reseed) source does NOT — only no-signal reasons are
    /// noted. The rendered preface names `signal:none` explicitly and the
    /// unscoped reason distinctly.
    #[test]
    fn compute_source_cursor_notes_no_signal_reasons() {
        use crate::binding::BuildMode;
        use crate::pipeline::IngestTrigger;

        let engine = crate::Engine::from_mounts(Vec::new()).unwrap();
        // No `.git` over the workspace → mtime strategy for `auto`/`mtime`.
        let ws = tempfile::tempdir().unwrap();
        let root = ws.path();
        std::fs::write(root.join("a.rs"), "x").unwrap();

        let allow_rs = || {
            vec![PatternEntry {
                path: "**/*.rs".to_string(),
                mode: PatternMode::Allow,
            }]
        };
        let src = |facet: &str, declared: &str, scope: Vec<PatternEntry>| {
            ResolvedSource::Primary(Source {
                name: facet.to_string(),
                medium_type: MediumType::Filesystem,
                pointer: String::new(),
                change_detection: Some(declared.to_string()),
                scope,
                engagement: None,
                preparation: None,
            })
        };

        let resolved = ResolvedIngest {
            name: "ing".to_string(),
            mode: BuildMode::Discovery,
            trigger: IngestTrigger::Loop,
            batch_size: 20,
            deny_paths: vec![],
            projection_ref: "m/p".to_string(),
            projection_mem: "m".to_string(),
            projection_name: "p".to_string(),
            intent: None,
            sources: vec![
                // signal:none → DetectionNone note (even though it is scoped).
                src("plan", "none", allow_rs()),
                // mtime + no allows → Unscoped note.
                src("blind", "mtime", vec![]),
                // mtime + allows, first-seen → Reseed, NOT a no-signal note.
                src("watched", "mtime", allow_rs()),
            ],
            destination_mem: "m".to_string(),
            rules: None,
            post_actions: None,
        };

        let cursor = compute_source_cursor(&engine, &resolved, root);
        let reasons: BTreeMap<&str, NoSignalReason> = cursor
            .no_signal
            .iter()
            .map(|n| (n.source.as_str(), n.reason))
            .collect();
        assert_eq!(reasons.get("plan"), Some(&NoSignalReason::DetectionNone));
        assert_eq!(reasons.get("blind"), Some(&NoSignalReason::Unscoped));
        assert!(
            !reasons.contains_key("watched"),
            "a first-seen (reseed) source is not a no-signal note"
        );
        assert_eq!(cursor.no_signal.len(), 2);
        // The reseed source still produced a reseed command.
        assert!(cursor.reseed.iter().any(|c| c.key == "ing/watched#synced"));

        // The rendered preface names signal:none and the unscoped reason.
        let out = crate::ingest::brief::render_changed_slice(&cursor);
        assert!(out.contains("- `plan`: `signal:none`"));
        assert!(out.contains("- `blind`: unscoped facet"));
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

    /// Engine self-exclusion: `.memstead/**`, `.memstead.cache/**`, and
    /// every mount's resolved storage location (here a mem-repo at a
    /// NON-default directory name) are absent from the enumeration
    /// regardless of configuration — explicit allow globs covering them
    /// do not admit them.
    #[test]
    fn engine_state_never_enumerates_even_when_allowed() {
        let ws = tempfile::tempdir().unwrap();
        let root = ws.path();
        for rel in [
            ".memstead/state/findings/muehle/f.json",
            ".memstead/projections/muehle/f.json",
            ".memstead.cache/ingest/source-cursor/muehle/f/f.json",
            "custom-repo/README.md",
            "Allgemein/Protokoll.md",
            "Allgemein/Vertrag.md",
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "x").unwrap();
        }
        // Engine-managed workspace state resolving the mem-repo at
        // `custom-repo/` — the exclusion must key on this resolved
        // location, not on the literal default name `mem-repo/`.
        std::fs::write(
            root.join(".memstead/workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join(".memstead/state/mounts.json"),
            serde_json::json!({
                "format": "memstead-mounts-3",
                "mounts": [{
                    "mem": "muehle",
                    "schema": "default@1.0.0",
                    "storage": {
                        "type": "git-branch",
                        "gitdir": "custom-repo/.git",
                        "branch": "refs/heads/muehle"
                    },
                    "capability": "write",
                    "lifecycle": "eager",
                    "cross_linkable": true
                }]
            })
            .to_string(),
        )
        .unwrap();

        // Allow everything AND explicitly try to admit engine state.
        let source = primary(vec![
            PatternEntry {
                path: "**/*".to_string(),
                mode: PatternMode::Allow,
            },
            PatternEntry {
                path: ".memstead/**".to_string(),
                mode: PatternMode::Allow,
            },
            PatternEntry {
                path: "custom-repo/**".to_string(),
                mode: PatternMode::Allow,
            },
        ]);
        let got = enumerate_facet_files(&source, &[], root);
        assert_eq!(
            got,
            vec!["Allgemein/Protokoll.md", "Allgemein/Vertrag.md"],
            "only source artifacts may enter the denominator"
        );
    }

    /// The git strategy pushes the same engine-state excludes as
    /// pathspecs: a diff touching `.memstead/**` and the resolved
    /// mem-repo path yields a slice naming neither — denominator and
    /// slice stay strategy-invariant.
    #[test]
    fn git_slice_excludes_engine_state() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        git(root, &["init", "-q"]);
        std::fs::write(
            root.join("workspace.rs"), // placeholder so base commit is non-empty
            "x",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".memstead/state")).unwrap();
        std::fs::write(
            root.join(".memstead/workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join(".memstead/state/mounts.json"),
            serde_json::json!({
                "format": "memstead-mounts-3",
                "mounts": [{
                    "mem": "muehle",
                    "schema": "default@1.0.0",
                    "storage": {
                        "type": "git-branch",
                        "gitdir": "custom-repo/.git",
                        "branch": "refs/heads/muehle"
                    },
                    "capability": "write",
                    "lifecycle": "eager",
                    "cross_linkable": true
                }]
            })
            .to_string(),
        )
        .unwrap();
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

        // Move: one real file, one engine-state file, one mem-repo file.
        std::fs::write(root.join("real.md"), "signal").unwrap();
        std::fs::write(root.join(".memstead/state/findings.json"), "self").unwrap();
        std::fs::create_dir_all(root.join("custom-repo")).unwrap();
        std::fs::write(root.join("custom-repo/README.md"), "repo").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "move"]);

        let source = primary(vec![PatternEntry {
            path: "**/*".to_string(),
            mode: PatternMode::Allow,
        }]);
        match compute_git_slice(&source, &[], root, Some(&baseline)) {
            SliceOutcome::Changed { slice, .. } => {
                assert_eq!(
                    slice.added,
                    vec!["real.md"],
                    "engine state leaked: {slice:?}"
                );
                assert!(slice.modified.is_empty(), "{slice:?}");
            }
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    /// The dead-deny lint never flags the scaffold's own default hygiene
    /// entries (they can never match on a git-enumerated source — the
    /// engine must not call its own output a typo), while a user-authored
    /// entry that matches nothing keeps the loud warning and one that
    /// matches stays silent.
    #[test]
    fn dead_deny_lint_exempts_scaffold_defaults_but_not_user_typos() {
        use crate::binding::{BuildMode, DEFAULT_SCAFFOLD_DENY_PATHS};
        use crate::pipeline::IngestTrigger;

        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        git(root, &["init", "-q"]);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "code").unwrap();

        let mut deny_paths: Vec<String> = DEFAULT_SCAFFOLD_DENY_PATHS
            .iter()
            .map(|s| s.to_string())
            .collect();
        deny_paths.push("typo/**".to_string()); // user typo — matches nothing
        deny_paths.push("src/**".to_string()); // user entry that matches

        let resolved = ResolvedIngest {
            name: "ing".to_string(),
            mode: BuildMode::Discovery,
            trigger: IngestTrigger::Loop,
            batch_size: 20,
            deny_paths,
            projection_ref: "m/p".to_string(),
            projection_mem: "m".to_string(),
            projection_name: "p".to_string(),
            intent: None,
            sources: vec![ResolvedSource::Primary(primary(vec![PatternEntry {
                path: "**/*.rs".to_string(),
                mode: PatternMode::Allow,
            }]))],
            destination_mem: "m".to_string(),
            rules: None,
            post_actions: None,
        };

        let dead = dead_deny_entries(&resolved, root);
        assert_eq!(
            dead,
            vec!["typo/**".to_string()],
            "only the user typo is flagged — scaffold defaults and matching \
             entries stay silent"
        );
    }
}
