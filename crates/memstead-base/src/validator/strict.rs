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
use crate::entity::parser::{Frontmatter, mask_code_blocks, split_sections};
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
    // No byte-order-mark strip here: `split_frontmatter_core` owns it, and a
    // second strip is how the two paths came to disagree about where a
    // document begins.
    let (meta_block, body) = split_frontmatter_strict(raw_bytes, path)?;
    check_metadata(meta_block, entity, schema, path)?;
    check_title_presence(body, path)?;
    check_sections_present(entity, schema, path)?;
    check_unknown_sections(body, schema, path)?;
    check_relationships_syntax(body, path)?;
    check_relationship_types(entity, path)?;
    check_wiki_links(body, path)?;

    Ok(())
}

/// Verify the frontmatter opens with `---` on the first line and
/// closes with `\n---`. Returns (metadata block text, body text).
/// Body excludes the closing `\n---` line and the newline after it.
pub(crate) fn split_frontmatter_strict<'a>(
    raw: &'a str,
    path: &str,
) -> Result<(&'a str, &'a str), ValidationError> {
    // The refusing wrapper. The arithmetic is `split_frontmatter_core`'s; what
    // is strict here is only that each failure becomes a typed error instead
    // of a degradation, and those refusals are wire-visible at archive
    // ingress.
    match crate::entity::parser::split_frontmatter_core(raw).1 {
        Frontmatter::Present { meta, body } => Ok((meta, body)),
        Frontmatter::NoOpeningDelimiter => Err(ValidationError::MissingFrontmatter {
            path: path.to_string(),
        }),
        Frontmatter::Unclosed => Err(ValidationError::InvalidFrontmatter {
            path: path.to_string(),
            reason: "frontmatter block is not closed with `\\n---`".to_string(),
        }),
    }
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
        let is_required = field.is_required();
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
///
/// Scans the masked body, because the parser's title extraction does:
/// a `# ` line inside a code block is not a title on either side of
/// the seam. Line *counting* stays on the original so a leading code
/// block still consumes the three-line window.
fn check_title_presence(body: &str, path: &str) -> Result<(), ValidationError> {
    let masked_body = mask_code_blocks(body);
    let mut lines_seen = 0;
    for (line, masked) in body.lines().zip(masked_body.lines()) {
        if line.trim().is_empty() {
            continue;
        }
        lines_seen += 1;
        if lines_seen > 3 {
            break;
        }
        if let Some(rest) = masked.strip_prefix("# ")
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

    // Section boundaries come from the one splitter — the validator
    // must not carry a second definition of what opens a section.
    let (_, _, raw_headings) = split_sections(body, &mask_code_blocks(body));
    for heading in &raw_headings {
        if !known_headings.contains(&heading.as_str()) {
            return Err(ValidationError::UnknownSection {
                path: path.to_string(),
                section: heading.clone(),
            });
        }
    }
    Ok(())
}

/// The engine-side format declaration for the auto-managed
/// `## Relationships` section — the first consumer of the shared
/// section-format mechanism (plan 08). The Relationships section is
/// engine-managed, not a schema `SectionDef`, so the declaration
/// lives here: an optional bullet list (empty sections are legal)
/// whose items carry the canonical relation-line shape. Replaces the
/// pre-plan hand-rolled per-line regex scan — one format-check
/// implementation in the tree.
fn relationships_format_def() -> &'static memstead_schema::SectionDef {
    static DEF: OnceLock<memstead_schema::SectionDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let content = "list(bullet)?";
        memstead_schema::SectionDef {
            key: "relationships".to_string(),
            heading: "Relationships".to_string(),
            required: false,
            load_bearing: None,
            search_weight: 0.0,
            catch_all: false,
            write_rules: vec![],
            description: None,
            content: Some(content.to_string()),
            item_pattern: Some(r"\*\*[A-Z_]+\*\*:\s*\[\[[^\]]+\]\](\s*—.*)?".to_string()),
            table: None,
            example: Some("- **USES**: [[target-name]]".to_string()),
            format_severity: memstead_schema::ConstraintSeverity::Block,
            compiled_content: Some(
                memstead_schema::content_expr::ContentExpr::parse(content)
                    .expect("engine-side declaration is valid"),
            ),
            format_problems: Vec::new(),
        }
    })
}

fn check_relationships_syntax(body: &str, path: &str) -> Result<(), ValidationError> {
    // The section body comes from the one splitter, sliced from the
    // ORIGINAL body — not reassembled from the masked copy. The
    // content checker below is a CommonMark parser; feeding it a body
    // whose code blocks had already become whitespace was the seam
    // where splitter and validator judged differently shaped input.
    let (sections, _, _) = split_sections(body, &mask_code_blocks(body));
    let section = match sections.get("relationships") {
        Some((_, s)) if !s.trim().is_empty() => s.clone(),
        _ => return Ok(()),
    };
    if let Some(v) =
        crate::section_format::check_section_format(relationships_format_def(), &section)
            .into_iter()
            .next()
    {
        let line = match &v {
            crate::section_format::SectionFormatViolation::ItemPatternMismatch { text, .. } => {
                text.clone()
            }
            other => other.describe(),
        };
        return Err(ValidationError::InvalidRelationshipLine {
            path: path.to_string(),
            line,
        });
    }
    Ok(())
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
/// wiki-link in the body. Operates on the masked body, so content in
/// any CommonMark code block is ignored and inline `` `…` `` spans are
/// invisible too — grammar examples like `` `[[<target>]]` `` in prose
/// don't trip the slug-regex check. Exactly the masking
/// `entity::parser::extract_inline_links` performs on the extraction
/// side: one definition, both paths.
///
/// Inline code spans come from the CommonMark parser, not from a
/// delimiter-count regex: multi-backtick spans (``` `` ` `` ```, used
/// to display a literal backtick) are one span to the parser, where a
/// single-backtick regex would slice into the middle of one and leave
/// stray `` ` `` / `[[` remnants that then fool the bracket checker.
fn check_wiki_links(body: &str, path: &str) -> Result<(), ValidationError> {
    let masked = crate::markdown::mask_code_blocks_and_spans(body);

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
                reason: "reserved `::` cross-mem syntax is not accepted".to_string(),
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

        // Delegate the slug / mem grammar checks to the shared
        // strict resolver. The validator passes an empty current
        // mem — the strict resolver's self-prefix-strip step is
        // skipped (it's a Tier-1 convenience; the grammar gate fires
        // before it), so the grammar outcome is mem-independent.
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
        validate_strict(content, entity, &spec_type(), "test.md")
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
        let content = MINIMAL_SPEC.replacen("level: M0", "level: M0\nunexpected_key: oops", 1);
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
        let content = MINIMAL_SPEC.replacen("## Purpose\n\nWhy it exists.\n\n", "", 1);
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
        let content = format!("{MINIMAL_SPEC}\n## Relationships\n\n- USES: [[target]]\n");
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::InvalidRelationshipLine { .. }
        ));
    }

    #[test]
    fn accepts_valid_relationship_line() {
        let content = format!("{MINIMAL_SPEC}\n## Relationships\n\n- **USES**: [[target-name]]\n");
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn rejects_invalid_wiki_link_uppercase() {
        let content = format!("{MINIMAL_SPEC}\nSee [[MyThing]] for details.\n");
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
    fn accepts_tier_two_cross_mem_link() {
        let content = format!(
            "{MINIMAL_SPEC}\nSee [[engine:health]] and [[engine:architecture/result]] for more.\n"
        );
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    /// Hierarchical mem paths are first-class. The install-side check
    /// converges onto `wiki_link_to_id`, which already accepts
    /// hierarchical Tier-2 prefixes — install no longer rejects what
    /// create produces.
    #[test]
    fn accepts_hierarchical_tier_two_link() {
        let content = format!("{MINIMAL_SPEC}\nSee [[external/engine:health]] for details.\n");
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
    fn rejects_reserved_cross_mem_syntax() {
        let content = format!("{MINIMAL_SPEC}\nSee [[other-mem::entity]] for details.\n");
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
        let content = format!("{MINIMAL_SPEC}\nSee [[planned-feature]] and [[a/b/c]] for more.\n");
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn accepts_wiki_link_inside_inline_code() {
        let content =
            format!("{MINIMAL_SPEC}\nOne line per edge, shape `- **<REL>**: [[<target>]]`.\n");
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
        let content = format!("{MINIMAL_SPEC}\n| Inline code | `` `[[slug]]` `` | note. |\n");
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn accepts_wiki_link_with_alias() {
        let content = format!("{MINIMAL_SPEC}\nSee [[target|Display Text]] for more.\n");
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn accepts_wiki_link_with_parent_relative_and_md() {
        let content = format!("{MINIMAL_SPEC}\nSee [[../parent/entity.md]] for more.\n");
        let entity = parse(&content);
        validate(&content, &entity).unwrap();
    }

    #[test]
    fn accepts_wiki_link_inside_code_block() {
        let content = format!("{MINIMAL_SPEC}\n```\nlet x = [[this is not a link]];\n```\n");
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
        // Nothing upstream strips the mark: archive extraction keeps the
        // raw bytes so offset reporting stays truthful, and `validate_strict`
        // hands them straight to the core, which strips. Both the marked and
        // the unmarked form must therefore validate identically.
        let raw = format!("\u{feff}{MINIMAL_SPEC}");
        let stripped = raw.strip_prefix('\u{feff}').unwrap();
        let entity = parse(stripped);
        validate(&raw, &entity).unwrap();
    }

    // --- the CommonMark referee: validator and splitter agree ------

    /// Build a spec whose `## Specifies` body is `body`.
    fn spec_with_specifies(body: &str) -> String {
        MINIMAL_SPEC.replace("What it covers.", body)
    }

    /// Every builtin type declares a catch-all section, which makes
    /// `check_unknown_sections` a no-op for them — so the check is
    /// exercised against a variant of the spec type whose sections all
    /// declare themselves closed.
    fn spec_type_without_catch_all() -> std::sync::Arc<memstead_schema::TypeDefinition> {
        let mut t = (*spec_type()).clone();
        for section in &mut t.sections {
            section.catch_all = false;
        }
        std::sync::Arc::new(t)
    }

    fn validate_no_catch_all(content: &str) -> Result<(), ValidationError> {
        let ty = spec_type_without_catch_all();
        let entity = crate::entity::parser::parse_markdown(content, "test.md", &ty, "specs")
            .unwrap()
            .entity;
        validate_strict(content, &entity, &ty, "test.md")
    }

    /// The unknown-section check now draws boundaries from the one
    /// splitter, so a `## ` inside any CommonMark code block is not a
    /// section on either side of the seam.
    #[test]
    fn unknown_section_check_ignores_code_block_headings() {
        for body in [
            "```\n## Not A Section\n```",
            "~~~\n## Not A Section\n~~~",
            "> ```\n> ## Not A Section\n> ```",
            "````\n```\n## Not A Section\n```\n````",
            "    ## Not A Section",
        ] {
            let content = spec_with_specifies(body);
            validate_no_catch_all(&content).unwrap_or_else(|e| {
                panic!("code-block heading must not be a section: {body:?} -> {e}")
            });
        }
    }

    /// Complement: a real column-0 `## ` in prose is still an unknown
    /// section.
    #[test]
    fn unknown_section_check_still_refuses_a_prose_heading() {
        let content = spec_with_specifies("text\n\n## Invented Section\n\nmore");
        let err = validate_no_catch_all(&content).unwrap_err();
        assert!(
            matches!(err, ValidationError::UnknownSection { ref section, .. } if section == "Invented Section"),
            "{err:?}"
        );
    }

    /// The title check scans the masked body, because the parser's
    /// title extraction does.
    #[test]
    fn title_check_ignores_a_heading_inside_a_code_block() {
        let content = MINIMAL_SPEC.replace("# Test Entity", "```\n# Fake\n```");
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(
            matches!(err, ValidationError::MissingTitle { .. }),
            "{err:?}"
        );
    }

    /// One inline-code definition: a link the validator cannot see is
    /// a link no path synthesises an edge from. A multi-backtick span
    /// is the case a delimiter-count regex slices through.
    #[test]
    fn inline_code_spans_hide_links_from_the_validator() {
        let content = spec_with_specifies("`[[Not A Slug]]` and ``[[Also Not]]`` are literals.");
        let entity = parse(&content);
        validate(&content, &entity).expect("links inside inline code are not links to any path");
    }

    /// Complement: the same malformed link in prose is still refused.
    #[test]
    fn a_malformed_link_in_prose_is_still_refused() {
        let content = spec_with_specifies("See [[Not A Slug]].");
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(
            matches!(err, ValidationError::InvalidWikiLink { .. }),
            "{err:?}"
        );
    }

    /// The seam: the relationship-syntax check used to reassemble the
    /// section by line-scanning the *masked* body, so the CommonMark
    /// content checker judged a body whose code blocks had already
    /// become whitespace — and a code block in the Relationships
    /// section read as an empty section and passed silently. The
    /// section now comes from the one splitter, sliced from the
    /// original.
    #[test]
    fn relationships_section_is_judged_on_the_original_body() {
        let content = MINIMAL_SPEC.replace(
            "## Rationale\n\nDesign notes.\n",
            "## Rationale\n\nDesign notes.\n\n## Relationships\n\n```\n- **USES**: [[x]]\n```\n",
        );
        let entity = parse(&content);
        let err = validate(&content, &entity).expect_err("a code block is not a relationship list");
        assert!(
            matches!(err, ValidationError::InvalidRelationshipLine { .. }),
            "{err:?}"
        );
    }

    /// Complement: the ordinary bullet list still validates, and a
    /// `## Relationships` heading that only appears inside a code
    /// block still opens no section.
    #[test]
    fn relationships_section_complements() {
        let ok = MINIMAL_SPEC.replace(
            "## Rationale\n\nDesign notes.\n",
            "## Rationale\n\nDesign notes.\n\n## Relationships\n\n- **USES**: [[some-target]]\n",
        );
        let entity = parse(&ok);
        validate(&ok, &entity).expect("a bullet relationship list is valid");

        let fenced = spec_with_specifies("```\n## Relationships\n\nnot a list at all\n```");
        let entity = parse(&fenced);
        validate(&fenced, &entity).expect("a fenced `## Relationships` opens no section to check");
    }

    /// The empty-target refusal — the asymmetry's typed side — stays.
    #[test]
    fn empty_wiki_link_target_is_still_refused() {
        let content = spec_with_specifies("An empty [[]] link.");
        let entity = parse(&content);
        let err = validate(&content, &entity).unwrap_err();
        assert!(
            matches!(err, ValidationError::InvalidWikiLink { ref reason, .. } if reason == "empty target"),
            "{err:?}"
        );
    }
}
