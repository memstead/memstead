//! Engine lifecycle — settings/workspace-root setters, runtime
//! mem add/remove, reload, and export.
//!
//! `register_writable_mem` / `unregister_writable_mem` are the
//! engine-level primitives the `memstead_mem_create` / `memstead_mem_delete`
//! handlers build on. `reload_one_mem*` re-reads a mount's backend
//! and refreshes the in-memory store; `reload_each_writable_mem*`
//! sweeps every writable mount. `export_markdown` regenerates entity
//! markdown for folder mounts; `export_mem` produces a portable
//! `.mem` archive via the backend-aware dispatch in
//! [`crate::ops::export`].

use std::cell::OnceCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::backend::{BackendError, MemBackend};
use crate::engine_fallback_type;
use crate::entity::EntityId;
use crate::entity::generator::generate_markdown;
use crate::entity::loader::parse_entries;
use crate::entity::store_builder::push_entities_into_store;
use crate::mem::MemOrigin;
use crate::ops::WarningHint;
use crate::workspace::{Mount, MountStorage, WorkspaceSettings};

use super::boot::collect_source_entries;
use super::{BackendFactory, Engine, EngineError, GitBranchOps, MountedBackend};

/// What [`Engine::stage_sealed_schema`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaStaging {
    /// The pin already resolves in this workspace — nothing written.
    /// Re-installing a mem, and installing a second mem that pins the
    /// same schema, both land here.
    AlreadyResolvable,
    /// The package was written into the workspace's local schema
    /// storage and is resolvable in this process from now on.
    Staged,
    /// The archive carries no embedded schema tree. Nothing to stage;
    /// the pin must resolve on its own or the mount refuses.
    NoEmbeddedSchema,
    /// The workspace has no sealed-package storage (a folder-shaped
    /// workspace: its `.memstead/schemas/` is the authoring tier and
    /// never holds a third party's sealed bytes). The package was read
    /// and checked against the pin, nothing was written, and the mount
    /// resolves its vocabulary from the archive itself — on this shape
    /// the archive IS the schema storage.
    CarriedByArchive,
}

impl Engine {
    /// Replace the workspace-level settings. Called by
    /// [`Self::from_workspace_root`] (and the full counterpart) after
    /// reading `.memstead/workspace.toml`. Tests / direct callers leave
    /// the default empty value in place. Cheap clone — settings
    /// carry only data shapes (raw rule lists, link policy map),
    /// no compiled matchers. Invalidates the lazy
    /// `create_rule_set_memo` so the next synthesis call rebuilds
    /// from the new policy.
    pub fn set_settings(&mut self, settings: WorkspaceSettings) {
        self.settings = settings;
        self.create_rule_set_memo = OnceCell::new();
    }

    /// Replace the backend factory. Full consumers call this once at boot
    /// (`engine_from_workspace_root`) to install
    /// `memstead_git_branch::storage::instantiate_full_backend` so the engine
    /// can materialise git-branch backends on top of folder + archive.
    /// Consumers without the git-branch crate leave the default in place.
    pub fn set_backend_factory(&mut self, factory: BackendFactory) {
        self.backend_factory = factory;
    }

    /// Insert into the engine's schema map, bumping the schemas epoch
    /// (the second half of the derived-memo key — flywheel W8/01):
    /// derived structures depend on schemas, so any change here must
    /// be visible to the memo-invalidation hooks even when the store
    /// generation did not move.
    pub(crate) fn schemas_insert(
        &mut self,
        mem: String,
        schema: std::sync::Arc<memstead_schema::Schema>,
    ) {
        self.schemas_epoch += 1;
        self.schemas.insert(mem, schema);
    }

    /// Remove from the engine's schema map, bumping the schemas epoch
    /// — see [`Self::schemas_insert`].
    pub(crate) fn schemas_remove(&mut self, mem: &str) {
        self.schemas_epoch += 1;
        self.schemas.remove(mem);
    }

    /// Install the unmounted-mem storage discovery hook (flywheel
    /// W7/02). Full boot sets it; without one, writes referencing
    /// unmounted mems keep the forward-reference mechanic unchanged.
    pub fn set_unmounted_storage_prober(&mut self, prober: super::UnmountedStorageProber) {
        self.unmounted_storage_prober = Some(prober);
    }

    /// Replace the mutation-timestamp clock — the source every
    /// engine-stamped metadata field (`init_timestamp` /
    /// `auto_timestamp` schema flags: `created_date`, `last_modified`)
    /// reads. A testing affordance for suites that assert over
    /// canonical entity bytes (e.g. cross-surface hash parity):
    /// pin both engines to the same constant and byte-level
    /// nondeterminism from wall-clock seconds disappears. Production
    /// code never calls this — the default installed at construction
    /// is the system clock, and the stamped format is unchanged
    /// either way.
    pub fn set_mutation_clock(&mut self, clock: crate::engine::MutationClock) {
        self.mutation_clock = clock;
    }

    /// Set the caller-declared role for subsequent mutations
    ///. The surface calls this before every
    /// mutation with the per-call parameter resolved against its
    /// session default (per-call wins); `Role::Unspecified` records
    /// as absence.
    pub fn set_role(&mut self, role: crate::vcs::Role) {
        self.current_role = role;
    }

    /// The currently declared role — what the next mutation records.
    pub fn current_role(&self) -> crate::vcs::Role {
        self.current_role
    }

    /// Set the caller-declared identity for subsequent mutations and
    /// checks. The surface calls this before
    /// every operation with the per-call parameter resolved against
    /// its session default (per-call wins); `None` records as
    /// absence. Callers pass an already-normalised value
    /// ([`crate::vcs::normalise_identity`]).
    pub fn set_identity(&mut self, identity: Option<String>) {
        self.current_identity = identity;
    }

    /// Set the transport's actor for the commits this session causes
    /// without a per-call actor (config writes, sync-state stamps, the
    /// anchor writers). The CLI sets `Cli` at boot; the MCP server
    /// sets `Agent`.
    pub fn set_actor(&mut self, actor: crate::vcs::Actor) {
        self.current_actor = actor;
    }

    /// The transport's actor (see [`Self::set_actor`]).
    pub fn current_actor(&self) -> crate::vcs::Actor {
        self.current_actor
    }

    /// Set the transport's client id, when it has one (the MCP client
    /// after `initialize`; the CLI's own id).
    pub fn set_client(&mut self, client: Option<crate::vcs::ClientId>) {
        self.current_client = client;
    }

    /// The transport's client id (see [`Self::set_client`]).
    pub fn current_client(&self) -> Option<&crate::vcs::ClientId> {
        self.current_client.as_ref()
    }

    /// The commit context of one mutation: its tool, actor, client and
    /// note, with the session's declared role and identity. Every
    /// commit the engine writes is built through here (or through
    /// [`Self::session_commit_context`]), so no path can drop a trailer.
    pub fn commit_context<'a>(
        &self,
        tool: Option<&'a str>,
        actor: crate::vcs::Actor,
        client: Option<crate::vcs::ClientId>,
        note: Option<String>,
    ) -> crate::vcs::CommitContext<'a> {
        crate::vcs::CommitContext::new(
            tool,
            actor,
            client,
            note,
            self.current_role,
            self.current_identity.clone(),
        )
    }

    /// The commit context of a mutation the session causes as itself:
    /// the transport's actor and client (see [`Self::set_actor`] and
    /// [`Self::set_client`]) with the session's role and identity.
    pub fn session_commit_context<'a>(
        &self,
        tool: Option<&'a str>,
        note: Option<String>,
    ) -> crate::vcs::CommitContext<'a> {
        self.commit_context(tool, self.current_actor, self.current_client.clone(), note)
    }

    /// The currently declared identity — what the next mutation or
    /// check records.
    pub fn current_identity(&self) -> Option<&str> {
        self.current_identity.as_deref()
    }

    /// Current mutation timestamp as the second-granularity ISO form
    /// the stamping paths write. Reads [`Self::mutation_clock`] — the
    /// system clock unless a test pinned it.
    pub(crate) fn now_iso(&self) -> String {
        crate::engine::mutation::iso_from_system_time((self.mutation_clock)())
    }

    /// Install the git-branch ops bundle. Full boot
    /// (`memstead_git_branch::engine_from_workspace_root`) calls this once
    /// at construction. Consumers without the git-branch crate leave it unset
    /// and the git-branch dispatch branches collapse to typed errors / empty
    /// reports; such an engine serves no git-branch mounts.
    pub fn set_git_branch_ops(&mut self, ops: GitBranchOps) {
        self.git_branch_ops = Some(ops);
    }
}

// -------------------------------------------------------------------------
// Cut by phase. Each child opens its own `impl Engine` block, so every
// method keeps its name, signature and visibility and no consumer moves.
// -------------------------------------------------------------------------

mod export;
mod mem_settings;
mod mounts;
mod reload;
mod schema;
mod schema_pin;

pub use schema_pin::*;

#[cfg(test)]
mod tests;
