//! Per-entity strict checks over raw markdown bytes.
//!
//! Complements the tolerant `entity::parser::parse_markdown` — every
//! invariant the tolerant parser papers over (missing title, missing
//! frontmatter, unknown keys, unbalanced brackets) is checked here
//! against the raw bytes before the archive is accepted.

use std::sync::OnceLock;

use memstead_schema::{FieldType, TypeDefinition};
use regex::Regex;

use super::ValidationError;
use crate::entity::id::wiki_link_to_id;
use crate::entity::parser::mask_code_blocks;
use crate::entity::{Entity, MetadataValue};

/// Run every strict check against one entity. `raw_bytes` is the
/// archive's markdown bytes for this entity (pre-canonicalization, so
/// line endings may still be CRLF and a BOM may still be leading).
pub fn validate_strict(
    raw_bytes: &str,
    entity: &Entity,
    schema: &TypeDefinition,
    path: &str,
) -> Result<(), ValidationError> {
    let raw = strip_bom(raw_bytes);

    let (meta_block, body) = split_frontmatter_strict(raw, path)?;
    check_metadata(meta_block, entity, schema, path)?;
    check_title_presence(body, path)?;
    check_sections_present(entity, schema, path)?;
    check_unknown_sections(body, schema, path)?;
    check_relationships_syntax(body, path)?;
    check_relationship_types(entity, path)?;
    check_wiki_links(body, path)?;

    Ok(())
}

fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

/// Verify the frontmatter opens with `---` on the first line and
/// closes with `\n---`. Returns (metadata block text, body text).
/// Body excludes the closing `\n---` line and the newline after it.
fn split_frontmatter_strict<'a>(
    raw: &'a str,
    path: &str,
) -> Result<(&'a str, &'a str), ValidationError> {
    let after_open_len = if raw.starts_with("---\n") {
        4
    } else if raw.starts_with("---\r\n") {
        5
    } else {
        return Err(ValidationError::MissingFrontmatter {
            path: path.to_string(),
        });
    };

    let after_open = &raw[after_open_len..];
    let close_pos = after_open.find("\n---").ok_or_else(|| {
        ValidationError::InvalidFrontmatter {
            path: path.to_string(),
            reason: "frontmatter block is not closed with `\\n---`".to_string(),
        }
    })?;
    let meta_block = &after_open[..close_pos];

    let body_start_rel = close_pos + "\n---".len();
    let body_rest = &after_open[body_start_rel..];
    let body = body_rest
        .strip_prefix("\r\n")
        .or_else(|| body_rest.strip_prefix('\n'))
        .unwrap_or(body_rest);

    Ok((meta_block, body))
}

fn check_metadata(
    meta_block: &str,
    entity: &Entity,
    schema: &TypeDefinition,
    path: &str,
) -> Result<(), ValidationError> {
    // 1. Unknown keys against the raw YAML (the tolerant parser accepts
    //    them but strict ingress rejects anything not declared by the
    //    type. `type:` itself is injected by the schema definition via
    //    `meta_type()`, so it's already in `metadata_fields`.
    let known_keys: Vec<&str> = schema
        .metadata_fields
        .iter()
        .map(|f| f.key.as_str())
        .collect();
    for line in meta_block.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(colon) = trimmed.find(':') else {
            continue;
        };
        let key = trimmed[..colon].trim();
        if !known_keys.contains(&key) {
            return Err(ValidationError::UnknownFrontmatterKey {
                path: path.to_string(),
                key: key.to_string(),
            });
        }
    }

    // 2. Required fields present, 3. types match, 4. enum violations.
    for field in &schema.metadata_fields {
        let is_required = !field.optional;
        let value = entity.metadata.get(field.key.as_str());
        match (is_required, value) {
            (true, None) => {
                return Err(ValidationError::MissingRequiredField {
                    path: path.to_string(),
                    field: field.key.to_string(),
                });
            }
            (_, Some(v)) => {
                if !value_matches_type(v, field.field_type) {
                    return Err(ValidationError::FieldTypeMismatch {
                        path: path.to_string(),
                        field: field.key.to_string(),
                        expected: format!("{:?}", field.field_type),
                    });
                }
                if let Some(ref allowed) = field.enum_values {
                    let got = v.to_frontmatter_string();
                    if !allowed.iter().any(|a| a == &got) {
                        return Err(ValidationError::EnumViolation {
                            path: path.to_string(),
                            field: field.key.to_string(),
                            got,
                        });
                    }
                }
            }
            (false, None) => {}
        }
    }
    Ok(())
}

fn value_matches_type(value: &MetadataValue, expected: FieldType) -> bool {
    match (value, expected) {
        (MetadataValue::Bool(_), FieldType::Boolean) => true,
        (MetadataValue::Integer(_) | MetadataValue::Float(_), FieldType::Number) => true,
        (MetadataValue::String(s), FieldType::Date) => {
            // YYYY-MM-DD or the ISO-8601 datetime form `YYYY-MM-DDTHH:MM:SSZ`.
            // Shared with the CRUD write path so import-ingress and
            // create/update accept exactly the same date values.
            crate::runtime_validator::is_date_shaped(s)
        }
        (MetadataValue::String(_), FieldType::String) => true,
        // CSV array fields have `field_type: String` in the schema but
        // arrive as strings regardless — accept.
        (MetadataValue::String(_), _) => false,
        _ => false,
    }
}

/// Check that `# Title` appears on one of the first three non-empty
/// lines of the body. The tolerant parser falls back to the filename
/// slug when no `# ` heading is found — the point of this check is to
/// make that fallback unreachable at ingress.
fn check_title_presence(body: &str, path: &str) -> Result<(), ValidationError> {
    let mut lines_seen = 0;
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        lines_seen += 1;
        if lines_seen > 3 {
            break;
        }
        if let Some(rest) = line.strip_prefix("# ")
            && !rest.trim().is_empty()
        {
            return Ok(());
        }
    }
    Err(ValidationError::MissingTitle {
        path: path.to_string(),
    })
}

fn check_sections_present(
    entity: &Entity,
    schema: &TypeDefinition,
    path: &str,
) -> Result<(), ValidationError> {
    for section in &schema.sections {
        if !section.required || section.catch_all {
            continue;
        }
        let present = entity
            .sections
            .get(section.key.as_str())
            .is_some_and(|v| !v.trim().is_empty());
        if !present {
            return Err(ValidationError::MissingRequiredSection {
                path: path.to_string(),
                section: section.heading.clone(),
            });
        }
    }
    Ok(())
}

fn check_unknown_sections(
    body: &str,
    schema: &TypeDefinition,
    path: &str,
) -> Result<(), ValidationError> {
    if schema.sections.iter().any(|s| s.catch_all) {
        return Ok(());
    }
    let known_headings: Vec<&str> = schema
        .sections
        .iter()
        .map(|s| s.heading.as_str())
        .chain(std::iter::once("Relationships"))
        .collect();

    let masked = mask_code_blocks(body);
    for line in masked.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            let heading = rest.trim();
            if !known_headings.contains(&heading) {
                return Err(ValidationError::UnknownSection {
                    path: path.to_string(),
                    section: heading.to_string(),
                });
            }
        }
    }
    Ok(())
}

fn check_relationships_syntax(body: &str, path: &str) -> Result<(), ValidationError> {
    let masked = mask_code_blocks(body);
    let mut in_rel = false;
    for line in masked.lines() {
        if line.starts_with("## ") {
            in_rel = line.strip_prefix("## ").map(str::trim) == Some("Relationships");
            continue;
        }
        if !in_rel {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.starts_with('-') {
            continue;
        }
        if !relationship_line_regex().is_match(trimmed) {
            return Err(ValidationError::InvalidRelationshipLine {
                path: path.to_string(),
                line: trimmed.to_string(),
            });
        }
    }
    Ok(())
}

fn relationship_line_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^-\s+\*\*[A-Z_]+\*\*:\s*\[\[[^\]]+\]\](\s*—.*)?$").unwrap()
    })
}

fn check_relationship_types(entity: &Entity, path: &str) -> Result<(), ValidationError> {
    for rel in &entity.relationships {
        if !rel_type_regex().is_match(&rel.rel_type) {
            return Err(ValidationError::InvalidRelationshipType {
                path: path.to_string(),
                rel_type: rel.rel_type.clone(),
            });
        }
    }
    Ok(())
}

fn rel_type_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Z_]+$").unwrap())
}

/// Bracket-balance + slug-regex + reserved-syntax check for every
/// wiki-link in the body. Operates on the code-block-masked body so
/// fenced code sections are ignored. Inline `` `…` `` spans are also
/// stripped so grammar examples like `` `[[<target>]]` `` in prose don't
/// trip the slug-regex check — mirrors what `entity::parser::extract_inline_links`
/// already does on the extraction side.
///
/// Two inline passes, in order: first the CommonMark double-backtick
/// delimiter (``` `` ` `` ``` — used to display a literal backtick, or
/// to wrap content that itself contains a single backtick), then plain
/// single-backtick spans. Running them in that order prevents the
/// single-backtick regex from slicing into the middle of a double-
/// backtick span and leaving stray `` ` `` / `[[` remnants that would
/// then fool the bracket checker.
fn check_wiki_links(body: &str, path: &str) -> Result<(), ValidationError> {
    let fenced_masked = mask_code_blocks(body);
    let after_double = inline_double_backtick_regex()
        .replace_all(&fenced_masked, "")
        .to_string();
    let masked = inline_code_regex()
        .replace_all(&after_double, "")
        .to_string();

    check_bracket_balance(&masked, path)?;

    let link_re = wiki_link_regex();
    for cap in link_re.captures_iter(&masked) {
        let inner = &cap[1];
        // Structural refusals fire first so their message stays
        // specific. Slug-form grammar then routes through the same
        // `wiki_link_to_id` that the create/update mutation pipeline
        // calls — install-path and create-path refuse the same inputs
        // by construction.
        if inner.is_empty() {
            return Err(ValidationError::InvalidWikiLink {
                path: path.to_string(),
                link: format!("[[{inner}]]"),
                reason: "empty target".to_string(),
            });
        }
        if inner.contains("::") {
            return Err(ValidationError::InvalidWikiLink {
                path: path.to_string(),
                link: format!("[[{inner}]]"),
                reason: "reserved `::` cross-vault syntax is not accepted".to_string(),
            });
        }
        let target = match inner.find('|') {
            Some(i) => &inner[..i],
            None => inner,
        };
        if target.contains('#') {
            return Err(ValidationError::InvalidWikiLink {
                path: path.to_string(),
                link: format!("[[{inner}]]"),
                reason: "reserved `#` deep-link syntax is not accepted".to_string(),
            });
        }

        // Delegate the slug / vault grammar checks to the shared
        // strict resolver. The validator passes an empty current
        // vault — the strict resolver's self-prefix-strip step is
        // skipped (it's a Tier-1 convenience; the grammar gate fires
        // before it), so the grammar outcome is vault-independent.
        if let Err(e) = wiki_link_to_id(inner, "") {
            return Err(ValidationError::InvalidWikiLink {
                path: path.to_string(),
                link: format!("[[{inner}]]"),
                reason: e.to_string(),
            });
        }
    }
    Ok(())
}

fn check_bracket_balance(masked: &str, path: &str) -> Result<(), ValidationError> {
    let bytes = masked.as_bytes();
    let mut i = 0;
    let mut open = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if open > 0 {
                return Err(ValidationError::UnbalancedBrackets {
                    path: path.to_string(),
                });
            }
            open += 1;
            i += 2;
            continue;
        }
        if bytes[i] == b']' && bytes[i + 1] == b']' {
            if open == 0 {
                return Err(ValidationError::UnbalancedBrackets {
                    path: path.to_string(),
                });
            }
            open -= 1;
            i += 2;
            continue;
        }
        i += 1;
    }
    if open > 0 {
        return Err(ValidationError::UnbalancedBrackets {
            path: path.to_string(),
        });
    }
    Ok(())
}

fn wiki_link_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[\[([^\]]*)\]\]").unwrap())
}

fn inline_code_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`[^`]+`").unwrap())
}

fn inline_double_backtick_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"``[\s\S]*?``").unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::parser::parse_markdown;
    use memstead_schema::type_by_name;

    fn spec_type() -> std::sync::Arc<memstead_schema::TypeDefinition> {
        type_by_name("spec").unwrap()
    }

    fn parse(content: &str) -> Entity {
        parse_markdown(content, "test.md", &spec_type(), "v")
            .unwrap()
            .entity
    }

    fn validate(content: &str, entity: &Entity) -> Result<(), ValidationError> {
        validate_strict(
            content,
            entity,
            &spec_type(),
            "test.md",
        )
    }

    const MINIMAL_SPEC: &str = "\
---
type: spec
created_date: 2026-01-15
last_modified: 2026-01-15
level: M0
---
# Test Entity

## Identity

A meaningful identity line.

## Purpose

Why it exists.

## Specifies

What it covers.

## Constraints

Its limits.

## Rationale

Design notes.
";

    #[test]
    fn accepts_valid_spec() {
        let entity = parse(MINIMAL_SPEC);
        validate(MINIMAL_SPEC, &entity).unwrap();
    }

    #[test]
    fn rejects_missing_frontmatter() {
        let content = "# No Frontmatter\n\n## Identity\nBody.\n";
        let entity = parse(&format!("---\ntype: spec\n---\n{content}"));
        let err = validate(content, &entity).unwrap_err();
        assert!(matches!(err, ValidationError::MissingFrontmatter { .. }));
    }

    #[test]
    fn rejects_unclosed_frontmatter() {
        let content = "---\ntype: spec\n# stuck in frontmatter\n";
        let entity = parse(MINIMAL_SPEC); // entity parses fine; we still reject raw
        let err = validate(content, &entity).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidFrontmatter { .. }));
    }

    #[test]
    fn rejects_unknown_frontmatter_key() {
        let content = MINIMAL_SPEC.replacen(
            "level: M0",
            "level: M0\nunexpected_key: oops",
            1,
        );
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        match err {
            ValidationError::UnknownFrontmatterKey { key, .. } => {
                assert_eq!(key, "unexpected_key");
            }
            other => panic!("expected UnknownFrontmatterKey, got {other:?}"),
        }
    }

    #[test]
    fn rejects_missing_required_field() {
        let content = MINIMAL_SPEC.replacen("level: M0\n", "", 1);
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(matches!(err, ValidationError::MissingRequiredField { .. }));
    }

    #[test]
    fn rejects_missing_title() {
        let content = MINIMAL_SPEC.replacen("# Test Entity\n", "\n", 1);
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(matches!(err, ValidationError::MissingTitle { .. }));
    }

    #[test]
    fn rejects_missing_required_section() {
        let content = MINIMAL_SPEC.replacen(
            "## Purpose\n\nWhy it exists.\n\n",
            "",
            1,
        );
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::MissingRequiredSection { .. }
        ));
    }

    // Note: every shipping schema today declares exactly one catch_all
    // section (pinned by schemas::tests::every_schema_has_exactly_one_catch_all),
    // so `check_unknown_sections` cannot fire for the 10 registered
    // schemas. The check stays as defense-in-depth for hypothetical
    // future no-catch-all schemas; testing it would require a test-only
    // TypeDefinition fixture, deferred.

    #[test]
    fn rejects_malformed_relationship_line() {
        let content = format!(
            "{MINIMAL_SPEC}\n## Relationships\n\n- USES: [[target]]\n"
        );
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::InvalidRelationshipLine { .. }
        ));
    }

    #[test]
    fn accepts_valid_relationship_line() {
        let content = format!(
            "{MINIMAL_SPEC}\n## Relationships\n\n- **USES**: [[target-name]]\n"
        );
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn rejects_invalid_wiki_link_uppercase() {
        let content = format!(
            "{MINIMAL_SPEC}\nSee [[MyThing]] for details.\n"
        );
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidWikiLink { .. }));
    }

    #[test]
    fn rejects_invalid_wiki_link_underscore() {
        let content = format!("{MINIMAL_SPEC}\nSee [[a_b]] for details.\n");
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidWikiLink { .. }));
    }

    #[test]
    fn rejects_invalid_wiki_link_space() {
        let content = format!("{MINIMAL_SPEC}\nSee [[a b]] for details.\n");
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidWikiLink { .. }));
    }

    #[test]
    fn accepts_tier_two_cross_vault_link() {
        let content = format!(
            "{MINIMAL_SPEC}\nSee [[engine:health]] and [[engine:architecture/result]] for more.\n"
        );
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    /// Hierarchical vault paths are first-class. The install-side check
    /// converges onto `wiki_link_to_id`, which already accepts
    /// hierarchical Tier-2 prefixes — install no longer rejects what
    /// create produces.
    #[test]
    fn accepts_hierarchical_tier_two_link() {
        let content = format!(
            "{MINIMAL_SPEC}\nSee [[external/engine:health]] for details.\n"
        );
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn rejects_tier_two_with_empty_leaf() {
        let content = format!("{MINIMAL_SPEC}\nSee [[:slug]] for details.\n");
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidWikiLink { .. }));
    }

    #[test]
    fn rejects_tier_two_with_empty_slug() {
        let content = format!("{MINIMAL_SPEC}\nSee [[engine:]] for details.\n");
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidWikiLink { .. }));
    }

    #[test]
    fn rejects_tier_two_with_invalid_leaf_chars() {
        let content = format!("{MINIMAL_SPEC}\nSee [[Engine:slug]] for details.\n");
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidWikiLink { .. }));
    }

    #[test]
    fn rejects_tier_two_with_invalid_slug_chars() {
        let content = format!("{MINIMAL_SPEC}\nSee [[engine:Slug]] for details.\n");
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidWikiLink { .. }));
    }

    #[test]
    fn rejects_reserved_cross_vault_syntax() {
        let content = format!(
            "{MINIMAL_SPEC}\nSee [[other-vault::entity]] for details.\n"
        );
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        match err {
            ValidationError::InvalidWikiLink { reason, .. } => {
                assert!(reason.contains("::"), "reason={reason}");
            }
            other => panic!("expected InvalidWikiLink, got {other:?}"),
        }
    }

    #[test]
    fn rejects_reserved_deep_link_syntax() {
        let content = format!("{MINIMAL_SPEC}\nSee [[entity#section]]");
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        match err {
            ValidationError::InvalidWikiLink { reason, .. } => {
                assert!(reason.contains("#"), "reason={reason}");
            }
            other => panic!("expected InvalidWikiLink, got {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_wiki_link() {
        let content = format!("{MINIMAL_SPEC}\nSee [[]]");
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidWikiLink { .. }));
    }

    #[test]
    fn rejects_unbalanced_brackets() {
        let content = format!("{MINIMAL_SPEC}\nSee [[unterminated for details.\n");
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(matches!(err, ValidationError::UnbalancedBrackets { .. }));
    }

    #[test]
    fn accepts_valid_stub_wiki_link() {
        let content = format!(
            "{MINIMAL_SPEC}\nSee [[planned-feature]] and [[a/b/c]] for more.\n"
        );
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn accepts_wiki_link_inside_inline_code() {
        let content = format!(
            "{MINIMAL_SPEC}\nOne line per edge, shape `- **<REL>**: [[<target>]]`.\n"
        );
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn accepts_literal_backtick_via_double_delimiter() {
        // Real-world line from a macOS spec documenting the inline-markup
        // tokenizer: mixes `` ` `` (double-delim showing a literal `) with
        // `[[` inside single-delim backticks. Before the double-backtick
        // pre-pass was added, the single-backtick regex sliced through the
        // `` ` `` span and left a stray `[[` that tripped the bracket
        // checker.
        let content = format!(
            "{MINIMAL_SPEC}\nWalks left-to-right looking for the earliest of `**`, `` ` ``, `[[`. Done.\n"
        );
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn accepts_brackets_inside_double_backtick_span() {
        // `` `text` `` — double-backtick delimiter wrapping content that
        // itself contains single backticks. The stripped span may hold
        // unbalanced brackets without leaking to the bracket checker.
        let content = format!(
            "{MINIMAL_SPEC}\n| Inline code | `` `[[slug]]` `` | note. |\n"
        );
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn accepts_wiki_link_with_alias() {
        let content = format!(
            "{MINIMAL_SPEC}\nSee [[target|Display Text]] for more.\n"
        );
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn accepts_wiki_link_with_parent_relative_and_md() {
        let content = format!(
            "{MINIMAL_SPEC}\nSee [[../parent/entity.md]] for more.\n"
        );
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn accepts_wiki_link_inside_code_block() {
        let content = format!(
            "{MINIMAL_SPEC}\n```\nlet x = [[this is not a link]];\n```\n"
        );
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn accepts_windows_line_endings() {
        let content = MINIMAL_SPEC.replace('\n', "\r\n");
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn accepts_leading_bom() {
        // The BOM is stripped by the archive extraction layer before
        // parse_markdown sees the bytes — mirror that here. The strict
        // checker also tolerates BOM on the raw pass (defense-in-depth).
        let raw = format!("\u{feff}{MINIMAL_SPEC}");
        let stripped = raw.strip_prefix('\u{feff}').unwrap();
        let entity = parse(stripped);
        validate(&raw, &entity).unwrap();
    }
}
