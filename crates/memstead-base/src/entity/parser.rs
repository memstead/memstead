//! Markdown → Entity parser. Handles YAML frontmatter, sections, wiki-links.
//!
//! Key design decisions:
//! - Hand-rolled YAML frontmatter parser (NOT serde_yaml) to match JS type coercion
//! - Code blocks are masked before section/link detection to prevent false matches
//! - The parser is schema-aware: it uses the schema to determine catch-all sections

use std::collections::{HashMap, HashSet};
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

    // Mask fenced code blocks so patterns inside them are not detected
    let masked = mask_code_blocks(content);

    // Extract YAML frontmatter
    let (metadata, body, masked_body) = split_frontmatter(content, &masked)?;

    // Extract title (first # heading)
    let title = extract_title(&body).unwrap_or_else(|| id.name().to_string());

    // Split body into ## sections (match against masked, slice from original).
    // Duplicate `## Heading` lines whose slug matches a schema-declared key
    // become `DuplicateSectionHeading` warnings below; first-wins is the
    // resolution policy.
    let (sections_map, duplicate_headings) = split_sections(&body, &masked_body);

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
            .map(|s| s.as_str())
            .unwrap_or(""),
        mem,
        Some(&entity_id_for_rel_warnings),
    );

    // Build catch-all section content
    let catch_all_content = build_catch_all(&sections_map, schema);

    // Extract schema-defined section values.
    // IndexMap + this loop order is what guarantees sections iterate in the
    // schema's declared order downstream. Do not change to a HashMap.
    let mut result_sections = IndexMap::new();
    for s in &schema.sections {
        if s.catch_all {
            result_sections.insert(s.key.clone(), catch_all_content.trim().to_string());
        } else {
            let val = sections_map
                .get(s.key.as_str())
                .map(|v| v.trim().to_string())
                .unwrap_or_default();
            result_sections.insert(s.key.clone(), val);
        }
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

/// Extract the `type:` value from frontmatter without running the full parser.
///
/// Used by the loader to resolve each file's type independently — the mem
/// config's default type is only a fallback for files that don't declare one.
/// Returns None if there's no frontmatter, no `type:` line, or it's empty.
pub fn peek_type_from_frontmatter(content: &str) -> Option<String> {
    let after_open = if content.starts_with("---\r\n") {
        5
    } else if content.starts_with("---\n") {
        4
    } else {
        return None;
    };

    let close_pos = content[after_open..].find("\n---")?;
    let frontmatter = &content[after_open..after_open + close_pos];

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
    let title = extract_title(body_after_frontmatter(content));
    (title, entity_type)
}

/// Return the body slice after a leading `---`-fenced frontmatter block,
/// or the whole input when no frontmatter is present. Mirrors the
/// offset arithmetic in [`split_frontmatter`] but borrows rather than
/// allocating — the title peek only needs to scan, not own.
fn body_after_frontmatter(content: &str) -> &str {
    let after_open = if content.starts_with("---\r\n") {
        5
    } else if content.starts_with("---\n") {
        4
    } else {
        return content;
    };
    let Some(close_pos) = content[after_open..].find("\n---") else {
        return content;
    };
    let body_start = after_open + close_pos + 4; // past "\n---"
    let rest = &content[body_start..];
    rest.strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
        .unwrap_or(rest)
}

/// Split content into frontmatter metadata string and body.
/// Returns (metadata_string, body, masked_body).
fn split_frontmatter<'a>(
    content: &'a str,
    masked: &'a str,
) -> Result<(String, String, String), ParseError> {
    // Look for YAML frontmatter: ---\n...\n---
    if content.starts_with("---\n") || content.starts_with("---\r\n") {
        let after_open = if content.starts_with("---\r\n") { 5 } else { 4 };
        // Find closing ---
        if let Some(close_pos) = content[after_open..].find("\n---") {
            let meta_end = after_open + close_pos;
            let metadata = content[after_open..meta_end].to_string();
            // Body starts after the closing --- and its newline
            let body_start = meta_end + 4; // "\n---"
            let body_start = if content[body_start..].starts_with('\n') {
                body_start + 1
            } else if content[body_start..].starts_with("\r\n") {
                body_start + 2
            } else {
                body_start
            };
            let body = content[body_start..].to_string();
            let masked_body = masked[body_start..].to_string();
            return Ok((metadata, body, masked_body));
        }
    }

    // No frontmatter found — entire content is body
    Ok((String::new(), content.to_string(), masked.to_string()))
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

/// Mask fenced code blocks by replacing content with spaces (preserves line count and offsets).
/// Handles unclosed code blocks safely — they mask to end of text.
pub fn mask_code_blocks(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut result = Vec::with_capacity(lines.len());
    let mut fence: Option<String> = None;

    for line in &lines {
        if let Some(ref _f) = fence {
            // Inside a code block — check for closing fence
            let trimmed = line.trim_end();
            if trimmed.starts_with("```") {
                result.push(" ".repeat(line.len()));
                fence = None;
            } else {
                result.push(" ".repeat(line.len()));
            }
        } else {
            // Outside — check for opening fence
            if line.starts_with("```") {
                fence = Some("```".to_string());
                result.push(" ".repeat(line.len()));
            } else {
                result.push((*line).to_string());
            }
        }
    }

    result.join("\n")
}

// ---------------------------------------------------------------------------
// Section splitting
// ---------------------------------------------------------------------------

/// Tracks one schema-declared section key seen more than once on parse.
/// `key` is the slugified storage key (e.g. `realization`); `heading` is
/// the original literal text from the first occurrence (e.g. `Realization`).
/// `occurrences` counts every header line for that key — first plus
/// duplicates.
pub(super) struct DuplicateSection {
    pub key: String,
    pub heading: String,
    pub occurrences: usize,
}

/// Split body into named sections. Returns `Map<lowercase_key, content>`
/// plus a list of duplicate-heading occurrences. Duplicate headings keep
/// the first occurrence's body; subsequent occurrences are dropped from
/// the storage value entirely (no embedded `## Heading` separator). The
/// caller decides whether each duplicate becomes a `WarningHint`
/// (schema-declared keys only — catch-all repetition stays silent).
pub(super) fn split_sections(
    body: &str,
    masked_body: &str,
) -> (HashMap<String, String>, Vec<DuplicateSection>) {
    let mut sections = HashMap::new();
    let mut duplicates: HashMap<String, DuplicateSection> = HashMap::new();
    static SECTION_RE: OnceLock<Regex> = OnceLock::new();
    let section_re = SECTION_RE.get_or_init(|| Regex::new(r"(?m)^## (.+)$").unwrap());

    let matches: Vec<_> = section_re.find_iter(masked_body).collect();

    for (i, m) in matches.iter().enumerate() {
        // Extract heading name from original body (not masked)
        let heading_line = &body[m.start()..m.end()];
        let name = heading_line
            .strip_prefix("## ")
            .unwrap_or(heading_line)
            .trim();

        let content_start = m.end();
        let content_end = if i + 1 < matches.len() {
            matches[i + 1].start()
        } else {
            body.len()
        };
        let content = body[content_start..content_end].trim().to_string();
        // Schema section keys are underscore-separated (e.g. `current_state`).
        // A heading like `## Current State` must slugify to the same form so
        // schema-declared sections land in `result_sections` under the right
        // key instead of falling through to catch-all — which would break
        // canonical byte-stability for any multi-word section.
        let key = name.to_lowercase().replace(' ', "_");

        match sections.entry(key.clone()) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(content);
                duplicates.insert(
                    key.clone(),
                    DuplicateSection {
                        key: key.clone(),
                        heading: name.to_string(),
                        occurrences: 1,
                    },
                );
            }
            std::collections::hash_map::Entry::Occupied(_) => {
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

    (sections, dup_list)
}

/// Extract the title from the first `# ` heading.
fn extract_title(body: &str) -> Option<String> {
    for line in body.lines() {
        if let Some(title) = line.strip_prefix("# ") {
            return Some(title.trim().to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Heading spans (H3–H6)
// ---------------------------------------------------------------------------

/// Extract H3–H6 heading spans from each section's content. Byte offsets are
/// into the (trimmed) section string stored in `result_sections`. Code blocks
/// are masked before scanning so `### foo` inside a fenced block is ignored.
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
fn build_catch_all(sections: &HashMap<String, String>, schema: &TypeDefinition) -> String {
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
    if let Some(content) = sections.get(catch_all.key.as_str())
        && !content.is_empty()
    {
        parts.push(content.clone());
    }

    // Then add all non-schema sections (with headings reconstructed).
    // `sections: &HashMap` — iteration order is randomized per process.
    // Non-determinism is invisible today because strict ingress rejects
    // unknown sections unless the schema declares a catch-all, and then
    // only the catch-all section itself lands here (see validator-v2 R6).
    // If a future schema change lets multiple non-schema sections coexist
    // under one catch-all, switch `sections` to `IndexMap` so canonical
    // bytes stay stable.
    for (key, content) in sections {
        if !known_sections.contains(key.as_str()) && !content.is_empty() {
            let heading = format!(
                "## {}{}",
                key.chars().next().unwrap_or_default().to_uppercase(),
                &key[key.chars().next().map_or(0, |c| c.len_utf8())..]
            );
            parts.push(format!("{heading}\n{content}"));
        }
    }

    parts.join("\n\n")
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
pub(crate) fn parse_relationships_with_warnings(
    text: &str,
    mem: &str,
    entity_id: Option<&EntityId>,
) -> (Vec<Relationship>, Vec<crate::ops::WarningHint>) {
    // Anchor on the canonical row prefix `- **TYPE**: [[<target>]]` and
    // capture everything that follows on the same line so the trailing
    // segment can be classified (simple, em-dash, or AMBIGUOUS).
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?m)^\s*-\s*\*\*(\w+)\*\*:\s*\[\[([^\]]+)\]\](?P<tail>[^\n]*)").unwrap()
    });
    let mut relationships = Vec::new();
    let mut warnings = Vec::new();
    for cap in re.captures_iter(text) {
        let rel_type = cap[1].to_uppercase();
        // Read-time parsing of the ## Relationships table tolerates
        // pre-strict on-disk drift so legacy rows whose target fails
        // the wiki-link grammar continue to round-trip. The mutation
        // pipeline (`memstead_relate`, declare_relations) gates strictly
        // via `validate_relation_target_grammar`.
        let target = wiki_link_to_id_lenient(&cap[2], mem);
        let tail = cap.name("tail").map(|m| m.as_str()).unwrap_or("");
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

/// A wiki-link found in markdown content.
#[derive(Debug, Clone)]
pub struct WikiLink {
    pub target: String,
    pub label: Option<String>,
}

/// The `[[target]]` / `[[target|label]]` wiki-link pattern, compiled once.
fn wiki_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[\[([^\]]+)\]\]").unwrap())
}

/// Inline code spans (masked out before link extraction), compiled once.
fn inline_code_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`[^`]+`").unwrap())
}

/// Extract all wiki-links from markdown content.
pub fn extract_wiki_links(content: &str) -> Vec<WikiLink> {
    let re = wiki_link_re();
    re.captures_iter(content)
        .map(|cap| {
            let raw = &cap[1];
            let (target, label) = match raw.find('|') {
                Some(i) => (raw[..i].to_string(), Some(raw[i + 1..].to_string())),
                None => (raw.to_string(), None),
            };
            WikiLink { target, label }
        })
        .collect()
}

/// Extract unique mem-prefixed entity IDs from inline wiki-links,
/// strictly validating each target against the slug-form grammar.
/// Strips fenced code blocks and inline code before scanning.
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
    let stripped = mask_code_blocks(text);
    let stripped = inline_code_re().replace_all(&stripped, "");

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
    let stripped = mask_code_blocks(text);
    let stripped = inline_code_re().replace_all(&stripped, "");

    let link_re = wiki_link_re();
    let mut seen = HashSet::new();
    let mut links = Vec::new();

    for cap in link_re.captures_iter(&stripped) {
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

#[cfg(test)]
mod tests {
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
        let meta =
            parse_metadata(":\n: value\nkey7: ✓\"\nkey8: 99999999999999999999999999\nkey9: -");
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
    fn extract_wiki_links_basic() {
        let links = extract_wiki_links("See [[target]] and [[other|label]]");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "target");
        assert_eq!(links[1].target, "other");
        assert_eq!(links[1].label.as_deref(), Some("label"));
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
        let md =
            "---\ntype: spec\n---\n# Title\n\n## Identity\n\n## Identity\n\nleftover content\n";
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
        let md =
            "---\ntype: spec\n---\n# Title\n\n## Specifies\n\nfirst\n\n## Specifies\n\nsecond\n";
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
}
