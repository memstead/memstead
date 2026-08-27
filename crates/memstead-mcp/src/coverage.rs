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

use memstead_base::ops::coverage::{AxisCoverage, CoverageDisposition, SurfaceCoverage};

const READS_DATA: &str = "returns data, not a verdict; an empty result is an empty \
     result, never an all-clear";
const MUTATION: &str = "mutation surface: reports what it did, never an all-clear \
     over unexamined state";

const HEALTH_SCOPE: &str = "descriptive or advisory in the health report; the \
     report's defect statement does not answer for this axis";
const OVERVIEW_SCOPE: &str = "overview is a descriptive composition; its only \
     all-clear claim is that the roster it renders is complete and its mounts serve";

/// `memstead_health`: the report treats these axes' findings as
/// defects (warnings, integrity findings, configuration and mount
/// conditions), so an empty defect statement reads as an all-clear
/// exactly over them. Everything descriptive or advisory is excluded
/// by name.
const HEALTH: SurfaceCoverage = SurfaceCoverage {
    surface: "memstead_health",
    disposition: CoverageDisposition::Verdict(AxisCoverage {
        examined: &[
            "dangling_links",
            "missing_required_outgoing",
            "constraints",
            "signals",
            "integrity",
            "config",
            "mounts",
        ],
        excluded: &[
            ("orphans", HEALTH_SCOPE),
            ("stubs", HEALTH_SCOPE),
            ("most_connected", HEALTH_SCOPE),
            ("missing_fields", HEALTH_SCOPE),
            ("stale", HEALTH_SCOPE),
            ("tags", HEALTH_SCOPE),
            ("labelling", HEALTH_SCOPE),
            (
                "conformance",
                "reported per entity beside the defect statement, never folded into it",
            ),
            (
                "anchors",
                "drifted anchors stay advisory; the verify surfaces carry the drift statement",
            ),
            ("friction", HEALTH_SCOPE),
            ("open_questions", HEALTH_SCOPE),
            ("stale_derivations", HEALTH_SCOPE),
            (
                "checks",
                "check states are derived views; the verdicts in them belong to their recording callers",
            ),
            ("ledger", HEALTH_SCOPE),
            (
                "projection",
                "projection fidelity is answered by the CLI status and projection verify surfaces",
            ),
        ],
    }),
};

const OVERVIEW: SurfaceCoverage = SurfaceCoverage {
    surface: "memstead_overview",
    disposition: CoverageDisposition::Verdict(AxisCoverage {
        examined: &["mounts", "config"],
        excluded: &[
            ("orphans", OVERVIEW_SCOPE),
            ("stubs", OVERVIEW_SCOPE),
            ("most_connected", OVERVIEW_SCOPE),
            ("missing_fields", OVERVIEW_SCOPE),
            ("stale", OVERVIEW_SCOPE),
            (
                "dangling_links",
                "rendered on request as a listing; the verdict over them is health's",
            ),
            ("tags", OVERVIEW_SCOPE),
            ("missing_required_outgoing", OVERVIEW_SCOPE),
            ("constraints", OVERVIEW_SCOPE),
            ("signals", OVERVIEW_SCOPE),
            ("labelling", OVERVIEW_SCOPE),
            ("conformance", OVERVIEW_SCOPE),
            ("integrity", OVERVIEW_SCOPE),
            ("anchors", OVERVIEW_SCOPE),
            ("friction", OVERVIEW_SCOPE),
            ("open_questions", OVERVIEW_SCOPE),
            ("stale_derivations", OVERVIEW_SCOPE),
            ("checks", OVERVIEW_SCOPE),
            ("ledger", OVERVIEW_SCOPE),
            ("projection", OVERVIEW_SCOPE),
        ],
    }),
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
