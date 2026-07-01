//! Post-processing — per-term snippets, heading-path resolution,
//! and proportional score breakdowns.
//!
//! The tantivy query layer proves a hit exists and returns the total BM25
//! score. This module turns each hit into agent-facing feedback: which terms
//! matched which fields, what the context looks like, and how the score
//! distributes across title vs section weights.
//!
//! Matching uses a case-folded + ASCII-folded substring find (not tantivy's
//! positions) because we need byte offsets into the *raw* section content to
//! look up heading spans. Tantivy's token-level positions don't trivially
//! map back to raw offsets once folding is applied. The fallback is robust
//! for the common cases; terms whose only match requires tokenizer-aware
//! adjacency (e.g. split-on-punctuation) simply don't produce a snippet for
//! that field — the tantivy hit still stands.
//!
//! Score breakdown is illustrative. We allocate the hit's score proportionally
//! across matched title + section weights so the components sum to the total
//! within floating-point tolerance. It is illustrative feedback, not a
//! numerically authoritative decomposition — the fractions show agents which
//! fields contributed, not the exact BM25 math.

use std::collections::{HashMap, HashSet};

use memstead_schema::TypeDefinition;

use crate::entity::{Entity, HeadingSpan};
use crate::ops::{Query, ScoreBreakdown, TermMatch};

/// At most this many distinct query terms produce a `matched_terms` entry
/// on one hit. Protects against large `any` lists blowing up the response.
const MAX_DISTINCT_TERMS: usize = 5;

/// Context window (in characters) on each side of a match for snippet
/// rendering.
const SNIPPET_CONTEXT_CHARS: usize = 50;

/// Compute `matched_terms` for one hit. Iterates the positive terms from the
/// query (`any` list + synthetic `phrase` entry) against every field the
/// query could target, building at most one `TermMatch` per (term, field)
/// pair. Returns `None` when no positive predicate produced a match — keeps
/// the wire shape lean.
pub fn compute_matched_terms(
    entity: &Entity,
    query: &Query,
) -> Option<HashMap<String, Vec<TermMatch>>> {
    let positive_terms = collect_positive_terms(query);
    if positive_terms.is_empty() {
        return None;
    }

    let target_fields = resolve_matched_term_fields(entity, query.field.as_deref());
    if target_fields.is_empty() {
        return None;
    }

    let mut out: HashMap<String, Vec<TermMatch>> = HashMap::new();
    for term in positive_terms.iter().take(MAX_DISTINCT_TERMS) {
        let mut matches: Vec<TermMatch> = Vec::new();
        for (field_name, content) in &target_fields {
            if let Some((start, end)) = find_folded(content, term) {
                let snippet = build_snippet_at(content, start, end);
                let heading_path = resolve_heading_path(entity, field_name, start);
                matches.push(TermMatch {
                    field: (*field_name).to_string(),
                    snippet,
                    heading_path,
                });
            }
        }
        if !matches.is_empty() {
            out.insert(term.clone(), matches);
        }
    }

    if out.is_empty() { None } else { Some(out) }
}

/// Proportionally allocate the hit's total `score` across matched fields,
/// using the schema's `title_weight` and per-section `search_weight` as the
/// distribution basis. Components sum to `score` within floating-point
/// tolerance.
///
/// When no field-level weights are available (e.g. the hit matched a
/// section whose `search_weight` is 0, or the hit came from an unknown
/// entity type), the breakdown collapses to `bm25 = score` so the sum
/// still equals `score`.
pub fn compute_score_breakdown(
    schema: &TypeDefinition,
    score: f32,
    matched_terms: &Option<HashMap<String, Vec<TermMatch>>>,
) -> ScoreBreakdown {
    let matched_fields: HashSet<&str> = matched_terms
        .as_ref()
        .map(|m| {
            m.values()
                .flat_map(|v| v.iter().map(|tm| tm.field.as_str()))
                .collect()
        })
        .unwrap_or_default();

    let title_weight = if matched_fields.contains("title") {
        schema.title_weight.max(0.0)
    } else {
        0.0
    };

    let mut field_weights_raw: HashMap<String, f32> = HashMap::new();
    for section_def in &schema.sections {
        if matched_fields.contains(section_def.key.as_str()) {
            let w = section_def.search_weight.max(0.0);
            if w > 0.0 {
                field_weights_raw.insert(section_def.key.clone(), w);
            }
        }
    }

    let total_weight: f32 = title_weight + field_weights_raw.values().sum::<f32>();
    if total_weight > 0.0 {
        let unit = score / total_weight;
        ScoreBreakdown {
            bm25: 0.0,
            title_boost: title_weight * unit,
            field_weights: field_weights_raw
                .into_iter()
                .map(|(k, v)| (k, v * unit))
                .collect(),
            expansion_decay: None,
        }
    } else {
        ScoreBreakdown {
            bm25: score,
            title_boost: 0.0,
            field_weights: HashMap::new(),
            expansion_decay: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Positive-term extraction + target field resolution
// ---------------------------------------------------------------------------

/// Collect terms we feed into snippet matching — the `any` list plus the
/// `phrase` as a single synthetic term. `not` is intentionally excluded
/// because negative predicates aren't feedback-loop targets.
fn collect_positive_terms(query: &Query) -> Vec<String> {
    let mut terms: Vec<String> = Vec::with_capacity(query.any.len() + 1);
    for t in &query.any {
        if !t.trim().is_empty() {
            terms.push(t.clone());
        }
    }
    if let Some(p) = &query.phrase
        && !p.trim().is_empty()
    {
        terms.push(p.clone());
    }
    terms
}

/// Fields to scan for per-term snippets. Respects `query.field` when set;
/// `None` returns `("title", title)` plus every `(section_key, content)`
/// the entity carries (schema + catch-all sections are both included).
fn resolve_matched_term_fields<'a>(
    entity: &'a Entity,
    query_field: Option<&str>,
) -> Vec<(&'a str, &'a str)> {
    match query_field {
        Some("title") => vec![("title", entity.title.as_str())],
        Some(key) => entity
            .sections
            .get_key_value(key)
            .map(|(k, c)| vec![(k.as_str(), c.as_str())])
            .unwrap_or_default(),
        None => {
            let mut out: Vec<(&str, &str)> = Vec::with_capacity(entity.sections.len() + 1);
            out.push(("title", entity.title.as_str()));
            for (key, content) in &entity.sections {
                if !content.is_empty() {
                    out.push((key.as_str(), content.as_str()));
                }
            }
            out
        }
    }
}

// ---------------------------------------------------------------------------
// Heading-path resolution
// ---------------------------------------------------------------------------

/// Given a match byte-offset inside one section's raw content, return the
/// `[outermost, …, innermost]` H3–H6 heading chain containing it. `None`
/// when the match is above any sub-heading, or when the field is the title
/// (no heading spans apply).
fn resolve_heading_path(
    entity: &Entity,
    field_name: &str,
    match_start: usize,
) -> Option<Vec<String>> {
    if field_name == "title" {
        return None;
    }
    let spans = entity.heading_spans.get(field_name)?;
    let containing: Vec<&HeadingSpan> = spans
        .iter()
        .filter(|s| s.start_offset <= match_start && match_start < s.end_offset)
        .collect();
    if containing.is_empty() {
        return None;
    }
    let mut sorted = containing;
    // Level ascending = outermost first (H3 before H4 before H5).
    sorted.sort_by_key(|s| s.level);
    Some(sorted.iter().map(|s| s.title.clone()).collect())
}

// ---------------------------------------------------------------------------
// Folded substring match + snippet rendering
// ---------------------------------------------------------------------------

/// Case- and diacritic-folded substring search. Returns `(start, end)` byte
/// offsets in `haystack` for the first match, or `None`.
fn find_folded(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let (hay_folded, positions) = fold_with_map(haystack);
    let (needle_folded, _) = fold_with_map(needle);
    if needle_folded.is_empty() {
        return None;
    }
    let start_folded = hay_folded.find(&needle_folded)?;
    let end_folded = start_folded + needle_folded.len();

    let start_raw = positions
        .get(start_folded)
        .copied()
        .unwrap_or_else(|| haystack.len());
    let end_raw = if end_folded < positions.len() {
        positions[end_folded]
    } else {
        haystack.len()
    };
    Some((start_raw, end_raw))
}

/// Fold `s` (lowercase + ASCII-fold diacritics) into a new string and
/// record, for every byte of the folded output, the byte offset in `s`
/// where the source char started. Used to map substring matches back from
/// the folded form to raw byte offsets.
fn fold_with_map(s: &str) -> (String, Vec<usize>) {
    let mut folded = String::with_capacity(s.len());
    let mut positions: Vec<usize> = Vec::with_capacity(s.len());
    for (byte_pos, ch) in s.char_indices() {
        for fc in fold_char(ch).chars() {
            let n = fc.len_utf8();
            folded.push(fc);
            for _ in 0..n {
                positions.push(byte_pos);
            }
        }
    }
    (folded, positions)
}

/// Map one char to its lowercased + ASCII-folded form. Keeps ASCII output
/// when possible so `find_folded` can search with `String::find`. Common
/// Latin diacritics and German eszett get explicit mappings; other chars
/// fall back to `char::to_lowercase`.
fn fold_char(ch: char) -> String {
    match ch {
        'ä' | 'Ä' | 'á' | 'Á' | 'à' | 'À' | 'â' | 'Â' | 'ã' | 'Ã' | 'å' | 'Å' => "a".into(),
        'ö' | 'Ö' | 'ó' | 'Ó' | 'ò' | 'Ò' | 'ô' | 'Ô' | 'õ' | 'Õ' | 'ø' | 'Ø' => "o".into(),
        'ü' | 'Ü' | 'ú' | 'Ú' | 'ù' | 'Ù' | 'û' | 'Û' => "u".into(),
        'é' | 'É' | 'è' | 'È' | 'ê' | 'Ê' | 'ë' | 'Ë' => "e".into(),
        'í' | 'Í' | 'ì' | 'Ì' | 'î' | 'Î' | 'ï' | 'Ï' => "i".into(),
        'ñ' | 'Ñ' => "n".into(),
        'ç' | 'Ç' => "c".into(),
        'ß' => "ss".into(),
        'æ' | 'Æ' => "ae".into(),
        'œ' | 'Œ' => "oe".into(),
        _ => ch.to_lowercase().collect(),
    }
}

/// Render a snippet around a match: at most `SNIPPET_CONTEXT_CHARS` chars
/// of context before and after, char-boundary safe, with the match wrapped
/// in `**bold**` and ellipses on any truncated edge.
fn build_snippet_at(content: &str, match_start: usize, match_end: usize) -> String {
    if match_start > match_end || match_end > content.len() {
        return String::new();
    }
    let start = content[..match_start]
        .char_indices()
        .rev()
        .nth(SNIPPET_CONTEXT_CHARS - 1)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let end = content[match_end..]
        .char_indices()
        .nth(SNIPPET_CONTEXT_CHARS)
        .map(|(i, _)| match_end + i)
        .unwrap_or(content.len());

    let prefix = if start > 0 { "..." } else { "" };
    let suffix = if end < content.len() { "..." } else { "" };
    let before = &content[start..match_start];
    let matched = &content[match_start..match_end];
    let after = &content[match_end..end];
    format!("{prefix}{before}**{matched}**{after}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_with_map_roundtrip_ascii() {
        let (f, p) = fold_with_map("Hello");
        assert_eq!(f, "hello");
        assert_eq!(p, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn fold_with_map_german_umlaut() {
        // "Häuser": H(0) ä(1-2) u(3) s(4) e(5) r(6)
        let (f, p) = fold_with_map("Häuser");
        assert_eq!(f, "hauser");
        assert_eq!(p.len(), 6);
        assert_eq!(p[0], 0); // h ← H
        assert_eq!(p[1], 1); // a ← ä (starts at byte 1)
        assert_eq!(p[2], 3); // u ← u (ä took bytes 1-2)
        assert_eq!(p[3], 4);
        assert_eq!(p[4], 5);
        assert_eq!(p[5], 6);
    }

    #[test]
    fn fold_with_map_eszett_expands() {
        // "Straße": S(0) t(1) r(2) a(3) ß(4-5) e(6)
        let (f, p) = fold_with_map("Straße");
        assert_eq!(f, "strasse");
        // Two consecutive positions both point at the ß start (byte 4).
        assert_eq!(p[4], 4);
        assert_eq!(p[5], 4);
        assert_eq!(p[6], 6);
    }

    #[test]
    fn find_folded_ascii() {
        let (s, e) = find_folded("hello world", "world").unwrap();
        assert_eq!(&"hello world"[s..e], "world");
    }

    #[test]
    fn find_folded_case_insensitive() {
        let (s, e) = find_folded("Hello World", "world").unwrap();
        assert_eq!(&"Hello World"[s..e], "World");
    }

    #[test]
    fn find_folded_diacritic() {
        let (s, e) = find_folded("Schöne Häuser hier", "hauser").unwrap();
        assert_eq!(&"Schöne Häuser hier"[s..e], "Häuser");
    }

    #[test]
    fn find_folded_no_match() {
        assert!(find_folded("hello", "xyz").is_none());
    }

    #[test]
    fn build_snippet_at_wraps_match() {
        let s = build_snippet_at("the graph engine processes queries", 4, 9);
        assert!(s.contains("**graph**"));
    }
}
