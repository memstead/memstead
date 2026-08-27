//! Deny-path verdicts for tool-call candidates — the engine half of the
//! plugin's PreToolUse deny hook, and the one home of the deny dialect.
//!
//! A binding's `deny_paths` are **workspace-relative glob patterns**, resolved
//! here by [`DenyOracle`] — the one deny resolver, used verbatim by the `S(D)`
//! enumeration and by the git strategy's post-diff filter, so a path denied at
//! the hook is denied everywhere. Sharing the `globset` library alone was not
//! enough: the two rules below extend the raw match, and until 2026-08-27 the
//! enumeration did not have them.
//!
//! Their resolution root is the WORKSPACE, while a facet's own `scope`
//! patterns resolve against their source's `pointer`. Two namespaces for two
//! questions: an ingest deny spans every source in the binding. The plugin hook
//! used to re-implement these semantics in JavaScript against an
//! engine-written deny-list cache; both are retired. The hook now asks the
//! engine (`memstead projection check-path`), and the only cross-process
//! state left is a **pointer** to the active binding — the deny list itself
//! is read fresh from the binding record on every check, so a stale *list*
//! can no longer be enforced by construction.
//!
//! Two rules extend the raw glob match, ported from the hook they replace:
//!
//! - **Directory-prefix rule.** The literal base of an entry (the portion
//!   before its first glob metacharacter, trailing `/` trimmed) blocks the
//!   directory itself: `dev/**` also blocks a read targeted at `dev`, which
//!   the glob alone would let through. A legacy bare name (`dev`) degrades to
//!   the same prefix block instead of erroring.
//! - **`..` candidates match.** A candidate outside the workspace resolves to
//!   a `../…` relative path and is matched verbatim — the dogfood mediums
//!   point at sibling directories, denied by `../…` entries.
//!
//! A malformed deny entry never disables enforcement: its glob half is
//! skipped, its literal-base prefix rule still applies.

use std::path::{Component, Path, PathBuf};

use globset::Glob;

use super::cursor::{normalize_lexical, relative_path};

/// One candidate's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathVerdict {
    /// The candidate as supplied.
    pub path: String,
    /// Whether the candidate is denied for the binding.
    pub denied: bool,
    /// The deny entry that matched (first match in declaration order), when
    /// denied — the machine-readable "why" a blocking consumer reports.
    pub matched: Option<String>,
}

/// Evaluate tool-call candidates against a binding's `deny_paths`.
///
/// Each candidate — a path or a Glob/Grep pattern — is resolved to its
/// workspace-relative form (absolute candidates as-is, relative ones against
/// `cwd`; `/`-separated; may contain `..`) and matched against every deny
/// entry: the entry's glob (engine `globset`, the enumeration dialect) plus
/// the literal-base directory-prefix rule. An empty deny list denies nothing
/// (default-open). Non-path candidate strings (a Grep regex like `TODO`)
/// resolve to harmless paths and match nothing.
pub fn check_deny_paths(
    deny_paths: &[String],
    candidates: &[String],
    cwd: &Path,
    workspace_root: &Path,
) -> Vec<PathVerdict> {
    // Symlink-consistent resolution: the caller's workspace root may be
    // canonical (the CLI's cwd is) while candidates arrive raw — on macOS a
    // temp path is `/var/…` for one and `/private/var/…` for the other, and a
    // lexical relative-path between them fabricates `../` chains that match
    // nothing. Canonicalising the longest EXISTING prefix of each input puts
    // all three in the same namespace without requiring candidates to exist
    // (they may be Glob patterns).
    let cwd = canonicalize_existing_prefix(cwd);
    let workspace_root = canonicalize_existing_prefix(workspace_root);
    let (cwd, workspace_root) = (cwd.as_path(), workspace_root.as_path());
    let oracle = DenyOracle::new(deny_paths);
    candidates
        .iter()
        .map(|candidate| {
            let rel = workspace_relative(candidate, cwd, workspace_root);
            let matched = oracle.matched_entry(&rel).map(str::to_string);
            PathVerdict {
                path: candidate.clone(),
                denied: matched.is_some(),
                matched,
            }
        })
        .collect()
}

/// The one deny resolver — the glob dialect **plus** the two rules that
/// extend it — shared verbatim by the path-check command and by the
/// enumeration that computes `S(D)`.
///
/// Sharing the `globset` library was never enough: the oracle also applied a
/// literal-base directory-prefix rule and a malformed-entry fallback that the
/// enumerator did not have, so the module's claim of "one dialect, one
/// implementation" was true of the library and false of the resolution rules.
/// A path could be denied at the hook and still counted in the denominator.
/// One type now answers both.
#[derive(Debug, Default)]
pub struct DenyOracle {
    entries: Vec<DenyEntry>,
}

#[derive(Debug)]
struct DenyEntry {
    /// The entry as written — what a verdict names.
    raw: String,
    /// Compiled glob, absent when the entry is malformed. A malformed entry
    /// never disables enforcement: its literal-base prefix rule still applies.
    matcher: Option<globset::GlobMatcher>,
    /// Literal prefix before the first glob metacharacter, trailing `/`
    /// trimmed. Empty when the entry opens with a metacharacter.
    base: String,
}

impl DenyOracle {
    /// Compile a deny list once. Empty entries are dropped.
    pub fn new(deny_paths: &[String]) -> Self {
        Self {
            entries: deny_paths
                .iter()
                .filter(|e| !e.is_empty())
                .map(|e| DenyEntry {
                    raw: e.clone(),
                    matcher: Glob::new(e).ok().map(|g| g.compile_matcher()),
                    base: literal_base(e),
                })
                .collect(),
        }
    }

    /// Whether this list denies nothing (default-open).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The first entry (declaration order) denying `rel` — a
    /// `/`-separated workspace-relative path — or `None`.
    pub fn matched_entry(&self, rel: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| {
                e.matcher.as_ref().is_some_and(|m| m.is_match(rel))
                    // Directory-prefix rule: `dev/**` also blocks `dev` itself,
                    // which the glob alone lets through, and a legacy bare name
                    // (`dev`) degrades to the same prefix block.
                    || (!e.base.is_empty()
                        && (rel == e.base || rel.starts_with(&format!("{}/", e.base))))
            })
            .map(|e| e.raw.as_str())
    }

    /// Whether `rel` is denied.
    pub fn is_denied(&self, rel: &str) -> bool {
        self.matched_entry(rel).is_some()
    }
}

/// The candidate's workspace-relative path in `/`-separated form. Absolute
/// candidates resolve as-is; relative ones against `cwd`. May contain `..`
/// when the candidate sits outside the workspace — expected, and matched by
/// `../…` deny entries.
fn workspace_relative(candidate: &str, cwd: &Path, workspace_root: &Path) -> String {
    let path = Path::new(candidate);
    let abs: PathBuf = if path.is_absolute() {
        canonicalize_existing_prefix(path)
    } else {
        normalize_lexical(&cwd.join(path))
    };
    let rel = relative_path(workspace_root, &abs);
    rel.components()
        .map(|c| match c {
            Component::ParentDir => "..".to_string(),
            other => other.as_os_str().to_string_lossy().to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Lexically normalize, then canonicalize the longest EXISTING prefix and
/// re-append the non-existing tail — symlink resolution that tolerates paths
/// (and Glob patterns) naming files that are not there. A path with no
/// existing prefix comes back lexically normalized only.
fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    let norm = normalize_lexical(path);
    let mut existing = norm.as_path();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if existing.exists() {
            break;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                existing = parent;
            }
            _ => return norm,
        }
    }
    let mut out = std::fs::canonicalize(existing).unwrap_or_else(|_| existing.to_path_buf());
    for name in tail.iter().rev() {
        out.push(name);
    }
    out
}

/// The literal path prefix of a deny entry (before its first glob
/// metacharacter), trailing `/` trimmed. Empty when the entry starts with a
/// metacharacter (e.g. a leading globstar) — then only the glob applies.
fn literal_base(entry: &str) -> String {
    let cut = entry
        .find(['*', '?', '[', '{'])
        .map_or(entry, |i| &entry[..i]);
    cut.trim_end_matches('/').to_string()
}

// ── the active-binding pointer ──────────────────────────────────────────────

/// The active-binding pointer:
/// `<workspace>/.memstead.cache/projection/active-binding.json`.
///
/// Successor to the retired deny-list cache. It carries only the canonical id
/// of the binding whose brief was last **consumed** — never a deny list — so
/// the enforcement path re-reads the binding record on every check and a
/// stale list is structurally impossible. A pointer to a binding that no
/// longer resolves refuses typed at the check, and the consumer fails open.
fn active_binding_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".memstead.cache")
        .join("projection")
        .join("active-binding.json")
}

/// Publish `binding_id` as the active binding, **stale-safe**: the previous
/// pointer is unlinked *before* the new write, so a failed write leaves *no*
/// pointer (the check refuses `NO_ACTIVE_BINDING`, consumers fail open)
/// rather than a previous binding's pointer. Best-effort engine cache, not a
/// tracked mutation.
pub fn write_active_binding_file(workspace_root: &Path, binding_id: &str) {
    let path = active_binding_path(workspace_root);
    let _ = std::fs::remove_file(&path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let payload = serde_json::json!({ "binding": binding_id });
    if let Ok(bytes) = serde_json::to_vec(&payload) {
        let _ = std::fs::write(&path, bytes);
    }
}

/// The active binding's canonical id, or `None` when no consuming render has
/// published one (or the pointer is unreadable — same answer, fail open).
pub fn read_active_binding_file(workspace_root: &Path) -> Option<String> {
    let raw = std::fs::read(active_binding_path(workspace_root)).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    value["binding"].as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WS: &str = "/home/dev/memstead";

    fn verdicts(deny: &[&str], candidates: &[&str], cwd: &str) -> Vec<PathVerdict> {
        let deny: Vec<String> = deny.iter().map(|s| s.to_string()).collect();
        let candidates: Vec<String> = candidates.iter().map(|s| s.to_string()).collect();
        check_deny_paths(&deny, &candidates, Path::new(cwd), Path::new(WS))
    }

    fn denied(deny: &[&str], candidate: &str) -> bool {
        verdicts(deny, &[candidate], WS)[0].denied
    }

    /// The retired shared fixture's cases, verbatim — the same entry list must
    /// block the same paths and pass the same paths it pinned when the dialect
    /// lived twice. Now both consumers run THIS code, and this test pins the
    /// dialect itself.
    #[test]
    fn fixture_cases_hold_at_the_one_seam() {
        let entries = &["dev/**", "**/VISION.md", "docs/meta/CLAUDE.md"];
        for blocked in [
            "dev/notes/a.md",
            "dev/x.rs",
            "dev/deep/nested/y.txt",
            "VISION.md",
            "crates/foo/VISION.md",
            "docs/meta/CLAUDE.md",
        ] {
            assert!(denied(entries, blocked), "must block {blocked}");
        }
        for allowed in [
            "src/lib.rs",
            "dev-tools/x.rs",
            "VISION-draft.md",
            "docs/meta/README.md",
            "other/CLAUDE.md",
            "crates/foo/mod.rs",
        ] {
            assert!(!denied(entries, allowed), "must allow {allowed}");
        }
    }

    /// The directory-prefix rule: `dev/**` blocks a read of the directory
    /// `dev` itself (and `dev/`), which the glob alone would allow — the one
    /// behaviour the JS clone carried that the engine had no equivalent for.
    #[test]
    fn subtree_entry_blocks_the_directory_itself() {
        let entries = &["dev/**"];
        assert!(denied(entries, "dev"));
        assert!(denied(entries, "dev/"));
        assert!(denied(entries, &format!("{WS}/dev")));
        // Sibling names do not over-match.
        assert!(!denied(entries, "dev-tools/x.rs"));
    }

    /// A legacy bare name (the pre-glob dialect) degrades to a
    /// directory/file-prefix block instead of erroring.
    #[test]
    fn legacy_bare_names_degrade_to_prefix_blocks() {
        let entries = &["VISION.md", "CLAUDE.md", "dev"];
        assert!(denied(entries, "CLAUDE.md"));
        assert!(denied(entries, &format!("{WS}/VISION.md")));
        assert!(denied(entries, "dev/notes/foo.md"));
        assert!(denied(entries, "dev"));
        // Sub-area CLAUDE.md files and similar names stay readable.
        assert!(!denied(entries, "subdir/CLAUDE.md"));
        assert!(!denied(entries, "VISION-draft.md"));
        assert!(!denied(entries, "engine/dev-tools/foo.rs"));
    }

    /// Glob/Grep patterns recursing a denied subtree are candidates too, and
    /// the same match logic catches them.
    #[test]
    fn glob_pattern_candidates_are_blocked() {
        let entries = &["dev/**"];
        assert!(denied(entries, "dev/**/*.md"));
        assert!(denied(entries, &format!("{WS}/dev/**")));
        assert!(denied(entries, "dev/notes/*"));
    }

    /// The dogfood `../` cross-medium dialect: deny entries and candidates
    /// both resolve against the workspace root, so a `../dev/**` entry blocks
    /// a sibling-directory read while in-workspace files stay readable.
    #[test]
    fn dot_dot_entries_match_out_of_workspace_candidates() {
        let ws = format!("{WS}/graph");
        let deny: Vec<String> = vec!["../dev/**".into(), "../CLAUDE.md".into()];
        let candidates: Vec<String> = vec![
            format!("{WS}/dev/notes/a.md"),
            format!("{WS}/CLAUDE.md"),
            format!("{ws}/src/x.rs"),
        ];
        let v = check_deny_paths(&deny, &candidates, Path::new(&ws), Path::new(&ws));
        assert!(v[0].denied, "sibling dev/ read is blocked");
        assert!(v[1].denied, "sibling CLAUDE.md read is blocked");
        assert!(!v[2].denied, "in-workspace file is untouched");
    }

    /// Default-open: an empty deny list denies nothing, and candidates
    /// outside the workspace are allowed unless an entry names them.
    #[test]
    fn empty_list_and_outside_paths_are_open() {
        assert!(!denied(&[], "CLAUDE.md"));
        assert!(!denied(&[], "dev/notes/foo.md"));
        assert!(!denied(&["dev/**"], "/etc/hosts"));
        assert!(!denied(&["dev/**"], "/tmp/something.md"));
    }

    /// Relative candidates resolve against `cwd`, not the workspace root —
    /// an agent working in a subdirectory still cannot reach a denied file
    /// through a relative path.
    #[test]
    fn relative_candidates_resolve_against_cwd() {
        let v = verdicts(&["dev/**"], &["../dev/notes/a.md"], &format!("{WS}/graph"));
        assert!(v[0].denied);
        let v = verdicts(&["dev/**"], &["notes/a.md"], &format!("{WS}/dev"));
        assert!(v[0].denied);
    }

    /// The verdict names the FIRST matching entry in declaration order — the
    /// machine-readable "why" for a blocking consumer's message.
    #[test]
    fn matched_entry_is_named_in_order() {
        let v = verdicts(&["**/VISION.md", "dev/**"], &["dev/VISION.md"], WS);
        assert_eq!(v[0].matched.as_deref(), Some("**/VISION.md"));
        let v = verdicts(&["dev/**"], &["src/lib.rs"], WS);
        assert_eq!(v[0].matched, None);
        assert!(!v[0].denied);
    }

    /// A malformed glob entry never disables enforcement: its literal-base
    /// prefix rule still applies, and the other entries are unaffected.
    #[test]
    fn malformed_entry_degrades_to_prefix_rule() {
        // `[` unclosed — globset refuses the pattern; the base `secrets/`
        // still prefix-blocks the subtree.
        let entries = &["secrets/[", "dev/**"];
        assert!(denied(entries, "secrets/key.txt"));
        assert!(denied(entries, "dev/x.rs"));
        assert!(!denied(entries, "src/lib.rs"));
    }

    /// Engine `globset` semantics apply beyond the old JS parity boundary:
    /// character classes and brace alternates match as globs — never treated
    /// as literals. (The literal-base prefix rule still applies on top, as it
    /// always has: an entry like `docs/[ab].md` also prefix-guards `docs/`
    /// up to its literal base `docs/` — deny-side over-blocking is the
    /// conservative direction and matches the retired hook verbatim.)
    #[test]
    fn character_classes_and_braces_follow_engine_semantics() {
        assert!(denied(&["docs/x[ab].md"], "docs/xa.md"));
        assert!(!denied(&["docs/x[ab].md"], "docs/xz.md"));
        assert!(denied(&["**/*.{png,jpg}"], "assets/logo.png"));
        assert!(!denied(&["**/*.{png,jpg}"], "assets/logo.svg"));
    }

    /// The pointer channel: publish X, overwrite with Y, and read back —
    /// nothing of X survives a later consume, and a missing pointer reads as
    /// `None` (consumers fail open).
    #[test]
    fn active_binding_pointer_overwrites_and_fails_open() {
        let ws = tempfile::tempdir().unwrap();
        assert_eq!(read_active_binding_file(ws.path()), None);

        write_active_binding_file(ws.path(), "engine/x-graph");
        assert_eq!(
            read_active_binding_file(ws.path()).as_deref(),
            Some("engine/x-graph")
        );

        write_active_binding_file(ws.path(), "project/y-graph");
        assert_eq!(
            read_active_binding_file(ws.path()).as_deref(),
            Some("project/y-graph")
        );
    }
}
