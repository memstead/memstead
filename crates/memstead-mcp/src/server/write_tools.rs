//! The entity write tools: every verb that mutates an entity and records provenance (a note and a role) for it.

use super::*;

#[tool_router(router = write_tool_router, vis = "pub(crate)")]
impl McpServer {
    // ----------------------------------------------------------------------
    // Write tools
    // ----------------------------------------------------------------------

    #[tool(
        name = "memstead_create",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_create(
        &self,
        Parameters(p): Parameters<CreateParams>,
    ) -> CallToolResult {
        if let Some(err) = validate_note(p.note.as_deref()) {
            return err;
        }
        let mem = self.resolve_mem(p.mem.as_deref());
        let dry_run = p.dry_run.unwrap_or(false);
        let (role, identity) =
            match self.resolve_call_provenance(p.role.as_deref(), p.identity.as_deref()) {
                Ok(v) => v,
                Err(resp) => return *resp,
            };

        // Wire JSON matches the `CreateResult` contract.
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        engine.set_role(role);
        engine.set_identity(identity);
        let relations: Vec<memstead_base::ops::RelateArg> = p
            .relations
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|r| memstead_base::ops::RelateArg {
                target: EntityId(r.target),
                rel_type: r.rel_type,
                description: r.description,
            })
            .collect();
        let anchors: Vec<memstead_base::anchor::AnchorInput> = p
            .anchors
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.into_engine())
            .collect();
        let args = memstead_base::CreateEntityArgs {
            mem: mem.clone(),
            title: p.title.clone(),
            entity_type: p.entity_type.clone(),
            sections: p.sections.unwrap_or_default(),
            metadata: p.metadata.unwrap_or_default(),
            relations,
            anchors,
            dry_run,
        };
        let client = self.client.get().cloned();
        match engine.create_entity(args, Actor::Agent, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                // Skip empty `incoming` / `None` `incoming_count`
                // manually to match full's
                // `#[serde(skip_serializing_if=...)]`.
                let mut body = serde_json::json!({
                    "id": outcome.id.to_string(),
                    "title": outcome.title,
                    "mem": outcome.mem,
                    "file_path": outcome.file_path,
                    "created_date": outcome.created_date,
                    "_hash": outcome.content_hash,
                    "write_id": outcome.write_id,
                    "warnings": outcome.warnings,
                    "type_guidance": outcome.type_guidance,
                });
                if let Some(count) = outcome.incoming_count {
                    body["incoming_count"] = serde_json::json!(count);
                }
                if !outcome.incoming.is_empty() {
                    body["incoming"] =
                        serde_json::to_value(&outcome.incoming).unwrap_or(serde_json::Value::Null);
                }
                if !outcome.relations_declared.is_empty() {
                    body["relations_declared"] = serde_json::to_value(&outcome.relations_declared)
                        .unwrap_or(serde_json::Value::Null);
                }
                attach_durability(&mut body, &engine, &outcome.mem);
                let (engine, finished_notices) = engine.finish();
                attach_mem_changed(&mut body, finished_notices);
                let res = json_response(&body);
                match mem_schema_ref_unified(&engine, &mem) {
                    Some(s) => with_mem_schema_anchor(res, &s),
                    None => res,
                }
            }
            Err(e) => {
                // The notice already rode `structured_content` here;
                // the text channel lacked the `MEM_RELOADED` line a
                // successful response carries. A mutation reloads inside
                // the engine, so reconstruct the warning from the
                // drained notices to match the success channel split —
                // collision (`HASH_MISMATCH`) is the path drift matters
                // most, since it lands on the very entity being written.
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
            }
        }
    }

    #[tool(
        name = "memstead_update",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_update(
        &self,
        Parameters(p): Parameters<UpdateParams>,
    ) -> CallToolResult {
        if let Some(err) = validate_entity_id(&p.id) {
            return err;
        }
        if let Some(err) = validate_note(p.note.as_deref()) {
            return err;
        }
        let id = EntityId::canonical(&p.id);
        let dry_run = p.dry_run.unwrap_or(false);
        let (role, identity) =
            match self.resolve_call_provenance(p.role.as_deref(), p.identity.as_deref()) {
                Ok(v) => v,
                Err(resp) => return *resp,
            };

        // Wire JSON matches the `UpdateResult` contract.
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        engine.set_role(role);
        engine.set_identity(identity);
        // The mem the anchors ride on is the resolved id's mem: a bare
        // slug the engine resolves to one mem must not stage its
        // anchors under the empty mem name. The verb itself receives
        // the id as given and announces the resolution.
        let mem_for_anchor = match engine.resolve_entity_id(&id) {
            Ok((resolved, _)) => resolved.mem().to_string(),
            Err(e) => return engine_err_unified(e, &engine),
        };
        let patch_sections: indexmap::IndexMap<String, Vec<memstead_base::ops::PatchArg>> = p
            .patch_sections
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    v.into_vec()
                        .into_iter()
                        .map(|v| memstead_base::ops::PatchArg {
                            old: v.old,
                            new: v.new,
                            all: v.all.unwrap_or(false),
                        })
                        .collect(),
                )
            })
            .collect();
        let declare_relations: Vec<memstead_base::ops::RelateArg> = p
            .declare_relations
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|r| memstead_base::ops::RelateArg {
                target: EntityId(r.target),
                rel_type: r.rel_type,
                description: r.description,
            })
            .collect();
        let anchors: Vec<memstead_base::anchor::AnchorInput> = p
            .anchors
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.into_engine())
            .collect();
        let anchors_unset: Vec<memstead_base::anchor::AnchorUnsetInput> = p
            .anchors_unset
            .unwrap_or_default()
            .into_iter()
            .map(|u| u.into_engine())
            .collect();
        let mut args = memstead_base::UpdateEntityArgs {
            anchors,
            anchors_unset,
            id: id.clone(),
            expected_hash: p.expected_hash.clone(),
            sections: p.sections.unwrap_or_default(),
            append_sections: p.append_sections.unwrap_or_default(),
            patch_sections,
            sections_unset: p.sections_unset.clone().unwrap_or_default(),
            metadata: p.metadata.unwrap_or_default(),
            metadata_unset: p.metadata_unset.unwrap_or_default(),
            dry_run,
            declare_relations,
            relations_unset: p
                .relations_unset
                .unwrap_or_default()
                .into_iter()
                .map(|r| memstead_base::ops::RelationUnsetArg {
                    rel_type: r.rel_type,
                    target: EntityId(r.target),
                })
                .collect(),
        };
        // The compare-and-swap gate, enforced HERE rather than by the engine
        // core, which has always checked only when a caller supplies a token
        // (consistency-sweep 03/04). Content-changing updates still demand it;
        // an anchors-only update does not, because the sidecar is outside
        // `_hash` and the token would compare a value the write cannot move.
        // `changes_content` is the engine's own predicate, so this surface and
        // the CLI cannot come to disagree about whether a write is safe.
        // An EMPTY token is no token on an anchors-only write, where the CLI
        // already discards it: leaving it Some("") sent it to the engine,
        // which compared "" against a real hash and refused HASH_MISMATCH for
        // a write the CLI accepted.
        //
        // Narrowed to that shape on purpose. Normalizing unconditionally made
        // this gate fire BEFORE the engine's existence and stub gates, so an
        // empty token on a stub reported a missing hash instead of
        // `STUB_NOT_UPDATABLE`, which is the more specific and more actionable
        // refusal. A content-changing payload therefore keeps its empty token
        // and keeps the engine's own ordering.
        if !args.changes_content() {
            args.expected_hash = args.expected_hash.filter(|h| !h.is_empty());
        }
        if args.expected_hash.is_none() && !args.dry_run && args.changes_content() {
            let message = format!(
                "`expected_hash` is required for an update that changes content. Read `{id}` \
                 first and pass its `_hash`, or pass `dry_run: true` to preview. Only an \
                 anchors-only update (`anchors` / `anchors_unset` and nothing else) may omit \
                 it, because anchors are outside the content hash."
            );
            return tool_error_with_payload(
                "EXPECTED_HASH_REQUIRED",
                &message,
                envelope(
                    "EXPECTED_HASH_REQUIRED",
                    message.clone(),
                    serde_json::json!({ "field": "expected_hash", "id": id.to_string() }),
                ),
            );
        }
        let client = self.client.get().cloned();
        match engine.update_entity(args, Actor::Agent, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                // ModifiedSections / ModifiedMetadata serialise
                // with `#[serde(skip_serializing_if = "Vec::is_empty")]`
                // on each inner vec — matching full's UpdateResult
                // wire shape.
                let mut body = serde_json::json!({
                    "id": outcome.id.to_string(),
                    "title": outcome.title,
                    "modified_sections": outcome.modified_sections,
                    "modified_metadata": outcome.modified_metadata,
                    "modified_date": outcome.modified_date,
                    "_hash": outcome.content_hash,
                    "write_id": outcome.write_id,
                    "warnings": outcome.warnings,
                    // Orphan-stub GC: removing a body wiki-link that was
                    // a stub target's last referrer GC's the stub and
                    // lists it here. Always present (empty array when
                    // nothing orphaned), matching the relate-remove
                    // always-emit shape so agents branch uniformly on
                    // the field rather than its presence.
                    "orphan_stubs_removed": outcome
                        .orphan_stubs_removed
                        .iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>(),
                });
                // Add `prospective_hash` only on the dry_run path
                // (matches full's `#[serde(skip_serializing_if = "Option::is_none")]`).
                if let Some(hash) = outcome.prospective_hash {
                    body["prospective_hash"] = serde_json::json!(hash);
                }
                // Present only when the update carried anchors or unsets:
                // whether the sidecar changed (a restated row writes
                // nothing and says so; an earlier plan).
                if let Some(changed) = outcome.anchors_changed {
                    body["anchors_changed"] = serde_json::json!(changed);
                }
                // Surface `relations_declared` when the agent used
                // `declare_relations`. Always-present-when-non-empty
                // wire shape so consumers branch on `.len()` rather
                // than on key presence; `serde(skip_serializing_if =
                // "Vec::is_empty")` on the outcome keeps the
                // no-batch case bytes-identical to pre-feature.
                if !outcome.relations_declared.is_empty() {
                    body["relations_declared"] = serde_json::to_value(&outcome.relations_declared)
                        .unwrap_or(serde_json::Value::Null);
                }
                attach_durability(&mut body, &engine, outcome.id.mem());
                let (engine, finished_notices) = engine.finish();
                attach_mem_changed(&mut body, finished_notices);
                let res = json_response(&body);
                match mem_schema_ref_unified(&engine, &mem_for_anchor) {
                    Some(s) => with_mem_schema_anchor(res, &s),
                    None => res,
                }
            }
            Err(e) => {
                // The notice already rode `structured_content` here;
                // the text channel lacked the `MEM_RELOADED` line a
                // successful response carries. A mutation reloads inside
                // the engine, so reconstruct the warning from the
                // drained notices to match the success channel split —
                // collision (`HASH_MISMATCH`) is the path drift matters
                // most, since it lands on the very entity being written.
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
            }
        }
    }

    #[tool(
        name = "memstead_relate",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_relate(
        &self,
        Parameters(p): Parameters<RelateParams>,
    ) -> CallToolResult {
        if let Some(err) = validate_note(p.note.as_deref()) {
            return err;
        }
        let (role, identity) =
            match self.resolve_call_provenance(p.role.as_deref(), p.identity.as_deref()) {
                Ok(v) => v,
                Err(resp) => return *resp,
            };
        if p.relations.is_empty() {
            let msg = "relations must carry at least one operation";
            return tool_error_with_payload(
                "INVALID_INPUT",
                msg,
                envelope(
                    "INVALID_INPUT",
                    msg.to_string(),
                    serde_json::json!({ "message": msg }),
                ),
            );
        }

        // The whole list is one engine batch: all-or-nothing in one
        // commit per touched mem, in-order validation, report-all
        // refusals. A list of one routes through the same path and
        // carries the same per-entry body today's single call did.
        let ops: Vec<(memstead_base::RelateEntityArgs, Option<String>)> = p
            .relations
            .iter()
            .map(|op| {
                (
                    memstead_base::RelateEntityArgs {
                        source: EntityId::canonical(&op.from),
                        target: EntityId::canonical(&op.to),
                        rel_type: op.rel_type.clone(),
                        remove: op.remove.unwrap_or(false),
                        expected_hash: None,
                        description: op.description.clone(),
                        dry_run: p.dry_run.unwrap_or(false),
                    },
                    p.note.clone(),
                )
            })
            .collect();
        let mem_for_anchor = ops[0].0.source.mem().to_string();

        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        let client = self.client.get().cloned();
        engine.set_role(role);
        engine.set_identity(identity);

        // A list of one routes through the single-op engine path so it
        // behaves byte-identically to the historical single call —
        // full error enrichment (recovery payloads, message text),
        // full warning parity — wrapped in the same plural envelope
        // larger lists produce.
        if p.relations.len() == 1 {
            let (args, note) = {
                let mut it = ops.into_iter();
                it.next().expect("len checked above")
            };
            return match engine.relate_entity(args, Actor::Agent, client.as_ref(), note.as_deref())
            {
                Ok(outcome) => {
                    let action = match outcome.action {
                        memstead_base::RelateAction::Added => "added",
                        memstead_base::RelateAction::Removed => "removed",
                        memstead_base::RelateAction::NoOpAlreadyPresent
                        | memstead_base::RelateAction::NoOpAbsent => "noop",
                    };
                    let mut body = serde_json::json!({
                        "results": [{
                            "from": outcome.from.to_string(),
                            "to": outcome.to.to_string(),
                            "rel_type": outcome.rel_type,
                            "action": action,
                            "source": outcome.source,
                            "_hash": outcome.content_hash,
                        }],
                        "write_id": outcome.write_id,
                        "warnings": outcome.warnings,
                        "orphan_stubs_removed": outcome
                            .orphan_stubs_removed
                            .iter()
                            .map(|i| i.to_string())
                            .collect::<Vec<_>>(),
                    });
                    attach_durability(&mut body, &engine, outcome.from.mem());
                    let (engine, finished_notices) = engine.finish();
                    attach_mem_changed(&mut body, finished_notices);
                    let res = json_response(&body);
                    match mem_schema_ref_unified(&engine, &mem_for_anchor) {
                        Some(sr) => with_mem_schema_anchor(res, &sr),
                        None => res,
                    }
                }
                Err(e) => {
                    let (engine, notices) = engine.finish();
                    let warnings = notices_as_reload_warnings(&notices);
                    attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
                }
            };
        }

        // Snapshot which targets are absent pre-call so applied
        // auto-stubs can surface the same AUTO_STUB_CREATED warning
        // the single call emitted.
        let absent_targets: std::collections::HashSet<String> = p
            .relations
            .iter()
            .filter(|op| !op.remove.unwrap_or(false))
            .map(|op| EntityId::canonical(&op.to))
            .filter(|to| engine.store().get(to).is_none())
            .map(|to| to.to_string())
            .collect();
        let dry_run = p.dry_run.unwrap_or(false);
        let result = match engine.batch_relate(ops, Actor::Agent, client.as_ref(), dry_run) {
            Ok(r) => r,
            Err(e) => {
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                return attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices);
            }
        };

        if !result.applied {
            // Report-all refusal: nothing committed; every failing
            // entry ships its typed envelope. A list of one surfaces
            // its single entry's own code top-level so error-branching
            // callers keep working; larger lists wrap under
            // BATCH_REFUSED with per-entry envelopes in
            // `details.entries`.
            let entries: Vec<serde_json::Value> = result
                .results
                .iter()
                .zip(p.relations.iter())
                .enumerate()
                .map(|(i, (entry, op))| {
                    let mut e = serde_json::json!({
                        "index": i,
                        "from": op.from,
                        "to": op.to,
                        "rel_type": op.rel_type,
                        "action": entry.action,
                    });
                    if let Some(err) = &entry.error {
                        e["code"] = serde_json::json!(err.code);
                        e["message"] = serde_json::json!(err.message);
                        e["details"] = err.details.clone();
                    }
                    e
                })
                .collect();
            let (_engine, notices) = engine.finish();
            let drift = notices_as_reload_warnings(&notices);
            let msg = format!(
                "batch refused — {} of {} operation(s) failed, nothing committed",
                result.failed,
                p.relations.len(),
            );
            let payload = envelope(
                "BATCH_REFUSED",
                msg.clone(),
                serde_json::json!({
                    "entries": entries,
                    "failed": result.failed,
                    "errors_suppressed": result.errors_suppressed,
                }),
            );
            return attach_drift_to_error(
                tool_error_with_payload("BATCH_REFUSED", &msg, payload),
                &drift,
                notices,
            );
        }

        // Applied: enrich every entry with the same fields today's
        // single response carried — canonical rel_type comes back via
        // the store edge, `source` labels body_link vs explicit, and
        // `_hash` is the source entity's post-commit hash (the next
        // valid expected_hash for that entity). Noop entries
        // additionally synthesize the typed no-op warning the single
        // call emitted (DUPLICATE_RELATIONSHIP / NO_SUCH_RELATIONSHIP).
        let mut warnings: Vec<memstead_base::ops::WarningHint> = Vec::new();
        let entries: Vec<serde_json::Value> = result
            .results
            .iter()
            .zip(p.relations.iter())
            .map(|(entry, op)| {
                let from = EntityId::canonical(&op.from);
                let to = EntityId::canonical(&op.to);
                let canonical_type = op.rel_type.to_uppercase();
                if entry.action == "noop" {
                    if op.remove.unwrap_or(false) {
                        warnings.push(memstead_base::ops::WarningHint::NoSuchRelationship {
                            rel_type: canonical_type.clone(),
                            from: from.clone(),
                            to: to.clone(),
                        });
                    } else {
                        warnings.push(memstead_base::ops::WarningHint::DuplicateRelationship {
                            rel_type: canonical_type.clone(),
                            from: from.clone(),
                            to: to.clone(),
                        });
                    }
                }
                // Real batch: the stub exists post-commit. Rehearsal:
                // the staged state rolled back, so a still-absent
                // target of a validated add entry IS the would-be stub
                // — same warning, reported instead of created.
                let stubbed = engine.store().get(&to).map(|e| e.stub).unwrap_or(false);
                let would_stub =
                    dry_run && entry.action == "added" && engine.store().get(&to).is_none();
                if !op.remove.unwrap_or(false)
                    && absent_targets.contains(&to.to_string())
                    && (stubbed || would_stub)
                {
                    // `pending` only on the rehearsal branch — the
                    // rolled-back stub was never written, so the
                    // warning must not claim a performed effect.
                    warnings.push(memstead_base::ops::WarningHint::AutoStubCreated {
                        stub_id: to.clone(),
                        pending: would_stub,
                    });
                }
                let source_label = engine
                    .store()
                    .outgoing(&from)
                    .iter()
                    .find(|e| e.target == to && e.rel_type.eq_ignore_ascii_case(&op.rel_type))
                    .map(|e| match e.source {
                        memstead_base::EdgeSource::BodyLink => "body_link",
                        memstead_base::EdgeSource::Hierarchy => "hierarchy",
                        memstead_base::EdgeSource::Explicit => "explicit",
                    })
                    .unwrap_or("explicit");
                // Real batch: post-commit hash (the next valid
                // `expected_hash`). Rehearsal: the rolled-back store
                // serves the CURRENT on-disk hash — still the value a
                // follow-up real call validates against.
                let hash = engine
                    .store()
                    .get(&from)
                    .map(|e| e.content_hash.clone())
                    .unwrap_or_default();
                serde_json::json!({
                    "from": from.to_string(),
                    "to": to.to_string(),
                    "rel_type": canonical_type,
                    "action": entry.action,
                    "source": source_label,
                    "_hash": hash,
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "results": entries,
            "write_id": result.write_id,
            "warnings": warnings,
            "orphan_stubs_removed": result
                .orphan_stubs_removed
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>(),
        });
        attach_durability(&mut body, &engine, mem_for_anchor.as_str());
        let (engine, finished_notices) = engine.finish();
        attach_mem_changed(&mut body, finished_notices);
        let res = json_response(&body);
        match mem_schema_ref_unified(&engine, &mem_for_anchor) {
            Some(s) => with_mem_schema_anchor(res, &s),
            None => res,
        }
    }

    #[tool(
        name = "memstead_delete",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_delete(
        &self,
        Parameters(p): Parameters<DeleteParams>,
    ) -> CallToolResult {
        if let Some(err) = validate_entity_id(&p.id) {
            return err;
        }
        if let Some(err) = validate_note(p.note.as_deref()) {
            return err;
        }
        let id = EntityId::canonical(&p.id);
        // Empty `expected_hash` is the stub-delete escape hatch —
        // stubs have an empty content_hash so the hash check is
        // skipped. Convert empty string to None at this boundary so
        // engine semantics stay opaque to the request shape.
        let expected_hash_opt = if p.expected_hash.is_empty() {
            None
        } else {
            Some(p.expected_hash.clone())
        };

        let (role, identity) =
            match self.resolve_call_provenance(p.role.as_deref(), p.identity.as_deref()) {
                Ok(v) => v,
                Err(resp) => return *resp,
            };
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        engine.set_role(role);
        engine.set_identity(identity);
        // Capture the schema-ref BEFORE the delete — mem may
        // drop with the last entity.
        let mem_for_anchor = match engine.resolve_entity_id(&id) {
            Ok((resolved, _)) => mem_schema_ref_unified(&engine, resolved.mem()),
            Err(e) => return engine_err_unified(e, &engine),
        };
        let args = memstead_base::DeleteEntityArgs {
            id: id.clone(),
            expected_hash: expected_hash_opt,
        };
        let client = self.client.get().cloned();
        match engine.delete_entity(args, Actor::Agent, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                let mut body = serde_json::json!({
                    "id": outcome.id.to_string(),
                    "relations_removed": outcome.relations_removed,
                    "write_id": outcome.write_id,
                });
                // Full skip-serialises empty `orphan_stubs_removed`;
                // mirror by only adding the field when populated.
                if !outcome.orphan_stubs_removed.is_empty() {
                    body["orphan_stubs_removed"] = serde_json::json!(
                        outcome
                            .orphan_stubs_removed
                            .iter()
                            .map(|i| i.to_string())
                            .collect::<Vec<_>>()
                    );
                }
                attach_durability(&mut body, &engine, id.mem());
                let (_engine, finished_notices) = engine.finish();
                attach_mem_changed(&mut body, finished_notices);
                let mut res = json_response(&body);
                if let Some(s) = mem_for_anchor.as_deref() {
                    res = with_mem_schema_anchor(res, s);
                }
                // Surface every engine-emitted warning on the outcome
                // (residual-stub demotion, and the `NOTE_MISSING`
                // provenance nudge the engine now emits when
                // `require_notes` is set and a commit landed).
                for w in &outcome.warnings {
                    res = append_warning_hint(res, w);
                }
                res
            }
            Err(e) => {
                // The notice already rode `structured_content` here;
                // the text channel lacked the `MEM_RELOADED` line a
                // successful response carries. A mutation reloads inside
                // the engine, so reconstruct the warning from the
                // drained notices to match the success channel split —
                // collision (`HASH_MISMATCH`) is the path drift matters
                // most, since it lands on the very entity being written.
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
            }
        }
    }

    #[tool(
        name = "memstead_check",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_check(&self, Parameters(p): Parameters<CheckParams>) -> CallToolResult {
        if let Some(err) = validate_entity_id(&p.entity) {
            return err;
        }
        let id = EntityId::canonical(&p.entity);
        let Some(verdict) = memstead_base::check::Verdict::from_wire(&p.verdict) else {
            let msg = format!(
                "unknown verdict {:?} — the vocabulary is: {}",
                p.verdict,
                memstead_base::check::VERDICTS.join(", ")
            );
            return tool_error_with_payload(
                "INVALID_VERDICT",
                &msg,
                envelope(
                    "INVALID_VERDICT",
                    msg.clone(),
                    serde_json::json!({ "allowed": memstead_base::check::VERDICTS }),
                ),
            );
        };
        let kind = match p.kind.as_deref() {
            None => memstead_base::check::RecordKind::Engine(
                memstead_base::check::CheckKind::Verification,
            ),
            Some(s) => match memstead_base::check::RecordKind::from_wire(s) {
                Some(k) => k,
                None => {
                    let msg = format!(
                        "unknown check kind {s:?} — the vocabulary is: {}",
                        memstead_base::check::RecordKind::vocabulary_hint()
                    );
                    return tool_error_with_payload(
                        "INVALID_CHECK_KIND",
                        &msg,
                        envelope(
                            "INVALID_CHECK_KIND",
                            msg.clone(),
                            serde_json::json!({
                                "allowed": memstead_base::check::CHECK_KINDS,
                                "foreign_prefix": memstead_base::check::FOREIGN_KIND_PREFIX,
                            }),
                        ),
                    );
                }
            },
        };
        let finding = match p.finding.as_ref() {
            None => None,
            Some(f) => match memstead_base::check::CheckFinding::from_json(
                serde_json::to_value(f).unwrap_or(serde_json::Value::Null),
            ) {
                Ok(f) => Some(f),
                Err(reason) => {
                    return tool_error_with_payload(
                        memstead_base::check::INVALID_CHECK_FINDING_CODE,
                        &reason,
                        envelope(
                            memstead_base::check::INVALID_CHECK_FINDING_CODE,
                            reason.clone(),
                            serde_json::json!({ "shape": memstead_base::check::CheckFinding::SHAPE }),
                        ),
                    );
                }
            },
        };
        let (role, identity) =
            match self.resolve_call_provenance(p.role.as_deref(), p.identity.as_deref()) {
                Ok(v) => v,
                Err(resp) => return *resp,
            };

        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        let _drift = engine.reload_if_stale(Some(id.mem()));
        engine.set_role(role);
        engine.set_identity(identity);
        let client = self.client.get().cloned();
        match engine.record_check_with(
            id.mem(),
            id.as_ref(),
            verdict,
            &kind,
            p.method.as_deref(),
            finding,
            Actor::Agent,
            client.as_ref(),
        ) {
            Ok(record) => {
                // A foreign kind moves no state: the response reports the
                // verification state, unchanged by this record.
                let state_result = match kind.engine_kind() {
                    Some(memstead_base::check::CheckKind::Conformance) => {
                        engine.entity_conformance_state(id.mem(), id.as_ref())
                    }
                    _ => engine.entity_check_state(id.mem(), id.as_ref()),
                };
                let (state, _) = match state_result {
                    Ok(pair) => pair,
                    Err(e) => return engine_err_unified(e, &engine),
                };
                let body = serde_json::json!({
                    "entity": record.entity,
                    "verdict": record.verdict,
                    "check_state": state.as_str(),
                    "kind": record.kind.as_deref().unwrap_or("verification"),
                    "schema_ref": record.schema_ref,
                    "role": record.role,
                    "identity": record.identity,
                    "ts": record.ts,
                    "method": record.method,
                    "finding": record.finding,
                });
                md_with_structured(
                    format!(
                        "Check recorded: {} — kind {}, verdict {}, state {}",
                        record.entity,
                        record.kind.as_deref().unwrap_or("verification"),
                        record.verdict,
                        state.as_str()
                    ),
                    body,
                )
            }
            Err(e) => engine_err_unified(e, &engine),
        }
    }

    #[tool(
        name = "memstead_rename",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_rename(
        &self,
        Parameters(p): Parameters<RenameParams>,
    ) -> CallToolResult {
        if let Some(err) = validate_entity_id(&p.id) {
            return err;
        }
        if let Some(err) = validate_note(p.note.as_deref()) {
            return err;
        }
        let id = EntityId::canonical(&p.id);
        let (role, identity) =
            match self.resolve_call_provenance(p.role.as_deref(), p.identity.as_deref()) {
                Ok(v) => v,
                Err(resp) => return *resp,
            };

        // Unified outcome exposes `old_path` / `new_path` directly
        // (matching full's `RenameResult` shape).
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        engine.set_role(role);
        engine.set_identity(identity);
        let mem_for_anchor = match engine.resolve_entity_id(&id) {
            Ok((resolved, _)) => resolved.mem().to_string(),
            Err(e) => return engine_err_unified(e, &engine),
        };
        let args = memstead_base::RenameEntityArgs {
            id: id.clone(),
            expected_hash: Some(p.expected_hash.clone()),
            new_title: p.new_title.clone(),
        };
        let client = self.client.get().cloned();
        match engine.rename_entity(args, Actor::Agent, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                let mut body = serde_json::json!({
                    "old_id": outcome.old_id.to_string(),
                    "new_id": outcome.new_id.to_string(),
                    "old_path": outcome.old_path,
                    "new_path": outcome.new_path,
                    "_hash": outcome.content_hash,
                    "write_id": outcome.write_id,
                    "warnings": outcome.warnings,
                });
                attach_durability(&mut body, &engine, id.mem());
                let (engine, finished_notices) = engine.finish();
                attach_mem_changed(&mut body, finished_notices);
                let res = json_response(&body);
                match mem_schema_ref_unified(&engine, &mem_for_anchor) {
                    Some(s) => with_mem_schema_anchor(res, &s),
                    None => res,
                }
            }
            Err(e) => {
                // The notice already rode `structured_content` here;
                // the text channel lacked the `MEM_RELOADED` line a
                // successful response carries. A mutation reloads inside
                // the engine, so reconstruct the warning from the
                // drained notices to match the success channel split —
                // collision (`HASH_MISMATCH`) is the path drift matters
                // most, since it lands on the very entity being written.
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
            }
        }
    }

    #[tool(
        name = "memstead_retype",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_retype(
        &self,
        Parameters(p): Parameters<RetypeParams>,
    ) -> CallToolResult {
        if let Some(err) = validate_entity_id(&p.id) {
            return err;
        }
        if let Some(err) = validate_note(p.note.as_deref()) {
            return err;
        }
        let dry_run = p.dry_run.unwrap_or(false);
        if !dry_run && p.expected_hash.as_deref().is_none_or(str::is_empty) {
            let msg = "`expected_hash` is required for a real retype (read the entity first and \
                       pass its `_hash`); only `dry_run: true` may omit it";
            return tool_error_with_payload(
                "INVALID_INPUT",
                msg,
                envelope(
                    "INVALID_INPUT",
                    msg.to_string(),
                    serde_json::json!({ "field": "expected_hash" }),
                ),
            );
        }
        let id = EntityId::canonical(&p.id);
        let (role, identity) =
            match self.resolve_call_provenance(p.role.as_deref(), p.identity.as_deref()) {
                Ok(v) => v,
                Err(resp) => return *resp,
            };
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        engine.set_role(role);
        engine.set_identity(identity);
        let mem_for_anchor = match engine.resolve_entity_id(&id) {
            Ok((resolved, _)) => resolved.mem().to_string(),
            Err(e) => return engine_err_unified(e, &engine),
        };
        let args = memstead_base::RetypeEntityArgs {
            id: id.clone(),
            expected_hash: p.expected_hash.clone(),
            target_type: p.target_type.clone(),
            section_map: p
                .section_map
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            drop_metadata: p.drop_metadata.clone().unwrap_or_default(),
            dry_run,
        };
        let client = self.client.get().cloned();
        match engine.retype_entity(args, Actor::Agent, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                let mut body = serde_json::to_value(&outcome).unwrap_or(serde_json::Value::Null);
                if let Some(obj) = body.as_object_mut() {
                    obj.insert("dry_run".into(), serde_json::json!(dry_run));
                }
                attach_durability(&mut body, &engine, id.mem());
                let (engine, finished_notices) = engine.finish();
                attach_mem_changed(&mut body, finished_notices);
                let res = json_response(&body);
                match mem_schema_ref_unified(&engine, &mem_for_anchor) {
                    Some(s) => with_mem_schema_anchor(res, &s),
                    None => res,
                }
            }
            Err(e) => {
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
            }
        }
    }

    #[tool(
        name = "memstead_reload",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_reload(
        &self,
        Parameters(p): Parameters<ReloadParams>,
    ) -> CallToolResult {
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        // Full mode: the additive schema-source + mount-manifest
        // re-scan runs FIRST (so newly registered mems join the
        // content sweep below), then the ordinary workspace-wide
        // content reload — coherence (MEM_RELOADED, expected_hash
        // discipline) rides the same content-reload path it always
        // did. Per-item refresh failures ride the `refresh` block;
        // they never abort the content reload.
        let refresh = if p.full.unwrap_or(false) {
            if p.mem.is_some() {
                let msg = "`full: true` is workspace-scoped — omit `mem`".to_string();
                return tool_error_with_payload(
                    "INVALID_INPUT",
                    &msg,
                    envelope(
                        "INVALID_INPUT",
                        msg.clone(),
                        serde_json::json!({ "message": msg }),
                    ),
                );
            }
            Some(engine.full_refresh())
        } else {
            None
        };
        let result = match p.mem.as_deref() {
            Some(name) => engine.reload_one_mem_report(name).map(|r| vec![r]),
            None => engine.reload_each_writable_mem_reports(),
        };
        match result {
            Ok(reports) => {
                let mut payload = serde_json::json!({ "reports": reports });
                if let Some(refresh) = refresh {
                    payload["refresh"] =
                        serde_json::to_value(&refresh).unwrap_or(serde_json::Value::Null);
                }
                json_response(&payload)
            }
            // `engine_err_unified` reads `EngineError::code()` for the
            // underlying variant so the wire envelope here carries the
            // same typed token the rest of the surface emits for the
            // same fire condition — `MEM_ERROR` for backend wraps,
            // `UNKNOWN_MEM` for missing-mem, etc.
            Err(e) => engine_err_unified(e, &engine),
        }
    }
}
