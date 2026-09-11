#![cfg(test)]

use super::*;

/// Redaction blanks exactly the two artifact-reference fields — to the
/// pinned sentinel, never removal — and keeps everything else: class,
/// grain, `at_version`, hash, hash-stability, binding, source, and the
/// per-entity anchor counts.
#[test]
fn redaction_blanks_references_and_keeps_trust_metadata() {
    let mut sidecar = AnchorSidecar::default();
    sidecar.set(
        "m--alpha",
        vec![
            Anchor {
                artifact: "src/lib.rs".into(),
                grain: AnchorGrain::File,
                class: AnchorProvenanceClass::Anchored,
                at_version: Some(AnchorVersion::Commit("abc123".into())),
                hash: Some("h1".into()),
                hash_stability: AnchorHashStability::Stable,
                derived_from: vec![],
                binding: Some("bhash".into()),
                source: Some("source-tree".into()),
                span_unvalidated: false,
                hash_source: None,
                last_observed: None,
            },
            Anchor {
                artifact: "docs/summary.md".into(),
                grain: AnchorGrain::File,
                class: AnchorProvenanceClass::Derived,
                at_version: None,
                hash: Some("h2".into()),
                hash_stability: AnchorHashStability::Unstable,
                derived_from: vec!["notes/a.md".into(), "notes/b.md".into()],
                binding: None,
                source: None,
                span_unvalidated: false,
                hash_source: None,
                last_observed: None,
            },
        ],
    );

    sidecar.redact_artifact_references();

    let anchors = sidecar.get("m--alpha");
    assert_eq!(anchors.len(), 2, "no anchor entry is dropped");
    for a in anchors {
        assert_eq!(a.artifact, REDACTED_ARTIFACT_SENTINEL);
        for d in &a.derived_from {
            assert_eq!(d, REDACTED_ARTIFACT_SENTINEL);
        }
    }
    assert_eq!(
        anchors[0].at_version,
        Some(AnchorVersion::Commit("abc123".into()))
    );
    assert_eq!(anchors[0].hash.as_deref(), Some("h1"));
    assert_eq!(anchors[0].binding.as_deref(), Some("bhash"));
    assert_eq!(anchors[0].source.as_deref(), Some("source-tree"));
    assert_eq!(anchors[1].class, AnchorProvenanceClass::Derived);
    assert_eq!(anchors[1].derived_from.len(), 2, "derivation arity kept");
    // A redacted sidecar is structurally valid — the sentinel is not
    // an empty reference.
    sidecar.validate_artifact_references().unwrap();
}

/// The structural reference check refuses empty `artifact` and empty
/// `derived_from` entries — including a botched redaction that blanked
/// to nothing instead of the sentinel.
#[test]
fn empty_artifact_references_are_refused() {
    let mut sidecar = AnchorSidecar::default();
    sidecar.set(
        "m--alpha",
        vec![Anchor {
            artifact: "".into(),
            grain: AnchorGrain::File,
            class: AnchorProvenanceClass::Anchored,
            at_version: None,
            hash: None,
            hash_stability: AnchorHashStability::Stable,
            derived_from: vec![],
            binding: None,
            source: None,
            span_unvalidated: false,
            hash_source: None,
            last_observed: None,
        }],
    );
    assert!(sidecar.validate_artifact_references().is_err());

    let mut sidecar = AnchorSidecar::default();
    sidecar.set(
        "m--beta",
        vec![Anchor {
            artifact: "docs/x.md".into(),
            grain: AnchorGrain::File,
            class: AnchorProvenanceClass::Derived,
            at_version: None,
            hash: None,
            hash_stability: AnchorHashStability::Stable,
            derived_from: vec!["  ".into()],
            binding: None,
            source: None,
            span_unvalidated: false,
            hash_source: None,
            last_observed: None,
        }],
    );
    assert!(sidecar.validate_artifact_references().is_err());
}

// -- wire vocabulary is the contract -----------------------------------

#[test]
fn class_wire_strings_are_stable() {
    assert_eq!(AnchorProvenanceClass::Anchored.as_wire(), "anchored");
    assert_eq!(AnchorProvenanceClass::Derived.as_wire(), "derived");
    assert_eq!(AnchorProvenanceClass::Authored.as_wire(), "authored");
    assert_eq!(AnchorProvenanceClass::InformedBy.as_wire(), "informed-by");
    for w in AnchorProvenanceClass::WIRE_VALUES {
        assert_eq!(AnchorProvenanceClass::from_wire(w).unwrap().as_wire(), *w);
    }
    assert!(AnchorProvenanceClass::from_wire("bogus").is_none());
}

#[test]
fn grain_wire_strings_are_stable() {
    for w in AnchorGrain::WIRE_VALUES {
        assert_eq!(AnchorGrain::from_wire(w).unwrap().as_wire(), *w);
    }
    assert_eq!(
        AnchorGrain::WIRE_VALUES,
        &["span", "file", "tree", "url", "entity"]
    );
    assert!(AnchorGrain::from_wire("chunk").is_none());
}

#[test]
fn stability_and_state_wire_strings_are_stable() {
    assert_eq!(AnchorHashStability::Stable.as_wire(), "stable");
    assert_eq!(AnchorHashStability::Unstable.as_wire(), "unstable");
    assert_eq!(AnchorState::Resolves.as_wire(), "resolves");
    assert_eq!(AnchorState::Drifted.as_wire(), "drifted");
    assert_eq!(AnchorState::Recheck.as_wire(), "recheck");
    assert_eq!(AnchorState::Orphaned.as_wire(), "orphaned");
}

#[test]
fn only_anchored_and_derived_are_hash_bearing() {
    assert!(AnchorProvenanceClass::Anchored.is_hash_bearing());
    assert!(AnchorProvenanceClass::Derived.is_hash_bearing());
    assert!(!AnchorProvenanceClass::Authored.is_hash_bearing());
    assert!(!AnchorProvenanceClass::InformedBy.is_hash_bearing());
}

// -- grain / namespace matrix ------------------------------------------

#[test]
fn grain_namespace_support_matches_capability_matrix() {
    // path-shaped grains need path / path+commit.
    for g in [AnchorGrain::Span, AnchorGrain::File, AnchorGrain::Tree] {
        assert!(g.supported_by_namespace("path"));
        assert!(g.supported_by_namespace("path+commit"));
        assert!(!g.supported_by_namespace("url"));
        assert!(!g.supported_by_namespace("entity"));
    }
    assert!(AnchorGrain::Url.supported_by_namespace("url"));
    // A URL is an absolute reference: admitted beside every medium.
    assert!(AnchorGrain::Url.supported_by_namespace("path"));
    assert!(AnchorGrain::Url.supported_by_namespace("entity"));
    assert!(AnchorGrain::Entity.supported_by_namespace("entity"));
    assert!(!AnchorGrain::Entity.supported_by_namespace("path"));
}

// -- validation refusals -----------------------------------------------

fn valid_input() -> AnchorInput {
    AnchorInput {
        artifact: Some("src/lib.rs".into()),
        grain: Some("file".into()),
        class: Some("anchored".into()),
        hash_stability: Some("stable".into()),
        hash: Some("abc123".into()),
        ..Default::default()
    }
}

fn span_input(artifact: &str) -> AnchorInput {
    AnchorInput {
        artifact: Some(artifact.into()),
        grain: Some("span".into()),
        class: Some("anchored".into()),
        ..Default::default()
    }
}

/// Criterion 1 (consistency-sweep 03/03): a locator that can never
/// address anything is refused at the moment of writing. Each of these
/// used to write successfully and could then never be adjudicated.
#[test]
fn a_span_locator_that_addresses_nothing_is_refused() {
    for artifact in [
        "src/lib.rs#",      // announces a span, names none
        "src/lib.rs#   ",   // the same, in whitespace
        "src/lib.rs#L0",    // lines are 1-based
        "src/lib.rs#L0-L4", // and so is a range's start
        "src/lib.rs#L9-L2", // ends before it starts
        "src/lib.rs#L4-L",  // half a range
        "src/lib.rs#L4-x",  // a range that stops being one
    ] {
        let err = span_input(artifact)
            .validate(Some(("codebase", "path")))
            .expect_err(artifact);
        assert!(
            matches!(err, AnchorValidationError::SpanLocatorUnusable { .. }),
            "{artifact} refused as {err:?}"
        );
        assert_eq!(err.code(), INVALID_ANCHOR_CODE);
        assert!(err.detail().contains_key("expected"), "carries the repair");
    }
}

/// Criterion 4's first half: a span that names something real still
/// writes. `Lx` forms within the artifact, a preparation's unit key, and
/// a bare path (which addresses the whole file, and is what a span's hash
/// covers anyway) are all legal.
#[test]
fn a_usable_span_locator_still_writes() {
    for artifact in [
        "src/lib.rs",
        "src/lib.rs#L1",
        "src/lib.rs#L4-L7",
        "logs/ops.md#2026-08-25T00:00:00",
    ] {
        span_input(artifact)
            .validate(Some(("codebase", "path")))
            .unwrap_or_else(|e| panic!("{artifact} refused: {e}"));
    }
}

/// Criterion 2: where the content is already in hand, a range beyond the
/// artifact's end is refused rather than stored as an anchor pointing at
/// lines the file does not have.
#[test]
fn a_span_beyond_supplied_content_is_refused() {
    let mut i = span_input("src/lib.rs#L2-L9");
    i.content = Some(
        "one
two
three
"
        .into(),
    );
    let err = i.validate(Some(("codebase", "path"))).unwrap_err();
    match err {
        AnchorValidationError::SpanOutsideContent { lines, .. } => assert_eq!(lines, 3),
        other => panic!("wrong refusal: {other:?}"),
    }

    let mut ok = span_input("src/lib.rs#L2-L3");
    ok.content = Some(
        "one
two
three
"
        .into(),
    );
    let a = ok.validate(Some(("codebase", "path"))).unwrap();
    assert!(
        !a.span_unvalidated,
        "a span checked against content is not unvalidated"
    );
}

/// Criterion 3: where the write path holds no content, the span cannot be
/// checked without a read it deliberately does not perform. The anchor is
/// accepted and the row says the span is unverified, so no later surface
/// reports it as adjudicated.
#[test]
fn an_uncheckable_span_is_accepted_and_recorded_as_unchecked() {
    let a = span_input("src/lib.rs#L4-L7")
        .validate(Some(("codebase", "path")))
        .unwrap();
    assert!(a.span_unvalidated);

    let whole_file = span_input("src/lib.rs")
        .validate(Some(("codebase", "path")))
        .unwrap();
    assert!(
        !whole_file.span_unvalidated,
        "no locator addresses the whole artifact, which the existence gate checks"
    );

    let file_grain = valid_input().validate(Some(("codebase", "path"))).unwrap();
    assert!(!file_grain.span_unvalidated, "never set off the span grain");
}

/// Criterion 8: a hash the writer supplied is recorded as theirs, so a
/// reader can later tell it from one the backfill inferred.
#[test]
fn an_authored_hash_records_that_the_author_pinned_it() {
    let a = valid_input().validate(Some(("codebase", "path"))).unwrap();
    assert_eq!(a.hash_source, Some(AnchorHashSource::Author));

    let mut hashless = valid_input();
    hashless.hash = None;
    let b = hashless.validate(Some(("codebase", "path"))).unwrap();
    assert_eq!(b.hash_source, None, "no baseline, no origin to record");
}

/// Criteria 5 and 6: a re-pin that says nothing about the hash keeps the
/// baseline it did not mention, one that supplies a hash replaces it, and
/// unsetting the row first is the explicit way to clear it.
#[test]
fn a_re_pin_keeps_the_baseline_it_did_not_mention() {
    let mut sc = AnchorSidecar::default();
    let mut pinned = file_anchor("src/a.rs", "h-original");
    pinned.hash_source = Some(AnchorHashSource::Author);
    sc.set("m--e", vec![pinned]);

    let mut repin = file_anchor("src/a.rs", "");
    repin.hash = None;
    repin.hash_source = None;
    sc.merge("m--e", &[], vec![repin], false);
    let row = &sc.entities["m--e"][0];
    assert_eq!(
        row.hash.as_deref(),
        Some("h-original"),
        "the baseline the caller did not mention survives"
    );
    assert_eq!(row.hash_source, Some(AnchorHashSource::Author));

    sc.merge("m--e", &[], vec![file_anchor("src/a.rs", "h-new")], false);
    assert_eq!(
        sc.entities["m--e"][0].hash.as_deref(),
        Some("h-new"),
        "a supplied hash still replaces"
    );

    // The explicit clear: unset the row, then write it fresh. Unsets are
    // applied before the merge, so the old row is gone first.
    let unset = AnchorUnset {
        artifact: "src/a.rs".into(),
        grain: None,
        class: None,
    };
    let mut fresh = file_anchor("src/a.rs", "");
    fresh.hash = None;
    fresh.hash_source = None;
    sc.merge("m--e", &[unset], vec![fresh], false);
    assert_eq!(
        sc.entities["m--e"][0].hash, None,
        "unset-then-write is how a caller clears a baseline"
    );
}

#[test]
fn validate_accepts_a_well_formed_anchor() {
    let a = valid_input().validate(Some(("codebase", "path"))).unwrap();
    assert_eq!(a.artifact, "src/lib.rs");
    assert_eq!(a.grain, AnchorGrain::File);
    assert_eq!(a.class, AnchorProvenanceClass::Anchored);
    assert_eq!(a.hash.as_deref(), Some("abc123"));
    assert_eq!(a.hash_stability, AnchorHashStability::Stable);
}

/// Path grains keep their `stable` default — pinned, because the
/// per-grain default that gives `url` its `unstable` must not leak.
#[test]
fn validate_defaults_hash_stability_to_stable() {
    for grain in ["span", "file", "tree"] {
        let mut i = valid_input();
        i.grain = Some(grain.into());
        i.hash_stability = None;
        let a = i.validate(None).unwrap();
        assert_eq!(a.hash_stability, AnchorHashStability::Stable, "{grain}");
    }
    let mut e = valid_input();
    e.grain = Some("entity".into());
    e.artifact = Some("m--e".into());
    e.hash_stability = None;
    assert_eq!(
        e.validate(None).unwrap().hash_stability,
        AnchorHashStability::Stable
    );
}

/// A `url` anchor defaults to `unstable` (a served page is a moving
/// target — a hash break resolves `recheck`, never `drifted`) unless the
/// author asserts `stable`.
#[test]
fn validate_defaults_url_grain_to_unstable_unless_declared() {
    let mut i = valid_input();
    i.grain = Some("url".into());
    i.artifact = Some("https://example.invalid/doc".into());
    i.hash_stability = None;
    assert_eq!(
        i.validate(None).unwrap().hash_stability,
        AnchorHashStability::Unstable
    );
    i.hash_stability = Some("stable".into());
    assert_eq!(
        i.validate(None).unwrap().hash_stability,
        AnchorHashStability::Stable
    );
}

/// Supplied `content` becomes the registry's prepared hash: for a `url`
/// anchor the same canonicalization the path grains use over what the
/// observer read; for `file`/`span` the hash the engine would compute
/// from the file itself. `hash` beside it is refused, as is content on
/// a grain the registry never prepares from bytes, or on a non-hash
/// class.
#[test]
fn content_yields_the_prepared_hash_through_the_registry() {
    let mut u = valid_input();
    u.grain = Some("url".into());
    u.artifact = Some("https://example.invalid/doc".into());
    u.hash = None;
    u.hash_stability = None;
    u.content = Some("<p>hello</p>\r\n".into());
    let a = u.validate(None).unwrap();
    assert_eq!(
        a.hash.as_deref(),
        Some(crate::preparation::url_prepared_hash(b"<p>hello</p>\n").as_str())
    );
    assert_eq!(a.hash_stability, AnchorHashStability::Unstable);

    let mut f = valid_input();
    f.hash = None;
    f.content = Some("fn a() {}\n".into());
    assert_eq!(
        f.validate(None).unwrap().hash.as_deref(),
        Some(prepared_content_hash(b"fn a() {}").as_str())
    );

    let mut both = valid_input();
    both.content = Some("x".into());
    assert_eq!(
        both.validate(None).unwrap_err(),
        AnchorValidationError::ContentAndHash
    );

    let mut ent = valid_input();
    ent.grain = Some("entity".into());
    ent.artifact = Some("m--e".into());
    ent.hash = None;
    ent.content = Some("x".into());
    let err = ent.validate(None).unwrap_err();
    assert_eq!(
        err,
        AnchorValidationError::ContentNotAcceptedForGrain { grain: "entity" }
    );
    assert_eq!(err.detail()["field"], "content");

    let mut tree = valid_input();
    tree.grain = Some("tree".into());
    tree.hash = None;
    tree.content = Some("x".into());
    assert!(matches!(
        tree.validate(None).unwrap_err(),
        AnchorValidationError::ContentNotAcceptedForGrain { grain: "tree" }
    ));

    let mut informed = valid_input();
    informed.class = Some("informed-by".into());
    informed.hash = None;
    informed.content = Some("x".into());
    assert!(matches!(
        informed.validate(None).unwrap_err(),
        AnchorValidationError::HashOnNonHashClass { .. }
    ));
}

#[test]
fn validate_refuses_unknown_class() {
    let mut i = valid_input();
    i.class = Some("guessed".into());
    let err = i.validate(None).unwrap_err();
    assert_eq!(err.code(), INVALID_ANCHOR_CODE);
    assert!(matches!(err, AnchorValidationError::UnknownClass { .. }));
    assert_eq!(err.detail()["field"], serde_json::json!("class"));
}

#[test]
fn validate_refuses_unknown_grain() {
    let mut i = valid_input();
    i.grain = Some("paragraph".into());
    let err = i.validate(None).unwrap_err();
    assert!(matches!(err, AnchorValidationError::UnknownGrain { .. }));
}

#[test]
fn validate_refuses_missing_artifact() {
    let mut i = valid_input();
    i.artifact = Some("   ".into());
    let err = i.validate(None).unwrap_err();
    assert!(matches!(err, AnchorValidationError::MissingArtifact));
    i.artifact = None;
    assert!(matches!(
        valid_input_with_artifact(None).validate(None).unwrap_err(),
        AnchorValidationError::MissingArtifact
    ));
    let _ = i;
}

fn valid_input_with_artifact(a: Option<String>) -> AnchorInput {
    AnchorInput {
        artifact: a,
        ..valid_input()
    }
}

#[test]
fn validate_refuses_hash_on_non_hash_class() {
    let mut i = valid_input();
    i.class = Some("authored".into());
    // hash still supplied → refuse
    let err = i.validate(None).unwrap_err();
    assert!(matches!(
        err,
        AnchorValidationError::HashOnNonHashClass { class: "authored" }
    ));
}

#[test]
fn validate_accepts_non_hash_class_without_hash() {
    let mut i = valid_input();
    i.class = Some("informed-by".into());
    i.hash = None;
    let a = i.validate(None).unwrap();
    assert_eq!(a.class, AnchorProvenanceClass::InformedBy);
    assert!(a.hash.is_none());
}

#[test]
fn validate_refuses_grain_unsupported_by_medium_namespace() {
    // span grain on a web (url namespace) medium.
    let mut i = valid_input();
    i.grain = Some("span".into());
    i.class = Some("authored".into());
    i.hash = None;
    let err = i.validate(Some(("web", "url"))).unwrap_err();
    match err {
        AnchorValidationError::GrainNamespaceUnsupported {
            grain,
            anchor_namespace,
            ..
        } => {
            assert_eq!(grain, "span");
            assert_eq!(anchor_namespace, "url");
        }
        other => panic!("expected GrainNamespaceUnsupported, got {other:?}"),
    }
}

#[test]
fn validate_skips_namespace_check_without_medium_context() {
    // span grain, no medium → namespace rule not applied.
    let mut i = valid_input();
    i.grain = Some("span".into());
    assert!(i.validate(None).is_ok());
}

// -- prepared-content hash ----------------------------------------------

/// The prepared form is stable across meaningless byte noise: BOM,
/// line-ending convention, and final-newline presence never move the
/// hash — a real content change always does.
#[test]
fn prepared_hash_is_stable_across_byte_noise() {
    let base = prepared_content_hash(b"fn a() {}\nfn b() {}\n");
    // CRLF and lone-CR line endings normalize away.
    assert_eq!(prepared_content_hash(b"fn a() {}\r\nfn b() {}\r\n"), base);
    assert_eq!(prepared_content_hash(b"fn a() {}\rfn b() {}\r"), base);
    // Final-newline presence (missing, single, several) is noise.
    assert_eq!(prepared_content_hash(b"fn a() {}\nfn b() {}"), base);
    assert_eq!(prepared_content_hash(b"fn a() {}\nfn b() {}\n\n\n"), base);
    // A leading UTF-8 BOM is stripped.
    assert_eq!(
        prepared_content_hash("\u{feff}fn a() {}\nfn b() {}\n".as_bytes()),
        base
    );
    // A real content change moves the hash.
    assert_ne!(prepared_content_hash(b"fn a() {}\nfn c() {}\n"), base);
    // House hash shape: 16 lowercase hex chars.
    assert_eq!(base.len(), 16);
    assert!(
        base.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
}

/// Interior whitespace is content, not noise: a trailing space inside a
/// line (markdown hard break) changes the hash.
#[test]
fn prepared_hash_preserves_interior_whitespace() {
    assert_ne!(
        prepared_content_hash(b"line one  \nline two\n"),
        prepared_content_hash(b"line one\nline two\n")
    );
}

/// Non-UTF-8 bytes hash raw — no text canonicalization is applied, and
/// any byte change moves the hash.
#[test]
fn prepared_hash_hashes_binary_bytes_raw() {
    let bin_a = [0xff_u8, 0xfe, 0x00, 0x0d, 0x0a];
    let bin_b = [0xff_u8, 0xfe, 0x00, 0x0a];
    assert_ne!(prepared_content_hash(&bin_a), prepared_content_hash(&bin_b));
    // Deterministic.
    assert_eq!(prepared_content_hash(&bin_a), prepared_content_hash(&bin_a));
}

// -- resolution --------------------------------------------------------

fn anchor(class: AnchorProvenanceClass, hash: Option<&str>, stab: AnchorHashStability) -> Anchor {
    Anchor {
        artifact: "src/lib.rs".into(),
        grain: AnchorGrain::File,
        class,
        at_version: None,
        hash: hash.map(str::to_string),
        hash_stability: stab,
        derived_from: Vec::new(),
        binding: None,
        source: None,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    }
}

#[test]
fn resolves_when_hash_matches() {
    let a = anchor(
        AnchorProvenanceClass::Anchored,
        Some("h1"),
        AnchorHashStability::Stable,
    );
    let obs = ArtifactObservation::Present {
        current_hash: Some("h1".into()),
    };
    assert_eq!(resolve_anchor(&a, &obs), AnchorState::Resolves);
}

#[test]
fn stable_hash_break_drifts_unstable_rechecks() {
    let stable = anchor(
        AnchorProvenanceClass::Anchored,
        Some("h1"),
        AnchorHashStability::Stable,
    );
    let unstable = anchor(
        AnchorProvenanceClass::Anchored,
        Some("h1"),
        AnchorHashStability::Unstable,
    );
    let obs = ArtifactObservation::Present {
        current_hash: Some("h2".into()),
    };
    assert_eq!(resolve_anchor(&stable, &obs), AnchorState::Drifted);
    assert_eq!(resolve_anchor(&unstable, &obs), AnchorState::Recheck);
}

#[test]
fn absent_artifact_is_orphaned() {
    let a = anchor(
        AnchorProvenanceClass::Anchored,
        Some("h1"),
        AnchorHashStability::Stable,
    );
    assert_eq!(
        resolve_anchor(&a, &ArtifactObservation::Absent),
        AnchorState::Orphaned
    );
}

#[test]
fn non_hash_classes_never_drift() {
    for class in [
        AnchorProvenanceClass::Authored,
        AnchorProvenanceClass::InformedBy,
    ] {
        let a = anchor(class, None, AnchorHashStability::Stable);
        // Content moved underneath — still resolves (excluded from
        // hash-drift adjudication).
        let obs = ArtifactObservation::Present {
            current_hash: Some("whatever".into()),
        };
        assert_eq!(resolve_anchor(&a, &obs), AnchorState::Resolves);
        // But an absent artifact is still orphaned.
        assert_eq!(
            resolve_anchor(&a, &ArtifactObservation::Absent),
            AnchorState::Orphaned
        );
    }
}

#[test]
fn unavailable_hash_rechecks_not_drifts() {
    let a = anchor(
        AnchorProvenanceClass::Anchored,
        Some("h1"),
        AnchorHashStability::Stable,
    );
    let obs = ArtifactObservation::Present { current_hash: None };
    assert_eq!(resolve_anchor(&a, &obs), AnchorState::Recheck);
}

// -- composition -------------------------------------------------------

#[test]
fn composition_counts_classes_grains_and_tree_fanout() {
    let anchors = vec![
        Anchor {
            artifact: "a.rs".into(),
            grain: AnchorGrain::File,
            class: AnchorProvenanceClass::Anchored,
            at_version: None,
            hash: Some("h".into()),
            hash_stability: AnchorHashStability::Stable,
            derived_from: Vec::new(),
            binding: None,
            source: None,
            span_unvalidated: false,
            hash_source: None,
            last_observed: None,
        },
        Anchor {
            artifact: "src/".into(),
            grain: AnchorGrain::Tree,
            class: AnchorProvenanceClass::Derived,
            at_version: None,
            hash: Some("t".into()),
            hash_stability: AnchorHashStability::Stable,
            derived_from: vec!["a.rs".into(), "b.rs".into()],
            binding: None,
            source: None,
            span_unvalidated: false,
            hash_source: None,
            last_observed: None,
        },
    ];
    let comp = compose_entity_anchors(&anchors);
    assert_eq!(comp.by_class["anchored"], 1);
    assert_eq!(comp.by_class["derived"], 1);
    assert_eq!(comp.by_grain["file"], 1);
    assert_eq!(comp.by_grain["tree"], 1);
    // Tree fan-out is a distinct axis — one row, never per-file credit.
    assert_eq!(comp.tree_grain_artifacts, vec!["src/".to_string()]);
    assert_eq!(
        comp.derived_inputs,
        vec![vec!["a.rs".to_string(), "b.rs".to_string()]]
    );
}

// -- sidecar round-trip -------------------------------------------------

#[test]
fn sidecar_round_trips_and_prunes_empty() {
    let mut sc = AnchorSidecar::default();
    assert!(sc.is_empty());
    let a = anchor(
        AnchorProvenanceClass::Anchored,
        Some("h1"),
        AnchorHashStability::Stable,
    );
    sc.set("specs--x", vec![a.clone()]);
    assert_eq!(sc.get("specs--x").len(), 1);

    let bytes = sc.to_bytes();
    let round = AnchorSidecar::from_bytes(&bytes).unwrap();
    assert_eq!(round, sc);

    // Setting empty prunes the key.
    sc.set("specs--x", vec![]);
    assert!(sc.is_empty());
    assert!(sc.get("specs--x").is_empty());
}

// -- merge / unset arithmetic ------------------------------------------

fn file_anchor(artifact: &str, hash: &str) -> Anchor {
    Anchor {
        artifact: artifact.into(),
        grain: AnchorGrain::File,
        class: AnchorProvenanceClass::Anchored,
        at_version: None,
        hash: Some(hash.into()),
        hash_stability: AnchorHashStability::Stable,
        derived_from: Vec::new(),
        binding: None,
        source: None,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    }
}

/// Merge appends a new triple and leaves the existing set untouched —
/// the incremental-anchoring contract (N existing + 1 new ⇒ N+1).
#[test]
fn merge_appends_new_triple_without_touching_others() {
    let mut sc = AnchorSidecar::default();
    sc.set(
        "m--e",
        vec![file_anchor("a.rs", "h-a"), file_anchor("b.rs", "h-b")],
    );
    sc.merge("m--e", &[], vec![file_anchor("c.rs", "h-c")], false);
    let row = sc.get("m--e");
    assert_eq!(row.len(), 3);
    assert_eq!(row[0], file_anchor("a.rs", "h-a"));
    assert_eq!(row[1], file_anchor("b.rs", "h-b"));
    assert_eq!(row[2], file_anchor("c.rs", "h-c"));
}

/// An incoming anchor with an existing `(artifact, grain, class)`
/// triple replaces exactly that one, in place; others stay
/// byte-identical.
#[test]
fn merge_replaces_same_triple_in_place() {
    let mut sc = AnchorSidecar::default();
    sc.set(
        "m--e",
        vec![file_anchor("a.rs", "h-old"), file_anchor("b.rs", "h-b")],
    );
    sc.merge("m--e", &[], vec![file_anchor("a.rs", "h-new")], false);
    let row = sc.get("m--e");
    assert_eq!(row.len(), 2);
    assert_eq!(row[0], file_anchor("a.rs", "h-new"));
    assert_eq!(row[1], file_anchor("b.rs", "h-b"));
}

/// Same artifact under a different grain or class is a different
/// identity — it appends rather than replaces (the triple is the merge
/// key, not the artifact alone).
#[test]
fn merge_treats_grain_and_class_as_identity() {
    let mut sc = AnchorSidecar::default();
    sc.set("m--e", vec![file_anchor("a.rs", "h-a")]);
    let mut span = file_anchor("a.rs", "h-span");
    span.grain = AnchorGrain::Span;
    let mut informed = file_anchor("a.rs", "h-a");
    informed.class = AnchorProvenanceClass::InformedBy;
    informed.hash = None;
    sc.merge("m--e", &[], vec![span, informed], false);
    assert_eq!(sc.get("m--e").len(), 3);
}

/// Re-sending an entity's full current set is a no-op on the stored
/// bytes, and merging an empty list changes nothing.
#[test]
fn merge_full_resend_and_empty_are_noops() {
    let mut sc = AnchorSidecar::default();
    sc.set(
        "m--e",
        vec![file_anchor("a.rs", "h-a"), file_anchor("b.rs", "h-b")],
    );
    let before = sc.to_bytes();
    sc.merge(
        "m--e",
        &[],
        vec![file_anchor("a.rs", "h-a"), file_anchor("b.rs", "h-b")],
        false,
    );
    assert_eq!(sc.to_bytes(), before, "full re-send is byte-stable");
    sc.merge("m--e", &[], Vec::new(), false);
    assert_eq!(sc.to_bytes(), before, "empty merge is a no-op");
}

/// A bare-artifact unset removes all of that artifact's anchors and
/// nothing else; a grain/class-narrowed unset removes only the match;
/// a selector matching nothing is a no-op.
#[test]
fn unset_selects_by_artifact_with_optional_narrowing() {
    let mut span = file_anchor("a.rs", "h-span");
    span.grain = AnchorGrain::Span;
    let mut sc = AnchorSidecar::default();
    sc.set(
        "m--e",
        vec![
            file_anchor("a.rs", "h-a"),
            span.clone(),
            file_anchor("b.rs", "h-b"),
        ],
    );

    // Narrowed: only the span-grain anchor on a.rs goes.
    let narrowed = AnchorUnset {
        artifact: "a.rs".into(),
        grain: Some(AnchorGrain::Span),
        class: None,
    };
    sc.merge("m--e", &[narrowed], Vec::new(), false);
    assert_eq!(
        sc.get("m--e"),
        &[file_anchor("a.rs", "h-a"), file_anchor("b.rs", "h-b")]
    );

    // Nonexistent target: idempotent no-op.
    let missing = AnchorUnset {
        artifact: "never-there.rs".into(),
        grain: None,
        class: None,
    };
    sc.merge("m--e", &[missing], Vec::new(), false);
    assert_eq!(sc.get("m--e").len(), 2);

    // Bare artifact: everything on a.rs goes, b.rs untouched.
    let bare = AnchorUnset {
        artifact: "a.rs".into(),
        grain: None,
        class: None,
    };
    sc.merge("m--e", &[bare], Vec::new(), false);
    assert_eq!(sc.get("m--e"), &[file_anchor("b.rs", "h-b")]);
}

/// Unset applies before merge in the same call: unsetting an artifact
/// and writing a new anchor on it lands the new anchor (full-replace
/// stays expressible in one call).
#[test]
fn unset_applies_before_merge() {
    let mut span = file_anchor("a.rs", "h-span");
    span.grain = AnchorGrain::Span;
    let mut sc = AnchorSidecar::default();
    sc.set("m--e", vec![file_anchor("a.rs", "h-old"), span]);
    let bare = AnchorUnset {
        artifact: "a.rs".into(),
        grain: None,
        class: None,
    };
    sc.merge("m--e", &[bare], vec![file_anchor("a.rs", "h-new")], false);
    assert_eq!(sc.get("m--e"), &[file_anchor("a.rs", "h-new")]);
}

/// A row emptied by unsets prunes its key — the sidecar never keeps
/// empty rows.
#[test]
fn merge_prunes_row_emptied_by_unset() {
    let mut sc = AnchorSidecar::default();
    sc.set("m--e", vec![file_anchor("a.rs", "h-a")]);
    let bare = AnchorUnset {
        artifact: "a.rs".into(),
        grain: None,
        class: None,
    };
    sc.merge("m--e", &[bare], Vec::new(), false);
    assert!(sc.is_empty());
    assert!(!sc.to_bytes().windows(5).any(|w| w == b"m--e\""));
}

/// The unset validator: artifact required; grain/class, when supplied,
/// must be known wire strings; absent narrowing means "any".
#[test]
fn unset_input_validates_typed() {
    let ok = AnchorUnsetInput {
        artifact: Some("  a.rs  ".into()),
        grain: Some("span".into()),
        class: None,
    }
    .validate()
    .unwrap();
    assert_eq!(ok.artifact, "a.rs");
    assert_eq!(ok.grain, Some(AnchorGrain::Span));
    assert_eq!(ok.class, None);

    let missing = AnchorUnsetInput::default().validate().unwrap_err();
    assert!(matches!(missing, AnchorValidationError::MissingArtifact));
    assert_eq!(missing.code(), INVALID_ANCHOR_CODE);

    let bad_grain = AnchorUnsetInput {
        artifact: Some("a.rs".into()),
        grain: Some("paragraph".into()),
        class: None,
    }
    .validate()
    .unwrap_err();
    assert!(matches!(
        bad_grain,
        AnchorValidationError::UnknownGrain { .. }
    ));

    let bad_class = AnchorUnsetInput {
        artifact: Some("a.rs".into()),
        grain: None,
        class: Some("guessed".into()),
    }
    .validate()
    .unwrap_err();
    assert!(matches!(
        bad_class,
        AnchorValidationError::UnknownClass { .. }
    ));
}

#[test]
fn sidecar_rename_leaves_zero_rows_under_old_id() {
    let mut sc = AnchorSidecar::default();
    sc.set(
        "specs--old",
        vec![anchor(
            AnchorProvenanceClass::Anchored,
            Some("h"),
            AnchorHashStability::Stable,
        )],
    );
    sc.rename("specs--old", "specs--new");
    assert!(sc.get("specs--old").is_empty());
    assert_eq!(sc.get("specs--new").len(), 1);
}

#[test]
fn sidecar_remove_drops_entity_anchors() {
    let mut sc = AnchorSidecar::default();
    sc.set(
        "specs--gone",
        vec![anchor(
            AnchorProvenanceClass::Anchored,
            Some("h"),
            AnchorHashStability::Stable,
        )],
    );
    sc.remove("specs--gone");
    assert!(sc.get("specs--gone").is_empty());
    // Idempotent.
    sc.remove("specs--gone");
}

#[test]
fn empty_bytes_parse_as_empty_sidecar() {
    assert!(AnchorSidecar::from_bytes(b"").unwrap().is_empty());
    assert!(AnchorSidecar::from_bytes(b"  \n ").unwrap().is_empty());
}

#[test]
fn anchor_json_shape_omits_empty_optionals() {
    let a = anchor(
        AnchorProvenanceClass::Anchored,
        Some("h1"),
        AnchorHashStability::Stable,
    );
    let v = serde_json::to_value(&a).unwrap();
    assert_eq!(v["artifact"], "src/lib.rs");
    assert_eq!(v["grain"], "file");
    assert_eq!(v["class"], "anchored");
    assert_eq!(v["hash"], "h1");
    assert_eq!(v["hash_stability"], "stable");
    // Absent optionals are skipped, not null.
    assert!(v.get("at_version").is_none());
    assert!(v.get("derived_from").is_none());
    assert!(v.get("binding").is_none());
}

#[test]
fn anchor_version_serialises_tagged() {
    let a = Anchor {
        at_version: Some(AnchorVersion::Commit("deadbeef".into())),
        ..anchor(
            AnchorProvenanceClass::Anchored,
            Some("h"),
            AnchorHashStability::Stable,
        )
    };
    let v = serde_json::to_value(&a).unwrap();
    assert_eq!(v["at_version"]["kind"], "commit");
    assert_eq!(v["at_version"]["value"], "deadbeef");
}

/// `source` rides validation: a non-empty name is carried, absent
/// stays absent, and present-but-empty refuses `INVALID_ANCHOR`
/// with `field: source` in the recovery detail.
#[test]
fn validate_source_carried_absent_or_refused_when_empty() {
    let mut input = AnchorInput {
        artifact: Some("src/lib.rs".into()),
        grain: Some("file".into()),
        class: Some("anchored".into()),
        ..Default::default()
    };
    assert_eq!(
        input.validate(None).unwrap().source,
        None,
        "absent stays absent"
    );

    input.source = Some("  api-docs  ".into());
    assert_eq!(
        input.validate(None).unwrap().source.as_deref(),
        Some("api-docs"),
        "non-empty name is carried (trimmed)"
    );

    input.source = Some("   ".into());
    let err = input.validate(None).unwrap_err();
    assert_eq!(err.code(), INVALID_ANCHOR_CODE);
    assert!(matches!(err, AnchorValidationError::EmptySource));
    assert_eq!(
        err.detail().get("field"),
        Some(&serde_json::json!("source"))
    );
}

/// A sidecar written before the `source` field existed loads
/// unchanged (additive, optional — no migration, no version bump),
/// and a sourced anchor round-trips through serde.
#[test]
fn source_is_additive_on_the_persisted_shape() {
    let pre_plan = r#"{
            "artifact": "src/lib.rs",
            "grain": "file",
            "class": "anchored",
            "hash_stability": "stable"
        }"#;
    let a: Anchor = serde_json::from_str(pre_plan).expect("pre-plan anchor loads");
    assert_eq!(a.source, None, "no backfill, no default");

    let sourced = Anchor {
        source: Some("api-docs".into()),
        ..a
    };
    let json = serde_json::to_string(&sourced).unwrap();
    let back: Anchor = serde_json::from_str(&json).unwrap();
    assert_eq!(back.source.as_deref(), Some("api-docs"));
}

// --- sidecar version 2, supplied observations, the url namespace rule ---

#[test]
fn sidecar_v1_loads_and_upgrades_in_memory_v3_refuses() {
    let v1 = br#"{"version":1,"entities":{"m--e":[{"artifact":"https://x.test/a","grain":"url","class":"informed-by","hash_stability":"unstable"}]}}"#;
    let sc = AnchorSidecar::from_bytes(v1).expect("version 1 loads");
    assert_eq!(sc.version, ANCHOR_SIDECAR_VERSION, "upgraded in memory");
    assert_eq!(sc.get("m--e").len(), 1);
    assert!(sc.get("m--e")[0].last_observed.is_none(), "rows unchanged");
    let rewritten = String::from_utf8(sc.to_bytes()).unwrap();
    assert!(rewritten.contains("\"version\": 2"), "{rewritten}");

    let v3 = br#"{"version":3,"entities":{}}"#;
    let err = AnchorSidecar::from_bytes(v3).expect_err("unknown higher version refuses");
    assert!(
        err.to_string()
            .contains("unsupported anchors sidecar version 3"),
        "{err}"
    );
}

#[test]
fn last_observed_round_trips_and_is_absent_when_none() {
    let mut a = valid_input().validate(None).unwrap();
    let json = serde_json::to_value(&a).unwrap();
    assert!(json.get("last_observed").is_none());
    a.last_observed = Some(AnchorObservation {
        at: "2026-09-01T10:00:00Z".into(),
        hash: Some("abc".into()),
        state: AnchorState::Resolves,
    });
    let json = serde_json::to_value(&a).unwrap();
    assert_eq!(json["last_observed"]["state"], "resolves");
    let back: Anchor = serde_json::from_value(json).unwrap();
    assert_eq!(back, a);
}

#[test]
fn url_grain_is_admitted_beside_a_path_medium_and_path_grains_refuse_a_url_artifact() {
    let mut i = valid_input();
    i.grain = Some("url".into());
    i.artifact = Some("https://example.org/doc.pdf".into());
    i.class = Some("anchored".into());
    i.hash = None;
    i.content = Some("the document text".into());
    i.hash_stability = None;
    let a = i
        .validate(Some(("filesystem", "path")))
        .expect("url beside a path medium is legal");
    assert_eq!(a.grain, AnchorGrain::Url);
    assert_eq!(a.hash_source, Some(AnchorHashSource::Author));
    assert_eq!(
        a.hash_stability,
        AnchorHashStability::Unstable,
        "url default"
    );

    for grain in ["span", "file", "tree"] {
        let mut i = valid_input();
        i.grain = Some(grain.into());
        i.artifact = Some("https://example.org/doc.pdf#L1-L3".into());
        i.class = Some("informed-by".into());
        i.hash = None;
        let err = i.validate(Some(("filesystem", "path"))).unwrap_err();
        assert!(
            matches!(&err, AnchorValidationError::PathGrainOnUrlArtifact { grain: g, .. } if *g == grain),
            "{grain}: {err:?}"
        );
        assert_eq!(err.code(), INVALID_ANCHOR_CODE);
        assert!(err.to_string().contains("never enters a path namespace"));
    }
    assert!(looks_like_url("https://a.b/c"));
    assert!(looks_like_url("file://x"));
    assert!(!looks_like_url("src/main.rs"));
    assert!(!looks_like_url("://nope"));
    assert!(!looks_like_url("http://"));
}

#[test]
fn supplied_observations_validate_all_or_nothing() {
    let now = "2026-09-02T12:00:00Z";
    let rows = vec![
        SuppliedObservationInput {
            artifact: Some("https://a.test/1".into()),
            hash: Some("h1".into()),
            ..Default::default()
        },
        SuppliedObservationInput {
            artifact: Some("https://a.test/2".into()),
            content: Some("body\r\n".into()),
            observed_at: Some("2026-08-01".into()),
            ..Default::default()
        },
        SuppliedObservationInput {
            artifact: Some("https://a.test/3".into()),
            absent: Some(true),
            ..Default::default()
        },
    ];
    let ok = validate_supplied_observations(&rows, now).unwrap();
    assert_eq!(ok.len(), 3);
    assert_eq!(ok["https://a.test/1"].at, now);
    assert_eq!(
        ok["https://a.test/2"].outcome,
        SuppliedOutcome::Present {
            hash: prepared_content_hash(b"body\r\n"),
            content: Some("body\r\n".into()),
        },
        "content hashes under the write path's canonicalization and is kept for re-preparation"
    );
    assert_eq!(ok["https://a.test/2"].at, "2026-08-01");
    assert_eq!(ok["https://a.test/3"].outcome, SuppliedOutcome::Absent);

    // hash + content on one row: ambiguous, refused by row number.
    let bad = vec![SuppliedObservationInput {
        artifact: Some("https://a.test/1".into()),
        hash: Some("h".into()),
        content: Some("c".into()),
        ..Default::default()
    }];
    let err = validate_supplied_observations(&bad, now).unwrap_err();
    assert!(matches!(
        err,
        ObservationValidationError::OutcomeAmbiguous { row: 1, .. }
    ));
    assert_eq!(err.code(), INVALID_OBSERVATION_CODE);
    // nothing at all
    let bad = vec![SuppliedObservationInput {
        artifact: Some("https://a.test/1".into()),
        ..Default::default()
    }];
    assert!(matches!(
        validate_supplied_observations(&bad, now).unwrap_err(),
        ObservationValidationError::OutcomeAmbiguous { .. }
    ));
    let bad = vec![SuppliedObservationInput {
        artifact: Some("https://a.test/1".into()),
        hash: Some("h".into()),
        observed_at: Some("yesterday".into()),
        ..Default::default()
    }];
    assert!(matches!(
        validate_supplied_observations(&bad, now).unwrap_err(),
        ObservationValidationError::BadTimestamp { .. }
    ));
    let dup = vec![rows[0].clone(), rows[0].clone()];
    assert!(matches!(
        validate_supplied_observations(&dup, now).unwrap_err(),
        ObservationValidationError::DuplicateArtifact {
            first: 1,
            second: 2,
            ..
        }
    ));
    assert!(matches!(
        validate_supplied_observations(&[SuppliedObservationInput::default()], now).unwrap_err(),
        ObservationValidationError::MissingArtifact { row: 1 }
    ));
}

#[test]
fn days_between_ages_by_civil_date() {
    assert_eq!(days_between("2026-08-01", "2026-09-02T00:00:00Z"), Some(32));
    assert_eq!(
        days_between("2026-09-02T23:59:59Z", "2026-09-02T00:00:00Z"),
        Some(0)
    );
    assert_eq!(days_between("2026-09-03", "2026-09-02"), Some(0), "floored");
    assert_eq!(days_between("garbage", "2026-09-02"), None);
    assert_eq!(iso_days_since_epoch("1970-01-01"), Some(0));
    assert_eq!(iso_days_since_epoch("2000-03-01"), Some(11017));
}
