//! `memstead reload` — refresh the engine's in-memory store from on-disk
//! branch state. CLI surface parity with the MCP `memstead_reload` tool:
//! without the `Reload` subcommand variant, `memstead reload` would refuse
//! with `unrecognized subcommand` while the same op stays reachable through
//! MCP. AGENTS.md's parity rule
//! ("every operation reachable through the engine SHOULD be
//! reachable via MCP and CLI alike") makes this the correct
//! direction to close.

use clap::Parser;

use crate::CliError;
use crate::output::{print_json, print_markdown};
use crate::setup::CliContext;

#[derive(Parser, Debug)]
pub struct Args {
    /// Writable mem name to reload. Omit to reload every writable
    /// mem. Mirrors the MCP `memstead_reload` parameter shape and the
    /// op's semantics: per-mem form is cheap and skips the
    /// workspace-level settings refresh; workspace-wide form
    /// (omit `--mem`) reloads every mem and also re-reads the
    /// workspace policy to pick up edits.
    #[arg(long)]
    pub mem: Option<String>,

    /// Additive full refresh: re-scan the schema sources and the
    /// mount manifest on top of the workspace-wide content reload.
    /// Out-of-band schema installs become resolvable and out-of-band
    /// mem registrations mount cold; removals are skipped and
    /// reported (they take effect on restart). Workspace-scoped —
    /// conflicts with `--mem`. Mirrors MCP `memstead_reload
    /// full=true`. (Mostly useful against a live server via MCP; in a
    /// fresh CLI process boot already sees everything — the flag
    /// exists for parity and for exercising the refresh path.)
    #[arg(long, conflicts_with = "mem")]
    pub full: bool,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let mut engine = ctx.cli_engine()?.into_base();
    let refresh = args.full.then(|| engine.full_refresh());
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
        let mut payload = serde_json::json!({ "reports": reports });
        if let Some(refresh) = &refresh {
            payload["refresh"] = serde_json::to_value(refresh).unwrap_or(serde_json::Value::Null);
        }
        print_json(&payload)?;
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
        if let Some(refresh) = &refresh {
            lines.push(String::new());
            lines.push(format!("## Full refresh ({} ms)", refresh.elapsed_ms));
            lines.push(format!(
                "- schemas added: {}",
                render_list(&refresh.schemas_added)
            ));
            lines.push(format!(
                "- schema removals skipped: {}",
                render_list(&refresh.schema_removals_skipped)
            ));
            lines.push(format!(
                "- mems mounted: {}",
                render_list(&refresh.mems_mounted)
            ));
            lines.push(format!(
                "- mem removals skipped: {}",
                render_list(&refresh.mem_removals_skipped)
            ));
            for f in &refresh.failures {
                lines.push(format!("- ✗ {} — {}", f.item, f.error));
            }
        }
        print_markdown(&lines.join("\n"));
    }
    Ok(())
}

fn render_list(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(", ")
    }
}

fn short_sha(sha: &str) -> &str {
    let n = sha.len().min(8);
    &sha[..n]
}
