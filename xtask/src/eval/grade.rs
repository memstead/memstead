//! Blinding and aggregation.
//!
//! Two jobs: scrub any tell out of an answer before the judge sees it
//! ([`strip_tells`]), and turn per-trial scores into a per-task result whose
//! delta is signed and never floored ([`TaskResult`]).

/// Remove the tells that would let a judge infer which arm produced an answer.
///
/// The structural blinding is that [`super::Judge::score`] has no arm-label
/// parameter — it physically cannot read a label. This scrub closes the *content*
/// channel: an answer that says "according to the mounted mem…" or names a
/// `memstead_*` tool would leak the arm through its prose. The mem-on arm's
/// system prompt also forbids citing its sources; this is the defence in depth.
///
/// The scrub is intentionally answer-content-only — it never touches the
/// reference, and it collapses the surrounding whitespace so a redaction does not
/// itself become a tell (a lone double-space where a phrase was removed).
pub fn strip_tells(text: &str) -> String {
    // Case-insensitive removal of arm-identifying phrases and tool tokens. Order
    // matters: longer phrases first so "the mounted mem" is removed whole
    // rather than leaving a dangling "the … mem".
    const TELLS: &[&str] = &[
        "according to the mounted mem",
        "from the mounted mem",
        "the mounted mem",
        "the mounted graph",
        "mounted mem",
        "the memstead mem",
        "the knowledge mem",
        "the mem tells",
        "querying the mem",
        "the mem",
    ];
    let owned: Vec<String> = TELLS.iter().map(|t| (*t).to_string()).collect();
    strip_tells_with(text, &owned)
}

/// Blind an answer against a caller-supplied tell list — the general form behind
/// [`strip_tells`], and the divergence mode's per-arm extension of it.
///
/// [`strip_tells`] carries a hardcoded Arm-B-only list; the divergence campaign
/// reads both arms' tell lists from the pre-registration package and must strip
/// **both** directions from every answer, since the judge is blind to which arm
/// produced it. The tells are removed longest-first so a longer phrase is taken
/// whole before a shorter one nested inside it (`memstead_search` before
/// `memstead`, `the mounted mem` before `the mem`) — the same ordering guarantee
/// the hardcoded path relies on, reconstructed here because a package-supplied
/// list arrives in package order, not length order. The `memstead_*` token drop
/// is always applied as a backstop, so an Arm-B tool token leaks through neither
/// the phrase list nor a missing package entry.
pub fn strip_tells_with(text: &str, tells: &[String]) -> String {
    let mut ordered: Vec<&String> = tells.iter().collect();
    ordered.sort_by_key(|t| std::cmp::Reverse(t.len()));
    let mut out = text.to_string();
    for tell in ordered {
        out = remove_ci(&out, tell);
    }
    out = strip_memstead_tokens(&out);
    collapse_ws(&out)
}

/// Remove every case-insensitive occurrence of `needle` from `haystack`.
fn remove_ci(haystack: &str, needle: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let hay_lower = haystack.to_lowercase();
    let need_lower = needle.to_lowercase();
    let mut out = String::with_capacity(haystack.len());
    let mut cursor = 0usize;
    while let Some(rel) = hay_lower[cursor..].find(&need_lower) {
        let start = cursor + rel;
        out.push_str(&haystack[cursor..start]);
        cursor = start + needle.len();
    }
    out.push_str(&haystack[cursor..]);
    out
}

/// Remove whitespace-delimited tokens that contain a memstead tool marker.
fn strip_memstead_tokens(text: &str) -> String {
    text.split_whitespace()
        .filter(|tok| {
            let t = tok.to_lowercase();
            !(t.contains("memstead__") || t.starts_with("memstead_"))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Collapse runs of whitespace to single spaces and trim the ends.
fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The aggregated result for one task: the raw per-trial scores for each arm and
/// the summary statistics derived from them.
#[derive(Clone, Debug, serde::Serialize)]
pub struct TaskResult {
    pub task_id: String,
    pub on_scores: Vec<f64>,
    pub off_scores: Vec<f64>,
    pub on_mean: f64,
    pub off_mean: f64,
    /// `on_mean - off_mean`. **Signed and never floored** — a flat or negative
    /// result reports as it is. There is deliberately no `max(0.0, …)` anywhere
    /// on this path.
    pub delta: f64,
    pub on_stddev: f64,
    pub off_stddev: f64,
}

impl TaskResult {
    pub fn new(task_id: String, on_scores: Vec<f64>, off_scores: Vec<f64>) -> Self {
        let on_mean = mean(&on_scores);
        let off_mean = mean(&off_scores);
        Self {
            delta: on_mean - off_mean,
            on_stddev: stddev(&on_scores, on_mean),
            off_stddev: stddev(&off_scores, off_mean),
            on_mean,
            off_mean,
            task_id,
            on_scores,
            off_scores,
        }
    }
}

/// Arithmetic mean; empty slice → 0.0.
pub fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// Population standard deviation around `mean`; fewer than two samples → 0.0.
pub fn stddev(xs: &[f64], mean: f64) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / xs.len() as f64;
    var.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_mem_citation() {
        let leaked = "According to the mounted mem, X happened in commit abc.";
        let clean = strip_tells(leaked);
        assert!(!clean.to_lowercase().contains("mem"), "{clean}");
        assert!(!clean.to_lowercase().contains("mounted"), "{clean}");
        assert!(clean.contains("X happened in commit abc."), "{clean}");
    }

    #[test]
    fn strip_removes_tool_tokens() {
        let leaked = "I called mcp__memstead__memstead_search and found the answer.";
        let clean = strip_tells(leaked);
        assert!(!clean.contains("memstead"), "{clean}");
        assert!(clean.contains("found the answer"), "{clean}");
    }

    #[test]
    fn strip_is_idempotent_on_clean_text() {
        let clean = "X is implemented in module foo and called from bar.";
        assert_eq!(strip_tells(clean), clean);
    }

    #[test]
    fn strip_leaves_no_double_spaces_after_redaction() {
        let leaked = "The answer is the mem here.";
        let clean = strip_tells(leaked);
        assert!(!clean.contains("  "), "double space remained: {clean:?}");
    }

    #[test]
    fn strip_tells_with_strips_both_arm_directions() {
        // A single combined list carrying both Arm B (mem/tool) and Arm A
        // (substrate) vocabulary — the reader-path blinder strips both.
        let tells: Vec<String> = [
            "the mounted mem",
            "memstead_search",
            "memstead",
            "wikilink",
            "index.md",
            "the notes directory",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let arm_b_leak = "According to the mounted mem I ran memstead_search and found it.";
        let cleaned_b = strip_tells_with(arm_b_leak, &tells);
        assert!(!cleaned_b.to_lowercase().contains("mounted mem"), "{cleaned_b}");
        assert!(!cleaned_b.to_lowercase().contains("memstead"), "{cleaned_b}");
        assert!(cleaned_b.contains("found it."), "{cleaned_b}");

        let arm_a_leak = "The wikilink in index.md under the notes directory points to it.";
        let cleaned_a = strip_tells_with(arm_a_leak, &tells);
        assert!(!cleaned_a.to_lowercase().contains("wikilink"), "{cleaned_a}");
        assert!(!cleaned_a.to_lowercase().contains("index.md"), "{cleaned_a}");
        assert!(!cleaned_a.to_lowercase().contains("notes directory"), "{cleaned_a}");
        assert!(cleaned_a.contains("points to it."), "{cleaned_a}");
    }

    #[test]
    fn strip_tells_with_removes_longer_phrase_whole() {
        // "the mounted mem" must be taken whole even though "mem" is also a tell,
        // regardless of the order the package lists them in.
        let tells: Vec<String> = ["mem", "the mounted mem"].iter().map(|s| s.to_string()).collect();
        let cleaned = strip_tells_with("stored in the mounted mem today", &tells);
        assert!(!cleaned.to_lowercase().contains("mounted"), "{cleaned}");
        assert!(cleaned.contains("stored in"), "{cleaned}");
        assert!(cleaned.contains("today"), "{cleaned}");
    }

    #[test]
    fn strip_tells_with_drops_memstead_tokens_not_in_list() {
        // Backstop: a memstead_* tool token leaks through even if the package
        // list happened to omit it.
        let cleaned = strip_tells_with("I used mcp__memstead__memstead_entity here.", &[]);
        assert!(!cleaned.contains("memstead"), "{cleaned}");
        assert!(cleaned.contains("here."), "{cleaned}");
    }

    #[test]
    fn delta_is_signed_not_floored() {
        // mem-off strictly better → negative delta, reported plainly.
        let r = TaskResult::new("t".into(), vec![0.2, 0.2], vec![0.8, 0.8]);
        assert!((r.delta + 0.6).abs() < 1e-9, "{}", r.delta);
    }

    #[test]
    fn delta_is_zero_when_arms_tie() {
        let r = TaskResult::new("t".into(), vec![0.5, 0.5, 0.5], vec![0.5, 0.5, 0.5]);
        assert!(r.delta.abs() < 1e-9);
        assert_eq!(r.on_stddev, 0.0);
    }

    #[test]
    fn stddev_captures_run_to_run_variance() {
        let r = TaskResult::new("t".into(), vec![0.0, 1.0], vec![0.5, 0.5]);
        assert!(r.on_stddev > 0.0, "on_stddev = {}", r.on_stddev);
        assert_eq!(r.off_stddev, 0.0);
    }

    #[test]
    fn mean_and_stddev_match_known_values() {
        let xs = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let m = mean(&xs);
        assert!((m - 5.0).abs() < 1e-9);
        assert!((stddev(&xs, m) - 2.0).abs() < 1e-9);
    }
}
