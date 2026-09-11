#![cfg(test)]

use super::*;

#[test]
fn empty_list_renders_empty_string() {
    let items: Vec<String> = Vec::new();
    assert_eq!(format_inline_list_overflow(&items, "x"), "");
}

#[test]
fn list_at_cap_renders_all_no_overflow_suffix() {
    let items = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    assert_eq!(format_inline_list_overflow(&items, "x"), "a, b, c");
}

#[test]
fn list_under_cap_renders_all_no_overflow_suffix() {
    let items = vec!["a".to_string(), "b".to_string()];
    assert_eq!(format_inline_list_overflow(&items, "x"), "a, b");
}

#[test]
fn list_over_cap_appends_count_and_field_name() {
    let items: Vec<String> = (0..23).map(|i| format!("id{i}")).collect();
    let rendered = format_inline_list_overflow(&items, "referrers");
    assert_eq!(rendered, "id0, id1, id2 +20 more — see details.referrers");
}

#[test]
fn list_six_items_truncates_to_three_plus_three() {
    let items: Vec<String> = (0..6).map(|i| format!("t{i}")).collect();
    let rendered = format_inline_list_overflow(&items, "missing");
    assert_eq!(rendered, "t0, t1, t2 +3 more — see details.missing");
}

#[test]
fn has_incoming_refs_display_inlines_first_three_referrer_ids() {
    let referrers: Vec<ReferrerInfo> = (0..23)
        .map(|i| ReferrerInfo {
            from_id: format!("specs--ref{i}"),
            rel_types: vec!["USES".to_string()],
            mem: "specs".to_string(),
        })
        .collect();
    let err = EngineError::HasIncomingRefs {
        id: "specs--hub".to_string(),
        referrers,
    };
    let s = err.to_string();
    // First three ids appear inline; the rest are summarised plus a
    // pointer to `details.referrers` on the structured channel.
    assert!(
        s.contains("specs--ref0, specs--ref1, specs--ref2"),
        "got: {s}"
    );
    assert!(s.contains("+20 more — see details.referrers"), "got: {s}");
    // Pre-fix the message only carried the count; check the count
    // still appears so callers parsing it for "N references" keep
    // working.
    assert!(s.contains("23 incoming reference"), "got: {s}");
}

#[test]
fn wiki_link_without_relation_display_lists_all_when_under_cap() {
    let missing = vec![
        MissingWikiLink {
            section_key: "specifies".to_string(),
            target_id: "specs--a".to_string(),
        },
        MissingWikiLink {
            section_key: "specifies".to_string(),
            target_id: "specs--b".to_string(),
        },
        MissingWikiLink {
            section_key: "rationale".to_string(),
            target_id: "specs--c".to_string(),
        },
    ];
    let err = EngineError::WikiLinkWithoutRelation {
        from_id: "specs--src".to_string(),
        missing,
    };
    let s = err.to_string();
    assert!(s.contains("specifies→specs--a"), "got: {s}");
    assert!(s.contains("specifies→specs--b"), "got: {s}");
    assert!(s.contains("rationale→specs--c"), "got: {s}");
    assert!(!s.contains("more — see details"), "got: {s}");
}

#[test]
fn wiki_link_without_relation_display_truncates_at_cap_with_pointer() {
    let missing: Vec<MissingWikiLink> = (0..6)
        .map(|i| MissingWikiLink {
            section_key: format!("s{i}"),
            target_id: format!("specs--t{i}"),
        })
        .collect();
    let err = EngineError::WikiLinkWithoutRelation {
        from_id: "specs--src".to_string(),
        missing,
    };
    let s = err.to_string();
    assert!(
        s.contains("s0→specs--t0, s1→specs--t1, s2→specs--t2"),
        "got: {s}"
    );
    assert!(s.contains("+3 more — see details.missing"), "got: {s}");
}

#[test]
fn relation_has_body_links_display_inlines_section_keys() {
    let err = EngineError::RelationHasBodyLinks {
        from_id: "specs--src".to_string(),
        to_id: "specs--dst".to_string(),
        rel_type: "USES".to_string(),
        body_links: vec!["specifies".to_string(), "rationale".to_string()],
    };
    let s = err.to_string();
    assert!(s.contains("specifies, rationale"), "got: {s}");
    assert!(!s.contains("more — see details"), "got: {s}");
}

// --- prose_render -----------------------------------------------
// The text
// channel inlines full recovery payloads (no `+N more — see
// details.X` pointer). Display stays terse for logs; prose_render
// is the rich method MCP / CLI surfaces call for `content[0].text`.

#[test]
fn prose_render_has_incoming_refs_inlines_every_referrer() {
    let referrers = (0..7)
        .map(|i| ReferrerInfo {
            from_id: format!("specs--r{i}"),
            rel_types: vec!["DEPENDS_ON".to_string()],
            mem: "specs".to_string(),
        })
        .collect();
    let err = EngineError::HasIncomingRefs {
        id: "specs--target".to_string(),
        referrers,
    };
    let prose = err.prose_render();
    for i in 0..7 {
        assert!(
            prose.contains(&format!("specs--r{i}")),
            "every referrer must appear inline; missing r{i} in: {prose}"
        );
    }
    assert!(!prose.contains("see details"), "got: {prose}");
    // Display stays terse with the overflow suffix.
    let display = err.to_string();
    assert!(
        display.contains("+4 more — see details.referrers"),
        "got: {display}"
    );
}

#[test]
fn prose_render_required_field_unset_inlines_field_description_and_rules() {
    // Update-path semantic: `on_create: false` → "cannot unset".
    let err = EngineError::RequiredFieldUnset {
        field: "verified_on".to_string(),
        entity_type: "requirement".to_string(),
        field_description: Some("ISO-8601 date the requirement was last validated".to_string()),
        enum_values: vec![],
        type_write_rules: vec!["bump verified_on on every status change".to_string()],
        on_create: false,
        missing: Vec::new(),
    };
    let prose = err.prose_render();
    assert!(
        prose.contains("ISO-8601 date"),
        "field_description missing: {prose}"
    );
    assert!(
        prose.contains("bump verified_on"),
        "type_write_rules missing: {prose}"
    );
    assert!(!prose.contains("see details"), "got: {prose}");
    assert!(
        prose.contains("cannot unset"),
        "update-path wording must say 'cannot unset': {prose}"
    );
}

/// Create
/// path renders "not provided" instead of "cannot unset" — the
/// pre-fix wording was misleading on a path where nothing was
/// ever set in the first place.
#[test]
fn prose_render_required_field_unset_create_path_uses_not_provided_wording() {
    let err = EngineError::RequiredFieldUnset {
        field: "verified_on".to_string(),
        entity_type: "requirement".to_string(),
        field_description: Some("ISO-8601 date the requirement was last validated".to_string()),
        enum_values: vec![],
        type_write_rules: vec![],
        on_create: true,
        missing: Vec::new(),
    };
    let prose = err.prose_render();
    assert!(
        prose.contains("not provided"),
        "create-path wording must say 'not provided': {prose}"
    );
    assert!(
        !prose.contains("cannot unset"),
        "create-path wording must NOT say 'cannot unset': {prose}"
    );
    // Same Display dispatch — `to_string()` mirrors `prose_render`'s
    // create-path lead.
    let display = err.to_string();
    assert!(
        display.contains("not provided"),
        "Display must match: {display}"
    );
    assert!(
        !display.contains("cannot unset"),
        "Display must match: {display}"
    );
}

/// The
/// create-path multi-field accumulator surfaces every required-
/// no-default field unset in `details.missing[]`. Each entry
/// carries `{field, description, enum_values, write_rules}` so
/// the agent fixes the whole set in one retry. The singular
/// `details.field` echoes `missing[0].field` for back-compat.
#[test]
fn details_required_field_unset_multi_field_envelope_shape() {
    use crate::runtime_validator::MissingRequiredField;
    let err = EngineError::RequiredFieldUnset {
        field: "decided_on".to_string(),
        entity_type: "decision".to_string(),
        field_description: Some("Date the decision was accepted. ISO YYYY-MM-DD.".to_string()),
        enum_values: vec![],
        type_write_rules: vec!["status transitions: proposed → accepted".to_string()],
        on_create: true,
        missing: vec![
            MissingRequiredField {
                entity_type: "decision".to_string(),
                key: "decided_on".to_string(),
                description: "Date the decision was accepted. ISO YYYY-MM-DD.".to_string(),
                enum_values: vec![],
            },
            MissingRequiredField {
                entity_type: "decision".to_string(),
                key: "deciders".to_string(),
                description: "Who made the call. Comma-separated handles.".to_string(),
                enum_values: vec![],
            },
        ],
    };
    let details = err.details();
    // Back-compat: singular `field` echoes the first-missing entry.
    assert_eq!(details["field"].as_str(), Some("decided_on"));
    // Multi-field accumulator surfaces every entry in
    // declaration order.
    let missing = details["missing"].as_array().expect("missing[] array");
    assert_eq!(missing.len(), 2);
    assert_eq!(missing[0]["field"].as_str(), Some("decided_on"));
    assert_eq!(missing[1]["field"].as_str(), Some("deciders"));
    // First entry's `field` agrees with the singular shape.
    assert_eq!(details["field"], missing[0]["field"]);
    // Per-entry `write_rules` echoes the type-level rules for
    // self-containment.
    assert_eq!(missing[0]["write_rules"], details["type_write_rules"]);
    // Prose mentions both field names so the agent reading the
    // text channel sees the whole set without crossing into the
    // structured channel.
    let prose = err.prose_render();
    assert!(prose.contains("decided_on"), "got: {prose}");
    assert!(prose.contains("deciders"), "got: {prose}");
}

/// The unset path's singular shape is
/// preserved — `missing[]` is empty (the user targeted one field
/// by definition); the singular fields above are authoritative.
/// The typed code stays `REQUIRED_FIELD_UNSET`.
#[test]
fn details_required_field_unset_singular_shape_for_unset_path() {
    let err = EngineError::RequiredFieldUnset {
        field: "decided_on".to_string(),
        entity_type: "decision".to_string(),
        field_description: Some("…".to_string()),
        enum_values: vec![],
        type_write_rules: vec![],
        on_create: false,
        missing: Vec::new(),
    };
    let details = err.details();
    assert_eq!(details["field"].as_str(), Some("decided_on"));
    let missing = details["missing"]
        .as_array()
        .expect("missing[] array present");
    assert!(missing.is_empty(), "unset-path missing[] must be empty");
    assert_eq!(err.code(), "REQUIRED_FIELD_UNSET");
}

#[test]
fn prose_render_missing_required_section_enumerates_each_section_with_write_rules() {
    use crate::runtime_validator::MissingRequiredSection;
    let sections = vec![
        MissingRequiredSection {
            entity_type: "spec".to_string(),
            key: "purpose".to_string(),
            heading: "Purpose".to_string(),
            write_rules: vec!["one-sentence statement of intent".to_string()],
        },
        MissingRequiredSection {
            entity_type: "spec".to_string(),
            key: "scope".to_string(),
            heading: "Scope".to_string(),
            write_rules: vec!["what is in and out of scope".to_string()],
        },
    ];
    let mut type_guidance: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    type_guidance.insert(
        "spec".to_string(),
        vec!["specs are immutable once stable".to_string()],
    );
    let err = EngineError::MissingRequiredSection {
        entity_type: "spec".to_string(),
        missing_count: 2,
        sections,
        type_guidance,
        pre_announced_missing_fields: Vec::new(),
    };
    let prose = err.prose_render();
    assert!(prose.contains("purpose"), "got: {prose}");
    assert!(prose.contains("scope"), "got: {prose}");
    assert!(
        prose.contains("one-sentence statement of intent"),
        "got: {prose}"
    );
    assert!(
        prose.contains("specs are immutable once stable"),
        "got: {prose}"
    );
    assert!(!prose.contains("see details"), "got: {prose}");
}

#[test]
fn prose_render_relationship_cycle_inlines_existing_path() {
    use crate::entity::EntityId;
    let path = vec![
        EntityId::canonical("specs--a"),
        EntityId::canonical("specs--b"),
        EntityId::canonical("specs--c"),
        EntityId::canonical("specs--a"),
    ];
    let err = EngineError::RelationshipCycle {
        rel_type: "PART_OF".to_string(),
        from: EntityId::canonical("specs--a"),
        to: EntityId::canonical("specs--c"),
        existing_path: path,
        path_truncated: false,
        acyclic_set: None,
        existing_path_rel_types: None,
    };
    let prose = err.prose_render();
    assert!(
        prose.contains("specs--a → specs--b → specs--c → specs--a"),
        "got: {prose}"
    );
    assert!(!prose.contains("see details"), "got: {prose}");
}

#[test]
fn prose_render_falls_back_to_display_for_trivial_variants() {
    // ReadOnlyMount has no list payload — Display already inlines
    // the recovery context.
    let err = EngineError::ReadOnlyMount("archive-2024".to_string());
    assert_eq!(err.prose_render(), err.to_string());
}

/// A slug collision names the occupying title on both channels —
/// two distinct titles can derive one id, and the id alone does
/// not tell the caller which one is already there.
#[test]
fn already_exists_names_the_occupying_title_on_both_channels() {
    let err = EngineError::AlreadyExists {
        id: "muehle--bösenberg-grundstücks-gmbh-co-kg".to_string(),
        existing_title: "Bösenberg Grundstücks GmbH Co KG".to_string(),
        existing_is_stub: false,
    };
    assert!(
        err.to_string()
            .contains("occupied by 'Bösenberg Grundstücks GmbH Co KG'"),
        "got: {err}"
    );
    let details = err.details();
    assert_eq!(
        details["existing_title"],
        "Bösenberg Grundstücks GmbH Co KG"
    );
    assert_eq!(details["existing_is_stub"], false);
    assert_eq!(details["id"], "muehle--bösenberg-grundstücks-gmbh-co-kg");
}

/// A stub occupant states it is a stub; a titleless stub must not
/// render as an empty or missing title.
#[test]
fn already_exists_stub_occupant_never_renders_an_empty_title() {
    let titled = EngineError::AlreadyExists {
        id: "specs--x".to_string(),
        existing_title: "X".to_string(),
        existing_is_stub: true,
    };
    assert!(
        titled.to_string().contains("a stub titled 'X'"),
        "got: {titled}"
    );

    let untitled = EngineError::AlreadyExists {
        id: "specs--x".to_string(),
        existing_title: String::new(),
        existing_is_stub: true,
    };
    let msg = untitled.to_string();
    assert!(msg.contains("occupied by a stub"), "got: {msg}");
    assert!(!msg.contains("''"), "empty title must not render: {msg}");
}
