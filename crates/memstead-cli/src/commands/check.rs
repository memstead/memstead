//! `memstead check` — record a check of one entity (agent-trust
//! plan 14).
//!
//! Mirrors the MCP `memstead_check` tool 1:1. A check is the
//! engine-recorded act of verification: verdict from the closed
//! vocabulary (`ok` | `failed`), optional method note, plan-13
//! provenance (actor, client, the session's `--role`), and the
//! entity's `content_hash` at check time — appended to the
//! workspace's append-only check ledger. Checking mutates nothing:
//! no entity write, no mem commit. Derived check state is served by
//! `memstead entity <id> --provenance`.

use clap::Parser;
use memstead_base::EntityId;
use memstead_base::check::{VERDICTS, Verdict};
use memstead_base::vcs::Actor;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::CliContext;

#[derive(Parser, Debug)]
pub struct Args {
    /// Full entity id (`mem--slug`) of the entity that was checked.
    pub id: String,

    /// The verdict: `ok` | `failed`. The vocabulary is closed —
    /// nuance goes in `--method` or in process-mem entities.
    #[arg(long)]
    pub verdict: String,

    /// Free-text method note — how the check was performed.
    #[arg(long)]
    pub method: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let Some(verdict) = Verdict::from_wire(&args.verdict) else {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_VERDICT",
            format!(
                "unknown verdict {:?} — the vocabulary is: {}",
                args.verdict,
                VERDICTS.join(", ")
            ),
        )
        .into());
    };
    let id = EntityId::canonical(&args.id);
    let mut engine = ctx.cli_engine()?.into_base();
    let client = crate::setup::cli_client_id();
    let record = engine
        .record_check(
            id.mem(),
            id.as_ref(),
            verdict,
            args.method.as_deref(),
            Actor::Cli,
            Some(&client),
        )
        .map_err(CliError::from_engine_op)?;
    let (state, _) = engine
        .entity_check_state(id.mem(), id.as_ref())
        .map_err(CliError::from_engine_op)?;
    if ctx.json {
        print_json(&serde_json::json!({
            "entity": record.entity,
            "verdict": record.verdict,
            "check_state": state.as_str(),
            "role": record.role,
            "ts": record.ts,
            "method": record.method,
        }))?;
        return Ok(());
    }
    print_markdown(&format!(
        "Check recorded: `{}` — verdict **{}**, state `{}` (role: {})",
        record.entity,
        record.verdict,
        state.as_str(),
        record.role
    ));
    Ok(())
}
