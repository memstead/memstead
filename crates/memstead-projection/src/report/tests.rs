#![cfg(test)]

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
                covered_artifacts: 9,
                describing_entities: 4,
                unit: COVERAGE_UNIT,
                direct_covered: 6,
                tree_only_covered: 3,
                uncovered: vec!["src/a.rs".to_string()],
                excluded: 0,
                unanchored_mentions: Vec::new(),
                tree_anchors: vec![TreeFanout {
                    entity: "engine--big".to_string(),
                    artifact: "src/".to_string(),
                    fanout: 3,
                }],
                unanchored_entities: Vec::new(),
                excluded_entities: 0,
            },
            anchors: AnchorComposition {
                by_class: BTreeMap::from([
                    ("anchored".to_string(), 5),
                    ("authored".to_string(), 2),
                ]),
                by_grain: BTreeMap::from([("file".to_string(), 4), ("tree".to_string(), 1)]),
                authored: 2,
                observed: 5,
                figure: memstead_base::anchor::AnchorResolutionFigure::new(
                    4,
                    "over 6 counted row(s) on 6 distinct artifact(s), with 0 unobserved this pass (state unavailable, never scored as resolved)",
                    true,
                )
                .unwrap(),
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
            excluded_entity_rationales: Vec::new(),
            degradations: vec!["hash-adjudication-deferred — 1 anchor(s) recheck".to_string()],
            intent_findings: Vec::new(),
        }
}

/// A counted row resting on a recorded observation (a url anchor) renders
/// with its age beside the resolution figures it contributed to.
#[test]
fn aging_rows_render_with_their_age() {
    let mut r = base_report();
    r.anchors.aging = vec![AgingAnchor {
        entity: "engine--cites".to_string(),
        artifact: "https://w.test/living".to_string(),
        observed_at: "2026-08-03T09:00:00Z".to_string(),
        unobserved_for_days: 30,
    }];
    let md = render_fidelity_report(&r, 8_000, &[]).markdown;
    assert!(
        md.contains("1 counted row(s) rest on a recorded observation"),
        "{md}"
    );
    assert!(
            md.contains(
                "`engine--cites` → `https://w.test/living`: unobserved for 30 days (observed 2026-08-03T09:00:00Z)"
            ),
            "{md}"
        );
    let plain = render_fidelity_report(&base_report(), 8_000, &[]).markdown;
    assert!(!plain.contains("recorded observation"), "{plain}");
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
/// mod-time signal) has no retrievable base version, so its facet
/// capability reports `base_version_retrievable: false`; the same
/// filesystem medium backed by `git` reports `true`.
#[test]
fn b1_base_retrievability_follows_resolved_strategy_not_medium_ceiling() {
    use memstead_base::pipeline::MediumType;

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
        "filesystem+mtime has no retrievable base version"
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
        "filesystem backed by git retrieves a base version"
    );

    // A detection-less strategy also has no base leg.
    assert!(!strategy_retrieves_base(ChangeStrategy::None));
    assert!(!strategy_retrieves_base(ChangeStrategy::Mtime));
    assert!(strategy_retrieves_base(ChangeStrategy::Git));
    assert!(strategy_retrieves_base(ChangeStrategy::Graph));
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
    let forced = render_fidelity_report(&r, hard_cost + 5, &["uncovered_artifacts".to_string()]);
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

/// B4 — a persisted disposition keeps an artifact out of the exhaustive
/// findings count.
///
/// Reworked 2026-09-03 (C7): the subtraction moved OUT of the renderer.
/// The composer now drops declared-excluded artifacts from
/// `coverage.uncovered` itself, so the array a gate reads is already net
/// and the renderer must not subtract again. The fixture therefore holds
/// the post-composer shape (one uncovered, one excluded) rather than the
/// pre-subtraction one, and the renderer is checked for NOT
/// double-counting.
#[test]
fn b4_disposition_excludes_from_exhaustive_findings() {
    let mut r = base_report();
    r.coverage_semantics = CoverageSemantics::Exhaustive;
    // `src/b.rs` was excluded and is already out of `uncovered`.
    r.coverage.uncovered = vec!["src/a.rs".to_string()];
    r.coverage.excluded = 1;
    r.disposed_excluded = 1;
    let md = render_fidelity_report(&r, 8_000, &[]).markdown;
    assert!(md.contains("1 unaccounted artifact(s)"), "{md}");
    assert!(
        md.contains("1 disposed excluded, already out of that count"),
        "{md}"
    );
    // …and the clause is GATED: see
    // `c7_no_exclusions_renders_byte_identically_to_the_pre_plan_output`.
    // The figure beside the coverage numbers carries the excluded count,
    // so a reader is not left to wonder where the artifact went.
    assert!(
        md.contains("- uncovered (no anchor): 1; excluded on purpose (not owed): 1"),
        "{md}"
    );
}

/// C9 — the uncovered action sentence counts the SET IT DESCRIBES, so
/// the headline and the body can never disagree.
///
/// Filed from a run whose headline said 17 uncovered while its body
/// listed 583: the sentence counted findings recorded under the pass cap,
/// the list counted the enumerated coverage set. Both were right about
/// their own set, which is exactly why the report was misleading. Every
/// other class still counts findings, because that is what those
/// sentences claim.
#[test]
fn c9_the_uncovered_action_counts_the_body_list_not_the_recorded_findings() {
    let mut r = base_report();
    r.coverage_semantics = CoverageSemantics::Exhaustive;
    // The body enumerates three; the pass recorded only one finding,
    // as a cap or a sample would leave it.
    r.coverage.uncovered = vec![
        "src/a.rs".to_string(),
        "src/b.rs".to_string(),
        "src/c.rs".to_string(),
    ];
    r.findings_by_class = [("uncovered".to_string(), 1)].into_iter().collect();
    let rollup = r.rollup();
    let uncovered_action = rollup
        .actions
        .iter()
        .find(|a| a.contains("carry no anchor"))
        .expect("the uncovered action is present");
    assert!(
        uncovered_action.starts_with("3 in-scope source artifact(s)"),
        "the sentence counts the enumerated list it describes: {uncovered_action}"
    );
    // The findings tally itself is untouched: it answers a different
    // question and the verdict line still reports it.
    assert_eq!(rollup.findings_total, 1, "{rollup:?}");

    // A different class keeps counting findings.
    let mut d = base_report();
    d.coverage.uncovered = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];
    d.findings_by_class = [("drifted".to_string(), 1)].into_iter().collect();
    let drift_action = d
        .rollup()
        .actions
        .iter()
        .find(|a| a.contains("moved since the entity was written"))
        .cloned()
        .expect("the drift action is present");
    assert!(
        drift_action.starts_with("1 anchored artifact(s)"),
        "{drift_action}"
    );
}

/// C7 — a binding with no exclusions renders BYTE-IDENTICALLY to the
/// pre-plan output, which is the plan's constraint and its refusal
/// complement. Both C7 additions are gated on there being an exclusion:
/// the trailing clause on the uncovered figure, and the "already out of
/// that count" clause on the exhaustive headline.
///
/// The first version of this test asserted only the bare uncovered line
/// and the absence of "excluded on purpose". It passed while the
/// exhaustive headline had gained an ungated clause, so every exhaustive
/// binding in the world rendered differently and the constraint was
/// broken with a green test. The grader caught it by diffing two
/// binaries; these assertions pin BOTH lines exactly so a future
/// addition cannot slip in the same way.
#[test]
fn c7_no_exclusions_renders_byte_identically_to_the_pre_plan_output() {
    let mut r = base_report();
    r.coverage_semantics = CoverageSemantics::Exhaustive;
    r.coverage.uncovered = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];
    r.coverage.excluded = 0;
    r.disposed_excluded = 0;
    let md = render_fidelity_report(&r, 8_000, &[]).markdown;
    assert!(md.contains("- uncovered (no anchor): 2\n"), "{md}");
    assert!(!md.contains("excluded on purpose"), "{md}");
    // The pre-plan wording of the headline, to the closing parenthesis.
    assert!(
        md.contains("2 unaccounted artifact(s)") && md.contains("(0 disposed excluded)"),
        "{md}"
    );
    assert!(!md.contains("already out of that count"), "{md}");

    // The onboarding branch renders its pre-plan wording too.
    let mut adopt = r.clone();
    adopt.adopt = true;
    let md = render_fidelity_report(&adopt, 8_000, &[]).markdown;
    assert!(md.contains("(0 disposed excluded)"), "{md}");
    assert!(!md.contains("already out of that count"), "{md}");
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
        reason: "the medium type(s) are not enumerable".to_string(),
    };
    let md2 = render_fidelity_report(&non, 8_000, &[]).markdown;
    assert!(md2.contains("No `S(D)` denominator"));
    assert!(md2.contains("not enumerable"));
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
    assert!(md.contains("**Backfill path:** run `memstead projection brief engine/graph --sync`"));
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

use crate::findings::verify_binding;
use memstead_base::anchor::{Anchor, AnchorHashStability, AnchorProvenanceClass, AnchorSidecar};
use memstead_base::binding::{
    BINDING_VERSION, Binding, BuildMode, BuildOperation, DEFAULT_ADJUDICATION_CAP,
    DEFAULT_FULL_RESYNC_EVERY, Operations, VerifyOperation,
};
use memstead_base::binding_run::resolve_binding_run;
use memstead_base::pipeline::{IngestTrigger, MediumType, PatternEntry, PatternMode};
use memstead_base::pipeline_store::{load_pipeline_configs, write_binding};
use memstead_base::workspace::{
    Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
};
use memstead_base::workspace_store::WorkspaceStoreAdapter;

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
) -> (FidelityReport, crate::findings::VerifyOutcome, String) {
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
    memstead_base::FileWorkspaceStore::new()
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
        last_observed: None,
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
        mem_dir.join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
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
            sources: vec![memstead_base::pipeline::Source {
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

    // Populate the durable findings store — read-only on the mem.
    let outcome = verify_binding(&engine, root, binding, &resolved).unwrap();

    // Assemble the tier-1 report under the same key.
    let report = compute_fidelity_report(&engine, root, binding, &resolved, &outcome.key);
    let md = render_fidelity_report(&report, 8_000, &[]).markdown;
    (report, outcome, md)
}

fn end_to_end_body(report: &FidelityReport, outcome: &crate::findings::VerifyOutcome, md: &str) {
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
    memstead_base::FileWorkspaceStore::new()
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
            sources: vec![memstead_base::pipeline::Source {
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

    // No anchors + no baseline → the mem predates its binding.
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
