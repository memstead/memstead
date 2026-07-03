//! Full-flavor engine extension for Memstead.
//!
//! This crate is the source-boundary home for code that the lean
//! engine (`memstead-base`) must not carry. The boundary cut targets
//! multi-mem lifecycle and the engine-only `EngineError` variants.
//!
//! Today the crate hosts:
//! - The typed full-error envelope ([`FullEngineError`]) — wraps
//!   `memstead_base::EngineError` and carries the lifecycle-only variants
//!   ([`error::FullEngineError::MemPathNotAllowed`],
//!   [`error::FullEngineError::MemReferencedByPolicy`],
//!   [`error::FullEngineError::MemSchemaNotAllowed`],
//!   [`error::FullEngineError::ConfigAlreadyExists`]).
//! - The mem-lifecycle orchestrators ([`mem_management::create_mem`],
//!   [`mem_management::delete_mem`]) and their param/response
//!   types. They consume `&mut memstead_base::Engine` directly; full
//!   contributes lifecycle as free functions over the lean engine
//!   rather than via a wrapper struct or a policy-provider trait.
//!
//! The matcher primitives ([`memstead_base::CreateRuleSet`],
//! [`memstead_base::DeleteRuleSet`], [`memstead_base::MatcherSet`]) stay in
//! lean — the lean engine's `cross_mem_link_allowed` synthesises a
//! `CreateRuleSet` on multi-folder workspaces, so they are a lean
//! policy primitive used by both flavors.
//!
//! Lifecycle functions currently return `Result<_, memstead_base::EngineError>`
//! so the full-MCP server's `engine_err_unified` mapper continues to
//! consume them unchanged. A follow-up commit switches the return type
//! to `Result<_, FullEngineError>` and drops the four lifecycle-only
//! variants from `memstead_base::EngineError`.

pub mod error;
pub mod health;
pub mod overview;
pub mod mem_management;
pub mod workspace_config_edit;

pub use error::{FullEngineError, RecoveryAction};
pub use health::{ComposeHealthError, HealthArgs, HealthConfig, compose_health};
pub use overview::{
    ALLOWED_OVERVIEW_INCLUDE_KEYS, ComposeOverviewError, DEFAULT_OVERVIEW_BUDGET, OverviewArgs,
    OverviewOutput, Surface, compose_overview,
};
