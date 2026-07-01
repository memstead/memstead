//! `SchemaSource` — a storage backend's schema-storage location.
//!
//! Each open storage backend owns a place its authored schema packages
//! live: the folder backend's `<workspace>/.memstead/schemas/`, the
//! git-branch backend's `__MEMSTEAD:schemas/` ref, an archive's sealed
//! `schemas/` directory inside the zip. A `SchemaSource` abstracts read
//! (and, for open backends, write) of that location behind one trait, so
//! the resolution layer ([`crate::engine::SchemaResolver`]) and the
//! authoring layer (`memstead schema install`) work against a uniform
//! surface regardless of where "local storage" physically is.
//!
//! The folder source lives here in `memstead-base`. The git-branch
//! source implements this trait in `memstead-git-branch` (where the
//! `gix` read/write of the `__MEMSTEAD` ref lives); the archive source
//! is read-only (sealed).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use memstead_schema::Schema;

/// Failure reading or writing a schema source.
#[derive(Debug, thiserror::Error)]
pub enum SchemaSourceError {
    /// Reading or parsing the source's schema packages failed.
    #[error("schema source read failed: {0}")]
    Read(String),
    /// Writing a schema package into the source failed.
    #[error("schema source write failed: {0}")]
    Write(String),
    /// The source is sealed/read-only and cannot accept a write (archive).
    #[error("schema source is read-only: {0}")]
    ReadOnly(&'static str),
}

/// A storage backend's schema-storage location.
///
/// `read_schemas` returns every schema package the source carries,
/// parsed. `write_schema` installs an authored package; read-only
/// sources (archive) return [`SchemaSourceError::ReadOnly`]. The engine
/// consults sources in a fixed order — local storage (a backend's own
/// `SchemaSource`), built-in, remote (reserved) — see
/// [`crate::engine::SchemaResolver`] for the resolution side.
pub trait SchemaSource {
    /// Every schema package this source carries, parsed.
    fn read_schemas(&self) -> Result<Vec<Arc<Schema>>, SchemaSourceError>;

    /// Write a schema package — `(relative-path, bytes)` pairs such as
    /// `("schema.yaml", …)`, `("types/<t>.yaml", …)`,
    /// `("mem-template.json", …)` — into the source's storage.
    /// Read-only sources return [`SchemaSourceError::ReadOnly`].
    fn write_schema(
        &self,
        name: &str,
        version: &str,
        files: &[(String, Vec<u8>)],
    ) -> Result<(), SchemaSourceError>;
}

/// The folder backend's schema source: schemas live under
/// `<workspace>/.memstead/schemas/<name>@<version>/`.
pub struct FolderSchemaSource {
    /// The `<workspace>/.memstead/schemas` directory.
    schemas_dir: PathBuf,
}

impl FolderSchemaSource {
    /// Build a folder source rooted at a workspace.
    pub fn for_workspace(workspace_root: &Path) -> Self {
        Self {
            schemas_dir: workspace_root.join(".memstead").join("schemas"),
        }
    }

    /// The `.memstead/schemas` directory this source reads and writes.
    pub fn schemas_dir(&self) -> &Path {
        &self.schemas_dir
    }
}

impl SchemaSource for FolderSchemaSource {
    fn read_schemas(&self) -> Result<Vec<Arc<Schema>>, SchemaSourceError> {
        // Same walker the boot path uses — absent dir resolves to empty.
        crate::engine::boot::load_workspace_schemas(Some(&self.schemas_dir))
            .map_err(|e| SchemaSourceError::Read(e.to_string()))
    }

    fn write_schema(
        &self,
        name: &str,
        version: &str,
        files: &[(String, Vec<u8>)],
    ) -> Result<(), SchemaSourceError> {
        let pkg_dir = self.schemas_dir.join(format!("{name}@{version}"));
        for (rel, bytes) in files {
            let dest = pkg_dir.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    SchemaSourceError::Write(format!("create {}: {e}", parent.display()))
                })?;
            }
            std::fs::write(&dest, bytes)
                .map_err(|e| SchemaSourceError::Write(format!("write {}: {e}", dest.display())))?;
        }
        Ok(())
    }
}

/// The archive backend's **read-only** schema source — schemas are
/// embedded in a sealed `.mem` archive at `.memstead/schema/`. The
/// archive is frozen at seal time, so `write_schema` always refuses.
pub struct ArchiveSchemaSource {
    bytes: Vec<u8>,
}

impl ArchiveSchemaSource {
    /// From the raw `.mem` archive bytes.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    /// Read the `.mem` archive at `path`.
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            bytes: std::fs::read(path)?,
        })
    }
}

impl SchemaSource for ArchiveSchemaSource {
    fn read_schemas(&self) -> Result<Vec<Arc<Schema>>, SchemaSourceError> {
        let entries = crate::validator::archive::extract_entries(
            &self.bytes,
            &crate::validator::ValidatorLimits::default(),
        )
        .map_err(|e| SchemaSourceError::Read(e.to_string()))?;
        crate::engine::archive::load_embedded_schemas(&entries.schema_files)
            .map_err(|e| SchemaSourceError::Read(e.to_string()))
    }

    fn write_schema(
        &self,
        _name: &str,
        _version: &str,
        _files: &[(String, Vec<u8>)],
    ) -> Result<(), SchemaSourceError> {
        Err(SchemaSourceError::ReadOnly(
            "archive backend is sealed — schemas are embedded at seal time and cannot be written",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn folder_source_round_trips_a_written_package() {
        let tmp = TempDir::new().unwrap();
        let source = FolderSchemaSource::for_workspace(tmp.path());

        // Empty workspace → no schemas.
        assert!(source.read_schemas().unwrap().is_empty());

        // Write a minimal valid package, then read it back.
        let manifest = br#"name: srctest
version: 0.1.0
description: A folder SchemaSource round-trip fixture.
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        let doc_type = br#"name: doc
description: t
when_to_use: here
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: _default
propagating_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;
        source
            .write_schema(
                "srctest",
                "0.1.0",
                &[
                    ("schema.yaml".to_string(), manifest.to_vec()),
                    ("types/doc.yaml".to_string(), doc_type.to_vec()),
                ],
            )
            .unwrap();

        let schemas = source.read_schemas().unwrap();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].manifest.name, "srctest");
    }

    const TEST_MANIFEST: &[u8] = br#"name: archsrc
version: 0.1.0
description: An archive-embedded schema fixture.
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    const TEST_DOC: &[u8] = br#"name: doc
description: t
when_to_use: here
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: _default
propagating_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;

    #[test]
    fn archive_source_reads_embedded_schema_and_refuses_writes() {
        use std::io::Write;

        // Build a minimal `.mem` carrying a config + an embedded schema.
        let mut bytes = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file(".memstead/config.json", opts).unwrap();
            zw.write_all(br#"{"schema":"archsrc@0.1.0"}"#).unwrap();
            zw.start_file(".memstead/schema/schema.yaml", opts).unwrap();
            zw.write_all(TEST_MANIFEST).unwrap();
            zw.start_file(".memstead/schema/types/doc.yaml", opts).unwrap();
            zw.write_all(TEST_DOC).unwrap();
            zw.finish().unwrap();
        }

        let source = ArchiveSchemaSource::from_bytes(bytes);
        let schemas = source.read_schemas().unwrap();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].manifest.name, "archsrc");

        // Sealed — writes always refuse.
        let err = source
            .write_schema("x", "0.1.0", &[("schema.yaml".to_string(), b"x".to_vec())])
            .unwrap_err();
        assert!(matches!(err, SchemaSourceError::ReadOnly(_)), "got {err:?}");
    }
}
