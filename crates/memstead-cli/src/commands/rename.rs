//! `memstead rename` — change an entity's title, ID, file path, and every incoming wiki-link.
//!
//! Hash handling matches `memstead update`: strict by default, `--auto-hash`
//! refetches from the store, `--force` refetches too and names the intent to
//! overwrite (the engine still compares, so a concurrent write is refused).

use clap::Parser;

use memstead_base::vcs::Actor;
use memstead_base::{EntityId, RenameEntityArgs};

use crate::CliError;
use crate::output::{print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

#[derive(Parser, Debug)]
#[command(after_long_help = super::slug_derivation_help())]
pub struct Args {
    /// Current entity ID. A bare slug without the `mem--` prefix resolves
    /// when exactly one mounted mem carries it (announced as
    /// `SHORT_ID_RESOLVED`); otherwise refuses `ENTITY_ID_MISSING_MEM` naming
    /// the candidates.
    pub id: String,

    /// New title. The ID is re-derived from the title.
    pub new_title: String,

    /// Hash from `memstead entity <id>`. Required unless `--auto-hash` or `--force`.
    #[arg(long = "expected-hash", value_name = "HASH")]
    pub expected_hash: Option<String>,

    /// Refetch the current hash immediately before writing.
    #[arg(long, conflicts_with_all = ["expected_hash", "force"])]
    pub auto_hash: bool,

    /// Read the current hash and present it, naming the intent to overwrite.
    /// The engine still compares, so a write that lands after the read is
    /// refused rather than overwritten.
    #[arg(long, conflicts_with_all = ["expected_hash", "auto_hash"])]
    pub force: bool,

    /// Agent-authored provenance note (≤280 chars). When
    /// `[mutations].require_notes = true` a missing note adds a
    /// `NOTE_MISSING` warning.
    #[arg(long)]
    pub note: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let id = EntityId::canonical(&args.id);
    let new_title = args.new_title.clone();

    match ctx.cli_engine()? {
        CliEngine::MemRepo(engine) => {
            let mut engine = memstead_base::OperationScope::begin(engine);
            let lookup_id = crate::setup::preflight_id(&mut engine, &id)?;
            let expected_hash = super::hash_statement(
                &mut engine,
                &lookup_id,
                args.expected_hash.clone(),
                args.auto_hash || args.force,
            )?;
            let result = engine
                .rename_entity_with_ctx(
                    &id,
                    &new_title,
                    expected_hash,
                    &ctx.commit_ctx_with_note(args.note.clone()),
                )
                .map_err(CliError::from_engine_op)?;
            let (_engine, mem_changed) = engine.finish();
            if ctx.json {
                let mut body = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
                super::merge_mem_changed_json(&mut body, &mem_changed);
                print_json(&body)?;
            } else {
                let mut body = format!(
                    "# Renamed\n\n- `{}` → `{}`\n- Path: {} → {}\n- Hash: `{}`",
                    result.old_id,
                    result.new_id,
                    result.old_path,
                    result.new_path,
                    result.content_hash,
                );
                // The same warnings line the filesystem branch and the
                // sibling verbs render: a `SHORT_ID_RESOLVED` or
                // `NOTE_MISSING` hint must not be visible only under
                // `--json`.
                if !result.warnings.is_empty() {
                    let parts: Vec<String> =
                        result.warnings.iter().map(|w| w.to_string()).collect();
                    body.push_str(&format!("\n- Warnings: {}", parts.join("; ")));
                }
                body.push_str(&super::render_mem_changed_block(&mem_changed));
                print_markdown(&body);
            }
        }
        CliEngine::Filesystem(mut engine) => {
            let lookup_id = crate::setup::preflight_id(&mut engine, &id)?;
            let expected_hash = super::hash_statement(
                &mut engine,
                &lookup_id,
                args.expected_hash.clone(),
                args.auto_hash || args.force,
            )?;
            let outcome = engine
                .rename_entity(
                    RenameEntityArgs {
                        id: id.clone(),
                        expected_hash,
                        new_title: new_title.clone(),
                    },
                    Actor::Cli,
                    None,
                    args.note.as_deref(),
                )
                .map_err(CliError::from_engine_op)?;
            if ctx.json {
                print_json(&serde_json::json!({
                    "old_id": outcome.old_id.as_ref(),
                    "new_id": outcome.new_id.as_ref(),
                    "old_path": outcome.old_path,
                    "new_path": outcome.new_path,
                    "_hash": outcome.content_hash,
                    // Backend write identity — response-shape parity with
                    // the MCP server.
                    "write_id": outcome.write_id,
                    // Engine-emitted warnings (e.g. `NOTE_MISSING` under
                    // `[mutations].require_notes`).
                    "warnings": outcome.warnings,
                }))?;
            } else {
                let mut body = format!(
                    "# Renamed\n\n- `{}` → `{}`\n- Path: {} → {}\n- Hash: `{}`",
                    outcome.old_id,
                    outcome.new_id,
                    outcome.old_path,
                    outcome.new_path,
                    outcome.content_hash,
                );
                if !outcome.warnings.is_empty() {
                    let parts: Vec<String> =
                        outcome.warnings.iter().map(|w| w.to_string()).collect();
                    body.push_str(&format!("\n- Warnings: {}", parts.join("; ")));
                }
                print_markdown(&body);
            }
        }
    }
    Ok(())
}
