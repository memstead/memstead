//! `memstead quickstart` — the batteries-included cold start.
//!
//! One run in a fresh (or trivially-dirty) directory leaves: a bootable
//! filesystem-mem workspace pinned to the default schema, one seed
//! entity so the graph is non-empty, and the MCP wiring for the
//! selected agent targets. Output names each artifact plus the single
//! next action.
//!
//! Contract split against `memstead init`: `init` is the deliberate,
//! script-safe verb — exact pins, strict emptiness, no side effects
//! beyond `.memstead/`. `quickstart` is the newcomer verb — it derives
//! the mem name from the directory, tolerates dotfiles and
//! README-grade files, and writes agent config. It composes the same
//! engine primitives (`init_filesystem_mem`, `Engine::create_entity`)
//! rather than forking a second init path; the write-validation
//! strictness downstream of the doorway is untouched.
//!
//! Interactivity ceiling: two prompts, both TTY-only, both with a flag
//! alternative — the agent-target selection (`--agent` bypasses) and
//! the mem name when derivation from the directory fails (`--name`
//! bypasses). Non-interactive runs never block: no `--agent` defaults
//! to Claude Code (and says so), an underivable name refuses with the
//! exact command to run instead.

use std::io::{IsTerminal, Write as _};
use std::path::{Path, PathBuf};

use clap::{Args as ClapArgs, ValueEnum};
use memstead_base::filesystem::config::{
    config_path, init_filesystem_mem, validate_mem_name,
};
use memstead_base::vcs::Actor;
use memstead_base::{CreateEntityArgs, Engine as BaseEngine};
use serde_json::json;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::CliContext;

use super::init::find_ancestor_workspace;

/// `memstead quickstart` arguments.
#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Target folder. Defaults to the current working directory.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Mem name. Normally derived from the directory name; pass this
    /// when the derivation fails (or to override it). Slug-shaped:
    /// `^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$`.
    #[arg(long)]
    pub name: Option<String>,

    /// Agent target(s) to write MCP wiring for. Repeatable. Skips the
    /// interactive selection prompt. Without a TTY and without this
    /// flag, quickstart defaults to `claude-code`.
    #[arg(long = "agent", value_enum)]
    pub agents: Vec<AgentTarget>,
}

/// The supported agent targets and the wiring each one gets. The three
/// file-writing targets take project-scoped MCP config; Codex reads
/// MCP servers only from its global `~/.codex/config.toml`, so its
/// wiring is the exact `codex mcp add` command printed as the next
/// action — quickstart never writes outside the target directory.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentTarget {
    /// Claude Code — project `.mcp.json`.
    ClaudeCode,
    /// OpenAI Codex — prints the `codex mcp add` one-liner (Codex has
    /// no project-scoped MCP config file).
    Codex,
    /// Cursor — project `.cursor/mcp.json`.
    Cursor,
    /// Gemini CLI — project `.gemini/settings.json`.
    Gemini,
}

impl AgentTarget {
    fn label(self) -> &'static str {
        match self {
            AgentTarget::ClaudeCode => "Claude Code",
            AgentTarget::Codex => "Codex",
            AgentTarget::Cursor => "Cursor",
            AgentTarget::Gemini => "Gemini CLI",
        }
    }

    /// Project-relative MCP config file, or `None` for the
    /// print-a-command target (Codex).
    fn config_file(self) -> Option<&'static str> {
        match self {
            AgentTarget::ClaudeCode => Some(".mcp.json"),
            AgentTarget::Cursor => Some(".cursor/mcp.json"),
            AgentTarget::Gemini => Some(".gemini/settings.json"),
            AgentTarget::Codex => None,
        }
    }

    const ALL: [AgentTarget; 4] = [
        AgentTarget::ClaudeCode,
        AgentTarget::Codex,
        AgentTarget::Cursor,
        AgentTarget::Gemini,
    ];
}

/// One wiring outcome per selected target, for the report.
struct WiringOutcome {
    target: AgentTarget,
    /// What happened, as a report line fragment.
    action: String,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let target = args
        .path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    if target.exists() && !target.is_dir() {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "target {} exists but is not a directory — point at a folder: \
                 memstead quickstart my-graph",
                target.display(),
            ),
        )
        .into());
    }
    if !target.exists() {
        std::fs::create_dir_all(&target).map_err(|e| {
            CliError::new(
                ExitKind::Generic,
                crate::INTERNAL_CODE,
                format!("failed to create target directory {}: {e}", target.display()),
            )
        })?;
    }

    // Conflict gate 1: the target itself already carries `.memstead/`.
    check_no_local_memstead(&target)?;

    // Conflict gate 2: never nest inside an existing workspace — same
    // rule and walker as `memstead init`.
    if let Some(found_at) = find_ancestor_workspace(&target)? {
        return Err(CliError::new(
            ExitKind::Validation,
            crate::WORKSPACE_ALREADY_EXISTS_ABOVE_CODE,
            format!(
                "an existing memstead workspace lives above {} at {}; quickstart \
                 refuses to nest workspaces. To add a mem inside the existing \
                 workspace, run: memstead mem init <name>",
                target.display(),
                found_at.display(),
            ),
        )
        .with_details(json!({ "found_at": found_at.display().to_string() }))
        .into());
    }

    // Conflict gate 3: tolerant emptiness. Dotfiles and non-`.md`
    // README-grade files are fine — the folder backend only reads `.md`
    // files, so they can never leak into the graph. Anything else is a
    // genuine conflict named in full; `.md` files especially, because a
    // filesystem mem owns every `.md` file in its folder and quickstart
    // must never silently adopt user content into the graph.
    let blocking = blocking_entries(&target)?;
    if !blocking.is_empty() {
        let md_note = if blocking.iter().any(|f| f.ends_with(".md`")) {
            " (a filesystem mem owns every `.md` file in its folder, so quickstart \
             would silently adopt them into the graph)"
        } else {
            ""
        };
        return Err(CliError::new(
            ExitKind::Validation,
            crate::TARGET_NOT_EMPTY_CODE,
            format!(
                "target {} has content quickstart won't touch: {}{md_note} — move it \
                 out, or start in a fresh folder: mkdir my-graph && cd my-graph && \
                 memstead quickstart",
                target.display(),
                blocking.join(", "),
            ),
        )
        .with_details(json!({
            "path": target.display().to_string(),
            "found": blocking,
        }))
        .into());
    }

    // Mem name: flag > derivation from the directory > TTY prompt >
    // refusal carrying the exact command.
    let name = resolve_mem_name(&target, args.name.as_deref())?;

    // Agent targets: flag > TTY prompt > default (Claude Code, stated).
    let (agents, agents_defaulted) = resolve_agents(&args.agents)?;

    // Schema pin: the current default builtin, resolved by name so the
    // printed pin tracks the catalogue instead of a hardcoded version.
    let schema_pin = default_schema_pin()?;

    // Workspace + config through the same shared initialiser `memstead
    // init` uses — one code path, byte-identical output.
    init_filesystem_mem(&target, &name, &schema_pin).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            crate::INTERNAL_CODE,
            format!("initialise filesystem mem: {e}"),
        )
    })?;

    // Seed entity, through the engine's validated create path.
    let seed_id = seed_entity(&target, &name)?;

    // MCP wiring per selected target.
    let mcp_bin = resolve_mcp_binary();
    let mut wirings = Vec::with_capacity(agents.len());
    for agent in &agents {
        wirings.push(wire_agent(&target, *agent, &mcp_bin.command)?);
    }

    report(
        ctx,
        &target,
        &name,
        &schema_pin,
        &seed_id,
        &wirings,
        agents_defaulted,
        &mcp_bin,
    )
}

/// Refuse when the target already carries `.memstead/` — either a
/// finished workspace (point at the next command, don't re-initialise)
/// or a foreign/partial `.memstead/` directory quickstart must not
/// adopt or overwrite.
fn check_no_local_memstead(target: &Path) -> anyhow::Result<()> {
    let store = target.join(memstead_base::WORKSPACE_STORE_DIR);
    if !store.exists() {
        return Ok(());
    }
    if memstead_base::is_workspace_root(target) {
        return Err(CliError::new(
            ExitKind::Validation,
            "WORKSPACE_ALREADY_INITIALISED",
            format!(
                "{} is already a Memstead workspace — nothing to bootstrap. \
                 Inspect it with: memstead overview",
                target.display(),
            ),
        )
        .with_details(json!({ "path": target.display().to_string() }))
        .into());
    }
    Err(CliError::new(
        ExitKind::Validation,
        "FOREIGN_MEMSTEAD_DIR",
        format!(
            "{} contains a `.memstead/` directory that is not a workspace \
             (no workspace.toml) — quickstart won't adopt or overwrite it. \
             Move it aside, or start fresh: mkdir my-graph && cd my-graph && \
             memstead quickstart",
            target.display(),
        ),
    )
    .with_details(json!({ "path": store.display().to_string() }))
    .into())
}

/// Directory entries that block quickstart. Tolerated: dotfiles
/// (`.git`, `.gitignore`, `.mcp.json`, editor config, …) and non-`.md`
/// README-grade files (README, LICENSE.txt, …). Every `.md` file blocks
/// — including `README.md` — because the folder backend treats each
/// `.md` in the mem folder as an entity, and silently adopting user
/// content into the graph is the one thing quickstart must never do.
/// `.memstead` is handled earlier by [`check_no_local_memstead`].
fn blocking_entries(target: &Path) -> anyhow::Result<Vec<String>> {
    let read_err = |e: std::io::Error| {
        CliError::new(
            ExitKind::Generic,
            crate::INTERNAL_CODE,
            format!("read target {}: {e}", target.display()),
        )
    };
    let mut blocking = Vec::new();
    for entry in std::fs::read_dir(target).map_err(read_err)? {
        let entry = entry.map_err(read_err)?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let lower = name.to_lowercase();
        let readme_grade = lower.starts_with("readme")
            || lower.starts_with("license")
            || lower.starts_with("licence");
        if readme_grade && !lower.ends_with(".md") {
            continue;
        }
        blocking.push(format!("`{name}`"));
    }
    blocking.sort();
    Ok(blocking)
}

/// Resolve the mem name: `--name` wins, then slug derivation from the
/// directory basename, then (TTY only) one prompt, else a refusal
/// carrying the exact retry command.
fn resolve_mem_name(target: &Path, flag: Option<&str>) -> anyhow::Result<String> {
    if let Some(name) = flag {
        validate_mem_name(name).map_err(|e| {
            CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!(
                    "invalid --name: {e}. Retry with a slug, e.g.: memstead quickstart \
                     --name {}",
                    derive_mem_name(name).unwrap_or_else(|| "my-graph".to_string()),
                ),
            )
        })?;
        return Ok(name.to_string());
    }
    let basename = std::fs::canonicalize(target)
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_default();
    if let Some(derived) = derive_mem_name(&basename) {
        return Ok(derived);
    }
    if std::io::stdin().is_terminal() {
        let answer = prompt_line(&format!(
            "Could not derive a mem name from `{basename}`. Mem name (lowercase letters, digits, hyphens): ",
        ))?;
        let answer = answer.trim();
        validate_mem_name(answer).map_err(|e| {
            CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!("invalid mem name: {e}. Retry with: memstead quickstart --name my-graph"),
            )
        })?;
        return Ok(answer.to_string());
    }
    Err(CliError::new(
        ExitKind::Validation,
        "INVALID_INPUT",
        format!(
            "could not derive a mem name from directory `{basename}` — \
             pass one explicitly: memstead quickstart --name my-graph",
        ),
    )
    .with_details(json!({ "directory": basename }))
    .into())
}

/// Slug-derive a mem name from a directory basename: lowercase,
/// non-alphanumerics to hyphens, runs collapsed, edges trimmed, capped
/// at the 64-char rule. `None` when nothing valid survives.
fn derive_mem_name(basename: &str) -> Option<String> {
    let mut out = String::with_capacity(basename.len());
    for c in basename.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut slug: String = out.trim_matches('-').chars().take(64).collect();
    slug = slug.trim_matches('-').to_string();
    validate_mem_name(&slug).ok().map(|()| slug)
}

/// Resolve the agent-target list. Returns the targets plus whether the
/// non-interactive Claude Code default was applied (the report states
/// it, so a scripted run knows the choice was made for it).
fn resolve_agents(flag: &[AgentTarget]) -> anyhow::Result<(Vec<AgentTarget>, bool)> {
    if !flag.is_empty() {
        let mut seen = Vec::with_capacity(flag.len());
        for a in flag {
            if !seen.contains(a) {
                seen.push(*a);
            }
        }
        return Ok((seen, false));
    }
    if std::io::stdin().is_terminal() {
        return Ok((prompt_agents()?, false));
    }
    Ok((vec![AgentTarget::ClaudeCode], true))
}

/// The one interactive agent-target prompt. Empty answer means Claude
/// Code; otherwise comma-separated numbers from the printed list.
fn prompt_agents() -> anyhow::Result<Vec<AgentTarget>> {
    let menu: Vec<String> = AgentTarget::ALL
        .iter()
        .enumerate()
        .map(|(i, a)| format!("  {}) {}", i + 1, a.label()))
        .collect();
    let answer = prompt_line(&format!(
        "Which agents should connect to this mem? (comma-separated, Enter = Claude Code)\n{}\n> ",
        menu.join("\n"),
    ))?;
    let answer = answer.trim();
    if answer.is_empty() {
        return Ok(vec![AgentTarget::ClaudeCode]);
    }
    let mut selected = Vec::new();
    for token in answer.split(',') {
        let token = token.trim();
        let picked = match token.parse::<usize>() {
            Ok(n) if (1..=AgentTarget::ALL.len()).contains(&n) => AgentTarget::ALL[n - 1],
            _ => {
                return Err(CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    format!(
                        "unrecognised selection `{token}` — expected numbers 1-{max} \
                         (comma-separated). Skip the prompt with: memstead quickstart \
                         --agent claude-code --agent cursor",
                        max = AgentTarget::ALL.len(),
                    ),
                )
                .into());
            }
        };
        if !selected.contains(&picked) {
            selected.push(picked);
        }
    }
    Ok(selected)
}

/// Print `msg` to stderr (stdout carries the command's report) and read
/// one line from stdin.
fn prompt_line(msg: &str) -> anyhow::Result<String> {
    let mut stderr = std::io::stderr();
    stderr.write_all(msg.as_bytes()).ok();
    stderr.flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            crate::INTERNAL_CODE,
            format!("read answer from stdin: {e}"),
        )
    })?;
    Ok(line)
}

/// Resolve the default builtin schema to its concrete pin.
fn default_schema_pin() -> anyhow::Result<memstead_schema::SchemaRef> {
    let reg = memstead_schema::SchemaRegistry::builtin();
    match reg.resolve_by_name("default") {
        Ok(Some(schema)) => {
            let (name, version) = schema.id();
            Ok(memstead_schema::SchemaRef::new(name, version))
        }
        _ => Err(CliError::new(
            ExitKind::Generic,
            crate::INTERNAL_CODE,
            "builtin schema catalogue has no `default` schema — this binary is broken, please report",
        )
        .into()),
    }
}

/// Create the seed entity through the engine's validated create path,
/// so the very first entity in the graph went through the same gate
/// every later one will.
fn seed_entity(target: &Path, mem: &str) -> anyhow::Result<String> {
    let mut engine = BaseEngine::from_workspace_root(target).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            crate::INTERNAL_CODE,
            format!("boot engine at {}: {e:#}", target.display()),
        )
    })?;
    let mut sections = indexmap::IndexMap::new();
    sections.insert(
        "definition".to_string(),
        "This mem is a typed knowledge graph: markdown entities validated against a schema, \
         connected by typed relationships."
            .to_string(),
    );
    sections.insert(
        "explanation".to_string(),
        "`memstead quickstart` seeded this entity so the graph starts non-empty. Read it back \
         with `memstead entity <id>`, list types with `memstead type`, create your own with \
         `memstead create`, and delete this one any time with `memstead delete <id>`."
            .to_string(),
    );
    let outcome = engine
        .create_entity(
            CreateEntityArgs {
                mem: mem.to_string(),
                title: "Welcome to Memstead".to_string(),
                entity_type: "concept".to_string(),
                sections,
                metadata: indexmap::IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            Actor::Cli,
            None,
            Some("seeded by memstead quickstart"),
        )
        .map_err(CliError::from_engine_op)?;
    Ok(outcome.id.as_ref().to_string())
}

/// The resolved `memstead-mcp` launch command plus a warning when the
/// binary could not be found (the wiring is still written with the
/// bare name so a later install fixes it without re-running).
struct McpBinary {
    command: String,
    warning: Option<String>,
}

/// Resolve the `memstead-mcp` binary: sibling of the running `memstead`
/// binary first (one install ships both), then `PATH`. Falls back to
/// the bare name with a warning naming the install command.
fn resolve_mcp_binary() -> McpBinary {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("memstead-mcp");
            if sibling.is_file() {
                return McpBinary {
                    command: sibling.display().to_string(),
                    warning: None,
                };
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("memstead-mcp");
            if candidate.is_file() {
                return McpBinary {
                    command: candidate.display().to_string(),
                    warning: None,
                };
            }
        }
    }
    McpBinary {
        command: "memstead-mcp".to_string(),
        warning: Some(
            "`memstead-mcp` was not found next to this binary or on PATH — the wiring uses the \
             bare name and will work once it is installed (curl -sSf https://memstead.io/install.sh | sh)"
                .to_string(),
        ),
    }
}

/// Write (or merge into) the target's MCP config for one agent. JSON
/// configs get an `mcpServers.memstead` entry added, preserving every
/// existing key; an existing `memstead` entry is never overwritten.
/// Codex gets the exact `codex mcp add` command as its action line.
fn wire_agent(target: &Path, agent: AgentTarget, mcp_command: &str) -> anyhow::Result<WiringOutcome> {
    let Some(rel) = agent.config_file() else {
        return Ok(WiringOutcome {
            target: agent,
            action: format!("run: `codex mcp add memstead -- {mcp_command}`"),
        });
    };
    let path = target.join(rel);
    let mut root: serde_json::Value = if path.is_file() {
        let bytes = std::fs::read(&path).map_err(|e| {
            CliError::new(
                ExitKind::Generic,
                crate::INTERNAL_CODE,
                format!("read {}: {e}", path.display()),
            )
        })?;
        serde_json::from_slice(&bytes).map_err(|e| {
            CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!(
                    "{} exists but is not valid JSON ({e}) — fix or remove it, then re-run \
                     memstead quickstart --agent {}",
                    path.display(),
                    agent
                        .to_possible_value()
                        .map(|v| v.get_name().to_string())
                        .unwrap_or_default(),
                ),
            )
        })?
    } else {
        json!({})
    };

    let servers = root
        .as_object_mut()
        .ok_or_else(|| {
            CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!(
                    "{} exists but its top level is not a JSON object — fix or remove \
                     it, then re-run memstead quickstart",
                    path.display(),
                ),
            )
        })?
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    let servers = servers.as_object_mut().ok_or_else(|| {
        CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "{}'s `mcpServers` is not a JSON object — fix or remove it first",
                path.display(),
            ),
        )
    })?;

    if servers.contains_key("memstead") {
        return Ok(WiringOutcome {
            target: agent,
            action: format!("`{rel}` already has a `memstead` server entry — left untouched"),
        });
    }
    servers.insert("memstead".to_string(), json!({ "command": mcp_command }));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CliError::new(
                ExitKind::Generic,
                crate::INTERNAL_CODE,
                format!("create {}: {e}", parent.display()),
            )
        })?;
    }
    let rendered = format!("{}\n", serde_json::to_string_pretty(&root).unwrap_or_default());
    std::fs::write(&path, rendered).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            crate::INTERNAL_CODE,
            format!("write {}: {e}", path.display()),
        )
    })?;
    Ok(WiringOutcome {
        target: agent,
        action: format!("wrote `{rel}` (server `memstead`)"),
    })
}

/// Final report: every artifact by name, then the single next action.
#[allow(clippy::too_many_arguments)]
fn report(
    ctx: &CliContext,
    target: &Path,
    name: &str,
    schema_pin: &memstead_schema::SchemaRef,
    seed_id: &str,
    wirings: &[WiringOutcome],
    agents_defaulted: bool,
    mcp_bin: &McpBinary,
) -> anyhow::Result<()> {
    let restart_labels: Vec<&str> = wirings.iter().map(|w| w.target.label()).collect();
    let next_action = format!(
        "Restart {} so the `memstead` MCP server registers — then try: memstead overview",
        restart_labels.join(" / "),
    );

    if ctx.json {
        return print_json(&json!({
            "workspace_root": target.display().to_string(),
            "config_path": config_path(target).display().to_string(),
            "name": name,
            "schema": schema_pin.as_display(),
            "seed_entity": seed_id,
            "mcp_command": mcp_bin.command,
            "agents": wirings
                .iter()
                .map(|w| json!({
                    "target": w.target.to_possible_value().map(|v| v.get_name().to_string()),
                    "action": w.action,
                }))
                .collect::<Vec<_>>(),
            "agents_defaulted": agents_defaulted,
            "next_action": next_action,
            "warnings": mcp_bin.warning.as_ref().map(|w| vec![w.clone()]).unwrap_or_default(),
        }));
    }

    let mut lines = vec![
        format!("# Quickstart complete — mem `{name}`"),
        String::new(),
        format!("- Workspace:   `{}`", target.display()),
        format!("- Schema pin:  `{}`", schema_pin.as_display()),
        format!(
            "- Seed entity: `{seed_id}` (remove any time: `memstead delete {seed_id}`)"
        ),
    ];
    for w in wirings {
        lines.push(format!("- {}: {}", w.target.label(), w.action));
    }
    if agents_defaulted {
        lines.push(
            "- No `--agent` given and no terminal to ask — defaulted to Claude Code \
             (re-run with `--agent` for others)"
                .to_string(),
        );
    }
    if let Some(warning) = &mcp_bin.warning {
        lines.push(String::new());
        lines.push(format!("> warning: {warning}"));
    }
    lines.push(String::new());
    lines.push(format!("Next: {next_action}"));
    print_markdown(&lines.join("\n"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_mem_name_handles_common_directory_names() {
        assert_eq!(derive_mem_name("my-graph").as_deref(), Some("my-graph"));
        assert_eq!(derive_mem_name("My Project").as_deref(), Some("my-project"));
        assert_eq!(derive_mem_name("Notes_2026 (v2)").as_deref(), Some("notes-2026-v2"));
        // Nothing valid survives: prompt/refusal path.
        assert_eq!(derive_mem_name("日本語"), None);
        assert_eq!(derive_mem_name(""), None);
        // Single char fails the two-char slug rule.
        assert_eq!(derive_mem_name("a"), None);
    }

    #[test]
    fn blocking_entries_tolerates_dotfiles_and_readme_grade() {
        let tmp = tempfile::tempdir().unwrap();
        for f in [".gitignore", ".mcp.json", "README", "LICENSE", "Readme.txt"] {
            std::fs::write(tmp.path().join(f), b"x").unwrap();
        }
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        assert!(blocking_entries(tmp.path()).unwrap().is_empty());

        // A `.md` README blocks — the folder backend would adopt it as
        // an entity, and quickstart never ingests user content.
        std::fs::write(tmp.path().join("README.md"), b"# hi").unwrap();
        assert_eq!(blocking_entries(tmp.path()).unwrap(), vec!["`README.md`"]);
        std::fs::remove_file(tmp.path().join("README.md")).unwrap();

        std::fs::write(tmp.path().join("main.rs"), b"fn main() {}").unwrap();
        assert_eq!(blocking_entries(tmp.path()).unwrap(), vec!["`main.rs`"]);
    }

    #[test]
    fn wire_agent_merges_and_never_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        // Fresh write.
        let outcome = wire_agent(tmp.path(), AgentTarget::ClaudeCode, "/bin/memstead-mcp").unwrap();
        assert!(outcome.action.contains("wrote"), "got: {}", outcome.action);
        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(tmp.path().join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(parsed["mcpServers"]["memstead"]["command"], "/bin/memstead-mcp");

        // Existing foreign server entries survive; existing `memstead`
        // entry is never overwritten.
        std::fs::write(
            tmp.path().join(".mcp.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "mcpServers": {
                    "other": { "command": "/bin/other" },
                    "memstead": { "command": "/custom/memstead-mcp" },
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let outcome = wire_agent(tmp.path(), AgentTarget::ClaudeCode, "/bin/memstead-mcp").unwrap();
        assert!(outcome.action.contains("left untouched"), "got: {}", outcome.action);
        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(tmp.path().join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(parsed["mcpServers"]["memstead"]["command"], "/custom/memstead-mcp");
        assert_eq!(parsed["mcpServers"]["other"]["command"], "/bin/other");
    }

    #[test]
    fn wire_agent_codex_prints_command_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let outcome = wire_agent(tmp.path(), AgentTarget::Codex, "/bin/memstead-mcp").unwrap();
        assert!(
            outcome.action.contains("codex mcp add memstead -- /bin/memstead-mcp"),
            "got: {}",
            outcome.action,
        );
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }
}
