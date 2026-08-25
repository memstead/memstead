//! Read-surface headroom: the diagnostic sibling of the divergence campaign.
//!
//! The divergence campaign varied two things between its arms at once, the
//! storage form and the access surface, and pre-declared that it was doing so.
//! This experiment holds the bytes fixed and varies only the surface, over the
//! campaign's own frozen round-10 corpora, its own twelve-query battery, its own
//! blinding and its own judge. Its output is a distance, not a verdict: how much
//! read-side ground is available on the same bytes, and whether typed bytes can
//! outread flat ones once the surface is right.
//!
//! Pre-registration: `docs/proof/read-surface/prereg.md`. Everything the run
//! consumes was committed before the first arm ran; this module is the executable
//! form of that document and must not diverge from it silently.
//!
//! Reads only. No writer sessions exist here, and every arm is served from a
//! fresh copy, so the committed corpora are never a working directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::divergence::{DivergenceJudge, Package, base_session_args, spawn_claude_session_with_env};

/// The five access surfaces. The corpus each reads is fixed by the surface; the
/// only thing that varies across the five is how the reader is allowed to reach
/// it (prereg, "The five arms").
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Surface {
    /// Arm 1: the typed corpus, mounted as a mem, read through the three engine
    /// read tools. Reproduces the campaign's Arm B reader configuration.
    Engine,
    /// Arm 2: the typed corpus as plain files, read with filesystem tools. Takes
    /// the control's substrate block byte-identically; only the directory differs.
    Files,
    /// Arm 3: the typed corpus concatenated into the prompt, no tools at all.
    Dump,
    /// Arm 4: the typed corpus as plain files, read with filesystem tools plus a
    /// shell, on a `PATH` from which the memstead binaries are absent.
    Shell,
    /// Arm 5: the tolerant markdown corpus, read with filesystem tools.
    /// Reproduces the campaign's Arm A reader configuration; the control.
    Control,
}

impl Surface {
    pub const ALL: [Surface; 5] = [
        Surface::Engine,
        Surface::Files,
        Surface::Dump,
        Surface::Shell,
        Surface::Control,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Surface::Engine => "engine",
            Surface::Files => "files",
            Surface::Dump => "dump",
            Surface::Shell => "shell",
            Surface::Control => "control",
        }
    }

    /// The substrate block spliced into the shared reader skeleton. Arms 2, 4 and
    /// 5 read a directory of markdown files and say so in the same words; arm 4
    /// adds exactly one clause for the one added mechanic. Arm 1 quotes the
    /// campaign package. Arm 3 has no tool mechanic to describe, so its block
    /// carries the corpus and is assembled by the caller.
    fn substrate_block(self, pkg: &Package) -> String {
        match self {
            Surface::Engine => pkg.prompts.reader_substrate.arm_b.clone(),
            Surface::Files | Surface::Control => pkg.prompts.reader_substrate.arm_a.clone(),
            Surface::Shell => format!(
                "{} and shell tools.",
                pkg.prompts
                    .reader_substrate
                    .arm_a
                    .trim_end()
                    .trim_end_matches('.')
                    .trim_end_matches(" tools")
            ),
            Surface::Dump => String::new(),
        }
    }
}

/// Where each arm's materialised copy lives, plus the sandbox the tool-free and
/// MCP arms run from. Built once, before the first arm, so all five arms are
/// served from one generation of copies (prereg, control 3).
#[derive(Clone, Debug)]
pub struct Materialised {
    /// Full copy of the typed corpus including `.memstead/`, for the engine arm.
    pub engine_workspace: PathBuf,
    /// Entity `.md` files only, no engine metadata: arms 2, 3 and 4.
    pub typed_files: PathBuf,
    /// The tolerant corpus's entity files: arm 5.
    pub control_files: PathBuf,
    /// Empty directory the engine and dump arms run from, so claude's built-in
    /// file tools find no codebase.
    pub sandbox: PathBuf,
    /// MCP config mounting `engine_workspace`.
    pub mcp_config: PathBuf,
}

/// Copy the frozen corpora into `work`, leaving the originals untouched.
pub fn materialise(arm_a: &Path, arm_b: &Path, work: &Path, mcp_binary: &Path) -> Result<Materialised> {
    let engine_workspace = work.join("engine-workspace");
    let typed_files = work.join("typed-files");
    let control_files = work.join("control-files");
    let sandbox = work.join("sandbox");
    for d in [&engine_workspace, &typed_files, &control_files, &sandbox] {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    copy_tree(arm_b, &engine_workspace)?;
    copy_markdown(arm_b, &typed_files)?;
    copy_markdown(arm_a, &control_files)?;

    // The server must run *inside* the mem, because `memstead-mcp` finds its
    // workspace by walking up from its working directory. A `cwd` key in the MCP
    // config is not honoured, so the working directory is set the way the
    // campaign's own committed config sets it: by wrapping the binary in a shell
    // that cd's first. The smoke run that found this had an engine arm which
    // silently fell back to file tools and scored as if it were a file arm.
    let mcp_config = work.join("engine.mcp.json");
    let cfg = serde_json::json!({
        "mcpServers": {
            "memstead": {
                "command": "sh",
                "args": [
                    "-c",
                    format!(
                        "cd {} && exec {}",
                        shell_quote(&engine_workspace.display().to_string()),
                        shell_quote(&mcp_binary.display().to_string())
                    )
                ]
            }
        }
    });
    std::fs::write(&mcp_config, serde_json::to_vec_pretty(&cfg)?)
        .with_context(|| format!("writing {}", mcp_config.display()))?;

    Ok(Materialised {
        engine_workspace,
        typed_files,
        control_files,
        sandbox,
        mcp_config,
    })
}

/// Single-quote a path for `sh -c`. Paths here come from the run's own
/// directories, but a quoted path is one class of silent failure fewer.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    for entry in walk(from)? {
        let rel = entry.strip_prefix(from).unwrap();
        let dest = to.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&entry, &dest)
            .with_context(|| format!("copying {} to {}", entry.display(), dest.display()))?;
    }
    Ok(())
}

fn copy_markdown(from: &Path, to: &Path) -> Result<()> {
    for entry in walk(from)? {
        if entry.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        // Entity files sit at the corpus root; anything nested is engine
        // metadata and is deliberately not copied.
        if entry.parent() != Some(from) {
            continue;
        }
        let dest = to.join(entry.file_name().unwrap());
        std::fs::copy(&entry, &dest)
            .with_context(|| format!("copying {} to {}", entry.display(), dest.display()))?;
    }
    Ok(())
}

fn walk(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .flatten()
        {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The whole typed corpus as one document, for the dump arm. Same concatenation
/// the campaign's auditor path uses, so the bytes the dump arm sees are the bytes
/// the other typed arms could reach.
pub fn concatenated(dir: &Path) -> Result<String> {
    let (corpus, _count) = super::divergence::read_corpus(dir)?;
    Ok(corpus)
}

/// A `PATH` with no directory that holds a `memstead` binary, so the shell arm
/// cannot quietly become an engine arm (prereg, control 1). The value used is
/// recorded in the result so the claim is auditable after the fact.
pub fn stripped_path() -> String {
    let candidates = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"];
    candidates
        .iter()
        .filter(|d| {
            let d = Path::new(d);
            !d.join("memstead").exists() && !d.join("memstead-mcp").exists()
        })
        .copied()
        .collect::<Vec<_>>()
        .join(":")
}

/// One scored session, kept verbatim. Without this a score of zero is not
/// interpretable: an arm that honestly answered "the knowledge base does not
/// say" and an arm whose answer the blinder shredded both land on 0.0, and only
/// the text tells them apart. The campaign persists its transcripts for the same
/// reason; discarding them here cost one smoke run to learn.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SessionRecord {
    pub query: String,
    pub trial: usize,
    /// What the reader actually said.
    pub answer: String,
    /// What the judge saw, after tell-stripping. A large gap between this and
    /// `answer` means the blinder, not the substrate, produced the score.
    pub blinded: String,
    pub score: f64,
    pub tool_calls: Vec<String>,
}

/// One arm's scored result over the whole battery.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ArmResult {
    pub surface: Surface,
    pub label: String,
    /// Mean over the per-query means.
    pub mean: f64,
    /// Population standard deviation of the per-query means.
    pub stddev: f64,
    /// `stddev / sqrt(n_queries)`.
    pub stderr: f64,
    pub per_query: BTreeMap<String, f64>,
    pub tokens: u64,
    pub non_cache_tokens: u64,
    /// Every session this arm ran, verbatim.
    pub sessions: Vec<SessionRecord>,
}

/// The experiment's output. Both pre-registered measures are computed here so
/// the report cannot be written from a different arithmetic than the run used.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HeadroomResult {
    pub package_hash: String,
    pub model: String,
    pub queries: usize,
    pub trials: usize,
    pub stripped_path: String,
    pub arms: Vec<ArmResult>,
    /// `max(files, dump, shell) - engine`.
    pub headroom: f64,
    /// `max(engine, files, dump, shell) - control`.
    pub substrate_gap: f64,
}

impl HeadroomResult {
    fn arm(&self, s: Surface) -> Option<&ArmResult> {
        self.arms.iter().find(|a| a.surface == s)
    }

    /// Render the two measures and the five means as a fixed table. No band is
    /// applied: this is a diagnostic, and the pre-registration owns the reading.
    pub fn summary(&self) -> String {
        let mut out = String::from("Read-surface headroom\n\n");
        out.push_str(&format!(
            "package {} · model {} · {} queries × {} trials\n\n",
            &self.package_hash[..12.min(self.package_hash.len())],
            self.model,
            self.queries,
            self.trials
        ));
        out.push_str("| arm | mean | stderr |\n|---|---:|---:|\n");
        for a in &self.arms {
            out.push_str(&format!(
                "| {} | {:.3} | {:.3} |\n",
                a.label, a.mean, a.stderr
            ));
        }
        out.push_str(&format!(
            "\nheadroom (best typed surface - engine) = {:+.3}\n\
             substrate gap (best typed surface - control) = {:+.3}\n",
            self.headroom, self.substrate_gap
        ));
        out
    }
}

/// Run the experiment. `queries` may be truncated by the caller for a smoke pass;
/// the number actually run is recorded in the result so a partial pass can never
/// be mistaken for the full one.
#[allow(clippy::too_many_arguments)]
pub fn run(
    pkg: &Package,
    queries: &[super::TaskSpec],
    trials: usize,
    model: &str,
    claude: &str,
    mat: &Materialised,
    judge: &dyn DivergenceJudge,
) -> Result<HeadroomResult> {
    if queries.is_empty() {
        bail!("the query battery is empty — nothing to measure");
    }
    // Fail before spawning dozens of sessions, not during: the engine arm is
    // the only one with a precondition beyond "the files are there", and a
    // missing mem config would fail every one of its sessions identically.
    let cfg = mat.engine_workspace.join(".memstead").join("config.json");
    if !cfg.is_file() {
        bail!(
            "the engine arm's copy at {} carries no `.memstead/config.json` — it would not mount",
            mat.engine_workspace.display()
        );
    }
    let tells = pkg.tell_lists.combined();
    let path = stripped_path();
    if path.is_empty() {
        bail!("no memstead-free directory found for the shell arm's PATH");
    }
    let dump_corpus = concatenated(&mat.typed_files)?;

    let mut arms = Vec::new();
    for surface in Surface::ALL {
        let mut per_query: BTreeMap<String, f64> = BTreeMap::new();
        let mut sessions: Vec<SessionRecord> = Vec::new();
        let mut tokens = 0u64;
        let mut non_cache = 0u64;
        for q in queries {
            let mut scores = Vec::with_capacity(trials);
            for trial in 0..trials {
                let block = if surface == Surface::Dump {
                    format!(
                        "The knowledge base is reproduced in full below.\n\n{dump_corpus}"
                    )
                } else {
                    surface.substrate_block(pkg)
                };
                let prompt = pkg
                    .prompts
                    .reader_skeleton
                    .replace("{SUBSTRATE_BLOCK}", &block)
                    .replace("{QUERY}", &q.prompt);
                let (args, cwd, env) = session_for(surface, model, mat, &path);
                let out = spawn_claude_session_with_env(claude, &args, &prompt, cwd, &env)?;
                tokens += out.tokens;
                non_cache += out.non_cache_tokens;
                let blinded = super::grade::strip_tells_with(&out.text, &tells);
                let scored = judge.score(model, &q.reference, blinded.as_str())?;
                tokens += scored.tokens;
                non_cache += scored.non_cache_tokens;
                scores.push(scored.score);
                if surface == Surface::Engine
                    && !out
                        .tool_calls
                        .iter()
                        .any(|t| t.starts_with("mcp__memstead__"))
                {
                    bail!(
                        "the engine arm answered {} without calling a single memstead tool \
                         (tools used: {:?}) — the MCP server did not attach, so this arm is \
                         not measuring the engine surface",
                        q.id,
                        out.tool_calls
                    );
                }
                sessions.push(SessionRecord {
                    query: q.id.clone(),
                    trial,
                    answer: out.text.clone(),
                    blinded,
                    score: scored.score,
                    tool_calls: out.tool_calls.clone(),
                });
            }
            per_query.insert(q.id.clone(), mean(&scores));
        }
        let means: Vec<f64> = per_query.values().copied().collect();
        let m = mean(&means);
        let sd = pstddev(&means, m);
        arms.push(ArmResult {
            surface,
            label: surface.label().to_string(),
            mean: m,
            stddev: sd,
            stderr: if means.len() > 1 {
                sd / (means.len() as f64).sqrt()
            } else {
                0.0
            },
            per_query,
            tokens,
            non_cache_tokens: non_cache,
            sessions,
        });
    }

    let mut result = HeadroomResult {
        package_hash: pkg.content_hash.clone(),
        model: model.to_string(),
        queries: queries.len(),
        trials,
        stripped_path: path,
        arms,
        headroom: 0.0,
        substrate_gap: 0.0,
    };
    let best_typed_non_engine = [Surface::Files, Surface::Dump, Surface::Shell]
        .iter()
        .filter_map(|s| result.arm(*s).map(|a| a.mean))
        .fold(f64::NEG_INFINITY, f64::max);
    let engine = result.arm(Surface::Engine).map(|a| a.mean).unwrap_or(0.0);
    let control = result.arm(Surface::Control).map(|a| a.mean).unwrap_or(0.0);
    result.headroom = best_typed_non_engine - engine;
    result.substrate_gap = best_typed_non_engine.max(engine) - control;
    Ok(result)
}

/// Argument vector, working directory and environment overrides per surface.
/// This function is the whole treatment: everything else in the run is shared.
fn session_for<'a>(
    surface: Surface,
    model: &str,
    mat: &'a Materialised,
    stripped_path: &str,
) -> (Vec<String>, &'a Path, Vec<(String, String)>) {
    // The campaign ran uncapped (amendment A3); so does this.
    let mut args = base_session_args(model, None);
    args.push("--allowedTools".to_string());
    match surface {
        Surface::Engine => {
            args.push(
                "mcp__memstead__memstead_overview,mcp__memstead__memstead_search,mcp__memstead__memstead_entity"
                    .to_string(),
            );
            args.push("--mcp-config".to_string());
            args.push(mat.mcp_config.display().to_string());
            (args, mat.sandbox.as_path(), Vec::new())
        }
        Surface::Files => {
            args.push("Read,Grep,Glob,LS".to_string());
            (args, mat.typed_files.as_path(), Vec::new())
        }
        Surface::Shell => {
            args.push("Read,Grep,Glob,LS,Bash".to_string());
            (
                args,
                mat.typed_files.as_path(),
                vec![("PATH".to_string(), stripped_path.to_string())],
            )
        }
        Surface::Dump => {
            args.push(String::new());
            (args, mat.sandbox.as_path(), Vec::new())
        }
        Surface::Control => {
            args.push("Read,Grep,Glob,LS".to_string());
            (args, mat.control_files.as_path(), Vec::new())
        }
    }
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

fn pstddev(xs: &[f64], m: f64) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    (xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / xs.len() as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arms_two_and_five_get_the_byte_identical_substrate_block() {
        // The cleanest isolation the experiment offers: the typed-files arm and
        // the control differ in exactly one thing, the directory they are
        // pointed at. If their prompts ever diverge, that isolation is gone and
        // the experiment measures something else.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../docs/proof/divergence/prereg");
        if !dir.join("prompts.json").exists() {
            return; // package not in this checkout
        }
        let pkg = Package::load(&dir).expect("the committed package must load");
        assert_eq!(
            Surface::Files.substrate_block(&pkg),
            Surface::Control.substrate_block(&pkg),
            "arms 2 and 5 must receive byte-identical substrate blocks"
        );
    }

    #[test]
    fn the_shell_arm_path_holds_no_memstead_binary() {
        let path = stripped_path();
        assert!(!path.is_empty(), "a stripped PATH must remain non-empty");
        for dir in path.split(':') {
            assert!(
                !Path::new(dir).join("memstead").exists(),
                "{dir} carries a memstead binary — the shell arm would not be blind to the engine"
            );
        }
    }

    #[test]
    fn copy_markdown_leaves_engine_metadata_behind() {
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(src.join(".memstead").join("state")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("one.md"), "# one").unwrap();
        std::fs::write(src.join(".memstead").join("config.json"), "{}").unwrap();
        std::fs::write(src.join(".memstead").join("nested.md"), "# not an entity").unwrap();
        copy_markdown(&src, &dst).unwrap();
        let copied: Vec<_> = std::fs::read_dir(&dst)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(copied, vec!["one.md".to_string()]);
    }
}
