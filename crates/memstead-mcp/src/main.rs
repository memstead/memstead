//! Memstead MCP Server — binary entry point producing the `memstead-mcp`
//! binary that every external integration invokes (Claude Code plugin,
//! install scripts, `MEMSTEAD_MCP_BIN` env var).
//!
//! One crate, two build configs. The default build (`mem-repo` feature
//! on) serves the multi-mem, git-backed engine; `--no-default-features`
//! serves the folder + archive engine only (no `gix`, no
//! `memstead-git-branch`) — a CI / wasm-adjacent config, not shipped.
//!
//! Workspace resolution (both configs): walk upward from cwd for the
//! first ancestor that carries `.memstead/workspace.toml`. Operators on
//! pre-rebuild layouts run `memstead mem-repo init` to bootstrap a
//! fresh workspace.

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use rmcp::ServiceExt;
use rmcp::transport::stdio;

#[cfg(feature = "mem-repo")]
use clap::ArgAction;

/// memstead-mcp — serves the Memstead graph engine over MCP on stdio.
#[derive(Parser, Debug)]
#[command(name = "memstead-mcp", version, about, long_about = None)]
struct Args {
    /// Attach a sealed `.mem` mem as a read-only reference. Repeatable —
    /// `--read-mem a.mem --read-mem b.mem` attaches both. Each path
    /// is installed into the global mem cache (if the cached file is
    /// missing) and registered in the first writable mem's `readMems`
    /// with `source: { type: "local" }` so the next run picks it up from
    /// the config alone.
    #[cfg(feature = "mem-repo")]
    #[arg(long = "read-mem", value_name = "PATH", action = ArgAction::Append)]
    read_mems: Vec<PathBuf>,

    /// Operator-mode startup signal. When set, mem-lifecycle calls
    /// (`memstead_mem_create`, `memstead_mem_delete`) bypass the
    /// `[mem_management]` allowlists in `.memstead/workspace.toml` and
    /// the `MEM_REFERENCED_BY_POLICY` safeguard on delete. The flag is
    /// process-scoped — children spawned without it are not in
    /// operator-mode, and there is no env-var equivalent. `memstead`
    /// sets this flag when it spawns `memstead-mcp` for `memstead mem init`
    /// / `memstead mem delete`. Agent-spawned servers (e.g. the Claude
    /// Code plugin) do not.
    #[cfg(feature = "mem-repo")]
    #[arg(long = "operator-mode", default_value_t = false)]
    operator_mode: bool,

    /// Session-level default role for every mutation this server
    /// performs (agent-trust plan 13): `author` | `checker` |
    /// `verifier`. Per-call `role` parameters win. Omit to record
    /// mutations as unspecified unless a call declares otherwise.
    #[arg(long = "role")]
    role: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cwd = std::env::current_dir().context("Could not determine current directory")?;

    let workspace_root = find_workspace_root(&cwd).ok_or_else(|| {
        anyhow::anyhow!(
            "memstead-mcp: ERROR [WORKSPACE_NOT_INITIALISED]: no `.memstead/workspace.toml` \
             workspace found in cwd or any ancestor — run `memstead mem-repo init` to bootstrap \
             a new workspace"
        )
    })?;

    run(args, workspace_root).await
}

/// Render a workspace-level boot failure onto stderr in the same
/// typed shape the CLI prints (`ERROR [<CODE>]: <message>`, message
/// from [`memstead_base::BootError::surface_message`]), then build the
/// mem-less diagnostic-shell engine that serves in its place — the
/// server STARTS regardless (degrade, never disappear; agent-trust
/// plan 04): overview/health answer with this diagnosis instead of
/// the historical `-32000 Connection closed` exit, so a session can
/// always ask why the graph is gone. Mem-level failures never reach
/// here — they quarantine inside a normally-booted engine.
fn diagnostic_shell_engine(
    workspace_root: &std::path::Path,
    e: memstead_base::BootError,
) -> memstead_base::Engine {
    let message = e.surface_message(workspace_root);
    eprintln!("memstead-mcp: ERROR [{}]: {message}", e.code());
    memstead_base::Engine::diagnostic_shell(e.code().to_string(), message)
}

/// Walk upward from `cwd` looking for the first ancestor that carries
/// `.memstead/workspace.toml` (the workspace marker). Returns the
/// workspace root on hit, `None` when no ancestor carries the marker.
fn find_workspace_root(cwd: &std::path::Path) -> Option<PathBuf> {
    let mut current: &std::path::Path = cwd;
    loop {
        if memstead_base::is_workspace_root(current) {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

fn init_tracing() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
}

/// Boot the lean MCP server (folder + archive backends only).
#[cfg(not(feature = "mem-repo"))]
async fn run(_args: Args, workspace_root: PathBuf) -> anyhow::Result<()> {
    init_tracing();

    // Name the shape actually opened, not the build config: this line
    // is what someone debugging an `UNSUPPORTED_WORKSPACE_SHAPE`
    // refusal reads, and a boot line that disagrees with the refusal
    // reads as a spurious error.
    tracing::info!(
        "boot: {} workspace at {} (lean build: folder + archive backends only)",
        memstead_base::workspace_shape_label(&workspace_root),
        workspace_root.display()
    );

    let server = match memstead_mcp::filesystem_server::FilesystemMcpServer::from_workspace_root(
        &workspace_root,
    ) {
        Ok(server) => server,
        Err(e) => {
            let shell = diagnostic_shell_engine(&workspace_root, e);
            memstead_mcp::filesystem_server::FilesystemMcpServer::from_engine(
                shell,
                workspace_root.clone(),
            )
        }
    };

    let service = server.serve(stdio()).await?;
    service.waiting().await?;

    Ok(())
}

/// Boot the full MCP server. Constructs the unified engine through
/// `memstead_git_branch::engine_from_workspace_root`, then sources
/// `token_budget` / `disabled_tools` / `mutations` / `plugin` from
/// `Engine::settings()`.
#[cfg(feature = "mem-repo")]
async fn run(args: Args, workspace_root: PathBuf) -> anyhow::Result<()> {
    use memstead_mcp::config::{DEFAULT_TOKEN_BUDGET, validate_disabled_tools};
    use memstead_mcp::read_mems;

    let default_role = match args.role.as_deref() {
        None => memstead_base::vcs::Role::Unspecified,
        Some(s) => memstead_base::vcs::Role::from_wire(s).ok_or_else(|| {
            anyhow::anyhow!(
                "memstead-mcp: ERROR [INVALID_ROLE]: unknown role {s:?} — declarable roles: {}",
                memstead_base::vcs::Role::DECLARABLE.join(", ")
            )
        })?,
    };

    init_tracing();

    // Name the shape actually opened. The full build serves both
    // shapes, and the mem-repo-only subcommands refuse on one of them —
    // a boot line that always said "mem-repo" made that refusal look
    // spurious to anyone reading the log.
    tracing::info!(
        "boot: {} workspace at {}",
        memstead_base::workspace_shape_label(&workspace_root),
        workspace_root.display()
    );

    let mut engine =
        match memstead_git_branch::workspace_store::engine_from_workspace_root(&workspace_root) {
            Ok(engine) => engine,
            Err(e) => diagnostic_shell_engine(&workspace_root, e),
        };

    let stats = engine.status();
    tracing::info!(
        "Engine ready: {} entities, {} edges, {} communities",
        stats.entity_count,
        stats.edge_count,
        stats.community_count,
    );

    if args.operator_mode {
        tracing::info!(
            "memstead-mcp: --operator-mode active — mem-lifecycle calls bypass \
             `[mem_management]` allowlists and the `MEM_REFERENCED_BY_POLICY` \
             safeguard for this process."
        );
    }

    let settings = engine.settings();
    let token_budget = settings.mcp.token_budget.unwrap_or(DEFAULT_TOKEN_BUDGET);
    let disabled_tools_raw: Vec<String> = settings.mcp.disabled_tools.clone().unwrap_or_default();
    let mutations = settings.mutations.clone();
    let plugin = settings.plugin.clone();

    if !args.read_mems.is_empty() {
        let cwd = std::env::current_dir()
            .context("Could not determine current directory for --read-mem resolution")?;
        let results = read_mems::install_read_mems(&mut engine, &args.read_mems, &cwd);
        let mut any_mount_change = false;
        for result in results {
            match result {
                read_mems::ReadMemResult::Installed {
                    archive,
                    outcome,
                    mount,
                } => {
                    if mount != memstead_git_branch::mem_cache::MountRegistration::AlreadyRegistered
                    {
                        any_mount_change = true;
                    }
                    tracing::info!(
                        "installed read-mem {} from {} (cache_copy={}, mount={:?})",
                        outcome.mem_name,
                        archive.display(),
                        outcome.copied_to_cache,
                        mount,
                    );
                    // Install warnings surface on the boot log — the
                    // install happens before the MCP transport exists, so
                    // the log is the response channel here.
                    for warning in &outcome.warnings {
                        tracing::warn!(
                            "read-mem {}: [{}] {}",
                            outcome.mem_name,
                            warning.code(),
                            warning.message(),
                        );
                    }
                }
                read_mems::ReadMemResult::Failed { archive, error } => {
                    tracing::warn!("skipped --read-mem {}: {}", archive.display(), error);
                }
            }
        }
        if any_mount_change && let Err(e) = engine.persist_state() {
            tracing::warn!("--read-mem mount-state persistence failed: {e}");
        }
    }

    let known_tool_names: Vec<String> = memstead_mcp::server::McpServer::tool_router()
        .list_all()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    let (effective_disabled, unknown_disabled) =
        validate_disabled_tools(&disabled_tools_raw, &known_tool_names);
    for name in &unknown_disabled {
        tracing::warn!(
            unknown_tool = name.as_str(),
            known_tools = ?known_tool_names,
            "[mcp].disabled_tools entry does not match any compiled-in tool — ignoring",
        );
    }
    if !effective_disabled.is_empty() {
        let mut sorted: Vec<&String> = effective_disabled.iter().collect();
        sorted.sort();
        tracing::info!(
            "memstead-mcp: hiding {} tool(s) via [mcp].disabled_tools: {:?}",
            effective_disabled.len(),
            sorted,
        );
    }

    let config_source = Some(
        workspace_root
            .join(memstead_base::WORKSPACE_STORE_DIR)
            .join("workspace.toml"),
    );
    let server = memstead_mcp::server::McpServer::new_with_config(
        engine,
        token_budget,
        effective_disabled,
        config_source,
        mutations,
        plugin,
    )
    .with_operator_mode(args.operator_mode)
    .with_default_role(default_role);

    let service = server.serve(stdio()).await?;
    service.waiting().await?;

    Ok(())
}
