//! Memstead MCP Server — binary entry point producing the `memstead-mcp`
//! binary that every external integration invokes (Claude Code plugin,
//! macOS spawn path, install scripts, `MEMSTEAD_MCP_BIN` env var).
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
    /// / `memstead mem delete`. Agent-spawned servers (Claude Code
    /// plugin, macOS chat subprocess) do not.
    #[cfg(feature = "mem-repo")]
    #[arg(long = "operator-mode", default_value_t = false)]
    operator_mode: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cwd = std::env::current_dir().context("Could not determine current directory")?;

    let workspace_root = find_workspace_root(&cwd).ok_or_else(|| {
        anyhow::anyhow!(
            "CONFIG_ERROR: no `.memstead/workspace.toml` workspace found in cwd or any \
             ancestor. Run `memstead mem-repo init` to bootstrap a new workspace."
        )
    })?;

    run(args, workspace_root).await
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
        match current.parent() {
            Some(p) => current = p,
            None => return None,
        }
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

    tracing::info!(
        "boot: lean workspace at {} (folder + archive backends only)",
        workspace_root.display()
    );

    let server = memstead_mcp::filesystem_server::FilesystemMcpServer::from_workspace_root(
        &workspace_root,
    )
    .with_context(|| format!("init lean engine at {}", workspace_root.display()))?;

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

    init_tracing();

    tracing::info!("boot: mem-repo workspace at {}", workspace_root.display());

    let engine = memstead_git_branch::workspace_store::engine_from_workspace_root(&workspace_root)
        .with_context(|| format!("failed to load workspace at {}", workspace_root.display()))?;

    let stats = engine.stats();
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
    let disabled_tools_raw: Vec<String> =
        settings.mcp.disabled_tools.clone().unwrap_or_default();
    let mutations = settings.mutations.clone();
    let plugin = settings.plugin.clone();

    if !args.read_mems.is_empty() {
        let target_mem_name = engine
            .mem_router()
            .writable_mems()
            .iter()
            .next()
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "CONFIG_ERROR: --read-mem was supplied but no writable mem is registered to receive the registration."
                )
            })?;
        let target = memstead_git_branch::mem_cache::TargetMem::MemRepo {
            workspace_root: &workspace_root,
            mem_name: &target_mem_name,
        };
        let install_ctx = memstead_git_branch::CommitContext {
            actor: memstead_git_branch::Actor::Cli,
            client: Some(memstead_git_branch::ClientId {
                name: "memstead-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            }),
            tool: None,
            note: None,
            logical_operation_id: None,
            entity_ids: None,
        };
        let install_message = format!(
            "memstead: install (read-mem registration into {target_mem_name})"
        );
        let cwd = std::env::current_dir()
            .context("Could not determine current directory for --read-mem resolution")?;
        // Pass the workspace's writable-mount roster so
        // `install_read_mem` can refuse archives whose authoritative
        // name shadows a writable. An earlier shape reported install
        // success while `hydrate_read_mems` silently skipped the
        // registration.
        let writable: Vec<String> = engine
            .mem_router()
            .writable_mems()
            .iter()
            .map(|n| n.to_string())
            .collect();
        let writable_refs: Vec<&str> = writable.iter().map(String::as_str).collect();
        let results = read_mems::install_read_mems(
            &args.read_mems,
            target,
            &install_ctx,
            &install_message,
            &cwd,
            &writable_refs,
        );
        for result in results {
            match result {
                read_mems::ReadMemResult::Installed { archive, outcome } => {
                    tracing::info!(
                        "installed read-mem {} from {} (cache_copy={}, registered={})",
                        outcome.mem_name,
                        archive.display(),
                        outcome.copied_to_cache,
                        outcome.registered_in_config,
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
                    tracing::warn!(
                        "skipped --read-mem {}: {}",
                        archive.display(),
                        error
                    );
                }
            }
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
    .with_operator_mode(args.operator_mode);

    let service = server.serve(stdio()).await?;
    service.waiting().await?;

    Ok(())
}
