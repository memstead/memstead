//! Tier 3 wiki-link resolution against cached
//! `.memstead/memstead-io/<scope>/<name>.mem` archives.
//!
//! ## What Tier 3 means in filesystem-mem context
//!
//! - **Tier 1** `[[slug]]` — same-mem.
//! - **Tier 2** `[[leaf:slug]]` — cross-mem (mem-repo only —
//!   multi-mem under one repo).
//! - **Tier 3** `[[scope/name:slug]]` — registry-published mem. The
//!   filesystem-mem workspace caches the dep at
//!   `<workspace_root>/.memstead/memstead-io/<scope>/<name>.mem` (populated
//!   by `memstead link <scope/name>`); this module reads that archive
//!   and resolves the slug to a cross-mem [`EntityId`] of the
//!   existing shape.
//!
//! ## mem-repo invariance
//!
//! The wiki-link parser at [`crate::entity::id::wiki_link_to_id`]
//! still falls Tier 3 syntax back to Tier 1 silently — preserving
//! existing mem-repo behaviour. This module is the
//! filesystem-mem-only counterpart that surfaces resolution
//! warnings as a separate validation pass over loaded entities.
//! No engine path consumes it yet — wiring it into the unified
//! [`crate::Engine`] folder-mount load is the pending step.
//!
//! ## What this module does NOT do (yet)
//!
//! - Rewrite `parse_result.inline_links` or store edges. The current
//!   v1 surface returns warnings; the resolved [`crate::EntityId`] is
//!   available via [`Tier3Ref::resolve`] but the engine's load path
//!   does not yet swap the same-mem fallback in `inline_links` for
//!   the resolved cross-mem id. A follow-up plan can take that
//!   final step once the warning-only surface settles.

use std::path::{Path, PathBuf};

use regex::Regex;
use std::sync::OnceLock;

use crate::entity::EntityId;
use crate::entity::loader::LoadError;
use crate::entity::source::EntitySource;

/// Parsed Tier 3 reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier3Ref {
    pub scope: String,
    pub name: String,
    pub slug: String,
}

impl Tier3Ref {
    /// On-disk path of the cached archive for this dep, given the
    /// workspace root — the `.mem` cache file.
    pub fn cache_path(&self, workspace_root: &Path) -> PathBuf {
        self.cache_dir(workspace_root).join(format!(
            "{}.{}",
            self.name,
            memstead_schema::ARCHIVE_EXTENSION
        ))
    }

    fn cache_dir(&self, workspace_root: &Path) -> PathBuf {
        workspace_root
            .join(crate::workspace_store::WORKSPACE_STORE_DIR)
            .join("memstead-io")
            .join(&self.scope)
    }

    /// Resolve to a cross-mem [`EntityId`] by reading the cached
    /// archive. The mem component of the returned id is the
    /// archive's mem `name` (matching the archive's
    /// `.memstead/config.json` `name` field — same value as the dep
    /// reference's `name`).
    ///
    /// Returns [`Tier3ResolveError`] when the cache file is missing,
    /// the archive cannot be read, or the slug is not present.
    pub fn resolve(&self, workspace_root: &Path) -> Result<EntityId, Tier3ResolveError> {
        // `.mem` is what `memstead link` writes — the sole cache spelling.
        let cache_path = self.cache_path(workspace_root);
        if !cache_path.is_file() {
            return Err(Tier3ResolveError::CacheMissing {
                cache_path,
                tier3: self.as_display(),
            });
        }

        let source = EntitySource::ZipArchive(cache_path.clone());
        let (entries, _) = source
            .read_all()
            .map_err(|e| Tier3ResolveError::ArchiveRead {
                cache_path: cache_path.clone(),
                tier3: self.as_display(),
                error: e.to_string(),
            })?;

        // Match by `relative_path` stem — the archive's entity ids
        // are computed from the relative path via
        // `file_path_to_id`, and the slug part of the
        // `[[scope/name:slug]]` reference matches the file path
        // (without the `.md` extension).
        let want = format!("{}.md", self.slug);
        let found = entries.iter().any(|e| e.relative_path == want);
        if !found {
            return Err(Tier3ResolveError::SlugAbsent {
                cache_path,
                tier3: self.as_display(),
            });
        }

        Ok(EntityId::new(&self.name, &self.slug))
    }

    /// Display form used in warning messages and tests:
    /// `<scope>/<name>:<slug>`.
    pub fn as_display(&self) -> String {
        format!("{}/{}:{}", self.scope, self.name, self.slug)
    }
}

impl std::fmt::Display for Tier3Ref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_display())
    }
}

/// Errors surfaced by [`Tier3Ref::resolve`].
#[derive(Debug, thiserror::Error)]
pub enum Tier3ResolveError {
    /// The cached archive does not exist on disk. Run `memstead link
    /// <scope>/<name>` to populate it.
    #[error(
        "tier 3 link {tier3} cannot resolve: cached archive missing at {} \
         — run `memstead link {{scope}}/{{name}}` to populate it",
        cache_path.display()
    )]
    CacheMissing { cache_path: PathBuf, tier3: String },
    /// The cached archive is present but does not contain an entity
    /// with the requested slug. The dep version may be stale (run
    /// `memstead link <scope>/<name>` again to refresh) or the slug may
    /// be a typo.
    #[error(
        "tier 3 link {tier3} cannot resolve: slug not found in cached archive at {}",
        cache_path.display()
    )]
    SlugAbsent { cache_path: PathBuf, tier3: String },
    /// The cached archive could not be read — corrupt zip,
    /// permission error, or similar. Concrete IO error message is
    /// preserved for debugging.
    #[error(
        "tier 3 link {tier3} cannot resolve: archive at {} unreadable: {error}",
        cache_path.display()
    )]
    #[allow(dead_code)]
    ArchiveRead {
        cache_path: PathBuf,
        tier3: String,
        error: String,
    },
}

impl Tier3ResolveError {
    /// Workspace-relative reference being resolved (e.g.
    /// `"anthropic/core:agents"`). Used to attach context in
    /// validation-pass warning emission.
    pub fn tier3(&self) -> &str {
        match self {
            Tier3ResolveError::CacheMissing { tier3, .. } => tier3,
            Tier3ResolveError::SlugAbsent { tier3, .. } => tier3,
            Tier3ResolveError::ArchiveRead { tier3, .. } => tier3,
        }
    }
}

/// LoadError thin alias used by callers that want to forward archive
/// IO problems through the load-error surface. Not constructed
/// inside this module — exposed only so external callers don't have
/// to reach into `crate::entity::loader` for the type.
pub type Tier3LoadError = LoadError;

/// Match `[[scope/name:slug]]` exactly. Same character class as the
/// strict slug regex (lowercase + digits + hyphens), and rejects
/// extra `[` / `]` / `:` / whitespace inside the link body.
fn tier3_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Tier 3 syntax: scope and name are slug-shaped per the
        // registry's own validator. Slug part is more permissive —
        // matches the parser's wider id-character class so legacy
        // entities with `--`-shaped paths can still be referenced.
        Regex::new(
            r"\[\[([a-z0-9][a-z0-9-]{0,62}[a-z0-9])/([a-z0-9][a-z0-9-]{0,62}[a-z0-9]):([A-Za-z0-9][A-Za-z0-9_./\-]*)\]\]",
        )
        .expect("tier-3 regex must compile")
    })
}

/// Walk `text` for every `[[scope/name:slug]]` occurrence and
/// produce a [`Tier3Ref`] per hit. Iteration is in-order; duplicate
/// references in the same body are emitted once per occurrence
/// (callers that want unique-by-key dedup do so themselves).
pub fn extract_tier3_refs(text: &str) -> Vec<Tier3Ref> {
    let re = tier3_re();
    re.captures_iter(text)
        .map(|cap| Tier3Ref {
            scope: cap[1].to_string(),
            name: cap[2].to_string(),
            slug: cap[3].to_string(),
        })
        .collect()
}

/// Validation-pass result: one entry per Tier 3 reference that
/// could not resolve.
#[derive(Debug, Clone)]
pub struct Tier3Warning {
    /// The entity that contained the unresolved reference.
    pub entity_id: EntityId,
    /// The Tier 3 reference body, e.g. `"anthropic/core:agents"`.
    pub tier3: String,
    /// Why resolution failed. Stable string — agents grep on the
    /// prefix (`cache missing` vs `slug not found`).
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    fn write_archive(path: &Path, entries: &[(&str, &str)]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, content) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    fn cache_archive(workspace_root: &Path, scope: &str, name: &str, entries: &[(&str, &str)]) {
        let dir = workspace_root
            .join(".memstead")
            .join("memstead-io")
            .join(scope);
        std::fs::create_dir_all(&dir).unwrap();
        write_archive(&dir.join(format!("{name}.mem")), entries);
    }

    #[test]
    fn extract_tier3_refs_finds_simple_references() {
        let body = "See [[anthropic/core:agents]] and [[scope/name:foo-bar]].";
        let refs = extract_tier3_refs(body);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].as_display(), "anthropic/core:agents");
        assert_eq!(refs[1].as_display(), "scope/name:foo-bar");
    }

    #[test]
    fn extract_tier3_refs_ignores_tier1_and_tier2() {
        // Tier 1 (`[[slug]]`) and Tier 2 (`[[leaf:slug]]`) must NOT
        // match — only the three-part scope/name:slug form.
        let body = "Tier 1: [[plain]]. Tier 2: [[leaf:slug]]. Mixed.";
        let refs = extract_tier3_refs(body);
        assert!(refs.is_empty());
    }

    #[test]
    fn extract_tier3_refs_rejects_uppercase_in_scope_or_name() {
        let body = "[[Anthropic/core:agents]] and [[anthropic/Core:agents]]";
        let refs = extract_tier3_refs(body);
        assert!(refs.is_empty());
    }

    #[test]
    fn resolve_succeeds_against_present_cache() {
        let tmp = TempDir::new().unwrap();
        cache_archive(
            tmp.path(),
            "anthropic",
            "core",
            &[
                (
                    "agents.md",
                    "---\ntype: spec\n---\n# Agents\n\n## Identity\n\nA.\n",
                ),
                (
                    "tools.md",
                    "---\ntype: spec\n---\n# Tools\n\n## Identity\n\nT.\n",
                ),
            ],
        );

        let r = Tier3Ref {
            scope: "anthropic".into(),
            name: "core".into(),
            slug: "agents".into(),
        };
        let id = r.resolve(tmp.path()).unwrap();
        assert_eq!(id.as_ref(), "core--agents");
    }

    #[test]
    fn resolve_fails_when_cache_missing() {
        let tmp = TempDir::new().unwrap();
        let r = Tier3Ref {
            scope: "anthropic".into(),
            name: "core".into(),
            slug: "agents".into(),
        };
        let err = r.resolve(tmp.path()).expect_err("missing cache must error");
        match err {
            Tier3ResolveError::CacheMissing { .. } => {}
            other => panic!("expected CacheMissing, got {other:?}"),
        }
        assert_eq!(err.tier3(), "anthropic/core:agents");
    }

    #[test]
    fn resolve_fails_when_slug_absent_from_cache() {
        let tmp = TempDir::new().unwrap();
        cache_archive(
            tmp.path(),
            "anthropic",
            "core",
            &[(
                "tools.md",
                "---\ntype: spec\n---\n# Tools\n\n## Identity\n\nT.\n",
            )],
        );

        let r = Tier3Ref {
            scope: "anthropic".into(),
            name: "core".into(),
            slug: "agents".into(),
        };
        let err = r.resolve(tmp.path()).expect_err("absent slug must error");
        match err {
            Tier3ResolveError::SlugAbsent { .. } => {}
            other => panic!("expected SlugAbsent, got {other:?}"),
        }
    }

    #[test]
    fn cache_path_lands_under_memstead_memstead_io() {
        let r = Tier3Ref {
            scope: "anthropic".into(),
            name: "core".into(),
            slug: "agents".into(),
        };
        let path = r.cache_path(Path::new("/ws"));
        assert_eq!(
            path,
            PathBuf::from("/ws/.memstead/memstead-io/anthropic/core.mem")
        );
    }
}
