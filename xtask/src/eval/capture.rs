//! Capture: turning one source corpus into the two substrates the substrate-quality
//! test compares — a free-form capture (B) and a schema-forced capture (C).
//!
//! This is the write-side variable made concrete. Both arms capture the **same
//! sources** with the **same model**, and both **reason freely first, then write**
//! — the only legitimate difference is the *storage form* the write step targets:
//!
//! - **Free-form (B)** — the agent writes its free extraction as good-faith,
//!   lightly-structured markdown notes. No enforced schema; structure emerges.
//! - **Schema-forced (C)** — the agent maps that same free extraction into the
//!   engine's schema as a second step: typed entities, required fields, typed
//!   relationships. The substrate bytes are then read back from the mem.
//!
//! Three properties keep the capture from rigging the downstream comparison, each
//! enforced here:
//!
//! - **Prompt parity on everything but storage** ([`check_single_capture_variable`])
//!   — the two capture configs share model, the shared free-reasoning instruction,
//!   and the corpus; they differ only in [`CaptureKind`]. A pair that diverges in
//!   the capture model or the reasoning prompt is refused — that is the most common
//!   way the schema arm is handed an unfair head start.
//! - **Sequencing held constant** ([`build_capture_prompt`]) — both prompts carry
//!   the *identical* free-reasoning instruction, then a storage-specific write
//!   instruction. C never reasons *directly into* rigid schema output (a known
//!   generation penalty); it reasons free, then maps. Baking reasoning-into-schema
//!   into only one arm would confound a sequencing effect with the storage effect
//!   under test.
//! - **Same sources** — the corpus block is byte-identical across arms; the guard
//!   refuses a pair that captured different sources.
//!
//! The produced [`Substrate`]s flow straight into [`super::substrate`]'s in-context
//! comparison: capture here, answer + judge there.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

use super::claude::{ClaudeRunner, parse_stream_json};
use super::replay::mcp_config_json;
use super::substrate::Substrate;

/// The storage form a capture targets — the single variable the substrate test
/// isolates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptureKind {
    /// Good-faith, lightly-structured markdown notes. No enforced schema.
    FreeForm,
    /// Typed entities with required fields and typed relationships, via the engine.
    SchemaForced,
}

impl CaptureKind {
    pub fn label(self) -> &'static str {
        match self {
            CaptureKind::FreeForm => "free-form",
            CaptureKind::SchemaForced => "schema-forced",
        }
    }
}

/// The configuration of one capture arm. Two arms over the same corpus are
/// identical in every field except `kind` — that one field *is* the storage
/// variable, and the write instruction it selects is the only legitimate prompt
/// difference between the arms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureConfig {
    pub kind: CaptureKind,
    pub model: String,
    /// The free-reasoning instruction, byte-identical across arms. The write step
    /// is appended from [`CaptureKind`]; this text must not differ.
    pub reasoning_instruction: String,
    /// The source bytes both arms capture. Byte-identical across arms — both
    /// capture the *same sources*.
    pub corpus: String,
}

/// Build the matched capture-config pair for a corpus. By construction the two
/// share model, reasoning instruction, and corpus; they differ only in `kind`.
/// Returns `(schema_forced, free_form)`.
pub fn build_capture_configs(
    model: &str,
    reasoning_instruction: &str,
    corpus: &str,
) -> (CaptureConfig, CaptureConfig) {
    let cfg = |kind| CaptureConfig {
        kind,
        model: model.to_string(),
        reasoning_instruction: reasoning_instruction.to_string(),
        corpus: corpus.to_string(),
    };
    (cfg(CaptureKind::SchemaForced), cfg(CaptureKind::FreeForm))
}

/// Refuse the capture if the two arms differ in anything but the storage form.
///
/// `kind` is *expected* to differ — it is the variable, and it selects the write
/// instruction. Any divergence in the capture model, the free-reasoning
/// instruction, or the corpus is a confound: handing the schema arm a stronger
/// model or a richer reasoning prompt, or letting it capture different sources, is
/// exactly how this test gets rigged in the schema's favour. The error names every
/// offending field.
pub fn check_single_capture_variable(c: &CaptureConfig, b: &CaptureConfig) -> Result<()> {
    let mut confounds = Vec::new();
    if c.model != b.model {
        confounds.push(format!("capture model ({:?} vs {:?})", c.model, b.model));
    }
    if c.reasoning_instruction != b.reasoning_instruction {
        confounds.push("reasoning_instruction".to_string());
    }
    if c.corpus != b.corpus {
        confounds.push("corpus (the two arms captured different sources)".to_string());
    }
    if c.kind == b.kind {
        confounds.push(format!(
            "kind (both {} — the arms are not distinct storage forms)",
            c.kind.label()
        ));
    }
    if !confounds.is_empty() {
        bail!(
            "refusing to capture: the two capture arms differ in more than the storage form — {}. \
             The only permitted difference is enforced-schema-vs-not (the CaptureKind); model, \
             reasoning instruction, and corpus must be identical, or the resulting C-B delta is \
             unattributable.",
            confounds.join(", ")
        );
    }
    Ok(())
}

/// A sensible default free-reasoning step. The CLI passes this into both arms so
/// they share it verbatim; tests pass their own. Whatever the value, holding it
/// *identical across arms* is what keeps the comparison a *storage* test rather
/// than a *sequencing* test — enforced by [`check_single_capture_variable`].
pub const DEFAULT_REASONING_STEP: &str = "Read the source material below. First, work through it \
freely and extract every concrete fact, entity, and relationship you can find. Reason in your own \
words — do not yet impose any fixed structure or format on what you extract.";

/// The write step for each storage form, appended after the (identical) reasoning
/// step and corpus.
fn write_step(kind: CaptureKind) -> &'static str {
    match kind {
        CaptureKind::FreeForm => {
            "Now write up everything you extracted as clear markdown notes: short headings, one \
             idea per paragraph, links between related ideas where natural. Let the structure \
             emerge from the material — do not invent a rigid schema. Output only the notes."
        }
        CaptureKind::SchemaForced => {
            "Now record everything you extracted into the knowledge graph using the available \
             tools. Create one typed entity per subject, fill in every required field, and declare \
             the typed relationships between them. Map your free extraction onto the schema as this \
             second step — do not discard a fact that is awkward to type; record it in the closest \
             fitting field or note."
        }
    }
}

/// Compose the full capture prompt: the (config-supplied) reasoning step, the
/// corpus, then the storage-specific write step.
///
/// The reasoning step and the corpus are byte-identical for both kinds — they come
/// from the same [`CaptureConfig`] fields the guard pins equal — and only the
/// trailing write step differs. That difference *is* the storage variable.
pub fn build_capture_prompt(config: &CaptureConfig) -> String {
    format!(
        "{}\n\n# Source material\n\n{}\n\n# Write step\n\n{}",
        config.reasoning_instruction,
        config.corpus,
        write_step(config.kind)
    )
}

/// Read the schema-forced substrate back out of a mem's entity directory.
///
/// After a schema-forced capture the bytes that go into context are the entity
/// markdown the engine wrote — read (never mutated) directly off disk, which is a
/// read path and so does not touch engine invariants. Files are concatenated in a
/// deterministic order (sorted by path) so the substrate is stable across runs.
pub fn read_back_substrate(entity_dir: &std::path::Path, label: &str) -> Result<Substrate> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_markdown(entity_dir, &mut files)
        .with_context(|| format!("reading captured entities under {}", entity_dir.display()))?;
    files.sort();
    if files.is_empty() {
        bail!(
            "schema-forced capture produced no entity markdown under {} — the capture did not \
             write to the mem",
            entity_dir.display()
        );
    }
    let mut content = String::new();
    for path in &files {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading entity {}", path.display()))?;
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str(&text);
    }
    Ok(Substrate::new(label, content))
}

/// Recursively collect `*.md` files under `dir` (skips `.git` and other dotdirs).
fn collect_markdown(dir: &std::path::Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_markdown(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
    Ok(())
}

/// Build the `claude -p` argument vector for a capture run.
///
/// Pure and unit-tested. The free-form arm runs with **no** tools (it writes its
/// notes as the answer text); the schema-forced arm mounts the engine MCP and is
/// allowed only `mcp__memstead__*`, so it records typed entities into the mem.
/// Everything before the kind-specific tail — model, permission mode, the prompt —
/// is identical, so the storage form is the lone variable here too.
pub fn build_capture_args(config: &CaptureConfig, mcp_config: Option<&std::path::Path>) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        build_capture_prompt(config),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--model".to_string(),
        config.model.clone(),
        "--permission-mode".to_string(),
        "dontAsk".to_string(),
        "--strict-mcp-config".to_string(),
    ];
    match (config.kind, mcp_config) {
        (CaptureKind::SchemaForced, Some(cfg)) => {
            args.push("--mcp-config".to_string());
            args.push(cfg.display().to_string());
            args.push("--allowedTools".to_string());
            args.push("mcp__memstead__*".to_string());
        }
        // Free-form: no engine, write notes as the answer. (A schema-forced kind
        // with no mcp-config is a misconfiguration the runner rejects before here.)
        _ => {
            args.push("--allowedTools".to_string());
            args.push(String::new());
        }
    }
    args
}

/// Produces a [`Substrate`] from a [`CaptureConfig`]. The real impl shells to
/// `claude -p`; tests use a stub.
pub trait CaptureRunner {
    fn capture(&self, config: &CaptureConfig) -> Result<Substrate>;
}

/// Build the `memstead init` argument vector that bootstraps a fresh, empty
/// folder-backend mem at `workspace` — the destination the schema-forced capture
/// writes into.
///
/// Pure and unit-tested. A folder-backend mem stores each entity as a `.md` file
/// in the workspace root, so after capture the substrate is read straight back off
/// disk with no export step — which is why the destination is a folder mem, not a
/// git-branch one.
pub fn build_init_args(workspace: &std::path::Path, name: &str, schema: &str) -> Vec<String> {
    vec![
        "init".to_string(),
        workspace.display().to_string(),
        "--name".to_string(),
        name.to_string(),
        "--schema".to_string(),
        schema.to_string(),
        "--quiet".to_string(),
    ]
}

/// Provision a fresh empty destination mem for schema-forced capture and return
/// its `(mcp_config, entity_dir)` — the two paths [`ClaudeCapture`] needs.
///
/// This is what turns a real capture run into a single self-contained command: the
/// harness clears `workspace`, shells `memstead init` (the sanctioned CLI route —
/// the engine owns mem state, so provisioning goes through it, never a raw file
/// write), and writes an mcp-config pointing `memstead-mcp` at the fresh mem. The
/// entity dir is the workspace root, where the folder backend lands each `.md`.
/// Clearing first keeps re-runs reproducible.
pub fn provision_capture_mem(
    cli_binary: &std::path::Path,
    mcp_binary: &std::path::Path,
    workspace: &std::path::Path,
    name: &str,
    schema: &str,
) -> Result<(PathBuf, PathBuf)> {
    if workspace.exists() {
        std::fs::remove_dir_all(workspace)
            .with_context(|| format!("clearing stale capture workspace {}", workspace.display()))?;
    }
    std::fs::create_dir_all(workspace)
        .with_context(|| format!("creating capture workspace {}", workspace.display()))?;
    let status = Command::new(cli_binary)
        .args(build_init_args(workspace, name, schema))
        .status()
        .with_context(|| format!("spawning `{}` to init capture mem", cli_binary.display()))?;
    if !status.success() {
        bail!(
            "`memstead init` failed provisioning the capture mem at {} (exit {})",
            workspace.display(),
            status
        );
    }
    // The mcp-config launches the server with `cd <mem> && exec <mcp-binary>`, so
    // a relative binary path would resolve against the mem dir and fail to start.
    // Canonicalise it to an absolute path so a relative `--mcp-binary` works.
    let mcp_abs = std::fs::canonicalize(mcp_binary)
        .with_context(|| format!("resolving mcp binary {}", mcp_binary.display()))?;
    let cfg_path = workspace.join("capture.mcp.json");
    std::fs::write(&cfg_path, mcp_config_json(workspace, &mcp_abs))
        .with_context(|| format!("writing capture mcp-config {}", cfg_path.display()))?;
    Ok((cfg_path, workspace.to_path_buf()))
}

/// Capture both substrates from one corpus, refusing first if the arms differ in
/// anything but the storage form.
///
/// Returns `(schema_forced, free_form)` — the C and B substrates ready for the
/// in-context comparison.
pub fn capture_pair<R: CaptureRunner>(
    runner: &R,
    model: &str,
    reasoning_instruction: &str,
    corpus: &str,
) -> Result<(Substrate, Substrate)> {
    let (c_cfg, b_cfg) = build_capture_configs(model, reasoning_instruction, corpus);
    check_single_capture_variable(&c_cfg, &b_cfg)?;
    let c = runner.capture(&c_cfg)?;
    let b = runner.capture(&b_cfg)?;
    Ok((c, b))
}

/// The real capture runner: shells to `claude -p` for the free-reason-then-write
/// step.
///
/// Free-form capture takes the agent's answer text as the substrate. Schema-forced
/// capture mounts the engine at `schema_mcp_config`, lets the agent record typed
/// entities, then reads the written markdown back from `schema_entity_dir`.
pub struct ClaudeCapture {
    pub runner: ClaudeRunner,
    /// MCP config mounting the (empty) destination mem for schema-forced capture.
    pub schema_mcp_config: Option<PathBuf>,
    /// The mem's entity directory, read back as the schema-forced substrate.
    pub schema_entity_dir: Option<PathBuf>,
}

impl CaptureRunner for ClaudeCapture {
    fn capture(&self, config: &CaptureConfig) -> Result<Substrate> {
        std::fs::create_dir_all(&self.runner.sandbox_dir).with_context(|| {
            format!("creating capture sandbox {}", self.runner.sandbox_dir.display())
        })?;
        let mcp_config = if config.kind == CaptureKind::SchemaForced {
            let cfg = self.schema_mcp_config.as_ref().context(
                "schema-forced capture needs an mcp-config mounting the destination mem",
            )?;
            Some(cfg.as_path())
        } else {
            None
        };
        let args = build_capture_args(config, mcp_config);
        let output = Command::new(&self.runner.executable)
            .args(&args)
            .current_dir(&self.runner.sandbox_dir)
            .env("MCP_TIMEOUT", "60000")
            .output()
            .with_context(|| format!("spawning capture `{}`", self.runner.executable))?;
        if !output.status.success() {
            bail!(
                "capture claude exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        match config.kind {
            CaptureKind::FreeForm => {
                let answer = parse_stream_json(&String::from_utf8_lossy(&output.stdout))?;
                Ok(Substrate::new(config.kind.label(), answer.text))
            }
            CaptureKind::SchemaForced => {
                let dir = self.schema_entity_dir.as_ref().context(
                    "schema-forced capture needs the mem entity dir to read the substrate back",
                )?;
                read_back_substrate(dir, config.kind.label())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REASONING: &str = "extract the facts freely first";
    const CORPUS: &str = "Widget X depends on Gadget Y. Y was added in v2.";

    #[test]
    fn build_configs_differ_only_in_kind() {
        let (c, b) = build_capture_configs("claude-x", REASONING, CORPUS);
        assert_eq!(c.kind, CaptureKind::SchemaForced);
        assert_eq!(b.kind, CaptureKind::FreeForm);
        assert_eq!(c.model, b.model);
        assert_eq!(c.reasoning_instruction, b.reasoning_instruction);
        assert_eq!(c.corpus, b.corpus);
        check_single_capture_variable(&c, &b).unwrap();
    }

    #[test]
    fn confound_different_capture_model_is_refused() {
        // The named negative test: a second difference (capture model) is refused.
        let (mut c, b) = build_capture_configs("claude-x", REASONING, CORPUS);
        c.model = "claude-y".into();
        let err = check_single_capture_variable(&c, &b).unwrap_err().to_string();
        assert!(err.contains("capture model"), "{err}");
    }

    #[test]
    fn confound_different_reasoning_prompt_is_refused() {
        // The named negative test: a second difference (capture prompt) is refused.
        let (c, mut b) = build_capture_configs("claude-x", REASONING, CORPUS);
        b.reasoning_instruction = "a different, richer reasoning prompt".into();
        let err = check_single_capture_variable(&c, &b).unwrap_err().to_string();
        assert!(err.contains("reasoning_instruction"), "{err}");
    }

    #[test]
    fn confound_different_corpus_is_refused() {
        let (c, mut b) = build_capture_configs("claude-x", REASONING, CORPUS);
        b.corpus = "an entirely different source".into();
        let err = check_single_capture_variable(&c, &b).unwrap_err().to_string();
        assert!(err.contains("corpus"), "{err}");
    }

    #[test]
    fn non_distinct_kinds_are_refused() {
        let (c, _) = build_capture_configs("claude-x", REASONING, CORPUS);
        let err = check_single_capture_variable(&c, &c).unwrap_err().to_string();
        assert!(err.contains("kind"), "{err}");
    }

    #[test]
    fn both_prompts_carry_the_identical_reasoning_step_and_corpus() {
        // Sequencing held constant: the free-reasoning instruction and the corpus
        // are byte-identical; only the write step differs.
        let (c, b) = build_capture_configs("m", REASONING, CORPUS);
        let cp = build_capture_prompt(&c);
        let bp = build_capture_prompt(&b);
        // Shared reasoning step (the config's, used verbatim) present in both.
        assert!(cp.contains(REASONING) && bp.contains(REASONING));
        // Same corpus in both.
        assert!(cp.contains(CORPUS) && bp.contains(CORPUS));
        // The common prefix (reasoning + corpus) is identical up to the write step.
        let split = "# Write step";
        let c_head = cp.split(split).next().unwrap();
        let b_head = bp.split(split).next().unwrap();
        assert_eq!(c_head, b_head, "the pre-write-step prompt must be identical");
        // The write steps differ — that is the storage variable.
        assert_ne!(cp, bp);
        assert!(cp.contains("typed entity") || cp.contains("typed relationships"));
        assert!(bp.contains("markdown notes"));
    }

    #[test]
    fn schema_forced_args_mount_engine_free_form_does_not() {
        let (c, b) = build_capture_configs("m", REASONING, CORPUS);
        let c_args = build_capture_args(&c, Some(std::path::Path::new("/tmp/dest.json")));
        assert!(c_args.windows(2).any(|w| w[0] == "--mcp-config" && w[1] == "/tmp/dest.json"));
        assert!(c_args.windows(2).any(|w| w[0] == "--allowedTools" && w[1] == "mcp__memstead__*"));
        let b_args = build_capture_args(&b, None);
        assert!(!b_args.iter().any(|a| a == "--mcp-config"), "{b_args:?}");
        let idx = b_args.iter().position(|a| a == "--allowedTools").unwrap();
        assert_eq!(b_args[idx + 1], "");
    }

    #[test]
    fn read_back_concatenates_entity_markdown_deterministically() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("b.md"), "second entity").unwrap();
        std::fs::write(dir.path().join("a.md"), "first entity").unwrap();
        std::fs::write(dir.path().join("sub/c.md"), "nested entity").unwrap();
        // A dotdir is ignored (it would be mem-repo .git, never substrate).
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref: x").unwrap();
        // A non-markdown file is ignored.
        std::fs::write(dir.path().join("notes.txt"), "not markdown").unwrap();
        let s = read_back_substrate(dir.path(), "schema-forced").unwrap();
        assert_eq!(s.label, "schema-forced");
        // Sorted by path: a.md, b.md, sub/c.md.
        let pos_a = s.content.find("first entity").unwrap();
        let pos_b = s.content.find("second entity").unwrap();
        let pos_c = s.content.find("nested entity").unwrap();
        assert!(pos_a < pos_b && pos_b < pos_c, "not in path order: {}", s.content);
        assert!(!s.content.contains("ref: x"), "leaked .git");
        assert!(!s.content.contains("not markdown"), "leaked non-md");
    }

    #[test]
    fn read_back_empty_dir_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_back_substrate(dir.path(), "schema-forced").is_err());
    }

    /// A stub capture runner: echoes the kind so the orchestration can be exercised
    /// without `claude`.
    struct StubCapture;
    impl CaptureRunner for StubCapture {
        fn capture(&self, config: &CaptureConfig) -> Result<Substrate> {
            Ok(Substrate::new(
                config.kind.label(),
                format!("captured {} from corpus", config.kind.label()),
            ))
        }
    }

    #[test]
    fn capture_pair_returns_distinct_substrates() {
        let (c, b) = capture_pair(&StubCapture, "m", REASONING, CORPUS).unwrap();
        assert_eq!(c.label, "schema-forced");
        assert_eq!(b.label, "free-form");
        assert_ne!(c.content, b.content);
    }

    #[test]
    fn init_args_bootstrap_a_named_folder_mem() {
        let args = build_init_args(std::path::Path::new("/tmp/cap-ws"), "corpus", "default@1.0.0");
        assert_eq!(args[0], "init");
        assert!(args.iter().any(|a| a == "/tmp/cap-ws"));
        assert!(args.windows(2).any(|w| w[0] == "--name" && w[1] == "corpus"));
        assert!(args.windows(2).any(|w| w[0] == "--schema" && w[1] == "default@1.0.0"));
    }
}
