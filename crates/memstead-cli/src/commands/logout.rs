//! `memstead logout` — remove the stored credentials entry for one
//! registry host. Silent no-op if nothing was stored.

use clap::Parser;
use serde_json::json;

use crate::auth::credentials;
use crate::output::{print_json, print_markdown};
use crate::registry;
use crate::setup::CliContext;

#[derive(Parser, Debug)]
pub struct Args {
    /// Registry URL (overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io).
    #[arg(long, value_name = "URL")]
    pub registry: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let base = registry::registry_base(args.registry.as_deref());
    let host = registry::registry_host(&base);

    let removed = credentials::remove_for(&host)?;

    if ctx.json {
        print_json(&json!({ "ok": true, "registry": host, "removed": removed }))?;
    } else if removed {
        print_markdown(&format!("# Logged out from {host}"));
    } else {
        print_markdown(&format!("# Not logged in to {host} (nothing to remove)"));
    }
    Ok(())
}
