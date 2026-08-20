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
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};

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
    List {
        items: Vec<(usize, String)>,
    },
    /// Each source line of the paragraph: `(line, text)`.
    Paragraph {
        lines: Vec<(usize, String)>,
    },
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
    // The engine's one CommonMark dialect — never a second inline
    // construction. A flag added here and not there (or the reverse)
    // silently re-opens the two-referee problem.
    let options = crate::markdown::parser_options();

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
                                CodeBlockKind::Fenced(info) => {
                                    info.split_whitespace().next().unwrap_or("").to_string()
                                }
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
                            let first_line = line_of(source, para_range_start);
                            for (line_no, l) in (first_line..).zip(slice.lines()) {
                                let t = l.trim();
                                if !t.is_empty() {
                                    lines.push((line_no, t.to_string()));
                                }
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

// ---------------------------------------------------------------------------
// Format evaluation
// ---------------------------------------------------------------------------

/// One violation of a section's declared format. The serde shape is
/// the wire `details` payload of the corresponding refusal code —
/// `SECTION_CONTENT_MISMATCH` / `SECTION_ITEM_PATTERN_MISMATCH` /
/// `INVALID_TABLE_COLUMNS` — plus the reserved-setext case, which
/// rides the pre-existing `SECTION_CONTENT_INVALID` family.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SectionFormatViolation {
    ContentMismatch {
        section: String,
        /// The declared expression, verbatim.
        expected: String,
        /// The observed top-level block sequence (display forms).
        found: Vec<String>,
        /// 1-based source line of the offending block (the line after
        /// the last block when the body ended too early).
        failed_at: usize,
        /// Display forms of the terminals legal at that position.
        expected_next: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        example: Option<String>,
    },
    ItemPatternMismatch {
        section: String,
        /// 0-based index of the offending unit (list item / paragraph
        /// line) within its kind.
        item_index: usize,
        /// 1-based source line of the unit.
        line: usize,
        /// The unit's text as matched (items: lazy continuation
        /// joined; paragraphs: the source line).
        text: String,
        /// The declared pattern, verbatim (anchoring is implicit).
        pattern: String,
        /// The pattern's named capture groups — the parts a
        /// conforming unit would carry.
        groups: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        example: Option<String>,
    },
    TableColumns {
        section: String,
        /// What went wrong: `header` (names/order mismatch),
        /// `cell_count` (row width vs declared columns), or
        /// `cell_pattern` (a cell failing its column's regex).
        reason: String,
        expected_columns: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        found_columns: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        row_line: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        expected_cells: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        found_cells: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        column: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cell: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        example: Option<String>,
    },
    /// A setext h1/h2 inside a format-checked section — the reserved
    /// levels the byte-class line guard cannot see.
    SetextReserved {
        section: String,
        line: usize,
        depth: u8,
    },
}

impl SectionFormatViolation {
    /// The wire code of this violation's refusal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ContentMismatch { .. } => "SECTION_CONTENT_MISMATCH",
            Self::ItemPatternMismatch { .. } => "SECTION_ITEM_PATTERN_MISMATCH",
            Self::TableColumns { .. } => "INVALID_TABLE_COLUMNS",
            Self::SetextReserved { .. } => "SECTION_CONTENT_INVALID",
        }
    }

    /// The declared conforming example, when the violation carries one.
    pub fn example(&self) -> Option<&str> {
        match self {
            Self::ContentMismatch { example, .. }
            | Self::ItemPatternMismatch { example, .. }
            | Self::TableColumns { example, .. } => example.as_deref(),
            Self::SetextReserved { .. } => None,
        }
    }

    /// One-line human rendering for refusal message text.
    pub fn describe(&self) -> String {
        match self {
            Self::ContentMismatch {
                section,
                expected,
                found,
                failed_at,
                expected_next,
                ..
            } => format!(
                "section '{section}' does not match its declared shape `{expected}` — found [{}], expected {} at line {failed_at}",
                found.join(", "),
                if expected_next.is_empty() {
                    "end of section".to_string()
                } else {
                    expected_next.join(" | ")
                },
            ),
            Self::ItemPatternMismatch {
                section,
                line,
                text,
                pattern,
                ..
            } => format!(
                "section '{section}' line {line} does not match the declared item pattern `{pattern}`: {text}"
            ),
            Self::TableColumns {
                section, reason, ..
            } => format!("section '{section}' violates its table contract ({reason})"),
            Self::SetextReserved {
                section,
                line,
                depth,
            } => format!(
                "section '{section}' line {line} is a setext h{depth} heading — h1/h2 are the entity's own levels"
            ),
        }
    }
}

/// Evaluate one section body against its declared format. Returns
/// every violation in document order (the write path refuses with the
/// first; health reports all). A section declaring no `content` — or
/// one whose expression failed to compile, which the loader refuses
/// anyway — produces no violations (free-form).
pub fn check_section_format(
    def: &memstead_schema::SectionDef,
    body: &str,
) -> Vec<SectionFormatViolation> {
    let Some(expr) = def.compiled_content.as_ref() else {
        return Vec::new();
    };
    let section = def.key.as_str();
    let reduced = reduce_section(body);
    let mut out: Vec<SectionFormatViolation> = Vec::new();

    for setext in &reduced.setext_reserved {
        out.push(SectionFormatViolation::SetextReserved {
            section: section.to_string(),
            line: setext.line,
            depth: setext.depth,
        });
    }

    let observed = reduced.observed();
    if let Err(failure) = expr.match_blocks(&observed) {
        let failed_at_line = reduced
            .blocks
            .get(failure.failed_at)
            .map(|b| b.line)
            .unwrap_or_else(|| reduced.blocks.last().map(|b| b.line + 1).unwrap_or(1));
        out.push(SectionFormatViolation::ContentMismatch {
            section: section.to_string(),
            expected: expr.source().to_string(),
            found: observed.iter().map(|b| b.display()).collect(),
            failed_at: failed_at_line,
            expected_next: failure.expected_next,
            example: def.example.clone(),
        });
    }

    if let Some(pattern_src) = &def.item_pattern
        // The loader guarantees the pattern compiles and the content
        // expression names exactly one of list/paragraph.
        && let Ok(pattern) = regex::Regex::new(&format!("^(?:{pattern_src})$"))
    {
        let groups: Vec<String> = pattern
            .capture_names()
            .flatten()
            .map(str::to_string)
            .collect();
        let targets_lists = expr.mentioned_names().contains(&"list");
        let mut unit_index = 0usize;
        for block in &reduced.blocks {
            match &block.detail {
                BlockDetail::List { items } if targets_lists => {
                    for (line, text) in items {
                        if !pattern.is_match(text) {
                            out.push(SectionFormatViolation::ItemPatternMismatch {
                                section: section.to_string(),
                                item_index: unit_index,
                                line: *line,
                                text: text.clone(),
                                pattern: pattern_src.clone(),
                                groups: groups.clone(),
                                example: def.example.clone(),
                            });
                        }
                        unit_index += 1;
                    }
                }
                BlockDetail::Paragraph { lines } if !targets_lists => {
                    for (line, text) in lines {
                        if !pattern.is_match(text) {
                            out.push(SectionFormatViolation::ItemPatternMismatch {
                                section: section.to_string(),
                                item_index: unit_index,
                                line: *line,
                                text: text.clone(),
                                pattern: pattern_src.clone(),
                                groups: groups.clone(),
                                example: def.example.clone(),
                            });
                        }
                        unit_index += 1;
                    }
                }
                _ => {}
            }
        }
    }

    if let Some(table_format) = &def.table {
        for block in &reduced.blocks {
            let BlockDetail::Table { header, rows } = &block.detail else {
                continue;
            };
            if header != &table_format.columns {
                out.push(SectionFormatViolation::TableColumns {
                    section: section.to_string(),
                    reason: "header".to_string(),
                    expected_columns: table_format.columns.clone(),
                    found_columns: header.clone(),
                    row_line: None,
                    expected_cells: None,
                    found_cells: None,
                    column: None,
                    pattern: None,
                    cell: None,
                    example: def.example.clone(),
                });
                // A wrong header makes per-cell checks noise.
                continue;
            }
            for row in rows {
                if row.raw_cell_count != table_format.columns.len() {
                    out.push(SectionFormatViolation::TableColumns {
                        section: section.to_string(),
                        reason: "cell_count".to_string(),
                        expected_columns: table_format.columns.clone(),
                        found_columns: Vec::new(),
                        row_line: Some(row.line),
                        expected_cells: Some(table_format.columns.len()),
                        found_cells: Some(row.raw_cell_count),
                        column: None,
                        pattern: None,
                        cell: None,
                        example: def.example.clone(),
                    });
                    continue;
                }
                for (column, pattern_src) in &table_format.column_patterns {
                    let Some(col_idx) = table_format.columns.iter().position(|c| c == column)
                    else {
                        continue;
                    };
                    let Some(cell) = row.cells.get(col_idx) else {
                        continue;
                    };
                    let Ok(pattern) = regex::Regex::new(&format!("^(?:{pattern_src})$")) else {
                        continue;
                    };
                    if !pattern.is_match(cell) {
                        out.push(SectionFormatViolation::TableColumns {
                            section: section.to_string(),
                            reason: "cell_pattern".to_string(),
                            expected_columns: table_format.columns.clone(),
                            found_columns: Vec::new(),
                            row_line: Some(row.line),
                            expected_cells: None,
                            found_cells: None,
                            column: Some(column.clone()),
                            pattern: Some(pattern_src.clone()),
                            cell: Some(cell.clone()),
                            example: def.example.clone(),
                        });
                    }
                }
            }
        }
    }

    out.sort_by_key(|v| match v {
        SectionFormatViolation::ContentMismatch { failed_at, .. } => *failed_at,
        SectionFormatViolation::ItemPatternMismatch { line, .. } => *line,
        SectionFormatViolation::TableColumns { row_line, .. } => row_line.unwrap_or(0),
        SectionFormatViolation::SetextReserved { line, .. } => *line,
    });
    out
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
            vec![ObservedBlock::Code {
                lang: String::new()
            }]
        );
        assert_eq!(
            observed("    - not a list\n"),
            vec![ObservedBlock::Code {
                lang: String::new()
            }]
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
            &[
                (1, "erste zeile".to_string()),
                (2, "zweite zeile".to_string())
            ]
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
        assert_eq!(
            items.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(),
            vec!["parent", "second"]
        );
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

#[cfg(test)]
mod check_tests {
    use super::*;
    use memstead_schema::{ConstraintSeverity, SectionDef, TableFormat};

    fn def(
        content: &str,
        item_pattern: Option<&str>,
        table: Option<TableFormat>,
        example: Option<&str>,
    ) -> SectionDef {
        SectionDef {
            key: "body".to_string(),
            heading: "Body".to_string(),
            required: true,
            search_weight: 1.0,
            catch_all: true,
            write_rules: vec![],
            description: None,
            content: Some(content.to_string()),
            item_pattern: item_pattern.map(str::to_string),
            table,
            example: example.map(str::to_string),
            format_severity: ConstraintSeverity::Block,
            compiled_content: Some(
                memstead_schema::content_expr::ContentExpr::parse(content).unwrap(),
            ),
            format_problems: Vec::new(),
        }
    }

    #[test]
    fn content_mismatch_carries_position_expectation_and_example() {
        let d = def(
            "(heading(3) list(bullet))+",
            None,
            None,
            Some("### Phase 1\n- **Kickoff** — 2026-09-01\n"),
        );
        let violations = check_section_format(&d, "### Phase 1\n\nprose statt liste\n");
        assert_eq!(violations.len(), 1);
        let SectionFormatViolation::ContentMismatch {
            failed_at,
            expected_next,
            found,
            example,
            ..
        } = &violations[0]
        else {
            panic!("expected content mismatch: {violations:?}");
        };
        assert_eq!(*failed_at, 3, "line of the offending paragraph");
        assert_eq!(expected_next, &vec!["list(bullet)".to_string()]);
        assert_eq!(
            found,
            &vec!["heading(3)".to_string(), "paragraph".to_string()]
        );
        assert!(example.as_deref().unwrap().contains("Kickoff"));
        assert_eq!(violations[0].code(), "SECTION_CONTENT_MISMATCH");

        // Conforming body: no violations.
        assert!(check_section_format(&d, "### Phase 1\n- **Kickoff** — 2026-09-01\n").is_empty());
    }

    #[test]
    fn item_pattern_flags_each_nonconforming_item_with_groups() {
        let d = def(
            "list(bullet)",
            Some(r"\*\*(?<name>[^*]+)\*\* — (?<datum>\d{4}-\d{2}-\d{2})"),
            None,
            None,
        );
        let ok = "- **Kickoff** — 2026-09-01\n- **Zwei** — 2026-09-02\n";
        assert!(check_section_format(&d, ok).is_empty());

        // Lazy continuation still matches (joined by a single space).
        let lazy = "- **Kickoff** —\n  2026-09-01\n";
        assert!(
            check_section_format(&d, lazy).is_empty(),
            "continuation never changes the match: {:?}",
            check_section_format(&d, lazy)
        );

        let bad = "- **Kickoff** — 2026-09-01\n- kein format\n";
        let violations = check_section_format(&d, bad);
        assert_eq!(violations.len(), 1);
        let SectionFormatViolation::ItemPatternMismatch {
            item_index,
            line,
            text,
            groups,
            ..
        } = &violations[0]
        else {
            panic!("expected item mismatch: {violations:?}");
        };
        assert_eq!(*item_index, 1);
        assert_eq!(*line, 2);
        assert_eq!(text, "kein format");
        assert_eq!(groups, &vec!["name".to_string(), "datum".to_string()]);
        assert_eq!(violations[0].code(), "SECTION_ITEM_PATTERN_MISMATCH");
    }

    #[test]
    fn paragraph_pattern_checks_each_source_line() {
        // The anker two-halves citation shape — the left half may
        // contain spaces (the `bverfg:1 BvR 2649/21` case must pass).
        let d = def(
            "paragraph+",
            Some(r"(?<quelle>\S[^|]*?) \| (?<aussage>.+)"),
            None,
            None,
        );
        let ok = "bverfg:1 BvR 2649/21 Rn. 183 | Der Staat schuldet Schutz.\ngg:art-20a | Schutzauftrag.\n";
        assert!(
            check_section_format(&d, ok).is_empty(),
            "{:?}",
            check_section_format(&d, ok)
        );
        // The observed silent-deviation shape: missing ` | ` separator.
        let bad = "bverfg:1 BvR 2649/21 Rn. 183 — Der Staat schuldet Schutz.\n";
        let violations = check_section_format(&d, bad);
        assert_eq!(violations.len(), 1);
        assert!(matches!(
            &violations[0],
            SectionFormatViolation::ItemPatternMismatch { line: 1, .. }
        ));
    }

    #[test]
    fn table_contract_enforces_columns_counts_and_cell_patterns() {
        let table = TableFormat {
            columns: vec!["Name".into(), "Datum".into()],
            column_patterns: [("Datum".to_string(), r"\d{4}-\d{2}-\d{2}".to_string())]
                .into_iter()
                .collect(),
        };
        let d = def("table", None, Some(table), None);

        let ok = "| Name | Datum |\n| --- | --- |\n| Kickoff | 2026-09-01 |\n";
        assert!(check_section_format(&d, ok).is_empty());

        // Wrong header order.
        let wrong_header = "| Datum | Name |\n| --- | --- |\n| 2026-09-01 | Kickoff |\n";
        let violations = check_section_format(&d, wrong_header);
        assert!(matches!(
            &violations[0],
            SectionFormatViolation::TableColumns { reason, .. } if reason == "header"
        ));

        // Short row — GFM would silently pad it.
        let short = "| Name | Datum |\n| --- | --- |\n| nur-eine |\n";
        let violations = check_section_format(&d, short);
        let SectionFormatViolation::TableColumns {
            reason,
            expected_cells,
            found_cells,
            row_line,
            ..
        } = &violations[0]
        else {
            panic!("expected table violation: {violations:?}");
        };
        assert_eq!(reason, "cell_count");
        assert_eq!(*expected_cells, Some(2));
        assert_eq!(*found_cells, Some(1));
        assert_eq!(*row_line, Some(3));
        assert_eq!(violations[0].code(), "INVALID_TABLE_COLUMNS");

        // Cell pattern violation names column, row, pattern.
        let bad_cell = "| Name | Datum |\n| --- | --- |\n| Kickoff | morgen |\n";
        let violations = check_section_format(&d, bad_cell);
        let SectionFormatViolation::TableColumns {
            reason,
            column,
            pattern,
            cell,
            row_line,
            ..
        } = &violations[0]
        else {
            panic!("expected cell violation: {violations:?}");
        };
        assert_eq!(reason, "cell_pattern");
        assert_eq!(column.as_deref(), Some("Datum"));
        assert!(pattern.as_deref().unwrap().contains("d{4}"));
        assert_eq!(cell.as_deref(), Some("morgen"));
        assert_eq!(*row_line, Some(3));
    }

    /// The plenum coordinate grammar (plan criterion 9): the
    /// seven-times-duplicated two-halves Belegzeile — machine
    /// coordinate `<quelle>:<dokument>:<von>-<bis>:<hash12>:<hash12>`,
    /// ` | `, then the public Fundstelle — expressed as a declaration,
    /// without a line of project Python.
    #[test]
    fn plenum_coordinate_grammar_is_declarable() {
        let d = def(
            "paragraph+",
            Some(
                r"(?<quelle>[a-z]+):(?<dokument>[^:|]+):(?<von>\d+)-(?<bis>\d+):(?<dokument_hash>[0-9a-f]{12}):(?<span_hash>[0-9a-f]{12}) \| (?<fundstelle>.+)",
            ),
            None,
            None,
        );
        let ok = "btp:20/13/073:4559-4985:09b80726ef42:0a582b1c5530 | 2022-01-26 · Tino Chrupalla · https://dserver.bundestag.de/btp/20/20013.pdf
";
        assert!(
            check_section_format(&d, ok).is_empty(),
            "{:?}",
            check_section_format(&d, ok)
        );
        // Missing span hash — the checker's regex class, declared.
        let bad =
            "btp:20/13/073:4559-4985:09b80726ef42 | 2022-01-26 · Chrupalla · https://example.org
";
        assert_eq!(check_section_format(&d, bad).len(), 1);
        // Missing the two-halves separator.
        let bad = "btp:20/13/073:4559-4985:09b80726ef42:0a582b1c5530 2022-01-26
";
        assert_eq!(check_section_format(&d, bad).len(), 1);
    }

    #[test]
    fn setext_reserved_headings_refuse_in_checked_sections() {
        let d = def("paragraph+", None, None, None);
        let violations = check_section_format(&d, "Titel\n=====\n\ntext\n");
        assert!(
            violations
                .iter()
                .any(|v| matches!(v, SectionFormatViolation::SetextReserved { depth: 1, .. })),
            "{violations:?}"
        );
        assert_eq!(
            violations
                .iter()
                .find(|v| matches!(v, SectionFormatViolation::SetextReserved { .. }))
                .unwrap()
                .code(),
            "SECTION_CONTENT_INVALID"
        );
    }

    #[test]
    fn free_form_section_produces_no_violations() {
        let d = SectionDef {
            key: "body".to_string(),
            heading: "Body".to_string(),
            required: true,
            search_weight: 1.0,
            catch_all: true,
            write_rules: vec![],
            description: None,
            content: None,
            item_pattern: None,
            table: None,
            example: None,
            format_severity: ConstraintSeverity::Block,
            compiled_content: None,
            format_problems: Vec::new(),
        };
        assert!(check_section_format(&d, "anything\n=====\n\n- mixed\n* markers\n").is_empty());
    }
}
