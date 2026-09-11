//! The **tier-1 fidelity report**: deterministic, engine-rendered,
//! token-budgeted.
//!
//! Verify records durable findings; this module *renders* a
//! measurement over them plus the live anchor / capability / freshness state.
//! It performs **no LLM call** and **no destination-mem mutation** — it reads
//! the engine, the findings store, the advance store, and the capability
//! matrix, and formats a report. Any repair instruction is the sync brief's job
//!, never this report's.
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
//!   [`memstead_base::overview`]: aggregates are hard-required and always ship; heavy
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

use memstead_base::Engine;
use memstead_base::anchor::{AnchorGrain, AnchorProvenanceClass, AnchorState};
use memstead_base::binding::{Binding, CoverageSemantics, MediumCapabilities, medium_capabilities};
use memstead_base::chunking::estimate_tokens;

use super::advance::read_advance_store;
use super::cursor::{enumerate_source_artifacts_reported, source_moved};
use super::findings::{FindingClass, FindingKey, read_findings_store};
use memstead_base::binding_run::{
    ChangeStrategy, ResolvedIngest, ResolvedSource, resolve_change_strategy,
};

/// Default token budget for the report's heavy content. Mirrors
/// [`memstead_base::overview::DEFAULT_OVERVIEW_BUDGET`] — one house envelope, one
/// default.
pub const DEFAULT_REPORT_BUDGET: usize = 8_000;

/// Heavy-content include keys the renderer recognises, in **greedy-fill
/// priority order**. A key listed in `include` forces its section in past the
/// budget (mirroring the overview envelope); an unlisted key greedy-fills until
/// the budget is exhausted, then surfaces as a hint. An unknown key is ignored
/// with a warning line.
pub const ALLOWED_REPORT_INCLUDE_KEYS: &[&str] = &[
    "uncovered_artifacts",
    "unanchored_mentions",
    "tree_fanout",
    "superseded_findings",
];

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
    /// The medium is non-enumerable (its type has no enumeration):
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

/// The coverage unit, stated on every fidelity report (A3 AC4).
pub const COVERAGE_UNIT: &str = "describing entities per artifact — an artifact counts once \
     when at least one entity anchors it, however many anchor rows it carries; anchor rows \
     are counted on the resolution axis, never here";

/// Grain-classed coverage over `S(D)` (B1). Tree-anchor fan-out is a **separate
/// axis** — `direct_covered` and `tree_only_covered` are never summed into one
/// blended percentage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrainCoverage {
    /// The denominator basis (B5).
    pub denominator: DenominatorBasis,
    /// `S(D)` artifacts covered by at least one describing entity, direct or
    /// tree — the coverage unit (2026-09-02, basket line 7): an artifact
    /// counts once however many anchor rows or entities describe it, so
    /// three anchors from one entity on one file are one covered artifact.
    pub covered_artifacts: usize,
    /// Distinct destination entities that describe at least one `S(D)`
    /// artifact through this binding's anchors.
    pub describing_entities: usize,
    /// The unit statement, verbatim on the report, so the figure is never
    /// read as an anchor-row count.
    pub unit: &'static str,
    /// `S(D)` files directly covered by a non-tree (file / span) anchor.
    pub direct_covered: usize,
    /// `S(D)` files covered **only** via a tree-grain anchor (the fan-out axis,
    /// kept distinct from `direct_covered`).
    pub tree_only_covered: usize,
    /// `S(D)` files with no anchor at all (the heavy artifact list).
    pub uncovered: Vec<String>,
    /// In-scope artifacts carrying no anchor that a `projection exclude`
    /// declared out of intent, and which are therefore NOT in `uncovered`
    /// (C7). Dropped rather than marked: a reader counts the array and a gate
    /// reads it, so an entry left in it stays owed however it is annotated.
    /// The rationales ride `disposed_excluded_rationales` on the report.
    pub excluded: usize,
    /// Destination entities naming an `S(D)` artifact they carry no anchor on
    /// (the `unanchored-mention` findings of the current batch): claims no
    /// verify watches. Each entry names the entity, the artifact and the
    /// section (the heavy list); the count rides beside `uncovered`.
    pub unanchored_mentions: Vec<UnanchoredMention>,
    /// Per tree anchor, its fan-out over `S(D)` (the heavy detail list).
    pub tree_anchors: Vec<TreeFanout>,
    /// Destination entities (non-stub) that hold no anchor at all — the
    /// entity side of coverage: nothing in any source stands behind them, so
    /// no verify can speak to them. A reading, never a verdict: under curated
    /// semantics an entity can legitimately be authored from no single
    /// artifact. Net of the declared entity exclusions, sorted.
    pub unanchored_entities: Vec<String>,
    /// Unanchored destination entities a `projection exclude
    /// --entity-exclusions` declared out of intent, dropped from
    /// `unanchored_entities` and named with their reasons on the report
    /// (`excluded_entity_rationales`).
    pub excluded_entities: usize,
}

/// One claim no verify watches: a destination entity names an in-scope
/// artifact in its prose and carries no anchor on it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct UnanchoredMention {
    /// The entity whose body names the artifact.
    pub entity: String,
    /// The `S(D)` artifact named.
    pub artifact: String,
    /// The section the mention sits in.
    pub section: String,
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
    /// Non-`authored` anchors that resolved clean, as the figure type that
    /// carries its population (`resolves`, `population`,
    /// `fully_adjudicated` at this level in JSON): the count is never
    /// reachable apart from what it was computed over.
    #[serde(flatten)]
    pub figure: memstead_base::anchor::AnchorResolutionFigure,
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
    /// Counted rows whose state rests on a RECORDED observation rather than
    /// a live one (url rows, which the engine never observes itself), each
    /// with how many whole days that observation is old. Their state counts
    /// above as observed; this says how current that observation is.
    pub aging: Vec<AgingAnchor>,
}

/// One counted row resting on a recorded observation, by age.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgingAnchor {
    pub entity: String,
    pub artifact: String,
    pub observed_at: String,
    pub unobserved_for_days: u64,
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
            // and `none` detects nothing — so a medium whose type-level
            // capability row (e.g. filesystem) advertises base retrievability
            // reports `false` here under those strategies.
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
    /// case. When `true`, the report leads with the expected-0%-anchored
    /// onboarding framing and the concrete backfill path, and the coverage
    /// section frames uncovered artifacts as the backfill worklist rather than
    /// as defects: no failure/error framing and no red verdict is produced
    /// **solely** by pre-binding history.
    pub adopt: bool,
    /// The binding's EFFECTIVE coverage (B4) — declared when the author
    /// wrote the field, otherwise resolved per medium
    /// ([`memstead_base::binding::effective_coverage_semantics`]).
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
    /// Findings recorded under a **prior** `hash(D)`,
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
    /// The entity-side exclusion ledger as it applies this pass: `(entity,
    /// rationale)` for each destination entity without an anchor that a
    /// declaration marks deliberately so. Named, so the reader sees why the
    /// entity is not owed, never merely subtracted.
    pub excluded_entity_rationales: Vec<(String, String)>,
    /// Degradation flags (B1) — typed, human/agent-readable strings.
    pub degradations: Vec<String>,
    /// The binding's intent checked against the destination schema's
    /// relationship vocabulary ([`memstead_base::binding_intent`]): one entry per all-caps
    /// token the schema does not declare, code
    /// `BINDING_INTENT_UNKNOWN_RELATIONSHIP`. Reported, never refused, on a
    /// record that already carries one; empty proves a clean intent. Not a
    /// fidelity figure, so it never moves the rollup verdict.
    pub intent_findings: Vec<memstead_base::binding_intent::IntentFinding>,
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
const CLASS_SEVERITY: [&str; 6] = [
    "wrong",
    "drifted",
    "unresolvable-anchor",
    "uncovered",
    "unanchored-mention",
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
             and update the entity, then re-verify with `--advance` to move the baseline"
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
        "unanchored-mention" => format!(
            "{n} claim(s) name an in-scope artifact the entity does not anchor, so no verify \
             watches them — add the anchor (`memstead_update` with `anchors`), or exclude the \
             artifact with a rationale (`memstead projection exclude {binding}`)"
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
    /// The number an action sentence should state for `class`.
    ///
    /// Every class counts the findings RECORDED this pass, which is what its
    /// sentence claims — except `uncovered`, whose sentence says "N in-scope
    /// source artifact(s) carry no anchor". That is a statement about the
    /// enumerated coverage set, not about how many findings the pass got
    /// round to recording under its cap, and the two are not the same number
    /// once sampling or a cap bites. On the run this rule was filed from, the
    /// headline said 17 while the body listed 583: both were right about
    /// their own set, and the report contradicted itself in public.
    ///
    /// So `uncovered` takes its count from `coverage.uncovered`, the very
    /// list the body prints. Reconciling the two numbers afterwards was the
    /// rejected alternative: it would have hidden that they answer different
    /// questions instead of making the sentence count what it describes.
    fn action_count(&self, class: &str, findings: usize) -> usize {
        if class == "uncovered" {
            self.coverage.uncovered.len()
        } else {
            findings
        }
    }

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
        // Rows the axis could not adjudicate.
        // These EXTEND the existing blind-spot mechanism rather
        // than adding a parallel one, so an axis that measured only part of
        // its population reaches the inconclusive verdict the three-valued
        // rollup already provides.
        //
        // EXCLUSIONS ARE DELIBERATELY ABSENT from this list. An out-of-scope
        // or other-binding anchor is legal, excluded and named: a complete,
        // correct answer about a row this binding does not answer for. Folding
        // it in here would be the same collapse repaired on the
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
                actions.push(class_action(
                    class,
                    self.action_count(class, n),
                    &self.binding,
                ));
            }
        }
        // Any class the vocabulary grew past this list still surfaces, after
        // the ranked ones — an unknown class is never silently dropped.
        for (class, &n) in &self.findings_by_class {
            if n > 0 && !CLASS_SEVERITY.contains(&class.as_str()) {
                actions.push(class_action(class, n, &self.binding));
            }
        }

        // The adopt case: a mem that predates its binding is expected to
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
/// hints) mirroring [`memstead_base::overview::OverviewOutput`].
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

    // --- Adopt / onboarding framing ---
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
        "- uncovered (no anchor): {}{}\n",
        report.coverage.uncovered.len(),
        if report.coverage.excluded > 0 {
            format!(
                "; excluded on purpose (not owed): {}",
                report.coverage.excluded
            )
        } else {
            String::new()
        }
    ));
    // Claims no verify watches, beside the artifacts no entity covers: the
    // count is the finding class's, and the remedy names both dispositions.
    md.push_str(&format!(
        "- unanchored mentions (an entity names an in-scope artifact it does not anchor): {}{}\n",
        report.coverage.unanchored_mentions.len(),
        if report.coverage.unanchored_mentions.is_empty() {
            String::new()
        } else {
            " — remedy: add the anchor (`memstead_update` with `anchors`), or \
             `memstead projection exclude <binding>` the artifact with a rationale"
                .to_string()
        }
    ));
    // The unit, stated beside the figures (A3 AC4): the count is over
    // artifacts described, never over anchor rows.
    md.push_str(&format!(
        "- coverage unit: {} — {} describing entit{} over {} covered artifact(s)\n\n",
        report.coverage.unit,
        report.coverage.describing_entities,
        if report.coverage.describing_entities == 1 {
            "y"
        } else {
            "ies"
        },
        report.coverage.covered_artifacts
    ));

    // The entity side of coverage: destination entities no artifact stands
    // behind. A reading; the declared ones are named below with their reasons.
    if !report.coverage.unanchored_entities.is_empty() || report.coverage.excluded_entities > 0 {
        let listed: Vec<String> = report
            .coverage
            .unanchored_entities
            .iter()
            .take(20)
            .map(|e| format!("`{e}`"))
            .collect();
        let more = report
            .coverage
            .unanchored_entities
            .len()
            .saturating_sub(listed.len());
        md.push_str(&format!(
            "- entities without anchors (no verify can speak to them): {}{}{}{}\n\n",
            report.coverage.unanchored_entities.len(),
            if report.coverage.excluded_entities > 0 {
                format!(
                    "; excluded on purpose (not owed): {}",
                    report.coverage.excluded_entities
                )
            } else {
                String::new()
            },
            if listed.is_empty() {
                String::new()
            } else {
                format!(" — {}", listed.join(", "))
            },
            if more > 0 {
                format!(" … and {more} more")
            } else {
                String::new()
            },
        ));
    }

    // Coverage-semantics framing (B4). REFUSAL: under adopt, the exhaustive
    // branch must NOT frame the uncovered artifacts as defect findings — they are
    // the expected backfill worklist of a mem that predates its binding, never a
    // red verdict caused solely by pre-binding history.
    match report.coverage_semantics {
        CoverageSemantics::Exhaustive if report.adopt => {
            // `uncovered` is already net of the declared exclusions (C7), so
            // its length IS the backlog; subtracting again would double-count.
            let backlog = report.coverage.uncovered.len();
            md.push_str(&format!(
                "**Exhaustive coverage (onboarding):** {backlog} in-scope artifact(s) carry no \
                 entity yet ({} disposed excluded{}) — the expected first-sync backfill \
                 worklist for a mem that predates its binding, not defects.\n\n",
                report.disposed_excluded,
                already_out_clause(report.disposed_excluded)
            ));
        }
        CoverageSemantics::Exhaustive => {
            // `uncovered` is already net of the declared exclusions (C7).
            let findings = report.coverage.uncovered.len();
            md.push_str(&format!(
                "**Exhaustive coverage:** {findings} unaccounted artifact(s) — not anchored, not \
                 declared-excluded, no persisted disposition ({} disposed excluded{}) — are \
                 **findings**.\n\n",
                report.disposed_excluded,
                already_out_clause(report.disposed_excluded)
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

    // The entity-side ledger: each entity declared to carry no anchor, with
    // the reason, so a withdrawn claim reads as a decision and not as a gap.
    if !report.excluded_entity_rationales.is_empty() {
        md.push_str("**Entities excluded on purpose (declared to carry no anchor):**\n");
        for (entity, rationale) in &report.excluded_entity_rationales {
            if rationale.is_empty() {
                md.push_str(&format!("- `{entity}`\n"));
            } else {
                md.push_str(&format!("- `{entity}` — {rationale}\n"));
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
        "- resolution (non-`authored`, observed): **anchor-resolution %:** {}; drifted {}, \
         recheck {}, orphaned {}\n",
        report.anchors.figure.ratio(report.anchors.observed),
        report.anchors.drifted,
        report.anchors.recheck,
        report.anchors.orphaned,
    ));
    // What the denominator counted, stated rather than left to be assumed.
    // Rows and artifacts differ
    // whenever one artifact carries several legitimate rows at different
    // grains or classes, and a reader reads the figures above as being about
    // artifacts.
    md.push_str(&format!(
        "- the figures above count anchor ROWS: {} row(s) over {} distinct artifact(s)\n",
        report.anchors.counted_rows, report.anchors.distinct_artifacts
    ));
    // Rows adjudicated from a recorded observation (url rows): their state
    // above is as old as the observation it rests on, so the age travels
    // with the count.
    if !report.anchors.aging.is_empty() {
        md.push_str(&format!(
            "- {} counted row(s) rest on a recorded observation rather than a live one \
             (url anchors; the engine never fetches):\n",
            report.anchors.aging.len()
        ));
        const AGING_CAP: usize = 10;
        for a in report.anchors.aging.iter().take(AGING_CAP) {
            md.push_str(&format!(
                "  - `{}` → `{}`: unobserved for {} days (observed {})\n",
                a.entity, a.artifact, a.unobserved_for_days, a.observed_at
            ));
        }
        if report.anchors.aging.len() > AGING_CAP {
            md.push_str(&format!(
                "  - …and {} more\n",
                report.anchors.aging.len() - AGING_CAP
            ));
        }
    }
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
        "- superseded (prior `hash(D)`, segregated): {}\n\n",
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

    // --- Binding intent (hard-required only when there is something to say:
    // a clean intent renders nothing, so a clean report is unchanged) ---
    if !report.intent_findings.is_empty() {
        md.push_str("## Binding intent\n\n");
        for f in &report.intent_findings {
            md.push_str(&format!("- {f}\n"));
        }
        md.push_str(&format!(
            "\nRead each token as prose, never as an edge to write; fix the intent with \
             `memstead projection edit {} --patch '{{\"intent\": \"...\"}}'`.\n\n",
            report.binding
        ));
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

    // unanchored_mentions
    let mut s = String::new();
    if !report.coverage.unanchored_mentions.is_empty() {
        s.push_str("## Unanchored mentions\n\n");
        for m in &report.coverage.unanchored_mentions {
            s.push_str(&format!(
                "- `{}` names `{}` in `{}`\n",
                m.entity, m.artifact, m.section
            ));
        }
        s.push('\n');
    }
    out.push(("unanchored_mentions", s));

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

/// The clause that tells a reader the disposed-excluded artifacts are already
/// out of the headline count (C7). Empty when there are none: a binding with
/// no exclusions must render byte-identical to its pre-C7 output, which the
/// plan states as a constraint and which an ungated clause quietly broke for
/// every exhaustive binding in the workspace.
fn already_out_clause(disposed_excluded: usize) -> &'static str {
    if disposed_excluded > 0 {
        ", already out of that count"
    } else {
        ""
    }
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
            ChangeStrategy::Git => memstead_base::binding_run::find_git_root(
                &memstead_base::binding_run::source_base_path(p, workspace_root),
            )
            .is_some(),
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
            reason: "the medium type(s) are not enumerable".to_string(),
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
    let mut describing: BTreeSet<String> = BTreeSet::new();
    let mut tree_fanout: BTreeMap<(String, String), usize> = BTreeMap::new();
    let entity_end_reconciled = engine.entity_set_is_reconcilable(dest.as_str()).is_ok();
    for file in &s_d {
        // Filtered by BINDING, not merely by mem.
        // The mem filter alone let an anchor written by one
        // binding mark a file covered for another, which is the same
        // population defect the resolution figures had, one axis over. An
        // anchor with no recorded binding still counts, by the same
        // pre-provenance fallback the population uses: a mem whose anchors
        // predate the field must not read as wholly uncovered on upgrade.
        //
        // An anchor whose ENTITY is gone covers nothing either:
        // the artifact would otherwise read as covered on the
        // strength of a row no entity stands behind. Only applied when the
        // entity end could be reconciled at all, so an unreconcilable mem
        // keeps its old coverage rather than reading as wholly uncovered.
        let refs = engine.anchors_referencing_artifact(file);
        let mine: Vec<&(memstead_base::EntityId, memstead_base::anchor::Anchor)> = refs
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
        describing.extend(mine.iter().map(|(eid, _)| eid.as_ref().to_string()));
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

    // --- Durable authored-exclusion ledger (B4), applied to coverage (C7) ---
    // The advance store's `exclusions` map survives advance completion (unlike
    // its transient `dispositions`), and holds each artifact under the
    // canonical workspace-relative id `record_exclusions` resolved it to, which
    // is the spelling `s_d` enumerates. An artifact declared excluded is
    // therefore DROPPED from `uncovered` rather than marked: a reader counts
    // the array, and a gate reads it, so an entry that stays in it stays owed
    // however it is annotated. The count rides beside the figures as
    // `excluded`, and the rationales keep their own block below.
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
    let excluded_set: std::collections::BTreeSet<&str> = disposed_excluded_rationales
        .iter()
        .map(|(a, _)| a.as_str())
        .collect();
    uncovered.retain(|f| !excluded_set.contains(f.as_str()));

    // The entity side (the claims-register move, 2026-09-05): every
    // destination entity no anchor row stands behind, net of the entity
    // exclusions the ledger declares, which are named with their reasons.
    let holders = engine.mem_anchor_holders(dest.as_str());
    let mut unanchored_entities: Vec<String> = engine
        .store()
        .all_entities()
        .filter(|e| e.mem == dest.as_str() && !e.stub && !holders.contains(e.id.as_ref()))
        .map(|e| e.id.to_string())
        .collect();
    unanchored_entities.sort();
    let mut excluded_entity_rationales: Vec<(String, String)> = Vec::new();
    if let Some((mem, name)) = binding_id.split_once('/')
        && let Ok(Some(state)) = read_advance_store(workspace_root, mem, name)
    {
        for (entity, rationale) in &state.entity_exclusions {
            if unanchored_entities.iter().any(|e| e == entity) {
                excluded_entity_rationales.push((entity.clone(), rationale.clone()));
            }
        }
    }
    let excluded_entity_set: std::collections::BTreeSet<&str> = excluded_entity_rationales
        .iter()
        .map(|(e, _)| e.as_str())
        .collect();
    unanchored_entities.retain(|e| !excluded_entity_set.contains(e.as_str()));
    let excluded_entities = excluded_entity_rationales.len();

    // The claims no verify watches, read off the durable store's current
    // batch: the verify pass records one `unanchored-mention` finding per
    // (entity, artifact), already net of the exclusion ledger.
    let mut unanchored_mentions: Vec<UnanchoredMention> = Vec::new();
    if let Some((mem, name)) = binding_id.split_once('/')
        && let Ok(Some(store)) = read_findings_store(workspace_root, mem, name)
    {
        for f in store.current(key) {
            if let super::findings::FindingTarget::Mention {
                entity,
                artifact,
                section,
            } = &f.target
            {
                unanchored_mentions.push(UnanchoredMention {
                    entity: entity.clone(),
                    artifact: artifact.clone(),
                    section: section.clone(),
                });
            }
        }
    }
    unanchored_mentions.sort();

    let coverage = GrainCoverage {
        denominator,
        covered_artifacts: direct_covered + tree_only_covered,
        describing_entities: describing.len(),
        unit: COVERAGE_UNIT,
        direct_covered,
        tree_only_covered,
        uncovered: uncovered.clone(),
        excluded: disposed_excluded,
        unanchored_mentions,
        tree_anchors,
        unanchored_entities,
        excluded_entities,
    };

    // --- Anchor composition + resolution over THIS BINDING'S anchors ---
    // Scoped rather than mem-wide (consistency-sweep 03/01): the axis answers
    // for the population this binding is responsible for, and names the rest.
    let population =
        crate::anchor_population::population_for(engine, resolved, Some(key.binding_hash.as_str()));
    let mut resolves = 0usize;
    let mut anchors = AnchorComposition {
        counted_rows: population.included.len(),
        distinct_artifacts: population.distinct_artifacts(),
        excluded_other_binding: population
            .excluded_count(crate::anchor_population::ExclusionReason::OtherBinding),
        excluded_out_of_scope: population
            .excluded_count(crate::anchor_population::ExclusionReason::OutOfScope),
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
                r.anchor.hash_source == Some(memstead_base::anchor::AnchorHashSource::Backfill)
            })
            .count(),
        aging: {
            let today = memstead_base::engine::mutation::iso_now();
            let mut rows: Vec<AgingAnchor> = population
                .included
                .iter()
                .filter_map(|(eid, r)| {
                    let at = r.observed_at.as_deref()?;
                    Some(AgingAnchor {
                        entity: eid.as_ref().to_string(),
                        artifact: r.anchor.artifact.clone(),
                        observed_at: at.to_string(),
                        unobserved_for_days: memstead_base::anchor::days_between(at, &today)
                            .unwrap_or(0),
                    })
                })
                .collect();
            rows.sort_by_key(|a| std::cmp::Reverse(a.unobserved_for_days));
            rows
        },
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
                resolves += 1;
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
    }
    // The figure closes here, with the population it was computed over: rows
    // and artifacts differ whenever one artifact carries several legitimate
    // rows, and a reader reads the figure as being about artifacts.
    anchors.figure = memstead_base::anchor::AnchorResolutionFigure::new(
        resolves,
        format!(
            "over {} counted row(s) on {} distinct artifact(s), with {} unobserved this pass \
             (state unavailable, never scored as resolved)",
            anchors.counted_rows, anchors.distinct_artifacts, anchors.unobserved
        ),
        anchors.recheck == 0 && anchors.unobserved == 0,
    )
    .expect("the population statement is never empty");
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

    // Adopt / onboarding signal — the single canonical predicate shared with
    // the sync brief and the status rollup: a mem with no anchors and no recorded
    // `#synced` baseline predates its binding, so 0% anchored is expected.
    let adopt = super::render::mem_predates_binding(engine, resolved);
    let effective_coverage = memstead_base::binding::effective_coverage_semantics(binding);
    let intent_findings = memstead_base::binding_intent::binding_intent_findings(
        engine,
        &dest,
        binding.intent.as_deref(),
    );

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
        excluded_entity_rationales,
        degradations,
        intent_findings,
    }
}

/// Whether a resolved change-detection strategy can retrieve a prior base
/// version of an artifact (B1). Only git-backed strategies (`git`, `graph`)
/// hold prior content; `mtime` reports *that* an artifact changed but not its
/// previous bytes, and `none` detects nothing. The report states this as a
/// fact about the source; nothing in the engine acts on it (prune proposes
/// and never merges).
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
        super::findings::FindingTarget::Mention {
            entity,
            artifact,
            section,
        } => format!("{entity} names {artifact} in {section}"),
    }
}

#[cfg(test)]
mod rollup_tests;
#[cfg(test)]
mod tests;
