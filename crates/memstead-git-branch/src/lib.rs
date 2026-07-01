//! Git-branch storage backend for Memstead — sibling to the folder and
//! archive backends in `memstead-base`. Implements
//! [`memstead_base::backend::VaultBackend`] over a multi-root
//! `vault-repo-git` repository: each vault lives on its own branch
//! (`refs/heads/<vault>` for flat layouts, `refs/heads/<path>/<vault>`
//! for hierarchical), entities are blobs in the per-vault tree,
//! provenance is encoded in commit objects (subject = mutation kind +
//! entity, trailer block = actor / client / tool, body paragraph =
//! agent note). Workspace-level state — per-vault config, schema
//! bodies — lives on the `__MEMSTEAD` umbrella ref.
//!
//! ## Crate role
//!
//! This crate is one of three storage backends. The workspace
//! concept itself, the unified `Engine`, the entity loader, the
//! schema registry, and the runtime validator all live in
//! `memstead-base`. This crate's exports are:
//!
//! - [`storage::git_tree::GitTreeVaultWriter`] — the
//!   [`memstead_base::backend::VaultBackend`] implementation that buffers
//!   mutations and applies them via `gix::object::tree::Editor`
//!   against the per-vault branch.
//! - [`storage::instantiate_pro_backend`] — the [`memstead_base::BackendFactory`]
//!   pro consumers install on the unified `Engine` at boot
//!   ([`workspace_store::engine_from_workspace_root`]) so the
//!   factory can materialise git-branch backends in addition to
//!   folder + archive.
//! - [`workspace_store::engine_from_workspace_root`] — pro-flavour
//!   workspace boot path that loads the two-layer file adapter
//!   (`.memstead/workspace.toml` + `.memstead/state/mounts.json`).
//! - Pro-side helpers consumed by the unified surface:
//!   `vault_repo_config::commit_config_at_gitdir` (per-vault config
//!   writes into `__MEMSTEAD` for git-branch mounts),
//!   `ops::agent_notes::agent_notes_since`,
//!   `ops::changes::changes_since`, and `ops::export::export_vault`
//!   (consumed through the trait or directly by `memstead_base::Engine`).
//!
//! Built only with the `vault-repo` Cargo feature on `memstead-mcp` /
//! `memstead-cli` (or via the workspace-level `--features vault-repo`).
//! Basis builds skip this crate entirely; the basis MCP / CLI
//! binaries link only `memstead-base`.

pub use memstead_base::chunking;
pub use memstead_base::graph;
pub use memstead_base::render;
pub use memstead_base::search_index;
pub use memstead_base::store;
pub use memstead_base::validator;
pub use memstead_base::vault;
pub use memstead_base::workspace_root;

pub mod discover;
pub mod entity;
pub mod ops;
pub mod storage;
pub mod storage_memstead;
pub mod vault_cache;
pub mod vault_repo_config;
pub mod vault_repo_schemas;
pub mod vcs;
pub mod workspace_store;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

use std::path::PathBuf;

use memstead_schema::SchemaRef;

pub use entity::{Entity, EntityId, MetadataValue, ParseResult, Relationship};
pub use graph::{ClusterInfo, LouvainOutput};
pub use ops::agent_notes::{AgentNotesReport, CommitNote};
pub use ops::changes::{
    ChangeEnvelope, ChangesReport, EMPTY_TREE_SHA, RENAME_SIMILARITY_DEFAULT,
    RENAME_SIMILARITY_MAX, RENAME_SIMILARITY_MIN,
};
pub use ops::export::VaultExportError;
pub use ops::{
    BatchResult, ContextResult, CreateArgs, CreateResult, DeleteResult, ExportResult, Facets,
    HealthReport, HealthSummary, ListResult, ModifiedMetadata, ModifiedSections, Query, RelateArg,
    RelateResult, ReloadReport, ReloadResult, RenameResult, SearchHit, SearchResult, SearchScope,
    UpdateArgs,
    UpdateResult, VaultExportResult, WarningHint,
};
pub use store::{Edge, EdgeSource, InEdge, Store};
pub use vault::{VaultOrigin, VaultRouterSnapshot};
pub use vcs::{Actor, ClientId, CommitContext, Vcs, VcsError};

/// Description of a vault discovered on a git-branch backend. Produced
/// by [`vault_repo_config::vault_init_from_branch`] for the macOS
/// `discover_vaults` UniFFI helper, which seeds the UI's vault list
/// before the engine itself is constructed.
#[derive(Debug, Clone)]
pub struct VaultInit {
    pub name: String,
    pub dir: Option<PathBuf>,
    /// Schema this vault is pinned to. Mirrors `.memstead/config.json`
    /// `schema: "name@version"`.
    pub schema_ref: SchemaRef,
}

pub use memstead_base::engine_fallback_type;
