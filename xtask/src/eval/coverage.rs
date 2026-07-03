//! Information-loss / coverage: what a substrate *dropped* relative to the source.
//!
//! Task accuracy alone can hide a recall loss. A schema-forced substrate that
//! coerces or discards a fact that did not fit the schema may still *score higher*
//! on the task set — winning on the facts it kept while silently losing the ones
//! it dropped. This is the "straitjacket" seam skeptics attack first, so the
//! harness measures it directly: against a list of **source facts** (ground truth
//! drawn from the corpus, not from either capture), it reports, per substrate,
//! which facts survived and which were dropped.
//!
//! The signal sits *alongside* the task delta in the output, never folded into it.
//! A run where C beats B on tasks but drops source facts surfaces both numbers, so
//! a precision win cannot pass itself off as a clean victory.
//!
//! Coverage is itself a judgement ("is this fact present in this substrate?"), so
//! like the grader it is a trait ([`CoverageChecker`]) with an LLM-backed real
//! impl and a deterministic stub for tests.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::claude::parse_stream_json;

/// One ground-truth fact from the source corpus. Authored from the sources, not
/// from either substrate — the fixed yardstick both captures are measured against.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SourceFact {
    pub id: String,
    pub statement: String,
}

/// Load `[{id, statement}, …]` from a JSON facts file.
pub fn load_facts(path: &Path) -> Result<Vec<SourceFact>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading facts file {}", path.display()))?;
    let facts: Vec<SourceFact> = serde_json::from_str(&text)
        .with_context(|| format!("parsing facts file {}", path.display()))?;
    if facts.is_empty() {
        bail!("facts file {} contains no facts", path.display());
    }
    Ok(facts)
}

/// Decides whether a source fact is present in / supported by a substrate's bytes.
/// The real impl asks an LLM; tests use a deterministic stub.
pub trait CoverageChecker {
    fn covers(&self, substrate_content: &str, fact: &str) -> Result<bool>;
}

/// The coverage of one substrate against the source facts.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SubstrateCoverage {
    /// Which substrate this measures (`free-form` / `schema-forced`).
    pub substrate_label: String,
    /// Fact ids the substrate retained.
    pub covered: Vec<String>,
    /// Fact ids the substrate **dropped** — the information loss, surfaced.
    pub dropped: Vec<String>,
    /// `covered / total` — the recall of this substrate over the source facts.
    pub coverage: f64,
}

/// Measure one substrate's coverage of the source facts.
///
/// Every fact is checked against the substrate; the dropped set is the recall loss
/// the criterion requires the harness to expose rather than hide.
pub fn measure_coverage<C: CoverageChecker>(
    checker: &C,
    substrate_label: &str,
    substrate_content: &str,
    facts: &[SourceFact],
) -> Result<SubstrateCoverage> {
    let mut covered = Vec::new();
    let mut dropped = Vec::new();
    for fact in facts {
        if checker.covers(substrate_content, &fact.statement)? {
            covered.push(fact.id.clone());
        } else {
            dropped.push(fact.id.clone());
        }
    }
    let coverage = if facts.is_empty() {
        0.0
    } else {
        covered.len() as f64 / facts.len() as f64
    };
    Ok(SubstrateCoverage {
        substrate_label: substrate_label.to_string(),
        covered,
        dropped,
        coverage,
    })
}

/// The coverage checker's standing instruction: a strict presence test, pinned to
/// a `PRESENT: YES|NO` line so [`parse_presence`] has a reliable marker.
const COVERAGE_SYSTEM: &str = "You are checking whether a specific FACT is present in or directly \
supported by a body of REFERENCE material. Answer only about what the reference actually contains \
— do not use outside knowledge. Output exactly one line: `PRESENT: YES` if the reference states or \
directly supports the fact, or `PRESENT: NO` if it does not. Output nothing else.";

/// An LLM-backed coverage checker via the `claude` CLI.
pub struct ClaudeCoverageChecker {
    pub executable: String,
    pub model: String,
}

impl ClaudeCoverageChecker {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            executable: "claude".to_string(),
            model: model.into(),
        }
    }
}

impl CoverageChecker for ClaudeCoverageChecker {
    fn covers(&self, substrate_content: &str, fact: &str) -> Result<bool> {
        let prompt = build_coverage_prompt(substrate_content, fact);
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
                "--allowedTools",
                "",
                "--system-prompt",
                COVERAGE_SYSTEM,
            ])
            .output()
            .with_context(|| format!("spawning coverage checker `{}`", self.executable))?;
        if !output.status.success() {
            bail!(
                "coverage claude exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let answer = parse_stream_json(&String::from_utf8_lossy(&output.stdout))?;
        parse_presence(&answer.text)
    }
}

/// Compose the coverage prompt. Reference first, then the single fact under test.
pub fn build_coverage_prompt(substrate_content: &str, fact: &str) -> String {
    format!(
        "REFERENCE:\n{substrate_content}\n\nFACT:\n{fact}\n\nIs the fact present in the reference?"
    )
}

/// Read a yes/no presence verdict from the checker's text, preferring an explicit
/// `PRESENT: YES|NO` marker and falling back to the first yes/no token.
pub fn parse_presence(text: &str) -> Result<bool> {
    let lower = text.to_lowercase();
    let after_marker = lower.split("present:").nth(1).unwrap_or(&lower);
    // First decisive token wins.
    for word in after_marker.split(|c: char| !c.is_ascii_alphabetic()) {
        match word {
            "yes" | "true" => return Ok(true),
            "no" | "false" => return Ok(false),
            _ => {}
        }
    }
    bail!("no YES/NO presence verdict found in coverage output: {text:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Vec<SourceFact> {
        vec![
            SourceFact {
                id: "f1".into(),
                statement: "Widget X depends on Gadget Y".into(),
            },
            SourceFact {
                id: "f2".into(),
                statement: "Gadget Y was added in v2".into(),
            },
            SourceFact {
                id: "f3".into(),
                statement: "X exposes a read-only port".into(),
            },
        ]
    }

    #[test]
    fn parse_presence_reads_marker() {
        assert!(parse_presence("PRESENT: YES").unwrap());
        assert!(!parse_presence("PRESENT: NO").unwrap());
        assert!(parse_presence("reasoning...\nPRESENT: YES\n").unwrap());
    }

    #[test]
    fn parse_presence_falls_back_to_first_token() {
        assert!(parse_presence("yes, it is there").unwrap());
        assert!(!parse_presence("no, the reference omits it").unwrap());
    }

    #[test]
    fn parse_presence_errors_when_undecidable() {
        assert!(parse_presence("I am not sure").is_err());
    }

    #[test]
    fn coverage_prompt_puts_reference_and_fact() {
        let p = build_coverage_prompt("ref body", "some fact");
        assert!(p.contains("ref body") && p.contains("some fact"));
    }

    /// A stub checker driven by a set of fact statements it considers present.
    struct StubChecker {
        present: Vec<&'static str>,
    }
    impl CoverageChecker for StubChecker {
        fn covers(&self, _substrate: &str, fact: &str) -> Result<bool> {
            Ok(self.present.iter().any(|p| fact.contains(p)))
        }
    }

    #[test]
    fn measure_coverage_splits_covered_and_dropped() {
        let checker = StubChecker {
            present: vec!["Widget X", "read-only"],
        };
        let cov = measure_coverage(&checker, "schema-forced", "…", &facts()).unwrap();
        assert_eq!(cov.covered, vec!["f1", "f3"]);
        assert_eq!(cov.dropped, vec!["f2"]);
        assert!((cov.coverage - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn full_coverage_drops_nothing() {
        let checker = StubChecker {
            present: vec!["Widget X", "Gadget Y", "read-only"],
        };
        let cov = measure_coverage(&checker, "free-form", "…", &facts()).unwrap();
        assert!(cov.dropped.is_empty());
        assert!((cov.coverage - 1.0).abs() < 1e-9);
    }

    #[test]
    fn precision_up_recall_down_is_surfaced_not_hidden() {
        // The constructed case: the schema-forced substrate (C) is the one that
        // would *win on tasks* (precision), yet it drops a source fact the
        // free-form substrate (B) kept (recall loss). The harness must surface the
        // drop alongside, never fold it away.
        let b_checker = StubChecker {
            present: vec!["Widget X", "Gadget Y", "read-only"],
        };
        let c_checker = StubChecker {
            present: vec!["Widget X", "read-only"],
        }; // drops f2
        let b_cov = measure_coverage(&b_checker, "free-form", "…", &facts()).unwrap();
        let c_cov = measure_coverage(&c_checker, "schema-forced", "…", &facts()).unwrap();
        // C has the higher implied precision but the lower coverage — the recall loss.
        assert!(
            c_cov.coverage < b_cov.coverage,
            "C should drop a fact B kept"
        );
        assert_eq!(c_cov.dropped, vec!["f2"]);

        // Surfaced in the serialized output: build the series the way the run does
        // (C-wins task delta + both coverage reports) and confirm the dropped fact
        // is present in the JSON, not hidden.
        let series = crate::eval::series::DataSeries {
            subject_mem: "engine".into(),
            points: vec![crate::eval::series::SeriesPoint::aggregate(
                "schema-forced − free-form".into(),
                2,
                &[crate::eval::grade::TaskResult::new(
                    "t".into(),
                    vec![0.9, 0.9],
                    vec![0.5, 0.5],
                )],
            )],
            excluded_contaminated: vec![],
            coverage: vec![c_cov, b_cov],
        };
        let json = series.to_json().unwrap();
        // The task win is present…
        assert!(json.contains("\"delta\""));
        // …and so is the dropped source fact — the recall loss is not hidden.
        assert!(json.contains("\"dropped\""), "{json}");
        assert!(
            json.contains("\"f2\""),
            "dropped fact f2 not surfaced: {json}"
        );
    }
}
