//! The mem-lifecycle tools: create, delete, configure and re-pin a mem, plus the unified variants that sit with their tool.

use super::*;

#[tool_router(router = mem_tool_router, vis = "pub(crate)")]
impl McpServer {
    // ----------------------------------------------------------------------
    // Mem lifecycle tools
    // ----------------------------------------------------------------------

    #[tool(
        name = "memstead_mem_create",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_mem_create(
        &self,
        Parameters(p): Parameters<crate::lifecycle::MemCreateParams>,
    ) -> CallToolResult {
        // Collisions surface via the snapshot probe with the
        // `{name, source}` envelope (callers branch on `code` only).
        let unified = self.unified_engine();
        self.memstead_mem_create_unified(p, unified.clone())
    }

    #[tool(
        name = "memstead_mem_delete",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_mem_delete(
        &self,
        Parameters(p): Parameters<crate::lifecycle::MemDeleteParams>,
    ) -> CallToolResult {
        let unified = self.unified_engine();
        self.memstead_mem_delete_unified(p, unified.clone())
    }

    #[tool(
        name = "memstead_mem_set_version",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_mem_set_version(
        &self,
        Parameters(p): Parameters<crate::lifecycle::MemSetVersionParams>,
    ) -> CallToolResult {
        let new_version = match semver::Version::parse(&p.version) {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("version {:?} is not a valid semver: {e}", p.version);
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
        match engine.set_mem_version(&p.name, new_version, p.note.as_deref()) {
            Ok(outcome) => {
                let mut body = serde_json::json!({
                    "mem": outcome.mem,
                    "old_version": outcome.old_version.map(|v| v.to_string()),
                    "new_version": outcome.new_version.to_string(),
                    "warnings": outcome.warnings,
                });
                let (_engine, finished_notices) = engine.finish();
                attach_mem_changed(&mut body, finished_notices);
                json_response(&body)
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
        name = "memstead_mem_configure",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_mem_configure(
        &self,
        Parameters(p): Parameters<crate::lifecycle::MemConfigureParams>,
    ) -> CallToolResult {
        if let Some(err) = validate_note(p.note.as_deref()) {
            return err;
        }
        if p.subject.is_some() && p.clear_subject {
            let msg = "`subject` and `clear_subject` are mutually exclusive — pass one";
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
        let (role, identity) =
            match self.resolve_call_provenance(p.role.as_deref(), p.identity.as_deref()) {
                Ok(v) => v,
                Err(resp) => return *resp,
            };
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        engine.set_role(role);
        engine.set_identity(identity);
        let mut warnings: Vec<memstead_base::ops::WarningHint> = Vec::new();

        // "Set what is present": each Some field routes through the
        // engine setter the CLI verb uses; empty string clears.
        if let Some(t) = p.title.clone() {
            let value = if t.is_empty() { None } else { Some(t) };
            match engine.set_mem_title(&p.name, value, p.note.as_deref()) {
                Ok(o) => warnings.extend(o.warnings),
                Err(e) => {
                    let (engine, notices) = engine.finish();
                    let drift = notices_as_reload_warnings(&notices);
                    return attach_drift_to_error(engine_err_unified(e, &engine), &drift, notices);
                }
            }
        }
        if let Some(d) = p.description.clone() {
            let value = if d.is_empty() { None } else { Some(d) };
            match engine.set_mem_description(&p.name, value, p.note.as_deref()) {
                Ok(o) => warnings.extend(o.warnings),
                Err(e) => {
                    let (engine, notices) = engine.finish();
                    let drift = notices_as_reload_warnings(&notices);
                    return attach_drift_to_error(engine_err_unified(e, &engine), &drift, notices);
                }
            }
        }
        if p.subject.is_some() || p.clear_subject {
            let value = p.subject.clone().map(|s| s.into_engine());
            match engine.set_mem_subject(&p.name, value, p.note.as_deref()) {
                Ok(o) => warnings.extend(o.warnings),
                Err(e) => {
                    let (engine, notices) = engine.finish();
                    let drift = notices_as_reload_warnings(&notices);
                    return attach_drift_to_error(engine_err_unified(e, &engine), &drift, notices);
                }
            }
        }

        // No-field calls still validate the mem exists (a pure no-op
        // against an unknown name would be a silent lie).
        let no_field =
            p.title.is_none() && p.description.is_none() && p.subject.is_none() && !p.clear_subject;
        if no_field && engine.mount(&p.name).is_none() {
            let e = engine.unknown_mem_error(&p.name);
            let (engine, notices) = engine.finish();
            let drift = notices_as_reload_warnings(&notices);
            return attach_drift_to_error(engine_err_unified(e, &engine), &drift, notices);
        }

        // Post-call state from the loaded config — the stable response
        // shape regardless of which fields were touched.
        let config = engine
            .mem_configs_named()
            .find(|(name, _)| *name == p.name)
            .map(|(_, c)| c.clone());
        let mut body = serde_json::json!({
            "mem": p.name,
            "title": config.as_ref().and_then(|c| c.title.clone()),
            "description": config.as_ref().and_then(|c| c.description.clone()),
            "subject": config.as_ref().and_then(|c| c.subject.as_ref().map(|sub| serde_json::json!({
                "scope": sub.scope,
                "method": sub.method,
                "exclusions": sub.exclusions,
            }))),
            "warnings": warnings,
        });
        let (_engine, finished_notices) = engine.finish();
        attach_mem_changed(&mut body, finished_notices);
        json_response(&body)
    }

    #[tool(
        name = "memstead_mem_set_schema",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_mem_set_schema(
        &self,
        Parameters(p): Parameters<crate::lifecycle::MemSetSchemaParams>,
    ) -> CallToolResult {
        let target = match p.schema.parse::<memstead_schema::SchemaRef>() {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("invalid schema ref {:?}: {e}", p.schema);
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
        match engine.set_mem_schema(&p.mem, &target) {
            Ok(outcome) => {
                let mut body = serde_json::to_value(&outcome).expect("SetSchemaOutcome serialises");
                let (_engine, finished_notices) = engine.finish();
                attach_mem_changed(&mut body, finished_notices);
                json_response(&body)
            }
            Err(e) => {
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
            }
        }
    }

    /// Body of [`Self::memstead_mem_create`].
    pub(super) fn memstead_mem_create_unified(
        &self,
        p: crate::lifecycle::MemCreateParams,
        unified: Arc<Mutex<memstead_base::Engine>>,
    ) -> CallToolResult {
        let (role, identity) =
            match self.resolve_call_provenance(p.role.as_deref(), p.identity.as_deref()) {
                Ok(v) => v,
                Err(resp) => return *resp,
            };
        let mut engine = crate::lock_engine!(unified);
        // The call's role and identity (or the session's) ride the seed
        // commit like every entity mutation's commit.
        engine.set_role(role);
        engine.set_identity(identity);

        let schema_ref = match p.schema.parse::<memstead_schema::SchemaRef>() {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("invalid schema ref {:?}: {e}", p.schema);
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
        };

        // Resolve the inlined-schema verbosity up front — *before* the
        // create side-effect — so a bad value refuses cleanly rather than
        // after the mem has already landed on disk. Only meaningful when
        // `include_schema` is set; ignored otherwise per the param
        // contract (so a moot typo doesn't sink an otherwise-valid create).
        let schema_verbosity = if p.include_schema {
            match p.schema_verbosity.as_deref() {
                // Absent → lite, mirroring `memstead_schema`'s default so
                // the inlined body stays byte-identical to that tool's
                // default reply.
                None => render::SchemaVerbosity::Lite,
                Some(v) => match render::SchemaVerbosity::from_wire(v) {
                    Some(sv) => sv,
                    None => {
                        let msg = format!(
                            "unknown schema_verbosity: \"{v}\" — expected \"full\" or \"lite\""
                        );
                        return tool_error_with_payload(
                            "INVALID_INPUT",
                            &msg,
                            envelope(
                                "INVALID_INPUT",
                                msg.clone(),
                                serde_json::json!({
                                    "value": v,
                                    "allowed": ["full", "lite"],
                                }),
                            ),
                        );
                    }
                },
            }
        } else {
            render::SchemaVerbosity::Full
        };

        // Curation fields ride the create call and are applied through
        // the same setters the CLI verbs use, after the create lands.
        let cur_title = p.title.clone().filter(|t| !t.is_empty());
        let cur_description = p.description.clone().filter(|d| !d.is_empty());
        let cur_subject = p.subject.clone();
        let cur_note = p.note.clone();

        // Hierarchical paths are first-class. The separate `path`
        // wire-shape field retired; `name` carries the full
        // identifier (`team/sub-mem`) verbatim.
        let params = memstead_base::mem_management::MemCreateParams {
            name: p.name,
            location: std::path::PathBuf::from(p.location),
            schema_ref,
            vcs: p.vcs.map(Into::into),
            note: p.note,
            operator_mode: self.operator_mode,
            // Forward the optional
            // recovery action from the MCP wire shape. Bare creates
            // pass `None` and route via the tombstone-driven
            // default; explicit values (`reattach` /
            // `force_overwrite` / `hard_cleanup_first`) override.
            recovery: p.recovery.map(Into::into),
            // Optional per-instance writing guidance from the wire
            // shape, forwarded opaquely into the seed config.
            write_guidance: p.write_guidance,
            actor: Actor::Agent,
            client: self.client.get().cloned(),
            // The MCP wire shape does not expose the storage override
            // yet — the workspace-shape heuristic keeps behaviour
            // identical.
            storage: None,
        };

        match memstead_base::mem_management::create_mem(&mut engine, params) {
            Ok(response) => {
                // Apply curation at creation — same setters, same
                // validation, same storage as the CLI verbs. Each is
                // its own config commit; a failure here surfaces after
                // the mem exists (the create itself has no implicit
                // rollback, matching the seed-commit contract).
                let mut curation_warnings: Vec<memstead_base::ops::WarningHint> = Vec::new();
                if let Some(t) = cur_title {
                    match engine.set_mem_title(&response.name, Some(t), cur_note.as_deref()) {
                        Ok(o) => curation_warnings.extend(o.warnings),
                        Err(e) => return engine_err_unified(e, &engine),
                    }
                }
                if let Some(d) = cur_description {
                    match engine.set_mem_description(&response.name, Some(d), cur_note.as_deref()) {
                        Ok(o) => curation_warnings.extend(o.warnings),
                        Err(e) => return engine_err_unified(e, &engine),
                    }
                }
                if let Some(subj) = cur_subject {
                    match engine.set_mem_subject(
                        &response.name,
                        Some(subj.into_engine()),
                        cur_note.as_deref(),
                    ) {
                        Ok(o) => curation_warnings.extend(o.warnings),
                        Err(e) => return engine_err_unified(e, &engine),
                    }
                }

                // Build wire response with the same shape full emits.
                let body = serde_json::json!({
                    "name": response.name,
                    "location": response.location,
                    "schema_ref": response.schema_ref.to_string(),
                    "seed_write_id": response.seed_write_id,
                });
                // The engine ships the
                // `MEM_REATTACHED_AFTER_UNREGISTER` warning on the
                // create response. Surface every response-side warning
                // via `append_warning_hint` so MCP callers see the
                // structured envelope alongside the success payload.
                let mut create_warnings: Vec<memstead_base::ops::WarningHint> =
                    response.warnings.clone();
                create_warnings.extend(curation_warnings);
                let res = json_response(&body);
                let res = create_warnings.iter().fold(res, append_warning_hint);
                // Inline the
                // full schema body only when the caller opts in via
                // `include_schema: true`. Otherwise every successful
                // create would ship ~25 KB of schema body, even for the
                // agent's second+ mem on the same schema where the
                // value is workspace-stable and already cached.
                let schema_payload = if p.include_schema {
                    engine.schemas().get(&response.name).cloned().map(|s| {
                        let origin = engine.schema_origin(&s);
                        render::build_schema_payload(
                            &s,
                            vec![response.name.clone()],
                            schema_verbosity,
                            origin,
                        )
                    })
                } else {
                    None
                };
                let res = if let Some(payload) = schema_payload {
                    let mut res = res;
                    if let Some(sc) = res.structured_content.as_mut()
                        && let Some(obj) = sc.as_object_mut()
                    {
                        obj.insert("schema".to_string(), payload);
                        if let Ok(text) = serde_json::to_string_pretty(&*sc) {
                            res.content = vec![rmcp::model::ContentBlock::text(text)];
                        }
                    }
                    res
                } else {
                    res
                };
                match mem_schema_ref_unified(&engine, &response.name) {
                    Some(s) => with_mem_schema_anchor(res, &s),
                    None => res,
                }
            }
            Err(e) => full_engine_err_unified(e, &engine),
        }
    }

    /// Unified-engine path for [`Self::memstead_mem_delete`].
    pub(super) fn memstead_mem_delete_unified(
        &self,
        p: crate::lifecycle::MemDeleteParams,
        unified: Arc<Mutex<memstead_base::Engine>>,
    ) -> CallToolResult {
        let (role, identity) =
            match self.resolve_call_provenance(p.role.as_deref(), p.identity.as_deref()) {
                Ok(v) => v,
                Err(resp) => return *resp,
            };
        let mut engine = crate::lock_engine!(unified);
        // The call's role and identity (or the session's) ride the
        // prune commit like every entity mutation's commit; without
        // this the engine carried whatever the previous call left on it.
        engine.set_role(role);
        engine.set_identity(identity);

        // MCP `memstead_mem_delete`
        // always means destructive. The wire shape no longer exposes
        // `delete_files`; the wrapper hardcodes `true` so the engine
        // runs both refusal gates (`MEM_REFERENCED_BY_POLICY`,
        // `MEM_HAS_INCOMING_REFS`) and the policy scrub on success.
        let params = memstead_base::mem_management::MemDeleteParams {
            name: p.name,
            delete_files: true,
            note: p.note,
            actor: Actor::Agent,
            client: self.client.get().cloned(),
            operator_mode: self.operator_mode,
            detach_incoming: false,
        };

        // Snapshot the schema-ref BEFORE the delete so the response
        // can still anchor to the now-departed mem's schema.
        let mem_for_anchor = mem_schema_ref_unified(&engine, &params.name);

        match memstead_base::mem_management::delete_mem(&mut engine, params) {
            Ok(response) => {
                let body = serde_json::json!({
                    "name": response.name,
                    "deleted_from_router": response.deleted_from_router,
                    "files_deleted": response.files_deleted,
                    // Surface scrubbed
                    // `.memstead/workspace.toml` entries so the agent
                    // doesn't have to re-read `workspace show` to
                    // learn the policy side effects of the delete.
                    "allowlist_entries_removed": &response.allowlist_entries_removed,
                });
                let res = json_response(&body);
                let res = match mem_for_anchor {
                    Some(s) => with_mem_schema_anchor(res, &s),
                    None => res,
                };
                // Surface every engine-emitted warning: disk-cleanup
                // outcome (rmdir_failed / backend_prune_failed) and the
                // `NOTE_MISSING` provenance nudge the engine now emits
                // when `require_notes` is set.
                let mut res = res;
                for w in &response.warnings {
                    res = append_warning_hint(res, w);
                }
                res
            }
            Err(e) => full_engine_err_unified(e, &engine),
        }
    }
}
