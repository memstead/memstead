#![cfg(test)]

use super::*;
use memstead_schema::{builtin_names, type_by_name};
use std::sync::Arc;

fn spec_schema() -> Arc<TypeDefinition> {
    type_by_name(builtin_names::SPEC).unwrap()
}

/// Wrap a `## Specifies` body in a minimal, valid spec entity.
fn entity_with_specifies(body: &str) -> ParseResult {
    let md = format!(
        "---\ntype: spec\n---\n\n# Referee Test\n\n## Identity\n\nx\n\n## Specifies\n\n{body}\n"
    );
    parse_markdown(&md, "referee-test.md", &spec_schema(), "specs").unwrap()
}

fn headings(result: &ParseResult) -> Vec<&str> {
    result
        .entity
        .raw_section_headings
        .iter()
        .map(String::as_str)
        .collect()
}

fn link_targets(result: &ParseResult) -> Vec<String> {
    result.inline_links.iter().map(|id| id.0.clone()).collect()
}

/// The complement first: without it, every assertion below could
/// pass because the paths do nothing at all.
#[test]
fn complement_prose_headings_and_links_still_work() {
    let r = entity_with_specifies("See [[real-target]] here.");
    assert_eq!(headings(&r), vec!["Identity", "Specifies"]);
    assert_eq!(link_targets(&r), vec!["specs--real-target".to_string()]);
    assert_eq!(r.entity.title, "Referee Test");
}

#[test]
fn class_1_indented_code_block() {
    let r = entity_with_specifies("Example:\n\n    ## Not A Section\n    [[not-a-link]]\n");
    assert_eq!(headings(&r), vec!["Identity", "Specifies"]);
    assert!(link_targets(&r).is_empty(), "{:?}", link_targets(&r));
}

#[test]
fn class_2_fence_indented_one_to_three_spaces() {
    let r =
        entity_with_specifies("- item\n\n   ```\n   ## Not A Section\n   [[not-a-link]]\n   ```\n");
    assert_eq!(headings(&r), vec!["Identity", "Specifies"]);
    assert!(link_targets(&r).is_empty(), "{:?}", link_targets(&r));
}

#[test]
fn class_3_tilde_fence() {
    let r = entity_with_specifies("~~~\n## Not A Section\n[[not-a-link]]\n~~~\n");
    assert_eq!(headings(&r), vec!["Identity", "Specifies"]);
    assert!(link_targets(&r).is_empty(), "{:?}", link_targets(&r));
}

#[test]
fn class_4_info_string_on_the_closing_line() {
    let r =
        entity_with_specifies("```\ncode\n``` still-code\n## Not A Section\n[[not-a-link]]\n```\n");
    assert_eq!(headings(&r), vec!["Identity", "Specifies"]);
    assert!(link_targets(&r).is_empty(), "{:?}", link_targets(&r));
}

#[test]
fn class_5_fence_inside_a_blockquote() {
    let r = entity_with_specifies("> ```\n> ## Not A Section\n> [[not-a-link]]\n> ```\n");
    assert_eq!(headings(&r), vec!["Identity", "Specifies"]);
    assert!(link_targets(&r).is_empty(), "{:?}", link_targets(&r));
}

#[test]
fn class_6_opening_fence_length_is_honoured_on_close() {
    let r = entity_with_specifies("````\n```\n## Not A Section\n[[not-a-link]]\n```\n````\n");
    assert_eq!(headings(&r), vec!["Identity", "Specifies"]);
    assert!(link_targets(&r).is_empty(), "{:?}", link_targets(&r));
}

/// The title extractor was the one scanner with no masking at all.
#[test]
fn a_heading_inside_a_code_block_never_becomes_the_title() {
    let md = "---\ntype: spec\n---\n\n```\n# Fake Title\n```\n\n# Real Title\n\n## Identity\n\nx\n";
    let r = parse_markdown(md, "title-test.md", &spec_schema(), "specs").unwrap();
    assert_eq!(r.entity.title, "Real Title");
}

/// …and when the code block is the whole body, the filename
/// fallback applies rather than a title mined out of code.
#[test]
fn a_code_block_only_body_falls_back_to_the_filename() {
    let md = "---\ntype: spec\n---\n\n    # Fake Title\n\n## Identity\n\nx\n";
    let r = parse_markdown(md, "fallback-test.md", &spec_schema(), "specs").unwrap();
    assert_eq!(r.entity.title, "fallback-test");
}

#[test]
fn heading_spans_ignore_code_block_content() {
    let r = entity_with_specifies("### Real Sub\n\n~~~\n### Fake Sub\n~~~\n");
    let spans = r.entity.heading_spans.get("specifies").expect("spans");
    let titles: Vec<&str> = spans.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, vec!["Real Sub"]);
}

/// One definition of "not visible to a link scanner": a link the
/// strict validator cannot see is a link no path turns into an
/// edge. Multi-backtick spans are the case a delimiter-count regex
/// slices through.
#[test]
fn inline_code_spans_hide_links_on_the_extraction_path() {
    let r = entity_with_specifies("`[[hidden-one]]` and ``[[hidden-two]]`` but [[visible]].");
    assert_eq!(link_targets(&r), vec!["specs--visible".to_string()]);
}

/// The empty-target asymmetry: the strict extractor now *sees*
/// `[[]]` and routes it to the same refusal the validator emits,
/// instead of a pattern that could not match it at all.
#[test]
fn empty_wiki_link_target_is_refused_by_the_strict_extractor() {
    let errors =
        extract_inline_links("an empty [[]] link", "specs").expect_err("empty target must refuse");
    assert_eq!(errors.len(), 1, "{errors:?}");
}

/// The read side tolerates drift by ignoring what it cannot
/// decode — but it sees the same token the strict side refuses.
#[test]
fn empty_wiki_link_target_yields_no_id_on_the_lenient_path() {
    assert!(extract_inline_links_lenient("an empty [[]] link", "specs").is_empty());
}

/// A conflicted file must be refused however its frontmatter is
/// shaped. Masking the whole file let a YAML value that reads as a
/// fence opener blank the body — markers included — so the file
/// loaded as a normal entity with BOTH merge sides fused into one
/// body, which is the exact outcome the guard exists to prevent.
#[test]
fn merge_conflict_markers_are_seen_through_fence_shaped_frontmatter() {
    let body = "\n# T\n\n## Identity\n\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\n";
    for fm in [
        "---\ntype: spec\n---",
        "---\ntype: spec\nnotes: |\n  ```rust\n  fn x() {}\n---",
        "---\ntype: spec\nnotes: |\n   ~~~\n---",
        "---\ntype: spec\nnotes: |\n    indented block\n---",
    ] {
        assert!(
            has_merge_conflict_markers(&format!("{fm}{body}")),
            "conflict markers must be seen through frontmatter: {fm:?}"
        );
    }
}

/// …and markers in the frontmatter itself count: git writes them
/// wherever the hunks fall, including above the `---`.
#[test]
fn merge_conflict_markers_in_frontmatter_are_seen() {
    let content =
        "---\n<<<<<<< HEAD\ntype: spec\n=======\ntype: memo\n>>>>>>> branch\n---\n\n# T\n";
    assert!(has_merge_conflict_markers(content));
}

/// Complement: a fenced code example documenting conflict markers
/// in a section body still does not trip the guard.
#[test]
fn a_fenced_conflict_marker_example_still_does_not_trip_the_guard() {
    let content = "---\ntype: spec\n---\n\n# T\n\n## Identity\n\n```\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\n```\n";
    assert!(!has_merge_conflict_markers(content));
}

/// A relationship row inside a code block is an example of the
/// syntax, not a relationship. It used to become a live edge and an
/// auto-stub while the strict validator — which masks — could not
/// see the link at all: one path synthesising an edge from what
/// another path refuses to see.
#[test]
fn a_relationship_row_inside_a_code_block_is_not_a_relationship() {
    for body in [
        "```\n- **REFERENCES**: [[ghost]]\n```",
        "~~~\n- **REFERENCES**: [[ghost]]\n~~~",
        "    - **REFERENCES**: [[ghost]]",
        "> ```\n> - **REFERENCES**: [[ghost]]\n> ```",
        "````\n```\n- **REFERENCES**: [[ghost]]\n```\n````",
    ] {
        let (rels, _) = parse_relationships_with_warnings(body, "specs", None);
        assert!(
            rels.is_empty(),
            "code-block row must not become an edge: {body:?} -> {rels:?}"
        );
    }
}

/// …and a row hidden inside an INLINE CODE SPAN is not one either.
/// A blocks-only mask left this row invisible to the strict
/// validator and to every link extractor — both of which mask
/// spans — while still building an edge and a stub from it. A
/// multi-line span is the shape that bites: a lazy paragraph
/// continuation keeps the backticks open across the row.
#[test]
fn a_relationship_row_inside_an_inline_code_span_is_not_a_relationship() {
    for body in [
        // The row is indented, so it does not interrupt the
        // paragraph as a list — it is a lazy continuation and the
        // backtick pair holds the span open across all three lines.
        // (At column 0 a `-` DOES start a list, the span never
        // forms, and the row is a real relationship — correctly.)
        "Example `open\n    - **REFERENCES**: [[ghost]]\nclose`",
        "A `- **REFERENCES**: [[ghost]]` sample.",
        "A ``- **REFERENCES**: [[ghost]]`` sample.",
    ] {
        let (rels, _) = parse_relationships_with_warnings(body, "specs", None);
        assert!(
            rels.is_empty(),
            "code-span row must not become an edge: {body:?} -> {rels:?}"
        );
    }
}

/// Complement: a real row still parses, keeps its type, target and
/// em-dash description, and still warns on an ambiguous delimiter —
/// every captured span is read from the original, not the mask.
#[test]
fn real_relationship_rows_are_unchanged_by_the_mask() {
    let body = "- **REFERENCES**: [[alpha]]\n- **uses**: [[beta]] — because it must\n\n```\n- **REFERENCES**: [[ghost]]\n```\n";
    let (rels, _) = parse_relationships_with_warnings(body, "specs", None);
    assert_eq!(rels.len(), 2, "{rels:?}");
    assert_eq!(rels[0].rel_type, "REFERENCES");
    assert_eq!(rels[0].target.0, "specs--alpha");
    assert_eq!(rels[0].description, None);
    assert_eq!(
        rels[1].rel_type, "USES",
        "case is normalised from the original"
    );
    assert_eq!(rels[1].target.0, "specs--beta");
    assert_eq!(rels[1].description.as_deref(), Some("because it must"));
}

#[test]
fn ambiguous_delimiter_warning_still_fires_on_a_real_row() {
    let id = file_path_to_id("x.md", "specs");
    let (_, warnings) = parse_relationships_with_warnings(
        "- **REFERENCES**: [[alpha]] -- not an em dash\n",
        "specs",
        Some(&id),
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}

/// Frontmatter is not markdown. Masking the whole file would hand
/// a CommonMark parser YAML it can read as block structure: a
/// value line that looks like a fence opener (legal at 1–3 spaces
/// since the indented-fence class was fixed) would open a code
/// block that runs past the closing `---` and mask the entire
/// body — no title, no sections, no links. The split happens
/// first; only the body is masked.
#[test]
fn frontmatter_never_opens_a_code_block_over_the_body() {
    for fm in [
        "notes: |\n  ```rust",
        "notes: |\n   ~~~",
        "notes: |\n  ```\n  still open",
        "notes: |\n    indented block\n",
    ] {
        let md = format!(
            "---\ntype: spec\n{fm}\n---\n\n# Real Title\n\n## Identity\n\nSee [[a-link]].\n"
        );
        let r = parse_markdown(&md, "fm-test.md", &spec_schema(), "specs").unwrap();
        assert_eq!(
            r.entity.title, "Real Title",
            "frontmatter ate the title: {fm:?}"
        );
        assert_eq!(
            headings(&r),
            vec!["Identity"],
            "frontmatter ate the sections: {fm:?}"
        );
        assert_eq!(
            link_targets(&r),
            vec!["specs--a-link".to_string()],
            "frontmatter ate the links: {fm:?}"
        );
    }
}
