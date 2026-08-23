//! Build identity: best-effort capture of the git commit the binary
//! is built from, so dev builds between releases are distinguishable
//! (the plan-05 "version changed → re-read roster" signal and the
//! plan-02 ENGINE_VERSION_SKEW stamp comparison can fire in dogfood
//! use, where every build otherwise reports the same crate semver).
//!
//! `MEMSTEAD_BUILD_SHA` is ALWAYS emitted — the short HEAD sha (plus
//! a `-dirty` suffix when tracked build inputs are modified) inside a
//! git checkout, the empty string everywhere else (crates.io builds,
//! vendored trees): a failing git probe must never break the build.
//! `crate::build_info` turns the value into the full build version.

use std::process::Command;

/// Run `git <args>` in the crate directory; `None` on any failure or
/// empty output — the caller treats every `None` as "no git identity".
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn main() {
    // Best-effort rebuild trigger when the commit moves. Outside a git
    // checkout there is nothing to watch — silently skipped.
    //
    // Watching `HEAD` alone is NOT enough, and the gap is easy to miss:
    // `HEAD` holds `ref: refs/heads/<branch>` and only changes when you
    // switch branches. Committing on the branch you are already on
    // rewrites `refs/heads/<branch>`, leaving `HEAD` byte-identical — so
    // the stamped sha went stale across every ordinary commit and only
    // refreshed on a checkout. That matters because the MCP server's
    // instructions tell agents `serverInfo.version` changing is the
    // signal to re-read the tool roster: a stale sha silently retires
    // the surface's own change signal. Watch the resolved ref too, plus
    // `packed-refs` for the case where the loose ref file does not exist.
    if let Some(git_dir) = git(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/packed-refs");
        if let Some(head_ref) = git(&["rev-parse", "--symbolic-full-name", "HEAD"]) {
            // Detached HEAD reports `HEAD`; that file is already watched.
            if head_ref != "HEAD" {
                println!("cargo:rerun-if-changed={git_dir}/{head_ref}");
            }
        }
        // The dirty flag must follow the working tree, not only the refs:
        // with the refs alone, editing a crate and building recompiled
        // the crate without re-running this script, so the stamp kept
        // the clean form it had computed at the last commit and the
        // `-dirty` suffix could only ever appear after HEAD moved, when
        // the tree was clean again. Watch the build inputs the probe
        // scopes to (a directory entry re-runs on any change beneath it);
        // the cost is one `git status` per build that would rebuild
        // anyway.
        if let Some(top) = git(&["rev-parse", "--show-toplevel"]) {
            for input in ["crates", "Cargo.toml", "Cargo.lock"] {
                println!("cargo:rerun-if-changed={top}/{input}");
            }
        }
    }
    let sha = git(&["rev-parse", "--short", "HEAD"])
        .map(|sha| {
            // `--untracked-files=no`: only modified TRACKED files
            // mark the build dirty; a stray scratch file does not.
            // Scoped to the build inputs (the crates and the two
            // manifests): a modified doc, workflow or folder-mem file
            // elsewhere in the repository changes no byte of the
            // binary, and a `-dirty` stamp it did not earn would
            // fail the staleness check that compares the stamp with
            // the committed tree. A failed probe reads as clean —
            // best-effort throughout.
            // `:/` anchors each pathspec to the repository top: the
            // script runs in the crate directory, where a bare
            // `crates` would name a path that does not exist and match
            // nothing (which read as "clean").
            let dirty = git(&[
                "status",
                "--porcelain",
                "--untracked-files=no",
                "--",
                ":/crates",
                ":/Cargo.toml",
                ":/Cargo.lock",
            ])
            .is_some();
            if dirty { format!("{sha}-dirty") } else { sha }
        })
        .unwrap_or_default();
    println!("cargo:rustc-env=MEMSTEAD_BUILD_SHA={sha}");
}
