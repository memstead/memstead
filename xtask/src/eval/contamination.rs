//! The contamination guard: the no-substrate (A) arm.
//!
//! The substrate test only measures substrate quality if the tasks are answerable
//! *only from the corpus*. A task the bare model already knows — from its
//! parametric knowledge — measures prior knowledge, not the substrate, and would
//! inflate (or muddy) the C−B delta. So before the comparison runs, every task is
//! screened against an **A arm**: the same model, the same task, but **no
//! substrate in context** and no tools. A is told to answer from its own
//! knowledge; if it succeeds, the task is guessable and is **excluded** from the
//! B-vs-C delta (and reported, never silently dropped).
//!
//! This is the seam the plan calls out: "the no-substrate (A) arm — or the model
//! with web/parametric knowledge — must reliably *fail* [the tasks]; any task it
//! answers is discarded as measuring prior knowledge." The screen is the objective
//! test of whether a corpus is clean enough.
//!
//! The screen is parametric-only by construction (empty tool allow-list, empty
//! sandbox cwd — the same isolation the in-context answer path uses), so it is
//! deterministic to wire and conservative: a web-enabled probe would catch *more*
//! contamination, never less, so a task that passes this screen is not guaranteed
//! clean against the web — that stricter probe is a future tightening, not a
//! correctness gap in the exclusion logic.

use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::claude::{ClaudeRunner, parse_stream_json};
use super::grade::{mean, strip_tells};
use super::{AgentAnswer, Judge, TaskSpec};

/// The A arm's standing instruction: answer from parametric knowledge alone. It
/// must *not* reference any substrate (there is none) — the point is to find out
/// what the bare model already knows.
pub const BARE_SYSTEM: &str = "Answer the question as precisely and completely as you can from your \
own knowledge. If you do not know the answer, say so plainly rather than guessing.";

/// A task excluded from the B-vs-C delta because the bare model answered it — i.e.
/// it measures prior knowledge, not the substrate.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ExcludedTask {
    pub task_id: String,
    /// The bare model's mean score against the reference — what flagged it.
    pub bare_score: f64,
}

/// The outcome of screening a task set against the A arm.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ContaminationReport {
    /// The contamination threshold the screen used (`bare_score >= threshold`
    /// excludes the task). Recorded so the run is self-describing.
    pub threshold: f64,
    /// Tasks excluded as guessable, with the score that flagged them.
    pub excluded: Vec<ExcludedTask>,
}

/// Build the bare-model `claude -p` argument vector — no substrate, no tools.
///
/// Pure and unit-tested. Identical in shape to the in-context arm minus the
/// substrate: empty `--allowedTools` under `dontAsk` denies every tool, and the
/// system prompt is [`BARE_SYSTEM`] (answer from own knowledge), so the answer
/// reflects only what the model already knew.
pub fn build_bare_args(model: &str, task_text: &str) -> Vec<String> {
    vec![
        "-p".to_string(),
        task_text.to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--model".to_string(),
        model.to_string(),
        "--permission-mode".to_string(),
        "dontAsk".to_string(),
        "--strict-mcp-config".to_string(),
        "--system-prompt".to_string(),
        BARE_SYSTEM.to_string(),
        "--allowedTools".to_string(),
        String::new(),
    ]
}

/// Runs the bare model (A arm) for a task. The real impl shells to `claude -p`;
/// tests use a stub.
pub trait BareRunner {
    fn run_bare(&self, model: &str, task_text: &str) -> Result<AgentAnswer>;
}

impl BareRunner for ClaudeRunner {
    fn run_bare(&self, model: &str, task_text: &str) -> Result<AgentAnswer> {
        std::fs::create_dir_all(&self.sandbox_dir).with_context(|| {
            format!("creating A-arm sandbox {}", self.sandbox_dir.display())
        })?;
        let args = build_bare_args(model, task_text);
        let output = Command::new(&self.executable)
            .args(&args)
            .current_dir(&self.sandbox_dir)
            .output()
            .with_context(|| format!("spawning A-arm `{}`", self.executable))?;
        if !output.status.success() {
            bail!(
                "A-arm claude exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        parse_stream_json(&String::from_utf8_lossy(&output.stdout))
    }
}

/// Screen `tasks` against the A arm: any task the bare model answers at or above
/// `threshold` is excluded as guessable.
///
/// Returns the kept tasks (those the bare model failed — answerable only from the
/// corpus) and a [`ContaminationReport`] of the excluded ones. The bare answer is
/// tell-stripped and scored by the same blind judge the comparison uses, so the
/// threshold is on the same scale as the C/B scores.
pub fn screen_tasks<R: BareRunner, J: Judge>(
    runner: &R,
    judge: &J,
    tasks: &[TaskSpec],
    model: &str,
    threshold: f64,
    n_trials: usize,
) -> Result<(Vec<TaskSpec>, ContaminationReport)> {
    let mut kept = Vec::new();
    let mut excluded = Vec::new();
    for task in tasks {
        let mut scores = Vec::with_capacity(n_trials);
        for _ in 0..n_trials {
            let ans = runner.run_bare(model, &task.prompt)?;
            scores.push(judge.score(&task.reference, &strip_tells(&ans.text))?);
        }
        let bare_score = mean(&scores);
        if bare_score >= threshold {
            excluded.push(ExcludedTask {
                task_id: task.id.clone(),
                bare_score,
            });
        } else {
            kept.push(task.clone());
        }
    }
    Ok((kept, ContaminationReport { threshold, excluded }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str) -> TaskSpec {
        TaskSpec {
            id: id.into(),
            prompt: format!("prompt {id}"),
            reference: format!("reference {id}"),
        }
    }

    #[test]
    fn bare_args_have_no_substrate_and_no_tools() {
        let args = build_bare_args("claude-x", "what is X?");
        assert!(!args.iter().any(|a| a == "--mcp-config"), "{args:?}");
        let idx = args.iter().position(|a| a == "--allowedTools").unwrap();
        assert_eq!(args[idx + 1], "");
        // The system prompt is the bare/own-knowledge instruction, with no
        // reference-material block (that would defeat the probe).
        let sys = args.iter().position(|a| a == "--system-prompt").unwrap();
        assert_eq!(args[sys + 1], BARE_SYSTEM);
        assert!(!args[sys + 1].to_lowercase().contains("reference material"));
        assert!(args.windows(2).any(|w| w[0] == "-p" && w[1] == "what is X?"));
    }

    /// A stub bare model that "knows" one specific task and fails the rest.
    struct StubBare {
        known: &'static str,
    }
    impl BareRunner for StubBare {
        fn run_bare(&self, _model: &str, task_text: &str) -> Result<AgentAnswer> {
            let q = if task_text.ends_with(self.known) { 0.95 } else { 0.05 };
            Ok(AgentAnswer { text: format!("q={q:.3}"), tool_calls: vec![] })
        }
    }
    struct ParseQualityJudge;
    impl Judge for ParseQualityJudge {
        fn score(&self, _r: &str, answer: &str) -> Result<f64> {
            Ok(answer
                .split("q=")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0))
        }
    }

    #[test]
    fn guessable_task_is_caught_and_excluded() {
        // Task "g" is answerable from prior knowledge; "c" is not.
        let tasks = vec![task("g"), task("c")];
        let runner = StubBare { known: "g" };
        let (kept, report) =
            screen_tasks(&runner, &ParseQualityJudge, &tasks, "m", 0.5, 3).unwrap();
        // The guessable task is excluded and reported, not counted.
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "c");
        assert_eq!(report.excluded.len(), 1);
        assert_eq!(report.excluded[0].task_id, "g");
        assert!(report.excluded[0].bare_score >= 0.5);
        assert_eq!(report.threshold, 0.5);
    }

    #[test]
    fn corpus_only_tasks_all_survive_the_screen() {
        // A clean corpus: the bare model fails every task → none excluded.
        let tasks = vec![task("a"), task("b")];
        let runner = StubBare { known: "none-of-them" };
        let (kept, report) =
            screen_tasks(&runner, &ParseQualityJudge, &tasks, "m", 0.5, 2).unwrap();
        assert_eq!(kept.len(), 2);
        assert!(report.excluded.is_empty());
    }

    #[test]
    fn threshold_boundary_excludes_at_or_above() {
        // bare_score exactly at threshold is excluded (>=).
        let tasks = vec![task("x")];
        struct ExactStub;
        impl BareRunner for ExactStub {
            fn run_bare(&self, _m: &str, _t: &str) -> Result<AgentAnswer> {
                Ok(AgentAnswer { text: "q=0.500".into(), tool_calls: vec![] })
            }
        }
        let (kept, report) =
            screen_tasks(&ExactStub, &ParseQualityJudge, &tasks, "m", 0.5, 1).unwrap();
        assert!(kept.is_empty());
        assert_eq!(report.excluded.len(), 1);
    }
}
