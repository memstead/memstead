//! Top-level test-support helpers used across the workspace's
//! integration tests.
//!
//! `Engine::init_with_settings` fail-fasts when the workspace lacks
//! `<workspace_root>/mem-repo/.git/`. Tests that need the engine to
//! boot use [`init_real_mem_repo`] to lay down a real `mem-repo-git`
//! carrying `main` (operator docs) plus `__MEMSTEAD` (with
//! `mems/<name>/config.json` blobs) plus an empty-tree initial commit
//! on each requested mem branch.
//!
//! # Visibility
//!
//! The module is compiled in for the crate's own `cfg(test)` builds and
//! for downstream consumers that opt into the `test-support` Cargo
//! feature in their `[dev-dependencies]` — that is how `memstead-cli`,
//! `memstead-mcp`, and `memstead-swift` reach in.
//!
//! # Negative-case coverage
//!
//! The fail-fast itself is exercised by
//! `tests::engine_init_against_workspace_without_mem_repo_fails` in
//! `memstead-git-branch/src/lib.rs`. That test deliberately avoids calling this
//! helper so the missing-dir error path stays covered.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use memstead_base::{
    FileWorkspaceStore, Mount, MountCapability, MountLifecycle, MountStorage, Workspace,
    WorkspaceStoreAdapter,
};

/// Write `.memstead/workspace.toml` + `.memstead/state/mounts.json` carrying
/// one git-branch mount per `(mem, branch)` pair so the new-layout
/// loader (`Engine::from_workspace_root` / `engine_from_workspace_root`)
/// discovers the workspace. `branch` defaults to `mem` when omitted.
/// `mems` carries `(mem_name, branch, schema_pin_str)` per mount.
/// Reading the schema pin off the just-seeded gitdir would work too,
/// but callers already know it — passing it through keeps the helper
/// independent of the read path (and side-steps `read_config_at_gitdir`'s
/// dependency on the gitdir already being committed).
fn seed_new_layout(workspace_root: &Path, gitdir: &Path, mems: &[(&str, &str, &str)]) {
    let memstead = workspace_root.join(".memstead");
    std::fs::create_dir_all(&memstead).unwrap();
    let toml_path = memstead.join("workspace.toml");
    if !toml_path.is_file() {
        // schemas_dir is a top-level key — emit it BEFORE the
        // [persistence_adapter] section so TOML's greedy section
        // parsing doesn't fold it into that table.
        let mut head = String::from("format = \"memstead-git-branch-2\"\n");
        head.push_str("\n[persistence_adapter]\nname = \"file-two-layer\"\n");
        std::fs::write(&toml_path, head).unwrap();
    }
    let mounts: Vec<Mount> = mems
        .iter()
        .map(|(mem, branch, schema)| {
            let pin: memstead_schema::SchemaRef = schema
                .parse()
                .unwrap_or_else(|e| panic!("test seed: invalid schema pin {schema:?}: {e}"));
            Mount {
                mem: mem.to_string(),
                schema: Some(pin),
                storage: MountStorage::GitBranch {
                    gitdir: gitdir.to_path_buf(),
                    branch: branch.to_string(),
                },
                capability: MountCapability::Write,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            }
        })
        .collect();
    let workspace = Workspace {
        mounts,
        settings: Default::default(),
    };
    FileWorkspaceStore::new()
        .save_state(workspace_root, &workspace)
        .unwrap();
}

/// Initialise `<workspace_root>/mem-repo/.git/` as a real bare repo
/// carrying `main` (operator-facing docs, empty in tests) plus
/// `__SYSTEM` (with `<name>/config.json` blobs for every mem) plus an
/// empty-tree initial commit on each per-mem branch
/// (`refs/heads/<name>`).
///
/// `mems` is `&[(name, schema_name)]`. Each entry produces:
/// - one `<name>/config.json` blob on `__SYSTEM` of the form
///   `{"schema": "<schema>"}`, and
/// - one branch `refs/heads/<name>` with an empty-tree initial commit.
///
/// Mirrors the K-shape registry layout so `mem_repo_config::read_config`
/// and `mem_init_from_branch` both work against the result.
///
/// Idempotent: running against an existing bare repo with the same
/// shape is a no-op (the gix repo open succeeds, the per-branch ref
/// exists). Running against an existing repo with a *different* shape
/// will append commits — tests should pass a fresh `TempDir` per run.
///
/// Panics on any gix or fs failure, mirroring the `.unwrap()` pattern
/// the rest of the integration tests use.
pub fn init_real_mem_repo(workspace_root: &Path, mems: &[(&str, &str)]) -> PathBuf {
    // Process-wide lock so two parallel test threads racing on the
    // same workspace_root (e.g. multiple `PathBuf::from(".")` callers)
    // do not interleave between the `gix::open` probe and the
    // subsequent `gix::init_bare`.
    static INIT_LOCK: Mutex<()> = Mutex::new(());

    let gitdir = workspace_root.join("mem-repo").join(".git");
    std::fs::create_dir_all(&gitdir).unwrap_or_else(|e| {
        panic!(
            "failed to create test mem-repo at {}: {e}",
            gitdir.display()
        )
    });

    let _guard = INIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let repo = match gix::open(&gitdir) {
        Ok(r) => r,
        Err(_) => gix::init_bare(&gitdir).unwrap_or_else(|e| {
            panic!(
                "failed to initialise bare mem-repo-git at {}: {e}",
                gitdir.display()
            )
        }),
    };

    // Skip the seed if `__SYSTEM` already exists — idempotent re-runs.
    if matches!(repo.try_find_reference("refs/heads/__SYSTEM"), Ok(Some(_))) {
        return workspace_root.to_path_buf();
    }

    let actor = gix::actor::Signature {
        name: "test".into(),
        email: "test@example.com".into(),
        time: gix::date::Time {
            seconds: 0,
            offset: 0,
        },
    };

    // Seed `main` with empty tree (operator-facing docs branch — content
    // not used by engine).
    {
        let mut buf = gix::date::parse::TimeBuf::default();
        let actor_ref = actor.to_ref(&mut buf);
        repo.commit_as(
            actor_ref,
            actor_ref,
            "refs/heads/main",
            "seed main",
            repo.empty_tree().id().detach(),
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();
    }

    // Seed `__SYSTEM` with one `<name>/config.json` blob per mem.
    let mut system_editor = repo.empty_tree().edit().unwrap();
    for (name, schema) in mems {
        let config = format!(r#"{{"schema": "{schema}"}}"#);
        let blob = repo.write_blob(config.as_bytes()).unwrap().detach();
        system_editor
            .upsert(
                format!("{name}/config.json"),
                gix::objs::tree::EntryKind::Blob,
                blob,
            )
            .unwrap();
    }
    let system_tree = system_editor.write().unwrap().detach();
    let mut buf = gix::date::parse::TimeBuf::default();
    let actor_ref = actor.to_ref(&mut buf);
    repo.commit_as(
        actor_ref,
        actor_ref,
        "refs/heads/__SYSTEM",
        "seed __SYSTEM",
        system_tree,
        Vec::<gix::ObjectId>::new(),
    )
    .unwrap();

    let empty_tree = repo.empty_tree().id().detach();
    for (name, _) in mems {
        let mut buf = gix::date::parse::TimeBuf::default();
        let actor_ref = actor.to_ref(&mut buf);
        let ref_name = format!("refs/heads/{name}");
        repo.commit_as(
            actor_ref,
            actor_ref,
            ref_name.as_str(),
            format!("seed {name}"),
            empty_tree,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();
    }

    // Project __SYSTEM onto __MEMSTEAD so post-s140 reads land.
    crate::storage_memstead::migrate_to_memstead_ref(&gitdir).unwrap();

    seed_new_layout(
        workspace_root,
        &gitdir,
        &mems
            .iter()
            .map(|(n, schema)| (*n, *n, *schema))
            .collect::<Vec<_>>(),
    );

    workspace_root.to_path_buf()
}

/// Auto-detect every disk-shaped mem directory directly under
/// `workspace_root` (any subdir carrying `.memstead/config.json`), then
/// seed `<workspace_root>/mem-repo/.git/` from those dirs. Returns a
/// default `WorkspaceSettings`.
pub fn auto_seeded_settings(workspace_root: &Path) -> memstead_base::WorkspaceSettings {
    auto_seed_with_settings(workspace_root, memstead_base::WorkspaceSettings::default())
}

/// Like [`auto_seeded_settings`] but preserves caller-supplied
/// `WorkspaceSettings` overrides (e.g. `mem_create_rules`,
/// `schemas_dir`).
pub fn auto_seed_with_settings(
    workspace_root: &Path,
    settings: memstead_base::WorkspaceSettings,
) -> memstead_base::WorkspaceSettings {
    let mut mems: Vec<(std::path::PathBuf, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(workspace_root) {
        for entry in rd.flatten() {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            // Skip the mem-repo gitdir itself.
            if p.file_name().and_then(|s| s.to_str()) == Some("mem-repo") {
                continue;
            }
            let cfg_path = p.join(".memstead").join("config.json");
            if !cfg_path.is_file() {
                continue;
            }
            // Configs no longer carry an in-config `name` field. The
            // mem leaf identifier is the disk-path basename.
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                mems.push((p.clone(), name.to_string()));
            }
        }
    }
    mems.sort_by(|a, b| a.1.cmp(&b.1));
    let refs: Vec<(&Path, &str)> = mems
        .iter()
        .map(|(p, n)| (p.as_path(), n.as_str()))
        .collect();
    init_real_mem_repo_from_disk(workspace_root, &refs);
    settings
}

/// Re-seed `<workspace_root>/mem-repo/.git/` from disk after the disk
/// shape has changed. Deletes the existing gitdir and re-runs
/// [`init_real_mem_repo_from_disk`].
///
/// Useful for tests that mutate disk mem content via `fs::write`
/// between `init` and `engine.reload()` — without re-seeding, the
/// dispatcher's GitTree read path keeps returning the stale tree
/// state. Mirrors disk-fall-through behaviour where reload re-reads
/// the live disk content.
pub fn reseed_mem_repo_from_disk(workspace_root: &Path, mems: &[(&Path, &str)]) -> PathBuf {
    let gitdir = workspace_root.join("mem-repo").join(".git");
    let _ = std::fs::remove_dir_all(&gitdir);
    init_real_mem_repo_from_disk(workspace_root, mems)
}

/// Migration helper: bootstrap `<workspace_root>/mem-repo/.git/` from
/// the layout of disk-shaped mem directories already on disk.
///
/// For each `(mem_dir, name)` entry, reads the disk-shape mem at
/// `<workspace_root>/<mem_dir>/`:
/// - `<mem_dir>/.memstead/config.json` (if present) → `__SYSTEM:<name>/config.json`
/// - every `*.md` file under `<mem_dir>/` (recursive) → branch tree
///
/// Use this from tests that pre-populate disk mems via `fs::write`
/// before initialising the engine. The helper preserves the entity
/// content so dispatcher GitTree reads return the same data the disk
/// path used to surface.
///
/// `name` is the mem name (matches `MemInit.name`). It is used
/// for the branch ref and the `<name>/config.json` blob on `__SYSTEM`.
pub fn init_real_mem_repo_from_disk(workspace_root: &Path, mems: &[(&Path, &str)]) -> PathBuf {
    let triples: Vec<(&Path, &str, &str)> = mems
        .iter()
        .map(|(dir, name)| (*dir, *name, *name))
        .collect();
    init_real_mem_repo_from_disk_with_paths(workspace_root, &triples)
}

/// Hierarchical-layout variant of [`init_real_mem_repo_from_disk`].
///
/// `mems` is `&[(mem_dir, leaf, full_path)]`. The branch ref is
/// sealed at `refs/heads/<full_path>` and the `__SYSTEM` config blob
/// lands at `<full_path>/config.json`, but the engine still references
/// the mem by its `leaf` name (matches `MemInit.name`). Pass
/// `full_path = leaf` to get flat-layout behaviour identical to
/// [`init_real_mem_repo_from_disk`].
///
/// Use this from tests that need to exercise the production-shape
/// hierarchical workspace (e.g. `memstead/engine` branch with leaf
/// `engine`). Without a hierarchical fixture the per-mem drift
/// detection helper that maps leaf → branch ref looks identical to
/// the flat case and will not catch ref-format regressions.
pub fn init_real_mem_repo_from_disk_with_paths(
    workspace_root: &Path,
    mems: &[(&Path, &str, &str)],
) -> PathBuf {
    static INIT_LOCK: Mutex<()> = Mutex::new(());

    let gitdir = workspace_root.join("mem-repo").join(".git");
    std::fs::create_dir_all(&gitdir).unwrap_or_else(|e| {
        panic!(
            "failed to create test mem-repo at {}: {e}",
            gitdir.display()
        )
    });

    let _guard = INIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let repo = match gix::open(&gitdir) {
        Ok(r) => r,
        Err(_) => gix::init_bare(&gitdir).unwrap_or_else(|e| {
            panic!(
                "failed to initialise bare mem-repo-git at {}: {e}",
                gitdir.display()
            )
        }),
    };

    if matches!(repo.try_find_reference("refs/heads/__SYSTEM"), Ok(Some(_))) {
        return workspace_root.to_path_buf();
    }

    let actor = gix::actor::Signature {
        name: "test".into(),
        email: "test@example.com".into(),
        time: gix::date::Time {
            seconds: 0,
            offset: 0,
        },
    };

    // main: empty (operator-facing docs branch — content not used by engine).
    {
        let mut buf = gix::date::parse::TimeBuf::default();
        let actor_ref = actor.to_ref(&mut buf);
        repo.commit_as(
            actor_ref,
            actor_ref,
            "refs/heads/main",
            "seed main",
            repo.empty_tree().id().detach(),
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();
    }

    // __SYSTEM: read each mem_dir's `.memstead/config.json` (or synthesise
    // a default) and seed it as `<full_path>/config.json` so the
    // resolver maps `<leaf>` → `<full_path>` correctly.
    let mut system_editor = repo.empty_tree().edit().unwrap();
    for (mem_dir, _leaf, full_path) in mems {
        let cfg_path = mem_dir.join(".memstead").join("config.json");
        let cfg_bytes = match std::fs::read(&cfg_path) {
            Ok(b) => b,
            Err(_) => r#"{"schema": "default@1.0.0"}"#.to_string().into_bytes(),
        };
        let blob = repo.write_blob(&cfg_bytes).unwrap().detach();
        system_editor
            .upsert(
                format!("{full_path}/config.json"),
                gix::objs::tree::EntryKind::Blob,
                blob,
            )
            .unwrap();
    }
    let system_tree = system_editor.write().unwrap().detach();
    let mut buf = gix::date::parse::TimeBuf::default();
    let actor_ref = actor.to_ref(&mut buf);
    repo.commit_as(
        actor_ref,
        actor_ref,
        "refs/heads/__SYSTEM",
        "seed __SYSTEM",
        system_tree,
        Vec::<gix::ObjectId>::new(),
    )
    .unwrap();

    // Per-mem branches: walk each mem_dir for `*.md` files and
    // upsert them into the branch tree at their relative path. Branch
    // ref is `refs/heads/<full_path>` (flat == hierarchical when
    // `full_path == leaf`).
    let empty_tree = repo.empty_tree().id().detach();
    for (mem_dir, _leaf, full_path) in mems {
        let mut entities: Vec<(String, Vec<u8>)> = Vec::new();
        if mem_dir.is_dir() {
            collect_md_entities(mem_dir, mem_dir, &mut entities);
        }
        let tree_id = if entities.is_empty() {
            empty_tree
        } else {
            let mut editor = repo.empty_tree().edit().unwrap();
            for (rel, bytes) in &entities {
                let blob = repo.write_blob(bytes).unwrap().detach();
                editor
                    .upsert(rel.clone(), gix::objs::tree::EntryKind::Blob, blob)
                    .unwrap();
            }
            editor.write().unwrap().detach()
        };

        let mut buf = gix::date::parse::TimeBuf::default();
        let actor_ref = actor.to_ref(&mut buf);
        let ref_name = format!("refs/heads/{full_path}");
        repo.commit_as(
            actor_ref,
            actor_ref,
            ref_name.as_str(),
            format!("seed {full_path}"),
            tree_id,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();
    }

    // Project the just-written __SCHEMAS + __SYSTEM content onto the
    // unified `__MEMSTEAD` ref. The migrator is idempotent — re-runs are
    // no-ops — and is the canonical projection logic, so test
    // fixtures stay in lockstep with what `engine_from_workspace_root`
    // produces at boot (s136). Without this, tests that bypass the
    // boot path (e.g., direct `Engine::from_mounts` calls) would have
    // `__MEMSTEAD` missing and the schema reader's legacy `__SCHEMAS`
    // fallback (retired in s139) would start failing. Per-fixture
    // failures collapse via `unwrap()` — fixture setup is hard-fail.
    crate::storage_memstead::migrate_to_memstead_ref(&gitdir).unwrap();

    // For each mem, lift the schema pin from `<mem_dir>/.memstead/config.json`
    // (the same source the just-written `__SYSTEM` blob used). When the
    // file is missing or the field is absent we fall back to the
    // sentinel "default" pin — matches the pre-deletion `LegacyProStore`
    // behaviour and keeps the test surface stable.
    let owned: Vec<(String, String, String)> = mems
        .iter()
        .map(|(mem_dir, leaf, full)| {
            let pin = std::fs::read_to_string(mem_dir.join(".memstead").join("config.json"))
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| {
                    v.get("schema")
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| "default@1.0.0".to_string());
            ((*leaf).to_string(), (*full).to_string(), pin)
        })
        .collect();
    let refs: Vec<(&str, &str, &str)> = owned
        .iter()
        .map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()))
        .collect();
    seed_new_layout(workspace_root, &gitdir, &refs);

    workspace_root.to_path_buf()
}

fn collect_md_entities(root: &Path, current: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(rd) = std::fs::read_dir(current) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        // Skip the `.memstead/` and any `.git/` subtrees inside the mem.
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_md_entities(root, &path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("md")
            && let Ok(rel) = path.strip_prefix(root)
            && let Ok(bytes) = std::fs::read(&path)
        {
            out.push((rel.to_string_lossy().into_owned(), bytes));
        }
    }
}

/// `(name, schema, [(path, content)])` seed tuple consumed by
/// [`init_real_mem_repo_with_entities`].
pub type MemSeed<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

/// Initialise `<workspace_root>/mem-repo/.git/` as a real bare repo
/// like [`init_real_mem_repo`], but additionally writes the supplied
/// entity blobs into each mem's branch tree. `mems_with_entities`
/// is `&[(name, schema, &[(path, content)])]` — the path/content tuples
/// land at the corresponding paths on the per-mem branch's tree.
///
/// Use this when the test asserts on entity content surfaced through
/// the dispatcher's GitTree read path. For the simpler "engine boots,
/// no entities" case use [`init_real_mem_repo`].
pub fn init_real_mem_repo_with_entities(
    workspace_root: &Path,
    mems_with_entities: &[MemSeed<'_>],
) -> PathBuf {
    static INIT_LOCK: Mutex<()> = Mutex::new(());

    let gitdir = workspace_root.join("mem-repo").join(".git");
    std::fs::create_dir_all(&gitdir).unwrap_or_else(|e| {
        panic!(
            "failed to create test mem-repo at {}: {e}",
            gitdir.display()
        )
    });

    let _guard = INIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let repo = match gix::open(&gitdir) {
        Ok(r) => r,
        Err(_) => gix::init_bare(&gitdir).unwrap_or_else(|e| {
            panic!(
                "failed to initialise bare mem-repo-git at {}: {e}",
                gitdir.display()
            )
        }),
    };

    if matches!(repo.try_find_reference("refs/heads/__SYSTEM"), Ok(Some(_))) {
        return workspace_root.to_path_buf();
    }

    let actor = gix::actor::Signature {
        name: "test".into(),
        email: "test@example.com".into(),
        time: gix::date::Time {
            seconds: 0,
            offset: 0,
        },
    };

    // main: empty (operator-facing docs branch).
    {
        let mut buf = gix::date::parse::TimeBuf::default();
        let actor_ref = actor.to_ref(&mut buf);
        repo.commit_as(
            actor_ref,
            actor_ref,
            "refs/heads/main",
            "seed main",
            repo.empty_tree().id().detach(),
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();
    }

    // __SYSTEM: per-mem `<name>/config.json` blobs.
    let mut system_editor = repo.empty_tree().edit().unwrap();
    for (name, schema, _) in mems_with_entities {
        let config = format!(r#"{{"schema": "{schema}"}}"#);
        let blob = repo.write_blob(config.as_bytes()).unwrap().detach();
        system_editor
            .upsert(
                format!("{name}/config.json"),
                gix::objs::tree::EntryKind::Blob,
                blob,
            )
            .unwrap();
    }
    let system_tree = system_editor.write().unwrap().detach();
    let mut buf = gix::date::parse::TimeBuf::default();
    let actor_ref = actor.to_ref(&mut buf);
    repo.commit_as(
        actor_ref,
        actor_ref,
        "refs/heads/__SYSTEM",
        "seed __SYSTEM",
        system_tree,
        Vec::<gix::ObjectId>::new(),
    )
    .unwrap();

    let empty_tree = repo.empty_tree().id().detach();
    for (name, _, entities) in mems_with_entities {
        let tree_id = if entities.is_empty() {
            empty_tree
        } else {
            let mut editor = repo.empty_tree().edit().unwrap();
            for (path, content) in *entities {
                let blob = repo.write_blob(content.as_bytes()).unwrap().detach();
                editor
                    .upsert((*path).to_string(), gix::objs::tree::EntryKind::Blob, blob)
                    .unwrap();
            }
            editor.write().unwrap().detach()
        };

        let mut buf = gix::date::parse::TimeBuf::default();
        let actor_ref = actor.to_ref(&mut buf);
        let ref_name = format!("refs/heads/{name}");
        repo.commit_as(
            actor_ref,
            actor_ref,
            ref_name.as_str(),
            format!("seed {name}"),
            tree_id,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();
    }

    // Project __SYSTEM onto __MEMSTEAD so post-s140 reads land.
    crate::storage_memstead::migrate_to_memstead_ref(&gitdir).unwrap();

    seed_new_layout(
        workspace_root,
        &gitdir,
        &mems_with_entities
            .iter()
            .map(|(name, schema, _)| (*name, *name, *schema))
            .collect::<Vec<_>>(),
    );

    workspace_root.to_path_buf()
}
