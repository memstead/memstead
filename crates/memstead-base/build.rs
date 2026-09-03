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
//!
//! One case inside a checkout is deliberately sha-LESS: a clean build
//! whose HEAD carries the tag of the crate version it is building. That
//! is a release build, and its bare semver is the whole truth about it —
//! the tag pins the commit, so the sha adds nothing a consumer cannot
//! already look up. The distinction is load-bearing beyond cosmetics:
//! it is the only signal by which a consumer holding just a version
//! string (the plugin capability gate, which owns no repository) can
//! tell "this IS release 0.18.0" from "this is some build that calls
//! itself 0.18.0". Version thresholds are release numbers, so a build
//! that is not a release cannot be placed on that ladder at all, and
//! the gate has to fail closed instead of guessing.

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

/// Is this build a release of `version`?
///
/// Two signals, both requiring the version to match, so neither can
/// declare some other release's build a release of this one.
///
/// `MEMSTEAD_RELEASE_VERSION` is the explicit escape hatch: a builder
/// that knows it is cutting a release says so, for the case where no
/// usable git identity is at hand. It is checked first and it is the
/// only signal that works without git.
///
/// Otherwise HEAD's own tags decide. `git tag --points-at HEAD` reads
/// the local tag refs, which is what a tag-ref checkout leaves behind —
/// verified against a depth-1 `fetch`+`checkout` of `refs/tags/v0.17.0`,
/// the shape the release workflow's checkout produces, where the tag is
/// present and the sha matches the published artifact's. No tags, no
/// git, or a tag naming a different version all read as "not a release
/// build": the sha-bearing form is the conservative answer everywhere.
fn is_release_build(version: &str) -> bool {
    println!("cargo:rerun-if-env-changed=MEMSTEAD_RELEASE_VERSION");
    if let Ok(declared) = std::env::var("MEMSTEAD_RELEASE_VERSION") {
        let declared = declared.trim();
        let declared = declared.strip_prefix('v').unwrap_or(declared);
        if !declared.is_empty() {
            return declared == version;
        }
    }
    let Some(tags) = git(&["tag", "--points-at", "HEAD"]) else {
        return false;
    };
    tags.lines()
        .map(str::trim)
        .any(|tag| tag == format!("v{version}") || tag == version)
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
    // A clean build at the crate version's own tag is a release build:
    // report the bare semver. A dirty tree keeps its sha even at the tag —
    // the tree no longer IS the tag, and that is exactly the build a
    // release-only claim must not cover.
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let sha = if sha.is_empty() || sha.ends_with("-dirty") || !is_release_build(&version) {
        sha
    } else {
        String::new()
    };
    println!("cargo:rustc-env=MEMSTEAD_BUILD_SHA={sha}");
}
