//! Unified engine.
//!
//! **One [`Engine`] type, three storage backends**: the engine sits
//! above [`VaultBackend`] and routes reads / writes to the backend
//! named by each mount's vault. The MCP filesystem-vault server
//! (`memstead_mcp::filesystem_server::FilesystemMcpServer`), every CLI
//! basis subcommand, and the macOS UniFFI consumer all reach the
//! engine through [`Engine::from_workspace_root`] (basis: folder +
//! archive backends) or `memstead_git_branch::engine_from_workspace_root`
//! (pro: adds git-branch).
//!
//! ## Routing
//!
//! Each mount holds one vault. Lookup is by vault name: the first
//! mount whose `vault` field equals the requested name wins. One mount
//! per vault is enforced — duplicates are a configuration bug, not a
//! feature, and the constructor rejects them.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use memstead_schema::Schema;

use crate::backend::{BackendError, VaultBackend};
use crate::graph::LouvainOutput;
use crate::ops::WarningHint;
#[cfg(not(target_arch = "wasm32"))]
use crate::search_index::VaultIndex;
use crate::store::Store;
use crate::vault::VaultRouterSnapshot;
use crate::workspace::{Mount, WorkspaceSettings};

pub mod apply_commit;
pub mod archive;
pub mod boot;
pub mod drift;
pub mod error;
pub mod events;
#[cfg(feature = "file-watcher")]
pub mod file_watcher;
pub mod lifecycle;
pub mod mutation;
pub mod outcomes;
pub mod query;

pub use archive::FromArchiveBytesError;
pub use error::{
    BootError, EngineError, INLINE_LIST_CAP, MissingWikiLink, ReferrerInfo,
    SchemaSourceDiagnostic, format_inline_list_overflow,
};
pub use events::{EventCallback, SubscriptionHandle, VaultChangedEvent};
#[cfg(feature = "tokio")]
pub use events::DEFAULT_BROADCAST_CAPACITY;
#[cfg(feature = "file-watcher")]
pub use file_watcher::{FileWatcherError, VaultRepoWatcher, watch_vault_repo};
pub use mutation::delete::DeleteReferrers;
pub use mutation::{PATCH_OLD_NOT_FOUND_CONTENT_CAP, RELATIONSHIP_CYCLE_PATH_CAP};
pub use outcomes::{
    SetSchemaOutcome, SetSchemaResult,
    CreateEntityArgs, CreateEntityOutcome, DeleteEntityArgs, DeleteEntityOutcome,
    RelateAction, RelateEntityArgs, RelateEntityOutcome, RenameEntityArgs, RenameEntityOutcome,
    UpdateEntityArgs, UpdateEntityOutcome,
};

pub use boot::{SchemaResolver, resolve_builtin_schema_pin_pub};

/// One vault attachment, paired with the backend that serves it.
/// Constructed by [`Engine::from_mounts`] and held internally.
struct MountedBackend {
    mount: Mount,
    backend: Box<dyn VaultBackend>,
    /// Last cursor returned by `backend.current_head()`. Seeded in
    /// [`Engine::from_mounts`]; refreshed by
    /// [`Engine::reload_if_stale`] after a successful reload.
    /// `None` means the backend doesn't track a head (folder /
    /// archive) — drift detection is a no-op for this mount.
    last_known_head: Option<String>,
    /// Per-vault `.memstead/config.json` payload — surfaces via
    /// [`Engine::vault_config_for`] for handlers that need
    /// `write_guidance` / `extra` (`memstead_health
    /// { include_config: true }`'s per-vault detail block).
    ///
    /// Loaded at construction for folder backends (read from
    /// `<path>/.memstead/config.json`). Git-branch + archive backends
    /// carry `None` for now — the read-from-storage-backend path
    /// lifts in a follow-up session.
    vault_config: Option<memstead_schema::config::VaultConfig>,
    /// Per-vault authoring-provenance payload read from the archive's
    /// `.memstead/provenance.json` at construction (via
    /// [`crate::backend::VaultBackend::read_archive_provenance`]). `None`
    /// when the backend carries no provenance member (a pre-provenance
    /// archive, or a backend that does not surface one) — surfaced as
    /// provenance-absent via [`Engine::archive_provenance_for`]. A
    /// malformed payload is downgraded to `None` rather than failing the
    /// mount: the member is additive.
    archive_provenance: Option<memstead_schema::ArchiveProvenance>,
}

/// Unified engine. Holds a list of mounted backends and routes
/// vault-named operations to the right one.
///
/// `Send` so the engine can sit behind a `Mutex` (today's pattern
/// in the MCP server). The trait object's `Send + Sync` bound on
/// `VaultBackend` keeps the inner backends thread-safe; the engine
/// itself is single-threaded by design (the lazy memos are
/// `OnceCell`, which is `!Sync`).
///
/// `Debug` is hand-written to avoid requiring `Debug` on the
/// `dyn VaultBackend` trait object — backend impls are free to
/// stay non-`Debug`.
///
/// ## Load-on-init
///
/// `Engine::from_mounts` walks each backend at construction time
/// (`list_entities` + `read_entity` + parse) and populates a single
/// shared [`Store`] with entities and edges from every mount. Each
/// mount's schema resolves from its own pin (the backend config's
/// schema, or the mount-record assertion as fallback) through the
/// `SchemaResolver`, so `schemas` holds genuinely heterogeneous
/// schemas in a multi-schema workspace. Per-file errors don't fail
/// construction; they collect into [`Engine::load_errors`] for the
/// operator to inspect.
pub struct Engine {
    mounts: Vec<MountedBackend>,
    store: Store,
    schemas: HashMap<String, Arc<Schema>>,
    /// Workspace-authored schemas loaded from
    /// `WorkspaceSettings.schemas_dir` at construction. Distinct from
    /// `schemas` (per-vault, only schemas pinned by a mount): this
    /// catalogue carries every workspace-loaded schema regardless of
    /// whether a vault pins it. Surfaced via
    /// [`Self::workspace_schemas`] for handlers that need to enumerate
    /// schemas referenced by `vault_create_rules.schemas[]` but not
    /// pinned by any vault — `memstead_overview` lists them in `## Schemas`
    /// so an agent sees what could be pinned. Empty when no
    /// `schemas_dir` was passed.
    workspace_schemas: Vec<Arc<Schema>>,
    /// Embedded built-in schemas loaded once at boot from
    /// `memstead_schema::builtins::load_builtin_schemas()`. The boot path
    /// uses this catalogue to resolve each mount's schema pin; storing
    /// it on the engine lets read handlers (MCP's `memstead_schema`,
    /// `memstead_overview`'s `## Schemas` rendering) surface every built-in
    /// without re-walking the embedded directory. Schemas declared in
    /// `workspace_schemas` shadow built-ins on `(name, version)`
    /// collision — handlers walking both lists must check workspace
    /// first.
    builtin_schemas: Vec<Arc<Schema>>,
    load_errors: Vec<(PathBuf, String)>,
    /// Lazily-computed Louvain community detection across the
    /// engine-wide store. Populated on first call to
    /// [`Self::communities`]; invalidated by
    /// [`Self::invalidate_communities`] which every mutation method
    /// calls after a successful write. `OnceCell` is `!Sync`; the
    /// engine is `Send` (it is moved into a `Mutex` by every consumer)
    /// but not `Sync`.
    community_memo: OnceCell<LouvainOutput>,
    /// Lazily-computed per-vault search index map. Built on first call
    /// to [`Self::search_indexes`] via [`build_all`]; invalidated by
    /// [`Self::invalidate_search_indexes`] alongside the community
    /// cache so every mutation triggers a fresh build on the next
    /// search. Absent on `wasm32` targets — search lives behind the
    /// bridge (see `EngineError::SearchUnavailable`).
    #[cfg(not(target_arch = "wasm32"))]
    search_indexes_memo: OnceCell<HashMap<String, VaultIndex>>,
    /// Workspace-level operator policy — vault create/delete rules,
    /// cross-vault link permissions. Defaults to empty; populated via
    /// [`Self::set_settings`] when [`Self::from_workspace_root`] (or
    /// the pro counterpart) reads `.memstead/workspace.toml`. Surfaced
    /// read-only via [`Self::settings`] for MCP handlers and other
    /// consumers.
    settings: WorkspaceSettings,
    /// Lazily-compiled [`crate::vault_management::CreateRuleSet`] over
    /// `settings.vault_create_rules`. Built on first
    /// [`Self::cross_vault_link_allowed`] call that needs synthesis;
    /// invalidated by [`Self::set_settings`] (so a fresh policy
    /// re-compiles on the next call). Compilation errors are logged
    /// and the cache stays empty — synthesis is best-effort, the
    /// resolver falls back to explicit-policy resolution. Operators
    /// who want hard validation pre-compile via
    /// [`crate::vault_management::CreateRuleSet::new`] before passing
    /// settings.
    create_rule_set_memo: OnceCell<crate::vault_management::CreateRuleSet>,
    /// Workspace root path — set when the engine boots from a
    /// workspace store ([`Self::from_workspace_root`] or the pro
    /// counterpart). `None` for tests + ad-hoc consumers that build
    /// the engine directly from a mount list. Surfaced via
    /// [`Self::workspace_root`] for handlers that need filesystem
    /// context (e.g. [`Self::health`]'s outer-repo .gitignore
    /// check).
    workspace_root: Option<PathBuf>,
    /// Typed warnings surfaced during vault load — drift findings
    /// like [`WarningHint::SuspiciousNestedPrefix`] and
    /// [`WarningHint::DuplicateSectionHeading`] that the loader
    /// pipeline collects per entity. Empty for the V1 unified
    /// engine; the field is in place so handlers and the health
    /// surface can include them when the loader pipeline grows the
    /// warning generators.
    load_warnings: Vec<WarningHint>,
    /// Pipeline configs (Medium / Facet / Projection / Ingest) loaded
    /// from the workspace store at boot. Empty for engines built via
    /// `from_mounts*` (tests, in-memory consumers) and for any workspace
    /// that declares no pipelines; the workspace-root boot paths
    /// (`from_workspace_root` and the pro counterpart) populate it via
    /// [`crate::pipeline_store::load_pipeline_configs`]. Read-only
    /// runtime surface — exposed through [`Self::pipeline_configs`]; the
    /// engine neither runs nor schedules pipelines (the ingest skill and
    /// future consumers do).
    pipeline_configs: crate::pipeline_store::PipelineConfigs,
    /// Runtime snapshot of writable / visible vaults. Derived from
    /// the mount list at construction: writable mounts
    /// (`MountCapability::Write`) register via `add_writable` with
    /// the storage's directory path (folder → `path`, git-branch →
    /// None, archive shouldn't be writable); read-only mounts
    /// register via `add_writable` (folder/git-branch) or
    /// `add_read_only` (archive). Used by MCP handlers that need the
    /// writable/visible roster + per-vault origin (`memstead_health
    /// include_config: true`, `memstead_overview`'s vault list,
    /// `memstead_vault_create`'s collision check).
    ///
    /// Wrapped in `Arc` so the COW-snapshot discipline — clone the
    /// snapshot, mutate the clone, swap the `Arc` — keeps writers
    /// and concurrent readers from contending on the live mount
    /// list.
    vault_router: Arc<VaultRouterSnapshot>,
    /// Backend factory — function pointer used by
    /// [`crate::vault_management::create_vault`] (and future runtime
    /// mount-add paths) to materialise a [`VaultBackend`] from a
    /// [`Mount`] declaration. Defaults to
    /// [`crate::workspace_store::instantiate_basis_backend`] so basis
    /// (folder + archive only) consumers work out of the box. Pro
    /// consumers swap in `memstead_git_branch::storage::instantiate_pro_backend`
    /// via [`Self::set_backend_factory`] after constructing the engine —
    /// `engine_from_workspace_root` does this once at boot. Function
    /// pointer (not `Box<dyn Fn>`) because the backend factory is
    /// stateless, `Send + Sync + Copy`, and one less allocation on the
    /// hot path matters for the multi-vault pattern this engine is
    /// designed around.
    backend_factory: BackendFactory,
    /// Git-branch ops bundle — function pointers for the per-mount
    /// operations whose implementations live in `memstead-git-branch`
    /// (and therefore can't sit on the `VaultBackend` trait without
    /// inverting the crate dependency). Pro boot
    /// (`memstead_git_branch::engine_from_workspace_root`) installs the
    /// bundle via [`Self::set_git_branch_ops`]; basis consumers leave
    /// it `None` and `Engine::changes_since` / `Engine::export_vault`
    /// fall through to the folder/archive-only branches.
    git_branch_ops: Option<GitBranchOps>,
    /// Per-vault subscriber registry for [`VaultChangedEvent`]s. Held
    /// behind `Arc<Mutex<_>>` so [`SubscriptionHandle`]s — which own
    /// the consumer's view of the subscription lifetime — can call
    /// back into the registry on `Drop` without a self-reference cycle
    /// to the engine. The emit path (in `record_self_write`) snapshots
    /// the per-vault callback list under the lock, releases the lock,
    /// and then invokes the callbacks — so a callback that re-enters
    /// the engine for a read does not deadlock against the registry.
    event_subscribers: Arc<std::sync::Mutex<events::SubscriberRegistry>>,
    /// Reload-before-operation notices accumulated by
    /// [`Self::reload_if_stale`] when an operation triggered a vault
    /// reload. Built at reload time — when the backend's current head
    /// equals the head we reloaded to, *before* any mutation in the
    /// same operation commits — so the delta describes only the
    /// sibling's change, never the engine's own follow-on write. The
    /// response layer drains them via
    /// [`Self::take_vault_changed_notices`] and attaches the structured
    /// `vault_changed` notice to the operation's response. Every entity
    /// op that can reload drains after; an undrained accumulation would
    /// leak into the next operation's response, so callers that reload
    /// must take.
    pending_vault_changed: Vec<crate::ops::VaultChangedNotice>,
}

/// Backend factory function pointer. Both flavours' existing
/// `instantiate_*_backend` functions match this signature, so the
/// type alias is what bridges the dependency direction (memstead-base
/// can't depend on memstead-git-branch) without an extra trait.
/// Stateless, `Send + Sync + Copy`.
pub type BackendFactory =
    fn(&Mount) -> Result<Box<dyn VaultBackend>, crate::workspace_store::InstantiateError>;

/// `Engine::changes_since` dispatch for git-branch mounts.
///
/// Signature matches `memstead_git_branch::ops::changes::changes_since` after
/// adapting the `Store` parameter away (the engine performs enrichment
/// downstream) and the `head_ref` parameter (`refs/heads/<branch>` is
/// constructed inside the impl from `branch`).
pub type GitBranchChangesSinceFn = fn(
    gitdir: &Path,
    branch: &str,
    vault: &str,
    since: &str,
    rename_similarity: f32,
) -> Result<crate::ops::BackendChanges, BackendError>;

/// `Engine::export_vault` dispatch for git-branch mounts.
///
/// Signature mirrors `memstead_git_branch::ops::export::export_vault_from_branch`.
pub type GitBranchExportFn = fn(
    gitdir: &Path,
    branch: &str,
    vault: &str,
    config: &memstead_schema::VaultConfig,
    output_path: &Path,
    workspace_root: Option<&Path>,
    workspace_schemas_dir: Option<&Path>,
    // Engine-sourced authoring-provenance payload bytes (from the mount's
    // `read_provenance` log) to embed at `.memstead/provenance.json`.
    // `None` when the vault carried no noted mutations.
    provenance_bytes: Option<&[u8]>,
) -> Result<crate::ops::VaultExportResult, BackendError>;

/// `Engine::export_vault_to_bytes` dispatch for git-branch mounts.
///
/// Signature mirrors `memstead_git_branch::ops::export::export_vault_from_branch_to_bytes`.
/// Symmetric to `GitBranchExportFn`: same inputs minus the output path,
/// returns archive bytes plus metadata instead of writing to disk.
pub type GitBranchExportToBytesFn = fn(
    gitdir: &Path,
    branch: &str,
    vault: &str,
    config: &memstead_schema::VaultConfig,
    workspace_root: Option<&Path>,
    workspace_schemas_dir: Option<&Path>,
    // Pre-built authoring-provenance payload bytes the engine sourced from
    // the mount's `read_provenance` log, to embed at
    // `.memstead/provenance.json`. `None` when the vault carried no noted
    // mutations. The engine sources it (it holds the backend); the hook
    // only embeds, keeping git history-walking out of the fn-pointer.
    provenance_bytes: Option<&[u8]>,
) -> Result<crate::ops::VaultExportBytes, BackendError>;

/// `Engine::diff` dispatch for git-branch mounts. Walks the two refs
/// inside the workspace's vault-repo gitdir, produces a per-entity
/// [`crate::ops::Diff`]. Refs are arbitrary `gix::rev_parse_single`
/// inputs — branch names, commit SHAs, tag names. Resolves each
/// independently so cross-branch (cross-vault) diffs work uniformly.
pub type GitBranchDiffFn = fn(
    gitdir: &Path,
    vault: &str,
    ref_a: &str,
    ref_b: &str,
    config: &crate::ops::DiffConfig,
) -> Result<crate::ops::Diff, BackendError>;

/// `Engine::fetch` dispatch for git-branch mounts.
pub type GitBranchFetchFn = fn(
    gitdir: &Path,
    remote: &str,
    refspecs: &[String],
) -> Result<crate::ops::FetchOutcome, BackendError>;

/// Read every `.md` blob at `ref_name` in `gitdir`, returning
/// `(relative_path, utf8_content)` pairs. Skips `.memstead/` engine-internal
/// entries and non-blob nodes. Used by the pre-merge schema-validation
/// pass `Engine::pull` and `Engine::push` run before they advance the
/// branch pointer / push to the remote.
pub type GitBranchReadTreeFn = fn(
    gitdir: &Path,
    ref_name: &str,
) -> Result<Vec<(String, String)>, BackendError>;

/// `Engine::pull` dispatch for git-branch mounts.
pub type GitBranchPullFn = fn(
    gitdir: &Path,
    remote: &str,
    vault: &str,
) -> Result<crate::ops::PullOutcome, BackendError>;

/// `Engine::push` dispatch for git-branch mounts.
pub type GitBranchPushFn = fn(
    gitdir: &Path,
    remote: &str,
    vault: &str,
    force: bool,
) -> Result<crate::ops::PushOutcome, BackendError>;

/// `Engine::branch_reset` dispatch for git-branch mounts. Returns the
/// outcome on success; surfaces `BackendError::Other` carrying an
/// in-band marker (`UNKNOWN_REF:<raw>` or
/// `PUSHED_COMMITS_PROTECTED:<sha,sha,...>`) the engine layer
/// un-marshals into typed `EngineError`s.
pub type GitBranchBranchResetFn = fn(
    gitdir: &Path,
    branch: &str,
    target_sha: &str,
) -> Result<crate::ops::BranchResetOutcome, BackendError>;

/// Residue-prune dispatch for git-branch mounts.
/// The `create_vault` orchestrator calls this when
/// `RecoveryAction::ForceOverwrite` is selected against pre-existing
/// storage residue. Drops `refs/heads/<branch_full_path>` and the
/// `__MEMSTEAD:vaults/<branch_full_path>/config.json` blob in one
/// ref-edit transaction (the same call the
/// `VaultBackend::delete_artifacts` impl wraps for delete-files
/// flows). Surfaces as a function pointer so `memstead-engine` can
/// drive a prune against an unmounted gitdir without depending on
/// `memstead-git-branch`.
pub type GitBranchPruneResidueFn = fn(
    gitdir: &Path,
    branch_full_path: &str,
) -> Result<(), BackendError>;

/// `Engine::install_schema` dispatch for the git-branch backend: write a
/// schema package (`(relative-path, bytes)` pairs) onto the workspace's
/// unified `__MEMSTEAD:schemas/<name>@<version>/` ref and return the
/// resulting commit sha. Mirrors
/// `memstead_git_branch::storage_memstead::write_schema_to_memstead_ref`.
pub type GitBranchWriteSchemaFn = fn(
    gitdir: &Path,
    name: &str,
    version: &str,
    files: &[(String, Vec<u8>)],
) -> Result<String, BackendError>;

/// Bundle of git-branch-specific op dispatchers. Installed on the
/// engine at pro boot. Each field is one ops-method that previously
/// lived on the `VaultBackend` trait; moving them off the trait keeps
/// the bytes-level primitive surface clean.
#[derive(Clone, Copy)]
pub struct GitBranchOps {
    pub changes_since: GitBranchChangesSinceFn,
    pub diff: GitBranchDiffFn,
    pub branch_reset: GitBranchBranchResetFn,
    pub fetch: GitBranchFetchFn,
    pub pull: GitBranchPullFn,
    pub push: GitBranchPushFn,
    pub read_tree: GitBranchReadTreeFn,
    pub export: GitBranchExportFn,
    pub export_to_bytes: GitBranchExportToBytesFn,
    pub prune_residue: GitBranchPruneResidueFn,
    pub write_schema: GitBranchWriteSchemaFn,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field(
                "vaults",
                &self
                    .mounts
                    .iter()
                    .map(|m| m.mount.vault.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}




#[cfg(test)]
mod in_memory_vault;

#[cfg(test)]
pub(super) mod test_helpers {
    use std::io::Write as _;
    use std::path::{Path, PathBuf};

    use memstead_schema::SchemaRef;

    use crate::backend::VaultBackend;
    use crate::storage::FilesystemVaultWriter;
    use crate::vcs::{Actor, ClientId};
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    use super::{CreateEntityArgs, CreateEntityOutcome, Engine, RelateEntityArgs};

    use indexmap::IndexMap;
    use tempfile::TempDir;

    pub(crate) fn pin(name: &str) -> SchemaRef {
        let version = match name {
            "default" => semver::Version::new(1, 0, 0),
            _ => semver::Version::new(0, 1, 0),
        };
        SchemaRef::new(name, version)
    }

    pub(crate) fn folder_mount(vault: &str, path: PathBuf) -> Mount {
        Mount {
            vault: vault.to_string(),
            schema: Some(pin("default")),
            storage: MountStorage::Folder { path },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        }
    }

    pub(crate) fn in_memory_mount(vault: &str) -> Mount {
        Mount {
            vault: vault.to_string(),
            schema: Some(pin("default")),
            storage: MountStorage::InMemory,
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        }
    }

    pub(crate) fn archive_mount(vault: &str, path: PathBuf) -> Mount {
        Mount {
            vault: vault.to_string(),
            schema: Some(pin("default")),
            storage: MountStorage::Archive { path },
            capability: MountCapability::ReadOnly,
            lifecycle: MountLifecycle::Lazy,
            cross_linkable: false,
            migration_target: None,
        }
    }

    /// Build a sealed archive at `tmp/<name>.mem` from
    /// `(relative_path, bytes)` pairs and return the path.
    pub(crate) fn build_archive(tmp: &Path, name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = tmp.join(format!("{name}.mem"));
        let file = std::fs::File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for (rel, bytes) in entries {
            writer.start_file(*rel, opts).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    /// Write a schema manifest + minimal type bodies under
    /// `<root>/<name>/`. Each type gets a body with a single
    /// `body` section and `_default` hierarchy/propagation — enough
    /// to load and parse markdown that uses that type. Used by tests
    /// that need a custom schema with shape or vocabulary constraints.
    pub(crate) fn write_schema_files_with_default_type(
        root: &Path,
        name: &str,
        manifest: &str,
        types: &[&str],
    ) {
        const TYPE_BODY: &str = r#"description: t
when_to_use: Here
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
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("types")).unwrap();
        std::fs::write(dir.join("schema.yaml"), manifest).unwrap();
        for type_name in types {
            let body = format!("name: {type_name}\n{TYPE_BODY}");
            std::fs::write(
                dir.join("types").join(format!("{type_name}.yaml")),
                body,
            )
            .unwrap();
        }
    }

    pub(crate) fn empty_create_args(vault: &str, title: &str) -> CreateEntityArgs {
        // The
        // create path refuses on missing required sections. The
        // default `spec` type requires `identity` + `purpose`. Seed
        // both with a single space so the test fixture remains a
        // valid creation request — every test that uses this helper
        // as a fixture builder continues to work, and tests that
        // specifically exercise the refusal supply an explicit
        // empty-sections payload (see the dedicated refusal tests).
        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "fixture identity body".to_string());
        sections.insert("purpose".to_string(), "fixture purpose body".to_string());
        CreateEntityArgs {
            vault: vault.to_string(),
            title: title.to_string(),
            entity_type: "spec".to_string(),
            sections,
            metadata: IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        }
    }

    pub(crate) fn cli_actor() -> (Actor, ClientId) {
        (
            Actor::Cli,
            ClientId {
                name: "claude-code".to_string(),
                version: "2.1.0".to_string(),
            },
        )
    }

    pub(crate) fn engine_with_seed(
        tmp: &TempDir,
        title: &str,
    ) -> (Engine, CreateEntityOutcome) {
        let vault_dir = tmp.path().to_path_buf();
        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        let outcome = engine
            .create_entity(empty_create_args("specs", title), actor, Some(&client), None)
            .unwrap();
        (engine, outcome)
    }
    pub(crate) fn build_demo_engine(tmp: &TempDir) -> Engine {
        let vault_dir = tmp.path().to_path_buf();
        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        let source = engine
            .create_entity(
                empty_create_args("specs", "Source One"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let target = engine
            .create_entity(
                empty_create_args("specs", "Target Two"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        engine
            .create_entity(
                empty_create_args("specs", "Lonely Three"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        engine
    }

}
