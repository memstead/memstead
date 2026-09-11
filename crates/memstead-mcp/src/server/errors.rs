//! Error translation: the exhaustive `EngineError` to typed-envelope mapping both handler flavours share, with the not-found and did-you-mean helpers that dress it.

use super::*;

/// Convert an `EngineError` to a tool error response. Exhaustive over every
/// variant — each case produces a `{code, message, details}` envelope on
/// `structured_content` so agents can branch on the stable `UPPER_SNAKE_CASE`
/// Typed-envelope translator for the unified `memstead_base::EngineError`.
/// Mutation handlers' unified branches return this when the engine
/// surfaces an error so the wire shape carries a stable `code` and
/// (where applicable) the recovery `details` payload full callers
/// already branch on.
///
/// The match is exhaustive — no wildcard arm. Every `EngineError`
/// variant maps to a typed envelope; adding a new variant fails
/// compilation here until an arm picks a code (and, when applicable,
/// a structured details payload). The compiler is the forcing
/// function. An earlier shape carried
/// a `_ => INTERNAL` arm that silently swallowed `DescriptionNotPermitted`,
/// `WikiLinkWithoutRelation`, `MissingRequiredDescription`, and several
/// rename/parse-path variants — trained agents to ignore `INTERNAL`
/// even when the underlying error was user-recoverable. The exhaustive
/// match makes that regression class structurally impossible: every
/// future `EngineError` variant must declare its wire shape here
/// before it can land.
pub(super) fn engine_err_unified(
    e: memstead_base::EngineError,
    engine: &memstead_base::Engine,
) -> CallToolResult {
    use memstead_base::EngineError as E;
    // The text-channel message uses the rich-prose renderer so an agent
    // reading only `result.content[0].text` sees the full recovery
    // payload inline rather than a "+N more — see details.X" pointer
    // to a structured channel the MCP client doesn't surface. The
    // structured payload below is unchanged.
    let message = e.prose_render();
    match &e {
        E::NotFound { id } => {
            // #55: attach `suggestions` so this generic mapper carries the
            // same recovery detail as the dedicated `not_found_error`, no
            // matter which internal path raised the error.
            let suggestions = suggest_similar(engine.store(), id);
            tool_error_with_payload(
                "ENTITY_NOT_FOUND",
                &message,
                envelope(
                    "ENTITY_NOT_FOUND",
                    message.clone(),
                    serde_json::json!({ "id": id, "suggestions": suggestions }),
                ),
            )
        }
        E::EntityIdMissingMem { .. } => tool_error_with_payload(
            "ENTITY_ID_MISSING_MEM",
            &message,
            envelope("ENTITY_ID_MISSING_MEM", message.clone(), e.details()),
        ),
        E::AlreadyExists { .. } => tool_error_with_payload(
            "ENTITY_ALREADY_EXISTS",
            &message,
            // Payload comes from `details()` so the occupying title
            // cannot drift between the CLI and MCP envelopes.
            envelope("ENTITY_ALREADY_EXISTS", message.clone(), e.details()),
        ),
        // Block-tier declared-constraint refusals — code and recovery
        // payload come from the error itself (`code()` / `details()`),
        // so the envelope stays aligned with the CLI `--json` shape.
        // Code and payload from the error itself, so the
        // buried-section list reaches the agent identically on both surfaces.
        E::UnterminatedFenceInStoredBody { .. } => tool_error_with_payload(
            "UNTERMINATED_FENCE_IN_STORED_BODY",
            &message,
            envelope(
                "UNTERMINATED_FENCE_IN_STORED_BODY",
                message.clone(),
                e.details(),
            ),
        ),
        E::ConstraintUnsatisfied { .. }
        | E::RequiredOutgoingUnsatisfied { .. }
        | E::SectionFormatRefused { .. } => tool_error_with_payload(
            e.code(),
            &message,
            envelope(e.code(), message.clone(), e.details()),
        ),
        E::HashMismatch {
            id,
            current,
            is_stub,
        } => tool_error_with_payload(
            "HASH_MISMATCH",
            &message,
            envelope(
                "HASH_MISMATCH",
                message.clone(),
                serde_json::json!({
                    "id": id,
                    "current": current,
                    "is_stub": is_stub,
                }),
            ),
        ),
        E::UnknownMem(name) => {
            // #55: attach `known_mems` so this generic mapper matches the
            // dedicated mem-not-found handler regardless of which path
            // raised the error.
            let known_mems: Vec<String> = engine.mounts().iter().map(|m| m.mem.clone()).collect();
            tool_error_with_payload(
                "UNKNOWN_MEM",
                &message,
                envelope(
                    "UNKNOWN_MEM",
                    message.clone(),
                    serde_json::json!({ "name": name, "known_mems": known_mems }),
                ),
            )
        }
        E::MemUnmounted { mem } => tool_error_with_payload(
            "MEM_UNMOUNTED",
            &message,
            envelope(
                "MEM_UNMOUNTED",
                message.clone(),
                serde_json::json!({ "mem": mem }),
            ),
        ),
        E::MemQuarantined {
            mem,
            reason_code,
            reason_message,
        } => tool_error_with_payload(
            "MEM_QUARANTINED",
            &message,
            envelope(
                "MEM_QUARANTINED",
                message.clone(),
                serde_json::json!({
                    "mem": mem,
                    "reason_code": reason_code,
                    "reason_message": reason_message,
                }),
            ),
        ),
        E::UnknownRef(raw) => tool_error_with_payload(
            "UNKNOWN_REF",
            &message,
            envelope(
                "UNKNOWN_REF",
                message.clone(),
                serde_json::json!({ "ref": raw }),
            ),
        ),
        E::BranchResetHeadMoved {
            mem,
            expected,
            current,
        } => tool_error_with_payload(
            "BRANCH_RESET_HEAD_MOVED",
            &message,
            envelope(
                "BRANCH_RESET_HEAD_MOVED",
                message.clone(),
                serde_json::json!({
                    "mem": mem,
                    "expected": expected,
                    "current": current,
                }),
            ),
        ),
        E::PushedCommitsProtected {
            mem,
            target_sha,
            pushed_shas,
        } => tool_error_with_payload(
            "PUSHED_COMMITS_PROTECTED",
            &message,
            envelope(
                "PUSHED_COMMITS_PROTECTED",
                message.clone(),
                serde_json::json!({
                    "mem": mem,
                    "target_sha": target_sha,
                    "pushed_shas": pushed_shas,
                }),
            ),
        ),
        E::UnknownRemote(name) => tool_error_with_payload(
            "UNKNOWN_REMOTE",
            &message,
            envelope(
                "UNKNOWN_REMOTE",
                message.clone(),
                serde_json::json!({ "remote": name }),
            ),
        ),
        E::LocalDivergence { mem, remote_ref } => tool_error_with_payload(
            "LOCAL_DIVERGENCE",
            &message,
            envelope(
                "LOCAL_DIVERGENCE",
                message.clone(),
                serde_json::json!({ "mem": mem, "remote_ref": remote_ref }),
            ),
        ),
        E::NonFastForward { mem, remote } => tool_error_with_payload(
            "NON_FAST_FORWARD",
            &message,
            envelope(
                "NON_FAST_FORWARD",
                message.clone(),
                serde_json::json!({ "mem": mem, "remote": remote }),
            ),
        ),
        E::LocalInvalidState {
            mem,
            remote,
            detail,
        } => tool_error_with_payload(
            "LOCAL_INVALID_STATE",
            &message,
            envelope(
                "LOCAL_INVALID_STATE",
                message.clone(),
                serde_json::json!({
                    "mem": mem,
                    "remote": remote,
                    "detail": detail,
                }),
            ),
        ),
        E::SchemaViolationInFetch {
            mem,
            ref_name,
            violations,
        } => tool_error_with_payload(
            "SCHEMA_VIOLATION_IN_FETCH",
            &message,
            envelope(
                "SCHEMA_VIOLATION_IN_FETCH",
                message.clone(),
                serde_json::json!({
                    "mem": mem,
                    "ref": ref_name,
                    "violations": violations,
                }),
            ),
        ),
        E::ReadOnlyMount(mem) => tool_error_with_payload(
            "READ_ONLY_MOUNT",
            &message,
            envelope(
                "READ_ONLY_MOUNT",
                message.clone(),
                serde_json::json!({ "mem": mem }),
            ),
        ),
        E::CheckNotRecorded { reason } => tool_error_with_payload(
            "CHECK_NOT_RECORDED",
            &message,
            envelope(
                "CHECK_NOT_RECORDED",
                message.clone(),
                serde_json::json!({ "reason": reason }),
            ),
        ),
        E::UnknownType {
            name,
            schema_ref,
            declared,
            suggestion,
        } => tool_error_with_payload(
            "UNKNOWN_ENTITY_TYPE",
            &message,
            envelope(
                "UNKNOWN_ENTITY_TYPE",
                message.clone(),
                serde_json::json!({
                    "name": name,
                    "schema_ref": schema_ref,
                    "declared": declared,
                    "suggestion": suggestion,
                }),
            ),
        ),
        E::HasIncomingRefs { id, referrers } => {
            // Project each ReferrerInfo into the wire shape the
            // memstead_delete description advertises: `{ from_id,
            // rel_types, mem, capability: "write" }`. The capability
            // is constant on this path — only Write-Mem referrers
            // ever surface here (ReadOnly referrers ride the
            // residual-stub demotion path). Per-source dedup happens
            // upstream: `rel_types` carries every edge
            // type from this source to the deletion target.
            let referrers_json: Vec<_> = referrers
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "from_id": r.from_id,
                        "rel_types": r.rel_types,
                        "mem": r.mem,
                        "capability": "write",
                    })
                })
                .collect();
            tool_error_with_payload(
                e.code(),
                &message,
                envelope(
                    e.code(),
                    message.clone(),
                    serde_json::json!({ "id": id, "referrers": referrers_json }),
                ),
            )
        }
        E::MemHasIncomingRefs { mem, referrers } => {
            // Mem-level mirror of HasIncomingRefs (F15 / CLI F8): the
            // mem-delete edge-graph check. Same `{from_id,
            // rel_types, mem}` projection; capability is omitted
            // because the mem-level check already filtered to
            // Write-Mem sources upstream.
            let referrers_json: Vec<_> = referrers
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "from_id": r.from_id,
                        "rel_types": r.rel_types,
                        "mem": r.mem,
                    })
                })
                .collect();
            tool_error_with_payload(
                e.code(),
                &message,
                envelope(
                    e.code(),
                    message.clone(),
                    serde_json::json!({ "mem": mem, "referrers": referrers_json }),
                ),
            )
        }
        E::CrossMemLinkNotAllowed { from_mem, to_mem } => tool_error_with_payload(
            e.code(),
            &message,
            envelope(
                e.code(),
                message.clone(),
                serde_json::json!({
                    "from_mem": from_mem,
                    "to_mem": to_mem,
                }),
            ),
        ),
        E::CrossMemTargetNotFound {
            target_id,
            target_mem,
        } => tool_error_with_payload(
            e.code(),
            &message,
            envelope(
                e.code(),
                message.clone(),
                serde_json::json!({
                    "target_id": target_id,
                    "target_mem": target_mem,
                }),
            ),
        ),
        E::CrossMemEdgeNotDeclared {
            source_schema,
            target_schema,
            rel_type,
            from_id,
            to_id,
        } => tool_error_with_payload(
            e.code(),
            &message,
            envelope(
                e.code(),
                message.clone(),
                serde_json::json!({
                    "source_schema": source_schema,
                    "target_schema": target_schema,
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                }),
            ),
        ),
        E::RepairNotNeeded { id, recovery } => tool_error_with_payload(
            "REPAIR_NOT_NEEDED",
            &message,
            envelope(
                "REPAIR_NOT_NEEDED",
                message.clone(),
                serde_json::json!({ "id": id, "recovery": recovery }),
            ),
        ),
        E::ConflictingSectionModes { section, modes } => tool_error_with_payload(
            "CONFLICTING_SECTION_MODES",
            &message,
            envelope(
                "CONFLICTING_SECTION_MODES",
                message.clone(),
                serde_json::json!({ "section": section, "modes": modes }),
            ),
        ),
        E::RelationshipCycle {
            rel_type,
            from,
            to,
            existing_path,
            path_truncated,
            acyclic_set,
            existing_path_rel_types,
        } => {
            let existing_path_json: Vec<String> =
                existing_path.iter().map(|id| id.to_string()).collect();
            let mut details = serde_json::json!({
                "rel_type": rel_type,
                "from": from.to_string(),
                "to": to.to_string(),
                "existing_path": existing_path_json,
                "path_truncated": path_truncated,
            });
            // Additive set-refusal extras; single-rel-type refusals
            // keep their byte-identical payload.
            if let Some(set) = &acyclic_set {
                details["acyclic_set"] = serde_json::json!(set);
            }
            if let Some(rels) = &existing_path_rel_types {
                details["existing_path_rel_types"] = serde_json::json!(rels);
            }
            tool_error_with_payload(
                "RELATIONSHIP_CYCLE",
                &message,
                envelope("RELATIONSHIP_CYCLE", message.clone(), details),
            )
        }
        E::RequiredFieldUnset {
            field,
            entity_type,
            field_description,
            enum_values,
            type_write_rules,
            // Path-aware prose flows through `prose_render()`; the
            // structured payload below is the same on both paths so
            // `on_create` is intentionally not surfaced here.
            on_create: _,
            missing,
        } => {
            // `details.missing[]` carries every required-no-default
            // field unset on the create path. Each entry echoes
            // the type-level `write_rules` for self-containment.
            let missing_json: Vec<_> = missing
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "field": m.key,
                        "description": m.description,
                        "enum_values": m.enum_values,
                        "write_rules": type_write_rules,
                    })
                })
                .collect();
            tool_error_with_payload(
                "REQUIRED_FIELD_UNSET",
                &message,
                envelope(
                    "REQUIRED_FIELD_UNSET",
                    message.clone(),
                    serde_json::json!({
                        "field": field,
                        "entity_type": entity_type,
                        "field_description": field_description,
                        "enum_values": enum_values,
                        "type_write_rules": type_write_rules,
                        "missing": missing_json,
                    }),
                ),
            )
        }
        E::MissingRequiredSection {
            entity_type,
            missing_count,
            sections,
            type_guidance,
            pre_announced_missing_fields,
        } => {
            let sections_json: Vec<_> = sections
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "entity_type": s.entity_type,
                        "key": s.key,
                        "heading": s.heading,
                        "write_rules": s.write_rules,
                    })
                })
                .collect();
            let mut details = serde_json::json!({
                "entity_type": entity_type,
                "missing_count": missing_count,
                "sections": sections_json,
                "type_guidance": type_guidance,
            });
            // Cross-gate pre-announcement — additive, only when
            // non-empty; element shape mirrors REQUIRED_FIELD_UNSET's
            // details.missing[] so one decoder reads both.
            if !pre_announced_missing_fields.is_empty() {
                let type_rules = type_guidance.get(entity_type).cloned().unwrap_or_default();
                let missing_json: Vec<_> = pre_announced_missing_fields
                    .iter()
                    .map(|m| {
                        serde_json::json!({
                            "field": m.key,
                            "description": m.description,
                            "enum_values": m.enum_values,
                            "write_rules": type_rules,
                        })
                    })
                    .collect();
                details["pre_announced"] = serde_json::json!({
                    "required_field_unset": { "missing": missing_json }
                });
            }
            tool_error_with_payload(
                "MISSING_REQUIRED_SECTION",
                &message,
                envelope("MISSING_REQUIRED_SECTION", message.clone(), details),
            )
        }
        E::SetAndUnsetConflict { keys } => tool_error_with_payload(
            "SET_AND_UNSET_CONFLICT",
            &message,
            envelope(
                "SET_AND_UNSET_CONFLICT",
                message.clone(),
                serde_json::json!({ "keys": keys }),
            ),
        ),
        E::PatchSectionEmpty { section } => tool_error_with_payload(
            "PATCH_SECTION_EMPTY",
            &message,
            envelope(
                "PATCH_SECTION_EMPTY",
                message.clone(),
                serde_json::json!({ "section": section }),
            ),
        ),
        E::PatchOldNotFound {
            section,
            current_content,
            truncated,
            found_in_sections,
        } => tool_error_with_payload(
            "PATCH_OLD_NOT_FOUND",
            &message,
            envelope(
                "PATCH_OLD_NOT_FOUND",
                message.clone(),
                serde_json::json!({
                    "section": section,
                    "current_content": current_content,
                    "truncated": truncated,
                    "found_in_sections": found_in_sections,
                }),
            ),
        ),
        E::InvalidTitle(slug_err) => {
            use memstead_base::SlugError;
            let reason = slug_err.reason();
            let details = match &slug_err {
                SlugError::IdTooLong { input, length, max } => serde_json::json!({
                    "reason": reason,
                    "input": input,
                    "length": length,
                    "max": max,
                }),
                SlugError::TitleEmpty { input } => serde_json::json!({
                    "reason": reason,
                    "input": input,
                }),
                SlugError::TitleHasControlChars {
                    input,
                    control_chars,
                    proposed_slug,
                } => {
                    let control_chars_str: Vec<String> = control_chars
                        .iter()
                        .map(|c| c.escape_default().to_string())
                        .collect();
                    serde_json::json!({
                        "reason": reason,
                        "input": input,
                        "control_chars": control_chars_str,
                        "proposed_slug": proposed_slug,
                    })
                }
            };
            tool_error_with_payload(
                "INVALID_TITLE",
                &message,
                envelope("INVALID_TITLE", message.clone(), details),
            )
        }
        E::StubCannotRelate { id } => tool_error_with_payload(
            "STUB_CANNOT_RELATE",
            &message,
            envelope(
                "STUB_CANNOT_RELATE",
                message.clone(),
                serde_json::json!({ "id": id }),
            ),
        ),
        E::StubNotUpdatable { id } => tool_error_with_payload(
            "STUB_NOT_UPDATABLE",
            &message,
            envelope(
                "STUB_NOT_UPDATABLE",
                message.clone(),
                serde_json::json!({ "id": id }),
            ),
        ),
        E::StubNotRenamable { id } => tool_error_with_payload(
            "STUB_NOT_RENAMABLE",
            &message,
            envelope(
                "STUB_NOT_RENAMABLE",
                message.clone(),
                serde_json::json!({ "id": id }),
            ),
        ),
        E::InvalidEntityId { id, reason } => tool_error_with_payload(
            "INVALID_ENTITY_ID",
            &message,
            envelope(
                "INVALID_ENTITY_ID",
                message.clone(),
                serde_json::json!({ "id": id, "reason": reason }),
            ),
        ),
        E::InvalidWikiLinkTarget {
            raw,
            suggested,
            section,
            link_source,
            reason,
        } => tool_error_with_payload(
            "INVALID_WIKI_LINK_TARGET",
            &message,
            envelope(
                "INVALID_WIKI_LINK_TARGET",
                message.clone(),
                serde_json::json!({
                    "raw": raw,
                    "suggested": suggested,
                    "section": section,
                    "source": link_source,
                    "reason": reason,
                }),
            ),
        ),
        E::InvalidWikiLinkMem {
            raw,
            section,
            reason,
        } => tool_error_with_payload(
            "INVALID_MEM_NAME",
            &message,
            envelope(
                "INVALID_MEM_NAME",
                message.clone(),
                serde_json::json!({
                    "raw": raw,
                    "section": section,
                    "reason": reason,
                }),
            ),
        ),
        E::RelationHasBodyLinks {
            from_id,
            to_id,
            rel_type,
            body_links,
        } => tool_error_with_payload(
            "RELATION_HAS_BODY_LINKS",
            &message,
            envelope(
                "RELATION_HAS_BODY_LINKS",
                message.clone(),
                serde_json::json!({
                    "from_id": from_id,
                    "to_id": to_id,
                    "rel_type": rel_type,
                    "body_links": body_links,
                }),
            ),
        ),
        E::Validation(verr) => unified_validation_envelope(verr.clone()),
        // Lifecycle envelopes: the unified
        // `mem_management::create_mem` / `delete_mem` paths
        // surface these on the same wire contract.
        E::MemNameCollision {
            name,
            source_origin,
        } => tool_error_with_payload(
            "MEM_NAME_COLLISION",
            &message,
            envelope(
                "MEM_NAME_COLLISION",
                message.clone(),
                serde_json::json!({
                    "name": name,
                    "source": source_origin,
                }),
            ),
        ),
        e @ E::SchemaNotFound { .. } => tool_error_with_payload(
            "SCHEMA_NOT_FOUND",
            &message,
            envelope("SCHEMA_NOT_FOUND", message.clone(), e.details()),
        ),
        E::EmbeddedSchemaInvalid { mem, pin, reason } => tool_error_with_payload(
            "EMBEDDED_SCHEMA_INVALID",
            &message,
            envelope(
                "EMBEDDED_SCHEMA_INVALID",
                message.clone(),
                serde_json::json!({ "mem": mem, "schema": pin, "error": reason }),
            ),
        ),
        E::SchemaPackageInvalid {
            name,
            version,
            message: detail,
        } => tool_error_with_payload(
            "SCHEMA_VALIDATION_FAILED",
            &message,
            envelope(
                "SCHEMA_VALIDATION_FAILED",
                message.clone(),
                serde_json::json!({
                    "schema": format!("{name}@{version}"),
                    "error": detail,
                }),
            ),
        ),
        E::SchemaResolverInit(detail) => tool_error_with_payload(
            "SCHEMA_RESOLVER_INIT_FAILED",
            &message,
            envelope(
                "SCHEMA_RESOLVER_INIT_FAILED",
                message.clone(),
                serde_json::json!({ "detail": detail }),
            ),
        ),
        E::Mem(detail) => tool_error_with_payload(
            "MEM_ERROR",
            &message,
            envelope(
                "MEM_ERROR",
                message.clone(),
                serde_json::json!({ "detail": detail }),
            ),
        ),
        E::InvalidInput(msg) => tool_error_with_payload(
            "INVALID_INPUT",
            &message,
            envelope(
                "INVALID_INPUT",
                message.clone(),
                serde_json::json!({ "message": msg }),
            ),
        ),
        E::MergeConflictUnsupportedBackend { mem } => tool_error_with_payload(
            e.code(),
            &message,
            envelope(e.code(), message.clone(), serde_json::json!({ "mem": mem })),
        ),
        E::NotConflicted { id } => tool_error_with_payload(
            e.code(),
            &message,
            envelope(e.code(), message.clone(), serde_json::json!({ "id": id })),
        ),
        E::RenameSimilarityOutOfRange {
            requested,
            allowed_min,
            allowed_max,
        } => tool_error_with_payload(
            "INVALID_INPUT",
            &message,
            envelope(
                "INVALID_INPUT",
                message.clone(),
                serde_json::json!({
                    "field": "rename_similarity",
                    "requested": requested,
                    "allowed_range": [allowed_min, allowed_max],
                }),
            ),
        ),
        E::MemConfigIncomplete {
            mem,
            missing_fields,
        } => tool_error_with_payload(
            "MEM_CONFIG_INCOMPLETE",
            &message,
            envelope(
                "MEM_CONFIG_INCOMPLETE",
                message.clone(),
                serde_json::json!({
                    "mem": mem,
                    "missing_fields": missing_fields,
                    "set_via": format!("memstead mem set-version {mem} <version>"),
                }),
            ),
        ),
        E::WikiLinkWithoutRelation { from_id, missing } => tool_error_with_payload(
            "WIKILINK_WITHOUT_RELATION",
            &message,
            envelope(
                "WIKILINK_WITHOUT_RELATION",
                message.clone(),
                serde_json::json!({
                    "from_id": from_id,
                    "missing": missing,
                }),
            ),
        ),
        E::DescriptionNotPermitted {
            rel_type,
            from_id,
            to_id,
        } => tool_error_with_payload(
            "DESCRIPTION_NOT_PERMITTED",
            &message,
            envelope(
                "DESCRIPTION_NOT_PERMITTED",
                message.clone(),
                serde_json::json!({
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                }),
            ),
        ),
        E::MissingRequiredDescription {
            rel_type,
            from_id,
            to_id,
        } => tool_error_with_payload(
            "MISSING_REQUIRED_DESCRIPTION",
            &message,
            envelope(
                "MISSING_REQUIRED_DESCRIPTION",
                message.clone(),
                serde_json::json!({
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                }),
            ),
        ),
        E::RelationManualAuthoringForbidden {
            rel_type,
            from_id,
            to_id,
            guidance,
        } => tool_error_with_payload(
            "RELATION_MANUAL_AUTHORING_FORBIDDEN",
            &message,
            envelope(
                "RELATION_MANUAL_AUTHORING_FORBIDDEN",
                message.clone(),
                serde_json::json!({
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                    "guidance": guidance,
                }),
            ),
        ),
        // Retype refusals carry their report-all payload from `details()`
        // so the CLI and MCP envelopes cannot drift; the code is the
        // shared problem code or `RETYPE_REFUSED` (see `EngineError::code`).
        E::RetypeRefused { .. }
        | E::RetypeNoOp { .. }
        | E::RetypeReferrerUnprobeable { .. }
        | E::InvalidCheckFinding { .. } => tool_error_with_payload(
            e.code(),
            &message,
            envelope(e.code(), message.clone(), e.details()),
        ),
        E::RenameNoOp { id, new_title } => tool_error_with_payload(
            "RENAME_NO_OP",
            &message,
            envelope(
                "RENAME_NO_OP",
                message.clone(),
                serde_json::json!({
                    "id": id,
                    "new_title": new_title,
                }),
            ),
        ),
        E::RenameBlockedByCrossMemPolicy {
            from_mem,
            blocked_referrers,
        } => {
            let entries: Vec<_> = blocked_referrers
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "from_mem": r.from_mem,
                        "to_mem": r.to_mem,
                        "count": r.count,
                    })
                })
                .collect();
            tool_error_with_payload(
                "RENAME_BLOCKED_BY_CROSS_MEM_POLICY",
                &message,
                envelope(
                    "RENAME_BLOCKED_BY_CROSS_MEM_POLICY",
                    message.clone(),
                    serde_json::json!({
                        "from_mem": from_mem,
                        "blocked_referrers": entries,
                    }),
                ),
            )
        }
        E::RenamePartialFailure {
            committed_mems,
            failed_mem,
            failure_cause,
        } => tool_error_with_payload(
            "RENAME_PARTIAL_FAILURE",
            &message,
            envelope(
                "RENAME_PARTIAL_FAILURE",
                message.clone(),
                serde_json::json!({
                    "committed_mems": committed_mems,
                    "failed_mem": failed_mem,
                    "failure_cause": failure_cause,
                }),
            ),
        ),
        E::DuplicateMem(name) => tool_error_with_payload(
            "DUPLICATE_MEM",
            &message,
            envelope(
                "DUPLICATE_MEM",
                message.clone(),
                serde_json::json!({ "name": name }),
            ),
        ),
        // Parse-after-write / parse / backend: typed code, free-form
        // detail string so callers can render the underlying message
        // without grepping it back out of the envelope's `message`.
        E::ParseAfterWrite(detail) => tool_error_with_payload(
            "PARSE_ERROR",
            &message,
            envelope(
                "PARSE_ERROR",
                message.clone(),
                serde_json::json!({ "detail": detail }),
            ),
        ),
        E::Parse(inner) => tool_error_with_payload(
            "PARSE_ERROR",
            &message,
            envelope(
                "PARSE_ERROR",
                message.clone(),
                serde_json::json!({ "detail": inner.to_string() }),
            ),
        ),
        E::Backend(inner) => tool_error_with_payload(
            "MEM_ERROR",
            &message,
            envelope(
                "MEM_ERROR",
                message.clone(),
                serde_json::json!({ "detail": inner.to_string() }),
            ),
        ),
        E::SearchUnavailable => tool_error_with_payload(
            "SEARCH_UNAVAILABLE_IN_WASM",
            &message,
            envelope(
                "SEARCH_UNAVAILABLE_IN_WASM",
                message.clone(),
                serde_json::json!({}),
            ),
        ),
        // Typed refusal when
        // `export_markdown` targets a mem whose active backend
        // doesn't support markdown regeneration.
        E::MarkdownExportUnsupportedBackend {
            mem,
            active_backend,
            supported_backends,
        } => tool_error_with_payload(
            "MARKDOWN_EXPORT_UNSUPPORTED_BACKEND",
            &message,
            envelope(
                "MARKDOWN_EXPORT_UNSUPPORTED_BACKEND",
                message.clone(),
                serde_json::json!({
                    "mem": mem,
                    "active_backend": active_backend,
                    "supported_backends": supported_backends,
                }),
            ),
        ),
        E::EmptyUpdate { id } => tool_error_with_payload(
            "EMPTY_UPDATE",
            &message,
            envelope(
                "EMPTY_UPDATE",
                message.clone(),
                serde_json::json!({
                    "id": id,
                    // The engine's own list, not a copy.
                    "recognised_keys":
                        memstead_base::engine::error::RECOGNISED_MUTATION_KEYS,
                }),
            ),
        ),
        E::InvalidChangesCursor { mem, since } | E::InvalidTimestampCursor { mem, since } => {
            tool_error_with_payload(
                "INVALID_CURSOR",
                &message,
                envelope(
                    "INVALID_CURSOR",
                    message.clone(),
                    serde_json::json!({ "mem": mem, "since": since }),
                ),
            )
        }
        // Review-mark diff on a markless mem — typed refusal so agents
        // never equate "no mark" with "no changes".
        E::ReviewMarkNotSet { mem } => tool_error_with_payload(
            "REVIEW_MARK_NOT_SET",
            &message,
            envelope(
                "REVIEW_MARK_NOT_SET",
                message.clone(),
                serde_json::json!({ "mem": mem }),
            ),
        ),
        // Malformed `anchors[]` element on create/update: typed
        // `INVALID_ANCHOR` with the wrapped anchor error's recovery detail
        // (offending field, bad value, allowed set). The whole mutation
        // refused and the entity was not written.
        E::InvalidAnchor(anchor_err) => tool_error_with_payload(
            memstead_base::anchor::INVALID_ANCHOR_CODE,
            &message,
            envelope(
                memstead_base::anchor::INVALID_ANCHOR_CODE,
                message.clone(),
                serde_json::Value::Object(anchor_err.detail().into_iter().collect()),
            ),
        ),
    }
}

/// Typed-envelope translator for `FullEngineError`. Delegates wrapped
/// base-engine errors to [`engine_err_unified`]; constructs the lifecycle-
/// specific envelopes (`MEM_PATH_NOT_ALLOWED`,
/// `MEM_REFERENCED_BY_POLICY`, `MEM_SCHEMA_NOT_ALLOWED`,
/// `CONFIG_ERROR`) here. The wire shape is
/// bit-identical to what `engine_err_unified` produced for the same
/// variants before the lifecycle variants moved off
/// `memstead_base::EngineError`; the move is pure plumbing.
pub(super) fn full_engine_err_unified(
    e: memstead_base::FullEngineError,
    engine: &memstead_base::Engine,
) -> CallToolResult {
    use memstead_base::FullEngineError as PE;
    // The text-channel message uses the rich-prose renderer so lifecycle
    // refusals (MEM_PATH_NOT_ALLOWED, MEM_SCHEMA_NOT_ALLOWED,
    // MEM_REFERENCED_BY_POLICY) inline their full recovery payload
    // inline rather than relying on the structured channel for
    // recovery context.
    let message = e.prose_render();
    // Shared structured payload — computed before the match so the
    // lifecycle arms cannot drift from the CLI envelope, which lifts
    // the same `details()`.
    let shared_details = e.details();
    match e {
        // #55: thread the engine so the wrapped base-engine path enriches
        // not-found envelopes the same as every other call site.
        PE::Engine(inner) => engine_err_unified(inner, engine),
        PE::MemPathNotAllowed { .. } => tool_error_with_payload(
            "MEM_PATH_NOT_ALLOWED",
            &message,
            envelope("MEM_PATH_NOT_ALLOWED", message.clone(), shared_details),
        ),
        PE::InvalidMemName { name, reason } => tool_error_with_payload(
            "INVALID_MEM_NAME",
            &message,
            envelope(
                "INVALID_MEM_NAME",
                message.clone(),
                serde_json::json!({
                    "name": name,
                    "reason": reason,
                }),
            ),
        ),
        PE::MemSchemaNotAllowed {
            candidate,
            matched_pattern,
            requested_schema,
            allowed_schemas,
        } => tool_error_with_payload(
            "MEM_SCHEMA_NOT_ALLOWED",
            &message,
            envelope(
                "MEM_SCHEMA_NOT_ALLOWED",
                message.clone(),
                serde_json::json!({
                    "candidate": candidate,
                    "matched_pattern": matched_pattern,
                    "requested_schema": requested_schema,
                    "allowed_schemas": allowed_schemas,
                }),
            ),
        ),
        PE::MemReferencedByPolicy {
            name,
            referring_mems,
        } => tool_error_with_payload(
            "MEM_REFERENCED_BY_POLICY",
            &message,
            envelope(
                "MEM_REFERENCED_BY_POLICY",
                message.clone(),
                serde_json::json!({
                    "name": name,
                    "referring_mems": referring_mems,
                }),
            ),
        ),
        PE::ConfigAlreadyExists { path } => tool_error_with_payload(
            "CONFIG_ERROR",
            &message,
            envelope(
                "CONFIG_ERROR",
                message.clone(),
                serde_json::json!({
                    "path": path.display().to_string(),
                    "reason": "config_already_exists",
                }),
            ),
        ),
        PE::MemStorageResidueDetected {
            branch_ref,
            config_blob,
            entity_count,
        } => tool_error_with_payload(
            "MEM_STORAGE_RESIDUE_DETECTED",
            &message,
            envelope(
                "MEM_STORAGE_RESIDUE_DETECTED",
                message.clone(),
                serde_json::json!({
                    "branch_ref": branch_ref,
                    "config_blob": config_blob,
                    "entity_count": entity_count,
                    "recovery": ["reattach", "force_overwrite", "hard_cleanup_first"],
                }),
            ),
        ),
    }
}

/// Map a runtime [`memstead_base::runtime_validator::ValidationError`] to
/// the MCP wire envelope. Thin delegation to the shared
/// [`crate::error_envelopes::validation_envelope`] so the unified
/// engine's mutation handlers emit the same wire shape full's
/// filesystem-server already does.
pub(super) fn unified_validation_envelope(
    err: memstead_base::runtime_validator::ValidationError,
) -> CallToolResult {
    crate::error_envelopes::validation_envelope(err)
}

/// Find entity IDs that end with the given suffix (slug or medium--slug).
/// Returns up to `max` suggestions for "did you mean?" messages.
///
/// Takes `&Store` directly so the helper composes against any
/// engine — `memstead_base::Engine::store()` exposes a `memstead_base::Store`.
pub(super) fn suggest_similar(store: &memstead_base::Store, input: &str) -> Vec<String> {
    let needle = input.trim_start_matches("@memstead/");
    store
        .all_ids()
        .filter(|id| {
            let haystack = id.as_ref();
            // Match if the stored ID ends with the input (e.g. "mcp-server" matches "...specs--mcp-server")
            haystack.ends_with(needle)
                || haystack.ends_with(&format!("--{needle}"))
                // Also match if the slug portion contains the input
                || haystack.rsplit_once("--").is_some_and(|(_, slug)| slug.contains(needle))
        })
        .take(5)
        .map(|id| id.to_string())
        .collect()
}

/// Build a "not found" error with suggestions.
///
/// Routes through [`tool_error_with_payload`] so the text channel
/// carries the `ERROR [ENTITY_NOT_FOUND]: …` prefix and
/// `structured_content` carries the `{ code, message, details }`
/// envelope — matching every other not-found return on the surface.
/// Takes `&Store` so the helper works for both full and unified engines.
pub(super) fn not_found_error(store: &memstead_base::Store, id: &EntityId) -> CallToolResult {
    let suggestions = suggest_similar(store, id.as_ref());
    let msg = if suggestions.is_empty() {
        format!("Entity not found: {id}")
    } else {
        format!(
            "Entity not found: \"{id}\". Did you mean: {}",
            suggestions.join(", ")
        )
    };
    tool_error_with_payload(
        "ENTITY_NOT_FOUND",
        &msg,
        envelope(
            "ENTITY_NOT_FOUND",
            msg.clone(),
            serde_json::json!({
                "id": id.as_ref(),
                "suggestions": suggestions,
            }),
        ),
    )
}
