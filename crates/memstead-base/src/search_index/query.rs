//! Translate a [`crate::ops::Query`] into a tantivy boolean query and execute
//! it against a single mem's index. Explanation extraction and per-term
//! snippet metadata are layered on top in [`super::snippets`].
//!
//! Semantics (mirrors the flat `Query` contract):
//! - `any: Vec<term>` → disjunction across all targeted fields per term,
//!   wrapped in a single `Must` so at least one term must match.
//! - `not: Vec<term>` → per-term `MustNot` clauses across targeted fields.
//! - `phrase: Some(s)` → tokenised `PhraseQuery` across targeted fields,
//!   wrapped in a `Must` so at least one field must contain the phrase.
//! - `field: None` → the targeted set is `title` + every indexed section.
//!   `field: Some(f)` narrows to that field only (title or section key).
//! - All-`Not` queries (no positive clause) receive an implicit `AllQuery`
//!   baseline so tantivy matches the "everything except …" intent.
//!
//! Scoring:
//! - Per-field boosts derive from the mem schema's `title_weight` and
//!   `search_weight` — maxed across types whose section key collides.
//! - Sections whose max `search_weight` across types is `0.0` are skipped
//!   for `field: None` queries (they remain reachable via explicit `field`).

use std::sync::Arc;

use memstead_schema::Schema;
use tantivy::query::{
    AllQuery, BooleanQuery, BoostQuery, EmptyQuery, Occur, PhraseQuery, Query as TantivyQuery,
    TermQuery,
};
use tantivy::schema::{Field, IndexRecordOption, Value};
use tantivy::{TantivyDocument, Term, collector::TopDocs};

use crate::entity::EntityId;
use crate::ops::Query;

use super::writer::MemIndex;

/// Execute a flat query against one mem's index. Returns `(EntityId, bm25-score)`
/// pairs for every document that matches, sorted by tantivy-native score.
///
/// `top_n` bounds how many hits to pull back; callers that need pagination
/// totals over the full result set must request a generous ceiling. With an
/// in-RAM index of ~10k entities, collecting all is microseconds.
pub fn execute_on_mem(
    mem_idx: &MemIndex,
    mem_schema: Option<&Arc<Schema>>,
    query: &Query,
    top_n: usize,
) -> tantivy::Result<Vec<(EntityId, f32)>> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let tantivy_query = build_tantivy_query(mem_idx, mem_schema, query)?;
    let reader = mem_idx.index.reader()?;
    let searcher = reader.searcher();
    let top_docs = searcher.search(
        tantivy_query.as_ref(),
        &TopDocs::with_limit(top_n.max(1)).order_by_score(),
    )?;

    let mut out = Vec::with_capacity(top_docs.len());
    for (score, addr) in top_docs {
        let doc: TantivyDocument = searcher.doc(addr)?;
        if let Some(v) = doc.get_first(mem_idx.fields.id)
            && let Some(s) = v.as_str()
        {
            out.push((EntityId(s.to_string()), score));
        }
    }
    Ok(out)
}

/// Translate a [`Query`] into the tantivy tree we execute. Public so
/// the explanation/snippet extraction layer can inspect the shape.
pub fn build_tantivy_query(
    mem_idx: &MemIndex,
    mem_schema: Option<&Arc<Schema>>,
    query: &Query,
) -> tantivy::Result<Box<dyn TantivyQuery>> {
    let target_fields = resolve_target_fields(mem_idx, mem_schema, query.field.as_deref());

    // No indexable target field (e.g. `field: "unknown"`) — the query can't
    // match anything. Return an EmptyQuery rather than building a BooleanQuery
    // with no clauses, which tantivy would reject.
    if target_fields.is_empty() {
        return Ok(Box::new(EmptyQuery));
    }

    let mut top_clauses: Vec<(Occur, Box<dyn TantivyQuery>)> = Vec::new();

    // `any` → at-least-one-term requirement; each term is itself a
    // field-disjunction (the term must match in at least one targeted field).
    if !query.any.is_empty() {
        let mut any_clauses: Vec<(Occur, Box<dyn TantivyQuery>)> = Vec::new();
        for term_str in &query.any {
            if let Some(q) = build_term_across_fields(mem_idx, &target_fields, term_str)? {
                any_clauses.push((Occur::Should, q));
            }
        }
        if !any_clauses.is_empty() {
            top_clauses.push((Occur::Must, Box::new(BooleanQuery::new(any_clauses))));
        }
    }

    // `not` → per-term exclusion across the same target fields. Each exclusion
    // is itself a field-disjunction so a match in any targeted field drops
    // the doc.
    for term_str in &query.not {
        if let Some(q) = build_term_across_fields(mem_idx, &target_fields, term_str)? {
            top_clauses.push((Occur::MustNot, q));
        }
    }

    // `phrase` → tokenised `PhraseQuery` per targeted field, combined as a
    // disjunction under a top-level Must so the phrase must appear in at
    // least one field.
    if let Some(phrase) = &query.phrase {
        let mut phrase_clauses: Vec<(Occur, Box<dyn TantivyQuery>)> = Vec::new();
        for (field, boost) in &target_fields {
            if let Some(pq) = build_phrase_query_for_field(mem_idx, *field, phrase, *boost)? {
                phrase_clauses.push((Occur::Should, pq));
            }
        }
        if !phrase_clauses.is_empty() {
            top_clauses.push((Occur::Must, Box::new(BooleanQuery::new(phrase_clauses))));
        }
    }

    // All `MustNot`, no positive clause: tantivy's BooleanQuery requires at
    // least one Must/Should. Add an AllQuery baseline so "everything except X"
    // works as written.
    let has_positive = top_clauses
        .iter()
        .any(|(o, _)| matches!(o, Occur::Must | Occur::Should));
    if !has_positive {
        top_clauses.push((Occur::Must, Box::new(AllQuery)));
    }

    if top_clauses.is_empty() {
        // Every clause built empty (e.g. all terms tokenised to nothing).
        // Degrade to EmptyQuery so no doc matches — matches the caller's
        // "empty-query ⇒ no hits" intent without crashing tantivy.
        Ok(Box::new(EmptyQuery))
    } else {
        Ok(Box::new(BooleanQuery::new(top_clauses)))
    }
}

/// Build a single-term query that matches the token in any of the targeted
/// fields. Multi-token terms (after tokenisation) degrade to a phrase query
/// within each field — agents rarely pass multi-word terms into `any`
/// because they split morphology into separate variants.
fn build_term_across_fields(
    mem_idx: &MemIndex,
    target_fields: &[(Field, f32)],
    term_str: &str,
) -> tantivy::Result<Option<Box<dyn TantivyQuery>>> {
    let mut clauses: Vec<(Occur, Box<dyn TantivyQuery>)> = Vec::new();
    for (field, boost) in target_fields {
        if let Some(q) = build_field_term_query(mem_idx, *field, term_str, *boost)? {
            clauses.push((Occur::Should, q));
        }
    }
    if clauses.is_empty() {
        Ok(None)
    } else if clauses.len() == 1 {
        // Single clause — unwrap to avoid a pointless BooleanQuery wrap.
        Ok(Some(clauses.into_iter().next().unwrap().1))
    } else {
        Ok(Some(Box::new(BooleanQuery::new(clauses))))
    }
}

/// Build a single-field query for one term. Tokenises with the field's
/// analyzer (the memstead tokenizer ⇒ lowercase + ASCII-fold, no stemming) and
/// builds a `TermQuery` when one token falls out, a `PhraseQuery` for
/// multi-token terms (rare — e.g. "client side agent" accidentally pasted
/// into `any`). Zero tokens after normalization ⇒ `None`.
fn build_field_term_query(
    mem_idx: &MemIndex,
    field: Field,
    term_str: &str,
    boost: f32,
) -> tantivy::Result<Option<Box<dyn TantivyQuery>>> {
    let tokens = tokenize(mem_idx, field, term_str)?;
    if tokens.is_empty() {
        return Ok(None);
    }
    let query: Box<dyn TantivyQuery> = if tokens.len() == 1 {
        Box::new(TermQuery::new(
            Term::from_field_text(field, &tokens[0]),
            IndexRecordOption::WithFreqsAndPositions,
        ))
    } else {
        let terms: Vec<Term> = tokens
            .iter()
            .map(|t| Term::from_field_text(field, t))
            .collect();
        Box::new(PhraseQuery::new(terms))
    };
    Ok(Some(apply_boost(query, boost)))
}

/// Build a phrase query within one field. Empty tokens list ⇒ `None`; a
/// single token degenerates to a `TermQuery` (PhraseQuery::new panics with
/// fewer than two terms).
fn build_phrase_query_for_field(
    mem_idx: &MemIndex,
    field: Field,
    phrase: &str,
    boost: f32,
) -> tantivy::Result<Option<Box<dyn TantivyQuery>>> {
    let tokens = tokenize(mem_idx, field, phrase)?;
    if tokens.is_empty() {
        return Ok(None);
    }
    let query: Box<dyn TantivyQuery> = if tokens.len() == 1 {
        Box::new(TermQuery::new(
            Term::from_field_text(field, &tokens[0]),
            IndexRecordOption::WithFreqsAndPositions,
        ))
    } else {
        let terms: Vec<Term> = tokens
            .iter()
            .map(|t| Term::from_field_text(field, t))
            .collect();
        Box::new(PhraseQuery::new(terms))
    };
    Ok(Some(apply_boost(query, boost)))
}

fn apply_boost(query: Box<dyn TantivyQuery>, boost: f32) -> Box<dyn TantivyQuery> {
    if (boost - 1.0).abs() > f32::EPSILON {
        Box::new(BoostQuery::new(query, boost))
    } else {
        query
    }
}

/// Tokenise a free-form string against one field's analyzer so query terms
/// follow the exact pipeline used at index time. Returns lowercased + ASCII-
/// folded tokens in document order.
fn tokenize(mem_idx: &MemIndex, field: Field, text: &str) -> tantivy::Result<Vec<String>> {
    let mut analyzer = mem_idx.index.tokenizer_for_field(field)?;
    let mut stream = analyzer.token_stream(text);
    let mut tokens = Vec::new();
    while stream.advance() {
        tokens.push(stream.token().text.clone());
    }
    Ok(tokens)
}

/// Resolve the set of tantivy text fields a query should target, paired
/// with each field's boost. `query_field: Some(f)` restricts to exactly
/// that field (or no fields if the key is unknown). `None` returns the
/// title plus every section whose max `search_weight` across types is
/// greater than zero.
fn resolve_target_fields(
    mem_idx: &MemIndex,
    mem_schema: Option<&Arc<Schema>>,
    query_field: Option<&str>,
) -> Vec<(Field, f32)> {
    match query_field {
        Some("title") => vec![(mem_idx.fields.title, title_weight(mem_schema).max(1.0))],
        Some(key) => mem_idx
            .fields
            .sections
            .get(key)
            .map(|&f| vec![(f, section_weight(key, mem_schema).max(1.0))])
            .unwrap_or_default(),
        None => {
            let mut out = Vec::with_capacity(mem_idx.fields.sections.len() + 1);
            out.push((mem_idx.fields.title, title_weight(mem_schema).max(1.0)));
            for (key, &f) in &mem_idx.fields.sections {
                let w = section_weight(key, mem_schema);
                if w > 0.0 {
                    out.push((f, w));
                }
            }
            out
        }
    }
}

fn title_weight(schema: Option<&Arc<Schema>>) -> f32 {
    schema
        .map(|s| {
            s.types
                .values()
                .map(|t| t.title_weight)
                .fold(0.0_f32, f32::max)
        })
        .unwrap_or(1.0)
}

fn section_weight(key: &str, schema: Option<&Arc<Schema>>) -> f32 {
    schema
        .map(|s| {
            s.types
                .values()
                .filter_map(|t| t.section(key))
                .map(|sec| sec.search_weight)
                .fold(0.0_f32, f32::max)
        })
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{Entity, EntityId};
    use crate::ops::Query;
    use indexmap::IndexMap;
    use std::collections::HashMap;

    fn make_entity(name: &str, mem: &str, identity: &str) -> Entity {
        let mut sections = IndexMap::new();
        sections.insert("identity".into(), identity.to_string());
        sections.insert("purpose".into(), format!("Purpose of {name}."));
        Entity {
            id: EntityId::new(mem, name),
            title: name.to_string(),
            entity_type: "spec".into(),
            mem: mem.into(),
            file_path: format!("{name}.md"),
            metadata: IndexMap::new(),
            sections,
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: HashMap::new(),
        }
    }

    fn build_idx(entities: &[Entity]) -> (MemIndex, Arc<Schema>) {
        let schema = Schema::builtin_default();
        let mut idx = MemIndex::build_in_ram("specs".into(), Some(&schema)).unwrap();
        for e in entities {
            idx.index_entity(e).unwrap();
        }
        idx.commit().unwrap();
        (idx, schema)
    }

    #[test]
    fn any_term_returns_entity_matching_in_any_field() {
        let entities = vec![
            make_entity("alpha", "specs", "uses graph database"),
            make_entity("beta", "specs", "unrelated content"),
        ];
        let (idx, schema) = build_idx(&entities);
        let q = Query {
            any: vec!["graph".into()],
            ..Default::default()
        };
        let hits = execute_on_mem(&idx, Some(&schema), &q, 100).unwrap();
        assert_eq!(hits.len(), 1, "only alpha mentions `graph`: {hits:?}");
        assert_eq!(hits[0].0.name(), "alpha");
    }

    #[test]
    fn not_term_excludes_entity() {
        let entities = vec![
            make_entity("alpha", "specs", "uses graph database"),
            make_entity("beta", "specs", "uses graph but is a mock"),
        ];
        let (idx, schema) = build_idx(&entities);
        let q = Query {
            any: vec!["graph".into()],
            not: vec!["mock".into()],
            ..Default::default()
        };
        let hits = execute_on_mem(&idx, Some(&schema), &q, 100).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.name(), "alpha");
    }

    #[test]
    fn phrase_requires_adjacency() {
        let entities = vec![
            make_entity("alpha", "specs", "uses the graph database directly"),
            make_entity("beta", "specs", "graph lookups reach a database table"),
        ];
        let (idx, schema) = build_idx(&entities);
        let q = Query {
            phrase: Some("graph database".into()),
            ..Default::default()
        };
        let hits = execute_on_mem(&idx, Some(&schema), &q, 100).unwrap();
        assert_eq!(hits.len(), 1, "only alpha has adjacent words: {hits:?}");
        assert_eq!(hits[0].0.name(), "alpha");
    }

    #[test]
    fn field_restricts_match_region() {
        let mut beta = make_entity("beta", "specs", "unrelated");
        beta.sections
            .insert("rationale".into(), "graph reasoning details".into());
        let alpha = make_entity("alpha", "specs", "uses graph database");
        let entities = vec![alpha, beta];
        let (idx, schema) = build_idx(&entities);
        let q = Query {
            any: vec!["graph".into()],
            field: Some("identity".into()),
            ..Default::default()
        };
        let hits = execute_on_mem(&idx, Some(&schema), &q, 100).unwrap();
        assert_eq!(hits.len(), 1, "only identity-field matches count");
        assert_eq!(hits[0].0.name(), "alpha");
    }

    #[test]
    fn diacritic_folding_is_symmetric() {
        let entities = vec![make_entity("alpha", "specs", "Schöne Häuser hier")];
        let (idx, schema) = build_idx(&entities);
        let q = Query {
            any: vec!["hauser".into()],
            ..Default::default()
        };
        let hits = execute_on_mem(&idx, Some(&schema), &q, 100).unwrap();
        assert_eq!(hits.len(), 1, "ASCII fold must match `Häuser`");
    }

    #[test]
    fn empty_query_returns_no_hits() {
        let entities = vec![make_entity("alpha", "specs", "whatever")];
        let (idx, schema) = build_idx(&entities);
        let hits = execute_on_mem(&idx, Some(&schema), &Query::default(), 100).unwrap();
        assert!(hits.is_empty(), "empty query ⇒ no hits");
    }

    #[test]
    fn only_not_matches_everything_except_excluded() {
        let entities = vec![
            make_entity("alpha", "specs", "contains mock"),
            make_entity("beta", "specs", "contains no trigger word"),
        ];
        let (idx, schema) = build_idx(&entities);
        let q = Query {
            not: vec!["mock".into()],
            ..Default::default()
        };
        let hits = execute_on_mem(&idx, Some(&schema), &q, 100).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.name(), "beta");
    }

    #[test]
    fn unknown_field_returns_no_hits() {
        let entities = vec![make_entity("alpha", "specs", "anything")];
        let (idx, schema) = build_idx(&entities);
        let q = Query {
            any: vec!["anything".into()],
            field: Some("this_field_does_not_exist".into()),
            ..Default::default()
        };
        let hits = execute_on_mem(&idx, Some(&schema), &q, 100).unwrap();
        assert!(hits.is_empty());
    }
}
