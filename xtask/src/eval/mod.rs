//! Compounding-proof eval harness.
//!
//! The harness has two arm flavours that share one defensible core (matched arms,
//! a blind judge, [`grade::strip_tells`], variance, and a signed/unfloored delta):
//!
//! - **Substrate quality** ([`substrate`]) — the *primary* test. An arm is *which
//!   substrate is in the agent's context*: a free-form capture (B) vs a
//!   schema-forced capture (C) of the same sources, retrieval held out. Answers
//!   "does forcing knowledge into a schema during capture produce a better
//!   substrate?" — the write-side bet.
//! - **Mount / retrieval** (this module, [`run_task`]/[`run_series`]) — the
//!   *secondary* mode, retained from the harness's first framing. An arm is
//!   *whether the subject mem is mounted over MCP*. Answers "does mounting this
//!   mem make an agent better at retrieving answers about its subject?"
//!
//! The mount path below enforces, in code, the properties that make its answer
//! defensible:
//!
//! - **One variable** ([`arm::check_single_variable`]) — the two arms differ only
//!   in mount state. Same model, same system prompt, same task text. A run whose
//!   arms diverge in anything else is refused, not silently averaged.
//! - **Real mount path** ([`arm::validate_mount_evidence`]) — the mem-on arm must
//!   show `memstead_*` tool use and the mem-off arm must show none, or the trial
//!   is invalid. The proof reflects the product an actual user gets, not a synthetic
//!   context-stuff.
//! - **Blind grading** ([`grade::strip_tells`], [`Judge`]) — the judge scores one
//!   answer against a reference and never receives an arm label, a tool trace, or a
//!   mem citation. It *cannot* infer the arm; the blinding is structural, not a
//!   convention.
//! - **Honesty** ([`grade::TaskResult::delta`]) — the delta is `on_mean - off_mean`,
//!   signed, never floored. A flat or negative result is reported plainly.
//! - **Variance** — every arm runs `n_trials` times; the series carries the standard
//!   deviation so a single lucky shot is not mistaken for a result.
//! - **Compounding axis** ([`run_series`]) — the same task set scored against an
//!   ordered list of mem states yields delta-as-a-function-of-state, the data
//!   behind "the line goes up as the graph grows."
//!
//! The agent and the judge are traits ([`Runner`], [`Judge`]). The real
//! [`Runner`] shells to `claude -p --mcp-config` (mem-on) or without it
//! (mem-off); the real [`Judge`] calls an LLM against a reference. Tests drive
//! the whole loop with deterministic stubs so the wiring, the guards, and the
//! aggregation are verified without a network call.

pub mod arm;
pub mod capture;
pub mod claude;
pub mod contamination;
pub mod coverage;
pub mod grade;
pub mod judge;
pub mod replay;
pub mod selftest;
pub mod series;
pub mod substrate;
pub mod tasks;

use anyhow::Result;

pub use arm::{ArmConfig, build_arms, check_single_variable, validate_mount_evidence};
pub use grade::{TaskResult, strip_tells};
pub use series::{DataSeries, SeriesPoint};

/// Which arm of the experiment — the single variable under test.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Condition {
    /// The subject mem is mounted over the real MCP surface.
    MemOn,
    /// No mem is mounted; the agent has every other tool but `memstead_*`.
    MemOff,
}

impl Condition {
    pub fn label(self) -> &'static str {
        match self {
            Condition::MemOn => "mem-on",
            Condition::MemOff => "mem-off",
        }
    }
}

/// One task: a prompt and the reference answer authored from codebase ground
/// truth (not from either arm's output).
///
/// The shape is deliberately source-agnostic. A hand-authored question about a
/// subsystem and a git-mined task ("what broke when commit X landed?") both
/// reduce to a `prompt` plus a `reference`, so both flow through the same scorer
/// — the credible git-mined task set can replace the bootstrap hand-authored one
/// without touching the grading path.
#[derive(Clone, Debug)]
pub struct TaskSpec {
    pub id: String,
    pub prompt: String,
    pub reference: String,
}

/// An agent's answer for one arm.
///
/// `tool_calls` records the tools the agent invoked. It serves two jobs: proving
/// the mem-on arm really exercised the MCP mount ([`arm::validate_mount_evidence`])
/// and being discarded — never shown to the judge — so a tool trace cannot leak
/// which arm produced the answer.
#[derive(Clone, Debug)]
pub struct AgentAnswer {
    pub text: String,
    pub tool_calls: Vec<String>,
}

/// A historical (or current) mount of the subject mem. The compounding axis
/// scores the task set against an ordered list of these.
///
/// `mcp_config` points at the MCP config that mounts *this* state of the mem
/// (e.g. a worktree checked out at an older commit). `None` models an
/// empty/absent mem — the instrument must show ~zero delta there.
#[derive(Clone, Debug)]
pub struct MemState {
    pub label: String,
    pub mcp_config: Option<std::path::PathBuf>,
}

/// Produces an agent answer for a given arm. The real impl shells to `claude -p`;
/// tests use a deterministic stub.
pub trait Runner {
    fn run(&self, arm: &ArmConfig) -> Result<AgentAnswer>;
}

/// Scores one answer against a reference, in `0.0..=1.0`. Receives only the
/// reference and the (tell-stripped) answer text — no arm label, no tool trace,
/// no mount state. The absence of a label parameter is the blinding: the judge
/// is structurally incapable of conditioning on the arm.
pub trait Judge {
    fn score(&self, reference: &str, answer: &str) -> Result<f64>;
}

/// Run one task through both arms, `n_trials` times each, and aggregate.
///
/// Refuses up front if the two arms differ in anything but mount state. Every
/// trial validates the mount evidence (mem-on used `memstead_*`, mem-off did
/// not) and strips tells before the judge sees an answer.
pub fn run_task<R: Runner, J: Judge>(
    runner: &R,
    judge: &J,
    task: &TaskSpec,
    on_arm: &ArmConfig,
    off_arm: &ArmConfig,
    n_trials: usize,
) -> Result<TaskResult> {
    check_single_variable(on_arm, off_arm)?;
    let mut on_scores = Vec::with_capacity(n_trials);
    let mut off_scores = Vec::with_capacity(n_trials);
    for _ in 0..n_trials {
        let on = runner.run(on_arm)?;
        validate_mount_evidence(Condition::MemOn, &on)?;
        let off = runner.run(off_arm)?;
        validate_mount_evidence(Condition::MemOff, &off)?;
        on_scores.push(judge.score(&task.reference, &strip_tells(&on.text))?);
        off_scores.push(judge.score(&task.reference, &strip_tells(&off.text))?);
    }
    Ok(TaskResult::new(task.id.clone(), on_scores, off_scores))
}

/// The compounding axis: score `tasks` against each `state` in order and collect
/// the deltas into a chart-ready series.
///
/// `mcp_off` is the mem-off MCP config (no `memstead_*`); for mem-on, each
/// state supplies its own historical mount via [`MemState::mcp_config`]. A
/// state with no mount (`None`) still runs — the instrument must report ~zero
/// delta for an empty graph rather than skipping it.
#[allow(clippy::too_many_arguments)]
pub fn run_series<R: Runner, J: Judge>(
    runner: &R,
    judge: &J,
    subject_mem: &str,
    tasks: &[TaskSpec],
    states: &[MemState],
    model: &str,
    system_prompt: &str,
    n_trials: usize,
) -> Result<DataSeries> {
    let mut points = Vec::with_capacity(states.len());
    for state in states {
        let mut task_results = Vec::with_capacity(tasks.len());
        for task in tasks {
            let (on_arm, off_arm) =
                build_arms(task, model, system_prompt, state.mcp_config.clone());
            task_results.push(run_task(runner, judge, task, &on_arm, &off_arm, n_trials)?);
        }
        points.push(SeriesPoint::aggregate(state.label.clone(), n_trials, &task_results));
    }
    Ok(DataSeries {
        subject_mem: subject_mem.to_string(),
        points,
        // The mount/retrieval mode does not screen for contamination or coverage.
        excluded_contaminated: Vec::new(),
        coverage: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic stub that answers a task differently depending on whether
    /// the mem is mounted and how rich the named state is. It lets the tests
    /// drive the full loop — guards, blinding, aggregation, series — without a
    /// network call.
    struct StubRunner {
        /// Quality the mem-on arm reaches for the current state, 0.0..=1.0.
        /// The mem-off arm always reaches `off_quality`.
        on_quality: f64,
        off_quality: f64,
    }

    impl Runner for StubRunner {
        fn run(&self, arm: &ArmConfig) -> Result<AgentAnswer> {
            // Mem-on must carry mount evidence; mem-off must not — exactly the
            // shape the real runner produces and the validator enforces.
            match arm.condition {
                Condition::MemOn => Ok(AgentAnswer {
                    text: format!("ANSWER q={:.3} via the mounted mem", self.on_quality),
                    tool_calls: vec!["mcp__memstead__memstead_search".into()],
                }),
                Condition::MemOff => Ok(AgentAnswer {
                    text: format!("ANSWER q={:.3}", self.off_quality),
                    tool_calls: vec![],
                }),
            }
        }
    }

    /// Parses the `q=<f>` the stub embeds and returns it as the score. Crucially
    /// it has no way to see an arm label — the blinding is in the trait shape.
    struct ParseQualityJudge;

    impl Judge for ParseQualityJudge {
        fn score(&self, _reference: &str, answer: &str) -> Result<f64> {
            let q = answer
                .split("q=")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            Ok(q)
        }
    }

    fn task(id: &str) -> TaskSpec {
        TaskSpec {
            id: id.into(),
            prompt: format!("prompt for {id}"),
            reference: format!("reference for {id}"),
        }
    }

    #[test]
    fn run_task_reports_positive_delta_when_mem_helps() {
        let runner = StubRunner { on_quality: 0.9, off_quality: 0.4 };
        let t = task("t1");
        let (on, off) = build_arms(&t, "claude-x", "sys", Some("/tmp/on.json".into()));
        let r = run_task(&runner, &ParseQualityJudge, &t, &on, &off, 5).unwrap();
        assert!((r.delta - 0.5).abs() < 1e-9, "delta = {}", r.delta);
        assert!(r.on_mean > r.off_mean);
    }

    #[test]
    fn run_task_reports_negative_delta_plainly() {
        // Honesty: when mem-off beats mem-on, the delta is negative — no floor.
        let runner = StubRunner { on_quality: 0.2, off_quality: 0.7 };
        let t = task("t1");
        let (on, off) = build_arms(&t, "claude-x", "sys", Some("/tmp/on.json".into()));
        let r = run_task(&runner, &ParseQualityJudge, &t, &on, &off, 3).unwrap();
        assert!(r.delta < 0.0, "delta = {}", r.delta);
        assert!((r.delta + 0.5).abs() < 1e-9, "delta = {}", r.delta);
    }

    #[test]
    fn unrelated_mem_yields_near_zero_delta() {
        // Honesty: a mem that does not help the task produces on == off → ~0.
        let runner = StubRunner { on_quality: 0.5, off_quality: 0.5 };
        let t = task("t1");
        let (on, off) = build_arms(&t, "claude-x", "sys", Some("/tmp/on.json".into()));
        let r = run_task(&runner, &ParseQualityJudge, &t, &on, &off, 4).unwrap();
        assert!(r.delta.abs() < 1e-9, "delta = {}", r.delta);
    }

    #[test]
    fn series_is_delta_as_a_function_of_state() {
        // Compounding axis: an empty state, then two progressively richer states.
        // The empty state reads ~0; the delta climbs with state richness.
        struct StatefulRunner;
        impl Runner for StatefulRunner {
            fn run(&self, arm: &ArmConfig) -> Result<AgentAnswer> {
                let q = match (arm.condition, arm.mcp_config.as_ref()) {
                    (Condition::MemOff, _) => 0.4,
                    (Condition::MemOn, None) => 0.4, // empty/absent state: no lift
                    (Condition::MemOn, Some(p)) => {
                        // richer state label → higher quality
                        if p.to_string_lossy().contains("rich") { 0.9 } else { 0.6 }
                    }
                };
                let tool_calls = match arm.condition {
                    Condition::MemOn if arm.mcp_config.is_some() => {
                        vec!["mcp__memstead__memstead_search".to_string()]
                    }
                    // An empty/absent mem-on state still mounts the (empty) surface.
                    Condition::MemOn => vec!["mcp__memstead__memstead_overview".to_string()],
                    Condition::MemOff => vec![],
                };
                Ok(AgentAnswer { text: format!("q={q:.3}"), tool_calls })
            }
        }
        let tasks = vec![task("a"), task("b")];
        let states = vec![
            MemState { label: "empty".into(), mcp_config: None },
            MemState { label: "v1".into(), mcp_config: Some("/tmp/v1.json".into()) },
            MemState { label: "v2-rich".into(), mcp_config: Some("/tmp/rich.json".into()) },
        ];
        let series = run_series(
            &StatefulRunner,
            &ParseQualityJudge,
            "engine",
            &tasks,
            &states,
            "claude-x",
            "sys",
            3,
        )
        .unwrap();
        assert_eq!(series.points.len(), 3);
        // Empty state: no signal.
        assert!(series.points[0].delta.abs() < 1e-9, "{:?}", series.points[0]);
        // Monotone climb with richness.
        assert!(series.points[1].delta > series.points[0].delta);
        assert!(series.points[2].delta > series.points[1].delta);
    }
}
