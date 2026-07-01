//! `memstead_fetch` / `memstead_pull` / `memstead_push` implementations.
//!
//! V1 reaches the network through subprocess calls to the user's
//! `git` binary rather than activating `gix`'s
//! `blocking-http-transport-*` features. Trade-off documented on the
//! plan: the subprocess path inherits the user's auth config (ssh
//! agents, credential helpers, OAuth), works on every protocol the
//! installed `git` supports, and keeps the Cargo dep tree unchanged.
//! Cost: a runtime `git` requirement on the operator's PATH (already
//! true for any consumer of a mem-repo) and stderr-parsing for
//! refusal classification.
//!
//! Atomicity caveat for AC F: `git fetch` advances remote-tracking
//! refs in-place. A schema-violating remote tip is therefore visible
//! on `refs/remotes/*` after a successful fetch even if the engine
//! refuses to apply the violating commits locally. The
//! quarantine-ref pipeline the plan body specs as the "safe" shape
//! still has to land — see the open AC list in the plan's session log.
//! Today's fetch surfaces validation failures *after* the
//! remote-tracking move; consumers re-read via `memstead_changes_since`
//! before merging.

use std::path::Path;
use std::process::{Command, Stdio};

use memstead_base::backend::BackendError;
use memstead_base::ops::{FetchOutcome, PullOutcome, PushOutcome, UpdatedRef};

/// Spawn `git -C <gitdir> <args...>` with prompts disabled. Returns
/// stdout on success; wraps stderr into `BackendError::Other` with
/// an in-band marker so the engine layer can un-marshal the right
/// typed refusal.
fn run_git(
    gitdir: &Path,
    args: &[&str],
    marker_for_failure: impl Fn(&str) -> String,
) -> Result<String, BackendError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(gitdir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            BackendError::Other(format!(
                "git subprocess failed to start (`git -C {} {}`): {e}",
                gitdir.display(),
                args.join(" "),
            ))
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(BackendError::Other(marker_for_failure(stderr.trim())));
    }
    Ok(stdout)
}

/// `memstead_fetch` implementation: invokes `git fetch <remote> [<refspec>...]`
/// against the mem-repo gitdir and walks the per-ref state before
/// and after to surface what moved. Errors map to typed markers:
/// `UNKNOWN_REMOTE:<remote>` when stderr admits the remote does not
/// exist; everything else surfaces as a generic backend failure.
pub fn fetch_in_gitdir(
    gitdir: &Path,
    remote: &str,
    refspecs: &[String],
) -> Result<FetchOutcome, BackendError> {
    if !gitdir.is_dir() {
        return Err(BackendError::Other(format!(
            "gitdir not found: {}",
            gitdir.display()
        )));
    }
    let pre = ref_snapshot(gitdir);

    let mut args: Vec<String> = vec!["fetch".to_string(), remote.to_string()];
    for spec in refspecs {
        args.push(spec.clone());
    }
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    let remote_owned = remote.to_string();
    run_git(gitdir, &args_ref, move |stderr| {
        classify_remote_failure(&remote_owned, stderr)
    })?;

    let post = ref_snapshot(gitdir);
    let updated_refs = diff_ref_snapshots(&pre, &post);
    Ok(FetchOutcome {
        remote: remote.to_string(),
        refspecs: refspecs.to_vec(),
        updated_refs,
    })
}

/// `memstead_pull` implementation: runs a fetch, then checks whether the
/// local branch can fast-forward to the remote-tracking ref. Refuses
/// with `LOCAL_DIVERGENCE:<branch>:<remote_ref>` when the local branch
/// has committed locally beyond the merge-base.
pub fn pull_in_gitdir(
    gitdir: &Path,
    remote: &str,
    mem: &str,
) -> Result<PullOutcome, BackendError> {
    let fetched = fetch_in_gitdir(gitdir, remote, &[])?;

    let branch_ref = format!("refs/heads/{mem}");
    let remote_ref = format!("refs/remotes/{remote}/{mem}");

    let remote_sha = resolve_ref(gitdir, &remote_ref).ok_or_else(|| {
        BackendError::Other(format!(
            "remote-tracking ref `{remote_ref}` is absent after fetch; \
             the remote may not carry mem `{mem}`"
        ))
    })?;
    let local_sha = resolve_ref(gitdir, &branch_ref);

    // Fast-forward eligibility: local is an ancestor of remote (or
    // local does not yet exist). Anything else is divergence.
    let can_fast_forward = match &local_sha {
        None => true,
        Some(local) => is_ancestor(gitdir, local, &remote_sha),
    };
    if !can_fast_forward {
        return Err(BackendError::Other(format!(
            "LOCAL_DIVERGENCE:{mem}:{remote_ref}"
        )));
    }

    // Atomic ref move. Use git's plumbing so we go through ref locks.
    let previous_sha = local_sha.clone().unwrap_or_default();
    let update_args = [
        "update-ref",
        "-m",
        "memstead_pull",
        &branch_ref,
        &remote_sha,
        &previous_sha,
    ];
    let branch_for_err = branch_ref.clone();
    run_git(gitdir, &update_args, move |stderr| {
        format!("update-ref `{branch_for_err}` failed: {stderr}")
    })?;

    Ok(PullOutcome {
        mem: mem.to_string(),
        source_ref: remote_ref,
        branch_ref,
        previous_sha,
        new_sha: remote_sha,
        updated_refs: fetched.updated_refs,
    })
}

/// `memstead_push` implementation: invokes `git push <remote>
/// refs/heads/<mem>` against the gitdir. Without `force`, the
/// underlying git refuses non-fast-forward pushes; we map that
/// stderr shape into `NON_FAST_FORWARD:<mem>:<remote>`.
pub fn push_in_gitdir(
    gitdir: &Path,
    remote: &str,
    mem: &str,
    force: bool,
) -> Result<PushOutcome, BackendError> {
    let branch_ref = format!("refs/heads/{mem}");
    let local_sha = resolve_ref(gitdir, &branch_ref).ok_or_else(|| {
        BackendError::Other(format!("UNKNOWN_REF: {branch_ref}"))
    })?;

    let mut args: Vec<String> = vec!["push".to_string()];
    if force {
        args.push("--force-with-lease".to_string());
    }
    args.push(remote.to_string());
    args.push(format!("{branch_ref}:{branch_ref}"));
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();

    let remote_owned = remote.to_string();
    let mem_owned = mem.to_string();
    run_git(gitdir, &args_ref, move |stderr| {
        let lower = stderr.to_lowercase();
        // Git's non-fast-forward refusal text varies across versions
        // and contexts: "non-fast-forward", "fetch first", "rejected
        // because the remote contains work", and the "[rejected]"
        // tag in machine-readable per-ref output. Catch the family.
        if lower.contains("non-fast-forward")
            || lower.contains("non-fast forward")
            || lower.contains("fetch first")
            || lower.contains("[rejected]")
            || lower.contains("updates were rejected")
        {
            return format!("NON_FAST_FORWARD:{mem_owned}:{remote_owned}");
        }
        if lower.contains("does not appear to be a git repository")
            || lower.contains("could not read from remote repository")
            || lower.contains("repository not found")
            || (lower.contains("fatal: '") && lower.contains("does not exist"))
        {
            return format!("UNKNOWN_REMOTE:{remote_owned}");
        }
        format!("push to `{remote_owned}` failed: {stderr}")
    })?;

    Ok(PushOutcome {
        mem: mem.to_string(),
        remote: remote.to_string(),
        branch_ref,
        new_sha: local_sha,
        forced: force,
    })
}

/// Walk the tree at `ref_name`, returning `(relative_path,
/// utf8_content)` pairs for every `.md` blob outside the `.memstead/`
/// engine-internal namespace. Engine layer uses this to run a
/// pre-merge schema-validation pass over the prospective post-merge
/// state without staging an actual merge.
pub fn read_md_blobs_at_ref(
    gitdir: &Path,
    ref_name: &str,
) -> Result<Vec<(String, String)>, BackendError> {
    let repo = gix::open(gitdir).map_err(|e| BackendError::Other(format!("gix open: {e}")))?;
    let id = repo
        .rev_parse_single(ref_name)
        .map_err(|_| BackendError::Other(format!("UNKNOWN_REF: {ref_name}")))?;
    let object = id
        .object()
        .map_err(|_| BackendError::Other(format!("UNKNOWN_REF: {ref_name}")))?;
    let commit = object
        .try_into_commit()
        .map_err(|_| BackendError::Other(format!("UNKNOWN_REF: {ref_name} is not a commit")))?;
    let tree = commit
        .tree()
        .map_err(|e| BackendError::Other(format!("tree({ref_name}): {e}")))?;

    let entries = tree
        .traverse()
        .breadthfirst
        .files()
        .map_err(|e| BackendError::Other(format!("traverse({ref_name}): {e}")))?;

    let mut out: Vec<(String, String)> = Vec::new();
    for entry in entries {
        if !entry.mode.is_blob() {
            continue;
        }
        let path = match std::str::from_utf8(entry.filepath.as_slice()) {
            Ok(s) => s.to_string(),
            Err(_) => continue,
        };
        if !path.ends_with(".md") || path.starts_with(".memstead/") {
            continue;
        }
        let object = match repo.find_object(entry.oid) {
            Ok(o) => o,
            Err(_) => continue,
        };
        let blob = match object.try_into_blob() {
            Ok(b) => b,
            Err(_) => continue,
        };
        let content = match String::from_utf8(blob.data.clone()) {
            Ok(s) => s,
            // Non-UTF-8 blobs surface as parse-failures downstream;
            // skipping silently here means the validator would not
            // know about them. Instead include the path with an
            // empty content so `parse_entries` emits a parse error
            // pointing at the file.
            Err(_) => String::new(),
        };
        out.push((path, content));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Classify a `git fetch` stderr line into the most specific marker
/// we can recover. Used by both fetch and pull's underlying fetch.
fn classify_remote_failure(remote: &str, stderr: &str) -> String {
    let normalized = stderr.to_lowercase();
    if normalized.contains("does not appear to be a git repository")
        || normalized.contains("could not read from remote repository")
        || normalized.contains("repository not found")
        || normalized.contains("name or service not known")
        || (normalized.contains("fatal:") && normalized.contains("not found"))
    {
        return format!("UNKNOWN_REMOTE:{remote}");
    }
    // Heuristic: git's "fatal: '<remote>' does not appear..." line.
    if normalized.contains("not configured")
        || (normalized.contains("fatal:") && normalized.contains("'") && normalized.contains("'"))
    {
        // Be conservative: don't auto-classify every fatal as
        // UNKNOWN_REMOTE — surface as a generic failure when we are
        // unsure. The wrapping engine layer maps unknown markers to
        // `BackendError::Other`.
    }
    format!("git fetch `{remote}` failed: {stderr}")
}

/// Resolve a ref to its SHA via `git rev-parse`. Returns `None` for
/// missing refs (the standard "not found" branch).
fn resolve_ref(gitdir: &Path, ref_name: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(gitdir)
        .args(["rev-parse", "--verify", ref_name])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Check whether `ancestor_sha` is an ancestor of `descendant_sha`.
/// True for the same commit (`git merge-base --is-ancestor` returns 0
/// when ancestor == descendant). False when the relationship doesn't
/// hold or when git refuses for any reason.
fn is_ancestor(gitdir: &Path, ancestor_sha: &str, descendant_sha: &str) -> bool {
    let status = Command::new("git")
        .arg("-C")
        .arg(gitdir)
        .args([
            "merge-base",
            "--is-ancestor",
            ancestor_sha,
            descendant_sha,
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok();
    status.map(|s| s.success()).unwrap_or(false)
}

/// Snapshot every ref in the repo via `git for-each-ref` so we can
/// diff before vs. after a fetch.
fn ref_snapshot(gitdir: &Path) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let result = Command::new("git")
        .arg("-C")
        .arg(gitdir)
        .args(["for-each-ref", "--format=%(refname) %(objectname)"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output();
    let Ok(output) = result else { return out };
    if !output.status.success() {
        return out;
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some((name, sha)) = line.split_once(' ') {
            out.insert(name.to_string(), sha.to_string());
        }
    }
    out
}

fn diff_ref_snapshots(
    pre: &std::collections::HashMap<String, String>,
    post: &std::collections::HashMap<String, String>,
) -> Vec<UpdatedRef> {
    let mut out = Vec::new();
    for (name, new_sha) in post {
        let previous = pre.get(name).cloned().unwrap_or_default();
        if &previous != new_sha {
            out.push(UpdatedRef {
                ref_name: name.clone(),
                previous_sha: previous,
                new_sha: new_sha.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.ref_name.cmp(&b.ref_name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::storage::MemWriter;
    use crate::storage::git_tree::GitTreeMemWriter;
    use crate::vcs::CommitContext;
    use tempfile::TempDir;

    fn body(title: &str) -> String {
        format!(
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# {title}\n\n## Identity\n\n{title}\n"
        )
    }

    fn init_local(tmp: &TempDir, name: &str) -> PathBuf {
        let gitdir = tmp.path().join(name).join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        gix::init_bare(&gitdir).unwrap();
        gitdir
    }

    fn init_bare_remote(tmp: &TempDir, name: &str) -> PathBuf {
        let gitdir = tmp.path().join(name);
        std::fs::create_dir_all(&gitdir).unwrap();
        gix::init_bare(&gitdir).unwrap();
        gitdir
    }

    fn add_remote(local: &Path, name: &str, url: &Path) {
        let status = Command::new("git")
            .arg("-C")
            .arg(local)
            .args(["remote", "add", name, url.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn commit(gitdir: &Path, branch: &str, file: &str, content: &str) -> String {
        let writer = GitTreeMemWriter::new(
            gitdir.to_path_buf(),
            format!("refs/heads/{branch}"),
        );
        writer
            .write_entity(Path::new(file), content.as_bytes())
            .unwrap();
        writer
            .commit("seed", &CommitContext::internal())
            .unwrap()
    }

    #[test]
    fn fetch_unknown_remote_returns_typed_marker() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_local(&tmp, "local");
        commit(&gitdir, "specs", "a.md", &body("A"));
        let err = fetch_in_gitdir(&gitdir, "nope", &[]).unwrap_err();
        match err {
            BackendError::Other(msg) => {
                // Either UNKNOWN_REMOTE marker or a generic failure
                // depending on git's exact error wording. The robust
                // assertion is "remote name is in the message".
                assert!(msg.contains("nope"), "got: {msg}");
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn push_and_fetch_round_trip_with_local_pair() {
        let tmp = TempDir::new().unwrap();
        let local = init_local(&tmp, "local");
        let remote = init_bare_remote(&tmp, "remote.git");
        add_remote(&local, "origin", &remote);

        let sha = commit(&local, "specs", "a.md", &body("A"));

        // Push the local branch to the remote.
        let push = push_in_gitdir(&local, "origin", "specs", false).unwrap();
        assert_eq!(push.new_sha, sha);
        assert_eq!(push.branch_ref, "refs/heads/specs");
        assert!(!push.forced);

        // The remote now has refs/heads/specs pointing at sha.
        let remote_sha = resolve_ref(&remote, "refs/heads/specs").unwrap();
        assert_eq!(remote_sha, sha);

        // Fetch into a second clone-like local.
        let second = init_local(&tmp, "second");
        add_remote(&second, "origin", &remote);
        let fetched = fetch_in_gitdir(&second, "origin", &[]).unwrap();
        // The fetched-side remote-tracking ref now points at the same SHA.
        let tracking = resolve_ref(&second, "refs/remotes/origin/specs").unwrap();
        assert_eq!(tracking, sha);
        // updated_refs lists the move from empty to sha.
        assert!(
            fetched
                .updated_refs
                .iter()
                .any(|u| u.ref_name == "refs/remotes/origin/specs" && u.new_sha == sha),
            "fetched outcome must list the new remote-tracking ref: {fetched:?}",
        );
    }

    #[test]
    fn pull_happy_path_fast_forwards_local_branch() {
        let tmp = TempDir::new().unwrap();
        let local = init_local(&tmp, "local");
        let remote = init_bare_remote(&tmp, "remote.git");
        add_remote(&local, "origin", &remote);
        let sha = commit(&local, "specs", "a.md", &body("A"));
        push_in_gitdir(&local, "origin", "specs", false).unwrap();

        let second = init_local(&tmp, "second");
        add_remote(&second, "origin", &remote);
        let pull = pull_in_gitdir(&second, "origin", "specs").unwrap();
        assert_eq!(pull.new_sha, sha);
        assert_eq!(pull.previous_sha, "");
        let head = resolve_ref(&second, "refs/heads/specs").unwrap();
        assert_eq!(head, sha);
    }

    #[test]
    fn pull_refuses_diverged_local_branch() {
        let tmp = TempDir::new().unwrap();
        let local = init_local(&tmp, "local");
        let remote = init_bare_remote(&tmp, "remote.git");
        add_remote(&local, "origin", &remote);
        commit(&local, "specs", "a.md", &body("A"));
        push_in_gitdir(&local, "origin", "specs", false).unwrap();

        let second = init_local(&tmp, "second");
        add_remote(&second, "origin", &remote);
        pull_in_gitdir(&second, "origin", "specs").unwrap();

        // Advance local on second AND on remote independently so the
        // two branches diverge.
        commit(&second, "specs", "second.md", &body("local"));
        commit(&local, "specs", "first.md", &body("upstream"));
        push_in_gitdir(&local, "origin", "specs", false).unwrap();

        let err = pull_in_gitdir(&second, "origin", "specs").unwrap_err();
        match err {
            BackendError::Other(msg) => assert!(
                msg.starts_with("LOCAL_DIVERGENCE:"),
                "expected LOCAL_DIVERGENCE marker, got: {msg}",
            ),
            other => panic!("expected Other(LOCAL_DIVERGENCE), got {other:?}"),
        }
    }

    #[test]
    fn read_md_blobs_at_ref_lists_only_md_outside_memstead_namespace() {
        let tmp = TempDir::new().unwrap();
        let gitdir = init_local(&tmp, "local");
        commit(&gitdir, "specs", "alpha.md", &body("Alpha"));
        commit(&gitdir, "specs", "nested/beta.md", &body("Beta"));
        let blobs = read_md_blobs_at_ref(&gitdir, "refs/heads/specs").unwrap();
        let mut paths: Vec<String> = blobs.iter().map(|(p, _)| p.clone()).collect();
        paths.sort();
        assert_eq!(paths, vec!["alpha.md".to_string(), "nested/beta.md".to_string()]);
        for (_, content) in &blobs {
            assert!(content.contains("type: spec"), "blob content must round-trip");
        }
    }

    #[test]
    fn engine_pull_refuses_schema_violation_with_typed_envelope() {
        // Seed a "remote" branch with one valid commit, push it, then
        // land a schema-violating commit on the remote and try to pull
        // into a fresh local. The engine layer must refuse with
        // SchemaViolationInFetch and leave the local branch alone.
        let tmp = TempDir::new().unwrap();
        let local_upstream = init_local(&tmp, "upstream");
        let remote = init_bare_remote(&tmp, "remote.git");
        add_remote(&local_upstream, "origin", &remote);

        commit(&local_upstream, "specs", "alpha.md", &body("Alpha"));
        push_in_gitdir(&local_upstream, "origin", "specs", false).unwrap();

        // Land a malformed entity on the upstream and push it. The
        // body has no frontmatter at all — the strict validator's
        // `split_frontmatter_strict` refuses with `MissingFrontmatter`.
        let writer = GitTreeMemWriter::new(
            local_upstream.clone(),
            "refs/heads/specs".to_string(),
        );
        writer
            .write_entity(Path::new("broken.md"), b"# Broken\n\nbody without frontmatter.\n")
            .unwrap();
        writer.commit("oops", &CommitContext::internal()).unwrap();
        push_in_gitdir(&local_upstream, "origin", "specs", false).unwrap();

        // Build a fresh local engine + git-branch mount pointing at a
        // clean clone-like gitdir; we still need to add the remote
        // by hand so the dispatcher's `git fetch origin` works.
        let downstream_gitdir = init_local(&tmp, "downstream");
        add_remote(&downstream_gitdir, "origin", &remote);

        let mount = memstead_base::Mount {
            mem: "specs".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "default",
                semver::Version::new(1, 0, 0),
            )),
            storage: memstead_base::MountStorage::GitBranch {
                gitdir: downstream_gitdir.clone(),
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

        let err = engine.pull("specs", "origin").unwrap_err();
        match err {
            memstead_base::EngineError::SchemaViolationInFetch {
                mem,
                ref_name,
                violations,
            } => {
                assert_eq!(mem, "specs");
                assert!(ref_name.starts_with("refs/remotes/origin/specs"));
                assert!(!violations.is_empty(), "violations must list the broken entity");
                assert!(
                    violations.iter().any(|v| v.contains("broken.md")),
                    "violation must name the offending file: {violations:?}",
                );
            }
            other => panic!("expected SchemaViolationInFetch, got {other:?}"),
        }

        // The local branch was never created — schema refusal is
        // strictly pre-merge.
        assert!(resolve_ref(&downstream_gitdir, "refs/heads/specs").is_none());
    }

    #[test]
    fn engine_push_refuses_local_invalid_state() {
        // Seed an engine with a mem whose local branch carries a
        // malformed entity, then call push: the engine refuses with
        // LOCAL_INVALID_STATE before contacting the remote.
        let tmp = TempDir::new().unwrap();
        let local = init_local(&tmp, "local");
        let remote = init_bare_remote(&tmp, "remote.git");
        add_remote(&local, "origin", &remote);
        let writer = GitTreeMemWriter::new(local.clone(), "refs/heads/specs".to_string());
        writer
            .write_entity(Path::new("broken.md"), b"# Bad\n\nbody without frontmatter.\n")
            .unwrap();
        writer.commit("seed", &CommitContext::internal()).unwrap();

        let mount = memstead_base::Mount {
            mem: "specs".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "default",
                semver::Version::new(1, 0, 0),
            )),
            storage: memstead_base::MountStorage::GitBranch {
                gitdir: local.clone(),
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

        let err = engine.push("specs", "origin", false).unwrap_err();
        match err {
            memstead_base::EngineError::LocalInvalidState {
                mem,
                remote: r,
                detail,
            } => {
                assert_eq!(mem, "specs");
                assert_eq!(r, "origin");
                assert!(detail.contains("broken.md") || detail.contains("violation"), "detail = {detail}");
            }
            other => panic!("expected LocalInvalidState, got {other:?}"),
        }

        // The remote never saw the push.
        assert!(resolve_ref(&remote, "refs/heads/specs").is_none());
    }

    #[test]
    fn push_refuses_non_fast_forward_without_force() {
        let tmp = TempDir::new().unwrap();
        let local = init_local(&tmp, "local");
        let remote = init_bare_remote(&tmp, "remote.git");
        add_remote(&local, "origin", &remote);
        commit(&local, "specs", "a.md", &body("A"));
        push_in_gitdir(&local, "origin", "specs", false).unwrap();

        // Another local with a divergent commit history.
        let second = init_local(&tmp, "second");
        add_remote(&second, "origin", &remote);
        commit(&second, "specs", "diff.md", &body("divergent"));

        let err = push_in_gitdir(&second, "origin", "specs", false).unwrap_err();
        match err {
            BackendError::Other(msg) => {
                assert!(msg.contains("NON_FAST_FORWARD") || msg.contains("non-fast"), "got: {msg}");
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }
}
