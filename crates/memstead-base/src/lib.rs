//! Backend-agnostic graph kernel for Memstead.
//!
//! Hosts the parts of the engine that do not assume a git-backed
//! storage: token-budget chunking, workspace-root utilities, the
//! entity types and markdown pipeline (parse / generate / write), the
//! in-memory store, graph queries and community detection, the mem
//! router, the operation request/response types (including the
//! gix-free read paths in `ops::health` / `ops::search`), the search
//! index, validators, and rendering.
//!
//! Consumers that need only graph operations against directory or
//! archive storage depend on this crate alone — its dependency closure
//! does not include `gix`. Code that needs git-backed mems depends on
//! `memstead-git-branch`, which builds on `memstead-base`.

pub mod backend;
pub mod binding;
pub mod binding_migrate;
pub mod chunking;
pub mod domain_authority_wire;
pub mod engine;
pub mod entity;
pub mod filesystem;
pub mod graph;
pub mod ingest;
pub mod mem;
pub mod mem_management;
pub mod ops;
pub mod overview;
pub mod pipeline;
pub mod pipeline_edit;
pub mod pipeline_migrate;
pub mod pipeline_store;
pub mod provenance;
pub mod render;
pub mod runtime_validator;
pub mod schema_source;
#[cfg(not(target_arch = "wasm32"))]
pub mod search_index;
pub mod storage;
pub mod store;
pub mod validator;
pub mod vcs;
pub mod workspace;
pub mod workspace_root;
pub mod workspace_store;

use std::sync::Arc;

use memstead_schema::{TypeDefinition, type_by_name};

pub use backend::{BackendError, MemBackend};
pub use binding_migrate::{
    BindingMigrateError, MigratedBinding, migrate_gen2_bindings, resolve_migrated_binding,
};
pub use engine::{
    BackendFactory, BootError, CreateEntityArgs, CreateEntityOutcome, DeleteEntityArgs,
    DeleteEntityOutcome, DeleteReferrers, Engine, EngineError, FromArchiveBytesError,
    GitBranchBranchResetFn, GitBranchChangesSinceFn, GitBranchDiffFn, GitBranchExportFn,
    GitBranchExportToBytesFn, GitBranchFetchFn, GitBranchOps, GitBranchPullFn, GitBranchPushFn,
    GitBranchReadTreeFn, INLINE_LIST_CAP, ReferrerInfo, RelateAction, RelateEntityArgs,
    RelateEntityOutcome, RenameEntityArgs, RenameEntityOutcome, SchemaSourceDiagnostic,
    UpdateEntityArgs, UpdateEntityOutcome, format_inline_list_overflow,
};
pub use entity::id::{ENTITY_ID_MAX_LEN, SlugError};
pub use entity::{Entity, EntityId, MetadataValue, ParseResult, Relationship};
pub use graph::{ClusterInfo, LouvainOutput};
pub use mem::{MEM_META_DIR, MemOrigin, MemRouterSnapshot};
pub use mem_management::{CreateRuleSet, DeleteRuleSet, MatcherSet, MatcherSetError};
pub use ops::{
    BackendChanges, BatchEntry, BatchError, BatchResult, ChangeEnvelope, ChangesReport,
    ContextResult, CreateArgs, CreateResult, DeleteResult, EMPTY_TREE_SHA, ExportResult, Facets,
    HealthReport, HealthSummary, ListResult, MemExportResult, ModifiedMetadata, ModifiedSections,
    Query, RENAME_SIMILARITY_DEFAULT, RENAME_SIMILARITY_MAX, RENAME_SIMILARITY_MIN, RelateArg,
    RelateResult, ReloadReport, ReloadResult, RenameResult, SearchHit, SearchResult, SearchScope,
    SetMemVersionOutcome, UpdateArgs, UpdateResult, WarningHint,
};
pub use pipeline::{
    Facet, IngestTrigger, Medium, MediumType, PatternEntry, PatternMode, Projection,
};
pub use pipeline_edit::PipelineEditError;
pub use pipeline_migrate::{migrate_legacy_pipeline, read_legacy_pipeline_configs};
pub use pipeline_store::{
    BindingConfigs, MemPipelineRecord, PipelineConfigs, PipelineRecord,
    load_legacy_pipeline_configs, load_pipeline_configs,
};
pub use provenance::{Provenance, ProvenanceKind};
pub use store::{Edge, EdgeSource, InEdge, Store};
pub use workspace::{
    CreateRuleSetting, DeleteRuleSetting, Mount, MountCapability, MountLifecycle, MountStorage,
    SCHEMA_WILDCARD, Workspace, WorkspaceSettings,
};
pub use workspace_store::{
    FileWorkspaceStore, InstantiateError, Layout, StoreError, WORKSPACE_STORE_DIR,
    WorkspaceStoreAdapter, detect_layout, instantiate_lean_backend, is_workspace_root,
    standalone_workspace,
};

/// Engine-wide sentinel type. Used by ops that need a shared reference
/// type at the Engine level (search, health, list) — always resolves to
/// `default@1.0.0::spec`. Per-entity resolution still goes through
/// `type_by_name` + schema lookup.
pub fn engine_fallback_type() -> Arc<TypeDefinition> {
    type_by_name("spec").expect("spec type must exist in the default builtin schema")
}
