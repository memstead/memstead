//! The one health assembly every surface calls: the kernel composer plus
//! the maintenance loop's own axis. The kernel's `compose_health` takes the
//! loop's warnings as data it never interprets; this function computes them
//! (the unanchored-mention findings the bindings' verify passes recorded)
//! and hands the whole report back. CLI, MCP and every embedder compose
//! health through here, so the loop's contribution can never be forgotten
//! by one surface and kept by another — the parity pin between the CLI's
//! JSON and the MCP `structured_content` rests on it.

use crate::ops::health_compose::{ComposeHealthError, HealthArgs, HealthConfig};

/// Compose the complete health payload — see the module doc.
pub fn compose_health(
    engine: &mut crate::Engine,
    args: &HealthArgs,
    drift_warnings: Vec<crate::WarningHint>,
    config: &HealthConfig,
) -> Result<serde_json::Value, ComposeHealthError> {
    let loop_warnings = super::findings::unanchored_mention_warnings(engine);
    crate::ops::health_compose::compose_health(engine, args, drift_warnings, loop_warnings, config)
}
