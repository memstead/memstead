//! Export: the markdown and mem-archive writers, and the pipeline-edit provenance they record.

use super::*;

impl Engine {
    /// Regenerate entity markdown files from the in-memory store.
    ///
    /// Dispatch:
    /// - When `mem_filter` is `Some(name)`, only that mem's mount
    ///   is considered. If its active backend doesn't support markdown
    ///   regeneration in place (today: anything other than
    ///   `MountStorage::Folder`), the call refuses with
    ///   [`EngineError::MarkdownExportUnsupportedBackend`] carrying
    ///   the active backend's id and the supported-backend list.
    /// - When `mem_filter` is `None`, every mount is iterated.
    ///   Folder mounts regenerate as today; non-folder mounts are
    ///   recorded in [`crate::ops::ExportResult::skipped_mounts`] so
    ///   the caller can surface the partial-success shape.
    ///
    /// Per-folder-mount behaviour: iterate the store, regenerate each
    /// non-stub entity belonging to the mount's mem, compare to the
    /// on-disk file, write if changed.
    ///
    /// `schema_filter` narrows the per-entity-type subset: when
    /// `Some(name)`, only entities whose `entity_type` matches are
    /// regenerated. `None` exports every type.
    ///
    /// Pre-fix this returned
    /// `ExportResult { written: 0, unchanged: 0 }` for git-branch /
    /// archive mounts — a successful-looking no-op that masked the
    /// backend-incompatibility. The typed refusal (per-mem) and the
    /// `skipped_mounts` channel (workspace-wide) give the caller an
    /// agent-actionable signal in one round-trip.
    pub fn export_markdown(
        &self,
        mem_filter: Option<&str>,
        schema_filter: Option<&str>,
    ) -> Result<crate::ops::ExportResult, EngineError> {
        use crate::workspace::MountStorage;
        let fallback = engine_fallback_type();
        let supported_backends = vec!["folder".to_string()];

        if let Some(name) = mem_filter {
            let mount = self
                .mounts
                .iter()
                .find(|m| m.mount.mem == name)
                .ok_or_else(|| self.unknown_mem_error(name))?;
            if !matches!(mount.mount.storage, MountStorage::Folder { .. }) {
                return Err(EngineError::MarkdownExportUnsupportedBackend {
                    mem: name.to_string(),
                    active_backend: mount.mount.storage.backend_id().to_string(),
                    supported_backends,
                });
            }
        }

        let mut total_written = 0;
        let mut refused: Vec<crate::ops::RefusedEntity> = Vec::new();
        let mut total_unchanged = 0;
        let mut skipped_mounts: Vec<crate::ops::SkippedMount> = Vec::new();

        for mount in &self.mounts {
            let mem_name = mount.mount.mem.as_str();
            if let Some(filter) = mem_filter
                && mem_name != filter
            {
                continue;
            }
            let MountStorage::Folder { path: mem_dir } = &mount.mount.storage else {
                skipped_mounts.push(crate::ops::SkippedMount {
                    mem: mem_name.to_string(),
                    active_backend: mount.mount.storage.backend_id().to_string(),
                    reason: "backend_does_not_support_markdown_export".to_string(),
                });
                continue;
            };
            let schema = match self.schemas.get(mem_name) {
                Some(s) => s,
                None => continue,
            };

            for entity in self.store.all_entities() {
                if entity.stub || entity.file_path.is_empty() {
                    continue;
                }
                if entity.id.mem() != mem_name {
                    continue;
                }
                if let Some(filter) = schema_filter
                    && entity.entity_type != filter
                {
                    continue;
                }
                let type_def = schema
                    .get_type(&entity.entity_type)
                    .unwrap_or_else(|| fallback.clone());
                let generated = generate_markdown(entity, type_def.as_ref());

                let full_path = mem_dir.join(&entity.file_path);
                let needs_write = match std::fs::read_to_string(&full_path) {
                    Ok(existing) => existing != generated,
                    Err(_) => true,
                };
                if needs_write {
                    match crate::entity::writer::write_entity(entity, mem_dir, type_def.as_ref()) {
                        Ok(_) => total_written += 1,
                        // The one refusal the export must not swallow: writing
                        // this entity would bury the sections its open fence
                        // absorbed. Name it and carry on — an export that
                        // aborted here would strand every other entity.
                        Err(e @ crate::entity::writer::WriteError::UnterminatedFence { .. }) => {
                            refused.push(crate::ops::RefusedEntity {
                                id: entity.id.to_string(),
                                reason: "UNTERMINATED_FENCE_IN_STORED_BODY".to_string(),
                                detail: e.to_string(),
                            });
                        }
                        Err(_) => {}
                    }
                } else {
                    total_unchanged += 1;
                }
            }
        }

        Ok(crate::ops::ExportResult {
            refused_entities: refused,
            written: total_written,
            unchanged: total_unchanged,
            skipped_mounts,
        })
    }

    /// Export a mem as a portable `.mem` archive.
    ///
    /// Dispatch is internal: the engine looks up the mount whose mem
    /// name matches and branches on its `MountStorage`. Folder mounts
    /// produce a snapshot archive (current `.md` files + config);
    /// git-branch mounts invoke the registered [`GitBranchOps::export`]
    /// hook to produce a history archive (the per-mem branch tip's
    /// tree); archive mounts reject with `BackendError::Sealed`
    /// (already-an-archive — no meaningful re-export).
    ///
    /// The mem's `MemConfig` is looked up via
    /// [`Self::mem_config_for`]; unloaded configs (folder mounts
    /// without a `.memstead/config.json`, git-branch mounts without a
    /// `__MEMSTEAD:mems/<mem>/config.json`) surface as
    /// `EngineError::InvalidInput`. Workspace-level schema dir is
    /// threaded from `self.settings.schemas_dir` for the
    /// schema-source resolution chain.
    /// Resolve the mem's pinned schema from the workspace's
    /// `__MEMSTEAD:schemas/` ref (git-branch schema store) for the
    /// export paths of NON-git-branch mounts. `None` when the git-branch
    /// ops are not wired, the workspace has no mem-repo, or the
    /// package is not on the ref — callers then fall through to the
    /// disk/builtin chain unchanged. Without this, a folder mem whose
    /// schema `memstead schema install` sealed on the ref LOADS but
    /// cannot EXPORT: the loader and the archive assembler must read
    /// the same store.
    pub(crate) fn ref_schema_source_for(
        &self,
        config: &memstead_schema::MemConfig,
    ) -> Option<Vec<memstead_schema::SchemaSourceFile>> {
        let ops = self.git_branch_ops.as_ref()?;
        let root = self.workspace_root.as_deref()?;
        let pin = config.schema.as_ref()?;
        (ops.collect_ref_schema_source)(root, pin).ok().flatten()
    }

    pub fn export_mem(
        &self,
        mem_name: &str,
        output_path: &std::path::Path,
    ) -> Result<crate::ops::MemExportResult, EngineError> {
        let mount = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        let config = self.mem_config_for(mem_name).ok_or_else(|| {
            EngineError::InvalidInput(format!(
                "mem '{mem_name}' has no loaded MemConfig — cannot export"
            ))
        })?;
        // F1: surface the missing-version case as a typed
        // `MEM_CONFIG_INCOMPLETE` envelope with structured recovery
        // details, rather than letting it bubble through as the
        // backend's `INTERNAL` collapse pointing at the wrong path
        // (`.memstead/config.json` is the folder-backend layout — the
        // mem-repo backend keeps the blob under `__MEMSTEAD:mems/`).
        // The check fires for both backends symmetrically.
        if config.version.is_none() {
            return Err(EngineError::MemConfigIncomplete {
                mem: mem_name.to_string(),
                missing_fields: vec!["version".to_string()],
            });
        }
        // Collected before the backend split so both storage kinds report
        // it. `install` refuses the archive for each of these; naming them at
        // export time is the same courtesy the dangling cross-mem edges get,
        // and for the same reason: an operator who learns at install time has
        // already shared the archive.
        let fenced: Vec<String> = self
            .store
            .all_entities()
            .filter(|e| e.mem == mem_name && !e.stub)
            .filter(|e| {
                e.sections
                    .values()
                    .any(|v| crate::markdown::closing_fence_if_unterminated(v.trim()).is_some())
            })
            .map(|e| e.id.to_string())
            .collect();
        let workspace_root = self.workspace_root.as_deref();
        // Authored schemas live at the fixed `<workspace>/.memstead/schemas/`
        // location (the `schemas_dir` key is retired). Absent dir → the
        // schema-source chain falls through to cache/built-in, as before.
        let fixed_schemas_dir = workspace_root.map(|r| r.join(".memstead").join("schemas"));
        let workspace_schemas_dir = fixed_schemas_dir.as_deref();
        let exported = match &mount.mount.storage {
            MountStorage::Folder { path } => crate::ops::export::export_mem(
                path,
                config,
                output_path,
                workspace_root,
                workspace_schemas_dir,
                self.ref_schema_source_for(config),
            )
            .map_err(|e| EngineError::Backend(BackendError::Other(format!("export_mem: {e}")))),
            MountStorage::GitBranch { gitdir, branch } => {
                let hook = self.git_branch_ops.as_ref().ok_or_else(|| {
                    EngineError::Backend(BackendError::Other(
                        "git-branch export hook not installed (git-branch ops not wired)"
                            .to_string(),
                    ))
                })?;
                // Source per-entity provenance from the git-branch mutation
                // log (commit trailers) and hand the serialised payload to
                // the export hook to embed — symmetric with the bytes path.
                let entity_paths = crate::ops::export::entity_paths_of(
                    &mount
                        .backend
                        .list_entities()
                        .map_err(EngineError::Backend)?,
                );
                let records = mount.backend.read_provenance(None).unwrap_or_default();
                let (provenance, redactions) =
                    crate::ops::export::build_redacted_archive_provenance(&records, &entity_paths);
                let provenance_bytes = provenance.to_archive_bytes().ok();
                // Source the anchors sidecar from the branch tip — symmetric
                // with the bytes-export path so the disk `.mem` carries anchors.
                let anchors_bytes = mount.backend.read_anchors_sidecar().ok().flatten();
                (hook.export)(
                    gitdir,
                    branch,
                    mem_name,
                    config,
                    output_path,
                    workspace_root,
                    workspace_schemas_dir,
                    provenance_bytes.as_deref(),
                    anchors_bytes.as_deref(),
                )
                .map(|mut r| {
                    r.redactions = redactions;
                    r
                })
                .map_err(EngineError::Backend)
            }
            MountStorage::Archive { .. } => Err(EngineError::Backend(BackendError::Sealed)),
            // `.mem` export from an in-memory mem lands with the
            // writable-session-server plan (it needs a backend-level
            // archive builder); this plan adds the backend, not the
            // export path, so refuse explicitly rather than silently.
            MountStorage::InMemory => Err(EngineError::Backend(BackendError::Other(
                "export not yet supported for in-memory backend".to_string(),
            ))),
        };
        exported.map(|mut r| {
            r.unterminated_fence_entities = fenced;
            r
        })
    }

    /// Update a mem's `version` field in its per-mem config and
    /// persist it through the backend. Backend-symmetric: folder
    /// backends rewrite `.memstead/config.json`; git-branch backends
    /// commit `__MEMSTEAD:mems/<mem>/config.json`. Archive mounts
    /// reject with `BackendError::Sealed`.
    ///
    /// Returns the (mem, old_version, new_version) triple so
    /// callers can surface the change without an extra read. Reads
    /// the current value from the in-memory `MemConfig` and
    /// updates it on success, keeping the next call free of a
    /// stale-version read.
    ///
    /// `EngineError::UnknownMem` when the name resolves to no
    /// mount; `EngineError::ReadOnlyMount` when the mount is sealed
    /// for writes; `EngineError::InvalidInput` when the mount has no
    /// loaded `MemConfig` (folder mount with no
    /// `.memstead/config.json`; the residual missing-config path is
    /// distinct from the missing-version path). F1.
    /// Record pipeline-edit provenance through `mem`'s backend — the
    /// bridge the pipeline-edit block (outside the engine module) uses
    /// to reach a mount's backend. A mem that isn't currently mounted
    /// is a successful no-op: pipeline configs may reference unmounted
    /// mems, and provenance is recorded against the mounted set.
    pub fn record_pipeline_edit_provenance(
        &self,
        mem: &str,
        kind: &str,
        edits: &[(String, Option<Vec<u8>>)],
        note: Option<&str>,
        verb: &str,
    ) -> Result<(), crate::backend::BackendError> {
        let ctx =
            self.session_commit_context(Some("memstead_pipeline_edit"), note.map(String::from));
        match self.mounts.iter().find(|m| m.mount.mem == mem) {
            Some(m) => m.backend.record_pipeline_edit(kind, edits, &ctx, verb),
            None => Ok(()),
        }
    }
}
