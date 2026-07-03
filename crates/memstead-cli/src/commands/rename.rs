//! `memstead rename` — change an entity's title, ID, file path, and every incoming wiki-link.
//!
//! Hash handling matches `memstead update`: strict by default, `--auto-hash`
//! refetches from the store, `--force` explicitly accepts the overwrite.

use clap::Parser;

use memstead_base::vcs::Actor;
use memstead_base::{EntityId, RenameEntityArgs};

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

#[derive(Parser, Debug)]
#[command(after_long_help = super::SLUG_DERIVATION_HELP)]
pub struct Args {
    /// Current entity ID.
    pub id: String,

    /// New title. The ID is re-derived from the title.
    pub new_title: String,

    /// Hash from `memstead entity <id>`. Required unless `--auto-hash` or `--force`.
    #[arg(long = "expected-hash", value_name = "HASH")]
    pub expected_hash: Option<String>,

    /// Refetch the current hash immediately before writing.
    #[arg(long, conflicts_with_all = ["expected_hash", "force"])]
    pub auto_hash: bool,

    /// Skip the hash check (explicit overwrite).
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
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(mut engine) => {
            let expected_hash = resolve_expected_hash_mem_repo(&engine, &id, &args)?;
            let result = engine
                .rename_entity_with_ctx(
                    &id,
                    &new_title,
                    &expected_hash,
                    &crate::setup::cli_ctx_with_note(args.note.clone()),
                )
                .map_err(CliError::from_engine_op)?;
            let mem_changed = engine.take_mem_changed_notices();
            if ctx.json {
                let mut body = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
                super::merge_mem_changed_json(&mut body, &mem_changed);
                print_json(&body)?;
            } else {
                print_markdown(&format!(
                    "# Renamed\n\n- `{}` → `{}`\n- Path: {} → {}\n- Hash: `{}`{}",
                    result.old_id,
                    result.new_id,
                    result.old_path,
                    result.new_path,
                    result.content_hash,
                    super::render_mem_changed_block(&mem_changed),
                ));
            }
        }
        CliEngine::Filesystem(mut engine) => {
            let expected_hash = resolve_expected_hash_filesystem(&engine, &id, &args)?;
            let outcome = engine
                .rename_entity(
                    RenameEntityArgs {
                        id: id.clone(),
                        expected_hash: Some(expected_hash),
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

/// Resolve the `expected_hash` for the mem-repo path: either the
/// flag value, or the live hash from the store under `--auto-hash` /
/// `--force`. Mirrors the original inline logic — extracted only so
/// the filesystem path can run the same flag plumbing without
/// duplicating it.
#[cfg(feature = "mem-repo")]
fn resolve_expected_hash_mem_repo(
    engine: &memstead_base::Engine,
    id: &EntityId,
    args: &Args,
) -> anyhow::Result<String> {
    if args.auto_hash || args.force {
        Ok(engine
            .get_entity(id)
            .ok_or_else(|| {
                CliError::new(
                    ExitKind::NotFound,
                    "ENTITY_NOT_FOUND",
                    format!("entity not found: {id}"),
                )
                .with_details(serde_json::json!({ "id": id.to_string() }))
            })?
            .content_hash
            .clone())
    } else {
        args.expected_hash
            .clone()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| {
                CliError::new(
                    ExitKind::Validation,
                    crate::HASH_FLAG_REQUIRED_CODE,
                    "missing --expected-hash. Read the entity first (memstead entity <id>) and pass its `_hash`, \
                     or use --auto-hash / --force.",
                )
                .into()
            })
    }
}

fn resolve_expected_hash_filesystem(
    engine: &memstead_base::Engine,
    id: &EntityId,
    args: &Args,
) -> anyhow::Result<String> {
    if args.auto_hash || args.force {
        Ok(engine
            .get_entity(id)
            .ok_or_else(|| {
                CliError::new(
                    ExitKind::NotFound,
                    "ENTITY_NOT_FOUND",
                    format!("entity not found: {id}"),
                )
                .with_details(serde_json::json!({ "id": id.to_string() }))
            })?
            .content_hash
            .clone())
    } else {
        args.expected_hash
            .clone()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| {
                CliError::new(
                    ExitKind::Validation,
                    crate::HASH_FLAG_REQUIRED_CODE,
                    "missing --expected-hash. Read the entity first (memstead entity <id>) and pass its `_hash`, \
                     or use --auto-hash / --force.",
                )
                .into()
            })
    }
}
