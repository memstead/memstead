//! The **tier-1 fidelity report** (bundle plan `05-verify-sync-engine`, group
//! B) — deterministic, engine-rendered, token-budgeted.
//!
//! Verify (group A) records durable findings; this module *renders* a
//! measurement over them plus the live anchor / capability / freshness state.
//! It performs **no LLM call** and **no destination-mem mutation** — it reads
//! the engine, the findings store, the advance store, and the capability
//! matrix, and formats a report. Any repair instruction is the sync brief's job
//! (group C), never this report's.
//!
//! ## What the report states honestly (B1–B5)
//!
//! - **Grain-classed coverage** with tree-anchor fan-out kept on its **own
//!   axis** — a 1-entity/200-file tree anchor shows as one anchor fanning out
//!   over 200 files, never laundered into a blended coverage percentage (B1).
//! - **Anchor-resolution %** over the mem's observed anchors, with `authored`
//!   provenance **excluded** from the coverage/accuracy denominators and shown
//!   as its own bucket (B1).
//! - **Freshness** vs. both `sync_state` tokens (`#synced` / `#verified`). A
//!   detection-less medium (the capability matrix marks it non-change-
//!   detectable) renders `signal: none` → *"freshness unknowable"*; a green
//!   freshness verdict is **structurally unreachable** for such a medium (B2).
//! - **Token-budgeted** in the house envelope shape shared with
//!   [`crate::overview`]: aggregates are hard-required and always ship; heavy
//!   per-artifact lists greedy-fill by priority and, when they do not fit,
//!   drop to `## Hints` with an `estimated_tokens` figure — never rendered
//!   unbounded (B3).
//! - **Coverage semantics** branch: under `curated`, the unaccounted share is
//!   information; under `exhaustive`, unaccounted artifacts (not anchored, not
//!   declared-excluded, no persisted disposition) are findings (B4).
//! - **Denominator provenance** is stated: coverage is relative to the
//!   per-medium enumeration `S(D)` (B5).

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::Engine;
use crate::anchor::{AnchorGrain, AnchorProvenanceClass, AnchorState};
use crate::binding::{BindingV1, CoverageSemantics, MediumCapabilities, medium_capabilities};
use crate::chunking::estimate_tokens;

use super::advance::read_advance_store;
use super::cursor::{enumerate_facet_files, source_moved};
use super::findings::{FindingClass, FindingKey, read_findings_store};
use super::resolve::{ChangeStrategy, ResolvedIngest, ResolvedSource, resolve_change_strategy};

/// Default token budget for the report's heavy content. Mirrors
/// [`crate::overview::DEFAULT_OVERVIEW_BUDGET`] — one house envelope, one
/// default.
pub const DEFAULT_REPORT_BUDGET: usize = 8_000;

/// Heavy-content include keys the renderer recognises, in **greedy-fill
/// priority order**. A key listed in `include` forces its section in past the
/// budget (mirroring the overview envelope); an unlisted key greedy-fills until
/// the budget is exhausted, then surfaces as a hint. An unknown key is ignored
/// with a warning line.
pub const ALLOWED_REPORT_INCLUDE_KEYS: &[&str] =
    &["uncovered_artifacts", "tree_fanout", "superseded_findings"];

// ---------------------------------------------------------------------------
// Structured report — the deterministic, pre-computed data the pure renderer
// formats. Assembling it (`compute_fidelity_report`) reads the engine; the
// renderer (`render_fidelity_report`) is a pure function over this data, so
// every B1–B5 assertion tests against a hand-built value with no IO.
// ---------------------------------------------------------------------------

/// The denominator basis for coverage (B5): coverage is reported relative to
/// the per-medium enumeration `S(D)`, or — when the medium cannot be
/// enumerated — the report says so rather than inventing a denominator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DenominatorBasis {
    /// `S(D)` was enumerated: `count` source artifacts in scope (after
    /// `deny_paths`), the coverage denominator.
    Enumerated {
        /// `|S(D)|` — the enumerated source-artifact count.
        count: usize,
    },
    /// The medium is non-enumerable (or its type is not enumerated this cycle):
    /// no `S(D)`, so coverage is reported over anchors only and the denominator
    /// is stated unavailable.
    NonEnumerable {
        /// Why no `S(D)` could be computed.
        reason: String,
    },
}

/// One tree-grain anchor's fan-out over `S(D)` (B1). A tree anchor is one row
/// here whatever its fan-out — the per-file count is an observation, never a
/// per-file coverage credit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TreeFanout {
    /// The entity id carrying the tree anchor.
    pub entity: String,
    /// The tree artifact reference.
    pub artifact: String,
    /// How many `S(D)` files fall under this tree.
    pub fanout: usize,
}

/// Grain-classed coverage over `S(D)` (B1). Tree-anchor fan-out is a **separate
/// axis** — `direct_covered` and `tree_only_covered` are never summed into one
/// blended percentage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrainCoverage {
    /// The denominator basis (B5).
    pub denominator: DenominatorBasis,
    /// `S(D)` files directly covered by a non-tree (file / span) anchor.
    pub direct_covered: usize,
    /// `S(D)` files covered **only** via a tree-grain anchor (the fan-out axis,
    /// kept distinct from `direct_covered`).
    pub tree_only_covered: usize,
    /// `S(D)` files with no anchor at all (the heavy artifact list).
    pub uncovered: Vec<String>,
    /// Per tree anchor, its fan-out over `S(D)` (the heavy detail list).
    pub tree_anchors: Vec<TreeFanout>,
}

/// Anchor composition + resolution tally over the destination mem's anchors
/// (B1). `authored` provenance is pulled into its own bucket and **excluded**
/// from the resolution (coverage/accuracy) tally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct AnchorComposition {
    /// Count per provenance-class wire string across **all** the mem's anchors
    /// (the full transparency breakdown, including `authored`).
    pub by_class: BTreeMap<String, usize>,
    /// Count per grain wire string across all the mem's anchors.
    pub by_grain: BTreeMap<String, usize>,
    /// `authored`-class anchors — the own bucket, excluded from the resolution
    /// denominator below.
    pub authored: usize,
    /// Non-`authored` anchors that carry a resolution state this pass.
    pub observed: usize,
    /// Non-`authored` anchors that resolved clean.
    pub resolves: usize,
    /// Non-`authored` anchors that drifted (stable-medium hash break).
    pub drifted: usize,
    /// Non-`authored` anchors deferred for re-examination (unstable / no hash).
    pub recheck: usize,
    /// Non-`authored` anchors whose artifact is gone.
    pub orphaned: usize,
    /// Non-`authored` anchors that could **not** be observed this pass (state
    /// `None`) — reported honestly, never counted as resolved.
    pub unobserved: usize,
}

/// One facet's capability-matrix row + resolved change signal (B1 capability
/// block; B2 change-detectability; B5 enumeration provenance).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FacetCapability {
    /// The source facet.
    pub facet: String,
    /// The medium type wire string.
    pub medium_type: String,
    /// Whether the medium's scope is enumerable (`S(D)` computable).
    pub enumerable: bool,
    /// Whether the medium provides a change signal.
    pub change_signal: bool,
    /// Whether a base version is retrievable (three-way-merge feasibility).
    pub base_version_retrievable: bool,
    /// The anchor namespace (`path` / `path+commit` / `entity` / `url`).
    pub anchor_namespace: String,
    /// The resolved change-detection signal (`git` / `mtime` / `graph` /
    /// `none`).
    pub signal: String,
}

impl FacetCapability {
    fn from_caps(
        facet: String,
        medium_type: String,
        caps: MediumCapabilities,
        signal: String,
    ) -> Self {
        FacetCapability {
            facet,
            medium_type,
            enumerable: caps.enumerable,
            change_signal: caps.change_signal,
            base_version_retrievable: caps.base_version_retrievable,
            anchor_namespace: caps.anchor_namespace.to_string(),
            signal,
        }
    }
}

/// One facet's freshness state vs. both `sync_state` tokens (B1/B2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FacetFreshness {
    /// The source facet.
    pub facet: String,
    /// The resolved change signal (`git` / `mtime` / `graph` / `none`).
    pub signal: String,
    /// The `#synced` baseline token, or `None` when never synced.
    pub synced: Option<String>,
    /// The `#verified` baseline token, or `None` when never verified.
    pub verified: Option<String>,
    /// Whether the medium is change-detectable at all: the capability matrix
    /// marks a change signal **and** a strategy resolved (signal ≠ `none`).
    /// When `false`, freshness is **unknowable** and the renderer is
    /// structurally incapable of printing a green verdict for this facet (B2).
    pub change_detectable: bool,
}

/// The tier-1 fidelity report — fully computed, deterministic data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FidelityReport {
    /// The canonical binding id `<mem>/<stem>`.
    pub binding: String,
    /// The destination mem.
    pub destination_mem: String,
    /// Whether the binding claims exhaustive or curated coverage (B4).
    pub coverage_semantics: CoverageSemantics,
    /// Per-facet capability rows (B1 capability block).
    pub capabilities: Vec<FacetCapability>,
    /// Per-facet freshness (B1/B2).
    pub freshness: Vec<FacetFreshness>,
    /// Binding-level: has any change-detectable source moved past its `#synced`
    /// baseline this pass? `None` when no source is change-detectable (nothing
    /// to compare) — never a fabricated `false`.
    pub source_moved_past_synced: Option<bool>,
    /// Grain-classed coverage over `S(D)` (B1/B5).
    pub coverage: GrainCoverage,
    /// Anchor composition + resolution (B1).
    pub anchors: AnchorComposition,
    /// Findings tally by class over the current key.
    pub findings_by_class: BTreeMap<String, usize>,
    /// Tier-3 backlog depth — findings queued for adjudication (B1).
    pub backlog: usize,
    /// Findings recorded under a **prior** `(hash(D), source_head)` key,
    /// segregated as superseded (the heavy detail list is the count's backing).
    pub superseded: Vec<String>,
    /// Persisted dispositions that exclude an otherwise-uncovered artifact from
    /// the exhaustive findings set (B4) — count only.
    pub disposed_excluded: usize,
    /// Degradation flags (B1) — typed, human/agent-readable strings.
    pub degradations: Vec<String>,
}

// ---------------------------------------------------------------------------
// Rendered output
// ---------------------------------------------------------------------------

/// The rendered report: markdown plus the structured envelope bits (mode,
/// hints) mirroring [`crate::overview::OverviewOutput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedFidelityReport {
    /// The rendered markdown.
    pub markdown: String,
    /// `"complete"` / `"reduced"` / `"overbudget"` — the same tri-state the
    /// overview envelope uses.
    pub mode: String,
    /// Drill-in hints for heavy sections omitted under the budget:
    /// `(key, estimated_tokens)`.
    pub hints: Vec<(String, usize)>,
    /// The budget actually consumed by hard-required + emitted heavy content.
    pub budget_used: usize,
}

// ---------------------------------------------------------------------------
// Pure renderer
// ---------------------------------------------------------------------------

/// Render `N/D (P%)`, or `N/D (n/a)` when the denominator is zero.
fn ratio(num: usize, den: usize) -> String {
    if den == 0 {
        format!("{num}/{den} (n/a)")
    } else {
        let pct = (num as f64) * 100.0 / (den as f64);
        format!("{num}/{den} ({pct:.1}%)")
    }
}

/// Render the hard-required (always-ships) aggregate markdown for a report.
/// This is the content B3's "aggregated counts always ship" rests on — it is
/// concatenated whatever the budget.
fn render_hard_required(report: &FidelityReport) -> String {
    let mut md = String::new();
    md.push_str(&format!("# Fidelity report — `{}`\n\n", report.binding));
    md.push_str(&format!(
        "- **Destination mem:** `{}`\n- **Coverage semantics:** {}\n\n",
        report.destination_mem,
        match report.coverage_semantics {
            CoverageSemantics::Exhaustive => "exhaustive",
            CoverageSemantics::Curated => "curated",
        }
    ));

    // --- Denominator provenance (B5) ---
    md.push_str("## Denominator provenance\n\n");
    match &report.coverage.denominator {
        DenominatorBasis::Enumerated { count } => md.push_str(&format!(
            "Coverage is reported relative to the per-medium enumeration `S(D)` = **{count}** \
             source artifact(s) in scope (after `deny_paths`).\n\n"
        )),
        DenominatorBasis::NonEnumerable { reason } => md.push_str(&format!(
            "No `S(D)` denominator: {reason}. Coverage is reported over anchors only; the \
             per-medium enumeration is unavailable.\n\n"
        )),
    }

    // --- Capability matrix (B1) ---
    md.push_str("## Capability matrix\n\n");
    if report.capabilities.is_empty() {
        md.push_str("_(no primary sources resolved)_\n\n");
    } else {
        for c in &report.capabilities {
            md.push_str(&format!("### `{}` ({})\n\n", c.facet, c.medium_type));
            md.push_str(&format!(
                "- enumerable: {} | change_signal: {} | base_version_retrievable: {}\n",
                c.enumerable, c.change_signal, c.base_version_retrievable
            ));
            md.push_str(&format!(
                "- anchor_namespace: `{}` | resolved signal: `{}`\n\n",
                c.anchor_namespace, c.signal
            ));
        }
    }

    // --- Freshness (B1/B2) ---
    md.push_str("## Freshness\n\n");
    if report.freshness.is_empty() {
        md.push_str("_(no source facets)_\n\n");
    } else {
        for f in &report.freshness {
            md.push_str(&format!("### `{}`\n\n", f.facet));
            md.push_str(&format!("- signal: `{}`\n", f.signal));
            if !f.change_detectable {
                // B2 REFUSAL: a non-change-detectable medium NEVER prints a
                // green freshness verdict — only "unknowable". This branch is
                // the only place `signal: none` freshness is rendered.
                md.push_str(
                    "- **freshness unknowable** — this medium is not change-detectable \
                     (no change signal); `#synced` / `#verified` cannot be adjudicated as fresh\n",
                );
            } else {
                match &f.synced {
                    Some(t) => md.push_str(&format!("- `#synced`: `{t}`\n")),
                    None => md.push_str("- `#synced`: never synced\n"),
                }
                match &f.verified {
                    Some(t) => md.push_str(&format!("- `#verified`: `{t}`\n")),
                    None => md.push_str("- `#verified`: never verified\n"),
                }
            }
            md.push('\n');
        }
        // Binding-level move verdict — only when something is change-detectable.
        match report.source_moved_past_synced {
            Some(true) => md.push_str(
                "**Source moved past its `#synced` baseline** — the graph is stale for the \
                 moved facet(s); a sync pass is due.\n\n",
            ),
            Some(false) => {
                md.push_str("Every change-detectable source is at its `#synced` baseline.\n\n")
            }
            None => {}
        }
    }

    // --- Coverage (B1, B4) ---
    md.push_str("## Coverage (grain-classed)\n\n");
    let den = match &report.coverage.denominator {
        DenominatorBasis::Enumerated { count } => *count,
        DenominatorBasis::NonEnumerable { .. } => 0,
    };
    md.push_str(&format!(
        "- direct-covered (file / span anchors): {}\n",
        ratio(report.coverage.direct_covered, den)
    ));
    // Tree fan-out is a DISTINCT axis — reported separately, never blended into
    // the direct-covered percentage (B1).
    let tree_files: usize = report.coverage.tree_anchors.iter().map(|t| t.fanout).sum();
    md.push_str(&format!(
        "- tree-anchor fan-out (separate axis): {} tree anchor(s) fanning out over {} file(s); \
         {} file(s) covered ONLY via a tree anchor\n",
        report.coverage.tree_anchors.len(),
        tree_files,
        report.coverage.tree_only_covered
    ));
    md.push_str(&format!(
        "- uncovered (no anchor): {}\n\n",
        report.coverage.uncovered.len()
    ));

    // Coverage-semantics framing (B4).
    match report.coverage_semantics {
        CoverageSemantics::Exhaustive => {
            let findings = report
                .coverage
                .uncovered
                .len()
                .saturating_sub(report.disposed_excluded);
            md.push_str(&format!(
                "**Exhaustive coverage:** {findings} unaccounted artifact(s) — not anchored, not \
                 declared-excluded, no persisted disposition ({} disposed excluded) — are \
                 **findings**.\n\n",
                report.disposed_excluded
            ));
        }
        CoverageSemantics::Curated => {
            md.push_str(&format!(
                "**Curated coverage:** {} unaccounted artifact(s) are **information**, not \
                 defects — a curated binding covers a deliberate slice.\n\n",
                report.coverage.uncovered.len()
            ));
        }
    }

    // --- Anchors (B1) ---
    md.push_str("## Anchors\n\n");
    md.push_str(&format!(
        "- by class: {}\n",
        render_counts(&report.anchors.by_class)
    ));
    md.push_str(&format!(
        "- by grain: {}\n",
        render_counts(&report.anchors.by_grain)
    ));
    md.push_str(&format!(
        "- `authored` bucket (excluded from coverage/accuracy denominators): {}\n",
        report.anchors.authored
    ));
    md.push_str(&format!(
        "- resolution (non-`authored`, observed): resolves {}, drifted {}, recheck {}, orphaned {}\n",
        report.anchors.resolves,
        report.anchors.drifted,
        report.anchors.recheck,
        report.anchors.orphaned
    ));
    md.push_str(&format!(
        "- **anchor-resolution %:** {}\n",
        ratio(report.anchors.resolves, report.anchors.observed)
    ));
    md.push_str(&format!(
        "- unobserved this pass (state unavailable, never scored as resolved): {}\n\n",
        report.anchors.unobserved
    ));

    // --- Findings + backlog (B1) ---
    md.push_str("## Findings\n\n");
    md.push_str(&format!(
        "- by class: {}\n",
        render_counts(&report.findings_by_class)
    ));
    md.push_str(&format!(
        "- **tier-3 adjudication backlog:** {}\n",
        report.backlog
    ));
    md.push_str(&format!(
        "- superseded (prior `(hash(D), source_head)` key, segregated): {}\n\n",
        report.superseded.len()
    ));

    // --- Degradations (B1) ---
    md.push_str("## Degradations\n\n");
    if report.degradations.is_empty() {
        md.push_str("_(none)_\n\n");
    } else {
        for d in &report.degradations {
            md.push_str(&format!("- {d}\n"));
        }
        md.push('\n');
    }

    md
}

/// Render a `BTreeMap<String, usize>` as `k=v, k=v` (or `(none)`).
fn render_counts(counts: &BTreeMap<String, usize>) -> String {
    if counts.is_empty() {
        return "(none)".to_string();
    }
    counts
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The three heavy sections, in greedy-fill priority order — each a
/// `(key, markdown)` pair whose markdown is empty when the section has nothing
/// to show (an empty section is emitted free, never hinted).
fn heavy_sections(report: &FidelityReport) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();

    // uncovered_artifacts
    let mut s = String::new();
    if !report.coverage.uncovered.is_empty() {
        s.push_str("## Uncovered artifacts\n\n");
        for a in &report.coverage.uncovered {
            s.push_str(&format!("- `{a}`\n"));
        }
        s.push('\n');
    }
    out.push(("uncovered_artifacts", s));

    // tree_fanout
    let mut s = String::new();
    if !report.coverage.tree_anchors.is_empty() {
        s.push_str("## Tree-anchor fan-out (detail)\n\n");
        for t in &report.coverage.tree_anchors {
            s.push_str(&format!(
                "- `{}` → `{}` fans out over {} file(s)\n",
                t.entity, t.artifact, t.fanout
            ));
        }
        s.push('\n');
    }
    out.push(("tree_fanout", s));

    // superseded_findings
    let mut s = String::new();
    if !report.superseded.is_empty() {
        s.push_str("## Superseded findings (detail)\n\n");
        for f in &report.superseded {
            s.push_str(&format!("- {f}\n"));
        }
        s.push('\n');
    }
    out.push(("superseded_findings", s));

    out
}

/// Render the tier-1 fidelity report into markdown, token-budgeted in the house
/// envelope shape (B3). Aggregated counts (the hard-required block) always ship;
/// heavy per-artifact lists greedy-fill by priority and drop to `## Hints` when
/// they do not fit — `include`-listed keys force their section in past the
/// budget, exactly as the overview envelope does.
///
/// - `budget` — the target token budget for **heavy** content (the aggregates
///   ship in addition, so total output exceeds this when the report is large).
/// - `include` — keys forced in regardless of budget; an unknown key adds a
///   warning line, mirroring the overview composer.
pub fn render_fidelity_report(
    report: &FidelityReport,
    budget: usize,
    include: &[String],
) -> RenderedFidelityReport {
    let hard = render_hard_required(report);
    let hard_cost = estimate_tokens(&hard);
    let overbudget = hard_cost > budget;

    let include_set: std::collections::BTreeSet<&str> = include
        .iter()
        .map(String::as_str)
        .filter(|k| ALLOWED_REPORT_INCLUDE_KEYS.contains(k))
        .collect();
    let unknown_includes: Vec<&String> = include
        .iter()
        .filter(|k| !ALLOWED_REPORT_INCLUDE_KEYS.contains(&k.as_str()))
        .collect();

    let sections = heavy_sections(report);
    let mut emitted: Vec<String> = Vec::new();
    let mut hints: Vec<(String, usize)> = Vec::new();
    let mut used = hard_cost;
    let mut remaining = budget.saturating_sub(hard_cost);

    for (key, section_md) in &sections {
        if section_md.is_empty() {
            continue; // nothing to show — never hinted, never charged
        }
        let cost = estimate_tokens(section_md);
        let forced = include_set.contains(key);
        if forced {
            emitted.push(section_md.clone());
            used += cost;
            remaining = remaining.saturating_sub(cost);
        } else if !overbudget && remaining >= cost {
            emitted.push(section_md.clone());
            used += cost;
            remaining -= cost;
        } else {
            hints.push(((*key).to_string(), cost));
        }
    }

    let mode = if overbudget {
        "overbudget"
    } else if hints.is_empty() {
        "complete"
    } else {
        "reduced"
    };

    let mut md = String::new();
    md.push_str("---\n");
    md.push_str(&format!("_report_mode: {mode}\n"));
    md.push_str(&format!("_budget_requested: {budget}\n"));
    md.push_str(&format!("_budget_used: {used}\n"));
    md.push_str("---\n\n");
    md.push_str(&hard);
    for section in &emitted {
        md.push_str(section);
    }

    if !hints.is_empty() {
        md.push_str("## Hints\n\n");
        md.push_str(
            "_(heavy sections omitted under the token budget — re-query with the key)_\n\n",
        );
        for (key, tokens) in &hints {
            md.push_str(&format!("- `{key}` — estimated_tokens: {tokens}\n"));
        }
        md.push('\n');
    }

    if !unknown_includes.is_empty() {
        md.push_str("## Warnings\n\n");
        for k in &unknown_includes {
            md.push_str(&format!(
                "- unknown include key `{k}` — allowed: {}\n",
                ALLOWED_REPORT_INCLUDE_KEYS.join(", ")
            ));
        }
        md.push('\n');
    }

    RenderedFidelityReport {
        markdown: md,
        mode: mode.to_string(),
        hints,
        budget_used: used,
    }
}

// ---------------------------------------------------------------------------
// Assembly — reads the engine, findings store, advance store, capability matrix
// ---------------------------------------------------------------------------

/// Assemble the tier-1 [`FidelityReport`] for a binding (B1–B5). Read-only on
/// the destination mem — it borrows `&Engine` (shared), reads the durable
/// findings store under `key`, the advance store, and the live anchor /
/// enumeration / freshness state. It performs no mutation and no LLM call.
///
/// `key` is the current `(hash(D), source_head)` the verify pass recorded
/// under (from [`super::findings::VerifyOutcome::key`]); the report's findings
/// tally is the store's `current(key)` slice, and the superseded count is
/// everything under prior keys.
pub fn compute_fidelity_report(
    engine: &Engine,
    workspace_root: &Path,
    binding: &BindingV1,
    resolved: &ResolvedIngest,
    key: &FindingKey,
) -> FidelityReport {
    let binding_id = resolved.name.clone();
    let dest = resolved.destination_mem.clone();

    // --- Capabilities + freshness, per primary facet ---
    let sync_state = engine
        .mem_config_for(&dest)
        .map(|c| c.sync_state.clone())
        .unwrap_or_default();
    let mut capabilities: Vec<FacetCapability> = Vec::new();
    let mut freshness: Vec<FacetFreshness> = Vec::new();
    let mut any_change_detectable = false;
    for source in &resolved.sources {
        let ResolvedSource::Primary(p) = source else {
            continue;
        };
        let caps = medium_capabilities(p.medium_type);
        let medium_type = serde_json::to_value(p.medium_type)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let strategy = resolve_change_strategy(p, workspace_root);
        let signal = signal_wire(strategy).to_string();
        let change_detectable = caps.change_signal && strategy != ChangeStrategy::None;
        any_change_detectable |= change_detectable;

        capabilities.push(FacetCapability::from_caps(
            p.facet_ref.clone(),
            medium_type,
            caps,
            signal.clone(),
        ));

        let synced = sync_state
            .get(&format!("{binding_id}/{}#synced", p.facet_ref))
            .cloned();
        let verified = sync_state
            .get(&format!("{binding_id}/{}#verified", p.facet_ref))
            .cloned();
        freshness.push(FacetFreshness {
            facet: p.facet_ref.clone(),
            signal,
            synced,
            verified,
            change_detectable,
        });
    }

    let source_moved_past_synced = if any_change_detectable {
        Some(source_moved(engine, resolved, workspace_root))
    } else {
        None
    };

    // --- S(D) enumeration + grain-classed coverage ---
    let mut s_d: Vec<String> = Vec::new();
    let mut enumerable_facets = 0usize;
    for source in &resolved.sources {
        if let ResolvedSource::Primary(p) = source {
            let caps = medium_capabilities(p.medium_type);
            if caps.enumerable {
                enumerable_facets += 1;
            }
            s_d.extend(enumerate_facet_files(
                p,
                &resolved.deny_paths,
                workspace_root,
            ));
        }
    }
    s_d.sort();
    s_d.dedup();

    let denominator = if !s_d.is_empty() {
        DenominatorBasis::Enumerated { count: s_d.len() }
    } else if enumerable_facets == 0 {
        DenominatorBasis::NonEnumerable {
            reason: "the medium type(s) are not enumerable this cycle".to_string(),
        }
    } else {
        // Enumerable per the matrix but the walk yielded nothing (empty scope /
        // non-path medium type not walked this cycle).
        DenominatorBasis::NonEnumerable {
            reason: "no source artifacts enumerated in scope".to_string(),
        }
    };

    let mut direct_covered = 0usize;
    let mut tree_only_covered = 0usize;
    let mut uncovered: Vec<String> = Vec::new();
    let mut tree_fanout: BTreeMap<(String, String), usize> = BTreeMap::new();
    for file in &s_d {
        let refs = engine.anchors_referencing_artifact(file);
        let mine: Vec<&(crate::EntityId, crate::anchor::Anchor)> = refs
            .iter()
            .filter(|(eid, _)| eid.mem() == dest.as_str())
            .collect();
        if mine.is_empty() {
            uncovered.push(file.clone());
            continue;
        }
        let has_non_tree = mine.iter().any(|(_, a)| a.grain != AnchorGrain::Tree);
        if has_non_tree {
            direct_covered += 1;
        } else {
            tree_only_covered += 1;
        }
        // Attribute tree fan-out (separate axis) for every covering tree anchor.
        for (eid, a) in &mine {
            if a.grain == AnchorGrain::Tree {
                *tree_fanout
                    .entry((eid.as_ref().to_string(), a.artifact.clone()))
                    .or_insert(0) += 1;
            }
        }
    }
    let tree_anchors: Vec<TreeFanout> = tree_fanout
        .into_iter()
        .map(|((entity, artifact), fanout)| TreeFanout {
            entity,
            artifact,
            fanout,
        })
        .collect();

    let coverage = GrainCoverage {
        denominator,
        direct_covered,
        tree_only_covered,
        uncovered: uncovered.clone(),
        tree_anchors,
    };

    // --- Anchor composition + resolution over the mem's anchors ---
    let mut anchors = AnchorComposition::default();
    for (_eid, resolved_anchor) in engine.mem_anchors_resolved(&dest) {
        let a = &resolved_anchor.anchor;
        *anchors
            .by_class
            .entry(a.class.as_wire().to_string())
            .or_insert(0) += 1;
        *anchors
            .by_grain
            .entry(a.grain.as_wire().to_string())
            .or_insert(0) += 1;
        if a.class == AnchorProvenanceClass::Authored {
            anchors.authored += 1;
            continue; // own bucket — excluded from the resolution denominator
        }
        match resolved_anchor.state {
            Some(AnchorState::Resolves) => {
                anchors.resolves += 1;
                anchors.observed += 1;
            }
            Some(AnchorState::Drifted) => {
                anchors.drifted += 1;
                anchors.observed += 1;
            }
            Some(AnchorState::Recheck) => {
                anchors.recheck += 1;
                anchors.observed += 1;
            }
            Some(AnchorState::Orphaned) => {
                anchors.orphaned += 1;
                anchors.observed += 1;
            }
            None => anchors.unobserved += 1,
        }
    }

    // --- Findings tally + backlog + superseded, from the durable store ---
    let mut findings_by_class: BTreeMap<String, usize> = BTreeMap::new();
    let mut backlog = 0usize;
    let mut superseded: Vec<String> = Vec::new();
    if let Some((mem, name)) = binding_id.split_once('/')
        && let Ok(Some(store)) = read_findings_store(workspace_root, mem, name)
    {
        for f in store.current(key) {
            *findings_by_class
                .entry(f.class.as_wire().to_string())
                .or_insert(0) += 1;
            if f.class == FindingClass::QueuedForAdjudication {
                backlog += 1;
            }
        }
        for f in store.superseded(key) {
            superseded.push(format!(
                "[{}] {} ({})",
                f.class.as_wire(),
                finding_target_label(&f.target),
                f.facet
            ));
        }
    }

    // --- Persisted dispositions that exclude uncovered artifacts (B4) ---
    let mut disposed_excluded = 0usize;
    if let Some((mem, name)) = binding_id.split_once('/')
        && let Ok(Some(state)) = read_advance_store(workspace_root, mem, name)
    {
        let uncovered_set: std::collections::BTreeSet<&str> =
            uncovered.iter().map(String::as_str).collect();
        disposed_excluded = state
            .dispositions
            .keys()
            .filter(|a| uncovered_set.contains(a.as_str()))
            .count();
    }

    // --- Degradation flags (B1) ---
    let mut degradations: Vec<String> = Vec::new();
    for c in &capabilities {
        if !c.change_signal || c.signal == "none" {
            degradations.push(format!(
                "change-signal-none:`{}` — freshness is unknowable for this facet",
                c.facet
            ));
        }
        if !c.enumerable {
            degradations.push(format!(
                "enumeration-unavailable:`{}` — `S(D)` coverage denominator not computable",
                c.facet
            ));
        }
        if !c.base_version_retrievable {
            degradations.push(format!(
                "base-version-unretrievable:`{}` — prune degrades to conflict-flagging",
                c.facet
            ));
        }
    }
    if anchors.recheck > 0 {
        degradations.push(format!(
            "hash-adjudication-deferred — {} anchor(s) recheck (unstable medium / hash \
             unavailable), not asserted drift",
            anchors.recheck
        ));
    }
    if anchors.unobserved > 0 {
        degradations.push(format!(
            "anchors-unobserved — {} anchor(s) could not be observed this pass",
            anchors.unobserved
        ));
    }

    FidelityReport {
        binding: binding_id,
        destination_mem: dest,
        coverage_semantics: binding.coverage_semantics,
        capabilities,
        freshness,
        source_moved_past_synced,
        coverage,
        anchors,
        findings_by_class,
        backlog,
        superseded,
        disposed_excluded,
        degradations,
    }
}

/// The `signal` wire string for a [`ChangeStrategy`] — `none` for detection-less
/// (never a fabricated token, B2).
fn signal_wire(strategy: ChangeStrategy) -> &'static str {
    match strategy {
        ChangeStrategy::None => "none",
        ChangeStrategy::Git => "git",
        ChangeStrategy::Mtime => "mtime",
        ChangeStrategy::Graph => "graph",
    }
}

/// A compact label for a finding target (superseded detail).
fn finding_target_label(target: &super::findings::FindingTarget) -> String {
    match target {
        super::findings::FindingTarget::Anchor { entity, artifact } => {
            format!("{entity} → {artifact}")
        }
        super::findings::FindingTarget::Artifact { artifact } => artifact.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- pure-renderer fixtures ------------------------------------------

    fn base_report() -> FidelityReport {
        FidelityReport {
            binding: "engine/graph".to_string(),
            destination_mem: "engine".to_string(),
            coverage_semantics: CoverageSemantics::Exhaustive,
            capabilities: vec![FacetCapability {
                facet: "src".to_string(),
                medium_type: "codebase".to_string(),
                enumerable: true,
                change_signal: true,
                base_version_retrievable: true,
                anchor_namespace: "path".to_string(),
                signal: "git".to_string(),
            }],
            freshness: vec![FacetFreshness {
                facet: "src".to_string(),
                signal: "git".to_string(),
                synced: Some("deadbeef".to_string()),
                verified: None,
                change_detectable: true,
            }],
            source_moved_past_synced: Some(false),
            coverage: GrainCoverage {
                denominator: DenominatorBasis::Enumerated { count: 10 },
                direct_covered: 6,
                tree_only_covered: 3,
                uncovered: vec!["src/a.rs".to_string()],
                tree_anchors: vec![TreeFanout {
                    entity: "engine--big".to_string(),
                    artifact: "src/".to_string(),
                    fanout: 3,
                }],
            },
            anchors: AnchorComposition {
                by_class: BTreeMap::from([
                    ("anchored".to_string(), 5),
                    ("authored".to_string(), 2),
                ]),
                by_grain: BTreeMap::from([("file".to_string(), 4), ("tree".to_string(), 1)]),
                authored: 2,
                observed: 5,
                resolves: 4,
                drifted: 0,
                recheck: 1,
                orphaned: 0,
                unobserved: 0,
            },
            findings_by_class: BTreeMap::from([
                ("uncovered".to_string(), 1),
                ("queued-for-adjudication".to_string(), 1),
            ]),
            backlog: 1,
            superseded: Vec::new(),
            disposed_excluded: 0,
            degradations: vec!["hash-adjudication-deferred — 1 anchor(s) recheck".to_string()],
        }
    }

    /// B1 — the report renders every required element deterministically, with
    /// tree fan-out on its own axis, `authored` as its own excluded bucket, and
    /// the backlog depth. Two renders of the same input are byte-identical (no
    /// LLM, no clock).
    #[test]
    fn b1_renders_all_elements_deterministically() {
        let r = base_report();
        let a = render_fidelity_report(&r, 8_000, &[]);
        let b = render_fidelity_report(&r, 8_000, &[]);
        assert_eq!(a.markdown, b.markdown, "deterministic — identical bytes");

        let md = &a.markdown;
        // Grain-classed coverage with tree fan-out SEPARATE, never blended.
        assert!(md.contains("direct-covered (file / span anchors): 6/10"));
        assert!(md.contains(
            "tree-anchor fan-out (separate axis): 1 tree anchor(s) fanning out over 3 file(s)"
        ));
        // The direct % is NOT (6+3)/10 — the tree fan-out is not folded in.
        assert!(
            !md.contains("9/10"),
            "tree fan-out must not blend into direct coverage"
        );
        // anchor-resolution % over non-authored observed.
        assert!(md.contains("anchor-resolution %:** 4/5"));
        // authored is its own excluded bucket.
        assert!(md.contains("`authored` bucket (excluded from coverage/accuracy denominators): 2"));
        // tier-3 backlog depth from the store tally.
        assert!(md.contains("tier-3 adjudication backlog:** 1"));
        // capability-matrix block + degradation flags.
        assert!(md.contains("## Capability matrix"));
        assert!(md.contains("## Degradations"));
        assert!(md.contains("hash-adjudication-deferred"));
        // B5 denominator provenance.
        assert!(md.contains("per-medium enumeration `S(D)` = **10**"));
    }

    /// B2 — a detection-less medium renders `signal: none` → "freshness
    /// unknowable", and NO green freshness verdict appears for it.
    #[test]
    fn b2_detectionless_medium_freshness_unknowable_never_green() {
        let mut r = base_report();
        r.capabilities = vec![FacetCapability {
            facet: "manual".to_string(),
            medium_type: "web".to_string(),
            enumerable: false,
            change_signal: false,
            base_version_retrievable: false,
            anchor_namespace: "url".to_string(),
            signal: "none".to_string(),
        }];
        r.freshness = vec![FacetFreshness {
            facet: "manual".to_string(),
            signal: "none".to_string(),
            // Even if a stale token were somehow present, it must never be
            // rendered as a fresh/green verdict.
            synced: Some("should-never-render-green".to_string()),
            verified: Some("nor-this".to_string()),
            change_detectable: false,
        }];
        r.source_moved_past_synced = None;
        let out = render_fidelity_report(&r, 8_000, &[]);
        let md = &out.markdown;
        assert!(md.contains("signal: `none`"));
        assert!(md.contains("freshness unknowable"));
        // REFUSAL: no fabricated green token, no fresh verdict, no baseline
        // token laundered as fresh.
        assert!(!md.contains("should-never-render-green"));
        assert!(
            !md.contains("`#synced`: `"),
            "no synced token rendered for a non-detectable medium"
        );
        assert!(
            !md.contains("at its `#synced` baseline"),
            "no green 'at baseline' verdict"
        );
    }

    /// B3 — aggregates always ship at budget 0 (mode overbudget, every heavy
    /// list dropped to hints).
    #[test]
    fn b3_aggregates_always_ship_at_zero_budget() {
        let r = base_report();
        let out = render_fidelity_report(&r, 0, &[]);
        assert_eq!(out.mode, "overbudget");
        let md = &out.markdown;
        // Aggregated counts still ship.
        assert!(md.contains("direct-covered (file / span anchors): 6/10"));
        assert!(md.contains("tier-3 adjudication backlog:** 1"));
        assert!(md.contains("## Capability matrix"));
        // The per-artifact list did NOT render inline; it is a hint.
        assert!(!md.contains("## Uncovered artifacts"));
        assert!(md.contains("## Hints"));
        assert!(out.hints.iter().any(|(k, _)| k == "uncovered_artifacts"));
    }

    /// B3 — a large facet's per-artifact list never renders unbounded under a
    /// small budget: it is dropped to a hint with an estimated_tokens figure.
    /// The complement: `include` forces it in past the budget.
    #[test]
    fn b3_large_facet_list_truncates_then_include_forces() {
        let mut r = base_report();
        // A large uncovered facet — 500 artifacts.
        r.coverage.uncovered = (0..500).map(|i| format!("src/file_{i}.rs")).collect();
        // A budget large enough for the aggregates but not the huge list.
        let hard_cost = estimate_tokens(&render_hard_required(&r));
        let out = render_fidelity_report(&r, hard_cost + 5, &[]);
        assert_eq!(out.mode, "reduced");
        assert!(
            !out.markdown.contains("src/file_499.rs"),
            "big list not rendered unbounded"
        );
        assert!(out.markdown.contains("## Hints"));
        let (_, est) = out
            .hints
            .iter()
            .find(|(k, _)| k == "uncovered_artifacts")
            .expect("uncovered list hinted");
        assert!(*est > 5, "the hint carries a real estimated_tokens figure");

        // Complement: include forces the section in past the budget.
        let forced =
            render_fidelity_report(&r, hard_cost + 5, &["uncovered_artifacts".to_string()]);
        assert!(
            forced.markdown.contains("src/file_499.rs"),
            "include forces the full list"
        );
    }

    /// B4 — exhaustive vs curated framing differs: exhaustive calls unaccounted
    /// artifacts findings; curated calls them information.
    #[test]
    fn b4_curated_vs_exhaustive_framing() {
        let mut exhaustive = base_report();
        exhaustive.coverage_semantics = CoverageSemantics::Exhaustive;
        let ex_md = render_fidelity_report(&exhaustive, 8_000, &[]).markdown;
        assert!(ex_md.contains("Exhaustive coverage:"));
        assert!(ex_md.contains("are **findings**"));

        let mut curated = base_report();
        curated.coverage_semantics = CoverageSemantics::Curated;
        let cur_md = render_fidelity_report(&curated, 8_000, &[]).markdown;
        assert!(cur_md.contains("Curated coverage:"));
        assert!(cur_md.contains("**information**"));
        assert!(
            !cur_md.contains("are **findings**"),
            "curated never frames unaccounted as findings"
        );
    }

    /// B4 — a persisted disposition removes an uncovered artifact from the
    /// exhaustive findings count.
    #[test]
    fn b4_disposition_excludes_from_exhaustive_findings() {
        let mut r = base_report();
        r.coverage_semantics = CoverageSemantics::Exhaustive;
        r.coverage.uncovered = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];
        r.disposed_excluded = 1;
        let md = render_fidelity_report(&r, 8_000, &[]).markdown;
        // 2 uncovered − 1 disposed = 1 finding.
        assert!(md.contains("1 unaccounted artifact(s)"));
        assert!(md.contains("(1 disposed excluded)"));
    }

    /// B5 — the denominator provenance is stated; a non-enumerable medium says
    /// so rather than inventing a denominator.
    #[test]
    fn b5_denominator_provenance_stated() {
        let r = base_report();
        let md = render_fidelity_report(&r, 8_000, &[]).markdown;
        assert!(md.contains("## Denominator provenance"));
        assert!(md.contains("per-medium enumeration `S(D)` = **10**"));

        let mut non = base_report();
        non.coverage.denominator = DenominatorBasis::NonEnumerable {
            reason: "the medium type(s) are not enumerable this cycle".to_string(),
        };
        let md2 = render_fidelity_report(&non, 8_000, &[]).markdown;
        assert!(md2.contains("No `S(D)` denominator"));
        assert!(md2.contains("not enumerable this cycle"));
    }

    /// An unknown include key is surfaced as a warning, not silently dropped.
    #[test]
    fn unknown_include_key_warns() {
        let r = base_report();
        let out = render_fidelity_report(&r, 8_000, &["bogus".to_string()]);
        assert!(out.markdown.contains("unknown include key `bogus`"));
    }

    // ---- assembly (impure) end-to-end ------------------------------------

    use crate::anchor::{Anchor, AnchorHashStability, AnchorProvenanceClass, AnchorSidecar};
    use crate::binding::{
        BINDING_VERSION, BindingV1, BuildMode, BuildOperation, DEFAULT_ADJUDICATION_CAP,
        DEFAULT_FULL_RESYNC_EVERY, Operations, VerifyOperation,
    };
    use crate::ingest::findings::verify_binding;
    use crate::ingest::resolve::resolve_binding_run;
    use crate::pipeline::{Facet, IngestTrigger, Medium, MediumType, PatternEntry, PatternMode};
    use crate::pipeline_store::{load_pipeline_configs, write_binding, write_facet, write_medium};
    use crate::workspace::{
        Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
    };
    use crate::workspace_store::WorkspaceStoreAdapter;

    /// The assembly reads the engine, findings store, and enumeration end to
    /// end: coverage is classed over `S(D)` with a direct-covered file, a
    /// tree-only file, and an uncovered file; the tree fan-out is on its own
    /// axis; the `authored` anchor is its own excluded bucket; the tier-3
    /// backlog reads from the store the verify pass populated. Read-only on the
    /// mem throughout (`&Engine`).
    #[test]
    fn compute_report_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mem_dir = root.join("mem");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();

        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        let mount = Mount {
            mem: "engine".to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: MountStorage::Folder {
                path: mem_dir.clone(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        };
        crate::FileWorkspaceStore::new()
            .save_state(
                root,
                &Workspace {
                    mounts: vec![mount],
                    settings: WorkspaceSettings::default(),
                },
            )
            .unwrap();

        let out = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success());
        std::fs::create_dir_all(root.join("src").join("sub")).unwrap();
        std::fs::write(root.join("src").join("present.rs"), "fn a() {}\n").unwrap();
        std::fs::write(root.join("src").join("uncovered.rs"), "fn b() {}\n").unwrap();
        std::fs::write(root.join("src").join("sub").join("deep.rs"), "fn c() {}\n").unwrap();

        let mk = |artifact: &str, grain: AnchorGrain, class: AnchorProvenanceClass| Anchor {
            artifact: artifact.to_string(),
            grain,
            class,
            at_version: None,
            hash: class.is_hash_bearing().then(|| "recorded".to_string()),
            hash_stability: AnchorHashStability::Stable,
            derived_from: Vec::new(),
            binding: None,
        };
        let mut sidecar = AnchorSidecar::default();
        sidecar.set(
            "engine--direct",
            vec![mk(
                "src/present.rs",
                AnchorGrain::File,
                AnchorProvenanceClass::Anchored,
            )],
        );
        sidecar.set(
            "engine--tree",
            vec![mk(
                "src/sub/",
                AnchorGrain::Tree,
                AnchorProvenanceClass::Anchored,
            )],
        );
        // An authored anchor — its own excluded bucket, never scored.
        sidecar.set(
            "engine--auth",
            vec![mk(
                "src/present.rs",
                AnchorGrain::File,
                AnchorProvenanceClass::Authored,
            )],
        );
        std::fs::write(
            mem_dir.join(crate::anchor::ANCHOR_SIDECAR_PATH),
            sidecar.to_bytes(),
        )
        .unwrap();

        write_medium(
            root,
            "engine",
            "graph",
            &Medium {
                name: "graph".to_string(),
                medium_type: MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
            },
        )
        .unwrap();
        write_facet(
            root,
            "engine",
            "graph",
            &Facet {
                name: "graph".to_string(),
                medium: "graph".to_string(),
                scope: vec![PatternEntry {
                    path: "src/**/*.rs".to_string(),
                    mode: PatternMode::Allow,
                }],
                engagement: None,
                preparation: None,
            },
        )
        .unwrap();
        write_binding(
            root,
            "engine",
            "graph",
            &BindingV1 {
                version: BINDING_VERSION,
                intent: None,
                source_facets: vec!["graph".to_string()],
                reference_mems: Vec::new(),
                destination_mem: "engine".to_string(),
                deny_paths: Vec::new(),
                coverage_semantics: CoverageSemantics::Exhaustive,
                rules: None,
                operations: Operations {
                    build: Some(BuildOperation {
                        mode: BuildMode::Discovery,
                        trigger: IngestTrigger::Loop,
                        batch_size: 20,
                        post_actions: None,
                    }),
                    sync: None,
                    verify: Some(VerifyOperation {
                        trigger: IngestTrigger::Manual,
                        batch_size: 20,
                        adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                        full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
                    }),
                },
            },
        )
        .unwrap();

        let engine = Engine::from_workspace_root(root).unwrap();
        let configs = load_pipeline_configs(root).unwrap();
        let binding = &configs.bindings[0].config;
        let resolved = resolve_binding_run(&configs, "engine/graph", binding).unwrap();

        // Populate the durable findings store (group A) — read-only on the mem.
        let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();

        // Assemble the tier-1 report (group B) under the same key.
        let report = compute_fidelity_report(&engine, root, binding, &resolved, &outcome.key);

        // S(D) = the three .rs files under src/.
        assert_eq!(
            report.coverage.denominator,
            DenominatorBasis::Enumerated { count: 3 }
        );
        // present.rs is directly covered; sub/deep.rs is tree-only; uncovered.rs
        // is uncovered.
        assert_eq!(report.coverage.direct_covered, 1);
        assert_eq!(report.coverage.tree_only_covered, 1);
        assert_eq!(
            report.coverage.uncovered,
            vec!["src/uncovered.rs".to_string()]
        );
        // The tree anchor's fan-out is on its own axis — one anchor over one file.
        assert_eq!(report.coverage.tree_anchors.len(), 1);
        assert_eq!(report.coverage.tree_anchors[0].fanout, 1);
        assert_eq!(report.coverage.tree_anchors[0].artifact, "src/sub/");
        // `authored` is its own excluded bucket, never in the resolution tally.
        assert_eq!(report.anchors.authored, 1);
        assert_eq!(report.anchors.by_class.get("authored"), Some(&1));
        // Two hash-bearing anchors present (file + tree) → both recheck (no
        // prepared hash this pass), never drift; observed excludes authored.
        assert_eq!(report.anchors.observed, 2);
        assert_eq!(report.anchors.recheck, 2);
        assert_eq!(report.anchors.drifted, 0);
        // Backlog reads from the store the verify pass populated.
        assert_eq!(report.backlog, outcome.backlog);
        // A degradation flag for the deferred hash adjudication.
        assert!(
            report
                .degradations
                .iter()
                .any(|d| d.contains("hash-adjudication-deferred"))
        );
        // The rendered report is deterministic and carries the S(D) statement.
        let md = render_fidelity_report(&report, 8_000, &[]).markdown;
        assert!(md.contains("per-medium enumeration `S(D)` = **3**"));
    }
}
