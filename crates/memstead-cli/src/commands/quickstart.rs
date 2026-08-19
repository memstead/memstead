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
use memstead_base::binding::ScaffoldParams;
use memstead_base::filesystem::config::{config_path, init_filesystem_mem_at, validate_mem_name};
use memstead_base::pipeline_store::write_binding;
use memstead_base::vcs::Actor;
use memstead_base::{CreateEntityArgs, Engine as BaseEngine};
use serde_json::json;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, memstead_program, shell_quote};

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

    /// Point at an existing repository: bootstrap the workspace *and*
    /// scaffold a codebase binding over that tree, then print what the
    /// starter mem does and does not contain. `--repo .` in the repo
    /// you already have is the whole guided path.
    ///
    /// Without a `PATH` argument the repository *is* the workspace:
    /// `.memstead/` and the mem's own folder land inside it, so the
    /// binding points at `.` and every artifact id is repo-relative.
    /// With a `PATH` argument the workspace is bootstrapped there as
    /// usual and the binding points back at the repository — a
    /// supported layout whose one caveat quickstart prints.
    ///
    /// Nothing is ingested: the binding is the standing obligation,
    /// the ingest loop is what fills the mem.
    #[arg(long = "repo", value_name = "PATH")]
    pub repo: Option<PathBuf>,
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

/// What happened to one agent's wiring. Held as data rather than as a
/// rendered sentence because the sentence names a FILE, and a file has to
/// be named in the frame of whoever is reading — the human receipt speaks
/// from the reader's directory, the JSON speaks workspace-relative.
enum WiringAction {
    /// The config file was written (or the entry added to it).
    Wrote,
    /// A `memstead` entry was already there and was left alone.
    LeftUntouched,
    /// No project config exists for this target; this command IS the
    /// wiring. Carries no path, so it renders the same in either frame.
    RunCommand(String),
}

impl WiringAction {
    /// The report line fragment, with any path rendered by `path`.
    fn render(&self, target: AgentTarget, path: &dyn Fn(&str) -> String) -> String {
        match self {
            WiringAction::Wrote => match target.config_file() {
                Some(rel) => format!("wrote `{}` (server `memstead`)", path(rel)),
                None => "wrote its config".to_string(),
            },
            WiringAction::LeftUntouched => match target.config_file() {
                Some(rel) => format!(
                    "`{}` already has a `memstead` server entry — left untouched",
                    path(rel)
                ),
                None => "already wired — left untouched".to_string(),
            },
            WiringAction::RunCommand(cmd) => format!("run: `{cmd}`"),
        }
    }
}

/// One wiring outcome per selected target, for the report.
struct WiringOutcome {
    target: AgentTarget,
    /// What happened. Rendered per surface — see [`WiringAction`].
    action: WiringAction,
    /// `Some(_)` when a pre-existing `memstead` server entry was left
    /// untouched: the entry's `command` value as found in the file
    /// (`None` inside the option is impossible — a non-string command
    /// yields `Some(None)`-like absence via the outer `None`). The
    /// receipt uses this so its verify line checks what the file
    /// actually wires, never what quickstart would have written.
    existing_command: Option<String>,
    /// True when the wiring was skipped because an entry already
    /// existed — regardless of whether its command could be read.
    preexisting: bool,
    /// True when the config FILE was already on disk before this run.
    /// Distinct from [`Self::preexisting`], which is about the `memstead`
    /// server ENTRY: a file that existed and gained an entry was modified,
    /// not created, and a receipt that calls it new is wrong about the
    /// reader's tree.
    file_existed: bool,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    // The repository must already exist — quickstart creates workspaces,
    // never repositories, and a typo'd `--repo` that silently produced an
    // empty tree would scaffold a binding over nothing.
    if let Some(repo) = &args.repo
        && !repo.is_dir()
    {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "--repo {} is not an existing directory — point it at the repository \
                 you already have: memstead quickstart --repo .",
                repo.display(),
            ),
        )
        .with_details(json!({ "repo": repo.display().to_string() }))
        .into());
    }

    let target = args
        .path
        .clone()
        .or_else(|| args.repo.clone())
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
    let target_created = !target.exists();
    if target_created {
        // Typed, not INTERNAL: an unwritable or missing parent is an
        // environment condition the caller can act on (fix permissions,
        // pick another target). The path rides `details` so an agent
        // recovers without parsing prose.
        std::fs::create_dir_all(&target).map_err(|e| {
            CliError::new(
                ExitKind::Generic,
                "INTERNAL_IO_ERROR",
                format!(
                    "failed to create target directory {}: {e}",
                    target.display()
                ),
            )
            .with_details(serde_json::json!({ "path": target.display().to_string() }))
        })?;
    }

    // Layout. Without `--repo` the workspace root is the target and the
    // mem folder collapses onto it — today's shape exactly. With `--repo`,
    // the mem takes a folder of its own whenever the workspace and the
    // repository OVERLAP: `--repo .` with no target path (the flagship
    // case), a workspace nested in the repo (`quickstart ./graph --repo .`),
    // and a workspace at the common parent of several repos alike.
    //
    // The overlap test is the rule, not a convenience, and it earns its
    // keep at both ends. Workspace inside the source tree: the mem's own
    // entity files would be inside the binding's scope, and the one
    // mechanism that keeps them out — the engine's unconditional
    // exclusion of every mount's storage location — is skipped for a
    // mount that IS the workspace root (excluding `**` there would empty
    // every denominator); a folder of its own puts the mem back under
    // that exclusion. Repository inside the workspace — the common-parent
    // layout the out-of-root warning itself recommends: the
    // tolerant-emptiness gate would refuse that parent for containing the
    // repo, and a folder of its own moves the gate onto the mem's folder,
    // so the recipe the engine prints is reachable from the front door
    // that prints it.
    let mem_in_subfolder = args
        .repo
        .as_deref()
        .is_some_and(|repo| workspace_overlaps_repo(&target, repo));

    // Conflict gate 1: the target itself already carries `.memstead/`.
    check_no_local_memstead(&target)?;

    // Conflict gate 2: never nest inside an existing workspace — same
    // rule and walker as `memstead init`. The alternatives named here
    // must be viable in the workspaces quickstart itself creates
    // (filesystem-shaped, no mem-lifecycle allowlist), so the message
    // points at working in the existing workspace or starting a
    // separate one — never at `memstead mem init`, which refuses on
    // both counts there.
    if let Some(found_at) = find_ancestor_workspace(&target)? {
        return Err(CliError::new(
            ExitKind::Validation,
            crate::WORKSPACE_ALREADY_EXISTS_ABOVE_CODE,
            format!(
                "{} is already inside the memstead workspace at {} — quickstart \
                 refuses to nest workspaces. Work in that workspace (memstead \
                 overview), or start a separate graph outside it: mkdir my-graph && \
                 cd my-graph && memstead quickstart",
                target.display(),
                found_at.display(),
            ),
        )
        .with_details(json!({ "found_at": found_at.display().to_string() }))
        .into());
    }

    // Mem name: flag > derivation from the directory > TTY prompt >
    // refusal carrying the exact command. Resolved before gate 3 because
    // the guided layout names the mem's folder after it.
    let name = resolve_mem_name(&target, args.name.as_deref(), args.repo.as_deref())?;

    // The mem's folder — the workspace root itself in the collapsed
    // shape, a subdirectory named after the mem in the guided in-repo one.
    let mem_dir = if mem_in_subfolder {
        target.join(&name)
    } else {
        target.clone()
    };

    // Conflict gate 3: tolerant emptiness — of the folder the mem will
    // own, which is the only folder whose `.md` files the graph would
    // adopt. In the guided in-repo layout that is the fresh subdirectory,
    // so the repository's own files never reach this gate; they are not
    // the mem's folder, and nothing adopts them.
    if mem_in_subfolder {
        guard_guided_mem_folder(&target, &mem_dir, &name)?;
    }
    let blocking = blocking_entries(&mem_dir)?;
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
                mem_dir.display(),
                blocking.join(", "),
            ),
        )
        .with_details(json!({
            "path": mem_dir.display().to_string(),
            "found": blocking,
        }))
        .into());
    }

    // Agent targets: flag > TTY prompt > default (Claude Code, stated).
    let (agents, agents_defaulted) = resolve_agents(&args.agents)?;

    // Preflight every selected agent's existing config file BEFORE any
    // write lands: a malformed `.mcp.json` must refuse while "re-run
    // memstead quickstart" is still true — discovering it after the
    // workspace exists would leave a half-bootstrapped directory and a
    // printed retry command that can no longer succeed.
    for agent in &agents {
        if let Some(rel) = agent.config_file() {
            read_agent_config(&target.join(rel))?;
        }
    }

    // Schema pin: the current default builtin, resolved by name so the
    // printed pin tracks the catalogue instead of a hardcoded version.
    let schema_pin = default_schema_pin()?;

    // Everything the guided mode needs to say and write is derived
    // BEFORE the first write: a pointer or stem that cannot be formed
    // must refuse while "re-run memstead quickstart" is still true.
    let guided_plan = match &args.repo {
        Some(repo) => Some(GuidedPlan::derive(&target, repo, &name)?),
        None => None,
    };

    // Workspace + config through the same shared initialiser `memstead
    // init` uses — one code path, byte-identical output.
    init_filesystem_mem_at(&target, &mem_dir, &name, &schema_pin).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "INTERNAL_IO_ERROR",
            format!("initialise filesystem mem: {e}"),
        )
    })?;

    // Seed entity, through the engine's validated create path.
    let seed_id = seed_entity(&target, &name)?;

    // The binding: the standing source→mem obligation the ingest loop
    // runs against. Scaffolded through the engine's own scaffold, so a
    // guided binding is the same record `memstead projection init`
    // writes — no quickstart-private shape.
    let guided = match guided_plan {
        Some(plan) => Some(plan.write(&target)?),
        None => None,
    };

    // MCP wiring per selected target.
    let mcp_bin = resolve_mcp_binary();
    let mut wirings = Vec::with_capacity(agents.len());
    for agent in &agents {
        wirings.push(wire_agent(&target, *agent, &mcp_bin.command)?);
    }

    report(
        ctx,
        &target,
        &mem_dir,
        &name,
        &schema_pin,
        &seed_id,
        &wirings,
        agents_defaulted,
        &mcp_bin,
        guided.as_ref(),
        target_created,
    )
}

/// Whether the workspace root and the repository overlap — either one
/// containing the other, the equal case included. Both sides are
/// canonicalized where possible so a symlinked parent (`/tmp` on macOS)
/// does not read as disjoint; a path that cannot be canonicalized falls
/// back to its own form, which claims no overlap.
fn workspace_overlaps_repo(target: &Path, repo: &Path) -> bool {
    let t = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    let r = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    t.starts_with(&r) || r.starts_with(&t)
}

/// The workspace root expressed relative to the repository, or `None`
/// when the workspace is not inside it (`Some("")` for the equal case).
///
/// This is the frame the receipt owes a reader asking "what appeared in
/// my repository?": every artifact path quickstart knows is relative to
/// the WORKSPACE, and the two frames coincide only when the two roots do.
fn workspace_within_repo(target: &Path, repo: &Path) -> Option<String> {
    let t = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    let r = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let rel = t.strip_prefix(&r).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// Refuse a guided in-repo layout whose mem folder would collide with
/// something already in the repository. The mem folder is a directory
/// quickstart creates and the graph then owns; adopting a folder the
/// repository already uses is the same defect as adopting its `.md`
/// files, so this refuses and names the flag that resolves it.
fn guard_guided_mem_folder(repo: &Path, mem_dir: &Path, name: &str) -> anyhow::Result<()> {
    if !mem_dir.exists() {
        return Ok(());
    }
    let retry = ShellCmd::new(memstead_program())
        .arg("quickstart")
        .arg("--repo")
        .arg(repo.display().to_string())
        .arg("--name")
        .arg(format!("{name}-mem"))
        .render();
    if !mem_dir.is_dir() {
        return Err(CliError::new(
            ExitKind::Validation,
            crate::TARGET_NOT_EMPTY_CODE,
            format!(
                "the mem would take the folder {}, and that path already exists as a file \
                 — name the mem something else: {retry}",
                mem_dir.display(),
            ),
        )
        .with_details(json!({ "path": mem_dir.display().to_string() }))
        .into());
    }
    let occupied = std::fs::read_dir(mem_dir)
        .map(|entries| entries.count() > 0)
        .unwrap_or(true);
    if occupied {
        return Err(CliError::new(
            ExitKind::Validation,
            crate::TARGET_NOT_EMPTY_CODE,
            format!(
                "the mem would take the folder {}, and that folder already exists and is \
                 not empty — quickstart won't adopt a folder the repository already uses. \
                 Name the mem something else: {retry}",
                mem_dir.display(),
            ),
        )
        .with_details(json!({ "path": mem_dir.display().to_string() }))
        .into());
    }
    Ok(())
}

/// The guided mode's binding, resolved before any write and written
/// after the workspace exists. Split in two so a pointer or stem that
/// cannot be formed refuses while the retry command is still true.
struct GuidedPlan {
    /// The medium pointer, workspace-relative (`.` for the in-repo layout).
    pointer: String,
    /// The `<stem>` half of the binding id.
    stem: String,
    /// The repository as the reader sees it, for the receipt.
    repo_display: String,
    /// The layout caveat, when the repository sits outside the workspace.
    layout_warning: Option<String>,
    /// The workspace root relative to the repository, when it is inside
    /// it at all (`Some("")` when they are the same directory). `None`
    /// means this run wrote nothing into the repository.
    workspace_in_repo: Option<String>,
    /// Whether the source tree is actually a git repository.
    is_git_repo: bool,
    mem: String,
}

/// What the receipt reports about the scaffolded binding.
struct GuidedOutcome {
    binding_id: String,
    pointer: String,
    repo_display: String,
    /// Workspace-relative path of the written record.
    record: String,
    /// The deny globs the record actually carries — printed from the
    /// record, never from the constant, so the brief cannot drift from
    /// what was written.
    deny_paths: Vec<String>,
    /// Which operations the record declares.
    operations: Vec<String>,
    /// Scaffold + layout warnings, in that order.
    warnings: Vec<String>,
    /// The workspace root relative to the repository — see
    /// [`GuidedPlan::workspace_in_repo`].
    workspace_in_repo: Option<String>,
    /// Whether the source tree is actually a git repository. `--repo`
    /// accepts any directory and the binding works either way, but the
    /// brief must not describe history a plain folder does not have.
    is_git_repo: bool,
}

impl GuidedPlan {
    fn derive(workspace_root: &Path, repo: &Path, mem: &str) -> anyhow::Result<Self> {
        let workspace_abs = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        let repo_abs = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
        let pointer = if repo_abs == workspace_abs {
            ".".to_string()
        } else {
            let rel = memstead_base::ingest::cursor::relative_to(&workspace_abs, &repo_abs);
            if rel.as_os_str().is_empty() {
                ".".to_string()
            } else {
                rel.to_string_lossy().replace('\\', "/")
            }
        };
        // The stem is a file-path component and half the binding id, so
        // it gets the same slug treatment as the mem name; a repository
        // basename that slugs to nothing falls back to the mem name,
        // which already passed the slug rule.
        let stem = repo_abs
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .and_then(|n| derive_mem_name(&n))
            .unwrap_or_else(|| mem.to_string());
        let layout_warning = memstead_base::ingest::cursor::out_of_root_layout_warning(
            &pointer,
            &workspace_abs,
            memstead_base::MediumType::Codebase,
        );
        Ok(GuidedPlan {
            pointer,
            stem,
            repo_display: repo_abs.display().to_string(),
            layout_warning,
            workspace_in_repo: workspace_within_repo(workspace_root, repo),
            is_git_repo: repo_abs.join(".git").exists(),
            mem: mem.to_string(),
        })
    }

    fn write(self, workspace_root: &Path) -> anyhow::Result<GuidedOutcome> {
        let GuidedPlan {
            pointer,
            stem,
            repo_display,
            layout_warning,
            workspace_in_repo,
            is_git_repo,
            mem,
        } = self;
        let scaffolded = memstead_base::binding::scaffold_binding(ScaffoldParams {
            destination_mem: &mem,
            source_name: &stem,
            pointer: &pointer,
            medium_type: memstead_base::MediumType::Codebase,
            intent: Some(format!(
                "Model the `{stem}` codebase in the `{mem}` mem: what each part is for, \
                 how the parts fit together, and the decisions behind them."
            )),
            additional_deny_paths: Vec::new(),
        });
        write_binding(workspace_root, &mem, &stem, &scaffolded.binding).map_err(|e| {
            CliError::new(
                ExitKind::Generic,
                "PROJECTION_INIT_FAILED",
                format!("could not scaffold binding `{mem}/{stem}`: {e}"),
            )
            .with_details(json!({ "binding": format!("{mem}/{stem}"), "error": e.to_string() }))
        })?;
        let mut warnings: Vec<String> = scaffolded.warnings;
        warnings.extend(layout_warning);
        Ok(GuidedOutcome {
            binding_id: format!("{mem}/{stem}"),
            pointer,
            repo_display,
            record: format!(".memstead/projections/{mem}/{stem}.json"),
            deny_paths: scaffolded.binding.deny_paths.clone(),
            operations: scaffolded
                .operations
                .iter()
                .map(|o| (*o).to_string())
                .collect(),
            warnings,
            workspace_in_repo,
            is_git_repo,
        })
    }
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
    // A folder that does not exist yet blocks nothing — the guided
    // layout's mem folder is created by the initialiser.
    if !target.exists() {
        return Ok(Vec::new());
    }
    let read_err = |e: std::io::Error| {
        CliError::new(
            ExitKind::Generic,
            "INTERNAL_IO_ERROR",
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
fn resolve_mem_name(
    target: &Path,
    flag: Option<&str>,
    repo: Option<&Path>,
) -> anyhow::Result<String> {
    // A refusal's retry command must reproduce the invocation that hit it:
    // a guided run retried without `--repo` lands on the tolerant-emptiness
    // gate instead of succeeding. The guided form is built, never formatted
    // — a repository path can contain anything a shell would eat. The plain
    // form stays the literal it has always been: it takes no path argument,
    // and its wording is pinned by test as part of the unchanged plain path.
    let retry = |name: &str| match repo {
        Some(repo) => ShellCmd::new(memstead_program())
            .arg("quickstart")
            .arg("--repo")
            .arg(repo.display().to_string())
            .arg("--name")
            .arg(name)
            .render(),
        None => format!("memstead quickstart --name {name}"),
    };
    if let Some(name) = flag {
        validate_mem_name(name).map_err(|e| {
            CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!(
                    "invalid --name: {e}. Retry with a slug, e.g.: {}",
                    retry(&derive_mem_name(name).unwrap_or_else(|| "my-graph".to_string())),
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
                format!("invalid mem name: {e}. Retry with: {}", retry("my-graph")),
            )
        })?;
        return Ok(answer.to_string());
    }
    Err(CliError::new(
        ExitKind::Validation,
        "INVALID_INPUT",
        format!(
            "could not derive a mem name from directory `{basename}` — \
             pass one explicitly: {}",
            retry("my-graph"),
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
            "INTERNAL_IO_ERROR",
            format!("read answer from stdin: {e}"),
        )
    })?;
    Ok(line)
}

/// Resolve the default builtin schema to its concrete pin — the
/// current generation (1.3.0, the required-opt-in metadata-polarity
/// generation), so fresh workspaces never start on a superseded
/// vocabulary.
fn default_schema_pin() -> anyhow::Result<memstead_schema::SchemaRef> {
    let reg = memstead_schema::SchemaRegistry::builtin();
    match reg.get("default", &semver::Version::new(1, 3, 0)) {
        Some(schema) => {
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
                anchors: Vec::new(),
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
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("memstead-mcp");
        if sibling.is_file() {
            return McpBinary {
                command: sibling.display().to_string(),
                warning: None,
            };
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

/// Read and shape-check an agent's existing MCP config file: must be
/// valid JSON, a top-level object, with `mcpServers` absent or an
/// object. A missing file is an empty object. Called once as a
/// preflight before any write lands (so the refusal's "re-run
/// memstead quickstart" stays true) and again by [`wire_agent`].
fn read_agent_config(path: &Path) -> anyhow::Result<serde_json::Value> {
    if !path.is_file() {
        return Ok(json!({}));
    }
    let fix_hint = "fix or remove the file, then re-run: memstead quickstart";
    let bytes = std::fs::read(path).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "INTERNAL_IO_ERROR",
            format!("read {}: {e}", path.display()),
        )
    })?;
    let root: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "{} exists but is not valid JSON ({e}) — {fix_hint}",
                path.display()
            ),
        )
    })?;
    if !root.is_object() {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "{} exists but its top level is not a JSON object — {fix_hint}",
                path.display(),
            ),
        )
        .into());
    }
    let servers = &root["mcpServers"];
    if !servers.is_null() && !servers.is_object() {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "{}'s `mcpServers` is not a JSON object — {fix_hint}",
                path.display(),
            ),
        )
        .into());
    }
    Ok(root)
}

/// Write (or merge into) the target's MCP config for one agent. JSON
/// configs get an `mcpServers.memstead` entry added, preserving every
/// existing key; an existing `memstead` entry is never overwritten.
/// Codex gets the exact `codex mcp add` command as its action line.
fn wire_agent(
    target: &Path,
    agent: AgentTarget,
    mcp_command: &str,
) -> anyhow::Result<WiringOutcome> {
    let Some(rel) = agent.config_file() else {
        // Codex has no project config, so this command IS the wiring —
        // it must survive an mcp path containing a space exactly as the
        // verification commands must.
        let add = ShellCmd::new("codex")
            .arg("mcp")
            .arg("add")
            .arg("memstead")
            .end_of_options()
            .arg(mcp_command)
            .render();
        return Ok(WiringOutcome {
            target: agent,
            action: WiringAction::RunCommand(add),
            existing_command: None,
            preexisting: false,
            file_existed: false,
        });
    };
    let path = target.join(rel);
    let file_existed = path.exists();
    let mut root = read_agent_config(&path)?;

    let servers = root
        .as_object_mut()
        .expect("read_agent_config only returns JSON objects")
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    let servers = servers.as_object_mut().ok_or_else(|| {
        CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "{}'s `mcpServers` is not a JSON object — fix or remove the file, then \
                 re-run: memstead quickstart",
                path.display(),
            ),
        )
    })?;

    if let Some(existing) = servers.get("memstead") {
        // Capture what the file actually wires so the receipt can
        // verify it (or state honestly that it could not).
        let existing_command = existing
            .get("command")
            .and_then(|c| c.as_str())
            .map(str::to_string);
        return Ok(WiringOutcome {
            target: agent,
            action: WiringAction::LeftUntouched,
            existing_command,
            preexisting: true,
            file_existed,
        });
    }
    servers.insert("memstead".to_string(), json!({ "command": mcp_command }));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CliError::new(
                ExitKind::Generic,
                "INTERNAL_IO_ERROR",
                format!("create {}: {e}", parent.display()),
            )
        })?;
    }
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(&root).unwrap_or_default()
    );
    std::fs::write(&path, rendered).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "INTERNAL_IO_ERROR",
            format!("write {}: {e}", path.display()),
        )
    })?;
    Ok(WiringOutcome {
        target: agent,
        action: WiringAction::Wrote,
        existing_command: None,
        preexisting: false,
        file_existed,
    })
}

/// One command line the receipt prints for the reader to run.
///
/// Every printed command goes through this rather than through an ad-hoc
/// `format!`, because each one needs the same three things and each was
/// independently getting one of them wrong: the program resolved to
/// something the reader can actually invoke, every argument shell-quoted,
/// and a `cd` when the command must run inside the new workspace.
///
/// The `cd` uses the `--` terminator so a directory named `-graph`
/// reaches `cd` as an operand instead of an option.
/// One word of a command line: a value to be quoted, or shell syntax
/// to emit as-is.
enum Word {
    Value(String),
    Literal(&'static str),
}

struct ShellCmd {
    /// `cd` here first. `None` runs wherever the reader is standing.
    cd: Option<String>,
    program: String,
    args: Vec<Word>,
}

impl ShellCmd {
    fn new(program: impl Into<String>) -> Self {
        ShellCmd {
            cd: None,
            program: program.into(),
            args: Vec::new(),
        }
    }

    fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(Word::Value(arg.into()));
        self
    }

    /// The literal `--` end-of-options separator. Distinct from
    /// [`Self::arg`] because it is syntax, not a value: quoting it
    /// would be harmless to the shell but noise to the reader, and the
    /// leading-dash rule that protects values must not fire on it.
    fn end_of_options(mut self) -> Self {
        self.args.push(Word::Literal("--"));
        self
    }

    /// Prefix a `cd` into `dir` unless the reader is already there.
    fn in_dir(mut self, dir: &Path, already_there: bool) -> Self {
        if !already_there {
            self.cd = Some(dir.display().to_string());
        }
        self
    }

    /// The runnable line. This is what both receipts print — the
    /// markdown surface only adds its own bullet and backticks.
    fn render(&self) -> String {
        let mut out = String::new();
        if let Some(dir) = &self.cd {
            out.push_str(&format!("cd -- {} && ", shell_quote(dir)));
        }
        out.push_str(&shell_quote(&self.program));
        for arg in &self.args {
            out.push(' ');
            match arg {
                Word::Value(v) => out.push_str(&shell_quote(v)),
                Word::Literal(l) => out.push_str(l),
            }
        }
        out
    }
}

/// Final report: every artifact by name, then the single next action.
#[allow(clippy::too_many_arguments)]
fn report(
    ctx: &CliContext,
    target: &Path,
    mem_dir: &Path,
    name: &str,
    schema_pin: &memstead_schema::SchemaRef,
    seed_id: &str,
    wirings: &[WiringOutcome],
    agents_defaulted: bool,
    mcp_bin: &McpBinary,
    guided: Option<&GuidedOutcome>,
    // Whether this run created the workspace directory itself — the
    // difference between "one new directory appeared" and "files appeared
    // inside a directory you already had".
    target_created: bool,
) -> anyhow::Result<()> {
    let restart_labels: Vec<&str> = wirings.iter().map(|w| w.target.label()).collect();

    // Every command this receipt prints must run verbatim, from the
    // directory the caller is actually standing in, with whatever
    // characters their paths happen to contain. Each printed command is
    // therefore built as a [`ShellCmd`] rather than formatted inline —
    // three separate rounds of this receipt shipped a command that did
    // not run, each time because one `format!` had been missed.
    //
    // A verification step the reader cannot reproduce is the same
    // defect as an undisclosed shape, so this is not cosmetic.
    let absolute = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    let in_cwd = std::env::current_dir()
        .ok()
        .is_some_and(|cwd| cwd == absolute);
    let memstead = memstead_program();
    let overview_cmd = ShellCmd::new(&memstead)
        .arg("overview")
        .in_dir(target, in_cwd)
        .render();
    let delete_cmd = ShellCmd::new(&memstead)
        .arg("delete")
        .arg(seed_id)
        .in_dir(target, in_cwd)
        .render();
    let version_cmd = ShellCmd::new(&mcp_bin.command).arg("--version").render();
    // The one command that starts the ingest loop the brief points at.
    let brief_cmd = guided.map(|g| {
        ShellCmd::new(&memstead)
            .arg("projection")
            .arg("brief")
            .arg(&g.binding_id)
            .in_dir(target, in_cwd)
            .render()
    });
    // Where the mem's own files actually are, resolved for the JSON
    // surface the same way `absolute` resolves the workspace root.
    let mem_absolute = mem_dir.canonicalize().unwrap_or_else(|_| {
        if mem_dir == target {
            absolute.clone()
        } else {
            mem_dir.to_path_buf()
        }
    });
    // A path the reader is told to open, rendered from where the receipt
    // leaves them standing. Every command beside it carries a `cd` when
    // the workspace is not the cwd; a bare workspace-relative path in that
    // company is the same defect in prose form — it does not resolve from
    // the directory the reader is actually in.
    //
    // Resolve, then re-express: joining the workspace-relative form onto
    // the root and canonicalizing collapses `..` instead of printing
    // `./ws/..`, which is the only way an upward pointer comes out as
    // something the reader can act on. Falls back to the absolute path
    // when the target is not under the cwd — still resolvable, never a
    // form that resolves somewhere else.
    let cwd_canon = std::env::current_dir()
        .ok()
        .map(|c| c.canonicalize().unwrap_or(c));
    let from_here = |workspace_relative: &str| -> String {
        let joined = absolute.join(workspace_relative);
        let resolved = joined.canonicalize().unwrap_or(joined);
        let absolute_form = resolved.display().to_string();
        let Some(cwd) = &cwd_canon else {
            return absolute_form;
        };
        let rel = memstead_base::ingest::cursor::relative_to(cwd, &resolved)
            .to_string_lossy()
            .replace('\\', "/");
        if rel.is_empty() {
            return ".".to_string();
        }
        // An upward chain is fine — the `cd` beside it uses the same
        // shape — but past the point where it is longer than the plain
        // absolute path it stops helping anyone read it.
        if rel.len() <= absolute_form.len() {
            rel
        } else {
            absolute_form
        }
    };
    // The mem's own folder in the reader's frame, for the prose that
    // tells them where their entities are. `mem_folder_rel` below stays
    // workspace-relative: it is the machine field, and an agent reading
    // `--json` already has `workspace_root` to resolve it against.
    // Everything a human is told to open goes through `from_here`.
    let mem_folder_here: Option<String> = match mem_dir
        .strip_prefix(target)
        .ok()
        .map(|r| r.to_string_lossy().to_string())
        .filter(|r| !r.is_empty())
    {
        Some(rel) => Some(from_here(&rel)),
        // The mem collapsed onto the workspace root. "In this folder" is
        // true only if the reader is standing in it — in the guided
        // disjoint layout they are not, so name the folder instead. The
        // plain path keeps the inherited wording: its receipt is pinned
        // observably equivalent, and this branch never fires there.
        None if guided.is_some() && !in_cwd => Some(from_here(".")),
        None => None,
    };
    // The mem's own folder, workspace-relative, when it is not the root.
    let mem_folder_rel: Option<String> = mem_dir
        .strip_prefix(target)
        .ok()
        .map(|r| r.to_string_lossy().to_string())
        .filter(|r| !r.is_empty());

    // Codex is wired by a command the reader still has to run, so for
    // that target the restart registers nothing until they run it. Say
    // so in order rather than naming a restart that would no-op.
    let codex_pending = wirings
        .iter()
        .any(|w| w.target == AgentTarget::Codex && matches!(w.action, WiringAction::RunCommand(_)));
    let restart_clause = format!(
        "Restart {} so the `memstead` MCP server registers its tools",
        restart_labels.join(" / "),
    );
    let next_action = if codex_pending {
        format!(
            "Run the `codex mcp add` command above first — it is Codex's wiring, and a restart \
             registers nothing without it. Then: {restart_clause} — then try: {overview_cmd}"
        )
    } else {
        format!("{restart_clause} — then try: {overview_cmd}")
    };
    // …but an agent session that just ran onboarding cannot restart
    // itself mid-run, so the wiring it wrote must be checkable from
    // inside that session. Held as `{what, command}` pairs so the JSON
    // surface ships runnable commands and the markdown surface adds its
    // own bullet decoration — an agent should never have to strip
    // backticks off a machine field.
    let mut verify_now: Vec<(&str, String)> = Vec::new();
    // Only claim the binary answers when we actually found one. In the
    // not-found case the warning above already names the install
    // command, and printing an unrunnable check under the heading "no
    // restart needed" would be the exact defect this block exists to
    // remove.
    // Verify only what is actually in the wiring files. A pre-existing
    // `memstead` entry was left untouched, so checking the binary
    // quickstart *would* have wired asserts nothing about that file —
    // seeded with a broken entry, the old check passed while the wiring
    // was broken. Fresh wirings (and the codex instruction) still
    // verify the resolved binary; preserved entries verify their own
    // command, or are stated as left-as-is when no plain command exists.
    let fresh_wiring = wirings.iter().any(|w| !w.preexisting);
    if fresh_wiring && mcp_bin.warning.is_none() {
        verify_now.push(("the wired binary answers", version_cmd));
    }
    let mut seen_existing: Vec<String> = Vec::new();
    for w in wirings.iter().filter(|w| w.preexisting) {
        match &w.existing_command {
            Some(cmd) if !seen_existing.contains(cmd) => {
                seen_existing.push(cmd.clone());
                verify_now.push((
                    "the pre-existing `memstead` entry's binary answers",
                    ShellCmd::new(cmd).arg("--version").render(),
                ));
            }
            _ => {}
        }
    }
    verify_now.push(("the graph is already readable", overview_cmd.clone()));
    if let Some(cmd) = &brief_cmd {
        verify_now.push(("the binding renders its ingest brief", cmd.clone()));
    }

    // The honest brief: what the starter mem holds now, what it does not,
    // and what turns the second into the first. Every line is derived from
    // what was just written — the deny list comes off the record, the
    // commands off the builder — because a brief that is composed rather
    // than derived is exactly how printed claims drift from behaviour.
    // Built once per FRAME, not once: the brief names paths, and the
    // human receipt speaks from the reader's directory while the JSON
    // speaks workspace-relative. One rendering serving both is how a
    // payload ends up carrying a path that resolves in neither.
    let build_brief = |path: &dyn Fn(&str) -> String| -> Vec<String> {
        match (guided, &brief_cmd) {
            (Some(g), Some(brief)) => {
                let mut b = vec![
                    "## What this mem holds".to_string(),
                    String::new(),
                    format!(
                        "- Now: one seed entity (`{seed_id}`). Nothing else — scaffolding a \
                     binding reads no source file and creates no entity from one."
                    ),
                    format!(
                        "- Not yet: anything from `{}`. Its {} are the binding's subject, \
                     not its content.",
                        g.repo_display,
                        if g.is_git_repo {
                            "code, docs and history"
                        } else {
                            "files"
                        },
                    ),
                    format!(
                        "- Growth: the ingest loop against binding `{}` — one batch at a \
                     time, each entity written through the same validated path as the \
                     seed. Start with: `{brief}`",
                        g.binding_id,
                    ),
                    format!(
                        "- Scope: everything under `{}`, minus what the record denies ({}) \
                     and minus {}, which the engine excludes unconditionally. The deny \
                     list is yours to edit: `{}`",
                        path(&g.pointer),
                        g.deny_paths
                            .iter()
                            .map(|d| format!("`{d}`"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        // Say what this layout actually excludes. The mem's
                        // folder is only inside the scope — and only excluded
                        // by the mount rule — when it is a folder of its own;
                        // in the collapsed layout the workspace sits outside
                        // the source tree, so naming it here would claim an
                        // exclusion that never had to fire.
                        match &mem_folder_rel {
                            Some(rel) => {
                                format!("engine state and the mem's own folder `{}/`", path(rel))
                            }
                            None => "engine state (`.memstead/`)".to_string(),
                        },
                        path(&g.record),
                    ),
                    format!(
                        "- Operations the binding declares: {}",
                        g.operations.join(", ")
                    ),
                ];
                // What appeared in the reader's repository — the one claim
                // they can check with `git status` in ten seconds, so it is
                // stated in THEIR frame, not the workspace's. Every artifact
                // path quickstart holds is workspace-relative; the two frames
                // coincide only when the workspace is the repository, so the
                // paths are re-expressed against the repo root and the line
                // is omitted entirely when the workspace is somewhere else.
                if let Some(ws_rel) = &g.workspace_in_repo {
                    let in_repo = |p: &str| {
                        if ws_rel.is_empty() {
                            format!("`{p}`")
                        } else {
                            format!("`{ws_rel}/{p}`")
                        }
                    };
                    let mut written = Vec::new();
                    if target_created && !ws_rel.is_empty() {
                        // The whole workspace directory is what `git status`
                        // will show — one untracked path, not three inside it.
                        written.push(format!(
                            "`{ws_rel}/` (the workspace: its state, the binding record, \
                         the mem, and the agent wiring — plus the engine's cache, \
                         which appears inside it once the binding is first measured)"
                        ));
                    } else {
                        written.push(format!(
                            "{} (workspace state and the binding record; a sibling \
                         `.memstead.cache/` appears once the binding is first measured)",
                            in_repo(".memstead/")
                        ));
                        if let Some(rel) = &mem_folder_rel {
                            written.push(format!("{} (the mem)", in_repo(&format!("{rel}/"))));
                        }
                        // A config file that already existed was MODIFIED, not
                        // added — `preexisting` tracks the `memstead` server
                        // entry, which is a different fact from the file.
                        for w in wirings.iter().filter(|w| !w.preexisting) {
                            if let Some(f) = w.target.config_file() {
                                let verb = if w.file_existed {
                                    "agent wiring added to it"
                                } else {
                                    "agent wiring"
                                };
                                written.push(format!("{} ({verb})", in_repo(f)));
                            }
                        }
                    }
                    b.push(format!(
                        "- Written into your {}: {}. Nothing else in the tree was touched.",
                        if g.is_git_repo {
                            "repository"
                        } else {
                            "source directory"
                        },
                        written.join(", "),
                    ));
                }
                b
            }
            _ => Vec::new(),
        }
    };
    // The reader's frame for the printed receipt; the workspace frame for
    // the machine surface, where `workspace_root` is the resolution base.
    let brief_lines = build_brief(&from_here);
    let brief_lines_machine = build_brief(&|rel: &str| rel.to_string());

    if ctx.json {
        let mut payload = json!({
            // Absolute, so a caller that passed a relative argument can
            // use these without reconstructing its own cwd.
            "workspace_root": absolute.display().to_string(),
            // The MEM's config, which lives under the MEM's folder — the
            // same directory as its entities. Only in the collapsed shape
            // is that also the workspace root, so this is derived from the
            // mem folder, never from the root.
            "config_path": config_path(&mem_absolute).display().to_string(),
            "seed_entity_delete_command": delete_cmd,
            "name": name,
            "schema": schema_pin.as_display(),
            "seed_entity": seed_id,
            "mcp_command": mcp_bin.command,
            "agents": wirings
                .iter()
                .map(|w| json!({
                    "target": w.target.to_possible_value().map(|v| v.get_name().to_string()),
                    "action": w.action.render(w.target, &|rel: &str| rel.to_string()),
                }))
                .collect::<Vec<_>>(),
            "agents_defaulted": agents_defaulted,
            "workspace_shape": crate::setup::WorkspaceShape::Filesystem.label(),
            // The agent surface gets the whole disclosure, not just the
            // label: which shape, what it cannot do, the command for
            // the other one — the same three parts the markdown block
            // carries, from the same value.
            "workspace_shape_disclosure":
                crate::setup::shape_disclosure_in(
                    crate::setup::WorkspaceShape::Filesystem,
                    mem_folder_rel.as_deref(),
                ).to_json(),
            "next_action": next_action,
            "verify_now": verify_now
                .iter()
                .map(|(what, command)| json!({ "what": what, "command": command }))
                .collect::<Vec<_>>(),
            "warnings": mcp_bin.warning.as_ref().map(|w| vec![w.clone()]).unwrap_or_default(),
        });
        // Guided-mode fields are additive and present only in guided mode:
        // a plain quickstart's JSON is the same document it has always been.
        if let Some(g) = guided {
            payload["mem_folder"] =
                json!(mem_folder_rel.clone().unwrap_or_else(|| ".".to_string()));
            payload["repo"] = json!(g.repo_display);
            payload["binding"] = json!({
                "id": g.binding_id,
                "pointer": g.pointer,
                "record": g.record,
                "deny_paths": g.deny_paths,
                "operations": g.operations,
            });
            payload["brief"] = json!(
                brief_lines_machine
                    .iter()
                    .filter(|l| l.starts_with("- "))
                    .map(|l| l.trim_start_matches("- ").to_string())
                    .collect::<Vec<_>>()
            );
            if !g.warnings.is_empty() {
                let mut w: Vec<String> = payload["warnings"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                w.extend(g.warnings.iter().cloned());
                payload["warnings"] = json!(w);
            }
        }
        return print_json(&payload);
    }

    let mut lines = vec![
        format!("# Quickstart complete — mem `{name}`"),
        String::new(),
        // The guided path is normally invoked as `--repo .`, and a
        // receipt that answers "where is it?" with "." tells the reader
        // nothing they did not type; resolve it for that case only.
        format!(
            "- Workspace:   `{}`",
            if guided.is_some() {
                absolute.display().to_string()
            } else {
                target.display().to_string()
            }
        ),
    ];
    if let Some(rel) = &mem_folder_rel {
        lines.push(format!(
            "- Mem folder:  `{}/` (the graph owns this folder and nothing else)",
            from_here(rel),
        ));
    }
    lines.push(format!("- Schema pin:  `{}`", schema_pin.as_display()));
    lines.push(format!(
        "- Seed entity: `{seed_id}` (remove any time: `{delete_cmd}`)"
    ));
    if let Some(g) = guided {
        lines.push(format!(
            "- Binding:     `{}` over `{}` (record: `{}`)",
            g.binding_id,
            from_here(&g.pointer),
            from_here(&g.record),
        ));
    }
    for w in wirings {
        lines.push(format!(
            "- {}: {}",
            w.target.label(),
            w.action.render(w.target, &from_here),
        ));
    }
    if agents_defaulted {
        lines.push(
            "- No `--agent` given and no terminal to ask — defaulted to Claude Code \
             (re-run with `--agent` for others)"
                .to_string(),
        );
    }
    let mut warnings: Vec<String> = mcp_bin.warning.iter().cloned().collect();
    warnings.extend(guided.iter().flat_map(|g| g.warnings.iter().cloned()));
    if !warnings.is_empty() {
        lines.push(String::new());
        for warning in &warnings {
            lines.push(format!("> warning: {warning}"));
        }
    }
    if !brief_lines.is_empty() {
        lines.push(String::new());
        lines.extend(brief_lines.iter().cloned());
    }
    // The shape disclosure sits between the artifact list and the next
    // action: quickstart picked one of two workspace shapes just now,
    // and this receipt is the only output the newcomer is guaranteed
    // to read before they hit the first mem-repo-only refusal.
    lines.push(String::new());
    lines.extend(crate::setup::shape_disclosure_lines_in(
        crate::setup::WorkspaceShape::Filesystem,
        mem_folder_here.as_deref(),
    ));
    lines.push(String::new());
    lines.push(format!("Next: {next_action}"));
    lines.push(String::new());
    lines.push("Verify from this session, no restart needed:".to_string());
    lines.extend(
        verify_now
            .iter()
            .map(|(what, command)| format!("- {what}: `{command}`")),
    );
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
        assert_eq!(
            derive_mem_name("Notes_2026 (v2)").as_deref(),
            Some("notes-2026-v2")
        );
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
        let rendered = outcome
            .action
            .render(outcome.target, &|rel: &str| rel.to_string());
        assert!(rendered.contains("wrote"), "got: {rendered}");
        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(tmp.path().join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(
            parsed["mcpServers"]["memstead"]["command"],
            "/bin/memstead-mcp"
        );

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
        let rendered = outcome
            .action
            .render(outcome.target, &|rel: &str| rel.to_string());
        assert!(rendered.contains("left untouched"), "got: {rendered}");
        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(tmp.path().join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(
            parsed["mcpServers"]["memstead"]["command"],
            "/custom/memstead-mcp"
        );
        assert_eq!(parsed["mcpServers"]["other"]["command"], "/bin/other");
    }

    #[test]
    fn shell_quote_leaves_ordinary_paths_alone_and_quotes_the_rest() {
        assert_eq!(
            shell_quote("/usr/local/bin/memstead-mcp"),
            "/usr/local/bin/memstead-mcp"
        );
        assert_eq!(shell_quote("my-graph"), "my-graph");
        // The case that motivated this: a directory name with a space.
        assert_eq!(shell_quote("My Graph"), "'My Graph'");
        assert_eq!(
            shell_quote("/Users/a b/bin/memstead-mcp"),
            "'/Users/a b/bin/memstead-mcp'"
        );
        // Shell metacharacters are contained, not executed.
        assert_eq!(shell_quote("a;rm -rf /"), "'a;rm -rf /'");
        assert_eq!(shell_quote("$(whoami)"), "'$(whoami)'");
        // An embedded single quote closes, escapes, and reopens.
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn wire_agent_codex_prints_command_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let outcome = wire_agent(tmp.path(), AgentTarget::Codex, "/bin/memstead-mcp").unwrap();
        let rendered = outcome
            .action
            .render(outcome.target, &|rel: &str| rel.to_string());
        assert!(
            rendered.contains("codex mcp add memstead -- /bin/memstead-mcp"),
            "got: {rendered}",
        );
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }
}
