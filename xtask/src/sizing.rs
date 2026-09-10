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
//! Then the **warm path** at the same size point: one `memstead-mcp`
//! server is spawned over the workspace, boots once (a warm-up call
//! absorbs the load), and the three reads an agent issues most are
//! timed per call, request-to-reply over the server's stdio JSON-RPC:
//!
//! - **warm search** — `memstead_search` for a term present in the
//!   corpus, `limit` 10.
//! - **warm entity** — `memstead_entity` with `include_relations`, the
//!   read that scans the store for incoming edges.
//! - **warm overview** — `memstead_overview`, the workspace-global
//!   partition and roster.
//!
//! The warm figures are what a long-lived agent session pays per
//! call after boot; the cold figures are what a fresh CLI command
//! pays. Both come from the same corpus and the same binaries.
//!
//! Every workspace lives in a `tempfile::TempDir` and is deleted when
//! the run ends — a harness run leaves no state behind that `git
//! status` or the test suite would see. Results are written as one
//! machine-readable JSON document (`format: "sizing-curve/v2"`; v1
//! carried the cold leg alone, v2 adds the warm leg beside it); the
//! committed curve document `docs/sizing-curve.md` is written from that
//! output, never from prose estimates.
//!
//! Deliberately measurement-only: the harness contains no engine code
//! and no tuning knobs that would change engine behaviour.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};

/// `xtask sizing-curve` arguments.
#[derive(clap::Args, Debug)]
pub struct SizingArgs {
    /// Comma-separated workspace sizes (entity counts) to measure.
    /// The default grid spans below, inside, and above the advertised
    /// 1,000–5,000 range, with the top point matching the largest
    /// real deployment observed (7.4k).
    #[arg(long, default_value = "500,2500,5000,7500")]
    pub sizes: String,

    /// Timed iterations per cold operation per size (median is reported).
    #[arg(long, default_value_t = 3)]
    pub iterations: usize,

    /// Timed calls per warm operation per size (median is reported). Warm
    /// calls are sub-millisecond at the small end, so the warm leg runs
    /// more of them and reports microseconds.
    #[arg(long, default_value_t = 20)]
    pub warm_iterations: usize,

    /// Where to write the machine-readable results JSON.
    #[arg(long, default_value = "target/sizing-curve.json")]
    pub output: PathBuf,

    /// Path to a pre-built `memstead` binary. When omitted, the harness
    /// builds `memstead-cli` in release mode and uses that — benchmark
    /// numbers from a debug binary would be fiction.
    #[arg(long)]
    pub memstead: Option<PathBuf>,

    /// Path to a pre-built `memstead-mcp` binary for the warm leg. When
    /// omitted, the harness builds `memstead-mcp` in release mode
    /// beside the CLI.
    #[arg(long)]
    pub memstead_mcp: Option<PathBuf>,
}

#[derive(Serialize)]
struct Results {
    format: &'static str,
    /// Host fingerprint — the curve is hardware-relative.
    host: Host,
    /// Release binaries the run used: the CLI (cold leg) and the MCP
    /// server (warm leg).
    binary: String,
    mcp_binary: String,
    iterations: usize,
    warm_iterations: usize,
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
    /// Warm leg: one long-lived MCP server, boot paid once and excluded,
    /// per-call request-to-reply time in microseconds.
    warm_search: WarmStats,
    warm_entity: WarmStats,
    warm_overview: WarmStats,
}

#[derive(Serialize)]
struct WarmStats {
    median_us: u128,
    runs_us: Vec<u128>,
}

impl WarmStats {
    fn from_runs(runs: Vec<u128>) -> Self {
        let mut sorted = runs.clone();
        sorted.sort_unstable();
        Self {
            median_us: sorted[sorted.len() / 2],
            runs_us: runs,
        }
    }
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
    if sizes.is_empty() || args.iterations == 0 || args.warm_iterations == 0 {
        bail!("need at least one size and one iteration per leg");
    }

    let (binary, mcp_binary) = match (args.memstead, args.memstead_mcp) {
        (Some(cli), Some(mcp)) => (cli, mcp),
        (cli, mcp) => {
            let (built_cli, built_mcp) = build_release_binaries()?;
            (cli.unwrap_or(built_cli), mcp.unwrap_or(built_mcp))
        }
    };
    if !binary.exists() {
        bail!("memstead binary not found at {}", binary.display());
    }
    if !mcp_binary.exists() {
        bail!("memstead-mcp binary not found at {}", mcp_binary.display());
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
        // Warm leg: one server over the same workspace, boot paid once.
        eprintln!("   cold leg done; warm leg…");
        let mut session = McpSession::start(&mcp_binary, ws.path())?;
        // Warm-up: absorbs the boot and the first partition; not timed.
        session.call_tool("memstead_overview", json!({}))?;
        let mut warm_search = Vec::new();
        let mut warm_entity = Vec::new();
        let mut warm_overview = Vec::new();
        for iter in 0..args.warm_iterations {
            let target = format!("bench--topic-{}", (iter % n.min(50)) + 1);
            warm_search.push(session.timed_call(
                "memstead_search",
                json!({ "query": { "any": ["quicksilver"] }, "limit": 10 }),
            )?);
            warm_entity.push(session.timed_call(
                "memstead_entity",
                json!({ "id": target, "include_relations": true }),
            )?);
            warm_overview.push(session.timed_call("memstead_overview", json!({}))?);
        }
        drop(session);
        points.push(SizePoint {
            entities: n,
            generation_ms,
            boot: OpStats::from_runs(boot),
            update: OpStats::from_runs(update),
            search: OpStats::from_runs(search),
            overview: OpStats::from_runs(overview),
            warm_search: WarmStats::from_runs(warm_search),
            warm_entity: WarmStats::from_runs(warm_entity),
            warm_overview: WarmStats::from_runs(warm_overview),
        });
        // TempDir drop deletes the workspace — no residue.
    }

    let results = Results {
        format: "sizing-curve/v2",
        host: host_fingerprint(),
        binary: binary.display().to_string(),
        mcp_binary: mcp_binary.display().to_string(),
        iterations: args.iterations,
        warm_iterations: args.warm_iterations,
        sizes: points,
    };
    if let Some(parent) = args.output.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&args.output, serde_json::to_string_pretty(&results)?)
        .with_context(|| format!("write {}", args.output.display()))?;

    // Human summary on stderr; the JSON file is the machine contract.
    eprintln!(
        "\nentities  boot  update  search  overview (median ms) | warm search  warm entity  warm overview (median µs)"
    );
    for p in &results.sizes {
        eprintln!(
            "{:>8}  {:>4}  {:>6}  {:>6}  {:>8} | {:>11}  {:>11}  {:>13}",
            p.entities,
            p.boot.median_ms,
            p.update.median_ms,
            p.search.median_ms,
            p.overview.median_ms,
            p.warm_search.median_us,
            p.warm_entity.median_us,
            p.warm_overview.median_us,
        );
    }
    eprintln!("\nresults written to {}", args.output.display());
    Ok(())
}

/// Build the release `memstead` and `memstead-mcp` binaries and return
/// their paths (CLI first).
fn build_release_binaries() -> Result<(PathBuf, PathBuf)> {
    let root = crate::workspace_root();
    eprintln!("building release memstead and memstead-mcp binaries…");
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "-p",
            "memstead-cli",
            "-p",
            "memstead-mcp",
        ])
        .current_dir(&root)
        .status()
        .context("spawn cargo build")?;
    if !status.success() {
        bail!("cargo build --release -p memstead-cli -p memstead-mcp failed");
    }
    Ok((
        root.join("target/release/memstead"),
        root.join("target/release/memstead-mcp"),
    ))
}

/// One long-lived `memstead-mcp` child over stdio JSON-RPC: the warm
/// leg's instrument. The same newline-delimited protocol the MCP wire
/// tests drive; here every call is timed request-to-reply.
struct McpSession {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    reader: BufReader<ChildStdout>,
    next_id: i64,
}

impl McpSession {
    fn start(binary: &Path, cwd: &Path) -> Result<Self> {
        let mut child = Command::new(binary)
            .current_dir(cwd)
            .env("MEMSTEAD_OPERATOR_MODE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn memstead-mcp")?;
        let stdin = child.stdin.take().context("child stdin")?;
        let stdout = child.stdout.take().context("child stdout")?;
        let mut session = Self {
            child: Some(child),
            stdin: Some(stdin),
            reader: BufReader::new(stdout),
            next_id: 0,
        };
        let id = session.send(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "xtask-sizing-curve", "version": "0" }
            }),
        )?;
        session.read_response(id, Duration::from_secs(120))?;
        session.notify("notifications/initialized", json!({}))?;
        Ok(session)
    }

    fn send(&mut self, method: &str, params: Value) -> Result<i64> {
        self.next_id += 1;
        let id = self.next_id;
        let line = serde_json::to_string(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }))?;
        let stdin = self.stdin.as_mut().context("stdin open")?;
        writeln!(stdin, "{line}")?;
        stdin.flush()?;
        Ok(id)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let line = serde_json::to_string(&json!({
            "jsonrpc": "2.0", "method": method, "params": params,
        }))?;
        let stdin = self.stdin.as_mut().context("stdin open")?;
        writeln!(stdin, "{line}")?;
        stdin.flush()?;
        Ok(())
    }

    fn read_response(&mut self, want_id: i64, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        let mut line = String::new();
        loop {
            if Instant::now() >= deadline {
                bail!("no JSON-RPC response with id={want_id} within {timeout:?}");
            }
            line.clear();
            match self.reader.read_line(&mut line)? {
                0 => bail!("memstead-mcp closed stdout before id={want_id}"),
                _ => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                        continue;
                    };
                    if value.get("id").and_then(Value::as_i64) == Some(want_id) {
                        return Ok(value);
                    }
                }
            }
        }
    }

    /// `tools/call`, failing loudly on a JSON-RPC error or a tool-level
    /// error envelope (`isError`): a refused call is not a measurement.
    fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        let id = self.send(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )?;
        let response = self.read_response(id, Duration::from_secs(120))?;
        if let Some(err) = response.get("error") {
            bail!("{name}: JSON-RPC error {err}");
        }
        let result = response
            .get("result")
            .cloned()
            .context("tools/call reply carries no result")?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            bail!("{name} refused: {result}");
        }
        Ok(result)
    }

    /// Time one call, request written to reply read, in microseconds.
    fn timed_call(&mut self, name: &str, arguments: Value) -> Result<u128> {
        let start = Instant::now();
        self.call_tool(name, arguments)?;
        Ok(start.elapsed().as_micros())
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        drop(self.stdin.take());
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Set up one mem-repo workspace with `n` entities through the product
/// surface: `mem-repo init` → `mem init bench` → one `batch-create`.
fn generate_workspace(binary: &Path, ws: &Path, n: usize) -> Result<()> {
    run_ok(binary, ws, &["mem-repo", "init", "."])?;
    // Mem creation is refused by default (`MEM_PATH_NOT_ALLOWED` without
    // an allowlist rule); the grant is the operator act the harness
    // performs on its own throwaway workspace. `--schema *` keeps the
    // harness version-agnostic: `mem init` still pins the binary's
    // default generation.
    run_ok(
        binary,
        ws,
        &["workspace", "allow-create", "bench", "--schema", "*"],
    )?;
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
/// a field deployment's shape (typed prose + edge density ~3
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
