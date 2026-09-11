//! `Engine::update_entity` and `Engine::batch_update` — rewrite an
//! entity's sections / metadata in place, optimistically-locked.
//!
//! The module is cut by the phases every mutation shares, one file
//! each, in the order an update runs them: [`resolve`] locates the
//! mount and the entity and checks the optimistic lock; [`validate`]
//! gates the payload against the schema; [`compose`] applies the
//! delta, synthesises relations, renders the entity and judges the
//! composed state; [`stage`] puts the write and its sidecars into the
//! backend's pending buffer; [`commit`] lands the commit, the
//! provenance and the store application; [`outcome`] shapes what goes
//! back on the wire. [`batch`] drives the same phases for
//! `batch_update`, preparing every item first and committing once per
//! mem.

mod batch;
mod commit;
mod compose;
mod outcome;
mod resolve;
mod stage;
#[cfg(test)]
mod tests;
mod validate;

use std::sync::Arc;

use crate::engine::outcomes::RelationDeclared;
use crate::entity::EntityId;
use crate::ops::{ModifiedMetadata, ModifiedSections, WarningHint};
use crate::vcs::{Actor, ClientId, CommitContext};

use super::super::{Engine, EngineError, UpdateEntityArgs, UpdateEntityOutcome};

pub(super) use outcome::batch_error_envelope;

/// Result of [`Engine::prepare_update`] — the validation + markdown
/// step split out of the commit so the batch path can prepare every
/// item before committing the whole set atomically.
enum PrepareOutcome {
    /// No commit is needed: the no-op short-circuit (content unchanged)
    /// and the dry-run preview both return a finished outcome here.
    Done(UpdateEntityOutcome),
    /// A real change whose post-mutation markdown is ready to stage +
    /// commit.
    Prepared(PreparedUpdate),
}

/// Everything the commit step needs to stage one prepared update's
/// disk write and build its outcome. Carries no commit SHA — that's
/// produced when the (single or batched) commit lands.
struct PreparedUpdate {
    mount_idx: usize,
    id: EntityId,
    mem: String,
    type_def: Arc<memstead_schema::TypeDefinition>,
    file_path: String,
    markdown: String,
    /// Body wiki-link targets the entity had *before* this mutation —
    /// the GC sweep scopes orphan-stub detection to these.
    prev_body_targets: std::collections::HashSet<EntityId>,
    modified_date: String,
    modified_sections: ModifiedSections,
    modified_metadata: ModifiedMetadata,
    warnings: Vec<WarningHint>,
    relations_declared: Vec<RelationDeclared>,
    /// Validated anchors to merge into this entity's sidecar row — staged
    /// into the same commit as the disk write on the commit step. Empty
    /// when the update carried no `anchors[]`.
    anchors: Vec<crate::anchor::Anchor>,
    /// Validated explicit anchor removals, applied before the `anchors`
    /// merge in the same staged write. Empty when the update carried no
    /// `anchors_unset[]`.
    anchor_unsets: Vec<crate::anchor::AnchorUnset>,
    /// True when this update's *sole* delta is the anchors sidecar —
    /// sections, metadata, and relationships are byte-identical to the
    /// on-disk entity. Such a commit earns the distinct
    /// `memstead: anchor <id>` subject (parsed `tool_verb == "anchor"`)
    /// so an anchor-only refresh — which the `_hash` excludes and which
    /// therefore produces zero entity deltas — is still observable to an
    /// `--include-notes` reader. A content-changing update (even one that
    /// also carries anchors) keeps the `memstead: update <id>` subject:
    /// its content change already bumps `_hash` and surfaces as a delta,
    /// so the anchor activity riding it is already visible.
    anchor_only: bool,
    /// Whether the anchors merge changes the sidecar: `None` when the
    /// update carried no anchors or unsets, `Some(true)` when a row is
    /// added, replaced or removed, `Some(false)` when every supplied row
    /// restates what is stored (then nothing is staged).
    anchors_changed: Option<bool>,
    /// Whether the entity's content changed in this update: the anchors
    /// merge then re-baselines a restated row (a sync repair's re-pin).
    content_changed: bool,
}

impl Engine {
    /// Update an entity's sections and/or metadata.
    ///
    /// Same six-concern shape as [`Engine::create_entity`]. Optimistic
    /// locking via `args.expected_hash`: when `Some`, must match the
    /// store's current `content_hash` or returns
    /// [`EngineError::HashMismatch`]. The new engine's MCP-facing
    /// callers should always pass the hash; `None` is the
    /// `--force`-style escape hatch.
    ///
    /// Internally a two-step pipeline: [`Self::prepare_update`] runs
    /// all validation and computes the post-mutation markdown without
    /// committing, then [`Self::commit_prepared_update`] stages and
    /// commits the result. The split lets [`Self::batch_update`]
    /// prepare every item up front and commit the whole batch as one
    /// atomic unit.
    pub fn update_entity(
        &mut self,
        args: UpdateEntityArgs,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<UpdateEntityOutcome, EngineError> {
        // A short id resolves (or refuses) before anything reads its
        // mem; the announcement leads the outcome's warnings.
        let mut args = args;
        let (resolved, short_hint) = self.resolve_entity_id(&args.id)?;
        args.id = resolved;
        let mut drift_warnings: Vec<WarningHint> = short_hint.into_iter().collect();
        for r in &mut args.declare_relations {
            let (target, hint) = self.resolve_entity_id(&r.target)?;
            r.target = target;
            drift_warnings.extend(hint);
        }
        // Reload-before-operation: probe the mem ref and reload if a
        // sibling advanced it, so the `expected_hash` compare inside
        // `prepare_update` runs against current truth. A stale hash for
        // the targeted entity then trips a real `HASH_MISMATCH`; an
        // unrelated concurrent write leaves this entity's hash intact
        // and the update proceeds. The drift notice rides the outcome.
        drift_warnings.extend(self.reload_if_stale(Some(args.id.mem())));
        // Declared relations on an ACYCLIC rel-type (or one in an
        // `acyclic_sets` set, whose guard walks the set's UNION
        // subgraph) run the same whole-subgraph cycle guard relate
        // runs — full load first, or a cycle through a deferred mem's
        // edge is invisible (see the relate path's comment for the
        // demonstrated failure). Declared signals on the entity's
        // schema or any relation target's schema need the full load
        // too: the threshold-crossing diff counts edges that can
        // originate in any mem.
        if args.declare_relations.iter().any(|r| {
            self.schemas.get(args.id.mem()).is_some_and(|s| {
                s.relationship_acyclic(&r.rel_type)
                    || s.acyclic_set_containing(&r.rel_type).is_some()
            }) || self
                .schemas
                .get(r.target.mem())
                .is_some_and(|s| s.types.values().any(|td| !td.signals.is_empty()))
        }) || self
            .schemas
            .get(args.id.mem())
            .is_some_and(|s| s.types.values().any(|td| !td.signals.is_empty()))
        {
            self.ensure_mems_loaded(None);
        }
        let mut outcome = match self.prepare_update(args)? {
            PrepareOutcome::Done(outcome) => outcome,
            PrepareOutcome::Prepared(prepared) => {
                self.commit_prepared_update(prepared, actor, client, note)?
            }
        };
        drift_warnings.append(&mut outcome.warnings);
        outcome.warnings = drift_warnings;
        Ok(outcome)
    }

    /// Validate an update and compute its post-mutation markdown
    /// *without* committing — the resolve, validate and compose phases
    /// in that order. Returns [`PrepareOutcome::Done`] for the
    /// no-op / dry-run short-circuits (which never commit) and
    /// [`PrepareOutcome::Prepared`] for a real change whose write the
    /// caller stages + commits. May mutate the store in place via the
    /// alias-synthesis auto-stub upsert; the batch path snapshots the
    /// store before preparing so a refused batch can roll that back.
    fn prepare_update(&mut self, args: UpdateEntityArgs) -> Result<PrepareOutcome, EngineError> {
        let resolved = self.resolve_update(args)?;
        let validated = self.validate_update(resolved)?;
        self.compose_update(validated)
    }

    /// CommitContext-bundling wrapper around [`Self::update_entity`].
    /// See [`Self::create_entity_with_ctx`] for the rationale.
    pub fn update_entity_with_ctx(
        &mut self,
        args: UpdateEntityArgs,
        ctx: &CommitContext<'_>,
    ) -> Result<UpdateEntityOutcome, EngineError> {
        self.update_entity(args, ctx.actor, ctx.client.as_ref(), ctx.note.as_deref())
    }
}
