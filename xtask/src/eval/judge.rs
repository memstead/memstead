//! The real grader: an LLM judge that scores one answer against a reference.
//!
//! Blindness is structural — [`super::Judge::score`] takes only the reference and
//! the (already tell-stripped) answer. The judge process is never told there are
//! two arms, never sees a label, never sees a tool trace. It returns a single
//! `0.0..=1.0` correctness score, which [`parse_score`] extracts.

use std::process::Command;

use anyhow::{Context, Result, bail};

use super::Judge;
use super::claude::parse_stream_json;

/// The judge's standing instruction. It frames a single grading task and pins the
/// output format so [`parse_score`] has a reliable marker to read.
const JUDGE_SYSTEM: &str = "You are a strict grader. You are given a REFERENCE answer (the ground \
truth, authored from the codebase) and a CANDIDATE answer. Score how well the candidate matches \
the reference on factual correctness and completeness, ignoring style and verbosity. Output \
exactly one line: `SCORE: <x>` where <x> is a number from 0.0 (wrong or empty) to 1.0 (fully \
correct and complete). Output nothing else.";

/// An LLM grader backed by the `claude` CLI.
pub struct ClaudeJudge {
    pub executable: String,
    pub model: String,
}

impl ClaudeJudge {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            executable: "claude".to_string(),
            model: model.into(),
        }
    }
}

impl Judge for ClaudeJudge {
    fn score(&self, reference: &str, answer: &str) -> Result<f64> {
        let prompt = build_judge_prompt(reference, answer);
        let output = Command::new(&self.executable)
            .args([
                "-p",
                &prompt,
                "--output-format",
                "stream-json",
                "--verbose",
                "--model",
                &self.model,
                "--permission-mode",
                "dontAsk",
                "--tools",
                "",
                "--allowedTools",
                "",
                "--system-prompt",
                JUDGE_SYSTEM,
            ])
            .output()
            .with_context(|| format!("spawning judge `{}`", self.executable))?;
        if !output.status.success() {
            bail!(
                "judge claude exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let answer = parse_stream_json(&stdout)?;
        parse_score(&answer.text)
    }
}

/// Compose the grading prompt. Deliberately label-free: REFERENCE and CANDIDATE,
/// never "vault-on"/"vault-off".
pub fn build_judge_prompt(reference: &str, candidate: &str) -> String {
    format!("REFERENCE:\n{reference}\n\nCANDIDATE:\n{candidate}\n\nScore the candidate now.")
}

/// Extract a `0.0..=1.0` score from the judge's text, preferring an explicit
/// `SCORE: x` marker and falling back to the first float anywhere. Out-of-range
/// values are clamped (a judge that writes `SCORE: 95` meant 0.95-ish but we do
/// not guess — we clamp to the valid band and the variance will expose a noisy
/// judge).
pub fn parse_score(text: &str) -> Result<f64> {
    let lower = text.to_lowercase();
    let marked = lower
        .split("score:")
        .nth(1)
        .or_else(|| lower.split("score").nth(1));
    let value = marked
        .and_then(first_float)
        .or_else(|| first_float(&lower))
        .context("no numeric score found in judge output")?;
    Ok(value.clamp(0.0, 1.0))
}

/// Find the first float-looking token in `s` (e.g. `0.7`, `1`, `.5`, `-3`). A
/// leading `-` is captured so a negative score is not silently read as positive.
fn first_float(s: &str) -> Option<f64> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_digit() || c == '.' {
            // Capture a leading minus sign if one immediately precedes the number.
            let start = if i > 0 && bytes[i - 1] == b'-' { i - 1 } else { i };
            let mut seen_dot = false;
            while i < bytes.len() {
                let d = bytes[i] as char;
                if d.is_ascii_digit() {
                    i += 1;
                } else if d == '.' && !seen_dot {
                    seen_dot = true;
                    i += 1;
                } else {
                    break;
                }
            }
            if let Ok(v) = s[start..i].parse::<f64>() {
                return Some(v);
            }
        } else {
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_marked_score() {
        assert!((parse_score("SCORE: 0.7").unwrap() - 0.7).abs() < 1e-9);
        assert!((parse_score("Some reasoning.\nSCORE: 0.95\n").unwrap() - 0.95).abs() < 1e-9);
    }

    #[test]
    fn parses_case_insensitive_marker() {
        assert!((parse_score("score: 1.0").unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn falls_back_to_first_float() {
        assert!((parse_score("I'd give this a 0.4 overall").unwrap() - 0.4).abs() < 1e-9);
    }

    #[test]
    fn clamps_out_of_range() {
        assert_eq!(parse_score("SCORE: 95").unwrap(), 1.0);
        assert_eq!(parse_score("SCORE: -3").unwrap(), 0.0);
    }

    #[test]
    fn zero_is_parsed_not_treated_as_missing() {
        assert_eq!(parse_score("SCORE: 0.0").unwrap(), 0.0);
    }

    #[test]
    fn no_number_is_an_error() {
        assert!(parse_score("I cannot grade this").is_err());
    }

    #[test]
    fn judge_prompt_is_label_free() {
        let p = build_judge_prompt("ref text", "cand text");
        let lower = p.to_lowercase();
        assert!(!lower.contains("vault-on"));
        assert!(!lower.contains("vault-off"));
        assert!(!lower.contains("mounted"));
        assert!(p.contains("ref text") && p.contains("cand text"));
    }
}
