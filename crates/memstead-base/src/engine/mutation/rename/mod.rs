//! `Engine::rename_entity` — change an entity's slug, move its file
//! on the backend, and rewrite the in-memory store.
//!
//! The module is cut by the phases every mutation shares, one file
//! each, in the order a rename runs them: [`validate`] gates the call
//! and derives the new identity; [`compose`] renders the renamed
//! entity and, once [`resolve`] has found who refers to it and where
//! the engine may write, every referrer's rewrite; [`stage`] puts one
//! mem's part of the rename into its backend's pending buffer;
//! [`commit`] lands the commits, the provenance and the store
//! application, and shapes the outcome.

mod commit;
mod compose;
mod resolve;
mod stage;
#[cfg(test)]
mod tests;
mod validate;

use std::sync::Arc;

use indexmap::IndexMap;
use memstead_schema::TypeDefinition;

use crate::entity::id::SlugDerivation;
use crate::entity::{Entity, EntityId};
use crate::ops::WarningHint;
use crate::vcs::{Actor, ClientId, CommitContext};

use super::super::{Engine, EngineError, RenameEntityArgs, RenameEntityOutcome};

/// What the validate phase ends in.
enum RenameValidation {
    /// The new title derives the slug the entity has: nothing to do,
    /// and the outcome says so.
    Noop(RenameEntityOutcome),
    Proceed(Box<ValidatedRename>),
}

/// A rename that passed its gates: the entity as it stands and the
/// identity it moves to.
struct ValidatedRename {
    id: EntityId,
    new_id: EntityId,
    mem: String,
    mount_idx: usize,
    new_title: String,
    derivation: SlugDerivation,
    entity: Entity,
    type_def: Arc<TypeDefinition>,
}

/// The entities that refer to the renamed one, by where a rewrite can
/// land. Each list is sorted by id, so the writes of a rename replay
/// in one order across runs.
struct RenameReferrers {
    same_mem: Vec<EntityId>,
    cross_mem_write: Vec<EntityId>,
    /// In a read-only mem: left as they are; the old id stays behind as
    /// a stub that holds their edges.
    readonly: Vec<EntityId>,
}

/// One referrer's rewrite: its file as it will be written, and its
/// sections before and after. A wiki-link retarget moves marked bytes
/// without being an authorship event, so the origin marks of an
/// adopted body are recomputed over the new bytes in the same commit;
/// unfollowed, ordinary housekeeping would serve a contributor's
/// untouched sentences as the workspace's own.
struct ReferrerRewrite {
    id: EntityId,
    markdown: String,
    file_path: String,
    type_def: Arc<TypeDefinition>,
    before: IndexMap<String, String>,
    after: IndexMap<String, String>,
}

/// One peer mem's part of a rename: its mount and its referrers'
/// rewrites, which land in one commit of that mem. The peer mem's own
/// record is the one that holds its referrers' marks.
struct PeerMemPlan {
    mount_idx: usize,
    mem: String,
    rewrites: Vec<ReferrerRewrite>,
}

/// A rename computed up to, and not including, its first write.
struct PreparedRename {
    id: EntityId,
    new_id: EntityId,
    mem: String,
    mount_idx: usize,
    new_title: String,
    derivation: SlugDerivation,
    type_def: Arc<TypeDefinition>,
    /// The renamed entity as it will be written.
    markdown: String,
    old_file_path: String,
    new_file_path: String,
    /// The renamed entity's sections before and after its links to
    /// itself were retargeted.
    before_sections: IndexMap<String, String>,
    after_sections: IndexMap<String, String>,
    same_mem: Vec<ReferrerRewrite>,
    peers: std::collections::BTreeMap<String, PeerMemPlan>,
    readonly_referrers: Vec<EntityId>,
}

impl Engine {
    /// Positional + CommitContext wrapper around
    /// [`Self::rename_entity`].
    pub fn rename_entity_with_ctx(
        &mut self,
        old_id: &EntityId,
        new_title: &str,
        expected_hash: &str,
        ctx: &CommitContext<'_>,
    ) -> Result<RenameEntityOutcome, EngineError> {
        let args = RenameEntityArgs {
            id: old_id.clone(),
            expected_hash: Some(expected_hash.to_string()),
            new_title: new_title.to_string(),
        };
        self.rename_entity(args, ctx.actor, ctx.client.as_ref(), ctx.note.as_deref())
    }

    /// Rename an entity by changing its title — the slug, id, and
    /// on-disk file path follow.
    ///
    /// **Same-mem referrers and self-references are rewritten
    /// atomically.** The renaming entity is treated as the first
    /// referrer of itself: every entry in its own `relationships`
    /// list whose target equals the old id is updated to point at
    /// the new id, and every `[[<old-slug>]]` token in its own
    /// section bodies is rewritten to the new slug (respecting
    /// fenced-code and inline-code masking). Every other entity in
    /// the same mem that pointed at the old id (via an explicit
    /// relation or an inline body wiki-link) gets the same two-
    /// surface rewrite. All rewrites land in one per-mem commit.
    /// Cross-mem referrers in other Write mounts get the same
    /// rewrite (colon-form body links and relationship rows), each
    /// peer mem in its own commit pinned to the parent the scan saw,
    /// gated by the cross-mem link policy; a ReadOnly-mount referrer
    /// is left as it is and reported as a residual stub.
    ///
    /// The peer mems commit first and the renamed entity's own mem
    /// last. If a commit fails after another landed, the call returns
    /// `RENAME_PARTIAL_FAILURE`; the entity is then still under the id
    /// and the hash the caller named, and the identical call completes
    /// the rename.
    pub fn rename_entity(
        &mut self,
        args: RenameEntityArgs,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<RenameEntityOutcome, EngineError> {
        // A short id resolves (or refuses) before anything reads its mem.
        let mut args = args;

        // A rename learns what it writes by reading the workspace: who
        // refers to the entity, in which mems. So it takes every
        // writable mount's lock, reloads every mem and loads the
        // deferred ones before it looks for a referrer; a referrer in a
        // mem that was never loaded is rewritten like any other, and no
        // peer's rewrite is computed from a view older than its lock.
        let (_write_lock, reloaded) = self.lock_workspace_for_write_if(true)?;
        // A bare slug is resolved under the lock, against the reloaded
        // view: which mem carries a slug is a fact about every mem.
        let (resolved, short_hint) = self.resolve_entity_id(&args.id)?;
        args.id = resolved;
        let id = &args.id;
        let mem = id.mem().to_string();
        let mut drift_warnings: Vec<WarningHint> = short_hint.into_iter().collect();
        drift_warnings.extend(reloaded);
        let mount_idx = self.writable_mount_index(&mem)?;

        let validated = match self.validate_rename(&args, mount_idx)? {
            RenameValidation::Noop(outcome) => return Ok(outcome),
            RenameValidation::Proceed(validated) => *validated,
        };
        let prepared = self.prepare_rename(validated)?;
        self.commit_prepared_rename(prepared, drift_warnings, actor, client, note)
    }

    /// Everything a rename computes before it writes: the renamed
    /// entity, who refers to it, and every referrer's rewrite, in that
    /// order.
    fn prepare_rename(&self, validated: ValidatedRename) -> Result<PreparedRename, EngineError> {
        let (next, markdown) = self.compose_renamed_entity(&validated)?;
        let referrers = self.resolve_rename_referrers(&validated.id, &validated.mem)?;
        let (same_mem, peers) = self.compose_referrer_rewrites(&validated, &referrers)?;
        let ValidatedRename {
            id,
            new_id,
            mem,
            mount_idx,
            new_title,
            derivation,
            entity,
            type_def,
        } = validated;
        Ok(PreparedRename {
            id,
            new_id,
            mem,
            mount_idx,
            new_title,
            derivation,
            type_def,
            markdown,
            old_file_path: entity.file_path,
            new_file_path: next.file_path,
            before_sections: entity.sections,
            after_sections: next.sections,
            same_mem,
            peers,
            readonly_referrers: referrers.readonly,
        })
    }
}
