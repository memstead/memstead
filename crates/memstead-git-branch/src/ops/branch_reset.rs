//! `Engine::branch_reset` implementation for git-branch mounts.
//!
//! The single engine-level history-rewrite op. Sets a mem's branch
//! pointer to `target_sha` and refuses if any commit that would be
//! discarded by the reset is already pushed to a remote-tracking ref.
//! Replay workflows that operate over un-pushed commit segments
//! consume this op; nothing else in the engine moves a branch pointer
//! over existing commits.
//!
//! ## Safety contract
//!
//! "Pushed" is defined as: reachable from at least one `refs/remotes/*`
//! ref. The engine reads remote-tracking refs only; it does not
//! contact the remote during the safety probe. Operators who skirt
//! that convention (manual `refs/remotes/*` edits) can fool the
//! check, but the policy is git-standard.
//!
//! ## Atomicity
//!
//! The actual ref update goes through `gix`'s `edit_references`
//! transaction with `PreviousValue::MustExistAndMatch(current)`. A
//! sibling writer that advances the branch between our snapshot and
//! the commit phase trips the precondition and surfaces as a
//! `RefTransaction` error — the operator-recoverable case the
//! `mem_repo_config::commit_refs` machinery already documents.

use std::collections::HashSet;
use std::path::Path;

use gix::ObjectId;
use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
use gix::refs::{FullName, Target};

use memstead_base::backend::BackendError;
use memstead_base::ops::BranchResetOutcome;

/// Resolve `<gitdir>/refs/heads/<branch>` to its current tip SHA. The
/// branch is required to exist — a missing branch surfaces as
/// `UNKNOWN_REF:refs/heads/<branch>` so the engine layer maps it to
/// `EngineError::UnknownRef`.
fn current_head(repo: &gix::Repository, branch: &str) -> Result<ObjectId, BackendError> {
    let ref_name = format!("refs/heads/{branch}");
    let id = repo
        .rev_parse_single(ref_name.as_str())
        .map_err(|_| BackendError::Other(format!("UNKNOWN_REF: {ref_name}")))?;
    Ok(id.detach())
}

/// Resolve a caller-supplied target ref. Accepts any input
/// `gix::rev_parse_single` understands — branch names, short SHAs,
/// full SHAs. Unknown inputs flag via the `UNKNOWN_REF` marker.
fn resolve_target(repo: &gix::Repository, target: &str) -> Result<ObjectId, BackendError> {
    let id = repo
        .rev_parse_single(target)
        .map_err(|_| BackendError::Other(format!("UNKNOWN_REF: {target}")))?;
    Ok(id.detach())
}

/// Walk every `refs/remotes/*` ref and collect the set of all commit
/// SHAs reachable from any of them. The set is the engine's
/// definition of "pushed" for the safety probe.
fn pushed_commit_set(repo: &gix::Repository) -> Result<HashSet<ObjectId>, BackendError> {
    let platform = repo
        .references()
        .map_err(|e| BackendError::Other(format!("references(): {e}")))?;
    let iter = platform
        .remote_branches()
        .map_err(|e| BackendError::Other(format!("remote_branches(): {e}")))?;

    let mut tips: Vec<ObjectId> = Vec::new();
    for r in iter {
        let mut reference = match r {
            Ok(rf) => rf,
            Err(_) => continue,
        };
        if let Ok(commit) = reference.peel_to_id() {
            tips.push(commit.detach());
        }
    }

    let mut reachable: HashSet<ObjectId> = HashSet::new();
    if tips.is_empty() {
        return Ok(reachable);
    }

    let walk = repo
        .rev_walk(tips)
        .all()
        .map_err(|e| BackendError::Other(format!("rev-walk(remotes): {e}")))?;
    for info in walk {
        let info = match info {
            Ok(i) => i,
            Err(_) => continue,
        };
        reachable.insert(info.id);
    }
    Ok(reachable)
}

/// Walk commits reachable from `head` with `target` as the boundary
/// (target itself and its ancestors are excluded). Every commit
/// visited would be discarded if the branch pointer moved to
/// `target`. When `head == target` the set is empty (no-op reset).
fn discarded_commits(
    repo: &gix::Repository,
    head: ObjectId,
    target: ObjectId,
) -> Result<Vec<ObjectId>, BackendError> {
    if head == target {
        return Ok(Vec::new());
    }
    let walk = repo
        .rev_walk([head])
        .with_hidden([target])
        .all()
        .map_err(|e| BackendError::Other(format!("rev-walk(discard): {e}")))?;
    let mut out = Vec::new();
    for info in walk {
        let info = info.map_err(|e| BackendError::Other(format!("rev-walk-step: {e}")))?;
        out.push(info.id);
    }
    Ok(out)
}

/// Apply the branch_reset operation. See module-level docs for the
/// safety contract; fetch / pull / push / branch_reset form one
/// transport surface.
pub fn branch_reset_in_gitdir(
    gitdir: &Path,
    branch: &str,
    target_sha: &str,
    expected_head: Option<&str>,
) -> Result<BranchResetOutcome, BackendError> {
    if !gitdir.is_dir() {
        return Err(BackendError::Other(format!(
            "gitdir not found: {}",
            gitdir.display()
        )));
    }
    let repo = gix::open(gitdir).map_err(|e| BackendError::Other(format!("gix open: {e}")))?;

    // Mounts may carry the branch as a full ref (`refs/heads/<mem>`) or
    // short form — the same dual shape git_tree.rs normalizes at its
    // seams. Strip here so `refs/heads/` is prepended exactly once.
    let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);

    let current = current_head(&repo, branch)?;
    let target = resolve_target(&repo, target_sha)?;
    let branch_ref = format!("refs/heads/{branch}");

    // Optimistic-concurrency guard: the caller names the head it observed
    // (a review span's end, the head at preview time); a live head that
    // moved past it refuses instead of silently discarding the foreign
    // commits. The ref-update CAS below closes the residual read-to-write
    // window — a sibling advancing between this check and the transaction
    // fails the transaction, never overwrites.
    if let Some(expected) = expected_head {
        if current.to_string() != expected {
            return Err(BackendError::Other(format!(
                "EXPECTED_HEAD_MISMATCH: {current}"
            )));
        }
    }

    // Fast-path: target equals current head — no-op reset. Return an
    // outcome with empty `discarded` so callers can branch on
    // emptiness if they want to skip downstream side effects.
    if current == target {
        return Ok(BranchResetOutcome {
            mem: branch.to_string(),
            branch_ref,
            previous_sha: current.to_string(),
            new_sha: target.to_string(),
            discarded_commits: Vec::new(),
        });
    }

    let discarded = discarded_commits(&repo, current, target)?;

    let pushed = pushed_commit_set(&repo)?;
    let blocked: Vec<String> = discarded
        .iter()
        .filter(|c| pushed.contains(*c))
        .map(|c| c.to_string())
        .collect();
    if !blocked.is_empty() {
        return Err(BackendError::Other(format!(
            "PUSHED_COMMITS_PROTECTED: {}",
            blocked.join(",")
        )));
    }

    // Atomic ref update. PreviousValue::MustExistAndMatch refuses if
    // a sibling writer advanced the branch between our snapshot and
    // here — surfaces as a RefTransaction error rather than silently
    // overwriting their commit.
    let name: FullName = branch_ref
        .as_str()
        .try_into()
        .map_err(|e| BackendError::Other(format!("invalid ref name {branch_ref:?}: {e}")))?;
    let edit = RefEdit {
        change: Change::Update {
            log: LogChange {
                mode: RefLog::AndReference,
                force_create_reflog: false,
                message: format!("memstead_branch_reset {} -> {target_sha}", branch).into(),
            },
            expected: PreviousValue::MustExistAndMatch(Target::Object(current)),
            new: Target::Object(target),
        },
        name,
        deref: false,
    };
    repo.edit_references([edit])
        .map_err(|e| BackendError::Other(format!("edit_references: {e}")))?;

    Ok(BranchResetOutcome {
        mem: branch.to_string(),
        branch_ref,
        previous_sha: current.to_string(),
        new_sha: target.to_string(),
        discarded_commits: discarded.into_iter().map(|c| c.to_string()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemWriter;
    use crate::storage::git_tree::GitTreeMemWriter;
    use crate::vcs::CommitContext;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn init_gitdir(tmp: &TempDir) -> PathBuf {
        let gitdir = tmp.path().join("mem-repo").join(".git");
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
        let writer = GitTreeMemWriter::new(gitdir.to_path_buf(), format!("refs/heads/{branch}"));
        writer
            .write_entity(Path::new(file), content.as_bytes())
            .unwrap();
        writer.commit(subject, &CommitContext::internal()).unwrap()
    }

    /// Create a `refs/remotes/<remote>/<branch>` pointing at the given
    /// SHA so the pushed-status probe sees it as "pushed".
    fn set_remote_tracking(gitdir: &PathBuf, remote: &str, branch: &str, sha: &str) {
        let repo = gix::open(gitdir).unwrap();
        let name: FullName = format!("refs/remotes/{remote}/{branch}")
            .as_str()
            .try_into()
            .unwrap();
        let oid: ObjectId = sha.parse().unwrap();
        repo.edit_references([RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: false,
                    message: "test-remote".into(),
                },
                expected: PreviousValue::Any,
                new: Target::Object(oid),
            },
            name,
            deref: false,
        }])
        .unwrap();
    }

    #[test]
    fn branch_reset_unknown_branch_returns_unknown_ref_marker() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let err = branch_reset_in_gitdir(&gitdir, "nope", "abc", None).unwrap_err();
        match err {
            BackendError::Other(msg) => assert!(msg.starts_with("UNKNOWN_REF:"), "got: {msg}"),
            other => panic!("expected Other(UNKNOWN_REF), got {other:?}"),
        }
    }

    #[test]
    fn branch_reset_accepts_full_ref_branch_form() {
        // Mounts carry `branch` as either `specs` or `refs/heads/specs`;
        // the long form used to double-prefix into
        // `refs/heads/refs/heads/specs` and refuse with UNKNOWN_REF.
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let sha_a = commit(&gitdir, "specs", "a.md", &body("A"), "A");
        let _sha_b = commit(&gitdir, "specs", "b.md", &body("B"), "B");
        let outcome = branch_reset_in_gitdir(&gitdir, "refs/heads/specs", &sha_a, None).unwrap();
        assert_eq!(outcome.new_sha, sha_a);
        assert_eq!(outcome.branch_ref, "refs/heads/specs");
    }

    #[test]
    fn branch_reset_no_op_when_target_equals_head() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let sha = commit(&gitdir, "specs", "a.md", &body("A"), "seed");
        let outcome = branch_reset_in_gitdir(&gitdir, "specs", &sha, None).unwrap();
        assert!(outcome.discarded_commits.is_empty());
        assert_eq!(outcome.previous_sha, outcome.new_sha);
    }

    #[test]
    fn branch_reset_unpushed_commits_succeeds() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let sha_a = commit(&gitdir, "specs", "a.md", &body("A"), "A");
        let _sha_b = commit(&gitdir, "specs", "b.md", &body("B"), "B");
        let sha_c = commit(&gitdir, "specs", "c.md", &body("C"), "C");

        // Reset back to A; B and C are unpushed (no remote-tracking ref
        // exists), so the reset is allowed.
        let outcome = branch_reset_in_gitdir(&gitdir, "specs", &sha_a, None).unwrap();
        assert_eq!(outcome.new_sha, sha_a);
        assert_eq!(outcome.previous_sha, sha_c);
        assert_eq!(outcome.discarded_commits.len(), 2);

        // Branch now points to A.
        let repo = gix::open(&gitdir).unwrap();
        let head = repo
            .rev_parse_single("refs/heads/specs")
            .unwrap()
            .detach()
            .to_string();
        assert_eq!(head, sha_a);
    }

    #[test]
    fn branch_reset_refuses_when_expected_head_moved() {
        // Optimistic-concurrency guard: the caller observed head A→B, a
        // sibling then committed C. A reset carrying expected_head=B must
        // refuse with the live head — never silently discard C.
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let sha_a = commit(&gitdir, "specs", "a.md", &body("A"), "A");
        let sha_b = commit(&gitdir, "specs", "b.md", &body("B"), "B");
        // Sibling writer advances past the observed head.
        let sha_c = commit(&gitdir, "specs", "c.md", &body("C"), "C");

        let err =
            branch_reset_in_gitdir(&gitdir, "specs", &sha_a, Some(&sha_b)).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.starts_with("EXPECTED_HEAD_MISMATCH:") || msg.contains("EXPECTED_HEAD_MISMATCH"),
            "expected mismatch marker, got: {msg}"
        );
        assert!(msg.contains(&sha_c), "refusal names the live head: {msg}");

        // Nothing moved.
        let repo = gix::open(&gitdir).unwrap();
        let head = repo
            .rev_parse_single("refs/heads/specs")
            .unwrap()
            .detach()
            .to_string();
        assert_eq!(head, sha_c, "the branch pointer is untouched");

        // With the true live head as expected, the reset proceeds.
        let outcome =
            branch_reset_in_gitdir(&gitdir, "specs", &sha_a, Some(&sha_c)).unwrap();
        assert_eq!(outcome.new_sha, sha_a);
        assert_eq!(outcome.discarded_commits.len(), 2);
    }

    #[test]
    fn engine_branch_reset_routes_git_branch_mount_and_surfaces_typed_error() {
        // End-to-end: build an engine with a git-branch mount, install
        // the full ops bundle, then assert the typed surfacing:
        // `PUSHED_COMMITS_PROTECTED` un-marshals into the typed
        // `EngineError::PushedCommitsProtected` variant.
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let _sha_a = commit(&gitdir, "specs", "a.md", &body("A"), "A");
        let sha_b = commit(&gitdir, "specs", "b.md", &body("B"), "B");
        set_remote_tracking(&gitdir, "origin", "specs", &sha_b);
        let _sha_c = commit(&gitdir, "specs", "c.md", &body("C"), "C");

        let mount = memstead_base::Mount {
            mem: "specs".to_string(),
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
        let backend = crate::storage::instantiate_full_backend(&mount).unwrap();
        let mut engine = memstead_base::Engine::from_mounts(vec![(mount, backend)]).unwrap();
        engine.set_git_branch_ops(crate::storage::FULL_GIT_BRANCH_OPS);

        let err = engine.branch_reset("specs", &_sha_a, None).unwrap_err();
        match err {
            memstead_base::EngineError::PushedCommitsProtected {
                mem,
                target_sha,
                pushed_shas,
            } => {
                assert_eq!(mem, "specs");
                assert_eq!(target_sha, _sha_a);
                assert!(pushed_shas.contains(&sha_b));
            }
            other => panic!("expected PushedCommitsProtected, got {other:?}"),
        }
    }

    #[test]
    fn engine_branch_reset_surfaces_head_moved_and_read_only_typed() {
        // Typed surfacing of the two newest guards through the engine:
        // EXPECTED_HEAD_MISMATCH un-marshals into BranchResetHeadMoved, and
        // a non-Write mount refuses with ReadOnlyMount before dispatch.
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let sha_a = commit(&gitdir, "specs", "a.md", &body("A"), "A");
        let sha_b = commit(&gitdir, "specs", "b.md", &body("B"), "B");
        let sha_c = commit(&gitdir, "specs", "c.md", &body("C"), "C");

        let mount = memstead_base::Mount {
            mem: "specs".to_string(),
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
        let backend = crate::storage::instantiate_full_backend(&mount).unwrap();
        let mut engine = memstead_base::Engine::from_mounts(vec![(mount, backend)]).unwrap();
        engine.set_git_branch_ops(crate::storage::FULL_GIT_BRANCH_OPS);

        let err = engine
            .branch_reset("specs", &sha_a, Some(&sha_b))
            .unwrap_err();
        match err {
            memstead_base::EngineError::BranchResetHeadMoved {
                mem,
                expected,
                current,
            } => {
                assert_eq!(mem, "specs");
                assert_eq!(expected, sha_b);
                assert_eq!(current, sha_c);
            }
            other => panic!("expected BranchResetHeadMoved, got {other:?}"),
        }

        // Read-only capability refuses before any storage dispatch.
        let ro_mount = memstead_base::Mount {
            mem: "sealed".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "default",
                semver::Version::new(1, 0, 0),
            )),
            storage: memstead_base::MountStorage::GitBranch {
                gitdir: gitdir.clone(),
                branch: "specs".to_string(),
            },
            capability: memstead_base::MountCapability::ReadOnly,
            lifecycle: memstead_base::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let ro_backend = crate::storage::instantiate_full_backend(&ro_mount).unwrap();
        let mut ro_engine =
            memstead_base::Engine::from_mounts(vec![(ro_mount, ro_backend)]).unwrap();
        ro_engine.set_git_branch_ops(crate::storage::FULL_GIT_BRANCH_OPS);
        let refused = ro_engine.branch_reset("sealed", &sha_a, None).unwrap_err();
        assert!(
            matches!(refused, memstead_base::EngineError::ReadOnlyMount(_)),
            "expected ReadOnlyMount, got {refused:?}"
        );
    }

    #[test]
    fn branch_reset_refuses_when_discarded_commits_are_pushed() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_gitdir(&tmp);
        let sha_a = commit(&gitdir, "specs", "a.md", &body("A"), "A");
        let sha_b = commit(&gitdir, "specs", "b.md", &body("B"), "B");
        let _sha_c = commit(&gitdir, "specs", "c.md", &body("C"), "C");

        // Pretend B was pushed to origin.
        set_remote_tracking(&gitdir, "origin", "specs", &sha_b);

        // Try to reset back to A — B is pushed and would be discarded.
        let err = branch_reset_in_gitdir(&gitdir, "specs", &sha_a, None).unwrap_err();
        match err {
            BackendError::Other(msg) => {
                assert!(
                    msg.starts_with("PUSHED_COMMITS_PROTECTED:"),
                    "unexpected refusal: {msg}",
                );
                assert!(msg.contains(&sha_b), "must name the pushed commit: {msg}");
            }
            other => panic!("expected Other(PUSHED_COMMITS_PROTECTED), got {other:?}"),
        }

        // Branch pointer unchanged.
        let repo = gix::open(&gitdir).unwrap();
        let head = repo
            .rev_parse_single("refs/heads/specs")
            .unwrap()
            .detach()
            .to_string();
        assert_eq!(head, _sha_c);
    }
}
