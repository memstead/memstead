//! `memstead delete` — remove an entity, its file, and all its relationships.
//!
//! `--dry-run` does a non-destructive preview by reading the entity and
//! counting its relations; no engine-side dry-run exists — the MCP tool
//! carries no `dry_run` param, and optimistic locking via `expected_hash`
//! is the shipping safety mechanism.

use clap::Parser;

use memstead_base::vcs::Actor;
use memstead_base::{DeleteEntityArgs, EntityId};

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

#[derive(Parser, Debug)]
pub struct Args {
    /// Entity ID to delete.
    pub id: String,

    /// Show what would be removed without deleting anything.
    #[arg(long)]
    pub dry_run: bool,

    /// Agent-authored provenance note (≤280 chars). When
    /// `[mutations].require_notes = true` a missing note adds a
    /// `NOTE_MISSING` warning.
    #[arg(long)]
    pub note: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let id = EntityId::canonical(&args.id);
    match ctx.cli_engine()? {
        #[cfg(feature = "vault-repo")]
        CliEngine::VaultRepo(engine) => run_vault_repo(ctx, engine, id, args),
        CliEngine::Filesystem(engine) => run_filesystem(ctx, engine, id, args),
    }
}

#[cfg(feature = "vault-repo")]
fn run_vault_repo(
    ctx: &CliContext,
    mut engine: memstead_base::Engine,
    id: EntityId,
    args: Args,
) -> anyhow::Result<()> {
    if args.dry_run {
        let entity = engine
            .get_entity(&id)
            .ok_or_else(|| {
                CliError::new(
                    ExitKind::NotFound,
                    "ENTITY_NOT_FOUND",
                    format!("entity not found: {}", id),
                )
                .with_details(serde_json::json!({ "id": id.to_string() }))
            })?
            .clone();
        let referrers = engine.classify_delete_referrers(&id);
        let outgoing = engine.store().outgoing(&id).len();
        return print_dry_run(
            ctx,
            &id,
            &entity.title,
            &entity.file_path,
            &referrers,
            outgoing,
        );
    }

    // `expected_hash` is mandatory on `Engine::delete_entity`. The CLI
    // reads the current hash itself rather than exposing a flag — agents
    // and humans invoking `memstead delete <id>` want one-shot semantics, and
    // there's no meaningful external concurrency against a user-driven
    // CLI process. MCP keeps the full read-then-lock pattern so a
    // multi-agent workflow can't stomp itself.
    let current_hash = engine
        .get_entity(&id)
        .ok_or_else(|| {
            CliError::new(
                ExitKind::NotFound,
                "ENTITY_NOT_FOUND",
                format!("entity not found: {}", id),
            )
            .with_details(serde_json::json!({ "id": id.to_string() }))
        })?
        .content_hash
        .clone();
    let result = engine
        .delete_entity_with_ctx(
            &id,
            &current_hash,
            &crate::setup::cli_ctx_with_note(args.note.clone()),
        )
        .map_err(CliError::from_engine_op)?;
    let vault_changed = engine.take_vault_changed_notices();

    if ctx.json {
        let mut body = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
        super::merge_vault_changed_json(&mut body, &vault_changed);
        print_json(&body)?;
    } else {
        print_markdown(&format!(
            "# Deleted `{}`\n\n- Relations removed: {}{}",
            result.id,
            result.relations_removed,
            super::render_vault_changed_block(&vault_changed),
        ));
    }
    Ok(())
}

fn run_filesystem(
    ctx: &CliContext,
    mut engine: memstead_base::Engine,
    id: EntityId,
    args: Args,
) -> anyhow::Result<()> {
    if args.dry_run {
        let entity = engine
            .get_entity(&id)
            .ok_or_else(|| {
                CliError::new(
                    ExitKind::NotFound,
                    "ENTITY_NOT_FOUND",
                    format!("entity not found: {}", id),
                )
                .with_details(serde_json::json!({ "id": id.to_string() }))
            })?
            .clone();
        let referrers = engine.classify_delete_referrers(&id);
        let outgoing = engine.store().outgoing(&id).len();
        return print_dry_run(
            ctx,
            &id,
            &entity.title,
            &entity.file_path,
            &referrers,
            outgoing,
        );
    }

    // Same hash-snapshot posture as the vault-repo path.
    let current_hash = engine
        .get_entity(&id)
        .ok_or_else(|| {
            CliError::new(
                ExitKind::NotFound,
                "ENTITY_NOT_FOUND",
                format!("entity not found: {}", id),
            )
            .with_details(serde_json::json!({ "id": id.to_string() }))
        })?
        .content_hash
        .clone();
    let outcome = engine
        .delete_entity(
            DeleteEntityArgs {
                id: id.clone(),
                expected_hash: Some(current_hash),
            },
            Actor::Cli,
            None,
            args.note.as_deref(),
        )
        .map_err(CliError::from_engine_op)?;

    let relations_removed = outcome.removed_incoming.len();
    if ctx.json {
        print_json(&serde_json::json!({
            "id": outcome.id.as_ref(),
            "file_path": outcome.file_path,
            "relations_removed": relations_removed,
            // Engine-emitted warnings (e.g. `NOTE_MISSING` under
            // `[mutations].require_notes`).
            "warnings": outcome.warnings,
        }))?;
    } else {
        let mut body = format!(
            "# Deleted `{}`\n\n- Relations removed: {}",
            outcome.id, relations_removed,
        );
        if !outcome.warnings.is_empty() {
            let parts: Vec<String> = outcome.warnings.iter().map(|w| w.to_string()).collect();
            body.push_str(&format!("\n- Warnings: {}", parts.join("; ")));
        }
        print_markdown(&body);
    }
    Ok(())
}

/// Render the dry-run preview, including the would-be verdict. The
/// referrer classification comes straight from the engine's delete guard
/// (`classify_delete_referrers`), so the preview's verdict matches what
/// the real `memstead delete` would do: Write-Vault referrers would refuse
/// with `HAS_INCOMING_REFS`; ReadOnly-only referrers would proceed via
/// the residual-stub demotion; none would remove cleanly. The agent can
/// branch on the preview alone without re-encoding the deletion ruleset.
fn print_dry_run(
    ctx: &CliContext,
    id: &EntityId,
    title: &str,
    file_path: &str,
    referrers: &memstead_base::DeleteReferrers,
    outgoing: usize,
) -> anyhow::Result<()> {
    let blocking = &referrers.write_referrers;
    let readonly = &referrers.readonly_referrers;
    let incoming = blocking.len() + readonly.len();
    let relations_total = incoming + outgoing;
    let would_refuse = referrers.would_refuse();
    if ctx.json {
        let blocking_json: Vec<_> = blocking
            .iter()
            .map(|r| {
                serde_json::json!({
                    "from_id": r.from_id,
                    "rel_types": r.rel_types,
                    "vault": r.vault,
                })
            })
            .collect();
        let readonly_json: Vec<String> = readonly.iter().map(|r| r.to_string()).collect();
        print_json(&serde_json::json!({
            "id": id.as_ref(),
            "title": title,
            "file_path": file_path,
            // Same `referrers` key the failure-path payload uses, holding
            // the blocking (Write-Vault) sources only — these are what the
            // agent must clear before the real delete can proceed.
            "referrers": blocking_json,
            "readonly_referrers": readonly_json,
            "relations_incoming": incoming,
            "relations_outgoing": outgoing,
            "relations_total": relations_total,
            // The would-be verdict. `would_refuse` lets the agent branch
            // without applying the ruleset; `refusal_code` mirrors the real
            // error code so the preview and the failure share a vocabulary.
            "would_refuse": would_refuse,
            "refusal_code": if would_refuse { Some("HAS_INCOMING_REFS") } else { None },
            "blocking_referrers": blocking.len(),
            "dry_run": true,
        }))?;
    } else {
        let verdict = if would_refuse {
            format!(
                "would REFUSE — `HAS_INCOMING_REFS` ({} blocking referrer(s); remove them first)",
                blocking.len()
            )
        } else if !readonly.is_empty() {
            format!(
                "would PROCEED — {} read-only referrer(s) keep a residual stub at this id",
                readonly.len()
            )
        } else {
            "would PROCEED — clean removal".to_string()
        };
        let mut lines = vec![
            format!("# Dry-run `{}`", id),
            String::new(),
            format!("- Title: {title}"),
            format!("- File: {file_path}"),
            format!("- Relations in: {incoming}"),
            format!("- Relations out: {outgoing}"),
            format!("- Verdict: {verdict}"),
        ];
        if !blocking.is_empty() {
            lines.push(String::new());
            lines.push("## Blocking referrers".to_string());
            for r in blocking {
                lines.push(format!(
                    "- `{}` [{}] ({})",
                    r.from_id,
                    r.rel_types.join(", "),
                    r.vault
                ));
            }
        }
        if !readonly.is_empty() {
            lines.push(String::new());
            lines.push("## Read-only referrers (non-blocking)".to_string());
            for r in readonly {
                lines.push(format!("- `{r}`"));
            }
        }
        print_markdown(&lines.join("\n"));
    }
    Ok(())
}
