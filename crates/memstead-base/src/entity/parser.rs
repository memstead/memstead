//! Markdown → Entity parser. Handles YAML frontmatter, sections, wiki-links.
//!
//! Key design decisions:
//! - Hand-rolled YAML frontmatter parser (NOT serde_yaml) to match JS type coercion
//! - Code blocks are masked before section/link detection to prevent false matches
//! - The parser is schema-aware: it uses the schema to determine catch-all sections

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::Regex;
use sha2::{Digest, Sha256};

use memstead_schema::TypeDefinition;

use super::id::{WikiLinkError, file_path_to_id, wiki_link_to_id, wiki_link_to_id_lenient};
use super::{Entity, EntityId, HeadingSpan, MetadataValue, ParseResult, Relationship};

/// Parse a markdown string into an Entity.
pub fn parse_markdown(
    content: &str,
    relative_path: &str,
    schema: &TypeDefinition,
    mem: &str,
) -> Result<ParseResult, ParseError> {
    let id = file_path_to_id(relative_path, mem);

    // Compute content hash from raw markdown
    let content_hash = compute_hash(content);

    // Extract YAML frontmatter FIRST — frontmatter is not markdown, and
    // handing it to a CommonMark parser invents block structure that is
    // not there (a value line that reads as a fence opener would open a
    // code block running past the closing `---` and mask the whole
    // body). Only the body is markdown, so only the body is masked.
    let (metadata, body) = split_frontmatter(content)?;
    let masked_body = mask_code_blocks(&body);

    // Extract title (first # heading)
    let title = extract_title(&body, &masked_body).unwrap_or_else(|| id.name().to_string());

    // Split body into ## sections (match against masked, slice from original).
    // Duplicate `## Heading` lines whose slug matches a schema-declared key
    // become `DuplicateSectionHeading` warnings below; first-wins is the
    // resolution policy.
    let (sections_map, duplicate_headings, raw_section_headings) =
        split_sections(&body, &masked_body);

    // Parse typed relationships from the Relationships section.
    // The entity-id collector lets the parser surface
    // `AMBIGUOUS_DESCRIPTION_DELIMITER` warnings against a concrete
    // source so boot / reload / attach sites can report them in
    // `LoadCollector::warnings`.
    let rel_heading_key = "relationships";
    let entity_id_for_rel_warnings = file_path_to_id(relative_path, mem);
    let (relationships, rel_parse_warnings) = parse_relationships_with_warnings(
        sections_map
            .get(rel_heading_key)
            .map(|(_, content)| content.as_str())
            .unwrap_or(""),
        mem,
        Some(&entity_id_for_rel_warnings),
    );

    // Build catch-all section content
    let catch_all_content = build_catch_all(&sections_map, schema);

    // Extract schema-defined section values.
    // IndexMap + this loop order is what guarantees sections iterate in the
    // schema's declared order downstream. Do not change to a HashMap.
    // No re-trim here: `split_sections` already normalised each value
    // (leading blank lines dropped, first visible line's bytes kept,
    // trailing trimmed), and `build_catch_all` joins such values. A
    // second full trim silently promoted a whitespace-prefixed first
    // line (`\u{b}` + backticks) to column 0, where the CommonMark
    // referee suddenly saw a fence opener that the stored form did not
    // have — the section structure then shifted between rounds (fuzz
    // finding, long tier, corpus member `crash-fd71330e…`).
    let mut result_sections = IndexMap::new();
    for s in &schema.sections {
        if s.catch_all {
            // The catch-all key stays present when its own heading was in
            // the document or any non-schema section fed it — an absent
            // catch-all with no absorbed content stays absent, like every
            // other section below.
            if sections_map.contains_key(s.key.as_str()) || !catch_all_content.trim().is_empty() {
                result_sections.insert(s.key.clone(), catch_all_content.clone());
            }
        } else if let Some((_, content)) = sections_map.get(s.key.as_str()) {
            result_sections.insert(s.key.clone(), content.clone());
        }
        // A declared section whose heading the document does not carry is
        // ABSENT, not present-with-empty. Materialising every declared key
        // here (the pre-fix `unwrap_or_default`) made "no heading" and
        // "empty heading" the same entity, so the generator re-emitted a
        // scaffold heading for every declared-but-unwritten section and
        // `sections_unset` could not close one — the removed key came back
        // on the next round-trip.
    }

    // Parse metadata values with type coercion
    let mut parsed_metadata = parse_metadata(&metadata);

    // Determine type from metadata or default, and ensure it's in metadata.
    // The entity's `type:` frontmatter key takes precedence over the mem's
    // default type — parse-time resolution means each file is authoritative
    // about its own type.
    let type_name = parsed_metadata
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or(schema.name.as_str())
        .to_string();
    parsed_metadata.insert("type".to_string(), MetadataValue::String(type_name.clone()));

    // Extract inline wiki-links from text fields (excluding relationships section)
    let inline_link_text: String = schema
        .text_fields
        .iter()
        .filter_map(|f| result_sections.get(f.as_str()))
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    // Read-time scan: tolerate pre-strict on-disk drift so loaders
    // and dangling-link reporters keep working against legacy
    // entities. The mutation pipeline re-extracts strictly via
    // `extract_inline_links` and refuses on grammar violations.
    let inline_links = extract_inline_links_lenient(&inline_link_text, mem);

    // Filter out targets already covered by explicit relationships
    let explicit_targets: HashSet<_> = relationships.iter().map(|r| &r.target).collect();
    let inline_links: Vec<EntityId> = inline_links
        .into_iter()
        .filter(|link| !explicit_targets.contains(link))
        .collect();

    // Extract H3–H6 spans per section for search-time heading-path attribution.
    // Side-struct only: regenerated every parse, never persisted.
    let heading_spans = extract_heading_spans(&result_sections);

    // Build warnings for duplicate-heading occurrences whose slug matches a
    // schema-declared key. Catch-all keys (`s.catch_all`) absorb arbitrary
    // headings by design, so duplicates there are not surfaced.
    let declared_keys: HashSet<&str> = schema
        .sections
        .iter()
        .filter(|s| !s.catch_all)
        .map(|s| s.key.as_str())
        .collect();
    let entity_id_for_warnings = file_path_to_id(relative_path, mem);
    let mut parse_warnings: Vec<crate::ops::WarningHint> = duplicate_headings
        .into_iter()
        .filter(|d| declared_keys.contains(d.key.as_str()))
        .map(|d| crate::ops::WarningHint::DuplicateSectionHeading {
            entity_id: entity_id_for_warnings.clone(),
            section_key: d.key,
            heading: d.heading,
            occurrences: d.occurrences,
        })
        .collect();
    parse_warnings.extend(rel_parse_warnings);

    let entity = Entity {
        id,
        title,
        entity_type: type_name,
        mem: mem.to_string(),
        file_path: relative_path.to_string(),
        metadata: parsed_metadata,
        sections: result_sections,
        relationships,
        content_hash,
        stub: false,
        stub_kind: None,
        heading_spans,
        raw_section_headings,
    };

    Ok(ParseResult {
        entity,
        inline_links,
        parse_warnings,
    })
}

/// Parse an entity from a file on disk.
pub fn parse_file(
    path: &Path,
    mem_dir: &Path,
    schema: &TypeDefinition,
    mem: &str,
) -> Result<ParseResult, ParseError> {
    let content = std::fs::read_to_string(path)?;
    let relative_path = path.strip_prefix(mem_dir).unwrap_or(path).to_string_lossy();
    parse_markdown(&content, &relative_path, schema, mem)
}

// ---------------------------------------------------------------------------
// Frontmatter
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The frontmatter delimiter contract — one implementation
// ---------------------------------------------------------------------------

/// What a document's leading `---` block turned out to be.
///
/// The three historical readers differed only in what they did with these
/// three cases, never in how they found them: the tolerant path degraded on
/// both failures, the peek borrowed and degraded, the strict path refused with
/// a different typed error for each. That difference belongs to the wrappers;
/// the arithmetic below belongs here, once.
#[derive(Debug, PartialEq, Eq)]
pub enum Frontmatter<'a> {
    /// A closed block. Both slices borrow the input.
    Present { meta: &'a str, body: &'a str },
    /// The document does not open with `---` on its first line.
    NoOpeningDelimiter,
    /// It opens but never closes with `\n---`.
    Unclosed,
}

/// Split a document at its frontmatter delimiters.
///
/// Returns the byte-order-mark-stripped input alongside the verdict, because
/// every caller that degrades to "the whole document is body" must degrade to
/// the *stripped* document: a marked local file once parsed as all body and
/// lost its entire frontmatter precisely because two paths disagreed about
/// where the document began. **The mark is stripped here and nowhere else.**
///
/// Both returned slices are suffixes of the returned `stripped` slice, which
/// is itself a suffix of `content`. Two callers recover the frontmatter prefix
/// by subtracting the body's length, so a core that copied or normalised
/// anything would break them silently.
pub fn split_frontmatter_core(content: &str) -> (&str, Frontmatter<'_>) {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);

    let after_open = if content.starts_with("---\r\n") {
        5
    } else if content.starts_with("---\n") {
        4
    } else {
        return (content, Frontmatter::NoOpeningDelimiter);
    };

    let rest = &content[after_open..];
    let Some(close_pos) = rest.find("\n---") else {
        return (content, Frontmatter::Unclosed);
    };
    let meta = &rest[..close_pos];

    let body_rest = &rest[close_pos + "\n---".len()..];
    let body = body_rest
        .strip_prefix("\r\n")
        .or_else(|| body_rest.strip_prefix('\n'))
        .unwrap_or(body_rest);

    (content, Frontmatter::Present { meta, body })
}

/// The frontmatter block and the body of a document, both borrowed, or
/// `None` when the document carries no closed frontmatter (no opening
/// fence on its first line, or an opening fence never closed). The one
/// entry point every reader outside this module uses; the contract is
/// [`split_frontmatter_core`]'s: a byte-order mark is skipped, CRLF and LF
/// documents both split, the fence counts only at the start of the
/// document, and the body starts after the closing fence's line break.
pub fn frontmatter_parts(content: &str) -> Option<(&str, &str)> {
    match split_frontmatter_core(content) {
        (_, Frontmatter::Present { meta, body }) => Some((meta, body)),
        _ => None,
    }
}

/// Extract the `type:` value from frontmatter without running the full parser.
///
/// Used by the loader to resolve each file's type independently — the mem
/// config's default type is only a fallback for files that don't declare one.
/// Returns None if there's no frontmatter, no `type:` line, or it's empty.
pub fn peek_type_from_frontmatter(content: &str) -> Option<String> {
    let (_, split) = split_frontmatter_core(content);
    let Frontmatter::Present {
        meta: frontmatter, ..
    } = split
    else {
        return None;
    };

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(colon_idx) = trimmed.find(':') else {
            continue;
        };
        let key = trimmed[..colon_idx].trim();
        if key != "type" {
            continue;
        }
        let mut value = trimmed[colon_idx + 1..].trim();
        if let Some(hash_idx) = value.find('#') {
            value = value[..hash_idx].trim();
        }
        let value = value.trim_matches(|c| c == '"' || c == '\'');
        if value.is_empty() {
            return None;
        }
        return Some(value.to_string());
    }
    None
}

/// Peek the entity title (first `# ` heading in the body) and type
/// (`type:` frontmatter field) from raw markdown without running the
/// full schema-aware parser. Used by surfaces that read a markdown blob
/// outside the in-memory store — e.g. `memstead_diff` walking git trees
/// between two arbitrary refs, where the store snapshot (current HEAD)
/// is not a valid source for a non-HEAD ref. Returns `None` for `title`
/// when the body carries no `# ` heading and `None` for `entity_type`
/// when the frontmatter lacks a non-empty `type:`.
pub fn peek_title_and_type(content: &str) -> (Option<String>, Option<String>) {
    let entity_type = peek_type_from_frontmatter(content);
    let body = body_after_frontmatter(content);
    let title = extract_title(body, &mask_code_blocks(body));
    (title, entity_type)
}

/// Return the body slice after a leading `---` frontmatter block, or
/// the whole input when no frontmatter is present. Mirrors the offset
/// arithmetic in [`split_frontmatter`] but borrows rather than
/// allocating — a scan needs to read, not own.
///
/// **Call this before handing a whole entity file to any markdown
/// reader in this module.** Frontmatter is not markdown: a CommonMark
/// parser reads block structure into it that is not there, and a YAML
/// value that looks like a fence opener (legal at 1–3 spaces) opens a
/// code block that runs past the `---` terminator to end of file,
/// masking the entire body. Every reader here — [`mask_code_blocks`],
/// [`extract_inline_links`], [`extract_inline_links_lenient`],
/// [`split_sections`] — expects a body, and the callers inside the
/// engine that hold one already pass section bodies. A caller holding
/// a raw file or git blob does not, and must trim it here first.
pub fn body_after_frontmatter(content: &str) -> &str {
    match split_frontmatter_core(content) {
        (_, Frontmatter::Present { body, .. }) => body,
        (stripped, _) => stripped,
    }
}

/// Split content into frontmatter metadata string and body.
/// Returns (metadata_string, body). Both boundaries are found in the
/// raw content — the caller masks the body afterwards.
pub(crate) fn split_frontmatter(content: &str) -> Result<(String, String), ParseError> {
    // The every-local-read path: it never refuses. A document with no opening
    // delimiter, or an unclosed block, is entirely body — a hand-edited file
    // without frontmatter must still load.
    match split_frontmatter_core(content) {
        (_, Frontmatter::Present { meta, body }) => Ok((meta.to_string(), body.to_string())),
        (stripped, _) => Ok((String::new(), stripped.to_string())),
    }
}

/// Parse metadata key-value pairs with JS-compatible type coercion.
///
/// Handles: strings, integers, floats, booleans.
/// Strips inline comments (`value # comment`) and quotes (`"value"`).
fn parse_metadata(text: &str) -> IndexMap<String, MetadataValue> {
    let mut meta = IndexMap::new();
    if text.is_empty() {
        return meta;
    }

    for line in text.lines() {
        let trimmed = line.trim();
        // Skip empty lines, comments, heading markers, delimiters
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("---") {
            continue;
        }

        let Some(colon_idx) = trimmed.find(':') else {
            continue;
        };

        let key = trimmed[..colon_idx].trim().to_string();
        let raw_value = trimmed[colon_idx + 1..].trim();

        // Strip inline comments (# not inside the value)
        let value = strip_inline_comment(raw_value).trim().to_string();

        if value.is_empty() {
            meta.insert(key, MetadataValue::String(String::new()));
            continue;
        }

        // Type coercion (matching JS parser behavior exactly)
        if value == "true" {
            meta.insert(key, MetadataValue::Bool(true));
        } else if value == "false" {
            meta.insert(key, MetadataValue::Bool(false));
        } else if is_float_literal(&value) {
            if let Ok(f) = value.parse::<f64>() {
                meta.insert(key, MetadataValue::Float(f));
            } else {
                meta.insert(key, MetadataValue::String(strip_quotes(&value)));
            }
        } else if is_integer_literal(&value) {
            if let Ok(n) = value.parse::<i64>() {
                meta.insert(key, MetadataValue::Integer(n));
            } else {
                meta.insert(key, MetadataValue::String(strip_quotes(&value)));
            }
        } else {
            meta.insert(key, MetadataValue::String(strip_quotes(&value)));
        }
    }

    meta
}

/// Check if a string matches the JS float regex: /^-?\d+\.\d+$/
fn is_float_literal(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    if let Some((before, after)) = s.split_once('.') {
        !before.is_empty()
            && before.chars().all(|c| c.is_ascii_digit())
            && !after.is_empty()
            && after.chars().all(|c| c.is_ascii_digit())
    } else {
        false
    }
}

/// Check if a string matches the JS integer regex: /^-?\d+$/
fn is_integer_literal(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// Would `parse_metadata` coerce this raw value away from
/// `MetadataValue::String`? Exposed so the generator can decide whether
/// to YAML-quote a string value that would otherwise round-trip as
/// Integer / Float / Bool. Kept co-located with the coercion rules so
/// the two cannot drift.
pub(crate) fn would_coerce_from_string(s: &str) -> bool {
    s == "true" || s == "false" || is_integer_literal(s) || is_float_literal(s)
}

/// Strip inline comments: `value # comment` → `value`.
fn strip_inline_comment(s: &str) -> &str {
    // Find ` #` pattern (space followed by #)
    // But be careful not to strip inside quoted strings
    if let Some(idx) = s.find(" #") {
        s[..idx].trim_end()
    } else {
        s
    }
}

/// Strip surrounding quotes: `"value"` or `'value'` → `value`.
/// A lone quote character is not a quoted value — `len >= 2` keeps the
/// slice in bounds (a 1-char `"` satisfies both starts_with and ends_with).
fn strip_quotes(s: &str) -> String {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Code block masking
// ---------------------------------------------------------------------------

/// Mask every CommonMark code block by replacing its bytes with spaces
/// (preserves line count and byte offsets). Handles unclosed blocks
/// safely — they mask to end of text.
///
/// The definition lives in [`crate::markdown`] — one referee for every
/// content reader in the engine. Re-exported here because this module's
/// callers are the historical ones.
pub use crate::markdown::{mask_code_blocks, mask_code_blocks_and_spans};

// ---------------------------------------------------------------------------
// Merge-conflict detection
// ---------------------------------------------------------------------------

/// True when `text` — a whole entity file — carries a complete git
/// merge-conflict block: an ordered `<<<<<<< …` / `=======` /
/// `>>>>>>> …` triple at line starts.
///
/// The frontmatter is scanned **raw** and the body over
/// [`mask_code_blocks`] output. Both halves of that split are
/// load-bearing:
///
/// - Masking the body is why a code example documenting conflict
///   markers never trips the check. It is a legibility trade-off, not
///   a soundness one: a real conflict whose markers all fall inside
///   one code block goes undetected (the file then loads/degrades
///   exactly as it did before this check existed) — git writes markers
///   without regard for fences, so that shape is rare, while marker
///   examples in documentation entities are not.
/// - Frontmatter is **not** masked, because frontmatter is not
///   markdown. Handing it to a CommonMark parser invents block
///   structure that is not there: a YAML value that reads as a fence
///   opener (legal at 1–3 spaces) opens a code block that runs past
///   the `---` terminator to end of file and blanks the entire body,
///   markers and all — a conflicted file would then load with both
///   sides fused into one entity, which is precisely the outcome
///   `entity::loader`'s caller exists to prevent. Git also writes
///   conflict markers into frontmatter, so scanning it is not merely
///   safe, it is required.
pub fn has_merge_conflict_markers(text: &str) -> bool {
    // One view, not two scans: a conflict can straddle the `---`
    // terminator (git writes markers wherever the hunks fall), so the
    // raw frontmatter and the masked body are rejoined and scanned as
    // a single text. Masking preserves byte length, so the join is the
    // original file with only body code blocks blanked.
    let body = body_after_frontmatter(text);
    let frontmatter = &text[..text.len() - body.len()];
    let view = format!("{frontmatter}{}", mask_code_blocks(body));

    let mut seen_start = false;
    let mut seen_separator = false;
    for line in view.lines() {
        if line.starts_with("<<<<<<< ") {
            seen_start = true;
            seen_separator = false;
        } else if seen_start && line.trim_end() == "=======" {
            seen_separator = true;
        } else if seen_separator && line.starts_with(">>>>>>> ") {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Section splitting
// ---------------------------------------------------------------------------

/// Tracks one schema-declared section key seen more than once on parse.
/// `key` is the slugified storage key (e.g. `realization`); `heading` is
/// the original literal text from the first occurrence (e.g. `Realization`).
/// Sections keyed by derived key; each value is the heading line
/// VERBATIM from the original body plus the section's content.
pub(crate) type SplitSections = IndexMap<String, (String, String)>;

/// `occurrences` counts every header line for that key — first plus
/// duplicates.
pub(crate) struct DuplicateSection {
    pub key: String,
    pub heading: String,
    pub occurrences: usize,
}

/// The section boundary: a column-0 ATX `## ` line with a non-empty
/// rest, matched over a MASKED text. One definition, used over the
/// whole body and again over each piece the splitter reads from a
/// neutral start.
fn section_heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)^## (.+)$").unwrap())
}

/// A section's stored content and the body offset where it starts.
///
/// Leading trim drops blank lines wholesale but keeps the first
/// visible line's indentation: a full trim promoted an indented
/// heading-lookalike (` ## Specifies`) to column 0 inside stored
/// content, where the catch-all re-emit made the NEXT parse read
/// it as a real section heading — structure from content, and a
/// broken parse-generate fixpoint (fuzz finding, long tier,
/// 2026-08-24, corpus member `crash-de0c69e0…`). Trailing trim
/// stays full: it can never move a line to column 0. The
/// runtime validator's embedded-heading guard keeps its own full
/// trim, so the mutation path refuses exactly what it refused.
fn section_content(body: &str, content_start: usize, content_end: usize) -> (&str, usize) {
    let raw = &body[content_start..content_end];
    let visible_start = raw
        .split_inclusive('\n')
        .take_while(|line| line.trim().is_empty())
        .map(str::len)
        .sum::<usize>();
    (
        raw[visible_start..].trim_end(),
        content_start + visible_start,
    )
}

/// Every section boundary of `body` in document order, each the byte
/// range of its heading line.
///
/// The first pass is the regex over the masked body: a column-0 `## `
/// line outside every code block. That pass reads each heading where
/// it stands, and a heading can stand inside a block the referee is
/// still reading: an HTML block of a kind no blank line ends (`<!X`,
/// `<!--`, `<?`), or a table. The splitter does not narrow the
/// boundary there (a `## ` line inside an unterminated comment stays a
/// section, so a stray `<!--` never swallows the rest of the document),
/// but the section's content was then read in situ under that block's
/// context, and the generator writes every section from a neutral
/// start: heading line, newline, content. Read from there the same
/// bytes can mean something else. A `<!--` that was inert inside a
/// `<!X` block opens a comment, the tilde fence after it goes dead, and
/// a `## ` line the fence masked in situ reads as a heading on the next
/// parse; the second parse splits where the first did not and the two
/// generations differ (fuzz findings, CI runs 2026-09-08 and
/// 2026-09-09, corpus members `crash-c9e7bbf7…` and `crash-ce631bf5…`;
/// in the first the enclosing block was opened by the preceding
/// section, in the second by the preamble before any heading, so no
/// rule about closers between merged pieces can repair the class).
///
/// So the second pass reads every such section the way the generator
/// will write it. A heading the referee did not itself read as a
/// heading in situ was read inside some other block; its piece
/// (verbatim heading line, newline, trimmed content) is masked from a
/// neutral start, and the first `## ` line that surfaces becomes a
/// boundary of its own. The new piece is read the same way, and so on
/// until every piece shows exactly one boundary. Boundaries are only
/// ever added, and only at lines the next parse would have split on
/// anyway, so the first parse already lands on the fixpoint. A section
/// whose heading the referee read as a heading stands in a neutral
/// context already (a column-0 ATX heading ends every paragraph, list
/// and quote before it, and a code block would have masked it), so its
/// in-situ reading and the generator's agree; reading it again would
/// find nothing, which is why the heading list is only a cost filter
/// and most documents never enter the second pass.
fn section_boundaries(body: &str, masked_body: &str) -> Vec<Range<usize>> {
    let mut bounds: BTreeMap<usize, usize> = section_heading_re()
        .find_iter(masked_body)
        .map(|m| (m.start(), m.end()))
        .collect();
    if bounds.is_empty() {
        return Vec::new();
    }
    let read_as_heading = crate::markdown::heading_starts(body);
    let mut work: Vec<usize> = bounds
        .keys()
        .copied()
        .filter(|start| read_as_heading.binary_search(start).is_err())
        .collect();
    while let Some(start) = work.pop() {
        let end = bounds[&start];
        let content_end = bounds
            .range(start + 1..)
            .next()
            .map_or(body.len(), |(next, _)| *next);
        let heading_line = &body[start..end];
        let (content, content_start) = section_content(body, end, content_end);
        let piece = format!("{heading_line}\n{content}");
        let masked = mask_code_blocks(&piece);
        let Some(extra) = section_heading_re()
            .find_iter(&masked)
            .find(|m| m.start() > 0)
        else {
            continue;
        };
        // The piece is heading line, `\n`, content: an offset past the
        // heading line maps into the body at the content's start.
        let abs_start = content_start + (extra.start() - heading_line.len() - 1);
        bounds.insert(abs_start, abs_start + extra.len());
        work.push(abs_start);
    }
    bounds.into_iter().map(|(start, end)| start..end).collect()
}

/// Split body into named sections. Returns `Map<lowercase_key,
/// (heading_line, content)>` — the heading line VERBATIM from the
/// original body, because the catch-all re-emits it and a heading
/// rebuilt from the derived key changes what the CommonMark referee
/// sees (a CR inside a heading is a line ending of its own, so its
/// tail can be a live fence opener; the derived key lost the CRs, the
/// re-parse un-masked the section's content, and a promoted empty
/// heading then vanished — fuzz finding, corpus member
/// `crash-9fd95247…`) — plus a list of duplicate-heading occurrences.
/// Duplicate headings keep the first occurrence's body; subsequent
/// occurrences are dropped from the storage value entirely (no
/// embedded `## Heading` separator). The caller decides whether each
/// duplicate becomes a `WarningHint` (schema-declared keys only —
/// catch-all repetition stays silent). The third element is every
/// literal heading text in document order (duplicates included) — the
/// raw material for the health check that distinguishes "section
/// absent" from "content under a non-deriving heading".
pub(crate) fn split_sections(
    body: &str,
    masked_body: &str,
) -> (SplitSections, Vec<DuplicateSection>, Vec<String>) {
    // IndexMap, not HashMap: the catch-all builder re-emits non-schema
    // sections in this map's iteration order, so the order must be the
    // document's — hash-random order made canonical bytes unstable
    // across parses whenever more than one non-schema section coexisted
    // (reachable on the tolerant local-read path, which refuses nothing).
    let mut sections = IndexMap::new();
    let mut duplicates: HashMap<String, DuplicateSection> = HashMap::new();
    let mut raw_headings = Vec::new();
    let boundaries = section_boundaries(body, masked_body);

    for (i, m) in boundaries.iter().enumerate() {
        // Extract heading name from original body (not masked)
        let heading_line = &body[m.clone()];
        let name = heading_line
            .strip_prefix("## ")
            .unwrap_or(heading_line)
            .trim();

        let content_end = boundaries.get(i + 1).map_or(body.len(), |next| next.start);
        let (content, _) = section_content(body, m.end, content_end);
        let content = content.to_string();
        // Schema section keys are underscore-separated (e.g. `current_state`).
        // A heading like `## Current State` must derive to the same form so
        // schema-declared sections land in `result_sections` under the right
        // key instead of falling through to catch-all — which would break
        // canonical byte-stability for any multi-word section. The derivation
        // is shared with the schema loader's round-trip check — never inline
        // a second copy here.
        let key = memstead_schema::derive_section_key(name);
        raw_headings.push(name.to_string());

        match sections.entry(key.clone()) {
            indexmap::map::Entry::Vacant(slot) => {
                slot.insert((heading_line.to_string(), content));
                duplicates.insert(
                    key.clone(),
                    DuplicateSection {
                        key: key.clone(),
                        heading: name.to_string(),
                        occurrences: 1,
                    },
                );
            }
            indexmap::map::Entry::Occupied(_) => {
                // First-wins: drop this duplicate's body entirely. Bump the
                // occurrence count for the warning emitted by the caller.
                if let Some(d) = duplicates.get_mut(&key) {
                    d.occurrences += 1;
                }
            }
        }
    }

    let dup_list: Vec<DuplicateSection> = duplicates
        .into_values()
        .filter(|d| d.occurrences > 1)
        .collect();

    (sections, dup_list, raw_headings)
}

/// Extract the title from the first `# ` heading.
///
/// Scans the masked body so a `# ` line inside a code block can never
/// become the entity title, and reads the text back from the original —
/// masking preserves byte offsets and line count, so the two line
/// sequences correspond one-to-one.
fn extract_title(body: &str, masked_body: &str) -> Option<String> {
    for (line, masked) in body.lines().zip(masked_body.lines()) {
        if masked.starts_with("# ") {
            return Some(line[2..].trim().to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Heading spans (H3–H6)
// ---------------------------------------------------------------------------

/// Extract H3–H6 heading spans from each section's content. Byte offsets are
/// into the (trimmed) section string stored in `result_sections`. Code blocks
/// are masked before scanning so `### foo` inside any code block is ignored.
///
/// End offsets use a level-aware closing rule: a span closes at the next
/// heading with the same or lower level (H3 closes on next H3 or H2 — but
/// H2 doesn't appear here since sections are already split), otherwise at
/// the end of the section. Level skips (H2 → H4 without H3) are tolerated:
/// the H4 span is recorded flat, and query-time path resolution uses offset
/// containment to reconstruct ancestry.
fn extract_heading_spans(sections: &IndexMap<String, String>) -> HashMap<String, Vec<HeadingSpan>> {
    // Compiled once per process; shape-constrained so it can't fail at runtime.
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?m)^(#{3,6})[ \t]+(.+)$").unwrap());
    let mut out: HashMap<String, Vec<HeadingSpan>> = HashMap::new();

    for (key, content) in sections {
        if content.is_empty() {
            continue;
        }
        let masked = mask_code_blocks(content);

        // Collect (start_offset, level, title) in document order.
        let raw: Vec<(usize, u8, String)> = re
            .captures_iter(&masked)
            .map(|cap| {
                let whole = cap.get(0).unwrap();
                let level = cap[1].len() as u8; // 3..=6
                // Read the title from the original (unmasked) content so the
                // captured text survives code-block masking's space-padding.
                let line_end = content[whole.start()..]
                    .find('\n')
                    .map(|i| whole.start() + i)
                    .unwrap_or(content.len());
                let hashes_end = whole.start() + level as usize;
                let title = content[hashes_end..line_end].trim().to_string();
                (whole.start(), level, title)
            })
            .collect();

        if raw.is_empty() {
            continue;
        }

        let mut spans: Vec<HeadingSpan> = Vec::with_capacity(raw.len());
        for (i, &(start, level, ref title)) in raw.iter().enumerate() {
            // Scan forward for the next heading with level <= this one.
            let end = raw[i + 1..]
                .iter()
                .find(|(_, l, _)| *l <= level)
                .map(|(s, _, _)| *s)
                .unwrap_or(content.len());
            spans.push(HeadingSpan {
                level,
                title: title.clone(),
                start_offset: start,
                end_offset: end,
            });
        }
        out.insert(key.clone(), spans);
    }

    out
}

// ---------------------------------------------------------------------------
// Catch-all section
// ---------------------------------------------------------------------------

/// Build catch-all section content from its own section + non-schema sections.
fn build_catch_all(sections: &SplitSections, schema: &TypeDefinition) -> String {
    let catch_all = match schema.catch_all_section() {
        Some(s) => s,
        None => return String::new(),
    };

    let known_sections: HashSet<&str> = schema
        .sections
        .iter()
        .map(|s| s.key.as_str())
        .chain(std::iter::once("relationships"))
        .collect();

    let mut parts = Vec::new();

    // First, add the explicit catch-all section content
    if let Some((_, content)) = sections.get(catch_all.key.as_str())
        && !content.is_empty()
    {
        parts.push(content.clone());
    }

    // Then add all non-schema sections, each re-emitted under its
    // ORIGINAL heading line, byte-verbatim — never a heading rebuilt
    // from the derived key: the rebuilt form changed what the referee
    // sees (a CR inside a heading is a CommonMark line ending of its
    // own, so its tail can be a live fence opener the derived key
    // lost), and the re-parse then promoted masked content to
    // structure (fuzz finding, corpus member `crash-9fd95247…`).
    // Document order — `sections` is an IndexMap for exactly this
    // loop: with more than one non-schema section (reachable on the
    // tolerant local-read path, which refuses nothing) a hash-random
    // order made the reconstructed catch-all differ from parse to parse.
    for (key, (heading_line, content)) in sections {
        if !known_sections.contains(key.as_str()) && !content.is_empty() {
            parts.push(format!("{heading_line}\n{content}"));
        }
    }

    // Incremental context close: every close decision is judged over
    // the RUNNING string after each append — never over a piece in
    // isolation. Isolation misjudges in both directions (lazy
    // continuation and CR line endings make the same bytes a fence in
    // one context and prose in another): an isolation close injected a
    // spurious closer that the generator's part-level close then paired
    // into an empty fence block, growing the document every round
    // (corpus candidate `crash-619fe90c`), while skipping the close
    // entirely let a piece's dangling fence swallow the next piece's
    // heading (corpus member `crash-07c152bb`). Closing in context
    // after each piece keeps both: a dangling fence closes before the
    // next piece, and no closer is ever added for a construct the
    // document context does not read as a fence. The oracle verifies
    // its closer against the mask, so the appended line is a real
    // closer wherever it lands. The context is not only fences: an
    // HTML block of the kinds no blank line ends (`<!X`, `<!--`, `<?`,
    // `<![CDATA[`, a `<script>`-family tag) hides every fence the
    // referee would otherwise read, so a piece that ends inside one
    // changes what the NEXT piece's fences mean — a `## ` line that a
    // fence masked in situ surfaces as a heading after the merge, and
    // as an empty non-schema section it is dropped a round later
    // (fuzz finding, long tier, 0.17.0 release readiness run, corpus
    // member `crash-1233c134…`). The same oracle closes both.
    //
    // A closer between two pieces is always safe to write: every piece
    // reads as exactly one section from a neutral start
    // (`section_boundaries` guarantees it, reading each section the
    // way it is written here), and the verified closer leaves exactly
    // that start for the next piece. The mirror case, a closer that
    // changed how the next piece read because that piece had been read
    // inside a block the previous one left open, is settled at the
    // splitter, not judged here (fuzz findings, corpus members
    // `crash-c9e7bbf7…` and `crash-ce631bf5…`).
    let mut joined = String::new();
    for piece in parts {
        if joined.is_empty() {
            joined = piece;
        } else {
            joined.push_str("\n\n");
            joined.push_str(&piece);
        }
        if let Some(closer) = crate::markdown::closing_context_if_unterminated(&joined) {
            joined.push('\n');
            joined.push_str(&closer);
        }
    }
    joined
}

// ---------------------------------------------------------------------------
// Relationships
// ---------------------------------------------------------------------------

/// Parse typed relationships from the Relationships section.
///
/// Recognises two row shapes:
/// - simple: `- **TYPE**: [[target]]` → `description: None`
/// - em-dash: `- **TYPE**: [[target]] — text` → `description: Some(text)`
///
/// Returns the relations plus parse-time warnings flagging
/// AMBIGUOUS-delimiter rows (`-- text`, `- text`, en-dash, minus). On
/// AMBIGUOUS rows the description is dropped — the renderer will
/// normalise the row to the simple form on next write.
///
/// Rows inside a code block are not relationships. The scan runs over
/// the masked section body and reads every captured span from the
/// original, so a fenced or indented example of the row syntax — the
/// obvious thing to write in an entity documenting that syntax — no
/// longer becomes a live edge and an auto-stub. Without the mask this
/// path synthesised edges from links the strict validator cannot see
/// (`validator::strict::check_wiki_links` masks), which is exactly the
/// asymmetry the one-definition rule exists to prevent.
pub(crate) fn parse_relationships_with_warnings(
    text: &str,
    mem: &str,
    entity_id: Option<&EntityId>,
) -> (Vec<Relationship>, Vec<crate::ops::WarningHint>) {
    // Anchor on the canonical row prefix `- **TYPE**: [[<target>]]` and
    // capture everything that follows on the same line so the trailing
    // segment can be classified (simple, em-dash, or AMBIGUOUS).
    //
    // The target must not cross a line: a ROW is a line. A capture
    // spanning a newline only ever came from degenerate drift, and it
    // cannot round-trip — the generated multi-line token re-enters the
    // mask with different structure (a following `-` + tab line reads
    // as list-item indented code and swallows the closing `]]`), so
    // the row silently vanished one round later (fuzz finding, corpus
    // member `crash-93f0a4bd…`). Ids containing newlines can never
    // exist as entity files, so such pseudo-rows are consistently not
    // relationships in ANY round.
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?m)^\s*-\s*\*\*(\w+)\*\*:\s*\[\[([^\]\n]+)\]\](?P<tail>[^\n]*)").unwrap()
    });
    let mut relationships = Vec::new();
    let mut warnings = Vec::new();
    // Blocks AND inline spans — the same mask every link scanner uses.
    // A legitimate row's target can never sit inside a code span, so
    // masking spans costs nothing and closes the seam: with a
    // blocks-only mask a row inside a multi-line inline span stayed
    // invisible to the validator and to every extractor while still
    // building an edge and a stub. Masking preserves byte offsets, so a
    // match found in the masked copy indexes the original exactly.
    let masked = mask_code_blocks_and_spans(text);
    for cap in re.captures_iter(&masked) {
        let rel_type = text[cap.get(1).unwrap().range()].to_uppercase();
        // Read-time parsing of the ## Relationships table tolerates
        // pre-strict on-disk drift so legacy rows whose target fails
        // the wiki-link grammar continue to round-trip. The mutation
        // pipeline (`memstead_relate`, declare_relations) gates strictly
        // via `validate_relation_target_grammar`.
        let target = wiki_link_to_id_lenient(&text[cap.get(2).unwrap().range()], mem);
        // A raw target that decodes to an EMPTY path (`[[specs--]]`
        // after the self-prefix strip, `[[../]]` after decoration
        // stripping) is not a relationship: the generator would render
        // it as `[[]]`, which the row pattern cannot re-capture, so the
        // row silently vanished one round later (fuzz finding, corpus
        // member `crash-0c7207a1…`). Skipping it here mirrors how rows
        // that never match the pattern behave; both strict gates refuse
        // such targets outright.
        if target.path().is_empty() {
            continue;
        }
        let tail = cap.name("tail").map(|m| &text[m.range()]).unwrap_or("");
        let description = match classify_description_tail(tail) {
            DescriptionTail::None => None,
            DescriptionTail::EmDash(text) => Some(text),
            DescriptionTail::Ambiguous(literal) => {
                if let Some(id) = entity_id {
                    warnings.push(crate::ops::WarningHint::AmbiguousDescriptionDelimiter {
                        from: id.clone(),
                        rel_type: rel_type.clone(),
                        target: target.clone(),
                        trailing: literal,
                    });
                }
                None
            }
        };
        relationships.push(Relationship {
            rel_type,
            target,
            description,
        });
    }
    (relationships, warnings)
}

/// Classification of the per-line tail that follows `]]` on a
/// `## Relationships` row.
enum DescriptionTail {
    /// Tail is empty or whitespace-only.
    None,
    /// Tail begins with the canonical em-dash delimiter; carries the
    /// captured description text (trimmed of trailing whitespace).
    EmDash(String),
    /// Tail starts with a non-canonical dash-like delimiter (`-`,
    /// `--`, U+2013 en-dash, U+2212 minus). Carries the literal
    /// trailing content so the warning surfaces what was dropped.
    Ambiguous(String),
}

/// Inspect the post-`]]` tail of a `## Relationships` row and decide
/// what shape it takes. The em-dash delimiter is the exact three-byte
/// UTF-8 sequence of U+2014 framed by single ASCII spaces; everything
/// else falls into [`DescriptionTail::None`] or
/// [`DescriptionTail::Ambiguous`].
fn classify_description_tail(tail: &str) -> DescriptionTail {
    let trimmed_end = tail.trim_end();
    if trimmed_end.is_empty() {
        return DescriptionTail::None;
    }
    // Canonical: literal space + U+2014 + literal space + content.
    if let Some(rest) = trimmed_end.strip_prefix(" \u{2014} ") {
        if rest.is_empty() {
            return DescriptionTail::None;
        }
        return DescriptionTail::EmDash(rest.to_string());
    }
    // U+2014 directly after `]]` (no leading space) is also ambiguous
    // — the canonical form requires the framing space. Likewise an
    // em-dash with no trailing content (` — `) collapses to None.
    if let Some(rest) = trimmed_end.strip_prefix(" \u{2014}") {
        // ` —` (no trailing space, but content followed) lands here.
        return DescriptionTail::Ambiguous(format!(" \u{2014}{rest}"));
    }
    // Dash-likes: ASCII `--`, ASCII `-`, en-dash U+2013, minus U+2212.
    let starters = [" --", " -", " \u{2013}", " \u{2212}"];
    if starters
        .iter()
        .any(|prefix| trimmed_end.starts_with(prefix))
    {
        return DescriptionTail::Ambiguous(trimmed_end.to_string());
    }
    // Anything else after `]]` (e.g. inline comment, stray text) —
    // classify as ambiguous so the operator sees that content was
    // dropped rather than silently swallowed.
    DescriptionTail::Ambiguous(trimmed_end.to_string())
}

// ---------------------------------------------------------------------------
// Wiki-links
// ---------------------------------------------------------------------------

/// The `[[target]]` / `[[target|label]]` wiki-link pattern, compiled once.
///
/// The inner group is `*`, not `+`, so an empty target `[[]]` is *seen*
/// by every path — the strict validator refuses it with a typed
/// `InvalidWikiLink`, and this module's strict extractor routes it to
/// the same refusal. A pattern that cannot see `[[]]` is how one path
/// came to silently ignore what another path refused.
fn wiki_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[\[([^\]]*)\]\]").unwrap())
}

/// Extract unique mem-prefixed entity IDs from inline wiki-links,
/// strictly validating each target against the slug-form grammar.
/// Strips code blocks and inline code spans before scanning, by the
/// one CommonMark definition ([`crate::markdown`]).
///
/// Returns the deduped valid ids on success, or every refusal in the
/// scan window on failure (errors are collected, not fail-fast — the
/// agent sees every malformed link in a single round-trip).
///
/// Mutation-pipeline callers (`synthesise_alias_relations`, etc.) use
/// this strict variant and map [`WikiLinkError`] to the typed engine
/// envelope with section context. Read-side scanners that must
/// tolerate pre-strict on-disk drift use [`extract_inline_links_lenient`].
pub(crate) fn extract_inline_links(
    text: &str,
    mem: &str,
) -> Result<Vec<EntityId>, Vec<WikiLinkError>> {
    let stripped = mask_code_blocks_and_spans(text);

    let link_re = wiki_link_re();
    let mut seen = HashSet::new();
    let mut links = Vec::new();
    let mut errors = Vec::new();

    for cap in link_re.captures_iter(&stripped) {
        match wiki_link_to_id(&cap[1], mem) {
            Ok(id) => {
                if errors.is_empty() && seen.insert(id.0.clone()) {
                    links.push(id);
                }
            }
            Err(e) => errors.push(e),
        }
    }

    if errors.is_empty() {
        Ok(links)
    } else {
        Err(errors)
    }
}

/// Permissive sibling of [`extract_inline_links`] for read-side
/// scanners. Decodes every `[[...]]` token via [`wiki_link_to_id_lenient`]
/// so on-disk drift (legacy entities, archive-imports from pre-strict
/// engines, partial-mutation rollbacks) keeps flowing through dangling-
/// link reporters and graph inspectors. Mutation paths MUST NOT use this
/// helper — see [`extract_inline_links`] for the strict variant.
pub fn extract_inline_links_lenient(text: &str, mem: &str) -> Vec<EntityId> {
    let stripped = mask_code_blocks_and_spans(text);

    let link_re = wiki_link_re();
    let mut seen = HashSet::new();
    let mut links = Vec::new();

    for cap in link_re.captures_iter(&stripped) {
        // An empty target decodes to no id. The read side tolerates
        // drift by ignoring what it cannot decode; the strict side
        // refuses it (`extract_inline_links`). Both *see* it — that is
        // the part that must not diverge.
        if cap[1].is_empty() {
            continue;
        }
        let id = wiki_link_to_id_lenient(&cap[1], mem);
        if seen.insert(id.0.clone()) {
            links.push(id);
        }
    }

    links
}

// ---------------------------------------------------------------------------
// Content hash
// ---------------------------------------------------------------------------

/// Compute SHA-256 hash of content, truncated to 16 hex characters.
pub fn compute_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    crate::hex_lower(&result)[..16].to_string()
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("missing frontmatter")]
    MissingFrontmatter,
    #[error("invalid frontmatter: {0}")]
    InvalidFrontmatter(String),
    #[error("missing title")]
    MissingTitle,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// End-to-end pins for the six verified code-block misparse classes,
/// on the real entity paths: section splitting, title extraction,
/// heading spans, and wiki-link extraction. The unit-level pins for
/// the mask itself live in [`crate::markdown`]; these assert the
/// classes are actually fixed where they did damage.
#[cfg(test)]
mod commonmark_referee;

#[cfg(test)]
mod tests;
