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
    BootError, Engine, FileWorkspaceStore, MemBackend, Mount, WorkspaceStoreAdapter, detect_layout,
};

/// A mount paired with its instantiated backend — the unit the boot
/// pipeline hands to [`Engine`] construction.
type MountedBackend = (Mount, Box<dyn MemBackend>);

/// Outcome of the one-way legacy `readMems` boot migration.
struct LegacyReadMemMigration {
    /// The migrated mounts, ready to join the workspace roster.
    mounts: Vec<MountedBackend>,
    /// Read-mem names that were migrated (mount + config rewrite).
    migrated_mems: Vec<String>,
    /// Writable mems whose configs carried the legacy entries.
    from_host_mems: Vec<String>,
}

/// One-way boot migration: legacy per-host-mem `readMems` config
/// entries become workspace-level read-only mounts. For every entry
/// that resolves to a cache file, the mount is synthesised exactly as
/// the historical hydration did, and the entry is REMOVED from the
/// host mem's config through the engine-owned backend writer — so a
/// second boot finds no key and stays silent. Entries whose cache
/// file is missing stay in the config (visible, retried next boot)
/// rather than being dropped into nothing. Names already present in
/// the workspace mount roster are treated as migrated (the mount
/// exists; only the legacy key is cleaned up).
fn migrate_legacy_read_mems(
    writable_mounts: &[MountedBackend],
    writable_names: &std::collections::HashSet<String>,
    already_mounted: &std::collections::HashSet<String>,
) -> Result<LegacyReadMemMigration, BootError> {
    let cache_dir = crate::mem_cache::mem_cache_dir();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut extras: Vec<MountedBackend> = Vec::new();
    let mut migrated_mems: Vec<String> = Vec::new();
    let mut from_host_mems: Vec<String> = Vec::new();
    for (host_mount, backend) in writable_mounts {
        let bytes = match backend.read_mem_config() {
            Ok(Some(b)) => b,
            _ => continue,
        };
        let value: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let mut config = match memstead_schema::config::parse_mem_config(&value) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if config.read_mems.is_empty() {
            continue;
        }
        let mut retained: std::collections::BTreeMap<String, memstead_schema::config::ReadMemSpec> =
            Default::default();
        let mut host_migrated_any = false;
        for (mem_name, spec) in &config.read_mems {
            if writable_names.contains(mem_name) {
                // Shadowed by a writable mount — historically skipped;
                // the entry is dead weight either way. Drop it.
                host_migrated_any = true;
                continue;
            }
            if already_mounted.contains(mem_name) || !seen.insert(mem_name.clone()) {
                // Already a workspace mount (or migrated from an
                // earlier host in this same pass) — clean up the key.
                host_migrated_any = true;
                if !migrated_mems.contains(mem_name) {
                    migrated_mems.push(mem_name.clone());
                }
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
                // Cache file missing — keep the entry so the reference
                // stays visible (and the migration retries next boot)
                // instead of silently vanishing.
                retained.insert(mem_name.clone(), spec.clone());
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
            let ro_backend: Box<dyn MemBackend> =
                Box::new(memstead_base::storage::ArchiveBackend::new(archive_path));
            extras.push((read_mount, ro_backend));
            migrated_mems.push(mem_name.clone());
            host_migrated_any = true;
        }

        if host_migrated_any {
            // Rewrite the host config with only the retained (still
            // unresolvable) entries — engine-owned writer, one commit.
            config.read_mems = retained;
            if let Ok(mut out) = serde_json::to_vec_pretty(&config) {
                out.push(b'\n');
                if let Err(e) = backend.write_mem_config(&out) {
                    tracing::warn!(
                        mem = %host_mount.mem,
                        error = %e,
                        "readMems migration: mounts were created but the legacy \
                         key could not be removed from the host config — the \
                         migration warning will repeat next boot"
                    );
                }
            }
            from_host_mems.push(host_mount.mem.clone());
        }
    }
    Ok(LegacyReadMemMigration {
        mounts: extras,
        migrated_mems,
        from_host_mems,
    })
}

/// Load the workspace description (mount roster + settings) for a
/// root, without instantiating backends, resolving schemas, or loading
/// entities. The shared first step of [`engine_from_workspace_root`]
/// and the below-boot repair surface ([`crate::repair`]) — one loader,
/// so repair sees exactly the workspace the boot would.
pub(crate) fn load_workspace_description(
    workspace_root: &Path,
) -> Result<memstead_base::Workspace, BootError> {
    match detect_layout(workspace_root) {
        // Standalone collapse: a bare folder mem (`.memstead/config.json`,
        // no `workspace.toml`) roots as a one-mount workspace. Full-flavour
        // embedders boot through this entry, so the unified lone-mem
        // experience must hold here too, not only in the lean boot path.
        memstead_base::Layout::Empty => match memstead_base::standalone_workspace(workspace_root) {
            Some(ws) => Ok(ws),
            None => Err(BootError::NotInitialised(workspace_root.to_path_buf())),
        },
        memstead_base::Layout::New => Ok(FileWorkspaceStore::new().load(workspace_root)?),
    }
}

pub fn engine_from_workspace_root(workspace_root: &Path) -> Result<Engine, BootError> {
    let workspace = load_workspace_description(workspace_root)?;

    let settings = workspace.settings.clone();
    // The shadow set is WRITABLE mounts only — archive read-mounts in
    // the roster must not shadow-refuse their own migration cleanup.
    let writable_names: std::collections::HashSet<String> = workspace
        .mounts
        .iter()
        .filter(|m| m.capability == memstead_base::MountCapability::Write)
        .map(|m| m.mem.clone())
        .collect();
    let all_mounted_names: std::collections::HashSet<String> =
        workspace.mounts.iter().map(|m| m.mem.clone()).collect();
    let mut mounts: Vec<(Mount, Box<dyn MemBackend>)> = Vec::with_capacity(workspace.mounts.len());
    // Backend-instantiation failures quarantine the mem instead of
    // failing the workspace (degrade, never disappear); the entries
    // land on the engine's quarantine roster after construction.
    let mut instantiate_quarantine: Vec<memstead_base::engine::QuarantinedMem> = Vec::new();
    for mount in workspace.mounts {
        match crate::storage::instantiate_full_backend(&mount) {
            Ok(backend) => mounts.push((mount, backend)),
            Err(e) => instantiate_quarantine.push(memstead_base::engine::QuarantinedMem {
                reason_code: e.code().to_string(),
                reason_message: e.to_string(),
                mount,
            }),
        }
    }
    // One-way legacy migration: per-host-mem `readMems` entries become
    // workspace-level read-only mounts (registered in the engine-managed
    // mount state), the legacy keys are removed through the engine's own
    // config writers, and one warning names what moved. A second boot
    // finds no key and is silent.
    let migration = migrate_legacy_read_mems(&mounts, &writable_names, &all_mounted_names)?;
    let migration_happened = !migration.migrated_mems.is_empty();
    if !migration.mounts.is_empty() {
        mounts.extend(migration.mounts);
        // Persist the migrated mounts so the next boot loads them from
        // the mount state directly.
        let persisted = memstead_base::Workspace {
            mounts: mounts.iter().map(|(m, _)| m.clone()).collect(),
            settings: settings.clone(),
        };
        use memstead_base::workspace_store::WorkspaceStoreAdapter as _;
        if let Err(e) = FileWorkspaceStore::new().save_state(workspace_root, &persisted) {
            tracing::warn!(
                error = %e,
                "readMems migration: mount-state persistence failed — the \
                 migrated mounts serve this boot but the next boot repeats \
                 the migration"
            );
        }
    }
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
    // Root is known here, so an unresolved pin can be enriched with
    // the never-installed-package hint before it surfaces.
    let mut engine = Engine::from_mounts_with_schemas_dir_and_extra(
        mounts,
        Some(fixed_schemas_dir.as_path()),
        ref_schemas,
    )
    .map_err(|e| e.with_schema_install_probe(Some(workspace_root)))?;
    engine.extend_quarantine(instantiate_quarantine);
    engine.set_settings(settings);
    engine.set_workspace_root(workspace_root.to_path_buf());
    engine.set_backend_factory(crate::storage::instantiate_full_backend);
    engine.set_git_branch_ops(crate::storage::FULL_GIT_BRANCH_OPS);
    // Unmounted-mem storage discovery (flywheel W7/02): a write that
    // references a mem with NO mount record can still be verified
    // against the mem-repo's branch tree — the workspace layer owns
    // the branch convention memstead-base cannot see. The prober
    // resolves the mem's content branch (hierarchical paths included,
    // `main`/`__*` registry refs filtered), hands back a transient
    // git-tree backend over it, and reads the stored config's schema
    // pin so cross-schema edge routing keeps its authority without a
    // mount. Silent `None` on any miss — the forward-reference
    // mechanic then governs, unchanged.
    {
        let gitdir = workspace_root.join("mem-repo").join(".git");
        let gitdir = gitdir.canonicalize().unwrap_or(gitdir);
        engine.set_unmounted_storage_prober(Box::new(move |mem: &str| {
            if !gitdir.is_dir() {
                return None;
            }
            let branch_path =
                crate::mem_repo_config::resolve_full_path_at_gitdir(&gitdir, mem).ok()??;
            let backend = crate::storage::git_tree::GitTreeMemWriter::new(
                gitdir.clone(),
                format!("refs/heads/{branch_path}"),
            );
            let schema = memstead_base::MemBackend::read_mem_config(&backend)
                .ok()
                .flatten()
                .and_then(|bytes| {
                    serde_json::from_slice::<memstead_schema::config::MemConfig>(&bytes).ok()
                })
                .and_then(|cfg| cfg.schema);
            Some(memstead_base::engine::UnmountedMemStorage {
                backend: Box::new(backend),
                schema,
            })
        }));
    }
    // Load the workspace store's pipeline configs — the v2 single-record
    // binding store — into the read-only queryable surface, matching the
    // lean boot path. A malformed config surfaces a typed parse error; a
    // pre-v2 store refuses boot with the migrate-naming error (`memstead
    // projection migrate` is the only path from old-shape configs).
    engine.set_pipeline_configs(memstead_base::load_pipeline_configs(workspace_root)?);
    if migration_happened {
        engine.push_load_warning(memstead_base::ops::WarningHint::ReadMemsMigratedToMounts {
            mems: migration.migrated_mems,
            from_host_mems: migration.from_host_mems,
        });
    }
    // The authoring meta-schemas are NOT published here — see the note in
    // `memstead_base::engine::boot`. Publishing them at load made every read
    // of a mem write to the directory it read; it now happens in the
    // schema-authoring commands instead.
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

    /// Unmounted-mem storage discovery (flywheel W7/02): a relate into
    /// a mem with NO mount record but a real content branch in the
    /// mem-repo is verified against the branch tree — admitted with a
    /// LoadTime stub and no auto-stub/mem-uncreated warnings when the
    /// entity exists, and falling back to today's forward-reference
    /// mechanic (stub + both warnings) when it does not or when no
    /// branch exists at all.
    #[test]
    fn relate_into_unmounted_mem_verifies_against_branch_tree() {
        use memstead_base::engine::RelateEntityArgs;
        use memstead_base::ops::WarningHint;
        use memstead_base::storage::MemWriter;
        use memstead_base::vcs::{Actor, ClientId, CommitContext};

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".memstead").join("state")).unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            concat!(
                "format = \"memstead-git-branch-2\"\n\n",
                "[persistence_adapter]\nname = \"file-two-layer\"\n\n",
                "[cross_mem_links]\nsrc = [\"far\", \"nowhere\"]\n",
            ),
        )
        .unwrap();
        let gitdir = root.join("mem-repo").join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        gix::init_bare(&gitdir).unwrap();
        let gitdir = std::fs::canonicalize(&gitdir).unwrap();

        // Mounted source mem: a folder mount with one entity.
        let src_dir = root.join("src-mem");
        std::fs::create_dir_all(src_dir.join(".memstead")).unwrap();
        std::fs::write(
            src_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            src_dir.join("alpha.md"),
            "---
type: spec
---
# Alpha

## Identity

Body.
",
        )
        .unwrap();
        std::fs::write(
            root.join(".memstead").join("state").join("mounts.json"),
            format!(
                r#"{{"format":"memstead-mounts-3","mounts":[{{"mem":"src","schema":"default@1.0.0","storage":{{"type":"folder","path":{}}},"capability":"write","lifecycle":"eager","cross_linkable":true}}]}}"#,
                serde_json::to_string(&src_dir).unwrap()
            ),
        )
        .unwrap();

        // UNMOUNTED mem "far": a real content branch, no mounts row.
        let far = crate::storage::git_tree::GitTreeMemWriter::new(
            gitdir.clone(),
            "refs/heads/far".to_string(),
        );
        MemWriter::write_entity(
            &far,
            std::path::Path::new("topic.md"),
            b"---
type: spec
---
# Topic

## Identity

Body.
",
        )
        .unwrap();
        MemWriter::commit(
            &far,
            "seed far",
            &CommitContext {
                actor: Actor::Cli,
                client: Some(ClientId {
                    name: "test".to_string(),
                    version: "0".to_string(),
                }),
                tool: Some("test"),
                note: None,
                role: Default::default(),
                logical_operation_id: None,
                entity_ids: None,
            },
        )
        .unwrap();

        let mut engine = engine_from_workspace_root(root).unwrap();
        let relate = |engine: &mut Engine, mem: &str, to: &str| {
            engine.relate_entity(
                RelateEntityArgs {
                    source: memstead_base::EntityId::new("src", "alpha"),
                    expected_hash: None,
                    rel_type: "SUPPORTS".to_string(),
                    target: memstead_base::EntityId::new(mem, to),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                Actor::Cli,
                None,
                None,
            )
        };

        // Verified: the branch tree holds topic.md.
        let outcome =
            relate(&mut engine, "far", "topic").expect("verified unmounted target admits");
        assert!(
            !outcome.warnings.iter().any(|w| matches!(
                w,
                WarningHint::AutoStubCreated { .. }
                    | WarningHint::CrossMemTargetMemUncreated { .. }
            )),
            "a branch-verified target is neither an auto-stub case nor an uncreated mem: {:?}",
            outcome.warnings
        );
        let stub = engine
            .store()
            .get(&memstead_base::EntityId::new("far", "topic"))
            .expect("until-load stub present");
        assert_eq!(
            stub.stub_kind,
            Some(memstead_base::entity::StubKind::LoadTime)
        );
        assert!(
            engine.mount("far").is_none(),
            "verification never adds a mount — the twenty-mems case pays a tree lookup, not a workspace-shape change"
        );

        // Absent from the branch: forward-reference mechanic, both warnings.
        let outcome = relate(&mut engine, "far", "missing").expect("absent target auto-stubs");
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::AutoStubCreated { .. })),
            "absent target keeps the auto-stub warning: {:?}",
            outcome.warnings
        );

        // No branch at all: mechanic untouched, both warnings.
        let outcome =
            relate(&mut engine, "nowhere", "thing").expect("undiscoverable mem auto-stubs");
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::AutoStubCreated { .. }))
                && outcome
                    .warnings
                    .iter()
                    .any(|w| matches!(w, WarningHint::CrossMemTargetMemUncreated { .. })),
            "no discoverable storage keeps today's stub + warnings: {:?}",
            outcome.warnings
        );
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
no_self_loop_relationships: []
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
