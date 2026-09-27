#![cfg(test)]

use super::*;

/// `ALL_CODES` is what the strict gate and the graph referee filter
/// on, so a condition missing from it is a condition that stops
/// failing a gate — silently, since nothing else would break. The
/// exhaustive match below is the enforcement: add a variant and this
/// stops COMPILING, which is the only moment anyone would otherwise
/// have to remember.
#[test]
fn all_codes_covers_every_variant() {
    let every = [
        DanglingLinkKind::LinkTargetMissing,
        DanglingLinkKind::LinkNotRelated,
        DanglingLinkKind::RelationTargetMissing,
    ];
    for kind in every {
        // Exhaustive by construction: a new variant fails to compile
        // here before it can quietly miss the roster.
        match kind {
            DanglingLinkKind::LinkTargetMissing
            | DanglingLinkKind::LinkNotRelated
            | DanglingLinkKind::RelationTargetMissing => {}
        }
        assert!(
            DanglingLinkKind::ALL_CODES.contains(&kind.code()),
            "{} is emitted but absent from ALL_CODES, so every filter \
                 reading the roster would skip it",
            kind.code()
        );
        assert!(
            !kind.repair().is_empty(),
            "{} has no repair clause",
            kind.code()
        );
    }
    assert_eq!(
        DanglingLinkKind::ALL_CODES.len(),
        every.len(),
        "ALL_CODES carries a code no variant emits"
    );
    // The serialised `kind` IS the code — one spelling per condition.
    for kind in every {
        assert_eq!(
            serde_json::to_value(kind).unwrap(),
            serde_json::Value::String(kind.code().to_string())
        );
    }
}

// Locks the wire shape of `Query` across every combination of
// set/unset fields. Agents compose queries on the fly; a drift here
// silently changes the MCP tool's JSON contract.
#[test]
fn query_json_roundtrip_every_combination() {
    let cases: Vec<Query> = vec![
        Query::default(),
        Query {
            any: vec!["auth".into()],
            ..Default::default()
        },
        Query {
            not: vec!["mock".into()],
            ..Default::default()
        },
        Query {
            phrase: Some("client side agent".into()),
            ..Default::default()
        },
        Query {
            field: Some("identity".into()),
            ..Default::default()
        },
        Query {
            any: vec!["a".into(), "b".into()],
            not: vec!["x".into()],
            phrase: Some("ex act".into()),
            field: Some("purpose".into()),
        },
    ];
    for q in &cases {
        let json = serde_json::to_string(q).expect("serialize");
        let back: Query = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(q.any, back.any, "any field round-trip: {json}");
        assert_eq!(q.not, back.not, "not field round-trip: {json}");
        assert_eq!(q.phrase, back.phrase, "phrase field round-trip: {json}");
        assert_eq!(q.field, back.field, "field field round-trip: {json}");
        assert_eq!(q.is_empty(), back.is_empty());
    }
}

// Empty fields stay out of the wire shape — agents see a lean object.
#[test]
fn query_default_serializes_as_empty_object() {
    let q = Query::default();
    let json = serde_json::to_string(&q).unwrap();
    assert_eq!(json, "{}", "default query must serialize as `{{}}`");
}

// Null / missing keys all round-trip to the same default via serde.
#[test]
fn query_accepts_missing_and_null_fields() {
    let with_missing: Query = serde_json::from_str("{}").unwrap();
    let with_nulls: Query =
        serde_json::from_str(r#"{"any":[],"not":[],"phrase":null,"field":null}"#).unwrap();
    assert!(with_missing.is_empty());
    assert!(with_nulls.is_empty());
}

// Schema is generated via schemars so MCP agents see the full
// structured contract. Cheap smoke test — locks that the four known
// fields appear and nothing regresses to an action-discriminator.
#[test]
fn query_json_schema_exposes_four_fields() {
    let schema = schemars::schema_for!(Query);
    let rendered = serde_json::to_string(&schema).unwrap();
    for field in ["any", "not", "phrase", "field"] {
        assert!(
            rendered.contains(&format!("\"{field}\"")),
            "schema must mention `{field}`: {rendered}"
        );
    }
}

// ------------------------------------------------------------------
// WarningHint wire-envelope snapshots. Each variant locks `code`
// (stable UPPER_SNAKE_CASE), a message substring (phrasing may
// drift — we assert a durable anchor), and the `details` key-set.
// Arrays are asserted shape-only because their content depends on
// the active schema / allowed-include list.
// ------------------------------------------------------------------

fn to_envelope(w: &WarningHint) -> serde_json::Value {
    serde_json::to_value(w).expect("WarningHint serializes")
}

#[test]
fn warning_hint_missing_required_section_envelope() {
    // F9: type-level write_rules moved out of per-warning details
    // to the mutation response's top-level `type_guidance` map.
    // The warning now carries only section-axis fields.
    let w = WarningHint::MissingRequiredSection {
        entity_type: "spec".into(),
        key: "purpose".into(),
        heading: "Purpose".into(),
        write_rules: vec!["one sentence".into(), "state the why".into()],
    };
    let json = to_envelope(&w);
    assert_eq!(json["code"], "MISSING_REQUIRED_SECTION");
    assert!(
        json["message"]
            .as_str()
            .unwrap()
            .contains("required section")
    );
    assert_eq!(json["details"]["entity_type"], "spec");
    assert_eq!(json["details"]["key"], "purpose");
    assert_eq!(json["details"]["heading"], "Purpose");
    assert!(json["details"]["write_rules"].is_array());
    // type_write_rules no longer rides on the per-warning envelope.
    assert!(json["details"].get("type_write_rules").is_none());
}

#[test]
fn warning_hint_undeclared_relationship_open_envelope() {
    let w = WarningHint::UndeclaredRelationshipOpen {
        rel_type: "USES".into(),
        message: "USES admitted in open mode".into(),
    };
    let json = to_envelope(&w);
    assert_eq!(json["code"], "UNDECLARED_RELATIONSHIP_OPEN");
    // Display delegates to the stored message — substring anchor is safe.
    assert!(json["message"].as_str().unwrap().contains("open mode"));
    assert_eq!(json["details"]["rel_type"], "USES");
    // Consistency rule: details must not duplicate the envelope message.
    assert!(json["details"].get("message").is_none());
    // Only rel_type belongs under details for this variant.
    assert_eq!(json["details"].as_object().unwrap().len(), 1);
}

#[test]
fn warning_hint_duplicate_relationship_envelope() {
    let w = WarningHint::DuplicateRelationship {
        rel_type: "USES".into(),
        from: EntityId("specs--a".into()),
        to: EntityId("specs--b".into()),
    };
    let json = to_envelope(&w);
    assert_eq!(json["code"], "DUPLICATE_RELATIONSHIP");
    assert!(json["message"].as_str().unwrap().contains("already exists"));
    assert_eq!(json["details"]["rel_type"], "USES");
    assert_eq!(json["details"]["from"], "specs--a");
    assert_eq!(json["details"]["to"], "specs--b");
}

#[test]
fn warning_hint_no_such_relationship_envelope() {
    let w = WarningHint::NoSuchRelationship {
        rel_type: "USES".into(),
        from: EntityId("specs--a".into()),
        to: EntityId("specs--b".into()),
    };
    let json = to_envelope(&w);
    assert_eq!(json["code"], "NO_SUCH_RELATIONSHIP");
    assert!(json["message"].as_str().unwrap().contains("does not exist"));
    assert_eq!(json["details"]["rel_type"], "USES");
    assert_eq!(json["details"]["from"], "specs--a");
    assert_eq!(json["details"]["to"], "specs--b");
}

#[test]
fn warning_hint_unknown_include_key_envelope() {
    let w = WarningHint::UnknownIncludeKey {
        key: "bogus".into(),
        allowed: vec!["orphans".into(), "stubs".into()],
    };
    let json = to_envelope(&w);
    assert_eq!(json["code"], "UNKNOWN_INCLUDE_KEY");
    assert!(json["message"].as_str().unwrap().contains("bogus"));
    assert_eq!(json["details"]["key"], "bogus");
    assert!(json["details"]["allowed"].is_array());
}

#[test]
fn warning_hint_limit_clamped_envelope() {
    let w = WarningHint::LimitClamped {
        requested: 1000,
        actual: 100,
    };
    let json = to_envelope(&w);
    assert_eq!(json["code"], "LIMIT_CLAMPED");
    assert!(json["message"].as_str().unwrap().contains("clamped"));
    assert_eq!(json["details"]["requested"].as_u64(), Some(1000));
    assert_eq!(json["details"]["actual"].as_u64(), Some(100));
}

#[test]
fn warning_hint_title_normalized_to_slug_noop_envelope() {
    let w = WarningHint::TitleNormalizedToSlugNoop {
        requested_title: "Hello World!".into(),
        current_slug: "hello-world".into(),
    };
    let json = to_envelope(&w);
    assert_eq!(json["code"], "TITLE_NORMALIZED_TO_SLUG_NOOP");
    assert!(
        json["message"]
            .as_str()
            .unwrap()
            .contains("no change written to disk")
    );
    assert_eq!(json["details"]["requested_title"], "Hello World!");
    assert_eq!(json["details"]["current_slug"], "hello-world");
}

// Top-level envelope shape lock — every WarningHint emits exactly
// three keys and nothing else. Protects against accidental field
// additions at the envelope level.
#[test]
fn warning_hint_envelope_has_exactly_three_top_level_keys() {
    for w in &WarningHint::all_samples() {
        let json = to_envelope(w);
        let obj = json.as_object().expect("envelope is an object");
        assert_eq!(
            obj.len(),
            3,
            "{} must emit exactly 3 top-level keys; got {:?}",
            w.code(),
            obj.keys().collect::<Vec<_>>()
        );
        assert!(obj.contains_key("code"));
        assert!(obj.contains_key("message"));
        assert!(obj.contains_key("details"));
    }
}

// Stability lock — `code()` values are a public wire contract. Every
// variant must expose an UPPER_SNAKE_CASE identifier. Catches
// accidental rename / case drift in a single test.
#[test]
fn warning_hint_code_values_are_upper_snake_case() {
    let re = regex::Regex::new(r"^[A-Z][A-Z0-9_]*$").unwrap();
    for w in &WarningHint::all_samples() {
        let code = w.code();
        assert!(
            re.is_match(code),
            "code() violates UPPER_SNAKE_CASE: {code}"
        );
    }
}

// Envelope helper emits the same shape as WarningHint::serialize — one
// constructor, two callers (warnings + MCP error path).
#[test]
fn envelope_shape_is_code_message_details() {
    let v = envelope("FOO_BAR", "hello", serde_json::json!({ "x": 1 }));
    assert_eq!(v["code"], "FOO_BAR");
    assert_eq!(v["message"], "hello");
    assert_eq!(v["details"]["x"], 1);
    assert_eq!(
        v.as_object().unwrap().len(),
        3,
        "envelope has exactly 3 top-level keys"
    );
}

/// An export's link retarget carries an adopted body's origin marks. A
/// hierarchical mem publishes under its leaf, so the export rewrites every
/// self-qualified wiki-link, which moves the bytes a mark hashes. Unfollowed,
/// the archive forgets which of its bodies came from a contributor, and the
/// first deployment that vouches for the mem serves that contributor's
/// sentences as its owner's own.
///
/// The section maps come from the caller, which is what makes the comparison
/// exact: they are the store's schema-shaped map, the one the marks were
/// computed over, not a schema-free re-parse that disagrees on a catch-all.
#[test]
fn an_export_retarget_carries_an_adopted_bodys_marks() {
    use crate::preparation::entity_section_prepared_hash;

    let was = "beta, see [[planning/plan-x:other]]";
    let now = "beta, see [[plan-x:other]]";
    let mark = entity_section_prepared_hash("identity", was);
    // Nested, because the record keys by the whole slug and the export's
    // retarget only fires for a hierarchical mem in the first place.
    let record = format!(
        r#"{{"version":1,"proposals":[{{"id":"f@0","ancestor":"","base":"","target_tip":"","at":"",
           "entities":{{"architecture/beta":{{"disposition":"adopt","landed_sections":{{"identity":"{mark}"}}}}}}}}]}}"#
    );
    let sections =
        |body: &str| indexmap::IndexMap::from([("identity".to_string(), body.to_string())]);

    let carried = crate::ops::export::rekey_proposal_marks(
        Some(record.into_bytes()),
        &[(
            "architecture/beta".to_string(),
            sections(was),
            sections(now),
        )],
    )
    .expect("a record went in");

    let parsed = crate::ops::proposal::ProposalRecord::from_bytes(&carried).expect("parses");
    let marks = parsed.proposals[0].entities["architecture/beta"]
        .landed_sections
        .as_ref()
        .expect("the mark survived");
    assert_eq!(
        marks.get("identity").cloned(),
        Some(entity_section_prepared_hash("identity", now)),
        "the mark hashes the ARCHIVED bytes, so it still holds on the mount"
    );
}

/// A mark the owner had already expired is not resurrected by the retarget,
/// and the guard that decides it is the hash comparison, not an early return.
#[test]
fn an_export_retarget_does_not_resurrect_an_expired_mark() {
    use crate::preparation::entity_section_prepared_hash;

    let was = "the owner's words, see [[planning/plan-x:other]]";
    let now = "the owner's words, see [[plan-x:other]]";
    let stale = entity_section_prepared_hash("identity", "what the proposer wrote");
    let record = format!(
        r#"{{"version":1,"proposals":[{{"id":"f@0","ancestor":"","base":"","target_tip":"","at":"",
           "entities":{{"beta":{{"disposition":"adopt","landed_sections":{{"identity":"{stale}"}}}}}}}}]}}"#
    );
    let sections =
        |body: &str| indexmap::IndexMap::from([("identity".to_string(), body.to_string())]);

    let carried = crate::ops::export::rekey_proposal_marks(
        Some(record.into_bytes()),
        &[("beta".to_string(), sections(was), sections(now))],
    )
    .expect("a record went in");

    let parsed = crate::ops::proposal::ProposalRecord::from_bytes(&carried).expect("parses");
    assert_eq!(
        parsed.proposals[0].entities["beta"]
            .landed_sections
            .as_ref()
            .and_then(|m| m.get("identity"))
            .cloned(),
        Some(stale),
        "an expired mark stays expired: the carry follows bytes it still matched, nothing else"
    );
}

/// An export refuses a record the reader calls defective, the same stance the
/// rename takes: the mem is quarantined on purpose, and an archive claiming a
/// readable record would ship the quarantine away.
#[test]
fn an_export_leaves_a_defective_record_exactly_as_it_is() {
    let record =
        br#"{"version":1,"proposals":[{"id":"f@0","ancestor":"","base":"","target_tip":"","at":"",
         "entities":{"beta":{"disposition":"adopt","landed_sections":{}}}}]}"#;
    let sections =
        |body: &str| indexmap::IndexMap::from([("identity".to_string(), body.to_string())]);
    let out = crate::ops::export::rekey_proposal_marks(
        Some(record.to_vec()),
        &[("beta".to_string(), sections("a"), sections("b"))],
    )
    .expect("a record went in");
    assert_eq!(out, record.to_vec(), "byte-identical, evidence intact");
}
