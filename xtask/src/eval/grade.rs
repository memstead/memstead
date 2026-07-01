//! Blinding and aggregation.
//!
//! Two jobs: scrub any tell out of an answer before the judge sees it
//! ([`strip_tells`]), and turn per-trial scores into a per-task result whose
//! delta is signed and never floored ([`TaskResult`]).

/// Remove the tells that would let a judge infer which arm produced an answer.
///
/// The structural blinding is that [`super::Judge::score`] has no arm-label
/// parameter — it physically cannot read a label. This scrub closes the *content*
/// channel: an answer that says "according to the mounted vault…" or names a
/// `memstead_*` tool would leak the arm through its prose. The vault-on arm's
/// system prompt also forbids citing its sources; this is the defence in depth.
///
/// The scrub is intentionally answer-content-only — it never touches the
/// reference, and it collapses the surrounding whitespace so a redaction does not
/// itself become a tell (a lone double-space where a phrase was removed).
pub fn strip_tells(text: &str) -> String {
    // Case-insensitive removal of arm-identifying phrases and tool tokens. Order
    // matters: longer phrases first so "the mounted vault" is removed whole
    // rather than leaving a dangling "the … vault".
    const TELLS: &[&str] = &[
        "according to the mounted vault",
        "from the mounted vault",
        "the mounted vault",
        "the mounted graph",
        "mounted vault",
        "the memstead vault",
        "the knowledge vault",
        "the vault tells",
        "querying the vault",
        "the vault",
    ];
    let mut out = text.to_string();
    for tell in TELLS {
        out = remove_ci(&out, tell);
    }
    // Drop any explicit memstead_* / mcp__memstead__* token wherever it appears.
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
    fn strip_removes_vault_citation() {
        let leaked = "According to the mounted vault, X happened in commit abc.";
        let clean = strip_tells(leaked);
        assert!(!clean.to_lowercase().contains("vault"), "{clean}");
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
        let leaked = "The answer is the vault here.";
        let clean = strip_tells(leaked);
        assert!(!clean.contains("  "), "double space remained: {clean:?}");
    }

    #[test]
    fn delta_is_signed_not_floored() {
        // vault-off strictly better → negative delta, reported plainly.
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
