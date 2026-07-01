//! Pro-flavor engine extension for Memstead.
//!
//! This crate is the source-boundary home for code that the basis
//! engine (`memstead-base`) must not carry. The boundary cut targets
//! multi-vault lifecycle and the engine-only `EngineError` variants.
//!
//! Today the crate hosts:
//! - The typed pro-error envelope ([`ProEngineError`]) — wraps
//!   `memstead_base::EngineError` and carries the lifecycle-only variants
//!   ([`error::ProEngineError::VaultPathNotAllowed`],
//!   [`error::ProEngineError::VaultReferencedByPolicy`],
//!   [`error::ProEngineError::VaultSchemaNotAllowed`],
//!   [`error::ProEngineError::ConfigAlreadyExists`]).
//! - The vault-lifecycle orchestrators ([`vault_management::create_vault`],
//!   [`vault_management::delete_vault`]) and their param/response
//!   types. They consume `&mut memstead_base::Engine` directly; pro
//!   contributes lifecycle as free functions over the basis engine
//!   rather than via a wrapper struct or a policy-provider trait.
//!
//! The matcher primitives ([`memstead_base::CreateRuleSet`],
//! [`memstead_base::DeleteRuleSet`], [`memstead_base::MatcherSet`]) stay in
//! basis — the basis engine's `cross_vault_link_allowed` synthesises a
//! `CreateRuleSet` on multi-folder workspaces, so they are a basis
//! policy primitive used by both flavors.
//!
//! Lifecycle functions currently return `Result<_, memstead_base::EngineError>`
//! so the pro-MCP server's `engine_err_unified` mapper continues to
//! consume them unchanged. A follow-up commit switches the return type
//! to `Result<_, ProEngineError>` and drops the four lifecycle-only
//! variants from `memstead_base::EngineError`.

pub mod error;
pub mod health;
pub mod overview;
pub mod vault_management;
pub mod workspace_config_edit;

pub use error::{ProEngineError, RecoveryAction};
pub use health::{ComposeHealthError, HealthArgs, HealthConfig, compose_health};
pub use overview::{
    ALLOWED_OVERVIEW_INCLUDE_KEYS, ComposeOverviewError, DEFAULT_OVERVIEW_BUDGET, OverviewArgs,
    OverviewOutput, Surface, compose_overview,
};
