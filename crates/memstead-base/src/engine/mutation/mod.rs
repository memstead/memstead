//! Engine mutation entrypoints — split per mutation kind.
//!
//! Each sub-module implements one mutation of `Engine`: `create`,
//! `update` (with batch), `delete`, `relate`, `rename`. The shared
//! helpers (`today_iso`, `make_stub`, `gc_orphan_stubs`,
//! `lookup_title_and_type`, `unknown_type_error`) plus the typed
//! constants `PATCH_OLD_NOT_FOUND_CONTENT_CAP` and
//! `RELATIONSHIP_CYCLE_PATH_CAP` live here.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::entity::{Entity, EntityId};
use crate::store::Store;

use super::EngineError;

pub mod create;
pub mod delete;
pub mod mem_sweep;
pub mod parse_recovery;
pub mod relate;
pub mod rename;
pub mod update;

/// Look up an entity's `(title, entity_type)` pair in `store`. Both
/// `None` for missing-from-store ids — matches full's `title_for` /
/// `type_for` lossy-lookup contract. Used by [`Engine::changes_since`]
/// to enrich id-only envelopes the backend returned with metadata
/// from the in-memory store.
pub(super) fn lookup_title_and_type(
    store: &Store,
    id: &EntityId,
) -> (Option<String>, Option<String>) {
    match store.get(id) {
        Some(e) => (Some(e.title.clone()), Some(e.entity_type.clone())),
        None => (None, None),
    }
}

/// Maximum byte length of the truncated `current_content` snapshot
/// that [`EngineError::PatchOldNotFound`] carries. Keeps the wire
/// envelope bounded for sections with large bodies. Mirrors full's
/// `memstead_git_branch::PATCH_OLD_NOT_FOUND_CONTENT_CAP`.
pub const PATCH_OLD_NOT_FOUND_CONTENT_CAP: usize = 500;

/// Maximum number of entity IDs retained in
/// [`EngineError::RelationshipCycle::existing_path`]. Keeps the cycle
/// envelope bounded for pathologically long chains. Mirrors full's
/// `memstead_git_branch::RELATIONSHIP_CYCLE_PATH_CAP`.
pub const RELATIONSHIP_CYCLE_PATH_CAP: usize = 20;

/// Build an [`EngineError::UnknownType`] populated with the schema's
/// declared type names (sorted) and a fuzzy suggestion. Mirrors full's
/// `UnknownEntityType` recovery payload so MCP envelopes carry the
/// same `name` / `schema_ref` / `declared` / `suggestion` keys
/// regardless of which engine served the call.
/// Render an entity for a write, refusing the one state the generator would
/// silently make permanent.
///
/// **This is the mutation path's only way to bytes.** Calling
/// `generate_markdown` directly from a mutation verb bypasses the guard, and
/// the first version of this fix did exactly that: the check lived in
/// `update_entity`, so `memstead relate` against the same entity walked
/// straight past it and froze the absorption anyway (04/02, criterion 5,
/// found by the plan's grade). A guard a new verb can miss by following the
/// local idiom is not a guard; making the guarded call BE the idiom is.
///
/// The condition: a section whose stored body ends inside an unterminated
/// fence has already absorbed every section after it, and the generator
/// appends its closer AFTER those bytes. One write seals them inside a
/// legitimately fenced block where nothing can tell them from prose the
/// author meant to fence. It is refused rather than warned about because it
/// is unrecoverable once it lands.
///
/// The way out needs no special case: a caller who replaces the absorbing
/// section hands us an entity whose fence is closed, so the guard simply
/// does not fire.
pub(crate) fn render_for_write(
    entity: &Entity,
    type_def: &memstead_schema::TypeDefinition,
) -> Result<String, EngineError> {
    for (key, value) in &entity.sections {
        if let Some(fence) = crate::markdown::closing_fence_if_unterminated(value.trim()) {
            return Err(EngineError::UnterminatedFenceInStoredBody {
                id: entity.id.to_string(),
                section: key.clone(),
                fence,
                swallowed: crate::ops::integrity::swallowed_declared_sections(value, type_def),
            });
        }
    }
    Ok(crate::entity::generator::generate_markdown(
        entity, type_def,
    ))
}

pub(crate) fn unknown_type_error(schema: &memstead_schema::Schema, attempted: &str) -> EngineError {
    let mut declared: Vec<String> = schema.types.keys().cloned().collect();
    declared.sort();
    let (sname, sver) = schema.id();
    EngineError::UnknownType {
        name: attempted.to_string(),
        schema_ref: format!("{sname}@{sver}"),
        declared,
        suggestion: schema.suggest_type(attempted),
    }
}

/// The lowercase wire string for a [`crate::pipeline::MediumType`] — the
/// value the `INVALID_ANCHOR` recovery detail carries so an agent sees
/// which medium's namespace rejected the grain. Matches the enum's
/// `#[serde(rename_all = "lowercase")]` form.
pub(crate) fn medium_type_wire(t: crate::pipeline::MediumType) -> &'static str {
    use crate::pipeline::MediumType::*;
    match t {
        Codebase => "codebase",
        Filesystem => "filesystem",
        Graph => "graph",
        Git => "git",
        Web => "web",
    }
}

impl super::Engine {
    /// Resolve the single-source anchor-namespace context for `mem`, when
    /// unambiguous. An `anchors[]` element carries no source name, so the
    /// grain/namespace refusal ([`crate::anchor::AnchorValidationError::GrainNamespaceUnsupported`])
    /// can only be fired deterministically when the mem's bindings declare
    /// exactly one inline source; with zero or several the namespace check
    /// is skipped (the vocabulary + hash-semantics rules still apply).
    /// Returns the `(medium_type_wire, anchor_namespace)` pair the
    /// validator consumes — the medium *half* of the lone source.
    pub(crate) fn resolve_anchor_medium(&self, mem: &str) -> Option<(String, &'static str)> {
        let mut sources = self
            .pipeline_configs()
            .bindings
            .iter()
            .filter(|r| r.mem == mem)
            .flat_map(|r| r.config.sources.iter());
        let first = sources.next()?;
        if sources.next().is_some() {
            // Ambiguous — the anchor does not name which source it targets;
            // skip the namespace refinement rather than guess.
            return None;
        }
        let caps = crate::binding::medium_capabilities(first.medium_type);
        Some((
            medium_type_wire(first.medium_type).to_string(),
            caps.anchor_namespace,
        ))
    }

    /// Validate the permissive `anchors[]` inputs for a mutation against
    /// `mem`'s medium context into strict [`crate::anchor::Anchor`]s, or
    /// refuse the whole mutation with a typed
    /// [`EngineError::InvalidAnchor`]. Empty input yields an empty vec (no
    /// sidecar write); a single malformed element aborts before any state
    /// change so the entity is never written.
    pub(crate) fn validate_anchor_inputs(
        &self,
        mem: &str,
        inputs: &[crate::anchor::AnchorInput],
    ) -> Result<Vec<crate::anchor::Anchor>, EngineError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let medium = self.resolve_anchor_medium(mem);
        let medium_ref = medium.as_ref().map(|(t, ns)| (t.as_str(), *ns));
        let mut anchors: Vec<crate::anchor::Anchor> = inputs
            .iter()
            .map(|i| i.validate(medium_ref).map_err(EngineError::from))
            .collect::<Result<_, _>>()?;

        // One payload, one row per triple (consistency-sweep 03/03,
        // criterion 9). `(artifact, grain, class)` is the sidecar's merge
        // identity, so a payload naming it twice used to collapse to the last
        // occurrence and the caller was never told an anchor it wrote had
        // vanished. The unit is THIS payload: the same triple arriving in a
        // later call still replaces the stored row, which is what the
        // carry-forward depends on.
        {
            let mut seen: std::collections::HashSet<(&str, &str, &str)> =
                std::collections::HashSet::new();
            for a in &anchors {
                let key = (a.artifact.as_str(), a.grain.as_wire(), a.class.as_wire());
                if !seen.insert(key) {
                    return Err(EngineError::from(
                        crate::anchor::AnchorValidationError::DuplicateAnchorTriple {
                            artifact: a.artifact.clone(),
                            grain: a.grain.as_wire(),
                            class: a.class.as_wire(),
                        },
                    ));
                }
            }
        }

        // Supplied `content` under the source's preparation: the context-free
        // validator hashed the bytes under the default canonicalization; the
        // seam knows the anchor's source and re-hashes through the registry's
        // rule for that source (touchpoint A at write time), so the recorded
        // hash is the one a later observation computes.
        if inputs.iter().any(|i| i.content.is_some()) {
            let joins = self.anchor_source_roots(mem);
            for (input, anchor) in inputs.iter().zip(anchors.iter_mut()) {
                let (Some(content), Some(source)) = (&input.content, &anchor.source) else {
                    continue;
                };
                let Some(join) = joins.get(source) else {
                    continue;
                };
                if join.preparation.is_none() {
                    continue;
                }
                match crate::preparation::path_prepared_hash(
                    join.preparation.as_deref(),
                    &anchor.artifact,
                    anchor.grain,
                    content.as_bytes(),
                ) {
                    crate::preparation::PathPrepared::Hash(h) => anchor.hash = Some(h),
                    crate::preparation::PathPrepared::NoHash => {}
                    crate::preparation::PathPrepared::UnitAbsent => {
                        return Err(EngineError::from(
                            crate::anchor::AnchorValidationError::UnitAbsentFromContent {
                                artifact: anchor.artifact.clone(),
                            },
                        ));
                    }
                }
            }
        }

        // Source-vs-binding check: when an anchor names BOTH a producing
        // binding and a source, and that binding hash still resolves in
        // this workspace (reverse lookup — any later binding edit moves
        // the hash and drops earlier anchors into the accept-any-name
        // branch for good), the source must be one of the binding's
        // declared names. The bindings load is skipped entirely unless
        // some input needs it, and a missing workspace root or an
        // unloadable store degrades to accept-any-name — validation
        // must never require the binding to resolve.
        if anchors
            .iter()
            .any(|a| a.binding.is_some() && a.source.is_some())
            && let Some(root) = self.workspace_root()
            && let Ok(configs) = crate::pipeline_store::load_pipeline_configs(root)
        {
            for a in &anchors {
                let (Some(binding_hash), Some(source)) = (&a.binding, &a.source) else {
                    continue;
                };
                let Some(record) = configs
                    .bindings
                    .iter()
                    .find(|r| crate::binding::hash_binding(&r.config) == *binding_hash)
                else {
                    continue; // unresolvable binding: accept any non-empty name
                };
                let declared: Vec<String> = record
                    .config
                    .sources
                    .iter()
                    .map(|s| s.name.clone())
                    .collect();
                if !declared.iter().any(|n| n == source) {
                    return Err(EngineError::from(
                        crate::anchor::AnchorValidationError::SourceNotDeclared {
                            got: source.clone(),
                            declared,
                        },
                    ));
                }
            }
        }

        // Write time resolves or refuses (decision 26, plan 03a): a
        // path-grain anchor whose artifact resolves under NO candidate join
        // is a silently dead reference — refuse now, while the write moment
        // still has the binding context to say what the right dialect is.
        // Candidates in decision-29 priority: the source-join (the anchor's
        // `source` name resolved to its declared pointer) first, then the
        // workspace-relative form. An anchor naming a source the LOADED
        // roster does not declare (a typo, a renamed binding) gets no join
        // candidate — its workspace-relative form must resolve or the write
        // refuses, because resolution will make the same roster lookup and
        // orphan it at birth. The gate skips entirely only when it cannot
        // know: no workspace root, or a pipeline store whose LOAD fails as a
        // whole — validation never requires the binding store to be
        // readable. (A corrupted individual binding file does not fail the
        // load; it yields a reduced roster, and the gate then fail-closes on
        // paths that resolve under no surviving candidate.)
        if let Some(root) = self.workspace_root()
            && crate::pipeline_store::load_pipeline_configs(root).is_ok()
        {
            let source_roots = self.anchor_source_roots(mem);
            for a in &anchors {
                match a.grain {
                    crate::anchor::AnchorGrain::Span
                    | crate::anchor::AnchorGrain::File
                    | crate::anchor::AnchorGrain::Tree => {}
                    crate::anchor::AnchorGrain::Url | crate::anchor::AnchorGrain::Entity => {
                        continue;
                    }
                }
                let base = crate::engine::query::anchor_base_path(&a.artifact);
                let mut candidates: Vec<String> = Vec::new();
                if let Some(source) = &a.source
                    && let Some(join) = source_roots.get(source)
                {
                    candidates.push(crate::engine::query::join_pointer(&join.pointer, base));
                }
                candidates.push(base.to_string());
                if !candidates.iter().any(|c| root.join(c).exists()) {
                    return Err(EngineError::from(
                        crate::anchor::AnchorValidationError::ArtifactUnresolvable {
                            artifact: a.artifact.clone(),
                            candidates,
                        },
                    ));
                }
            }
        }

        Ok(anchors)
    }

    /// Validate an update's `anchors_unset[]` payload up front — a
    /// malformed selector (missing artifact, unknown grain/class wire
    /// string) refuses the whole mutation with the same typed
    /// `INVALID_ANCHOR` envelope as a malformed `anchors[]` element.
    /// Empty payload → empty vec.
    pub(crate) fn validate_anchor_unsets(
        inputs: &[crate::anchor::AnchorUnsetInput],
    ) -> Result<Vec<crate::anchor::AnchorUnset>, EngineError> {
        inputs
            .iter()
            .map(|i| i.validate().map_err(EngineError::from))
            .collect()
    }

    /// Record verify-observed prepared-content hashes onto **hash-less
    /// hash-bearing** anchors in `mem_name`'s anchors sidecar — the
    /// measurement-bookkeeping backfill the verify pass hands over via
    /// [`crate::ingest::VerifyOutcome::hash_backfill`].
    ///
    /// This mutates **only** the engine-owned sidecar
    /// ([`crate::anchor::ANCHOR_SIDECAR_PATH`]): no entity content, no
    /// section, no `_hash` is touched — an anchor-only commit yields zero
    /// entity deltas by construction. Guards enforced at this write seam,
    /// not left to callers:
    ///
    /// - only a hash-bearing class (`anchored` / `derived`) may gain a hash —
    ///   an `authored` / `informed-by` anchor is never written, whatever the
    ///   caller observed;
    /// - an anchor that already carries a hash is never overwritten — the
    ///   recorded hash is the drift baseline, so the backfill is idempotent
    ///   (a second identical call stages nothing and produces no commit).
    ///
    /// Returns how many anchors gained a hash. Zero writes ⇒ no commit.
    pub fn record_anchor_observed_hashes(
        &mut self,
        mem_name: &str,
        observed: &[crate::anchor::ObservedArtifactHash],
        note: Option<&str>,
    ) -> Result<usize, EngineError> {
        if observed.is_empty() {
            return Ok(0);
        }
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }
        // Same posture as every other commit-producing write: probe for
        // sibling-engine drift so the sidecar merge runs against current truth.
        let _warnings = self.reload_if_stale(Some(mem_name));

        let backend = self.mounts[mount_idx].backend.as_ref();
        let mut sidecar = read_sidecar(backend)?;
        let mut written = 0usize;
        for obs in observed {
            let Some(anchors) = sidecar.entities.get_mut(&obs.entity) else {
                continue;
            };
            for a in anchors {
                if a.class.is_hash_bearing() && a.hash.is_none() && a.artifact == obs.artifact {
                    a.hash = Some(obs.hash.clone());
                    // Stamp the origin (consistency-sweep 03/03, criterion 8):
                    // this baseline is the engine's inference from what it
                    // observed, not something an author pinned, and a reader
                    // comparing drift needs to know which.
                    a.hash_source = Some(crate::anchor::AnchorHashSource::Backfill);
                    written += 1;
                }
            }
        }
        if written == 0 {
            return Ok(0);
        }
        backend.write_anchors_sidecar(&sidecar.to_bytes())?;
        let ctx = crate::vcs::CommitContext {
            actor: crate::vcs::Actor::Agent,
            client: None,
            tool: Some("record_anchor_observed_hashes"),
            note: note.map(String::from),
            role: self.current_role,
            logical_operation_id: None,
            entity_ids: None,
        };
        let commit_sha = backend.commit(
            &format!("memstead: anchor-hash backfill ({written} anchor(s))"),
            &ctx,
        )?;
        self.record_self_write(mount_idx, &commit_sha);
        // Anchor-hash backfill returns a count, not an agent-facing response,
        // so an intervention report has nowhere to ride. Discarded knowingly:
        // the merge itself still happened, so nothing was lost — only the
        // notice that someone else had written (04/03, criterion 3).
        let _intervention_has_no_channel_here = self.stamp_mutation_versions(mount_idx);
        Ok(written)
    }

    /// Record on the mem which engine version and which resolved
    /// schema performed the mutation that just committed
    /// (`MemConfig.mutation_stamp`). Called by every mutation verb
    /// after `record_self_write` — one shared implementation, per the
    /// one-guard-on-all-write-paths principle — and deliberately NOT
    /// by `apply_external_commit`: a replayed sibling commit was
    /// stamped by the engine that performed it, and this engine must
    /// not claim it.
    ///
    /// Write-cheap by construction: the config write happens only when
    /// the stamp VALUE changes (a binary upgrade or a schema repin) —
    /// steady-state mutations compare and return. The write is
    /// best-effort: a failed stamp never fails the mutation that
    /// preceded it. On git-branch backends the config rides the
    /// `__MEMSTEAD` ref, so a stamp write never moves the mem branch
    /// head — mutation `commit_sha` cursors stay valid.
    pub(crate) fn stamp_mutation_versions(
        &mut self,
        mount_idx: usize,
    ) -> Vec<crate::ops::WarningHint> {
        let Some(state) = self.mounts.get(mount_idx) else {
            return Vec::new();
        };
        let mem = state.mount.mem.clone();
        let Some(schema) = self.schemas.get(&mem) else {
            return Vec::new();
        };
        let (name, version) = schema.id();
        // Full build version (semver + git build sha when present) so
        // a rebuild between mutations is a recordable — and hence
        // skew-detectable — event even between releases.
        let stamp = memstead_schema::MutationStamp {
            engine_version: crate::build_info::full_version().to_string(),
            schema: format!("{name}@{version}"),
        };
        // A mem with no loaded config has nowhere to carry the stamp;
        // skip silently (in-memory sketches, minimal fixtures).
        let Some(config) = self
            .mounts
            .get(mount_idx)
            .and_then(|s| s.mem_config.as_ref())
        else {
            return Vec::new();
        };
        // Skew at WRITE time, and before the restamp below erases the evidence
        // (04/04, criterion 9). Boot-only detection meant a long-lived server
        // that started under one binary and was written to by another never
        // said so, and the very write that would have revealed it wrote the
        // stamp that hid it. The write is never refused (criterion 10): a
        // deliberate downgrade is the operator's business.
        let mut warnings = Vec::new();
        if let Some(prior) = config.mutation_stamp.as_ref()
            && let Some(direction) = crate::build_info::skew_direction(
                &prior.engine_version,
                crate::build_info::full_version(),
            )
        {
            warnings.push(crate::ops::WarningHint::EngineVersionSkew {
                mem: mem.clone(),
                stamped_engine: prior.engine_version.clone(),
                running_engine: crate::build_info::full_version().to_string(),
                stamped_schema: prior.schema.clone(),
                direction,
            });
        }
        if config.mutation_stamp.as_ref() == Some(&stamp) {
            return warnings;
        }
        // Through the shared writer like the seven lifecycle setters
        // (04/03, criterion 7). This one is why the damage looked
        // spontaneous: it rides ordinary create/update/relate/rename/delete,
        // so an operator saw a config field vanish during an innocuous entity
        // write with no lifecycle call in sight. It stays exactly as dormant
        // as before, because the equality guard above still decides whether
        // to write at all; what changed is only what it writes over.
        // The intervention rides the ENTITY mutation's own response: this
        // writer has no response of its own, and the operator who sees a
        // config field move during an innocuous entity write is owed the
        // reason there (04/03, criterion 3, found by the plan's grade —
        // an earlier draft discarded this with `let _`).
        match self.write_mem_config_merged(
            mount_idx,
            &mem,
            Some("engine version stamp"),
            &move |c: &mut memstead_schema::config::MemConfig| {
                c.mutation_stamp = Some(stamp.clone());
            },
        ) {
            Ok((_, intervened)) if !intervened.is_empty() => {
                warnings.push(crate::ops::WarningHint::ConfigWriteIntervened {
                    mem,
                    fields: intervened,
                });
                warnings
            }
            _ => warnings,
        }
    }
}

/// Stage a write of `entity_id`'s anchors into the mem's anchors sidecar
/// through `backend`, merged over the existing sidecar at BOTH levels —
/// other entities' rows survive (document level), and within the entity's
/// own row `unsets` apply first, then each incoming anchor replaces the
/// existing anchor with the same `(artifact, grain, class)` triple or
/// appends ([`crate::anchor::AnchorSidecar::merge`]). Writing never
/// removes an anchor the call did not name in `unsets`. The write is
/// buffered into the SAME pending op set the entity write used — so the
/// next [`crate::backend::MemBackend::commit`] carries entity + anchors
/// as one atomic commit. Reads honour pending-buffer precedence, so
/// successive stages within one transaction compose.
/// Stage a mutation of the engine-owned derivations sidecar
/// (agent-trust plan 12) so it rides the SAME commit as the edge
/// write that produced it — the anchors-sidecar atomicity precedent.
/// The sidecar travels through the backend's normal entity-path
/// read/write under `.memstead/`, which every backend filters from
/// entity listings and every archive/export path carries as-is.
pub(crate) fn stage_derivation_sidecar(
    backend: &dyn crate::backend::MemBackend,
    mutate: impl FnOnce(&mut crate::derivation::DerivationSidecar),
) -> Result<(), EngineError> {
    let path = std::path::Path::new(crate::derivation::DERIVATION_SIDECAR_PATH);
    let mut sidecar = match backend.read_entity(path)? {
        Some(bytes) => crate::derivation::DerivationSidecar::from_bytes(&bytes).map_err(|e| {
            EngineError::Backend(crate::backend::BackendError::Other(format!(
                "derivations sidecar parse: {e}"
            )))
        })?,
        None => crate::derivation::DerivationSidecar::default(),
    };
    mutate(&mut sidecar);
    backend.write_entity(path, &sidecar.to_bytes())?;
    Ok(())
}

/// True when `schema` declares `rel_type` as a derivation
/// (`derivation: true` on the relationship definition) — the
/// predicate every write path shares, so baseline recording cannot
/// fork per verb.
pub(crate) fn rel_type_declares_derivation(
    schema: &memstead_schema::Schema,
    rel_type: &str,
) -> bool {
    schema
        .manifest
        .relationships
        .definitions
        .iter()
        .any(|d| d.name == rel_type && d.derivation)
}

pub(crate) fn stage_anchors_sidecar(
    backend: &dyn crate::backend::MemBackend,
    entity_id: &EntityId,
    unsets: &[crate::anchor::AnchorUnset],
    anchors: Vec<crate::anchor::Anchor>,
) -> Result<(), EngineError> {
    let mut sidecar = match backend.read_anchors_sidecar()? {
        Some(bytes) => crate::anchor::AnchorSidecar::from_bytes(&bytes).map_err(|e| {
            EngineError::Backend(crate::backend::BackendError::Other(format!(
                "anchors sidecar parse: {e}"
            )))
        })?,
        None => crate::anchor::AnchorSidecar::default(),
    };
    sidecar.merge(entity_id.as_ref(), unsets, anchors);
    backend.write_anchors_sidecar(&sidecar.to_bytes())?;
    Ok(())
}

/// Load the mem's anchors sidecar through `backend`, or the empty
/// document when none exists yet. Shared by the delete / rename legs
/// which must decide whether the entity actually has anchor rows before
/// staging a sidecar write (so an entity with none stays byte-identical
/// to a pre-anchor mutation).
fn read_sidecar(
    backend: &dyn crate::backend::MemBackend,
) -> Result<crate::anchor::AnchorSidecar, EngineError> {
    match backend.read_anchors_sidecar()? {
        Some(bytes) => crate::anchor::AnchorSidecar::from_bytes(&bytes).map_err(|e| {
            EngineError::Backend(crate::backend::BackendError::Other(format!(
                "anchors sidecar parse: {e}"
            )))
        }),
        None => Ok(crate::anchor::AnchorSidecar::default()),
    }
}

/// Stage removal of `entity_id`'s anchor row into the same commit as an
/// entity delete — a no-op (no sidecar write, so byte-identical to today)
/// when the entity carries no anchors. Returns whether a write was staged.
pub(crate) fn stage_anchors_removal(
    backend: &dyn crate::backend::MemBackend,
    entity_id: &EntityId,
) -> Result<bool, EngineError> {
    let mut sidecar = read_sidecar(backend)?;
    if sidecar.get(entity_id.as_ref()).is_empty() {
        return Ok(false);
    }
    sidecar.remove(entity_id.as_ref());
    backend.write_anchors_sidecar(&sidecar.to_bytes())?;
    Ok(true)
}

/// Stage a move of `from`'s anchor row to `to` into the same commit as an
/// entity rename — leaving zero rows under the old id. A no-op (byte-
/// identical to today) when the renamed entity carries no anchors. Returns
/// whether a write was staged.
pub(crate) fn stage_anchors_rename(
    backend: &dyn crate::backend::MemBackend,
    from: &EntityId,
    to: &EntityId,
) -> Result<bool, EngineError> {
    let mut sidecar = read_sidecar(backend)?;
    if sidecar.get(from.as_ref()).is_empty() {
        return Ok(false);
    }
    sidecar.rename(from.as_ref(), to.as_ref());
    backend.write_anchors_sidecar(&sidecar.to_bytes())?;
    Ok(true)
}

// A `today_iso()` wall-clock convenience used to live here, for tests
// comparing an auto-stamp against "roughly now". It is deliberately
// gone: the stamp is second-resolution, so every such comparison races
// the clock between the mutation and the assertion, and one of them
// duly failed on a suite run that straddled midnight. Tests that need
// a stamped value pin `Engine::set_mutation_clock` and derive the
// expected string from the same instant via [`iso_from_system_time`].

/// An instant as a full ISO-8601 datetime string `YYYY-MM-DDTHH:MM:SSZ`
/// (UTC). Used by mutation paths that auto-stamp metadata fields
/// (e.g. `last_modified` on update, `created_date` on create).
///
/// This is second-resolution (rather than
/// date-only `YYYY-MM-DD`) so intra-day
/// updates produce distinguishable timestamps and drift / staleness
/// queries become per-update aware. The strict-mode date validator
/// already accepts both forms (`^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}:\d{2}Z)?$`)
/// so existing entities written with the date-only form continue to
/// load; new writes carry the wider form.
///
/// Pure function: no allocation outside the `format!` invocation,
/// no error path (the fallback to UNIX epoch on an instant before
/// the epoch is acceptable for a best-effort timestamp).
/// Howard-Hinnant civil-from-days for the date half; trivial modular
/// arithmetic for the time half.
pub(super) fn iso_from_system_time(t: std::time::SystemTime) -> String {
    let now = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();
    let days = secs / 86400;
    let secs_of_day = secs % 86400;
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    let ss = secs_of_day % 60;
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Sweep stubs whose last incoming edge has just disappeared. Returns
/// the dropped ids so callers can surface them to the agent (e.g. via
/// [`DeleteEntityOutcome::orphan_stubs_removed`]).
///
/// Stubs are auto-created when a relate names an absent target — a
/// "promise" that a real entity will land there later (see
/// [`make_stub`]). When the last referrer drops its edge or is itself
/// deleted, the promise has no holder and becomes pure bloat. Only
/// stubs are eligible — real entities never count as orphans via this
/// path.
pub(super) fn gc_orphan_stubs(store: &mut Store) -> Vec<EntityId> {
    let stub_ids: Vec<EntityId> = store
        .all_entities()
        .filter(|e| e.stub)
        .map(|e| e.id.clone())
        .collect();
    gc_orphan_stubs_among(store, &stub_ids)
}

/// Scoped orphan-stub sweep: GC only the stubs *among `candidates`*
/// whose last incoming edge has just disappeared, returning the dropped
/// ids. This is the single home of the orphan-stub predicate (`stub &&
/// no incoming`) — the three write paths that can sever a stub's last
/// referrer all funnel through here so they cannot drift:
/// [`gc_orphan_stubs`] (delete's full-store sweep) supplies every stub
/// id; the `memstead_relate(remove)` path supplies the just-severed target;
/// the `memstead_update` alias-resync path supplies the entity's
/// pre-mutation body-link targets (the only edges that commit could
/// have dropped). Scoping to a candidate set rather than walking the
/// whole store keeps each path from GC'ing pre-existing orphans that
/// aren't its responsibility. Candidates are de-duplicated; a candidate
/// that is absent, not a stub, or still has a referrer is left
/// untouched.
pub(super) fn gc_orphan_stubs_among<'a>(
    store: &mut Store,
    candidates: impl IntoIterator<Item = &'a EntityId>,
) -> Vec<EntityId> {
    let mut removed: Vec<EntityId> = Vec::new();
    let mut seen: std::collections::HashSet<&EntityId> = std::collections::HashSet::new();
    for id in candidates {
        if !seen.insert(id) {
            continue;
        }
        if store.get(id).is_some_and(|e| e.stub) && store.incoming(id).is_empty() {
            store.remove(id);
            removed.push(id.clone());
        }
    }
    removed
}

/// Shared target-id grammar validator. The wiki-link grammar gate
/// runs on every relation-authoring path (`memstead_relate`,
/// `memstead_create.relations[]`, future inline-relation surfaces) so a
/// malformed target id (e.g. `bad@chars$here`) cannot land an
/// auto-stub at the literal id — that stub would later fail every
/// wiki-link parse that referenced it. Pre-Item-02 the gate lived
/// only on `memstead_relate`; the create path admitted the same input
/// silently.
pub(super) fn validate_relation_target_grammar(target: &EntityId) -> Result<(), EngineError> {
    if let Err(reason) = crate::entity::id::validate_mem_name_grammar(target.mem()) {
        return Err(EngineError::InvalidEntityId {
            id: target.to_string(),
            reason,
        });
    }
    if let Err(reason) = crate::entity::id::validate_id_path_grammar(target.path()) {
        return Err(EngineError::InvalidEntityId {
            id: target.to_string(),
            reason,
        });
    }
    Ok(())
}

/// Auto-stamp `auto_timestamp` metadata fields on an entity that's
/// about to be re-written. Extracted from the update-path hot loop so
/// the relate-path (add and remove) and the rename-path (the renaming
/// entity plus every referrer the rewrite cascade touched) can
/// invoke the same engine-driven stamp.
///
/// Walks the type's metadata-field declarations; any field flagged
/// `auto_timestamp: true` (the default schema declares this on
/// `last_modified`) is set to the supplied `today` ISO string. The
/// helper is a no-op on schemas that declare no auto-timestamp
/// fields. Callers pre-compute `today` via [`today_iso`] so a single
/// mutation that touches multiple entities (rename's referrer rewrite
/// cascade) stamps them all with the same value.
pub(super) fn auto_stamp_timestamps(
    entity: &mut Entity,
    type_def: &memstead_schema::TypeDefinition,
    today: &str,
) {
    for field_def in &type_def.metadata_fields {
        if field_def.auto_timestamp {
            entity.metadata.insert(
                field_def.key.clone(),
                crate::entity::MetadataValue::String(today.to_string()),
            );
        }
    }
}

/// Build a stub [`Entity`] for an unresolved relate target. Callers
/// declare the stub's origin via [`crate::entity::StubKind`] —
/// `ForwardReference` for `memstead_relate` to an absent target,
/// `Residual { since_commit, readonly_referrers }` for the
/// delete/rename demote path. The kind persists for the engine
/// instance's lifetime; a reload reduces every stub to `LoadTime`
/// — the kind is annotation, not state.
///
/// The stub is in-store but unwritten to disk — `entity_type` empty,
/// `file_path` empty, no metadata, no sections, `stub: true` and
/// `stub_kind: Some(kind)` set together. A later
/// [`Engine::create_entity`] at the same id promotes the stub to a
/// real entity (loader / parse-result merge handles the upgrade
/// path; `stub_kind` clears to `None`).
pub(super) fn make_stub(id: &EntityId, kind: crate::entity::StubKind) -> Entity {
    Entity {
        id: id.clone(),
        title: id.name().to_string(),
        entity_type: String::new(),
        mem: id.mem().to_string(),
        file_path: String::new(),
        metadata: IndexMap::new(),
        sections: IndexMap::new(),
        relationships: Vec::new(),
        content_hash: String::new(),
        stub: true,
        stub_kind: Some(kind),
        heading_spans: HashMap::new(),
        raw_section_headings: Vec::new(),
    }
}

/// Cross-mem add-path policy gate. Same-mem writes bypass; the
/// `[cross_mem_links]` table only gates writes that cross the
/// mem boundary. Cross-mem writes consult
/// [`super::Engine::cross_mem_link_allowed`] in the edge's actual
/// direction (`source_mem → target_mem`). Disallowed pairings
/// surface [`EngineError::CrossMemLinkNotAllowed`] with the
/// `(from_mem, to_mem)` payload an agent already sees on
/// `memstead_relate`.
///
/// After the grant admits the pairing, a target absent from a
/// `MountCapability::ReadOnly` mount refuses with
/// [`EngineError::CrossMemTargetNotFound`]: the engine cannot
/// persist a stub through the read-only boundary, and a read-only
/// mem never gains the entity later — a missing target there is a
/// wrong link, not a pending forward reference. Same-mem targets,
/// cross-mem targets in Write mounts, and unmounted target mems all
/// retain the auto-stub mechanic.
///
/// Funnel point for every add-shaped edge write — `memstead_relate`,
/// `memstead_create.relations[]`, `memstead_update.declare_relations`,
/// body-wiki-link alias synthesis, and any future add-path mutation
/// surface route through one gate so the policy can't drift between
/// sites. Remove-shaped writes (cleanup) remain permissive and call
/// this helper not at all.
pub(super) fn validate_cross_mem_add_policy(
    engine: &super::Engine,
    source_mem: &str,
    target: &EntityId,
) -> Result<(), EngineError> {
    let target_mem = target.mem();
    if source_mem == target_mem {
        return Ok(());
    }
    if !engine.cross_mem_link_allowed(source_mem, target_mem) {
        return Err(EngineError::CrossMemLinkNotAllowed {
            from_mem: source_mem.to_string(),
            to_mem: target_mem.to_string(),
        });
    }
    if let Some(mount) = engine.mount(target_mem)
        && mount.capability == crate::workspace::MountCapability::ReadOnly
        && !engine.store.contains(target)
        && !matches!(
            probe_deferred_target(engine, target)?,
            DeferredTargetProbe::Exists
        )
    {
        return Err(EngineError::CrossMemTargetNotFound {
            target_id: target.to_string(),
            target_mem: target_mem.to_string(),
        });
    }
    Ok(())
}

/// Storage verdict for a cross-mem target whose mem is mounted but
/// DEFERRED (lazy, not yet loaded) — the write-time verification of
/// flywheel W7/02. The check asks the mem's real storage through the
/// cheap [`crate::backend::MemBackend::entity_exists`] probe
/// (tree-lookup-class on git-branch, metadata-class on folder) and
/// never triggers the mem's load: verification must not convert into
/// a full-load side effect (plan 01's seam).
pub(super) enum DeferredTargetProbe {
    /// The target's mem is loaded (the store is the truth) or not
    /// mounted at all (no storage handle to ask — the
    /// forward-reference mechanic governs).
    NotApplicable,
    /// Storage holds the entity: the reference is verified against
    /// real storage even though the mem is unloaded.
    Exists,
    /// Storage answers and the entity is not there.
    Absent,
}

pub(super) fn probe_deferred_target(
    engine: &super::Engine,
    target: &EntityId,
) -> Result<DeferredTargetProbe, EngineError> {
    let rel_path = crate::entity::id::id_to_file_path(target);
    if engine.mem_is_deferred(target.mem()) {
        let Some(mounted) = engine.mounts.iter().find(|m| m.mount.mem == target.mem()) else {
            return Ok(DeferredTargetProbe::NotApplicable);
        };
        return Ok(
            if mounted
                .backend
                .entity_exists(std::path::Path::new(&rel_path))?
            {
                DeferredTargetProbe::Exists
            } else {
                DeferredTargetProbe::Absent
            },
        );
    }
    // UNMOUNTED mem: ask the workspace layer's discovery hook. No
    // hook, or no discoverable storage → NotApplicable (the
    // forward-reference mechanic governs, unchanged).
    if engine.mount(target.mem()).is_none()
        && let Some(prober) = &engine.unmounted_storage_prober
        && let Some(storage) = prober(target.mem())
    {
        return Ok(
            if storage
                .backend
                .entity_exists(std::path::Path::new(&rel_path))?
            {
                DeferredTargetProbe::Exists
            } else {
                DeferredTargetProbe::Absent
            },
        );
    }
    Ok(DeferredTargetProbe::NotApplicable)
}

/// The one-blob type read for a storage-verified deferred target: the
/// shape check needs the target's real entity type, and a tree hit
/// proves existence, not type. Reads exactly the resolved path's
/// bytes and peeks the frontmatter `type:` — never the mem. Returns
/// `None` when the entity is absent or declares no type (the shape
/// gate then admits, the same posture the stub-bound case has
/// always had — the check never guesses).
/// The stub kind for a target being auto-stubbed on an add path: a
/// target that storage VERIFIES inside a deferred mem gets the
/// load-time kind — the stub is only plan 01's until-load
/// representation of a link into an unloaded mem, not a forward
/// reference to something that awaits creation. Everything else keeps
/// `ForwardReference`. Kinds stay annotation-not-state either way.
pub(super) fn deferred_verified_stub_kind(
    engine: &super::Engine,
    target: &EntityId,
) -> Result<crate::entity::StubKind, EngineError> {
    Ok(
        if matches!(
            probe_deferred_target(engine, target)?,
            DeferredTargetProbe::Exists
        ) {
            crate::entity::StubKind::LoadTime
        } else {
            crate::entity::StubKind::ForwardReference
        },
    )
}

pub(super) fn peek_deferred_target_type(
    engine: &super::Engine,
    target: &EntityId,
) -> Result<Option<String>, EngineError> {
    let rel_path = crate::entity::id::id_to_file_path(target);
    let bytes = if engine.mem_is_deferred(target.mem()) {
        let Some(mounted) = engine.mounts.iter().find(|m| m.mount.mem == target.mem()) else {
            return Ok(None);
        };
        mounted
            .backend
            .read_entity(std::path::Path::new(&rel_path))?
    } else if engine.mount(target.mem()).is_none()
        && let Some(prober) = &engine.unmounted_storage_prober
        && let Some(storage) = prober(target.mem())
    {
        storage
            .backend
            .read_entity(std::path::Path::new(&rel_path))?
    } else {
        return Ok(None);
    };
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    Ok(String::from_utf8(bytes)
        .ok()
        .and_then(|c| crate::entity::parser::peek_type_from_frontmatter(&c)))
}

/// The target-schema REF for cross-schema edge routing. Loaded mems
/// answer from the engine's schema catalogue; an UNMOUNTED mem with
/// discoverable storage answers from its stored config's pin via the
/// discovery hook (flywheel W7/02) — the routing check
/// (`validate_cross_mem_edge`) needs only the ref, never the full
/// schema, so the source schema's `cross_mem_relationships:` entry
/// keeps its authority without a mount. `None` falls back to the
/// intra-mem path, exactly the pre-existing posture.
pub(super) fn target_schema_ref_for_routing(
    engine: &super::Engine,
    target_mem: &str,
) -> Option<memstead_schema::SchemaRef> {
    if let Some(s) = engine.schemas.get(target_mem) {
        let (name, version) = s.id();
        return Some(memstead_schema::SchemaRef::new(name, version));
    }
    if engine.mount(target_mem).is_none()
        && let Some(prober) = &engine.unmounted_storage_prober
        && let Some(storage) = prober(target_mem)
    {
        return storage.schema;
    }
    None
}

/// Outcome of the engine's edge-validation router for a single
/// inline / explicit relate. Carries the optional open-mode warning
/// from the intra-mem flow; the cross-mem flow has no
/// open-mode (cross-mem entries are declared vocabulary).
pub(super) enum EdgeRouteOutcome {
    Ok,
    OpenModeWarning(Box<crate::ops::WarningHint>),
}

/// Run rel-type + shape validation for one edge, routing through
/// intra-mem vocabulary or the source schema's
/// `cross_mem_relationships:` section as appropriate.
///
/// The routing rule:
/// when `source_mem != target_mem` AND the target mem's
/// pinned schema differs from the source schema by name or by
/// version, the source schema's `cross_mem_relationships:` entry
/// for the target schema is the sole authority for both the
/// vocabulary check (`INVALID_REL_TYPE`) and the shape check
/// (`INVALID_REL_SHAPE`). If no matching entry exists, surface
/// [`EngineError::CrossMemEdgeNotDeclared`].
///
/// Otherwise (same-mem, same-schema cross-mem, or target mem
/// unmounted) the call falls through to the existing intra-mem
/// validators — the same behaviour the intra-mem path always had.
///
/// `check_shape` mirrors the relate path's add-only shape posture:
/// pass `false` to skip the shape check (currently only the
/// `memstead_relate --remove` path). The vocabulary check still fires
/// in that case, matching the intra-mem behaviour where
/// `validate_rel_type` runs on both add and remove.
// The nine parameters are one edge's full coordinates; a params struct
// would restate the same fields at every call site without grouping
// anything that travels together elsewhere.
#[allow(clippy::too_many_arguments)]
pub(super) fn route_edge_validation(
    engine: &super::Engine,
    rel_type: &str,
    from_type: &str,
    to_type: Option<&str>,
    source_mem: &str,
    target_mem: &str,
    from_id: &EntityId,
    to_id: &EntityId,
    check_shape: bool,
) -> Result<EdgeRouteOutcome, EngineError> {
    use crate::runtime_validator::{
        CrossMemRelCheck, RelationshipCheck, validate_cross_mem_edge, validate_rel_shape,
        validate_rel_type,
    };
    use memstead_schema::SchemaRef;

    let source_schema = engine
        .schemas
        .get(source_mem)
        .expect("schema present for every registered mount");

    let target_schema_ref: Option<SchemaRef> = if source_mem == target_mem {
        None
    } else {
        target_schema_ref_for_routing(engine, target_mem)
    };
    let cross_mem_different = match (&target_schema_ref, source_schema.id()) {
        (Some(target), (src_name, _)) => target.name != src_name,
        (None, _) => false,
    };

    if cross_mem_different {
        let target_ref = target_schema_ref
            .as_ref()
            .expect("target_schema_ref is Some when cross_mem_different");
        if !check_shape {
            // Cleanup posture: cross-mem remove stays permissive so
            // pre-tightening edges remain droppable without first
            // re-declaring them. Mirrors the intra-mem shape gate's
            // add-only stance.
            return Ok(EdgeRouteOutcome::Ok);
        }
        match validate_cross_mem_edge(
            rel_type,
            from_type,
            to_type,
            source_schema.as_ref(),
            target_ref,
        ) {
            CrossMemRelCheck::Ok => Ok(EdgeRouteOutcome::Ok),
            CrossMemRelCheck::EdgeNotDeclared => {
                let (src_name, src_version) = source_schema.id();
                Err(EngineError::CrossMemEdgeNotDeclared {
                    source_schema: SchemaRef::new(src_name, src_version).as_display(),
                    target_schema: target_ref.as_display(),
                    rel_type: rel_type.to_string(),
                    from_id: from_id.to_string(),
                    to_id: to_id.to_string(),
                })
            }
            CrossMemRelCheck::Invalid(v) => Err(EngineError::Validation(v)),
        }
    } else {
        let warning_hint = match validate_rel_type(rel_type, source_schema.as_ref())? {
            RelationshipCheck::Ok => None,
            RelationshipCheck::OpenWarning(message) => {
                Some(crate::ops::WarningHint::UndeclaredRelationshipOpen {
                    rel_type: rel_type.to_string(),
                    message,
                })
            }
        };
        if check_shape {
            validate_rel_shape(rel_type, from_type, to_type, source_schema.as_ref())?;
        }
        Ok(match warning_hint {
            Some(w) => EdgeRouteOutcome::OpenModeWarning(Box::new(w)),
            None => EdgeRouteOutcome::Ok,
        })
    }
}

/// Cycle-family gate for one prospective edge — the single owner of
/// both refusals, shared by every edge-writing verb (`memstead_relate`,
/// `memstead_create.relations[]`, `memstead_update.declare_relations`, and the
/// batch paths, which stage prior items' edges into `store` so an
/// intra-batch cycle refuses like a stored one):
///
/// - **Self-loop on a listed no-self-loop rel-type.** `from == to` on
///   any rel-type the source type lists in `no_self_loop_relationships`
///   refuses, regardless of the `acyclic` flag — the declaration's one
///   effect (see `TypeDefinition::no_self_loop_relationships`).
/// - **Cycle on an acyclic rel-type.** An add closing a back-path
///   `to → … → from` (via [`crate::graph::query::would_cycle`]) refuses
///   with the existing path, capped at [`RELATIONSHIP_CYCLE_PATH_CAP`].
/// - **Cycle in a declared acyclicity set.** When the rel-type belongs
///   to a `relationships.acyclic_sets` set, an add closing a back-path
///   in the set's UNION subgraph (via
///   [`crate::graph::query::would_cycle_in_set`]) refuses; the payload
///   additionally echoes the set and the path's per-hop rel-types.
///
/// All refuse [`EngineError::RelationshipCycle`] (`RELATIONSHIP_CYCLE`)
/// with identical recovery detail on every path. Callers skip this on
/// remove paths — removal can only break cycles, never close one.
pub(super) fn validate_edge_acyclicity(
    store: &Store,
    schema: &memstead_schema::Schema,
    from: &EntityId,
    from_type: &str,
    to: &EntityId,
    rel_type: &str,
) -> Result<(), EngineError> {
    if from == to && schema.type_refuses_self_loop(from_type, rel_type) {
        return Err(EngineError::RelationshipCycle {
            rel_type: rel_type.to_string(),
            from: from.clone(),
            to: to.clone(),
            existing_path: vec![from.clone()],
            path_truncated: false,
            acyclic_set: None,
            existing_path_rel_types: None,
        });
    }
    if schema.relationship_acyclic(rel_type)
        && let Some(path) = crate::graph::query::would_cycle(store, from, to, rel_type)
    {
        let truncated = path.len() > RELATIONSHIP_CYCLE_PATH_CAP;
        let mut existing_path = path;
        if truncated {
            existing_path.truncate(RELATIONSHIP_CYCLE_PATH_CAP);
        }
        return Err(EngineError::RelationshipCycle {
            rel_type: rel_type.to_string(),
            from: from.clone(),
            to: to.clone(),
            existing_path,
            path_truncated: truncated,
            acyclic_set: None,
            existing_path_rel_types: None,
        });
    }
    // Cycle in a declared acyclicity SET: the union subgraph of the
    // set must stay acyclic, so the back-path may mix rel-types. The
    // refusal is additive — it echoes the declared set and one
    // rel-type per hop of the path.
    if let Some(set) = schema.acyclic_set_containing(rel_type)
        && let Some((path, path_rels)) =
            crate::graph::query::would_cycle_in_set(store, from, to, set)
    {
        let truncated = path.len() > RELATIONSHIP_CYCLE_PATH_CAP;
        let mut existing_path = path;
        let mut existing_path_rel_types = path_rels;
        if truncated {
            existing_path.truncate(RELATIONSHIP_CYCLE_PATH_CAP);
            existing_path_rel_types.truncate(existing_path.len().saturating_sub(1));
        }
        return Err(EngineError::RelationshipCycle {
            rel_type: rel_type.to_string(),
            from: from.clone(),
            to: to.clone(),
            existing_path,
            path_truncated: truncated,
            acyclic_set: Some(set.to_vec()),
            existing_path_rel_types: Some(existing_path_rel_types),
        });
    }
    Ok(())
}

/// Validate the per-edge description posture declared on the rel-type
/// in the routing-appropriate definition (intra-mem when source and
/// target share the schema; cross-mem entry when they don't). Emits
/// `MissingRequiredDescription` / `DescriptionNotPermitted` on
/// violations; `optional` and unknown rel-types are no-ops (the
/// vocabulary / shape gates already catch undeclared names — posture
/// only fires for declared names).
///
/// `description` is the normalised value (empty / whitespace-only
/// collapses to `None` before reaching this gate). Called from every
/// add path: `memstead_relate`, `declare_relations` on `memstead_create` and
/// `memstead_update`.
pub(super) fn validate_description_posture(
    engine: &super::Engine,
    rel_type: &str,
    description: Option<&str>,
    source_mem: &str,
    target_mem: &str,
    from_id: &EntityId,
    to_id: &EntityId,
) -> Result<(), EngineError> {
    use memstead_schema::{PerEdgeDescription, SchemaRef};

    let source_schema = engine
        .schemas
        .get(source_mem)
        .expect("schema present for every registered mount");
    let target_schema_ref: Option<SchemaRef> = if source_mem == target_mem {
        None
    } else {
        target_schema_ref_for_routing(engine, target_mem)
    };
    let cross_mem_different = match (&target_schema_ref, source_schema.id()) {
        (Some(target), (src_name, _)) => target.name != src_name,
        (None, _) => false,
    };

    let posture = if cross_mem_different {
        // Look up the matching cross-mem entry's definition. If the
        // entry exists but the rel-type isn't enumerated under it, the
        // vocabulary gate (route_edge_validation) will surface
        // `CROSS_MEM_EDGE_NOT_DECLARED`; posture is a no-op there.
        let target_ref = target_schema_ref
            .as_ref()
            .expect("target_schema_ref is Some when cross_mem_different");
        source_schema
            .cross_mem_entries(&target_ref.name)
            .iter()
            .find_map(|entry| entry.definitions.iter().find(|d| d.name == rel_type))
            .map(|d| d.per_edge_description)
    } else {
        source_schema
            .relationship_def(rel_type)
            .map(|d| d.per_edge_description)
    };

    match posture {
        Some(PerEdgeDescription::Required) if description.is_none() => {
            Err(EngineError::MissingRequiredDescription {
                rel_type: rel_type.to_string(),
                from_id: from_id.to_string(),
                to_id: to_id.to_string(),
            })
        }
        Some(PerEdgeDescription::Forbidden) if description.is_some() => {
            Err(EngineError::DescriptionNotPermitted {
                rel_type: rel_type.to_string(),
                from_id: from_id.to_string(),
                to_id: to_id.to_string(),
            })
        }
        _ => Ok(()),
    }
}

/// Validate the manual-authoring posture declared on the rel-type.
/// Fires only on explicit-author paths (`memstead_relate`, inline
/// `relations:` on `memstead_create`, `declare_relations` on
/// `memstead_update`). The body-link → relation alias machinery
/// synthesises relations from wiki-links — that path bypasses this
/// gate by construction (it never calls this function), keeping the
/// alias path for `manual_authoring: forbidden` rel-types (e.g.
/// REFERENCES) intact.
pub(super) fn validate_manual_authoring_posture(
    engine: &super::Engine,
    rel_type: &str,
    source_mem: &str,
    from_id: &EntityId,
    to_id: &EntityId,
) -> Result<(), EngineError> {
    use memstead_schema::ManualAuthoring;

    let source_schema = engine
        .schemas
        .get(source_mem)
        .expect("schema present for every registered mount");
    let posture = source_schema.relationship_manual_authoring(rel_type);
    if matches!(posture, ManualAuthoring::Forbidden) {
        let guidance = source_schema
            .relationship_when_to_use(rel_type)
            .unwrap_or_default();
        return Err(EngineError::RelationManualAuthoringForbidden {
            rel_type: rel_type.to_string(),
            from_id: from_id.to_string(),
            to_id: to_id.to_string(),
            guidance,
        });
    }
    Ok(())
}

/// Alias-synthesis pass — populates `next.relationships` with engine-
/// emitted relations of the source schema's `alias_target_rel_type`
/// pointer for every body wiki-link not already backed by an
/// in-section-body explicit relation. Runs before the
/// `scan_wikilinks_without_relation` validator; after this pass the
/// validator finds zero missing wiki-links for the pointer rel-type.
///
/// Three cases:
/// 1. Schema has no pointer (`alias_target_rel_type` absent): no-op.
///    Caller's validator continues to refuse unbacked links exactly as
///    today.
/// 2. Schema has a pointer, body wiki-link target is in the same mem
///    OR cross-mem policy admits it: append `Relationship { rel_type:
///    pointer, target, description: None }` to `next.relationships` if
///    no relation of `(pointer, target)` is already present. Dedupe is
///    `(target, rel_type)` — a USES or DEPENDS_ON edge to the same
///    target does not suppress synthesis of the pointer rel-type.
/// 3. Schema has a pointer but a body wiki-link crosses a mem
///    boundary the workspace doesn't grant — or targets an entity
///    absent from a read-only mount: return the funnel's typed
///    refusal ([`EngineError::CrossMemLinkNotAllowed`] /
///    [`EngineError::CrossMemTargetNotFound`], via
///    [`validate_cross_mem_add_policy`]). The entire mutation
///    aborts — no partial state.
///
/// GC: when `prev` is `Some`, the pass also drops pointer-rel-type
/// relations whose target was a body wiki-link in `prev` but no longer
/// appears in `next.sections`. The loader forces `manual_authoring:
/// forbidden` on every schema's `alias_target_rel_type` pointer, so the
/// only path to a pointer-rel-type edge is the body-link channel; the
/// GC rule therefore reduces to "drop pointer-rel-type relations whose
/// target is not in the new body". Targeting prev's wiki-link set
/// specifically (rather than every pointer-rel-type relation) keeps the
/// pass correct even for an explicit-author relation that predates the
/// forbid posture.
///
/// Returns the list of relations the pass emitted (in body iteration
/// order) — `create.rs` / `update.rs` use it to surface
/// `relations_emitted` on the response envelope.
/// Returns the synthesised relations (in body iteration order) and a flag
/// signalling whether a body wiki-link to the entity's own id was dropped
/// (F11). The caller surfaces that as a `SELF_LINK_IGNORED` warning — the
/// pass has no warning channel of its own.
pub(super) fn synthesise_alias_relations(
    engine: &super::Engine,
    prev_body_targets: &std::collections::HashSet<EntityId>,
    next: &mut Entity,
) -> Result<(Vec<crate::entity::Relationship>, bool), super::EngineError> {
    let schema = engine
        .schemas
        .get(next.mem.as_str())
        .expect("schema present for every registered mount");
    let Some(pointer) = schema.alias_target_rel_type().map(str::to_string) else {
        return Ok((Vec::new(), false));
    };

    // 1. GC: drop pointer-rel-type relations whose target was a body
    //    wiki-link in the prev entity state but isn't in next. Targets
    //    not in prev's wiki-link set are explicit-author relations and
    //    are never touched — the rule preserves explicit edges even
    //    while the 5 built-ins still admit explicit REFERENCES.
    //
    //    `extract_inline_links` is strict — non-slug-form targets refuse
    //    here with the typed `InvalidWikiLinkTarget` envelope rather
    //    than silently flowing into the GC's retain set as malformed
    //    EntityIds. Section context comes from the iteration key.
    let mut next_targets: std::collections::HashSet<EntityId> = std::collections::HashSet::new();
    for (section_key, body) in next.sections.iter() {
        let ids = crate::entity::parser::extract_inline_links(body, &next.mem)
            .map_err(|errs| map_wiki_link_errors(section_key, errs))?;
        next_targets.extend(ids);
    }
    next.relationships.retain(|r| {
        !(r.rel_type == pointer
            && prev_body_targets.contains(&r.target)
            && !next_targets.contains(&r.target))
    });

    // 2. Walk body wiki-links in section iteration order and append
    //    one relation per `(target, pointer)` pair not already
    //    present. Cross-mem gate fires on the first refusal.
    let existing: std::collections::HashSet<(String, EntityId)> = next
        .relationships
        .iter()
        .map(|r| (r.rel_type.clone(), r.target.clone()))
        .collect();
    let mut emitted: Vec<crate::entity::Relationship> = Vec::new();
    let mut already_synthesised: std::collections::HashSet<EntityId> =
        std::collections::HashSet::new();
    let mut self_link_ignored = false;
    for (section_key, body) in next.sections.iter() {
        let ids = crate::entity::parser::extract_inline_links(body, &next.mem)
            .map_err(|errs| map_wiki_link_errors(section_key, errs))?;
        for target in ids {
            // F11: a body wiki-link to the entity's own id is a vacuous
            // self-edge (renders as both Outgoing and Incoming, inflates
            // connectivity). Drop it — but don't refuse: the author may
            // have written their own slug. The caller surfaces
            // `SELF_LINK_IGNORED` so the dropped link stays observable.
            if target == next.id {
                self_link_ignored = true;
                continue;
            }
            let key = (pointer.clone(), target.clone());
            if existing.contains(&key) || already_synthesised.contains(&target) {
                continue;
            }
            validate_cross_mem_add_policy(engine, &next.mem, &target)?;
            let rel = crate::entity::Relationship::new(pointer.clone(), target.clone());
            next.relationships.push(rel.clone());
            already_synthesised.insert(target);
            emitted.push(rel);
        }
    }
    Ok((emitted, self_link_ignored))
}

/// Map the first [`crate::entity::id::WikiLinkError`] from a body
/// wiki-link extraction into the typed [`EngineError`] envelope,
/// attaching the offending section's key. Errors after the first are
/// dropped — the agent reads the error, fixes the link, retries, and
/// surfaces the next one on the follow-up call. Keeps the envelope
/// shape stable (single typed payload rather than a list) so MCP /
/// CLI clients don't need a fan-out renderer.
pub(super) fn map_wiki_link_errors(
    section_key: &str,
    errors: Vec<crate::entity::id::WikiLinkError>,
) -> EngineError {
    use crate::entity::id::WikiLinkError;
    let first = errors
        .into_iter()
        .next()
        .expect("map_wiki_link_errors called with non-empty error list");
    match first {
        WikiLinkError::InvalidTarget {
            raw,
            suggested,
            reason,
        } => EngineError::InvalidWikiLinkTarget {
            raw,
            suggested,
            section: section_key.to_string(),
            link_source: "body_link".to_string(),
            reason,
        },
        WikiLinkError::InvalidMemName { raw, reason } => EngineError::InvalidWikiLinkMem {
            raw,
            section: section_key.to_string(),
            reason,
        },
    }
}

/// Compute the set of body wiki-link targets in an entity. Used by
/// callers of `synthesise_alias_relations` to capture the pre-mutation
/// state once, before any borrow conflicts re-enter the engine's
/// schemas / store maps. Uses the lenient decoder — this snapshot
/// must tolerate on-disk drift on pre-strict entities whose bodies
/// may still contain non-conformant links; the strict gate fires
/// only on the post-mutation `next` state.
pub(super) fn collect_body_link_targets(entity: &Entity) -> std::collections::HashSet<EntityId> {
    entity
        .sections
        .iter()
        .flat_map(|(_, body)| {
            crate::entity::parser::extract_inline_links_lenient(body, &entity.mem)
        })
        .collect()
}

/// Alias-existence invariant validator. Given the post-mutation entity
/// state, scan every section body for wiki-links whose target has no
/// corresponding explicit relation in `entity.relationships`. Returns
/// the list of `(section_key, target_id)` pairs that violate the
/// invariant — empty when the post-mutation state is clean.
///
/// Used by [`Engine::create_entity`] and [`Engine::update_entity`]
/// (and `batch_update`). The validator runs unconditionally — under
/// the alias model body wiki-links are foreign-key references on the
/// `## Relationships` table and every reference must be backed.
///
/// Sections from the auto-managed `## Relationships` heading are
/// not scanned (the engine generates them from the relations list
/// at write time; the parser keeps them out of
/// `entity.sections` so they never reach this function).
///
/// Reuses [`crate::entity::parser::extract_inline_links`] so the
/// lexical discipline (fenced-code masking, inline-code masking,
/// alias handling, cross-mem forms) matches every other validator
/// surface in the engine.
pub(super) fn scan_wikilinks_without_relation(
    next: &Entity,
) -> Result<Vec<(String, EntityId)>, EngineError> {
    let explicit_targets: std::collections::HashSet<EntityId> = next
        .relationships
        .iter()
        .map(|r| r.target.clone())
        .collect();
    let mut missing: Vec<(String, EntityId)> = Vec::new();
    for (section_key, body) in next.sections.iter() {
        let ids = crate::entity::parser::extract_inline_links(body, &next.mem)
            .map_err(|errs| map_wiki_link_errors(section_key, errs))?;
        for target in ids {
            // A self-targeting body link is intentionally unbacked: the
            // alias pass drops its (vacuous) self-edge (F11), so it has no
            // backing relation by design and must not trip the
            // unbacked-link refusal here.
            if target == next.id {
                continue;
            }
            if !explicit_targets.contains(&target)
                && !missing
                    .iter()
                    .any(|(k, t)| k == section_key && t == &target)
            {
                missing.push((section_key.clone(), target));
            }
        }
    }
    Ok(missing)
}

#[cfg(test)]
mod tests {

    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::engine::test_helpers::*;
    use crate::engine::{CreateEntityArgs, Engine, UpdateEntityArgs};

    use crate::storage::FilesystemMemWriter;
    use crate::vcs::CommitContext;

    use indexmap::IndexMap;

    #[test]
    fn with_ctx_wrappers_delegate_to_explicit_forms() {
        // Each *_with_ctx wrapper bundles a CommitContext and
        // routes through the corresponding 4-arg method. Verify
        // create → update → rename → delete via the wrappers
        // observably mutate the store the same way the explicit
        // forms would.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let ctx = CommitContext::internal();

        // create_entity_with_ctx
        let create_args = CreateEntityArgs {
            anchors: Vec::new(),
            mem: "specs".to_string(),
            title: "Seed".to_string(),
            entity_type: "spec".to_string(),
            sections: IndexMap::from_iter([
                ("identity".to_string(), "seed identity".to_string()),
                ("purpose".to_string(), "seed purpose".to_string()),
            ]),
            metadata: IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        };
        let created = engine.create_entity_with_ctx(create_args, &ctx).unwrap();
        assert_eq!(created.title, "Seed");
        assert!(engine.store().get(&created.id).is_some());

        // update_entity_with_ctx
        let update_args = UpdateEntityArgs {
            anchors: Vec::new(),
            id: created.id.clone(),
            expected_hash: Some(created.content_hash.clone()),
            sections: IndexMap::from_iter([("identity".to_string(), "updated".to_string())]),
            append_sections: IndexMap::new(),
            patch_sections: IndexMap::new(),
            metadata: IndexMap::new(),
            metadata_unset: Vec::new(),
            dry_run: false,
            declare_relations: Vec::new(),
            relations_unset: Vec::new(),
            anchors_unset: Vec::new(),
        };
        let updated = engine.update_entity_with_ctx(update_args, &ctx).unwrap();
        assert!(
            !updated.commit_sha.is_empty()
                || (updated.modified_sections.replaced.is_empty()
                    && updated.modified_sections.appended.is_empty()
                    && updated.modified_sections.patched.is_empty())
        );

        // rename_entity_with_ctx
        let renamed = engine
            .rename_entity_with_ctx(&created.id, "Renamed", &updated.content_hash, &ctx)
            .unwrap();
        assert_ne!(renamed.old_id, renamed.new_id);
        assert!(engine.store().get(&renamed.new_id).is_some());

        // delete_entity_with_ctx
        let deleted = engine
            .delete_entity_with_ctx(&renamed.new_id, &renamed.content_hash, &ctx)
            .unwrap();
        assert_eq!(deleted.id, renamed.new_id);
        assert!(engine.store().get(&renamed.new_id).is_none());
    }

    /// Minimal on-disk MemConfig pinning `default@1.0.0`, with an
    /// optional pre-set mutation stamp — the carrier the stamp path
    /// and the boot skew check both read.
    fn write_config(dir: &std::path::Path, stamp: Option<memstead_schema::MutationStamp>) {
        let meta = dir.join(memstead_schema::MEM_META_DIR);
        std::fs::create_dir_all(&meta).unwrap();
        let mut config: memstead_schema::MemConfig =
            serde_json::from_str(r#"{"schema": "default@1.0.0"}"#).unwrap();
        config.mutation_stamp = stamp;
        std::fs::write(
            meta.join("config.json"),
            serde_json::to_vec_pretty(&config).unwrap(),
        )
        .unwrap();
    }

    fn stamped_engine_fixture(mem_dir: std::path::PathBuf) -> Engine {
        Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(FilesystemMemWriter::new(mem_dir)) as Box<dyn MemBackend>,
        )])
        .unwrap()
    }

    fn disk_stamp(dir: &std::path::Path) -> Option<memstead_schema::MutationStamp> {
        let bytes =
            std::fs::read(dir.join(memstead_schema::MEM_META_DIR).join("config.json")).unwrap();
        let config: memstead_schema::MemConfig = serde_json::from_slice(&bytes).unwrap();
        config.mutation_stamp
    }

    fn spec_create_args(title: &str) -> CreateEntityArgs {
        CreateEntityArgs {
            anchors: Vec::new(),
            mem: "specs".to_string(),
            title: title.to_string(),
            entity_type: "spec".to_string(),
            sections: IndexMap::from_iter([
                ("identity".to_string(), "seed identity".to_string()),
                ("purpose".to_string(), "seed purpose".to_string()),
            ]),
            metadata: IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        }
    }

    /// Criterion 3 (agent-trust plan 02): a mutation stamps the mem's
    /// engine-owned state with the running engine version and resolved
    /// schema; a read-only load writes nothing.
    #[test]
    fn mutation_writes_version_stamp_and_read_only_load_does_not() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        write_config(&mem_dir, None);

        // Read-only session: boot and drop without mutating — the
        // stamp stays absent.
        drop(stamped_engine_fixture(mem_dir.clone()));
        assert!(
            disk_stamp(&mem_dir).is_none(),
            "a read-only load must not write a stamp"
        );

        // A mutation stamps engine version + resolved schema.
        let mut engine = stamped_engine_fixture(mem_dir.clone());
        engine
            .create_entity_with_ctx(spec_create_args("Seed"), &CommitContext::internal())
            .unwrap();
        let stamp = disk_stamp(&mem_dir).expect("mutation must write the stamp");
        assert_eq!(stamp.engine_version, crate::build_info::full_version());
        assert_eq!(stamp.schema, "default@1.0.0");

        // A second mutation under the same binary leaves the stamp at
        // the same value (the write path compares and no-ops).
        let mut engine = stamped_engine_fixture(mem_dir.clone());
        engine
            .create_entity_with_ctx(spec_create_args("Second"), &CommitContext::internal())
            .unwrap();
        let again = disk_stamp(&mem_dir).expect("stamp survives");
        assert_eq!(again, stamp);
    }

    /// The reported damage, reproduced (04/03, criteria 7 and 8): a
    /// long-lived engine boots, a sibling writes the config out of band, and
    /// the engine's next ENTITY mutation stamps the version. Before the fix
    /// that stamp serialized the boot-time struct and the sibling's write was
    /// gone. No lifecycle call is involved anywhere in this test, which is why
    /// the loss looked spontaneous to the operator who reported it.
    ///
    /// The divergent stamp is written to disk rather than injected, because
    /// the running binary's version is a compile-time constant with no runtime
    /// seam; this is how the existing skew coverage reaches the condition too.
    #[test]
    fn a_sibling_config_write_survives_the_next_entity_mutation() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        // Seed a stamp that disagrees with this binary, so the stamp writer is
        // live rather than dormant: that is the two-binary topology the report
        // came from.
        write_config(
            &mem_dir,
            Some(memstead_schema::MutationStamp {
                engine_version: "0.0.1-other".to_string(),
                schema: "default@1.0.0".to_string(),
            }),
        );

        // The long-lived engine boots and caches the config as it is now.
        let mut engine = stamped_engine_fixture(mem_dir.clone());

        // A sibling process sets a description. The engine never learns:
        // a config-only write advances no entity head and appends no change
        // log line, so the staleness probe cannot see it.
        let path = mem_dir
            .join(memstead_schema::MEM_META_DIR)
            .join("config.json");
        let mut sibling: memstead_schema::MemConfig =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        sibling.description = Some("written by the sibling".to_string());
        std::fs::write(&path, serde_json::to_vec_pretty(&sibling).unwrap()).unwrap();

        // An ordinary entity write. Nothing about it mentions config.
        engine
            .create_entity_with_ctx(spec_create_args("Seed"), &CommitContext::internal())
            .unwrap();

        let after: memstead_schema::MemConfig =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            after.description.as_deref(),
            Some("written by the sibling"),
            "the sibling's description must survive an entity mutation"
        );
        assert_eq!(
            after.mutation_stamp.map(|s| s.engine_version),
            Some(crate::build_info::full_version().to_string()),
            "and the stamp this engine came to write must still land"
        );
    }

    /// Criterion 3 for the stamp writer: the intervention reaches the ENTITY
    /// mutation's own response. The stamp has no response of its own, and an
    /// earlier draft discarded the report with `let _`, so an operator whose
    /// config moved during an innocuous entity write was told nothing.
    #[test]
    fn the_stamps_intervention_rides_the_entity_mutations_response() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        write_config(
            &mem_dir,
            Some(memstead_schema::MutationStamp {
                engine_version: "0.0.1-other".to_string(),
                schema: "default@1.0.0".to_string(),
            }),
        );
        let mut engine = stamped_engine_fixture(mem_dir.clone());

        let path = mem_dir
            .join(memstead_schema::MEM_META_DIR)
            .join("config.json");
        let mut sibling: memstead_schema::MemConfig =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        sibling.description = Some("theirs".to_string());
        std::fs::write(&path, serde_json::to_vec_pretty(&sibling).unwrap()).unwrap();

        let outcome = engine
            .create_entity_with_ctx(spec_create_args("Seed"), &CommitContext::internal())
            .unwrap();
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.code() == "CONFIG_WRITE_INTERVENED"),
            "the entity mutation must report the config intervention: {:?}",
            outcome.warnings
        );
    }

    /// Criterion 5: the folder backend's config write is a compare-and-set,
    /// not check-then-write. A write whose `expected` no longer matches the
    /// file must refuse rather than overwrite.
    #[test]
    fn the_folder_config_write_refuses_a_stale_expectation() {
        use crate::backend::MemBackend;
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        write_config(&mem_dir, None);
        let backend = FilesystemMemWriter::new(mem_dir.clone());
        let observed = backend.read_mem_config().unwrap().expect("config exists");

        // Someone else writes.
        let path = mem_dir
            .join(memstead_schema::MEM_META_DIR)
            .join("config.json");
        std::fs::write(&path, br#"{"schema": "default@1.0.0", "title": "theirs"}"#).unwrap();

        // A write against the stale expectation is refused, not applied.
        let wrote = backend
            .write_mem_config_cas(Some(&observed), b"{\"schema\": \"default@1.0.0\"}", None)
            .unwrap();
        assert!(!wrote, "a stale expectation must not overwrite");
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            on_disk.contains("theirs"),
            "their write survived: {on_disk}"
        );

        // And against the current bytes it lands.
        let current = backend.read_mem_config().unwrap().unwrap();
        assert!(
            backend
                .write_mem_config_cas(Some(&current), b"{\"schema\": \"default@1.0.0\"}", None)
                .unwrap(),
            "a current expectation writes"
        );
    }

    /// Criterion 7's complement: the stamp does not become a busy writer. With
    /// a stamp that already agrees, an entity mutation must not touch the
    /// config at all, so a sibling's write is untouched for the boring reason
    /// rather than the interesting one.
    #[test]
    fn a_matching_stamp_still_writes_no_config_at_all() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        write_config(
            &mem_dir,
            Some(memstead_schema::MutationStamp {
                engine_version: crate::build_info::full_version().to_string(),
                schema: "default@1.0.0".to_string(),
            }),
        );
        let mut engine = stamped_engine_fixture(mem_dir.clone());
        let path = mem_dir
            .join(memstead_schema::MEM_META_DIR)
            .join("config.json");
        let before = std::fs::read(&path).unwrap();
        engine
            .create_entity_with_ctx(spec_create_args("Seed"), &CommitContext::internal())
            .unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "a mutation whose stamp already matches must write no config"
        );
    }

    /// 04/04, criteria 9 and 10: skew reaches the write that meets it, before
    /// that write's own restamp erases the evidence, and the write still
    /// lands.
    ///
    /// Boot-only detection meant a long-lived server started under one binary
    /// and written to by another never said so, because the first mutation
    /// both revealed and hid the fact.
    #[test]
    fn skew_is_reported_at_the_write_and_the_write_still_lands() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        write_config(
            &mem_dir,
            Some(memstead_schema::MutationStamp {
                engine_version: "0.0.1".to_string(),
                schema: "default@1.0.0".to_string(),
            }),
        );
        let mut engine = stamped_engine_fixture(mem_dir.clone());
        let outcome = engine
            .create_entity_with_ctx(spec_create_args("Seed"), &CommitContext::internal())
            .unwrap();

        let skew: Vec<_> = outcome
            .warnings
            .iter()
            .filter(|w| w.code() == "ENGINE_VERSION_SKEW")
            .collect();
        assert_eq!(
            skew.len(),
            1,
            "the write that meets the skew must report it: {:?}",
            outcome.warnings
        );
        assert!(
            matches!(
                skew[0],
                crate::ops::WarningHint::EngineVersionSkew {
                    direction: crate::build_info::SkewDirection::StampedOlder,
                    ..
                }
            ),
            "and say which way: {:?}",
            skew[0]
        );
        // Criterion 10: it landed. An older engine is not prevented from
        // writing; a deliberate downgrade is the operator's business.
        assert!(engine.get_entity(&outcome.id).is_some());
        assert_eq!(
            disk_stamp(&mem_dir).map(|s| s.engine_version),
            Some(crate::build_info::full_version().to_string()),
            "and the restamp still happened"
        );

        // Second write, same binary: nothing left to report.
        let again = engine
            .create_entity_with_ctx(spec_create_args("Second"), &CommitContext::internal())
            .unwrap();
        assert!(
            !again
                .warnings
                .iter()
                .any(|w| w.code() == "ENGINE_VERSION_SKEW"),
            "the skew is resolved once restamped: {:?}",
            again.warnings
        );
    }

    /// Criterion 8's complement at the write tier: a stamp from the same
    /// release with a different build hash is not skew, so a workspace whose
    /// binary is rebuilt from source is not told its engine disagrees on
    /// every mutation.
    #[test]
    fn a_rebuild_of_the_same_release_is_not_skew_at_the_write() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        write_config(
            &mem_dir,
            Some(memstead_schema::MutationStamp {
                engine_version: format!("{}+gdeadbee", crate::ENGINE_VERSION),
                schema: "default@1.0.0".to_string(),
            }),
        );
        let mut engine = stamped_engine_fixture(mem_dir.clone());
        let outcome = engine
            .create_entity_with_ctx(spec_create_args("Seed"), &CommitContext::internal())
            .unwrap();
        assert!(
            !outcome
                .warnings
                .iter()
                .any(|w| w.code() == "ENGINE_VERSION_SKEW"),
            "a differing build hash on the same version is not skew: {:?}",
            outcome.warnings
        );
    }

    /// Criterion 3/4 (agent-trust plan 02): boot under a different
    /// binary version surfaces the warn-tier `ENGINE_VERSION_SKEW`
    /// naming both versions, on load warnings AND in `health()`;
    /// a stamp-less mem and a matching stamp are silent.
    #[test]
    fn boot_skew_warning_fires_only_on_disagreeing_stamp() {
        use crate::ops::WarningHint;

        // Disagreeing stamp → warning on boot and in health.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        write_config(
            &mem_dir,
            Some(memstead_schema::MutationStamp {
                engine_version: "0.0.1".to_string(),
                schema: "default@1.0.0".to_string(),
            }),
        );
        let engine = stamped_engine_fixture(mem_dir);
        let skew: Vec<_> = engine
            .load_warnings()
            .iter()
            .filter(|w| matches!(w, WarningHint::EngineVersionSkew { .. }))
            .collect();
        assert_eq!(skew.len(), 1, "one skewed mem, one warning: {skew:?}");
        if let WarningHint::EngineVersionSkew {
            mem,
            stamped_engine,
            running_engine,
            stamped_schema,
            direction,
        } = skew[0]
        {
            assert_eq!(mem, "specs");
            assert_eq!(stamped_engine, "0.0.1");
            assert_eq!(running_engine, crate::build_info::full_version());
            assert_eq!(stamped_schema, "default@1.0.0");
            // 0.0.1 against any shipped version: the mem is behind us.
            assert_eq!(*direction, crate::build_info::SkewDirection::StampedOlder);
        }
        let health = engine.health();
        assert!(
            health
                .warnings
                .iter()
                .any(|w| w.code() == "ENGINE_VERSION_SKEW"),
            "health() must surface the skew without an include gate: {:?}",
            health.warnings,
        );

        // Matching stamp → silent.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        write_config(
            &mem_dir,
            Some(memstead_schema::MutationStamp {
                engine_version: crate::build_info::full_version().to_string(),
                schema: "default@1.0.0".to_string(),
            }),
        );
        let engine = stamped_engine_fixture(mem_dir);
        assert!(
            !engine
                .load_warnings()
                .iter()
                .any(|w| matches!(w, WarningHint::EngineVersionSkew { .. })),
            "a matching stamp is not skew"
        );

        // No stamp → silent (absence of a stamp is not skew).
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        write_config(&mem_dir, None);
        let engine = stamped_engine_fixture(mem_dir);
        assert!(
            !engine
                .load_warnings()
                .iter()
                .any(|w| matches!(w, WarningHint::EngineVersionSkew { .. })),
            "a stamp-less (pre-plan) mem boots without warning noise"
        );
    }
}
