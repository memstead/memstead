//! Token-budget chunking for large MCP responses.

/// Estimate token count from a string (rough: chars / 4).
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count() / 4
}

/// Minimum transport chunk budget for surfaces that reuse a *content*
/// budget as the chunk size — the `overview` CLI passes its
/// `--token-budget` straight into [`apply_chunking`]. A tiny content
/// budget should shrink *what is included* (the composer's greedy-fill
/// already drops heavy content), not fragment the always-shipped
/// hard-required body into hundreds of mid-word pieces. So the chunk
/// size is floored here while the content budget stays as the caller
/// set it.
///
/// Deliberately NOT applied to `memstead_entity`'s `token_budget`: there the
/// value IS an explicit transport cap the agent set on the text channel,
/// so honouring small values (chunking) is the contract — see
/// `apply_chunking` callers in the entity read path.
pub const MIN_TRANSPORT_CHUNK_BUDGET: usize = 4096;

/// Floor a requested chunk budget at [`MIN_TRANSPORT_CHUNK_BUDGET`] so a
/// sub-floor budget ships small bodies as one chunk instead of
/// fragmenting them.
pub fn floor_chunk_budget(requested: usize) -> usize {
    requested.max(MIN_TRANSPORT_CHUNK_BUDGET)
}

/// Split a large response into chunks that fit within a token budget.
/// Splits at the nearest newline boundary to avoid breaking mid-syntax.
/// Returns `None` if the content fits within the budget (no chunking needed).
pub fn chunk_markdown(markdown: &str, budget: usize) -> Option<Vec<String>> {
    let char_budget = budget * 4;
    if markdown.len() <= char_budget {
        return None;
    }

    let mut chunks = Vec::new();
    let mut remaining = markdown;

    while !remaining.is_empty() {
        if remaining.len() <= char_budget {
            chunks.push(remaining.to_string());
            break;
        }
        // Round the byte budget down to the nearest char boundary so the
        // initial slice never lands inside a multi-byte UTF-8 character.
        let safe_budget = remaining.floor_char_boundary(char_budget);
        // If the budget rounds down to 0 (the caller asked for budget=0,
        // or a single multi-byte char wider than `char_budget` sits at
        // position 0), there is no usable prefix at all. Emit the whole
        // remaining content as a final chunk — the engine never panics
        // under any input, and `_overview_mode: overbudget` already
        // signals to callers that they're below the productive range.
        if safe_budget == 0 {
            chunks.push(remaining.to_string());
            break;
        }
        // `rfind('\n')` on a char-bounded prefix lands on a `\n` byte
        // (1-byte ASCII) so `split + 1` is also a valid char boundary.
        // When rfind fails we split at `safe_budget` and advance to the
        // same offset — no `+1`, since there's no newline byte to skip.
        let (split_at, advance) = match remaining[..safe_budget]
            .rfind('\n')
            .filter(|&i| i > 0)
        {
            Some(i) => (i, i + 1),
            None => (safe_budget, safe_budget),
        };
        chunks.push(remaining[..split_at].to_string());
        remaining = &remaining[advance..];
    }

    Some(chunks)
}

/// Apply chunking to a markdown response. Returns the chunk at `idx` (0-based)
/// with appropriate frontmatter metadata.
///
/// `_chunk: N` and `_total_chunks: M` are always injected (including
/// the `1 of 1` case) so an agent walking the surface can size
/// pagination without first peeking at the response length. A request
/// for `chunk > total_chunks` is always an error — even when the body
/// fits in a single chunk and the silent-cap behaviour would have
/// hidden the overshoot.
pub fn apply_chunking(
    markdown: &str,
    budget: usize,
    chunk: Option<usize>,
    extra_fm: &[(&str, &str)],
) -> Result<String, String> {
    let chunks_opt = chunk_markdown(markdown, budget);
    let total = chunks_opt.as_ref().map(|c| c.len()).unwrap_or(1);
    let idx = chunk.unwrap_or(1).saturating_sub(1);
    if idx >= total {
        return Err(format!(
            "Chunk {} does not exist. Content has {} chunk{s}.",
            idx + 1,
            total,
            s = if total == 1 { "" } else { "s" },
        ));
    }

    // Single-chunk case: preserve the original frontmatter and body
    // verbatim; only inject the chunk-walk signals so an agent can
    // size pagination without first peeking at the response length.
    if chunks_opt.is_none() {
        return Ok(inject_chunk_frontmatter(markdown, 1, 1, false));
    }

    let chunks = chunks_opt.unwrap();
    let is_last = idx == total - 1;

    // Every chunk carries the entity-level frontmatter (`type`,
    // `level`, `stability`, `created_date`, `last_modified`,
    // `_tokens_unfiltered_body`, …) merged with caller-supplied
    // `extra_fm` (`_hash`, `_vault_schema`) and the chunk-walk signals
    // (`_truncated`, `_chunk`, `_total_chunks`). The entity frontmatter
    // is preserved on every chunk so an agent reading any single chunk
    // in isolation can answer "what kind of entity is this and when was
    // it last touched" without re-fetching chunk 1.
    let original_fm = extract_frontmatter_lines(markdown);
    let merged_fm = merge_chunk_frontmatter(
        &original_fm,
        extra_fm,
        idx + 1,
        total,
        !is_last,
    );

    let result = if idx == 0 {
        if let Some(end) = find_frontmatter_end(&chunks[idx]) {
            format!("---\n{merged_fm}\n---{}", &chunks[idx][end..])
        } else {
            format!("---\n{merged_fm}\n---\n\n{}", chunks[idx])
        }
    } else {
        format!("---\n{merged_fm}\n---\n\n{}", chunks[idx])
    };

    Ok(result)
}

/// Parse the frontmatter inner block of `markdown` into ordered
/// `(key, value)` pairs. Returns an empty vec when `markdown` has no
/// frontmatter, when the block is empty, or when a line doesn't match
/// `key: value` (those lines are skipped — the chunker is not a
/// general YAML parser and the engine's renderer only emits simple
/// scalars + bracket-delimited arrays). The order is preserved so
/// the re-emitted frontmatter on each chunk matches the source's
/// declared order.
fn extract_frontmatter_lines(markdown: &str) -> Vec<(String, String)> {
    let Some(end) = find_frontmatter_end(markdown) else {
        return Vec::new();
    };
    // `end` points past the closing `\n---`; back up 4 to land on the
    // closing marker's leading newline. The inner block starts after
    // `---\n` (4 chars from the start) and ends at `inner_end`.
    let inner_end = end - 4;
    let inner = &markdown[4..inner_end];
    inner
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                return None;
            }
            let colon = trimmed.find(':')?;
            let key = trimmed[..colon].trim().to_string();
            let value = trimmed[colon + 1..].trim_start().to_string();
            if key.is_empty() {
                None
            } else {
                Some((key, value))
            }
        })
        .collect()
}

/// Compose the merged frontmatter text for one chunk. Order: original
/// entity frontmatter (in source order), then caller-supplied
/// `extra_fm` entries (overriding any matching key from the original),
/// then engine-time chunk-walk keys (`_truncated`, `_chunk`,
/// `_total_chunks`). The chunk-walk keys are always engine-authored; if
/// `extra_fm` carries one of those keys it loses to the chunker's own
/// value.
fn merge_chunk_frontmatter(
    original: &[(String, String)],
    extra_fm: &[(&str, &str)],
    idx: usize,
    total: usize,
    truncated: bool,
) -> String {
    use indexmap::IndexMap;
    const CHUNK_WALK_KEYS: &[&str] = &["_truncated", "_chunk", "_total_chunks"];

    let mut keyed: IndexMap<String, String> = IndexMap::new();
    for (k, v) in original {
        if CHUNK_WALK_KEYS.contains(&k.as_str()) {
            continue; // Re-derived per chunk; never inherit.
        }
        keyed.insert(k.clone(), v.clone());
    }
    for (k, v) in extra_fm {
        if CHUNK_WALK_KEYS.contains(k) {
            continue;
        }
        keyed.insert((*k).to_string(), (*v).to_string());
    }
    if truncated {
        keyed.insert("_truncated".to_string(), "true".to_string());
    }
    keyed.insert("_chunk".to_string(), format!("{idx} of {total}"));
    keyed.insert("_total_chunks".to_string(), total.to_string());

    keyed
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Inject `_chunk: N of M` and `_total_chunks: M` into the markdown's
/// existing frontmatter, preserving every other key. When the body
/// has no frontmatter yet, prepend a fresh one carrying just these
/// keys. Single-chunk path only — multi-chunk uses the existing
/// caller-driven `extra_fm` rewrite shape.
fn inject_chunk_frontmatter(markdown: &str, idx: usize, total: usize, truncated: bool) -> String {
    let chunk_line = format!("_chunk: {idx} of {total}");
    let total_line = format!("_total_chunks: {total}");
    let truncated_line = if truncated { "_truncated: true\n" } else { "" };
    match find_frontmatter_end(markdown) {
        Some(end) => {
            // `end` points just past the closing `\n---`; back up 4 to
            // re-anchor on the closing marker, then trim the trailing
            // `\n` from the inner block so we can re-emit it cleanly.
            let inner_end = end - 4;
            let inner = markdown[4..inner_end].trim_end_matches('\n');
            let separator = if inner.is_empty() { "" } else { "\n" };
            format!(
                "---\n{inner}{separator}{truncated_line}{chunk_line}\n{total_line}\n---{}",
                &markdown[end..]
            )
        }
        None => format!(
            "---\n{truncated_line}{chunk_line}\n{total_line}\n---\n\n{markdown}"
        ),
    }
}

/// Find the end of YAML frontmatter (position of the closing `---` including it).
fn find_frontmatter_end(text: &str) -> Option<usize> {
    if !text.starts_with("---\n") {
        return None;
    }
    // Find closing ---
    text[4..].find("\n---").map(|pos| pos + 4 + 4) // skip opening "---\n" + matched "\n---"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_basic() {
        assert_eq!(estimate_tokens("hello world!"), 3); // 12 chars / 4
    }

    /// A sub-floor content budget is raised to the transport floor so a
    /// small body ships as one chunk; a budget already above the floor
    /// is untouched.
    #[test]
    fn floor_chunk_budget_raises_tiny_budgets_only() {
        assert_eq!(floor_chunk_budget(5), MIN_TRANSPORT_CHUNK_BUDGET);
        assert_eq!(floor_chunk_budget(0), MIN_TRANSPORT_CHUNK_BUDGET);
        assert_eq!(floor_chunk_budget(25_000), 25_000);
        // A small body chunked at the floored budget stays one chunk,
        // where chunking at the raw tiny budget would fragment it.
        let small_body = "line one\nline two\nline three\n";
        assert!(chunk_markdown(small_body, floor_chunk_budget(5)).is_none());
        assert!(chunk_markdown(small_body, 5).unwrap().len() > 1);
    }

    #[test]
    fn chunk_small_content_returns_none() {
        let text = "short";
        assert!(chunk_markdown(text, 100).is_none());
    }

    #[test]
    fn chunk_splits_at_newline_boundaries() {
        let text = "line1\nline2\nline3\nline4\nline5\n";
        // Budget of 2 tokens = 8 chars
        let chunks = chunk_markdown(text, 2).unwrap();
        assert!(chunks.len() > 1);
        // Each chunk should end at a newline boundary
        for chunk in &chunks[..chunks.len() - 1] {
            assert!(chunk.ends_with('\n') || !chunk.contains('\n'));
        }
    }

    #[test]
    fn apply_chunking_no_split_needed_injects_chunk_metadata() {
        let md = "---\n_hash: abc\n---\n\n# Title\n\nContent";
        let result = apply_chunking(md, 10000, None, &[]).unwrap();
        assert!(result.contains("_hash: abc"), "preserves existing frontmatter key");
        assert!(result.contains("_chunk: 1 of 1"), "got: {result}");
        assert!(result.contains("_total_chunks: 1"), "got: {result}");
        assert!(result.ends_with("# Title\n\nContent"), "preserves body");
    }

    #[test]
    fn apply_chunking_invalid_chunk_returns_error() {
        let md = "a\nb\n".repeat(100);
        let result = apply_chunking(&md, 1, Some(999), &[]);
        assert!(result.is_err());
    }

    #[test]
    fn apply_chunking_out_of_range_errors_even_when_no_split_needed() {
        // F26: requesting `chunk=99` on a body that fits in one chunk
        // used to silently cap to chunk 1. Agents walking a large
        // surface blind need the engine to flag the overshoot.
        let md = "---\n_hash: x\n---\n\n# Small\n";
        let result = apply_chunking(md, 10000, Some(99), &[]);
        assert!(result.is_err(), "out-of-range request must fail");
    }

    #[test]
    fn apply_chunking_no_frontmatter_prepends_one() {
        let md = "# Bare\n\nNo frontmatter here.";
        let result = apply_chunking(md, 10000, None, &[]).unwrap();
        assert!(result.starts_with("---\n_chunk: 1 of 1\n_total_chunks: 1\n---"));
        assert!(result.contains("# Bare"));
    }

    /// Every chunk carries the entity-level frontmatter merged with
    /// caller-supplied `extra_fm` and the chunk-walk keys — chunk 1
    /// retains the entity frontmatter rather than being overwritten by
    /// `extra_fm` only.
    #[test]
    fn apply_chunking_preserves_entity_frontmatter_on_chunk_1() {
        // A multi-chunk body whose source markdown carries entity-
        // level frontmatter. Each line in the body adds ~5 chars; we
        // want enough body to force >1 chunk under a small budget.
        let mut md = String::from(
            "---\n\
             _hash: abc123\n\
             type: spec\n\
             level: M0\n\
             stability: stable\n\
             created_date: 2026-01-01\n\
             last_modified: 2026-05-17\n\
             _tokens: 9999\n\
             ---\n\n\
             # Title\n\n\
             ",
        );
        for i in 0..200 {
            md.push_str(&format!("body line {i} with enough content to span chunks\n"));
        }

        let result = apply_chunking(&md, /* tiny budget */ 100, Some(1), &[
            ("_hash", "fresh-hash"),
            ("_vault_schema", "default@1.0.0"),
        ])
        .unwrap();
        for key in [
            "type:",
            "level:",
            "stability:",
            "created_date:",
            "last_modified:",
        ] {
            assert!(
                result.contains(key),
                "chunk 1 must carry the entity-level `{key}` frontmatter key — got:\n{result}",
            );
        }
        // Caller-supplied keys win on collision.
        assert!(
            result.contains("_hash: fresh-hash"),
            "extra_fm must override the original frontmatter's `_hash`",
        );
        assert!(
            result.contains("_vault_schema: default@1.0.0"),
            "extra_fm key must be present",
        );
        // Chunk-walk signals always emitted.
        assert!(result.contains("_truncated: true"));
        assert!(result.contains("_chunk: 1 of "));
    }

    /// Chunks 2..N also carry the entity-level frontmatter.
    #[test]
    fn apply_chunking_preserves_entity_frontmatter_on_later_chunks() {
        let mut md = String::from(
            "---\n\
             _hash: abc123\n\
             type: memo\n\
             level: M1\n\
             created_date: 2026-01-01\n\
             _tokens_unfiltered_body: 5000\n\
             ---\n\n\
             # Title\n\n\
             ",
        );
        for i in 0..300 {
            md.push_str(&format!("body line {i}: long enough content for spread chunking\n"));
        }

        // Get the chunk count first via Chunk 1; then read Chunks 2
        // and 3 individually and assert each carries the entity FM.
        let chunk_1 = apply_chunking(&md, 100, Some(1), &[("_hash", "h")]).unwrap();
        // The chunk-1 frontmatter has `_total_chunks: <N>` — parse
        // N out so the test exercises every middle/last chunk.
        let total_chunks: usize = chunk_1
            .lines()
            .find_map(|l| l.strip_prefix("_total_chunks: "))
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("chunk 1 must declare _total_chunks: {chunk_1}"));
        assert!(total_chunks >= 3, "test fixture must produce ≥3 chunks");

        for chunk_idx in 2..=total_chunks {
            let chunk = apply_chunking(&md, 100, Some(chunk_idx), &[("_hash", "h")]).unwrap();
            for key in ["type: memo", "level: M1", "created_date: 2026-01-01"] {
                assert!(
                    chunk.contains(key),
                    "chunk {chunk_idx} must carry `{key}` in its frontmatter — got:\n{chunk}",
                );
            }
        }
    }

    /// extra_fm and original frontmatter collide on `_hash`.
    /// The caller's value (`extra_fm`) wins because the engine wants
    /// to authoritatively override post-mutation hashes per chunk.
    #[test]
    fn apply_chunking_caller_supplied_wins_on_collision() {
        let mut md = String::from(
            "---\n\
             _hash: stale-from-prior-write\n\
             type: spec\n\
             ---\n\n",
        );
        for i in 0..200 {
            md.push_str(&format!("line {i}: filler to force multi-chunk emission\n"));
        }

        let chunk_1 =
            apply_chunking(&md, 100, Some(1), &[("_hash", "post-mutation-hash")]).unwrap();
        assert!(chunk_1.contains("_hash: post-mutation-hash"));
        assert!(
            !chunk_1.contains("_hash: stale-from-prior-write"),
            "stale hash must not survive the merge",
        );
    }

    /// `chunk_markdown` must never panic on multi-byte input regardless
    /// of where the budget boundary lands. The 2026-05-18 CLI probe
    /// (F5) reproduced `memstead overview --token-budget 0` panicking with
    /// `start byte index N is not a char boundary; it is inside '—'`
    /// against the schema description's em-dash. The fix uses
    /// `floor_char_boundary` on every byte-indexed slice — these tests
    /// cover the budgets that collapse the char-budget heuristic to
    /// values inside a multi-byte char.
    #[test]
    fn chunk_tiny_budgets_em_dash() {
        // `—` (U+2014, 3 bytes) at byte offsets 0, 4, 8, … of the body.
        // budget=0 → char_budget=0; budget=1 → 4; budget=2 → 8.
        for body in ["—text", "te—xt", "text—", "—a—b—c—", "  —  —  —"] {
            for budget in 0..=2 {
                let _ = chunk_markdown(body, budget); // must not panic
            }
        }
    }

    #[test]
    fn chunk_tiny_budgets_cjk() {
        // CJK chars are 3 bytes each (e.g. `日`, `本`). budget=1 →
        // char_budget=4 which lands mid-`日` if the body starts there.
        for body in ["日本語", "日本語テスト", "abc日本語def", "日a本b語c"] {
            for budget in 0..=4 {
                let _ = chunk_markdown(body, budget);
            }
        }
    }

    #[test]
    fn chunk_tiny_budgets_emoji_vs() {
        // Emoji with variation selector — `❤` (U+2764, 3 bytes) +
        // VS-16 (U+FE0F, 3 bytes) = 6 bytes per glyph.
        let heart_vs = "\u{2764}\u{FE0F}";
        let body = format!("{heart_vs}{heart_vs}{heart_vs}text{heart_vs}{heart_vs}");
        for budget in 0..=5 {
            let _ = chunk_markdown(&body, budget);
        }
    }

    #[test]
    fn chunk_markdown_budget_zero_emits_single_chunk() {
        // budget=0 → no usable prefix at any iteration. The whole body
        // ships as a single chunk; no panic, no infinite loop.
        let chunks = chunk_markdown("any non-trivial body", 0).expect("non-empty body chunks");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "any non-trivial body");
    }

    #[test]
    fn apply_chunking_tiny_budgets_with_em_dash() {
        // Direct apply_chunking path — the production call surface.
        // The frontmatter description text the F5 probe hit carried an
        // em-dash; recreate that shape.
        let md = "---\n_hash: x\n---\n\n# Title — with em-dash\n\nMore body — even more.";
        for budget in 0..=2 {
            let result = apply_chunking(md, budget, None, &[]);
            assert!(result.is_ok(), "budget {budget} must not panic or error: {result:?}");
        }
    }

    #[test]
    fn chunk_markdown_byte_identical_for_budget_ten_plus() {
        // Bisect-green constraint: budgets ≥ 10 produce identical
        // output to a body that has no multi-byte chars within the
        // first slice. The fix is a no-op for the productive range.
        let body = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\n".repeat(20);
        for budget in [10, 25, 50, 100, 250, 1000] {
            let chunks = chunk_markdown(&body, budget).unwrap_or_else(|| vec![body.clone()]);
            // Re-concatenate the chunks; the chunker is allowed to
            // drop a single `\n` separator per split, but the joined
            // form with `\n` between chunks must reproduce the body.
            let rejoined = chunks.join("\n");
            assert!(
                rejoined == body || rejoined == body.trim_end_matches('\n'),
                "budget={budget}: chunk roundtrip must equal source"
            );
        }
    }

    /// Cross-chunk frontmatter consistency — the entity-level fields
    /// read identically across every chunk for one rendering.
    #[test]
    fn apply_chunking_cross_chunk_frontmatter_consistency() {
        let mut md = String::from(
            "---\n\
             _hash: abc\n\
             type: decision\n\
             level: M2\n\
             stability: stable\n\
             ---\n\n",
        );
        for i in 0..300 {
            md.push_str(&format!("line {i}: filler\n"));
        }

        let chunk_1 = apply_chunking(&md, 100, Some(1), &[]).unwrap();
        let chunk_2 = apply_chunking(&md, 100, Some(2), &[]).unwrap();
        for key in ["type: decision", "level: M2", "stability: stable"] {
            assert!(chunk_1.contains(key), "chunk_1 missing `{key}`");
            assert!(chunk_2.contains(key), "chunk_2 missing `{key}`");
        }
    }
}
