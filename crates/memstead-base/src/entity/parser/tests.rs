#![cfg(test)]

use super::*;
use memstead_schema::{builtin_names, type_by_name};
use std::sync::Arc;

fn spec_schema() -> Arc<TypeDefinition> {
    type_by_name(builtin_names::SPEC).unwrap()
}

fn memo_schema() -> Arc<TypeDefinition> {
    type_by_name(builtin_names::MEMO).unwrap()
}

#[test]
fn parse_metadata_types() {
    let meta = parse_metadata("key: value\nnum: 42\nfloat: 0.85\nbool: true\nfalsy: false");
    assert_eq!(meta["key"], MetadataValue::String("value".to_string()));
    assert_eq!(meta["num"], MetadataValue::Integer(42));
    assert_eq!(meta["float"], MetadataValue::Float(0.85));
    assert_eq!(meta["bool"], MetadataValue::Bool(true));
    assert_eq!(meta["falsy"], MetadataValue::Bool(false));
}

#[test]
fn parse_metadata_strips_comments() {
    let meta = parse_metadata("key: value # this is a comment");
    assert_eq!(meta["key"], MetadataValue::String("value".to_string()));
}

#[test]
fn parse_metadata_strips_quotes() {
    let meta = parse_metadata("key: \"quoted value\"\nkey2: 'single'");
    assert_eq!(
        meta["key"],
        MetadataValue::String("quoted value".to_string())
    );
    assert_eq!(meta["key2"], MetadataValue::String("single".to_string()));
}

#[test]
fn parse_metadata_survives_malformed_values() {
    // A lone quote character satisfies both starts_with and ends_with —
    // the old unguarded slice `s[1..s.len()-1]` panicked on it.
    let meta = parse_metadata(
        "key: \"\nkey2: '\nkey3: \"\"\nkey4: ''\nkey5: \"unterminated\nkey6: mixed'\"",
    );
    assert_eq!(meta["key"], MetadataValue::String("\"".to_string()));
    assert_eq!(meta["key2"], MetadataValue::String("'".to_string()));
    assert_eq!(meta["key3"], MetadataValue::String(String::new()));
    assert_eq!(meta["key4"], MetadataValue::String(String::new()));
    assert_eq!(
        meta["key5"],
        MetadataValue::String("\"unterminated".to_string())
    );
    assert_eq!(meta["key6"], MetadataValue::String("mixed'\"".to_string()));

    // More frontmatter shapes that must parse to a value, never panic:
    // colon-only lines, multi-byte values, keyless colons, huge digits.
    let meta = parse_metadata(":\n: value\nkey7: ✓\"\nkey8: 99999999999999999999999999\nkey9: -");
    assert_eq!(meta["key7"], MetadataValue::String("✓\"".to_string()));
    assert_eq!(
        meta["key8"],
        MetadataValue::String("99999999999999999999999999".to_string())
    );
    assert_eq!(meta["key9"], MetadataValue::String("-".to_string()));
}

#[test]
fn parse_metadata_skips_comments_and_empty() {
    let meta = parse_metadata("# comment\n\nkey: val\n---");
    assert_eq!(meta.len(), 1);
    assert_eq!(meta["key"], MetadataValue::String("val".to_string()));
}

#[test]
fn peek_type_finds_value() {
    let content = "---\ntype: memo\ntitle: Test\n---\n# Body\n";
    assert_eq!(
        peek_type_from_frontmatter(content),
        Some("memo".to_string())
    );
}

#[test]
fn peek_type_returns_none_when_missing() {
    let content = "---\ntitle: Test\n---\n# Body\n";
    assert_eq!(peek_type_from_frontmatter(content), None);
}

#[test]
fn peek_type_returns_none_without_frontmatter() {
    let content = "# Just a heading\n\nBody with type: concept inside text.\n";
    assert_eq!(peek_type_from_frontmatter(content), None);
}

/// The contract on a carriage-return document: the opening fence is
/// five bytes, the meta slice carries no fence, and the body starts
/// after the closing fence's `\r\n` — the shape the CR-blind site of
/// the 2026-09-04 census got wrong with a four-byte offset constant.
#[test]
fn split_core_crlf_document_borrows_meta_and_body() {
    let content = "---\r\ntype: spec\r\nlevel: M0\r\n---\r\n# Title\r\n\r\nBody.\r\n";
    let (stripped, split) = split_frontmatter_core(content);
    assert_eq!(stripped, content);
    assert_eq!(
        split,
        Frontmatter::Present {
            meta: "type: spec\r\nlevel: M0\r",
            body: "# Title\r\n\r\nBody.\r\n"
        }
    );
    let (meta, body) = frontmatter_parts(content).unwrap();
    assert_eq!(meta, "type: spec\r\nlevel: M0\r");
    assert_eq!(body, "# Title\r\n\r\nBody.\r\n");
    assert_eq!(body_after_frontmatter(content), body);
    // The same document with a byte-order mark splits identically.
    let marked = format!("\u{feff}{content}");
    assert_eq!(frontmatter_parts(&marked), Some((meta, body)));
}

/// A fence anywhere but the first line is not frontmatter: the whole
/// document is body, and nothing is injected or stripped at the first
/// fence found — the shape the fence-searching site of the census got
/// wrong.
#[test]
fn first_fence_not_at_document_start_is_no_frontmatter() {
    let content = "# Intro\n\n---\ntype: spec\n---\n\nBody.\n";
    assert_eq!(
        split_frontmatter_core(content),
        (content, Frontmatter::NoOpeningDelimiter)
    );
    assert_eq!(frontmatter_parts(content), None);
    assert_eq!(body_after_frontmatter(content), content);
    assert_eq!(peek_type_from_frontmatter(content), None);
    // An opening fence that never closes is not frontmatter either.
    let unclosed = "---\ntype: spec\nno close\n";
    assert_eq!(
        split_frontmatter_core(unclosed),
        (unclosed, Frontmatter::Unclosed)
    );
    assert_eq!(frontmatter_parts(unclosed), None);
}

#[test]
fn peek_type_handles_windows_line_endings() {
    let content = "---\r\ntype: principle\r\n---\r\n# Body\r\n";
    assert_eq!(
        peek_type_from_frontmatter(content),
        Some("principle".to_string())
    );
}

#[test]
fn peek_type_strips_quotes_and_comments() {
    let quoted = "---\ntype: \"concept\"\n---\n";
    assert_eq!(
        peek_type_from_frontmatter(quoted),
        Some("concept".to_string())
    );
    let commented = "---\ntype: memo # kind of\n---\n";
    assert_eq!(
        peek_type_from_frontmatter(commented),
        Some("memo".to_string())
    );
}

#[test]
fn peek_type_empty_value_returns_none() {
    let content = "---\ntype:\n---\n";
    assert_eq!(peek_type_from_frontmatter(content), None);
}

#[test]
fn peek_type_ignores_legacy_schema_key() {
    // After the hard break, a bare `schema:` in frontmatter is not
    // recognized as the type key — it's just arbitrary metadata.
    let content = concat!("---\n", "schema", ": memo\n---\n");
    assert_eq!(peek_type_from_frontmatter(content), None);
}

#[test]
fn mask_code_blocks_basic() {
    let input = "before\n```\ncode [[link]]\n```\nafter";
    let masked = mask_code_blocks(input);
    assert!(!masked.contains("[[link]]"));
    assert!(masked.contains("before"));
    assert!(masked.contains("after"));
}

#[test]
fn mask_code_blocks_preserves_line_count() {
    let input = "line1\n```\ncode\nmore code\n```\nline6";
    let masked = mask_code_blocks(input);
    assert_eq!(input.lines().count(), masked.lines().count());
}

#[test]
fn mask_code_blocks_unclosed() {
    let input = "before\n```\ncode\nmore code";
    let masked = mask_code_blocks(input);
    assert!(masked.contains("before"));
    assert!(!masked.contains("code"));
}

#[test]
fn parse_relationships_basic() {
    let text = "- **USES**: [[target-entity]]\n- **PART_OF**: [[parent]]";
    let rels = parse_relationships_with_warnings(text, "specs", None).0;
    assert_eq!(rels.len(), 2);
    assert_eq!(rels[0].rel_type, "USES");
    assert_eq!(rels[0].target.0, "specs--target-entity");
    assert_eq!(rels[1].rel_type, "PART_OF");
    assert_eq!(rels[1].target.0, "specs--parent");
    // Simple form parses without a description.
    assert!(rels[0].description.is_none());
    assert!(rels[1].description.is_none());
}

#[test]
fn parse_relationships_canonical_em_dash_captures_description() {
    let text = "- **OTHER**: [[a]] \u{2014} replaced by checkout-flow";
    let (rels, warnings) = parse_relationships_with_warnings(text, "specs", None);
    assert_eq!(rels.len(), 1);
    assert_eq!(
        rels[0].description.as_deref(),
        Some("replaced by checkout-flow")
    );
    assert!(warnings.is_empty(), "canonical em-dash does not warn");
}

#[test]
fn parse_relationships_em_dash_inside_description_body() {
    let text = "- **OTHER**: [[a]] \u{2014} note with — inside body";
    let (rels, warnings) = parse_relationships_with_warnings(text, "specs", None);
    assert_eq!(rels.len(), 1);
    assert_eq!(
        rels[0].description.as_deref(),
        Some("note with — inside body"),
        "the parser captures up to end-of-line; em-dashes inside the body survive"
    );
    assert!(warnings.is_empty());
}

#[test]
fn parse_relationships_ambiguous_double_hyphen_warns_and_drops_content() {
    let text = "- **USES**: [[a]] -- legacy delimiter";
    let entity_id = EntityId::new("specs", "src");
    let (rels, warnings) = parse_relationships_with_warnings(text, "specs", Some(&entity_id));
    assert_eq!(rels.len(), 1);
    assert!(rels[0].description.is_none(), "trailing content is dropped");
    assert_eq!(warnings.len(), 1);
    assert!(matches!(
        warnings[0],
        crate::ops::WarningHint::AmbiguousDescriptionDelimiter { .. }
    ));
}

#[test]
fn parse_relationships_ambiguous_single_hyphen_warns_and_drops_content() {
    let text = "- **USES**: [[a]] - single hyphen";
    let entity_id = EntityId::new("specs", "src");
    let (rels, warnings) = parse_relationships_with_warnings(text, "specs", Some(&entity_id));
    assert_eq!(rels.len(), 1);
    assert!(rels[0].description.is_none());
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code(), "AMBIGUOUS_DESCRIPTION_DELIMITER");
}

#[test]
fn parse_relationships_hyphenated_slug_target_parses_unambiguously() {
    let text = "- **USES**: [[some-slug-with-hyphens]] \u{2014} ok";
    let (rels, warnings) = parse_relationships_with_warnings(text, "specs", None);
    assert_eq!(rels.len(), 1);
    assert_eq!(rels[0].target.path(), "some-slug-with-hyphens");
    assert_eq!(rels[0].description.as_deref(), Some("ok"));
    assert!(warnings.is_empty());
}

#[test]
fn parse_full_entity() {
    let md = "\
---
type: spec
created_date: 2026-01-15
last_modified: 2026-04-12
level: M0
tags: backend, api
---
# Test Entity

## Identity

This is a test entity.

## Purpose

Testing the parser.

## Relationships

- **USES**: [[other-entity]]

## Specifies

Some specification content with [[inline-link]].
";
    let result = parse_markdown(md, "test-entity.md", &spec_schema(), "specs").unwrap();
    let entity = &result.entity;
    assert_eq!(entity.id.0, "specs--test-entity");
    assert_eq!(entity.title, "Test Entity");
    assert_eq!(entity.mem, "specs");
    assert_eq!(
        entity.metadata["type"],
        MetadataValue::String("spec".to_string())
    );
    assert_eq!(
        entity.metadata["level"],
        MetadataValue::String("M0".to_string())
    );
    assert_eq!(
        entity.metadata["tags"],
        MetadataValue::String("backend, api".to_string())
    );
    assert_eq!(entity.sections["identity"], "This is a test entity.");
    assert_eq!(entity.sections["purpose"], "Testing the parser.");
    assert_eq!(entity.relationships.len(), 1);
    assert_eq!(entity.relationships[0].rel_type, "USES");
    assert_eq!(entity.relationships[0].target.0, "specs--other-entity");
    assert_eq!(result.inline_links.len(), 1);
    assert_eq!(result.inline_links[0].0, "specs--inline-link");
}

#[test]
fn parse_full_entity_memo_schema() {
    let md = "\
---
type: memo
created_date: 2026-01-15
last_modified: 2026-04-12
status: active
tags: decision, architecture
---
# Use Sled For Storage

## Claim

Sled is the right embedded store for this workload.

## Context

We evaluated sled, rocksdb, and sqlite for the in-process graph cache.

## Substance

Sled wins on pure-Rust dependency footprint.
";
    let result = parse_markdown(md, "use-sled.md", &memo_schema(), "memos").unwrap();
    let entity = &result.entity;
    assert_eq!(entity.id.0, "memos--use-sled");
    assert_eq!(entity.title, "Use Sled For Storage");
    assert_eq!(entity.mem, "memos");
    assert_eq!(
        entity.metadata["type"],
        MetadataValue::String("memo".to_string())
    );
    assert_eq!(
        entity.metadata["status"],
        MetadataValue::String("active".to_string())
    );
    assert_eq!(
        entity.sections["claim"],
        "Sled is the right embedded store for this workload."
    );
    assert_eq!(
        entity.sections["context"],
        "We evaluated sled, rocksdb, and sqlite for the in-process graph cache."
    );
    assert_eq!(
        entity.sections["substance"],
        "Sled wins on pure-Rust dependency footprint."
    );
    assert!(!entity.sections.contains_key("identity"));
    assert!(!entity.sections.contains_key("purpose"));
}

#[test]
fn parse_entity_without_frontmatter() {
    let md = "# No Frontmatter\n\n## Identity\n\nJust a title and section.";
    let result = parse_markdown(md, "no-fm.md", &spec_schema(), "specs").unwrap();
    assert_eq!(result.entity.title, "No Frontmatter");
    // Only the auto-injected type field should be present
    assert_eq!(result.entity.metadata.len(), 1);
    assert_eq!(
        result.entity.metadata.get("type"),
        Some(&MetadataValue::String("spec".to_string()))
    );
}

#[test]
fn parse_entity_code_blocks_not_detected() {
    let md = "\
---
type: spec
---
# Code Test

## Identity

Test entity.

## Specifies

```
## Not A Section
- **USES**: [[not-a-link]]
```

Real content after code block.
";
    let result = parse_markdown(md, "code-test.md", &spec_schema(), "specs").unwrap();
    // The ## inside code block should NOT be parsed as a section
    assert!(!result.entity.sections.contains_key("not a section"));
    // The wiki-link inside code block should NOT be extracted
    assert!(result.inline_links.is_empty());
}

// Fixture pinned by the adversarial harness (seed 0x5eedf001, case 224):
// a BOM'd file parsed tolerantly as all-body, losing its entire
// frontmatter, while the strict validator and the archive path stripped
// the mark and saw it. That divergence is why the mark is now stripped in
// `split_frontmatter_core` and nowhere else: one implementation cannot
// disagree with itself about where a document begins.
#[test]
fn bom_prefixed_frontmatter_is_recognized() {
    let md = "\u{feff}---\ntype: spec\n---\n# Bom Entity\n\n## Identity\n\nBody.\n";
    assert_eq!(peek_type_from_frontmatter(md), Some("spec".to_string()));
    assert_eq!(
        body_after_frontmatter(md),
        "# Bom Entity\n\n## Identity\n\nBody.\n"
    );
    let (meta, body) = split_frontmatter(md).unwrap();
    assert_eq!(meta, "type: spec");
    assert_eq!(body, "# Bom Entity\n\n## Identity\n\nBody.\n");
    let result = parse_markdown(md, "bom.md", &spec_schema(), "specs").unwrap();
    assert_eq!(
        result.entity.metadata["type"],
        MetadataValue::String("spec".to_string())
    );
    assert_eq!(result.entity.sections["identity"], "Body.");
}

// Fixture pinned by the adversarial harness (seed 0x5eedf001, case 46):
// a section whose content ends inside an open code fence absorbed every
// section the generator wrote after it on the next parse — content
// shifted between sections and the document GREW on every
// parse→generate round. The generator now terminates the open fence;
// the first round normalises, then parse→generate is a fixpoint.
#[test]
fn open_fence_in_section_content_does_not_swallow_following_sections() {
    let md = "\
---
type: spec
---
# Code Test

## Identity

Base.

## Specifies

```
truncated code with no closer";
    let schema = spec_schema();
    let e1 = parse_markdown(md, "open-fence.md", &schema, "specs").unwrap();
    let m1 = crate::entity::generator::generate_markdown(&e1.entity, &schema);
    let e2 = parse_markdown(&m1, "open-fence.md", &schema, "specs").unwrap();
    assert_eq!(
        e2.entity.sections["identity"], "Base.",
        "sections before the open fence survive"
    );
    assert!(
        !e2.entity.sections["specifies"].contains("## Constraints"),
        "the generated sections after the fence are not absorbed into it"
    );
    let m2 = crate::entity::generator::generate_markdown(&e2.entity, &schema);
    assert_eq!(
        m1, m2,
        "parse→generate is a fixpoint after one normalising round"
    );
}

// Fixture pinned by the adversarial harness (seed 0x5eedf001, case 26):
// a document carrying MULTIPLE non-schema sections — reachable on the
// tolerant local-read path, which refuses nothing — reconstructed its
// catch-all in HashMap iteration order, so canonical bytes differed
// from parse to parse of the same input. The catch-all must re-emit
// non-schema sections in document order, and parse→generate must be
// idempotent for such input.
#[test]
fn catch_all_reconstruction_is_document_ordered_and_idempotent() {
    let md = "\
---
type: spec
---
# Multi Unknown

## Identity

Base.

## Claim

First unknown.

## Context

Second unknown.

## Substance

Third unknown.
";
    let schema = spec_schema();
    let e1 = parse_markdown(md, "multi-unknown.md", &schema, "specs").unwrap();
    assert_eq!(
        e1.entity.sections["specifies"],
        "## Claim\nFirst unknown.\n\n## Context\nSecond unknown.\n\n## Substance\nThird unknown.",
        "non-schema sections land in the catch-all in document order"
    );
    let m1 = crate::entity::generator::generate_markdown(&e1.entity, &schema);
    let e2 = parse_markdown(&m1, "multi-unknown.md", &schema, "specs").unwrap();
    let m2 = crate::entity::generator::generate_markdown(&e2.entity, &schema);
    assert_eq!(
        m1, m2,
        "parse→generate is idempotent over multi-unknown-section input"
    );
}

// Fixture pinned by the coverage-guided long tier (local run,
// 2026-08-24; corpus member `crash-fd71330e…`): parse_markdown
// re-trimmed every section value after split_sections had already
// normalised it, silently promoting a whitespace-prefixed first
// line (vertical tab + backticks) to column 0 — where the
// CommonMark referee saw a fence opener the stored form did not
// have, so the section structure shifted between rounds. The
// splitter's trim is the only content trim.
#[test]
fn first_line_whitespace_prefix_survives_storage_and_round_trips() {
    let schema = spec_schema();
    let md = "---\ntype: spec\n---\n# T\n\n## Identity\n\u{b}```\nx\n\n## Purpose\np\n";
    let e1 = parse_markdown(md, "vt.md", &schema, "specs").unwrap();
    assert_eq!(
        e1.entity.sections["identity"], "\u{b}```\nx",
        "the first visible line keeps its whitespace prefix byte-exactly"
    );
    let m1 = crate::entity::generator::generate_markdown(&e1.entity, &schema);
    let e2 = parse_markdown(&m1, "vt.md", &schema, "specs").unwrap();
    let m2 = crate::entity::generator::generate_markdown(&e2.entity, &schema);
    assert_eq!(m1, m2, "parse→generate is a fixpoint");
}

// Fixture pinned by the coverage-guided long tier (0.17.0 release
// readiness run, 2026-09-02; corpus member `crash-1233c134…`): a
// non-schema section whose content ends inside an HTML block of a
// kind no blank line ends (`<!X`, type 4) was merged in front of a
// later piece. In situ a `>` line in the catch-all's own content had
// closed the block, so the later piece's tilde fence opened and
// masked a `## ` line into content; merged, the block stayed open,
// the fence became prose, the masked line surfaced as an empty
// non-schema heading, and the next round dropped it. The merge's
// incremental context close now terminates HTML blocks like fences.
#[test]
fn merged_open_html_block_is_closed_so_later_fences_keep_masking() {
    let schema = spec_schema();
    let md = "\
---
type: spec
---
# T

## Claim
<!X

## Specifies
done >

## Later
~~~
## Hidden
";
    let e1 = parse_markdown(md, "html.md", &schema, "specs").unwrap();
    assert_eq!(
        e1.entity.sections["specifies"],
        "done >\n\n## Claim\n<!X\n>\n\n## Later\n~~~\n## Hidden\n~~~",
        "the open HTML block closes before the next piece, the dangling fence after it"
    );
    let m1 = crate::entity::generator::generate_markdown(&e1.entity, &schema);
    let e2 = parse_markdown(&m1, "html.md", &schema, "specs").unwrap();
    let m2 = crate::entity::generator::generate_markdown(&e2.entity, &schema);
    assert_eq!(m1, m2, "parse→generate is a fixpoint");
}

// Fixture pinned by the CI fuzz run of 2026-09-08 (corpus member
// `crash-c9e7bbf7…`), the mirror image of the case above: two
// adjacent non-schema sections, the first ending inside a `<!X`
// block that the SECOND closes with its `>` line, so the second
// heading stood inside the block. In situ the `<!--` before that
// line was inert inside the block and the tilde fence after it
// masked `## Hidden` into content. Written from a neutral start,
// the `<!--` is a live comment, the fence is dead and `## Hidden`
// is a heading; the second parse split there and carried a second
// `-->`. The splitter now reads a section that stood inside a
// block the way it will be written, so `## Hidden` is a boundary
// on the first parse already (an empty non-schema section, dropped
// as every such section is) and the comment closes before it.
#[test]
fn section_that_stood_inside_the_previous_sections_block_is_read_as_written() {
    let schema = spec_schema();
    let md = "\
---
type: spec
---
# T

## First
<!X

## Second
<!--
>
~~~
## Hidden
";
    let e1 = parse_markdown(md, "html.md", &schema, "specs").unwrap();
    assert_eq!(
        e1.entity.sections["specifies"], "## First\n<!X\n>\n\n## Second\n<!--\n>\n~~~\n-->",
        "the second section ends where a neutral read would split it, and its comment closes there"
    );
    assert_eq!(
        e1.entity.raw_section_headings,
        vec!["First", "Second", "Hidden"],
        "the surfaced line is a boundary on the first parse"
    );
    let m1 = crate::entity::generator::generate_markdown(&e1.entity, &schema);
    let e2 = parse_markdown(&m1, "html.md", &schema, "specs").unwrap();
    let m2 = crate::entity::generator::generate_markdown(&e2.entity, &schema);
    assert_eq!(m1, m2, "parse→generate is a fixpoint");
}

// Fixture pinned by the CI fuzz run of 2026-09-09 (corpus member
// `crash-ce631bf5…`), the sibling with no section before the block:
// the `<!X` block opens in the preamble under the title, so the only
// section's heading stands inside it and no closer between merged
// pieces exists to blame. Same in-situ reading (inert `<!--`, live
// fence, masked `## Hidden`), same neutral reading (live comment,
// dead fence, surfaced heading); the splitter's second pass makes
// the first parse agree with the second.
#[test]
fn section_that_stood_inside_a_preamble_block_is_read_as_written() {
    let schema = spec_schema();
    let md = "\
---
type: spec
---
# T
<!X
## Only
<!--
>
~~~
## Hidden
";
    let e1 = parse_markdown(md, "html.md", &schema, "specs").unwrap();
    assert_eq!(
        e1.entity.sections["specifies"], "## Only\n<!--\n>\n~~~\n-->",
        "the section ends where a neutral read would split it, and its comment closes there"
    );
    let m1 = crate::entity::generator::generate_markdown(&e1.entity, &schema);
    let e2 = parse_markdown(&m1, "html.md", &schema, "specs").unwrap();
    let m2 = crate::entity::generator::generate_markdown(&e2.entity, &schema);
    assert_eq!(m1, m2, "parse→generate is a fixpoint");
}

// Fixture pinned by the coverage-guided long tier (first dispatch,
// 2026-08-24; corpus member `crash-de0c69e0…`): the splitter's full
// content trim promoted an INDENTED heading-lookalike on the first
// content line (` ## Specifies`) to column 0 inside stored content;
// the catch-all re-emit then made the next parse read it as a real
// duplicate section heading, whose first-wins rule dropped the
// content — structure from content, and a broken fixpoint. Leading
// blank lines still drop; the first visible line keeps its
// indentation.
#[test]
fn indented_heading_lookalike_stays_content_and_round_trips() {
    let md = "\
---
type: spec
---
# Promoted Heading

## Identity

Base.

## Unknown Extra

 ## Specifies

Some content that must survive.
";
    let schema = spec_schema();
    let e1 = parse_markdown(md, "indent.md", &schema, "specs").unwrap();
    assert!(
        e1.entity.sections["specifies"].contains(" ## Specifies"),
        "the indented lookalike keeps its indentation inside the catch-all"
    );
    assert!(
        e1.entity.sections["specifies"].contains("Some content that must survive."),
        "content after the lookalike is preserved"
    );
    let m1 = crate::entity::generator::generate_markdown(&e1.entity, &schema);
    let e2 = parse_markdown(&m1, "indent.md", &schema, "specs").unwrap();
    let m2 = crate::entity::generator::generate_markdown(&e2.entity, &schema);
    assert_eq!(
        m1, m2,
        "parse→generate is a fixpoint after one normalising round"
    );
    assert!(
        e2.entity.sections["specifies"].contains("Some content that must survive."),
        "no content is lost across rounds"
    );
}

#[test]
fn compute_hash_deterministic() {
    let hash1 = compute_hash("test content");
    let hash2 = compute_hash("test content");
    assert_eq!(hash1, hash2);
    assert_eq!(hash1.len(), 16);
}

#[test]
fn compute_hash_differs() {
    let hash1 = compute_hash("content a");
    let hash2 = compute_hash("content b");
    assert_ne!(hash1, hash2);
}

#[test]
fn is_float_literal_matches() {
    assert!(is_float_literal("0.85"));
    assert!(is_float_literal("-1.5"));
    assert!(is_float_literal("100.0"));
    assert!(!is_float_literal(".5"));
    assert!(!is_float_literal("1."));
    assert!(!is_float_literal("42"));
    assert!(!is_float_literal("hello"));
}

#[test]
fn is_integer_literal_matches() {
    assert!(is_integer_literal("42"));
    assert!(is_integer_literal("-1"));
    assert!(is_integer_literal("0"));
    assert!(!is_integer_literal("0.5"));
    assert!(!is_integer_literal("hello"));
    assert!(!is_integer_literal(""));
}

// Regression lock for metadata-key order. The parser reads frontmatter
// line-by-line into an IndexMap, so metadata iteration yields the file's
// declared key order. Render sites iterate entity.metadata directly (see
// `render::render_entity_markdown`), so any regression to HashMap
// reintroduces hash-seed-dependent frontmatter ordering in MCP output.
#[test]
fn parse_preserves_frontmatter_key_order() {
    let md = "\
---
type: principle
universality: domain-wide
authority: proposed
tags: a, b, c
created_date: 2026-01-15
last_modified: 2026-04-12
---
# Key Order
";
    let result = parse_markdown(
        md,
        "key-order.md",
        &type_by_name(builtin_names::PRINCIPLE).unwrap(),
        "knowledge",
    )
    .unwrap();
    let keys: Vec<&str> = result.entity.metadata.keys().map(|s| s.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            "type",
            "universality",
            "authority",
            "tags",
            "created_date",
            "last_modified",
        ],
        "metadata iteration must preserve frontmatter declaration order"
    );
}

// Regression lock for section-order round-trip stability. Today this
// passes by construction: the parser inserts keys in schema-declared
// order, the generator writes them in schema-declared order, and
// `IndexMap` preserves that order across re-parses. HashMap iteration
// order was the hole — an IndexMap-based entity.sections closes it.
// Keep the test; if a future refactor reintroduces a HashMap anywhere on
// the parse/write path, this catches it.
#[test]
fn parse_write_roundtrip_preserves_section_order() {
    let md = "\
---
type: spec
created_date: 2026-01-15
last_modified: 2026-04-12
level: M0
---
# Order Roundtrip

## Identity

Identity content.

## Purpose

Purpose content.

## Specifies

Specifies content.
";
    let schema = spec_schema();
    let first = parse_markdown(md, "order-roundtrip.md", &schema, "specs").unwrap();
    let regenerated = crate::entity::generator::generate_markdown(&first.entity, &schema);
    let second = parse_markdown(&regenerated, "order-roundtrip.md", &schema, "specs").unwrap();

    let first_keys: Vec<&String> = first.entity.sections.keys().collect();
    let second_keys: Vec<&String> = second.entity.sections.keys().collect();
    assert_eq!(
        first_keys, second_keys,
        "section iteration order must survive parse -> generate -> parse"
    );
}

// ------------------------------------------------------------------
// Heading-spans extraction (H3–H6)
//
// These lock the parser contract: one extra pass per section that
// records H3+ headings as a side-struct. Flat storage; level skips
// are tolerated; code blocks are ignored. See
// `extract_heading_spans`.
// ------------------------------------------------------------------

#[test]
fn parser_extracts_single_h3() {
    let md = "\
---
type: spec
---
# Entity

## Identity

Body.

## Specifies

### Response Shapes

Content under response shapes.
";
    let result = parse_markdown(md, "h3-single.md", &spec_schema(), "specs").unwrap();
    let spans = result
        .entity
        .heading_spans
        .get("specifies")
        .expect("specifies section should have spans");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].level, 3);
    assert_eq!(spans[0].title, "Response Shapes");
    // The section is trimmed, so the H3 sits at offset 0.
    assert_eq!(spans[0].start_offset, 0);
    let section = result.entity.sections.get("specifies").unwrap();
    assert_eq!(spans[0].end_offset, section.len());
    // Non-specifies sections either get no entry or the content has no H3+ headings.
    assert!(
        result
            .entity
            .heading_spans
            .get("identity")
            .is_none_or(Vec::is_empty)
    );
}

#[test]
fn parser_extracts_nested_h3_h4() {
    let md = "\
---
type: spec
---
# Entity

## Identity

Body.

## Specifies

### Outer

Outer body.

#### Inner

Inner body.
";
    let result = parse_markdown(md, "h3-h4.md", &spec_schema(), "specs").unwrap();
    let spans = result.entity.heading_spans.get("specifies").unwrap();
    assert_eq!(spans.len(), 2, "both H3 and H4 must be recorded");
    assert_eq!(spans[0].level, 3);
    assert_eq!(spans[0].title, "Outer");
    assert_eq!(spans[1].level, 4);
    assert_eq!(spans[1].title, "Inner");
    assert!(
        spans[0].start_offset < spans[1].start_offset,
        "spans must be in document order"
    );
    // H3 contains H4: H3.end_offset must cover H4.start_offset.
    assert!(
        spans[0].end_offset > spans[1].start_offset,
        "outer H3 must contain inner H4 by offset"
    );
}

#[test]
fn parser_ignores_headings_in_code_blocks() {
    let md = "\
---
type: spec
---
# Entity

## Identity

Body.

## Specifies

Prefix.

```
### Not a heading
Still code.
```

Suffix.
";
    let result = parse_markdown(md, "h3-code.md", &spec_schema(), "specs").unwrap();
    let spans = result
        .entity
        .heading_spans
        .get("specifies")
        .cloned()
        .unwrap_or_default();
    assert!(
        spans.is_empty(),
        "a '### ' inside a fenced block must not register as a heading span: {spans:?}"
    );
}

#[test]
fn parser_handles_level_skip() {
    let md = "\
---
type: spec
---
# Entity

## Identity

Body.

## Specifies

#### Skipped To H4

Content under a sudden H4 — no virtual H3 is inserted.
";
    let result = parse_markdown(md, "h2-h4.md", &spec_schema(), "specs").unwrap();
    let spans = result.entity.heading_spans.get("specifies").unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].level, 4);
    assert_eq!(spans[0].title, "Skipped To H4");
}

#[test]
fn parser_handles_duplicate_siblings() {
    let md = "\
---
type: spec
---
# Entity

## Identity

Body.

## Specifies

### Same Title

First occurrence body.

### Same Title

Second occurrence body.
";
    let result = parse_markdown(md, "h3-dup.md", &spec_schema(), "specs").unwrap();
    let spans = result.entity.heading_spans.get("specifies").unwrap();
    assert_eq!(spans.len(), 2, "duplicate siblings must produce two spans");
    assert_eq!(spans[0].title, spans[1].title);
    assert_ne!(
        spans[0].start_offset, spans[1].start_offset,
        "spans with identical titles must be distinguishable by offset"
    );
    // Siblings at the same level: neither contains the other.
    assert!(
        spans[0].end_offset <= spans[1].start_offset,
        "first sibling must close before the second starts"
    );
}

// Duplicate `## Heading` lines for a schema-declared key collapse to the
// first occurrence's body and emit a `DuplicateSectionHeading` warning.
// Catch-all keys absorb arbitrary headings by design and do not warn.

#[test]
fn duplicate_declared_heading_two_populated_keeps_first_warns() {
    let md = "---\ntype: spec\n---\n# Title\n\n## Identity\n\nfirst body\n\n## Identity\n\nsecond body\n";
    let result = parse_markdown(md, "x.md", &spec_schema(), "v").unwrap();
    assert_eq!(
        result.entity.sections.get("identity").map(String::as_str),
        Some("first body"),
        "first body must win"
    );
    assert!(
        !result
            .entity
            .sections
            .get("identity")
            .unwrap()
            .contains("## Identity"),
        "storage value must not embed a duplicate heading"
    );
    assert_eq!(result.parse_warnings.len(), 1);
    match &result.parse_warnings[0] {
        crate::ops::WarningHint::DuplicateSectionHeading {
            section_key,
            heading,
            occurrences,
            ..
        } => {
            assert_eq!(section_key, "identity");
            assert_eq!(heading, "Identity");
            assert_eq!(*occurrences, 2);
        }
        other => panic!("expected DuplicateSectionHeading, got {other:?}"),
    }
}

#[test]
fn duplicate_declared_heading_blank_then_populated_keeps_blank() {
    // First-wins is mechanical: a blank first occurrence wins over a
    // populated second one. The warning surfaces so the operator
    // notices content was discarded.
    let md = "---\ntype: spec\n---\n# Title\n\n## Identity\n\n## Identity\n\nleftover content\n";
    let result = parse_markdown(md, "x.md", &spec_schema(), "v").unwrap();
    assert_eq!(
        result.entity.sections.get("identity").map(String::as_str),
        Some(""),
        "first (blank) occurrence wins; second body is dropped"
    );
    assert_eq!(result.parse_warnings.len(), 1);
}

#[test]
fn duplicate_declared_heading_three_occurrences() {
    let md = "---\ntype: spec\n---\n# Title\n\n## Constraints\n\nA\n\n## Constraints\n\n## Constraints\n\nC\n";
    let result = parse_markdown(md, "x.md", &spec_schema(), "v").unwrap();
    assert_eq!(
        result
            .entity
            .sections
            .get("constraints")
            .map(String::as_str),
        Some("A"),
    );
    assert_eq!(result.parse_warnings.len(), 1);
    match &result.parse_warnings[0] {
        crate::ops::WarningHint::DuplicateSectionHeading { occurrences, .. } => {
            assert_eq!(*occurrences, 3);
        }
        _ => unreachable!(),
    }
}

#[test]
fn no_warning_when_each_declared_section_appears_once() {
    let md = "---\ntype: spec\n---\n# Title\n\n## Identity\n\nID\n\n## Purpose\n\nP\n\n## Constraints\n\nC\n";
    let result = parse_markdown(md, "x.md", &spec_schema(), "v").unwrap();
    assert!(result.parse_warnings.is_empty());
}

#[test]
fn no_warning_when_catch_all_section_repeats() {
    // `specifies` is the spec schema's catch-all section. Repetition
    // there is silent — duplicates only warn for non-catch-all keys.
    let md = "---\ntype: spec\n---\n# Title\n\n## Specifies\n\nfirst\n\n## Specifies\n\nsecond\n";
    let result = parse_markdown(md, "x.md", &spec_schema(), "v").unwrap();
    assert!(
        result.parse_warnings.is_empty(),
        "catch-all repetition must not warn"
    );
}

// Three `## Realization` headings on a spec entity. The default-schema
// `spec` does not declare `realization`, so it flows to the catch-all
// `specifies` bucket and emits no warning, but the storage must still
// not concatenate duplicate heading bytes — that was the bug being
// fixed. Workspaces that declare `realization` (e.g. `software@0.1.0`)
// additionally surface a `DuplicateSectionHeading` warning.
#[test]
fn duplicate_realization_does_not_concatenate_headers_in_storage() {
    let md = "---\ntype: spec\n---\n# Title\n\n## Identity\n\nID\n\n## Realization\n\n- a.mjs\n- b.mjs\n\n## Realization\n\n## Realization\n\n- c.mjs\n\n## Constraints\n\nC\n";
    let result = parse_markdown(md, "x.md", &spec_schema(), "v").unwrap();
    let catch_all = result.entity.sections.get("specifies").unwrap();
    let header_count = catch_all.matches("## Realization").count();
    assert!(
        header_count <= 1,
        "catch-all bucket must not contain multiple `## Realization` headers — got {header_count}: {catch_all:?}"
    );
}

// After a parse → render round-trip, an entity that was loaded from a
// markdown file with three `## Identity` headings emits exactly one
// `## Identity` heading on re-render. This is the self-heal contract:
// the next read-modify-write of a duplicate-heading entity collapses
// the markdown to one heading per declared section.
#[test]
fn parse_render_round_trip_collapses_duplicate_headings() {
    let md = "---\ntype: spec\n---\n# Title\n\n## Identity\n\nA\n\n## Identity\n\n## Identity\n\nC\n\n## Purpose\n\nP\n";
    let result = parse_markdown(md, "x.md", &spec_schema(), "v").unwrap();
    let rendered = crate::render::render_entity_markdown(&result.entity, None);
    let identity_count = rendered.matches("## Identity").count();
    assert_eq!(
        identity_count, 1,
        "rendered output must carry exactly one `## Identity`, got {identity_count}: {rendered}"
    );
    // First-wins: the rendered Identity body is `A`, not `C`.
    assert!(rendered.contains("\n## Identity\n\nA\n"));
    assert!(!rendered.contains("C\n"), "second body must not survive");
}
