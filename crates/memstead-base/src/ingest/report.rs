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
//! - **Anchor-resolution %** over the binding's in-scope anchors (per-binding
//!   scoping — see the struct docs below), with `authored`
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

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;

use crate::Engine;
use crate::anchor::{AnchorGrain, AnchorProvenanceClass, AnchorState};
use crate::binding::{Binding, CoverageSemantics, MediumCapabilities, medium_capabilities};
use crate::chunking::estimate_tokens;

use super::advance::read_advance_store;
use super::cursor::{enumerate_source_artifacts_reported, source_moved};
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
    /// `S(D)` was enumerated but is known INCOMPLETE — a scope pattern would
    /// not compile, so its share of the population never entered the walk.
    /// The surviving set is reported as a count and never as a percentage:
    /// a ratio over a denominator that is not the population is the
    /// unexamined answer this campaign exists to remove.
    Partial {
        /// How many artifacts the surviving patterns did enumerate.
        count: usize,
        /// Why the enumeration is incomplete, naming the offending patterns.
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
    /// Count per provenance-class wire string across **this binding's
    /// population** (the full transparency breakdown, including `authored`).
    /// Mem-wide until consistency-sweep 03/01 scoped the axis.
    pub by_class: BTreeMap<String, usize>,
    /// Count per grain wire string across this binding's population.
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
    /// Anchor ROWS in this binding's population, whatever their state. The
    /// figures above partition it; this is its size, so `rows` and
    /// `distinct_artifacts` are always comparable. Deriving it as
    /// `observed + authored` omitted the unobserved rows and printed fewer
    /// rows than artifacts.
    pub counted_rows: usize,
    /// Distinct artifacts among the counted anchors. One artifact legitimately
    /// carries several rows at different grains or classes, and a reader reads
    /// the figures above as being about artifacts, so the two are stated side
    /// by side rather than the rows being merged.
    pub distinct_artifacts: usize,
    /// Anchors another binding wrote, excluded from every figure above. That
    /// binding reports on them.
    pub excluded_other_binding: usize,
    /// Anchors pointing at artifacts this binding's scope does not cover,
    /// excluded from every figure above.
    pub excluded_out_of_scope: usize,
    /// The excluded anchors by artifact, named rather than merely counted: a
    /// number a reader cannot act on reproduces the original defect one level
    /// up.
    pub excluded_artifacts: Vec<String>,
    /// Counted anchors that carry no producing binding and were kept by the
    /// pre-provenance fallback. Stated so a reader can tell a population
    /// established by provenance from one resting on the fallback.
    pub counted_without_provenance: usize,
    /// Sidecar rows whose ENTITY is gone (consistency-sweep 03/02). In no
    /// binding's population and in none of the state buckets above: they used
    /// to resolve against their artifact alone and raise the numerator for an
    /// entity that does not exist.
    pub dangling: usize,
    /// Those rows named, `entity → artifact`. Reported, never repaired: the
    /// row is the only remaining trace that something wrote this mem behind
    /// the engine's back.
    pub dangling_rows: Vec<String>,
    /// Why the entity end could not be reconciled this pass, when it could
    /// not. An empty `dangling` means "none found" only when this is `None`;
    /// otherwise it means "not looked for", and the report says which.
    pub unreconciled: Option<String>,
    /// Counted `span`-grain rows whose locator was never checked against the
    /// artifact (consistency-sweep 03/03). The write path reads no source, so
    /// a span written without content in hand is unverified; stating it here
    /// stops the axis reporting such a row as adjudicated.
    pub span_unvalidated: usize,
    /// Counted rows whose hash baseline the engine inferred by backfill
    /// rather than an author pinning it. A baseline nobody chose is weaker
    /// evidence of fidelity than one somebody did, and the difference used to
    /// be invisible.
    pub hash_from_backfill: usize,
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
        strategy: ChangeStrategy,
    ) -> Self {
        FacetCapability {
            facet,
            medium_type,
            enumerable: caps.enumerable,
            change_signal: caps.change_signal,
            // Effective, not the static ceiling: a base version is retrievable
            // only when the *resolved* strategy actually holds prior content.
            // `mtime` reports that an artifact changed, not its previous bytes,
            // and `none` detects nothing — either degrades prune to
            // conflict-flagging even on a medium whose type-level capability
            // row (e.g. filesystem) advertises base retrievability.
            base_version_retrievable: caps.base_version_retrievable
                && strategy_retrieves_base(strategy),
            anchor_namespace: caps.anchor_namespace.to_string(),
            signal: signal_wire(strategy).to_string(),
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
    /// Whether the destination mem predates its binding — the adopt / onboarding
    /// case (E1). When `true`, the report leads with the expected-0%-anchored
    /// onboarding framing and the concrete backfill path, and the coverage
    /// section frames uncovered artifacts as the backfill worklist rather than
    /// as defects: no failure/error framing and no red verdict is produced
    /// **solely** by pre-binding history.
    pub adopt: bool,
    /// The binding's EFFECTIVE coverage (B4) — declared when the author
    /// wrote the field, otherwise resolved per medium
    /// ([`crate::binding::effective_coverage_semantics`]).
    pub coverage_semantics: CoverageSemantics,
    /// `true` when the binding declared the field; `false` when the
    /// effective value was resolved from the sources' media. The render
    /// marks the resolved case so a reader never mistakes a resolution
    /// for an author's assertion.
    pub coverage_semantics_declared: bool,
    /// Scope patterns still written in the retired workspace-relative dialect,
    /// each as `` `<pattern>` in facet `<facet>` ``. Reported whether or not
    /// the walk came up empty: a MIXED scope enumerates fine and silently
    /// omits whatever the old-dialect patterns would have selected, which is
    /// precisely the case a reader cannot see from the numbers.
    pub legacy_dialect_patterns: Vec<String>,
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
    /// the exhaustive findings set (B4) — the count (`= disposed_excluded_rationales.len()`).
    pub disposed_excluded: usize,
    /// The durable authored-exclusion ledger consulted under exhaustive coverage
    /// (B4): `(artifact, rationale)` for each uncovered artifact a persisted
    /// disposition marks deliberately excluded. Removed from the findings /
    /// backfill denominator and rendered with its reasoning so the editorial
    /// decision stays visible.
    pub disposed_excluded_rationales: Vec<(String, String)>,
    /// Degradation flags (B1) — typed, human/agent-readable strings.
    pub degradations: Vec<String>,
}

// ---------------------------------------------------------------------------
// Rollup verdict
// ---------------------------------------------------------------------------

/// The one-word answer a CI gate and a human reader branch on, derived from
/// an assembled [`FidelityReport`] — never measured separately, so it cannot
/// disagree with the figures under it.
///
/// Three values, because there are three honest answers and the third is the
/// one that matters: a measurement can complete without being able to support
/// a green claim. A medium with no change signal cannot observe drift; an
/// empty enumerated scope makes coverage vacuous; a pass that adjudicated no
/// anchor observed nothing. Summarizing any of those as "clean" would be the
/// report asserting more than it measured, so they resolve to
/// [`RollupVerdict::Inconclusive`] with the blindness named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RollupVerdict {
    /// The pass was substantive on every axis and recorded no findings.
    Clean,
    /// Findings were recorded over the current key.
    Drifted,
    /// The pass completed but cannot support a green claim — see
    /// [`Rollup::because`] and [`Rollup::blind_spots`].
    Inconclusive,
}

impl RollupVerdict {
    /// The stable wire string (`clean` / `drifted` / `inconclusive`).
    pub fn wire(&self) -> &'static str {
        match self {
            RollupVerdict::Clean => "clean",
            RollupVerdict::Drifted => "drifted",
            RollupVerdict::Inconclusive => "inconclusive",
        }
    }
}

/// The rollup block: the verdict, the tally behind it, why it is what it is,
/// and the concrete next actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rollup {
    /// The verdict.
    pub verdict: RollupVerdict,
    /// Total findings over the current key, summed across every class.
    pub findings_total: usize,
    /// One sentence explaining the verdict. Always populated — a verdict
    /// without a reason is a number a reader has to re-derive.
    pub because: String,
    /// Axes this measurement could not speak to, each named concretely.
    /// Empty on a substantive pass. Non-empty forces `Inconclusive` unless
    /// findings were actually recorded (an observed finding is real whatever
    /// else the pass could not see).
    pub blind_spots: Vec<String>,
    /// Top concrete actions, most severe class first. Empty when there is
    /// nothing to act on.
    pub actions: Vec<String>,
}

/// Finding classes in the order a reader should act on them: a wrong
/// projection misleads, drift is stale, an unresolvable anchor is broken
/// bookkeeping, uncovered is unwritten work, and a queued item is not yet
/// adjudicated at all.
const CLASS_SEVERITY: [&str; 5] = [
    "wrong",
    "drifted",
    "unresolvable-anchor",
    "uncovered",
    "queued-for-adjudication",
];

/// The concrete action for one finding class.
fn class_action(class: &str, n: usize, binding: &str) -> String {
    match class {
        "wrong" => format!(
            "{n} entity/entities contradict their source — read them against the source and \
             correct the entity (`memstead projection brief {binding}` lists them)"
        ),
        "drifted" => format!(
            "{n} anchored artifact(s) moved since the entity was written — re-read the source \
             and update the entity, then re-verify to advance the baseline"
        ),
        "unresolvable-anchor" => format!(
            "{n} anchor(s) no longer resolve to anything — repoint them at the artifact's new \
             location or unset them (`memstead_update` `anchors_unset`)"
        ),
        "uncovered" => format!(
            "{n} in-scope source artifact(s) carry no anchor — cover them via \
             `memstead projection brief {binding} --sync`, or record a disposition for the \
             ones deliberately excluded"
        ),
        "queued-for-adjudication" => format!(
            "{n} finding(s) are queued and not yet adjudicated — run \
             `memstead projection verify {binding} --full` to work the backlog down"
        ),
        other => format!("{n} `{other}` finding(s) recorded"),
    }
}

impl FidelityReport {
    /// Derive the [`Rollup`] from this report's own figures.
    ///
    /// Pure and total — same report, same verdict, no engine access. The
    /// derivation is deliberately conservative in one direction only: it will
    /// downgrade a green claim it cannot support, and it will never upgrade a
    /// recorded finding away.
    pub fn rollup(&self) -> Rollup {
        let findings_total: usize = self.findings_by_class.values().sum();

        let mut blind_spots: Vec<String> = Vec::new();
        match &self.coverage.denominator {
            DenominatorBasis::NonEnumerable { reason } => blind_spots.push(format!(
                "the source scope is not enumerable ({reason}) — coverage is reported over \
                 anchors only, so an uncovered artifact cannot be detected"
            )),
            DenominatorBasis::Enumerated { count: 0 } => blind_spots.push(
                "the enumerated source scope is empty (0 artifacts) — every coverage figure \
                 below is vacuous, not clean"
                    .to_string(),
            ),
            DenominatorBasis::Partial { count, reason } => blind_spots.push(format!(
                "the source enumeration is INCOMPLETE ({reason}) — {count} artifact(s) \
                 survived, but their share of the population is unknown, so no coverage \
                 percentage is reported below"
            )),
            DenominatorBasis::Enumerated { .. } => {}
        }
        if !self.legacy_dialect_patterns.is_empty() {
            blind_spots.push(format!(
                "scope pattern(s) are still written against the workspace root rather than the \
                 source pointer and select nothing under the pointer join, so whatever they \
                 were meant to cover is absent from the denominator: {}. Rewrite them relative \
                 to the source's pointer",
                self.legacy_dialect_patterns.join(", ")
            ));
        }
        if self.anchors.observed == 0 {
            blind_spots.push(
                "no anchor carried a resolution state this pass — nothing was adjudicated"
                    .to_string(),
            );
        }
        // Rows the axis could not adjudicate (consistency-sweep 03/05,
        // criterion 4). These EXTEND the existing blind-spot mechanism rather
        // than adding a parallel one, so an axis that measured only part of
        // its population reaches the inconclusive verdict the three-valued
        // rollup already provides.
        //
        // EXCLUSIONS ARE DELIBERATELY ABSENT from this list. An out-of-scope
        // or other-binding anchor is legal, excluded and named: a complete,
        // correct answer about a row this binding does not answer for. Folding
        // it in here would be the same collapse criterion 2 repairs on the
        // standalone surface, treating a known exclusion as an unknown.
        if self.anchors.unobserved > 0 {
            blind_spots.push(format!(
                "{} counted anchor(s) could not be observed at all this pass, so their state is unknown rather than clean",
                self.anchors.unobserved
            ));
        }
        if self.anchors.span_unvalidated > 0 {
            blind_spots.push(format!(
                "{} counted span anchor(s) were never checked against their artifact, so the span they name is unverified even where the hash resolves",
                self.anchors.span_unvalidated
            ));
        }
        if let Some(why) = &self.anchors.unreconciled {
            blind_spots.push(format!(
                "the entity end of these anchors was not reconciled ({why}), so a row naming an entity the mem no longer holds would not have been detected"
            ));
        }
        // A facet is change-blind if EITHER its medium cannot signal change
        // or the binding resolved that medium to no strategy. The two are
        // different: a `codebase` medium reports `change_signal: true` while
        // a binding declaring `change_detection: "none"` resolves it to
        // `ChangeStrategy::None`, which is exactly the freshness row's
        // `change_detectable`. Reading only the capability row let such a
        // binding render CLEAN while the report body two screens down said
        // "freshness unknowable" — the headline disagreeing with its own
        // evidence, which is the one thing this derivation exists to prevent.
        let change_blind: std::collections::BTreeSet<&str> = self
            .freshness
            .iter()
            .filter(|f| !f.change_detectable)
            .map(|f| f.facet.as_str())
            .collect();
        for cap in &self.capabilities {
            if !cap.change_signal {
                blind_spots.push(format!(
                    "facet `{}` ({}) provides no change signal — drift on it cannot be \
                     observed at all",
                    cap.facet, cap.medium_type
                ));
            } else if change_blind.contains(cap.facet.as_str()) {
                blind_spots.push(format!(
                    "facet `{}` ({}) declares change-detection `{}` but this pass could \
                     not read that signal — either the binding asked for none, or the \
                     checkout cannot deliver it (a `git` source with no `.git`: an \
                     archive, a container COPY, a vendored drop). Drift on it cannot \
                     be observed",
                    cap.facet, cap.medium_type, cap.signal
                ));
            }
            // Checked per facet, not only on the binding-level denominator:
            // in a MIXED binding one enumerable facet makes `S(D)` non-empty,
            // so the denominator reads `Enumerated` and the binding-level
            // blind spot above never fires — while the non-enumerable facet's
            // coverage stays unmeasurable. Every medium that is non-enumerable
            // today also lacks a change signal, so this adds no blind spot
            // under the current matrix; it is here so a future
            // non-enumerable-but-change-detectable medium cannot silently
            // render a mixed binding green.
            if !cap.enumerable {
                blind_spots.push(format!(
                    "facet `{}` ({}) is not enumerable — an uncovered artifact under it \
                     cannot be detected, only an anchored one",
                    cap.facet, cap.medium_type
                ));
            }
        }

        let mut actions: Vec<String> = Vec::new();
        for class in CLASS_SEVERITY {
            if let Some(&n) = self.findings_by_class.get(class)
                && n > 0
            {
                actions.push(class_action(class, n, &self.binding));
            }
        }
        // Any class the vocabulary grew past this list still surfaces, after
        // the ranked ones — an unknown class is never silently dropped.
        for (class, &n) in &self.findings_by_class {
            if n > 0 && !CLASS_SEVERITY.contains(&class.as_str()) {
                actions.push(class_action(class, n, &self.binding));
            }
        }

        // The adopt case (E1): a mem that predates its binding is expected to
        // be 0% anchored, so uncovered findings there are the backfill
        // worklist, not drift. A red verdict must never be produced SOLELY by
        // pre-binding history — but the pass is not clean either, so it lands
        // inconclusive with the onboarding reason.
        let only_uncovered = findings_total > 0
            && self
                .findings_by_class
                .iter()
                .all(|(class, &n)| n == 0 || class == "uncovered");

        let (verdict, because) = if self.adopt && only_uncovered {
            (
                RollupVerdict::Inconclusive,
                format!(
                    "this mem predates its binding — the {findings_total} uncovered artifact(s) \
                     are the backfill worklist, not drift"
                ),
            )
        } else if findings_total > 0 {
            let tally = self
                .findings_by_class
                .iter()
                .filter(|(_, n)| **n > 0)
                .map(|(class, n)| format!("{class}: {n}"))
                .collect::<Vec<_>>()
                .join(", ");
            (
                RollupVerdict::Drifted,
                format!("{findings_total} finding(s) recorded over the current key ({tally})"),
            )
        } else if !blind_spots.is_empty() {
            (
                RollupVerdict::Inconclusive,
                format!(
                    "no findings recorded, but the pass could not speak to {} axis/axes — \
                     this is not a clean bill of health",
                    blind_spots.len()
                ),
            )
        } else {
            (
                RollupVerdict::Clean,
                "the pass was substantive on every axis and recorded no findings".to_string(),
            )
        };

        Rollup {
            verdict,
            findings_total,
            because,
            blind_spots,
            actions,
        }
    }
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

    // --- Rollup verdict (opens the report) ---
    // A reader gets the answer before the provenance. Derived from the
    // figures below, never measured separately, so the headline cannot
    // disagree with its own body.
    let rollup = report.rollup();
    md.push_str(&format!(
        "**Verdict: {}** — {}.\n\n",
        rollup.verdict.wire().to_uppercase(),
        rollup.because
    ));
    if !rollup.actions.is_empty() {
        md.push_str("**Do next:**\n\n");
        for action in &rollup.actions {
            md.push_str(&format!("1. {action}\n"));
        }
        md.push('\n');
    }
    if !rollup.blind_spots.is_empty() {
        md.push_str("**This pass could not see:**\n\n");
        for spot in &rollup.blind_spots {
            md.push_str(&format!("- {spot}\n"));
        }
        md.push('\n');
    }

    md.push_str(&format!(
        "- **Destination mem:** `{}`\n- **Coverage semantics:** {}{}\n\n",
        report.destination_mem,
        match report.coverage_semantics {
            CoverageSemantics::Exhaustive => "exhaustive",
            CoverageSemantics::Curated => "curated",
        },
        if report.coverage_semantics_declared {
            ""
        } else {
            " (resolved from the sources' media — not declared)"
        }
    ));

    // --- Adopt / onboarding framing (E1) ---
    // When the mem predates its binding, the report LEADS with onboarding
    // framing: the expected-0%-anchored statement plus the concrete backfill
    // path. REFUSAL: this is never a failure/error framing and the report never
    // produces a red verdict solely from pre-binding history — the coverage
    // section below reframes uncovered artifacts as the backfill worklist.
    if report.adopt {
        md.push_str("## Adopting — first verify\n\n");
        md.push_str(
            "This mem predates its binding: it carries no anchors and has no prior sync \
             baseline, so **0% anchored is expected — this is onboarding, not a failure.** \
             Do not read the coverage numbers below as drift or a red verdict; the uncovered \
             artifacts are the backfill worklist, not defects.\n\n",
        );
        md.push_str(&format!(
            "**Backfill path:** run `memstead projection brief {} --sync` to work through the in-scope \
             source artifacts that carry no entity yet, covering the clearly-new concepts among \
             them through the normal mutation surface. Backfilling is incremental — a partial \
             pass is fine, and the next sync continues where you left off.\n\n",
            report.binding
        ));
    }

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
        DenominatorBasis::Partial { count, reason } => md.push_str(&format!(
            "`S(D)` is **partial**: {reason}. **{count}** source artifact(s) were \
             enumerated by the patterns that did resolve, but that set is not the \
             population, so the coverage figures below are counts and carry no \
             percentage.\n\n"
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
    // A partial enumeration reports counts and no percentage: `ratio` renders
    // `n/a` for a zero denominator, which is exactly the honest shape here —
    // the numerator is real, the population is not known.
    let den = match &report.coverage.denominator {
        DenominatorBasis::Enumerated { count } => *count,
        DenominatorBasis::NonEnumerable { .. } | DenominatorBasis::Partial { .. } => 0,
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

    // Coverage-semantics framing (B4). REFUSAL (E1): under adopt, the exhaustive
    // branch must NOT frame the uncovered artifacts as defect findings — they are
    // the expected backfill worklist of a mem that predates its binding, never a
    // red verdict caused solely by pre-binding history.
    match report.coverage_semantics {
        CoverageSemantics::Exhaustive if report.adopt => {
            let backlog = report
                .coverage
                .uncovered
                .len()
                .saturating_sub(report.disposed_excluded);
            md.push_str(&format!(
                "**Exhaustive coverage (onboarding):** {backlog} in-scope artifact(s) carry no \
                 entity yet ({} disposed excluded) — the expected first-sync backfill worklist \
                 for a mem that predates its binding, not defects.\n\n",
                report.disposed_excluded
            ));
        }
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

    // Authored exclusion ledger (B4) — surface the reasoning behind each
    // deliberately-excluded artifact so an editorial decision stays visible and
    // auditable, not just subtracted from a denominator.
    if !report.disposed_excluded_rationales.is_empty() {
        md.push_str("**Excluded on purpose (persisted dispositions):**\n");
        for (artifact, rationale) in &report.disposed_excluded_rationales {
            if rationale.is_empty() {
                md.push_str(&format!("- `{artifact}`\n"));
            } else {
                md.push_str(&format!("- `{artifact}` — {rationale}\n"));
            }
        }
        md.push('\n');
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
    // The figure and the population it was computed over render as ONE unit
    // (consistency-sweep 03/05, criteria 1 and 3). Separate bullets were the
    // defect: a budget-reduced or excerpted rendering could carry the
    // percentage and drop the caveat, and a percentage alone is read as
    // health. `scripts/check-anchor-figure-sites.py` fails on a rendering that
    // shows a resolution count without saying what it covered.
    md.push_str(&format!(
        "- resolution (non-`authored`, observed): resolves {}, drifted {}, recheck {}, \
         orphaned {}; **anchor-resolution %:** {} over {} counted row(s) on {} distinct \
         artifact(s), with {} unobserved this pass (state unavailable, never scored as \
         resolved)\n",
        report.anchors.resolves,
        report.anchors.drifted,
        report.anchors.recheck,
        report.anchors.orphaned,
        ratio(report.anchors.resolves, report.anchors.observed),
        report.anchors.counted_rows,
        report.anchors.distinct_artifacts,
        report.anchors.unobserved
    ));
    // What the denominator counted, stated rather than left to be assumed
    // (consistency-sweep 03/01, criterion 5). Rows and artifacts differ
    // whenever one artifact carries several legitimate rows at different
    // grains or classes, and a reader reads the figures above as being about
    // artifacts.
    md.push_str(&format!(
        "- the figures above count anchor ROWS: {} row(s) over {} distinct artifact(s)\n",
        report.anchors.counted_rows, report.anchors.distinct_artifacts
    ));
    // The population, and what is outside it. Named, never merely counted: a
    // number a reader cannot act on reproduces the defect one level up.
    if report.anchors.excluded_other_binding > 0 || report.anchors.excluded_out_of_scope > 0 {
        md.push_str(&format!(
            "- excluded from this binding's population: {} written by another binding, \
             {} outside this binding's declared scope (legal, reported here, never deleted)\n",
            report.anchors.excluded_other_binding, report.anchors.excluded_out_of_scope
        ));
        // Capped inside the always-ships section. Its analogue,
        // `uncovered_artifacts`, is a budget-gated heavy list; an unbounded
        // list here would inflate the hard cost past `--budget` on the very
        // multi-binding mem this plan was written for and flip the whole
        // report to overbudget, suppressing every heavy section. The counts
        // above are always complete; the names are a sample when long.
        const NAMED_CAP: usize = 10;
        for a in report.anchors.excluded_artifacts.iter().take(NAMED_CAP) {
            md.push_str(&format!("  - {a}\n"));
        }
        if report.anchors.excluded_artifacts.len() > NAMED_CAP {
            md.push_str(&format!(
                "  - …and {} more (counts above are complete)\n",
                report.anchors.excluded_artifacts.len() - NAMED_CAP
            ));
        }
    }
    // The entity end (03/02). Always stated, both ways: an empty dangling set
    // means "reconciled, none found" only when the reconciliation ran, and a
    // surface that printed nothing in the other case would report a clean
    // anchor axis over state it never examined.
    match (&report.anchors.unreconciled, report.anchors.dangling) {
        (Some(why), _) => md.push_str(&format!(
            "- the entity end of these anchors was NOT reconciled this pass ({why}), so \
             dangling sidecar rows would not have been detected\n"
        )),
        (None, 0) => {}
        (None, n) => {
            md.push_str(&format!(
                "- {n} sidecar row(s) name an entity this mem no longer holds. Excluded from \
                 every figure above, reported rather than repaired: the row is the trace of a \
                 writer that went around the engine\n"
            ));
            const NAMED_CAP: usize = 10;
            for r in report.anchors.dangling_rows.iter().take(NAMED_CAP) {
                md.push_str(&format!("  - {r}\n"));
            }
            if report.anchors.dangling_rows.len() > NAMED_CAP {
                md.push_str(&format!(
                    "  - …and {} more (the count above is complete)\n",
                    report.anchors.dangling_rows.len() - NAMED_CAP
                ));
            }
        }
    }
    // What the axis could not adjudicate, and whose baseline it is
    // (consistency-sweep 03/03). Both are always-ships aggregates: a
    // resolution figure resting on unverified spans or on baselines the
    // engine inferred means less than a reader assumes, and the difference
    // was invisible until it was counted.
    if report.anchors.span_unvalidated > 0 {
        md.push_str(&format!(
            "- {} counted span row(s) were never checked against their artifact, so their \
             span is unverified even where the hash resolves\n",
            report.anchors.span_unvalidated
        ));
    }
    if report.anchors.hash_from_backfill > 0 {
        md.push_str(&format!(
            "- {} counted row(s) carry a baseline the engine inferred by backfill rather than \
             one an author pinned\n",
            report.anchors.hash_from_backfill
        ));
    }
    if report.anchors.counted_without_provenance > 0 {
        md.push_str(&format!(
            "- {} counted anchor(s) record no producing binding and are included by the \
             pre-provenance fallback, so this population rests partly on that fallback \
             rather than wholly on provenance\n",
            report.anchors.counted_without_provenance
        ));
    }
    md.push('\n');

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
/// tally is the store's `current(key)` slice — all open findings under the
/// key's `hash(D)`, regardless of the head each was observed at — and the
/// superseded count is everything under prior binding hashes.
pub fn compute_fidelity_report(
    engine: &Engine,
    workspace_root: &Path,
    binding: &Binding,
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
        // Detectable means THIS PASS could read the signal, not that the
        // binding declared one. A `git` strategy over a tree with no `.git`
        // — a `git archive`, a Docker `COPY`, a vendored drop — declares a
        // signal the checkout cannot deliver: the head resolves empty and no
        // baseline is written. Reporting `change_detectable: true` there let
        // the rollup call such a pass "substantive on every axis" and render
        // CLEAN, which is the worst failure a gate can have. The declaration
        // is not second-guessed (that is the resolver's job); what the run
        // could observe is reported honestly.
        let signal_readable = match strategy {
            ChangeStrategy::Git => {
                super::resolve::find_git_root(&super::resolve::source_base_path(p, workspace_root))
                    .is_some()
            }
            _ => true,
        };
        let change_detectable =
            caps.change_signal && strategy != ChangeStrategy::None && signal_readable;
        any_change_detectable |= change_detectable;

        capabilities.push(FacetCapability::from_caps(
            p.name.clone(),
            medium_type,
            caps,
            strategy,
        ));

        let synced = sync_state
            .get(&format!("{binding_id}/{}#synced", p.name))
            .cloned();
        let verified = sync_state
            .get(&format!("{binding_id}/{}#verified", p.name))
            .cloned();
        freshness.push(FacetFreshness {
            facet: p.name.clone(),
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
    // Facets whose medium the matrix marks enumerable and whose OWN walk came
    // back empty. Tracked per facet, not over the union: in a mixed binding one
    // facet that walks makes `S(D)` non-empty, so a binding-level flag reads
    // "something was enumerated" while the empty facet's coverage stays
    // unmeasured — and the degradation below, which names a facet, could not
    // honestly speak for it. Same reasoning as the per-facet blind spot above.
    let mut empty_enumerable_facets: BTreeSet<String> = BTreeSet::new();
    // Patterns the enumeration could not honour, and patterns still written in
    // the retired workspace-relative dialect. The first makes `S(D)` partial;
    // the second is the real cause behind an empty walk that would otherwise
    // be blamed on the author having scoped nothing.
    let mut malformed_patterns: Vec<String> = Vec::new();
    let mut legacy_patterns: Vec<String> = Vec::new();
    // Either cause makes the denominator partial. A malformed pattern was
    // skipped; a legacy-dialect pattern selects nothing under the pointer
    // join. Both leave the surviving set short of the population, and the
    // mixed case is the dangerous one: it enumerates, so the subset looks
    // whole.
    let mut partiality_reasons: Vec<String> = Vec::new();
    for source in &resolved.sources {
        if let ResolvedSource::Primary(p) = source {
            let caps = medium_capabilities(p.medium_type);
            if caps.enumerable {
                enumerable_facets += 1;
            }
            let walked = enumerate_source_artifacts_reported(
                engine,
                p,
                &resolved.deny_paths,
                workspace_root,
            );
            if caps.enumerable && walked.files.is_empty() {
                empty_enumerable_facets.insert(p.name.clone());
            }
            for m in &walked.malformed {
                malformed_patterns.push(format!("`{}` in facet `{}`", m, p.name));
            }
            for note in &walked.legacy_dialect {
                legacy_patterns.push(format!("`{}` in facet `{}`", note.pattern, p.name));
            }
            if let Some(reason) = walked.partiality_reason() {
                partiality_reasons.push(format!("facet `{}`: {reason}", p.name));
            }
            s_d.extend(walked.files);
        }
    }
    s_d.sort();
    s_d.dedup();

    let denominator = if !partiality_reasons.is_empty() {
        // Known-incomplete beats every other basis: whatever the surviving
        // patterns enumerated, the population is not known.
        DenominatorBasis::Partial {
            count: s_d.len(),
            reason: partiality_reasons.join("; "),
        }
    } else if !s_d.is_empty() {
        DenominatorBasis::Enumerated { count: s_d.len() }
    } else if enumerable_facets == 0 {
        DenominatorBasis::NonEnumerable {
            reason: "the medium type(s) are not enumerable this cycle".to_string(),
        }
    } else if !legacy_patterns.is_empty() {
        // The walk came up empty and the scope is still in the retired
        // workspace-relative dialect: that is the cause, and saying "nothing
        // was in scope" would blame the author for patterns that DO select
        // artifacts, just not under the reading the enumerator now uses.
        DenominatorBasis::NonEnumerable {
            reason: format!(
                "scope pattern(s) still written against the workspace root rather than the \
                 source pointer, so they select nothing under the pointer join: {}. Rewrite \
                 them relative to the source's pointer",
                legacy_patterns.join(", ")
            ),
        }
    } else {
        // Enumerable per the matrix but the walk yielded nothing — an empty
        // or over-narrow scope. The degradation block below says so out loud;
        // `--full` refuses this case outright rather than measuring it.
        DenominatorBasis::NonEnumerable {
            reason: "no source artifacts enumerated in scope".to_string(),
        }
    };

    let mut direct_covered = 0usize;
    let mut tree_only_covered = 0usize;
    let mut uncovered: Vec<String> = Vec::new();
    let mut tree_fanout: BTreeMap<(String, String), usize> = BTreeMap::new();
    let entity_end_reconciled = engine.entity_set_is_reconcilable(dest.as_str()).is_ok();
    for file in &s_d {
        // Filtered by BINDING, not merely by mem (consistency-sweep 03/01,
        // criterion 7). The mem filter alone let an anchor written by one
        // binding mark a file covered for another, which is the same
        // population defect the resolution figures had, one axis over. An
        // anchor with no recorded binding still counts, by the same
        // pre-provenance fallback the population uses: a mem whose anchors
        // predate the field must not read as wholly uncovered on upgrade.
        //
        // An anchor whose ENTITY is gone covers nothing either (03/02,
        // criterion 5): the artifact would otherwise read as covered on the
        // strength of a row no entity stands behind. Only applied when the
        // entity end could be reconciled at all, so an unreconcilable mem
        // keeps its old coverage rather than reading as wholly uncovered.
        let refs = engine.anchors_referencing_artifact(file);
        let mine: Vec<&(crate::EntityId, crate::anchor::Anchor)> = refs
            .iter()
            .filter(|(eid, a)| {
                eid.mem() == dest.as_str()
                    && a.binding
                        .as_deref()
                        .map(|b| b == key.binding_hash.as_str())
                        .unwrap_or(true)
                    && (!entity_end_reconciled || !engine.entity_is_absent(eid))
            })
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

    // --- Anchor composition + resolution over THIS BINDING'S anchors ---
    // Scoped rather than mem-wide (consistency-sweep 03/01): the axis answers
    // for the population this binding is responsible for, and names the rest.
    let population = crate::ingest::anchor_population::population_for(
        engine,
        resolved,
        Some(key.binding_hash.as_str()),
    );
    let mut anchors = AnchorComposition {
        counted_rows: population.included.len(),
        distinct_artifacts: population.distinct_artifacts(),
        excluded_other_binding: population
            .excluded_count(crate::ingest::anchor_population::ExclusionReason::OtherBinding),
        excluded_out_of_scope: population
            .excluded_count(crate::ingest::anchor_population::ExclusionReason::OutOfScope),
        excluded_artifacts: population
            .excluded
            .iter()
            .map(|e| format!("{} ({})", e.artifact, e.reason.as_wire()))
            .collect(),
        counted_without_provenance: population.without_provenance,
        dangling: population.dangling.len(),
        dangling_rows: population
            .dangling
            .iter()
            .map(|d| format!("{} → {}", d.entity, d.artifact))
            .collect(),
        unreconciled: population.unreconciled.map(str::to_string),
        span_unvalidated: population
            .included
            .iter()
            .filter(|(_, r)| r.anchor.span_unvalidated)
            .count(),
        hash_from_backfill: population
            .included
            .iter()
            .filter(|(_, r)| {
                r.anchor.hash_source == Some(crate::anchor::AnchorHashSource::Backfill)
            })
            .count(),
        ..Default::default()
    };
    for (_eid, resolved_anchor) in population.included {
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

    // --- Durable authored-exclusion ledger (B4) ---
    // The advance store's `exclusions` map survives advance completion (unlike
    // its transient `dispositions`), so an artifact mined-and-deliberately-
    // excluded no longer re-surfaces as `uncovered` on every verify — and keeps
    // its reasoning. Consult it for every uncovered artifact.
    let mut disposed_excluded_rationales: Vec<(String, String)> = Vec::new();
    if let Some((mem, name)) = binding_id.split_once('/')
        && let Ok(Some(state)) = read_advance_store(workspace_root, mem, name)
    {
        let uncovered_set: std::collections::BTreeSet<&str> =
            uncovered.iter().map(String::as_str).collect();
        for (artifact, rationale) in &state.exclusions {
            if uncovered_set.contains(artifact.as_str()) {
                disposed_excluded_rationales.push((artifact.clone(), rationale.clone()));
            }
        }
    }
    let disposed_excluded = disposed_excluded_rationales.len();

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
        } else if empty_enumerable_facets.contains(&c.facet) {
            // The matrix CLAIMS this medium enumerates and the walk produced
            // nothing. That is a capability unavailable in this pass, and the
            // block above only ever spoke for media the matrix already marks
            // non-enumerable — so the honest case rendered `Degradations:
            // (none)` beside a report with no denominator. `--full` refuses
            // this outright; a plain pass measures what it can and must say
            // what it could not.
            degradations.push(format!(
                "enumeration-empty:`{}` — the medium claims enumerability but the walk yielded \
                 no artifacts; coverage is reported over anchors only",
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

    // Adopt / onboarding signal (E1) — the single canonical predicate shared with
    // the sync brief and the status rollup: a mem with no anchors and no recorded
    // `#synced` baseline predates its binding, so 0% anchored is expected.
    let adopt = super::render::mem_predates_binding(engine, resolved);
    let effective_coverage = crate::binding::effective_coverage_semantics(binding);

    FidelityReport {
        legacy_dialect_patterns: legacy_patterns,
        binding: binding_id,
        destination_mem: dest,
        adopt,
        coverage_semantics: effective_coverage.value,
        coverage_semantics_declared: effective_coverage.declared,
        capabilities,
        freshness,
        source_moved_past_synced,
        coverage,
        anchors,
        findings_by_class,
        backlog,
        superseded,
        disposed_excluded,
        disposed_excluded_rationales,
        degradations,
    }
}

/// Whether a resolved change-detection strategy can retrieve a prior base
/// version for a three-way merge (B1). Only git-backed strategies (`git`,
/// `graph`) hold prior content; `mtime` reports *that* an artifact changed but
/// not its previous bytes, and `none` detects nothing — both leave prune with
/// no base leg, so it degrades to conflict-flagging regardless of the medium
/// type's static base-retrievability ceiling. This is why filesystem+mtime —
/// a common non-git dogfood binding — must surface the conflict-flag
/// degradation even though `MediumType::Filesystem` advertises retrievability.
fn strategy_retrieves_base(strategy: ChangeStrategy) -> bool {
    matches!(strategy, ChangeStrategy::Git | ChangeStrategy::Graph)
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
            legacy_dialect_patterns: Vec::new(),
            binding: "engine/graph".to_string(),
            destination_mem: "engine".to_string(),
            adopt: false,
            coverage_semantics: CoverageSemantics::Exhaustive,
            coverage_semantics_declared: true,
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
                ..Default::default()
            },
            findings_by_class: BTreeMap::from([
                ("uncovered".to_string(), 1),
                ("queued-for-adjudication".to_string(), 1),
            ]),
            backlog: 1,
            superseded: Vec::new(),
            disposed_excluded: 0,
            disposed_excluded_rationales: Vec::new(),
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

    /// B1 — base retrievability is *effective*, keyed on the resolved
    /// change-detection strategy, not the medium type's static ceiling. A
    /// filesystem binding that resolves to `mtime` (no prior content, only a
    /// mod-time signal) has no retrievable base leg, so its facet capability
    /// reports `base_version_retrievable: false` — which is exactly what the
    /// degradation loop keys on to surface the conflict-flag posture. The same
    /// filesystem medium backed by `git` keeps the full never-clobber base leg.
    #[test]
    fn b1_base_retrievability_follows_resolved_strategy_not_medium_ceiling() {
        use crate::pipeline::MediumType;

        // The medium type's static ceiling advertises retrievability…
        assert!(medium_capabilities(MediumType::Filesystem).base_version_retrievable);

        // …but the effective capability derives from the resolved strategy.
        let fs_mtime = FacetCapability::from_caps(
            "prose".to_string(),
            "filesystem".to_string(),
            medium_capabilities(MediumType::Filesystem),
            ChangeStrategy::Mtime,
        );
        assert!(
            !fs_mtime.base_version_retrievable,
            "filesystem+mtime has no retrievable base leg — degrades to conflict-flag"
        );
        assert_eq!(fs_mtime.signal, "mtime");

        let fs_git = FacetCapability::from_caps(
            "prose".to_string(),
            "filesystem".to_string(),
            medium_capabilities(MediumType::Filesystem),
            ChangeStrategy::Git,
        );
        assert!(
            fs_git.base_version_retrievable,
            "filesystem backed by git keeps the never-clobber base leg"
        );

        // A detection-less strategy also has no base leg.
        assert!(!strategy_retrieves_base(ChangeStrategy::None));
        assert!(!strategy_retrieves_base(ChangeStrategy::Mtime));
        assert!(strategy_retrieves_base(ChangeStrategy::Git));
        assert!(strategy_retrieves_base(ChangeStrategy::Graph));

        // The linkage the fix restores: a false effective flag drives the
        // conflict-flag degradation the report renders (mirrors the derivation
        // in compute_fidelity_report's degradation loop).
        let mut r = base_report();
        r.capabilities = vec![fs_mtime.clone()];
        r.degradations = if !fs_mtime.base_version_retrievable {
            vec![format!(
                "base-version-unretrievable:`{}` — prune degrades to conflict-flagging",
                fs_mtime.facet
            )]
        } else {
            Vec::new()
        };
        let md = render_fidelity_report(&r, 8_000, &[]).markdown;
        assert!(
            md.contains("base-version-unretrievable:`prose` — prune degrades to conflict-flagging"),
            "filesystem+mtime surfaces the conflict-flag degradation in the report"
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

    /// B4 — the authored-exclusion ledger renders each excluded artifact with
    /// its reasoning, so the editorial decision stays visible (not just counted).
    #[test]
    fn b4_authored_exclusion_rationale_is_rendered() {
        let mut r = base_report();
        r.coverage_semantics = CoverageSemantics::Exhaustive;
        r.coverage.uncovered = vec!["src/gen.rs".to_string()];
        r.disposed_excluded = 1;
        r.disposed_excluded_rationales =
            vec![("src/gen.rs".to_string(), "generated; no entity".to_string())];
        let md = render_fidelity_report(&r, 8_000, &[]).markdown;
        assert!(md.contains("Excluded on purpose (persisted dispositions):"));
        assert!(md.contains("`src/gen.rs` — generated; no entity"));
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

    /// E1 (report half) — a mem that predates its binding renders the onboarding
    /// framing: the expected-0%-anchored statement plus the concrete backfill
    /// path. REFUSAL: no failure/error framing and no red "are findings" verdict
    /// is produced solely by pre-binding history — the uncovered artifacts are
    /// reframed as the backfill worklist.
    #[test]
    fn e1_adopt_report_renders_onboarding_no_red_verdict() {
        let mut r = base_report();
        r.adopt = true;
        r.coverage_semantics = CoverageSemantics::Exhaustive;
        r.coverage.uncovered = (0..5).map(|i| format!("src/file_{i}.rs")).collect();
        let md = render_fidelity_report(&r, 8_000, &[]).markdown;

        // Onboarding framing leads, with the expected-0% statement …
        assert!(md.contains("## Adopting — first verify"));
        assert!(md.contains("0% anchored is expected — this is onboarding, not a failure."));
        // … and the concrete backfill path.
        assert!(
            md.contains("**Backfill path:** run `memstead projection brief engine/graph --sync`")
        );
        // REFUSAL: the exhaustive branch never frames uncovered as red defect
        // "findings" under adopt — it is the onboarding backfill worklist.
        assert!(
            !md.contains("are **findings**"),
            "pre-binding history must not produce a red findings verdict"
        );
        assert!(md.contains("Exhaustive coverage (onboarding):"));
        assert!(md.contains("backfill worklist"));

        // Complement: without adopt, the same uncovered set IS framed as findings.
        r.adopt = false;
        let md2 = render_fidelity_report(&r, 8_000, &[]).markdown;
        assert!(!md2.contains("## Adopting — first verify"));
        assert!(md2.contains("are **findings**"));
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
        BINDING_VERSION, Binding, BuildMode, BuildOperation, DEFAULT_ADJUDICATION_CAP,
        DEFAULT_FULL_RESYNC_EVERY, Operations, VerifyOperation,
    };
    use crate::ingest::findings::verify_binding;
    use crate::ingest::resolve::resolve_binding_run;
    use crate::pipeline::{IngestTrigger, MediumType, PatternEntry, PatternMode};
    use crate::pipeline_store::{load_pipeline_configs, write_binding};
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
        let (report, outcome, md) = end_to_end_report(tmp.path(), &["direct", "tree", "auth"]);
        end_to_end_body(&report, &outcome, &md);
    }

    /// Criterion 5 (consistency-sweep 03/02): the same workspace with only the
    /// tree entity present. `src/present.rs` was directly covered by the two
    /// file anchors those two entities held, and a row no entity stands behind
    /// is not evidence that an artifact is covered.
    #[test]
    fn coverage_does_not_rest_on_an_anchor_whose_entity_is_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let (report, _outcome, md) = end_to_end_report(tmp.path(), &["tree"]);
        assert_eq!(
            report.coverage.direct_covered, 0,
            "the only direct anchor on present.rs is dangling, so nothing covers it directly"
        );
        assert!(
            report
                .coverage
                .uncovered
                .contains(&"src/present.rs".to_string()),
            "and the artifact reads uncovered rather than covered by a phantom"
        );
        // Both file-anchor rows are dangling; the tree row remains counted.
        assert_eq!(report.anchors.dangling, 2);
        assert_eq!(report.anchors.counted_rows, 1);
        assert_eq!(report.anchors.unreconciled, None);
        assert!(
            md.contains("name an entity this mem no longer holds"),
            "and the report says so on the page, not only in the struct"
        );
    }

    /// The end-to-end workspace, with the set of entities the sidecar's keys
    /// name as a parameter: dropping one is how 03/02's condition is built
    /// (a row whose entity the mem does not hold), and the criterion-5 test
    /// below needs the same three-file source and the same three anchors to
    /// compare against.
    fn end_to_end_report(
        root: &std::path::Path,
        entity_slugs: &[&str],
    ) -> (
        FidelityReport,
        crate::ingest::findings::VerifyOutcome,
        String,
    ) {
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
            source: None,
            span_unvalidated: false,
            hash_source: None,
        };
        // The entity the sidecar is keyed to. Written, because it exists:
        // a row whose entity does not is DANGLING (consistency-sweep 03/02)
        // and leaves the population before any figure counts it.
        for slug in entity_slugs {
            std::fs::write(
                mem_dir.join(format!("{slug}.md")),
                "---\ntype: decision\n---\n\n# E\n\n## Decision\n\nBody.\n",
            )
            .unwrap();
        }
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

        write_binding(
            root,
            "engine",
            "graph",
            &Binding {
                version: BINDING_VERSION,
                intent: None,
                sources: vec![crate::pipeline::Source {
                    name: "graph".to_string(),
                    medium_type: MediumType::Codebase,
                    pointer: String::new(),
                    change_detection: Some("git".to_string()),
                    scope: vec![PatternEntry {
                        path: "src/**/*.rs".to_string(),
                        mode: PatternMode::Allow,
                    }],
                    engagement: None,
                    preparation: None,
                }],
                reference_mems: Vec::new(),
                destination_mem: "engine".to_string(),
                deny_paths: Vec::new(),
                coverage_semantics: None,
                rules: None,
                prune: None,
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
        let resolved = resolve_binding_run("engine/graph", binding).unwrap();

        // Populate the durable findings store (group A) — read-only on the mem.
        let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();

        // Assemble the tier-1 report (group B) under the same key.
        let report = compute_fidelity_report(&engine, root, binding, &resolved, &outcome.key);
        let md = render_fidelity_report(&report, 8_000, &[]).markdown;
        (report, outcome, md)
    }

    fn end_to_end_body(
        report: &FidelityReport,
        outcome: &crate::ingest::findings::VerifyOutcome,
        md: &str,
    ) {
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
        // Two hash-bearing anchors present: the file anchor's recorded hash
        // mismatches the observed prepared form → deterministic drift; the
        // tree anchor has no prepared form without a code map → recheck (honest
        // deferral, never fabricated drift). Observed excludes authored.
        assert_eq!(report.anchors.observed, 2);
        assert_eq!(report.anchors.recheck, 1);
        assert_eq!(report.anchors.drifted, 1);
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
        assert!(md.contains("per-medium enumeration `S(D)` = **3**"));
        // This mem carries anchors, so it does NOT predate its binding — no
        // onboarding framing (the E1 complement).
        assert!(!report.adopt);
        assert!(!md.contains("## Adopting — first verify"));
    }

    /// E1 (report half) end-to-end — a mem with **no** anchors and no `#synced`
    /// baseline predates its binding: `compute_fidelity_report` sets `adopt` from
    /// the live engine, and the rendered report leads with onboarding framing
    /// with no red findings verdict. Read-only on the mem (`&Engine`).
    #[test]
    fn compute_report_adopt_when_mem_predates_binding() {
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
        std::fs::create_dir_all(root.join("src")).unwrap();
        // In-scope source with no anchor yet — the backfill worklist.
        std::fs::write(root.join("src").join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(root.join("src").join("b.rs"), "fn b() {}\n").unwrap();

        write_binding(
            root,
            "engine",
            "graph",
            &Binding {
                version: BINDING_VERSION,
                intent: None,
                sources: vec![crate::pipeline::Source {
                    name: "graph".to_string(),
                    medium_type: MediumType::Codebase,
                    pointer: String::new(),
                    change_detection: Some("git".to_string()),
                    scope: vec![PatternEntry {
                        path: "src/**/*.rs".to_string(),
                        mode: PatternMode::Allow,
                    }],
                    engagement: None,
                    preparation: None,
                }],
                reference_mems: Vec::new(),
                destination_mem: "engine".to_string(),
                deny_paths: Vec::new(),
                coverage_semantics: None,
                rules: None,
                prune: None,
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
        let resolved = resolve_binding_run("engine/graph", binding).unwrap();
        let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();
        let report = compute_fidelity_report(&engine, root, binding, &resolved, &outcome.key);

        // No anchors + no baseline → the mem predates its binding (E1).
        assert!(
            report.adopt,
            "a no-anchor, never-synced mem predates its binding"
        );
        let md = render_fidelity_report(&report, 8_000, &[]).markdown;
        assert!(md.contains("## Adopting — first verify"));
        assert!(md.contains("0% anchored is expected"));
        // REFUSAL: the uncovered source is NOT a red findings verdict here.
        assert!(!md.contains("are **findings**"));
        assert!(md.contains("Exhaustive coverage (onboarding):"));
    }

    /// The report renders the EFFECTIVE coverage and marks the case
    /// where it was resolved from the media rather than declared —
    /// a reader never mistakes a resolution for an author's assertion.
    #[test]
    fn report_marks_resolved_coverage_semantics() {
        let mut resolved = base_report();
        resolved.coverage_semantics = CoverageSemantics::Curated;
        resolved.coverage_semantics_declared = false;
        let md = render_hard_required(&resolved);
        assert!(
            md.contains("curated (resolved from the sources' media — not declared)"),
            "resolved value carries the marker: {md}"
        );

        let declared = base_report(); // declared: true in the fixture
        let md = render_hard_required(&declared);
        assert!(
            md.contains("**Coverage semantics:** exhaustive\n"),
            "declared value renders bare: {md}"
        );
        assert!(
            !md.contains("(resolved from the sources' media"),
            "no resolution marker on a declared value: {md}"
        );
    }
}

#[cfg(test)]
mod rollup_tests {
    use super::*;

    /// A report whose every axis is substantive and whose findings are empty
    /// — the only shape that may verdict `clean`. Each test degrades exactly
    /// one axis from here, so a failure names the axis that moved.
    fn clean_report() -> FidelityReport {
        FidelityReport {
            legacy_dialect_patterns: Vec::new(),
            binding: "engine/graph".to_string(),
            destination_mem: "engine".to_string(),
            adopt: false,
            coverage_semantics: CoverageSemantics::Exhaustive,
            coverage_semantics_declared: true,
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
                denominator: DenominatorBasis::Enumerated { count: 4 },
                direct_covered: 4,
                tree_only_covered: 0,
                uncovered: Vec::new(),
                tree_anchors: Vec::new(),
            },
            anchors: AnchorComposition {
                by_class: BTreeMap::from([("anchored".to_string(), 4)]),
                by_grain: BTreeMap::from([("file".to_string(), 4)]),
                authored: 0,
                observed: 4,
                resolves: 4,
                drifted: 0,
                recheck: 0,
                orphaned: 0,
                unobserved: 0,
                ..Default::default()
            },
            findings_by_class: BTreeMap::new(),
            backlog: 0,
            superseded: Vec::new(),
            disposed_excluded: 0,
            disposed_excluded_rationales: Vec::new(),
            degradations: Vec::new(),
        }
    }

    /// Criterion 4 (consistency-sweep 03/05): rows the axis could not
    /// adjudicate make it inconclusive, not clean. And the complement that
    /// gives the criterion its teeth: EXCLUSIONS do not, because an
    /// out-of-scope or other-binding anchor is a complete, correct answer
    /// about a row this binding does not answer for.
    #[test]
    fn unadjudicated_rows_block_clean_but_exclusions_do_not() {
        let mut r = clean_report();
        assert_eq!(
            r.rollup().verdict,
            RollupVerdict::Clean,
            "the baseline is clean"
        );

        // An exclusion is legal and named; it is not an unknown.
        r.anchors.excluded_out_of_scope = 3;
        r.anchors.excluded_other_binding = 2;
        r.anchors.excluded_artifacts = vec!["src/a.rs (out-of-scope)".into()];
        assert_eq!(
            r.rollup().verdict,
            RollupVerdict::Clean,
            "excluding a row this binding does not answer for is an ANSWER, not a blind spot"
        );

        // A row that could not be observed is an unknown.
        let mut unobserved = r.clone();
        unobserved.anchors.unobserved = 1;
        let roll = unobserved.rollup();
        assert_eq!(roll.verdict, RollupVerdict::Inconclusive);
        assert!(
            roll.blind_spots
                .iter()
                .any(|b| b.contains("could not be observed")),
            "and it names itself: {:?}",
            roll.blind_spots
        );

        // A span never checked against its artifact is an unknown.
        let mut span = r.clone();
        span.anchors.span_unvalidated = 2;
        assert_eq!(span.rollup().verdict, RollupVerdict::Inconclusive);

        // An entity end nobody reconciled is an unknown.
        let mut ent = r.clone();
        ent.anchors.unreconciled = Some("the mem's lazy entity load has not run".into());
        assert_eq!(ent.rollup().verdict, RollupVerdict::Inconclusive);
    }

    /// Criterion 7: each condition plans 01, 02 and 03 introduce is REACHABLE
    /// in the rendered report. Reachable means expressible and rendered, not
    /// failing: none of the five is a finding, which is exactly why criterion
    /// 4 has the axis report honestly over them rather than cleanly.
    #[test]
    fn all_five_conditions_are_reachable_in_the_report() {
        let mut r = clean_report();
        r.anchors.excluded_out_of_scope = 1;
        r.anchors.excluded_other_binding = 1;
        r.anchors.excluded_artifacts = vec![
            "src/a.rs (out-of-scope)".into(),
            "src/b.rs (other-binding)".into(),
        ];
        r.anchors.dangling = 1;
        r.anchors.dangling_rows = vec!["engine--gone → src/c.rs".into()];
        r.anchors.span_unvalidated = 1;
        r.anchors.hash_from_backfill = 1;

        let md = render_fidelity_report(&r, 8_000, &[]).markdown;
        for (needle, condition) in [
            ("outside this binding's declared scope", "scope-excluded"),
            ("written by another binding", "other-binding"),
            ("no longer holds", "dangling entity"),
            ("never checked against their artifact", "span not validated"),
            ("inferred by backfill", "baseline established by backfill"),
        ] {
            assert!(
                md.contains(needle),
                "{condition} is not reachable in the report; looked for {needle:?} in:\n{md}"
            );
        }
    }

    /// A substantive pass with nothing recorded is the only way to green.
    #[test]
    fn clean_requires_a_substantive_pass_and_no_findings() {
        let mut r = clean_report();
        assert_eq!(r.rollup().verdict, RollupVerdict::Clean);
        assert!(r.rollup().blind_spots.is_empty());
        assert!(r.rollup().actions.is_empty());

        r.findings_by_class.insert("drifted".to_string(), 2);
        let roll = r.rollup();
        assert_eq!(roll.verdict, RollupVerdict::Drifted);
        assert_eq!(roll.findings_total, 2);
        assert!(
            roll.actions[0].contains("moved since the entity was written"),
            "the top action is the concrete next step: {:?}",
            roll.actions
        );
    }

    /// Criterion 4's complement: a vacuous measurement is never summarized as
    /// clean. The graph medium's `0/0` case reports `enumerable: true` and
    /// enumerates nothing, which is exactly how a "0 findings" run could look
    /// green while having observed no source at all.
    #[test]
    fn a_vacuous_zero_over_zero_is_inconclusive_not_clean() {
        let mut r = clean_report();
        r.coverage.denominator = DenominatorBasis::Enumerated { count: 0 };
        let roll = r.rollup();
        assert_eq!(
            roll.verdict,
            RollupVerdict::Inconclusive,
            "0/0 is not a clean bill of health"
        );
        assert!(
            roll.blind_spots.iter().any(|s| s.contains("vacuous")),
            "the blindness is named, not implied: {:?}",
            roll.blind_spots
        );
    }

    /// A facet that cannot be enumerated blocks green on its own, even when
    /// a sibling facet makes the binding-level denominator `Enumerated`. The
    /// mixed-binding case is exactly where a per-binding check would miss it.
    #[test]
    fn a_non_enumerable_facet_blocks_green_even_in_a_mixed_binding() {
        let mut r = clean_report();
        r.capabilities.push(FacetCapability {
            facet: "site".to_string(),
            medium_type: "web".to_string(),
            enumerable: false,
            // Deliberately TRUE: isolates the enumerability axis from the
            // change-signal one, so this test fails if only the latter is
            // checked.
            change_signal: true,
            base_version_retrievable: false,
            anchor_namespace: "url".to_string(),
            signal: "none".to_string(),
        });
        // The enumerable sibling keeps the denominator populated.
        assert!(matches!(
            r.coverage.denominator,
            DenominatorBasis::Enumerated { count } if count > 0
        ));
        let roll = r.rollup();
        assert_eq!(
            roll.verdict,
            RollupVerdict::Inconclusive,
            "one enumerable facet must not launder a non-enumerable one: {roll:?}"
        );
        assert!(
            roll.blind_spots
                .iter()
                .any(|s| s.contains("not enumerable")),
            "{:?}",
            roll.blind_spots
        );
    }

    /// A binding that declares `change_detection: "none"` over a medium that
    /// COULD signal change is change-blind all the same. The capability row
    /// still reads `change_signal: true` — only the resolved signal and the
    /// freshness row know — so a rollup reading capabilities alone renders
    /// this green while its own body prints "freshness unknowable".
    #[test]
    fn a_resolved_signal_of_none_blocks_green_even_when_the_medium_could_signal() {
        let mut r = clean_report();
        // Exactly the shape `change_detection: "none"` over a codebase
        // produces: the MEDIUM can signal, the BINDING declined to.
        r.capabilities[0].change_signal = true;
        r.capabilities[0].signal = "none".to_string();
        r.freshness[0].change_detectable = false;
        r.freshness[0].signal = "none".to_string();
        let roll = r.rollup();
        assert_eq!(
            roll.verdict,
            RollupVerdict::Inconclusive,
            "a change-blind binding is not a clean bill of health: {roll:?}"
        );
        assert!(
            roll.blind_spots
                .iter()
                .any(|s| s.contains("could not read that signal")),
            "the blind spot names the unreadable signal: {:?}",
            roll.blind_spots
        );
    }

    /// A medium with no change signal cannot observe drift, so it cannot
    /// support a green verdict on that axis — the capability row decides,
    /// not the finding count.
    #[test]
    fn a_facet_without_a_change_signal_blocks_green() {
        let mut r = clean_report();
        r.capabilities[0].change_signal = false;
        let roll = r.rollup();
        assert_eq!(roll.verdict, RollupVerdict::Inconclusive);
        assert!(
            roll.blind_spots
                .iter()
                .any(|s| s.contains("no change signal")),
            "{:?}",
            roll.blind_spots
        );
    }

    /// A non-enumerable scope means an uncovered artifact is undetectable —
    /// silence there is absence of evidence, not evidence of absence.
    #[test]
    fn a_non_enumerable_scope_blocks_green() {
        let mut r = clean_report();
        r.coverage.denominator = DenominatorBasis::NonEnumerable {
            reason: "web medium".to_string(),
        };
        assert_eq!(r.rollup().verdict, RollupVerdict::Inconclusive);
    }

    /// A pass that adjudicated nothing observed nothing.
    #[test]
    fn zero_observed_anchors_blocks_green() {
        let mut r = clean_report();
        r.anchors.observed = 0;
        r.anchors.resolves = 0;
        assert_eq!(r.rollup().verdict, RollupVerdict::Inconclusive);
    }

    /// E1: a mem that predates its binding is expected to be 0% anchored, so
    /// uncovered findings there are the backfill worklist. No red verdict may
    /// be produced SOLELY by pre-binding history — but it is not clean either.
    #[test]
    fn adopt_with_only_uncovered_is_never_red() {
        let mut r = clean_report();
        r.adopt = true;
        r.findings_by_class.insert("uncovered".to_string(), 12);
        let roll = r.rollup();
        assert_eq!(
            roll.verdict,
            RollupVerdict::Inconclusive,
            "onboarding is neither drift nor a clean bill: {roll:?}"
        );
        assert!(
            roll.because.contains("backfill worklist"),
            "the reason states the onboarding framing: {}",
            roll.because
        );

        // Real drift on an adopting mem is still drift — the E1 framing
        // covers pre-binding history, not everything that follows it.
        r.findings_by_class.insert("drifted".to_string(), 1);
        assert_eq!(r.rollup().verdict, RollupVerdict::Drifted);
    }

    /// An observed finding outranks a blind spot: the pass could not see
    /// everything, but what it did see is real.
    #[test]
    fn findings_outrank_blind_spots() {
        let mut r = clean_report();
        r.capabilities[0].change_signal = false;
        r.findings_by_class.insert("wrong".to_string(), 1);
        let roll = r.rollup();
        assert_eq!(roll.verdict, RollupVerdict::Drifted);
        assert!(
            !roll.blind_spots.is_empty(),
            "the blindness is still reported alongside the verdict"
        );
    }

    /// Actions are ordered by what a reader should fix first, and a class the
    /// vocabulary grows past the ranked list is never silently dropped.
    #[test]
    fn actions_are_severity_ordered_and_never_drop_a_class() {
        let mut r = clean_report();
        r.findings_by_class.insert("uncovered".to_string(), 3);
        r.findings_by_class.insert("wrong".to_string(), 1);
        r.findings_by_class
            .insert("some-future-class".to_string(), 2);
        let roll = r.rollup();
        assert!(
            roll.actions[0].contains("contradict their source"),
            "{roll:?}"
        );
        assert_eq!(roll.actions.len(), 3, "{roll:?}");
        assert!(
            roll.actions.iter().any(|a| a.contains("some-future-class")),
            "an unranked class still surfaces: {roll:?}"
        );
    }

    /// The wire vocabulary is closed and stable — consumers branch on it.
    #[test]
    fn verdict_wire_strings_are_stable() {
        assert_eq!(RollupVerdict::Clean.wire(), "clean");
        assert_eq!(RollupVerdict::Drifted.wire(), "drifted");
        assert_eq!(RollupVerdict::Inconclusive.wire(), "inconclusive");
        let json = serde_json::to_string(&RollupVerdict::Inconclusive).unwrap();
        assert_eq!(json, "\"inconclusive\"");
    }
}
