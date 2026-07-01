//! Pro-flavor engine error envelope.
//!
//! Mirrors the wrap-not-embed pattern: pro errors **wrap** basis
//! errors via `From<memstead_base::EngineError>`, so pro code paths can
//! transparently propagate a basis failure without re-wrapping at each
//! call site. The pro MCP render layer reads the wrapped chain to
//! produce the typed `code`; the basis render layer only ever sees
//! basis errors.
//!
//! The four lifecycle-only variants live here rather than on
//! `memstead_base::EngineError`: they are produced by this crate's
//! vault-management orchestrator (`create_vault` / `delete_vault`),
//! which returns `Result<_, ProEngineError>`, so the basis crate
//! carries no pro-specific lifecycle types.

use std::path::PathBuf;

use memstead_base::EngineError;

/// Errors surfaced by the pro engine extension.
///
/// `Basis(EngineError)` wraps any failure that originates in the
/// underlying basis engine — pro orchestrators that delegate to
/// `memstead_base::Engine` propagate basis errors verbatim through this
/// variant (`#[from]`), so the wire-rendering layer at the pro MCP
/// surface can recover the basis `code()` for any wrapped variant.
///
/// The remaining variants are **lifecycle-only**: they fire from the
/// pro vault management orchestrator (`create_vault` / `delete_vault`)
/// and have no basis-side fire conditions. They live in this crate
/// alongside their orchestrator.
#[derive(Debug, thiserror::Error)]
pub enum ProEngineError {
    /// Wrapped basis-engine error. Use this variant whenever a pro
    /// code path delegates to `memstead_base::Engine` and a basis-side
    /// failure should surface unchanged.
    #[error(transparent)]
    Basis(#[from] EngineError),

    /// `create_vault` / `delete_vault` rejected because the vault
    /// path is not covered by an allowlist rule. `reason` is one of
    /// `no_allowlist_configured` / `no_match` / `outside_workspace`.
    /// `policy_table` names the refusing allowlist —
    /// `"vault_management.create"` or `"vault_management.delete"` —
    /// so an agent recovering from the envelope knows which TOML
    /// table to edit without threading subcommand context through
    /// error handling. The two discriminators are orthogonal: `reason`
    /// names *why* the gate refused; `policy_table` names *which*
    /// gate refused.
    #[error("vault path not allowed by [[{policy_table}]]: {candidate} ({reason})")]
    VaultPathNotAllowed {
        attempted: PathBuf,
        candidate: String,
        patterns: Vec<String>,
        reason: &'static str,
        policy_table: &'static str,
    },

    /// `create_vault` rejected before the allowlist check because the
    /// supplied `name` is structurally malformed — empty, whitespace,
    /// invalid characters, or carries the reserved `__` prefix.
    /// `reason` discriminates the four shapes so an agent who typed
    /// the wrong thing gets a recoverable signal instead of an
    /// allowlist refusal. Split out of the `VaultPathNotAllowed
    /// (no_match)` catch-all so the structural failure modes are
    /// visible.
    #[error("vault name `{name}` is invalid ({reason})")]
    InvalidVaultName {
        name: String,
        reason: &'static str,
    },

    /// `delete_vault` rejected because the workspace
    /// `[cross_vault_links]` policy grants one or more other vaults
    /// permission to write into this one. `referring_vaults` lists the
    /// granting vaults sorted alphabetically so the agent can walk
    /// the policy table. The condition is a *policy grant*, not a
    /// materialised graph edge — revoking the grant in
    /// `.memstead/workspace.toml` is the recovery path.
    #[error(
        "vault {name} cannot be deleted: workspace `[cross_vault_links]` policy grants {referring_vaults:?} write-into permission — revoke that grant and retry"
    )]
    VaultReferencedByPolicy {
        name: String,
        referring_vaults: Vec<String>,
    },

    /// `create_vault` rejected because the matched create-rule does
    /// not allow the requested schema. `allowed_schemas` is the
    /// canonicalised allow-list (each entry `name@version`).
    #[error(
        "schema {requested_schema} not allowed by create-rule {matched_pattern:?} for candidate {candidate:?}"
    )]
    VaultSchemaNotAllowed {
        candidate: String,
        matched_pattern: String,
        requested_schema: String,
        allowed_schemas: Vec<String>,
    },

    /// `create_vault` rejected because the target `.memstead/config.json`
    /// already exists at the requested location — the engine never
    /// silently overwrites a prior attempt.
    #[error("config already exists at {path}")]
    ConfigAlreadyExists { path: PathBuf },

    /// `create_vault` detected on-disk storage residue for the
    /// requested branch path that is not reflected in the in-memory
    /// vault router — typically left over by a crash or a
    /// partially-failed delete. The caller must select an
    /// explicit recovery action via [`VaultCreateParams::recovery`]
    /// (`Reattach`, `ForceOverwrite`, or `HardCleanupFirst`) and
    /// retry; the special case of `unregistered_at`-tombstoned
    /// residue (deliberate operator state from `memstead vault
    /// unregister`) defaults to `Reattach` without this refusal. The
    /// payload carries the composed branch ref, the config-blob path,
    /// and the entity count of the residual data so the caller can
    /// decide between adopting and discarding.
    #[error(
        "vault storage residue detected at branch `{branch_ref}`: \
         {entity_count} entities preserved from a prior session — \
         re-run with `recovery: reattach` to adopt, `recovery: \
         force_overwrite` to destroy, or `recovery: \
         hard_cleanup_first` to refuse until `memstead vault delete` is run"
    )]
    VaultStorageResidueDetected {
        /// Composed branch reference (`refs/heads/<branch_leaf>`)
        /// that carries the residue.
        branch_ref: String,
        /// Tree path of the `__MEMSTEAD:vaults/<branch_leaf>/config.json`
        /// blob (or `None` when the branch exists but the config blob
        /// has already been pruned).
        config_blob: Option<String>,
        /// Best-effort entity count on the residual branch. Reads
        /// the branch's tip tree and counts `.md` entries; `0` when
        /// the count is unavailable.
        entity_count: usize,
    },
}

/// Recovery shape for `create_vault` against pre-existing storage
/// residue. A single enum field with three variants structurally
/// enforces mutual exclusion on the wire (a three-boolean shape would
/// need a runtime-validation step instead).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Adopt the residual entities and register the existing branch
    /// as a fresh writable mount. The seed-commit step is skipped —
    /// the prior session's history is preserved unchanged. Emits a
    /// `VaultReattachedAfterUnregister` warning when the residue
    /// carries an `unregistered_at` tombstone (audit signal).
    Reattach,
    /// Destroy the residual branch + `__MEMSTEAD` config blob (and any
    /// tombstone) in one ref-edit transaction, then proceed with
    /// the normal create path. The prior entities are gone.
    ForceOverwrite,
    /// Refuse with a typed code instructing the caller to run
    /// `memstead vault delete <name>` first. Hard barrier against
    /// destructive auto-recovery even with an explicit recovery
    /// flag — for operators who want the residue cleanup to be a
    /// separate, named operation.
    HardCleanupFirst,
}

impl RecoveryAction {
    /// Wire-token rendering (`reattach` / `force_overwrite` /
    /// `hard_cleanup_first`). Stable across the surface — used by
    /// the MCP serde tag, the CLI flag bridge, and error-envelope
    /// rendering.
    pub fn as_wire_str(&self) -> &'static str {
        match self {
            RecoveryAction::Reattach => "reattach",
            RecoveryAction::ForceOverwrite => "force_overwrite",
            RecoveryAction::HardCleanupFirst => "hard_cleanup_first",
        }
    }
}

impl ProEngineError {
    /// Render rich, fully-inlined recovery prose for the agent-visible
    /// text channel. Closes the asymmetry where structured `details.X`
    /// fields stayed off the agent's text channel. Each lifecycle
    /// variant with a structured list (`patterns`, `referring_vaults`,
    /// `allowed_schemas`) inlines the full payload; basis wraps
    /// delegate to [`EngineError::prose_render`]; trivial variants
    /// fall back to `Display`.
    pub fn prose_render(&self) -> String {
        match self {
            ProEngineError::Basis(inner) => inner.prose_render(),
            ProEngineError::VaultPathNotAllowed {
                attempted,
                candidate,
                patterns,
                reason,
                policy_table,
            } => {
                let patterns_inline = if patterns.is_empty() {
                    "(no rules configured)".to_string()
                } else {
                    patterns.iter().map(|p| format!("'{p}'")).collect::<Vec<_>>().join(", ")
                };
                format!(
                    "vault path not allowed by `[[{policy_table}]]`: candidate '{candidate}' (resolved location '{}') did not match any allowlist rule (reason: {reason}). Configured patterns: {patterns_inline}.",
                    attempted.display()
                )
            }
            ProEngineError::VaultSchemaNotAllowed {
                candidate,
                matched_pattern,
                requested_schema,
                allowed_schemas,
            } => {
                let allowed_inline = if allowed_schemas.is_empty() {
                    "(none)".to_string()
                } else {
                    allowed_schemas.join(", ")
                };
                format!(
                    "schema '{requested_schema}' not allowed by create-rule '{matched_pattern}' for candidate '{candidate}' — allowed schemas: {allowed_inline}. Pick a schema from this list or add a new `[[vault_management.create]]` rule covering this candidate."
                )
            }
            ProEngineError::VaultReferencedByPolicy {
                name,
                referring_vaults,
            } => {
                let inline = if referring_vaults.is_empty() {
                    "(none)".to_string()
                } else {
                    referring_vaults.join(", ")
                };
                format!(
                    "vault {name} cannot be deleted: workspace `[cross_vault_links]` policy grants the following vaults write-into permission: {inline}. Revoke each grant (`memstead_workspace_revoke_cross_link`) and retry."
                )
            }
            // InvalidVaultName, ConfigAlreadyExists, VaultStorageResidueDetected:
            // `Display` already inlines every field; fall back.
            _ => self.to_string(),
        }
    }

    /// Variant-specific recovery payload, rendered as a structured
    /// JSON object that surfaces under `error.details` in MCP / CLI
    /// envelopes. The CLI's vault commands used to discard the engine's
    /// structured details because the lift code didn't have a single
    /// source of truth —
    /// this mirrors `EngineError::details()` so the lift can call
    /// `err.details()` directly without hand-maintaining each per-
    /// variant payload at the CLI surface.
    ///
    /// `Basis(inner)` delegates to `EngineError::details()`. Lifecycle
    /// variants return the same JSON object shape `pro_engine_err_unified`
    /// builds on the MCP wire — both surfaces share the payload here
    /// so they cannot drift.
    pub fn details(&self) -> serde_json::Value {
        match self {
            ProEngineError::Basis(inner) => inner.details(),
            ProEngineError::VaultPathNotAllowed {
                attempted,
                candidate,
                patterns,
                reason,
                policy_table,
            } => serde_json::json!({
                "attempted": attempted.display().to_string(),
                "candidate": candidate,
                "patterns": patterns,
                "reason": reason,
                "policy_table": policy_table,
            }),
            ProEngineError::InvalidVaultName { name, reason } => {
                serde_json::json!({ "name": name, "reason": reason })
            }
            ProEngineError::VaultReferencedByPolicy {
                name,
                referring_vaults,
            } => serde_json::json!({
                "name": name,
                "referring_vaults": referring_vaults,
            }),
            ProEngineError::VaultSchemaNotAllowed {
                candidate,
                matched_pattern,
                requested_schema,
                allowed_schemas,
            } => serde_json::json!({
                "candidate": candidate,
                "matched_pattern": matched_pattern,
                "requested_schema": requested_schema,
                "allowed_schemas": allowed_schemas,
            }),
            ProEngineError::ConfigAlreadyExists { path } => serde_json::json!({
                "path": path.display().to_string(),
                "reason": "config_already_exists",
            }),
            ProEngineError::VaultStorageResidueDetected {
                branch_ref,
                config_blob,
                entity_count,
            } => serde_json::json!({
                "branch_ref": branch_ref,
                "config_blob": config_blob,
                "entity_count": entity_count,
                "recovery": ["reattach", "force_overwrite", "hard_cleanup_first"],
            }),
        }
    }

    /// Stable, surface-independent error code token.
    ///
    /// Matches `memstead_base::EngineError::code()` for every variant —
    /// wrapped basis errors delegate to the basis mapping, lifecycle
    /// variants return the exact strings the basis enum returned for
    /// them today. This is load-bearing: the wire-shape pins in
    /// `memstead-mcp/tests/wire_shape.rs` assert these exact code strings.
    pub fn code(&self) -> &'static str {
        match self {
            ProEngineError::Basis(e) => e.code(),
            ProEngineError::VaultPathNotAllowed { .. } => "VAULT_PATH_NOT_ALLOWED",
            ProEngineError::InvalidVaultName { .. } => "INVALID_VAULT_NAME",
            ProEngineError::VaultReferencedByPolicy { .. } => "VAULT_REFERENCED_BY_POLICY",
            ProEngineError::VaultSchemaNotAllowed { .. } => "VAULT_SCHEMA_NOT_ALLOWED",
            ProEngineError::ConfigAlreadyExists { .. } => "CONFIG_ERROR",
            ProEngineError::VaultStorageResidueDetected { .. } => "VAULT_STORAGE_RESIDUE_DETECTED",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Code strings track the wire vocabulary the pro MCP surface
    /// publishes. `VAULT_REFERENCED_BY_POLICY` was renamed from the
    /// pre-04 `VAULT_HAS_REFERENCES` so the typed code matches the
    /// actual fire condition (a workspace `[cross_vault_links]` grant,
    /// not a materialised graph edge); the other three lifecycle
    /// codes are unchanged from when these variants lived on
    /// `memstead_base::EngineError`.
    #[test]
    fn lifecycle_codes_pin_wire_vocabulary() {
        let e = ProEngineError::VaultPathNotAllowed {
            attempted: PathBuf::from("/x"),
            candidate: "x".into(),
            patterns: vec![],
            reason: "no_match",
            policy_table: "vault_management.create",
        };
        assert_eq!(e.code(), "VAULT_PATH_NOT_ALLOWED");

        let e = ProEngineError::VaultReferencedByPolicy {
            name: "x".into(),
            referring_vaults: vec![],
        };
        assert_eq!(e.code(), "VAULT_REFERENCED_BY_POLICY");

        let e = ProEngineError::VaultSchemaNotAllowed {
            candidate: "x".into(),
            matched_pattern: "p".into(),
            requested_schema: "s".into(),
            allowed_schemas: vec![],
        };
        assert_eq!(e.code(), "VAULT_SCHEMA_NOT_ALLOWED");

        let e = ProEngineError::ConfigAlreadyExists {
            path: PathBuf::from("/x"),
        };
        assert_eq!(e.code(), "CONFIG_ERROR");
    }

    /// Wrapped basis errors delegate `code()` to the basis mapping.
    /// Any drift in the basis enum's code strings rolls through this
    /// path automatically — the pro layer never re-stringifies.
    #[test]
    fn wrapped_basis_error_delegates_code() {
        let e: ProEngineError = EngineError::UnknownVault("specs".into()).into();
        assert_eq!(e.code(), "UNKNOWN_VAULT");
    }

    /// The `policy_table` field disambiguates which allowlist refused
    /// without forcing an agent to thread subcommand context through
    /// error handling. The structured `details` payload and the
    /// `prose_render` text both surface the table name.
    #[test]
    fn vault_path_not_allowed_carries_policy_table_in_details_and_prose() {
        let create_err = ProEngineError::VaultPathNotAllowed {
            attempted: PathBuf::from("/ws/scratch-2"),
            candidate: "scratch-2".into(),
            patterns: vec!["specs".into()],
            reason: "no_match",
            policy_table: "vault_management.create",
        };
        let details = create_err.details();
        assert_eq!(details["policy_table"], "vault_management.create");
        assert_eq!(details["reason"], "no_match");
        let prose = create_err.prose_render();
        assert!(
            prose.contains("vault_management.create"),
            "prose must name the refusing allowlist: {prose}"
        );

        // Delete-path symmetric — policy_table flips to the delete table.
        let delete_err = ProEngineError::VaultPathNotAllowed {
            attempted: PathBuf::from("/ws/archive-src"),
            candidate: "archive-src".into(),
            patterns: vec!["specs".into()],
            reason: "no_match",
            policy_table: "vault_management.delete",
        };
        assert_eq!(
            delete_err.details()["policy_table"],
            "vault_management.delete"
        );
        let prose = delete_err.prose_render();
        assert!(
            prose.contains("vault_management.delete"),
            "prose must name the refusing allowlist: {prose}"
        );
    }
}
