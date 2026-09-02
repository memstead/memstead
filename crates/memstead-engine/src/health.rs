//! The `memstead_health` composer, re-exported from `memstead-base` so the
//! full engine's callers keep their import path. The composer lives in
//! the base crate because every renderer of the health report (the MCP
//! server, the CLI in both its full and lean builds) must build the same
//! bytes from one implementation (backlog-engine plan A7).

pub use memstead_base::ops::health_compose::{
    ComposeHealthError, HealthArgs, HealthConfig, compose_health, render_health_markdown,
};
