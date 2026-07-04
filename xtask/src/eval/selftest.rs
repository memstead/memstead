//! `eval --self-test` — run the whole harness loop end-to-end with deterministic
//! stubs (no `claude`, no real mem) and emit a chart-ready data series.
//!
//! This proves the pipeline wires up — guards, blinding, aggregation, series
//! emission — and gives an operator a sample artifact to point a chart renderer
//! at. It is **not** a real measurement: the runner and judge are fixed stubs.
//! The real `claude -p` runner and the git-history mount replay are wired in a
//! later session; until then this is the runnable proof that the scaffold holds.

use std::path::Path;

use anyhow::Result;

use super::{AgentAnswer, ArmConfig, Condition, Judge, MemState, Runner, TaskSpec, run_series};

/// A stub agent: the mem-on arm reaches a quality that climbs with how rich the
/// named state is; the mem-off arm holds a fixed baseline. Mirrors the real
/// runner's contract — mem-on carries a `memstead_*` tool call, mem-off does
/// not — so the mount-evidence validator exercises the same path it will in anger.
struct StubRunner;

impl Runner for StubRunner {
    fn run(&self, arm: &ArmConfig) -> Result<AgentAnswer> {
        let baseline = 0.45;
        match arm.condition {
            Condition::MemOff => Ok(AgentAnswer {
                text: format!("q={baseline:.3}"),
                tool_calls: vec![],
            }),
            Condition::MemOn => {
                let q = match arm
                    .mcp_config
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                {
                    None => baseline, // empty/absent state — no lift
                    Some(label) if label.contains("mature") => 0.88,
                    Some(label) if label.contains("growing") => 0.66,
                    Some(_) => baseline + 0.05,
                };
                Ok(AgentAnswer {
                    // A deliberate tell — the blinding scrub must remove "the mem".
                    text: format!("q={q:.3} per the mem"),
                    tool_calls: vec!["mcp__memstead__memstead_search".to_string()],
                })
            }
        }
    }
}

/// A stub judge: parses the `q=<f>` the stub runner embeds. It never sees an arm
/// label — the trait shape forbids it.
struct ParseQualityJudge;

impl Judge for ParseQualityJudge {
    fn score(&self, _reference: &str, answer: &str) -> Result<f64> {
        Ok(answer
            .split("q=")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0))
    }
}

/// Run the stub pipeline against three synthetic states and write the series.
pub fn run(output: &Path) -> Result<()> {
    let tasks = vec![
        TaskSpec {
            id: "what-changed".into(),
            prompt: "What changed when the projection surface landed?".into(),
            reference: "The engine added a read-only projection surface.".into(),
        },
        TaskSpec {
            id: "why-chosen".into(),
            prompt: "Why git-branch backend over a folder backend?".into(),
            reference: "Per-mem branches give isolated history and optimistic locking.".into(),
        },
    ];
    let states = vec![
        MemState {
            label: "empty".into(),
            mcp_config: None,
        },
        MemState {
            label: "growing".into(),
            mcp_config: Some("/tmp/eval/growing.json".into()),
        },
        MemState {
            label: "mature".into(),
            mcp_config: Some("/tmp/eval/mature.json".into()),
        },
    ];
    let series = run_series(
        &StubRunner,
        &ParseQualityJudge,
        "engine",
        &tasks,
        &states,
        "claude-opus-4-8",
        "Answer the question. Do not mention your sources or tools.",
        4,
    )?;
    series.write(output)?;
    eprintln!(
        "eval self-test: wrote {} states for subject mem {:?} to {}",
        series.points.len(),
        series.subject_mem,
        output.display()
    );
    for p in &series.points {
        eprintln!(
            "  {:<10} delta={:+.3} (on={:.3} off={:.3}, n={})",
            p.state_label, p.delta, p.on_mean, p.off_mean, p.n_trials
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_test_writes_a_climbing_series() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("series.json");
        run(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let points = v["points"].as_array().unwrap();
        assert_eq!(points.len(), 3);
        // Empty state shows no signal; the delta climbs as the state matures.
        let d0 = points[0]["delta"].as_f64().unwrap();
        let d1 = points[1]["delta"].as_f64().unwrap();
        let d2 = points[2]["delta"].as_f64().unwrap();
        assert!(d0.abs() < 1e-9, "empty state delta = {d0}");
        assert!(d1 > d0 && d2 > d1, "deltas not climbing: {d0} {d1} {d2}");
    }
}
