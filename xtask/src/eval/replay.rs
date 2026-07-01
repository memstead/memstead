//! Git-history replay for the compounding axis.
//!
//! The git-branch backend stores each vault as a branch in `vault-repo`; the
//! vault's content at any past point is just that branch at an older commit. To
//! score the task set against a historical state we **copy** the live workspace,
//! move the vault branch ref in the *copy* to the chosen commit, and mount the
//! copy read-only over MCP.
//!
//! The live `vault-repo` is never mutated — only read as the copy source — so
//! this honours the engine-owns-vault-repo rule: every ref edit lands on a
//! throwaway copy the agent only ever reads.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use super::VaultState;

/// The mcp-config JSON that mounts the workspace at `graph_dir` as a server named
/// `memstead` (so `--allowedTools mcp__memstead__*` matches).
pub fn mcp_config_json(graph_dir: &Path, mcp_binary: &Path) -> String {
    let cmd = format!("cd {} && exec {}", graph_dir.display(), mcp_binary.display());
    serde_json::json!({
        "mcpServers": { "memstead": { "command": "sh", "args": ["-c", cmd] } }
    })
    .to_string()
}

/// Pick `count` commits evenly across `commits` (oldest→newest), always including
/// the first and last so the series spans the full growth of the vault.
pub fn pick_commits(commits: &[String], count: usize) -> Vec<String> {
    if commits.is_empty() || count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![commits[commits.len() - 1].clone()];
    }
    if count >= commits.len() {
        return commits.to_vec();
    }
    let n = commits.len();
    (0..count)
        .map(|i| commits[i * (n - 1) / (count - 1)].clone())
        .collect()
}

/// List commits on `branch` in oldest→newest order.
pub fn branch_commits(vault_repo: &Path, branch: &str) -> Result<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(vault_repo)
        .args(["rev-list", "--reverse", branch])
        .output()
        .with_context(|| format!("git rev-list {branch} in {}", vault_repo.display()))?;
    if !out.status.success() {
        bail!(
            "git rev-list {branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

/// Prepare one historical state: copy the live workspace to `dest_root/<label>`,
/// move `vault_branch` to `commit` in the copy, clear the cached mount/index
/// state so the engine re-reads the moved ref, and write the mcp-config. Returns
/// a [`VaultState`] pointing at the written config.
pub fn prepare_state(
    live_graph: &Path,
    dest_root: &Path,
    label: &str,
    vault_branch: &str,
    commit: &str,
    mcp_binary: &Path,
) -> Result<VaultState> {
    let dest = dest_root.join(label);
    if dest.exists() {
        std::fs::remove_dir_all(&dest)
            .with_context(|| format!("clearing stale state dir {}", dest.display()))?;
    }
    let cp = Command::new("cp")
        .arg("-R")
        .arg(live_graph)
        .arg(&dest)
        .status()
        .context("copying live workspace")?;
    if !cp.success() {
        bail!("copying {} → {} failed", live_graph.display(), dest.display());
    }
    let vault_repo = dest.join("vault-repo");
    let mv = Command::new("git")
        .arg("-C")
        .arg(&vault_repo)
        .args(["branch", "-f", vault_branch, commit])
        .status()
        .context("moving vault branch ref")?;
    if !mv.success() {
        bail!("git branch -f {vault_branch} {commit} failed in {}", vault_repo.display());
    }
    // Leave `.memstead/state/mounts.json` in place — it is the mount list, and
    // deleting it unmounts every vault. The mount points at the branch *ref*
    // (`refs/heads/<branch>`), so moving the ref is enough: the engine detects
    // the changed HEAD and reloads the vault's content from the new commit.

    let cfg_path = dest_root.join(format!("{label}.mcp.json"));
    std::fs::write(&cfg_path, mcp_config_json(&dest, mcp_binary))
        .with_context(|| format!("writing mcp-config {}", cfg_path.display()))?;
    Ok(VaultState {
        label: label.to_string(),
        mcp_config: Some(cfg_path),
    })
}

/// Prepare `count` states spanning `vault_branch`'s history (oldest→newest). The
/// oldest is the near-empty state the compounding axis must read as ~0 delta; the
/// newest is the current graph.
pub fn prepare_history(
    live_graph: &Path,
    dest_root: &Path,
    vault_branch: &str,
    count: usize,
    mcp_binary: &Path,
) -> Result<Vec<VaultState>> {
    let commits = branch_commits(&live_graph.join("vault-repo"), vault_branch)?;
    if commits.is_empty() {
        bail!("no commits on {vault_branch}");
    }
    let picked = pick_commits(&commits, count);
    std::fs::create_dir_all(dest_root)
        .with_context(|| format!("creating state dir {}", dest_root.display()))?;
    picked
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let label = format!("s{:02}-{}", i, &c[..8.min(c.len())]);
            prepare_state(live_graph, dest_root, &label, vault_branch, c, mcp_binary)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_config_mounts_named_memstead_server() {
        let json = mcp_config_json(Path::new("/x/graph"), Path::new("/x/memstead-mcp"));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["mcpServers"]["memstead"]["command"], "sh");
        let arg = v["mcpServers"]["memstead"]["args"][1].as_str().unwrap();
        assert!(arg.contains("cd /x/graph"), "{arg}");
        assert!(arg.contains("exec /x/memstead-mcp"), "{arg}");
    }

    fn commits(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("c{i:02}")).collect()
    }

    #[test]
    fn pick_spans_first_and_last() {
        let c = commits(10);
        let picked = pick_commits(&c, 3);
        assert_eq!(picked, vec!["c00", "c04", "c09"]);
        // First and last are always present.
        assert_eq!(picked.first().unwrap(), "c00");
        assert_eq!(picked.last().unwrap(), "c09");
    }

    #[test]
    fn pick_two_is_endpoints() {
        assert_eq!(pick_commits(&commits(426), 2), vec!["c00", "c425"]);
    }

    #[test]
    fn pick_one_is_newest() {
        assert_eq!(pick_commits(&commits(10), 1), vec!["c09"]);
    }

    #[test]
    fn pick_count_ge_len_returns_all() {
        let c = commits(3);
        assert_eq!(pick_commits(&c, 5), c);
    }

    #[test]
    fn pick_empty_or_zero_is_empty() {
        assert!(pick_commits(&[], 3).is_empty());
        assert!(pick_commits(&commits(5), 0).is_empty());
    }
}
