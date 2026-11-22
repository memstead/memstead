//! Git-branch workspace boot helper.
//!
//! Provides [`engine_from_workspace_root`] — the git-branch counterpart to
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
                // Boot-time bookkeeping: no session, no caller; the
                // context records that (unspecified role, no identity).
                let ctx = memstead_base::vcs::CommitContext::new(
                    Some("readMems migration"),
                    memstead_base::vcs::Actor::Agent,
                    None,
                    None,
                    memstead_base::vcs::Role::Unspecified,
                    None,
                );
                if let Err(e) = backend.write_mem_config(&out, &ctx) {
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
    // A cached-archive roster entry resolves against this machine's
    // archive cache, which this crate owns.
    memstead_base::workspace_store::set_mem_cache_dir_provider(crate::mem_cache::mem_cache_dir);
    match detect_layout(workspace_root) {
        // Standalone collapse: a bare folder mem (`.memstead/config.json`,
        // no `workspace.toml`) roots as a one-mount workspace. The product
        // binaries boot through this entry, so the unified lone-mem
        // experience must hold here too, not only in the folder boot path.
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
            Ok(backend) => {
                // Before the first load: a load waits for the mem's
                // write lock no longer than the workspace says.
                backend.set_lock_wait(settings.mutations.write_lock_timeout());
                mounts.push((mount, backend))
            }
            Err(e) => {
                let (reason_code, reason_message) =
                    memstead_base::engine::boot::instantiate_failure_reason(
                        &mount,
                        e.code(),
                        e.to_string(),
                    );
                instantiate_quarantine.push(memstead_base::engine::QuarantinedMem {
                    reason_code,
                    reason_message,
                    mount,
                })
            }
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
    // Read authored schemas off the `__MEMSTEAD:schemas/` registries
    // (empty for a fresh or pre-migration workspace) and overlay them,
    // so a schema installed onto the ref by `memstead schema install`
    // is resolvable at boot. The registries are the git directories the
    // git-branch mounts name plus the workspace's own mem-repo: a mem
    // mounted from another workspace's mem-repo resolves its pin from
    // the repository that holds it. Quarantined mounts count, their
    // repository may be the one that carries the schema.
    let ref_schemas = crate::mem_repo_schemas::read_workspace_ref_schemas(
        workspace_root,
        mounts
            .iter()
            .map(|(m, _)| m)
            .chain(instantiate_quarantine.iter().map(|q| &q.mount)),
    );
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
    // Unmounted-mem storage discovery: a write that
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
            let backend = crate::storage::git_tree::GitTreeBackend::new(
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
    // folder boot path. A malformed config surfaces a typed parse error; a
    // retired-format file quarantines its binding.
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

    /// Unmounted-mem storage discovery: a relate into
    /// a mem with NO mount record but a real content branch in the
    /// mem-repo is verified against the branch tree — admitted with a
    /// LoadTime stub and no auto-stub/mem-uncreated warnings when the
    /// entity exists, and falling back to today's forward-reference
    /// mechanic (stub + both warnings) when it does not or when no
    /// branch exists at all.
    #[test]
    fn relate_into_unmounted_mem_verifies_against_branch_tree() {
        use memstead_base::backend::MemBackend;
        use memstead_base::engine::RelateEntityArgs;
        use memstead_base::ops::WarningHint;
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
        let far = crate::storage::git_tree::GitTreeBackend::new(
            gitdir.clone(),
            "refs/heads/far".to_string(),
        );
        MemBackend::write_entity(
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
        MemBackend::commit(
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
                identity: None,
                logical_operation_id: None,
                entity_ids: None,
                proposal: None,
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

    /// A small schema no built-in carries, for the ref-registry tests.
    const REFSRC_MANIFEST: &[u8] = br#"name: refsrc
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

    const REFSRC_DOC: &[u8] = br#"name: doc
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

        crate::mem_repo_schemas::GitBranchSchemaSource::for_workspace(root)
            .write_schema(
                "refsrc",
                "0.1.0",
                &[
                    ("schema.yaml".to_string(), REFSRC_MANIFEST.to_vec()),
                    ("types/doc.yaml".to_string(), REFSRC_DOC.to_vec()),
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

    /// A workspace that holds engine state only (no `mem-repo/` of its
    /// own) and mounts a mem that lives in another workspace's mem-repo
    /// resolves the mem's pin from the registry of the repository the
    /// mount names: warm, once the schema is installed there out of
    /// band, and cold, at the next boot. Before the install the mem
    /// quarantines with the ordinary not-found reason.
    #[test]
    fn a_mount_of_another_mem_repo_resolves_its_schema_from_that_repo() {
        use memstead_base::schema_source::SchemaSource as _;
        use memstead_base::workspace_store::WorkspaceStoreAdapter as _;

        let tmp = TempDir::new().unwrap();
        let holder = tmp.path().join("holder");
        let view = tmp.path().join("view");

        // The holding workspace: a real mem-repo with one mem pinned to
        // a schema that is not installed yet.
        crate::test_support::init_real_mem_repo(&holder, &[("notes", "refsrc@0.1.0")]);
        let holder_gitdir = std::fs::canonicalize(holder.join("mem-repo").join(".git")).unwrap();

        // The mounting workspace: engine state only.
        std::fs::create_dir_all(view.join(".memstead")).unwrap();
        std::fs::write(
            view.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        FileWorkspaceStore::new()
            .save_state(
                &view,
                &memstead_base::Workspace {
                    mounts: vec![Mount {
                        mem: "notes".to_string(),
                        schema: Some("refsrc@0.1.0".parse().unwrap()),
                        storage: memstead_base::MountStorage::GitBranch {
                            gitdir: holder_gitdir.clone(),
                            branch: "notes".to_string(),
                        },
                        capability: memstead_base::MountCapability::ReadOnly,
                        lifecycle: memstead_base::MountLifecycle::Eager,
                        cross_linkable: true,
                        migration_target: None,
                    }],
                    settings: Default::default(),
                },
            )
            .unwrap();
        assert!(!view.join("mem-repo").exists());

        // Not installed anywhere: quarantined, not found.
        let mut engine = engine_from_workspace_root(&view).unwrap();
        let reasons: Vec<&str> = engine
            .quarantined_mems()
            .iter()
            .map(|q| q.reason_code.as_str())
            .collect();
        assert_eq!(reasons, ["SCHEMA_NOT_FOUND"]);

        // Installed into the repository that holds the mem.
        crate::mem_repo_schemas::GitBranchSchemaSource::for_gitdir(&holder_gitdir)
            .write_schema(
                "refsrc",
                "0.1.0",
                &[
                    ("schema.yaml".to_string(), REFSRC_MANIFEST.to_vec()),
                    ("types/doc.yaml".to_string(), REFSRC_DOC.to_vec()),
                ],
            )
            .unwrap();

        // Warm: a full refresh of the running engine finds it.
        let report = engine.full_refresh();
        assert_eq!(
            report.schemas_added,
            vec!["refsrc@0.1.0".to_string()],
            "failures: {:?}",
            report.failures
        );

        // Cold: the next boot serves the mem.
        let engine = engine_from_workspace_root(&view).unwrap();
        assert!(
            engine.quarantined_mems().is_empty(),
            "quarantined: {:?}",
            engine
                .quarantined_mems()
                .iter()
                .map(|q| (&q.mount.mem, &q.reason_message))
                .collect::<Vec<_>>()
        );
        assert_eq!(engine.mem_names(), ["notes"]);
    }

    /// A running engine follows a read-only git-branch row another
    /// process adds to its roster or withdraws from it: the mem is
    /// served from the next operation on, refuses writes, and stops
    /// being served once the row is gone, without a restart.
    #[test]
    fn a_running_engine_follows_a_read_only_git_branch_row() {
        use memstead_base::workspace_store::WorkspaceStoreAdapter as _;

        let tmp = TempDir::new().unwrap();
        let holder = tmp.path().join("holder");
        let view = tmp.path().join("view");

        // The holding workspace: one mem with one entity.
        crate::test_support::init_real_mem_repo(&holder, &[("notes", "default@1.0.0")]);
        let holder_gitdir = std::fs::canonicalize(holder.join("mem-repo").join(".git")).unwrap();
        let args = |mem: &str, title: &str| memstead_base::engine::CreateEntityArgs {
            anchors: Vec::new(),
            mem: mem.to_string(),
            title: title.to_string(),
            entity_type: "spec".to_string(),
            sections: indexmap::IndexMap::from_iter([
                ("identity".to_string(), "i".to_string()),
                ("purpose".to_string(), "p".to_string()),
            ]),
            metadata: indexmap::IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        };
        let ctx = memstead_base::vcs::CommitContext::internal();
        let mut holding = engine_from_workspace_root(&holder).unwrap();
        holding
            .create_entity_with_ctx(args("notes", "Held"), &ctx)
            .expect("the holding workspace writes its mem");

        // The mounting workspace: engine state only, nothing mounted yet.
        std::fs::create_dir_all(view.join(".memstead")).unwrap();
        std::fs::write(
            view.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        let save = |mounts: Vec<Mount>| {
            FileWorkspaceStore::new()
                .save_state(
                    &view,
                    &memstead_base::Workspace {
                        mounts,
                        settings: Default::default(),
                    },
                )
                .unwrap();
        };
        save(Vec::new());
        let mut engine = engine_from_workspace_root(&view).unwrap();
        assert!(engine.mem_names().is_empty());

        // Another process grants the read.
        save(vec![Mount {
            mem: "notes".to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: memstead_base::MountStorage::GitBranch {
                gitdir: holder_gitdir.clone(),
                branch: "notes".to_string(),
            },
            capability: memstead_base::MountCapability::ReadOnly,
            lifecycle: memstead_base::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        }]);
        engine.reload_if_stale(None);
        assert_eq!(engine.mem_names(), ["notes"]);
        assert!(
            engine
                .get_entity(&memstead_base::EntityId("notes--held".into()))
                .is_some(),
            "the granted mem is read without a restart"
        );
        assert!(!engine.mem_router().is_writable("notes"));
        let refused = engine
            .create_entity_with_ctx(args("notes", "Refused"), &ctx)
            .expect_err("a read-only mount refuses the write");
        assert_eq!(refused.code(), "READ_ONLY_MOUNT", "{refused}");

        // The read is withdrawn.
        save(Vec::new());
        engine.reload_if_stale(None);
        assert!(
            engine.mem_names().is_empty(),
            "a withdrawn read is not served"
        );
        assert!(
            !engine.mem_router().is_visible("notes"),
            "the router's read-only slot is cleared with the mount"
        );
        assert!(
            engine
                .get_entity(&memstead_base::EntityId("notes--held".into()))
                .is_none()
        );
        assert_eq!(engine.unknown_mem_error("notes").code(), "MEM_UNMOUNTED");
    }

    /// A running engine in a workspace of engine state only serves a
    /// mem another process mounts with the mount verb from its next
    /// operation on, also when the mem is pinned to a schema that only
    /// the mem-repo it lives in has installed, and stops serving it
    /// after the unregister. Nothing restarts.
    #[test]
    fn a_running_engine_follows_a_mount_and_its_unregister() {
        use memstead_base::mem_management::{
            MemDeleteParams, MemMountParams, MountSource, delete_mem, mount_mem,
        };
        use memstead_base::schema_source::SchemaSource as _;

        let tmp = TempDir::new().unwrap();
        let holder = tmp.path().join("holder");
        let view = tmp.path().join("view");
        crate::test_support::init_real_mem_repo(&holder, &[("notes", "refsrc@0.1.0")]);
        let holder_gitdir = std::fs::canonicalize(holder.join("mem-repo").join(".git")).unwrap();
        crate::mem_repo_schemas::GitBranchSchemaSource::for_gitdir(&holder_gitdir)
            .write_schema(
                "refsrc",
                "0.1.0",
                &[
                    ("schema.yaml".to_string(), REFSRC_MANIFEST.to_vec()),
                    ("types/doc.yaml".to_string(), REFSRC_DOC.to_vec()),
                ],
            )
            .unwrap();
        crate::lay_workspace_marker(&view).unwrap();

        // The long-lived engine: nothing mounted yet.
        let mut running = engine_from_workspace_root(&view).unwrap();
        assert!(running.reload_if_stale(None).is_empty());
        assert!(running.mem_names().is_empty());

        // Another process mounts the holder's mem, read-only.
        let mount = |capability| MemMountParams {
            name: "notes".to_string(),
            source: MountSource::Branch {
                branch: "notes".to_string(),
                gitdir: Some(holder_gitdir.clone()),
            },
            capability,
            schema: None,
            note: None,
            operator_mode: true,
            actor: memstead_base::vcs::Actor::Cli,
            client: None,
        };
        let unregister = || MemDeleteParams {
            name: "notes".to_string(),
            delete_files: false,
            note: None,
            actor: memstead_base::vcs::Actor::Cli,
            client: None,
            operator_mode: true,
            detach_incoming: false,
        };
        {
            let mut other = engine_from_workspace_root(&view).unwrap();
            let mounted =
                mount_mem(&mut other, mount(memstead_base::MountCapability::ReadOnly)).unwrap();
            assert_eq!(mounted.schema_ref, "refsrc@0.1.0");
            assert!(!mounted.config_written && !mounted.fetched);
        }

        // The running engine's next operation serves it, read-only,
        // under the schema the holder's mem-repo has installed.
        let codes: Vec<String> = running
            .reload_if_stale(None)
            .iter()
            .map(|w| w.code().to_string())
            .collect();
        assert!(codes.iter().any(|c| c == "MEM_ROSTER_CHANGED"), "{codes:?}");
        assert_eq!(running.mem_names(), ["notes"]);
        assert!(running.quarantined_mems().is_empty());
        assert!(!running.mem_router().is_writable("notes"));

        // Unregistered by another process: gone from the next operation.
        {
            let mut other = engine_from_workspace_root(&view).unwrap();
            let gone = delete_mem(&mut other, unregister()).unwrap();
            assert!(gone.deleted_from_router && !gone.files_deleted);
        }
        running.reload_if_stale(None);
        assert!(running.mem_names().is_empty());
        assert_eq!(running.unknown_mem_error("notes").code(), "MEM_UNMOUNTED");

        // Mounted again with capability write: followed as a write mount.
        {
            let mut other = engine_from_workspace_root(&view).unwrap();
            mount_mem(&mut other, mount(memstead_base::MountCapability::Write)).unwrap();
        }
        running.reload_if_stale(None);
        assert!(running.mem_router().is_writable("notes"));
    }

    /// A name that is unregistered and mounted again onto another git
    /// directory between two operations of a running engine, under the
    /// same capability, is another mount: the running engine serves the
    /// store the roster names now, says the roster changed, and writes
    /// there, not into the store it served before.
    #[test]
    fn a_running_engine_follows_a_name_mounted_onto_another_store() {
        use memstead_base::mem_management::{
            MemDeleteParams, MemMountParams, MountSource, delete_mem, mount_mem,
        };

        let tmp = TempDir::new().unwrap();
        let view = tmp.path().join("view");
        let mut gitdirs = Vec::new();
        for (dir, title) in [("first", "Alpha"), ("second", "Beta")] {
            let holder = tmp.path().join(dir);
            crate::test_support::init_real_mem_repo(&holder, &[("notes", "default@1.0.0")]);
            let mut engine = engine_from_workspace_root(&holder).unwrap();
            operate(&mut engine, "holder", None, Some(("notes", title)));
            gitdirs.push(std::fs::canonicalize(holder.join("mem-repo").join(".git")).unwrap());
        }
        crate::lay_workspace_marker(&view).unwrap();
        let mount = |gitdir: &std::path::Path| MemMountParams {
            name: "notes".to_string(),
            source: MountSource::Branch {
                branch: "notes".to_string(),
                gitdir: Some(gitdir.to_path_buf()),
            },
            capability: memstead_base::MountCapability::Write,
            schema: None,
            note: None,
            operator_mode: true,
            actor: memstead_base::vcs::Actor::Cli,
            client: None,
        };
        let serves = |engine: &Engine, id: &str| {
            engine
                .get_entity(&memstead_base::EntityId(id.into()))
                .is_some()
        };
        let tip = |gitdir: &std::path::Path| {
            crate::ops::transport::resolve_ref_in_gitdir(gitdir, "refs/heads/notes")
                .unwrap()
                .unwrap()
        };

        {
            let mut other = engine_from_workspace_root(&view).unwrap();
            mount_mem(&mut other, mount(&gitdirs[0])).unwrap();
        }
        let mut running = engine_from_workspace_root(&view).unwrap();
        running.reload_if_stale(None);
        assert!(serves(&running, "notes--alpha") && !serves(&running, "notes--beta"));

        // Unregistered and mounted onto the second store, with no
        // operation of the running engine in between.
        {
            let mut other = engine_from_workspace_root(&view).unwrap();
            delete_mem(
                &mut other,
                MemDeleteParams {
                    name: "notes".to_string(),
                    delete_files: false,
                    note: None,
                    actor: memstead_base::vcs::Actor::Cli,
                    client: None,
                    operator_mode: true,
                    detach_incoming: false,
                },
            )
            .unwrap();
            mount_mem(&mut other, mount(&gitdirs[1])).unwrap();
        }
        let codes: Vec<String> = running
            .reload_if_stale(None)
            .iter()
            .map(|w| w.code().to_string())
            .collect();
        assert!(codes.iter().any(|c| c == "MEM_ROSTER_CHANGED"), "{codes:?}");
        assert!(
            serves(&running, "notes--beta") && !serves(&running, "notes--alpha"),
            "the running engine serves the store the roster names now"
        );

        // A write through the running engine lands in the second store.
        let (first_before, second_before) = (tip(&gitdirs[0]), tip(&gitdirs[1]));
        operate(&mut running, "writer", None, Some(("notes", "Gamma")));
        assert_eq!(
            tip(&gitdirs[0]),
            first_before,
            "the first store is not written"
        );
        assert_ne!(
            tip(&gitdirs[1]),
            second_before,
            "the write landed in the second"
        );

        // A further reconciliation over the unchanged roster moves nothing.
        let quiet: Vec<String> = running
            .reload_if_stale(None)
            .iter()
            .map(|w| w.code().to_string())
            .collect();
        assert!(
            !quiet.iter().any(|c| c == "MEM_ROSTER_CHANGED"),
            "{quiet:?}"
        );
    }

    /// A mount that a running engine holds quarantined (its git
    /// directory was away) is served again from the engine's next
    /// operation once the directory is back and the name was
    /// unregistered and mounted again, which leaves the roster's rows
    /// exactly as they were: every write of the roster makes a running
    /// engine try its quarantined rows again.
    #[test]
    fn a_running_engine_tries_a_quarantined_mount_again_when_the_roster_is_written() {
        use memstead_base::mem_management::{
            MemDeleteParams, MemMountParams, MountSource, delete_mem, mount_mem,
        };

        let tmp = TempDir::new().unwrap();
        let view = tmp.path().join("view");
        let holder = tmp.path().join("holder");
        let away = tmp.path().join("holder-away");
        crate::test_support::init_real_mem_repo(&holder, &[("notes", "default@1.0.0")]);
        {
            let mut engine = engine_from_workspace_root(&holder).unwrap();
            operate(&mut engine, "holder", None, Some(("notes", "Alpha")));
        }
        let gitdir = std::fs::canonicalize(holder.join("mem-repo").join(".git")).unwrap();
        crate::lay_workspace_marker(&view).unwrap();
        let mount = || MemMountParams {
            name: "notes".to_string(),
            source: MountSource::Branch {
                branch: "notes".to_string(),
                gitdir: Some(gitdir.clone()),
            },
            capability: memstead_base::MountCapability::ReadOnly,
            schema: None,
            note: None,
            operator_mode: true,
            actor: memstead_base::vcs::Actor::Cli,
            client: None,
        };
        let codes = |engine: &mut Engine| -> Vec<String> {
            engine
                .reload_if_stale(None)
                .iter()
                .map(|w| w.code().to_string())
                .collect()
        };
        let mut running = engine_from_workspace_root(&view).unwrap();
        codes(&mut running);

        // The mount is added while its git directory is away: the
        // running engine quarantines it.
        {
            let mut other = engine_from_workspace_root(&view).unwrap();
            mount_mem(&mut other, mount()).unwrap();
        }
        std::fs::rename(&holder, &away).unwrap();
        codes(&mut running);
        assert!(running.mem_names().is_empty());
        assert_eq!(running.quarantined_mems().len(), 1);

        // A roster that is not written leaves the quarantine alone,
        // also once the directory is back.
        std::fs::rename(&away, &holder).unwrap();
        let quiet = codes(&mut running);
        assert!(
            !quiet.iter().any(|c| c == "MEM_ROSTER_CHANGED"),
            "{quiet:?}"
        );
        assert_eq!(running.quarantined_mems().len(), 1);

        // Unregistered and mounted again by another process: the same
        // row, written again.
        let before = std::fs::read(view.join(".memstead/state/mounts.json")).unwrap();
        {
            let mut other = engine_from_workspace_root(&view).unwrap();
            delete_mem(
                &mut other,
                MemDeleteParams {
                    name: "notes".to_string(),
                    delete_files: false,
                    note: None,
                    actor: memstead_base::vcs::Actor::Cli,
                    client: None,
                    operator_mode: true,
                    detach_incoming: false,
                },
            )
            .unwrap();
            mount_mem(&mut other, mount()).unwrap();
        }
        assert_eq!(
            std::fs::read(view.join(".memstead/state/mounts.json")).unwrap(),
            before,
            "the roster's bytes are as they were"
        );
        let told = codes(&mut running);
        assert!(told.iter().any(|c| c == "MEM_ROSTER_CHANGED"), "{told:?}");
        assert_eq!(running.mem_names(), ["notes"]);
        assert!(running.quarantined_mems().is_empty());
        assert!(
            running
                .get_entity(&memstead_base::EntityId("notes--alpha".into()))
                .is_some()
        );
    }

    /// A running engine holds a write mount quarantined (its git
    /// directory is gone). Another process takes the mount off the
    /// roster with the unregister; the running engine stops reporting
    /// the mem as quarantined on its next operation.
    #[test]
    fn a_running_engine_drops_a_quarantined_mount_another_process_unregistered() {
        use memstead_base::mem_management::{
            MemDeleteParams, MemMountParams, MountSource, delete_mem, mount_mem,
        };

        let tmp = TempDir::new().unwrap();
        let view = tmp.path().join("view");
        let holder = tmp.path().join("holder");
        crate::test_support::init_real_mem_repo(&holder, &[("notes", "default@1.0.0")]);
        {
            let mut engine = engine_from_workspace_root(&holder).unwrap();
            operate(&mut engine, "holder", None, Some(("notes", "Alpha")));
        }
        let gitdir = std::fs::canonicalize(holder.join("mem-repo").join(".git")).unwrap();
        crate::lay_workspace_marker(&view).unwrap();
        {
            let mut other = engine_from_workspace_root(&view).unwrap();
            mount_mem(
                &mut other,
                MemMountParams {
                    name: "notes".to_string(),
                    source: MountSource::Branch {
                        branch: "notes".to_string(),
                        gitdir: Some(gitdir.clone()),
                    },
                    capability: memstead_base::MountCapability::Write,
                    schema: None,
                    note: None,
                    operator_mode: true,
                    actor: memstead_base::vcs::Actor::Cli,
                    client: None,
                },
            )
            .unwrap();
        }
        std::fs::remove_dir_all(&holder).unwrap();
        let mut running = engine_from_workspace_root(&view).unwrap();
        running.reload_if_stale(None);
        assert!(running.mem_names().is_empty());
        assert_eq!(running.quarantined_mems().len(), 1);

        {
            let mut other = engine_from_workspace_root(&view).unwrap();
            let removed = delete_mem(
                &mut other,
                MemDeleteParams {
                    name: "notes".to_string(),
                    delete_files: false,
                    note: None,
                    actor: memstead_base::vcs::Actor::Cli,
                    client: None,
                    operator_mode: true,
                    detach_incoming: false,
                },
            )
            .expect("a quarantined write mount leaves the roster");
            assert!(removed.deleted_from_router && !removed.files_deleted);
        }
        let roster = std::fs::read_to_string(view.join(".memstead/state/mounts.json")).unwrap();
        assert!(!roster.contains("notes"), "{roster}");
        assert!(!holder.exists(), "nothing is laid where the storage was");

        running.reload_if_stale(None);
        assert!(
            running.quarantined_mems().is_empty(),
            "{:?}",
            running.quarantined_mems()
        );
        assert!(running.mem_names().is_empty());
    }

    fn caller_fixture() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("ws");
        crate::test_support::init_real_mem_repo(
            &root,
            &[("notes", "default@1.0.0"), ("other", "default@1.0.0")],
        );
        (tmp, root)
    }

    fn spec(mem: &str, title: &str) -> memstead_base::engine::CreateEntityArgs {
        memstead_base::engine::CreateEntityArgs {
            anchors: Vec::new(),
            mem: mem.to_string(),
            title: title.to_string(),
            entity_type: "spec".to_string(),
            sections: indexmap::IndexMap::from_iter([
                ("identity".to_string(), "i".to_string()),
                ("purpose".to_string(), "p".to_string()),
            ]),
            metadata: indexmap::IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        }
    }

    /// One operation of `caller`: reload (workspace-wide, or scoped to
    /// `scope`), optionally create an entity, and hand back the notices
    /// and warning codes the operation carries.
    fn operate(
        engine: &mut Engine,
        caller: &str,
        scope: Option<&str>,
        create: Option<(&str, &str)>,
    ) -> (Vec<memstead_base::ops::MemChangedNotice>, Vec<String>) {
        let mut op =
            memstead_base::OperationScope::begin_as(engine, memstead_base::CallerKey::new(caller));
        let mut codes: Vec<String> = op
            .reload_if_stale(scope)
            .iter()
            .map(|w| w.code().to_string())
            .collect();
        if let Some((mem, title)) = create {
            let outcome = op
                .create_entity_with_ctx(
                    spec(mem, title),
                    &memstead_base::vcs::CommitContext::internal(),
                )
                .expect("the write lands");
            codes.extend(outcome.warnings.iter().map(|w| w.code().to_string()));
        }
        let (_, notices) = op.finish();
        (notices, codes)
    }

    fn listed_ids(notice: &memstead_base::ops::MemChangedNotice) -> Vec<String> {
        let value = serde_json::to_value(notice).unwrap();
        value["changes"]["entries"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|e| e["id"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Two callers on one engine: what one writes, the other is told on
    /// its next operation, whatever that operation is, and only once;
    /// the writer is not told about its own write; a caller seen for the
    /// first time is told nothing.
    #[test]
    fn a_caller_is_told_once_what_another_caller_wrote_through_the_same_engine() {
        let (_tmp, root) = caller_fixture();
        let mut engine = engine_from_workspace_root(&root).unwrap();

        // First contact of each caller: the baseline, no notice.
        assert!(operate(&mut engine, "a", None, None).0.is_empty());
        assert!(operate(&mut engine, "b", None, None).0.is_empty());

        // A writes: no notice for its own write, now or on its next call.
        let (notices, _) = operate(&mut engine, "a", Some("notes"), Some(("notes", "From A")));
        assert!(
            notices.is_empty(),
            "a writer is not told about its own write"
        );

        // B's next operation is a read scoped to another mem: it carries
        // the notice for the mem A wrote, listing the entity.
        let (notices, codes) = operate(&mut engine, "b", Some("other"), None);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(notices[0].mem, "notes");
        assert_eq!(listed_ids(&notices[0]), ["notes--from-a"]);
        assert!(
            codes.iter().any(|c| c == "MEM_RELOADED"),
            "the text channel's warning rides along: {codes:?}"
        );

        // Told once: B's following operation carries none, nor does A's.
        assert!(operate(&mut engine, "b", None, None).0.is_empty());
        assert!(operate(&mut engine, "a", None, None).0.is_empty());

        // A caller seen for the first time after all that is told nothing.
        let (notices, codes) = operate(&mut engine, "c", None, None);
        assert!(notices.is_empty());
        assert!(!codes.iter().any(|c| c == "MEM_RELOADED"), "{codes:?}");

        // B writes in turn; A and C are each told once, B is not.
        let (notices, _) = operate(&mut engine, "b", Some("other"), Some(("other", "From B")));
        assert!(notices.is_empty());
        for caller in ["a", "c"] {
            let (notices, _) = operate(&mut engine, caller, None, None);
            assert_eq!(notices.len(), 1, "{caller}: {notices:?}");
            assert_eq!(notices[0].mem, "other");
            assert_eq!(listed_ids(&notices[0]), ["other--from-b"]);
            assert!(operate(&mut engine, caller, None, None).0.is_empty());
        }
        assert!(operate(&mut engine, "b", None, None).0.is_empty());
    }

    /// A change made by another process reaches every caller of this
    /// engine exactly once, each on its own next operation; and a
    /// surface with one caller is told as it was when the notice was
    /// relative to the engine.
    #[test]
    fn a_change_from_another_process_reaches_every_caller_once() {
        let (_tmp, root) = caller_fixture();
        let mut engine = engine_from_workspace_root(&root).unwrap();
        assert!(operate(&mut engine, "a", None, None).0.is_empty());
        assert!(operate(&mut engine, "b", None, None).0.is_empty());

        // Another engine over the same mem-repo writes.
        let mut sibling = engine_from_workspace_root(&root).unwrap();
        sibling
            .create_entity_with_ctx(
                spec("notes", "Outside"),
                &memstead_base::vcs::CommitContext::internal(),
            )
            .unwrap();

        for caller in ["a", "b"] {
            let (notices, codes) = operate(&mut engine, caller, None, None);
            assert_eq!(notices.len(), 1, "{caller}: {notices:?}");
            assert_eq!(notices[0].mem, "notes");
            assert_eq!(listed_ids(&notices[0]), ["notes--outside"]);
            assert_eq!(
                codes.iter().filter(|c| *c == "MEM_RELOADED").count(),
                1,
                "{caller}: one reload warning, not two: {codes:?}"
            );
            assert!(operate(&mut engine, caller, None, None).0.is_empty());
        }

        // The single caller of a surface that names none: the default
        // key, known since before the sibling's next write.
        let mut single = engine_from_workspace_root(&root).unwrap();
        sibling
            .create_entity_with_ctx(
                spec("notes", "Outside Again"),
                &memstead_base::vcs::CommitContext::internal(),
            )
            .unwrap();
        let mut op = memstead_base::OperationScope::begin(&mut single);
        let codes: Vec<String> = op
            .reload_if_stale(None)
            .iter()
            .map(|w| w.code().to_string())
            .collect();
        let (_, notices) = op.finish();
        assert_eq!(notices.len(), 1);
        assert_eq!(listed_ids(&notices[0]), ["notes--outside-again"]);
        assert_eq!(codes.iter().filter(|c| *c == "MEM_RELOADED").count(), 1);
    }

    /// A caller that reloads explicitly reads what the reload brought
    /// in from its report: its next operation carries no notice for
    /// it, as when the notice was relative to the engine. Another
    /// caller of the same engine is still told.
    #[test]
    fn an_explicit_reload_counts_as_seen_by_the_caller_that_asked_for_it() {
        let (_tmp, root) = caller_fixture();
        let mut engine = engine_from_workspace_root(&root).unwrap();
        let mut sibling = engine_from_workspace_root(&root).unwrap();
        let ctx = memstead_base::vcs::CommitContext::internal();

        // The default caller and a named one are both known.
        let mut op = memstead_base::OperationScope::begin(&mut engine);
        op.reload_if_stale(None);
        assert!(op.finish().1.is_empty());
        assert!(operate(&mut engine, "b", None, None).0.is_empty());

        for (title, workspace_wide) in [("Outside One", false), ("Outside Two", true)] {
            sibling
                .create_entity_with_ctx(spec("notes", title), &ctx)
                .unwrap();

            // The default caller reloads explicitly, without the
            // reload-before-operation step.
            let mut op = memstead_base::OperationScope::begin(&mut engine);
            if workspace_wide {
                op.reload_each_writable_mem_reports().unwrap();
            } else {
                op.reload_one_mem_report("notes").unwrap();
            }
            assert!(op.finish().1.is_empty(), "the report is the answer");

            // Its next operation carries nothing for what it reloaded.
            let mut op = memstead_base::OperationScope::begin(&mut engine);
            let codes: Vec<String> = op
                .reload_if_stale(None)
                .iter()
                .map(|w| w.code().to_string())
                .collect();
            assert!(op.finish().1.is_empty(), "{title}");
            assert!(!codes.iter().any(|c| c == "MEM_RELOADED"), "{codes:?}");

            // The other caller did not ask for the reload: it is told.
            let (notices, codes) = operate(&mut engine, "b", None, None);
            assert_eq!(notices.len(), 1, "{title}: {notices:?}");
            assert_eq!(notices[0].mem, "notes");
            assert_eq!(codes.iter().filter(|c| *c == "MEM_RELOADED").count(), 1);
            assert!(operate(&mut engine, "b", None, None).0.is_empty());
        }
    }

    /// One operation of `caller` that reloads nothing before it acts:
    /// an explicit reload of one mem, of every mem, or nothing at all.
    fn operate_without_the_reload_step(
        engine: &mut Engine,
        caller: &str,
        explicit: Option<Option<&str>>,
    ) -> Vec<memstead_base::ops::MemChangedNotice> {
        let mut op =
            memstead_base::OperationScope::begin_as(engine, memstead_base::CallerKey::new(caller));
        match explicit {
            Some(Some(mem)) => {
                op.reload_one_mem_report(mem).unwrap();
            }
            Some(None) => {
                op.reload_each_writable_mem_reports().unwrap();
            }
            None => {}
        }
        op.finish().1
    }

    /// The told caller's next operation may be one that reloads nothing
    /// before it acts. It carries the notice all the same, and the
    /// operation after it carries none; and such an operation counts as
    /// a first contact like any other.
    #[test]
    fn an_operation_that_reloads_nothing_first_still_tells_its_caller() {
        // Closed list: an explicit reload of the written mem, of the
        // other mem, of every mem, and an operation that does nothing.
        let kinds: [Option<Option<&str>>; 4] =
            [Some(Some("notes")), Some(Some("other")), Some(None), None];
        for (n, kind) in kinds.into_iter().enumerate() {
            let (_tmp, root) = caller_fixture();
            let mut engine = engine_from_workspace_root(&root).unwrap();
            assert!(operate(&mut engine, "a", None, None).0.is_empty());
            assert!(operate(&mut engine, "b", None, None).0.is_empty());

            operate(&mut engine, "a", Some("notes"), Some(("notes", "From A")));
            let notices = operate_without_the_reload_step(&mut engine, "b", kind);
            assert_eq!(notices.len(), 1, "kind {n}: {notices:?}");
            assert_eq!(notices[0].mem, "notes");
            assert_eq!(listed_ids(&notices[0]), ["notes--from-a"]);
            assert!(
                operate(&mut engine, "b", None, None).0.is_empty(),
                "kind {n}"
            );
            assert!(operate_without_the_reload_step(&mut engine, "a", kind).is_empty());

            // As a first contact: nothing on it, and the caller is known
            // from then on.
            assert!(operate_without_the_reload_step(&mut engine, "new", kind).is_empty());
            operate(
                &mut engine,
                "a",
                Some("notes"),
                Some(("notes", "From A Again")),
            );
            let (notices, _) = operate(&mut engine, "new", None, None);
            assert_eq!(notices.len(), 1, "kind {n}: {notices:?}");
            assert_eq!(listed_ids(&notices[0]), ["notes--from-a-again"]);
        }
    }

    /// A caller that is behind the engine for a mem while storage is
    /// further ahead still is told everything in one notice whose two
    /// ends agree with its entries, and nothing of it a second time
    /// when a later operation looks at that mem.
    #[test]
    fn a_notice_covers_what_storage_holds_and_nothing_is_told_twice() {
        let (_tmp, root) = caller_fixture();
        let mut engine = engine_from_workspace_root(&root).unwrap();
        let mut sibling = engine_from_workspace_root(&root).unwrap();
        let ctx = memstead_base::vcs::CommitContext::internal();
        assert!(operate(&mut engine, "a", None, None).0.is_empty());
        assert!(operate(&mut engine, "b", None, None).0.is_empty());

        // The engine reloads the first outside write for A; a second one
        // lands that no operation has looked at.
        sibling
            .create_entity_with_ctx(spec("notes", "W One"), &ctx)
            .unwrap();
        let (notices, _) = operate(&mut engine, "a", Some("notes"), None);
        assert_eq!(listed_ids(&notices[0]), ["notes--w-one"]);
        sibling
            .create_entity_with_ctx(spec("notes", "W Two"), &ctx)
            .unwrap();
        let storage_head = {
            let mut probe = engine_from_workspace_root(&root).unwrap();
            probe.reload_one_mem_report("notes").unwrap().head_after
        };

        // B's operation is scoped to the other mem: it is behind the
        // engine for `notes`, so it is told, and told all of it.
        let (notices, codes) = operate(&mut engine, "b", Some("other"), None);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(listed_ids(&notices[0]), ["notes--w-one", "notes--w-two"]);
        assert_eq!(
            notices[0].to_head, storage_head,
            "the ends agree with the entries"
        );
        assert_eq!(codes.iter().filter(|c| *c == "MEM_RELOADED").count(), 1);
        assert!(
            operate(&mut engine, "b", None, None).0.is_empty(),
            "not twice"
        );

        // A was told the first write already: only the second reaches it.
        let (notices, _) = operate(&mut engine, "a", None, None);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(listed_ids(&notices[0]), ["notes--w-two"]);

        // The same with a write as the operation of the caller that is
        // behind: one notice, not two.
        sibling
            .create_entity_with_ctx(spec("notes", "W Three"), &ctx)
            .unwrap();
        operate(&mut engine, "a", Some("notes"), None);
        sibling
            .create_entity_with_ctx(spec("notes", "W Four"), &ctx)
            .unwrap();
        let (notices, _) = operate(&mut engine, "b", Some("other"), Some(("other", "By B")));
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(listed_ids(&notices[0]), ["notes--w-four", "notes--w-three"]);
        assert!(operate(&mut engine, "b", None, None).0.is_empty());
    }

    /// A caller that came while storage was ahead of the engine for a
    /// mem gives its later operations no reason to look at that mem: an
    /// operation scoped to another mem stays scoped, as it does for a
    /// caller that was there before, and the change that lands later is
    /// told to each caller once the mem is reloaded.
    #[test]
    fn a_caller_that_came_while_storage_was_ahead_makes_no_scoped_operation_look_further() {
        // Closed list of the new caller's first and second operation:
        // a read scoped to the other mem, nothing at all, an explicit
        // reload of the other mem.
        fn other_only(engine: &mut Engine, caller: &str, kind: usize) -> usize {
            match kind {
                0 => operate(engine, caller, Some("other"), None).0.len(),
                1 => operate_without_the_reload_step(engine, caller, None).len(),
                _ => operate_without_the_reload_step(engine, caller, Some(Some("other"))).len(),
            }
        }
        for first in 0..3 {
            for second in 0..3 {
                let (_tmp, root) = caller_fixture();
                let mut engine = engine_from_workspace_root(&root).unwrap();
                let mut sibling = engine_from_workspace_root(&root).unwrap();
                let ctx = memstead_base::vcs::CommitContext::internal();
                assert!(operate(&mut engine, "k", None, None).0.is_empty());

                sibling
                    .create_entity_with_ctx(spec("notes", "E One"), &ctx)
                    .unwrap();
                assert_eq!(other_only(&mut engine, "n", first), 0);
                sibling
                    .create_entity_with_ctx(spec("notes", "E Two"), &ctx)
                    .unwrap();

                // Nobody has reloaded `notes`: neither the caller that
                // was there before nor the new one is told by an
                // operation that stays with the other mem.
                assert_eq!(other_only(&mut engine, "k", second), 0, "{first}/{second}");
                assert_eq!(other_only(&mut engine, "n", second), 0, "{first}/{second}");
                assert_eq!(other_only(&mut engine, "k", second), 0, "{first}/{second}");

                // Once an operation looks at `notes`, each is told what
                // is news to it, once.
                let (notices, _) = operate(&mut engine, "n", None, None);
                assert_eq!(notices.len(), 1, "{first}/{second}: {notices:?}");
                assert_eq!(listed_ids(&notices[0]), ["notes--e-two"]);
                let (notices, _) = operate(&mut engine, "k", Some("other"), None);
                assert_eq!(notices.len(), 1, "{first}/{second}: {notices:?}");
                assert_eq!(listed_ids(&notices[0]), ["notes--e-one", "notes--e-two"]);
                assert!(operate(&mut engine, "n", None, None).0.is_empty());
                assert!(operate(&mut engine, "k", None, None).0.is_empty());
            }
        }
    }

    /// Seeded sequences over four callers, another process and every
    /// kind of operation: whatever the order, a caller learns of each
    /// entity someone else created after its first contact exactly
    /// once (by a notice, or by the report of a reload it asked for),
    /// and no notice ever lists its own write or an entity that was
    /// there before it came. A reload's report describes what the
    /// engine brought in, so it may list what landed before the caller
    /// came; a notice may not.
    #[test]
    fn any_sequence_tells_each_caller_each_foreign_change_exactly_once() {
        use std::collections::HashMap;
        const CALLERS: [&str; 4] = ["", "a", "b", "c"];
        for seed in 0u64..40 {
            let (_tmp, root) = caller_fixture();
            let mut engine = engine_from_workspace_root(&root).unwrap();
            let mut sibling = engine_from_workspace_root(&root).unwrap();
            let ctx = memstead_base::vcs::CommitContext::internal();
            let mut state = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let mut next = |bound: u64| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (state >> 33) % bound
            };
            // id -> (creator, step); caller -> first-contact step;
            // (caller, id) -> times told.
            let mut created: HashMap<String, (String, usize)> = HashMap::new();
            let mut first_contact: HashMap<&str, usize> = HashMap::new();
            let mut told: HashMap<(String, String), usize> = HashMap::new();
            let mut reported: HashMap<(String, String), usize> = HashMap::new();
            let mut trace: Vec<String> = Vec::new();
            let mut serial = 0usize;

            // The single caller of a surface has been there since boot.
            let mut op = memstead_base::OperationScope::begin(&mut engine);
            op.reload_if_stale(None);
            assert!(op.finish().1.is_empty());
            first_contact.insert("", 0);

            let steps = 30usize;
            for step in 1..=steps + CALLERS.len() {
                // The last steps: every caller looks at everything.
                let closing = step > steps;
                let actor = if closing { 0 } else { next(5) };
                if actor == 4 {
                    serial += 1;
                    let mem = if next(2) == 0 { "notes" } else { "other" };
                    let title = format!("E{serial}");
                    sibling
                        .create_entity_with_ctx(spec(mem, &title), &ctx)
                        .unwrap();
                    created.insert(format!("{mem}--e{serial}"), ("outside".to_string(), step));
                    trace.push(format!("{step}: outside writes {mem}--e{serial}"));
                    continue;
                }
                let caller = if closing {
                    CALLERS[step - steps - 1]
                } else {
                    CALLERS[next(4) as usize]
                };
                let kind = if closing { 0 } else { next(9) };
                let mut op = memstead_base::OperationScope::begin_as(
                    &mut engine,
                    memstead_base::CallerKey::new(caller),
                );
                let mut seen_now: Vec<String> = Vec::new();
                let mut in_report: Vec<String> = Vec::new();
                let mut wrote: Option<String> = None;
                match kind {
                    0 => {
                        op.reload_if_stale(None);
                    }
                    1 => {
                        op.reload_if_stale(Some("notes"));
                    }
                    2 => {
                        op.reload_if_stale(Some("other"));
                    }
                    3 | 4 => {
                        let mem = if kind == 3 { "notes" } else { "other" };
                        serial += 1;
                        op.reload_if_stale(Some(mem));
                        op.create_entity_with_ctx(spec(mem, &format!("E{serial}")), &ctx)
                            .expect("the write lands");
                        wrote = Some(format!("{mem}--e{serial}"));
                    }
                    5 | 6 => {
                        let mem = if kind == 5 { "notes" } else { "other" };
                        let report = op.reload_one_mem_report(mem).unwrap();
                        in_report.extend(report.changed_entity_ids.iter().map(|id| id.to_string()));
                    }
                    7 => {
                        for report in op.reload_each_writable_mem_reports().unwrap() {
                            in_report
                                .extend(report.changed_entity_ids.iter().map(|id| id.to_string()));
                        }
                    }
                    _ => {}
                }
                let (_, notices) = op.finish();
                let mut mems_told: Vec<&str> = notices.iter().map(|n| n.mem.as_str()).collect();
                mems_told.sort_unstable();
                let distinct = mems_told.len();
                mems_told.dedup();
                assert_eq!(
                    distinct,
                    mems_told.len(),
                    "seed {seed}: one notice per mem: {trace:#?}"
                );
                for notice in &notices {
                    seen_now.extend(listed_ids(notice));
                }
                trace.push(format!(
                    "{step}: {caller:?} kind {kind} wrote {wrote:?} told {seen_now:?} report {in_report:?}"
                ));
                first_contact.entry(caller).or_insert(step);
                if let Some(id) = wrote {
                    created.insert(id, (caller.to_string(), step));
                }
                for id in seen_now {
                    *told.entry((caller.to_string(), id)).or_default() += 1;
                }
                for id in in_report {
                    *reported.entry((caller.to_string(), id)).or_default() += 1;
                }
            }

            for caller in CALLERS {
                let Some(&came) = first_contact.get(caller) else {
                    continue;
                };
                for (id, (creator, at)) in &created {
                    let key = (caller.to_string(), id.clone());
                    let noticed = told.get(&key).copied().unwrap_or(0);
                    let by_report = reported.get(&key).copied().unwrap_or(0);
                    let context = format!(
                        "seed {seed}: caller {caller:?} (came at {came}) and {id} (by {creator} at {at}): {trace:#?}"
                    );
                    if creator == caller || *at <= came {
                        assert_eq!(noticed, 0, "no notice for it: {context}");
                    } else {
                        assert_eq!(noticed + by_report, 1, "learnt exactly once: {context}");
                    }
                }
            }
        }
    }

    /// The warning a named caller's operation returns is the text form
    /// of its notice: it runs from the head that caller last saw, also
    /// when the operation's own reload moved the engine by less, and
    /// there is none where the reload is no news to the caller.
    #[test]
    fn the_reload_warning_of_a_named_caller_describes_its_own_notice() {
        let (_tmp, root) = caller_fixture();
        let mut engine = engine_from_workspace_root(&root).unwrap();
        let mut sibling = engine_from_workspace_root(&root).unwrap();
        let ctx = memstead_base::vcs::CommitContext::internal();
        let reloads = |engine: &mut Engine, caller: &str| {
            let mut op = memstead_base::OperationScope::begin_as(
                engine,
                memstead_base::CallerKey::new(caller),
            );
            let warnings: Vec<(String, String, usize)> = op
                .reload_if_stale(None)
                .into_iter()
                .filter_map(|w| match w {
                    memstead_base::ops::WarningHint::MemReloaded {
                        old_head,
                        new_head,
                        entities_loaded,
                        ..
                    } => Some((old_head, new_head, entities_loaded)),
                    _ => None,
                })
                .collect();
            (warnings, op.finish().1)
        };
        assert!(reloads(&mut engine, "a").1.is_empty());
        assert!(reloads(&mut engine, "c").1.is_empty());

        // The engine reloads the first outside write for A; C's own
        // operation then reloads the second.
        sibling
            .create_entity_with_ctx(spec("notes", "X One"), &ctx)
            .unwrap();
        let (warnings, notices) = reloads(&mut engine, "a");
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].0, notices[0].from_head);
        sibling
            .create_entity_with_ctx(spec("notes", "X Two"), &ctx)
            .unwrap();
        let (warnings, notices) = reloads(&mut engine, "c");
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(listed_ids(&notices[0]), ["notes--x-one", "notes--x-two"]);
        assert_eq!(
            warnings,
            [(notices[0].from_head.clone(), notices[0].to_head.clone(), 2)],
            "the warning runs from the head C last saw and counts both"
        );

        // A caller that comes after a change gets no warning for the
        // reload its first operation performs.
        sibling
            .create_entity_with_ctx(spec("notes", "X Three"), &ctx)
            .unwrap();
        let (warnings, notices) = reloads(&mut engine, "late");
        assert!(notices.is_empty() && warnings.is_empty(), "{warnings:?}");

        // The default caller in the same seat as C: where it shares the
        // engine and is behind it, its warning runs from the head it
        // last saw as well. (As the single caller of a surface it is
        // never behind, and its warning is the reload's own.)
        // (It has been known since the engine's first reload, so its
        // first operation here brings it up to date.)
        reloads(&mut engine, "");
        sibling
            .create_entity_with_ctx(spec("notes", "X Four"), &ctx)
            .unwrap();
        reloads(&mut engine, "a");
        sibling
            .create_entity_with_ctx(spec("notes", "X Five"), &ctx)
            .unwrap();
        let (warnings, notices) = reloads(&mut engine, "");
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(listed_ids(&notices[0]), ["notes--x-five", "notes--x-four"]);
        assert_eq!(
            warnings,
            [(notices[0].from_head.clone(), notices[0].to_head.clone(), 2)]
        );
    }

    /// What landed before a caller's first contact is never news to it,
    /// also while the engine has not reloaded that change yet: the
    /// caller starts from what storage holds, and the gap to the
    /// engine's older head does not read as a move, neither backwards
    /// before the engine catches up nor forwards when it does.
    #[test]
    fn a_change_the_engine_had_not_reloaded_at_a_first_contact_is_never_news_to_it() {
        // Closed list of first operations that do not look at the
        // changed mem: nothing at all, a read scoped to the other mem,
        // an explicit reload of the other mem.
        for kind in 0..3 {
            let (_tmp, root) = caller_fixture();
            let mut engine = engine_from_workspace_root(&root).unwrap();
            let mut sibling = engine_from_workspace_root(&root).unwrap();
            let ctx = memstead_base::vcs::CommitContext::internal();
            assert!(operate(&mut engine, "a", None, None).0.is_empty());

            sibling
                .create_entity_with_ctx(spec("notes", "Before"), &ctx)
                .unwrap();
            let first = match kind {
                0 => operate_without_the_reload_step(&mut engine, "new", None),
                1 => operate(&mut engine, "new", Some("other"), None).0,
                _ => operate_without_the_reload_step(&mut engine, "new", Some(Some("other"))),
            };
            assert!(first.is_empty(), "kind {kind}: {first:?}");

            // While the engine still holds the older head: no notice
            // backwards.
            let notices = operate_without_the_reload_step(&mut engine, "new", None);
            assert!(notices.is_empty(), "kind {kind}: {notices:?}");
            let (notices, _) = operate(&mut engine, "new", Some("other"), None);
            assert!(notices.is_empty(), "kind {kind}: {notices:?}");

            // The engine catches up: news to the caller that was there
            // before, not to the one that came after the change.
            let (notices, codes) = operate(&mut engine, "new", None, None);
            assert!(notices.is_empty(), "kind {kind}: {notices:?}");
            assert!(!codes.iter().any(|c| c == "MEM_RELOADED"), "{codes:?}");
            let (notices, _) = operate(&mut engine, "a", None, None);
            assert_eq!(notices.len(), 1, "kind {kind}: {notices:?}");
            assert_eq!(listed_ids(&notices[0]), ["notes--before"]);

            // From then on the new caller is told like anyone else.
            sibling
                .create_entity_with_ctx(spec("notes", "After"), &ctx)
                .unwrap();
            let (notices, _) = operate(&mut engine, "new", None, None);
            assert_eq!(notices.len(), 1, "kind {kind}: {notices:?}");
            assert_eq!(listed_ids(&notices[0]), ["notes--after"]);
        }
    }

    /// A caller's first contact hands out nothing, whatever its first
    /// operation is and whatever landed before it came: also when
    /// another process wrote in between and the first operation is a
    /// write to another mem, which reloads mem by mem. What landed
    /// before the caller came never reaches it later either. The same
    /// holds for a caller the cap had forgotten.
    #[test]
    fn a_first_contact_is_silent_whatever_landed_before_it() {
        let (_tmp, root) = caller_fixture();
        let mut engine = engine_from_workspace_root(&root).unwrap();
        let mut sibling = engine_from_workspace_root(&root).unwrap();
        let outside = |sibling: &mut Engine, title: &str| {
            sibling
                .create_entity_with_ctx(
                    spec("notes", title),
                    &memstead_base::vcs::CommitContext::internal(),
                )
                .unwrap();
        };

        // First operations of a never-seen caller, each after an outside
        // write the engine has not reloaded yet. Closed list: a write to
        // the other mem, a write to the same mem, a read scoped to the
        // other mem, an unscoped read.
        type First = (
            &'static str,
            Option<&'static str>,
            Option<(&'static str, &'static str)>,
        );
        let firsts: [First; 4] = [
            (
                "new-write-other",
                Some("other"),
                Some(("other", "By New One")),
            ),
            (
                "new-write-same",
                Some("notes"),
                Some(("notes", "By New Two")),
            ),
            ("new-read-other", Some("other"), None),
            ("new-read-all", None, None),
        ];
        for (n, (caller, scope, create)) in firsts.into_iter().enumerate() {
            outside(&mut sibling, &format!("Before {n}"));
            let (notices, _) = operate(&mut engine, caller, scope, create);
            assert!(
                notices.is_empty(),
                "{caller}: first contact told {notices:?}"
            );
            // Nor later: operations that touch every mem bring no notice
            // for what landed before the caller came.
            let (notices, _) = operate(&mut engine, caller, None, None);
            assert!(notices.is_empty(), "{caller}: told later {notices:?}");
            let (notices, _) = operate(&mut engine, caller, Some("notes"), None);
            assert!(notices.is_empty(), "{caller}: told later {notices:?}");
        }

        // From then on each is told like anyone else: about what landed
        // after its own first contact, and nothing from before it.
        outside(&mut sibling, "After All");
        let (notices, _) = operate(&mut engine, "new-read-all", None, None);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(listed_ids(&notices[0]), ["notes--after-all"]);
        let (notices, _) = operate(&mut engine, "new-write-other", None, None);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(
            listed_ids(&notices[0]),
            [
                "notes--after-all",
                "notes--before-1",
                "notes--before-2",
                "notes--before-3",
                "notes--by-new-two"
            ],
            "everything since its first contact, and not the write that preceded it"
        );

        // A caller the cap forgot returns with a write to the other mem
        // after an outside write: a first contact again.
        assert!(operate(&mut engine, "early", None, None).0.is_empty());
        for n in 0..memstead_base::MAX_TRACKED_CALLERS {
            operate(&mut engine, &format!("crowd-{n}"), Some("other"), None);
        }
        outside(&mut sibling, "While Forgotten");
        let (notices, _) = operate(
            &mut engine,
            "early",
            Some("other"),
            Some(("other", "Early Returns")),
        );
        assert!(
            notices.is_empty(),
            "a forgotten caller's return told {notices:?}"
        );
        let (notices, _) = operate(&mut engine, "early", None, None);
        assert!(notices.is_empty(), "and nothing stale later: {notices:?}");
    }
}
