//! Filesystem-mem → `.mem` archive assembler.
//!
//! Walks a workspace root on disk, reads the workspace config,
//! projects it to the strict archive shape, embeds the resolved schema
//! source under `.memstead/schema/`, and packs every entity `.md` file
//! into a deterministic zip.
//!
//! Engine-agnostic: the caller passes a path. Used by both
//! `memstead publish` and `memstead export --format mem`, neither of which
//! needs a live engine for the archive build (the workspace config
//! and the entity walker both read from disk directly).
//!
//! ## Archive layout (matches `memstead-base::validator::archive`)
//!
//! ```text
//! .memstead/config.json               # archive shape (PublishedMemConfig)
//! .memstead/schema/schema.yaml        # schema manifest
//! .memstead/schema/types/<name>.yaml  # per-type definitions
//! <mem-relative entity path>.md     # one per entity in the workspace
//! ```
//!
//! ## Determinism
//!
//! - Entity `.md` files are emitted in mem-relative path order — the
//!   same order [`crate::entity::source::EntitySource::Directory`]
//!   yields them on read.
//! - Schema files come from
//!   [`memstead_schema::collect_schema_source`], which already sorts by
//!   `archive_path`.
//! - The `.memstead/config.json` is serialised pretty-printed for human
//!   inspection but with deterministic key order via
//!   [`memstead_schema::PublishedMemConfig`]'s `Serialize` impl.
//! - Compression is fixed at `Stored` so repeated assembly of the same
//!   workspace yields byte-identical archive bytes (modulo zip-level
//!   timestamps, which the writer leaves at zero by default).
//!
//! ## What this does NOT do
//!
//! - HTTP. The CLI's `commands::publish` posts the bytes; this module
//!   stops at "build the byte buffer". Same separation as the
//!   mem-repo `export_mem` → `commands::publish` flow.
//! - Validate. The bytes go through `validator::archive::extract_entries`
//!   on the registry side; doing it here too would double the work for
//!   the same answer. Tests in this module *do* re-validate so future
//!   layout changes surface as test failures rather than registry
//!   rejections.

use std::io::{Cursor, Write as _};
use std::path::Path;

use memstead_schema::{
    ARCHIVE_CONFIG_PATH, ARCHIVE_SCHEMA_PREFIX, PublishConversionError, SchemaRef,
    SchemaSourceError, collect_schema_source,
};
use zip::CompressionMethod;
use zip::result::ZipError;
use zip::write::SimpleFileOptions;

use super::config::{WorkspaceConfigError, read_workspace_config};
use crate::entity::source::EntitySource;

/// Errors surfaced by [`assemble_archive`].
#[derive(Debug, thiserror::Error)]
pub enum AssembleError {
    /// The workspace config could not be read
    /// or parsed (missing file, malformed JSON, format mismatch).
    #[error("workspace config: {0}")]
    WorkspaceConfig(#[from] WorkspaceConfigError),
    /// The workspace config does not project cleanly to the archive
    /// shape — typically because `version` is unset on the workspace
    /// config.
    #[error("config projection: {0}")]
    Config(#[from] PublishConversionError),
    /// Resolving the schema's source files failed — either the
    /// `name@version` pin does not match any builtin (and there's no
    /// workspace-local schema dir) or the on-disk schema directory is
    /// malformed.
    #[error("schema source: {0}")]
    Schema(#[from] SchemaSourceError),
    /// I/O while reading entity `.md` files from the workspace.
    #[error("workspace io: {0}")]
    Io(String),
    /// Zip-level error while writing into the in-memory buffer.
    /// Should not happen in practice — the buffer is unbounded — but
    /// surfaces cleanly if a future zip version starts failing earlier.
    #[error("zip writer: {0}")]
    Zip(#[from] ZipError),
    /// Serialising the archive's `.memstead/config.json`.
    #[error("config serialisation: {0}")]
    Serialise(#[from] serde_json::Error),
}

/// Build the archive bytes for the workspace at `workspace_root`.
///
/// The caller writes the bytes to a tempfile and POSTs them to the
/// registry — this function does not touch the network. It reads
/// the workspace config and walks every `.md` file
/// under `workspace_root`; both reads are direct (no engine
/// involvement).
pub fn assemble_archive(workspace_root: &Path) -> Result<Vec<u8>, AssembleError> {
    // 1. Read the workspace config and project it to the strict
    //    archive shape.
    let config = read_workspace_config(workspace_root)?;
    let published = config.to_published()?;
    // The projection guarantees a versioned schema pin; reuse it for
    // the schema-source resolver.
    let schema_ref: SchemaRef = published.schema.clone();

    // 2. Resolve the schema source files. v1 is built-in-only — pass
    //    `None` for both the workspace-schemas-dir and the
    //    workspace-root cache hint so the resolver falls through to
    //    the embedded set. When workspace-defined schemas land, plumb
    //    the workspace_root + a future schemas_dir override through
    //    here.
    let schema_files = collect_schema_source(None, None, &schema_ref)?;

    // 3. Walk every entity `.md` under the workspace.
    let source = EntitySource::Directory {
        root: workspace_root.to_path_buf(),
    };
    let (source_entries, read_errors) = source
        .read_all()
        .map_err(|e| AssembleError::Io(e.to_string()))?;
    if let Some(first) = read_errors.first() {
        return Err(AssembleError::Io(format!(
            "{}: {}",
            first.source_path.display(),
            first.error
        )));
    }

    // 4. Pack into a zip. Sort entries by archive path for
    //    determinism — the directory walker already sorts but
    //    re-sorting here makes the contract explicit (a future change
    //    in the walker won't break archive determinism).
    let mut buf: Vec<u8> = Vec::new();
    {
        let cursor = Cursor::new(&mut buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let opts = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .last_modified_time(zip::DateTime::default());

        // .memstead/config.json (archive shape — `deps` and other
        // workspace-local fields are dropped by `to_published`).
        let config_bytes = serde_json::to_vec_pretty(&published)?;
        zip.start_file(ARCHIVE_CONFIG_PATH, opts)?;
        zip.write_all(&config_bytes)
            .map_err(|e| AssembleError::Io(format!("write config: {e}")))?;

        // .memstead/schema/* — `collect_schema_source` returns paths
        // rooted at the schema dir (`schema.yaml`, `types/<name>.yaml`).
        // Prepend the archive's `.memstead/schema/` root so the validator
        // picks them up under the right path.
        for sf in &schema_files {
            let archive_path = format!("{ARCHIVE_SCHEMA_PREFIX}{}", sf.archive_path);
            zip.start_file(&archive_path, opts)?;
            zip.write_all(&sf.bytes)
                .map_err(|e| AssembleError::Io(format!("write schema: {e}")))?;
        }

        // Entity .md files. Source-walked paths use the platform
        // separator on Directory; normalise to forward-slash so the
        // archive is portable across OSes.
        let mut entries = source_entries;
        entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        for entry in &entries {
            let archive_path = entry.relative_path.replace('\\', "/");
            zip.start_file(&archive_path, opts)?;
            zip.write_all(entry.content.as_bytes())
                .map_err(|e| AssembleError::Io(format!("write entity {archive_path}: {e}")))?;
        }

        zip.finish()?;
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::filesystem::config::{WorkspaceConfig, write_workspace_config};
    use crate::validator::ValidatorLimits;
    use crate::validator::archive::extract_entries;
    use memstead_schema::SchemaRef;
    use tempfile::TempDir;

    fn versioned(name: &str, version: &str) -> SchemaRef {
        SchemaRef::new(name, semver::Version::parse(version).unwrap())
    }

    /// Create the mem in a folder *named after it* (identity is path-derived
    /// under the unified layout) and return the mem root.
    fn write_workspace(tmp: &TempDir, name: &str, with_version: bool) -> PathBuf {
        let root = tmp.path().join(name);
        std::fs::create_dir_all(&root).unwrap();
        // F1: `WorkspaceConfig::new` now seeds `version = Some(0.1.0)`
        // by default. To simulate the pre-gate / externally-imported
        // config in the no-version test path, clear it explicitly.
        let mut cfg = WorkspaceConfig::new(name, versioned("default", "1.0.0"));
        if with_version {
            cfg.description = Some("test mem".into());
            cfg.add_dep("anthropic/core".parse().unwrap());
        } else {
            cfg.version = None;
        }
        write_workspace_config(&root, &cfg).unwrap();
        root
    }

    /// Write a minimal valid spec entity directly to disk.
    /// `assemble_archive` walks the directory itself so the entity
    /// just needs to exist on disk in canonical markdown form.
    fn write_spec(root: &Path, slug: &str, title: &str) {
        std::fs::write(
            root.join(format!("{slug}.md")),
            format!("---\ntype: spec\n---\n# {title}\n"),
        )
        .unwrap();
    }

    #[test]
    fn assemble_archive_round_trips_through_validator() {
        let tmp = TempDir::new().unwrap();
        let root = write_workspace(&tmp, "demo", true);

        // Two entities so the archive has a non-empty markdown set.
        write_spec(&root, "first", "First");
        write_spec(&root, "second", "Second");

        let bytes = assemble_archive(&root).expect("archive must build");
        assert!(!bytes.is_empty());

        // Round-trip through the same validator the registry uses.
        let limits = ValidatorLimits::default();
        let entries = extract_entries(&bytes, &limits).expect("validator must accept");

        // Config: present and projects to the archive shape (no `deps`).
        let cfg_text = String::from_utf8_lossy(&entries.config_bytes);
        assert!(cfg_text.contains("\"name\": \"demo\""));
        assert!(cfg_text.contains("\"version\": \"0.1.0\""));
        assert!(!cfg_text.contains("\"deps\""), "deps must drop on publish");

        // Schema: at least the manifest is present.
        let schema_paths: Vec<_> = entries
            .schema_files
            .iter()
            .map(|s| s.archive_path.as_str())
            .collect();
        assert!(schema_paths.contains(&".memstead/schema/schema.yaml"));

        // Entities: both markdown files made it in.
        let md_paths: Vec<_> = entries
            .markdown_files
            .iter()
            .map(|m| m.path.as_str())
            .collect();
        assert!(md_paths.contains(&"first.md"));
        assert!(md_paths.contains(&"second.md"));
    }

    #[test]
    fn assemble_archive_rejects_workspace_without_version() {
        let tmp = TempDir::new().unwrap();
        // Skip `version` on the workspace config — `to_published`
        // surfaces `MissingVersion`.
        let root = write_workspace(&tmp, "demo", false);

        let err = assemble_archive(&root).expect_err("missing version must fail");
        assert!(matches!(
            err,
            AssembleError::Config(PublishConversionError::MissingVersion)
        ));
    }

    #[test]
    fn assemble_archive_excludes_engine_internal_dirs() {
        // The walker already skips `.git/` and `.memstead/`; this is the
        // contract test that the publish path inherits that behaviour. A
        // stray markdown file inside the meta dir must NOT land in the
        // archive's markdown set.
        let tmp = TempDir::new().unwrap();
        let root = write_workspace(&tmp, "demo", true);
        std::fs::write(
            root.join(".memstead").join("rogue.md"),
            "---\ntype: spec\n---\n# Rogue\n\n## Identity\n\nNo.\n",
        )
        .unwrap();

        write_spec(&root, "visible", "Visible");

        let bytes = assemble_archive(&root).unwrap();
        let limits = ValidatorLimits::default();
        let entries = extract_entries(&bytes, &limits).unwrap();
        let md_paths: Vec<_> = entries
            .markdown_files
            .iter()
            .map(|m| m.path.as_str())
            .collect();
        assert!(md_paths.contains(&"visible.md"));
        assert!(!md_paths.iter().any(|p| p.contains("rogue")));
    }

    #[test]
    fn assemble_archive_is_deterministic_across_calls() {
        let tmp = TempDir::new().unwrap();
        let root = write_workspace(&tmp, "demo", true);
        for (slug, title) in [("a", "A"), ("b", "B"), ("c", "C")] {
            write_spec(&root, slug, title);
        }

        let bytes1 = assemble_archive(&root).unwrap();
        let bytes2 = assemble_archive(&root).unwrap();
        assert_eq!(
            bytes1, bytes2,
            "two assemble calls on the same workspace must yield byte-identical archives"
        );
    }
}
