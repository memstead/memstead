//! `Engine::retype_entity` — change an entity's type in place.
//!
//! The identity triple (`mem` / `id` / `type`) is reserved on `update`
//! by decision: `type` set refuses `READ_ONLY_FIELD`. A type change is
//! its own verb because it is its own validation regime — the existing
//! sections and metadata have to satisfy the TARGET type, and every edge
//! touching the entity, incoming and outgoing, same-mem and cross-mem,
//! has to fit the target type's relationship pins — and its own
//! provenance kind. The id, the file path, and every incoming edge stay:
//! nothing is deleted and re-created, so history and provenance survive.
//!
//! Report-all refusal: every unknown section, missing required section,
//! unknown or invalid metadata value, unsatisfied block-tier constraint,
//! and shape-violating edge is collected and returned in ONE envelope
//! (`RetypeRefused`), with the target's declared sections, its catch-all,
//! and a proposed `section_map` in the recovery payload, so a second
//! attempt can be the right one. Nothing is written before the set is
//! empty.
//!
//! The edge re-check on both directions is mandatory, not thoroughness:
//! the loader drops a shape-invalid edge at the next boot with only a
//! `PARSED_RELATION_INVALID` warning, so a retype that skipped it would
//! amputate the graph on restart. Referrers that live in a lazy (deferred,
//! unloaded) mem are not in the store; they are probed through storage —
//! the same backend the relate path probes deferred targets through — and
//! when a deferred mem cannot be enumerated the retype refuses naming it,

use std::collections::BTreeMap;
use std::path::Path;

use indexmap::IndexMap;

use crate::engine_fallback_type;
use crate::entity::EntityId;
use crate::entity::parser::parse_markdown;
use crate::entity::store_builder::push_entities_into_store;
use crate::ops::WarningHint;
use crate::provenance::ProvenanceKind;
use crate::runtime_validator::{
    CatchAllContext, CrossMemRelCheck, READ_ONLY_METADATA_KEYS, ValidationError,
    missing_required_fields, missing_required_sections, parse_metadata_value,
    validate_cross_mem_edge, validate_rel_shape, validate_section_content, validate_section_keys,
};
use crate::vcs::{Actor, ClientId};

use crate::engine::mutation::unknown_type_error;
use crate::engine::outcomes::{RetypeEdge, RetypeEdgeDirection, RetypeProblem};
use crate::engine::{Engine, EngineError, RetypeEntityArgs, RetypeEntityOutcome};

mod commit;
mod compose;
mod outcome;
mod resolve;
mod stage;
#[cfg(test)]
mod tests;
mod validate;

/// The entity a retype targets, read under the lock: everything later
/// phases read from the workspace before they decide anything.
struct ResolvedRetype {
    args: RetypeEntityArgs,
    mem: String,
    mount_idx: usize,
    entity: crate::entity::Entity,
    schema: std::sync::Arc<memstead_schema::Schema>,
    target_def: std::sync::Arc<memstead_schema::TypeDefinition>,
}

/// The entity as the target type would hold it, every problem cleared.
struct ValidatedRetype {
    resolved: ResolvedRetype,
    next: crate::entity::Entity,
    sections_renamed: Vec<(String, String)>,
    edges_rechecked: usize,
    warnings: Vec<WarningHint>,
}

/// The rendered file a retype writes.
struct ComposedRetype {
    validated: ValidatedRetype,
    markdown: String,
}

/// What a retype's commit returns to the outcome phase.
struct CommittedRetype {
    composed: ComposedRetype,
    write_id: String,
    landed_warnings: Vec<WarningHint>,
    stamp_warnings: Vec<WarningHint>,
}

enum RetypePrepareOutcome {
    /// A dry run: validated and rendered, nothing written.
    Done(Box<RetypeEntityOutcome>),
    Prepared(Box<ComposedRetype>),
}

impl Engine {
    /// Change `args.id`'s type to `args.target_type` in place. See the
    /// module docs for the contract; the outcome names the old and new
    /// type, the renamed sections, the edges re-checked, and states that
    /// check records and derivation baselines on the entity are stale
    /// (its content hash moved).
    pub fn retype_entity(
        &mut self,
        args: RetypeEntityArgs,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<RetypeEntityOutcome, EngineError> {
        let mut args = args;
        // A retype rechecks the edges that point at the entity, and they
        // can come from any mem: like a delete, it locks every writable
        // mount, reloads every mem and loads the deferred ones before it
        // walks an edge, so the CAS compare and the edge walk run
        // against current truth everywhere.
        let (_write_lock, reloaded) = self.lock_workspace_for_write_if(!args.dry_run)?;
        // A bare slug is resolved under the lock, against the reloaded
        // view: which mem carries a slug is a fact about every mem.
        let (resolved, short_hint) = self.resolve_entity_id(&args.id)?;
        args.id = resolved;
        let mut drift_warnings: Vec<WarningHint> = short_hint.into_iter().collect();
        drift_warnings.extend(reloaded);
        match self.prepare_retype(args)? {
            RetypePrepareOutcome::Done(outcome) => Ok(*outcome),
            RetypePrepareOutcome::Prepared(composed) => {
                self.commit_prepared_retype(*composed, drift_warnings, actor, client, note)
            }
        }
    }

    /// Everything a retype decides before it writes: the entity and the
    /// target type, the entity under the target type with every problem
    /// cleared, and the rendered file; a dry run ends here.
    fn prepare_retype(&self, args: RetypeEntityArgs) -> Result<RetypePrepareOutcome, EngineError> {
        let resolved = self.resolve_retype(args)?;
        let validated = self.validate_retype(resolved)?;
        let composed = self.compose_retype(validated)?;
        if composed.validated.resolved.args.dry_run {
            return Ok(RetypePrepareOutcome::Done(Box::new(
                self.retype_preview(composed)?,
            )));
        }
        Ok(RetypePrepareOutcome::Prepared(Box::new(composed)))
    }

    /// Write, commit and report a prepared retype.
    fn commit_prepared_retype(
        &mut self,
        composed: ComposedRetype,
        drift_warnings: Vec<WarningHint>,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<RetypeEntityOutcome, EngineError> {
        self.stage_retype(&composed)?;
        let committed = self.commit_retype(composed, actor, client, note)?;
        Ok(self.retype_outcome(committed, drift_warnings, note))
    }
}

/// The note every retype outcome carries: the entity's check records and
/// derivation baselines no longer match its content hash.
fn staleness_note(id: &EntityId, from: &str, to: &str) -> String {
    format!(
        "check records and derivation baselines on {id} are stale: its content hash \
         moved from {from} to {to} with the type change; re-check and re-baseline \
         deliberately"
    )
}
