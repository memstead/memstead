//! Section-body reduction — the markdown half of the section-format
//! vocabulary (agent-toolbox plan 08).
//!
//! Reduces a section body to its **top-level block sequence** with a
//! real CommonMark parser (`pulldown-cmark`, no default features,
//! GFM tables via the runtime `Options` flag). A line-scanner
//! disagrees with CommonMark on exactly the constructs agents produce
//! — lazy continuation lines, mixed bullet markers (`-` then `*` is
//! *two* lists), indented code blocks containing `- `, GFM tables
//! degrading to paragraphs on a malformed delimiter row — and a
//! validator that disagrees with the renderer every agent uses sends
//! repair loops that cannot converge. The parser is the referee.
//!
//! The reduction carries, per block kind, exactly the material the
//! declaration surface checks: list items (text a renderer shows —
//! lazy continuations joined by a single space), paragraph source
//! lines, table header + row cells. The expression matching itself
//! lives in `memstead_schema::content_expr` — this module only
//! observes.

use memstead_schema::content_expr::ObservedBlock;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// One top-level block of a section body: what it is, where it
/// starts, and the per-kind material the format checks consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducedBlock {
    pub observed: ObservedBlock,
    /// 1-based source line of the block's first byte.
    pub line: usize,
    pub detail: BlockDetail,
}

/// Per-kind check material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockDetail {
    None,
    /// Each item: `(line, text)` — the item's inline text with lazy
    /// continuation / soft breaks joined by a single space (the text
    /// a renderer shows; continuation never changes an `item_pattern`
    /// match). Nested blocks inside an item are not part of its text.
    List { items: Vec<(usize, String)> },
    /// Each source line of the paragraph: `(line, text)`.
    Paragraph { lines: Vec<(usize, String)> },
    /// Header cell texts plus each row's entry. `cells` come from the
    /// parser (already padded/truncated to header width — GFM
    /// normalizes silently); `raw_cell_count` is the REAL
    /// pipe-delimited cell count of the source row line, so the
    /// column contract can refuse what GFM would paper over.
    Table {
        header: Vec<String>,
        rows: Vec<TableRow>,
    },
}

/// One body row of a reduced table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRow {
    pub line: usize,
    /// Cell texts as the parser emits them (padded/truncated to the
    /// header width).
    pub cells: Vec<String>,
    /// The source row's real pipe-delimited cell count.
    pub raw_cell_count: usize,
}

/// A setext heading of depth 1–2 found anywhere in the body. The
/// byte-class line guard (`^# ` / `^## `) cannot see these — only the
/// real parser can — so the reduction reports them for the
/// format-checked-section refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetextReservedHeading {
    pub line: usize,
    pub depth: u8,
}

/// The reduced view of one section body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducedSection {
    pub blocks: Vec<ReducedBlock>,
    /// Setext h1/h2 occurrences (ATX `#`/`##` are already refused by
    /// the byte-class guard before any reduction runs).
    pub setext_reserved: Vec<SetextReservedHeading>,
}

impl ReducedSection {
    /// The observed block sequence, for expression matching.
    pub fn observed(&self) -> Vec<ObservedBlock> {
        self.blocks.iter().map(|b| b.observed.clone()).collect()
    }
}

/// 1-based line number of a byte offset.
fn line_of(source: &str, offset: usize) -> usize {
    source.as_bytes()[..offset.min(source.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

fn heading_depth(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Reduce a section body to its top-level block sequence.
pub fn reduce_section(source: &str) -> ReducedSection {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);

    let mut blocks: Vec<ReducedBlock> = Vec::new();
    let mut setext_reserved: Vec<SetextReservedHeading> = Vec::new();
    // Nesting depth of container tags; a Start at depth 0 opens a
    // top-level block.
    let mut depth: usize = 0;
    // Collector state for the currently-open top-level block.
    let mut current: Option<ReducedBlock> = None;
    // While inside a top-level list item: the item's start line, the
    // merged source range of its own inline content (markers
    // preserved — item_patterns are written against source shapes),
    // how many block-level containers are open inside it, and whether
    // its own (first) paragraph was seen.
    struct ItemState {
        line: usize,
        span: Option<(usize, usize)>,
        nested: usize,
        own_paragraph_seen: bool,
    }
    let mut item: Option<ItemState> = None;
    // While inside a table: header-cell mode, current row state.
    let mut in_table_head = false;
    let mut current_cell: Option<String> = None;
    let mut current_row: Option<(usize, Vec<String>)> = None;
    // Paragraph capture is source-based: remember the range start.
    let mut para_range_start: usize = 0;

    /// Is this tag a BLOCK container? Inline containers (emphasis,
    /// links, …) are transparent for item-text collection.
    fn is_block_tag(tag: &Tag) -> bool {
        matches!(
            tag,
            Tag::Paragraph
                | Tag::List(_)
                | Tag::Item
                | Tag::Table(_)
                | Tag::CodeBlock(_)
                | Tag::BlockQuote(_)
                | Tag::Heading { .. }
                | Tag::HtmlBlock
                | Tag::FootnoteDefinition(_)
        )
    }

    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                let line = line_of(source, range.start);
                if depth == 0 {
                    let observed = match &tag {
                        Tag::Paragraph => {
                            para_range_start = range.start;
                            Some(ObservedBlock::Paragraph)
                        }
                        Tag::List(ordering) => Some(ObservedBlock::List {
                            ordered: ordering.is_some(),
                        }),
                        Tag::Table(_) => Some(ObservedBlock::Table),
                        Tag::CodeBlock(kind) => Some(ObservedBlock::Code {
                            lang: match kind {
                                CodeBlockKind::Fenced(info) => info
                                    .split_whitespace()
                                    .next()
                                    .unwrap_or("")
                                    .to_string(),
                                CodeBlockKind::Indented => String::new(),
                            },
                        }),
                        Tag::BlockQuote(_) => Some(ObservedBlock::Blockquote),
                        Tag::Heading { level, .. } => Some(ObservedBlock::Heading {
                            depth: heading_depth(*level),
                        }),
                        Tag::HtmlBlock => Some(ObservedBlock::Html),
                        _ => None,
                    };
                    if let Some(observed) = observed {
                        let detail = match &observed {
                            ObservedBlock::List { .. } => BlockDetail::List { items: Vec::new() },
                            ObservedBlock::Paragraph => {
                                BlockDetail::Paragraph { lines: Vec::new() }
                            }
                            ObservedBlock::Table => BlockDetail::Table {
                                header: Vec::new(),
                                rows: Vec::new(),
                            },
                            _ => BlockDetail::None,
                        };
                        current = Some(ReducedBlock {
                            observed,
                            line,
                            detail,
                        });
                    }
                }
                // Setext h1/h2 detection anywhere in the body: a
                // setext heading's source slice does not start with
                // '#'.
                if let Tag::Heading { level, .. } = &tag {
                    let d = heading_depth(*level);
                    if d <= 2 {
                        let slice = &source[range.start..range.end.min(source.len())];
                        if !slice.trim_start().starts_with('#') {
                            setext_reserved.push(SetextReservedHeading { line, depth: d });
                        }
                    }
                }
                match &tag {
                    Tag::Item if depth == 1 => {
                        item = Some(ItemState {
                            line,
                            span: None,
                            nested: 0,
                            own_paragraph_seen: false,
                        });
                    }
                    Tag::TableHead if depth == 1 => in_table_head = true,
                    Tag::TableRow if depth == 1 => {
                        current_row = Some((line, Vec::new()));
                    }
                    Tag::TableCell => current_cell = Some(String::new()),
                    _ => {
                        if let Some(st) = item.as_mut()
                            && !is_block_tag(&tag)
                            && st.nested == 0
                            && current_cell.is_none()
                        {
                            // Inline container (emphasis, link, …):
                            // its range covers marker + content, so
                            // merging keeps the source markers in the
                            // item text.
                            merge_span(&mut st.span, range.start, range.end);
                        }
                        if let Some(st) = item.as_mut()
                            && is_block_tag(&tag)
                        {
                            // The item's own first paragraph is
                            // transparent; every other block-level
                            // start inside the item is nesting.
                            if matches!(tag, Tag::Paragraph)
                                && st.nested == 0
                                && !st.own_paragraph_seen
                            {
                                st.own_paragraph_seen = true;
                            } else {
                                st.nested += 1;
                            }
                        }
                    }
                }
                depth += 1;
            }
            Event::End(tag_end) => {
                depth -= 1;
                match tag_end {
                    TagEnd::Item if depth == 1 => {
                        if let (Some(st), Some(block)) = (item.take(), current.as_mut())
                            && let BlockDetail::List { items } = &mut block.detail
                        {
                            let text = st
                                .span
                                .map(|(a, b)| {
                                    source[a..b.min(source.len())]
                                        .lines()
                                        .map(str::trim)
                                        .filter(|l| !l.is_empty())
                                        .collect::<Vec<_>>()
                                        .join(" ")
                                })
                                .unwrap_or_default();
                            items.push((st.line, text));
                        }
                    }
                    TagEnd::TableHead if depth == 1 => in_table_head = false,
                    TagEnd::TableRow if depth == 1 => {
                        if let (Some((line, cells)), Some(block)) =
                            (current_row.take(), current.as_mut())
                            && let BlockDetail::Table { rows, .. } = &mut block.detail
                        {
                            let raw = raw_cell_count(source, line);
                            rows.push(TableRow {
                                line,
                                cells,
                                raw_cell_count: raw,
                            });
                        }
                    }
                    TagEnd::TableCell => {
                        if let Some(cell) = current_cell.take() {
                            let cell = cell.trim().to_string();
                            if in_table_head {
                                if let Some(block) = current.as_mut()
                                    && let BlockDetail::Table { header, .. } = &mut block.detail
                                {
                                    header.push(cell);
                                }
                            } else if let Some((_, cells)) = current_row.as_mut() {
                                cells.push(cell);
                            }
                        }
                    }
                    TagEnd::Paragraph => {
                        if depth == 0
                            && let Some(block) = current.as_mut()
                            && let BlockDetail::Paragraph { lines } = &mut block.detail
                        {
                            let end = range.end.min(source.len());
                            let slice = &source[para_range_start..end];
                            let mut line_no = line_of(source, para_range_start);
                            for l in slice.lines() {
                                let t = l.trim();
                                if !t.is_empty() {
                                    lines.push((line_no, t.to_string()));
                                }
                                line_no += 1;
                            }
                        }
                        if let Some(st) = item.as_mut()
                            && st.nested > 0
                        {
                            st.nested -= 1;
                        }
                    }
                    TagEnd::List(_)
                    | TagEnd::Item
                    | TagEnd::Table
                    | TagEnd::CodeBlock
                    | TagEnd::BlockQuote(_)
                    | TagEnd::Heading(_)
                    | TagEnd::HtmlBlock
                    | TagEnd::FootnoteDefinition => {
                        if let Some(st) = item.as_mut()
                            && st.nested > 0
                        {
                            st.nested -= 1;
                        }
                    }
                    _ => {}
                }
                if depth == 0
                    && let Some(block) = current.take()
                {
                    blocks.push(block);
                }
            }
            Event::Rule if depth == 0 => {
                blocks.push(ReducedBlock {
                    observed: ObservedBlock::ThematicBreak,
                    line: line_of(source, range.start),
                    detail: BlockDetail::None,
                });
            }
            Event::Text(t) | Event::Code(t) | Event::InlineHtml(t) => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.push_str(&t);
                } else if let Some(st) = item.as_mut()
                    && st.nested == 0
                {
                    merge_span(&mut st.span, range.start, range.end);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.push(' ');
                } else if let Some(st) = item.as_mut()
                    && st.nested == 0
                {
                    merge_span(&mut st.span, range.start, range.end);
                }
            }
            _ => {}
        }
    }

    ReducedSection {
        blocks,
        setext_reserved,
    }
}

/// Merge an event's byte range into the item's inline span. The span
/// is later sliced from the SOURCE, so inline markers (`**`, `` ` ``,
/// `[…](…)`) survive exactly as the author wrote them — item patterns
/// are written against source shapes. Inline-container markers sit
/// between their children's ranges, so min/max merging covers them.
fn merge_span(span: &mut Option<(usize, usize)>, start: usize, end: usize) {
    *span = Some(match span {
        None => (start, end),
        Some((a, b)) => ((*a).min(start), (*b).max(end)),
    });
}

/// The real pipe-delimited cell count of a table row's source line —
/// GFM pads/truncates silently at parse time, so the parser's cell
/// events cannot answer "did the author write the right number of
/// cells". Counts unescaped `|` delimiters on the 1-based `line`.
fn raw_cell_count(source: &str, line: usize) -> usize {
    let Some(row) = source.lines().nth(line.saturating_sub(1)) else {
        return 0;
    };
    let trimmed = row.trim();
    let inner = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed);
    let mut count = 1;
    let mut escaped = false;
    for c in inner.chars() {
        match c {
            '\\' if !escaped => escaped = true,
            '|' if !escaped => {
                count += 1;
                escaped = false;
            }
            _ => escaped = false,
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(source: &str) -> Vec<ObservedBlock> {
        reduce_section(source).observed()
    }

    #[test]
    fn simple_bullet_list_is_one_block() {
        assert_eq!(
            observed("- one\n- two\n"),
            vec![ObservedBlock::List { ordered: false }]
        );
        assert_eq!(
            observed("1. one\n2. two\n"),
            vec![ObservedBlock::List { ordered: true }]
        );
    }

    /// Divergence pin (plan criterion 3): a lazy-continuation list is
    /// ONE list, and the continuation joins the item text with a
    /// single space.
    #[test]
    fn lazy_continuation_stays_one_list_and_joins_item_text() {
        let src = "- **Kickoff** — Projektstart\n  mit allen Beteiligten — 2026-09-01\n- **Zwei** — kurz — 2026-09-02\n";
        let reduced = reduce_section(src);
        assert_eq!(
            reduced.observed(),
            vec![ObservedBlock::List { ordered: false }]
        );
        let BlockDetail::List { items } = &reduced.blocks[0].detail else {
            panic!("list detail");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0].1,
            "**Kickoff** — Projektstart mit allen Beteiligten — 2026-09-01",
        );
        assert_eq!(items[0].0, 1, "item line");
        assert_eq!(items[1].0, 3);

        // Truly lazy continuation (no indent) — same result.
        let lazy = "- alpha\nbeta\n";
        let reduced = reduce_section(lazy);
        let BlockDetail::List { items } = &reduced.blocks[0].detail else {
            panic!("list detail");
        };
        assert_eq!(items[0].1, "alpha beta");
    }

    /// Divergence pin: mixed `-` / `*` markers are TWO lists per
    /// CommonMark — a scanner would see one.
    #[test]
    fn mixed_markers_are_two_lists() {
        assert_eq!(
            observed("- one\n* two\n"),
            vec![
                ObservedBlock::List { ordered: false },
                ObservedBlock::List { ordered: false },
            ]
        );
    }

    /// Divergence pin: fenced or indented code containing `- ` lines
    /// is code, not a list.
    #[test]
    fn code_blocks_containing_dashes_are_not_lists() {
        assert_eq!(
            observed("```\n- not a list\n```\n"),
            vec![ObservedBlock::Code { lang: String::new() }]
        );
        assert_eq!(
            observed("    - not a list\n"),
            vec![ObservedBlock::Code { lang: String::new() }]
        );
        assert_eq!(
            observed("```rust\nfn x() {}\n```\n"),
            vec![ObservedBlock::Code {
                lang: "rust".to_string()
            }]
        );
    }

    /// Divergence pin: a malformed GFM delimiter row degrades the
    /// table to paragraphs — the parser decides, not a `|` scan.
    #[test]
    fn malformed_delimiter_row_is_not_a_table() {
        let good = "| Name | Datum |\n| --- | --- |\n| a | b |\n";
        assert_eq!(observed(good), vec![ObservedBlock::Table]);

        let bad = "| Name | Datum |\n| -x- | --- |\n| a | b |\n";
        assert!(
            !observed(bad).contains(&ObservedBlock::Table),
            "malformed delimiter row must not parse as a table: {:?}",
            observed(bad)
        );
    }

    #[test]
    fn table_reduction_carries_header_and_row_cells() {
        let src = "| Name | Beschreibung | Datum |\n| --- | --- | --- |\n| Kickoff | Start | 2026-09-01 |\n| Zwei | Kurz | 2026-09-02 |\n";
        let reduced = reduce_section(src);
        let BlockDetail::Table { header, rows } = &reduced.blocks[0].detail else {
            panic!("table detail");
        };
        assert_eq!(header, &["Name", "Beschreibung", "Datum"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].cells, vec!["Kickoff", "Start", "2026-09-01"]);
        assert_eq!(rows[0].raw_cell_count, 3);
        assert_eq!(rows[1].line, 4, "row line number");
    }

    /// GFM pads/truncates mismatched row widths silently — the
    /// reduction must preserve the REAL cell count so the column
    /// contract can refuse it.
    #[test]
    fn table_rows_keep_their_real_cell_count() {
        let src = "| A | B |\n| --- | --- |\n| only |\n| x | y | z |\n";
        let reduced = reduce_section(src);
        let BlockDetail::Table { rows, .. } = &reduced.blocks[0].detail else {
            panic!("table detail");
        };
        // The parser pads/truncates to header width; the RAW counts
        // preserve what the author actually wrote.
        assert_eq!(rows[0].raw_cell_count, 1, "short row really has 1 cell");
        assert_eq!(rows[1].raw_cell_count, 3, "long row really has 3 cells");
    }

    #[test]
    fn paragraph_lines_carry_source_lines() {
        let src = "erste zeile\nzweite zeile\n\nnächster absatz\n";
        let reduced = reduce_section(src);
        assert_eq!(
            reduced.observed(),
            vec![ObservedBlock::Paragraph, ObservedBlock::Paragraph]
        );
        let BlockDetail::Paragraph { lines } = &reduced.blocks[0].detail else {
            panic!("paragraph detail");
        };
        assert_eq!(
            lines,
            &[(1, "erste zeile".to_string()), (2, "zweite zeile".to_string())]
        );
        let BlockDetail::Paragraph { lines } = &reduced.blocks[1].detail else {
            panic!("paragraph detail");
        };
        assert_eq!(lines, &[(4, "nächster absatz".to_string())]);
    }

    #[test]
    fn setext_headings_are_reported() {
        let src = "Titel\n=====\n\ntext\n\nUnter\n-----\n";
        let reduced = reduce_section(src);
        assert_eq!(
            reduced.setext_reserved,
            vec![
                SetextReservedHeading { line: 1, depth: 1 },
                SetextReservedHeading { line: 6, depth: 2 },
            ]
        );
        // ATX h3 is a heading block, not a setext report.
        let reduced = reduce_section("### Phase 1\n- x\n");
        assert!(reduced.setext_reserved.is_empty());
        assert_eq!(
            reduced.observed(),
            vec![
                ObservedBlock::Heading { depth: 3 },
                ObservedBlock::List { ordered: false },
            ]
        );
    }

    #[test]
    fn nested_list_text_stays_out_of_parent_item() {
        let src = "- parent\n  - child\n- second\n";
        let reduced = reduce_section(src);
        assert_eq!(
            reduced.observed(),
            vec![ObservedBlock::List { ordered: false }]
        );
        let BlockDetail::List { items } = &reduced.blocks[0].detail else {
            panic!("list detail");
        };
        assert_eq!(items.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), vec![
            "parent", "second"
        ]);
    }

    #[test]
    fn blockquote_html_and_rule_reduce() {
        assert_eq!(observed("> quoted\n"), vec![ObservedBlock::Blockquote]);
        assert_eq!(observed("---\n"), vec![ObservedBlock::ThematicBreak]);
        assert_eq!(observed("<div>\nx\n</div>\n"), vec![ObservedBlock::Html]);
    }

    /// End-to-end with the expression layer: the plan's own example
    /// declaration accepts its own example snippet.
    #[test]
    fn plan_example_roundtrip() {
        use memstead_schema::content_expr::ContentExpr;
        let expr = ContentExpr::parse("(heading(3) list(bullet))+").unwrap();
        let body = "### Phase 1\n- **Kickoff** — Projektstart mit allen Beteiligten — 2026-09-01\n";
        assert!(expr.match_blocks(&reduce_section(body).observed()).is_ok());

        let wrong = "### Phase 1\n\nkein listenpunkt\n";
        let err = expr
            .match_blocks(&reduce_section(wrong).observed())
            .unwrap_err();
        assert_eq!(err.failed_at, 1);
        assert_eq!(err.expected_next, vec!["list(bullet)".to_string()]);
    }
}
