//! The substrate-quality arm: an arm is *which substrate is in the agent's
//! context*, not *whether a mem is mounted*.
//!
//! This is the write-side test. From one source corpus the harness builds two
//! substrates — a free-form capture (B) and a schema-forced capture (C) of the
//! same sources — and asks whether C makes a downstream agent measurably better
//! than B. Retrieval is **held out**: the whole substrate goes into the agent's
//! context and there is no search step, so the only thing that differs between
//! arms is the substrate bytes.
//!
//! Three properties make the answer defensible, each enforced here in code:
//!
//! - **One variable** ([`check_single_substrate_variable`]) — the two arms share
//!   model, base system prompt, task text, and token budget; they differ only in
//!   the substrate bytes (and its label). A run whose arms diverge in anything
//!   else is refused, not silently averaged. This is the substrate analogue of
//!   the mount path's [`super::arm::check_single_variable`].
//! - **Retrieval held out** ([`validate_no_retrieval`]) — neither arm may show a
//!   tool call. The substrate is placed wholly in context and the answer must come
//!   from it, not from a search the in-context test was built to exclude. A trial
//!   where either arm reached a tool is invalid.
//! - **Equal token budget** ([`fit_to_budget`]) — if a substrate exceeds the
//!   budget it is trimmed under one deterministic rule applied identically to both
//!   arms, so the variable stays "substrate form", never "who got more context".
//!
//! Blinding, the signed/unfloored delta, variance, and the series emitter are
//! reused unchanged from the mount path: the substrate run flows through the same
//! blind [`Judge`], [`strip_tells`], and [`TaskResult`]. The delta is `C − B` —
//! C is mapped to the `on` slot of [`TaskResult`] and B to `off`, so a flat or
//! negative result (B wins, or the two tie) is reported plainly.

use std::process::Command;

use anyhow::{Context, Result, bail};

use super::claude::ClaudeRunner;
use super::grade::{TaskResult, strip_tells};
use super::series::{AnswerTranscript, DataSeries, SeriesPoint};
use super::{AgentAnswer, Judge, TaskSpec};

/// A captured body of knowledge, ready to be placed in an agent's context.
///
/// `content` is the whole substrate as a single string (markdown for either
/// capture style). `label` names the capture form — e.g. `free-form`,
/// `schema-forced`, `raw` — and is recorded in the result; it is never shown to
/// the judge, so it cannot leak which arm produced an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Substrate {
    pub label: String,
    pub content: String,
}

impl Substrate {
    pub fn new(label: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            content: content.into(),
        }
    }
}

/// The full configuration of one substrate arm. Two arms of the same task are
/// identical in every field except `substrate` — that one field *is* the single
/// variable under test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubstrateArm {
    /// The bytes placed in context, already trimmed to `token_budget`.
    pub substrate: Substrate,
    pub model: String,
    /// The base instructions, identical across arms. The substrate is composed
    /// onto this at invocation time ([`compose_system_prompt`]); the base text
    /// itself never differs between arms.
    pub system_prompt: String,
    pub task_text: String,
    /// The shared context budget, in (approximate) tokens. Both arms are trimmed
    /// to this same budget under the same rule.
    pub token_budget: usize,
}

/// Build the matched substrate-arm pair for a task.
///
/// Both substrates are trimmed to the same `token_budget` under the same rule, so
/// by construction the two arms share model, system prompt, task text, and budget
/// and differ only in the substrate bytes. Callers go through here so the
/// single-variable property holds at the source; [`check_single_substrate_variable`]
/// is the backstop for a caller who hand-assembles arms.
///
/// Returns `(c_arm, b_arm)` — schema-forced first to mirror the `C − B` delta
/// convention, but order carries no other meaning.
pub fn build_substrate_arms(
    task: &TaskSpec,
    model: &str,
    system_prompt: &str,
    token_budget: usize,
    schema_forced: &Substrate,
    free_form: &Substrate,
) -> (SubstrateArm, SubstrateArm) {
    let arm = |s: &Substrate| SubstrateArm {
        substrate: Substrate::new(s.label.clone(), fit_to_budget(&s.content, token_budget)),
        model: model.to_string(),
        system_prompt: system_prompt.to_string(),
        task_text: task.prompt.clone(),
        token_budget,
    };
    (arm(schema_forced), arm(free_form))
}

/// Refuse the run if the two arms differ in anything but the substrate.
///
/// The substrate (its bytes and its label) is *expected* to differ — that pair is
/// the variable. Any divergence in model, base system prompt, task text, or token
/// budget is a confound that would make the delta unattributable: handing the
/// schema arm a richer prompt or a larger budget is the most common way this test
/// is rigged. The error names every offending field.
pub fn check_single_substrate_variable(c: &SubstrateArm, b: &SubstrateArm) -> Result<()> {
    let mut confounds = Vec::new();
    if c.model != b.model {
        confounds.push(format!("model ({:?} vs {:?})", c.model, b.model));
    }
    if c.system_prompt != b.system_prompt {
        confounds.push("system_prompt".to_string());
    }
    if c.task_text != b.task_text {
        confounds.push("task_text".to_string());
    }
    if c.token_budget != b.token_budget {
        confounds.push(format!(
            "token_budget ({} vs {})",
            c.token_budget, b.token_budget
        ));
    }
    // The substrates must actually be the two distinct arms, not the same one twice.
    if c.substrate.label == b.substrate.label {
        confounds.push(format!(
            "substrate label (both {:?} — the arms are not distinct)",
            c.substrate.label
        ));
    }
    if !confounds.is_empty() {
        bail!(
            "refusing to run: the two substrate arms differ in more than the substrate — {}. \
             The only permitted difference is the substrate bytes; everything else is a confound \
             that makes the C-B delta unattributable.",
            confounds.join(", ")
        );
    }
    Ok(())
}

/// Trim `content` to fit `token_budget`, on a whitespace boundary.
///
/// Token count is approximated as `chars / CHARS_PER_TOKEN` — a deliberately rough
/// heuristic that is *good enough for parity*: the exact ratio does not matter
/// because the same rule trims both arms, so neither arm can gain context the
/// other was denied. Content already within budget is returned untouched. When a
/// trim is needed, the cut lands at the last whitespace before the limit so the
/// substrate never ends mid-word.
pub fn fit_to_budget(content: &str, token_budget: usize) -> String {
    /// Rough characters-per-token; only the cross-arm equality of the rule matters.
    const CHARS_PER_TOKEN: usize = 4;
    let char_budget = token_budget.saturating_mul(CHARS_PER_TOKEN);
    if content.chars().count() <= char_budget {
        return content.to_string();
    }
    // Take `char_budget` chars, then back off to the last whitespace so we don't
    // cut a word in half. Operate on char indices to stay UTF-8 safe.
    let truncated: String = content.chars().take(char_budget).collect();
    match truncated.rfind(char::is_whitespace) {
        Some(idx) if idx > 0 => truncated[..idx].trim_end().to_string(),
        _ => truncated.trim_end().to_string(),
    }
}

/// Compose the full system prompt for an in-context run: the base instructions
/// followed by the substrate as reference material.
///
/// The base text is identical across arms; only the appended substrate differs.
/// Keeping the join deterministic and arm-independent is what makes the arg
/// vectors byte-identical up to the substrate payload.
pub fn compose_system_prompt(base: &str, substrate: &Substrate) -> String {
    format!(
        "{base}\n\n# Reference material\n\nThe following is the only source of \
         truth for this task. Answer from it alone.\n\n{}",
        substrate.content
    )
}

/// Build the `claude -p` argument vector for an in-context substrate arm.
///
/// Pure and unit-tested. Unlike the mount path this passes **no** `--mcp-config`
/// and an **empty** `--allowedTools`, so no tool — graph, web, or file — can fire:
/// retrieval is held out and the substrate (carried in the composed system prompt)
/// is the only knowledge source. Run from an empty sandbox cwd (see
/// [`ClaudeRunner::sandbox_dir`]) the built-in file tools also find nothing, so the
/// two arms are byte-identical apart from the substrate bytes inside the system
/// prompt.
pub fn build_incontext_args(arm: &SubstrateArm) -> Vec<String> {
    vec![
        "-p".to_string(),
        arm.task_text.clone(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--model".to_string(),
        arm.model.clone(),
        "--permission-mode".to_string(),
        "dontAsk".to_string(),
        "--strict-mcp-config".to_string(),
        "--system-prompt".to_string(),
        compose_system_prompt(&arm.system_prompt, &arm.substrate),
        // Empty allow-list under dontAsk: every tool is auto-denied. Retrieval held out.
        "--allowedTools".to_string(),
        String::new(),
    ]
}

/// Confirm the answer was produced with retrieval held out — i.e. the agent used
/// no tools at all.
///
/// The in-context test's premise is that the substrate alone answers the task. A
/// tool call (a `memstead_*` query, a web search, a file read) means the agent
/// reached outside the substrate, so the trial does not measure substrate quality
/// and is invalid. This is the substrate analogue of the mount path's
/// [`super::arm::validate_mount_evidence`], inverted: there a tool *must* fire;
/// here none may.
pub fn validate_no_retrieval(answer: &AgentAnswer) -> Result<()> {
    if !answer.tool_calls.is_empty() {
        bail!(
            "invalid trial: the in-context arm used tools ({}) — retrieval is held out for the \
             substrate test, so the answer must come from the in-context substrate alone",
            answer.tool_calls.join(", ")
        );
    }
    Ok(())
}

/// Produces an agent answer for a given substrate arm. The real impl shells to
/// `claude -p` with the substrate in context and no tools; tests use a stub.
pub trait SubstrateRunner {
    fn run(&self, arm: &SubstrateArm) -> Result<AgentAnswer>;
}

impl SubstrateRunner for ClaudeRunner {
    fn run(&self, arm: &SubstrateArm) -> Result<AgentAnswer> {
        std::fs::create_dir_all(&self.sandbox_dir).with_context(|| {
            format!("creating agent sandbox dir {}", self.sandbox_dir.display())
        })?;
        let args = build_incontext_args(arm);
        let output = Command::new(&self.executable)
            .args(&args)
            .current_dir(&self.sandbox_dir)
            .env("MCP_TIMEOUT", "60000")
            .output()
            .with_context(|| {
                format!(
                    "spawning `{}` — is the claude CLI installed and on PATH?",
                    self.executable
                )
            })?;
        if !output.status.success() {
            bail!(
                "claude exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        super::claude::parse_stream_json(&String::from_utf8_lossy(&output.stdout))
    }
}

/// Run one task through both substrate arms, `n_trials` times each, and aggregate.
///
/// Refuses up front if the two arms differ in anything but the substrate. Every
/// trial validates that neither arm reached a tool (retrieval held out) and strips
/// tells before the blind judge sees an answer. The resulting [`TaskResult`] maps
/// C to the `on` slot and B to `off`, so `delta = C_mean − B_mean` — signed and
/// never floored.
pub fn run_substrate_task<R: SubstrateRunner, J: Judge>(
    runner: &R,
    judge: &J,
    task: &TaskSpec,
    c_arm: &SubstrateArm,
    b_arm: &SubstrateArm,
    n_trials: usize,
) -> Result<(TaskResult, (String, String))> {
    check_single_substrate_variable(c_arm, b_arm)?;
    let mut c_scores = Vec::with_capacity(n_trials);
    let mut b_scores = Vec::with_capacity(n_trials);
    // Keep the first trial's raw answers so the eval's output doubles as an
    // auditable transcript. The judge still scores the tell-stripped text; the
    // captured answers are the real (unstripped) ones a reader should see.
    let mut first_answers: Option<(String, String)> = None;
    for _ in 0..n_trials {
        let c = runner.run(c_arm)?;
        validate_no_retrieval(&c)?;
        let b = runner.run(b_arm)?;
        validate_no_retrieval(&b)?;
        if first_answers.is_none() {
            first_answers = Some((c.text.clone(), b.text.clone()));
        }
        c_scores.push(judge.score(&task.reference, &strip_tells(&c.text))?);
        b_scores.push(judge.score(&task.reference, &strip_tells(&b.text))?);
    }
    Ok((
        TaskResult::new(task.id.clone(), c_scores, b_scores),
        first_answers.unwrap_or_default(),
    ))
}

/// Score a whole task set against one (C, B) substrate pair and aggregate into a
/// chart-ready [`DataSeries`] with a single point.
///
/// The substrate test compares two captures of *one* corpus, so its natural
/// output is one point (C − B over the task set) rather than the mount path's
/// series-over-states. The point reuses [`SeriesPoint::aggregate`], so the
/// per-task paired deltas, the cross-task delta spread, and the signed aggregate
/// delta are computed by the same code the mount path uses — the substrate result
/// is directly comparable and feeds the same downstream chart renderer.
#[allow(clippy::too_many_arguments)]
pub fn run_substrate_series<R: SubstrateRunner, J: Judge>(
    runner: &R,
    judge: &J,
    subject: &str,
    tasks: &[TaskSpec],
    schema_forced: &Substrate,
    free_form: &Substrate,
    model: &str,
    system_prompt: &str,
    token_budget: usize,
    n_trials: usize,
) -> Result<DataSeries> {
    let mut results = Vec::with_capacity(tasks.len());
    let mut transcripts = Vec::with_capacity(tasks.len());
    for task in tasks {
        let (c_arm, b_arm) = build_substrate_arms(
            task,
            model,
            system_prompt,
            token_budget,
            schema_forced,
            free_form,
        );
        let (result, (c_answer, b_answer)) =
            run_substrate_task(runner, judge, task, &c_arm, &b_arm, n_trials)?;
        transcripts.push(AnswerTranscript {
            task_id: task.id.clone(),
            prompt: task.prompt.clone(),
            reference: task.reference.clone(),
            schema_forced: c_answer,
            free_form: b_answer,
        });
        results.push(result);
    }
    let label = format!("{} − {}", schema_forced.label, free_form.label);
    Ok(DataSeries {
        subject_mem: subject.to_string(),
        points: vec![SeriesPoint::aggregate(label, n_trials, &results)],
        // The contamination screen and coverage measure run on the caller side and
        // their reports are attached to the returned series by the caller.
        excluded_contaminated: Vec::new(),
        coverage: Vec::new(),
        transcripts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> TaskSpec {
        TaskSpec {
            id: "t".into(),
            prompt: "what is X?".into(),
            reference: "X is Y".into(),
        }
    }

    fn b() -> Substrate {
        Substrate::new("free-form", "X is Y, captured as a free-form note.")
    }

    fn c() -> Substrate {
        Substrate::new("schema-forced", "X is Y, captured as a typed entity.")
    }

    #[test]
    fn build_arms_differ_only_in_substrate() {
        let (c_arm, b_arm) = build_substrate_arms(&task(), "claude-x", "sys", 1000, &c(), &b());
        // The substrate is the variable.
        assert_eq!(c_arm.substrate.label, "schema-forced");
        assert_eq!(b_arm.substrate.label, "free-form");
        assert_ne!(c_arm.substrate.content, b_arm.substrate.content);
        // Everything else matches.
        assert_eq!(c_arm.model, b_arm.model);
        assert_eq!(c_arm.system_prompt, b_arm.system_prompt);
        assert_eq!(c_arm.task_text, b_arm.task_text);
        assert_eq!(c_arm.token_budget, b_arm.token_budget);
        // ...and the matched pair passes the guard.
        check_single_substrate_variable(&c_arm, &b_arm).unwrap();
    }

    #[test]
    fn confound_different_model_is_refused() {
        let (mut c_arm, b_arm) = build_substrate_arms(&task(), "claude-x", "sys", 1000, &c(), &b());
        c_arm.model = "claude-y".into();
        let err = check_single_substrate_variable(&c_arm, &b_arm)
            .unwrap_err()
            .to_string();
        assert!(err.contains("model"), "{err}");
    }

    #[test]
    fn confound_different_system_prompt_is_refused() {
        // The classic rig: hand the schema arm richer instructions.
        let (c_arm, mut b_arm) = build_substrate_arms(&task(), "claude-x", "sys", 1000, &c(), &b());
        b_arm.system_prompt = "a different, leaner prompt".into();
        let err = check_single_substrate_variable(&c_arm, &b_arm)
            .unwrap_err()
            .to_string();
        assert!(err.contains("system_prompt"), "{err}");
    }

    #[test]
    fn confound_different_token_budget_is_refused() {
        // The other classic rig: give the schema arm more room.
        let (mut c_arm, b_arm) = build_substrate_arms(&task(), "claude-x", "sys", 1000, &c(), &b());
        c_arm.token_budget = 4000;
        let err = check_single_substrate_variable(&c_arm, &b_arm)
            .unwrap_err()
            .to_string();
        assert!(err.contains("token_budget"), "{err}");
    }

    #[test]
    fn non_distinct_substrates_are_refused() {
        let (c_arm, _) = build_substrate_arms(&task(), "claude-x", "sys", 1000, &c(), &b());
        let err = check_single_substrate_variable(&c_arm, &c_arm)
            .unwrap_err()
            .to_string();
        assert!(err.contains("substrate label"), "{err}");
    }

    #[test]
    fn fit_to_budget_leaves_small_content_untouched() {
        let small = "a short substrate";
        assert_eq!(fit_to_budget(small, 1000), small);
    }

    #[test]
    fn fit_to_budget_trims_on_a_word_boundary() {
        // budget 2 tokens ≈ 8 chars; "hello world foo" must be cut and not mid-word.
        let trimmed = fit_to_budget("hello world foo bar baz", 2);
        assert!(trimmed.chars().count() <= 8, "too long: {trimmed:?}");
        assert!(!trimmed.ends_with(' '), "trailing space: {trimmed:?}");
        // The cut landed on a boundary — no partial trailing word like "wor".
        assert!(
            "hello world foo bar baz".starts_with(&trimmed),
            "trimmed is not a prefix: {trimmed:?}"
        );
        assert!(
            trimmed.split_whitespace().all(|w| "hello world foo bar baz"
                .split_whitespace()
                .any(|orig| orig == w)),
            "a word was cut: {trimmed:?}"
        );
    }

    #[test]
    fn fit_to_budget_applied_equally_keeps_arms_matched() {
        // Two oversized substrates trimmed to the same budget still differ only in
        // their bytes — the guard still passes.
        let big_c = Substrate::new("schema-forced", "c ".repeat(1000));
        let big_b = Substrate::new("free-form", "b ".repeat(1000));
        let (c_arm, b_arm) = build_substrate_arms(&task(), "m", "sys", 50, &big_c, &big_b);
        assert_eq!(c_arm.token_budget, b_arm.token_budget);
        check_single_substrate_variable(&c_arm, &b_arm).unwrap();
    }

    #[test]
    fn incontext_args_carry_substrate_and_no_tools() {
        let (c_arm, _) = build_substrate_arms(&task(), "claude-opus-4-8", "sys", 1000, &c(), &b());
        let args = build_incontext_args(&c_arm);
        // No mount, retrieval held out.
        assert!(!args.iter().any(|a| a == "--mcp-config"), "{args:?}");
        let allow_idx = args.iter().position(|a| a == "--allowedTools").unwrap();
        assert_eq!(args[allow_idx + 1], "");
        // The substrate rides in the system prompt.
        let sys_idx = args.iter().position(|a| a == "--system-prompt").unwrap();
        assert!(
            args[sys_idx + 1].contains("typed entity"),
            "{:?}",
            args[sys_idx + 1]
        );
        // The task text is passed through.
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-p" && w[1] == "what is X?")
        );
    }

    #[test]
    fn incontext_args_differ_only_in_the_substrate_payload() {
        // Everything but the system-prompt substrate block is byte-identical.
        let (c_arm, b_arm) = build_substrate_arms(&task(), "m", "sys", 1000, &c(), &b());
        let c_args = build_incontext_args(&c_arm);
        let b_args = build_incontext_args(&b_arm);
        assert_eq!(c_args.len(), b_args.len());
        let sys_val = c_args
            .iter()
            .position(|a| a == "sys" || a.contains("Reference material"));
        // Find the lone differing slot and confirm it is the system-prompt value.
        let diffs: Vec<usize> = (0..c_args.len())
            .filter(|&i| c_args[i] != b_args[i])
            .collect();
        assert_eq!(diffs.len(), 1, "more than the substrate differs: {diffs:?}");
        let sys_idx = c_args.iter().position(|a| a == "--system-prompt").unwrap() + 1;
        assert_eq!(
            diffs[0], sys_idx,
            "the differing slot is not the system prompt"
        );
        let _ = sys_val;
    }

    #[test]
    fn validate_no_retrieval_rejects_a_tool_call() {
        let leaked = AgentAnswer {
            text: "answer".into(),
            tool_calls: vec!["mcp__memstead__memstead_search".into()],
        };
        assert!(validate_no_retrieval(&leaked).is_err());
    }

    #[test]
    fn validate_no_retrieval_accepts_a_pure_in_context_answer() {
        let clean = AgentAnswer {
            text: "answer from the substrate".into(),
            tool_calls: vec![],
        };
        validate_no_retrieval(&clean).unwrap();
    }

    /// A stub that scores the schema-forced arm higher than free-form and emits no
    /// tool calls — the exact shape the in-context runner produces and the
    /// no-retrieval validator enforces.
    struct StubRunner {
        c_quality: f64,
        b_quality: f64,
    }

    impl SubstrateRunner for StubRunner {
        fn run(&self, arm: &SubstrateArm) -> Result<AgentAnswer> {
            let q = if arm.substrate.label == "schema-forced" {
                self.c_quality
            } else {
                self.b_quality
            };
            Ok(AgentAnswer {
                text: format!("q={q:.3}"),
                tool_calls: vec![],
            })
        }
    }

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

    #[test]
    fn run_substrate_task_reports_positive_delta_when_schema_helps() {
        let runner = StubRunner {
            c_quality: 0.9,
            b_quality: 0.4,
        };
        let t = task();
        let (c_arm, b_arm) = build_substrate_arms(&t, "m", "sys", 1000, &c(), &b());
        let (r, _) =
            run_substrate_task(&runner, &ParseQualityJudge, &t, &c_arm, &b_arm, 5).unwrap();
        assert!((r.delta - 0.5).abs() < 1e-9, "delta = {}", r.delta);
        assert!(r.on_mean > r.off_mean, "C should beat B");
    }

    #[test]
    fn run_substrate_task_reports_negative_delta_plainly() {
        // Honesty: when free-form (B) beats schema-forced (C), the delta is
        // negative — no floor, no relabel.
        let runner = StubRunner {
            c_quality: 0.3,
            b_quality: 0.8,
        };
        let t = task();
        let (c_arm, b_arm) = build_substrate_arms(&t, "m", "sys", 1000, &c(), &b());
        let (r, _) =
            run_substrate_task(&runner, &ParseQualityJudge, &t, &c_arm, &b_arm, 3).unwrap();
        assert!(r.delta < 0.0, "delta = {}", r.delta);
        assert!((r.delta + 0.5).abs() < 1e-9, "delta = {}", r.delta);
    }

    #[test]
    fn run_substrate_task_refuses_a_confounded_pair() {
        let runner = StubRunner {
            c_quality: 0.9,
            b_quality: 0.4,
        };
        let t = task();
        let (mut c_arm, b_arm) = build_substrate_arms(&t, "m", "sys", 1000, &c(), &b());
        c_arm.token_budget = 9999; // a second variable
        assert!(run_substrate_task(&runner, &ParseQualityJudge, &t, &c_arm, &b_arm, 2).is_err());
    }

    #[test]
    fn run_substrate_series_emits_one_point_with_paired_deltas() {
        let runner = StubRunner {
            c_quality: 0.8,
            b_quality: 0.5,
        };
        let tasks = vec![
            task(),
            TaskSpec {
                id: "t2".into(),
                prompt: "what is Z?".into(),
                reference: "Z is W".into(),
            },
        ];
        let series = run_substrate_series(
            &runner,
            &ParseQualityJudge,
            "engine",
            &tasks,
            &c(),
            &b(),
            "m",
            "sys",
            1000,
            3,
        )
        .unwrap();
        assert_eq!(series.subject_mem, "engine");
        assert_eq!(series.points.len(), 1);
        let p = &series.points[0];
        assert!((p.delta - 0.3).abs() < 1e-9, "delta = {}", p.delta);
        assert_eq!(p.state_label, "schema-forced − free-form");
        // The per-task paired comparison is carried through.
        assert_eq!(p.per_task.len(), 2);
    }
}
