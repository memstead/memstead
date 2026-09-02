//! The CLI's axis-coverage registry: every subcommand a caller can
//! reach declares either which axes its clean verdict examined or why
//! it emits no verdict at all. The rule, its vocabulary, and the
//! validator live in `memstead_base::ops::coverage`; this module is
//! the CLI consumer's declaration, and the test at the bottom is the
//! gate: it walks the live clap tree, so a new subcommand fails here
//! until it declares, and a new axis fails every verdict row that
//! has not met it.

use memstead_base::ops::coverage::{AxisCoverage, CoverageDisposition, SurfaceCoverage};

// Shared no-verdict reasons. One string per surface family, because
// the reason is the same fact each time; the rows stay one per
// surface so a departed subcommand fails as its own stale entry.
const READS_DATA: &str = "returns data, not a verdict; an empty result is an empty \
     result, never an all-clear";
const MUTATION: &str = "mutation surface: reports what it did, never an all-clear \
     over unexamined state";
#[cfg(feature = "mem-repo")]
const TRANSPORT: &str = "transport operation: reports the transfer's own outcome";
const ACCOUNT_OP: &str = "registry or account operation: reports the operation's own \
     outcome";

// Shared exclusion reasons for the verdict rows.
const STATUS_SCOPE: &str = "outside the status rollup, whose verdict answers for \
     declared projection bindings only";
const ANCHORS_ONLY: &str = "the standalone anchor statement answers for anchors \
     alone; it examines no other axis and says so";
const VERIFY_SCOPE: &str = "the binding-scoped fidelity report answers for one \
     binding's projection and its anchors; other axes belong to health";
#[cfg(feature = "mem-repo")]
const DUMP_SCOPE: &str = "the dump is a configuration and roster snapshot; graph \
     axes belong to health";

/// `memstead health` under `--strict`: exit 0 is the clean verdict,
/// and it answers exactly for the promoted set, always-on
/// configuration and mount axes plus the include-gated promotions.
/// Everything advisory by the strict contract is excluded by name.
pub const HEALTH: SurfaceCoverage = SurfaceCoverage {
    surface: "health",
    // Shared content (memstead_base::ops::coverage::HEALTH_COVERAGE):
    // the same declaration the MCP composer stamps, so the CLI's
    // strict verdict and the composed report cannot diverge.
    disposition: CoverageDisposition::Verdict(memstead_base::ops::coverage::HEALTH_COVERAGE),
};

pub const STATUS: SurfaceCoverage = SurfaceCoverage {
    surface: "status",
    disposition: CoverageDisposition::Verdict(AxisCoverage {
        examined: &["projection"],
        advisory: &[],
        not_examined: &[
            ("orphans", STATUS_SCOPE),
            ("stubs", STATUS_SCOPE),
            ("most_connected", STATUS_SCOPE),
            ("missing_fields", STATUS_SCOPE),
            ("stale", STATUS_SCOPE),
            ("dangling_links", STATUS_SCOPE),
            ("tags", STATUS_SCOPE),
            ("missing_required_outgoing", STATUS_SCOPE),
            ("constraints", STATUS_SCOPE),
            ("signals", STATUS_SCOPE),
            ("labelling", STATUS_SCOPE),
            ("conformance", STATUS_SCOPE),
            ("integrity", STATUS_SCOPE),
            ("config", STATUS_SCOPE),
            ("anchors", STATUS_SCOPE),
            ("friction", STATUS_SCOPE),
            ("open_questions", STATUS_SCOPE),
            ("vital_signs", STATUS_SCOPE),
            ("stale_derivations", STATUS_SCOPE),
            ("checks", STATUS_SCOPE),
            ("ledger", STATUS_SCOPE),
            ("mounts", STATUS_SCOPE),
        ],
    }),
};

const OVERVIEW: SurfaceCoverage = SurfaceCoverage {
    surface: "overview",
    // The content is the shared constant the composer itself stamps
    // into the overview frontmatter, so registry and output cannot
    // diverge.
    disposition: CoverageDisposition::Verdict(memstead_base::ops::coverage::OVERVIEW_COVERAGE),
};

pub const VERIFY_ANCHORS: SurfaceCoverage = SurfaceCoverage {
    surface: "verify-anchors",
    disposition: CoverageDisposition::Verdict(AxisCoverage {
        examined: &["anchors"],
        advisory: &[],
        not_examined: &[
            ("orphans", ANCHORS_ONLY),
            ("stubs", ANCHORS_ONLY),
            ("most_connected", ANCHORS_ONLY),
            ("missing_fields", ANCHORS_ONLY),
            ("stale", ANCHORS_ONLY),
            ("dangling_links", ANCHORS_ONLY),
            ("tags", ANCHORS_ONLY),
            ("missing_required_outgoing", ANCHORS_ONLY),
            ("constraints", ANCHORS_ONLY),
            ("signals", ANCHORS_ONLY),
            ("labelling", ANCHORS_ONLY),
            ("conformance", ANCHORS_ONLY),
            ("integrity", ANCHORS_ONLY),
            ("config", ANCHORS_ONLY),
            ("friction", ANCHORS_ONLY),
            ("open_questions", ANCHORS_ONLY),
            ("vital_signs", ANCHORS_ONLY),
            ("stale_derivations", ANCHORS_ONLY),
            ("checks", ANCHORS_ONLY),
            ("ledger", ANCHORS_ONLY),
            ("projection", ANCHORS_ONLY),
            ("mounts", ANCHORS_ONLY),
        ],
    }),
};

pub const PROJECTION_VERIFY: SurfaceCoverage = SurfaceCoverage {
    surface: "projection verify",
    disposition: CoverageDisposition::Verdict(AxisCoverage {
        examined: &["projection", "anchors"],
        advisory: &[],
        not_examined: &[
            ("orphans", VERIFY_SCOPE),
            ("stubs", VERIFY_SCOPE),
            ("most_connected", VERIFY_SCOPE),
            ("missing_fields", VERIFY_SCOPE),
            ("stale", VERIFY_SCOPE),
            ("dangling_links", VERIFY_SCOPE),
            ("tags", VERIFY_SCOPE),
            ("missing_required_outgoing", VERIFY_SCOPE),
            ("constraints", VERIFY_SCOPE),
            ("signals", VERIFY_SCOPE),
            ("labelling", VERIFY_SCOPE),
            ("conformance", VERIFY_SCOPE),
            ("integrity", VERIFY_SCOPE),
            ("config", VERIFY_SCOPE),
            ("friction", VERIFY_SCOPE),
            ("open_questions", VERIFY_SCOPE),
            ("vital_signs", VERIFY_SCOPE),
            ("stale_derivations", VERIFY_SCOPE),
            ("checks", VERIFY_SCOPE),
            ("ledger", VERIFY_SCOPE),
            ("mounts", VERIFY_SCOPE),
        ],
    }),
};

#[cfg(feature = "mem-repo")]
pub const WORKSPACE_DUMP: SurfaceCoverage = SurfaceCoverage {
    surface: "workspace dump",
    disposition: CoverageDisposition::Verdict(AxisCoverage {
        examined: &["mounts", "config"],
        advisory: &[],
        not_examined: &[
            ("orphans", DUMP_SCOPE),
            ("stubs", DUMP_SCOPE),
            ("most_connected", DUMP_SCOPE),
            ("missing_fields", DUMP_SCOPE),
            ("stale", DUMP_SCOPE),
            ("dangling_links", DUMP_SCOPE),
            ("tags", DUMP_SCOPE),
            ("missing_required_outgoing", DUMP_SCOPE),
            ("constraints", DUMP_SCOPE),
            ("signals", DUMP_SCOPE),
            ("labelling", DUMP_SCOPE),
            ("conformance", DUMP_SCOPE),
            ("integrity", DUMP_SCOPE),
            ("anchors", DUMP_SCOPE),
            ("friction", DUMP_SCOPE),
            ("open_questions", DUMP_SCOPE),
            ("vital_signs", DUMP_SCOPE),
            ("stale_derivations", DUMP_SCOPE),
            ("checks", DUMP_SCOPE),
            ("ledger", DUMP_SCOPE),
            (
                "projection",
                "binding fidelity is answered by status and projection verify",
            ),
        ],
    }),
};

fn no_verdict(surface: &'static str, reason: &'static str) -> SurfaceCoverage {
    SurfaceCoverage {
        surface,
        disposition: CoverageDisposition::NoVerdict(reason),
    }
}

/// Every CLI surface's coverage row. Names are the clap path exactly
/// as the walk below produces it ("workspace dump", not "dump").
/// Feature-gated commands carry the same gate as their clap variant,
/// so the lean build's registry matches the lean build's walk.
pub fn surface_registry() -> Vec<SurfaceCoverage> {
    #[cfg_attr(not(feature = "mem-repo"), allow(unused_mut))]
    let mut rows = vec![
        STATUS,
        HEALTH,
        OVERVIEW,
        VERIFY_ANCHORS,
        PROJECTION_VERIFY,
        // Read surfaces that return data rather than a verdict.
        no_verdict("entity", READS_DATA),
        no_verdict("relations", READS_DATA),
        no_verdict("search", READS_DATA),
        no_verdict("list", READS_DATA),
        no_verdict("context", READS_DATA),
        no_verdict("type", READS_DATA),
        no_verdict("due", READS_DATA),
        no_verdict("gates", READS_DATA),
        no_verdict("export", READS_DATA),
        no_verdict("changes", READS_DATA),
        no_verdict("anchors", READS_DATA),
        no_verdict("conflicts list", READS_DATA),
        no_verdict("review-mark list", READS_DATA),
        no_verdict("review-mark diff", READS_DATA),
        no_verdict("projection brief", READS_DATA),
        no_verdict("projection check-path", READS_DATA),
        // The check ledger: the one surface deliberately outside the
        // rule, because its verdict is the caller's claim about the
        // caller's own work, never the engine's claim about state the
        // engine examined.
        no_verdict(
            "check",
            "records the caller's verdict about the caller's own work into the \
             append-only ledger; the engine derives no verdict of its own",
        ),
        // Schema tooling verdicts are total over the caller-named
        // input, so no workspace axis is claimed.
        no_verdict(
            "schema validate",
            "validates the caller-named schema package; the verdict is total over \
             exactly that input and claims no workspace axis",
        ),
        no_verdict("schema new", MUTATION),
        no_verdict("schema install", MUTATION),
        no_verdict(
            "schema migrate",
            "previews or applies rewrites of the caller-named schema package; reports \
             the rewrites, never an all-clear over any workspace axis",
        ),
        // Mutations and setup.
        no_verdict("create", MUTATION),
        no_verdict("update", MUTATION),
        no_verdict("relate", MUTATION),
        no_verdict("delete", MUTATION),
        no_verdict("rename", MUTATION),
        no_verdict("retype", MUTATION),
        no_verdict("conflicts resolve", MUTATION),
        no_verdict("review-mark set", MUTATION),
        no_verdict("review-mark clear", MUTATION),
        no_verdict("reload", MUTATION),
        no_verdict("init", MUTATION),
        no_verdict("quickstart", MUTATION),
        no_verdict("projection init", MUTATION),
        no_verdict("projection migrate", MUTATION),
        no_verdict("projection enable", MUTATION),
        no_verdict("projection edit", MUTATION),
        no_verdict("projection advance", MUTATION),
        no_verdict("projection exclude", MUTATION),
        // Registry and account operations.
        no_verdict("publish", ACCOUNT_OP),
        no_verdict("unpublish", ACCOUNT_OP),
        no_verdict("login", ACCOUNT_OP),
        no_verdict("logout", ACCOUNT_OP),
        no_verdict("domain keygen", ACCOUNT_OP),
        no_verdict("domain manifest", ACCOUNT_OP),
        no_verdict("admin takedown", ACCOUNT_OP),
        no_verdict("admin denylist", ACCOUNT_OP),
    ];
    #[cfg(feature = "mem-repo")]
    rows.extend([
        WORKSPACE_DUMP,
        no_verdict("install", MUTATION),
        no_verdict("uninstall", MUTATION),
        no_verdict("batch-update", MUTATION),
        no_verdict("batch-create", MUTATION),
        no_verdict("batch-relate", MUTATION),
        no_verdict("recover", MUTATION),
        no_verdict("fetch", TRANSPORT),
        no_verdict("pull", TRANSPORT),
        no_verdict("push", TRANSPORT),
        no_verdict("branch-reset", TRANSPORT),
        no_verdict("mem init", MUTATION),
        no_verdict("mem unregister", MUTATION),
        no_verdict("mem delete", MUTATION),
        no_verdict("mem rename", MUTATION),
        no_verdict("mem set-version", MUTATION),
        no_verdict("mem set-schema", MUTATION),
        no_verdict("mem set-description", MUTATION),
        no_verdict("mem set-title", MUTATION),
        no_verdict("mem set-subject", MUTATION),
        no_verdict("mem set-sync-state", MUTATION),
        no_verdict("mem set-internal", MUTATION),
        no_verdict("mem list", READS_DATA),
        no_verdict("mem-repo init", MUTATION),
        no_verdict("mem-repo remote-add", MUTATION),
        no_verdict("workspace show", READS_DATA),
        no_verdict("workspace allow-create", MUTATION),
        no_verdict("workspace revoke-create", MUTATION),
        no_verdict("workspace allow-delete", MUTATION),
        no_verdict("workspace revoke-delete", MUTATION),
        no_verdict("workspace grant-cross-link", MUTATION),
        no_verdict("workspace revoke-cross-link", MUTATION),
        no_verdict("workspace set-mutations", MUTATION),
    ]);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use memstead_base::ops::coverage::{validate_coverage, verdict_axes};

    /// Walk the live clap tree to its leaves. This is the discovery
    /// half of the gate: the roster comes from the binary's own
    /// command definition, never from a hand-kept list, so a
    /// subcommand cannot land outside the registry's sight.
    fn discovered_surfaces() -> Vec<String> {
        fn walk(cmd: &clap::Command, prefix: &str, out: &mut Vec<String>) {
            let mut leaves = 0;
            for sub in cmd.get_subcommands() {
                if sub.get_name() == "help" {
                    continue;
                }
                leaves += 1;
                let path = if prefix.is_empty() {
                    sub.get_name().to_string()
                } else {
                    format!("{prefix} {}", sub.get_name())
                };
                walk(sub, &path, out);
            }
            if leaves == 0 && !prefix.is_empty() {
                out.push(prefix.to_string());
            }
        }
        let mut out = Vec::new();
        let cmd = crate::cli::Cli::command();
        walk(&cmd, "", &mut out);
        out
    }

    /// The gate. A clean run means: every discoverable subcommand
    /// has a row, every verdict row speaks to every axis in the
    /// vocabulary, and no row is stale.
    #[test]
    fn every_cli_surface_declares_its_coverage() {
        let discovered = discovered_surfaces();
        let discovered_refs: Vec<&str> = discovered.iter().map(|s| s.as_str()).collect();
        let vocab = verdict_axes();
        let registry = surface_registry();
        let findings = validate_coverage(&vocab, &registry, &discovered_refs);
        assert!(
            findings.is_empty(),
            "{} coverage finding(s):\n{}",
            findings.len(),
            findings.join("\n")
        );
    }
}
