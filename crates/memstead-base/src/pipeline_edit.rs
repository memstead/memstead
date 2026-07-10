//! Edit operations over the four-primitive pipeline store, with referential
//! integrity.
//!
//! Sits above the dumb file ops in [`crate::pipeline_store`]: each function
//! loads the current store, enforces the Medium ← Facet ← Projection ← Ingest
//! reference model (no clobber, no dangling reference), then writes. The
//! engine's in-memory `pipeline_configs` cache is refreshed by the `Engine`
//! wrapper methods that call these — not here — so these functions stay pure
//! disk ops that unit-test with a bare `TempDir`.
//!
//! Identity is the file stem `(mem, name)`. Mediums and facets additionally
//! carry an embedded `name` field kept equal to the stem (facets reference
//! mediums by name, projections reference facets by name); rename here updates
//! the file location, the embedded field, and every dependent reference
//! together, so the store never holds a self-inconsistent or dangling record.
//!
//! Scope: medium / facet / projection — the editing chain the macOS app's
//! pipeline editor drives. Ingests are read for integrity (a projection can't
//! be deleted while an ingest runs it) and repointed on a projection rename,
//! but interactive ingest editing has no consumer yet and is not exposed here.

use std::path::Path;

use crate::engine::Engine;
use crate::pipeline::{Facet, Ingest, Medium, Projection};
use crate::pipeline_store::{self, PipelineConfigs};
use crate::workspace_store::StoreError;

/// Failure modes of a pipeline edit. Distinct from the entity-centric
/// [`crate::engine::EngineError`] — these describe four-primitive store edits.
/// `key` is the display identity: `"<mem>/<name>"`.
#[derive(Debug, thiserror::Error)]
pub enum PipelineEditError {
    /// The engine was not booted from a workspace root, so there is no
    /// four-primitive store to edit (e.g. an engine built from a bare mount
    /// list in a test or in-memory consumer).
    #[error("engine has no workspace root — pipeline edits require a workspace-backed engine")]
    NoWorkspaceRoot,
    /// The edit landed on disk but its provenance record (the
    /// `__MEMSTEAD` mirror commit carrying the note) could not be
    /// committed. The disk state is live; the audit trail is missing
    /// this event — callers surface it rather than silently dropping
    /// the note.
    #[error("pipeline edit landed, but recording provenance failed: {0}")]
    Provenance(String),
    /// A create targeted a `(mem, name)` that already holds a record.
    #[error("{primitive} '{key}' already exists")]
    AlreadyExists {
        primitive: &'static str,
        key: String,
    },
    /// An update / delete / rename targeted a record that does not exist.
    #[error("{primitive} '{key}' does not exist")]
    NotFound {
        primitive: &'static str,
        key: String,
    },
    /// A delete was refused because other records still reference the target.
    #[error("{primitive} '{key}' is referenced by {referrers:?} — remove or repoint them first")]
    Referenced {
        primitive: &'static str,
        key: String,
        referrers: Vec<String>,
    },
    /// A rename target `(mem, new)` already holds a record.
    #[error("rename target {primitive} '{key}' already exists")]
    RenameTargetExists {
        primitive: &'static str,
        key: String,
    },
    /// A JSON-string edit entry point received a payload that did not
    /// deserialize into the target primitive.
    #[error("invalid {primitive} JSON: {message}")]
    InvalidJson {
        primitive: &'static str,
        message: String,
    },
    /// Underlying store IO / parse failure.
    #[error(transparent)]
    Store(#[from] StoreError),
}

fn key(mem: &str, name: &str) -> String {
    format!("{mem}/{name}")
}

fn medium_exists(c: &PipelineConfigs, mem: &str, name: &str) -> bool {
    c.mediums.iter().any(|r| r.mem == mem && r.name == name)
}

fn facet_exists(c: &PipelineConfigs, mem: &str, name: &str) -> bool {
    c.facets.iter().any(|r| r.mem == mem && r.name == name)
}

fn projection_exists(c: &PipelineConfigs, mem: &str, name: &str) -> bool {
    c.projections.iter().any(|r| r.mem == mem && r.name == name)
}

/// Facet names (same mem) whose `medium` points at `name`.
fn facets_referencing_medium(c: &PipelineConfigs, mem: &str, name: &str) -> Vec<String> {
    c.facets
        .iter()
        .filter(|r| r.mem == mem && r.config.medium == name)
        .map(|r| r.name.clone())
        .collect()
}

/// Projection names (same mem) whose `source_facets` contain `name`.
fn projections_referencing_facet(c: &PipelineConfigs, mem: &str, name: &str) -> Vec<String> {
    c.projections
        .iter()
        .filter(|r| r.mem == mem && r.config.source_facets.iter().any(|f| f == name))
        .map(|r| r.name.clone())
        .collect()
}

/// Ingest names whose `projection` points at `<mem>/<name>`.
fn ingests_referencing_projection(c: &PipelineConfigs, mem: &str, name: &str) -> Vec<String> {
    let target = key(mem, name);
    c.ingests
        .iter()
        .filter(|r| r.config.projection == target)
        .map(|r| r.name.clone())
        .collect()
}

// --- Medium ----------------------------------------------------------------

/// Create a medium. Refuses if `(mem, name)` already holds one.
pub fn add_medium(
    root: &Path,
    mem: &str,
    name: &str,
    medium: &Medium,
) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if medium_exists(&configs, mem, name) {
        return Err(PipelineEditError::AlreadyExists {
            primitive: "medium",
            key: key(mem, name),
        });
    }
    pipeline_store::write_medium(root, mem, name, medium)?;
    Ok(())
}

/// Overwrite an existing medium. Refuses if `(mem, name)` does not exist.
pub fn update_medium(
    root: &Path,
    mem: &str,
    name: &str,
    medium: &Medium,
) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !medium_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "medium",
            key: key(mem, name),
        });
    }
    pipeline_store::write_medium(root, mem, name, medium)?;
    Ok(())
}

/// Delete a medium. Refuses if any facet in the same mem still references it.
pub fn delete_medium(root: &Path, mem: &str, name: &str) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !medium_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "medium",
            key: key(mem, name),
        });
    }
    let referrers = facets_referencing_medium(&configs, mem, name);
    if !referrers.is_empty() {
        return Err(PipelineEditError::Referenced {
            primitive: "medium",
            key: key(mem, name),
            referrers,
        });
    }
    pipeline_store::delete_medium(root, mem, name)?;
    Ok(())
}

/// Rename a medium within its mem, updating its embedded `name` and every
/// dependent facet's `medium` reference. No-op when `old == new`.
pub fn rename_medium(
    root: &Path,
    mem: &str,
    old: &str,
    new: &str,
) -> Result<(), PipelineEditError> {
    if old == new {
        return Ok(());
    }
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    let existing = configs
        .mediums
        .iter()
        .find(|r| r.mem == mem && r.name == old)
        .ok_or_else(|| PipelineEditError::NotFound {
            primitive: "medium",
            key: key(mem, old),
        })?;
    if medium_exists(&configs, mem, new) {
        return Err(PipelineEditError::RenameTargetExists {
            primitive: "medium",
            key: key(mem, new),
        });
    }
    let mut renamed = existing.config.clone();
    renamed.name = new.to_string();
    // Write the new stem first, then repoint referrers, then drop the old —
    // no point in the sequence leaves a facet pointing at a missing medium.
    pipeline_store::write_medium(root, mem, new, &renamed)?;
    for facet in facets_referencing_medium(&configs, mem, old) {
        if let Some(rec) = configs
            .facets
            .iter()
            .find(|r| r.mem == mem && r.name == facet)
        {
            let mut updated = rec.config.clone();
            updated.medium = new.to_string();
            pipeline_store::write_facet(root, mem, &facet, &updated)?;
        }
    }
    pipeline_store::delete_medium(root, mem, old)?;
    Ok(())
}

// --- Facet -----------------------------------------------------------------

/// Create a facet. Refuses if `(mem, name)` already holds one.
pub fn add_facet(
    root: &Path,
    mem: &str,
    name: &str,
    facet: &Facet,
) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if facet_exists(&configs, mem, name) {
        return Err(PipelineEditError::AlreadyExists {
            primitive: "facet",
            key: key(mem, name),
        });
    }
    pipeline_store::write_facet(root, mem, name, facet)?;
    Ok(())
}

/// Overwrite an existing facet. Refuses if `(mem, name)` does not exist.
pub fn update_facet(
    root: &Path,
    mem: &str,
    name: &str,
    facet: &Facet,
) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !facet_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "facet",
            key: key(mem, name),
        });
    }
    pipeline_store::write_facet(root, mem, name, facet)?;
    Ok(())
}

/// Delete a facet. Refuses if any projection in the same mem references it.
pub fn delete_facet(root: &Path, mem: &str, name: &str) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !facet_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "facet",
            key: key(mem, name),
        });
    }
    let referrers = projections_referencing_facet(&configs, mem, name);
    if !referrers.is_empty() {
        return Err(PipelineEditError::Referenced {
            primitive: "facet",
            key: key(mem, name),
            referrers,
        });
    }
    pipeline_store::delete_facet(root, mem, name)?;
    Ok(())
}

/// Rename a facet within its mem, updating its embedded `name` and every
/// dependent projection's `source_facets` entry. No-op when `old == new`.
pub fn rename_facet(root: &Path, mem: &str, old: &str, new: &str) -> Result<(), PipelineEditError> {
    if old == new {
        return Ok(());
    }
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    let existing = configs
        .facets
        .iter()
        .find(|r| r.mem == mem && r.name == old)
        .ok_or_else(|| PipelineEditError::NotFound {
            primitive: "facet",
            key: key(mem, old),
        })?;
    if facet_exists(&configs, mem, new) {
        return Err(PipelineEditError::RenameTargetExists {
            primitive: "facet",
            key: key(mem, new),
        });
    }
    let mut renamed = existing.config.clone();
    renamed.name = new.to_string();
    pipeline_store::write_facet(root, mem, new, &renamed)?;
    for proj in projections_referencing_facet(&configs, mem, old) {
        if let Some(rec) = configs
            .projections
            .iter()
            .find(|r| r.mem == mem && r.name == proj)
        {
            let mut updated = rec.config.clone();
            for f in updated.source_facets.iter_mut() {
                if f == old {
                    *f = new.to_string();
                }
            }
            pipeline_store::write_projection(root, mem, &proj, &updated)?;
        }
    }
    pipeline_store::delete_facet(root, mem, old)?;
    Ok(())
}

// --- Projection ------------------------------------------------------------

/// Create a projection. Refuses if `(mem, name)` already holds one.
pub fn add_projection(
    root: &Path,
    mem: &str,
    name: &str,
    projection: &Projection,
) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if projection_exists(&configs, mem, name) {
        return Err(PipelineEditError::AlreadyExists {
            primitive: "projection",
            key: key(mem, name),
        });
    }
    pipeline_store::write_projection(root, mem, name, projection)?;
    Ok(())
}

/// Overwrite an existing projection. Refuses if `(mem, name)` does not exist.
pub fn update_projection(
    root: &Path,
    mem: &str,
    name: &str,
    projection: &Projection,
) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !projection_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "projection",
            key: key(mem, name),
        });
    }
    pipeline_store::write_projection(root, mem, name, projection)?;
    Ok(())
}

/// Delete a projection. Refuses if any ingest still runs it.
pub fn delete_projection(root: &Path, mem: &str, name: &str) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !projection_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "projection",
            key: key(mem, name),
        });
    }
    let referrers = ingests_referencing_projection(&configs, mem, name);
    if !referrers.is_empty() {
        return Err(PipelineEditError::Referenced {
            primitive: "projection",
            key: key(mem, name),
            referrers,
        });
    }
    pipeline_store::delete_projection(root, mem, name)?;
    Ok(())
}

/// Rename a projection within its mem (a projection has no embedded name, so
/// the file moves), repointing every ingest whose `projection` was
/// `<mem>/<old>`. No-op when `old == new`.
pub fn rename_projection(
    root: &Path,
    mem: &str,
    old: &str,
    new: &str,
) -> Result<(), PipelineEditError> {
    if old == new {
        return Ok(());
    }
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !projection_exists(&configs, mem, old) {
        return Err(PipelineEditError::NotFound {
            primitive: "projection",
            key: key(mem, old),
        });
    }
    if projection_exists(&configs, mem, new) {
        return Err(PipelineEditError::RenameTargetExists {
            primitive: "projection",
            key: key(mem, new),
        });
    }
    pipeline_store::rename_projection(root, mem, old, new)?;
    let new_ref = key(mem, new);
    for ingest in ingests_referencing_projection(&configs, mem, old) {
        if let Some(rec) = configs.ingests.iter().find(|r| r.name == ingest) {
            let mut updated = rec.config.clone();
            updated.projection = new_ref.clone();
            pipeline_store::write_ingest(root, &ingest, &updated)?;
        }
    }
    Ok(())
}

// --- Ingest ----------------------------------------------------------------
//
// Ingests are *flat* (workspace-level, not mem-scoped) and are the leaf of
// the pipeline — nothing references an ingest — so add/update/delete only
// check the ingest's own existence; there is no referrer gate on delete. An
// ingest's `projection` field points at a `<mem>/<name>` projection key
// (delete_projection enforces the inverse integrity).

fn ingest_exists(c: &PipelineConfigs, name: &str) -> bool {
    c.ingests.iter().any(|r| r.name == name)
}

/// Create an ingest. Refuses if `name` already holds one.
pub fn add_ingest(root: &Path, name: &str, ingest: &Ingest) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if ingest_exists(&configs, name) {
        return Err(PipelineEditError::AlreadyExists {
            primitive: "ingest",
            key: name.to_string(),
        });
    }
    pipeline_store::write_ingest(root, name, ingest)?;
    Ok(())
}

/// Overwrite an existing ingest. Refuses if `name` does not exist.
pub fn update_ingest(root: &Path, name: &str, ingest: &Ingest) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !ingest_exists(&configs, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "ingest",
            key: name.to_string(),
        });
    }
    pipeline_store::write_ingest(root, name, ingest)?;
    Ok(())
}

/// Delete an ingest. Nothing references an ingest, so no referrer gate.
pub fn delete_ingest(root: &Path, name: &str) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !ingest_exists(&configs, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "ingest",
            key: name.to_string(),
        });
    }
    pipeline_store::delete_ingest(root, name)?;
    Ok(())
}

/// Rename an ingest (file-stem identity; nothing depends on it). No-op when
/// `old == new`.
pub fn rename_ingest(root: &Path, old: &str, new: &str) -> Result<(), PipelineEditError> {
    if old == new {
        return Ok(());
    }
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !ingest_exists(&configs, old) {
        return Err(PipelineEditError::NotFound {
            primitive: "ingest",
            key: old.to_string(),
        });
    }
    if ingest_exists(&configs, new) {
        return Err(PipelineEditError::RenameTargetExists {
            primitive: "ingest",
            key: new.to_string(),
        });
    }
    pipeline_store::rename_ingest(root, old, new)?;
    Ok(())
}

// --- Engine surface --------------------------------------------------------
//
// Thin wrappers that route an edit through the free functions above (disk +
// referential integrity) and then refresh the engine's in-memory
// `pipeline_configs` snapshot so a subsequent `pipeline_configs()` read sees
// the change. They use only the engine's public accessors, so this block stays
// out of the `engine` module's internals.

impl Engine {
    fn pipeline_edit_root(&self) -> Result<std::path::PathBuf, PipelineEditError> {
        self.workspace_root()
            .map(Path::to_path_buf)
            .ok_or(PipelineEditError::NoWorkspaceRoot)
    }

    fn refresh_pipeline_configs(&mut self, root: &Path) -> Result<(), PipelineEditError> {
        // The referential-integrity edit layer operates on the four-primitive
        // (legacy) shape — the version-gated binding loader is the live brief
        // path's, not the editor's.
        self.set_pipeline_configs(pipeline_store::load_legacy_pipeline_configs(root)?);
        Ok(())
    }

    /// Provenance bridge with error translation: the disk write has
    /// already landed when this runs, so a failure here is surfaced as
    /// `Provenance` (edit live, audit record missing) rather than
    /// rolling anything back.
    fn pipeline_provenance(
        &self,
        mem: &str,
        kind: &str,
        edits: &[(String, Option<Vec<u8>>)],
        note: Option<&str>,
        verb: &str,
    ) -> Result<(), PipelineEditError> {
        self.record_pipeline_edit_provenance(mem, kind, edits, note, verb)
            .map_err(|e| PipelineEditError::Provenance(e.to_string()))
    }

    /// Create a medium and refresh the in-memory snapshot. See [`add_medium`].
    pub fn add_medium(
        &mut self,
        mem: &str,
        name: &str,
        medium: &Medium,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        add_medium(&root, mem, name, medium)?;
        let bytes =
            serde_json::to_vec_pretty(medium).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "mediums",
            &[(name.to_string(), Some(bytes))],
            note,
            "add",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Overwrite a medium and refresh the snapshot. See [`update_medium`].
    pub fn update_medium(
        &mut self,
        mem: &str,
        name: &str,
        medium: &Medium,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        update_medium(&root, mem, name, medium)?;
        let bytes =
            serde_json::to_vec_pretty(medium).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "mediums",
            &[(name.to_string(), Some(bytes))],
            note,
            "update",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Delete a medium and refresh the snapshot. See [`delete_medium`].
    pub fn delete_medium(
        &mut self,
        mem: &str,
        name: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        delete_medium(&root, mem, name)?;
        self.pipeline_provenance(mem, "mediums", &[(name.to_string(), None)], note, "delete")?;
        self.refresh_pipeline_configs(&root)
    }

    /// Rename a medium and refresh the snapshot. See [`rename_medium`].
    pub fn rename_medium(
        &mut self,
        mem: &str,
        old: &str,
        new: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        rename_medium(&root, mem, old, new)?;
        // Mirror the rename as remove-old + upsert-new in one commit.
        // The in-memory snapshot still holds the record under its old
        // name here (refresh runs after), and a rename changes the
        // name only — the config bytes are those of the old record.
        let bytes = self
            .pipeline_configs()
            .mediums
            .iter()
            .find(|r| r.mem == mem && r.name == old)
            .map(|r| serde_json::to_vec_pretty(&r.config))
            .transpose()
            .map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "mediums",
            &[(old.to_string(), None), (new.to_string(), bytes)],
            note,
            "rename",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Create a facet and refresh the snapshot. See [`add_facet`].
    pub fn add_facet(
        &mut self,
        mem: &str,
        name: &str,
        facet: &Facet,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        add_facet(&root, mem, name, facet)?;
        let bytes =
            serde_json::to_vec_pretty(facet).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "facets",
            &[(name.to_string(), Some(bytes))],
            note,
            "add",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Overwrite a facet and refresh the snapshot. See [`update_facet`].
    pub fn update_facet(
        &mut self,
        mem: &str,
        name: &str,
        facet: &Facet,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        update_facet(&root, mem, name, facet)?;
        let bytes =
            serde_json::to_vec_pretty(facet).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "facets",
            &[(name.to_string(), Some(bytes))],
            note,
            "update",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Delete a facet and refresh the snapshot. See [`delete_facet`].
    pub fn delete_facet(
        &mut self,
        mem: &str,
        name: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        delete_facet(&root, mem, name)?;
        self.pipeline_provenance(mem, "facets", &[(name.to_string(), None)], note, "delete")?;
        self.refresh_pipeline_configs(&root)
    }

    /// Rename a facet and refresh the snapshot. See [`rename_facet`].
    pub fn rename_facet(
        &mut self,
        mem: &str,
        old: &str,
        new: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        rename_facet(&root, mem, old, new)?;
        // Mirror the rename as remove-old + upsert-new in one commit.
        // The in-memory snapshot still holds the record under its old
        // name here (refresh runs after), and a rename changes the
        // name only — the config bytes are those of the old record.
        let bytes = self
            .pipeline_configs()
            .facets
            .iter()
            .find(|r| r.mem == mem && r.name == old)
            .map(|r| serde_json::to_vec_pretty(&r.config))
            .transpose()
            .map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "facets",
            &[(old.to_string(), None), (new.to_string(), bytes)],
            note,
            "rename",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Create a projection and refresh the snapshot. See [`add_projection`].
    pub fn add_projection(
        &mut self,
        mem: &str,
        name: &str,
        projection: &Projection,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        add_projection(&root, mem, name, projection)?;
        let bytes =
            serde_json::to_vec_pretty(projection).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "projections",
            &[(name.to_string(), Some(bytes))],
            note,
            "add",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Overwrite a projection and refresh the snapshot. See [`update_projection`].
    pub fn update_projection(
        &mut self,
        mem: &str,
        name: &str,
        projection: &Projection,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        update_projection(&root, mem, name, projection)?;
        let bytes =
            serde_json::to_vec_pretty(projection).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "projections",
            &[(name.to_string(), Some(bytes))],
            note,
            "update",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Delete a projection and refresh the snapshot. See [`delete_projection`].
    pub fn delete_projection(
        &mut self,
        mem: &str,
        name: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        delete_projection(&root, mem, name)?;
        self.pipeline_provenance(
            mem,
            "projections",
            &[(name.to_string(), None)],
            note,
            "delete",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Rename a projection and refresh the snapshot. See [`rename_projection`].
    pub fn rename_projection(
        &mut self,
        mem: &str,
        old: &str,
        new: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        rename_projection(&root, mem, old, new)?;
        // Mirror the rename as remove-old + upsert-new in one commit.
        // The in-memory snapshot still holds the record under its old
        // name here (refresh runs after), and a rename changes the
        // name only — the config bytes are those of the old record.
        let bytes = self
            .pipeline_configs()
            .projections
            .iter()
            .find(|r| r.mem == mem && r.name == old)
            .map(|r| serde_json::to_vec_pretty(&r.config))
            .transpose()
            .map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "projections",
            &[(old.to_string(), None), (new.to_string(), bytes)],
            note,
            "rename",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Destination mem of an ingest: the `<mem>` half of its
    /// `projection: "<mem>/<name>"` pointer. Ingests are
    /// workspace-level; their provenance records against the mem the
    /// runs would land in.
    fn ingest_destination_mem(projection: &str) -> String {
        projection
            .split('/')
            .next()
            .unwrap_or(projection)
            .to_string()
    }

    /// Create an ingest and refresh the snapshot. See [`add_ingest`].
    pub fn add_ingest(
        &mut self,
        name: &str,
        ingest: &Ingest,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        add_ingest(&root, name, ingest)?;
        let bytes =
            serde_json::to_vec_pretty(ingest).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        let mem = Self::ingest_destination_mem(&ingest.projection);
        self.pipeline_provenance(
            &mem,
            "ingests",
            &[(name.to_string(), Some(bytes))],
            note,
            "add",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Overwrite an ingest and refresh the snapshot. See [`update_ingest`].
    pub fn update_ingest(
        &mut self,
        name: &str,
        ingest: &Ingest,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        update_ingest(&root, name, ingest)?;
        let bytes =
            serde_json::to_vec_pretty(ingest).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        let mem = Self::ingest_destination_mem(&ingest.projection);
        self.pipeline_provenance(
            &mem,
            "ingests",
            &[(name.to_string(), Some(bytes))],
            note,
            "update",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Delete an ingest and refresh the snapshot. See [`delete_ingest`].
    pub fn delete_ingest(
        &mut self,
        name: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        // Snapshot still holds the record — resolve the destination mem
        // before the delete lands.
        let mem = self
            .pipeline_configs()
            .ingests
            .iter()
            .find(|r| r.name == name)
            .map(|r| Self::ingest_destination_mem(&r.config.projection));
        delete_ingest(&root, name)?;
        if let Some(mem) = mem {
            self.pipeline_provenance(&mem, "ingests", &[(name.to_string(), None)], note, "delete")?;
        }
        self.refresh_pipeline_configs(&root)
    }

    /// Rename an ingest and refresh the snapshot. See [`rename_ingest`].
    pub fn rename_ingest(
        &mut self,
        old: &str,
        new: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        rename_ingest(&root, old, new)?;
        // Pre-refresh snapshot: the record still lives under its old name.
        let record = self
            .pipeline_configs()
            .ingests
            .iter()
            .find(|r| r.name == old);
        let mem = record.map(|r| Self::ingest_destination_mem(&r.config.projection));
        let bytes = record
            .map(|r| serde_json::to_vec_pretty(&r.config))
            .transpose()
            .map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        if let Some(mem) = mem {
            self.pipeline_provenance(
                &mem,
                "ingests",
                &[(old.to_string(), None), (new.to_string(), bytes)],
                note,
                "rename",
            )?;
        }
        self.refresh_pipeline_configs(&root)
    }

    // JSON-string entry points for serialization-boundary callers (UniFFI,
    // CLI) that carry a primitive as JSON rather than a typed value. They
    // deserialize here — where serde already lives — and delegate to the
    // typed methods above, so the FFI translation layer needs no JSON
    // dependency of its own. Only `add`/`update` carry a payload; `delete`
    // and `rename` take plain string identifiers and use the typed methods
    // directly.

    /// [`Self::add_medium`] from a JSON-encoded [`Medium`].
    pub fn add_medium_json(
        &mut self,
        mem: &str,
        name: &str,
        medium_json: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        self.add_medium(mem, name, &parse_json(medium_json, "medium")?, note)
    }

    /// [`Self::update_medium`] from a JSON-encoded [`Medium`].
    pub fn update_medium_json(
        &mut self,
        mem: &str,
        name: &str,
        medium_json: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        self.update_medium(mem, name, &parse_json(medium_json, "medium")?, note)
    }

    /// [`Self::add_facet`] from a JSON-encoded [`Facet`].
    pub fn add_facet_json(
        &mut self,
        mem: &str,
        name: &str,
        facet_json: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        self.add_facet(mem, name, &parse_json(facet_json, "facet")?, note)
    }

    /// [`Self::update_facet`] from a JSON-encoded [`Facet`].
    pub fn update_facet_json(
        &mut self,
        mem: &str,
        name: &str,
        facet_json: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        self.update_facet(mem, name, &parse_json(facet_json, "facet")?, note)
    }

    /// [`Self::add_projection`] from a JSON-encoded [`Projection`].
    pub fn add_projection_json(
        &mut self,
        mem: &str,
        name: &str,
        projection_json: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        self.add_projection(mem, name, &parse_json(projection_json, "projection")?, note)
    }

    /// [`Self::update_projection`] from a JSON-encoded [`Projection`].
    pub fn update_projection_json(
        &mut self,
        mem: &str,
        name: &str,
        projection_json: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        self.update_projection(mem, name, &parse_json(projection_json, "projection")?, note)
    }

    /// [`Self::add_ingest`] from a JSON-encoded [`Ingest`].
    pub fn add_ingest_json(
        &mut self,
        name: &str,
        ingest_json: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        self.add_ingest(name, &parse_json(ingest_json, "ingest")?, note)
    }

    /// [`Self::update_ingest`] from a JSON-encoded [`Ingest`].
    pub fn update_ingest_json(
        &mut self,
        name: &str,
        ingest_json: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        self.update_ingest(name, &parse_json(ingest_json, "ingest")?, note)
    }
}

/// Deserialize a pipeline primitive from JSON, mapping a parse failure to a
/// typed [`PipelineEditError::InvalidJson`] naming the primitive.
fn parse_json<T: serde::de::DeserializeOwned>(
    json: &str,
    primitive: &'static str,
) -> Result<T, PipelineEditError> {
    serde_json::from_str(json).map_err(|e| PipelineEditError::InvalidJson {
        primitive,
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{
        Ingest, IngestMode, IngestTrigger, MediumType, PatternEntry, PatternMode,
    };
    use tempfile::TempDir;

    fn medium(name: &str) -> Medium {
        Medium {
            name: name.to_string(),
            medium_type: MediumType::Codebase,
            pointer: "../src".to_string(),
            change_detection: None,
        }
    }

    fn facet(name: &str, medium: &str) -> Facet {
        Facet {
            name: name.to_string(),
            medium: medium.to_string(),
            scope: vec![PatternEntry {
                path: "**/*.rs".to_string(),
                mode: PatternMode::Allow,
            }],
            engagement: None,
            preparation: None,
        }
    }

    fn projection(facets: &[&str]) -> Projection {
        Projection {
            intent: Some("test".to_string()),
            source_facets: facets.iter().map(|s| s.to_string()).collect(),
            reference_mems: vec![],
            destination_mem: "v".to_string(),
            rules: None,
        }
    }

    fn ingest(projection: &str) -> Ingest {
        Ingest {
            projection: projection.to_string(),
            mode: IngestMode::Discovery,
            trigger: IngestTrigger::Loop,
            batch_size: 10,
            deny_paths: vec![],
            post_actions: None,
        }
    }

    #[test]
    fn add_then_duplicate_medium_refuses() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_medium(root, "v", "m", &medium("m")).unwrap();
        let err = add_medium(root, "v", "m", &medium("m")).unwrap_err();
        assert!(
            matches!(err, PipelineEditError::AlreadyExists { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn update_missing_medium_refuses() {
        let tmp = TempDir::new().unwrap();
        let err = update_medium(tmp.path(), "v", "m", &medium("m")).unwrap_err();
        assert!(
            matches!(err, PipelineEditError::NotFound { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn delete_medium_refused_while_a_facet_references_it() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_medium(root, "v", "m", &medium("m")).unwrap();
        add_facet(root, "v", "f", &facet("f", "m")).unwrap();

        let err = delete_medium(root, "v", "m").unwrap_err();
        match err {
            PipelineEditError::Referenced { referrers, .. } => assert_eq!(referrers, vec!["f"]),
            other => panic!("expected Referenced, got {other:?}"),
        }
        // Removing the facet frees the medium.
        delete_facet(root, "v", "f").unwrap();
        delete_medium(root, "v", "m").unwrap();
        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert!(configs.mediums.is_empty() && configs.facets.is_empty());
    }

    #[test]
    fn rename_medium_repoints_dependent_facets_and_updates_embedded_name() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_medium(root, "v", "old", &medium("old")).unwrap();
        add_facet(root, "v", "f", &facet("f", "old")).unwrap();

        rename_medium(root, "v", "old", "new").unwrap();

        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert_eq!(configs.mediums.len(), 1);
        assert_eq!(configs.mediums[0].name, "new");
        // Embedded name tracks the stem.
        assert_eq!(configs.mediums[0].config.name, "new");
        // Dependent facet now points at the new medium name.
        assert_eq!(configs.facets[0].config.medium, "new");
    }

    #[test]
    fn rename_medium_refuses_existing_target() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_medium(root, "v", "a", &medium("a")).unwrap();
        add_medium(root, "v", "b", &medium("b")).unwrap();
        let err = rename_medium(root, "v", "a", "b").unwrap_err();
        assert!(
            matches!(err, PipelineEditError::RenameTargetExists { .. }),
            "got {err:?}"
        );
        // Nothing lost.
        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert_eq!(configs.mediums.len(), 2);
    }

    #[test]
    fn rename_medium_to_same_name_is_a_noop() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_medium(root, "v", "m", &medium("m")).unwrap();
        rename_medium(root, "v", "m", "m").unwrap();
        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert_eq!(configs.mediums.len(), 1);
        assert_eq!(configs.mediums[0].config, medium("m"));
    }

    #[test]
    fn delete_facet_refused_while_a_projection_references_it() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_facet(root, "v", "f", &facet("f", "m")).unwrap();
        add_projection(root, "v", "p", &projection(&["f"])).unwrap();
        let err = delete_facet(root, "v", "f").unwrap_err();
        assert!(
            matches!(err, PipelineEditError::Referenced { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn rename_facet_repoints_dependent_projections() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_facet(root, "v", "old", &facet("old", "m")).unwrap();
        add_projection(root, "v", "p", &projection(&["old", "other"])).unwrap();

        rename_facet(root, "v", "old", "new").unwrap();

        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert_eq!(configs.facets[0].name, "new");
        assert_eq!(configs.facets[0].config.name, "new");
        assert_eq!(
            configs.projections[0].config.source_facets,
            vec!["new", "other"]
        );
    }

    #[test]
    fn delete_projection_refused_while_an_ingest_runs_it() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_projection(root, "v", "p", &projection(&[])).unwrap();
        pipeline_store::write_ingest(root, "i", &ingest("v/p")).unwrap();
        let err = delete_projection(root, "v", "p").unwrap_err();
        match err {
            PipelineEditError::Referenced { referrers, .. } => assert_eq!(referrers, vec!["i"]),
            other => panic!("expected Referenced, got {other:?}"),
        }
    }

    #[test]
    fn parse_json_accepts_a_valid_medium() {
        let m: Medium =
            parse_json(r#"{"name":"m","type":"codebase","pointer":".."}"#, "medium").unwrap();
        assert_eq!(m.name, "m");
        assert_eq!(m.medium_type, MediumType::Codebase);
    }

    #[test]
    fn parse_json_maps_a_bad_payload_to_invalid_json() {
        let err = parse_json::<Medium>("{ not json", "medium").unwrap_err();
        assert!(
            matches!(
                err,
                PipelineEditError::InvalidJson {
                    primitive: "medium",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn rename_projection_repoints_dependent_ingests() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_projection(root, "v", "old", &projection(&[])).unwrap();
        pipeline_store::write_ingest(root, "i", &ingest("v/old")).unwrap();

        rename_projection(root, "v", "old", "new").unwrap();

        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert_eq!(configs.projections[0].name, "new");
        assert_eq!(configs.ingests[0].config.projection, "v/new");
    }

    #[test]
    fn add_then_duplicate_ingest_refuses() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_ingest(root, "i", &ingest("v/p")).unwrap();
        let err = add_ingest(root, "i", &ingest("v/p")).unwrap_err();
        assert!(
            matches!(
                err,
                PipelineEditError::AlreadyExists {
                    primitive: "ingest",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn update_missing_ingest_refuses() {
        let tmp = TempDir::new().unwrap();
        let err = update_ingest(tmp.path(), "i", &ingest("v/p")).unwrap_err();
        assert!(
            matches!(
                err,
                PipelineEditError::NotFound {
                    primitive: "ingest",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn update_ingest_overwrites_and_delete_removes() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_ingest(root, "i", &ingest("v/p")).unwrap();
        let mut changed = ingest("v/p");
        changed.batch_size = 99;
        update_ingest(root, "i", &changed).unwrap();
        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert_eq!(configs.ingests[0].config.batch_size, 99);

        // Nothing references an ingest — delete needs no referrer gate.
        delete_ingest(root, "i").unwrap();
        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert!(configs.ingests.is_empty());
    }

    #[test]
    fn rename_ingest_moves_the_record() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_ingest(root, "old", &ingest("v/p")).unwrap();
        rename_ingest(root, "old", "new").unwrap();
        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert_eq!(configs.ingests.len(), 1);
        assert_eq!(configs.ingests[0].name, "new");
        // Existing target refuses.
        add_ingest(root, "other", &ingest("v/p")).unwrap();
        let err = rename_ingest(root, "other", "new").unwrap_err();
        assert!(
            matches!(
                err,
                PipelineEditError::RenameTargetExists {
                    primitive: "ingest",
                    ..
                }
            ),
            "got {err:?}"
        );
    }
}
