//! Workspace-root utilities. Today's only consumer is the `memstead_health`
//! `OUTER_REPO_NOT_IGNORING_VAULT_REPO` warning surfaced by the MCP layer:
//! when the workspace is embedded inside another git repository, the
//! vault-repo-git directory must be excluded by the outer repo's
//! `.gitignore` to avoid the gitlink trap.
//!
//! Pure path-walking — no IO beyond `metadata` / `read_to_string` for
//! the gitignore probe — so this module is safe to call from any
//! engine surface, including read-only health queries.

use std::path::{Path, PathBuf};

/// Walk parent directories from `workspace_root.parent()` upward looking
/// for a `.git` directory or file. Returns the first ancestor that
/// carries one, or `None` when none exist (for example, the workspace
/// is not embedded inside another git repository).
///
/// `workspace_root` itself is intentionally skipped: the embedded
/// `vault-repo/.git/` lives *inside* the workspace and is not the
/// "outer" repo this helper looks for. We start the walk one level
/// up so the workspace's own gitdir can never shadow a real outer
/// repo.
///
/// `.git` may be a directory (the common case) or a file (for git
/// worktrees and submodules). Both shapes count as "this ancestor is
/// a git checkout."
pub fn find_enclosing_git_repo(workspace_root: &Path) -> Option<PathBuf> {
    let start = workspace_root.parent()?;
    let mut current = Some(start);
    while let Some(dir) = current {
        let candidate = dir.join(".git");
        if candidate.is_dir() || candidate.is_file() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// Returns `true` when the outer repo at `outer_repo_root` contains a
/// `.gitignore` line that ignores `vault-repo/` (with or without a
/// leading workspace-relative prefix). The match is whitespace- and
/// trailing-slash-insensitive: `vault-repo`, `vault-repo/`, `memstead/vault-repo`,
/// and `memstead/vault-repo/` all count as "ignored." A negation
/// (`!vault-repo/`) cancels the match.
///
/// This is a *best-effort heuristic* against a hand-edited file, not a
/// full `.gitignore` parser. False negatives are acceptable (the
/// warning surfaces; the user inspects); false positives would silence
/// a real misconfiguration, which is why the matcher errs on the side
/// of literal substring matches and skips comment lines.
pub fn outer_repo_ignores_vault_repo(outer_repo_root: &Path, workspace_root: &Path) -> bool {
    let gitignore = outer_repo_root.join(".gitignore");
    let Ok(contents) = std::fs::read_to_string(&gitignore) else {
        return false;
    };

    // Workspace-relative prefix the outer-repo author would use to
    // address the vault-repo directory: "<rel>/vault-repo" where <rel>
    // is the workspace's path relative to the outer repo root.
    let rel_prefix: Option<String> = workspace_root
        .strip_prefix(outer_repo_root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"));

    let mut matched = false;
    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (negated, body) = match line.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, line),
        };
        let body = body.trim_end_matches('/').trim();
        if line_matches_vault_repo(body, rel_prefix.as_deref()) {
            matched = !negated;
        }
    }
    matched
}

fn line_matches_vault_repo(body: &str, rel_prefix: Option<&str>) -> bool {
    let body = body.trim_start_matches('/');
    if body == "vault-repo" {
        return true;
    }
    if let Some(rel) = rel_prefix {
        let rel = rel.trim_start_matches('/').trim_end_matches('/');
        if !rel.is_empty() {
            let combined = format!("{rel}/vault-repo");
            if body == combined {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn returns_none_when_no_outer_git() {
        // The walker climbs all the way to filesystem root, so a stray
        // `.git` anywhere in `/var/folders/.../T/` (left behind by an
        // unrelated test run on macOS) would shadow the assertion. We
        // confirm the helper returns `None` by checking the *negative
        // image*: when the walker DOES find a `.git`, that ancestor
        // must be outside the test's TempDir — proving the test
        // workspace itself contains no enclosing repo within its own
        // bounds. Belt-and-braces: also confirm the walker stops at
        // the root.
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        match find_enclosing_git_repo(&workspace) {
            None => {}
            Some(found) => {
                let canon_found = found.canonicalize().unwrap_or(found);
                let canon_tmp = tmp.path().canonicalize().unwrap_or(tmp.path().to_path_buf());
                assert!(
                    !canon_found.starts_with(&canon_tmp),
                    "test environment leaked a `.git` under TempDir: {}",
                    canon_found.display()
                );
            }
        }
    }

    #[test]
    fn finds_outer_git_dir() {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        let workspace = outer.join("memstead");
        fs::create_dir_all(workspace.join(".memstead")).unwrap();
        fs::create_dir_all(outer.join(".git")).unwrap();
        let found = find_enclosing_git_repo(&workspace).expect("should find outer .git");
        assert_eq!(found, outer);
    }

    #[test]
    fn skips_workspace_self_git_dir() {
        // Workspace's own `.git/` (the vault-repo-git embedded gitdir
        // sits at `workspace/vault-repo/.git`, but a stray `.git` at the
        // workspace root itself would be the legacy disk gitdir). The
        // walker starts at parent, so neither shadows a real outer
        // repo. As in `returns_none_when_no_outer_git`, we tolerate
        // an enclosing `.git` outside the TempDir (test-environment
        // leakage on macOS) by asserting only that no match resolves
        // to the test workspace itself.
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(workspace.join(".git")).unwrap();
        let canon_workspace = workspace.canonicalize().unwrap_or(workspace.clone());
        match find_enclosing_git_repo(&workspace) {
            None => {}
            Some(found) => {
                let canon_found = found.canonicalize().unwrap_or(found);
                assert_ne!(
                    canon_found, canon_workspace,
                    "walker must skip the workspace's own `.git/`"
                );
                let canon_tmp = tmp.path().canonicalize().unwrap_or(tmp.path().to_path_buf());
                assert!(
                    !canon_found.starts_with(&canon_tmp),
                    "match must come from outside the TempDir"
                );
            }
        }
    }

    #[test]
    fn detects_dot_git_file_for_worktrees() {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        let workspace = outer.join("memstead");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(outer.join(".git"), "gitdir: /elsewhere\n").unwrap();
        let found = find_enclosing_git_repo(&workspace).expect("should detect .git file");
        assert_eq!(found, outer);
    }

    #[test]
    fn ignore_check_matches_bare_vault_repo() {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        let workspace = outer.join("memstead");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(outer.join(".gitignore"), "vault-repo/\n").unwrap();
        assert!(outer_repo_ignores_vault_repo(&outer, &workspace));
    }

    #[test]
    fn ignore_check_matches_workspace_prefixed() {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        let workspace = outer.join("memstead");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(outer.join(".gitignore"), "memstead/vault-repo/\n").unwrap();
        assert!(outer_repo_ignores_vault_repo(&outer, &workspace));
    }

    #[test]
    fn ignore_check_returns_false_when_not_listed() {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        let workspace = outer.join("memstead");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(outer.join(".gitignore"), "node_modules/\ntarget/\n").unwrap();
        assert!(!outer_repo_ignores_vault_repo(&outer, &workspace));
    }

    #[test]
    fn ignore_check_returns_false_when_no_gitignore() {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        let workspace = outer.join("memstead");
        fs::create_dir_all(&workspace).unwrap();
        assert!(!outer_repo_ignores_vault_repo(&outer, &workspace));
    }

    #[test]
    fn ignore_check_honours_negation() {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        let workspace = outer.join("memstead");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(outer.join(".gitignore"), "vault-repo/\n!vault-repo/\n").unwrap();
        assert!(!outer_repo_ignores_vault_repo(&outer, &workspace));
    }

    #[test]
    fn ignore_check_skips_comments() {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        let workspace = outer.join("memstead");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(outer.join(".gitignore"), "# vault-repo/\n").unwrap();
        assert!(!outer_repo_ignores_vault_repo(&outer, &workspace));
    }
}
