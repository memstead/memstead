//! Shared MCP error-envelope translators.
//!
//! Both the mem-repo (`server.rs`) and filesystem-mem
//! (`filesystem_server.rs`) handlers map engine errors onto the
//! `{code, message, details}` wire envelope agents branch on. The
//! per-`ValidationError` translation is bit-identical across the two
//! surfaces — same variant set, same payload field names, same codes —
//! so factoring it into one helper keeps the wire shape from drifting.

use rmcp::model::{CallToolResult, Content};

use memstead_base::runtime_validator::ValidationError;

/// Map a runtime [`ValidationError`] to the MCP wire envelope. The
/// returned [`CallToolResult`] carries the typed envelope on
/// `structured_content` (so agents branching on `code` get the typed
/// shape) and the same code mirrored inline on the text channel as
/// `ERROR [<CODE>]: <message>` — bit-identical to
/// `server::tool_error_with_payload` so consumers that only read
/// `result.content[0].text` recover the UPPER_SNAKE_CASE code with one
/// regex match (`^ERROR \[([A-Z_]+)\]: `). Pre-fix the text channel
/// emitted a JSON-stringified copy of the payload, which programmatic
/// consumers could decode but humans and prose-renderers couldn't.
///
/// Codes emitted: `UNKNOWN_SECTION`, `UNKNOWN_METADATA_FIELD`,
/// `INVALID_ENUM_VALUE`, `READ_ONLY_FIELD`, `SECTION_NOT_UPDATABLE`,
/// `INVALID_REL_TYPE`, `INVALID_REL_SHAPE`. Each carries the recovery
/// payload pro-flavour callers already branch on (declared list,
/// suggestion, allowed enum values, etc.). The code-string set lives on
/// [`ValidationError::code()`]; this helper composes the wire envelope
/// around it (text-mirror summary, inline-list overflow for the message)
/// so no second list of code-strings exists on the MCP side.
pub fn validation_envelope(err: ValidationError) -> CallToolResult {
    let code = err.code();
    let details = err.details();
    // The text channel uses `ValidationError::prose_render()`'s rich
    // prose so agents reading only `result.content[0].text` see the full
    // declared / allowed list inline rather than a
    // `+N more — see details.X` pointer. The structured channel
    // (`details`) is unchanged.
    let message = err.prose_render();
    let payload = serde_json::json!({
        "code": code,
        "message": message,
        "details": details,
    });
    let text = format!("ERROR [{code}]: {message}");
    let mut result = CallToolResult::error(vec![Content::text(text)]);
    result.structured_content = Some(payload);
    result
}

// `validation_error_message` retired. The text-channel rendering lives
// in `ValidationError::prose_render()` so the inline-list rendering sits
// next to the variant rather than duplicated in `memstead-mcp` and the
// CLI surface. An earlier helper inlined only the first three items with
// a `+N more — see details.X` pointer; `prose_render` inlines the full
// list so an agent reading only the text channel recovers without
// consulting `structured_content`.

#[cfg(test)]
mod inline_list_tests {
    use super::*;
    use memstead_base::runtime_validator::RelationshipHint;

    fn extract_text(r: &CallToolResult) -> String {
        r.content
            .iter()
            .filter_map(|c| c.as_text())
            .map(|t| t.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The text channel inlines every declared section, regardless of
    /// list length, so an agent reading `result.content[0].text`
    /// recovers the full list in one round trip rather than consulting
    /// the structured channel.
    #[test]
    fn unknown_section_envelope_inlines_every_declared_key() {
        let declared = (0..6).map(|i| format!("sec{i}")).collect::<Vec<_>>();
        let r = validation_envelope(ValidationError::UnknownSection {
            key: "typo".to_string(),
            entity_type: "spec".to_string(),
            declared: declared.clone(),
            suggestion: Some("sec0".to_string()),
        });
        let text = extract_text(&r);
        assert!(text.starts_with("ERROR [UNKNOWN_SECTION]: "), "got: {text}");
        for d in &declared {
            assert!(text.contains(d.as_str()), "every declared key must appear inline; missing {d} in: {text}");
        }
        // Suggestion clause is inlined too.
        assert!(text.contains("Did you mean 'sec0'?"), "got: {text}");
        // No prior "see details" pointer survives anywhere in the text.
        assert!(!text.contains("see details"), "text channel must not point at the structured channel; got: {text}");
        // Structured channel still ships the full list verbatim.
        let sc = r.structured_content.expect("payload");
        assert_eq!(sc["details"]["declared"].as_array().unwrap().len(), declared.len());
    }

    #[test]
    fn unknown_section_envelope_lists_all_when_under_cap() {
        let declared = vec!["sec0".to_string(), "sec1".to_string()];
        let r = validation_envelope(ValidationError::UnknownSection {
            key: "typo".to_string(),
            entity_type: "spec".to_string(),
            declared,
            suggestion: None,
        });
        let text = extract_text(&r);
        assert!(text.contains("declared sections: sec0, sec1"), "got: {text}");
        assert!(!text.contains("see details"), "got: {text}");
    }

    #[test]
    fn invalid_enum_value_envelope_inlines_allowed_values() {
        let allowed = (0..10).map(|i| format!("v{i}")).collect::<Vec<_>>();
        let r = validation_envelope(ValidationError::InvalidEnumValue {
            field: "status".to_string(),
            value: "bogus".to_string(),
            allowed: allowed.clone(),
            field_description: None,
            suggestion: None,
            type_write_rules: vec![],
            entity_type: "decision".to_string(),
        });
        let text = extract_text(&r);
        for a in &allowed {
            assert!(text.contains(a.as_str()), "every allowed value must appear inline; missing {a} in: {text}");
        }
        assert!(!text.contains("see details"), "got: {text}");
    }

    #[test]
    fn invalid_rel_type_envelope_inlines_rel_type_names() {
        let allowed: Vec<RelationshipHint> = (0..5)
            .map(|i| RelationshipHint {
                name: format!("REL{i}"),
                when_to_use: None,
            })
            .collect();
        let r = validation_envelope(ValidationError::InvalidRelationshipType {
            input: "BOGUS".to_string(),
            allowed,
            suggestion: None,
        });
        let text = extract_text(&r);
        for n in ["REL0", "REL1", "REL2", "REL3", "REL4"] {
            assert!(text.contains(n), "every rel-type name must appear inline; missing {n} in: {text}");
        }
        assert!(!text.contains("see details"), "got: {text}");
    }
}
