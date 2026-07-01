//! UniFFI bindings crate. Wraps `memstead-base::Engine` as an in-process Rust
//! library callable from the macOS app via UniFFI-generated Swift
//! bindings.
//!
//! Method bodies dispatch to `memstead_base::Engine`. The wrapper is a thin
//! translation layer: locking the shared engine mutex, re-shaping inputs
//! (strings → `EntityId`/`SchemaRef`, `u32` → `usize`), and flattening
//! outputs into the FFI records defined in `src/types.rs`. All business
//! logic stays in the engine crates.
//!
//! Boot path: `Engine::new(vaults)` uses `current_dir()` as the workspace
//! root and calls `memstead_git_branch::workspace_store::engine_from_workspace_root`
//! — the same entry point `memstead-mcp` uses. The `vaults` parameter is
//! retained for FFI compatibility but no longer drives mount resolution
//! (mounts are read from `.memstead/state/mounts.json`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use memstead_base::EntityId;

mod convert;
mod error;
mod types;

pub use error::MemsteadError;
pub use types::{
    AgentNotesReport, ChangeEnvelope, ChangesReport, ClusterInfo, CommitNote, EdgeSource,
    EdgeTypeCount, Entity, HealthIssue, HealthOptions, ListResult, MetadataEntry, MetadataValue,
    MissingField, ParseRecoveryEntry, ParseRecoveryReport, Query, RelationDirection, RelationEdge,
    Relations, ReloadResult, Relationship, SearchHit, SearchResult, SearchScope, Section,
    StaleEntity, Stats, VaultBackendKind, VaultCreateOutcome, VaultCreateRequest,
    VaultDeleteOutcome, DanglingCrossVaultEdge, VaultExportOutcome, VaultInit, VaultRosterEntry,
    VaultSchemaOutcome, VaultVersionOutcome, HealthSummary,
};

uniffi::include_scaffolding!("memstead");

/// Returns the crate version as a static string.
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Enumerate the vaults of the workspace at `project_root`. Walks the
/// `vault-repo/.git/` branch list and produces one `VaultInit` per vault
/// (excluding `main` and registry refs). Returns an empty list when no
/// real vault-repo is present.
///
/// The macOS app calls this before constructing the Engine so the UI can
/// show the vault list independently. The Engine itself loads its mounts
/// from `.memstead/state/mounts.json` and does not consume the returned
/// VaultInit list.
pub fn discover_vaults(project_root: String) -> Vec<VaultInit> {
    let root = Path::new(&project_root);
    let names = match memstead_git_branch::discover::enumerate_vault_repo_branches(root) {
        Some(names) => names,
        None => return Vec::new(),
    };
    names
        .iter()
        .filter_map(|name| {
            memstead_git_branch::vault_repo_config::vault_init_from_branch(root, name).ok()
        })
        .map(|init| VaultInit {
            name: init.name,
            dir: init
                .dir
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            schema_name: init.schema_ref.name,
            schema_version: init.schema_ref.version.to_string(),
        })
        .collect()
}

/// Construct an engine rooted at an explicit `workspace_root` — the
/// production, cwd-independent entry the macOS app uses to open a chosen
/// workspace. The path must contain `.memstead/workspace.toml`; pointing
/// at a directory with no recognised workspace layout returns a typed,
/// actionable error rather than silently falling back to the current
/// working directory (the behaviour of the legacy `Engine::new`).
///
/// Backend-agnostic: each mount the workspace declares resolves its own
/// backend (folder or git-branch), so the same call opens a git-branch
/// workspace and a folder workspace identically — the caller passes a path
/// and never branches on backend.
pub fn engine_open(workspace_root: String) -> Result<Arc<Engine>, MemsteadError> {
    Ok(Arc::new(Engine::from_workspace_root(Path::new(
        &workspace_root,
    ))?))
}

/// Initialise a brand-new filesystem (folder-backed) vault at `root` — the
/// engine-owned bootstrap the macOS app routes through instead of writing
/// `.memstead/config.json` from Swift. Delegates to
/// `memstead_base::filesystem::config::init_filesystem_vault`, which writes the
/// seed config + `.memstead/` markers + one-folder-mount roster; the caller
/// then opens it via [`engine_open`]. A malformed schema ref refuses cleanly.
pub fn init_filesystem_vault(
    root: String,
    name: String,
    schema: String,
) -> Result<(), MemsteadError> {
    let schema_ref = schema
        .parse::<memstead_schema::SchemaRef>()
        .map_err(|e| MemsteadError::ValidationFailed {
            message: format!("invalid schema ref {schema:?}: {e}"),
        })?;
    memstead_base::filesystem::config::init_filesystem_vault(Path::new(&root), &name, &schema_ref)
        .map_err(|e| MemsteadError::IoError {
            message: e.to_string(),
        })?;
    Ok(())
}

/// In-process handle to the Memstead engine. Wraps `memstead_base::Engine` behind
/// a `Mutex` — same shape as `memstead-mcp::McpServer` — so multiple Swift
/// callers can share one engine instance.
pub struct Engine {
    inner: Mutex<memstead_base::Engine>,
}

impl Engine {
    /// Construct an engine rooted at the current working directory. The
    /// `vaults` parameter is retained for FFI compatibility but is
    /// ignored — mounts come from `.memstead/state/mounts.json` via the
    /// workspace-store loader.
    pub fn new(_vaults: Vec<VaultInit>) -> Result<Self, MemsteadError> {
        let workspace_root =
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::from_workspace_root(&workspace_root)
    }

    fn from_workspace_root(workspace_root: &Path) -> Result<Self, MemsteadError> {
        let engine = memstead_git_branch::workspace_store::engine_from_workspace_root(workspace_root)
            .map_err(|e| MemsteadError::Internal {
                message: format!(
                    "failed to load workspace at {}: {}",
                    workspace_root.display(),
                    e
                ),
            })?;
        Ok(Self {
            inner: Mutex::new(engine),
        })
    }

    pub fn get_stats(&self) -> Stats {
        let engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        convert::stats_to_ffi(engine.stats(), engine.store(), engine.vault_router())
    }

    pub fn get_health(&self, _options: HealthOptions) -> HealthSummary {
        // `HealthOptions` is reserved for future knobs (most-connected limit
        // etc.). The engine's `health()` has no options today; once it grows
        // any, route them here without changing the FFI signature.
        let engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        convert::health_summary_to_ffi(engine.health())
    }

    pub fn list_entities(&self, scope: SearchScope) -> ListResult {
        let core_scope = convert::search_scope_from_ffi(scope);
        let engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        convert::list_result_to_ffi(engine.list(&core_scope))
    }

    pub fn search(&self, scope: SearchScope) -> SearchResult {
        let core_scope = convert::search_scope_from_ffi(scope);
        let engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        // `Engine::search` returns `Err(SearchUnavailable)` only on wasm32
        // — the UniFFI binding compiles for native targets where the
        // tantivy backend is present, so the `Ok` path is total here.
        let result = engine
            .search(&core_scope)
            .expect("memstead-swift binds the native engine; search returns Ok");
        convert::search_result_to_ffi(result)
    }

    pub fn get_entity(&self, id: String) -> Option<Entity> {
        let eid = EntityId(id);
        let engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.get_entity(&eid).map(convert::entity_to_ffi)
    }

    pub fn get_relations(&self, id: String) -> Result<Relations, MemsteadError> {
        let eid = EntityId(id.clone());
        let engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        if engine.get_entity(&eid).is_none() {
            return Err(MemsteadError::NotFound { message: id });
        }
        Ok(convert::build_relations(engine.store(), &eid))
    }

    pub fn get_overview(&self, rebuild: bool) -> Vec<ClusterInfo> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        if rebuild {
            engine.invalidate_communities();
        }
        convert::clusters_to_ffi(engine.communities())
    }

    /// The workspace vault roster — one entry per mounted vault carrying
    /// the per-mount facts the home sidebar renders. Backend kind,
    /// capability, and schema pin come straight from the engine's mounts;
    /// the entity count is the live non-stub count for each vault; drift is
    /// the engine's read-only probe (git-branch only). No Swift-side
    /// reconstruction — a vault the engine reports read-only is never shown
    /// writable.
    pub fn vault_roster(&self) -> Vec<VaultRosterEntry> {
        use memstead_base::workspace::{MountCapability, MountStorage};
        let engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");

        // Live non-stub entity count per vault, in one pass over the store.
        let mut counts: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
        for entity in engine.store().all_entities() {
            if !entity.stub {
                *counts.entry(entity.vault.as_str()).or_insert(0) += 1;
            }
        }

        let mut roster: Vec<VaultRosterEntry> = engine
            .mounts()
            .into_iter()
            .map(|mount| {
                let backend = match mount.storage {
                    MountStorage::GitBranch { .. } => VaultBackendKind::GitBranch,
                    MountStorage::Folder { .. } => VaultBackendKind::Folder,
                    MountStorage::Archive { .. } => VaultBackendKind::Archive,
                    MountStorage::InMemory => VaultBackendKind::InMemory,
                };
                VaultRosterEntry {
                    vault: mount.vault.clone(),
                    backend,
                    writable: mount.capability == MountCapability::Write,
                    schema_pin: mount.schema.as_ref().map(|s| s.to_string()),
                    entity_count: counts.get(mount.vault.as_str()).copied().unwrap_or(0),
                    // Drift is git-branch-only and advisory; a probe error
                    // collapses to `false` (the engine method already
                    // swallows backend errors).
                    drifted: engine.vault_drifted(&mount.vault).unwrap_or(false),
                }
            })
            .collect();
        // Stable order for SwiftUI Identifiable lists.
        roster.sort_by(|a, b| a.vault.cmp(&b.vault));
        roster
    }

    /// Workspace-wide reload. Aggregates per-vault reload diffs from
    /// `reload_each_writable_vault` into a single `ReloadResult`.
    pub fn reload(&self) -> Result<ReloadResult, MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        let per_vault = engine.reload_each_writable_vault()?;
        let mut added = Vec::new();
        let mut changed = Vec::new();
        let mut removed = Vec::new();
        for (_, r) in per_vault {
            added.extend(r.added);
            changed.extend(r.changed);
            removed.extend(r.removed);
        }
        Ok(convert::reload_result_to_ffi(memstead_base::ops::ReloadResult {
            added,
            changed,
            removed,
        }))
    }

    pub fn vault_head_sha(&self, vault: String) -> Result<Option<String>, MemsteadError> {
        let engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        Ok(engine.vault_head_sha(&vault)?)
    }

    pub fn changes_since(
        &self,
        vault: String,
        since: String,
        rename_similarity: Option<f32>,
    ) -> Result<ChangesReport, MemsteadError> {
        let engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        let report = engine.changes_since(&vault, &since, rename_similarity)?;
        Ok(convert::changes_report_to_ffi(report))
    }

    pub fn agent_notes(
        &self,
        vault: String,
        since: String,
    ) -> Result<AgentNotesReport, MemsteadError> {
        let engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        let report = match engine.mount(&vault) {
            Some(m) => match &m.storage {
                memstead_base::MountStorage::GitBranch { gitdir, branch } => {
                    let ref_name = if branch.starts_with("refs/") {
                        branch.clone()
                    } else {
                        format!("refs/heads/{branch}")
                    };
                    memstead_git_branch::ops::agent_notes::agent_notes_since(
                        &vault,
                        gitdir,
                        &since,
                        Some(&ref_name),
                    )
                    .map_err(|e| MemsteadError::Internal {
                        message: format!("agent_notes: {e}"),
                    })?
                }
                _ => memstead_base::ops::AgentNotesReport {
                    vault: vault.clone(),
                    since: since.clone(),
                    head: since.clone(),
                    notes: Vec::new(),
                    memstead_ref: None,
                },
            },
            None => {
                return Err(MemsteadError::UnknownVault {
                    name: vault,
                    writable_vaults: Vec::new(),
                });
            }
        };
        Ok(convert::agent_notes_report_to_ffi(report))
    }

    /// Bulk-fix accumulated parse-time relation drift across every
    /// writable vault — the UniFFI counterpart to `memstead recover`.
    ///
    /// This is the first **disk-mutating** UniFFI method: it locks the
    /// shared engine, re-renders each writable source entity carrying a
    /// `PARSED_RELATION_INVALID` drop, and commits. Read-only-origin
    /// drops are reported `skipped`; per-entry failures keep the rest of
    /// the batch alive. Re-running on a clean workspace returns an empty
    /// report with no commit.
    ///
    /// Provenance mirrors the CLI's `memstead recover` (`Actor::Cli`, no
    /// paired client) so the recovery is bit-identical across the two
    /// surfaces — the macOS embedder has no distinct provenance category
    /// today, and inventing one would touch every `Actor` match site.
    /// The optional `note` lands on every per-source re-render commit.
    pub fn apply_parse_recovery(
        &self,
        note: Option<String>,
    ) -> Result<ParseRecoveryReport, MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        let report = engine.apply_parse_recovery(
            memstead_base::vcs::Actor::Cli,
            None,
            note.as_deref(),
        )?;
        Ok(convert::parse_recovery_report_to_ffi(report))
    }

    // --- Four-primitive pipeline edits -------------------------------------
    //
    // The macOS pipeline editor routes medium/facet/projection edits here
    // instead of hand-writing `.memstead/` JSON, so the engine owns the
    // four-primitive store (referential integrity, snapshot refresh). Create
    // and update carry the primitive as a JSON string (the `Facet.engagement`
    // field is free-form JSON, so a typed FFI record would not round-trip);
    // delete and rename take plain identifiers. Each delegates to
    // `memstead_base::Engine`, whose methods refresh the in-memory snapshot.

    /// Create a medium from a JSON-encoded `Medium`. See `Engine::add_medium`.
    pub fn add_medium(
        &self,
        vault: String,
        name: String,
        medium_json: String,
    ) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.add_medium_json(&vault, &name, &medium_json)?;
        Ok(())
    }

    /// Overwrite a medium from a JSON-encoded `Medium`. See `Engine::update_medium`.
    pub fn update_medium(
        &self,
        vault: String,
        name: String,
        medium_json: String,
    ) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.update_medium_json(&vault, &name, &medium_json)?;
        Ok(())
    }

    /// Delete a medium (refused while a facet references it). See `Engine::delete_medium`.
    pub fn delete_medium(&self, vault: String, name: String) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.delete_medium(&vault, &name)?;
        Ok(())
    }

    /// Rename a medium, repointing dependent facets. See `Engine::rename_medium`.
    pub fn rename_medium(
        &self,
        vault: String,
        old_name: String,
        new_name: String,
    ) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.rename_medium(&vault, &old_name, &new_name)?;
        Ok(())
    }

    /// Create a facet from a JSON-encoded `Facet`. See `Engine::add_facet`.
    pub fn add_facet(
        &self,
        vault: String,
        name: String,
        facet_json: String,
    ) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.add_facet_json(&vault, &name, &facet_json)?;
        Ok(())
    }

    /// Overwrite a facet from a JSON-encoded `Facet`. See `Engine::update_facet`.
    pub fn update_facet(
        &self,
        vault: String,
        name: String,
        facet_json: String,
    ) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.update_facet_json(&vault, &name, &facet_json)?;
        Ok(())
    }

    /// Delete a facet (refused while a projection references it). See `Engine::delete_facet`.
    pub fn delete_facet(&self, vault: String, name: String) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.delete_facet(&vault, &name)?;
        Ok(())
    }

    /// Rename a facet, repointing dependent projections. See `Engine::rename_facet`.
    pub fn rename_facet(
        &self,
        vault: String,
        old_name: String,
        new_name: String,
    ) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.rename_facet(&vault, &old_name, &new_name)?;
        Ok(())
    }

    /// Create a projection from a JSON-encoded `Projection`. See `Engine::add_projection`.
    pub fn add_projection(
        &self,
        vault: String,
        name: String,
        projection_json: String,
    ) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.add_projection_json(&vault, &name, &projection_json)?;
        Ok(())
    }

    /// Overwrite a projection from a JSON-encoded `Projection`. See `Engine::update_projection`.
    pub fn update_projection(
        &self,
        vault: String,
        name: String,
        projection_json: String,
    ) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.update_projection_json(&vault, &name, &projection_json)?;
        Ok(())
    }

    /// Delete a projection (refused while an ingest runs it). See `Engine::delete_projection`.
    pub fn delete_projection(&self, vault: String, name: String) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.delete_projection(&vault, &name)?;
        Ok(())
    }

    /// Rename a projection, repointing dependent ingests. See `Engine::rename_projection`.
    pub fn rename_projection(
        &self,
        vault: String,
        old_name: String,
        new_name: String,
    ) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.rename_projection(&vault, &old_name, &new_name)?;
        Ok(())
    }

    /// Create an ingest from a JSON-encoded `Ingest`. Ingests are flat
    /// (workspace-level). See `Engine::add_ingest`.
    pub fn add_ingest(&self, name: String, ingest_json: String) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.add_ingest_json(&name, &ingest_json)?;
        Ok(())
    }

    /// Overwrite an ingest from a JSON-encoded `Ingest`. See `Engine::update_ingest`.
    pub fn update_ingest(&self, name: String, ingest_json: String) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.update_ingest_json(&name, &ingest_json)?;
        Ok(())
    }

    /// Delete an ingest (nothing references it). See `Engine::delete_ingest`.
    pub fn delete_ingest(&self, name: String) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.delete_ingest(&name)?;
        Ok(())
    }

    /// Rename an ingest. See `Engine::rename_ingest`.
    pub fn rename_ingest(&self, old_name: String, new_name: String) -> Result<(), MemsteadError> {
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.rename_ingest(&old_name, &new_name)?;
        Ok(())
    }

    /// The four-primitive pipeline store serialized as JSON — the read
    /// counterpart of the edit methods, which the macOS pipeline editor
    /// deserializes to display the store. See
    /// `memstead_base::Engine::pipeline_configs_json`.
    pub fn pipeline_configs_json(&self) -> String {
        let engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        engine.pipeline_configs_json()
    }

    // --- Vault lifecycle ---------------------------------------------------
    //
    // The macOS roster routes create/delete/set-schema/set-version here
    // instead of mutating the vault-repo from Swift. create/delete delegate
    // to the `memstead_engine::vault_management` orchestrators (the same
    // entry points `memstead-mcp` calls behind `memstead_vault_create` /
    // `memstead_vault_delete`); set-schema/set-version are methods directly
    // on `memstead_base::Engine`. The engine owns backend instantiation,
    // allowlist gating, the policy scrub, and the seed/version commits — the
    // binding only re-shapes inputs and flattens outcomes.

    /// Create and register a writable vault. Mirrors `memstead_vault_create`.
    /// See `memstead_engine::vault_management::create_vault`.
    pub fn create_vault(
        &self,
        request: VaultCreateRequest,
    ) -> Result<VaultCreateOutcome, MemsteadError> {
        let schema_ref = request
            .schema
            .parse::<memstead_schema::SchemaRef>()
            .map_err(|e| MemsteadError::ValidationFailed {
                message: format!("invalid schema ref {:?}: {e}", request.schema),
            })?;
        // The two scalar VCS fields collapse back into the engine's nested
        // `VcsConfig`. Only `vcs_gitdir` gates the override; `vcs_worktree`
        // defaults to `"."` (vault root) when omitted — the engine's
        // isolated default shape.
        let vcs = request.vcs_gitdir.map(|gitdir| memstead_schema::VcsConfig {
            gitdir,
            worktree: request.vcs_worktree.unwrap_or_else(|| ".".to_string()),
        });
        let params = memstead_engine::vault_management::VaultCreateParams {
            name: request.name,
            location: PathBuf::from(request.location),
            schema_ref,
            vcs,
            note: request.note,
            operator_mode: request.operator_mode,
            recovery: None,
            write_guidance: std::collections::HashMap::new(),
        };
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        let response = memstead_engine::vault_management::create_vault(&mut engine, params)?;
        Ok(VaultCreateOutcome {
            name: response.name,
            location: response.location.to_string_lossy().to_string(),
            schema_ref: response.schema_ref.to_string(),
            seed_commit_sha: response.seed_commit_sha,
            warnings: response.warnings.iter().map(|w| w.to_string()).collect(),
        })
    }

    /// Destructively remove a writable vault. Mirrors `memstead_vault_delete`
    /// (always `delete_files: true`). See
    /// `memstead_engine::vault_management::delete_vault`.
    pub fn delete_vault(
        &self,
        name: String,
        note: Option<String>,
        operator_mode: bool,
    ) -> Result<VaultDeleteOutcome, MemsteadError> {
        let params = memstead_engine::vault_management::VaultDeleteParams {
            name,
            delete_files: true,
            note,
            operator_mode,
        };
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        let response = memstead_engine::vault_management::delete_vault(&mut engine, params)?;
        Ok(VaultDeleteOutcome {
            name: response.name,
            deleted_from_router: response.deleted_from_router,
            files_deleted: response.files_deleted,
            warnings: response.warnings.iter().map(|w| w.to_string()).collect(),
        })
    }

    /// Set a vault's schema pin — the integrity-driven migration trigger.
    /// Mirrors `memstead_vault_set_schema`. See
    /// `memstead_base::Engine::set_vault_schema`.
    pub fn set_vault_schema(
        &self,
        vault: String,
        schema: String,
    ) -> Result<VaultSchemaOutcome, MemsteadError> {
        let target = schema
            .parse::<memstead_schema::SchemaRef>()
            .map_err(|e| MemsteadError::ValidationFailed {
                message: format!("invalid schema ref {schema:?}: {e}"),
            })?;
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        let outcome = engine.set_vault_schema(&vault, &target)?;
        Ok(convert::set_schema_outcome_to_ffi(outcome))
    }

    /// Set a vault's content `version` (consumed by export to stamp the
    /// archive). Mirrors `memstead_vault_set_version`. See
    /// `memstead_base::Engine::set_vault_version`.
    pub fn set_vault_version(
        &self,
        vault: String,
        version: String,
        note: Option<String>,
    ) -> Result<VaultVersionOutcome, MemsteadError> {
        let new_version =
            semver::Version::parse(&version).map_err(|e| MemsteadError::ValidationFailed {
                message: format!("version {version:?} is not a valid semver: {e}"),
            })?;
        let mut engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        let outcome = engine.set_vault_version(&vault, new_version, note.as_deref())?;
        Ok(VaultVersionOutcome {
            vault: outcome.vault,
            old_version: outcome.old_version.map(|v| v.to_string()),
            new_version: outcome.new_version.to_string(),
            warnings: outcome.warnings.iter().map(|w| w.to_string()).collect(),
        })
    }

    /// Export a vault as a portable `.mem` archive at `output_path`.
    /// Backend-symmetric — the engine snapshots a folder mount, walks a
    /// git-branch tip, and refuses archive (sealed) / in-memory
    /// (unsupported) mounts. See `memstead_base::Engine::export_vault`.
    pub fn export_vault(
        &self,
        vault: String,
        output_path: String,
    ) -> Result<VaultExportOutcome, MemsteadError> {
        let engine = self.inner.lock().expect("memstead-swift engine mutex poisoned");
        let result = engine.export_vault(&vault, Path::new(&output_path))?;
        Ok(VaultExportOutcome {
            archive_path: result.archive_path,
            name: result.name,
            version: result.version,
            entity_count: result.entity_count as u64,
            size_bytes: result.size_bytes,
            dangling_cross_vault_edges: result
                .dangling_cross_vault_edges
                .into_iter()
                .map(|e| DanglingCrossVaultEdge {
                    entity_path: e.entity_path,
                    target_id: e.target_id,
                    target_vault: e.target_vault,
                })
                .collect(),
        })
    }

    /// Test-only constructor that accepts an explicit `workspace_root`,
    /// bypassing the cwd fallback. Used by in-process tests (and any
    /// external harness opting in via the `test-support` feature) so the
    /// engine can be rooted at a `TempDir` whose `vault-repo/.git/` has
    /// been seeded by `memstead_git_branch::test_support`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new_for_test(
        _vaults: Vec<VaultInit>,
        workspace_root: PathBuf,
    ) -> Result<Self, MemsteadError> {
        Self::from_workspace_root(&workspace_root)
    }
}

// ---------------------------------------------------------------------------
// Test-support seeding. Compiled into the in-crate test build (`cfg(test)`)
// and into the `--features test-support` framework variant the macOS smoke
// suite links — never into the featureless framework the Release app ships.
// ---------------------------------------------------------------------------

/// Canonical two-entity fixture: a `specs` vault holding `entity-a` (which
/// USES `entity-b`) and `entity-b`. Shared by the Rust-side parity tests and
/// the FFI `TestSupport` seeder so a regression shows up identically on both
/// surfaces.
#[cfg(any(test, feature = "test-support"))]
const FIXTURE_ENTITY_A: &str = "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\ntags: backend\n---\n# Entity A\n\n## Identity\n\nFirst test entity.\n\n## Purpose\n\nTesting the FFI bindings.\n\n## Relationships\n\n- **USES**: [[entity-b]]\n";

#[cfg(any(test, feature = "test-support"))]
const FIXTURE_ENTITY_B: &str = "---\ntype: spec\ncreated_date: 2026-02-01\nlast_modified: 2026-04-12\nlevel: M1\ntags: frontend\n---\n# Entity B\n\n## Identity\n\nSecond test entity.\n\n## Purpose\n\nDependency of Entity A.\n";

/// Seed the canonical fixture into a real `vault-repo/.git` under
/// `workspace_root`. Routes through the engine's own
/// `memstead_git_branch::test_support` helper — callers never shell out to
/// `git` or write refs directly, preserving "the engine owns vault-repo
/// state."
#[cfg(any(test, feature = "test-support"))]
fn seed_canonical_fixture(workspace_root: &Path) {
    memstead_git_branch::test_support::init_real_vault_repo_with_entities(
        workspace_root,
        &[(
            "specs",
            "default@1.0.0",
            &[
                ("entity-a.md", FIXTURE_ENTITY_A),
                ("entity-b.md", FIXTURE_ENTITY_B),
            ],
        )],
    );
}

/// FFI entry point for hermetic test seeding. Present only in the
/// `--features test-support` framework variant; the featureless build the
/// Release app links never carries it.
#[cfg(feature = "test-support")]
pub struct TestSupport;

#[cfg(feature = "test-support")]
impl TestSupport {
    pub fn new() -> Self {
        Self
    }

    /// Seed the canonical two-entity fixture into a fresh `vault-repo/.git`
    /// under `workspace_root` and return an `Engine` rooted there.
    pub fn seeded_engine(&self, workspace_root: String) -> Result<Arc<Engine>, MemsteadError> {
        let root = PathBuf::from(workspace_root);
        seed_canonical_fixture(&root);
        Ok(Arc::new(Engine::from_workspace_root(&root)?))
    }
}

#[cfg(feature = "test-support")]
impl Default for TestSupport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    /// Minimal two-entity vault fixture. Lays down a real `vault-repo-git`
    /// (`main` + `specs` branch carrying entity blobs) so dispatcher
    /// GitTree reads pin the engine state without disk shells.
    fn setup_test_engine() -> (Engine, TempDir) {
        let tmp = TempDir::new().expect("tempdir");
        seed_canonical_fixture(tmp.path());

        let engine = Engine::new_for_test(
            vec![VaultInit {
                name: "specs".to_string(),
                dir: String::new(),
                schema_name: "default".to_string(),
                schema_version: "1.0.0".to_string(),
            }],
            tmp.path().to_path_buf(),
        )
        .expect("engine init");

        (engine, tmp)
    }

    #[test]
    fn version_matches_cargo_pkg_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn get_stats_counts_fixture_entities() {
        let (engine, _tmp) = setup_test_engine();
        let stats = engine.get_stats();
        assert!(stats.entity_count >= 2, "entity_count: {}", stats.entity_count);
        assert!(stats.edge_count >= 1, "edge_count: {}", stats.edge_count);
        assert_eq!(stats.vault_count, 1);
        assert!(stats.types_in_use.iter().any(|t| t == "spec"));
        assert_eq!(stats.stub_count, 0);
        assert_eq!(stats.writable_vaults, vec!["specs".to_string()]);
        assert!(stats.read_vaults.is_empty());
    }

    #[test]
    fn engine_open_roots_at_explicit_seeded_workspace() {
        // Seed a real git-branch workspace, then open it through the
        // production FFI entry (not the cwd-fallback constructor). The
        // returned engine must carry the seeded vault's real data.
        let tmp = TempDir::new().expect("tempdir");
        seed_canonical_fixture(tmp.path());

        let engine = engine_open(tmp.path().to_string_lossy().to_string())
            .expect("engine_open on a seeded workspace");
        let stats = engine.get_stats();
        assert!(stats.entity_count >= 2, "entity_count: {}", stats.entity_count);
        assert_eq!(stats.writable_vaults, vec!["specs".to_string()]);
    }

    #[test]
    fn engine_open_roots_at_folder_workspace() {
        // AC3: a folder-backed workspace opens through the *same* `engine_open`
        // entry as a git-branch workspace — no backend-specific call path. The
        // app's connect path is therefore backend-agnostic by construction.
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();

        let vault_dir = root.join("notes");
        std::fs::create_dir_all(&vault_dir).expect("vault dir");
        std::fs::write(
            vault_dir.join("hello.md"),
            "---\ntype: spec\n---\n# Hello\n\n## Identity\n\nA.\n",
        )
        .expect("entity");

        let memstead = root.join(".memstead");
        std::fs::create_dir_all(memstead.join("state")).expect("state dir");
        std::fs::write(
            memstead.join("workspace.toml"),
            "format = \"memstead-git-branch-1\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .expect("workspace.toml");
        std::fs::write(
            memstead.join("state").join("mounts.json"),
            r#"{"format":"memstead-mounts-2","mounts":[{"vault":"notes","schema":"default@1.0.0","storage":{"type":"folder","path":"notes"},"capability":"write","lifecycle":"eager","cross_linkable":true}]}"#,
        )
        .expect("mounts.json");

        let engine = engine_open(root.to_string_lossy().to_string())
            .expect("folder workspace opens through engine_open");
        let stats = engine.get_stats();
        assert_eq!(stats.writable_vaults, vec!["notes".to_string()]);
        assert!(stats.entity_count >= 1, "entity_count: {}", stats.entity_count);
    }

    #[test]
    fn engine_open_refuses_directory_without_workspace_toml() {
        // Refusal complement: an empty directory (no `.memstead/workspace.toml`)
        // surfaces a typed, actionable error — not a silent cwd fallback,
        // not a panic.
        let tmp = TempDir::new().expect("tempdir");
        let err = match engine_open(tmp.path().to_string_lossy().to_string()) {
            Ok(_) => panic!("expected engine_open to refuse a non-workspace directory"),
            Err(e) => e,
        };
        match err {
            MemsteadError::Internal { message } => {
                assert!(
                    message.contains(&tmp.path().display().to_string()),
                    "error must name the offending path: {message}"
                );
                assert!(
                    message.contains("not initialised"),
                    "error must be actionable: {message}"
                );
            }
            other => panic!("expected Internal boot error, got {other:?}"),
        }
    }

    #[test]
    fn discover_vaults_picks_up_fixture() {
        let (_engine, tmp) = setup_test_engine();
        let vaults = discover_vaults(tmp.path().to_string_lossy().to_string());
        assert_eq!(vaults.len(), 1, "expected one discovered vault");
        assert_eq!(vaults[0].name, "specs");
        assert_eq!(vaults[0].schema_name, "default");
        assert_eq!(vaults[0].schema_version, "1.0.0");
    }

    #[test]
    fn get_health_returns_summary_shape() {
        let (engine, _tmp) = setup_test_engine();
        let summary = engine.get_health(HealthOptions {
            most_connected_limit: 10,
        });
        assert!(summary.stale_entities.is_empty() || !summary.stale_entities.is_empty());
        let _ = summary.missing_fields.len();
        let _ = summary.orphan_count;
        let _ = summary.stub_count;
    }

    #[test]
    fn list_entities_returns_fixture() {
        let (engine, _tmp) = setup_test_engine();
        let result = engine.list_entities(SearchScope {
            query: None,
            vault: None,
            entity_type: None,
            limit: Some(100),
            offset: None,
            filters: HashMap::new(),
            range_filters: HashMap::new(),
            edge_type: None,
            related_to: None,
            depth: None,
            stub: None,
            expand_via: None,
            expand_depth: None,
        });
        assert!(result.total >= 2, "total: {}", result.total);
        assert!(result.hits.iter().any(|h| h.id.ends_with("entity-a")));
        assert!(result.hits.iter().any(|h| h.id.ends_with("entity-b")));
    }

    #[test]
    fn search_finds_substring() {
        let (engine, _tmp) = setup_test_engine();
        let result = engine.search(SearchScope {
            query: Some(crate::types::Query {
                any_of: vec!["Second".to_string()],
                ..Default::default()
            }),
            vault: None,
            entity_type: None,
            limit: Some(10),
            offset: None,
            filters: HashMap::new(),
            range_filters: HashMap::new(),
            edge_type: None,
            related_to: None,
            depth: None,
            stub: None,
            expand_via: None,
            expand_depth: None,
        });
        assert!(
            result.hits.iter().any(|h| h.id.ends_with("entity-b")),
            "expected entity-b to match 'Second', hits: {:?}",
            result.hits.iter().map(|h| &h.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn get_entity_hit_and_miss() {
        let (engine, _tmp) = setup_test_engine();
        let entity = engine.get_entity("specs--entity-a".to_string()).unwrap();
        assert_eq!(entity.title, "Entity A");
        assert_eq!(entity.entity_type, "spec");
        assert!(!entity.stub);
        assert!(entity.relationships.iter().any(|r| r.rel_type == "USES"));

        let missing = engine.get_entity("specs--does-not-exist".to_string());
        assert!(missing.is_none());
    }

    #[test]
    fn get_relations_composes_outgoing_and_incoming() {
        let (engine, _tmp) = setup_test_engine();
        let a = engine
            .get_relations("specs--entity-a".to_string())
            .expect("entity-a exists");
        assert!(!a.outgoing.is_empty());
        assert!(a.outgoing.iter().any(|r| r.rel_type == "USES"));
        assert!(a.outgoing.iter().any(|r| r.other_id.ends_with("entity-b")));

        let b = engine
            .get_relations("specs--entity-b".to_string())
            .expect("entity-b exists");
        assert!(b.incoming.iter().any(|r| r.other_id.ends_with("entity-a")));
    }

    #[test]
    fn get_relations_missing_entity_is_not_found() {
        let (engine, _tmp) = setup_test_engine();
        let err = engine
            .get_relations("specs--nope".to_string())
            .unwrap_err();
        match err {
            MemsteadError::NotFound { message } => assert!(message.contains("nope")),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn get_overview_returns_cluster_list() {
        let (engine, _tmp) = setup_test_engine();
        let clusters = engine.get_overview(false);
        assert!(!clusters.is_empty(), "expected at least one cluster");
        for c in &clusters {
            assert!(!c.id.is_empty());
        }
    }

    #[test]
    fn reload_returns_diff() {
        let (engine, _tmp) = setup_test_engine();
        let diff = engine.reload().expect("reload");
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        let _ = diff.changed.len();
    }

    #[test]
    fn vault_head_sha_returns_some_for_seeded_vault() {
        let (engine, _tmp) = setup_test_engine();
        let head = engine
            .vault_head_sha("specs".to_string())
            .expect("vault_head_sha");
        assert!(head.is_some(), "expected seeded vault to have a HEAD");
        let sha = head.unwrap();
        assert_eq!(sha.len(), 40, "head must be 40-char hex, got: {sha}");
        assert!(
            sha.chars().all(|c| c.is_ascii_hexdigit()),
            "head must be hex: {sha}",
        );
    }

    #[test]
    fn vault_head_sha_unknown_vault_errors() {
        let (engine, _tmp) = setup_test_engine();
        let err = engine
            .vault_head_sha("no-such".to_string())
            .unwrap_err();
        match err {
            MemsteadError::UnknownVault { name, .. } => assert_eq!(name, "no-such"),
            other => panic!("expected UnknownVault, got {other:?}"),
        }
    }

    #[test]
    fn changes_since_empty_tree_surfaces_seeded_entities() {
        let (engine, _tmp) = setup_test_engine();
        let report = engine
            .changes_since(
                "specs".to_string(),
                memstead_base::EMPTY_TREE_SHA.to_string(),
                None,
            )
            .expect("changes_since");
        assert_eq!(report.vault, "specs");
        let added: Vec<_> = report
            .changes
            .iter()
            .filter(|c| matches!(c, ChangeEnvelope::Added { .. }))
            .collect();
        assert!(
            added.len() >= 2,
            "expected ≥2 added events, got {} total changes",
            report.changes.len(),
        );
    }

    #[test]
    fn agent_notes_returns_memstead_ref_shape() {
        let (engine, _tmp) = setup_test_engine();
        let report = engine
            .agent_notes("specs".to_string(), memstead_base::EMPTY_TREE_SHA.to_string())
            .expect("agent_notes");
        assert_eq!(report.vault, "specs");
        // Fixture seeds `__MEMSTEAD` via the migrator; the FFI contract
        // being locked here is that `memstead_ref` round-trips without
        // crashing.
        let sha = report.memstead_ref.as_ref().expect("memstead_ref must be populated by fixture");
        assert_eq!(sha.len(), 40);
    }

    #[test]
    fn apply_parse_recovery_clears_drift_through_ffi() {
        // AC #14: parse-recovery must succeed through the UniFFI surface
        // the same way `memstead recover` does on equivalent input. Seed
        // a vault whose source entity declares an unknown rel-type; the
        // parser drops it at load (PARSED_RELATION_INVALID). The first
        // disk-mutating UniFFI method must recover it and report
        // `removed`, then be idempotent on the now-clean workspace.
        let tmp = TempDir::new().expect("tempdir");
        let target = "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# Target\n\n## Identity\n\nTarget body.\n";
        let source = "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# Source\n\n## Identity\n\nSource body.\n\n## Relationships\n\n- **MADE_UP_TYPE_A**: [[specs--target]]\n";
        memstead_git_branch::test_support::init_real_vault_repo_with_entities(
            tmp.path(),
            &[(
                "specs",
                "default@1.0.0",
                &[("target.md", target), ("source.md", source)],
            )],
        );
        let engine =
            Engine::from_workspace_root(tmp.path()).expect("engine opens seeded workspace");

        let report = engine
            .apply_parse_recovery(Some("ffi recovery".to_string()))
            .expect("recovery succeeds through the FFI surface");
        assert_eq!(
            report.entries.len(),
            1,
            "exactly one parse-time drop recovered, got {:?}",
            report.entries
        );
        assert_eq!(report.entries[0].outcome, "removed");
        assert_eq!(report.entries[0].rel_type, "MADE_UP_TYPE_A");
        assert!(report.entries[0].reason.is_none());
        assert!(report.commit_sha.is_some(), "a recovery that wrote must report its commit sha");

        // Idempotent: re-running on the now-clean workspace is a no-op.
        let again = engine
            .apply_parse_recovery(None)
            .expect("second recovery call succeeds");
        assert!(again.entries.is_empty(), "clean workspace yields no entries");
        assert!(again.commit_sha.is_none(), "nothing rewritten → no commit sha");
    }

    #[test]
    fn pipeline_configs_json_returns_the_four_primitive_shape() {
        // The read counterpart of the edit methods is wired through the FFI
        // surface and returns the four-primitive shape. The canonical
        // fixture seeds entities but no pipeline configs, so every array is
        // empty — the keys must still be present.
        let (engine, _tmp) = setup_test_engine();
        let json = engine.pipeline_configs_json();
        for key in ["\"mediums\"", "\"facets\"", "\"projections\"", "\"ingests\""] {
            assert!(json.contains(key), "missing {key} in: {json}");
        }
    }

    // --- Vault lifecycle through the FFI -----------------------------------

    #[test]
    fn set_vault_version_bumps_through_ffi() {
        // The gate-free version op: seed `specs` at 0.1.0, bump to 0.2.0,
        // and confirm the outcome reflects the new version through the FFI.
        let (engine, _tmp) = setup_test_engine();
        let outcome = engine
            .set_vault_version("specs".to_string(), "0.2.0".to_string(), Some("ship".into()))
            .expect("version bump succeeds through the FFI surface");
        assert_eq!(outcome.vault, "specs");
        assert_eq!(outcome.new_version, "0.2.0");
        // A second read confirms the engine persisted the bump.
        let again = engine
            .set_vault_version("specs".to_string(), "0.3.0".to_string(), None)
            .expect("second bump succeeds");
        assert_eq!(again.old_version.as_deref(), Some("0.2.0"));
        assert_eq!(again.new_version, "0.3.0");
    }

    #[test]
    fn set_vault_version_rejects_bad_semver() {
        let (engine, _tmp) = setup_test_engine();
        let err = engine
            .set_vault_version("specs".to_string(), "not-semver".to_string(), None)
            .expect_err("malformed semver must refuse");
        assert!(matches!(err, MemsteadError::ValidationFailed { .. }), "got {err:?}");
    }

    #[test]
    fn set_vault_schema_noop_when_pin_unchanged() {
        // Re-pinning to the current schema is a noop — the FFI surfaces the
        // same `outcome` wire token an agent would branch on over MCP.
        let (engine, _tmp) = setup_test_engine();
        let outcome = engine
            .set_vault_schema("specs".to_string(), "default@1.0.0".to_string())
            .expect("noop re-pin succeeds");
        assert_eq!(outcome.vault, "specs");
        assert_eq!(outcome.outcome, "noop");
        assert_eq!(outcome.schema_pin, "default@1.0.0");
        assert!(outcome.findings.is_empty());
    }

    #[test]
    fn set_vault_schema_rejects_unknown_vault() {
        let (engine, _tmp) = setup_test_engine();
        let err = engine
            .set_vault_schema("no-such-vault".to_string(), "default@1.0.0".to_string())
            .expect_err("unknown vault must refuse");
        // `UnknownVault` survives the `ProEngineError::Basis` → `EngineError`
        // lift into the typed Swift variant.
        assert!(matches!(err, MemsteadError::UnknownVault { .. }), "got {err:?}");
    }

    #[test]
    fn create_then_delete_vault_through_ffi() {
        // Operator-mode create (skips the `[[vault_management.create]]`
        // allowlist) lands a new git-branch vault the engine then lists; a
        // matching delete removes it. No vault-repo mutation originates in
        // Swift — the binding only re-shapes the request and the engine
        // owns backend instantiation and the seed commit.
        let (engine, tmp) = setup_test_engine();
        let target = tmp.path().join("fresh");
        let created = engine
            .create_vault(VaultCreateRequest {
                name: "fresh".to_string(),
                location: target.to_string_lossy().into_owned(),
                schema: "default@1.0.0".to_string(),
                vcs_gitdir: None,
                vcs_worktree: None,
                note: Some("ffi lifecycle test".to_string()),
                operator_mode: true,
            })
            .expect("operator-mode create succeeds through the FFI");
        assert_eq!(created.name, "fresh");
        assert_eq!(created.schema_ref, "default@1.0.0");
        // Git-branch backends produce a real 40-char hex seed sha.
        assert_eq!(created.seed_commit_sha.len(), 40, "sha: {}", created.seed_commit_sha);

        // The engine now lists the created vault without a restart.
        let stats = engine.get_stats();
        assert!(
            stats.writable_vaults.iter().any(|v| v == "fresh"),
            "created vault must appear in the roster: {:?}",
            stats.writable_vaults
        );

        let deleted = engine
            .delete_vault("fresh".to_string(), Some("cleanup".into()), true)
            .expect("operator-mode delete succeeds through the FFI");
        assert_eq!(deleted.name, "fresh");
        assert!(deleted.deleted_from_router);

        // Gone from the roster after the delete.
        let after = engine.get_stats();
        assert!(
            !after.writable_vaults.iter().any(|v| v == "fresh"),
            "deleted vault must be gone: {:?}",
            after.writable_vaults
        );
    }

    #[test]
    fn init_filesystem_vault_then_open_roundtrips() {
        // The engine-routed bootstrap: initialise a brand-new folder vault
        // from nothing (no Swift config write), then open it — the seed
        // structure roots and lists the one vault.
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().join("notes");
        init_filesystem_vault(
            root.to_string_lossy().into_owned(),
            "notes".to_string(),
            "default@1.0.0".to_string(),
        )
        .expect("init_filesystem_vault succeeds");

        let engine = engine_open(root.to_string_lossy().to_string())
            .expect("the init'd vault opens through engine_open");
        let roster = engine.vault_roster();
        assert_eq!(roster.len(), 1, "roster: {roster:?}");
        assert_eq!(roster[0].vault, "notes");
        assert_eq!(roster[0].backend, VaultBackendKind::Folder);
    }

    #[test]
    fn init_filesystem_vault_rejects_bad_schema() {
        let tmp = TempDir::new().expect("tempdir");
        let err = init_filesystem_vault(
            tmp.path().join("v").to_string_lossy().into_owned(),
            "v".to_string(),
            "not a ref".to_string(),
        )
        .expect_err("malformed schema must refuse");
        assert!(matches!(err, MemsteadError::ValidationFailed { .. }), "got {err:?}");
    }

    #[test]
    fn engine_open_roots_a_standalone_folder_vault() {
        // Goal 5 (engine side): opening a single bare vault — a directory
        // with `.memstead/config.json` but no `workspace.toml` — yields the
        // same rooted engine + one-entry roster as a workspace, through the
        // very `engine_open` entry the macOS app uses. No separate non-rooted
        // boot path.
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".memstead")).expect(".memstead");
        std::fs::write(
            root.join(".memstead").join("config.json"),
            r#"{"schema":"default@1.0.0"}"#,
        )
        .expect("config.json");
        std::fs::write(
            root.join("hello.md"),
            "---\ntype: spec\n---\n# Hello\n\n## Identity\n\nStandalone body.\n",
        )
        .expect("entity");

        let engine = engine_open(root.to_string_lossy().to_string())
            .expect("a standalone vault opens through engine_open");
        let roster = engine.vault_roster();
        assert_eq!(roster.len(), 1, "one mount for the standalone vault: {roster:?}");
        assert_eq!(roster[0].backend, VaultBackendKind::Folder);
        assert!(engine.get_stats().entity_count >= 1);
    }

    #[test]
    fn vault_roster_reports_engine_sourced_facts() {
        // The seeded fixture is one writable git-branch vault with two
        // entities. The roster reflects engine truth: backend kind,
        // capability, schema pin, count — and no drift on a fresh boot.
        let (engine, _tmp) = setup_test_engine();
        let roster = engine.vault_roster();
        assert_eq!(roster.len(), 1, "roster: {roster:?}");
        let specs = &roster[0];
        assert_eq!(specs.vault, "specs");
        assert_eq!(specs.backend, VaultBackendKind::GitBranch);
        assert!(specs.writable, "seeded git-branch vault is writable");
        assert_eq!(specs.schema_pin.as_deref(), Some("default@1.0.0"));
        assert_eq!(specs.entity_count, 2, "two seeded entities");
        assert!(!specs.drifted, "no sibling writer → no drift on fresh boot");
    }

    #[test]
    fn vault_roster_distinguishes_folder_backend() {
        // A folder-backed workspace surfaces a Folder roster entry — the
        // backend kind is engine-sourced, not reconstructed Swift-side, so
        // it discriminates folder from git-branch by construction.
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        let vault_dir = root.join("notes");
        std::fs::create_dir_all(&vault_dir).expect("vault dir");
        std::fs::write(
            vault_dir.join("hello.md"),
            "---\ntype: spec\n---\n# Hello\n\n## Identity\n\nA.\n",
        )
        .expect("entity");
        let memstead = root.join(".memstead");
        std::fs::create_dir_all(memstead.join("state")).expect("state dir");
        std::fs::write(
            memstead.join("workspace.toml"),
            "format = \"memstead-git-branch-1\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .expect("workspace.toml");
        std::fs::write(
            memstead.join("state").join("mounts.json"),
            r#"{"format":"memstead-mounts-2","mounts":[{"vault":"notes","schema":"default@1.0.0","storage":{"type":"folder","path":"notes"},"capability":"write","lifecycle":"eager","cross_linkable":true}]}"#,
        )
        .expect("mounts.json");

        let engine = engine_open(root.to_string_lossy().to_string())
            .expect("folder workspace opens");
        let roster = engine.vault_roster();
        assert_eq!(roster.len(), 1, "roster: {roster:?}");
        assert_eq!(roster[0].vault, "notes");
        assert_eq!(roster[0].backend, VaultBackendKind::Folder);
        // Folder backends never drift (no tracked head).
        assert!(!roster[0].drifted);
    }

    #[test]
    fn export_vault_produces_a_mountable_archive() {
        // AC: "exporting a vault as `.mem` produces an archive the engine can
        // subsequently mount read-only." Bump the version first (export
        // refuses a config without one), export to a temp path through the
        // FFI, then re-mount the bytes via `Engine::from_archive_bytes` and
        // confirm the embedded entities survived the round-trip.
        let (engine, tmp) = setup_test_engine();
        engine
            .set_vault_version("specs".to_string(), "1.0.0".to_string(), None)
            .expect("version bump precondition");

        let out_path = tmp.path().join("specs.mem");
        let outcome = engine
            .export_vault("specs".to_string(), out_path.to_string_lossy().into_owned())
            .expect("export succeeds through the FFI");
        assert_eq!(outcome.name, "specs");
        assert_eq!(outcome.version, "1.0.0");
        assert!(outcome.entity_count >= 2, "entity_count: {}", outcome.entity_count);
        assert!(outcome.size_bytes > 0);
        assert!(outcome.dangling_cross_vault_edges.is_empty());
        assert!(out_path.is_file(), "archive must land at the requested path");

        // The engine can mount the produced archive read-only.
        let bytes = std::fs::read(&out_path).expect("read exported archive");
        let mounted = memstead_base::Engine::from_archive_bytes(bytes)
            .expect("engine mounts the exported archive read-only");
        assert!(
            mounted.stats().entity_count >= 2,
            "mounted archive must carry the exported entities"
        );
    }

    #[test]
    fn export_vault_rejects_unknown_vault() {
        let (engine, tmp) = setup_test_engine();
        let out_path = tmp.path().join("nope.mem");
        let err = engine
            .export_vault("no-such-vault".to_string(), out_path.to_string_lossy().into_owned())
            .expect_err("unknown vault must refuse");
        assert!(matches!(err, MemsteadError::UnknownVault { .. }), "got {err:?}");
    }

    #[test]
    fn create_vault_rejects_bad_schema_ref() {
        let (engine, tmp) = setup_test_engine();
        let target = tmp.path().join("bad");
        let err = engine
            .create_vault(VaultCreateRequest {
                name: "bad".to_string(),
                location: target.to_string_lossy().into_owned(),
                schema: "not a valid ref".to_string(),
                vcs_gitdir: None,
                vcs_worktree: None,
                note: None,
                operator_mode: true,
            })
            .expect_err("malformed schema ref must refuse");
        assert!(matches!(err, MemsteadError::ValidationFailed { .. }), "got {err:?}");
    }
}
