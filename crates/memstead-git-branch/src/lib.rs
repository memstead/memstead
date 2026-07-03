//! Git-branch storage backend for Memstead — sibling to the folder and
//! archive backends in `memstead-base`. Implements
//! [`memstead_base::backend::MemBackend`] over a multi-root
//! `mem-repo-git` repository: each mem lives on its own branch
//! (`refs/heads/<mem>` for flat layouts, `refs/heads/<path>/<mem>`
//! for hierarchical), entities are blobs in the per-mem tree,
//! provenance is encoded in commit objects (subject = mutation kind +
//! entity, trailer block = actor / client / tool, body paragraph =
//! agent note). Workspace-level state — per-mem config, schema
//! bodies — lives on the `__MEMSTEAD` umbrella ref.
//!
//! ## Crate role
//!
//! This crate is one of three storage backends. The workspace
//! concept itself, the unified `Engine`, the entity loader, the
//! schema registry, and the runtime validator all live in
//! `memstead-base`. This crate's exports are:
//!
//! - [`storage::git_tree::GitTreeMemWriter`] — the
//!   [`memstead_base::backend::MemBackend`] implementation that buffers
//!   mutations and applies them via `gix::object::tree::Editor`
//!   against the per-mem branch.
//! - [`storage::instantiate_full_backend`] — the [`memstead_base::BackendFactory`]
//!   full consumers install on the unified `Engine` at boot
//!   ([`workspace_store::engine_from_workspace_root`]) so the
//!   factory can materialise git-branch backends in addition to
//!   folder + archive.
//! - [`workspace_store::engine_from_workspace_root`] — full-flavour
//!   workspace boot path that loads the two-layer file adapter
//!   (`.memstead/workspace.toml` + `.memstead/state/mounts.json`).
//! - Full-side helpers consumed by the unified surface:
//!   `mem_repo_config::commit_config_at_gitdir` (per-mem config
//!   writes into `__MEMSTEAD` for git-branch mounts),
//!   `ops::agent_notes::agent_notes_since`,
//!   `ops::changes::changes_since`, and `ops::export::export_mem`
//!   (consumed through the trait or directly by `memstead_base::Engine`).
//!
//! Built only with the `mem-repo` Cargo feature on `memstead-mcp` /
//! `memstead-cli` (or via the workspace-level `--features mem-repo`).
//! Lean builds skip this crate entirely; the lean MCP / CLI
//! binaries link only `memstead-base`.

pub use memstead_base::chunking;
pub use memstead_base::graph;
pub use memstead_base::render;
pub use memstead_base::search_index;
pub use memstead_base::store;
pub use memstead_base::validator;
pub use memstead_base::mem;
pub use memstead_base::workspace_root;

pub mod discover;
pub mod entity;
pub mod ops;
pub mod storage;
pub mod storage_memstead;
pub mod mem_cache;
pub mod mem_repo_config;
pub mod mem_repo_schemas;
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
pub use ops::export::MemExportError;
pub use ops::{
    BatchResult, ContextResult, CreateArgs, CreateResult, DeleteResult, ExportResult, Facets,
    HealthReport, HealthSummary, ListResult, ModifiedMetadata, ModifiedSections, Query, RelateArg,
    RelateResult, ReloadReport, ReloadResult, RenameResult, SearchHit, SearchResult, SearchScope,
    UpdateArgs,
    UpdateResult, MemExportResult, WarningHint,
};
pub use store::{Edge, EdgeSource, InEdge, Store};
pub use mem::{MemOrigin, MemRouterSnapshot};
pub use vcs::{Actor, ClientId, CommitContext, Vcs, VcsError};

/// Description of a mem discovered on a git-branch backend. Produced
/// by [`mem_repo_config::mem_init_from_branch`] for the macOS
/// `discover_mems` UniFFI helper, which seeds the UI's mem list
/// before the engine itself is constructed.
#[derive(Debug, Clone)]
pub struct MemInit {
    pub name: String,
    pub dir: Option<PathBuf>,
    /// Schema this mem is pinned to. Mirrors `.memstead/config.json`
    /// `schema: "name@version"`.
    pub schema_ref: SchemaRef,
}

pub use memstead_base::engine_fallback_type;
