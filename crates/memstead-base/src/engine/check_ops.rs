//! The check operation and derived check state.
//!
//! `record_check` is the engine-recorded act of verification: it
//! appends one [`crate::check::CheckRecord`] — verdict, method note,
//! the entity's `content_hash` at check time, and provenance
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
            None,
            actor,
            client,
        )
    }

    /// [`Self::record_check`] with the full record shape: a kind that may
    /// be a caller-declared foreign `x-<name>` kind (recorded verbatim,
    /// never interpreted — it stamps no schema pin and moves no state),
    /// an optional structured finding (validated before anything is
    /// appended; a malformed one refuses `INVALID_CHECK_FINDING`), and an
    /// optional `observed_at`, the time the check was MADE as the caller
    /// states it (ISO-8601 UTC or a civil date; a shape the parser does
    /// not read or an instant in the future refuses `INVALID_INPUT`),
    /// stored beside the engine's own write time `ts`.
    #[allow(clippy::too_many_arguments)]
    pub fn record_check_with(
        &mut self,
        mem_name: &str,
        entity_id: &str,
        verdict: Verdict,
        kind: &crate::check::RecordKind,
        method: Option<&str>,
        finding: Option<crate::check::CheckFinding>,
        observed_at: Option<&str>,
        actor: Actor,
        client: Option<&ClientId>,
    ) -> Result<CheckRecord, EngineError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let observed_at = observed_at
            .map(|raw| crate::check::parse_observed_at(raw, now))
            .transpose()
            .map_err(EngineError::InvalidInput)?;
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
            .ok_or_else(|| self.not_found(entity_id))?;
        let Some(root) = self.workspace_root() else {
            return Err(EngineError::CheckNotRecorded {
                reason: "engine has no workspace root — no durable check store".to_string(),
            });
        };
        let ledger = CheckLedger::for_workspace(root);
        let record = CheckRecord {
            ts: now,
            observed_at,
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
            // records as absence.
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
            // The write the check saw: the entity's last write as the
            // mem's provenance holds it now.
            checked_write: self
                .mem_touches(mem_name)
                .last_write(entity_id)
                .map(str::to_string),
        };
        ledger
            .record_local(record)
            .map_err(|e| EngineError::CheckNotRecorded {
                reason: format!("ledger append failed: {e}"),
            })
    }

    /// The check records of one mem's entities, in standing order
    /// ([`CheckLedger::in_standing_order`]: by time of the act, a carried
    /// line after the records it supersedes, ties by the rows' bytes):
    /// every row, every kind, nothing derived. The read a
    /// consumer takes instead of parsing `checks.jsonl` itself, so the
    /// line shape can grow (as it did with `observed_at`) without
    /// breaking a reader. Refuses an unknown mem; an engine with no
    /// workspace root has no ledger and serves nothing. An archive mount
    /// keeps its sealed records under `entity --provenance`; this read is
    /// the workspace ledger's. The mem need not be mounted: the ledger
    /// keeps the rows of a mem after it was unmounted or deleted
    /// (archived bundles, deleted forks), and a history over them is
    /// exactly what a reader comes here for instead of the file; the
    /// second value says whether the mem is mounted now.
    pub fn check_records(&self, mem_name: &str) -> (Vec<CheckRecord>, bool) {
        let mounted = self.mounts.iter().any(|m| m.mount.mem == mem_name);
        let rows = self
            .workspace_root()
            .map(|root| CheckLedger::for_workspace(root).rows_for_mem(mem_name))
            .unwrap_or_default();
        (rows, mounted)
    }

    /// Every record of the workspace ledger, in standing order, whatever mem
    /// the entity belongs to and whether that mem is mounted.
    pub fn check_records_all(&self) -> Vec<CheckRecord> {
        self.workspace_root()
            .map(|root| CheckLedger::for_workspace(root).in_standing_order())
            .unwrap_or_default()
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

    /// The write-time reading of both transition forms. A form gates only
    /// the transition: a write after which the entity holds the form's
    /// gated value where before it did not exist, was of a type without
    /// the form, or did not hold the value. `before` is the stored entity
    /// with its type definition (`None` on a create). `violated` is the
    /// shared constraints pass over the result ([`crate::ops::health::unsatisfied_constraints`]);
    /// the result keeps its non-transition violations, keeps a
    /// `transition_requires_checks` violation only on a transition, and
    /// replaces every `transition_requires_self_check` reading with the
    /// transition rule: the write passes when a fresh independent
    /// confirming record of the declared kind names `result_hash` (the
    /// hash of the bytes the write produces: nothing to carry), or names
    /// the stored entity's hash while the write changes nothing but the
    /// gated field and the engine-stamped timestamps (the record is then
    /// carried, and its kind is returned). Otherwise the form reports a
    /// violation naming the content the write changed beside the gated
    /// field. The health axis keeps its state-based reading.
    pub(crate) fn gate_transition_forms(
        &self,
        before: Option<(&crate::entity::Entity, &memstead_schema::TypeDefinition)>,
        next: &crate::entity::Entity,
        result_hash: &str,
        type_def: &memstead_schema::TypeDefinition,
        violated: Vec<crate::ops::health::UnsatisfiedConstraint>,
    ) -> (Vec<crate::ops::health::UnsatisfiedConstraint>, Vec<String>) {
        use crate::ops::health::UnsatisfiedConstraint as U;
        use memstead_schema::ConstraintDef as C;
        if self.defer_transition_gates {
            let kept = violated
                .into_iter()
                .filter(|v| {
                    !matches!(
                        v,
                        U::TransitionRequiresChecks { .. } | U::TransitionRequiresSelfCheck { .. }
                    )
                })
                .collect();
            return (kept, Vec::new());
        }
        let holds = |e: &crate::entity::Entity, field: &str, value: &str| {
            e.metadata
                .get(field)
                .is_some_and(|v| v.to_frontmatter_string() == value)
        };
        // Whether the stored entity already stood at the gated value under
        // the same form: then the write is not the transition.
        let stood = |self_check: bool, field: &str, value: &str| {
            before.is_some_and(|(b, def)| {
                holds(b, field, value)
                    && def.constraints.iter().any(|c| match c {
                        C::TransitionRequiresSelfCheck {
                            field: f,
                            to_value: v,
                            ..
                        } => self_check && f == field && v == value,
                        C::TransitionRequiresChecks {
                            field: f,
                            to_value: v,
                            ..
                        } => !self_check && f == field && v == value,
                        _ => false,
                    })
            })
        };
        let mut out: Vec<U> = violated
            .into_iter()
            .filter(|v| match v {
                U::TransitionRequiresChecks {
                    field, to_value, ..
                } => !stood(false, field, to_value),
                U::TransitionRequiresSelfCheck { .. } => false,
                _ => true,
            })
            .collect();
        let provider = self.check_standing_provider();
        let mut result_entity = next.clone();
        result_entity.content_hash = result_hash.to_string();
        let mut carry: Vec<String> = Vec::new();
        for c in &type_def.constraints {
            let C::TransitionRequiresSelfCheck {
                field,
                to_value,
                check_kind,
                severity,
            } = c
            else {
                continue;
            };
            if !holds(next, field, to_value) || stood(true, field, to_value) {
                continue;
            }
            let at_result = provider(&result_entity, check_kind);
            if at_result.confirms() {
                continue;
            }
            let mut state = at_result.label().to_string();
            let mut changed = Vec::new();
            if let Some((b, _)) = before {
                let at_stored = provider(b, check_kind);
                if at_stored.confirms() {
                    changed = content_changed_beside(b, next, field);
                    if changed.is_empty() {
                        if !carry.contains(check_kind) {
                            carry.push(check_kind.clone());
                        }
                        continue;
                    }
                }
                state = at_stored.label().to_string();
            }
            out.push(U::TransitionRequiresSelfCheck {
                field: field.clone(),
                to_value: to_value.clone(),
                check_kind: check_kind.clone(),
                state,
                changed,
                severity: *severity,
            });
        }
        (out, carry)
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
        for rec in ledger.in_standing_order() {
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
            .ok_or_else(|| self.not_found(entity_id))?;
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
            .ok_or_else(|| self.not_found(entity_id))?;
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

/// The engine-stamped frontmatter keys a self-check transition write may
/// move beside the gated field.
const STAMPED_TIMESTAMPS: &[&str] = &["created_date", "last_modified"];

/// What `next` changes against `before` beside the gated `field` and the
/// engine-stamped timestamps: `type`, `title`, `section:<key>`,
/// `metadata:<key>`, `relationships`, in that order. Empty when the write
/// moves nothing but the gated value.
pub(crate) fn content_changed_beside(
    before: &crate::entity::Entity,
    next: &crate::entity::Entity,
    field: &str,
) -> Vec<String> {
    let mut changed = Vec::new();
    if before.entity_type != next.entity_type {
        changed.push("type".to_string());
    }
    if before.title != next.title {
        changed.push("title".to_string());
    }
    let mut keys: Vec<&String> = before.sections.keys().chain(next.sections.keys()).collect();
    keys.sort();
    keys.dedup();
    for k in keys {
        // Compared as the bytes are hashed: only the trailing newlines
        // the renderer drops are ignored; leading or other edge whitespace
        // is content (four leading spaces make a code block).
        let norm = |s: Option<&String>| {
            s.map(|b| b.trim_end_matches(['\n', '\r']).to_string())
                .filter(|b| !b.is_empty())
        };
        if norm(before.sections.get(k)) != norm(next.sections.get(k)) {
            changed.push(format!("section:{k}"));
        }
    }
    let mut keys: Vec<&String> = before.metadata.keys().chain(next.metadata.keys()).collect();
    keys.sort();
    keys.dedup();
    for k in keys {
        if k == field || STAMPED_TIMESTAMPS.contains(&k.as_str()) {
            continue;
        }
        let v = |m: &indexmap::IndexMap<String, crate::entity::MetadataValue>| {
            m.get(k.as_str()).map(|v| v.to_frontmatter_string())
        };
        if v(&before.metadata) != v(&next.metadata) {
            changed.push(format!("metadata:{k}"));
        }
    }
    let edges = |e: &crate::entity::Entity| {
        let mut r: Vec<(String, String)> = e
            .relationships
            .iter()
            .map(|r| (r.rel_type.clone(), r.target.0.clone()))
            .collect();
        r.sort();
        r.dedup();
        r
    };
    if edges(before) != edges(next) {
        changed.push("relationships".to_string());
    }
    changed
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
        // The workspace's fixed authored-schema location, so an export
        // of the mem finds the schema source to embed.
        let schemas_dir = tmp.path().join(".memstead").join("schemas");
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
  - memo
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
  - key: remark
    description: r
    field_type: string
    required: false
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
updatable_fields:
  - title
  - body
  - status
  - remark
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
        // The same type without the gate, for a retype onto it.
        let plan_yaml = std::fs::read_to_string(pkg.join("types").join("plan.yaml")).unwrap();
        let cut = plan_yaml.find("constraints:").unwrap();
        let memo_yaml =
            plan_yaml[..cut].replacen("name: plan", "name: memo", 1) + "write_rules: []\n";
        std::fs::write(pkg.join("types").join("memo.yaml"), memo_yaml).unwrap();
        let mem_dir = tmp.path().join("bundles");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format": 1, "schema": "selfgate@0.1.0", "version": "1.0.0"}"#,
        )
        .unwrap();
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
                expected_hash: crate::ExpectedHash::Current(current),
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

    /// One update: an optional metadata pair and an optional body text.
    fn update(
        engine: &mut crate::Engine,
        id: &crate::entity::EntityId,
        meta: Option<(&str, &str)>,
        body: Option<&str>,
    ) -> Result<crate::engine::UpdateEntityOutcome, crate::engine::error::EngineError> {
        let (actor, client) = crate::engine::test_helpers::cli_actor();
        let current = engine.get_entity(id).unwrap().content_hash.clone();
        let mut metadata = indexmap::IndexMap::new();
        if let Some((k, v)) = meta {
            metadata.insert(k.to_string(), v.to_string());
        }
        let mut sections = indexmap::IndexMap::new();
        if let Some(b) = body {
            sections.insert("body".to_string(), b.to_string());
        }
        engine.update_entity(
            crate::engine::UpdateEntityArgs {
                anchors: Vec::new(),
                id: id.clone(),
                expected_hash: crate::ExpectedHash::Current(current),
                sections,
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
                    expected_hash: crate::ExpectedHash::Current(current),
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
        check_plan_as(engine, identity, crate::vcs::Role::Checker, id, kind);
    }

    /// A check under `identity` declared in `role`: the walker's shape
    /// is `checker`; `author` and `unspecified` are the roles a gate
    /// refuses.
    fn check_plan_as(
        engine: &mut crate::Engine,
        identity: &str,
        role: crate::vcs::Role,
        id: &crate::entity::EntityId,
        kind: &str,
    ) {
        let (actor, client) = crate::engine::test_helpers::cli_actor();
        engine.set_identity(Some(identity.to_string()));
        engine.set_role(role);
        engine
            .record_check_with(
                "bundles",
                id.as_ref(),
                Verdict::Ok,
                &crate::check::RecordKind::from_wire(kind).unwrap(),
                Some("projection walked"),
                None,
                None,
                actor,
                Some(&client),
            )
            .unwrap();
        engine.set_role(crate::vcs::Role::Unspecified);
    }

    /// The defect: a bundle carries a fresh `x-projection` ok record under
    /// an independent identity, the author's `status: complete` passes
    /// the gate, and the write moves the hash; before the carry, the
    /// constraints axis reported the gate unsatisfied (`check_stale`) the
    /// moment after it admitted the write. Now the licensing record is
    /// carried to the post-write hash, marked, still the checker's; the
    /// constraints axis stays clean and the gate keeps holding across a
    /// later write to the complete plan.
    /// A gate on the `conformance` kind closes on a fresh conformance
    /// record: the gate's reader derives the state under the mem's
    /// schema pin, as the entity read does. Before, it derived by hash
    /// alone, every conformance record (which carries its pin) read
    /// `check_stale`, and the transition stayed blocked forever.
    #[test]
    fn a_conformance_self_check_gate_closes_on_a_fresh_record() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = self_check_gated_engine(&tmp, "conformance");
        engine.set_identity(Some("author-a".to_string()));
        let id = create_plan(&mut engine, "The Bundle");
        let entity = engine.get_entity(&id).unwrap().clone();
        let standing = (engine.check_standing_provider())(&entity, "conformance");
        assert_eq!(standing.state, CheckState::NeverChecked);

        check_plan(&mut engine, "checker-c", &id, "conformance");
        let entity = engine.get_entity(&id).unwrap().clone();
        let standing = (engine.check_standing_provider())(&entity, "conformance");
        assert_eq!(standing.state, CheckState::CheckedOk, "{standing:?}");
        assert!(standing.confirms(), "{standing:?}");

        engine.set_identity(Some("author-a".to_string()));
        set_status(&mut engine, &id, "complete").expect("the gate admits the write");
        assert!(engine.constraint_findings(Some("bundles")).is_empty());
    }

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
            .in_standing_order()
            .into_iter()
            .filter(|r| r.entity == id.as_ref())
            .collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].carried_from.is_none());
        assert_eq!(lines[0].entity_hash, hash_at_check);

        // A later edit of the complete plan is not a transition: it
        // commits, appends no ledger line, and leaves the approval behind.
        // The entity reads stale and the constraints axis reports the gate
        // as a standing violation until a new check is recorded.
        engine.set_identity(Some("author-a".to_string()));
        let lines_before = ledger.in_standing_order().len();
        append_body(&mut engine, &id);
        assert_eq!(
            ledger.in_standing_order().len(),
            lines_before,
            "nothing carried"
        );
        let stale = &engine.latest_foreign_checks("bundles", id.as_ref())[0];
        assert_ne!(
            stale.entity_hash,
            engine.get_entity(&id).unwrap().content_hash,
            "the approval stays keyed to the content it approved"
        );
        let findings = engine.constraint_findings(Some("bundles"));
        assert_eq!(findings.len(), 1, "{findings:?}");
        check_plan(&mut engine, "checker-c", &id, "x-projection");
        assert!(engine.constraint_findings(Some("bundles")).is_empty());
        // A write that leaves the gate unentered carries nothing: back to
        // draft, then an append, and the record is not moved again.
        set_status(&mut engine, &id, "draft").unwrap();
        let before = ledger.in_standing_order().len();
        append_body(&mut engine, &id);
        assert_eq!(
            ledger.in_standing_order().len(),
            before,
            "no gate passed, nothing carried"
        );
    }

    /// On both self-check fixtures (an `x-` kind and `verification`): an
    /// edit of a section and an edit of a metadata field other than the
    /// gated one, after the transition, each commit, append nothing and
    /// leave the entity `check_stale` with the gate listed on the
    /// constraints axis; a fresh independent check clears it. A write that
    /// sets the gated value and changes a section in the same call refuses
    /// `CONSTRAINT_UNSATISFIED` naming the gate and the changed content,
    /// with storage and ledger unchanged.
    #[test]
    fn an_approval_does_not_outlive_an_edit_of_what_it_approved() {
        for kind in ["x-projection", "verification"] {
            let tmp = tempfile::TempDir::new().unwrap();
            let mut engine = self_check_gated_engine(&tmp, kind);
            engine.set_identity(Some("author-a".to_string()));
            let id = create_plan(&mut engine, "The Bundle");
            let ledger = CheckLedger::for_workspace(tmp.path());

            // The combined write refuses first, before any approval stood.
            check_plan(&mut engine, "checker-c", &id, kind);
            engine.set_identity(Some("author-a".to_string()));
            let stored = engine.get_entity(&id).unwrap().content_hash.clone();
            let lines = ledger.in_standing_order().len();
            let err = update(
                &mut engine,
                &id,
                Some(("status", "complete")),
                Some("rewritten."),
            )
            .expect_err("a self-check transition write may change nothing else");
            assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED", "{kind}");
            let msg = err.to_string();
            assert!(msg.contains("transition_requires_self_check"), "{msg}");
            assert!(msg.contains("section:body"), "{msg}");
            assert_eq!(engine.get_entity(&id).unwrap().content_hash, stored);
            assert_eq!(ledger.in_standing_order().len(), lines);

            // The transition on its own commits and stands confirming.
            set_status(&mut engine, &id, "complete").expect("the gate admits the write");
            let entity = engine.get_entity(&id).unwrap().clone();
            assert!((engine.check_standing_provider())(&entity, kind).confirms());
            assert!(engine.constraint_findings(Some("bundles")).is_empty());

            for edit in [
                (None, Some("a section edit.")),
                (Some(("remark", "noted")), None),
            ] {
                let lines = ledger.in_standing_order().len();
                update(&mut engine, &id, edit.0, edit.1).expect("an edit is not a transition");
                assert_eq!(
                    ledger.in_standing_order().len(),
                    lines,
                    "{kind}: nothing carried"
                );
                let entity = engine.get_entity(&id).unwrap().clone();
                let standing = (engine.check_standing_provider())(&entity, kind);
                assert_eq!(standing.label(), "check_stale", "{kind}");
                let findings = engine.constraint_findings(Some("bundles"));
                assert_eq!(findings.len(), 1, "{kind}: {findings:?}");
                check_plan(&mut engine, "checker-c", &id, kind);
                engine.set_identity(Some("author-a".to_string()));
                assert!(
                    engine.constraint_findings(Some("bundles")).is_empty(),
                    "{kind}"
                );
            }
        }
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

    /// The role is the gate's third condition, orthogonal to the reading.
    /// An independent identity's ok record declared in role `author`
    /// reads `confirmed_independent` on the checks axis (the identities
    /// differ, and the axis says so) and still refuses the gate under
    /// the label `role_author`; a record with no role refuses under
    /// `role_unspecified`; a `verifier` record confirms like a
    /// `checker`'s. The axis row carries the role and whether it
    /// confirms.
    #[test]
    fn a_record_in_role_author_or_without_a_role_does_not_close_the_gate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = self_check_gated_engine(&tmp, "verification");
        engine.set_identity(Some("author-a".to_string()));
        let id = create_plan(&mut engine, "The Bundle");

        // An independent identity, declared as the author.
        check_plan_as(
            &mut engine,
            "checker-c",
            crate::vcs::Role::Author,
            &id,
            "verification",
        );
        let standing =
            (engine.check_standing_provider())(engine.get_entity(&id).unwrap(), "verification");
        assert_eq!(standing.state, CheckState::CheckedOk);
        assert_eq!(
            standing.independence,
            Some(crate::engine::independence::Independence::ConfirmedIndependent)
        );
        assert!(!standing.confirms(), "{standing:?}");
        assert_eq!(standing.label(), "role_author");
        engine.set_identity(Some("author-a".to_string()));
        let err = set_status(&mut engine, &id, "complete").unwrap_err();
        assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED", "{err}");
        assert!(err.to_string().contains("role_author"), "{err}");
        let axis = crate::ops::health::health_checks_axis(&engine, Some("bundles"));
        let row = axis["bundles"]["independence"]["readings"][id.as_ref()]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert_eq!(row["reading"], "confirmed_independent", "{axis}");
        assert_eq!(row["role"], "author", "{axis}");
        assert_eq!(row["role_confirms"], false, "{axis}");

        // The same identity, no role declared.
        check_plan_as(
            &mut engine,
            "checker-c",
            crate::vcs::Role::Unspecified,
            &id,
            "verification",
        );
        let standing =
            (engine.check_standing_provider())(engine.get_entity(&id).unwrap(), "verification");
        assert!(!standing.confirms(), "{standing:?}");
        assert_eq!(standing.label(), "role_unspecified");
        engine.set_identity(Some("author-a".to_string()));
        let err = set_status(&mut engine, &id, "complete").unwrap_err();
        assert!(err.to_string().contains("role_unspecified"), "{err}");

        // A verifier's record confirms like a checker's.
        check_plan_as(
            &mut engine,
            "verifier-v",
            crate::vcs::Role::Verifier,
            &id,
            "verification",
        );
        let standing =
            (engine.check_standing_provider())(engine.get_entity(&id).unwrap(), "verification");
        assert!(standing.confirms(), "{standing:?}");
        engine.set_identity(Some("author-a".to_string()));
        set_status(&mut engine, &id, "complete").expect("a verifier's record closes the gate");
        let axis = crate::ops::health::health_checks_axis(&engine, Some("bundles"));
        let row = axis["bundles"]["independence"]["readings"][id.as_ref()]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert_eq!(row["role"], "verifier", "{axis}");
        assert_eq!(row["role_confirms"], true, "{axis}");
    }

    /// The gate on the archive mount. A complete plan exported to a
    /// `.mem` and mounted from bytes used to report
    /// `transition_requires_self_check` unsatisfied (`unconfirmable`):
    /// the mount has no provenance to derive independence from. The
    /// export seals the reading beside the record, so the mount's
    /// constraints axis is clean and its checks axis lists the plan
    /// `confirmed_independent`, exactly as the source engine did.
    #[test]
    fn sealed_independence_holds_the_transition_gate_on_the_archive_mount() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = self_check_gated_engine(&tmp, "verification");
        engine.set_identity(Some("author-a".to_string()));
        let id = create_plan(&mut engine, "The Bundle");
        check_plan(&mut engine, "checker-c", &id, "verification");
        engine.set_identity(Some("author-a".to_string()));
        set_status(&mut engine, &id, "complete").expect("the gate admits the write");
        assert!(engine.constraint_findings(Some("bundles")).is_empty());

        let bytes = engine.export_mem_to_bytes("bundles").unwrap();
        let mounted = crate::Engine::from_archive_bytes(bytes).unwrap();
        assert!(mounted.is_archive_mount("bundles"));
        let findings = mounted.constraint_findings(Some("bundles"));
        assert!(findings.is_empty(), "{findings:?}");
        let entity = mounted.get_entity(&id).unwrap();
        let standing = (mounted.check_standing_provider())(entity, "verification");
        assert!(standing.confirms(), "{standing:?}");
        let axis = crate::ops::health::health_checks_axis(&mounted, Some("bundles"));
        assert_eq!(
            axis["bundles"]["independence"]["confirmed_independent"]["items"],
            serde_json::json!([id.as_ref()]),
            "{axis}"
        );
        assert_eq!(
            axis["bundles"]["checked_ok"],
            serde_json::json!(1),
            "{axis}"
        );
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
        let before = CheckLedger::for_workspace(tmp.path())
            .in_standing_order()
            .len();
        let err = set_status(&mut engine, &id, "complete").unwrap_err();
        assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED");
        assert!(err.to_string().contains("check_stale"), "{err}");
        assert_eq!(
            CheckLedger::for_workspace(tmp.path())
                .in_standing_order()
                .len(),
            before
        );
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

    /// Complement: a read-only mount refuses a check
    /// typed (`READ_ONLY_MOUNT`) — capability gating runs before the
    /// entity lookup, same as every mutation-shaped guard.
    /// Every reader of a standing reads the record whose act is latest,
    /// whatever order the ledger holds the lines in: rows arrive here
    /// newest first (as a transport merge appends an older row after a
    /// newer local one), and the entity read, the foreign-kind read, the
    /// gate provider, the constraints axis, the health checks axis and the
    /// export seal each stand on the newest act.
    #[test]
    fn every_reader_stands_on_the_newest_act_not_the_last_line() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = self_check_gated_engine(&tmp, "x-projection");
        engine.set_identity(Some("author-a".to_string()));
        let id = create_plan(&mut engine, "The Bundle");
        let hash = engine.get_entity(&id).unwrap().content_hash.clone();
        let ledger = CheckLedger::for_workspace(tmp.path());
        // Times of the act after the entity's creation, so the rows read
        // against the writes they saw (no `checked_write`: the `ts` rule).
        let base = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 2;
        let row = |verdict: &str, kind: Option<&str>, ts: u64| CheckRecord {
            ts,
            observed_at: None,
            entity: id.as_ref().to_string(),
            verdict: verdict.to_string(),
            method: None,
            entity_hash: hash.clone(),
            actor: "cli".to_string(),
            client: None,
            role: "checker".to_string(),
            identity: Some("checker-c".to_string()),
            kind: kind.map(str::to_string),
            schema_ref: None,
            finding: None,
            renamed_from: None,
            carried_from: None,
            checked_write: None,
        };
        // Local ok at t2, then a merged failed check made at t1 < t2.
        ledger.record(&row("ok", None, base + 20)).unwrap();
        ledger
            .record(&row("ok", Some("x-projection"), base + 20))
            .unwrap();
        ledger.record(&row("failed", None, base + 10)).unwrap();
        ledger
            .record(&row("failed", Some("x-projection"), base + 10))
            .unwrap();

        let (state, latest) = engine.entity_check_state("bundles", id.as_ref()).unwrap();
        assert_eq!(state, crate::check::CheckState::CheckedOk);
        assert_eq!(latest.unwrap().ts, base + 20);
        let foreign = engine.latest_foreign_checks("bundles", id.as_ref());
        assert_eq!(foreign[0].verdict, "ok");
        let entity = engine.get_entity(&id).unwrap().clone();
        assert!((engine.check_standing_provider())(&entity, "x-projection").confirms());
        let axis = crate::ops::health::health_checks_axis(&engine, Some("bundles"));
        assert_eq!(axis["bundles"]["checked_ok"], 1, "{axis}");
        assert_eq!(axis["bundles"]["check_failed"], 0, "{axis}");
        let (sealed, _) = crate::ops::export::sealed_checks_bytes_for(
            Some(tmp.path()),
            "bundles",
            &[id.path().to_string()],
            None,
        );
        let sealed = crate::check::SealedChecks::from_archive_bytes(&sealed.unwrap()).unwrap();
        assert_eq!(
            sealed.latest(id.path(), "verification").unwrap().verdict,
            "ok"
        );
        assert_eq!(
            sealed.latest(id.path(), "x-projection").unwrap().verdict,
            "ok"
        );
        // The gate admits the transition on the standing record, and the
        // constraints axis reads no violation after it.
        set_status(&mut engine, &id, "complete").expect("the newest act confirms");
        assert!(engine.constraint_findings(Some("bundles")).is_empty());

        // A merged failed check made later than the carried line stands.
        let post = engine.get_entity(&id).unwrap().content_hash.clone();
        ledger
            .record(&CheckRecord {
                entity_hash: post,
                ..row("failed", Some("x-projection"), base + 30)
            })
            .unwrap();
        let entity = engine.get_entity(&id).unwrap().clone();
        assert!(!(engine.check_standing_provider())(&entity, "x-projection").confirms());
        assert!(!engine.constraint_findings(Some("bundles")).is_empty());
    }

    /// Conflict resolution lands a self-check gated value only on a fresh
    /// independent record naming the resolved bytes' own hash: the side
    /// that is the checked content commits, the edited side refuses with
    /// the file and the ledger unchanged. The refusal failed before the
    /// repair: resolution validated section keys and content only.
    #[test]
    fn conflict_resolution_lands_a_self_check_gated_value_only_on_a_record_of_its_bytes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = self_check_gated_engine(&tmp, "x-projection");
        engine.set_identity(Some("author-a".to_string()));
        let id = create_plan(&mut engine, "The Bundle");
        check_plan(&mut engine, "checker-c", &id, "x-projection");
        engine.set_identity(Some("author-a".to_string()));
        set_status(&mut engine, &id, "complete").unwrap();
        let path = tmp.path().join("bundles").join("the-bundle.md");
        let checked = std::fs::read_to_string(&path).unwrap();
        let edited = checked.replace("the plan body.", "an edit nobody checked.");
        assert_ne!(edited, checked);
        let conflicted = format!(
            "<<<<<<< ours\n{}=======\n{}>>>>>>> theirs\n",
            edited, checked
        );
        let ledger = CheckLedger::for_workspace(tmp.path());
        let (actor, client) = crate::engine::test_helpers::cli_actor();

        std::fs::write(&path, &conflicted).unwrap();
        let lines = ledger.in_standing_order().len();
        let err = engine
            .resolve_merge_conflict(
                &id,
                crate::engine::conflicts::ConflictSide::Ours,
                actor,
                Some(&client),
                None,
            )
            .expect_err("the edited side was never checked");
        assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED", "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), conflicted);
        assert_eq!(ledger.in_standing_order().len(), lines);

        engine
            .resolve_merge_conflict(
                &id,
                crate::engine::conflicts::ConflictSide::Theirs,
                actor,
                Some(&client),
                None,
            )
            .expect("the checked side lands");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), checked);
    }

    /// GRADER c3: a rename on a folder mem must not drop the writers
    /// recorded under the old id. A creates, B rewrites, C renames, B
    /// checks the renamed entry: B wrote what it checked and must not read
    /// `confirmed_independent`; the first author must stay A.
    #[test]
    fn a_rename_keeps_every_writer_in_the_story_on_folder() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = self_check_gated_engine(&tmp, "x-projection");
        engine.set_identity(Some("identity-a".to_string()));
        let id = create_plan(&mut engine, "The Bundle");
        engine.set_identity(Some("identity-b".to_string()));
        update(&mut engine, &id, None, Some("rewritten by b.")).unwrap();
        engine.set_identity(Some("identity-c".to_string()));
        let (actor, client) = crate::engine::test_helpers::cli_actor();
        let current = engine.get_entity(&id).unwrap().content_hash.clone();
        let out = engine
            .rename_entity(
                crate::engine::RenameEntityArgs {
                    id: id.clone(),
                    expected_hash: crate::ExpectedHash::Current(current),
                    new_title: "The Renamed".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let new_id = out.new_id.clone();
        eprintln!(
            "story {:?}",
            engine.mem_touches("bundles").story(new_id.as_ref())
        );
        check_plan(&mut engine, "identity-b", &new_id, "x-projection");
        let touches = engine.mem_touches("bundles");
        let entity = engine.get_entity(&new_id).unwrap().clone();
        let standing = (engine.check_standing_provider())(&entity, "x-projection");
        let first = touches.first_author(new_id.as_ref()).map(str::to_string);
        engine.set_identity(Some("identity-e".to_string()));
        let gate = set_status(&mut engine, &new_id, "complete");
        assert_eq!(
            (standing.label(), first.as_deref(), gate.is_err()),
            ("self_checked", Some("identity-a"), true)
        );
    }

    /// Independence on an entity that is not a plan criterion counts every
    /// identity that wrote it up to the write the check saw, on the folder
    /// backend, with the rewrite one second after the create and in the
    /// same second. A creates, B rewrites and checks, C sets the gated
    /// value: refused, B's check reads `self_checked`. D, who wrote
    /// nothing, checks and sets the value in the same second: the gate
    /// opens and the carried line reads `confirmed_independent`. A rewrite
    /// by D before D's check, in the same second, makes it `self_checked`.
    /// A write without an identity before the check reads `unconfirmable`;
    /// a line without `checked_write` is read against the writes at or
    /// before its `ts`. The first author is A in both timings.
    #[test]
    fn independence_counts_every_identity_that_wrote_what_was_checked_on_folder() {
        for pause in [true, false] {
            let tmp = tempfile::TempDir::new().unwrap();
            let mut engine = self_check_gated_engine(&tmp, "x-projection");
            engine.set_identity(Some("identity-a".to_string()));
            let id = create_plan(&mut engine, "The Bundle");
            if pause {
                std::thread::sleep(std::time::Duration::from_millis(1100));
            }
            engine.set_identity(Some("identity-b".to_string()));
            update(&mut engine, &id, None, Some("rewritten by b.")).unwrap();
            check_plan(&mut engine, "identity-b", &id, "x-projection");
            let touches = engine.mem_touches("bundles");
            assert_eq!(
                touches.first_author(id.as_ref()),
                Some("identity-a"),
                "pause {pause}"
            );
            let entity = engine.get_entity(&id).unwrap().clone();
            let standing = (engine.check_standing_provider())(&entity, "x-projection");
            assert_eq!(standing.label(), "self_checked", "pause {pause}");
            engine.set_identity(Some("identity-c".to_string()));
            let err =
                set_status(&mut engine, &id, "complete").expect_err("b checked its own writing");
            assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED");

            check_plan(&mut engine, "identity-d", &id, "x-projection");
            engine.set_identity(Some("identity-d".to_string()));
            set_status(&mut engine, &id, "complete").expect("d wrote nothing it checked");
            let entity = engine.get_entity(&id).unwrap().clone();
            let standing = (engine.check_standing_provider())(&entity, "x-projection");
            assert_eq!(standing.label(), "confirmed_independent", "pause {pause}");
            let carried = &engine.latest_foreign_checks("bundles", id.as_ref())[0];
            assert!(carried.carried_from.is_some());
            assert_eq!(carried.identity.as_deref(), Some("identity-d"));
        }

        // A rewrite by D in the same second, before D's check.
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = self_check_gated_engine(&tmp, "x-projection");
        engine.set_identity(Some("identity-a".to_string()));
        let id = create_plan(&mut engine, "The Bundle");
        engine.set_identity(Some("identity-d".to_string()));
        update(&mut engine, &id, None, Some("rewritten by d.")).unwrap();
        check_plan(&mut engine, "identity-d", &id, "x-projection");
        let entity = engine.get_entity(&id).unwrap().clone();
        assert_eq!(
            (engine.check_standing_provider())(&entity, "x-projection").label(),
            "self_checked"
        );

        // A write without an identity before the check.
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = self_check_gated_engine(&tmp, "x-projection");
        engine.set_identity(Some("identity-a".to_string()));
        let id = create_plan(&mut engine, "The Bundle");
        engine.set_identity(None);
        update(&mut engine, &id, None, Some("anonymous edit.")).unwrap();
        check_plan(&mut engine, "identity-d", &id, "x-projection");
        let entity = engine.get_entity(&id).unwrap().clone();
        assert_eq!(
            (engine.check_standing_provider())(&entity, "x-projection").label(),
            "unconfirmable"
        );

        // A line an earlier engine wrote: no `checked_write`, read against
        // the writes at or before its `ts`.
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = self_check_gated_engine(&tmp, "x-projection");
        engine.set_identity(Some("identity-a".to_string()));
        let id = create_plan(&mut engine, "The Bundle");
        let entity = engine.get_entity(&id).unwrap().clone();
        let touches = engine.mem_touches("bundles");
        let legacy = |identity: &str, ts: u64| CheckRecord {
            ts,
            observed_at: None,
            entity: id.as_ref().to_string(),
            verdict: "ok".to_string(),
            method: None,
            entity_hash: entity.content_hash.clone(),
            actor: "cli".to_string(),
            client: None,
            role: "checker".to_string(),
            identity: Some(identity.to_string()),
            kind: Some("x-projection".to_string()),
            schema_ref: None,
            finding: None,
            renamed_from: None,
            carried_from: None,
            checked_write: None,
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let (r, _) = engine.independence_of(&entity, &legacy("identity-a", now + 5), &touches);
        assert_eq!(r, crate::engine::independence::Independence::SelfChecked);
        let (r, _) = engine.independence_of(&entity, &legacy("identity-d", now + 5), &touches);
        assert_eq!(
            r,
            crate::engine::independence::Independence::ConfirmedIndependent
        );
        let (r, _) = engine.independence_of(&entity, &legacy("identity-d", 1), &touches);
        assert_eq!(
            r,
            crate::engine::independence::Independence::Unconfirmable,
            "no write at or before the line's ts"
        );
    }

    fn create_args(title: &str, ty: &str, status: Option<&str>) -> crate::engine::CreateEntityArgs {
        let mut sections = indexmap::IndexMap::new();
        sections.insert("body".to_string(), "the plan body.".to_string());
        let mut metadata = indexmap::IndexMap::new();
        if let Some(v) = status {
            metadata.insert("status".to_string(), v.to_string());
        }
        crate::engine::CreateEntityArgs {
            anchors: Vec::new(),
            mem: "bundles".to_string(),
            title: title.to_string(),
            entity_type: ty.to_string(),
            sections,
            metadata,
            relations: Vec::new(),
            dry_run: false,
        }
    }

    fn update_args(
        engine: &crate::Engine,
        id: &crate::entity::EntityId,
        status: Option<&str>,
        body: Option<&str>,
    ) -> crate::engine::UpdateEntityArgs {
        let mut metadata = indexmap::IndexMap::new();
        if let Some(v) = status {
            metadata.insert("status".to_string(), v.to_string());
        }
        let mut sections = indexmap::IndexMap::new();
        if let Some(b) = body {
            sections.insert("body".to_string(), b.to_string());
        }
        crate::engine::UpdateEntityArgs {
            anchors: Vec::new(),
            id: id.clone(),
            expected_hash: crate::ExpectedHash::Current(
                engine.get_entity(id).unwrap().content_hash.clone(),
            ),
            sections,
            append_sections: indexmap::IndexMap::new(),
            patch_sections: indexmap::IndexMap::new(),
            sections_unset: Vec::new(),
            metadata,
            metadata_unset: Vec::new(),
            declare_relations: vec![],
            dry_run: false,
            relations_unset: Vec::new(),
            anchors_unset: Vec::new(),
        }
    }

    /// The self-check column of the verb table, per verb, storage and
    /// ledger unchanged on each refusal: create and batch create refuse a
    /// gated value (no record can name content that did not exist); batch
    /// update lands a transition that changes nothing else, carrying the
    /// record, and refuses one that also changes a section; a retype onto
    /// the gated type refuses (the write changes the type). Proposal merge
    /// composes through the same prepare steps as create and update.
    #[test]
    fn the_self_check_column_of_the_verb_table() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = self_check_gated_engine(&tmp, "x-projection");
        engine.set_identity(Some("author-a".to_string()));
        let ledger = CheckLedger::for_workspace(tmp.path());
        let (actor, client) = crate::engine::test_helpers::cli_actor();
        let files = |tmp: &tempfile::TempDir| {
            let mut v: Vec<_> = std::fs::read_dir(tmp.path().join("bundles"))
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            v.sort();
            v
        };
        let before = files(&tmp);

        let err = engine
            .create_entity(
                create_args("Born Done", "plan", Some("complete")),
                actor,
                Some(&client),
                None,
            )
            .expect_err("create");
        assert_eq!(err.code(), "CONSTRAINT_UNSATISFIED");
        let batch = engine.batch_create(
            vec![(create_args("Born Done Too", "plan", Some("complete")), None)],
            actor,
            Some(&client),
            false,
        );
        assert!(
            batch.as_ref().is_err() || !batch.as_ref().unwrap().applied,
            "batch create must not land the gated value: {batch:?}"
        );
        assert_eq!(files(&tmp), before);
        assert!(ledger.in_standing_order().is_empty());

        let id = create_plan(&mut engine, "The Bundle");
        check_plan(&mut engine, "checker-c", &id, "x-projection");
        engine.set_identity(Some("author-a".to_string()));
        let stored = engine.get_entity(&id).unwrap().content_hash.clone();
        let lines = ledger.in_standing_order().len();
        let refused = engine.batch_update(
            vec![(
                update_args(&engine, &id, Some("complete"), Some("rewritten.")),
                None,
            )],
            actor,
            Some(&client),
            false,
        );
        assert!(
            refused.as_ref().is_err() || !refused.as_ref().unwrap().applied,
            "{refused:?}"
        );
        assert_eq!(engine.get_entity(&id).unwrap().content_hash, stored);
        assert_eq!(ledger.in_standing_order().len(), lines);
        engine
            .batch_update(
                vec![(update_args(&engine, &id, Some("complete"), None), None)],
                actor,
                Some(&client),
                false,
            )
            .unwrap();
        assert_eq!(
            engine.get_entity(&id).unwrap().metadata["status"].to_frontmatter_string(),
            "complete"
        );
        assert_eq!(
            ledger.in_standing_order().len(),
            lines + 1,
            "the record is carried"
        );

        // Retype: a memo at `complete`, checked, retyped onto the gated plan.
        let memo = engine
            .create_entity(
                create_args("The Memo", "memo", Some("complete")),
                actor,
                Some(&client),
                None,
            )
            .unwrap()
            .id;
        check_plan(&mut engine, "checker-c", &memo, "x-projection");
        engine.set_identity(Some("author-a".to_string()));
        let stored = engine.get_entity(&memo).unwrap().content_hash.clone();
        let err = engine
            .retype_entity(
                crate::engine::RetypeEntityArgs {
                    id: memo.clone(),
                    expected_hash: crate::ExpectedHash::Current(stored.clone()),
                    target_type: "plan".to_string(),
                    section_map: indexmap::IndexMap::new(),
                    drop_metadata: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect_err("a retype onto the gate changes the type");
        assert!(err.to_string().contains("block-tier constraint"), "{err}");
        assert_eq!(engine.get_entity(&memo).unwrap().content_hash, stored);
    }

    /// A rename that rewrites a link inside another entry writes that
    /// entry: the renamer's own check of it reads `self_checked`.
    #[test]
    fn a_rename_counts_its_identity_on_every_referrer_it_rewrites_on_folder() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = self_check_gated_engine(&tmp, "x-projection");
        engine.set_identity(Some("identity-a".to_string()));
        let target = create_plan(&mut engine, "The Target");
        let (actor, client) = crate::engine::test_helpers::cli_actor();
        let mut sections = indexmap::IndexMap::new();
        sections.insert("body".to_string(), format!("see [[{}]].", target.0));
        let referrer = engine
            .create_entity(
                crate::engine::CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "bundles".to_string(),
                    title: "The Referrer".to_string(),
                    entity_type: "plan".to_string(),
                    sections,
                    metadata: indexmap::IndexMap::new(),
                    relations: vec![crate::ops::RelateArg {
                        target: target.clone(),
                        rel_type: "PART_OF".to_string(),
                        description: None,
                    }],
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap()
            .id;
        let before = engine.get_entity(&referrer).unwrap().content_hash.clone();
        engine.set_identity(Some("identity-c".to_string()));
        let current = engine.get_entity(&target).unwrap().content_hash.clone();
        engine
            .rename_entity(
                crate::engine::RenameEntityArgs {
                    id: target.clone(),
                    expected_hash: crate::ExpectedHash::Current(current),
                    new_title: "The Moved".to_string(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let after = engine.get_entity(&referrer).unwrap().content_hash.clone();
        assert_ne!(before, after, "the rename rewrote the referrer");
        check_plan(&mut engine, "identity-c", &referrer, "x-projection");
        let entity = engine.get_entity(&referrer).unwrap().clone();
        let standing = (engine.check_standing_provider())(&entity, "x-projection");
        assert_eq!(standing.label(), "self_checked");
        let touches = engine.mem_touches("bundles");
        assert_eq!(touches.first_author(referrer.as_ref()), Some("identity-a"));
    }

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
