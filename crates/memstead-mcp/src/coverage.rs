//! The MCP servers' axis-coverage registries: every tool an agent can
//! call declares either which axes its clean-reading output examined
//! or why it emits no verdict at all. The rule, its vocabulary, and
//! the validator live in `memstead_base::ops::coverage`; the gate
//! test (`tests/axis_coverage.rs`) walks each server's live tool
//! router, so a new tool fails until it declares, and a new axis
//! fails every verdict row that has not met it. Both flavours are
//! covered: the full server and the lean filesystem server carry
//! separate registries because their rosters differ, and each is
//! held against its own router's walk.

use memstead_base::ops::coverage::{CoverageDisposition, SurfaceCoverage};

const READS_DATA: &str = "returns data, not a verdict; an empty result is an empty \
     result, never an all-clear";
const MUTATION: &str = "mutation surface: reports what it did, never an all-clear \
     over unexamined state";

/// `memstead_health`: the report treats these axes' findings as
/// defects (warnings, integrity findings, configuration and mount
/// conditions), so an empty defect statement reads as an all-clear
/// exactly over them. Everything descriptive or advisory is excluded
/// by name.
pub const HEALTH: SurfaceCoverage = SurfaceCoverage {
    surface: "memstead_health",
    // Shared content (memstead_base::ops::coverage::HEALTH_COVERAGE):
    // the composer stamps the same declaration into the payload, so
    // registry and output cannot diverge.
    disposition: CoverageDisposition::Verdict(memstead_base::ops::coverage::HEALTH_COVERAGE),
};

const OVERVIEW: SurfaceCoverage = SurfaceCoverage {
    surface: "memstead_overview",
    // The content is the shared constant the composer itself stamps
    // into the overview frontmatter, so registry and output cannot
    // diverge.
    disposition: CoverageDisposition::Verdict(memstead_base::ops::coverage::OVERVIEW_COVERAGE),
};

fn no_verdict(surface: &'static str, reason: &'static str) -> SurfaceCoverage {
    SurfaceCoverage {
        surface,
        disposition: CoverageDisposition::NoVerdict(reason),
    }
}

/// The thirteen tools both server flavours expose.
fn shared_rows() -> Vec<SurfaceCoverage> {
    vec![
        HEALTH,
        OVERVIEW,
        no_verdict("memstead_entity", READS_DATA),
        no_verdict("memstead_search", READS_DATA),
        no_verdict("memstead_schema", READS_DATA),
        no_verdict("memstead_diff", READS_DATA),
        no_verdict("memstead_changes_since", READS_DATA),
        no_verdict("memstead_create", MUTATION),
        no_verdict("memstead_update", MUTATION),
        no_verdict("memstead_relate", MUTATION),
        no_verdict("memstead_delete", MUTATION),
        no_verdict("memstead_rename", MUTATION),
        no_verdict("memstead_retype", MUTATION),
        // The one surface deliberately outside the rule: its verdict
        // is the caller's claim about the caller's own work, never
        // the engine's claim about state the engine examined.
        no_verdict(
            "memstead_check",
            "records the caller's verdict about the caller's own work into the \
             append-only ledger; the engine derives no verdict of its own",
        ),
    ]
}

/// The lean filesystem server's registry.
pub fn lean_registry() -> Vec<SurfaceCoverage> {
    shared_rows()
}

/// The full server's registry: the shared roster plus reload and the
/// mem-lifecycle family.
#[cfg(feature = "mem-repo")]
pub fn full_registry() -> Vec<SurfaceCoverage> {
    let mut rows = shared_rows();
    rows.extend([
        no_verdict("memstead_reload", MUTATION),
        no_verdict("memstead_mem_configure", MUTATION),
        no_verdict("memstead_mem_create", MUTATION),
        no_verdict("memstead_mem_delete", MUTATION),
        no_verdict("memstead_mem_set_schema", MUTATION),
        no_verdict("memstead_mem_set_version", MUTATION),
    ]);
    rows
}
