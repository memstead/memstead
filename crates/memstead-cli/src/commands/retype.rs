//! `memstead retype <id> --type <target>` — change an entity's type in
//! place. The id, file path, and every incoming edge stay; the existing
//! sections and metadata are validated against the target type with a
//! report-all refusal, every edge touching the entity is re-checked
//! against the target's relationship pins, and one commit lands with its
//! own `retype` provenance kind. Because the content hash moves, the
//! response states that check records and derivation baselines on the
//! entity are stale.
//!
//! Hash handling matches `memstead update`: strict by default, `--auto-hash`
//! refetches from the store, `--force` explicitly accepts the overwrite;
//! `--dry-run` previews without a hash.

use clap::Parser;
use indexmap::IndexMap;

use memstead_base::vcs::Actor;
use memstead_base::{EntityId, RetypeEntityArgs};

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

/// Change an entity's type in place. The existing sections and metadata
/// must satisfy the target type; `--section-map old=new` renames section
/// keys on the way (a section the target does not declare refuses
/// `UNKNOWN_SECTION` with the target's declared sections and a proposed
/// map in the details). Every incoming and outgoing edge, cross-mem
/// included, is re-checked against the target type's relationship pins
/// and a violation refuses `INVALID_REL_SHAPE` listing the offending
/// edges; every problem is reported together in one refusal (mixed
/// classes carry `RETYPE_REFUSED`). The id, file path, and incoming edges
/// stay; one commit lands with the `retype` provenance kind; check records
/// and derivation baselines on the entity become stale because its
/// content hash moves, and the response says so. Referrers in a lazy
/// (unloaded) mem are probed through storage; a mem that cannot be probed
/// refuses `RETYPE_REFERRER_UNPROBEABLE` naming it.
#[derive(Parser, Debug)]
pub struct Args {
    /// Entity ID (`mem--slug`). A bare slug resolves when exactly one mounted
    /// mem carries it (announced as `SHORT_ID_RESOLVED`); otherwise refuses
    /// `ENTITY_ID_MISSING_MEM` naming the candidates.
    pub id: String,

    /// The target type, as declared by the mem's schema.
    #[arg(long = "type", value_name = "TYPE")]
    pub target_type: String,

    /// Section key renames applied before validation: `old=new`,
    /// repeatable or comma-separated (`statement=conclusion,notes=context`).
    #[arg(long = "section-map", value_name = "OLD=NEW", value_delimiter = ',')]
    pub section_map: Vec<String>,

    /// Metadata keys to drop explicitly, comma-separated or repeatable:
    /// fields the current type declares and the target does not (a spec's
    /// `level` on the way to a memo). Never inferred — an undeclared field
    /// that is not listed refuses `UNKNOWN_METADATA_FIELD`.
    #[arg(long = "drop-metadata", value_name = "KEY", value_delimiter = ',')]
    pub drop_metadata: Vec<String>,

    /// Hash from `memstead entity <id>`. Required unless `--auto-hash`,
    /// `--force`, or `--dry-run`.
    #[arg(long = "expected-hash", value_name = "HASH")]
    pub expected_hash: Option<String>,

    /// Refetch the current hash immediately before writing.
    #[arg(long, conflicts_with_all = ["expected_hash", "force"])]
    pub auto_hash: bool,

    /// Skip the hash check (explicit overwrite).
    #[arg(long, conflicts_with_all = ["expected_hash", "auto_hash"])]
    pub force: bool,

    /// Validate everything and report the prospective hash without
    /// writing, committing, or changing the store.
    #[arg(long)]
    pub dry_run: bool,

    /// Agent-authored provenance note (≤280 chars). When
    /// `[mutations].require_notes = true` a missing note adds a
    /// `NOTE_MISSING` warning.
    #[arg(long)]
    pub note: Option<String>,
}

fn parse_section_map(raw: &[String]) -> anyhow::Result<IndexMap<String, String>> {
    let mut map = IndexMap::new();
    for entry in raw {
        let Some((from, to)) = entry.split_once('=') else {
            return Err(CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!("--section-map entry `{entry}` is not `old=new`"),
            )
            .into());
        };
        let (from, to) = (from.trim(), to.trim());
        if from.is_empty() || to.is_empty() {
            return Err(CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!("--section-map entry `{entry}` has an empty side"),
            )
            .into());
        }
        map.insert(from.to_string(), to.to_string());
    }
    Ok(map)
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let id = EntityId::canonical(&args.id);
    let section_map = parse_section_map(&args.section_map)?;

    let outcome = match ctx.cli_engine()? {
        CliEngine::MemRepo(mut engine) => {
            // The hash preflight reads the entity the verb will act on,
            // so a bare slug has to be resolved through the engine's one
            // rule first — same seam `update`, `delete` and `rename` use.
            // Reading the raw id here refused a short id with
            // `ENTITY_NOT_FOUND` before the resolver ever ran.
            let lookup_id = crate::setup::preflight_id(&mut engine, &id)?;
            let expected_hash = resolve_expected_hash(&engine, &lookup_id, &args)?;
            let mem_repo_ctx = crate::setup::cli_ctx_with_note(args.note.clone());
            engine.set_role(mem_repo_ctx.role);
            engine.set_identity(mem_repo_ctx.identity.clone());
            let outcome = engine
                .retype_entity(
                    RetypeEntityArgs {
                        id: id.clone(),
                        expected_hash,
                        target_type: args.target_type.clone(),
                        section_map: section_map.clone(),
                        drop_metadata: args.drop_metadata.clone(),
                        dry_run: args.dry_run,
                    },
                    mem_repo_ctx.actor,
                    mem_repo_ctx.client.as_ref(),
                    args.note.as_deref(),
                )
                .map_err(CliError::from_engine_op)?;
            let mem_changed = engine.take_mem_changed_notices();
            (outcome, mem_changed)
        }
        CliEngine::Filesystem(mut engine) => {
            let lookup_id = crate::setup::preflight_id(&mut engine, &id)?;
            let expected_hash = resolve_expected_hash(&engine, &lookup_id, &args)?;
            let outcome = engine
                .retype_entity(
                    RetypeEntityArgs {
                        id: id.clone(),
                        expected_hash,
                        target_type: args.target_type.clone(),
                        section_map,
                        drop_metadata: args.drop_metadata.clone(),
                        dry_run: args.dry_run,
                    },
                    Actor::Cli,
                    None,
                    args.note.as_deref(),
                )
                .map_err(CliError::from_engine_op)?;
            (outcome, Vec::new())
        }
    };
    let (outcome, mem_changed) = outcome;

    if ctx.json {
        let mut body = serde_json::to_value(&outcome).unwrap_or(serde_json::Value::Null);
        if let Some(obj) = body.as_object_mut() {
            obj.insert("dry_run".into(), serde_json::json!(args.dry_run));
        }
        super::merge_mem_changed_json(&mut body, &mem_changed);
        print_json(&body)?;
    } else {
        let title = if args.dry_run {
            "# Retype — dry run, nothing written"
        } else {
            "# Retyped"
        };
        let mut body = format!(
            "{title}\n\n- `{}`: `{}` → `{}`\n- Path: {} (unchanged)\n- Hash: `{}`{}\n- Edges re-checked: {}\n",
            outcome.id,
            outcome.old_type,
            outcome.new_type,
            outcome.file_path,
            outcome.content_hash,
            outcome
                .prospective_hash
                .as_deref()
                .map(|h| format!(" (would become `{h}`)"))
                .unwrap_or_default(),
            outcome.edges_rechecked,
        );
        if !outcome.sections_renamed.is_empty() {
            body.push_str("- Sections renamed: ");
            body.push_str(
                &outcome
                    .sections_renamed
                    .iter()
                    .map(|(a, b)| format!("`{a}` → `{b}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            body.push('\n');
        }
        body.push_str(&format!("\n> {}\n", outcome.staleness_note));
        if !outcome.warnings.is_empty() {
            let parts: Vec<String> = outcome.warnings.iter().map(|w| w.to_string()).collect();
            body.push_str(&format!("\n- Warnings: {}\n", parts.join("; ")));
        }
        body.push_str(&super::render_mem_changed_block(&mem_changed));
        print_markdown(&body);
    }
    Ok(())
}

/// The `expected_hash` for the write: the flag value, or the live hash
/// under `--auto-hash` / `--force`; `None` on a dry run, which skips the
/// optimistic lock by contract.
fn resolve_expected_hash(
    engine: &memstead_base::Engine,
    id: &EntityId,
    args: &Args,
) -> anyhow::Result<Option<String>> {
    if args.dry_run {
        return Ok(None);
    }
    if args.auto_hash || args.force {
        return Ok(Some(
            engine
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
                .clone(),
        ));
    }
    args.expected_hash
        .clone()
        .filter(|h| !h.is_empty())
        .map(Some)
        .ok_or_else(|| {
            CliError::new(
                ExitKind::Validation,
                crate::HASH_FLAG_REQUIRED_CODE,
                "missing --expected-hash. Read the entity first (memstead entity <id>) and pass its `_hash`, \
                 or use --auto-hash / --force / --dry-run.",
            )
            .into()
        })
}
