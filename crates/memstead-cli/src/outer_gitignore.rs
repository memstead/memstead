//! Shared helper: detect the enclosing git repo and append a path to
//! its `.gitignore`.
//!
//! Used by both `memstead mem init` (legacy disk-mem bootstrap) and
//! `memstead mem-repo init` (post-cutover mem-repo-git bootstrap). The two
//! commands differ in *what* path they ignore (mem root vs. the
//! `mem-repo/` directory) but share every other rule:
//!
//! - Walk upward from a starting directory looking for `.git/`.
//! - Refuse to modify a `.gitignore` whose owning repo is `$HOME` —
//!   silent edits to dotfile repos are a recognized footgun.
//! - Stop the walk at filesystem root and at mount-boundary crossings
//!   (the latter via `metadata().dev()` on unix; disabled elsewhere).
//! - Append idempotently — if the ignore line is already present
//!   (modulo leading/trailing slash), no change is made.
//!
//! Public surface: [`apply_outer_gitignore`] takes the start directory
//! plus the path to ignore (which must be inside the outer repo for the
//! relative computation to succeed); returns an [`OuterRepoOutcome`]
//! the caller renders into a user-facing message.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::CliError;
use crate::output::ExitKind;

/// Outcome of the outer-repo `.gitignore` handling step.
#[derive(Debug)]
pub enum OuterRepoOutcome {
    /// A new line was appended to `<outer_root>/.gitignore`.
    Appended { outer_root: PathBuf, rel: String },
    /// An equivalent line already existed; no change.
    AlreadyIgnored { outer_root: PathBuf, rel: String },
    /// No outer git repo was found (or the walk crossed a mount).
    NoOuter,
    /// Caller passed `--no-gitignore` (or the equivalent suppression).
    Skipped,
}

/// Walk upward from `start` looking for an outer `.git/` directory; on
/// success append `ignore_path` (rendered relative to the outer root)
/// to that repo's `.gitignore`. The starting directory is the parent of
/// the new gitdir / mem-repo so we don't rediscover our own git.
///
/// `ignore_path` must lie inside the discovered outer repo for the
/// relative-path rendering to succeed; if `strip_prefix` fails the
/// helper returns [`OuterRepoOutcome::NoOuter`] defensively rather than
/// emit a garbled rule.
///
/// Refuses with a `Validation` error when the discovered outer root
/// equals `$HOME`. Callers are expected to render the error verbatim
/// or surface the `--no-gitignore` suggestion themselves.
pub fn apply_outer_gitignore(
    start: &Path,
    ignore_path: &Path,
) -> anyhow::Result<OuterRepoOutcome> {
    let mut cursor = start.to_path_buf();
    let start_dev = device_id(&cursor);

    loop {
        if cursor.join(".git").is_dir() {
            let outer_root = cursor.clone();
            let outer_dev = device_id(&outer_root);

            if start_dev.is_some() && outer_dev != start_dev {
                return Ok(OuterRepoOutcome::NoOuter);
            }

            return write_to_outer_gitignore(&outer_root, ignore_path);
        }
        match cursor.parent() {
            Some(parent) => {
                let parent_dev = device_id(parent);
                if start_dev.is_some() && parent_dev != start_dev {
                    return Ok(OuterRepoOutcome::NoOuter);
                }
                cursor = parent.to_path_buf();
            }
            None => return Ok(OuterRepoOutcome::NoOuter),
        }
    }
}

fn write_to_outer_gitignore(
    outer_root: &Path,
    ignore_path: &Path,
) -> anyhow::Result<OuterRepoOutcome> {
    if is_home_dir(outer_root) {
        return Err(CliError {
            code: "OUTER_GITIGNORE_HOME_REFUSED",
            kind: ExitKind::Validation,
            message: format!(
                "detected outer git repo at {} which equals $HOME; refusing to \
                 modify ~/.gitignore. Re-run with --no-gitignore (and edit \
                 ~/.gitignore manually if desired) or place the target under \
                 a different parent directory.",
                outer_root.display()
            ),
            details: None,
        }
        .into());
    }

    let rel = match ignore_path.strip_prefix(outer_root) {
        Ok(r) => format!("{}/", r.display()),
        Err(_) => {
            return Ok(OuterRepoOutcome::NoOuter);
        }
    };

    let gitignore_path = outer_root.join(".gitignore");
    let existing = fs::read_to_string(&gitignore_path).unwrap_or_default();

    let needle = rel.trim_end_matches('/');
    let already_ignored = existing.lines().any(|line| {
        let t = line.trim().trim_start_matches('/').trim_end_matches('/');
        t == needle
    });

    if already_ignored {
        return Ok(OuterRepoOutcome::AlreadyIgnored {
            outer_root: outer_root.to_path_buf(),
            rel,
        });
    }

    let mut block = String::new();
    if !existing.is_empty() && !existing.ends_with('\n') {
        block.push('\n');
    }
    if !existing.is_empty() {
        block.push('\n');
    }
    block.push_str("# added by `memstead-cli`\n");
    block.push_str(&rel);
    block.push('\n');

    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore_path)
        .map_err(|e| {
            CliError {
                code: crate::INTERNAL_CODE,
                kind: ExitKind::Generic,
                message: format!("open outer .gitignore: {e}"),
                details: None,
            }
        })?;
    f.write_all(block.as_bytes()).map_err(|e| CliError {
        code: crate::INTERNAL_CODE,
        kind: ExitKind::Generic,
        message: format!("append to outer .gitignore: {e}"),
        details: None,
    })?;

    Ok(OuterRepoOutcome::Appended {
        outer_root: outer_root.to_path_buf(),
        rel,
    })
}

fn is_home_dir(path: &Path) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let canon_home = fs::canonicalize(&home).unwrap_or(home);
    let canon_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    canon_path == canon_home
}

#[cfg(unix)]
fn device_id(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).ok().map(|m| m.dev())
}

#[cfg(not(unix))]
fn device_id(_path: &Path) -> Option<u64> {
    None
}
