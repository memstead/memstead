//! Full-flavour workspace boot helper.
//!
//! Provides [`engine_from_workspace_root`] — the full counterpart to
//! [`memstead_base::Engine::from_workspace_root`]. Loads the workspace via
//! [`memstead_base::FileWorkspaceStore`], hydrates read-only archive mounts
//! from each writable mem's `readMems` field, instantiates each
//! mount via [`crate::storage::instantiate_full_backend`] (which knows
//! how to materialise [`memstead_base::MountStorage::GitBranch`]), and
//! constructs the engine.

use std::path::Path;

use memstead_base::{
    BootError, Engine, FileWorkspaceStore, Mount, MemBackend, WorkspaceStoreAdapter,
    detect_layout,
};

fn hydrate_read_mems(
    writable_mounts: &[(Mount, Box<dyn MemBackend>)],
    writable_names: &std::collections::HashSet<String>,
) -> Result<Vec<(Mount, Box<dyn MemBackend>)>, BootError> {
    let cache_dir = crate::mem_cache::mem_cache_dir();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut extras: Vec<(Mount, Box<dyn MemBackend>)> = Vec::new();
    for (_, backend) in writable_mounts {
        let bytes = match backend.read_mem_config() {
            Ok(Some(b)) => b,
            _ => continue,
        };
        let value: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let config = match memstead_schema::config::parse_mem_config(&value) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for (mem_name, spec) in &config.read_mems {
            if writable_names.contains(mem_name) {
                continue;
            }
            if !seen.insert(mem_name.clone()) {
                continue;
            }
            // Content-addressed cache file:
            // `<name>-<cacheKey>.mem` when the registration carries a
            // `cacheKey`, else the bare `<name>.mem` for registrations
            // written before content-addressing.
            let stem = match spec.cache_key.as_deref() {
                Some(key) => format!("{mem_name}-{key}"),
                None => mem_name.clone(),
            };
            let archive_path = std::iter::once(memstead_schema::ARCHIVE_EXTENSION)
                .map(|ext| cache_dir.join(format!("{stem}.{ext}")))
                .find(|p| p.is_file());
            let Some(archive_path) = archive_path else {
                continue;
            };
            // Read the archive's actual schema pin from its
            // bundled `.memstead/config.json` instead of hardcoding
            // `default@1.0.0`. The pre-fix path silently lied to the
            // cross-mem validator about every RO mount's schema, so
            // `memstead relate` against an RO-mounted entity saw the engine
            // default and refused with `CROSS_MEM_EDGE_NOT_DECLARED`
            // even when the archive's actual schema admitted the edge.
            // `read_published_config` is the cheap path — it pulls the
            // single published-config entry from the zip without
            // re-running full archive validation (already enforced at
            // install time). If the read fails the archive is broken
            // beyond recovery; fall back to the engine default so the
            // workspace still boots and the failure is visible via the
            // RO mount's degraded surface (validators will refuse with
            // the typed shape; the operator reinstalls to recover).
            let archive_schema = crate::mem_cache::read_published_config(&archive_path)
                .map(|cfg| cfg.schema)
                .unwrap_or_else(|_| {
                    memstead_schema::SchemaRef::new("default", semver::Version::new(1, 0, 0))
                });
            let read_mount = Mount {
                mem: mem_name.clone(),
                schema: Some(archive_schema),
                storage: memstead_base::MountStorage::Archive {
                    path: archive_path.clone(),
                },
                capability: memstead_base::MountCapability::ReadOnly,
                lifecycle: memstead_base::MountLifecycle::Eager,
                cross_linkable: false,
            migration_target: None,
        };
            let backend: Box<dyn MemBackend> =
                Box::new(memstead_base::storage::ArchiveBackend::new(archive_path));
            extras.push((read_mount, backend));
        }
    }
    Ok(extras)
}

pub fn engine_from_workspace_root(workspace_root: &Path) -> Result<Engine, BootError> {
    let workspace = match detect_layout(workspace_root) {
        // Standalone collapse: a bare folder mem (`.memstead/config.json`,
        // no `workspace.toml`) roots as a one-mount workspace. The macOS app
        // boots through this full entry, so the unified lone-mem experience
        // must hold here too, not only in the lean boot path.
        memstead_base::Layout::Empty => {
            match memstead_base::standalone_workspace(workspace_root) {
                Some(ws) => ws,
                None => {
                    return Err(BootError::NotInitialised(workspace_root.to_path_buf()));
                }
            }
        }
        memstead_base::Layout::New => FileWorkspaceStore::new().load(workspace_root)?,
    };

    let settings = workspace.settings.clone();
    let writable_names: std::collections::HashSet<String> = workspace
        .mounts
        .iter()
        .map(|m| m.mem.clone())
        .collect();
    let mut mounts: Vec<(Mount, Box<dyn MemBackend>)> =
        Vec::with_capacity(workspace.mounts.len());
    for mount in workspace.mounts {
        let backend = crate::storage::instantiate_full_backend(&mount)?;
        mounts.push((mount, backend));
    }
    // Read-mem hydration. For each writable mount, read its
    // `__MEMSTEAD:mem.config.json` and attach each `readMems` entry as
    // a read-only `ArchiveBackend` pointing at the global mem cache.
    let extra_read_mounts = hydrate_read_mems(&mounts, &writable_names)?;
    mounts.extend(extra_read_mounts);
    // Read authored schemas off the git-branch `__MEMSTEAD:schemas/` ref
    // (empty for a fresh, legacy `__SCHEMAS`-only, or pre-migration
    // workspace) and overlay them, so a schema installed onto the ref by
    // `memstead schema install` is resolvable at boot — folder schemas
    // alone (`from_mounts_with_schemas_dir`) never covered the ref.
    use memstead_base::schema_source::SchemaSource as _;
    let ref_schemas =
        match crate::mem_repo_schemas::GitBranchSchemaSource::for_workspace(workspace_root)
            .read_schemas()
        {
            Ok(schemas) => schemas,
            // Best-effort overlay: a stub/invalid `mem-repo/.git` or a
            // transient gix read failure must not brick boot — fall back
            // to built-ins (the pre-overlay behaviour) and warn.
            Err(e) => {
                tracing::warn!(
                    "could not read schemas from `__MEMSTEAD:schemas/` ref at {}: {e}; \
                     resolving against built-ins only",
                    workspace_root.display()
                );
                Vec::new()
            }
        };
    // Folder mounts in a full workspace read authored schemas from the
    // fixed `<workspace>/.memstead/schemas/` location (the `schemas_dir`
    // key is retired). Git-branch mounts get their schemas from the
    // `__MEMSTEAD:schemas/` ref (`ref_schemas` above); the folder dir is
    // typically absent in a git-branch workspace → a no-op overlay.
    let fixed_schemas_dir = workspace_root.join(".memstead").join("schemas");
    let mut engine = Engine::from_mounts_with_schemas_dir_and_extra(
        mounts,
        Some(fixed_schemas_dir.as_path()),
        ref_schemas,
    )?;
    engine.set_settings(settings);
    engine.set_workspace_root(workspace_root.to_path_buf());
    engine.set_backend_factory(crate::storage::instantiate_full_backend);
    engine.set_git_branch_ops(crate::storage::FULL_GIT_BRANCH_OPS);
    // Load the workspace store's pipeline configs (Medium / Facet /
    // Projection / Ingest) into the read-only queryable surface, matching
    // the lean boot path. A malformed config surfaces a typed parse error.
    // Pipeline configs from the workspace store — the legacy folders are no
    // longer read at boot (the compat shim retired with the 2026-06-14 bundled
    // migration; `memstead pipeline migrate` is the only path from old-shape
    // configs).
    engine.set_pipeline_configs(memstead_base::load_pipeline_configs(workspace_root)?);
    // Publish the authoring meta-schemas into `.memstead/meta-schemas/`
    // (best-effort) so editors validate authored schema YAML.
    let _ = memstead_schema::meta_schema::publish_meta_schemas(workspace_root);
    Ok(engine)
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn engine_from_workspace_root_errors_for_empty_layout() {
        let tmp = TempDir::new().unwrap();
        let err = engine_from_workspace_root(tmp.path()).unwrap_err();
        assert!(matches!(err, BootError::NotInitialised(_)));
    }

    /// A schema installed onto the `__MEMSTEAD:schemas/` ref overlays
    /// into the engine's resolution catalogue at boot — so a mem can
    /// pin a git-branch-installed (non-built-in) schema. Regression for
    /// the gap where the full boot read folder schemas only and never the
    /// ref, leaving ref-installed schemas unresolvable.
    #[test]
    fn engine_from_workspace_root_overlays_ref_schemas() {
        use memstead_base::schema_source::SchemaSource as _;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        let gitdir = root.join("mem-repo").join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        gix::init_bare(&gitdir).unwrap();

        let manifest = br#"name: refsrc
version: 0.1.0
description: A ref-installed (non-built-in) schema.
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
        let doc = br#"name: doc
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
        crate::mem_repo_schemas::GitBranchSchemaSource::for_workspace(root)
            .write_schema(
                "refsrc",
                "0.1.0",
                &[
                    ("schema.yaml".to_string(), manifest.to_vec()),
                    ("types/doc.yaml".to_string(), doc.to_vec()),
                ],
            )
            .unwrap();

        let engine = engine_from_workspace_root(root).unwrap();
        assert!(
            engine
                .workspace_schemas()
                .iter()
                .any(|s| s.manifest.name == "refsrc"),
            "ref-installed schema must overlay into the catalogue: {:?}",
            engine
                .workspace_schemas()
                .iter()
                .map(|s| s.manifest.name.clone())
                .collect::<Vec<_>>()
        );
    }
}
