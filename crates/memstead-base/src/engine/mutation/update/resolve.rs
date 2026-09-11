//! Resolve phase of an update: locate the mount and gate on its
//! capability, find the entity, snapshot the body wiki-links it
//! carries before anything changes, refuse a stub, and check the
//! optimistic lock against the store's current hash. Nothing here
//! reads the payload.

use std::collections::HashSet;

use crate::entity::{Entity, EntityId};
use crate::workspace::MountCapability;

use super::{Engine, EngineError, UpdateEntityArgs};

/// An update after resolution: the mount, the entity as the store
/// holds it (cloned — the compose phase mutates its copy into the
/// post-mutation state), and the body wiki-link targets it carried
/// before the mutation.
pub(super) struct ResolvedUpdate {
    pub(super) args: UpdateEntityArgs,
    pub(super) mount_idx: usize,
    pub(super) mem: String,
    pub(super) entity: Entity,
    /// Body wiki-link targets the entity had *before* this mutation —
    /// the GC sweep scopes orphan-stub detection to these.
    pub(super) prev_body_targets: HashSet<EntityId>,
}

impl Engine {
    /// Resolve the mount and the entity, and check the optimistic lock.
    pub(super) fn resolve_update(
        &self,
        args: UpdateEntityArgs,
    ) -> Result<ResolvedUpdate, EngineError> {
        let id = &args.id;
        let mem = id.mem().to_string();

        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .ok_or_else(|| self.unknown_mem_error(&mem))?;
        if self.mounts[mount_idx].mount.capability != MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem));
        }

        let entity = self
            .store
            .get(id)
            .ok_or_else(|| EngineError::NotFound { id: id.to_string() })?;

        // Snapshot the prev entity's body wiki-link set before any
        // subsequent `&mut self` reborrow burns the `entity` borrow.
        // Fed to the alias-synthesis pass so the GC step can compare
        // prev vs. next body links and drop pointer-rel-type relations
        // whose target was a body link before but isn't any more.
        let prev_body_targets = super::super::collect_body_link_targets(entity);

        // Stub guard — stubs have no body, no metadata, no
        // schema-resolved type to validate against. The recovery is
        // `memstead_create` (stub adoption preserves incoming
        // references). Pre-Item-02 the update path fell through to
        // the `type_def` lookup below and surfaced the cryptic
        // `UnknownType { name: "" }` cascade. Mirrors the
        // `StubCannotRelate` guard on `memstead_relate`.
        if entity.stub {
            return Err(EngineError::StubNotUpdatable { id: id.to_string() });
        }

        // Skip the hash check on dry_run — full's dry_run is the
        // designated stale-hash recovery path. Agents preview a
        // change without holding a fresh hash, get back the current
        // `content_hash` and a `prospective_hash`, then call the
        // real update with `expected_hash = content_hash`.
        if !args.dry_run
            && let Some(expected) = args.expected_hash.as_deref()
            && entity.content_hash != expected
        {
            return Err(EngineError::HashMismatch {
                id: id.to_string(),
                current: entity.content_hash.clone(),
                is_stub: entity.stub,
            });
        }

        let entity = entity.clone();
        Ok(ResolvedUpdate {
            args,
            mount_idx,
            mem,
            entity,
            prev_body_targets,
        })
    }
}
