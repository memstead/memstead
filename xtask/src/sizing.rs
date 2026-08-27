//! `xtask sizing-curve` — the measured operating-limits harness.
//!
//! Generates graded synthetic mem-repo workspaces entirely through the
//! product surface (`memstead mem-repo init` → `memstead mem init` →
//! `memstead batch-create`), then times the four everyday cold-CLI
//! operations at every size point:
//!
//! - **boot** — first command against a cold engine (`memstead list
//!   --limit 1`): process spawn + engine boot + full workspace load,
//!   which is exactly what a user's first command pays.
//! - **update** — one `memstead update --auto-hash --append …`,
//!   including the write commit and index invalidation.
//! - **search** — `memstead search` immediately after the update, i.e.
//!   the run that pays the search-index rebuild.
//! - **overview** — `memstead overview`, the community/summary path.
//!
//! Every workspace lives in a `tempfile::TempDir` and is deleted when
//! the run ends — a harness run leaves no state behind that `git
//! status` or the test suite would see. Results are written as one
//! machine-readable JSON document (`format: "sizing-curve/v1"`); the
//! committed curve document `docs/sizing-curve.md` is written from that
//! output, never from prose estimates.
//!
//! Deliberately measurement-only: the harness contains no engine code
//! and no tuning knobs that would change engine behaviour.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde::Serialize;

/// `xtask sizing-curve` arguments.
#[derive(clap::Args, Debug)]
pub struct SizingArgs {
    /// Comma-separated workspace sizes (entity counts) to measure.
    /// The default grid spans below, inside, and above the advertised
    /// 1,000–5,000 range, with the top point matching the largest
    /// real deployment observed (7.4k).
    #[arg(long, default_value = "500,2500,5000,7500")]
    pub sizes: String,

    /// Timed iterations per operation per size (median is reported).
    #[arg(long, default_value_t = 3)]
    pub iterations: usize,

    /// Where to write the machine-readable results JSON.
    #[arg(long, default_value = "target/sizing-curve.json")]
    pub output: PathBuf,

    /// Path to a pre-built `memstead` binary. When omitted, the harness
    /// builds `memstead-cli` in release mode and uses that — benchmark
    /// numbers from a debug binary would be fiction.
    #[arg(long)]
    pub memstead: Option<PathBuf>,
}

#[derive(Serialize)]
struct Results {
    format: &'static str,
    /// Host fingerprint — the curve is hardware-relative.
    host: Host,
    /// Release binary the run used.
    binary: String,
    iterations: usize,
    sizes: Vec<SizePoint>,
}

#[derive(Serialize)]
struct Host {
    os: String,
    arch: String,
    /// Best-effort CPU model string (empty when the probe fails).
    cpu: String,
}

#[derive(Serialize)]
struct SizePoint {
    entities: usize,
    /// Wall-clock of the whole corpus generation leg (init + one
    /// `batch-create` call) — context, not one of the four operations.
    generation_ms: u128,
    boot: OpStats,
    update: OpStats,
    search: OpStats,
    overview: OpStats,
}

#[derive(Serialize)]
struct OpStats {
    median_ms: u128,
    runs_ms: Vec<u128>,
}

impl OpStats {
    fn from_runs(mut runs: Vec<u128>) -> Self {
        let mut sorted = runs.clone();
        sorted.sort_unstable();
        let median_ms = sorted[sorted.len() / 2];
        runs.shrink_to_fit();
        Self {
            median_ms,
            runs_ms: runs,
        }
    }
}

pub fn run(args: SizingArgs) -> Result<()> {
    let sizes: Vec<usize> = args
        .sizes
        .split(',')
        .map(|s| s.trim().parse::<usize>())
        .collect::<std::result::Result<_, _>>()
        .context("--sizes must be comma-separated entity counts")?;
    if sizes.is_empty() || args.iterations == 0 {
        bail!("need at least one size and one iteration");
    }

    let binary = match args.memstead {
        Some(p) => p,
        None => build_release_binary()?,
    };
    if !binary.exists() {
        bail!("memstead binary not found at {}", binary.display());
    }

    let mut points = Vec::new();
    for &n in &sizes {
        eprintln!("── size {n}: generating workspace…");
        let ws = tempfile::TempDir::new().context("create temp workspace")?;
        let gen_start = Instant::now();
        generate_workspace(&binary, ws.path(), n)?;
        let generation_ms = gen_start.elapsed().as_millis();
        eprintln!("   generated in {generation_ms} ms; measuring…");

        let mut boot = Vec::new();
        let mut update = Vec::new();
        let mut search = Vec::new();
        let mut overview = Vec::new();
        for iter in 0..args.iterations {
            // boot: first command against a cold engine.
            boot.push(timed(&binary, ws.path(), &["list", "--limit", "1"])?);
            // update: one mutation including its index invalidation.
            // A fresh appended sentence per iteration keeps the write
            // non-trivial and the search term below present.
            let target = format!("bench--topic-{}", (iter % n.min(50)) + 1);
            let append = format!("purpose=Benchmark touch {iter}, quicksilver probe.");
            update.push(timed(
                &binary,
                ws.path(),
                &["update", &target, "--auto-hash", "--append", &append],
            )?);
            // search: the first search after a mutation pays the
            // index rebuild.
            search.push(timed(
                &binary,
                ws.path(),
                &["search", "quicksilver", "--limit", "10"],
            )?);
            // overview: the community/summary path.
            overview.push(timed(&binary, ws.path(), &["overview"])?);
        }
        points.push(SizePoint {
            entities: n,
            generation_ms,
            boot: OpStats::from_runs(boot),
            update: OpStats::from_runs(update),
            search: OpStats::from_runs(search),
            overview: OpStats::from_runs(overview),
        });
        // TempDir drop deletes the workspace — no residue.
    }

    let results = Results {
        format: "sizing-curve/v1",
        host: host_fingerprint(),
        binary: binary.display().to_string(),
        iterations: args.iterations,
        sizes: points,
    };
    if let Some(parent) = args.output.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&args.output, serde_json::to_string_pretty(&results)?)
        .with_context(|| format!("write {}", args.output.display()))?;

    // Human summary on stderr; the JSON file is the machine contract.
    eprintln!("\nentities  boot  update  search  overview   (median ms)");
    for p in &results.sizes {
        eprintln!(
            "{:>8}  {:>4}  {:>6}  {:>6}  {:>8}",
            p.entities,
            p.boot.median_ms,
            p.update.median_ms,
            p.search.median_ms,
            p.overview.median_ms,
        );
    }
    eprintln!("\nresults written to {}", args.output.display());
    Ok(())
}

/// Build the release `memstead` binary and return its path.
fn build_release_binary() -> Result<PathBuf> {
    let root = crate::workspace_root();
    eprintln!("building release memstead binary…");
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "memstead-cli"])
        .current_dir(&root)
        .status()
        .context("spawn cargo build")?;
    if !status.success() {
        bail!("cargo build --release -p memstead-cli failed");
    }
    Ok(root.join("target/release/memstead"))
}

/// Set up one mem-repo workspace with `n` entities through the product
/// surface: `mem-repo init` → `mem init bench` → one `batch-create`.
fn generate_workspace(binary: &Path, ws: &Path, n: usize) -> Result<()> {
    run_ok(binary, ws, &["mem-repo", "init", "."])?;
    run_ok(binary, ws, &["mem", "init", "bench", "--no-gitignore"])?;

    let corpus = corpus_json(n);
    let corpus_path = ws.join("corpus.json");
    std::fs::write(&corpus_path, corpus)?;
    run_ok(
        binary,
        ws,
        &["batch-create", "--from", corpus_path.to_str().unwrap()],
    )?;
    // The corpus file is inside the TempDir; it dies with the
    // workspace. Remove eagerly anyway so the measured workspace holds
    // only engine-owned state.
    std::fs::remove_file(&corpus_path).ok();
    Ok(())
}

/// Synthetic corpus: `n` spec entities with realistic density —
/// two-to-three prose sections, rotating `level` metadata, two explicit
/// edges to earlier entities (USES / DEPENDS_ON), and one body
/// wiki-link (which alias-emits REFERENCES). The shape follows the
/// plenum field deployment's flavour (typed prose + edge density ~3
/// per entity) without depending on it.
fn corpus_json(n: usize) -> String {
    let levels = ["M0", "M0", "M0", "M1", "M2"]; // mostly concrete, like real mems
    let mut creates = Vec::with_capacity(n);
    for i in 1..=n {
        let level = levels[i % levels.len()];
        let link = if i > 1 {
            format!(" It builds on [[topic-{}]].", ((i - 2) % (i - 1)) + 1)
        } else {
            String::new()
        };
        let identity = format!(
            "Synthetic subject number {i} in the sizing corpus, one of {n} \
             entities generated to measure engine behaviour at scale. This \
             sentence exists to give the section body a realistic prose \
             length rather than a stub marker.{link}"
        );
        let purpose = format!(
            "Provides measurement mass for the sizing curve: entity {i} of \
             {n} contributes typical section text, metadata, and edges."
        );
        let specifies = format!(
            "- Grid position {i}\n- Level {level}\n- Two outgoing edges to \
             earlier grid entities\n- One body wiki-link for the alias pass"
        );
        let mut relations = Vec::new();
        if i > 2 {
            relations.push(serde_json::json!({
                "rel_type": "USES",
                "target": format!("bench--topic-{}", i - 1),
            }));
            relations.push(serde_json::json!({
                "rel_type": "DEPENDS_ON",
                "target": format!("bench--topic-{}", i - 2),
            }));
        }
        creates.push(serde_json::json!({
            "title": format!("Topic {i}"),
            "entity_type": "spec",
            "sections": {
                "identity": identity,
                "purpose": purpose,
                "specifies": specifies,
            },
            "metadata": { "level": level },
            "relations": relations,
        }));
    }
    serde_json::to_string(&serde_json::json!({ "creates": creates })).expect("corpus serialises")
}

/// Run the binary, fail loudly on non-zero exit.
fn run_ok(binary: &Path, cwd: &Path, args: &[&str]) -> Result<()> {
    let out = Command::new(binary)
        .args(args)
        .current_dir(cwd)
        .env("MEMSTEAD_OPERATOR_MODE", "1")
        .output()
        .with_context(|| format!("spawn memstead {args:?}"))?;
    if !out.status.success() {
        bail!(
            "memstead {args:?} failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }
    Ok(())
}

/// Run the binary and return wall-clock milliseconds (spawn to exit).
fn timed(binary: &Path, cwd: &Path, args: &[&str]) -> Result<u128> {
    let start = Instant::now();
    run_ok(binary, cwd, args)?;
    Ok(start.elapsed().as_millis())
}

fn host_fingerprint() -> Host {
    let cpu = Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .or_else(|| {
            std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("model name"))
                    .and_then(|l| l.split(':').nth(1))
                    .map(|v| v.trim().to_string())
            })
        })
        .unwrap_or_default();
    Host {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu,
    }
}
