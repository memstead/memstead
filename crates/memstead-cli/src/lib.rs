//! Memstead CLI library — the command modules, utility modules, and
//! the shared `CliError` behind the `memstead` binary (`src/main.rs`).
//!
//! One crate, two build configs. The default build (`mem-repo`
//! feature on) is the full `memstead`: every subcommand, including the
//! multi-mem / mem-repo lifecycle (mem, workspace, install,
//! batch-update, recover). `--no-default-features` drops the git-branch
//! backend and the mem-repo-only subcommands, yielding the lean
//! engine-agnostic surface (a CI / wasm-adjacent config, not shipped).

pub mod auth;
pub mod cli;
pub mod commands;
#[cfg(feature = "mem-repo")]
pub mod outer_gitignore;
pub mod output;
pub mod registry;
pub mod setup;

use output::ExitKind;

/// Stable wire token for genuinely-systemic failures the agent can't
/// recover from (I/O panic, store corruption, unreachable branch).
/// The `code` field is non-optional, so this constant exists for
/// callsites that explicitly choose it — defaulting to it is not
/// possible. Adding a new callsite using this constant should carry a
/// comment explaining why no recoverable typed code applies.
pub const INTERNAL_CODE: &str = "INTERNAL";

/// Argument-validation refusal: mutating commands require one of
/// `--auto-hash`, `--expected-hash`, or `--force`. The typed code lets
/// agents branch on the wire token rather than parsing the message.
pub const HASH_FLAG_REQUIRED_CODE: &str = "HASH_FLAG_REQUIRED";

/// `memstead init <target>` refusal: target directory is non-empty (the
/// init refuses to scribble over existing files / pre-existing
/// workspaces).
pub const TARGET_NOT_EMPTY_CODE: &str = "TARGET_NOT_EMPTY";

/// `memstead overview --chunk <N>` refusal: requested chunk index is
/// beyond the actual chunk count.
pub const CHUNK_OUT_OF_RANGE_CODE: &str = "CHUNK_OUT_OF_RANGE";

/// `memstead init` refusal: ancestor walk found an existing
/// `.memstead/workspace.toml` above the target. Without this guard a
/// standalone init would silently nest a fresh filesystem-mem
/// workspace inside an existing one, with neither workspace aware of
/// the other.
pub const WORKSPACE_ALREADY_EXISTS_ABOVE_CODE: &str = "WORKSPACE_ALREADY_EXISTS_ABOVE";

/// `memstead install <archive>` refusal: archive failed strict
/// validation (any of the variants the strict-archive validator can
/// produce).
pub const ARCHIVE_VALIDATION_FAILED_CODE: &str = "ARCHIVE_VALIDATION_FAILED";

/// Typed CLI error that carries an exit-code kind, a stable
/// `UPPER_SNAKE_CASE` code (matching `EngineError::code()` for
/// engine-sourced errors), and an optional structured details payload.
/// Wrap with `anyhow::Error` via `.into()` or `map_err` to propagate up
/// to `main`, which renders the error through
/// [`output::print_cli_error`] in the documented `{code, message, details}`
/// shape (or `memstead: ERROR [<CODE>]: <message>` on the text channel).
///
/// `code` is non-optional, so every construction site spells the wire
/// token — there is no `Option`-default-to-`INTERNAL` fallback that
/// could leak `INTERNAL` when a callsite forgets to set it.
/// Engine-sourced errors capture `EngineError::code()` via
/// [`CliError::from_engine_op`]; setup-layer paths pin their own typed
/// token at construction time.
///
/// `details` is the structured recovery payload — e.g. `HashMismatch`
/// populates `{"current": "<hash>"}` so scripts can lift the recovery
/// hash without re-reading the entity. The renderer surfaces it under
/// the `details` key of the JSON envelope.
#[derive(Debug)]
pub struct CliError {
    pub kind: ExitKind,
    pub code: &'static str,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

impl CliError {
    /// Construct a typed CLI error with an exit-kind, wire code, and
    /// human message. Every callsite spells the code at construction
    /// time — there is no Option-default-to-INTERNAL fallback. Use
    /// [`INTERNAL_CODE`] explicitly for genuinely-systemic paths.
    pub fn new(
        kind: ExitKind,
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Override the wire code on an existing `CliError`. This helper exists
    /// for callsites that build an error in steps (e.g.
    /// `setup` layers that mint the error before knowing whether to
    /// retag it with a more specific code). New code should spell the
    /// final code at [`CliError::new`] construction time.
    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = code;
        self
    }

    /// Field accessor preserved as a method for backward compatibility
    /// with the previous `Option<&'static str>` API. Now a trivial
    /// `self.code` return; kept so existing call sites don't need to
    /// flip method-call syntax to field-access syntax.
    pub fn effective_code(&self) -> &'static str {
        self.code
    }

    /// Map an [`memstead_base::EngineError`] to a typed CLI error with the
    /// right exit code (`NOT_FOUND` → 3, `HASH_MISMATCH` → 4,
    /// validation errors → 5, everything else → 1) and the typed wire
    /// code from `EngineError::code()`. Recovery payloads
    /// (`HashMismatch.current`, `HasIncomingRefs.referrers`,
    /// `WikiLinkWithoutRelation.missing`, etc.) land under `details` so
    /// `--json` callers consume the same `{code, message, details}`
    /// envelope they get over MCP — bit-identical wire shape across
    /// surfaces is the agent contract this method delivers.
    pub fn from_engine_op(e: memstead_base::EngineError) -> Self {
        use memstead_base::EngineError::*;
        let code = e.code();
        let (kind, details) = match &e {
            NotFound { id } => (
                ExitKind::NotFound,
                Some(serde_json::json!({ "id": id })),
            ),
            HashMismatch { id, current, is_stub } => (
                ExitKind::HashMismatch,
                Some(serde_json::json!({
                    "id": id,
                    "current": current,
                    "is_stub": is_stub,
                })),
            ),
            HasIncomingRefs { id, referrers } => {
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
                (
                    ExitKind::Validation,
                    Some(serde_json::json!({
                        "id": id,
                        "referrers": referrers_json,
                    })),
                )
            }
            MemHasIncomingRefs { mem, referrers } => {
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
                (
                    ExitKind::Validation,
                    Some(serde_json::json!({
                        "mem": mem,
                        "referrers": referrers_json,
                    })),
                )
            }
            WikiLinkWithoutRelation { from_id, missing } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "from_id": from_id,
                    "missing": missing,
                })),
            ),
            RelationHasBodyLinks { from_id, to_id, rel_type, body_links } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "from_id": from_id,
                    "to_id": to_id,
                    "rel_type": rel_type,
                    "body_links": body_links,
                })),
            ),
            InvalidEntityId { id, reason } => (
                ExitKind::Validation,
                Some(serde_json::json!({ "id": id, "reason": reason })),
            ),
            InvalidWikiLinkTarget {
                raw,
                suggested,
                section,
                link_source,
                reason,
            } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "raw": raw,
                    "suggested": suggested,
                    "section": section,
                    "source": link_source,
                    "reason": reason,
                })),
            ),
            InvalidWikiLinkMem { raw, section, reason } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "raw": raw,
                    "section": section,
                    "reason": reason,
                })),
            ),
            CrossMemLinkNotAllowed { from_mem, to_mem } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "from_mem": from_mem,
                    "to_mem": to_mem,
                })),
            ),
            CrossMemTargetNotFound { target_id, target_mem } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "target_id": target_id,
                    "target_mem": target_mem,
                })),
            ),
            RenameNoOp { id, new_title } => (
                ExitKind::Validation,
                Some(serde_json::json!({ "id": id, "new_title": new_title })),
            ),
            StubCannotRelate { id }
            | StubNotUpdatable { id }
            | StubNotRenamable { id } => (
                ExitKind::Validation,
                Some(serde_json::json!({ "id": id })),
            ),
            AlreadyExists { id } => (
                ExitKind::Validation,
                Some(serde_json::json!({ "id": id })),
            ),
            UnknownType { name, schema_ref, declared, suggestion } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "name": name,
                    "schema_ref": schema_ref,
                    "declared": declared,
                    "suggestion": suggestion,
                })),
            ),
            Validation(v) => (ExitKind::Validation, Some(v.details())),
            MemConfigIncomplete { mem, missing_fields } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "mem": mem,
                    "missing_fields": missing_fields,
                    "set_via": format!("memstead mem set-version {mem} <version>"),
                })),
            ),
            InvalidTitle(slug_err) => {
                use memstead_base::SlugError;
                let reason = slug_err.reason();
                let details = match slug_err {
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
                    SlugError::TitleHasInvalidChars {
                        input,
                        invalid_chars,
                        proposed_slug,
                    } => {
                        let invalid_chars_str: Vec<String> =
                            invalid_chars.iter().map(|c| c.to_string()).collect();
                        serde_json::json!({
                            "reason": reason,
                            "input": input,
                            "invalid_chars": invalid_chars_str,
                            "proposed_slug": proposed_slug,
                        })
                    }
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
                (ExitKind::Validation, Some(details))
            }
            // Exhaustiveness: the
            // arms below replace a pre-existing `_ => (Generic, None)`
            // wildcard that silently swallowed `DescriptionNotPermitted`,
            // `MissingRequiredDescription`, and the rename-policy /
            // partial-failure variants — trained CLI agents to treat
            // these as Generic (exit 1) without structured details. The
            // exhaustive match forces every new `EngineError` variant to
            // declare its CLI shape before it can land. Compiler is the
            // forcing function.
            DescriptionNotPermitted { rel_type, from_id, to_id } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                })),
            ),
            MissingRequiredDescription { rel_type, from_id, to_id } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                })),
            ),
            RelationManualAuthoringForbidden {
                rel_type,
                from_id,
                to_id,
                guidance,
            } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                    "guidance": guidance,
                })),
            ),
            CrossMemEdgeNotDeclared {
                source_schema,
                target_schema,
                rel_type,
                from_id,
                to_id,
            } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "source_schema": source_schema,
                    "target_schema": target_schema,
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                })),
            ),
            RepairNotNeeded { id, recovery } => (
                ExitKind::Validation,
                Some(serde_json::json!({ "id": id, "recovery": recovery })),
            ),
            ConflictingSectionModes { section, modes } => (
                ExitKind::Validation,
                Some(serde_json::json!({ "section": section, "modes": modes })),
            ),
            RelationshipCycle {
                rel_type,
                from,
                to,
                existing_path,
                path_truncated,
            } => {
                let existing_path_json: Vec<String> =
                    existing_path.iter().map(|id| id.to_string()).collect();
                (
                    ExitKind::Validation,
                    Some(serde_json::json!({
                        "rel_type": rel_type,
                        "from": from.to_string(),
                        "to": to.to_string(),
                        "existing_path": existing_path_json,
                        "path_truncated": path_truncated,
                    })),
                )
            }
            SetAndUnsetConflict { keys } => (
                ExitKind::Validation,
                Some(serde_json::json!({ "keys": keys })),
            ),
            RequiredFieldUnset {
                field,
                entity_type,
                field_description,
                enum_values,
                type_write_rules,
                // `on_create` is a prose-dispatch discriminator only;
                // the structured details payload is identical on both
                // call sites.
                on_create: _,
                missing,
            } => {
                // `details.missing[]` carries every required-no-
                // default field unset on the create path. Each
                // entry echoes the type-level `write_rules`.
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
                (
                    ExitKind::Validation,
                    Some(serde_json::json!({
                        "field": field,
                        "entity_type": entity_type,
                        "field_description": field_description,
                        "enum_values": enum_values,
                        "type_write_rules": type_write_rules,
                        "missing": missing_json,
                    })),
                )
            }
            MissingRequiredSection {
                entity_type,
                missing_count,
                sections,
                type_guidance,
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
                (
                    ExitKind::Validation,
                    Some(serde_json::json!({
                        "entity_type": entity_type,
                        "missing_count": missing_count,
                        "sections": sections_json,
                        "type_guidance": type_guidance,
                    })),
                )
            }
            PatchSectionEmpty { section } => (
                ExitKind::Validation,
                Some(serde_json::json!({ "section": section })),
            ),
            PatchOldNotFound { section, current_content, truncated } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "section": section,
                    "current_content": current_content,
                    "truncated": truncated,
                })),
            ),
            RenameBlockedByCrossMemPolicy { from_mem, blocked_referrers } => {
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
                (
                    ExitKind::Validation,
                    Some(serde_json::json!({
                        "from_mem": from_mem,
                        "blocked_referrers": entries,
                    })),
                )
            }
            RenamePartialFailure {
                committed_mems,
                failed_mem,
                failure_cause,
            } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "committed_mems": committed_mems,
                    "failed_mem": failed_mem,
                    "failure_cause": failure_cause,
                })),
            ),
            UnknownMem(name) => (
                // A missing/unmatched mem is a not-found condition, the
                // same category as `ENTITY_NOT_FOUND` (exit 3) — not a
                // validation refusal. This central engine-error path covers
                // `reload --mem nope` and every command that surfaces the
                // engine's `UnknownMem` rather than constructing the code
                // itself.
                ExitKind::NotFound,
                Some(serde_json::json!({ "name": name })),
            ),
            UnknownRef(raw) => (
                ExitKind::Validation,
                Some(serde_json::json!({ "ref": raw })),
            ),
            PushedCommitsProtected {
                mem,
                target_sha,
                pushed_shas,
            } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "mem": mem,
                    "target_sha": target_sha,
                    "pushed_shas": pushed_shas,
                })),
            ),
            UnknownRemote(name) => (
                ExitKind::Validation,
                Some(serde_json::json!({ "remote": name })),
            ),
            LocalDivergence { mem, remote_ref } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "mem": mem,
                    "remote_ref": remote_ref,
                })),
            ),
            NonFastForward { mem, remote } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "mem": mem,
                    "remote": remote,
                })),
            ),
            LocalInvalidState {
                mem,
                remote,
                detail,
            } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "mem": mem,
                    "remote": remote,
                    "detail": detail,
                })),
            ),
            SchemaViolationInFetch {
                mem,
                ref_name,
                violations,
            } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "mem": mem,
                    "ref": ref_name,
                    "violations": violations,
                })),
            ),
            ReadOnlyMount(mem) => (
                ExitKind::Validation,
                Some(serde_json::json!({ "mem": mem })),
            ),
            MemNameCollision { name, source_origin } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "name": name,
                    "source": source_origin,
                })),
            ),
            SchemaNotFound { mem, pin, sources } => (
                ExitKind::Validation,
                Some(serde_json::json!({ "mem": mem, "pin": pin, "sources": sources })),
            ),
            InvalidInput(msg) => (
                ExitKind::Validation,
                Some(serde_json::json!({ "message": msg })),
            ),
            RenameSimilarityOutOfRange {
                requested,
                allowed_min,
                allowed_max,
            } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "field": "rename_similarity",
                    "requested": requested,
                    "allowed_range": [allowed_min, allowed_max],
                })),
            ),
            // Engine-internal / boundary errors: no user-recoverable
            // structured payload. Code + message are sufficient — the
            // CLI surfaces the typed code via `e.code()` (already set
            // at the top of this fn) and the message text describes
            // the underlying cause.
            DuplicateMem(name) => (
                ExitKind::Generic,
                Some(serde_json::json!({ "name": name })),
            ),
            SchemaResolverInit(detail) => (
                ExitKind::Generic,
                Some(serde_json::json!({ "detail": detail })),
            ),
            Mem(detail) => (
                ExitKind::Generic,
                Some(serde_json::json!({ "detail": detail })),
            ),
            ParseAfterWrite(detail) => (
                ExitKind::Generic,
                Some(serde_json::json!({ "detail": detail })),
            ),
            Parse(inner) => (
                ExitKind::Generic,
                Some(serde_json::json!({ "detail": inner.to_string() })),
            ),
            Backend(inner) => (
                ExitKind::Generic,
                Some(serde_json::json!({ "detail": inner.to_string() })),
            ),
            SearchUnavailable => (ExitKind::Generic, Some(serde_json::json!({}))),
            // Typed refusal
            // when `memstead export --format markdown --mem-name <V>`
            // targets a backend that doesn't support markdown
            // regeneration. Validation-class exit code matches other
            // backend-incompatibility refusals.
            MarkdownExportUnsupportedBackend {
                mem,
                active_backend,
                supported_backends,
            } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "mem": mem,
                    "active_backend": active_backend,
                    "supported_backends": supported_backends,
                })),
            ),
            EmptyUpdate { id } => (
                ExitKind::Validation,
                Some(serde_json::json!({
                    "id": id,
                    "recognised_keys": [
                        "sections", "append_sections", "patch_sections",
                        "metadata", "metadata_unset", "declare_relations",
                    ],
                })),
            ),
            // A bad `--since` cursor surfaces the typed `INVALID_CURSOR` (via
            // `e.code()`) with the untruncated SHA — rather than leaking it
            // as the `MEM_ERROR` catch-all.
            InvalidChangesCursor { mem, since } => (
                ExitKind::Validation,
                Some(serde_json::json!({ "mem": mem, "since": since })),
            ),
        };
        // Route the CLI message through the rich-prose renderer so markdown-
        // default mode and `--json --message` consumers see the same
        // fully-inlined recovery prose the MCP text channel emits.
        // The `details` channel is unchanged.
        let message = e.prose_render();
        Self {
            kind,
            code,
            message,
            details,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::ExitKind;
    use memstead_base::engine::MissingWikiLink;
    use memstead_base::EngineError;

    /// `DescriptionNotPermitted` must reach the CLI wire as
    /// `code: DESCRIPTION_NOT_PERMITTED` with `ExitKind::Validation`
    /// (exit code 5) and structured details — not `Generic` (exit code
    /// 1) with `details: None`.
    #[test]
    fn from_engine_op_description_not_permitted_carries_validation_and_details() {
        let err = EngineError::DescriptionNotPermitted {
            rel_type: "REFERENCES".to_string(),
            from_id: "demo--source".to_string(),
            to_id: "demo--target".to_string(),
        };
        let cli = CliError::from_engine_op(err);
        assert_eq!(cli.kind, ExitKind::Validation);
        assert_eq!(cli.code, "DESCRIPTION_NOT_PERMITTED");
        let details = cli.details.expect("details must carry structured payload");
        assert_eq!(
            details.get("rel_type").and_then(|v| v.as_str()),
            Some("REFERENCES")
        );
        assert_eq!(
            details.get("from_id").and_then(|v| v.as_str()),
            Some("demo--source")
        );
        assert_eq!(
            details.get("to_id").and_then(|v| v.as_str()),
            Some("demo--target")
        );
    }

    /// `MissingRequiredDescription` shares the same
    /// envelope shape so the agent's branch logic is symmetric.
    #[test]
    fn from_engine_op_missing_required_description_carries_validation_and_details() {
        let err = EngineError::MissingRequiredDescription {
            rel_type: "CHOSEN".to_string(),
            from_id: "decisions--example".to_string(),
            to_id: "specs--target".to_string(),
        };
        let cli = CliError::from_engine_op(err);
        assert_eq!(cli.kind, ExitKind::Validation);
        assert_eq!(cli.code, "MISSING_REQUIRED_DESCRIPTION");
        let details = cli.details.expect("details must carry structured payload");
        assert_eq!(
            details.get("rel_type").and_then(|v| v.as_str()),
            Some("CHOSEN")
        );
    }

    /// `WikiLinkWithoutRelation` already had a typed CLI arm
    /// before the exhaustive-match work (the regression class was
    /// MCP-only), but a smoke test pins the contract so a future
    /// refactor doesn't drop it back into the wildcard.
    #[test]
    fn from_engine_op_wikilink_without_relation_carries_validation_and_missing_list() {
        let err = EngineError::WikiLinkWithoutRelation {
            from_id: "demo--source".to_string(),
            missing: vec![MissingWikiLink {
                section_key: "identity".to_string(),
                target_id: "demo--target".to_string(),
            }],
        };
        let cli = CliError::from_engine_op(err);
        assert_eq!(cli.kind, ExitKind::Validation);
        assert_eq!(cli.code, "WIKILINK_WITHOUT_RELATION");
        let details = cli.details.expect("details must carry structured payload");
        let missing = details
            .get("missing")
            .and_then(|v| v.as_array())
            .expect("details.missing[] must be an array");
        assert_eq!(missing.len(), 1);
        let first = &missing[0];
        assert_eq!(
            first.get("section_key").and_then(|v| v.as_str()),
            Some("identity")
        );
        assert_eq!(
            first.get("target_id").and_then(|v| v.as_str()),
            Some("demo--target")
        );
    }
}
