#![cfg(test)]

/// The remote comparison is read-only by construction: the function's
/// own source reaches the git-branch hooks through `ls_remote`,
/// `resolve_ref` and `is_ancestor` only, and names no hook that moves a
/// ref. A future edit that adds `fetch`, `pull`, `push`, `branch_reset`
/// or `remote_add` to it fails here before it ships.
#[test]
fn remote_status_carries_no_mutating_git_hook() {
    let src = include_str!("../drift.rs");
    let start = src
        .find("pub fn remote_status(")
        .expect("remote_status is defined in this file");
    let end = src[start..]
        .find("\n    }\n")
        .map(|i| start + i)
        .expect("remote_status ends");
    let body = &src[start..end];
    for allowed in ["hook.ls_remote", "hook.resolve_ref", "hook.is_ancestor"] {
        assert!(
            body.contains(allowed),
            "remote_status reads through {allowed}"
        );
    }
    for forbidden in [
        "hook.fetch",
        "hook.pull",
        "hook.push",
        "hook.branch_reset",
        "hook.remote_add",
        "hook.rename_mem_storage",
        "hook.prune_residue",
        "hook.write_schema",
    ] {
        assert!(
            !body.contains(forbidden),
            "remote_status must never reach {forbidden}: a status read moves no ref"
        );
    }
}
