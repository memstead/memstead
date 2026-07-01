//! `memstead relate` — add or remove a typed relationship between two entities.

use clap::Parser;

use memstead_base::vcs::Actor;
use memstead_base::{EntityId, RelateEntityArgs};

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

/// `memstead relate` accepts each argument as a positional OR as a
/// named flag — the named forms (`--from`, `--rel-type`, `--to`)
/// bring the command's call style in line with the sister mutation
/// commands (`memstead create`, `memstead update`), which use `--title`,
/// `--type`, etc. The positional form continues to work — existing
/// scripts that pipe positional args don't break.
#[derive(Parser, Debug)]
pub struct Args {
    /// Source entity ID (positional). Flag synonym: `--from`.
    #[arg(value_name = "FROM")]
    pub from_pos: Option<String>,

    /// Relationship type (positional). Flag synonym: `--rel-type`.
    /// UPPER_SNAKE_CASE, e.g. `USES`, `PART_OF`.
    #[arg(value_name = "REL_TYPE")]
    pub rel_type_pos: Option<String>,

    /// Target entity ID (positional). Flag synonym: `--to`. Creates
    /// a stub if the target doesn't exist.
    #[arg(value_name = "TO")]
    pub to_pos: Option<String>,

    /// Source entity ID (named flag form).
    #[arg(long = "from", value_name = "ID")]
    pub from_flag: Option<String>,

    /// Relationship type (named flag form).
    #[arg(long = "rel-type", value_name = "REL_TYPE")]
    pub rel_type_flag: Option<String>,

    /// Target entity ID (named flag form).
    #[arg(long = "to", value_name = "ID")]
    pub to_flag: Option<String>,

    /// Remove the relationship instead of creating it.
    #[arg(long)]
    pub remove: bool,

    /// Per-edge description applied on add. Validated against the
    /// rel-type's `per_edge_description` posture; rel-types declared
    /// `forbidden` reject this flag, `required` reject its absence.
    #[arg(long)]
    pub description: Option<String>,

    /// Agent-authored provenance note (≤280 chars). When
    /// `[mutations].require_notes = true` a missing note adds a
    /// `NOTE_MISSING` warning.
    #[arg(long)]
    pub note: Option<String>,
}

/// Resolve a per-slot value from the positional-OR-flag pair. Both
/// supplied is an error (a named flag and a positional would be
/// ambiguous); neither supplied is also an error (the slot is
/// required). Either one alone is the success path.
fn resolve_slot(
    slot: &str,
    positional: &Option<String>,
    flag: &Option<String>,
) -> Result<String, CliError> {
    match (positional.as_deref(), flag.as_deref()) {
        (Some(p), None) => Ok(p.to_string()),
        (None, Some(f)) => Ok(f.to_string()),
        (Some(_), Some(_)) => Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "`{slot}` supplied as both positional and flag; pick one form"
            ),
        )),
        (None, None) => Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "`{slot}` not supplied — pass either as positional or via the `--{slot}` flag"
            ),
        )),
    }
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let from_str = resolve_slot("from", &args.from_pos, &args.from_flag)?;
    let rel_type = resolve_slot("rel-type", &args.rel_type_pos, &args.rel_type_flag)?;
    let to_str = resolve_slot("to", &args.to_pos, &args.to_flag)?;
    let from = EntityId::canonical(&from_str);
    let to = EntityId::canonical(&to_str);
    let remove = args.remove;

    let mut engine = match ctx.cli_engine()? {
        #[cfg(feature = "vault-repo")]
        CliEngine::VaultRepo(engine) => engine,
        CliEngine::Filesystem(engine) => engine,
    };
    // Pass the `memstead-cli@<version>` client identity so the relate
    // commit body carries the same `Client:` provenance trailer as
    // create / update / rename (which set it via `cli_ctx_with_note`).
    let client = crate::setup::cli_client_id();
    let outcome = engine
        .relate_entity(
            RelateEntityArgs {
                source: from.clone(),
                target: to.clone(),
                rel_type: rel_type.clone(),
                remove,
                expected_hash: None,
                description: args.description.clone(),
            },
            Actor::Cli,
            Some(&client),
            args.note.as_deref(),
        )
        .map_err(CliError::from_engine_op)?;
    let vault_changed = engine.take_vault_changed_notices();
    if ctx.json {
        // Always surface `orphan_stubs_removed` so agents and scripts branch
        // uniformly — empty array on add paths and no-op removes,
        // populated on remove paths that GC'd a stub.
        let mut body = serde_json::json!({
            "from": outcome.from.as_ref(),
            "to": outcome.to.as_ref(),
            "rel_type": outcome.rel_type,
            "action": format!("{:?}", outcome.action),
            "_hash": outcome.content_hash,
            "warnings": outcome.warnings,
            "orphan_stubs_removed": outcome
                .orphan_stubs_removed
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>(),
        });
        super::merge_vault_changed_json(&mut body, &vault_changed);
        print_json(&body)?;
    } else {
        let verb = if remove { "Removed" } else { "Added" };
        let warnings_block = if outcome.warnings.is_empty() {
            String::new()
        } else {
            let lines: Vec<String> = outcome
                .warnings
                .iter()
                .map(|w| format!("> - {w}"))
                .collect();
            format!("\n\n> warnings:\n{}", lines.join("\n"))
        };
        let gc_block = if outcome.orphan_stubs_removed.is_empty() {
            String::new()
        } else {
            let ids: Vec<String> = outcome
                .orphan_stubs_removed
                .iter()
                .map(|i| format!("`{i}`"))
                .collect();
            format!("\n\n- orphan stubs GC'd: {}", ids.join(", "))
        };
        let vault_changed_block = super::render_vault_changed_block(&vault_changed);
        print_markdown(&format!(
            "# {verb} `{}` `{}` → `{}`{gc_block}{warnings_block}{vault_changed_block}",
            outcome.from, outcome.rel_type, outcome.to,
        ));
    }
    Ok(())
}
