You are grading two anonymous answers, A and B, to one question about Memstead. You do not know who wrote them and must not guess; ignore any stray hint of where an answer came from and judge content only.

Verify every checkable claim in both answers against the engine's Rust source under `<source>/crates` and, where a claim is about behaviour, by running the release binary `<memstead>` in a scratch workspace of your own under `<run>/grader-N3` (create it with `<memstead> init --name probe --schema default@1.3.0 <run>/grader-N3/probe` or `<memstead> quickstart <run>/grader-N3/probe`; never touch any other workspace or repository).

Return ONLY a JSON object with exactly these keys:
- "winner": "A", "B" or "tie"
- "margin": 1, 2 or 3 (1 close, 3 decisive; 0 for a tie)
- "scores": {"A": {"correctness": 0-5, "completeness": 0-5, "usefulness": 0-5, "honesty": 0-5}, "B": {same}}
- "errors_A": a list of strings, each one false or unverifiable claim in A with what the source or the binary shows instead; empty if none
- "errors_B": the same for B
- "rationale": under 120 words

Correctness is truth against the source and the binary; completeness is whether the question's parts are all answered; usefulness is whether a developer could act on it; honesty is whether the answer hedges what it could not establish and invents nothing.

Question:

My searches over a mem feel too literal. How do I turn on semantic (vector) search in Memstead and choose the embedding model? If that is not how it works, tell me how search actually ranks results and what I should do instead to find things by meaning.

# Answer A

There is no semantic or vector search to turn on, and no embedding model to choose. Memstead search is lexical: each writable mem gets an in-RAM tantivy full-text index, and ranking is tantivy-native BM25. The index does exact-token matching with no stemming and no synonym or semantic expansion; the caller is expected to enumerate morphological and synonym variants itself. Your searches feel literal because they are.

How ranking works in practice: the query is `{ any[], not[], phrase?, field? }`. Terms in `any` are OR-ed, and a hit that matches more of them scores higher. Per-field boosts come from the schema's `title_weight` and `search_weight`, so a title match outweighs a body match, and sections with a weight of 0 are skipped unless you target them explicitly with `field`. Metadata keys and values are indexed as one tokenized blob too, so a metadata-only hit is possible. Each hit returns `matched_terms` and a `score_breakdown` (bm25, title boost, field weights); treat the breakdown as proportions, not exact arithmetic. Stubs are never indexed, and read-only mems currently have no index at all.

What to do instead to find things by meaning:

1. Widen the lexical net: put every synonym and word form you can think of into `any`.
2. Use the graph. `related_to` plus `depth` returns an anchor entity's undirected neighbourhood ranked by proximity (nearer hops first, typed edges before co-mentions), capped at 100 with a `NEIGHBOURHOOD_CAPPED` warning. `expand_via[]` plus `expand_depth` pulls neighbours of your primary hits over chosen relationship types, with scores decayed by 0.5 per hop.
3. Narrow structurally with `entity_type`, `mem`, equality `filters` and `range_filters` on schema-declared filterable fields; ignored filters warn rather than silently emptying results.
4. Start from the overview: it reports Louvain community clusters, which group entities by graph structure and give you topical entry points without any keyword.

One caveat: search is unavailable in the wasm build entirely.

# Answer B

There is no semantic or vector search to turn on, and no embedding model to choose. The engine has no embedding, vector, or similarity code path anywhere, and no configuration key for one; the only search machinery is a per-mem, in-RAM tantivy full-text index. Your searches feel literal because they are literal by design.

How ranking actually works: every query becomes a tantivy boolean query and hits are ordered by BM25 score. Text is tokenised by splitting on non-letters, lowercasing, and ASCII-folding diacritics; there is deliberately no stemming, so "running" does not match "run", though "Häuser" does match "hauser". The query shape is flat: `any` (OR across terms, more matching terms rank higher), `not` (exclusion), `phrase` (exact adjacency), and `field` to narrow to the title or one section key. Fields are weighted from the schema: each type's `title_weight` and each section's `search_weight` become boosts, maxed across types sharing a key; a section weighted 0 is skipped unless you target it by `field` explicitly; a low-weight metadata keys-and-values field is also searched. Each hit reports `matched_terms` and an illustrative `score_breakdown` so you can see which fields carried the score.

What to do instead to find things by meaning:

1. Enumerate variants yourself in `any`: morphological forms and synonyms ("auth", "authentication", "login", "sign-in"). The engine expects the caller to do this.
2. Use the graph as the semantic layer. `expand_via` follows named relationship types from your text hits to their neighbours, scoring them at `0.5^depth`, and `related_to` with `depth` and `direction` returns a proximity-ranked neighbourhood around a known entity.
3. Combine text with structural filters (`entity_type`, `filters`, `range_filters`, `edge_type`) to cut noise rather than relying on the score alone.

If you need true embedding search, you would have to build it outside the engine over the markdown files; nothing in the engine can host it today.
