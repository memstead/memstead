//! The check operation and derived check state.
//!
//! `record_check` is the engine-recorded act of verification: it
//! appends one [`crate::check::CheckRecord`] — verdict, method note,
//! the entity's `content_hash` at check time, and plan-13 provenance
//! (actor, client, declared role) — to the workspace check ledger.
//! Checking mutates nothing: no entity write, no mem commit, no
//! `content_hash` change. That non-mutation is load-bearing — it is
//! what makes check-staleness derivable by hash comparison.
//!
//! `entity_check_state` derives never-checked | checked-ok |
//! check-failed | check-stale from the newest record against the
//! entity's current hash; the derivation lives in
//! [`crate::check::derive_state`] so surfaces and health share one
//! implementation.

use crate::check::{CheckKind, CheckLedger, CheckRecord, CheckState, Verdict, derive_state};
use crate::vcs::{Actor, ClientId};

use super::{Engine, error::EngineError};

impl Engine {
    /// Record a check of one entity. Refuses typed on unknown mem
    /// (quarantine included), unknown entity, read-only mounts, and
    /// on any persistence failure (`CHECK_NOT_RECORDED`) — recording
    /// is never best-effort, because a caller who believes an
    /// unrecorded check landed is the exact dishonesty this tier
    /// removes. The declared role rides engine session state
    /// ([`Engine::set_role`]), same as every mutation.
    ///
    /// `kind` selects the closed check-kind vocabulary. A
    /// `conformance` record is bound to the mem's schema pin, stamped
    /// HERE from the mount — never caller-supplied, so a verdict
    /// cannot claim a prose version the caller never read; a mem with
    /// no pin refuses (`INVALID_INPUT`), because a semantic judgment
    /// against no schema binds to nothing.
    #[allow(clippy::too_many_arguments)] // the record's own fields, no natural grouping
    pub fn record_check(
        &mut self,
        mem_name: &str,
        entity_id: &str,
        verdict: Verdict,
        kind: CheckKind,
        method: Option<&str>,
        actor: Actor,
        client: Option<&ClientId>,
    ) -> Result<CheckRecord, EngineError> {
        self.record_check_with(
            mem_name,
            entity_id,
            verdict,
            &crate::check::RecordKind::Engine(kind),
            method,
            None,
            actor,
            client,
        )
    }

    /// [`Self::record_check`] with the full record shape: a kind that may
    /// be a caller-declared foreign `x-<name>` kind (recorded verbatim,
    /// never interpreted — it stamps no schema pin and moves no state),
    /// and an optional structured finding (validated before anything is
    /// appended; a malformed one refuses `INVALID_CHECK_FINDING`).
    #[allow(clippy::too_many_arguments)]
    pub fn record_check_with(
        &mut self,
        mem_name: &str,
        entity_id: &str,
        verdict: Verdict,
        kind: &crate::check::RecordKind,
        method: Option<&str>,
        finding: Option<crate::check::CheckFinding>,
        actor: Actor,
        client: Option<&ClientId>,
    ) -> Result<CheckRecord, EngineError> {
        if let Some(f) = &finding {
            f.validate()
                .map_err(|reason| EngineError::InvalidCheckFinding { reason })?;
            // The code namespace: `UPPER_SNAKE` claims an engine health
            // condition (the acknowledgement `health --strict` honours),
            // so it must name one; a checker's own vocabulary keeps any
            // other spelling. An acknowledgement (a `failed` verdict on a
            // condition) names its owner and the closing plan in `method`.
            if crate::ops::strict::is_engine_code_shape(&f.code)
                && !crate::ops::strict::is_health_condition(&f.code)
            {
                return Err(EngineError::InvalidCheckFinding {
                    reason: format!(
                        "finding.code `{}` is spelled in the engine's UPPER_SNAKE namespace but \
                         names no health condition — an acknowledgement names one of: {}; a \
                         checker's own code takes another spelling (`hidden-premise`)",
                        f.code,
                        crate::ops::strict::HEALTH_CONDITIONS.join(", ")
                    ),
                });
            }
            if crate::ops::strict::is_health_condition(&f.code)
                && verdict == Verdict::Failed
                && method.is_none_or(|m| m.trim().is_empty())
            {
                return Err(EngineError::InvalidCheckFinding {
                    reason: format!(
                        "an acknowledgement of `{}` names its owner and the plan that closes it \
                         in `method`, and none was given",
                        f.code
                    ),
                });
            }
        }
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }
        let schema_ref = match kind.engine_kind() {
            None | Some(CheckKind::Verification) => None,
            Some(CheckKind::Conformance) => Some(
                self.mounts[mount_idx]
                    .mount
                    .schema
                    .as_ref()
                    .map(|s| s.as_display())
                    .ok_or_else(|| {
                        EngineError::InvalidInput(format!(
                            "a conformance check binds to the mem's schema pin, and mem \
                             `{mem_name}` declares none"
                        ))
                    })?,
            ),
        };
        let entity_hash = self
            .store
            .all_entities()
            .find(|e| e.mem == mem_name && e.id.0 == entity_id)
            .map(|e| e.content_hash.clone())
            .ok_or_else(|| EngineError::NotFound {
                id: entity_id.to_string(),
            })?;
        let Some(root) = self.workspace_root() else {
            return Err(EngineError::CheckNotRecorded {
                reason: "engine has no workspace root — no durable check store".to_string(),
            });
        };
        let ledger = CheckLedger::for_workspace(root);
        let record = CheckRecord {
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            entity: entity_id.to_string(),
            verdict: verdict.as_str().to_string(),
            method: method
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_string),
            entity_hash,
            actor: actor.as_trailer().to_string(),
            client: client.map(|c| format!("{}@{}", c.name, c.version)),
            role: self
                .current_role()
                .as_trailer()
                .unwrap_or("unspecified")
                .to_string(),
            // The caller-declared identity rides engine session state
            // ([`Engine::set_identity`]), same as the role — absence
            // records as absence (plan 15).
            identity: self.current_identity().map(str::to_string),
            // Verification records omit the kind entirely, so a
            // kind-omitted caller's ledger lines stay byte-identical
            // to the pre-kind shape.
            kind: match kind {
                crate::check::RecordKind::Engine(CheckKind::Verification) => None,
                other => Some(other.as_wire().to_string()),
            },
            schema_ref,
            finding,
            renamed_from: None,
            carried_from: None,
        };
        ledger
            .record(&record)
            .map_err(|e| EngineError::CheckNotRecorded {
                reason: format!("ledger append failed: {e}"),
            })?;
        Ok(record)
    }

    /// Carry the check ledger across a rename: every line recorded under
    /// `old` is appended again under `new`, marked `renamed_from`, so the
    /// renamed entity keeps its check history instead of reading
    /// `never_checked` while its record sits unreachable under an id
    /// nothing resolves any more. The carried lines keep their hash, so
    /// they derive `check_stale` against the renamed file — a rename is
    /// a content change (title, self-links), and the verdict is not
    /// re-asserted on content the checker never saw. An engine with no
    /// workspace root has no ledger and carries nothing. A failed
    /// append refuses, as recording does: the caller runs this before
    /// the rename commits.
    pub(crate) fn carry_checks_across_rename(
        &self,
        old: &crate::entity::EntityId,
        new: &crate::entity::EntityId,
    ) -> Result<usize, EngineError> {
        let Some(root) = self.workspace_root() else {
            return Ok(0);
        };
        CheckLedger::for_workspace(root)
            .carry_rename(old.as_ref(), new.as_ref())
            .map_err(|e| EngineError::CheckNotRecorded {
                reason: format!("ledger carry across rename {old} → {new} failed: {e}"),
            })
    }

    /// The `transition_requires_self_check` kinds a prepared write passes
    /// on the strength of a fresh, independent record: every such
    /// constraint of `type_def` whose gated value `next` holds and whose
    /// declared kind the provider confirms against `next.content_hash`
    /// (the PRE-write hash: the caller hands the composed entity before
    /// the store or disk moved). These are the records the write is
    /// about to stale; [`Self::carry_checks_across_transition`] carries
    /// them. Empty when the type declares no such gate or none passes.
    pub(crate) fn licensed_self_check_kinds(
        &self,
        next: &crate::entity::Entity,
        type_def: &memstead_schema::TypeDefinition,
    ) -> Vec<String> {
        let provider = self.check_standing_provider();
        let mut kinds: Vec<String> = Vec::new();
        for c in &type_def.constraints {
            let memstead_schema::ConstraintDef::TransitionRequiresSelfCheck {
                field,
                to_value,
                check_kind,
                ..
            } = c
            else {
                continue;
            };
            let triggered = next
                .metadata
                .get(field.as_str())
                .is_some_and(|v| v.to_frontmatter_string() == *to_value);
            if triggered && !kinds.contains(check_kind) && provider(next, check_kind).confirms() {
                kinds.push(check_kind.clone());
            }
        }
        kinds
    }

    /// Carry the records that licensed a gated transition across the
    /// write they licensed: for each wire kind in `kinds`, the newest
    /// record of `id` that is fresh at `pre_hash` and confirming is
    /// appended again keyed to `post_hash`, marked
    /// `carried_from: {hash, reason: "transition"}`
    /// ([`CheckLedger::carry_transition`]). Without this the write that
    /// a `transition_requires_self_check` gate admitted moves the
    /// entity's hash and stales the very record that admitted it, so the
    /// constraint reads unsatisfied the moment after it was satisfied.
    /// The carried line keeps the checker's identity: the writer is not
    /// the checker, and independence must go on reading it that way. A
    /// stale or failed record is not carried, it licensed nothing. An
    /// engine with no workspace root has no ledger and carries nothing.
    /// A failed append refuses, as recording does: the caller runs this
    /// before the write is staged. Returns the number of lines carried.
    pub(crate) fn carry_checks_across_transition(
        &self,
        id: &crate::entity::EntityId,
        kinds: &[String],
        pre_hash: &str,
        post_hash: &str,
    ) -> Result<usize, EngineError> {
        if kinds.is_empty() || pre_hash == post_hash {
            return Ok(0);
        }
        let Some(root) = self.workspace_root() else {
            return Ok(0);
        };
        let ledger = CheckLedger::for_workspace(root);
        let mut carried = 0;
        for kind in kinds {
            let moved = ledger
                .carry_transition(id.as_ref(), kind, pre_hash, post_hash)
                .map_err(|e| EngineError::CheckNotRecorded {
                    reason: format!(
                        "ledger carry of the `{kind}` record across the transition of {id} failed: {e}"
                    ),
                })?;
            carried += usize::from(moved);
        }
        Ok(carried)
    }

    /// The newest check record of one entity under one wire kind, from
    /// the one source the mount has: the sealed member for an archive
    /// mount (a workspace ledger beside it is never consulted, and an
    /// archive sealed without records has none), the workspace ledger
    /// for every other mount (a writable mem never reads a sealed
    /// member; an engine with no workspace root has no ledger). Every
    /// check-state read goes through here, so the derivation is the same
    /// whatever the mount.
    pub fn latest_check_record(
        &self,
        mem_name: &str,
        entity_id: &str,
        kind: &str,
    ) -> Option<CheckRecord> {
        if self.is_archive_mount(mem_name) {
            let path = crate::EntityId(entity_id.to_string());
            return self
                .archive_checks_for(mem_name)
                .and_then(|sealed| sealed.latest(path.path(), kind))
                .map(|sc| sc.to_record(entity_id, kind));
        }
        self.workspace_root()
            .map(CheckLedger::for_workspace)
            .and_then(|l| l.latest_for_wire_kind(entity_id, kind))
    }

    /// The newest record per foreign `x-<name>` kind of one entity, in
    /// kind order, from the same source as [`Self::latest_check_record`].
    /// Recorded, listed, never aggregated into a state.
    pub fn latest_foreign_checks(&self, mem_name: &str, entity_id: &str) -> Vec<CheckRecord> {
        if self.is_archive_mount(mem_name) {
            let path = crate::EntityId(entity_id.to_string());
            return self
                .archive_checks_for(mem_name)
                .map(|sealed| {
                    sealed
                        .kinds_of(path.path())
                        .filter(|(k, _)| k.starts_with(crate::check::FOREIGN_KIND_PREFIX))
                        .map(|(k, sc)| sc.to_record(entity_id, k))
                        .collect()
                })
                .unwrap_or_default();
        }
        let Some(ledger) = self.workspace_root().map(CheckLedger::for_workspace) else {
            return Vec::new();
        };
        let mut latest: std::collections::BTreeMap<String, CheckRecord> =
            std::collections::BTreeMap::new();
        for rec in ledger.all() {
            if rec.entity != entity_id {
                continue;
            }
            if let Some(k) = rec.foreign_kind() {
                latest.insert(k.to_string(), rec.clone());
            }
        }
        latest.into_values().collect()
    }

    /// Derive one entity's check state and newest check record.
    /// Refuses typed on unknown mem/entity; an engine with no
    /// workspace root has no check store and honestly derives
    /// `never_checked` (no recorded checks exist); an archive mount
    /// derives from its sealed member the same way.
    pub fn entity_check_state(
        &self,
        mem_name: &str,
        entity_id: &str,
    ) -> Result<(CheckState, Option<CheckRecord>), EngineError> {
        self.find_mount(mem_name)?;
        let current_hash = self
            .store
            .all_entities()
            .find(|e| e.mem == mem_name && e.id.0 == entity_id)
            .map(|e| e.content_hash.clone())
            .ok_or_else(|| EngineError::NotFound {
                id: entity_id.to_string(),
            })?;
        let latest =
            self.latest_check_record(mem_name, entity_id, CheckKind::Verification.as_str());
        Ok((derive_state(latest.as_ref(), &current_hash), latest))
    }

    /// Derive one entity's `conformance` state and newest conformance
    /// record: hash staleness plus pin staleness (a re-pinned or
    /// unpinned mem stales the verdict — the prose it judged against
    /// is no longer the prose in force). Same refusals as
    /// [`Self::entity_check_state`].
    pub fn entity_conformance_state(
        &self,
        mem_name: &str,
        entity_id: &str,
    ) -> Result<(CheckState, Option<CheckRecord>), EngineError> {
        let mount = self.find_mount(mem_name)?;
        let current_pin = mount.mount.schema.as_ref().map(|s| s.as_display());
        let current_hash = self
            .store
            .all_entities()
            .find(|e| e.mem == mem_name && e.id.0 == entity_id)
            .map(|e| e.content_hash.clone())
            .ok_or_else(|| EngineError::NotFound {
                id: entity_id.to_string(),
            })?;
        let latest = self.latest_check_record(mem_name, entity_id, CheckKind::Conformance.as_str());
        Ok((
            crate::check::derive_state_pinned(
                latest.as_ref(),
                &current_hash,
                current_pin.as_deref(),
            ),
            latest,
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::check::{CheckKind, CheckLedger, CheckRecord, CheckState, Verdict};
    use crate::vcs::Actor;
    use crate::workspace::MountCapability;

    /// A conformance check binds to the mem's schema pin; a mem that
    /// declares none cannot accept one. Today that refusal arrives as
    /// the quarantine gate (an unpinned mem quarantines at boot and
    /// serves nothing), which fires before the pin guard inside
    /// `record_check`; the guard's own `INVALID_INPUT` remains as
    /// defense in depth for any future backend that serves unpinned.
    #[test]
    fn conformance_refuses_without_a_schema_pin() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("anything.md"),
            "---\nid: anything\ntitle: Anything\ntype: note\n---\n\nBody.\n",
        )
        .unwrap();
        let mut mount = crate::engine::test_helpers::folder_mount("m", tmp.path().to_path_buf());
        mount.schema = None;
        let mut engine = crate::Engine::from_mounts(vec![(
            mount,
            Box::new(crate::storage::FilesystemBackend::new(
                tmp.path().to_path_buf(),
            )) as Box<dyn crate::backend::MemBackend>,
        )])
        .unwrap();
        let err = engine
            .record_check(
                "m",
                "m--anything",
                Verdict::Ok,
                CheckKind::Conformance,
                None,
                Actor::Cli,
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "MEM_QUARANTINED");
    }

    // --- the licensing record carries across the transition it licensed ---

    /// A workspace-rooted folder mem `bundles` whose `plan` type gates
    /// `status: complete` on a fresh independent record of `check_kind`
    /// on the plan itself (`transition_requires_self_check`).
    fn self_check_gated_engine(tmp: &tempfile::TempDir, check_kind: &str) -> crate::Engine {
        let schemas_dir = tmp.path().join("schemas");
        let pkg = schemas_dir.join("selfgate");
        std::fs::create_dir_all(pkg.join("types")).unwrap();
        std::fs::write(
            pkg.join("schema.yaml"),
            r#"name: selfgate
version: 0.1.0
description: self-check gate fixture
when_to_use: tests
types:
  - plan
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#,
        )
        .unwrap();
        std::fs::write(
            pkg.join("types").join("plan.yaml"),
            format!(
                r#"name: plan
description: p
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: status
    description: s
    field_type: string
    default_value: draft
    enum_values: [draft, complete]
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
updatable_fields:
  - title
  - body
  - status
health_required_fields: []
staleness_threshold_days: 90
constraints:
  - kind: transition_requires_self_check
    field: status
    to_value: complete
    check_kind: {check_kind}
    severity: block
write_rules: []
"#
            ),
        )
        .unwrap();
        let mem_dir = tmp.path().join("bundles");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let writer = crate::storage::FilesystemBackend::new(mem_dir.clone());
        let mount = crate::workspace::Mount {
            mem: "bundles".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "selfgate",
                semver::Version::new(0, 1, 0),
            )),
            storage: crate::workspace::MountStorage::Folder { path: mem_dir },
            capability: crate::workspace::MountCapability::Write,
            lifecycle: crate::workspace::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let mut engine = crate::Engine::from_mounts_with_schemas_dir(
            vec![(
                mount,
                Box::new(writer) as Box<dyn crate::backend::MemBackend>,
            )],
            Some(&schemas_dir),
        )
        .unwrap();
        engine.set_workspace_root(tmp.path().to_path_buf());
        engine
    }

    fn create_plan(engine: &mut crate::Engine, title: &str) -> crate::entity::EntityId {
        let (actor, client) = crate::engine::test_helpers::cli_actor();
        let mut sections = indexmap::IndexMap::new();
        sections.insert("body".to_string(), "the plan body.".to_string());
        engine
            .create_entity(
                crate::engine::CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "bundles".to_string(),
                    title: title.to_string(),
                    entity_type: "plan".to_string(),
                    sections,
                    metadata: indexmap::IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap()
            .id
    }

    fn set_status(
        engine: &mut crate::Engine,
        id: &crate::entity::EntityId,
        value: &str,
    ) -> Result<crate::engine::UpdateEntityOutcome, crate::engine::error::EngineError> {
        let (actor, client) = crate::engine::test_helpers::cli_actor();
        let current = engine.get_entity(id).unwrap().content_hash.clone();
        let mut metadata = indexmap::IndexMap::new();
        metadata.insert("status".to_string(), value.to_string());
        engine.update_entity(
            crate::engine::UpdateEntityArgs {
                anchors: Vec::new(),
                id: id.clone(),
                expected_hash: Some(current),
                sections: indexmap::IndexMap::new(),
                append_sections: indexmap::IndexMap::new(),
                patch_sections: indexmap::IndexMap::new(),
                sections_unset: Vec::new(),
                metadata,
                metadata_unset: Vec::new(),
                declare_relations: vec![],
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
    }

    fn append_body(engine: &mut crate::Engine, id: &crate::entity::EntityId) {
        let (actor, client) = crate::engine::test_helpers::cli_actor();
        let current = engine.get_entity(id).unwrap().content_hash.clone();
        let mut append = indexmap::IndexMap::new();
        append.insert("body".to_string(), "more.".to_string());
        engine
            .update_entity(
                crate::engine::UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: id.clone(),
                    expected_hash: Some(current),
                    sections: indexmap::IndexMap::new(),
                    append_sections: append,
                    patch_sections: indexmap::IndexMap::new(),
                    sections_unset: Vec::new(),
                    metadata: indexmap::IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: vec![],
                    dry_run: false,
                    relations_unset: Vec::new(),
                    anchors_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
    }

    fn check_plan(
        engine: &mut crate::Engine,
        identity: &str,
        id: &crate::entity::EntityId,
        kind: &str,
    ) {
        let (actor, client) = crate::engine::test_helpers::cli_actor();
        engine.set_identity(Some(identity.to_string()));
        engine
            .record_check_with(
                "bundles",
                id.as_ref(),
                Verdict::Ok,
                &crate::check::RecordKind::from_wire(kind).unwrap(),
                Some("projection walked"),
                None,
                actor,
                Some(&client),
            )
            .unwrap();
    }

    /// The defect: a bundle carries a fresh `x-projection` ok record under
    /// an independent identity, the author's `status: complete` passes
    /// the gate, and the write moves the hash; before the carry, the
    /// constraints axis reported the gate unsatisfied (`check_stale`) the
    /// moment after it admitted the write. Now the licensing record is
    /// carried to the post-write hash, marked, still the checker's; the
    /// constraints axis stays clean and the gate keeps holding across a
    /// later write to the complete plan.
    #[test]
    fn licensing_self_check_carries_across_the_transition_it_admitted() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = self_check_gated_engine(&tmp, "x-projection");
        engine.set_identity(Some("author-a".to_string()));
        let id = create_plan(&mut engine, "The Bundle");
        let hash_at_check = engine.get_entity(&id).unwrap().content_hash.clone();
        check_plan(&mut engine, "checker-c", &id, "x-projection");

        engine.set_identity(Some("author-a".to_string()));
        let outcome = set_status(&mut engine, &id, "complete").expect("the gate admits the write");
        assert_ne!(
            outcome.content_hash, hash_at_check,
            "the write moved the hash"
        );

        // The constraints axis reads no violation after the write.
        let findings = engine.constraint_findings(Some("bundles"));
        assert!(findings.is_empty(), "{findings:?}");

        // The carried line: post-write hash, marked, the checker's identity.
        let foreign = engine.latest_foreign_checks("bundles", id.as_ref());
        assert_eq!(foreign.len(), 1);
        let carried = &foreign[0];
        assert_eq!(carried.entity_hash, outcome.content_hash);
        assert_eq!(carried.identity.as_deref(), Some("checker-c"));
        assert_eq!(carried.method.as_deref(), Some("projection walked"));
        assert_eq!(
            carried.carried_from,
            Some(crate::check::CarriedFrom::transition(&hash_at_check))
        );
        // The gate's own reading of the carried record: fresh and independent.
        let entity = engine.get_entity(&id).unwrap().clone();
        let standing = (engine.check_standing_provider())(&entity, "x-projection");
        assert!(standing.confirms(), "{standing:?}");
        // Append-only: the checker's original line stays, unmarked.
        let ledger = CheckLedger::for_workspace(tmp.path());
        let lines: Vec<CheckRecord> = ledger
            .all()
            .into_iter()
            .filter(|r| r.entity == id.as_ref())
            .collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].carried_from.is_none());
        assert_eq!(lines[0].entity_hash, hash_at_check);

        // A later write to the complete plan passes on the carried record
        // and carries it onward, so the gate keeps holding.
        engine.set_identity(Some("author-a".to_string()));
        append_body(&mut engine, &id);
        assert!(engine.constraint_findings(Some("bundles")).is_empty());
        let onward = &engine.latest_foreign_checks("bundles", id.as_ref())[0];
        assert_eq!(
            onward.entity_hash,
            engine.get_entity(&id).unwrap().content_hash
        );
        assert_eq!(
            onward.carried_from.as_ref().unwrap().hash,
            outcome.content_hash
        );
        // A write that leaves the gate unentered carries nothing: back to
        // draft, then an append, and the record is not moved again.
        set_status(&mut engine, &id, "draft").unwrap();
        let before = ledger.all().len();
        append_body(&mut engine, &id);
        assert_eq!(
            ledger.all().len(),
            before,
            "no gate passed, nothing carried"
        );
    }

    /// The `verification` kind through the same gate: after the write the
    /// entity's check state reads `checked_ok` and the health checks axis
    /// lists it `confirmed_independent`: the carried record stays the
    /// checker's, never the writer's.
    #[test]
    fn carried_verification_record_reads_checked_ok_and_independent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = self_check_gated_engine(&tmp, "verification");
        engine.set_identity(Some("author-a".to_string()));
        let id = create_plan(&mut engine, "The Bundle");
        check_plan(&mut engine, "checker-c", &id, "verification");
        engine.set_identity(Some("author-a".to_string()));
        set_status(&mut engine, &id, "complete").expect("the gate admits the write");

        let (state, latest) = engine.entity_check_state("bundles", id.as_ref()).unwrap();
        assert_eq!(state, CheckState::CheckedOk);
        let latest = latest.unwrap();
        assert_eq!(latest.identity.as_deref(), Some("checker-c"));
        assert!(latest.carried_from.is_some());
        let axis = crate::ops::health::health_checks_axis(&engine, Some("bundles"));
        let independence = &axis["bundles"]["independence"];
        assert_eq!(
            independence["confirmed_independent"]["items"],
            serde_json::json!([id.as_ref()]),
            "{axis}"
        );
        assert_eq!(
            axis["bundles"]["checked_ok"],
            serde_json::json!(1),
            "{axis}"
        );
        assert!(engine.constraint_findings(Some("bundles")).is_empty());
    }

    /// A record already stale before the write licenses nothing: the
    /// write refuses as before, and the ledger gains no carried line.
    #[test]
    fn stale_self_check_is_not_carried_and_the_write_refuses() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = self_check_gated_engine(&tmp, "x-projection");
        engine.set_identity(Some("author-a".to_string()));
        let id = create_plan(&mut engine, "The Bundle");
        check_plan(&mut engine, "checker-c", &id, "x-projection");
        // The author edits after the check: the record goes stale.
        engine.set_identity(Some("author-a".to_string()));
        append_body(&mut engine, &id);
        let before = CheckLedger::for_workspace(tmp.path()).all().len();
        let err = set_status(&mut engine, &id, "complete").unwrap_err();
        assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED");
        assert!(err.to_string().contains("check_stale"), "{err}");
        assert_eq!(CheckLedger::for_workspace(tmp.path()).all().len(), before);
        assert_ne!(
            engine
                .get_entity(&id)
                .unwrap()
                .metadata
                .get("status")
                .map(|v| v.to_frontmatter_string())
                .as_deref(),
            Some("complete"),
            "the refused write left the entity as it was"
        );
    }

    /// Criterion 5 complement: a read-only mount refuses a check
    /// typed (`READ_ONLY_MOUNT`) — capability gating runs before the
    /// entity lookup, same as every mutation-shaped guard.
    #[test]
    fn check_refuses_read_only_mounts_typed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut mount = crate::engine::test_helpers::folder_mount("ro", tmp.path().to_path_buf());
        mount.capability = MountCapability::ReadOnly;
        let mut engine = crate::Engine::from_mounts(vec![(
            mount,
            Box::new(crate::storage::FilesystemBackend::new(
                tmp.path().to_path_buf(),
            )) as Box<dyn crate::backend::MemBackend>,
        )])
        .unwrap();
        let err = engine
            .record_check(
                "ro",
                "ro--anything",
                Verdict::Ok,
                crate::check::CheckKind::Verification,
                None,
                Actor::Cli,
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "READ_ONLY_MOUNT");
    }
}
