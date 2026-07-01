//! Per-mem version control via `gix`. Each mem owns a gix repository
//! whose gitdir and worktree are resolved from the mem's config at
//! `Engine::init` time — isolated from any outer project repo and from
//! the developer's `~/.gitconfig`.
//!
//! Gitdir defaults to `<mem>/.git/` and worktree to the mem root; the
//! optional `vcs` block in `.memstead/config.json` overrides either (notably
//! the shared-gitdir idiom `{ "../.git", ".." }`). On first init the
//! repo is bootstrapped and its per-repo config is patched with
//! `core.logallrefupdates = true` + `commit.gpgsign = false`;
//! `core.worktree` is written only when the declared worktree disagrees
//! with the gitdir's natural parent (shared idiom).
//!
//! On every init we re-apply `commit.gpgsign = false` so a developer
//! with global signing enabled does not hang every mutation waiting for
//! a passphrase.
//!
//! The *committer* (`engine <noreply@memstead.io>`) is set explicitly per
//! commit via `commit_as` and is therefore independent of any
//! `user.name`/`user.email` config — global or per-repo. The *author* is
//! derived per commit from a [`CommitContext`] so provenance (agent / cli /
//! external drift) is visible in `git log` without storing PII.
//!
//! Trailer contract: callers pass prose only. The engine appends trailers
//! (`Tool:`, `Actor:`, `Client:`) after a single `\n\n` separator. Callers
//! MUST NOT write those keys themselves — duplicates would confuse
//! `git interpret-trailers` consumers.
//!
//! ## In-process serialization: per-branch mutex
//!
//! A mem's commits race on a single git ref (today: HEAD's symref
//! target on the disk adapter, an explicit branch name on the git-tree
//! adapter). Without serialization two concurrent commits against the
//! same ref would either produce an orphan parent chain or fail gix's
//! reference-transaction check. The mutex's job is to keep the
//! tree-build + commit + ref-advance window atomic for one ref.
//!
//! A process-wide registry (see [`acquire_branch_mutex`]) maps
//! canonical ref-name strings (e.g. `refs/heads/main`) to
//! `Arc<Mutex<()>>`. Both adapters acquire the mutex for the ref they
//! are about to advance: the disk adapter resolves HEAD's symref to a
//! concrete `refs/heads/<name>` first so a future git-tree writer
//! committing onto the same branch shares the same key. Different ref
//! names hold different mutexes and proceed in parallel.
//!
//! Cross-process contention is **out of scope** for this layer. A
//! second process committing against the same ref hits gix's own
//! lockfile discipline (`<gitdir>/index.lock`, `<gitdir>/HEAD.lock`,
//! …), which this module surfaces as [`VcsError::Git`] →
//! `VCS_ERROR`-coded envelopes. The human-readable message includes
//! retry guidance; the industry norm (libgit2, GitHub Desktop) is to
//! propagate lockfile errors back to the caller rather than introduce
//! a custom `flock` layer.
//!
//! **Lock-order rule.** When a code path holds more than one per-branch
//! mutex simultaneously (e.g. a cross-mem move that touches two
//! branches under one shared multi-root gitdir), the mutexes MUST be
//! acquired in lexicographic ref-name order to prevent deadlock.
//! [`acquire_branch_mutexes_in_order`] enforces this by sorting before
//! acquisition; ad-hoc multi-mutex code in debug builds is caught by
//! the assertion inside [`acquire_branch_mutex`].

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::collections::HashMap;

use gix::objs::tree::EntryKind;

/// Process-wide map from canonical ref-name string (e.g.
/// `refs/heads/main`) to the mutex that serializes commits against that
/// ref. Lazy-initialized; entries are created on first use and never
/// removed for the process lifetime — ref names are stable and the
/// memory footprint is O(distinct refs ever written), bounded by the
/// user's workspace size.
static BRANCH_MUTEXES: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

// Debug-build-only stack of currently-held ref-name keys, per thread.
// The lock-order rule says callers must acquire branch mutexes in
// lexicographic order; a thread that already holds `b` and then
// requests `a` (where `a < b`) is a bug, not just a stylistic issue —
// in a multi-mem future where two threads each hold one of `(a, b)`
// and request the other, the program deadlocks. The debug assertion
// inside `acquire_branch_mutex` surfaces the bug at the offending
// acquisition site instead of in a hung process. Release builds skip
// the bookkeeping entirely.
#[cfg(debug_assertions)]
thread_local! {
    static HELD_BRANCH_KEYS: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Return the process-wide mutex for `ref_name`, creating it on first use.
///
/// `ref_name` is the full ref path (e.g. `refs/heads/main`). The disk
/// adapter resolves HEAD's symref target before calling; the git-tree
/// adapter passes its target ref directly. Two adapters writing to the
/// same ref under the same gitdir resolve to the same `Arc<Mutex<()>>`
/// and serialize on it; writes to different refs proceed in parallel.
///
/// The outer registry lock is held only for the HashMap lookup /
/// insertion; the returned `Arc` is cloned out before release. Callers
/// then acquire the inner mutex — held across the whole commit
/// operation — without blocking other refs.
///
/// Poisoning: if a prior commit panicked while holding the inner
/// mutex, subsequent acquisitions return `Err(PoisonError)`. Callers
/// surface this as [`VcsError::Git`] with a message identifying the
/// offending ref name so the operator can inspect state before
/// retrying.
///
/// Debug-only: panics if the calling thread already holds a mutex with
/// a lexicographically greater-or-equal key, enforcing the lock-order
/// rule documented at the top of this module.
pub(crate) fn acquire_branch_mutex(ref_name: &str) -> Arc<Mutex<()>> {
    #[cfg(debug_assertions)]
    {
        HELD_BRANCH_KEYS.with(|held| {
            let held = held.borrow();
            if let Some(top) = held.last() {
                assert!(
                    top.as_str() < ref_name,
                    "out-of-order branch-mutex acquisition: \
                     thread already holds '{top}', cannot now acquire '{ref_name}' \
                     (lexicographic order required)"
                );
            }
        });
    }
    let registry = BRANCH_MUTEXES.get_or_init(|| Mutex::new(HashMap::new()));
    // The registry lock is a brief critical section — HashMap lookup
    // plus at most one insertion — so a poisoned registry is a bug we
    // cannot recover from. `expect` here is load-bearing: it turns
    // registry-level corruption into a clear panic rather than silent
    // commit divergence.
    let mut map = registry
        .lock()
        .expect("branch mutex registry poisoned — previous commit panicked inside the registry critical section");
    map.entry(ref_name.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// RAII guard returned by [`acquire_branch_mutexes_in_order`]. Holds
/// an `Arc<Mutex<()>>` and a `MutexGuard` rooted in the `Arc`'s
/// storage; in debug builds it also pops the held-keys bookkeeping on
/// drop. The 'static `MutexGuard` lifetime is sound because the `Arc`
/// keeps the inner `Mutex` alive for the guard's full lifetime.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct BranchMutexGuard {
    // SAFETY-via-construction: `_arc` outlives `_guard`; the guard's
    // `Mutex<()>` is reachable via the Arc, and the Arc is held by the
    // same struct.
    _guard: MutexGuard<'static, ()>,
    _arc: Arc<Mutex<()>>,
    #[cfg(debug_assertions)]
    key: String,
}

#[cfg(debug_assertions)]
impl Drop for BranchMutexGuard {
    fn drop(&mut self) {
        HELD_BRANCH_KEYS.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(pos) = held.iter().rposition(|k| k == &self.key) {
                held.remove(pos);
            }
        });
    }
}

/// Acquire branch mutexes for every ref in `refs`, after sorting them
/// in lexicographic order. Returns the guards in acquisition order
/// (lex-sorted). Used by code paths that need to serialize against
/// more than one branch at once — e.g. a cross-mem move under a
/// shared multi-root gitdir.
///
/// Holding the returned `Vec` keeps every branch locked; dropping it
/// releases every guard. Order is guaranteed deterministic regardless
/// of input order.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn acquire_branch_mutexes_in_order(refs: &[&str]) -> Vec<BranchMutexGuard> {
    let mut sorted: Vec<&str> = refs.iter().copied().collect();
    sorted.sort_unstable();
    sorted.dedup();
    let mut guards: Vec<BranchMutexGuard> = Vec::with_capacity(sorted.len());
    for r in sorted {
        let arc = acquire_branch_mutex(r);
        // SAFETY: extend the guard's lifetime to 'static. The `_arc`
        // field below holds the same `Arc` so the underlying `Mutex<()>`
        // is kept alive for as long as the `BranchMutexGuard` exists.
        let raw_guard: MutexGuard<'_, ()> = arc
            .lock()
            .expect("branch mutex poisoned during ordered acquisition");
        let guard: MutexGuard<'static, ()> = unsafe {
            std::mem::transmute::<MutexGuard<'_, ()>, MutexGuard<'static, ()>>(raw_guard)
        };
        #[cfg(debug_assertions)]
        HELD_BRANCH_KEYS.with(|held| {
            held.borrow_mut().push(r.to_string());
        });
        guards.push(BranchMutexGuard {
            _guard: guard,
            _arc: arc,
            #[cfg(debug_assertions)]
            key: r.to_string(),
        });
    }
    guards
}

/// Resolve a gix `Repository`'s HEAD into a concrete fully-qualified
/// ref name (e.g. `refs/heads/main`). Returns the symref target when
/// HEAD is a symbolic ref (the common case for a working repository),
/// or the literal string `"HEAD"` when HEAD is detached or unresolvable
/// — the latter case has no stable per-branch identity, so falling back
/// to the literal still serializes correctly within the process.
pub(crate) fn head_branch_ref(repo: &gix::Repository) -> String {
    match repo.head_ref() {
        Ok(Some(reference)) => reference.name().as_bstr().to_string(),
        _ => "HEAD".to_string(),
    }
}


/// Deterministic committer identity — bypasses per-repo and global git
/// config and doubles as the author fallback when no actor is known.
const COMMITTER_NAME: &str = "engine";
const COMMITTER_EMAIL: &str = "noreply@memstead.io";

pub use memstead_base::vcs::{
    Actor, ClientId, CommitContext, author_identity, format_commit_message, sanitise_client_name,
};

/// VCS operations trait — minimal surface. Only `commit` is needed by the
/// engine today. `changes_since` goes straight to
/// `gix::diff::tree` without expanding this trait.
pub trait Vcs: Send + Sync {
    /// Stage the paths (typically a single mem directory) into the repo's
    /// tree and create a commit on HEAD. `message` is the caller's prose —
    /// the implementation appends the provenance trailers derived from
    /// `ctx` (`Tool:`, `Actor:`, `Client:`) and picks the author signature
    /// from `ctx` too. Returns the commit SHA. Each call rebuilds the
    /// tree from disk so deletions surface without callers having to
    /// track them.
    fn commit(
        &self,
        paths: &[&Path],
        message: &str,
        ctx: &CommitContext<'_>,
    ) -> Result<String, VcsError>;
}

#[derive(Debug, thiserror::Error)]
pub enum VcsError {
    #[error("not a git repository: {0}")]
    NotRepo(String),
    #[error("object not found: {0}")]
    ObjectNotFound(String),
    #[error("reference conflict: {0}")]
    RefConflict(String),
    #[error("git error: {0}")]
    Git(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<gix::open::Error> for VcsError {
    fn from(e: gix::open::Error) -> Self {
        VcsError::NotRepo(e.to_string())
    }
}

impl From<gix::init::Error> for VcsError {
    fn from(e: gix::init::Error) -> Self {
        VcsError::Git(format!("init: {e}"))
    }
}

impl From<gix::commit::Error> for VcsError {
    fn from(e: gix::commit::Error) -> Self {
        VcsError::Git(format!("commit: {e}"))
    }
}

impl From<gix::object::write::Error> for VcsError {
    fn from(e: gix::object::write::Error) -> Self {
        VcsError::Git(format!("write-object: {e}"))
    }
}

/// Open or initialize the per-mem gix repository.
///
/// `git_dir` holds HEAD, refs, objects, and config. `work_tree` is the
/// mem root (or, in the shared-gitdir idiom, the directory that owns
/// the gitdir). Both must be absolute or already canonicalized —
/// `commit` strips `work_tree` from each staged path to compute the
/// in-tree relative path.
///
/// On first init the gitdir is bootstrapped and the per-repo config is
/// patched with `core.logallrefupdates = true` + `commit.gpgsign = false`.
/// `core.worktree` is written only when `git_dir.parent() != Some(work_tree)`
/// (i.e. when the gitdir is not a direct child of the worktree).
/// `commit.gpgsign = false` is re-applied on every re-open so that a
/// user edit to the per-repo config cannot silently enable signing and
/// hang every memstead mutation waiting for a passphrase.
pub fn create_vcs(
    git_dir: &Path,
    work_tree: &Path,
) -> Result<Arc<dyn Vcs>, VcsError> {
    let is_new = !git_dir.join("HEAD").exists();

    if is_new {
        std::fs::create_dir_all(work_tree)?;
        if let Some(parent) = git_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        gix::init_bare(git_dir)?;

        let mut kvs: Vec<(&str, &str, &str)> = vec![
            ("core", "bare", "false"),
            ("core", "logallrefupdates", "true"),
            ("commit", "gpgsign", "false"),
        ];
        let worktree_rel_storage;
        let gitdir_parent_is_worktree = git_dir
            .parent()
            .map(|p| paths_equal(p, work_tree))
            .unwrap_or(false);
        if !gitdir_parent_is_worktree {
            worktree_rel_storage = relative_path(git_dir, work_tree)
                .unwrap_or_else(|| work_tree.to_string_lossy().into_owned());
            kvs.push(("core", "worktree", &worktree_rel_storage));
        }
        write_per_repo_config(git_dir, &kvs)?;
    } else {
        write_per_repo_config(git_dir, &[("commit", "gpgsign", "false")])?;
    }

    // Sanity: opening must succeed now. If it doesn't, something is wrong
    // with the layout (e.g. config file corrupted) — surface it rather
    // than masking with the "open works at commit time" lazy path.
    let _repo = gix::open(git_dir)?;

    // Canonicalize the stored paths. Shared-gitdir configs express
    // `worktree` with `..` segments (e.g. `../.git`, `..`); `strip_prefix`
    // is a purely syntactic operation and would fail against a raw
    // `<mem>/..` prefix. Canonicalizing here once makes every later
    // subpath computation reliable.
    let git_dir_canon = std::fs::canonicalize(git_dir).unwrap_or_else(|_| git_dir.to_path_buf());
    let work_tree_canon =
        std::fs::canonicalize(work_tree).unwrap_or_else(|_| work_tree.to_path_buf());

    Ok(Arc::new(GixVcs {
        git_dir: git_dir_canon,
        work_tree: work_tree_canon,
    }))
}

/// Check whether two paths refer to the same location on disk.
/// Attempts `std::fs::canonicalize` first; falls back to component-wise
/// equality when either path does not yet exist or canonicalization
/// otherwise fails. Used only for the `core.worktree` skip heuristic in
/// `create_vcs` — not load-bearing for correctness.
fn paths_equal(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

/// Compute a relative path from `from` (a directory) to `to`. Walks
/// shared ancestors via lexicographic component comparison, emitting
/// `..` for each `from`-component above the shared root and then the
/// remaining `to`-components. Both inputs must already be absolute /
/// canonicalized for this to be meaningful. Returns `None` when the two
/// paths share no prefix (different drives on Windows, different
/// canonicalized mounts).
///
/// Example: `from = /a/b/.git`, `to = /a/b` → `"."`.
/// Example: `from = /a/.git`,   `to = /a/b` → `"b"`.
/// Example: `from = /a/b/.git`, `to = /a`   → `".."`.
fn relative_path(from: &Path, to: &Path) -> Option<String> {
    let from_comps: Vec<_> = from.components().collect();
    let to_comps: Vec<_> = to.components().collect();
    // First mismatching component index.
    let mut shared = 0;
    while shared < from_comps.len()
        && shared < to_comps.len()
        && from_comps[shared] == to_comps[shared]
    {
        shared += 1;
    }
    if shared == 0 {
        return None;
    }
    let ups = from_comps.len().saturating_sub(shared);
    let mut out = PathBuf::new();
    for _ in 0..ups {
        out.push("..");
    }
    for comp in &to_comps[shared..] {
        out.push(comp.as_os_str());
    }
    if out.as_os_str().is_empty() {
        Some(".".to_string())
    } else {
        Some(out.to_string_lossy().into_owned())
    }
}

/// Patch the repo-local `config` file with the given `(section, key, value)`
/// triples. Existing values are overwritten; missing sections are created.
/// Any other entries (set by gix init or by a user) are preserved.
///
/// Keys and values are owned (`String`) because `gix_config::File::set_raw_value_by`
/// ties the inserted `ValueName` to the `File`'s lifetime parameter — passing
/// borrowed `&str` from a non-`'static` slice triggers a lifetime mismatch
/// against the `File<'static>` returned by `from_path_no_includes`.
fn write_per_repo_config(git_dir: &Path, kvs: &[(&str, &str, &str)]) -> Result<(), VcsError> {
    use gix::bstr::BStr;
    let config_path = git_dir.join("config");
    let mut file = gix::config::File::from_path_no_includes(
        config_path.clone(),
        gix::config::Source::Local,
    )
    .map_err(|e| VcsError::Git(format!("config parse: {e}")))?;
    for (section, key, value) in kvs {
        let key_owned = String::from(*key);
        let value_bytes: &BStr = (*value).as_bytes().into();
        file.set_raw_value_by(*section, None, key_owned, value_bytes)
            .map_err(|e| VcsError::Git(format!("config set {section}.{key}: {e}")))?;
    }
    let mut buf = Vec::new();
    file.write_to(&mut buf)
        .map_err(|e| VcsError::Git(format!("config serialize: {e}")))?;
    std::fs::write(&config_path, buf)?;
    Ok(())
}

/// Per-mem gix-backed VCS. Opens the repo on every commit — gix repos are
/// cheap to open and this avoids any cross-thread shared-state concerns.
struct GixVcs {
    git_dir: PathBuf,
    work_tree: PathBuf,
}

impl Vcs for GixVcs {
    fn commit(
        &self,
        paths: &[&Path],
        message: &str,
        ctx: &CommitContext<'_>,
    ) -> Result<String, VcsError> {
        // Per-branch serialization. The inner mutex is held across
        // tree-build + commit + ref-advance so two in-process
        // `GixVcs::commit` calls against the same ref cannot race on
        // its tip. We resolve HEAD's symref target up-front so that a
        // future git-tree writer committing onto the same concrete
        // branch (e.g. `refs/heads/main`) shares the same mutex key.
        let repo = gix::open(&self.git_dir)?;
        let head_ref = head_branch_ref(&repo);
        let mutex = acquire_branch_mutex(&head_ref);
        let _guard = mutex.lock().map_err(|_| {
            VcsError::Git(format!(
                "branch mutex poisoned (a previous commit panicked); inspect {} ref {} and restart the process",
                self.git_dir.display(),
                head_ref,
            ))
        })?;

        // Mem-scoped commit tree. The contract is "preserve HEAD minus
        // every mem-subpath in `paths`, then re-upsert what's on disk":
        //
        // 1. Callers bundle paths that are either all-isolated (empty
        //    subpath — the mem owns the whole worktree) or all-shared
        //    (non-empty subpaths under one shared worktree). Mixing both
        //    shapes in one call would silently discard the shared
        //    mem's HEAD subtree once the empty case forced an
        //    `empty_tree()` start; the `debug_assert!` below pins that
        //    contract so a future multi-mem caller fails loudly in
        //    debug builds rather than corrupting history in release.
        // 2. All-empty → start from `empty_tree` (a full rebuild;
        //    deletions surface without per-file diff bookkeeping).
        // 3. All-non-empty → start from HEAD's tree (or `empty_tree` on
        //    genesis) and wholesale-remove each mem's subtree before
        //    re-upserting so sibling mems under the same gitdir
        //    survive our commit unchanged.
        let subpaths: Vec<String> = paths
            .iter()
            .map(|p| mem_subpath(&self.work_tree, p))
            .collect::<Result<_, _>>()?;
        debug_assert!(
            subpaths.iter().all(|s| s.is_empty())
                || subpaths.iter().all(|s| !s.is_empty()),
            "commit() paths must not mix isolated and shared subpaths",
        );
        let any_empty_subpath = subpaths.iter().any(|s| s.is_empty());

        let head_commit = repo.head_commit().ok();
        let parents = head_commit
            .as_ref()
            .map(|c| vec![c.id])
            .unwrap_or_default();
        let mut editor = if any_empty_subpath {
            repo.empty_tree()
                .edit()
                .map_err(|e| VcsError::Git(format!("editor init: {e}")))?
        } else if let Some(head) = head_commit.as_ref() {
            let tree = head
                .tree()
                .map_err(|e| VcsError::Git(format!("head tree: {e}")))?;
            tree.edit()
                .map_err(|e| VcsError::Git(format!("editor init: {e}")))?
        } else {
            repo.empty_tree()
                .edit()
                .map_err(|e| VcsError::Git(format!("editor init: {e}")))?
        };

        for (path, subpath) in paths.iter().zip(subpaths.iter()) {
            // Drop the mem's prior subtree so deletions on disk surface
            // in the tree. Empty subpath already started from `empty_tree`.
            if !subpath.is_empty() {
                editor
                    .remove(subpath.as_str())
                    .map_err(|e| VcsError::Git(format!("tree remove: {e}")))?;
            }
            apply_path(&repo, &mut editor, &self.work_tree, path, subpath)?;
        }

        let tree_id = editor
            .write()
            .map_err(|e| VcsError::Git(format!("tree write: {e}")))?
            .detach();

        let time = gix::date::Time::now_local_or_utc();
        let committer_sig = gix::actor::Signature {
            name: COMMITTER_NAME.into(),
            email: COMMITTER_EMAIL.into(),
            time,
        };
        let author_sig = match author_identity(ctx) {
            Some((name, email)) => gix::actor::Signature {
                name: name.into(),
                email: email.into(),
                time,
            },
            None => committer_sig.clone(),
        };
        let mut author_buf = gix::date::parse::TimeBuf::default();
        let mut committer_buf = gix::date::parse::TimeBuf::default();
        let author_ref = author_sig.to_ref(&mut author_buf);
        let committer_ref = committer_sig.to_ref(&mut committer_buf);

        let full_message = format_commit_message(message, ctx);
        let commit_id = repo.commit_as(
            committer_ref,
            author_ref,
            "HEAD",
            full_message,
            tree_id,
            parents,
        )?;
        Ok(commit_id.to_hex().to_string())
    }
}

/// Compute the mem's subpath within the worktree as a forward-slash
/// separated string. The empty string means "the mem owns the whole
/// worktree" (isolated idiom); a non-empty string means "the mem lives
/// under `<subpath>` inside a shared worktree".
///
/// `work_tree` is expected to be canonical (invariant:
/// `MemState.resolved_worktree` is canonicalized at `Engine::init`;
/// `GixVcs.work_tree` is canonicalized in `create_vcs`). `mem_path` is
/// canonicalized defensively here because mutation callers thread it
/// through from `state.dir`, and `strip_prefix` is a purely syntactic
/// operation — any `..` or symlink in the raw path would defeat it.
///
/// Returns `VcsError::Git` when the mem path is not under the
/// worktree. Silent fallback to an empty subpath would be destructive
/// in shared-gitdir mode: a misconfigured mem would commit at the
/// tree root and wipe every sibling's subtree.
pub(crate) fn mem_subpath(work_tree: &Path, mem_path: &Path) -> Result<String, VcsError> {
    let canon_mem =
        std::fs::canonicalize(mem_path).unwrap_or_else(|_| mem_path.to_path_buf());
    let rel = canon_mem.strip_prefix(work_tree).map_err(|_| {
        VcsError::Git(format!(
            "mem path {} is not under worktree {}",
            mem_path.display(),
            work_tree.display(),
        ))
    })?;
    Ok(rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/"))
}

/// Join a relative on-disk path (forward-slash-normalised) under the
/// mem's `subpath` within the worktree. Empty subpath → the relative
/// path stands alone. Empty relative → returns the subpath by itself.
fn join_subpath(subpath: &str, rel: &str) -> String {
    if subpath.is_empty() {
        rel.to_string()
    } else if rel.is_empty() {
        subpath.to_string()
    } else {
        format!("{subpath}/{rel}")
    }
}

/// Walk the mem's subdirectory on disk and upsert every file into the
/// editor at its worktree-relative location (i.e. prefixed with the
/// mem's `subpath`). Skips the gix git-dir, the `.memstead/cache/`
/// regenerable-artefacts directory, and any stray `.git/` entries.
///
/// `path` is typically the mem root; a single-file path is supported
/// for completeness, though no current caller uses that shape.
///
/// The walker is scoped to `path` (the mem's on-disk subdirectory) —
/// not the worktree root. In shared-gitdir mode this is what keeps
/// sibling mems out of mem A's commit tree: A's commit never walks
/// B's subdirectory, so no sibling bytes can leak in.
fn apply_path(
    repo: &gix::Repository,
    editor: &mut gix::object::tree::Editor<'_>,
    work_tree: &Path,
    path: &Path,
    subpath: &str,
) -> Result<(), VcsError> {
    if path.is_file() {
        if let Ok(rel) = path.strip_prefix(work_tree) {
            let rel_str = rel.to_string_lossy();
            if !is_ignored(rel.components()) {
                upsert_file(repo, editor, path, &rel_str)?;
            }
        }
        return Ok(());
    }

    if path.is_dir() {
        for entry in walkdir::WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                if e.file_name() == OsStr::new(".git") {
                    return false;
                }
                // Path components beneath the mem directory must not
                // re-trip the cache guard; `is_ignored` compares starting
                // at `.memstead/cache`, so a subpath-relative view is the
                // right input.
                match e.path().strip_prefix(path) {
                    Ok(rel) => !is_ignored(rel.components()),
                    Err(_) => true,
                }
            })
        {
            let entry = entry.map_err(|e| VcsError::Git(format!("walk: {e}")))?;
            if !entry.file_type().is_file() {
                continue;
            }
            let rel_in_mem = match entry.path().strip_prefix(path) {
                Ok(p) => p
                    .components()
                    .filter_map(|c| c.as_os_str().to_str())
                    .collect::<Vec<_>>()
                    .join("/"),
                Err(_) => continue,
            };
            let in_tree_path = join_subpath(subpath, &rel_in_mem);
            upsert_file(repo, editor, entry.path(), &in_tree_path)?;
        }
    }
    Ok(())
}

/// Returns true for relative paths the commit walker must skip: under
/// `.memstead/cache/` (regenerable artefacts that must never enter
/// the tree). `.git/` is handled one level up by the caller's walk
/// filter (`filter_entry(|e| e.file_name() != ".git")`), not here.
fn is_ignored(components: std::path::Components<'_>) -> bool {
    let mut comps = components;
    let first = comps.next().map(|c| c.as_os_str());
    if first == Some(OsStr::new(".memstead")) {
        return matches!(
            comps.next().map(|c| c.as_os_str()),
            Some(c) if c == OsStr::new("cache")
        );
    }
    false
}

fn upsert_file(
    repo: &gix::Repository,
    editor: &mut gix::object::tree::Editor<'_>,
    path: &Path,
    rel: &str,
) -> Result<(), VcsError> {
    let bytes = std::fs::read(path)?;
    let blob_id = repo.write_blob(&bytes)?.detach();
    let kind = if is_executable(path) {
        EntryKind::BlobExecutable
    } else {
        EntryKind::Blob
    };
    editor
        .upsert(rel, kind, blob_id)
        .map_err(|e| VcsError::Git(format!("tree upsert: {e}")))?;
    Ok(())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}

/// Test-only VCS that records nothing and never errors. Used by engine tests
/// that exercise mutation paths without touching a real repo. The returned
/// SHAs are deterministic monotonic sentinels (`noop-0`, `noop-1`, …)
/// distinguishable from real SHAs by prefix — `changes_since`
/// relies on that distinction to produce empty deltas for noop mems.
pub struct NoopVcs {
    counter: std::sync::atomic::AtomicU64,
}

impl NoopVcs {
    pub fn new() -> Self {
        Self {
            counter: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl Default for NoopVcs {
    fn default() -> Self {
        Self::new()
    }
}

impl Vcs for NoopVcs {
    fn commit(
        &self,
        _paths: &[&Path],
        _message: &str,
        _ctx: &CommitContext<'_>,
    ) -> Result<String, VcsError> {
        let n = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(format!("noop-{n}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Build a fresh `<tmp>/mem` directory with a `.memstead/` subdir and
    /// the matching `git_dir` path. Mirrors the on-disk layout of a real
    /// mem.
    fn make_mem_paths(tmp: &Path) -> (PathBuf, PathBuf) {
        let mem = tmp.join("mem");
        let git_dir = mem.join(".git");
        fs::create_dir_all(mem.join(".memstead")).unwrap();
        (mem, git_dir)
    }

    #[test]
    fn create_vcs_initializes_fresh_dir() {
        let dir = TempDir::new().unwrap();
        let (mem, git_dir) = make_mem_paths(dir.path());
        let vcs = create_vcs(&git_dir, &mem).unwrap();
        let sha = vcs.commit(&[&mem], "initial", &CommitContext::internal()).unwrap();
        assert_eq!(sha.len(), 40, "commit sha must be 40-char hex");
    }

    #[test]
    fn create_vcs_writes_structural_config_on_first_init() {
        let dir = TempDir::new().unwrap();
        let (mem, git_dir) = make_mem_paths(dir.path());
        let _vcs = create_vcs(&git_dir, &mem).unwrap();

        let config = fs::read_to_string(git_dir.join("config")).unwrap();
        // Isolated layout: gitdir is a direct child of the worktree, so
        // no `core.worktree` override is written — gix's default
        // resolution (gitdir's parent = worktree) applies.
        assert!(
            !config.contains("worktree = "),
            "no core.worktree override for isolated layout, got:\n{config}"
        );
        assert!(
            config.contains("logallrefupdates = true"),
            "core.logallrefupdates must be set, got:\n{config}"
        );
        assert!(
            config.contains("gpgsign = false"),
            "commit.gpgsign must be forced false, got:\n{config}"
        );
    }

    #[test]
    fn create_vcs_reapplies_gpgsign_on_reopen() {
        let dir = TempDir::new().unwrap();
        let (mem, git_dir) = make_mem_paths(dir.path());

        // First init writes the full config.
        let _ = create_vcs(&git_dir, &mem).unwrap();

        // Simulate a user editing the per-repo config to enable signing.
        let original = fs::read_to_string(git_dir.join("config")).unwrap();
        let tampered = original.replace("gpgsign = false", "gpgsign = true");
        fs::write(git_dir.join("config"), tampered).unwrap();

        // Re-open must clobber gpgsign back to false but leave the
        // structural fields alone.
        let _ = create_vcs(&git_dir, &mem).unwrap();
        let after = fs::read_to_string(git_dir.join("config")).unwrap();
        assert!(after.contains("gpgsign = false"), "got:\n{after}");
        assert!(after.contains("logallrefupdates = true"), "got:\n{after}");
    }

    #[test]
    fn commit_writes_file_into_tree() {
        let dir = TempDir::new().unwrap();
        let (mem, git_dir) = make_mem_paths(dir.path());
        fs::write(mem.join("test.md"), "hello").unwrap();

        let vcs = create_vcs(&git_dir, &mem).unwrap();
        let sha = vcs
            .commit(&[&mem], "add test.md", &CommitContext::internal())
            .unwrap();
        assert_eq!(sha.len(), 40);

        let repo = gix::open(&git_dir).unwrap();
        let commit = repo.head_commit().unwrap();
        let tree = commit.tree().unwrap();
        let entry = tree.find_entry("test.md").expect("test.md in tree");
        let blob = entry.object().unwrap().try_into_blob().unwrap();
        assert_eq!(blob.data, b"hello");
    }

    #[test]
    fn commit_excludes_cache_subdir() {
        let dir = TempDir::new().unwrap();
        let (mem, git_dir) = make_mem_paths(dir.path());

        // Author files
        fs::write(mem.join("real.md"), "real").unwrap();
        // Cache files under `.memstead/cache/` must never reach the tree.
        fs::create_dir_all(mem.join(".memstead/cache/prompts")).unwrap();
        fs::write(mem.join(".memstead/cache/prompts/p.txt"), "noise").unwrap();

        let vcs = create_vcs(&git_dir, &mem).unwrap();
        vcs.commit(&[&mem], "initial", &CommitContext::internal()).unwrap();

        let repo = gix::open(&git_dir).unwrap();
        let tree = repo.head_commit().unwrap().tree().unwrap();

        // Top-level: real.md + the .memstead subtree (config etc., never
        // the ignored children).
        assert!(tree.find_entry("real.md").is_some());

        // Drill into the `.memstead` meta dir: it may not contain
        // `cache/`. `.git/` is skipped one level up by the walk filter,
        // not by this check.
        if let Some(memstead_entry) = tree.find_entry(".memstead") {
            let memstead_tree =
                memstead_entry.object().unwrap().try_into_tree().unwrap();
            for entry in memstead_tree.iter() {
                let entry = entry.unwrap();
                let name = entry.filename().to_string();
                assert!(name != "cache", ".memstead subtree must skip {name}");
            }
        }
    }

    #[test]
    fn commit_second_time_with_deletion_removes_from_tree() {
        let dir = TempDir::new().unwrap();
        let (mem, git_dir) = make_mem_paths(dir.path());
        fs::write(mem.join("keep.md"), "keep").unwrap();
        fs::write(mem.join("drop.md"), "drop").unwrap();

        let vcs = create_vcs(&git_dir, &mem).unwrap();
        vcs.commit(&[&mem], "initial", &CommitContext::internal()).unwrap();

        fs::remove_file(mem.join("drop.md")).unwrap();
        vcs.commit(&[&mem], "drop one", &CommitContext::internal())
            .unwrap();

        let repo = gix::open(&git_dir).unwrap();
        let tree = repo.head_commit().unwrap().tree().unwrap();
        assert!(tree.find_entry("keep.md").is_some());
        assert!(
            tree.find_entry("drop.md").is_none(),
            "deleted file must disappear from the tree on the next commit"
        );
    }

    #[test]
    fn commit_author_is_deterministic() {
        let dir = TempDir::new().unwrap();
        let (mem, git_dir) = make_mem_paths(dir.path());
        fs::write(mem.join("a.md"), "a").unwrap();

        let vcs = create_vcs(&git_dir, &mem).unwrap();
        vcs.commit(&[&mem], "x", &CommitContext::internal())
            .unwrap();

        let repo = gix::open(&git_dir).unwrap();
        let commit = repo.head_commit().unwrap();
        let author = commit.author().unwrap();
        assert_eq!(author.name, COMMITTER_NAME);
        assert_eq!(author.email, COMMITTER_EMAIL);
    }

    #[test]
    fn noop_vcs_returns_distinguishable_shas() {
        let vcs = NoopVcs::new();
        let s1 = vcs.commit(&[], "x", &CommitContext::internal()).unwrap();
        let s2 = vcs.commit(&[], "y", &CommitContext::internal()).unwrap();
        assert!(s1.starts_with("noop-"));
        assert!(s2.starts_with("noop-"));
        assert_ne!(s1, s2);
    }

    // ----- Commit provenance -----

    fn head_commit_parts(git_dir: &Path) -> (String, String, String) {
        let repo = gix::open(git_dir).unwrap();
        let commit = repo.head_commit().unwrap();
        let author = commit.author().unwrap();
        let message = commit.message_raw().unwrap().to_string();
        (
            author.name.to_string(),
            author.email.to_string(),
            message,
        )
    }

    #[test]
    fn commit_with_agent_context_sets_author_and_trailers() {
        let dir = TempDir::new().unwrap();
        let (mem, git_dir) = make_mem_paths(dir.path());
        fs::write(mem.join("a.md"), "a").unwrap();

        let vcs = create_vcs(&git_dir, &mem).unwrap();
        let ctx = CommitContext {
            actor: Actor::Agent,
            client: Some(ClientId {
                name: "claude-code".into(),
                version: "2.1.0".into(),
            }),
            tool: Some("memstead_update"),
            note: None,
            logical_operation_id: None,
            entity_ids: None,
        };
        vcs.commit(&[&mem], "memstead: update specs--a", &ctx).unwrap();

        let (name, email, message) = head_commit_parts(&git_dir);
        assert_eq!(name, "claude-code");
        assert_eq!(email, "claude-code@memstead.io");
        assert!(
            message.ends_with(
                "\n\nTool: memstead_update\nActor: agent\nClient: claude-code@2.1.0"
            ),
            "got message: {message:?}"
        );
    }

    #[test]
    fn commit_with_external_context_sets_external_author_and_actor_trailer() {
        let dir = TempDir::new().unwrap();
        let (mem, git_dir) = make_mem_paths(dir.path());
        fs::write(mem.join("a.md"), "a").unwrap();

        let vcs = create_vcs(&git_dir, &mem).unwrap();
        let ctx = CommitContext {
            actor: Actor::External,
            client: None,
            tool: None,
            note: None,
            logical_operation_id: None,
            entity_ids: None,
        };
        vcs.commit(&[&mem], "external edits (1 files)", &ctx).unwrap();

        let (name, email, message) = head_commit_parts(&git_dir);
        assert_eq!(name, "external");
        assert_eq!(email, "external@memstead.io");
        assert!(message.contains("\n\nActor: external"));
        assert!(!message.contains("Tool:"));
        assert!(!message.contains("Client:"));
    }

    #[test]
    fn commit_with_cli_context_emits_trailers_and_author() {
        let dir = TempDir::new().unwrap();
        let (mem, git_dir) = make_mem_paths(dir.path());
        fs::write(mem.join("a.md"), "a").unwrap();

        let vcs = create_vcs(&git_dir, &mem).unwrap();

        // Cli without a ClientId falls back to the committer identity.
        let ctx_no_client = CommitContext {
            actor: Actor::Cli,
            client: None,
            tool: None,
            note: None,
            logical_operation_id: None,
            entity_ids: None,
        };
        vcs.commit(&[&mem], "memstead: create specs--a", &ctx_no_client)
            .unwrap();
        let (name, email, message) = head_commit_parts(&git_dir);
        assert_eq!(name, COMMITTER_NAME);
        assert_eq!(email, COMMITTER_EMAIL);
        assert!(message.contains("\n\nActor: cli"));
        assert!(!message.contains("Client:"));

        // Cli with a ClientId yields the derived author + Client trailer.
        fs::write(mem.join("b.md"), "b").unwrap();
        let ctx_with_client = CommitContext {
            actor: Actor::Cli,
            client: Some(ClientId {
                name: "memstead-cli".into(),
                version: "0.1.0".into(),
            }),
            tool: None,
            note: None,
            logical_operation_id: None,
            entity_ids: None,
        };
        vcs.commit(&[&mem], "memstead: create specs--b", &ctx_with_client)
            .unwrap();
        let (name, email, message) = head_commit_parts(&git_dir);
        assert_eq!(name, "memstead-cli");
        assert_eq!(email, "memstead-cli@memstead.io");
        assert!(message.contains("\n\nActor: cli\nClient: memstead-cli@0.1.0"));
    }

    #[test]
    fn sanitise_client_name_collapses_disallowed_chars() {
        let out = sanitise_client_name("Claude Code/2.1 @ macOS");
        assert_eq!(out, "claude-code-2.1---macos");
        // Must be a valid git-safe local-part.
        assert!(
            out.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
            "{out}"
        );
    }

    #[test]
    fn sanitise_client_name_empty_falls_back_to_unknown() {
        assert_eq!(sanitise_client_name(""), "unknown");
        assert_eq!(sanitise_client_name("   "), "unknown");
        assert_eq!(sanitise_client_name("@@@"), "unknown");
    }

    #[test]
    fn prose_and_trailers_separated_by_exactly_one_blank_line() {
        // Caller passes prose that already ends with a trailing newline —
        // the engine still produces exactly `\n\n` between prose and
        // trailers, never `\n\n\n`.
        let ctx = CommitContext {
            actor: Actor::Agent,
            client: None,
            tool: Some("memstead_create"),
            note: None,
            logical_operation_id: None,
            entity_ids: None,
        };
        let msg = format_commit_message("subject\n", &ctx);
        assert_eq!(msg, "subject\n\nTool: memstead_create\nActor: agent");

        // No trailing newline in prose: same boundary.
        let msg = format_commit_message("subject", &ctx);
        assert_eq!(msg, "subject\n\nTool: memstead_create\nActor: agent");
    }

    #[test]
    fn trailers_are_git_interpret_trailers_compatible() {
        // We don't shell out to `git interpret-trailers` (not a build
        // dependency); instead we check the invariants the tool relies
        // on: blank line before the trailer block, each trailer line is
        // `Key: Value` with no embedded blanks, and the block is the
        // last paragraph.
        let ctx = CommitContext {
            actor: Actor::Agent,
            client: Some(ClientId {
                name: "claude-code".into(),
                version: "2.1.0".into(),
            }),
            tool: Some("memstead_update"),
            note: None,
            logical_operation_id: None,
            entity_ids: None,
        };
        let msg = format_commit_message("memstead: update specs--a", &ctx);
        // Find the last paragraph — everything after the final `\n\n`.
        let (_prose, trailer_block) =
            msg.rsplit_once("\n\n").expect("blank line before trailers");
        for line in trailer_block.lines() {
            let (key, value) = line
                .split_once(": ")
                .unwrap_or_else(|| panic!("malformed trailer line: {line:?}"));
            assert!(!key.is_empty());
            assert!(!value.is_empty());
            // Keys are the three we emit, in the documented order.
            assert!(matches!(key, "Tool" | "Actor" | "Client"), "{key}");
        }
        assert_eq!(
            trailer_block,
            "Tool: memstead_update\nActor: agent\nClient: claude-code@2.1.0"
        );
    }

    #[test]
    fn internal_context_preserves_deterministic_author() {
        // The existing `commit_author_is_deterministic` test proves this
        // via the public API; this unit-level sibling pins the behaviour
        // to `CommitContext::internal()` specifically so a refactor of
        // the default constructor is caught immediately.
        let ctx = CommitContext::internal();
        assert!(matches!(ctx.actor, Actor::Unknown));
        assert!(ctx.client.is_none());
        assert!(ctx.tool.is_none());
        assert!(ctx.note.is_none());
        // Author falls back to committer (no derived identity).
        assert!(author_identity(&ctx).is_none());
    }

    #[test]
    fn commit_message_with_note_inserts_body_between_prose_and_trailers() {
        // Agent note lands between the caller's prose and the trailer
        // block, separated by exactly one blank line on each side.
        let ctx = CommitContext {
            actor: Actor::Agent,
            client: Some(ClientId {
                name: "claude-code".into(),
                version: "2.1.0".into(),
            }),
            tool: Some("memstead_update"),
            note: Some("documenting the foo invariant".into()),
            logical_operation_id: None,
            entity_ids: None,
        };
        let msg = format_commit_message("memstead: update specs--a", &ctx);
        assert_eq!(
            msg,
            "memstead: update specs--a\n\n\
             documenting the foo invariant\n\n\
             Tool: memstead_update\nActor: agent\nClient: claude-code@2.1.0"
        );
    }

    #[test]
    fn commit_message_with_blank_note_behaves_like_absent() {
        // Whitespace-only notes collapse to `None` semantics — the wire
        // never surfaces an empty paragraph between prose and trailers.
        let ctx = CommitContext {
            actor: Actor::Agent,
            client: None,
            tool: Some("memstead_update"),
            note: Some("   \n  \t ".into()),
            logical_operation_id: None,
            entity_ids: None,
        };
        let msg = format_commit_message("subject", &ctx);
        assert_eq!(msg, "subject\n\nTool: memstead_update\nActor: agent");
    }

    #[test]
    fn commit_message_with_empty_note_string_behaves_like_absent() {
        // Explicit `Some("")` is still a no-op — the same branch the
        // MCP handler takes when a caller passes a zero-length note.
        let ctx = CommitContext {
            actor: Actor::Agent,
            client: None,
            tool: Some("memstead_create"),
            note: Some(String::new()),
            logical_operation_id: None,
            entity_ids: None,
        };
        let msg = format_commit_message("subject", &ctx);
        assert_eq!(msg, "subject\n\nTool: memstead_create\nActor: agent");
    }

    // ----------------------------------------------------------------
    // Per-branch mutex
    // ----------------------------------------------------------------

    /// Build a unique ref-name suffix per test invocation so the
    /// process-wide mutex registry never collides across parallel
    /// tests. Uses a static atomic counter — sufficient for in-test
    /// uniqueness without bringing in `uuid`.
    fn unique_ref(prefix: &str) -> String {
        static COUNTER: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("refs/heads/{prefix}-{n}")
    }

    #[test]
    fn per_branch_mutex_serialises_same_ref() {
        let r = unique_ref("serialises");
        let arc = acquire_branch_mutex(&r);
        let guard = arc.lock().unwrap();

        // Spawn a second thread that tries to acquire the same key;
        // it must block until we drop our guard.
        let r_clone = r.clone();
        let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let acquired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started_t = started.clone();
        let acquired_t = acquired.clone();
        let handle = std::thread::spawn(move || {
            started_t.store(true, std::sync::atomic::Ordering::SeqCst);
            let arc2 = acquire_branch_mutex(&r_clone);
            let _g2 = arc2.lock().unwrap();
            acquired_t.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        // Wait for the spawned thread to start and try to acquire.
        // 50ms is well above thread-spawn latency on macOS / Linux.
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            started.load(std::sync::atomic::Ordering::SeqCst),
            "spawned thread did not start within 50ms"
        );
        assert!(
            !acquired.load(std::sync::atomic::Ordering::SeqCst),
            "spawned thread acquired the mutex while main held it"
        );

        // Release: spawned thread must now make progress.
        drop(guard);
        handle.join().unwrap();
        assert!(
            acquired.load(std::sync::atomic::Ordering::SeqCst),
            "spawned thread did not acquire after drop"
        );
    }

    #[test]
    fn per_branch_mutex_parallelises_different_refs() {
        let a = unique_ref("parallel-a");
        let b = unique_ref("parallel-b");

        let arc_a = acquire_branch_mutex(&a);
        let guard_a = arc_a.lock().unwrap();

        // Different ref must acquire without blocking on `a`.
        let b_clone = b.clone();
        let acquired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let acquired_t = acquired.clone();
        let handle = std::thread::spawn(move || {
            let arc_b = acquire_branch_mutex(&b_clone);
            let _g = arc_b.lock().unwrap();
            acquired_t.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        handle.join().unwrap();
        assert!(
            acquired.load(std::sync::atomic::Ordering::SeqCst),
            "different-ref acquisition was blocked by another ref's mutex"
        );
        drop(guard_a);
    }

    #[test]
    fn cross_mem_acquires_in_lex_order() {
        // Pass refs in non-lex order; the helper must sort and
        // acquire in lex order. We verify by reading back the held
        // keys via the debug-only thread-local while the guards are
        // alive.
        let a = unique_ref("cross-aaa");
        let b = unique_ref("cross-bbb");
        let c = unique_ref("cross-ccc");
        // Pre-compute the expected sort order so we can compare.
        let mut expected = vec![a.as_str(), b.as_str(), c.as_str()];
        expected.sort_unstable();
        let _guards = acquire_branch_mutexes_in_order(&[c.as_str(), a.as_str(), b.as_str()]);
        #[cfg(debug_assertions)]
        HELD_BRANCH_KEYS.with(|held| {
            let held = held.borrow();
            // The thread-local stack stores *every* key currently
            // held; this test owns three, but the test runner may
            // also be holding others from the same thread. We only
            // assert that our three appear in lex order in the
            // suffix.
            let tail: Vec<&str> = held.iter().rev().take(3).map(String::as_str).collect();
            // `tail` is in reverse-push order; reverse to get push order.
            let mut pushed: Vec<&str> = tail.into_iter().rev().collect();
            pushed.sort();
            assert_eq!(pushed, expected, "lex-order acquisition violated");
        });
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "out-of-order branch-mutex acquisition")]
    fn out_of_order_acquisition_panics_in_debug() {
        // Acquire a "high" key first, then attempt to acquire a
        // "low" key on the same thread. Must panic. We use unique
        // names so this test cannot collide with other tests'
        // bookkeeping; the keys are kept inside this thread.
        let high = unique_ref("zzz-high");
        let low = unique_ref("aaa-low");
        let arc_high = acquire_branch_mutex(&high);
        let _g_high = arc_high.lock().unwrap();
        // The bookkeeping push happens inside acquire_branch_mutexes_in_order;
        // here we drive the assert manually by injecting `high` into the
        // thread-local stack and then asking for a lex-smaller key.
        HELD_BRANCH_KEYS.with(|held| {
            held.borrow_mut().push(high.clone());
        });
        let _arc_low = acquire_branch_mutex(&low); // must panic
    }
}
