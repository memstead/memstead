//! `xtask` — internal command runner for build-tooling tasks against the
//! engine workspace: `generate-docs` regenerates the deterministic
//! API-docs Markdown tree from the live MCP / CLI / UniFFI / WASM source
//! and the v1 binding schema + medium-capability matrix (the Registry
//! HTTP reference is generated separately by the private
//! `memstead-registry` crate); `release` runs the mechanical leg of
//! cutting a release (see `release.rs`); `eval` is the compounding-proof
//! harness.
//!
//! Invocation: `cargo run -p xtask -- <subcommand>`.

mod binding_ref;
mod errors;
mod eval;
mod mcp;
mod parity;
mod release;
mod udl;
mod wasm;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "xtask", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

// Boxing `EvalArgs` would break the clap derive (`Box<T>` doesn't
// implement `Args`), and exactly one Command exists per process anyway.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
enum Command {
    /// Regenerate the deterministic API-docs Markdown tree from the live
    /// source. Output is byte-stable on identical source — re-running the
    /// command on the same commit must produce zero diff.
    GenerateDocs(GenerateDocsArgs),
    /// Compounding-proof eval harness. `--self-test` runs the full loop with
    /// deterministic stubs (no `claude`, no real mem) and writes a chart-ready
    /// data series — proving the pipeline wires up end-to-end.
    Eval(EvalArgs),
    /// The mechanical leg of cutting a release: version bump across the
    /// workspace + inter-crate pins, changelog cut, docs-vs-binary guard,
    /// API-docs regeneration, and the test/lint matrix. Prints the outward
    /// steps (commit → push → CI green → tag → gitlink bump) — it never
    /// runs `git` mutations itself.
    Release(release::ReleaseArgs),
}

#[derive(clap::Args, Debug)]
struct EvalArgs {
    /// Run the harness loop with stub runner + judge and emit a sample data
    /// series — proves the scaffold without invoking `claude` or a real mem.
    #[arg(long)]
    self_test: bool,
    /// Where to write the JSON data series. Optional under `--self-test` (a
    /// scaffold check defaults to a temp file); required for a real run.
    #[arg(long)]
    output: Option<PathBuf>,

    // --- real-run options (used when --self-test is absent) ---
    /// Name of the subject mem under test (recorded in the series).
    #[arg(long)]
    subject: Option<String>,
    /// JSON task file: `[{"id","prompt","reference"}, …]`.
    #[arg(long)]
    tasks: Option<PathBuf>,
    /// A mem state to score, as `label=path-to-mcp-config`. Repeatable; the
    /// compounding axis needs ≥2. `label=` (empty path) is the no-mount baseline.
    #[arg(long = "state")]
    states: Vec<String>,
    /// Build the states from git history instead of `--state`: a mem branch
    /// (e.g. `memstead/engine`) whose commits become the compounding axis. The
    /// oldest picked commit is the near-empty baseline, the newest the current
    /// graph.
    #[arg(long)]
    replay_branch: Option<String>,
    /// How many historical states to span when `--replay-branch` is set.
    #[arg(long, default_value_t = 2)]
    replay_count: usize,
    /// The live workspace to replay from (the copy source).
    #[arg(long)]
    replay_workspace: Option<PathBuf>,
    /// Path to the `memstead-mcp` binary the generated mcp-configs invoke.
    #[arg(long)]
    mcp_binary: Option<PathBuf>,
    /// Where to materialise the per-state workspace copies + mcp-configs.
    #[arg(long, default_value = "/tmp/eval-states")]
    replay_dir: PathBuf,
    /// Model id passed to both arms (same model is the point).
    #[arg(long, default_value = "claude-opus-4-8")]
    model: String,
    /// Trials per arm per task — the N behind the variance.
    #[arg(long, default_value_t = 3)]
    trials: usize,

    // --- substrate-quality mode (the write-side test) ---
    /// The schema-forced substrate (C): a markdown file of the corpus captured as
    /// typed entities. Setting both `--substrate-c` and `--substrate-b` selects the
    /// substrate mode — the whole substrate goes in context, retrieval held out.
    #[arg(long)]
    substrate_c: Option<PathBuf>,
    /// The free-form substrate (B): a markdown file of the *same* corpus captured
    /// as good-faith, lightly-structured notes (the fair baseline).
    #[arg(long)]
    substrate_b: Option<PathBuf>,
    /// The shared context budget, in approximate tokens. Both substrates are
    /// trimmed to this same budget under the same rule.
    #[arg(long, default_value_t = 50_000)]
    token_budget: usize,
    /// A source corpus file. When set, the harness *captures* both substrates from
    /// it (free-reason-then-write, same model, prompt parity) instead of reading
    /// pre-built `--substrate-c`/`--substrate-b` files — the full write-side run.
    #[arg(long)]
    capture_corpus: Option<PathBuf>,
    /// MCP config mounting the (empty) destination mem for the schema-forced
    /// capture. Optional: when omitted, the harness self-provisions a fresh mem at
    /// `--capture-workspace` (a single self-contained capture run). Supply this only
    /// to point capture at a mem you provisioned yourself.
    #[arg(long)]
    capture_mcp_config: Option<PathBuf>,
    /// The destination mem's entity directory, read back as the schema-forced
    /// substrate after capture. Paired with `--capture-mcp-config`; both omitted →
    /// the harness self-provisions and uses the provisioned workspace.
    #[arg(long)]
    capture_entity_dir: Option<PathBuf>,
    /// Where to self-provision the empty destination mem when `--capture-mcp-config`
    /// is not supplied. Cleared and re-initialised each run.
    #[arg(long, default_value = "/tmp/eval-capture-mem")]
    capture_workspace: PathBuf,
    /// Path to the `memstead` CLI binary, used to `init` the self-provisioned
    /// capture mem. Required for self-provisioning (when `--capture-mcp-config` is
    /// absent and `--capture-corpus` is set).
    #[arg(long)]
    cli_binary: Option<PathBuf>,
    /// Schema pin for the self-provisioned capture mem.
    #[arg(long, default_value = "default@1.0.0")]
    capture_schema: String,
    /// Contamination threshold for the substrate mode's no-substrate (A) screen:
    /// any task the bare model answers at or above this score is excluded from the
    /// C−B delta as guessable. `0.0` disables the screen (no exclusion).
    #[arg(long, default_value_t = 0.5)]
    contamination_threshold: f64,
    /// JSON facts file `[{"id","statement"}, …]` of ground-truth source facts. When
    /// set, the substrate mode measures each substrate's coverage of these facts and
    /// surfaces the dropped (information-loss) set alongside the task delta.
    #[arg(long)]
    facts: Option<PathBuf>,

    // --- divergence mode (the longitudinal write-side test) ---
    /// The pre-registration package directory (e.g.
    /// `docs/proof/divergence/prereg`). Selects the divergence mode: the harness
    /// loads and content-hash-pins the package, then drives the round loop from it.
    #[arg(long)]
    package: Option<PathBuf>,
    /// Optional expected package content hash. When set, the divergence mode
    /// refuses to run unless the package hashes to exactly this value — the
    /// resume/pinning guard that stops a run against an edited package.
    #[arg(long)]
    pin: Option<String>,
    /// The pinned source repository clone (divergence mode): a `karalang/kara`
    /// checkout whose history contains every commit the package's `slices.json`
    /// references. The round digests (amendment A2) are assembled from it by `git`.
    #[arg(long)]
    source_repo: Option<PathBuf>,
    /// Smoke mode (divergence): drive a reduced in-memory campaign — one round, one
    /// query, one trial, one integrity audit — that exercises the whole live glue
    /// without editing the pinned package. The result is a throwaway verification
    /// (`smoke-result.json`), never the published campaign, and never resumed.
    #[arg(long)]
    smoke: bool,
    /// Documentary fallback (divergence, amendment A1): omit the per-session
    /// `--max-budget-usd` cap. Allowances become documentary targets, the ledger
    /// still publishes actual usage, and the report must state that allowances were
    /// not hard-enforced. Use when the proportional budget starves a session before
    /// it completes a legitimate write.
    #[arg(long)]
    no_budget: bool,
}

#[derive(clap::Args, Debug)]
struct GenerateDocsArgs {
    /// Target directory for the regenerated Markdown tree. Pre-existing
    /// files are overwritten; missing files are created.
    #[arg(long)]
    output: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::GenerateDocs(args) => generate_docs(args),
        Command::Eval(args) => run_eval(args),
        Command::Release(args) => release::run(args),
    }
}

fn run_eval(args: EvalArgs) -> Result<()> {
    if args.self_test {
        // A scaffold check shouldn't force choosing an output path; default it.
        let output = args
            .output
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("memstead-eval-selftest.json"));
        return eval::selftest::run(&output);
    }
    // Divergence mode: selected by --package. It consumes the pre-registration
    // package as its only configuration and does not use --subject/--tasks.
    if args.package.is_some() {
        return run_divergence_eval(&args);
    }
    let subject = args
        .subject
        .clone()
        .context("a real run needs --subject <mem name> (or pass --self-test)")?;
    let tasks_path = args
        .tasks
        .clone()
        .context("a real run needs --tasks <path to JSON task file>")?;
    let tasks = eval::tasks::load_tasks(&tasks_path)?;

    // Substrate-quality mode: the write-side test. Selected by supplying the
    // pre-built capture files or a corpus to capture from; the whole substrate goes
    // in context and retrieval is held out.
    if args.substrate_c.is_some() || args.substrate_b.is_some() || args.capture_corpus.is_some() {
        return run_substrate_eval(&args, &subject, &tasks);
    }
    let states = if let Some(branch) = args.replay_branch.as_ref() {
        let workspace = args
            .replay_workspace
            .context("--replay-branch needs --replay-workspace <path to a live workspace>")?;
        let mcp_binary = args
            .mcp_binary
            .context("--replay-branch needs --mcp-binary <path to memstead-mcp>")?;
        eprintln!(
            "preparing {} historical states of {branch}…",
            args.replay_count
        );
        eval::replay::prepare_history(
            &workspace,
            &args.replay_dir,
            branch,
            args.replay_count,
            &mcp_binary,
        )?
    } else if !args.states.is_empty() {
        args.states
            .iter()
            .map(|s| eval::tasks::parse_state_arg(s))
            .collect::<Result<Vec<_>>>()?
    } else {
        anyhow::bail!(
            "a real run needs either --replay-branch (git-history states) or \
             at least one --state label=path-to-mcp-config"
        );
    };
    let runner = eval::claude::ClaudeRunner::default();
    let judge = eval::judge::ClaudeJudge::new(args.model.clone());
    // Induce tool use (so the mem-on arm actually exercises the mount) while
    // forbidding source-citation (so the answer carries no tell of which arm
    // produced it). The mem-off arm has no tools, so it answers from the bare
    // model; the mem-on arm researches the graph, then answers cleanly.
    let system_prompt = "Answer the question as precisely and completely as you can. If any tools \
        are available to you, use them to research the answer before responding. In your final \
        answer, do NOT mention your sources, tools, or how you found the information — state the \
        answer directly, as a self-contained factual claim.";
    eprintln!(
        "eval run: subject={subject:?} tasks={} states={} model={} trials={}",
        tasks.len(),
        states.len(),
        args.model,
        args.trials
    );
    let series = eval::run_series(
        &runner,
        &judge,
        &subject,
        &tasks,
        &states,
        &args.model,
        system_prompt,
        args.trials,
    )?;
    let output = args
        .output
        .as_ref()
        .context("a real run needs --output <path>")?;
    series.write(output)?;
    for p in &series.points {
        eprintln!(
            "  {:<14} delta={:+.3} (on={:.3} off={:.3}, n={})",
            p.state_label, p.delta, p.on_mean, p.off_mean, p.n_trials
        );
    }
    eprintln!("wrote series to {}", output.display());
    Ok(())
}

/// Source the two substrates: either capture them from a corpus (the full
/// write-side run) or read pre-built capture files.
fn obtain_substrates(
    args: &EvalArgs,
) -> Result<(eval::substrate::Substrate, eval::substrate::Substrate)> {
    if let Some(corpus_path) = args.capture_corpus.as_ref() {
        let corpus = std::fs::read_to_string(corpus_path)
            .with_context(|| format!("reading capture corpus {}", corpus_path.display()))?;
        // Self-provision the destination mem unless the operator supplied one, so a
        // real capture run is a single self-contained command.
        let (mcp_config, entity_dir) = match args.capture_mcp_config.clone() {
            Some(cfg) => (
                Some(cfg),
                Some(args.capture_entity_dir.clone().context(
                    "--capture-mcp-config requires --capture-entity-dir to read the substrate back",
                )?),
            ),
            None => {
                let cli = args.cli_binary.as_ref().context(
                    "self-provisioning the capture mem needs --cli-binary <memstead> \
                     (or supply --capture-mcp-config + --capture-entity-dir yourself)",
                )?;
                let mcp = args.mcp_binary.as_ref().context(
                    "self-provisioning the capture mem needs --mcp-binary <memstead-mcp>",
                )?;
                eprintln!(
                    "provisioning empty capture mem at {}…",
                    args.capture_workspace.display()
                );
                let (cfg, dir) = eval::capture::provision_capture_mem(
                    cli,
                    mcp,
                    &args.capture_workspace,
                    "corpus",
                    &args.capture_schema,
                )?;
                (Some(cfg), Some(dir))
            }
        };
        let capture = eval::capture::ClaudeCapture {
            runner: eval::claude::ClaudeRunner::default(),
            schema_mcp_config: mcp_config,
            schema_entity_dir: entity_dir,
        };
        eprintln!("capturing free-form (B) and schema-forced (C) substrates from corpus…");
        return eval::capture::capture_pair(
            &capture,
            &args.model,
            eval::capture::DEFAULT_REASONING_STEP,
            &corpus,
        );
    }
    let c_path = args.substrate_c.as_ref().context(
        "substrate mode needs --substrate-c <schema-forced markdown> (or --capture-corpus to build it)",
    )?;
    let b_path = args.substrate_b.as_ref().context(
        "substrate mode needs --substrate-b <free-form markdown> (or --capture-corpus to build it)",
    )?;
    let c = eval::substrate::Substrate::new(
        "schema-forced",
        std::fs::read_to_string(c_path)
            .with_context(|| format!("reading schema-forced substrate {}", c_path.display()))?,
    );
    let b = eval::substrate::Substrate::new(
        "free-form",
        std::fs::read_to_string(b_path)
            .with_context(|| format!("reading free-form substrate {}", b_path.display()))?,
    );
    Ok((c, b))
}

/// The substrate-quality run: compare a schema-forced capture (C) against a
/// free-form capture (B) of the same corpus, each placed wholly in context with
/// retrieval held out. The only cross-arm difference is the substrate bytes.
fn run_substrate_eval(args: &EvalArgs, subject: &str, tasks: &[eval::TaskSpec]) -> Result<()> {
    let (c, b) = obtain_substrates(args)?;
    let runner = eval::claude::ClaudeRunner::default();
    let judge = eval::judge::ClaudeJudge::new(args.model.clone());
    // Identical across arms — the substrate is composed onto this base, never the
    // base itself. It must not name a capture form (that would be a tell and a
    // confound); it only sets the in-context-only contract.
    let system_prompt = "Answer the question as precisely and completely as you can, using only \
        the reference material provided. Do not use outside knowledge. In your final answer, do \
        NOT mention the reference material, its structure, or how it was organised — state the \
        answer directly, as a self-contained factual claim.";
    eprintln!(
        "substrate eval: subject={subject:?} tasks={} budget={} model={} trials={}",
        tasks.len(),
        args.token_budget,
        args.model,
        args.trials
    );
    // Contamination guard: screen out tasks the bare model already knows before the
    // comparison, so the C−B delta speaks only to substrate value, not prior
    // knowledge. A zero threshold disables the screen.
    let (kept, report) = if args.contamination_threshold > 0.0 {
        eprintln!(
            "contamination screen: A arm (no substrate) over {} tasks…",
            tasks.len()
        );
        let (kept, report) = eval::contamination::screen_tasks(
            &runner,
            &judge,
            tasks,
            &args.model,
            args.contamination_threshold,
            args.trials,
        )?;
        for ex in &report.excluded {
            eprintln!(
                "  excluded (guessable): {} bare_score={:.3}",
                ex.task_id, ex.bare_score
            );
        }
        (kept, report.excluded)
    } else {
        (tasks.to_vec(), Vec::new())
    };
    if kept.is_empty() {
        anyhow::bail!(
            "every task was excluded by the contamination screen — the corpus is fully guessable, \
             so there is no clean B-vs-C comparison to run"
        );
    }
    let mut series = eval::substrate::run_substrate_series(
        &runner,
        &judge,
        subject,
        &kept,
        &c,
        &b,
        &args.model,
        system_prompt,
        args.token_budget,
        args.trials,
    )?;
    series.excluded_contaminated = report;
    // Information-loss / coverage: measure what each substrate dropped relative to
    // the source facts, so a precision win that loses recall is surfaced alongside
    // the task delta rather than hidden by it.
    if let Some(facts_path) = args.facts.as_ref() {
        let facts = eval::coverage::load_facts(facts_path)?;
        let checker = eval::coverage::ClaudeCoverageChecker::new(args.model.clone());
        eprintln!(
            "coverage: measuring C and B against {} source facts…",
            facts.len()
        );
        for substrate in [&c, &b] {
            let cov = eval::coverage::measure_coverage(
                &checker,
                &substrate.label,
                &substrate.content,
                &facts,
            )?;
            eprintln!(
                "  {:<14} coverage={:.3} dropped={:?}",
                substrate.label, cov.coverage, cov.dropped
            );
            series.coverage.push(cov);
        }
    }
    let output = args
        .output
        .as_ref()
        .context("a real run needs --output <path>")?;
    series.write(output)?;
    for p in &series.points {
        eprintln!(
            "  {:<28} delta(C−B)={:+.3} (C={:.3} B={:.3}, n={})",
            p.state_label, p.delta, p.on_mean, p.off_mean, p.n_trials
        );
    }
    eprintln!("wrote series to {}", output.display());
    Ok(())
}

/// Divergence mode entry: load and content-hash-pin the pre-registration package,
/// validate it, report the resolved round plan, then drive the real campaign —
/// per-round `claude -p` writer sessions per arm, the blinded reader battery and
/// integrity audit at their checkpoints, one cost ledger across all four roles,
/// and the resumable serialised result. `--smoke` runs a reduced in-memory
/// campaign (one round / query / trial) to verify the live glue without editing
/// the pinned package.
fn run_divergence_eval(args: &EvalArgs) -> Result<()> {
    let package_dir = args
        .package
        .as_ref()
        .context("divergence mode needs --package <pre-registration package dir>")?;
    let pin = args.pin.as_deref();
    let pkg = eval::divergence::Package::load(package_dir).with_context(|| {
        format!(
            "loading pre-registration package at {}",
            package_dir.display()
        )
    })?;
    if let Some(expected) = pin {
        pkg.verify_pin(expected)?;
    }
    let model = pkg.single_model()?;
    let c = &pkg.campaign;
    eprintln!(
        "loaded pre-registration package at {}",
        package_dir.display()
    );
    eprintln!("  content hash : {}", pkg.content_hash);
    eprintln!(
        "  model        : {model} (judge {}, auditor {})",
        pkg.models.judge, pkg.models.auditor
    );
    eprintln!(
        "  schedule     : {} rounds, hurry {:?}, reader checkpoints {:?}, integrity audits {:?}",
        c.rounds, c.hurry_rounds, c.reader_checkpoints, c.integrity_audit_rounds
    );
    eprintln!(
        "  budgets      : {} trials; writer {}/{} (full/hurry), reader {}; cost cap {}; contamination {}",
        c.trials,
        c.writer_allowance_full_tokens,
        c.writer_allowance_hurry_tokens,
        c.reader_budget_tokens,
        c.cost_cap_tokens,
        c.contamination_threshold
    );
    eprintln!(
        "  tell lists   : {} arm-A + {} arm-B = {} blinding phrases",
        pkg.tell_lists.arm_a.len(),
        pkg.tell_lists.arm_b.len(),
        pkg.tell_lists.combined().len()
    );
    // Prompt parity self-check: the two arms' assembled writer prompts (and
    // reader prompts) must differ only where the substrate block is substituted
    // (criterion 5). A violation would mean the skeleton is not shared.
    use eval::divergence::Arm;
    let prompts = &pkg.prompts;
    let writer_parity = prompts.writer(Arm::A, false, "").replace(
        &prompts.writer_substrate.arm_a,
        &prompts.writer_substrate.arm_b,
    ) == prompts.writer(Arm::B, false, "");
    let reader_parity = prompts.reader(Arm::A, "").replace(
        &prompts.reader_substrate.arm_a,
        &prompts.reader_substrate.arm_b,
    ) == prompts.reader(Arm::B, "");
    eprintln!(
        "  prompts      : writer/reader skeletons loaded; substrate blocks writer {}/{}, reader {}/{} chars; parity writer {}, reader {}; auditor skeleton {} chars (arm-neutral)",
        prompts.writer_substrate.arm_a.len(),
        prompts.writer_substrate.arm_b.len(),
        prompts.reader_substrate.arm_a.len(),
        prompts.reader_substrate.arm_b.len(),
        if writer_parity { "OK" } else { "VIOLATED" },
        if reader_parity { "OK" } else { "VIOLATED" },
        prompts.auditor_skeleton.len()
    );
    eprintln!("  round plan   :");
    for rp in c.schedule() {
        let mut marks = Vec::new();
        if rp.hurry {
            marks.push("hurry");
        }
        if rp.reader_checkpoint {
            marks.push("reader-checkpoint");
        }
        if rp.integrity_audit {
            marks.push("integrity-audit");
        }
        let marks = if marks.is_empty() {
            "writer only".to_string()
        } else {
            marks.join(", ")
        };
        eprintln!(
            "    round {:>2}: writer {:>4} tokens — {}",
            rp.round, rp.writer_allowance_tokens, marks
        );
    }
    eprintln!("  package hash : {}", pkg.content_hash);

    // ---- build the real runner/judge/auditor and drive the campaign ----
    let cli_binary = args
        .cli_binary
        .as_ref()
        .context("divergence mode needs --cli-binary <memstead> to provision Arm B's mem")?;
    let mcp_binary = args
        .mcp_binary
        .as_ref()
        .context("divergence mode needs --mcp-binary <memstead-mcp> to mount Arm B's mem")?;
    let source_repo = args.source_repo.as_ref().context(
        "divergence mode needs --source-repo <kara clone containing the pinned slice commits>",
    )?;
    let out_dir = args
        .output
        .as_ref()
        .context("divergence mode needs --output <campaign output directory>")?;
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("creating campaign output dir {}", out_dir.display()))?;

    // Every round's mechanical digest, assembled from the pinned source (A2).
    let manifest = package_dir.join("slices.json");
    let mut slices = eval::divergence::extract_round_slices(source_repo, &manifest)
        .context("assembling round-slice digests from the source repo")?;
    let mut queries = eval::divergence::load_queries(package_dir)?;

    // Materialise both substrates. Arm A: a loose markdown directory. Arm B: a
    // fresh folder-backed mem pinned to the built-in `software@0.1.0` schema,
    // provisioned once through the CLI (the engine owns mem state) and reused on
    // resume. Both accumulate across rounds.
    let arm_a_dir = out_dir.join("arm-a");
    let arm_b_workspace = out_dir.join("arm-b-mem");
    let sandbox_dir = out_dir.join("sandbox");
    std::fs::create_dir_all(&arm_a_dir)?;
    std::fs::create_dir_all(&sandbox_dir)?;
    let arm_b_mcp_config = arm_b_workspace.join("capture.mcp.json");
    if !arm_b_mcp_config.exists() {
        eprintln!(
            "provisioning empty Arm B mem (software@0.1.0) at {}…",
            arm_b_workspace.display()
        );
        eval::capture::provision_capture_mem(
            cli_binary,
            mcp_binary,
            &arm_b_workspace,
            "kara-knowledge",
            "software@0.1.0",
        )?;
    } else {
        eprintln!("reusing existing Arm B mem at {}", arm_b_workspace.display());
    }

    let runner = eval::divergence::ClaudeDivergenceRunner {
        executable: "claude".to_string(),
        arm_a_dir,
        arm_b_workspace,
        arm_b_mcp_config,
        sandbox_dir: sandbox_dir.clone(),
        // Amendment A1: enforce allowances as proportional `--max-budget-usd` caps,
        // unless the documentary fallback (`--no-budget`) is selected.
        usd_per_output_token: if args.no_budget {
            None
        } else {
            Some(pkg.campaign.usd_per_output_token)
        },
    };
    if args.no_budget {
        eprintln!(
            "A1 documentary fallback: allowances are NOT hard-enforced (no --max-budget-usd cap)"
        );
    }
    let judge = eval::divergence::ClaudeDivergenceJudge {
        executable: "claude".to_string(),
        sandbox_dir: sandbox_dir.clone(),
    };
    let auditor = eval::divergence::ClaudeDivergenceAuditor {
        executable: "claude".to_string(),
        sandbox_dir,
    };

    // Smoke mode reduces the campaign in memory (never editing the pinned package):
    // one round, one query, one trial, one integrity audit — enough to exercise
    // every glue path (both writers, the reader battery + judge, the integrity
    // audit, entropy, ledger, serialisation) with ~8 live sessions.
    let pkg = if args.smoke {
        eprintln!("SMOKE run: reduced campaign (1 round, 1 query, 1 trial, 1 integrity audit)");
        let mut p = pkg.clone();
        p.campaign.rounds = 1;
        p.campaign.hurry_rounds = vec![];
        p.campaign.reader_checkpoints = vec![1];
        p.campaign.integrity_audit_rounds = vec![1];
        p.campaign.trials = 1;
        slices.truncate(1);
        queries.truncate(1);
        p
    } else {
        pkg
    };

    let state_path = out_dir.join("state.json");
    let result = eval::divergence::run_campaign(
        &runner,
        &judge,
        &auditor,
        &pkg,
        &slices,
        &queries,
        if args.smoke {
            None
        } else {
            Some(state_path.as_path())
        },
    )?;

    let result_path = out_dir.join(if args.smoke {
        "smoke-result.json"
    } else {
        "result.json"
    });
    std::fs::write(&result_path, result.to_json()?)
        .with_context(|| format!("writing campaign result {}", result_path.display()))?;

    let cost = result.ledger.summary();
    eprintln!("wrote campaign result to {}", result_path.display());
    eprintln!(
        "cost: {} / {} tokens (A writer {}, B writer {}, judge {}, auditor {})",
        cost.total_tokens,
        cost.cap_tokens,
        cost.arm_a_writer,
        cost.arm_b_writer,
        cost.judge,
        cost.auditor
    );
    for cp in &result.checkpoints {
        for r in &cp.results {
            eprintln!(
                "  reader@{} {}: delta(B-A)={:+.3} (B={:.3} A={:.3})",
                cp.round, r.task_id, r.delta, r.on_mean, r.off_mean
            );
        }
    }
    for ic in &result.integrity_checkpoints {
        eprintln!(
            "  integrity@{}: delta(A-B)={:+.3} defects/100 (A items {}, B items {})",
            ic.round, ic.result.delta, ic.arm_a_items, ic.arm_b_items
        );
    }
    Ok(())
}

fn generate_docs(args: GenerateDocsArgs) -> Result<()> {
    fs::create_dir_all(&args.output)
        .with_context(|| format!("creating output dir {}", args.output.display()))?;
    write_cli_reference(&args.output)?;
    write_uniffi_reference(&args.output)?;
    write_wasm_reference(&args.output)?;
    write_mcp_reference(&args.output)?;
    write_parity_matrix(&args.output)?;
    write_error_index(&args.output)?;
    write_binding_reference(&args.output)?;
    Ok(())
}

fn write_binding_reference(output: &Path) -> Result<()> {
    let schema_path = workspace_root().join("docs/schemas/memstead-plugin/v1/binding.schema.json");
    let rendered = binding_ref::render_from_file(&schema_path)?;
    write_if_changed(
        &output.join("binding.md"),
        &with_frontmatter("Binding format", &rendered),
    )
}

fn write_error_index(output: &Path) -> Result<()> {
    let rendered = errors::render(&workspace_root())?;
    write_if_changed(
        &output.join("errors.md"),
        &with_frontmatter("Error Code Index", &rendered),
    )
}

fn write_mcp_reference(output: &Path) -> Result<()> {
    write_if_changed(
        &output.join("mcp.md"),
        &with_frontmatter("MCP tools", &mcp::render()),
    )
}

fn write_parity_matrix(output: &Path) -> Result<()> {
    let workspace_root = workspace_root();
    let udl_text =
        std::fs::read_to_string(workspace_root.join("crates/memstead-swift/src/memstead.udl"))?;
    let wasm_path = workspace_root.join("crates/memstead-wasm/src/lib.rs");
    let wasm_methods = wasm::method_names_from_file(&wasm_path)?;
    let operations_toml = include_str!("../operations.toml");
    let inputs = parity::collect_inputs(&udl_text, wasm_methods);
    let rendered = parity::render(operations_toml, &inputs)?;
    write_if_changed(
        &output.join("parity.md"),
        &with_frontmatter("Surface Parity Matrix", &rendered),
    )
}

fn write_uniffi_reference(output: &Path) -> Result<()> {
    let workspace_root = workspace_root();
    let udl_path = workspace_root.join("crates/memstead-swift/src/memstead.udl");
    let rendered = udl::render_from_file(&udl_path)?;
    write_if_changed(
        &output.join("uniffi.md"),
        &with_frontmatter("UniFFI surface", &rendered),
    )
}

fn write_wasm_reference(output: &Path) -> Result<()> {
    let workspace_root = workspace_root();
    let wasm_path = workspace_root.join("crates/memstead-wasm/src/lib.rs");
    let rendered = wasm::render_from_file(&wasm_path)?;
    write_if_changed(
        &output.join("wasm.md"),
        &with_frontmatter("WASM surface", &rendered),
    )
}

/// Resolve the `engine` workspace root from the xtask crate
/// manifest path. Cargo sets `CARGO_MANIFEST_DIR` to
/// `<workspace>/xtask`; the parent is the workspace root.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask manifest dir has a parent")
        .to_path_buf()
}

fn write_cli_reference(output: &Path) -> Result<()> {
    let cli_dir = output.join("cli");
    fs::create_dir_all(&cli_dir).with_context(|| format!("creating {}", cli_dir.display()))?;

    // One CLI crate, one reference. `xtask` links `memstead-cli` with
    // `mem-repo` on, so this renders the full `memstead` surface.
    // clap help text is plain text; clap_markdown embeds it verbatim, so a
    // placeholder like `--status <S>` outside a code span parses as a raw
    // HTML tag downstream (tag names are case-insensitive — `<S>` is the
    // strikethrough element and struck out the rest of the rendered page).
    let cli = escape_raw_html_in_markdown(&clap_markdown::help_markdown_command(
        &memstead_cli::cli::Cli::command(),
    ));
    write_if_changed(
        &cli_dir.join("cli.md"),
        &with_frontmatter("CLI (`memstead`)", &cli),
    )?;
    Ok(())
}

/// Escape raw HTML-tag lookalikes in generated Markdown prose.
///
/// Replaces `<` with `&lt;` wherever it would open an HTML tag (next char
/// is a letter, `/`, `!`, or `?`), except inside fenced code blocks,
/// inline code spans, and autolinks (`<http://…>`, `<https://…>`,
/// `<mailto:…>`). Without this, plain-text help embedded in Markdown can
/// smuggle real elements into the rendered page — `<S>` (strikethrough)
/// visibly, any other placeholder by being silently swallowed.
fn escape_raw_html_in_markdown(md: &str) -> String {
    let mut out = String::with_capacity(md.len() + 64);
    let mut in_fence = false;
    for line in md.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        // Length of the opening backtick run of the current inline code
        // span; 0 = not inside a span. A span closes only on a backtick
        // run of exactly the same length (CommonMark).
        let mut span_ticks = 0usize;
        while i < chars.len() {
            if chars[i] == '`' {
                let mut run = 0;
                while i + run < chars.len() && chars[i + run] == '`' {
                    run += 1;
                }
                out.extend(std::iter::repeat_n('`', run));
                if span_ticks == 0 {
                    span_ticks = run;
                } else if run == span_ticks {
                    span_ticks = 0;
                }
                i += run;
                continue;
            }
            if chars[i] == '<' && span_ticks == 0 {
                let opens_tag = chars
                    .get(i + 1)
                    .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
                let rest: String = chars[i + 1..].iter().take(8).collect();
                let autolink = rest.starts_with("http://")
                    || rest.starts_with("https://")
                    || rest.starts_with("mailto:");
                if opens_tag && !autolink {
                    out.push_str("&lt;");
                    i += 1;
                    continue;
                }
            }
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Prepend a YAML frontmatter block carrying just the page title.
/// Starlight's content loader requires every entry to declare a title
/// (either via frontmatter or its schema's heading fallback); the
/// xtask emits the title explicitly so the rendered Markdown is
/// portable to any consumer that respects standard frontmatter.
fn with_frontmatter(title: &str, body: &str) -> String {
    let mut out = String::with_capacity(body.len() + 64);
    out.push_str("---\n");
    out.push_str("title: ");
    out.push_str(&yaml_double_quote(title));
    out.push('\n');
    out.push_str("---\n\n");
    out.push_str(body);
    out
}

fn yaml_double_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str(r#"\""#),
            '\\' => out.push_str(r"\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Write `contents` to `path` only when the file's existing bytes differ.
/// Idempotent writes keep mtimes stable for incremental builds and let
/// the drift-check workflow rely on `git diff --exit-code` to flag real
/// surface changes.
fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
    if let Ok(existing) = fs::read_to_string(path)
        && existing == contents
    {
        return Ok(());
    }
    fs::write(path, contents).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod escape_raw_html_tests {
    use super::escape_raw_html_in_markdown;

    #[test]
    fn escapes_tag_lookalikes_in_prose() {
        assert_eq!(
            escape_raw_html_in_markdown("--status <S>  Filter by status."),
            "--status &lt;S>  Filter by status."
        );
        // Case-insensitivity is the whole point: <S> == <s> == strikethrough.
        assert_eq!(escape_raw_html_in_markdown("a </del> b"), "a &lt;/del> b");
    }

    #[test]
    fn leaves_inline_code_spans_alone() {
        let s = "Usage: `memstead install [OPTIONS] <PATH or SCOPE/NAME>`";
        assert_eq!(escape_raw_html_in_markdown(s), s);
        // Double-backtick span containing a single backtick.
        let s = "``code with ` and <TAG>`` and prose <TAG>";
        assert_eq!(
            escape_raw_html_in_markdown(s),
            "``code with ` and <TAG>`` and prose &lt;TAG>"
        );
    }

    #[test]
    fn leaves_fenced_blocks_alone() {
        let s = "prose <X>\n```\ncode <Y>\n```\nprose <Z>\n";
        assert_eq!(
            escape_raw_html_in_markdown(s),
            "prose &lt;X>\n```\ncode <Y>\n```\nprose &lt;Z>\n"
        );
    }

    #[test]
    fn leaves_autolinks_and_comparisons_alone() {
        let s = "see <https://memstead.io> and note 3 < 5, also a <- b";
        assert_eq!(escape_raw_html_in_markdown(s), s);
    }
}
