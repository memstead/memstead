//! `memstead gates` — render the gates brief: the standing of every
//! schema-declared `transition_requires_checks` gate across the
//! workspace's mounted mems. The renderer is the shared engine entry
//! point `Engine::render_gates_brief`, so every consuming surface
//! serves byte-identical content — the due-brief precedent. There is
//! deliberately no MCP tool (briefs are the CLI family).

use clap::Args as ClapArgs;
use serde_json::json;

use crate::output::{print_json, print_markdown};
use crate::setup::CliContext;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Restrict the brief to one mem (default: every mounted mem whose
    /// schema declares a gated transition).
    #[arg(long)]
    pub mem: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let engine = ctx.cli_engine()?;
    let brief = engine.base().render_gates_brief(args.mem.as_deref());
    if ctx.json {
        print_json(&json!({
            "mem": args.mem,
            "brief": brief,
        }))?;
    } else {
        print_markdown(&brief);
    }
    Ok(())
}
