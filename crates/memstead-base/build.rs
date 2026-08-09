//! Build identity: best-effort capture of the git commit the binary
//! is built from, so dev builds between releases are distinguishable
//! (the plan-05 "version changed → re-read roster" signal and the
//! plan-02 ENGINE_VERSION_SKEW stamp comparison can fire in dogfood
//! use, where every build otherwise reports the same crate semver).
//!
//! `MEMSTEAD_BUILD_SHA` is ALWAYS emitted — the short HEAD sha (plus
//! a `-dirty` suffix when tracked files are modified) inside a git
//! checkout, the empty string everywhere else (crates.io builds,
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
    // Best-effort rebuild trigger when HEAD moves. Outside a git
    // checkout there is no HEAD file to watch — silently skipped.
    if let Some(git_dir) = git(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
    }
    let sha = git(&["rev-parse", "--short", "HEAD"])
        .map(|sha| {
            // `--untracked-files=no`: only modified TRACKED files
            // mark the build dirty; a stray scratch file does not.
            // A failed probe reads as clean — best-effort throughout.
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some();
            if dirty { format!("{sha}-dirty") } else { sha }
        })
        .unwrap_or_default();
    println!("cargo:rustc-env=MEMSTEAD_BUILD_SHA={sha}");
}
