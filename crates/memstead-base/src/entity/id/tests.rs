#![cfg(test)]

use super::*;

/// Mem-name grammar accepts hierarchical paths and refuses
/// malformations. Flat (single-segment) names continue to work —
/// the regex's `(/<segment>)*` tail matches zero or more times.
#[test]
fn validate_mem_name_grammar_accepts_hierarchical_paths() {
    // Flat layouts (regression).
    assert!(validate_mem_name_grammar("specs").is_ok());
    assert!(validate_mem_name_grammar("my-mem").is_ok());
    assert!(validate_mem_name_grammar("v1").is_ok());
    // Hierarchical layouts.
    assert!(validate_mem_name_grammar("team/sub-mem").is_ok());
    assert!(validate_mem_name_grammar("a/b/c/d").is_ok());
    assert!(validate_mem_name_grammar("planning/2026-q1").is_ok());
}

/// Grammar refusals are explicit. Each malformation
/// case (`/team`, `team/`, `team//sub`,
/// uppercase / underscore / dot) returns an `Err`.
#[test]
fn validate_mem_name_grammar_refuses_malformations() {
    // Leading slash.
    assert!(validate_mem_name_grammar("/team/sub").is_err());
    // Trailing slash.
    assert!(validate_mem_name_grammar("team/sub/").is_err());
    // Double slash.
    assert!(validate_mem_name_grammar("team//sub").is_err());
    // Empty.
    assert!(validate_mem_name_grammar("").is_err());
    // Uppercase.
    assert!(validate_mem_name_grammar("Team/Sub").is_err());
    // Underscore (not in allowed alphabet).
    assert!(validate_mem_name_grammar("team_sub").is_err());
    assert!(validate_mem_name_grammar("team/sub_mem").is_err());
    // Dot.
    assert!(validate_mem_name_grammar("team.sub").is_err());
    // Space.
    assert!(validate_mem_name_grammar("team sub").is_err());
}

#[test]
fn title_to_slug_basic() {
    assert_eq!(title_to_slug("My Entity").unwrap(), "my-entity");
    assert_eq!(title_to_slug("My  Entity  Name").unwrap(), "my-entity-name");
}

/// F1 (B+A) behaviour change: precomposed Latin diacritics are
/// preserved in the slug rather than transliterated to ASCII.
/// `Große Änderung` was `grosse-aenderung` pre-F1; it is now
/// `große-änderung`. Same applies to `naïve`, `Café résumé`,
/// `Łódź`, etc. — slug matches title in every script the
/// Unicode `is_alphanumeric` predicate accepts.
#[test]
fn title_to_slug_german() {
    assert_eq!(title_to_slug("Große Änderung").unwrap(), "große-änderung");
    assert_eq!(title_to_slug("Björn").unwrap(), "björn");
}

#[test]
fn title_to_slug_diacritics() {
    assert_eq!(title_to_slug("Café résumé").unwrap(), "café-résumé");
    assert_eq!(title_to_slug("naïve").unwrap(), "naïve");
}

#[test]
fn title_to_slug_special_chars() {
    assert_eq!(title_to_slug("Hello, World!").unwrap(), "hello-world");
    assert_eq!(
        title_to_slug("--leading--trailing--").unwrap(),
        "leading-trailing"
    );
}

#[test]
fn title_to_slug_polish() {
    assert_eq!(title_to_slug("Łódź").unwrap(), "łódź");
}

/// F1 (B+A): CJK titles round-trip cleanly. No transliteration,
/// no hash — the slug equals the title.
#[test]
fn title_to_slug_cjk() {
    assert_eq!(
        title_to_slug("日本語のタイトル").unwrap(),
        "日本語のタイトル"
    );
    // Spaces still collapse to hyphens.
    assert_eq!(title_to_slug("中文 標題").unwrap(), "中文-標題");
    // Mixed CJK + Latin + digits.
    assert_eq!(title_to_slug("Project 日本 v2").unwrap(), "project-日本-v2");
}

/// F1 (B+A): cased non-Latin scripts (Cyrillic, Greek, Armenian)
/// case-fold to lowercase the same way Latin does.
#[test]
fn title_to_slug_cyrillic() {
    assert_eq!(title_to_slug("Москва").unwrap(), "москва");
    assert_eq!(title_to_slug("Москва-проект").unwrap(), "москва-проект");
    assert_eq!(title_to_slug("ПРОЕКТ ПЛАН").unwrap(), "проект-план");
}

/// F1 (B+A): Right-to-left scripts. Hebrew and Arabic letters
/// are `\p{Lo}` (case-less); they pass through unchanged.
/// Hebrew niqqud and Arabic harakat are `\p{Mn}` (nonspacing
/// marks) carrying the Unicode `Other_Alphabetic` property, so
/// Rust's `is_alphanumeric` treats them as alphabetic and the
/// slug filter keeps them — wiki-link round-trip is exact for
/// titles that include vowelization. (The wiki-link regex
/// accepts the wider `\p{Mn}`/`\p{Mc}` class for the same
/// reason; see `slug_path_regex` in `validator/strict.rs`.)
#[test]
fn title_to_slug_rtl() {
    // Hebrew with niqqud — niqqud is preserved; spaces become hyphens.
    assert_eq!(title_to_slug("תַּפְקִיד עברי").unwrap(), "תַּפְקִיד-עברי");
    // Arabic with harakat — harakat preserved (same Other_Alphabetic property).
    assert_eq!(title_to_slug("مَرْحَبًا").unwrap(), "مَرْحَبًا");
    // Plain Hebrew without vowelization (the more common case)
    // round-trips letter-for-letter.
    assert_eq!(title_to_slug("שלום עולם").unwrap(), "שלום-עולם");
}

/// F1 (option A backstop): titles whose pipeline yields an
/// empty slug fall through to a deterministic short-hash id
/// rather than failing. Covers all-emoji, all-symbol,
/// all-punctuation, and empty/whitespace inputs.
#[test]
fn title_to_slug_residual_falls_back_to_hash() {
    // All emoji.
    let emoji = title_to_slug("🚀✨").unwrap();
    assert!(emoji.starts_with("entity-"), "got {emoji}");
    assert_eq!(emoji.len(), "entity-".len() + 8);
    // Same input always produces same hash (deterministic).
    assert_eq!(emoji, title_to_slug("🚀✨").unwrap());
    // Different inputs produce different hashes.
    assert_ne!(emoji, title_to_slug("🌟").unwrap());

    // Empty / whitespace / punctuation-only all hit the same path.
    assert!(title_to_slug("").unwrap().starts_with("entity-"));
    assert!(title_to_slug("   ").unwrap().starts_with("entity-"));
    assert!(title_to_slug("\t\n").unwrap().starts_with("entity-"));
    assert!(title_to_slug("---").unwrap().starts_with("entity-"));
    assert!(title_to_slug("!!!").unwrap().starts_with("entity-"));
    assert!(title_to_slug("!?.,;").unwrap().starts_with("entity-"));
}

/// NFC normalization is load-bearing for cross-platform safety:
/// a `Café` written NFD (`Cafe` + combining-acute U+0301) and
/// one written NFC (single codepoint U+00E9) must produce the
/// same slug. Pre-F1 the pipeline NFD-decomposed and stripped
/// combining marks, yielding `cafe` for both — that path is
/// gone, so the NFC normalization step is what holds the
/// invariant now.
#[test]
fn title_to_slug_nfc_normalization() {
    let nfc = "Café"; // single-codepoint é
    let nfd = "Cafe\u{0301}"; // e + combining acute
    assert_ne!(nfc, nfd, "NFC and NFD forms must differ at the byte level");
    assert_eq!(
        title_to_slug(nfc).unwrap(),
        title_to_slug(nfd).unwrap(),
        "NFC and NFD inputs must produce the same slug",
    );
}

/// F4: the strict mutation-entry gate rejects empty titles with
/// `TitleEmpty` so the wire envelope can carry `reason: "empty"`
/// rather than silently producing a hash-fallback slug.
#[test]
fn validate_and_derive_slug_rejects_empty() {
    // `"\t\n"` is no longer here: it contains control characters, so
    // the more specific control-char guard fires first (see
    // `validate_and_derive_slug_rejects_control_chars`). These cases
    // hold no control chars and collapse to an empty slug.
    for empty in ["", "   ", "---", " - - - ", "-"] {
        let err = validate_and_derive_slug(empty).unwrap_err();
        let SlugError::TitleEmpty { input } = err else {
            panic!("expected TitleEmpty for {empty:?}, got {err:?}");
        };
        assert_eq!(input, empty);
    }
}

/// F10 + F19: any character the permissive pipeline would drop
/// (emoji, punctuation, math/currency symbols, path separators)
/// is admitted — the title is display text — with the dropped
/// characters reported and the slug derived exactly as the
/// permissive pipeline would (the old refusal's `proposed_slug`
/// is now simply the slug).
#[test]
fn validate_and_derive_slug_admits_and_reports_dropped_chars() {
    let cases: &[(&str, &[char], &str)] = &[
        ("Hello, World!", &[',', '!'], "hello-world"),
        ("Café — résumé", &['—'], "café-résumé"),
        ("🚀 launch", &['🚀'], "launch"),
        ("price € 100", &['€'], "price-100"),
        ("../escape", &['.', '/'], "escape"),
        ("path/to/entity", &['/'], "pathtoentity"),
        ("a\\b", &['\\'], "ab"),
        ("Wohnung 2.OG rechts", &['.'], "wohnung-2og-rechts"),
        (
            "Anlage 4a – Leistungsbeschreibung",
            &['–'],
            "anlage-4a-leistungsbeschreibung",
        ),
        (
            "Bösenberg Grundstücks GmbH & Co. KG",
            &['&', '.'],
            "bösenberg-grundstücks-gmbh-co-kg",
        ),
    ];
    for (title, expected_dropped, expected_slug) in cases {
        let got = validate_and_derive_slug(title)
            .unwrap_or_else(|e| panic!("expected ok for {title:?}, got {e:?}"));
        assert_eq!(got.slug, *expected_slug, "title={title:?}");
        assert_eq!(got.dropped_chars, *expected_dropped, "title={title:?}");
        // Byte-identical to the permissive pipeline, always.
        assert_eq!(got.slug, title_to_slug(title).unwrap(), "title={title:?}");
    }
}

/// F8: control characters (newline, tab,
/// carriage return, other C0 controls) are refused with
/// `TitleHasControlChars` rather than silently folded to hyphens —
/// they would otherwise split the stored `# H1` and truncate every
/// read of the title. The proposed slug is the single-line form.
#[test]
fn validate_and_derive_slug_rejects_control_chars() {
    let cases: &[(&str, &[char], &str)] = &[
        (
            "Tab\tand\nnewline title",
            &['\t', '\n'],
            "tab-and-newline-title",
        ),
        ("line\rreturn", &['\r'], "line-return"),
        ("null\u{0}byte", &['\u{0}'], "nullbyte"),
    ];
    for (title, expected_control, expected_proposed) in cases {
        let err = validate_and_derive_slug(title).unwrap_err();
        let SlugError::TitleHasControlChars {
            input,
            control_chars,
            proposed_slug,
        } = err
        else {
            panic!("expected TitleHasControlChars for {title:?}, got {err:?}");
        };
        assert_eq!(input, *title);
        assert_eq!(control_chars, *expected_control, "title={title:?}");
        assert_eq!(proposed_slug, *expected_proposed, "title={title:?}");
    }
}

/// A plain space is whitespace but NOT a control character, so it
/// must keep folding to a hyphen (the control-char guard does not
/// narrow ordinary whitespace handling).
#[test]
fn validate_and_derive_slug_space_is_not_control() {
    assert_eq!(validate_and_derive_slug("a b c").unwrap().slug, "a-b-c");
}

/// Success path — titles whose every character survives the
/// pipeline round-trip cleanly produce the same slug as
/// `title_to_slug` would.
#[test]
fn validate_and_derive_slug_success() {
    let cases: &[(&str, &str)] = &[
        ("My Entity", "my-entity"),
        ("Große Änderung", "große-änderung"),
        ("日本語のタイトル", "日本語のタイトル"),
        ("--leading--trailing--", "leading-trailing"),
        ("Project 日本 v2", "project-日本-v2"),
    ];
    for (title, expected) in cases {
        let got = validate_and_derive_slug(title)
            .unwrap_or_else(|e| panic!("expected ok for {title:?}, got {e:?}"));
        assert_eq!(&got.slug, expected, "title={title:?}");
        // A title the old grammar admitted drops nothing.
        assert!(got.dropped_chars.is_empty(), "title={title:?}");
        // Must agree with the permissive pipeline for accepted titles.
        assert_eq!(got.slug, title_to_slug(title).unwrap(), "title={title:?}");
    }
}

/// The strict gate runs the same NFC normalization as the
/// permissive pipeline, so NFC and NFD spellings of the same
/// title produce the same slug (or both reject).
#[test]
fn validate_and_derive_slug_nfc_normalization() {
    let nfc = "Café";
    let nfd = "Cafe\u{0301}";
    assert_eq!(
        validate_and_derive_slug(nfc).unwrap().slug,
        validate_and_derive_slug(nfd).unwrap().slug,
    );
}

/// SlugError::reason() returns the stable discriminator each
/// surface uses on the `details.reason` field.
#[test]
fn slug_error_reason_discriminator() {
    let e = SlugError::TitleEmpty {
        input: "".to_string(),
    };
    assert_eq!(e.reason(), "empty");
    let e = SlugError::IdTooLong {
        input: "specs--x".to_string(),
        length: 201,
        max: 200,
    };
    assert_eq!(e.reason(), "id_too_long");
    let e = SlugError::TitleHasControlChars {
        input: "a\nb".to_string(),
        control_chars: vec!['\n'],
        proposed_slug: "a-b".to_string(),
    };
    assert_eq!(e.reason(), "control_chars");
}

/// F2 + F4: a title that derives a slug whose full
/// `mem--slug` id sits at the 200-char ceiling is accepted;
/// one byte over is rejected with a recovery-friendly error.
/// `build_id` is the canonical write-side entry, so both
/// behaviours land here.
#[test]
fn build_id_enforces_length_cap() {
    let mem = "specs";
    // mem.len()=5, "--"=2 → 7-char prefix. Slug of 193 chars
    // produces a 200-char id; 194 chars trips the cap.
    let just_fits = "a".repeat(ENTITY_ID_MAX_LEN - mem.len() - 2);
    let ok = build_id(mem, &just_fits).expect("at-cap id must pass");
    assert_eq!(ok.as_ref().chars().count(), ENTITY_ID_MAX_LEN);

    let one_over = "a".repeat(ENTITY_ID_MAX_LEN - mem.len() - 2 + 1);
    let err = build_id(mem, &one_over).unwrap_err();
    let SlugError::IdTooLong { input, length, max } = err else {
        panic!("expected IdTooLong, got {err:?}");
    };
    // `input` echoes the composed id, not the title, so it agrees
    // with `length`.
    assert_eq!(input, format!("{mem}--{one_over}"));
    assert_eq!(input.chars().count(), length);
    assert_eq!(length, ENTITY_ID_MAX_LEN + 1);
    assert_eq!(max, ENTITY_ID_MAX_LEN);
}

#[test]
fn build_id_basic() {
    assert_eq!(
        build_id("specs", "My Entity").unwrap().0,
        "specs--my-entity"
    );
}

/// F1 (B+A): non-Latin titles round-trip through `build_id`.
#[test]
fn build_id_non_latin() {
    assert_eq!(
        build_id("specs", "日本語のタイトル").unwrap().0,
        "specs--日本語のタイトル",
    );
    assert_eq!(
        build_id("specs", "Москва-проект").unwrap().0,
        "specs--москва-проект",
    );
}

#[test]
fn file_path_to_id_basic() {
    assert_eq!(
        file_path_to_id("architecture/result-entity.md", "specs").0,
        "specs--architecture/result-entity"
    );
    assert_eq!(
        file_path_to_id("result-entity.md", "specs").0,
        "specs--result-entity"
    );
}

#[test]
fn wiki_link_to_id_basic() {
    assert_eq!(
        wiki_link_to_id("result-entity", "specs").unwrap().0,
        "specs--result-entity"
    );
    assert_eq!(
        wiki_link_to_id("parent/child/entity", "specs").unwrap().0,
        "specs--parent/child/entity"
    );
}

#[test]
fn wiki_link_to_id_strips_alias() {
    assert_eq!(
        wiki_link_to_id("target|Display Name", "specs").unwrap().0,
        "specs--target"
    );
}

#[test]
fn wiki_link_to_id_strips_prefix_and_suffix() {
    assert_eq!(
        wiki_link_to_id("../parent/entity.md", "specs").unwrap().0,
        "specs--parent/entity"
    );
}

/// Agents writing the canonical fully-qualified id `[[mem--slug]]`
/// must not be doubly-prefixed into `mem--mem--slug`.
#[test]
fn wiki_link_to_id_strips_redundant_self_prefix() {
    assert_eq!(
        wiki_link_to_id("specs--result-entity", "specs").unwrap().0,
        "specs--result-entity"
    );
    assert_eq!(
        wiki_link_to_id("test-mem-mini--engine", "test-mem-mini")
            .unwrap()
            .0,
        "test-mem-mini--engine"
    );
    assert_eq!(
        wiki_link_to_id("specs--target.md|Display", "specs")
            .unwrap()
            .0,
        "specs--target"
    );
    assert_eq!(
        wiki_link_to_id("specs--parent/child", "specs").unwrap().0,
        "specs--parent/child"
    );
    // Self-prefix stripping is one-shot, not iterative — a second
    // embedded `<current_mem>--` is preserved so cross-mem-style
    // drift stays visible.
    assert_eq!(
        wiki_link_to_id("specs--specs--slug", "specs").unwrap().0,
        "specs--specs--slug"
    );
}

/// Cross-mem dash form `[[<mem>--<slug>]]` routes to the named
/// mem rather than silently re-prepending the source mem into a
/// phantom `specs--other--entity` stub.
/// The cross-mem policy gate (alias-synthesis pass) refuses the
/// auto-stub when the workspace policy denies the direction —
/// that gate is exercised in engine-layer tests.
#[test]
fn wiki_link_to_id_tier_zero_cross_mem_dash_form() {
    assert_eq!(
        wiki_link_to_id("other--entity", "specs").unwrap().0,
        "other--entity"
    );
    assert_eq!(
        wiki_link_to_id("nonexistent-mem--target", "specs")
            .unwrap()
            .0,
        "nonexistent-mem--target"
    );
}

/// Tier-0 dash form collapses cleanly when the named mem is the
/// source mem — equivalent to the self-prefix-strip fast path
/// for bare-slug authoring.
#[test]
fn wiki_link_to_id_tier_zero_self_mem_dash_form() {
    assert_eq!(
        wiki_link_to_id("specs--target", "specs").unwrap().0,
        "specs--target"
    );
}

/// Tier-0 only admits single-segment mem names — the
/// hierarchical dash form stays on the colon Tier-2 recovery
/// path. The pre-existing slash-dash ambiguity refusal is what
/// fires here (cross-mem into a hierarchical mem is
/// grammatically ambiguous with a same-mem hierarchical slug).
#[test]
fn wiki_link_to_id_tier_zero_refuses_hierarchical_prefix() {
    let err = wiki_link_to_id("team/sub-mem--target", "specs").unwrap_err();
    match err {
        WikiLinkError::InvalidTarget { suggested, .. } => {
            assert_eq!(suggested.as_deref(), Some("team/sub-mem:target"));
        }
        other => panic!("expected InvalidTarget, got {other:?}"),
    }
}

/// Section anchors strip as a display decoration alongside `|alias`,
/// `../`, and `.md`. Single anchor, multi-anchor, and the
/// combined anchor+alias form all collapse to the underlying
/// slug-form id.
#[test]
fn wiki_link_to_id_strips_section_anchor() {
    assert_eq!(
        wiki_link_to_id("login-service#identity", "specs")
            .unwrap()
            .0,
        "specs--login-service"
    );
    assert_eq!(
        wiki_link_to_id("specs--login-service#identity", "specs")
            .unwrap()
            .0,
        "specs--login-service"
    );
    // Multi-anchor — strip from first `#`.
    assert_eq!(
        wiki_link_to_id("specs--target#a#b", "specs").unwrap().0,
        "specs--target"
    );
    // Combined anchor + alias.
    assert_eq!(
        wiki_link_to_id("specs--target#section|Display", "specs")
            .unwrap()
            .0,
        "specs--target"
    );
}

/// Cross-mem routing and anchor stripping compose: a cross-mem
/// anchored form resolves under tier-0 and strips the anchor.
#[test]
fn wiki_link_to_id_cross_mem_anchored_composes() {
    assert_eq!(
        wiki_link_to_id("other--target#section", "specs").unwrap().0,
        "other--target"
    );
}

/// Empty `current_mem` opts out of self-prefix stripping so a
/// literal leading `--` (which would never legitimately occur, but
/// could collide with `format!("{mem}--", mem="")`) stays intact.
#[test]
fn wiki_link_to_id_empty_mem_does_not_strip() {
    assert_eq!(wiki_link_to_id("--weird", "").unwrap().0, "----weird");
}

#[test]
fn wiki_link_to_id_tier_two_cross_mem() {
    assert_eq!(
        wiki_link_to_id("engine:health", "plugin").unwrap().0,
        "engine--health"
    );
    assert_eq!(
        wiki_link_to_id("engine:architecture/result", "plugin")
            .unwrap()
            .0,
        "engine--architecture/result"
    );
}

#[test]
fn wiki_link_to_id_tier_two_self_prefix_collapses() {
    assert_eq!(
        wiki_link_to_id("specs:foo", "specs").unwrap().0,
        "specs--foo"
    );
    assert_eq!(
        wiki_link_to_id("specs:foo", "specs").unwrap(),
        wiki_link_to_id("foo", "specs").unwrap()
    );
}

#[test]
fn wiki_link_to_id_tier_two_combines_with_alias_and_md() {
    assert_eq!(
        wiki_link_to_id("engine:health.md|See health", "plugin")
            .unwrap()
            .0,
        "engine--health"
    );
}

#[test]
fn wiki_link_to_id_tier_two_accepts_hierarchical_prefix() {
    assert_eq!(
        wiki_link_to_id("external/engine:health", "plugin")
            .unwrap()
            .0,
        "external/engine--health"
    );
}

#[test]
fn wiki_link_to_id_tier_one_strips_hierarchical_self_prefix() {
    assert_eq!(
        wiki_link_to_id("team/sub-mem--auth-service", "team/sub-mem")
            .unwrap()
            .0,
        "team/sub-mem--auth-service"
    );
}

#[test]
fn wiki_link_to_id_tier_one_bare_slug_from_hierarchical_mem() {
    assert_eq!(
        wiki_link_to_id("auth-service", "team/sub-mem").unwrap().0,
        "team/sub-mem--auth-service"
    );
}

/// `::` is reserved syntax — strict refusal. The slug-grammar gate
/// refuses the `:` character outright.
#[test]
fn wiki_link_to_id_double_colon_refuses() {
    let err = wiki_link_to_id("engine::health", "plugin").unwrap_err();
    assert!(
        matches!(err, WikiLinkError::InvalidTarget { .. }),
        "got {err:?}"
    );
}

/// Empty halves around the colon refuse under strict mode — the
/// `:` character isn't in the slug-grammar character class, so
/// the Tier-1 fallback fails.
#[test]
fn wiki_link_to_id_empty_tier_two_halves_refuse() {
    assert!(matches!(
        wiki_link_to_id(":foo", "specs").unwrap_err(),
        WikiLinkError::InvalidTarget { .. }
    ));
    assert!(matches!(
        wiki_link_to_id("engine:", "specs").unwrap_err(),
        WikiLinkError::InvalidTarget { .. }
    ));
}

/// Natural-form (uppercase + whitespace) refuses with
/// `InvalidTarget` and a `title_to_slug`-derived suggestion the
/// agent lifts directly into a retry.
#[test]
fn wiki_link_to_id_natural_form_refuses_with_suggestion() {
    let err = wiki_link_to_id("Knowledge Graph", "specs").unwrap_err();
    let WikiLinkError::InvalidTarget { raw, suggested, .. } = err else {
        panic!("expected InvalidTarget, got {err:?}");
    };
    assert_eq!(raw, "Knowledge Graph");
    assert_eq!(suggested.as_deref(), Some("knowledge-graph"));
}

/// Tier-2 with natural-form slug suggests `mem:slug`
/// preserving the prefix. The agent rewrites only the slug part.
#[test]
fn wiki_link_to_id_tier_two_natural_slug_refuses_with_prefixed_suggestion() {
    let err = wiki_link_to_id("engine:Health Check", "plugin").unwrap_err();
    let WikiLinkError::InvalidTarget { raw, suggested, .. } = err else {
        panic!("expected InvalidTarget, got {err:?}");
    };
    assert_eq!(raw, "engine:Health Check");
    assert_eq!(suggested.as_deref(), Some("engine:health-check"));
}

/// Tier-1 dash form
/// with `/` in the would-be mem prefix is grammatically
/// ambiguous (cross-mem into a hierarchical mem vs same-mem
/// hierarchical slug). Refusal carries the colon-form as
/// `suggested` so the agent's recovery is a one-character edit.
#[test]
fn wiki_link_to_id_hierarchical_dash_form_refuses_with_colon_suggestion() {
    let err = wiki_link_to_id("team/sub-mem--auth-service", "test").unwrap_err();
    let WikiLinkError::InvalidTarget {
        raw,
        suggested,
        reason,
    } = err
    else {
        panic!("expected InvalidTarget, got {err:?}");
    };
    assert_eq!(raw, "team/sub-mem--auth-service");
    assert_eq!(suggested.as_deref(), Some("team/sub-mem:auth-service"));
    // Reason names both disambiguations.
    assert!(
        reason.contains("team/sub-mem:auth-service"),
        "reason must surface the cross-mem colon form: {reason}"
    );
    assert!(
        reason.contains("test:team/sub-mem--auth-service"),
        "reason must surface the same-mem hierarchical form: {reason}"
    );
}

/// For self-prefixed dash form, tier-0 splits on the FIRST `--`, so
/// `[[test--team/sub--target]]` resolves as mem `test`, slug
/// `team/sub--target` — a same-mem entity with a hierarchical
/// slug containing `--`. The ambiguity gate in the slug position
/// does not apply because the mem/slug boundary is
/// pinned by tier-0's grammar.
#[test]
fn wiki_link_to_id_self_prefixed_dash_form_resolves_via_tier_zero() {
    let id = wiki_link_to_id("test--team/sub--target", "test").unwrap();
    assert_eq!(id.mem(), "test");
    assert_eq!(id.path(), "team/sub--target");
}

/// A bare hierarchical slug (no `--`) continues to
/// resolve to a same-mem entity. The refusal is keyed on the
/// simultaneous presence of `/` AND `--`, not on `/` alone.
#[test]
fn wiki_link_to_id_bare_hierarchical_slug_still_resolves() {
    let id = wiki_link_to_id("team/sub-mem", "test").unwrap();
    assert_eq!(id.mem(), "test");
    assert_eq!(id.path(), "team/sub-mem");
}

/// Colon-form for cross-mem hierarchical reference
/// continues to resolve correctly (the canonical disambiguation).
#[test]
fn wiki_link_to_id_hierarchical_colon_form_resolves_cross_mem() {
    let id = wiki_link_to_id("team/sub-mem:auth-service", "test").unwrap();
    assert_eq!(id.mem(), "team/sub-mem");
    assert_eq!(id.path(), "auth-service");
}

/// Flat `[[<other-mem>--<slug>]]` routes to the named mem under
/// tier-0 rather than silently re-prefixing with the source mem into
/// a phantom `test--other--target` stub. The cross-mem policy
/// gate enforces routing legality in the alias-synthesis pass —
/// that gate is exercised in engine-layer tests.
#[test]
fn wiki_link_to_id_flat_foreign_dash_form_routes_via_tier_zero() {
    let id = wiki_link_to_id("other--target", "test").unwrap();
    assert_eq!(id.mem(), "other");
    assert_eq!(id.path(), "target");
}

/// Tier-2 with non-ASCII mem prefix refuses with
/// `InvalidMemName`. Mem names are ASCII-only operator
/// identifiers; the agent cannot auto-slugify them.
#[test]
fn wiki_link_to_id_tier_two_bad_mem_refuses_with_distinct_error() {
    let err = wiki_link_to_id("Other Mem:foo", "plugin").unwrap_err();
    let WikiLinkError::InvalidMemName { raw, .. } = err else {
        panic!("expected InvalidMemName, got {err:?}");
    };
    assert_eq!(raw, "Other Mem");
}

/// Pathological inputs (empty, all punctuation) refuse
/// with `suggested: None`.
#[test]
fn wiki_link_to_id_pathological_input_no_suggestion() {
    let err = wiki_link_to_id("!!!", "specs").unwrap_err();
    let WikiLinkError::InvalidTarget { suggested, .. } = err else {
        panic!("expected InvalidTarget, got {err:?}");
    };
    assert!(suggested.is_none(), "got {suggested:?}");
}

/// Slug-form across every script family the slug
/// pipeline accepts round-trips through the strict gate.
#[test]
fn wiki_link_to_id_accepts_slug_form_across_scripts() {
    let cases: &[(&str, &str)] = &[
        ("knowledge-graph", "v--knowledge-graph"),
        ("الرسم-البياني-للمعرفة", "v--الرسم-البياني-للمعرفة"),
        ("ज्ञान-ग्राफ", "v--ज्ञान-ग्राफ"),
        ("知识图谱", "v--知识图谱"),
        ("知識グラフ", "v--知識グラフ"),
        ("กราฟความรู้", "v--กราฟความรู้"),
        ("ידע-גרף", "v--ידע-גרף"),
    ];
    for (input, expected) in cases {
        let id = wiki_link_to_id(input, "v")
            .unwrap_or_else(|e| panic!("expected ok for {input:?}, got {e:?}"));
        assert_eq!(&id.0, expected, "input={input:?}");
    }
}

/// The lenient decoder preserves pre-strict behaviour
/// for read-side scanners that must tolerate on-disk drift.
/// Round-trip equivalence on the inputs the strict gate accepts.
#[test]
fn wiki_link_to_id_lenient_matches_strict_on_valid_input() {
    let inputs = &["knowledge-graph", "engine:health", "parent/child"];
    for input in inputs {
        let strict = wiki_link_to_id(input, "specs").unwrap();
        let lenient = wiki_link_to_id_lenient(input, "specs");
        assert_eq!(strict, lenient, "input={input:?}");
    }
}

/// The lenient decoder accepts what the strict gate
/// refuses, surfacing the literal drift for read-side reporting.
#[test]
fn wiki_link_to_id_lenient_admits_drift() {
    assert_eq!(
        wiki_link_to_id_lenient("Knowledge Graph", "specs").0,
        "specs--Knowledge Graph"
    );
    assert_eq!(
        wiki_link_to_id_lenient("engine::health", "plugin").0,
        "plugin--engine::health"
    );
}

/// Fuzz finding (long tier, frontmatter target, 2026-08-24; corpus
/// member `crash-ac181b5d…`): the alias/anchor cuts inside the
/// decoration strip run after its whitespace trim, so they exposed
/// trailing whitespace that reached the lenient id, and the
/// generated row re-parsed to a different id on the next round.
/// The lenient decoder now trims what the cuts expose; the strict
/// gate still refuses those shapes (no widening).
#[test]
fn wiki_link_to_id_lenient_trims_whitespace_exposed_by_alias_and_anchor_cuts() {
    assert_eq!(
        wiki_link_to_id_lenient("foo |label", "specs").0,
        "specs--foo"
    );
    assert_eq!(
        wiki_link_to_id_lenient("foo\r\n#anchor", "specs").0,
        "specs--foo"
    );
    // The idempotence shape itself: a second decode of the first
    // decode's output is byte-identical.
    let once = wiki_link_to_id_lenient("parent: x\r\n#tail", "specs");
    let twice = wiki_link_to_id_lenient(&format!("{}:{}", once.mem(), once.path()), "specs");
    assert_eq!(once, twice);
    // Strict is untouched: whitespace exposed by an anchor cut
    // still fails the grammar gate.
    assert!(wiki_link_to_id("foo #anchor", "specs").is_err());
}

#[test]
fn entity_id_parts() {
    let id = EntityId::new("specs", "parent/child");
    assert_eq!(id.mem(), "specs");
    assert_eq!(id.path(), "parent/child");
    assert_eq!(id.name(), "child");
}

#[test]
fn entity_id_no_mem() {
    let id = EntityId("result-entity".to_string());
    assert_eq!(id.mem(), "");
    assert_eq!(id.path(), "result-entity");
    assert_eq!(id.name(), "result-entity");
}

#[test]
fn id_to_file_path_basic() {
    let id = EntityId::new("specs", "architecture/result-entity");
    assert_eq!(id_to_file_path(&id), "architecture/result-entity.md");
}

#[test]
fn validate_rel_type_valid() {
    assert_eq!(validate_rel_type("PART_OF").unwrap(), "PART_OF");
    assert_eq!(validate_rel_type("uses").unwrap(), "USES");
}

#[test]
fn validate_rel_type_invalid() {
    assert!(validate_rel_type("has spaces").is_err());
    assert!(validate_rel_type("").is_err());
}

/// TITLE_GRAMMAR_RULE conformance: the documented sentence and the
/// validator agree — any single-line text is admitted, characters
/// outside the slug alphabet are dropped from the slug and
/// reported, and control characters are the rejection. If this
/// test fails, either the validator's behaviour or the constant
/// changed alone; change them together.
#[test]
fn title_grammar_rule_matches_validator_behaviour() {
    // Admitted with nothing dropped: Unicode alphanumerics,
    // whitespace, hyphen.
    for ok in [
        "Plain Title",
        "hyphen-ated",
        "Große Änderung", // non-ASCII alphanumerics
        "日本語 タイトル",
        "nbsp\u{a0}space", // non-control whitespace folds to hyphen
    ] {
        let got = validate_and_derive_slug(ok)
            .unwrap_or_else(|e| panic!("rule says {ok:?} is accepted, got {e:?}"));
        assert!(got.dropped_chars.is_empty(), "{ok:?} drops nothing");
    }
    // Admitted per the rule, with characters outside the slug
    // alphabet dropped from the slug and reported — the plenum
    // collision list plus representative symbol/punctuation cases.
    for (title, dropped) in [
        ("v1.0", '.'),
        ("a (draft)", '('),
        ("a (draft", '('),
        ("either/or", '/'),
        ("re: title", ':'),
        ("a \u{2014} b", '\u{2014}'), // em dash
        ("hello!", '!'),
    ] {
        let got = validate_and_derive_slug(title)
            .unwrap_or_else(|e| panic!("rule says {title:?} is admitted, got {e:?}"));
        assert!(
            got.dropped_chars.contains(&dropped),
            "{title:?}: the divergence report names {dropped:?}, got {:?}",
            got.dropped_chars
        );
        assert!(
            !got.slug.is_empty(),
            "{title:?}: a slug still derives from the surviving characters"
        );
    }
    // Control-class whitespace (tab, newline) is the rejection,
    // per the rule's parenthetical.
    for title in ["tabs\tinside", "line\nbreak"] {
        assert!(
            matches!(
                validate_and_derive_slug(title),
                Err(SlugError::TitleHasControlChars { .. })
            ),
            "rule says {title:?} is rejected as a control character"
        );
    }
}
