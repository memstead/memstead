//! Unified `__MEMSTEAD` ref — schemas + per-mem configs in one tree.
//!
//! The post-rebuild target collapses today's two registry-class refs
//! (`__SCHEMAS` for YAMLs, `__SYSTEM` for per-mem configs) onto a
//! single `__MEMSTEAD` ref with the layout:
//!
//! - `__MEMSTEAD:schemas/<name>@<version>/schema.yaml`
//! - `__MEMSTEAD:schemas/<name>@<version>/types/<type>.yaml`
//! - `__MEMSTEAD:mems/<mem>/config.json` (and any other per-mem
//!   blobs that today live under `__SYSTEM:<mem>/`)
//!
//! The `repo.json` blob from `__SYSTEM:` does NOT migrate — its
//! `canonical_name` field projects into the mount record in
//! `state/mounts.json` (the workspace store carries that today). Old
//! `__SCHEMAS` and `__SYSTEM` refs are left in place for one release
//! cycle so operators with an older binary can still read the
//! workspace.
//!
//! ## What this module ships
//!
//! - [`migrate_to_memstead_ref`] — read `__SCHEMAS` + `__SYSTEM`, write
//!   the unified `__MEMSTEAD` ref. Idempotent: re-running against a
//!   workspace whose `__MEMSTEAD` tree already matches the projection
//!   produces no new commit.
//! - [`load_schemas_from_memstead_ref`] — additive reader for the new
//!   layout. Returns the same [`crate::mem_repo_schemas::LoadOutcome`]
//!   shape so a future cutover session can drop in the new function
//!   without touching call sites.
//! - [`read_mem_config_from_memstead_ref`] — additive reader for the
//!   per-mem config under the new layout. Returns the same
//!   [`memstead_schema::MemConfig`] shape the legacy `read_config_at_gitdir`
//!   produces.
//!
//! Existing readers (`mem_repo_schemas::load_schemas_from_ref`,
//! `mem_repo_config::read_config_at_gitdir`) are NOT touched by this
//! module. The cutover (replace legacy reads with `__MEMSTEAD` reads + drop
//! the old refs) is deliberately a separate session — landing the
//! migration helpers first lets operators upgrade their workspaces
//! without coupling to the runtime read-path retire.

use std::path::Path;
use std::sync::Arc;

use memstead_schema::{Schema, MemConfig, loader::SchemaLoadError};

use crate::mem_repo_config::{
    RefSpec, MemRepoWriteError, commit_refs_at_gitdir, resolve_full_path_at_gitdir,
};
use crate::mem_repo_schemas::{LoadOutcome, MemRepoSchemasError};
use crate::vcs::{CommitContext, author_identity, format_commit_message};

const COMMITTER_NAME: &str = "engine";
const COMMITTER_EMAIL: &str = "noreply@memstead.io";

/// Errors raised while migrating to or reading from the unified
/// `__MEMSTEAD` ref.
#[derive(Debug, thiserror::Error)]
pub enum MemsteadRefError {
    #[error("could not open mem-repo gitdir: {0}")]
    GixOpen(String),
    #[error("git tree read error: {0}")]
    GitTree(String),
    #[error("git commit error: {0}")]
    GitCommit(String),
    #[error("schema blob {0} is not valid UTF-8: {1}")]
    NotUtf8(String, String),
    #[error("schema '{name}': {source}")]
    Schema {
        name: String,
        #[source]
        source: SchemaLoadError,
    },
    #[error("config json error at {path}: {message}")]
    Config { path: String, message: String },
}

/// Outcome of [`migrate_to_memstead_ref`].
#[derive(Debug, Clone)]
pub struct MemsteadMigrationOutcome {
    /// Hex commit id of the new `__MEMSTEAD` ref tip after migration.
    /// Equal to the prior tip when the migration was a no-op (the
    /// projection matched what `__MEMSTEAD` already carried).
    pub commit_sha: String,
    /// `true` when `__MEMSTEAD` already carried the projection — no new
    /// commit was written.
    pub already_current: bool,
    /// Number of schema entries projected from `__SCHEMAS`.
    pub schemas_migrated: usize,
    /// Number of per-mem entries projected from `__SYSTEM` (one
    /// per top-level entry under `__SYSTEM` minus `repo.json`).
    pub mems_migrated: usize,
}

/// Read `refs/heads/__SCHEMAS` and `refs/heads/__SYSTEM` from
/// `gitdir`, build a unified tree under the layout described in this
/// module's header docs, and write it as `refs/heads/__MEMSTEAD`.
///
/// Idempotent: re-running against a workspace whose `__MEMSTEAD` tree
/// already matches the projection produces no new commit (the
/// outcome carries `already_current: true` and the existing tip's
/// SHA).
///
/// Empty workspaces (no `__SYSTEM`, no `__SCHEMAS`) write an empty
/// tree under `__MEMSTEAD`. The runtime treats an empty `__MEMSTEAD` the
/// same way it treats absent `__SCHEMAS` / `__SYSTEM` — fall through
/// to legacy fallbacks while the cutover lands.
pub fn migrate_to_memstead_ref(gitdir: &Path) -> Result<MemsteadMigrationOutcome, MemsteadRefError> {
    let repo = gix::open(gitdir).map_err(|e| MemsteadRefError::GixOpen(e.to_string()))?;
    // 1. Read __SCHEMAS — for each schema directory, parse the
    //    manifest YAML to extract the version, then plan a write at
    //    `schemas/<name>@<version>/...` using the original subtree's
    //    object id (the entire schema directory copies as a tree
    //    object, byte-identical).
    let mut schema_entries: Vec<(String, gix::ObjectId)> = Vec::new();
    if let Some(reference) =
        repo.try_find_reference("refs/heads/__SCHEMAS")
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?
    {
        let id = reference
            .into_fully_peeled_id()
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
        let commit = id
            .object()
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?
            .try_into_commit()
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
        let tree = commit
            .tree()
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
        for entry_res in tree.iter() {
            let entry = entry_res.map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
            if !matches!(entry.mode().kind(), gix::object::tree::EntryKind::Tree) {
                continue;
            }
            let dir_name = match std::str::from_utf8(entry.filename()) {
                Ok(s) => s.to_string(),
                Err(_) => continue,
            };
            // Parse `<dir_name>/schema.yaml` to extract the version.
            let subtree = repo
                .find_object(entry.oid().to_owned())
                .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?
                .try_into_tree()
                .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
            let manifest_entry = subtree
                .lookup_entry_by_path("schema.yaml")
                .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
            let version = match manifest_entry {
                Some(me) => {
                    let manifest_obj = me
                        .object()
                        .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
                    let yaml = std::str::from_utf8(&manifest_obj.data)
                        .map_err(|e| {
                            MemsteadRefError::NotUtf8(
                                format!("{dir_name}/schema.yaml"),
                                e.to_string(),
                            )
                        })?
                        .to_string();
                    extract_manifest_version(&yaml).unwrap_or_else(|| "0.0.0".to_string())
                }
                None => "0.0.0".to_string(),
            };
            schema_entries.push((
                format!("{dir_name}@{version}"),
                entry.oid().to_owned(),
            ));
        }
    }

    // 2. Read __SYSTEM — copy every top-level entry except
    //    `repo.json` into `mems/<entry-name>/...`. Hierarchical
    //    structures (`<seg>/<seg>/<mem>/config.json`) are
    //    preserved by copying the subtree's object id verbatim.
    let mut mem_entries: Vec<(String, gix::ObjectId, gix::object::tree::EntryKind)> = Vec::new();
    if let Some(reference) =
        repo.try_find_reference("refs/heads/__SYSTEM")
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?
    {
        let id = reference
            .into_fully_peeled_id()
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
        let commit = id
            .object()
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?
            .try_into_commit()
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
        let tree = commit
            .tree()
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
        for entry_res in tree.iter() {
            let entry = entry_res.map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
            let name = match std::str::from_utf8(entry.filename()) {
                Ok(s) => s.to_string(),
                Err(_) => continue,
            };
            if name == "repo.json" {
                // Workspace-level metadata — projects into mounts.json,
                // not into __MEMSTEAD.
                continue;
            }
            mem_entries.push((name, entry.oid().to_owned(), entry.mode().kind()));
        }
    }

    // 3. Build the unified tree. Use the empty tree as the base and
    //    upsert each entry at its target path.
    let mut editor = repo
        .empty_tree()
        .edit()
        .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
    for (path_segment, oid) in &schema_entries {
        editor
            .upsert(
                format!("schemas/{path_segment}").as_str(),
                gix::object::tree::EntryKind::Tree,
                *oid,
            )
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
    }
    for (name, oid, kind) in &mem_entries {
        editor
            .upsert(format!("mems/{name}").as_str(), *kind, *oid)
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
    }
    let new_tree_id = editor
        .write()
        .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?
        .detach();

    // 4. Idempotency check — if __MEMSTEAD already exists with the same
    //    tree id, no commit needed.
    let existing_tip: Option<(gix::ObjectId, gix::ObjectId)> = repo
        .try_find_reference("refs/heads/__MEMSTEAD")
        .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?
        .and_then(|r| {
            let id = r.into_fully_peeled_id().ok()?;
            let commit = id.object().ok()?.try_into_commit().ok()?;
            let tree_id = commit.tree().ok()?.id;
            Some((id.detach(), tree_id))
        });
    if let Some((tip_id, tree_id)) = existing_tip
        && tree_id == new_tree_id
    {
        return Ok(MemsteadMigrationOutcome {
            commit_sha: tip_id.to_hex().to_string(),
            already_current: true,
            schemas_migrated: schema_entries.len(),
            mems_migrated: mem_entries.len(),
        });
    }

    // 5. Commit the new tree. Reuse the engine's deterministic
    //    committer identity (matches every other engine-produced
    //    commit; humans recognise the "engine" author).
    let time = gix::date::Time::now_local_or_utc();
    let signature = gix::actor::Signature {
        name: "engine".into(),
        email: "noreply@memstead.io".into(),
        time,
    };
    let mut buf = gix::date::parse::TimeBuf::default();
    let sig_ref = signature.to_ref(&mut buf);
    let parents: Vec<gix::ObjectId> = match existing_tip {
        Some((tip, _)) => vec![tip],
        None => Vec::new(),
    };
    let commit_id = repo
        .commit_as(
            sig_ref,
            sig_ref,
            "refs/heads/__MEMSTEAD",
            "memstead: storage migration to unified __MEMSTEAD ref",
            new_tree_id,
            parents,
        )
        .map_err(|e| MemsteadRefError::GitCommit(e.to_string()))?;

    Ok(MemsteadMigrationOutcome {
        commit_sha: commit_id.to_hex().to_string(),
        already_current: false,
        schemas_migrated: schema_entries.len(),
        mems_migrated: mem_entries.len(),
    })
}

/// Outcome of [`write_schema_to_memstead_ref`].
#[derive(Debug, Clone)]
pub struct SchemaWriteOutcome {
    /// Hex sha of the resulting `__MEMSTEAD` tip commit.
    pub commit_sha: String,
    /// `true` when the package was already present byte-for-byte, so no
    /// new commit was produced (the existing tip is returned).
    pub already_current: bool,
}

/// Write a schema package onto the unified `__MEMSTEAD` ref under
/// `schemas/<name>@<version>/`, committing on top of the current ref tip
/// (or an empty tree when the ref is absent). `files` are
/// `(relative-path, bytes)` pairs — e.g. `("schema.yaml", …)`,
/// `("types/decision.yaml", …)`, `("mem-template.json", …)`. Existing
/// `schemas/` and `mems/` entries on the ref are preserved (the new
/// package is upserted into the current tree). Idempotent: re-writing
/// identical bytes yields the same tree and produces no commit.
///
/// This is the git-branch backend's authoring write path — the engine
/// owns mem-repo state, so this lib function is invoked through the
/// engine, never by an external consumer directly. Mirrors the
/// committer identity and idempotency check of [`migrate_to_memstead_ref`].
pub fn write_schema_to_memstead_ref(
    gitdir: &Path,
    name: &str,
    version: &str,
    files: &[(String, Vec<u8>)],
) -> Result<SchemaWriteOutcome, MemsteadRefError> {
    let repo = gix::open(gitdir).map_err(|e| MemsteadRefError::GixOpen(e.to_string()))?;

    // Current `__MEMSTEAD` tip (commit id + tree id), if the ref exists.
    let existing_tip: Option<(gix::ObjectId, gix::ObjectId)> = repo
        .try_find_reference("refs/heads/__MEMSTEAD")
        .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?
        .and_then(|r| {
            let id = r.into_fully_peeled_id().ok()?;
            let commit = id.object().ok()?.try_into_commit().ok()?;
            let tree_id = commit.tree().ok()?.id;
            Some((id.detach(), tree_id))
        });

    // Edit from the current tree so existing schemas/mems survive; an
    // empty tree when the ref does not exist yet.
    let base_tree = match existing_tip {
        Some((_, tree_id)) => repo
            .find_object(tree_id)
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?
            .try_into_tree()
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?,
        None => repo.empty_tree(),
    };
    let mut editor = base_tree
        .edit()
        .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
    let prefix = format!("schemas/{name}@{version}");
    for (rel, bytes) in files {
        let blob_id = repo
            .write_blob(bytes)
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?
            .detach();
        editor
            .upsert(
                format!("{prefix}/{rel}").as_str(),
                gix::object::tree::EntryKind::Blob,
                blob_id,
            )
            .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
    }
    let new_tree_id = editor
        .write()
        .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?
        .detach();

    // Idempotency — identical resulting tree, no commit.
    if let Some((tip_id, tree_id)) = existing_tip
        && tree_id == new_tree_id
    {
        return Ok(SchemaWriteOutcome {
            commit_sha: tip_id.to_hex().to_string(),
            already_current: true,
        });
    }

    let time = gix::date::Time::now_local_or_utc();
    let signature = gix::actor::Signature {
        name: "engine".into(),
        email: "noreply@memstead.io".into(),
        time,
    };
    let mut buf = gix::date::parse::TimeBuf::default();
    let sig_ref = signature.to_ref(&mut buf);
    let parents: Vec<gix::ObjectId> = match existing_tip {
        Some((tip, _)) => vec![tip],
        None => Vec::new(),
    };
    let commit_id = repo
        .commit_as(
            sig_ref,
            sig_ref,
            "refs/heads/__MEMSTEAD",
            format!("memstead: install schema {name}@{version}"),
            new_tree_id,
            parents,
        )
        .map_err(|e| MemsteadRefError::GitCommit(e.to_string()))?;

    Ok(SchemaWriteOutcome {
        commit_sha: commit_id.to_hex().to_string(),
        already_current: false,
    })
}

/// Cheap hand-roll YAML scan: find the top-level `version: <value>`
/// line and return the value (stripped of quotes / whitespace). Used
/// only by the migration to label `schemas/<name>@<version>/`; the
/// canonical schema parse pipeline runs against the original blob
/// bytes after migration.
///
/// Returns `None` when no `version:` field is found at the top level.
/// Schemas without a manifest version are migrated under `name@0.0.0`
/// — operators see the placeholder and can re-publish with an
/// explicit version.
fn extract_manifest_version(yaml: &str) -> Option<String> {
    for line in yaml.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("version:") {
            let value = rest
                .trim()
                .trim_matches(|c: char| c == '"' || c == '\'');
            if value.is_empty() {
                return None;
            }
            return Some(value.to_string());
        }
    }
    None
}

/// Read schemas from the unified `__MEMSTEAD:schemas/` tree.
///
/// Mirrors [`crate::mem_repo_schemas::load_schemas_from_ref`]'s
/// `LoadOutcome` shape so a future cutover session can drop in the
/// new function without touching call sites. The shape is the same;
/// the read source is `refs/heads/__MEMSTEAD` instead of
/// `refs/heads/__SCHEMAS`.
pub fn load_schemas_from_memstead_ref(
    workspace_root: &Path,
) -> Result<LoadOutcome, MemRepoSchemasError> {
    let gitdir = workspace_root.join("mem-repo").join(".git");
    if !gitdir.is_dir() {
        return Ok(LoadOutcome::NoMemRepo);
    }
    load_schemas_from_memstead_ref_at_gitdir(&gitdir)
}

/// Gitdir-rooted variant of [`load_schemas_from_memstead_ref`].
pub fn load_schemas_from_memstead_ref_at_gitdir(
    gitdir: &Path,
) -> Result<LoadOutcome, MemRepoSchemasError> {
    let repo =
        gix::open(gitdir).map_err(|e| MemRepoSchemasError::GixOpen(e.to_string()))?;    let memstead_ref = match repo
        .try_find_reference("refs/heads/__MEMSTEAD")
        .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?
    {
        Some(r) => r,
        None => return Ok(LoadOutcome::NoMemRepo),
    };
    let id = memstead_ref
        .into_fully_peeled_id()
        .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?;
    let commit = id
        .object()
        .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?
        .try_into_commit()
        .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?;
    let tree = commit
        .tree()
        .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?;

    let schemas_entry = match tree
        .lookup_entry_by_path("schemas")
        .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?
    {
        Some(e) if e.mode().is_tree() => e,
        _ => return Ok(LoadOutcome::NoSchemas),
    };
    let schemas_tree = schemas_entry
        .object()
        .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?
        .try_into_tree()
        .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?;

    let mut entries: Vec<(String, gix::ObjectId, gix::object::tree::EntryKind)> = Vec::new();
    for entry_res in schemas_tree.iter() {
        let entry = entry_res.map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?;
        let name = match std::str::from_utf8(entry.filename()) {
            Ok(s) => s.to_string(),
            Err(_) => continue,
        };
        entries.push((name, entry.oid().to_owned(), entry.mode().kind()));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out: Vec<Arc<Schema>> = Vec::new();
    for (versioned_name, oid, kind) in entries {
        if !matches!(kind, gix::object::tree::EntryKind::Tree) {
            continue;
        }
        let schema_obj = repo
            .find_object(oid)
            .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?;
        let schema_tree = schema_obj
            .try_into_tree()
            .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?;

        let manifest_yaml =
            read_blob_string_for_schemas(&repo, &schema_tree, "schema.yaml", &versioned_name)?;
        let mut types_yamls: Vec<(String, String)> = Vec::new();
        if let Some(types_entry) = schema_tree
            .lookup_entry_by_path("types")
            .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?
        {
            if types_entry.mode().is_tree() {
                let types_obj = types_entry
                    .object()
                    .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?;
                let types_tree = types_obj
                    .try_into_tree()
                    .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?;
                for entry_res in types_tree.iter() {
                    let entry = entry_res
                        .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?;
                    if !entry.mode().is_blob() {
                        continue;
                    }
                    let filename = match std::str::from_utf8(entry.filename()) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    let stem = match filename.strip_suffix(".yaml") {
                        Some(s) => s.to_string(),
                        None => continue,
                    };
                    let blob = repo
                        .find_object(entry.oid().to_owned())
                        .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?;
                    let bytes = blob.data.clone();
                    let contents = String::from_utf8(bytes).map_err(|e| {
                        MemRepoSchemasError::NotUtf8(
                            format!("{versioned_name}/types/{filename}"),
                            e.to_string(),
                        )
                    })?;
                    types_yamls.push((stem, contents));
                }
                types_yamls.sort_by(|a, b| a.0.cmp(&b.0));
            }
        }

        let schema = memstead_schema::loader::load_schema_from_memory(&manifest_yaml, &types_yamls)
            .map_err(|source| MemRepoSchemasError::Schema {
                name: versioned_name.clone(),
                source,
            })?;
        out.push(Arc::new(schema));
    }

    if out.is_empty() {
        Ok(LoadOutcome::NoSchemas)
    } else {
        Ok(LoadOutcome::Schemas(out))
    }
}

fn read_blob_string_for_schemas(
    _repo: &gix::Repository,
    schema_tree: &gix::Tree<'_>,
    filename: &str,
    schema_name: &str,
) -> Result<String, MemRepoSchemasError> {
    let entry = schema_tree
        .lookup_entry_by_path(filename)
        .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?
        .ok_or_else(|| MemRepoSchemasError::GitTree(format!(
            "schema '{schema_name}': missing {filename} in __MEMSTEAD"
        )))?;
    let object = entry
        .object()
        .map_err(|e| MemRepoSchemasError::GitTree(e.to_string()))?;
    let bytes = object.data.clone();
    String::from_utf8(bytes)
        .map_err(|e| MemRepoSchemasError::NotUtf8(format!("{schema_name}/{filename}"), e.to_string()))
}

/// Read a per-mem config from `__MEMSTEAD:mems/<mem>/config.json`.
///
/// Mirrors [`crate::mem_repo_config::read_config_at_gitdir`]'s
/// return shape so callers can swap signatures without touching the
/// consumer code. Hierarchical mem paths
/// (`mems/<seg>/<seg>/<mem>/config.json`) resolve via the same
/// `resolve_full_path_at_gitdir` walker the legacy reader uses — but
/// rooted under `mems/` rather than the legacy `__SYSTEM:` tree.
/// When the leaf has no matching content branch yet (the
/// `mem_management::create_mem` flow before the per-mem commit
/// lands), the resolver returns `None` and the read falls back to the
/// flat `mems/<leaf>/config.json` layout — same fallback the write
/// side uses, so reads observe the just-written blob in either layout.
pub fn read_mem_config_from_memstead_ref(
    gitdir: &Path,
    mem_name: &str,
) -> Result<MemConfig, MemsteadRefError> {
    // Resolve the leaf to its full hierarchical tree path. Mirrors
    // commit_config_to_memstead_at_gitdir on the write side — the write
    // and read paths use identical resolution so a hierarchical-leaf
    // read of just-written content lands at the same tree position.
    let full_tree_path = match resolve_full_path_at_gitdir(gitdir, mem_name) {
        Ok(Some(p)) => p,
        Ok(None) => mem_name.to_string(),
        Err(e) => {
            return Err(MemsteadRefError::GitTree(e.to_string()));
        }
    };

    let repo = gix::open(gitdir).map_err(|e| MemsteadRefError::GixOpen(e.to_string()))?;    let memstead_ref = repo
        .try_find_reference("refs/heads/__MEMSTEAD")
        .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?
        .ok_or_else(|| {
            MemsteadRefError::Config {
                path: "refs/heads/__MEMSTEAD".to_string(),
                message: "ref not found".to_string(),
            }
        })?;
    let id = memstead_ref
        .into_fully_peeled_id()
        .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
    let commit = id
        .object()
        .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?
        .try_into_commit()
        .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
    let tree = commit
        .tree()
        .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;

    let path = format!("mems/{full_tree_path}/config.json");
    let entry = tree
        .lookup_entry_by_path(&path)
        .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?
        .ok_or_else(|| MemsteadRefError::Config {
            path: path.clone(),
            message: "config not found in __MEMSTEAD tree".to_string(),
        })?;
    let object = entry
        .object()
        .map_err(|e| MemsteadRefError::GitTree(e.to_string()))?;
    let bytes = object.data.clone();
    let raw = String::from_utf8(bytes).map_err(|e| MemsteadRefError::Config {
        path: path.clone(),
        message: format!("not utf-8: {e}"),
    })?;
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| MemsteadRefError::Config {
        path: path.clone(),
        message: format!("invalid json: {e}"),
    })?;
    memstead_schema::parse_mem_config(&value).map_err(|e| MemsteadRefError::Config {
        path,
        message: e.to_string(),
    })
}

/// Commit `mems/<mem_name>/config.json` to
/// `mem-repo-git:refs/heads/__MEMSTEAD`.
///
/// Mirrors [`crate::mem_repo_config::commit_config_at_gitdir`]'s
/// shape (read-modify-write against the snapshot, atomic ref
/// advance via [`commit_refs_at_gitdir`]) but targets the unified
/// `__MEMSTEAD` ref under `mems/<full_tree_path>/config.json` rather
/// than the legacy `__SYSTEM:<full_tree_path>/config.json`.
///
/// Hierarchical paths are resolved with the same
/// `resolve_full_path_at_gitdir` walker the legacy helper uses; for
/// callers writing the very first config blob before any per-mem
/// branch exists (the unified `mem_management::create_mem`
/// flow), the resolver returns `None` and the function falls back
/// to the flat `<mem_name>/config.json` shape.
///
/// When `__MEMSTEAD` does not yet exist (workspace was never migrated),
/// the function creates the ref atomically with a `MustNotExist`
/// precondition — this is race-safe against a sibling writer that
/// might be performing the migration concurrently.
pub fn commit_config_to_memstead_at_gitdir(
    gitdir: &Path,
    mem_name: &str,
    config_bytes: &[u8],
    ctx: &CommitContext<'_>,
    message: &str,
) -> Result<(), MemRepoWriteError> {
    let full_tree_path = match resolve_full_path_at_gitdir(gitdir, mem_name) {
        Ok(Some(p)) => p,
        Ok(None) => mem_name.to_string(),
        Err(crate::mem_repo_config::MemRepoConfigError::GitdirNotFound(p)) => {
            return Err(MemRepoWriteError::GixOpen {
                path: p,
                message: "mem-repo gitdir not found".to_string(),
            });
        }
        Err(e) => {
            return Err(MemRepoWriteError::GitTree(e.to_string()));
        }
    };

    let repo = gix::open(gitdir).map_err(|e| MemRepoWriteError::GixOpen {
        path: gitdir.display().to_string(),
        message: e.to_string(),
    })?;
    // Snapshot the existing __MEMSTEAD tip (or absence thereof) so the
    // ref-edit batch can use the matching precondition.
    let existing_tip: Option<gix::ObjectId> = repo
        .try_find_reference("refs/heads/__MEMSTEAD")
        .map_err(|e| MemRepoWriteError::GitTree(e.to_string()))?
        .map(|r| {
            r.into_fully_peeled_id()
                .map(|id| id.detach())
                .map_err(|e| MemRepoWriteError::GitTree(format!("peel __MEMSTEAD: {e}")))
        })
        .transpose()?;

    // Base tree: existing __MEMSTEAD tree, or the empty tree when the ref
    // does not yet exist. Either way, upsert the per-mem config blob
    // and write the resulting tree.
    let mut editor = match existing_tip {
        Some(tip) => {
            let commit = repo
                .find_object(tip)
                .map_err(|e| MemRepoWriteError::GitTree(format!("read __MEMSTEAD commit: {e}")))?
                .into_commit();
            let tree = commit
                .tree()
                .map_err(|e| MemRepoWriteError::GitTree(format!("peel __MEMSTEAD tree: {e}")))?;
            tree.edit().map_err(|e| {
                MemRepoWriteError::GitTree(format!("editor init for __MEMSTEAD: {e}"))
            })?
        }
        None => repo.empty_tree().edit().map_err(|e| {
            MemRepoWriteError::GitTree(format!("editor init (empty) for __MEMSTEAD: {e}"))
        })?,
    };
    let blob_id = repo
        .write_blob(config_bytes)
        .map_err(|e| MemRepoWriteError::GitTree(format!("write config blob: {e}")))?
        .detach();
    let tree_path = format!("mems/{full_tree_path}/config.json");
    editor
        .upsert(tree_path.as_str(), gix::objs::tree::EntryKind::Blob, blob_id)
        .map_err(|e| {
            MemRepoWriteError::GitTree(format!("tree upsert {tree_path}: {e}"))
        })?;
    let new_tree_id = editor
        .write()
        .map_err(|e| MemRepoWriteError::GitTree(format!("tree write for __MEMSTEAD: {e}")))?
        .detach();

    let time = gix::date::Time::now_local_or_utc();
    let committer_sig = gix::actor::Signature {
        name: COMMITTER_NAME.into(),
        email: COMMITTER_EMAIL.into(),
        time,
    };
    let author_sig = match author_identity(ctx) {
        Some((name, email)) => gix::actor::Signature {
            name: name.into(),
            email: email.into(),
            time,
        },
        None => committer_sig.clone(),
    };

    let full_message = format_commit_message(message, ctx);

    let parents: Vec<gix::ObjectId> = match existing_tip {
        Some(tip) => vec![tip],
        None => Vec::new(),
    };
    let commit = gix::objs::Commit {
        message: full_message.into(),
        tree: new_tree_id,
        author: author_sig,
        committer: committer_sig,
        encoding: None,
        parents: parents.into_iter().collect(),
        extra_headers: Default::default(),
    };
    let new_commit_id = repo
        .write_object(&commit)
        .map_err(|e| {
            MemRepoWriteError::GitTree(format!("write {tree_path} commit: {e}"))
        })?
        .detach();

    let expected = match existing_tip {
        Some(tip) => gix::refs::transaction::PreviousValue::MustExistAndMatch(
            gix::refs::Target::Object(tip),
        ),
        None => gix::refs::transaction::PreviousValue::MustNotExist,
    };
    commit_refs_at_gitdir(
        gitdir,
        &[RefSpec {
            ref_name: "refs/heads/__MEMSTEAD".to_string(),
            new_oid: new_commit_id,
            expected,
            log_message: format!("memstead: commit __MEMSTEAD:{tree_path}"),
        }],
    )?;

    Ok(())
}

/// Drop every git-branch artifact the engine wrote for one mem:
/// the per-mem content branch (`refs/heads/<branch_leaf>`) and the
/// `__MEMSTEAD:mems/<branch_leaf>/config.json` blob (collapsing any
/// empty `mems/…` ancestor directories along the way). Symmetric
/// counterpart to [`commit_config_to_memstead_at_gitdir`] +
/// [`crate::storage::git_tree::GitTreeMemWriter::commit`]'s seed
/// commit pair — `memstead_mem_create` writes those two, this helper
/// undoes them when `memstead_mem_delete delete_files=true`.
///
/// `branch_leaf` is the hierarchical branch name (e.g.
/// `planning/plan-q4-revamp` or the bare leaf for flat layouts).
/// The function does NOT walk the repo to resolve it — the caller
/// (the git-branch backend's [`memstead_base::backend::MemBackend::delete_artifacts`]
/// impl) has it directly in `self.ref_name` minus the `refs/heads/`
/// prefix.
///
/// Idempotent in both halves:
/// - Branch ref delete uses `PreviousValue::Any` so a missing branch
///   (sibling engine pruned it, manual surgery) is not an error.
/// - `__MEMSTEAD` rewrite is skipped entirely when the ref doesn't exist;
///   when it exists but doesn't carry the per-mem entry, the
///   helper still writes a no-op commit only if the tree changed
///   (the editor's `remove` on a missing path is a no-op there).
///
/// Both edits land in a single `edit_references` transaction — either
/// the branch + `__MEMSTEAD` advance both succeed, or the whole call is
/// rejected and no state changed.
pub fn delete_mem_artifacts_at_gitdir(
    gitdir: &Path,
    branch_leaf: &str,
    ctx: &CommitContext<'_>,
) -> Result<(), MemRepoWriteError> {
    use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
    use gix::refs::{FullName, Target};

    let repo = gix::open(gitdir).map_err(|e| MemRepoWriteError::GixOpen {
        path: gitdir.display().to_string(),
        message: e.to_string(),
    })?;
    // ---- Step 1: drop the per-mem entry from __MEMSTEAD (if present) ----
    let existing_memstead_tip: Option<gix::ObjectId> = repo
        .try_find_reference("refs/heads/__MEMSTEAD")
        .map_err(|e| MemRepoWriteError::GitTree(e.to_string()))?
        .map(|r| {
            r.into_fully_peeled_id()
                .map(|id| id.detach())
                .map_err(|e| MemRepoWriteError::GitTree(format!("peel __MEMSTEAD: {e}")))
        })
        .transpose()?;

    let mut new_memstead_commit: Option<gix::ObjectId> = None;
    if let Some(tip) = existing_memstead_tip {
        let commit = repo
            .find_object(tip)
            .map_err(|e| MemRepoWriteError::GitTree(format!("read __MEMSTEAD commit: {e}")))?
            .into_commit();
        let tree = commit
            .tree()
            .map_err(|e| MemRepoWriteError::GitTree(format!("peel __MEMSTEAD tree: {e}")))?;

        // Check whether the entry actually exists — skip the commit
        // entirely when it doesn't (e.g. a parallel run already
        // pruned it). Saves an empty no-op commit.
        let tree_path = format!("mems/{branch_leaf}/config.json");
        let entry_present = tree
            .lookup_entry_by_path(&tree_path)
            .map_err(|e| {
                MemRepoWriteError::GitTree(format!("lookup {tree_path}: {e}"))
            })?
            .is_some();

        if entry_present {
            let mut editor = tree.edit().map_err(|e| {
                MemRepoWriteError::GitTree(format!("editor init for __MEMSTEAD: {e}"))
            })?;
            editor.remove(tree_path.as_str()).map_err(|e| {
                MemRepoWriteError::GitTree(format!("tree remove {tree_path}: {e}"))
            })?;
            // gix's tree writer prunes empty subtrees on write, so
            // removing `mems/<path>/<name>/config.json` collapses
            // the now-empty `<path>/<name>/` and any `<path>/`
            // ancestors automatically. No manual walk needed.
            let new_tree_id = editor.write().map_err(|e| {
                MemRepoWriteError::GitTree(format!("tree write for __MEMSTEAD: {e}"))
            })?.detach();

            let time = gix::date::Time::now_local_or_utc();
            let committer_sig = gix::actor::Signature {
                name: COMMITTER_NAME.into(),
                email: COMMITTER_EMAIL.into(),
                time,
            };
            let author_sig = match author_identity(ctx) {
                Some((name, email)) => gix::actor::Signature {
                    name: name.into(),
                    email: email.into(),
                    time,
                },
                None => committer_sig.clone(),
            };
            let full_message = format_commit_message(
                &format!("memstead: prune __MEMSTEAD:mems/{branch_leaf}/config.json"),
                ctx,
            );
            let commit_obj = gix::objs::Commit {
                message: full_message.into(),
                tree: new_tree_id,
                author: author_sig,
                committer: committer_sig,
                encoding: None,
                parents: vec![tip].into_iter().collect(),
                extra_headers: Default::default(),
            };
            let new_commit_id = repo
                .write_object(&commit_obj)
                .map_err(|e| {
                    MemRepoWriteError::GitTree(format!("write __MEMSTEAD prune commit: {e}"))
                })?
                .detach();
            new_memstead_commit = Some(new_commit_id);
        }
    }

    // ---- Step 2: ref-edit batch (branch delete + __MEMSTEAD advance) ----
    let mut edits: Vec<RefEdit> = Vec::with_capacity(2);

    let branch_ref = format!("refs/heads/{branch_leaf}");
    let branch_full: FullName = branch_ref.as_str().try_into().map_err(|e| {
        MemRepoWriteError::RefTransaction(format!(
            "invalid branch ref {branch_ref:?}: {e}"
        ))
    })?;
    edits.push(RefEdit {
        change: Change::Delete {
            // `Any` keeps the delete idempotent — a missing branch
            // (sibling pruned it, manual surgery) is not an error.
            expected: PreviousValue::Any,
            log: RefLog::AndReference,
        },
        name: branch_full,
        deref: false,
    });

    if let (Some(prior_tip), Some(new_commit)) = (existing_memstead_tip, new_memstead_commit) {
        let memstead_full: FullName = "refs/heads/__MEMSTEAD".try_into().map_err(|e| {
            MemRepoWriteError::RefTransaction(format!("invalid __MEMSTEAD ref: {e}"))
        })?;
        edits.push(RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: false,
                    message: format!(
                        "memstead: prune __MEMSTEAD:mems/{branch_leaf}/config.json"
                    )
                    .as_str()
                    .into(),
                },
                expected: PreviousValue::MustExistAndMatch(Target::Object(prior_tip)),
                new: Target::Object(new_commit),
            },
            name: memstead_full,
            deref: false,
        });
    }

    repo.edit_references(edits)
        .map_err(|e| MemRepoWriteError::RefTransaction(e.to_string()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_repo_dir(tmp: &Path) -> PathBuf {
        let git_dir = tmp.join("mem-repo.git");
        gix::init_bare(&git_dir).unwrap();
        std::fs::canonicalize(&git_dir).unwrap()
    }

    fn actor_for_test() -> gix::actor::Signature {
        gix::actor::Signature {
            name: "test".into(),
            email: "test@example.com".into(),
            time: gix::date::Time {
                seconds: 0,
                offset: 0,
            },
        }
    }

    /// Build a minimal __SCHEMAS commit with one schema directory
    /// `<name>` containing `schema.yaml` (with a `version: <v>` field)
    /// and an optional types subtree of `(filename, body)` pairs.
    fn seed_schemas(gitdir: &Path, schemas: &[(&str, &str, &[(&str, &str)])]) {
        let repo = gix::open(gitdir).unwrap();
        let actor = actor_for_test();
        let mut buf = gix::date::parse::TimeBuf::default();
        let sig_ref = actor.to_ref(&mut buf);
        let mut editor = repo.empty_tree().edit().unwrap();
        for (name, version, types) in schemas {
            let manifest = format!("name: {name}\nversion: \"{version}\"\n");
            let manifest_blob = repo.write_blob(manifest.as_bytes()).unwrap().detach();
            editor
                .upsert(
                    format!("{name}/schema.yaml"),
                    gix::object::tree::EntryKind::Blob,
                    manifest_blob,
                )
                .unwrap();
            for (type_name, type_body) in *types {
                let blob = repo.write_blob(type_body.as_bytes()).unwrap().detach();
                editor
                    .upsert(
                        format!("{name}/types/{type_name}.yaml"),
                        gix::object::tree::EntryKind::Blob,
                        blob,
                    )
                    .unwrap();
            }
        }
        let tree_id = editor.write().unwrap().detach();
        repo.commit_as(
            sig_ref,
            sig_ref,
            "refs/heads/__SCHEMAS",
            "seed __SCHEMAS",
            tree_id,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();
    }

    /// Build a minimal __SYSTEM commit. `mems` is `(mem_name,
    /// config_json)` pairs; `repo_json` is the repo.json blob (or
    /// empty to skip).
    fn seed_system(gitdir: &Path, repo_json: &str, mems: &[(&str, &str)]) {
        let repo = gix::open(gitdir).unwrap();
        let actor = actor_for_test();
        let mut buf = gix::date::parse::TimeBuf::default();
        let sig_ref = actor.to_ref(&mut buf);
        let mut editor = repo.empty_tree().edit().unwrap();
        if !repo_json.is_empty() {
            let blob = repo.write_blob(repo_json.as_bytes()).unwrap().detach();
            editor
                .upsert(
                    "repo.json",
                    gix::object::tree::EntryKind::Blob,
                    blob,
                )
                .unwrap();
        }
        for (mem, config) in mems {
            let blob = repo.write_blob(config.as_bytes()).unwrap().detach();
            editor
                .upsert(
                    format!("{mem}/config.json"),
                    gix::object::tree::EntryKind::Blob,
                    blob,
                )
                .unwrap();
        }
        let tree_id = editor.write().unwrap().detach();
        repo.commit_as(
            sig_ref,
            sig_ref,
            "refs/heads/__SYSTEM",
            "seed __SYSTEM",
            tree_id,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();
    }

    /// Walk the `__MEMSTEAD` tree at `gitdir` and return every
    /// (path, blob_oid) entry — used to assert tree shape.
    fn list_memstead_entries(gitdir: &Path) -> Vec<String> {
        let repo = gix::open(gitdir).unwrap();
        let reference = repo
            .try_find_reference("refs/heads/__MEMSTEAD")
            .unwrap()
            .unwrap();
        let id = reference.into_fully_peeled_id().unwrap();
        let commit = id.object().unwrap().try_into_commit().unwrap();
        let tree = commit.tree().unwrap();
        let mut out: Vec<String> = Vec::new();
        walk(&repo, &tree, "", &mut out);
        out.sort();
        out
    }

    fn walk(repo: &gix::Repository, tree: &gix::Tree<'_>, prefix: &str, out: &mut Vec<String>) {
        for entry in tree.iter().flatten() {
            let name = std::str::from_utf8(entry.filename()).unwrap_or("").to_string();
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            match entry.mode().kind() {
                gix::object::tree::EntryKind::Tree => {
                    let subtree = repo
                        .find_object(entry.oid().to_owned())
                        .unwrap()
                        .into_tree();
                    walk(repo, &subtree, &path, out);
                }
                gix::object::tree::EntryKind::Blob
                | gix::object::tree::EntryKind::BlobExecutable => {
                    out.push(path);
                }
                _ => {}
            }
        }
    }

    #[test]
    fn migrate_writes_unified_tree_from_schemas_and_system() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        seed_schemas(
            &gitdir,
            &[
                ("default", "1.0.0", &[("spec", "name: spec\n")]),
                ("custom", "0.5.0", &[]),
            ],
        );
        seed_system(
            &gitdir,
            r#"{"name":"main"}"#,
            &[
                ("alpha", r#"{"format": 1, "schema": "default@1.0.0"}"#),
                ("beta", r#"{"format": 1, "schema": "custom@0.5.0"}"#),
            ],
        );

        let outcome = migrate_to_memstead_ref(&gitdir).unwrap();
        assert_eq!(outcome.schemas_migrated, 2);
        assert_eq!(outcome.mems_migrated, 2);
        assert!(!outcome.already_current);

        let entries = list_memstead_entries(&gitdir);
        // Schemas live under versioned paths.
        assert!(entries.contains(&"schemas/default@1.0.0/schema.yaml".to_string()));
        assert!(entries.contains(&"schemas/default@1.0.0/types/spec.yaml".to_string()));
        assert!(entries.contains(&"schemas/custom@0.5.0/schema.yaml".to_string()));
        // Mem configs live under mems/.
        assert!(entries.contains(&"mems/alpha/config.json".to_string()));
        assert!(entries.contains(&"mems/beta/config.json".to_string()));
        // repo.json explicitly NOT migrated.
        assert!(
            !entries.iter().any(|p| p.contains("repo.json")),
            "repo.json must not appear under __MEMSTEAD: {entries:?}"
        );
    }

    #[test]
    fn write_schema_to_memstead_ref_adds_package_idempotent_and_preserves_others() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());

        // Write a package onto an absent ref — the ref is created.
        let tiny = vec![
            ("schema.yaml".to_string(), b"name: tiny\nversion: 0.1.0\n".to_vec()),
            ("types/doc.yaml".to_string(), b"name: doc\n".to_vec()),
            ("mem-template.json".to_string(), b"{}\n".to_vec()),
        ];
        let out = write_schema_to_memstead_ref(&gitdir, "tiny", "0.1.0", &tiny).unwrap();
        assert!(!out.already_current);
        let entries = list_memstead_entries(&gitdir);
        for p in [
            "schemas/tiny@0.1.0/schema.yaml",
            "schemas/tiny@0.1.0/types/doc.yaml",
            "schemas/tiny@0.1.0/mem-template.json",
        ] {
            assert!(entries.iter().any(|e| e == p), "missing {p}: {entries:?}");
        }

        // Re-writing identical bytes is a no-op — same tip, no new commit.
        let again = write_schema_to_memstead_ref(&gitdir, "tiny", "0.1.0", &tiny).unwrap();
        assert!(again.already_current);
        assert_eq!(out.commit_sha, again.commit_sha);

        // A second package upserts into the existing tree, preserving the first.
        let other = vec![(
            "schema.yaml".to_string(),
            b"name: other\nversion: 2.0.0\n".to_vec(),
        )];
        let out2 = write_schema_to_memstead_ref(&gitdir, "other", "2.0.0", &other).unwrap();
        assert!(!out2.already_current);
        assert_ne!(out2.commit_sha, out.commit_sha);
        let entries2 = list_memstead_entries(&gitdir);
        assert!(entries2.iter().any(|e| e == "schemas/tiny@0.1.0/schema.yaml"));
        assert!(entries2.iter().any(|e| e == "schemas/other@2.0.0/schema.yaml"));
    }

    #[test]
    fn pro_git_branch_ops_write_schema_hook_writes_to_ref() {
        // The engine reaches the ref-write through the
        // `GitBranchOps.write_schema` dispatcher; this pins that the const
        // is wired to `write_schema_to_memstead_ref` and returns a sha.
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let files = vec![("schema.yaml".to_string(), b"name: h\nversion: 1.0.0\n".to_vec())];
        let commit =
            (crate::storage::FULL_GIT_BRANCH_OPS.write_schema)(&gitdir, "h", "1.0.0", &files)
                .expect("hook writes the package");
        assert!(!commit.is_empty());
        let entries = list_memstead_entries(&gitdir);
        assert!(
            entries.iter().any(|e| e == "schemas/h@1.0.0/schema.yaml"),
            "package must land on the ref: {entries:?}"
        );
    }

    #[test]
    fn migrate_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        seed_schemas(&gitdir, &[("default", "1.0.0", &[])]);
        seed_system(
            &gitdir,
            r#"{"name":"main"}"#,
            &[("alpha", r#"{"format": 1, "schema": "default@1.0.0"}"#)],
        );

        let first = migrate_to_memstead_ref(&gitdir).unwrap();
        assert!(!first.already_current);
        let second = migrate_to_memstead_ref(&gitdir).unwrap();
        assert!(second.already_current);
        // Same tip; no new commit was written.
        assert_eq!(first.commit_sha, second.commit_sha);
    }

    #[test]
    fn migrate_with_empty_workspace_writes_empty_tree() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        // No __SCHEMAS, no __SYSTEM — the migration writes an
        // empty __MEMSTEAD tree (the cutover session decides what to
        // do with that case).
        let outcome = migrate_to_memstead_ref(&gitdir).unwrap();
        assert_eq!(outcome.schemas_migrated, 0);
        assert_eq!(outcome.mems_migrated, 0);
        let entries = list_memstead_entries(&gitdir);
        assert!(entries.is_empty());
    }

    #[test]
    fn migrate_handles_missing_version_with_placeholder() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        // Schema YAML with no `version:` field.
        let repo = gix::open(&gitdir).unwrap();
        let actor = actor_for_test();
        let mut buf = gix::date::parse::TimeBuf::default();
        let sig_ref = actor.to_ref(&mut buf);
        let mut editor = repo.empty_tree().edit().unwrap();
        let blob = repo.write_blob(b"name: anonymous\n").unwrap().detach();
        editor
            .upsert(
                "anonymous/schema.yaml",
                gix::object::tree::EntryKind::Blob,
                blob,
            )
            .unwrap();
        let tree_id = editor.write().unwrap().detach();
        repo.commit_as(
            sig_ref,
            sig_ref,
            "refs/heads/__SCHEMAS",
            "seed",
            tree_id,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();

        let outcome = migrate_to_memstead_ref(&gitdir).unwrap();
        assert_eq!(outcome.schemas_migrated, 1);
        let entries = list_memstead_entries(&gitdir);
        assert!(
            entries.contains(&"schemas/anonymous@0.0.0/schema.yaml".to_string()),
            "missing-version schemas land under @0.0.0; got {entries:?}"
        );
    }

    #[test]
    fn read_mem_config_from_memstead_round_trips_after_migration() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        seed_schemas(&gitdir, &[("default", "1.0.0", &[])]);
        seed_system(
            &gitdir,
            r#"{"name":"main"}"#,
            &[("alpha", r#"{"format": 1, "schema": "default@1.0.0"}"#)],
        );
        let _ = migrate_to_memstead_ref(&gitdir).unwrap();

        let config = read_mem_config_from_memstead_ref(&gitdir, "alpha").unwrap();
        assert!(config.schema.is_some());
        assert_eq!(config.schema.unwrap().to_string(), "default@1.0.0");
    }

    #[test]
    fn read_mem_config_from_memstead_returns_typed_error_for_missing_mem() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        seed_schemas(&gitdir, &[("default", "1.0.0", &[])]);
        seed_system(
            &gitdir,
            r#"{"name":"main"}"#,
            &[("alpha", r#"{"format": 1, "schema": "default@1.0.0"}"#)],
        );
        let _ = migrate_to_memstead_ref(&gitdir).unwrap();
        match read_mem_config_from_memstead_ref(&gitdir, "nonexistent") {
            Err(MemsteadRefError::Config { path, .. }) => {
                assert!(path.contains("nonexistent"));
            }
            other => panic!("expected Config error for missing mem, got {other:?}"),
        }
    }

    #[test]
    fn commit_config_to_memstead_creates_ref_when_absent() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        // No __MEMSTEAD ref, no __SYSTEM ref — fresh repo. The helper
        // must create __MEMSTEAD from scratch via the MustNotExist
        // precondition.
        let ctx = CommitContext::internal();
        commit_config_to_memstead_at_gitdir(
            &gitdir,
            "alpha",
            br#"{"format": 1, "schema": "default@1.0.0"}"#,
            &ctx,
            "test commit",
        )
        .unwrap();

        let entries = list_memstead_entries(&gitdir);
        assert_eq!(entries, vec!["mems/alpha/config.json".to_string()]);

        let config = read_mem_config_from_memstead_ref(&gitdir, "alpha").unwrap();
        assert_eq!(
            config.schema.unwrap().to_string(),
            "default@1.0.0"
        );
    }

    #[test]
    fn commit_config_to_memstead_overwrites_existing_blob() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let ctx = CommitContext::internal();

        commit_config_to_memstead_at_gitdir(
            &gitdir,
            "alpha",
            br#"{"format": 1, "schema": "default@1.0.0"}"#,
            &ctx,
            "first",
        )
        .unwrap();
        commit_config_to_memstead_at_gitdir(
            &gitdir,
            "alpha",
            br#"{"format": 1, "schema": "default@2.0.0"}"#,
            &ctx,
            "second",
        )
        .unwrap();

        let config = read_mem_config_from_memstead_ref(&gitdir, "alpha").unwrap();
        assert_eq!(
            config.schema.unwrap().to_string(),
            "default@2.0.0"
        );
    }

    #[test]
    fn commit_config_to_memstead_preserves_sibling_mem_entries() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let ctx = CommitContext::internal();

        commit_config_to_memstead_at_gitdir(
            &gitdir,
            "alpha",
            br#"{"format": 1, "schema": "default@1.0.0"}"#,
            &ctx,
            "alpha",
        )
        .unwrap();
        commit_config_to_memstead_at_gitdir(
            &gitdir,
            "beta",
            br#"{"format": 1, "schema": "default@1.0.0"}"#,
            &ctx,
            "beta",
        )
        .unwrap();

        let entries = list_memstead_entries(&gitdir);
        assert_eq!(
            entries,
            vec![
                "mems/alpha/config.json".to_string(),
                "mems/beta/config.json".to_string(),
            ]
        );
    }

    #[test]
    fn extract_manifest_version_handles_quoted_and_unquoted() {
        assert_eq!(
            extract_manifest_version("name: foo\nversion: \"1.0.0\"\n"),
            Some("1.0.0".to_string())
        );
        assert_eq!(
            extract_manifest_version("name: foo\nversion: 1.0.0\n"),
            Some("1.0.0".to_string())
        );
        assert_eq!(
            extract_manifest_version("name: foo\nversion: '0.5.0'\n"),
            Some("0.5.0".to_string())
        );
        assert_eq!(extract_manifest_version("name: foo\n"), None);
        assert_eq!(extract_manifest_version(""), None);
    }
}
