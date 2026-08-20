//! The CommonMark referee — one definition of "code" for every content
//! reader in the engine.
//!
//! The engine used to carry two referees for the same markdown. Section
//! splitting and wiki-link scanning ran on a hand-rolled line scanner
//! that recognised exactly one shape of code block (a column-0 backtick
//! fence, closed by any backtick-prefixed line); section *content*
//! validation ([`crate::section_format`], agent-toolbox plan 08) ran on
//! `pulldown-cmark`. They disagreed in six verified ways, and the
//! disagreement sat on the write path — the validator judged content the
//! splitter had already mis-partitioned:
//!
//! 1. indented code blocks were not masked at all;
//! 2. legally indented fences (1–3 spaces — the normal shape inside a
//!    list item) did not open a block;
//! 3. tilde fences were unhandled;
//! 4. a closing line carrying an info string — content, per CommonMark —
//!    closed the block early;
//! 5. fences inside blockquotes were not masked;
//! 6. the opening fence was stored but never compared on close, so a
//!    ```` ```` ````-fenced block ended on the first ```` ``` ````.
//!
//! `section_format`'s header states the thesis this module generalises:
//! a reader that disagrees with the renderer every agent uses sends
//! repair loops that cannot converge. **The parser is the referee.**
//!
//! Both masks preserve byte offsets and line counts exactly — every
//! masked byte becomes an ASCII space except `\n` and `\r`, so a caller
//! may scan the masked copy and slice the original by the offsets it
//! finds. That is the whole mechanism: boundaries come from the parser,
//! bytes come from the original.
//!
//! Heading recognition is deliberately *not* widened here. A section is
//! still a column-0 ATX `## ` line and nothing else — setext headings
//! and indented ATX create sections nowhere. This module fixes what code
//! blocks hide; it does not change what counts as a heading.

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

/// The engine's CommonMark dialect — one `Options` for every reader, so
/// the block model the masks see is the block model
/// [`crate::section_format`] checks against.
pub fn parser_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options
}

/// Replace every byte of each range with a space, keeping `\n` and `\r`
/// so line counts and byte offsets survive.
///
/// Parser ranges are always on char boundaries and every replacement is
/// ASCII, so the result is valid UTF-8 of exactly the input's length.
fn mask_ranges(text: &str, ranges: &[Range<usize>]) -> String {
    let mut bytes = text.as_bytes().to_vec();
    for range in ranges {
        let end = range.end.min(bytes.len());
        let start = range.start.min(end);
        for b in &mut bytes[start..end] {
            if *b != b'\n' && *b != b'\r' {
                *b = b' ';
            }
        }
    }
    // Safe by construction: every replaced byte was inside a
    // char-boundary-aligned range and became ASCII.
    String::from_utf8(bytes).unwrap_or_else(|_| text.to_string())
}

/// Source ranges of every CommonMark code block (fenced — backtick or
/// tilde, at any legal indent, inside any container — and indented) and,
/// when `spans` is set, every inline code span.
fn code_ranges(text: &str, spans: bool) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    // Depth of open code blocks: nested containers never nest code
    // blocks, but the offset iterator hands us the whole block on
    // `Start`, so inline events inside it are already covered.
    let mut block_end = 0usize;
    for (event, range) in Parser::new_ext(text, parser_options()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => {
                block_end = block_end.max(range.end);
                out.push(range);
            }
            Event::Code(_) if spans && range.start >= block_end => out.push(range),
            _ => {}
        }
    }
    out
}

/// Mask every CommonMark code block, preserving byte offsets and line
/// count.
///
/// Content inside a masked block is whitespace to every line scanner
/// that runs over the result — it can never open a section, register a
/// heading, become a title, or yield a wiki-link.
pub fn mask_code_blocks(text: &str) -> String {
    mask_ranges(text, &code_ranges(text, false))
}

/// Mask every CommonMark code block *and* every inline code span,
/// preserving byte offsets and line count.
///
/// This is the single definition of "not visible to a link scanner".
/// Extraction, rewriting, and strict validation all use it, so a link
/// one path cannot see is a link no other path synthesises an edge
/// from. Multi-backtick delimiters (`` `` ``, `` ``` ``) are handled by
/// the parser, not by a delimiter-count regex.
pub fn mask_code_blocks_and_spans(text: &str) -> String {
    mask_ranges(text, &code_ranges(text, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every mask must be a byte-for-byte length match with the same
    /// newline positions — callers slice the original by masked offsets.
    fn assert_offset_preserving(input: &str, masked: &str) {
        assert_eq!(input.len(), masked.len(), "byte length must be preserved");
        assert_eq!(
            input.match_indices('\n').collect::<Vec<_>>(),
            masked.match_indices('\n').collect::<Vec<_>>(),
            "newline positions must be preserved"
        );
    }

    fn mask(input: &str) -> String {
        let masked = mask_code_blocks(input);
        assert_offset_preserving(input, &masked);
        masked
    }

    fn mask_all(input: &str) -> String {
        let masked = mask_code_blocks_and_spans(input);
        assert_offset_preserving(input, &masked);
        masked
    }

    // --- the six verified misparse classes -------------------------

    /// Class 1: indented code blocks were not masked at all.
    #[test]
    fn class_1_indented_code_block_is_masked() {
        let input = "Text:\n\n    ## Not A Heading\n    [[not-a-link]]\n\nAfter\n";
        let masked = mask(input);
        assert!(!masked.contains("## Not A Heading"));
        assert!(!masked.contains("[[not-a-link]]"));
        assert!(masked.contains("After"));
    }

    /// Class 2: a fence indented 1–3 spaces (the normal shape inside a
    /// list item) is a legal fence and must open a block.
    #[test]
    fn class_2_indented_fence_opens_a_block() {
        let input = "- item\n\n   ```\n   ## Not A Heading\n   ```\n\nAfter\n";
        let masked = mask(input);
        assert!(!masked.contains("## Not A Heading"));
        assert!(masked.contains("After"));
    }

    /// Class 3: tilde fences were unhandled.
    #[test]
    fn class_3_tilde_fence_is_masked() {
        let input = "~~~\n## Not A Heading\n[[not-a-link]]\n~~~\n\nAfter\n";
        let masked = mask(input);
        assert!(!masked.contains("## Not A Heading"));
        assert!(!masked.contains("[[not-a-link]]"));
        assert!(masked.contains("After"));
    }

    /// Class 4: a line that looks like a closer but carries an info
    /// string is content, not a closer — the block runs on.
    #[test]
    fn class_4_info_string_on_a_closing_line_does_not_close() {
        let input = "```\ncode\n``` not-a-closer\n## Not A Heading\n```\n\nAfter\n";
        let masked = mask(input);
        assert!(!masked.contains("## Not A Heading"));
        assert!(masked.contains("After"));
    }

    /// Class 5: fences inside blockquotes were not masked.
    #[test]
    fn class_5_fence_inside_a_blockquote_is_masked() {
        let input = "> ```\n> ## Not A Heading\n> [[not-a-link]]\n> ```\n\nAfter\n";
        let masked = mask(input);
        assert!(!masked.contains("## Not A Heading"));
        assert!(!masked.contains("[[not-a-link]]"));
        assert!(masked.contains("After"));
    }

    /// Class 6: the opening fence's length must be honoured on close —
    /// a four-backtick block does not end on a three-backtick line.
    #[test]
    fn class_6_longer_fence_is_not_closed_by_a_shorter_one() {
        let input = "````\n```\n## Not A Heading\n```\n````\n\nAfter\n";
        let masked = mask(input);
        assert!(!masked.contains("## Not A Heading"));
        assert!(masked.contains("After"));
    }

    // --- the complement: nothing outside the classes changes -------

    #[test]
    fn prose_heading_and_links_survive() {
        let input = "# Title\n\n## Section\n\nSee [[other-entity]] and [[a|b]].\n";
        let masked = mask(input);
        assert_eq!(masked, input);
    }

    #[test]
    fn fenced_block_masking_matches_the_old_bare_fence_behaviour() {
        let input = "before\n```rust\nfn f() {}\n```\nafter\n";
        let masked = mask(input);
        assert!(masked.starts_with("before\n"));
        assert!(masked.ends_with("after\n"));
        assert!(!masked.contains("fn f()"));
        assert!(!masked.contains("```"));
    }

    #[test]
    fn unclosed_fence_masks_to_end_of_text() {
        let input = "before\n```\n## Not A Heading\nstill inside\n";
        let masked = mask(input);
        assert!(masked.starts_with("before\n"));
        assert!(!masked.contains("## Not A Heading"));
        assert!(!masked.contains("still inside"));
    }

    #[test]
    fn multibyte_content_masks_without_corruption() {
        let input = "```\nGrüße — ünïcødé ✓\n```\nafter ✓\n";
        let masked = mask(input);
        assert!(!masked.contains("Grüße"));
        assert!(masked.contains("after ✓"));
    }

    #[test]
    fn crlf_line_endings_survive_masking() {
        let input = "before\r\n```\r\n## Not A Heading\r\n```\r\nafter\r\n";
        let masked = mask(input);
        assert!(!masked.contains("## Not A Heading"));
        assert!(masked.contains("after"));
        assert_eq!(input.matches('\r').count(), masked.matches('\r').count());
    }

    // --- inline spans ----------------------------------------------

    #[test]
    fn inline_span_hides_a_link() {
        let input = "See `[[not-a-link]]` but [[real-link]].\n";
        let masked = mask_all(input);
        assert!(!masked.contains("[[not-a-link]]"));
        assert!(masked.contains("[[real-link]]"));
    }

    #[test]
    fn double_backtick_span_hides_a_link() {
        let input = "See `` [[not-a-link]] `` but [[real-link]].\n";
        let masked = mask_all(input);
        assert!(!masked.contains("[[not-a-link]]"));
        assert!(masked.contains("[[real-link]]"));
    }

    /// A single-backtick regex slices into the middle of a
    /// double-backtick span and leaves `` ` ``/`[[` remnants behind;
    /// the parser does not.
    #[test]
    fn backtick_inside_a_double_backtick_span_leaves_no_remnant() {
        let input = "Literal ``a ` b`` then [[real-link]].\n";
        let masked = mask_all(input);
        assert!(!masked.contains('`'));
        assert!(masked.contains("[[real-link]]"));
    }

    #[test]
    fn block_mask_does_not_double_count_inline_spans_inside_blocks() {
        let input = "```\nlet s = `x`;\n```\nafter `y`.\n";
        let masked = mask_all(input);
        assert!(!masked.contains('`'));
        assert!(masked.contains("after"));
    }
}
