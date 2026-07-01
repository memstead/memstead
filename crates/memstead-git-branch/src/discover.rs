//! Branch enumeration for the workspace's `vault-repo/.git/`.
//!
//! Post-rebuild the canonical vault list lives in
//! `.memstead/state/mounts.json`. This helper is retained for the macOS
//! UniFFI `discover_vaults` entry point, which the app calls before the
//! engine is constructed to seed its UI. It reads the per-vault branch
//! list off the workspace's single `vault-repo/.git/` and filters out
//! `main` and any `__*` registry-class refs.

use std::collections::HashMap;
use std::path::Path;

/// Enumerate the workspace's per-vault content branches.
///
/// Walks `<workspace_root>/vault-repo/.git/refs/heads/`, filters out
/// `main` and any `__*`-prefixed registry refs (today: `__MEMSTEAD`), and
/// returns the sorted leaf names. The leaf is the last `/`-separated
/// segment of each branch shortname so hierarchical layouts surface
/// alongside flat ones.
///
/// Returns `Some(names)` when the gitdir exists and carries the
/// `__MEMSTEAD` umbrella ref (the post-rebuild "real workspace" gate).
/// `Some(empty_vec)` is healthy — a freshly-initialised workspace has
/// `__MEMSTEAD` but no per-vault branches yet.
///
/// Returns `None` when the gitdir is missing, cannot be opened, or
/// lacks `__MEMSTEAD`. Callers (notably `memstead-swift`) treat `None` as
/// "this workspace is not vault-repo-backed".
pub fn enumerate_vault_repo_branches(workspace_root: &Path) -> Option<Vec<String>> {
    let gitdir = workspace_root.join("vault-repo").join(".git");
    enumerate_branches_in_gitdir(&gitdir)
}

fn enumerate_branches_in_gitdir(gitdir: &Path) -> Option<Vec<String>> {
    if !gitdir.is_dir() {
        return None;
    }

    let repo = match gix::open(gitdir) {
        Ok(repo) => repo,
        Err(e) => {
            tracing::warn!(
                gitdir = %gitdir.display(),
                error = %e,
                "could not open vault-repo gitdir"
            );
            return None;
        }
    };

    if repo.find_reference("refs/heads/__MEMSTEAD").is_err() {
        return None;
    }

    let mut branch_names: Vec<String> = Vec::new();
    let mut seen_leaves: HashMap<String, String> = HashMap::new();
    let refs_platform = match repo.references() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                gitdir = %gitdir.display(),
                error = %e,
                "could not access vault-repo references"
            );
            return None;
        }
    };
    let iter = match refs_platform.local_branches() {
        Ok(it) => it,
        Err(e) => {
            tracing::warn!(
                gitdir = %gitdir.display(),
                error = %e,
                "could not enumerate vault-repo local branches"
            );
            return None;
        }
    };
    for r in iter {
        let reference = match r {
            Ok(reference) => reference,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "failed to read a vault-repo reference; skipping"
                );
                continue;
            }
        };
        let short = reference.name().shorten();
        let name = match std::str::from_utf8(short) {
            Ok(name) => name,
            Err(_) => continue,
        };
        if name == "main" {
            continue;
        }
        if name
            .split('/')
            .next()
            .map(|s| s.starts_with("__"))
            .unwrap_or(false)
        {
            continue;
        }
        let leaf = name.rsplit('/').next().unwrap_or(name);
        if let Some(prior) = seen_leaves.get(leaf) {
            tracing::warn!(
                leaf = leaf,
                existing = prior.as_str(),
                duplicate = name,
                "vault-repo has two branches with the same leaf; dropping the second"
            );
            continue;
        }
        seen_leaves.insert(leaf.to_string(), name.to_string());
        branch_names.push(leaf.to_string());
    }

    branch_names.sort();
    Some(branch_names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_vault_repo_with_branches(root: &Path, branches: &[&str]) {
        let vault_repo = root.join("vault-repo").join(".git");
        std::fs::create_dir_all(&vault_repo).unwrap();
        let repo = gix::init_bare(&vault_repo).unwrap();
        let actor = gix::actor::Signature {
            name: "test".into(),
            email: "test@example.com".into(),
            time: gix::date::Time {
                seconds: 0,
                offset: 0,
            },
        };
        let mut buf = gix::date::parse::TimeBuf::default();
        let actor_ref = actor.to_ref(&mut buf);
        let empty_tree = repo.empty_tree().id();
        repo.commit_as(
            actor_ref,
            actor_ref,
            "refs/heads/main",
            "test main",
            empty_tree,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();
        let mut buf = gix::date::parse::TimeBuf::default();
        let actor_ref = actor.to_ref(&mut buf);
        repo.commit_as(
            actor_ref,
            actor_ref,
            "refs/heads/__MEMSTEAD",
            "test __MEMSTEAD",
            empty_tree,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();
        for branch in branches {
            let mut buf = gix::date::parse::TimeBuf::default();
            let actor_ref = actor.to_ref(&mut buf);
            repo.commit_as(
                actor_ref,
                actor_ref,
                format!("refs/heads/{branch}"),
                "test seed",
                empty_tree,
                Vec::<gix::ObjectId>::new(),
            )
            .unwrap();
        }
    }

    #[test]
    fn enumerates_per_vault_branches_excluding_main_and_registry_refs() {
        let tmp = TempDir::new().unwrap();
        init_vault_repo_with_branches(tmp.path(), &["alpha", "beta", "gamma"]);
        let names = enumerate_vault_repo_branches(tmp.path()).expect("real workspace");
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn returns_none_when_vault_repo_missing() {
        let tmp = TempDir::new().unwrap();
        assert!(enumerate_vault_repo_branches(tmp.path()).is_none());
    }

    #[test]
    fn returns_none_when_memstead_ref_missing() {
        let tmp = TempDir::new().unwrap();
        let vault_repo = tmp.path().join("vault-repo").join(".git");
        std::fs::create_dir_all(&vault_repo).unwrap();
        gix::init_bare(&vault_repo).unwrap();
        assert!(enumerate_vault_repo_branches(tmp.path()).is_none());
    }

    #[test]
    fn surfaces_hierarchical_layout_as_leaf_names() {
        let tmp = TempDir::new().unwrap();
        init_vault_repo_with_branches(
            tmp.path(),
            &["demo/engine", "planning/exec-foo"],
        );
        let names = enumerate_vault_repo_branches(tmp.path()).expect("real workspace");
        assert_eq!(names, vec!["engine", "exec-foo"]);
    }
}
