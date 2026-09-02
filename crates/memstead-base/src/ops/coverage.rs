//! Axis-coverage declarations: no read surface reports `clean` over
//! state it did not examine. Where it cannot examine, it says so.
//!
//! WHY: eight findings in one sweep shared a single shape, a surface
//! emitting an all-clear that asserted less than it read as. Strict
//! health promoted a hand-remembered subset of conditions, `status`
//! defaulted to a clean rollup when nothing was declared, the
//! conformance linter could not fail against a mem's own schema, and
//! `workspace dump` silently dropped mounts whose config did not
//! parse. Each instance was fixed; this module is the rule that keeps
//! the class shut. A reader who is told `clean` stops looking, so a
//! clean verdict must carry the set of axes it answers for.
//!
//! THE RULE, STATED ONCE: every surface a caller can read is declared
//! in a per-consumer registry. A surface that emits a clean/ok
//! verdict declares, for EVERY axis in the workspace vocabulary,
//! either that its verdict examined the axis or that the axis is
//! excluded with a stated reason. A surface that emits no verdict
//! declares why not. The declaration states intent, which is exactly
//! what cannot be derived from the code, since the defect the rule
//! closes is surfaces doing less than they claim. Scoped statements
//! are instances of this rule, not siblings of it: the anchor
//! surface saying "reconciliation could not be performed" and the
//! verify rollup's blind-spots list are the per-run refinement of
//! the same obligation the static declaration carries per surface.
//!
//! The vocabulary reuses [`HEALTH_INCLUDE_KEYS`] rather than
//! inventing a parallel axis roster: those keys are already the one
//! shared statement of what the engine can examine, and only the
//! verdict subjects no health include covers are added here.
//!
//! ENFORCEMENT: [`validate_coverage`] is pure and total over data,
//! so the gate can be demonstrated red against synthetic fixtures
//! (a surface clean over an unexamined axis, an axis added without a
//! declaration update) without reconstructing any historical tree.
//! Each consumer crate holds a test that walks its own live surface
//! roster (the clap command tree, the MCP tool router), hands the
//! walk's output to the validator, and fails on any finding. Those
//! tests ride the ordinary `cargo nextest` legs of `run-tests.sh`,
//! the same path every other permanent guard runs on. One surface is
//! deliberately outside the rule: `check` records a caller's verdict
//! about the caller's own work, so its registry entry is
//! [`CoverageDisposition::NoVerdict`], not an examined-axes claim.

use crate::ops::health::HEALTH_INCLUDE_KEYS;

/// Verdict subjects no health include key covers. `projection` is the
/// fidelity axis `status` and `projection verify` answer for;
/// `mounts` is the roster axis `workspace dump` and `overview` answer
/// for (which mounts exist, which serve nothing, and why).
pub const EXTRA_VERDICT_AXES: &[&str] = &["projection", "mounts"];

/// The workspace axis vocabulary: everything a clean verdict can
/// answer for. Composed, never copied, so it cannot drift from the
/// health roster.
pub fn verdict_axes() -> Vec<&'static str> {
    let mut axes: Vec<&'static str> = HEALTH_INCLUDE_KEYS.to_vec();
    axes.extend_from_slice(EXTRA_VERDICT_AXES);
    axes
}

/// One surface's static coverage claim: which axes its verdict
/// answers for, and why the rest are outside its scope. The two
/// lists must jointly name every axis in the vocabulary; a blanket
/// "everything else" clause is deliberately impossible, because it
/// would swallow a newly introduced axis silently, and the one
/// permanent property this module owes is that a new axis fails
/// every declaration that has not met it.
#[derive(Debug, Clone, Copy)]
pub struct AxisCoverage {
    /// Axes the surface's clean verdict actually examined.
    pub examined: &'static [&'static str],
    /// Axes the verdict does not answer for, each with the reason a
    /// reader needs (typically: which surface answers for it instead).
    pub excluded: &'static [(&'static str, &'static str)],
}

/// What a declared surface claims about verdicts.
#[derive(Debug, Clone, Copy)]
pub enum CoverageDisposition {
    /// The surface can emit a clean/ok verdict and declares its axes.
    Verdict(AxisCoverage),
    /// The surface emits no clean/ok verdict; the reason says why the
    /// rule does not bind it (it returns data, it reports what a
    /// mutation did, or its verdict belongs to the caller).
    NoVerdict(&'static str),
}

/// One registry row: a surface name exactly as the consumer's own
/// mechanical walk produces it, plus its disposition.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceCoverage {
    pub surface: &'static str,
    pub disposition: CoverageDisposition,
}

impl AxisCoverage {
    /// The examined set as it is stamped into surface output.
    pub fn examined_wire(&self) -> Vec<&'static str> {
        self.examined.to_vec()
    }

    /// The exclusions as they are stamped into surface output:
    /// `(axis, reason)` pairs, so a reader can see which axes the
    /// verdict does not cover without reading the source.
    pub fn excluded_wire(&self) -> Vec<(&'static str, &'static str)> {
        self.excluded.to_vec()
    }

    /// The declaration as it is stamped into surface output: one
    /// compact line naming both axis sets, the same form on JSON,
    /// markdown, and frontmatter surfaces. Axis names only: the
    /// per-axis exclusion reasons stay a registry fact the gate test
    /// enforces, because stamping static prose into every response
    /// would tax each call's token budget, and the reader's question
    /// the stamp answers is WHICH axes the verdict covers.
    pub fn wire_line(&self) -> String {
        self.wire_line_promoting(&[])
    }

    /// The wire line with the named excluded axes promoted into the
    /// examined set — for a report that rendered an opt-in axis this
    /// pass (`--include anchors`) and therefore did examine it. An axis
    /// not in the excluded list is ignored; the static declaration is
    /// untouched.
    pub fn wire_line_promoting(&self, promoted: &[&str]) -> String {
        let mut examined: Vec<&str> = self.examined.to_vec();
        let mut not_examined: Vec<&str> = Vec::new();
        for (a, _) in self.excluded {
            if promoted.contains(a) {
                examined.push(a);
            } else {
                not_examined.push(a);
            }
        }
        format!(
            "examined={}; not_examined={}",
            examined.join(","),
            not_examined.join(",")
        )
    }
}

impl SurfaceCoverage {
    /// The verdict declaration, when this row carries one; the
    /// stamping sites use it so a surface can only stamp what its
    /// registry row declares.
    pub fn axis_coverage(&self) -> Option<&AxisCoverage> {
        match &self.disposition {
            CoverageDisposition::Verdict(c) => Some(c),
            CoverageDisposition::NoVerdict(_) => None,
        }
    }
}

/// The health surface's coverage claim, shared by every consumer
/// that renders a health report (the CLI command, the full MCP
/// server's composer, and the lean server's own assembly): the axes
/// whose findings the report treats as defects, so an empty defect
/// statement reads as an all-clear exactly over them. Everything
/// descriptive or advisory is excluded by name.
pub const HEALTH_COVERAGE: AxisCoverage = AxisCoverage {
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
        (
            "orphans",
            "descriptive list; the defect verdict polices orphan stubs through the integrity findings",
        ),
        (
            "stubs",
            "descriptive list; the defect verdict polices orphan stubs through the integrity findings",
        ),
        (
            "most_connected",
            "descriptive ranking with no pass/fail semantics",
        ),
        (
            "missing_fields",
            "advisory count, never part of the defect verdict",
        ),
        (
            "stale",
            "advisory freshness, never part of the defect verdict",
        ),
        (
            "tags",
            "descriptive distribution with no pass/fail semantics",
        ),
        (
            "labelling",
            "advisory audit, never part of the defect verdict",
        ),
        (
            "conformance",
            "reported per entity beside the verdict, never folded into it",
        ),
        (
            "anchors",
            "drifted anchors stay advisory; the verify surfaces carry the drift statement",
        ),
        ("friction", "descriptive ledger counts"),
        ("open_questions", "descriptive listing of open questions"),
        (
            "vital_signs",
            "descriptive model-truth counts; the remodel skill holds the thresholds",
        ),
        (
            "stale_derivations",
            "advisory freshness of derived artifacts",
        ),
        (
            "checks",
            "check states are derived views; the verdicts in them belong to their recording callers",
        ),
        ("ledger", "descriptive view of the check ledger"),
        (
            "projection",
            "projection fidelity is answered by status and projection verify",
        ),
    ],
};

/// The overview surface's coverage claim, shared by every consumer
/// that renders the composed overview (the CLI command and both MCP
/// servers), and stamped into the composed frontmatter by
/// `compose_overview` itself so the declaration and the output cannot
/// diverge.
pub const OVERVIEW_COVERAGE: AxisCoverage = AxisCoverage {
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
        (
            "vital_signs",
            "descriptive model-truth counts; the remodel skill holds the thresholds",
        ),
        ("stale_derivations", OVERVIEW_SCOPE),
        ("checks", OVERVIEW_SCOPE),
        ("ledger", OVERVIEW_SCOPE),
        ("projection", OVERVIEW_SCOPE),
    ],
};

const OVERVIEW_SCOPE: &str = "overview is a descriptive composition; its only \
     all-clear claim is that the roster it renders is complete and its mounts serve";

/// Hold a registry against the axis vocabulary and a mechanically
/// discovered surface roster. Returns one finding per defect; an
/// empty result is the only clean outcome. Pure and total: callers
/// in tests pass the live vocabulary and their own live walk,
/// fixtures pass synthetic ones.
///
/// The findings, each mapped to the failure it refuses:
/// - a discovered surface with no registry row (a surface landed
///   without declaring),
/// - a registry row no walk discovers (a stale declaration reading
///   as coverage),
/// - a duplicate row (two claims, no single truth),
/// - an axis named by a declaration that the vocabulary does not
///   carry (a stale axis reading as coverage),
/// - an axis in the vocabulary that a verdict declaration neither
///   examines nor excludes (a new axis met by silence: the clean
///   verdict would cover it by omission),
/// - an axis both examined and excluded (a contradiction),
/// - an exclusion or no-verdict claim with an empty reason (a
///   declaration that declares nothing).
pub fn validate_coverage(
    vocab: &[&str],
    registry: &[SurfaceCoverage],
    discovered: &[&str],
) -> Vec<String> {
    let mut findings = Vec::new();

    for d in discovered {
        if !registry.iter().any(|r| r.surface == *d) {
            findings.push(format!(
                "surface `{d}` is discoverable and has no coverage declaration: \
                 declare its verdict axes, or declare why it emits no verdict"
            ));
        }
    }

    let mut seen: Vec<&str> = Vec::new();
    for row in registry {
        if seen.contains(&row.surface) {
            findings.push(format!(
                "surface `{}` is declared more than once",
                row.surface
            ));
            continue;
        }
        seen.push(row.surface);

        if !discovered.contains(&row.surface) {
            findings.push(format!(
                "declared surface `{}` is not discoverable: a stale declaration \
                 reads as coverage, remove it or fix the walk",
                row.surface
            ));
        }

        match row.disposition {
            CoverageDisposition::NoVerdict(reason) => {
                if reason.trim().is_empty() {
                    findings.push(format!(
                        "surface `{}` declares no verdict without a reason",
                        row.surface
                    ));
                }
            }
            CoverageDisposition::Verdict(cov) => {
                for axis in cov.examined {
                    if !vocab.contains(axis) {
                        findings.push(format!(
                            "surface `{}` examines axis `{axis}`, which the \
                             vocabulary does not carry",
                            row.surface
                        ));
                    }
                    if cov.excluded.iter().any(|(a, _)| a == axis) {
                        findings.push(format!(
                            "surface `{}` both examines and excludes axis `{axis}`",
                            row.surface
                        ));
                    }
                }
                for (axis, reason) in cov.excluded {
                    if !vocab.contains(axis) {
                        findings.push(format!(
                            "surface `{}` excludes axis `{axis}`, which the \
                             vocabulary does not carry",
                            row.surface
                        ));
                    }
                    if reason.trim().is_empty() {
                        findings.push(format!(
                            "surface `{}` excludes axis `{axis}` without a reason",
                            row.surface
                        ));
                    }
                }
                for axis in vocab {
                    let examined = cov.examined.contains(axis);
                    let excluded = cov.excluded.iter().any(|(a, _)| a == axis);
                    if !examined && !excluded {
                        findings.push(format!(
                            "surface `{}` declares nothing for axis `{axis}`: \
                             its clean verdict would cover the axis by omission, \
                             examine it or exclude it with a reason",
                            row.surface
                        ));
                    }
                }
            }
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    const VOCAB: &[&str] = &["anchors", "mounts"];

    fn full() -> SurfaceCoverage {
        SurfaceCoverage {
            surface: "verify",
            disposition: CoverageDisposition::Verdict(AxisCoverage {
                examined: &["anchors"],
                excluded: &[("mounts", "the roster surface answers for mounts")],
            }),
        }
    }

    fn ledger() -> SurfaceCoverage {
        SurfaceCoverage {
            surface: "check",
            disposition: CoverageDisposition::NoVerdict(
                "records the caller's verdict about the caller's own work",
            ),
        }
    }

    /// The complement: a registry that declares everything, over a
    /// walk that finds exactly the declared surfaces, is clean, and
    /// the only burden it carried was the declaration itself.
    #[test]
    fn complete_registry_is_clean() {
        let findings = validate_coverage(VOCAB, &[full(), ledger()], &["verify", "check"]);
        assert!(findings.is_empty(), "{findings:?}");
    }

    /// The gate red, fixture one: a surface whose clean verdict
    /// covers an axis by omission. This reproduces the sweep's
    /// condition shape independently of whether the sweep happened,
    /// since the fixture is synthetic.
    #[test]
    fn clean_over_an_unexamined_axis_fails() {
        let silent = SurfaceCoverage {
            surface: "verify",
            disposition: CoverageDisposition::Verdict(AxisCoverage {
                examined: &["anchors"],
                excluded: &[],
            }),
        };
        let findings = validate_coverage(VOCAB, &[silent, ledger()], &["verify", "check"]);
        assert!(
            findings.iter().any(|f| f.contains("`verify`")
                && f.contains("`mounts`")
                && f.contains("by omission")),
            "{findings:?}"
        );
    }

    /// The gate red, fixture two: an axis is introduced and an
    /// existing declaration is not updated. This is the recurrence
    /// case; a gate that passes here is a one-time sweep.
    #[test]
    fn axis_added_without_declaration_update_fails() {
        let grown: &[&str] = &["anchors", "mounts", "fences"];
        let findings = validate_coverage(grown, &[full(), ledger()], &["verify", "check"]);
        assert!(
            findings
                .iter()
                .any(|f| f.contains("`verify`") && f.contains("`fences`")),
            "{findings:?}"
        );
    }

    /// A surface that landed without any declaration fails.
    #[test]
    fn undeclared_surface_fails() {
        let findings = validate_coverage(VOCAB, &[full(), ledger()], &["verify", "check", "dump"]);
        assert!(
            findings
                .iter()
                .any(|f| f.contains("`dump`") && f.contains("no coverage declaration")),
            "{findings:?}"
        );
    }

    /// A declaration whose surface departed fails rather than skips.
    #[test]
    fn stale_surface_declaration_fails() {
        let findings = validate_coverage(VOCAB, &[full(), ledger()], &["check"]);
        assert!(
            findings
                .iter()
                .any(|f| f.contains("`verify`") && f.contains("not discoverable")),
            "{findings:?}"
        );
    }

    /// An axis dropped from the vocabulary turns the declarations
    /// naming it into findings, so a stale axis cannot read as
    /// coverage.
    #[test]
    fn stale_axis_in_declaration_fails() {
        let shrunk: &[&str] = &["anchors"];
        let findings = validate_coverage(shrunk, &[full(), ledger()], &["verify", "check"]);
        assert!(
            findings
                .iter()
                .any(|f| f.contains("`mounts`") && f.contains("does not carry")),
            "{findings:?}"
        );
    }

    /// Excluding an axis without a reason fails: an unexplained
    /// exclusion is a silent drop with paperwork.
    #[test]
    fn exclusion_without_reason_fails() {
        let bare = SurfaceCoverage {
            surface: "verify",
            disposition: CoverageDisposition::Verdict(AxisCoverage {
                examined: &["anchors"],
                excluded: &[("mounts", "  ")],
            }),
        };
        let findings = validate_coverage(VOCAB, &[bare, ledger()], &["verify", "check"]);
        assert!(
            findings.iter().any(|f| f.contains("without a reason")),
            "{findings:?}"
        );
    }

    /// Examining and excluding the same axis is a contradiction, not
    /// a double assurance.
    #[test]
    fn examined_and_excluded_fails() {
        let both = SurfaceCoverage {
            surface: "verify",
            disposition: CoverageDisposition::Verdict(AxisCoverage {
                examined: &["anchors", "mounts"],
                excluded: &[("mounts", "also excluded")],
            }),
        };
        let findings = validate_coverage(VOCAB, &[both, ledger()], &["verify", "check"]);
        assert!(
            findings
                .iter()
                .any(|f| f.contains("both examines and excludes")),
            "{findings:?}"
        );
    }

    /// Two rows for one surface fail: two claims, no single truth.
    #[test]
    fn duplicate_declaration_fails() {
        let findings = validate_coverage(VOCAB, &[full(), full(), ledger()], &["verify", "check"]);
        assert!(
            findings
                .iter()
                .any(|f| f.contains("declared more than once")),
            "{findings:?}"
        );
    }

    /// The live vocabulary is the health roster plus the declared
    /// extras, nothing more: composition, not a copy that can drift.
    #[test]
    fn vocabulary_composes_health_roster() {
        let axes = verdict_axes();
        for key in HEALTH_INCLUDE_KEYS {
            assert!(axes.contains(key), "health include `{key}` missing");
        }
        for key in EXTRA_VERDICT_AXES {
            assert!(axes.contains(key), "extra axis `{key}` missing");
        }
        assert_eq!(
            axes.len(),
            HEALTH_INCLUDE_KEYS.len() + EXTRA_VERDICT_AXES.len()
        );
    }
}
