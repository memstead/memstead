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
//!
//! # Give these functions a BODY, never a whole entity file
//!
//! Frontmatter is not markdown. Handing it to a CommonMark parser
//! invents block structure that is not there: a YAML value that reads
//! as a fence opener — legal at 1–3 spaces, and honoured here since
//! indented fences were fixed — opens a code block that runs past the
//! `---` terminator to end of file and blanks the entire body. Every
//! `## ` heading, every `[[link]]`, and every git conflict marker in
//! that file becomes invisible to whatever scans the result.
//!
//! Callers holding a section body are already safe — section bodies
//! are frontmatter-free by construction. A caller holding a raw file
//! or a git blob is not, and must trim it first with
//! [`crate::entity::parser::body_after_frontmatter`]. This bit the
//! engine three times during the migration that introduced these
//! masks: in `parse_markdown`, in the git-branch ripple scanner, and
//! in the merge-conflict guard — the last one silently defeating a
//! data-integrity check. Any new caller is the fourth unless it trims.

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
///
/// **Pass a body, not a whole entity file** — see the module header.
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
///
/// **Pass a body, not a whole entity file** — see the module header.
pub fn mask_code_blocks_and_spans(text: &str) -> String {
    mask_ranges(text, &code_ranges(text, true))
}

/// When `text` — a section body — ends inside an unterminated fenced
/// code block that would swallow whatever the caller writes after it,
/// return the closing fence that terminates it. `None` when the text is
/// safely self-delimiting: balanced fences, indented code (a column-0
/// heading line ends it), or a fence inside a container a following
/// column-0 line closes implicitly (blockquote, list item).
///
/// The referee itself is the oracle — no second fence model exists here
/// to drift from the parser's. A probe line is appended and the mask
/// consulted: if the probe comes back masked, the open block would
/// swallow following content; the candidate closer (same char, the
/// opening fence's length) is then verified the same way.
///
/// The caller that needs this is the entity generator: a section whose
/// stored content ends inside an open fence would otherwise absorb every
/// section heading the generator writes after it on the next parse — a
/// document that grows and shifts content between sections on every
/// parse→generate round.
pub fn closing_fence_if_unterminated(text: &str) -> Option<String> {
    // Fast path: no run of three fence characters, nothing to leave open.
    if !text.contains("```") && !text.contains("~~~") {
        return None;
    }
    const PROBE: &str = "memstead-fence-probe";
    // `ends_with`, not `contains`: the probe is appended as the last
    // line, so only its own (un)masked state is consulted — a prose
    // occurrence of the probe string elsewhere cannot fake a pass.
    let survives = |t: &str| mask_code_blocks(&format!("{t}\n{PROBE}")).ends_with(PROBE);
    if survives(text) {
        return None;
    }
    // The open block is the one whose range reaches end of text; its
    // first line is the opening fence. Containers (blockquote, list)
    // never reach here — their fences close implicitly at the probe's
    // column-0 line, so the probe survives above.
    let ranges = code_ranges(text, false);
    let open = ranges.iter().rfind(|r| r.end >= text.len())?;
    let first_line = text[open.start..].lines().next().unwrap_or("");
    let fence = first_line.trim_start();
    let ch = fence.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let count = fence.chars().take_while(|c| *c == ch).count().max(3);
    let cand = ch.to_string().repeat(count);
    if survives(&format!("{text}\n{cand}")) {
        Some(cand)
    } else {
        None
    }
}

/// When `text` ends inside an open HTML block of a kind no blank line
/// ends (CommonMark types 1–5: a `<script>`/`<pre>`/`<style>`/`<textarea>`
/// tag, `<!--`, `<?`, `<!` + letter, `<![CDATA[`), return the line that
/// terminates it. `None` when the trailing context is neutral: no such
/// block open, or a type 6/7 block that the caller's blank-line join
/// already ends.
///
/// Inside such a block the referee reads no fence at all, so whatever a
/// caller appends after `text` is parsed differently than it was where
/// it came from: a fence opener that hid a `## ` line in situ is plain
/// text after the append, the line becomes a heading, and the document
/// shifts structure on every parse→generate round. Same oracle discipline
/// as [`closing_fence_if_unterminated`]: the referee decides via a probe
/// (a fence-plus-marker appended after a blank line must come back
/// masked), the candidate closer is derived from the open block's own
/// start condition, and the probe verifies it. A `text` that ends inside
/// an open fence is the fence oracle's case, not this one: it returns
/// `None`, so callers run the fence close first.
pub fn closing_html_block_if_unterminated(text: &str) -> Option<String> {
    if !text.contains('<') {
        return None;
    }
    const PROBE: &str = "memstead-html-probe";
    // A fence appended after a blank line must mask the marker; if the
    // marker survives, the trailing context hides fences.
    let fences_work =
        |t: &str| !mask_code_blocks(&format!("{t}\n\n```\n{PROBE}\n```")).contains(PROBE);
    if fences_work(text) {
        return None;
    }
    // Inside an open fence the probe's own fence line closes it and the
    // marker survives too — that case belongs to the fence oracle.
    if closing_fence_if_unterminated(text).is_some() {
        return None;
    }
    // The open block is the last HTML block whose range reaches the end
    // of the text; its first line carries the start condition.
    let open = Parser::new_ext(text, parser_options())
        .into_offset_iter()
        .filter(|(event, _)| matches!(event, Event::Start(Tag::HtmlBlock)))
        .map(|(_, range)| range)
        .filter(|r| r.end >= text.trim_end().len())
        .last()?;
    let first_line = text[open.start..].lines().next().unwrap_or("").trim_start();
    let lower = first_line.to_ascii_lowercase();
    let closer = if lower.starts_with("<!--") {
        "-->".to_string()
    } else if lower.starts_with("<?") {
        "?>".to_string()
    } else if lower.starts_with("<![cdata[") {
        "]]>".to_string()
    } else if lower.starts_with("<!") {
        ">".to_string()
    } else {
        let tag = ["script", "pre", "style", "textarea"]
            .into_iter()
            .find(|tag| {
                lower
                    .strip_prefix('<')
                    .and_then(|rest| rest.strip_prefix(tag))
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t', '>']))
            })?;
        format!("</{tag}>")
    };
    if fences_work(&format!("{text}\n{closer}")) {
        Some(closer)
    } else {
        None
    }
}

/// The one question both concatenation sites ask: does `text` end inside
/// a block context that would change how the next appended piece is
/// read? A fence first (it swallows headings), then an HTML block (it
/// hides fences); never both, since neither can open inside the other.
pub fn closing_context_if_unterminated(text: &str) -> Option<String> {
    closing_fence_if_unterminated(text).or_else(|| closing_html_block_if_unterminated(text))
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

    // --- open-fence termination helper -----------------------------

    #[test]
    fn unterminated_backtick_fence_yields_matching_closer() {
        assert_eq!(
            closing_fence_if_unterminated("```\ncode with no closer"),
            Some("```".to_string())
        );
        // The closer must honour the opening fence's length (class 6).
        assert_eq!(
            closing_fence_if_unterminated("````\n```\nstill inside"),
            Some("````".to_string())
        );
        assert_eq!(
            closing_fence_if_unterminated("~~~\ntilde block"),
            Some("~~~".to_string())
        );
    }

    #[test]
    fn balanced_and_container_fences_need_no_closer() {
        assert_eq!(closing_fence_if_unterminated("```\ncode\n```"), None);
        assert_eq!(closing_fence_if_unterminated("no fences at all"), None);
        // A blockquote fence closes implicitly when the quote ends at the
        // next column-0 line — nothing bleeds, nothing to terminate.
        assert_eq!(closing_fence_if_unterminated("> ```\n> quoted"), None);
        // Indented code ends at any column-0 line.
        assert_eq!(closing_fence_if_unterminated("text:\n\n    code"), None);
    }

    #[test]
    fn block_mask_does_not_double_count_inline_spans_inside_blocks() {
        let input = "```\nlet s = `x`;\n```\nafter `y`.\n";
        let masked = mask_all(input);
        assert!(!masked.contains('`'));
        assert!(masked.contains("after"));
    }

    #[test]
    fn unterminated_html_block_yields_its_own_closer() {
        // Type 4: `<!` + letter, ended only by a line containing `>`.
        assert_eq!(
            closing_html_block_if_unterminated("<!S**: [[-----\nmore"),
            Some(">".to_string())
        );
        // Type 2: a comment.
        assert_eq!(
            closing_html_block_if_unterminated("<!-- draft"),
            Some("-->".to_string())
        );
        // Type 3: a processing instruction.
        assert_eq!(
            closing_html_block_if_unterminated("<?php"),
            Some("?>".to_string())
        );
        // Type 5: CDATA.
        assert_eq!(
            closing_html_block_if_unterminated("<![CDATA[ raw"),
            Some("]]>".to_string())
        );
        // Type 1: the raw-text tags, case-insensitive, closed by their own tag.
        assert_eq!(
            closing_html_block_if_unterminated("<Script>\nlet x;"),
            Some("</script>".to_string())
        );
        assert_eq!(
            closing_html_block_if_unterminated("<pre class=\"x\">\ntext"),
            Some("</pre>".to_string())
        );
        // The combined oracle picks the same line.
        assert_eq!(
            closing_context_if_unterminated("<!-- draft"),
            Some("-->".to_string())
        );
    }

    #[test]
    fn closed_and_blank_line_ended_html_blocks_need_no_closer() {
        assert_eq!(closing_html_block_if_unterminated("<!-- a -->\ntext"), None);
        assert_eq!(
            closing_html_block_if_unterminated("<!S open\nclosed here >"),
            None
        );
        // Type 6/7 end at the blank line every caller joins with.
        assert_eq!(
            closing_html_block_if_unterminated("<div>\nstill html"),
            None
        );
        assert_eq!(
            closing_html_block_if_unterminated("<span>inline</span> prose"),
            None
        );
        assert_eq!(closing_html_block_if_unterminated("no markup"), None);
        // An open fence is the fence oracle's case; the combined oracle
        // returns the fence closer, not an HTML one.
        assert_eq!(
            closing_html_block_if_unterminated("```\n<!-- inside code"),
            None
        );
        assert_eq!(
            closing_context_if_unterminated("```\n<!-- inside code"),
            Some("```".to_string())
        );
    }
}
