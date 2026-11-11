//! Memstead CLI library — the command modules, utility modules, and
//! the shared `CliError` behind the `memstead` binary (`src/main.rs`).
//!
//! One binary, every subcommand, including the multi-mem / mem-repo
//! lifecycle (mem, workspace, install, batch-update, recover). The
//! same binary serves folder-only and mem-repo workspaces; the
//! mem-repo-only subcommands refuse on the folder shape with a typed
//! `UNSUPPORTED_WORKSPACE_SHAPE` envelope.

pub mod auth;
pub mod cli;
pub mod commands;
pub mod coverage;
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
    pub fn new(kind: ExitKind, code: &'static str, message: impl Into<String>) -> Self {
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
    /// A mem-management error: the wrapped engine error as
    /// [`Self::from_engine_op`], every lifecycle refusal at exit 5, with
    /// the engine's code, payload and message.
    pub fn from_full_engine_op(err: memstead_base::FullEngineError) -> Self {
        match err {
            memstead_base::FullEngineError::Engine(inner) => Self::from_engine_op(inner),
            lifecycle => Self {
                kind: ExitKind::Validation,
                code: lifecycle.code(),
                details: Some(lifecycle.details()),
                message: lifecycle.prose_render(),
            },
        }
    }

    /// An engine error with one sentence of this surface's own appended
    /// to the message: the recovery step in CLI terms (a command or a
    /// flag). The code and the payload stay the engine's.
    pub fn from_engine_with_hint(e: memstead_base::EngineError, hint: &str) -> Self {
        let mut err = Self::from_engine_op(e);
        err.message = format!("{} {hint}", err.message);
        err
    }

    /// A workspace-creation refusal at exit `Validation`, with this
    /// command's recovery sentence after the engine's message.
    pub fn from_workspace_create(
        e: memstead_base::workspace_create::WorkspaceCreateError,
        hint: &str,
    ) -> Self {
        Self {
            kind: ExitKind::Validation,
            code: e.code(),
            details: Some(e.details()),
            message: if hint.is_empty() {
                e.to_string()
            } else {
                format!("{e}. {hint}")
            },
        }
    }

    /// An engine error with facts of this run beside its payload (the
    /// binding it named, the figures it measured) and this command's
    /// recovery sentence after the message.
    pub fn from_engine_in_run(
        e: memstead_base::EngineError,
        run: serde_json::Value,
        hint: &str,
    ) -> Self {
        let mut err = Self::from_engine_with_hint(e, hint);
        if let (Some(d), serde_json::Value::Object(extra)) =
            (err.details.as_mut().and_then(|d| d.as_object_mut()), run)
        {
            d.extend(extra);
        }
        err
    }

    pub fn from_engine_op(e: memstead_base::EngineError) -> Self {
        use memstead_base::EngineError::*;
        // The per-entity wrapper of a batch act: the source decides the
        // exit kind, the code and the payload; the wrapper adds the
        // entity and the stage to the payload and prefixes the message
        // (the same composition `EngineError::prose_render` renders).
        let e = match e {
            InEntity {
                entity,
                stage,
                source,
            } => {
                let inner = Self::from_engine_op(*source);
                return Self {
                    kind: inner.kind,
                    code: inner.code,
                    message: format!("{entity} ({stage}): {}", inner.message),
                    details: Some(memstead_base::engine::error::scope_details_to_entity(
                        inner.details.unwrap_or(serde_json::Value::Null),
                        &entity,
                        &stage,
                    )),
                };
            }
            other => other,
        };
        let code = e.code();
        // The exit kind is this surface's encoding; the code, the payload
        // and the message are the engine's (one value for both surfaces).
        let kind = match &e {
            StaleView { .. }
            | CommitNotApplied { .. }
            | MemQuarantined { .. }
            | CheckNotRecorded { .. }
            | DuplicateMem { .. }
            | SchemaResolverInit { .. }
            | Mem { .. }
            | ParseAfterWrite { .. }
            | Parse { .. }
            | Backend { .. }
            | SearchUnavailable
            | ChunkOutOfRange { .. }
            | InvalidSchemaPin { .. }
            | NoWritableMem
            | ContextNotComputable { .. } => ExitKind::Generic,
            HashMismatch { .. } => ExitKind::HashMismatch,
            NotFound { .. }
            | EntityIdMissingMem { .. }
            | MemUnmounted { .. }
            | UnknownMem { .. } => ExitKind::NotFound,
            ExpectedHashRequired { .. }
            | MergeConflictUnsupportedBackend { .. }
            | NotConflicted { .. }
            | HasIncomingRefs { .. }
            | MemHasIncomingRefs { .. }
            | WikiLinkWithoutRelation { .. }
            | ConstraintUnsatisfied { .. }
            | RequiredOutgoingUnsatisfied { .. }
            | SectionFormatRefused { .. }
            | RelationHasBodyLinks { .. }
            | InvalidEntityId { .. }
            | InvalidWikiLinkTarget { .. }
            | InvalidWikiLinkMem { .. }
            | CrossMemLinkNotAllowed { .. }
            | CrossMemTargetNotFound { .. }
            | RenameNoOp { .. }
            | RetypeRefused { .. }
            | RetypeNoOp { .. }
            | RetypeReferrerUnprobeable { .. }
            | InvalidCheckFinding { .. }
            | StubCannotRelate { .. }
            | StubNotUpdatable { .. }
            | StubNotRenamable { .. }
            | AlreadyExists { .. }
            | UnknownType { .. }
            | Validation { .. }
            | MemConfigIncomplete { .. }
            | InvalidTitle { .. }
            | DescriptionNotPermitted { .. }
            | MissingRequiredDescription { .. }
            | RelationManualAuthoringForbidden { .. }
            | CrossMemEdgeNotDeclared { .. }
            | RepairNotNeeded { .. }
            | ConflictingSectionModes { .. }
            | RelationshipCycle { .. }
            | SetAndUnsetConflict { .. }
            | RequiredFieldUnset { .. }
            | MissingRequiredSection { .. }
            | PatchSectionEmpty { .. }
            | PatchOldNotFound { .. }
            | RenameBlockedByCrossMemPolicy { .. }
            | RenamePartialFailure { .. }
            | UnknownRef { .. }
            | ProposalDispositionsIncomplete { .. }
            | ProposalConflict { .. }
            | ProposalStale { .. }
            | ProposalUnattributed { .. }
            | ProposalLinksRejected { .. }
            | BranchResetHeadMoved { .. }
            | PushedCommitsProtected { .. }
            | UnknownRemote { .. }
            | LocalDivergence { .. }
            | NonFastForward { .. }
            | LocalInvalidState { .. }
            | SchemaViolationInFetch { .. }
            | ReadOnlyMount { .. }
            | MemNameCollision { .. }
            | ProcessMemNotEligible { .. }
            | SchemaNotFound { .. }
            | EmbeddedSchemaInvalid { .. }
            | SchemaPackageInvalid { .. }
            | InvalidInput { .. }
            | InvalidTargetSchema { .. }
            | UnknownSchema { .. }
            | UnknownTypes { .. }
            | AmbiguousMem { .. }
            | AnchorsSidecarUnreadable { .. }
            | AmbiguousQuery { .. }
            | MemNotReadOnly { .. }
            | NotGitBacked { .. }
            | TemplateIdentityMismatch { .. }
            | EntityIdTooLong { .. }
            | RenameSimilarityOutOfRange { .. }
            | MarkdownExportUnsupportedBackend { .. }
            | EmptyUpdate { .. }
            | InvalidChangesCursor { .. }
            | InvalidTimestampCursor { .. }
            | ReviewMarkNotSet { .. }
            | InvalidAnchor { .. }
            | UnterminatedFenceInStoredBody { .. } => ExitKind::Validation,
            InEntity { .. } => unreachable!("the per-entity wrapper is unwrapped above"),
        };
        let details = Some(e.details());
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
    use memstead_base::EngineError;
    use memstead_base::engine::MissingWikiLink;

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
