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
/// The sections an update leaves behind where it writes them: the
/// stored text with the call's text worked in the way the compose
/// phase does it (a whole section replaces, an append joins with a
/// newline, patches apply in order). A wiki-link in them is a
/// reference the update may add, and it can be one that neither the
/// stored text nor the call's text holds alone. A patch that does not
/// apply is skipped here; the compose phase refuses it.
pub(in crate::engine::mutation) fn update_result_texts(
    engine: &Engine,
    args: &UpdateEntityArgs,
) -> Vec<String> {
    let mut sections = engine
        .store
        .get(&args.id)
        .map(|e| e.sections.clone())
        .unwrap_or_default();
    for (key, body) in &args.sections {
        sections.insert(key.clone(), body.clone());
    }
    for (key, value) in &args.append_sections {
        let existing = sections.get(key).cloned().unwrap_or_default();
        let joined = if existing.trim().is_empty() {
            value.clone()
        } else {
            format!("{existing}\n{value}")
        };
        sections.insert(key.clone(), joined);
    }
    for (key, patches) in &args.patch_sections {
        for patch in patches {
            let Some(existing) = sections.get(key).cloned() else {
                continue;
            };
            let patched = if patch.all {
                existing.replace(&patch.old, &patch.new)
            } else {
                existing.replacen(&patch.old, &patch.new, 1)
            };
            sections.insert(key.clone(), patched);
        }
    }
    args.sections
        .keys()
        .chain(args.append_sections.keys())
        .chain(args.patch_sections.keys())
        .filter_map(|key| sections.get(key).cloned())
        .collect()
}

#[cfg(test)]
mod tests;
mod validate;

use std::sync::Arc;

use crate::engine::outcomes::RelationDeclared;
use crate::entity::EntityId;
use crate::ops::{ModifiedMetadata, ModifiedSections, WarningHint};
use crate::vcs::{Actor, ClientId, CommitContext};

use super::super::{Engine, EngineError, UpdateEntityArgs, UpdateEntityOutcome};

/// Result of [`Engine::prepare_update`] — the validation + markdown
/// step split out of the commit so the batch path can prepare every
/// item before committing the whole set atomically.
pub(in crate::engine::mutation) enum PrepareOutcome {
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
pub(in crate::engine::mutation) struct PreparedUpdate {
    pub(in crate::engine::mutation) mount_idx: usize,
    pub(in crate::engine::mutation) id: EntityId,
    pub(in crate::engine::mutation) mem: String,
    pub(in crate::engine::mutation) type_def: Arc<memstead_schema::TypeDefinition>,
    pub(in crate::engine::mutation) file_path: String,
    pub(in crate::engine::mutation) markdown: String,
    /// Body wiki-link targets the entity had *before* this mutation —
    /// the GC sweep scopes orphan-stub detection to these.
    pub(in crate::engine::mutation) prev_body_targets: std::collections::HashSet<EntityId>,
    pub(in crate::engine::mutation) modified_date: String,
    pub(in crate::engine::mutation) modified_sections: ModifiedSections,
    pub(in crate::engine::mutation) modified_metadata: ModifiedMetadata,
    pub(in crate::engine::mutation) warnings: Vec<WarningHint>,
    pub(in crate::engine::mutation) relations_declared: Vec<RelationDeclared>,
    /// Validated anchors to merge into this entity's sidecar row — staged
    /// into the same commit as the disk write on the commit step. Empty
    /// when the update carried no `anchors[]`.
    pub(in crate::engine::mutation) anchors: Vec<crate::anchor::Anchor>,
    /// Validated explicit anchor removals, applied before the `anchors`
    /// merge in the same staged write. Empty when the update carried no
    /// `anchors_unset[]`.
    pub(in crate::engine::mutation) anchor_unsets: Vec<crate::anchor::AnchorUnset>,
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
    pub(in crate::engine::mutation) anchor_only: bool,
    /// Whether the anchors merge changes the sidecar: `None` when the
    /// update carried no anchors or unsets, `Some(true)` when a row is
    /// added, replaced or removed, `Some(false)` when every supplied row
    /// restates what is stored (then nothing is staged).
    pub(in crate::engine::mutation) anchors_changed: Option<bool>,
    /// Whether the entity's content changed in this update: the anchors
    /// merge then re-baselines a restated row (a sync repair's re-pin).
    pub(in crate::engine::mutation) content_changed: bool,
    /// The self-check gates this write passed, when it passed any: the
    /// commit step carries their records to the post-write hash before
    /// staging, so the gate that admitted the write still holds after it.
    /// Boxed: the prepared write already rides inside enum variants sized
    /// against their peers.
    pub(in crate::engine::mutation) licensed_self_checks: Option<Box<LicensedSelfChecks>>,
}

/// The `transition_requires_self_check` gates a prepared write passed on
/// a fresh independent record ([`Engine::licensed_self_check_kinds`]),
/// with the hash those records are keyed to.
pub(in crate::engine::mutation) struct LicensedSelfChecks {
    /// The entity's `content_hash` before the write.
    pub(in crate::engine::mutation) hash_before: String,
    /// The passed gates' wire kinds, deduplicated.
    pub(in crate::engine::mutation) kinds: Vec<String>,
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
        // The lock first, then the ids: a bare slug, on the entity or on
        // a relation target, is resolved under the lock against the
        // reloaded view, and a call that carries one takes the whole
        // workspace for it. So does an update that declares a relation
        // on an acyclic relationship, whose cycle guard walks edges in
        // every mem. Otherwise the update locks its own mem; a stale
        // hash for the targeted entity then trips a real
        // `HASH_MISMATCH`, an unrelated concurrent write leaves this
        // entity's hash intact. The announcements of resolved ids lead
        // the outcome's warnings.
        let mut args = args;
        let named_mem = args.id.mem().to_string();
        let short = named_mem.is_empty()
            || args
                .declare_relations
                .iter()
                .any(|r| r.target.mem().is_empty());
        let (_write_lock, reloaded) = {
            let declared = &args.declare_relations;
            let update = &args;
            // Its own mem, and the mems its result refers to.
            self.lock_for_verb_referring(
                !args.dry_run,
                std::slice::from_ref(&named_mem),
                |engine| {
                    let texts = update_result_texts(engine, update);
                    crate::engine::write_lock::referred_mems(
                        &named_mem,
                        declared.iter().map(|r| &r.target),
                        texts
                            .iter()
                            .map(String::as_str)
                            .chain(declared.iter().filter_map(|r| r.description.as_deref())),
                    )
                },
                |engine| {
                    short
                        || declared
                            .iter()
                            .any(|r| engine.rel_type_is_acyclic(&named_mem, &r.rel_type))
                },
            )?
        };
        let (resolved, short_hint) = self.resolve_entity_id(&args.id)?;
        args.id = resolved;
        let mut drift_warnings: Vec<WarningHint> = short_hint.into_iter().collect();
        for r in &mut args.declare_relations {
            let (target, hint) = self.resolve_entity_id(&r.target)?;
            r.target = target;
            drift_warnings.extend(hint);
        }
        drift_warnings.extend(reloaded);
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
    pub(in crate::engine::mutation) fn prepare_update(
        &mut self,
        args: UpdateEntityArgs,
    ) -> Result<PrepareOutcome, EngineError> {
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
