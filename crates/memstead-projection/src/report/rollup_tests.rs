#![cfg(test)]

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
                covered_artifacts: 4,
                describing_entities: 2,
                unit: COVERAGE_UNIT,
                direct_covered: 4,
                tree_only_covered: 0,
                uncovered: Vec::new(),
                excluded: 0,
                unanchored_mentions: Vec::new(),
                tree_anchors: Vec::new(),
                unanchored_entities: Vec::new(),
                excluded_entities: 0,
            },
            anchors: AnchorComposition {
                by_class: BTreeMap::from([("anchored".to_string(), 4)]),
                by_grain: BTreeMap::from([("file".to_string(), 4)]),
                authored: 0,
                observed: 4,
                figure: memstead_base::anchor::AnchorResolutionFigure::new(
                    4,
                    "over 6 counted row(s) on 6 distinct artifact(s), with 0 unobserved this pass (state unavailable, never scored as resolved)",
                    true,
                )
                .unwrap(),
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
            excluded_entity_rationales: Vec::new(),
            degradations: Vec::new(),
            intent_findings: Vec::new(),
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
    r.anchors.figure = memstead_base::anchor::AnchorResolutionFigure::new(
            0,
            "over 0 counted row(s) on 0 distinct artifact(s), with 0 unobserved this pass (state unavailable, never scored as resolved)",
            true,
        )
        .unwrap();
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

/// A workspace with one folder mem `engine` (default@1.0.0), a git source
/// tree at the root, and `files` written relative to the root. Returns the
/// root; the caller writes the binding.
fn a3_workspace(root: &std::path::Path, files: &[&str]) {
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
    let mount = memstead_base::workspace::Mount {
        mem: "engine".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: memstead_base::workspace::MountStorage::Folder {
            path: mem_dir.clone(),
        },
        capability: memstead_base::workspace::MountCapability::Write,
        lifecycle: memstead_base::workspace::MountLifecycle::Eager,
        cross_linkable: false,
        migration_target: None,
    };
    memstead_base::workspace_store::WorkspaceStoreAdapter::save_state(
        &memstead_base::FileWorkspaceStore::new(),
        root,
        &memstead_base::workspace::Workspace {
            mounts: vec![mount],
            settings: memstead_base::workspace::WorkspaceSettings::default(),
        },
    )
    .unwrap();
    let out = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(out.status.success());
    for f in files {
        let p = root.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "fn x() {}\n").unwrap();
    }
}

fn a3_binding(
    sources: &[(&str, &str)],
    deny: &[&str],
    batch_size: u32,
) -> memstead_base::binding::Binding {
    memstead_base::binding::Binding {
        version: memstead_base::binding::BINDING_VERSION,
        intent: None,
        sources: sources
            .iter()
            .map(|(name, glob)| memstead_base::pipeline::Source {
                name: name.to_string(),
                medium_type: memstead_base::pipeline::MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
                scope: vec![memstead_base::pipeline::PatternEntry {
                    path: glob.to_string(),
                    mode: memstead_base::pipeline::PatternMode::Allow,
                }],
                engagement: None,
                preparation: None,
            })
            .collect(),
        reference_mems: Vec::new(),
        destination_mem: "engine".to_string(),
        deny_paths: deny.iter().map(|d| d.to_string()).collect(),
        coverage_semantics: None,
        rules: None,
        prune: None,
        operations: memstead_base::binding::Operations {
            build: Some(memstead_base::binding::BuildOperation {
                mode: memstead_base::binding::BuildMode::Discovery,
                trigger: memstead_base::pipeline::IngestTrigger::Loop,
                batch_size,
                post_actions: None,
            }),
            sync: None,
            verify: Some(memstead_base::binding::VerifyOperation {
                trigger: memstead_base::pipeline::IngestTrigger::Manual,
                batch_size,
                adjudication_cap: memstead_base::binding::DEFAULT_ADJUDICATION_CAP,
                // Scheduled full walks off: the sampled path is under test.
                full_resync_every: 0,
            }),
        },
    }
}

/// A3 AC4: coverage counts describing entities per artifact — an artifact
/// with three anchors from one entity counts once, one with an anchor from
/// each of two entities counts once; the report states the unit.
#[test]
fn coverage_counts_artifacts_described_not_anchor_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    a3_workspace(root, &["src/three.rs", "src/two.rs", "src/none.rs"]);
    let mem_dir = root.join("mem");
    for slug in ["one", "left", "right"] {
        std::fs::write(
            mem_dir.join(format!("{slug}.md")),
            "---\ntype: decision\n---\n\n# E\n\n## Decision\n\nBody.\n",
        )
        .unwrap();
    }
    let mk = |artifact: &str| memstead_base::anchor::Anchor {
        artifact: artifact.to_string(),
        grain: memstead_base::anchor::AnchorGrain::File,
        class: memstead_base::anchor::AnchorProvenanceClass::Anchored,
        at_version: None,
        hash: Some("recorded".to_string()),
        hash_stability: memstead_base::anchor::AnchorHashStability::Stable,
        derived_from: Vec::new(),
        binding: None,
        source: None,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    };
    let mut sidecar = memstead_base::anchor::AnchorSidecar::default();
    sidecar.set(
        "engine--one",
        vec![mk("src/three.rs"), mk("src/three.rs"), mk("src/three.rs")],
    );
    sidecar.set("engine--left", vec![mk("src/two.rs")]);
    sidecar.set("engine--right", vec![mk("src/two.rs")]);
    std::fs::write(
        mem_dir.join(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
        sidecar.to_bytes(),
    )
    .unwrap();
    let b = a3_binding(&[("graph", "src/**/*.rs")], &[], 20);
    memstead_base::pipeline_store::write_binding(root, "engine", "graph", &b).unwrap();

    let engine = memstead_base::Engine::from_workspace_root(root).unwrap();
    let resolved = memstead_base::binding_run::resolve_binding_run("engine/graph", &b).unwrap();
    let outcome = crate::findings::verify_binding(&engine, root, &b, &resolved).unwrap();
    let report = super::compute_fidelity_report(&engine, root, &b, &resolved, &outcome.key);
    assert_eq!(
        report.coverage.denominator,
        super::DenominatorBasis::Enumerated { count: 3 }
    );
    assert_eq!(
        report.coverage.covered_artifacts, 2,
        "{:?}",
        report.coverage
    );
    assert_eq!(
        report.coverage.describing_entities, 3,
        "{:?}",
        report.coverage
    );
    assert_eq!(report.coverage.uncovered, vec!["src/none.rs".to_string()]);
    let md = super::render_fidelity_report(&report, 8_000, &[]).markdown;
    assert!(
        md.contains("coverage unit: describing entities per artifact"),
        "{md}"
    );
    assert!(
        md.contains("3 describing entities over 2 covered artifact(s)"),
        "{md}"
    );
}
