//! `Engine::create_entity` — write a new entity into a mount's
//! backend and update the in-memory store.
//!
//! The module is cut by the phases every mutation shares, one file
//! each, in the order a create runs them: [`resolve`] locates the
//! mount, the schema and the type; [`validate`] gates the inputs and
//! derives the identity; [`compose`] synthesises the entity, renders
//! it and judges the composed state; [`stage`] puts the write and its
//! sidecars into the backend's pending buffer; [`commit`] lands the
//! commit, the provenance and the store application; [`outcome`]
//! shapes what goes back on the wire. [`batch`] drives the same phases
//! for `batch_create`, preparing every item first and committing once
//! per mem.

mod batch;
mod commit;
mod compose;
mod outcome;
mod resolve;
mod stage;
#[cfg(test)]
mod tests;
mod validate;

use crate::entity::EntityId;
use crate::ops::WarningHint;
use crate::vcs::{Actor, ClientId, CommitContext};

use super::super::{CreateEntityArgs, CreateEntityOutcome, Engine, EngineError};

/// Everything a validated create needs to hit disk — the create-side
/// twin of `PreparedUpdate`. Produced by `Engine::prepare_create`,
/// consumed by `Engine::commit_prepared_create` (single item) and by
/// `Engine::batch_create` (staged all-first, one commit per mem).
struct PreparedCreate {
    mount_idx: usize,
    id: EntityId,
    title: String,
    mem: String,
    file_path: String,
    markdown: String,
    anchors: Vec<crate::anchor::Anchor>,
    warnings: Vec<WarningHint>,
    type_guidance: std::collections::BTreeMap<String, Vec<String>>,
    relations_declared: Vec<crate::engine::outcomes::RelationDeclared>,
    /// Inline-relation targets — the commit tail materialises
    /// forward-reference stubs for the ones the store still lacks.
    relation_targets: Vec<EntityId>,
    type_def: std::sync::Arc<memstead_schema::TypeDefinition>,
}

/// Outcome of `Engine::prepare_create`: a dry-run completes at prepare
/// time; a real write returns the staged material.
enum CreatePrepareOutcome {
    Done(CreateEntityOutcome),
    Prepared(PreparedCreate),
}

impl Engine {
    /// Create a new entity in `args.mem`. Six concerns wired here
    /// in one shape regardless of which backend serves the mount:
    ///
    /// 1. **Capability gating** — rejects mounts with `ReadOnly`
    ///    capability before reaching the backend.
    /// 2. **Validator pipeline** — `validate_section_keys` +
    ///    `parse_metadata_value` enforce the pinned schema's strictness;
    ///    typed `ValidationError` lifts to `EngineError::Validation`.
    /// 3. **Provenance** — a `Provenance` record routes through
    ///    `backend.append_provenance` (folder writes JSONL, git-branch
    ///    no-ops since the commit subject + trailers carry the same
    ///    fields).
    /// 4. **Write + commit atomicity** — `backend.write_entity` then
    ///    `backend.commit` with the canonical `memstead: create <id>`
    ///    subject so the git-branch backend's `read_provenance` can
    ///    recover the kind.
    /// 5. **Store update** — re-parse the freshly-generated markdown
    ///    so the in-memory `Store` mirrors disk (including
    ///    generator-determined `content_hash`).
    /// 6. **Error envelope** — `BackendError::Sealed` lifts via the
    ///    `Backend` variant so MCP callers see the typed payload
    ///    intact; `HashMismatch` propagates likewise.
    pub fn create_entity(
        &mut self,
        args: CreateEntityArgs,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<CreateEntityOutcome, EngineError> {
        let drift_warnings = self.reload_if_stale(Some(&args.mem));
        // Declared relations on an ACYCLIC rel-type (or one in an
        // `acyclic_sets` set, whose guard walks the set's UNION
        // subgraph) run the same whole-subgraph cycle guard relate
        // runs — full load first, or a cycle through a deferred mem's
        // edge is invisible (see the relate path's comment for the
        // demonstrated failure). Declared signals on the new entity's
        // schema or any relation target's schema need the full load
        // too: the threshold-crossing diff counts edges that can
        // originate in any mem.
        if args.relations.iter().any(|r| {
            self.schemas.get(&args.mem).is_some_and(|s| {
                s.relationship_acyclic(&r.rel_type)
                    || s.acyclic_set_containing(&r.rel_type).is_some()
            }) || self
                .schemas
                .get(r.target.mem())
                .is_some_and(|s| s.types.values().any(|td| !td.signals.is_empty()))
        }) || self
            .schemas
            .get(&args.mem)
            .is_some_and(|s| s.types.values().any(|td| !td.signals.is_empty()))
        {
            self.ensure_mems_loaded(None);
        }
        match self.prepare_create(args, None, drift_warnings)? {
            CreatePrepareOutcome::Done(outcome) => Ok(outcome),
            CreatePrepareOutcome::Prepared(prepared) => {
                self.commit_prepared_create(prepared, actor, client, note)
            }
        }
    }

    /// Validate a create and compute everything up to (but not
    /// including) the disk write — the create-side prepare of the
    /// prepare-all-then-commit split `batch_update` established. Runs
    /// the resolve, validate and compose phases in that order.
    /// Returns [`CreatePrepareOutcome::Done`] for a dry-run (its
    /// outcome is complete), [`CreatePrepareOutcome::Prepared`] for a
    /// real write the caller commits via
    /// [`Self::commit_prepared_create`].
    ///
    /// `batch_skeleton_ids` is the batch path's staging set: ids the
    /// current batch has pre-inserted as skeleton entities so
    /// intra-batch references validate as REAL targets. A create whose
    /// id is in the set skips the already-exists refusal (the skeleton
    /// is this very entry's placeholder — batch-side identity checks
    /// have already refused genuine duplicates). Single-item callers
    /// pass `None`.
    /// NOTE: the reload-before-operation drift probe is the CALLER's
    /// job (single-item: `create_entity` probes its one mem; batch:
    /// `batch_create` probes every touched mem once, up front). A probe
    /// inside prepare would reload mid-batch and wipe the staged
    /// skeletons.
    fn prepare_create(
        &mut self,
        args: CreateEntityArgs,
        batch_skeleton_ids: Option<&std::collections::HashSet<EntityId>>,
        drift_warnings: Vec<WarningHint>,
    ) -> Result<CreatePrepareOutcome, EngineError> {
        let resolved = self.resolve_create(args)?;
        let validated = self.validate_create(resolved, batch_skeleton_ids, drift_warnings)?;
        self.compose_create(validated)
    }

    /// Cap on fully-detailed error envelopes in a refused batch's
    /// report — bounded reporting for very large failing batches.
    /// Entries beyond the cap still carry `action: "error"`; the
    /// result's `errors_suppressed` counts them. Never a silent
    /// truncation.
    pub const BATCH_ERROR_REPORT_CAP: usize = 50;

    /// CommitContext-bundling wrapper around [`Self::create_entity`].
    /// Destructures `CommitContext` into `(actor, client, note)`
    /// and delegates.
    pub fn create_entity_with_ctx(
        &mut self,
        args: CreateEntityArgs,
        ctx: &CommitContext<'_>,
    ) -> Result<CreateEntityOutcome, EngineError> {
        self.create_entity(args, ctx.actor, ctx.client.as_ref(), ctx.note.as_deref())
    }
}
