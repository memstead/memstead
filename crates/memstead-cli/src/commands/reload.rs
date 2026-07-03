//! `memstead reload` — refresh the engine's in-memory store from on-disk
//! branch state. CLI surface parity with the MCP `memstead_reload` tool:
//! without the `Reload` subcommand variant, `memstead reload` would refuse
//! with `unrecognized subcommand` while the same op stays reachable through
//! MCP. AGENTS.md's parity rule
//! ("every operation reachable through the engine SHOULD be
//! reachable via both UniFFI and CLI") makes this the correct
//! direction to close.

use clap::Parser;

use crate::CliError;
use crate::output::{print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

#[derive(Parser, Debug)]
pub struct Args {
    /// Writable mem name to reload. Omit to reload every writable
    /// mem. Mirrors the MCP `memstead_reload` parameter shape and the
    /// op's semantics: per-mem form is cheap and skips the
    /// workspace-level settings refresh; workspace-wide form
    /// (omit `--mem`) reloads every mem and also re-reads
    /// `.memstead/workspace.toml` to pick up policy edits.
    #[arg(long)]
    pub mem: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let mut engine = match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(engine) => engine,
        CliEngine::Filesystem(engine) => engine,
    };
    let reports = match args.mem.as_deref() {
        Some(name) => engine
            .reload_one_mem_report(name)
            .map(|r| vec![r])
            .map_err(CliError::from_engine_op)?,
        None => engine
            .reload_each_writable_mem_reports()
            .map_err(CliError::from_engine_op)?,
    };

    if ctx.json {
        print_json(&serde_json::json!({ "reports": reports }))?;
    } else {
        let mut lines = vec![
            format!("# Reloaded {} mem(s)", reports.len()),
            String::new(),
        ];
        for r in &reports {
            lines.push(format!(
                "- `{}` — {} entities, head {} → {}{}",
                r.mem,
                r.entities_loaded,
                short_sha(&r.head_before),
                short_sha(&r.head_after),
                if r.changed_entity_ids.is_empty() {
                    String::new()
                } else {
                    format!(" ({} changed)", r.changed_entity_ids.len())
                },
            ));
        }
        print_markdown(&lines.join("\n"));
    }
    Ok(())
}

fn short_sha(sha: &str) -> &str {
    let n = sha.len().min(8);
    &sha[..n]
}
