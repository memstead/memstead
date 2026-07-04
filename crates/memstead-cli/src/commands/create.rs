//! `memstead create` — create a new entity from flags or a JSON file.
//!
//! Mirrors `memstead_create` in the MCP surface. Two input modes:
//!
//! * **Flags.** `--title`, `--type` (required), plus repeatable
//!   `--section key=value`, `--metadata key=value`, `--relation type:to`.
//! * **JSON file.** `--from payload.json`. Same shape as `memstead_create`'s
//!   `CreateParams`.

use std::path::PathBuf;

use clap::Parser;
use indexmap::IndexMap;
use serde::Deserialize;

use memstead_base::CreateEntityArgs;
#[cfg(feature = "mem-repo")]
use memstead_base::EntityId;
#[cfg(feature = "mem-repo")]
use memstead_base::ops::RelateArg;
use memstead_base::vcs::Actor;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

#[derive(Parser, Debug)]
#[command(after_long_help = super::CREATE_AFTER_LONG_HELP)]
pub struct Args {
    /// Entity title. Required unless `--from` is given.
    #[arg(long)]
    pub title: Option<String>,

    /// Entity type (e.g. `spec`, `memo`, `concept`).
    /// Required unless `--from` is given.
    #[arg(long = "type")]
    pub entity_type: Option<String>,

    /// Mem name. Defaults to the first writable mem.
    #[arg(long)]
    pub mem: Option<String>,

    /// Section content: repeatable `--section key=value`. Body
    /// wiki-links must take slug-form (`[[idempotency]]`, not the
    /// title-case `[[Idempotency]]`) — a non-slug target refuses with
    /// `INVALID_WIKI_LINK_TARGET` carrying a `proposed_slug` to retry with.
    #[arg(long = "section", value_name = "KEY=VALUE")]
    pub sections: Vec<String>,

    /// Metadata override: repeatable `--metadata key=value`.
    #[arg(long = "metadata", value_name = "KEY=VALUE")]
    pub metadata: Vec<String>,

    /// Initial relationship: repeatable `--relation TYPE:target-id`.
    /// Mem-repo workspaces only — on filesystem mems this refuses;
    /// use `memstead relate` after creation there.
    #[arg(long = "relation", value_name = "TYPE:TARGET")]
    pub relations: Vec<String>,

    /// JSON file matching the MCP `memstead_create` args shape. If set,
    /// all `--title` / `--type` / `--section` / `--metadata` / `--relation`
    /// flags are ignored (the file is the single source of truth).
    /// The JSON type field is `entity_type` (not `type`), matching the
    /// response envelopes — a previous `--json` response pipes back in
    /// unchanged.
    #[arg(long = "from", value_name = "FILE")]
    pub from: Option<PathBuf>,

    /// Preview only — validate and compute the result without writing to
    /// disk, mutating the store, or producing a commit. Response carries
    /// the prospective id / file_path / content_hash plus any warnings.
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Agent-authored provenance note (≤280 chars, one sentence
    /// describing why this mutation happened). Lands in the per-mem
    /// commit body between the mechanical subject line and the
    /// provenance trailers. When `[mutations].require_notes = true` in
    /// workspace config a missing note adds a `NOTE_MISSING` warning
    /// to the response (the mutation still commits). When `--from` also
    /// carries a `note`, this flag takes precedence.
    #[arg(long)]
    pub note: Option<String>,
}

/// On-disk JSON payload shape — mirrors MCP `CreateParams` exactly.
///
/// The type field is `entity_type`, matching the response envelopes
/// every read/write surface emits, so an agent can pipe a previous
/// `memstead create --json` response back through `--from` for a
/// follow-up create without renaming the field.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePayload {
    title: String,
    entity_type: String,
    mem: Option<String>,
    #[serde(default)]
    sections: IndexMap<String, String>,
    #[serde(default)]
    metadata: IndexMap<String, String>,
    #[serde(default)]
    relations: Vec<RelationPayload>,
    /// Agent-authored provenance note — matches the MCP `memstead_create`
    /// shape's `note`. Optional; the command-line `--note` takes
    /// precedence when both are supplied.
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(feature = "mem-repo"), allow(dead_code))]
struct RelationPayload {
    to: String,
    #[serde(rename = "type")]
    rel_type: String,
    #[serde(default)]
    description: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let payload = if let Some(ref file) = args.from {
        let bytes = std::fs::read(file).map_err(|e| {
            CliError::new(
                ExitKind::Generic,
                "INVALID_INPUT",
                format!("failed to read {}: {e}", file.display()),
            )
        })?;
        let parsed: CreatePayload = serde_json::from_slice(&bytes).map_err(|e| {
            CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!("invalid JSON in {}: {e}", file.display()),
            )
            .with_details(serde_json::json!({
                "path": file.display().to_string(),
                "parser_error": e.to_string(),
            }))
        })?;
        parsed
    } else {
        let title = args.title.clone().ok_or_else(|| {
            CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                "missing --title (or pass --from <file.json>)",
            )
        })?;
        let entity_type = args.entity_type.clone().ok_or_else(|| {
            CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                "missing --type (or pass --from <file.json>)",
            )
        })?;
        CreatePayload {
            title,
            entity_type,
            mem: args.mem.clone(),
            sections: parse_kv_list(&args.sections, "--section")?,
            metadata: parse_kv_list(&args.metadata, "--metadata")?,
            relations: parse_relation_list(&args.relations)?,
            note: None,
        }
    };

    // `--note` (CLI flag) wins over a `note` carried in the `--from`
    // payload when both are present; otherwise the file's note is used.
    let note = args.note.clone().or_else(|| payload.note.clone());

    match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(mut engine) => {
            let mem = match payload.mem {
                Some(v) => v,
                None => first_writable_mem(&engine)?,
            };

            let create_args = CreateEntityArgs {
                title: payload.title,
                mem,
                entity_type: payload.entity_type,
                sections: payload.sections,
                metadata: payload.metadata,
                relations: payload
                    .relations
                    .into_iter()
                    .map(|r| RelateArg {
                        to: EntityId::canonical(&r.to),
                        rel_type: r.rel_type,
                        description: r.description,
                    })
                    .collect(),
                dry_run: args.dry_run,
            };

            let result = engine
                .create_entity_with_ctx(create_args, &crate::setup::cli_ctx_with_note(note.clone()))
                .map_err(CliError::from_engine_op)?;
            let mem_changed = engine.take_mem_changed_notices();

            if ctx.json {
                let mut body = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
                super::merge_mem_changed_json(&mut body, &mem_changed);
                print_json(&body)?;
            } else {
                let warnings = if result.warnings.is_empty() {
                    String::new()
                } else {
                    let rendered: Vec<String> =
                        result.warnings.iter().map(ToString::to_string).collect();
                    let warnings_block =
                        format!("\n\n> warnings:\n> - {}", rendered.join("\n> - "),);
                    let guidance_block = super::render_type_guidance_block(&result.type_guidance);
                    format!("{warnings_block}{guidance_block}")
                };
                let incoming_block = if result.incoming.is_empty() {
                    String::new()
                } else {
                    let heading = if args.dry_run {
                        format!("Would adopt incoming edges ({})", result.incoming.len())
                    } else {
                        format!("Adopted incoming edges ({})", result.incoming.len())
                    };
                    let rows: Vec<String> = result
                        .incoming
                        .iter()
                        .map(|r| {
                            format!("- {} --[{}]--> (this) [{}]", r.from, r.rel_type, r.source)
                        })
                        .collect();
                    format!("\n\n## {}\n\n{}", heading, rows.join("\n"))
                };
                let title_heading = if args.dry_run {
                    format!("Dry run — would create `{}`", result.id)
                } else {
                    format!("Created `{}`", result.id)
                };
                let mem_changed_block = super::render_mem_changed_block(&mem_changed);
                print_markdown(&format!(
                    "# {}\n\n- Title: {}\n- Mem: {}\n- File: {}\n- Hash: `{}`{}{}{}",
                    title_heading,
                    result.title,
                    result.mem,
                    result.file_path,
                    result.content_hash,
                    warnings,
                    incoming_block,
                    mem_changed_block,
                ));
            }
        }
        CliEngine::Filesystem(mut engine) => {
            // Filesystem-mem `memstead create` accepts `--mem` for shape
            // parity (matches mem-repo CLI), but the engine is single-
            // mem; an explicit `--mem` mismatch with the workspace's
            // pinned mem errors out so the user sees the misconfig
            // rather than a silent no-op.
            let workspace_mem = engine
                .mem_names()
                .into_iter()
                .next()
                .map(String::from)
                .unwrap_or_default();
            if let Some(requested) = payload.mem.as_deref()
                && requested != workspace_mem
            {
                return Err(CliError::new(
                        ExitKind::NotFound,
                        "UNKNOWN_MEM",
                        format!(
                            "filesystem-mem is single-mem: workspace mem is `{workspace_mem}`, request specified `{requested}`"
                        ),
                    )
                    .into());
            }
            // `--relation` and `--dry-run` are not yet honoured on the
            // filesystem path — the unified `Engine::create_entity`
            // surface accepts neither. Surface that as a clear
            // validation error rather than silently dropping the flags.
            if !payload.relations.is_empty() {
                return Err(CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    "--relation is not yet supported on filesystem-mem `memstead create` — use `memstead relate` after creation",
                )
                .into());
            }
            if args.dry_run {
                return Err(CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    "--dry-run is not yet supported on filesystem-mem `memstead create`",
                )
                .into());
            }

            let create_args = CreateEntityArgs {
                mem: workspace_mem,
                title: payload.title.clone(),
                entity_type: payload.entity_type,
                sections: payload.sections,
                metadata: payload.metadata,
                relations: Vec::new(),
                dry_run: false,
            };
            let outcome = engine
                .create_entity(create_args, Actor::Cli, None, note.as_deref())
                .map_err(CliError::from_engine_op)?;

            if ctx.json {
                // WarningHint's Serialize impl produces the
                // `{code, message, details}` envelope that full
                // already used, so the wire shape is unchanged.
                print_json(&serde_json::json!({
                    "id": outcome.id.as_ref(),
                    "title": payload.title,
                    "file_path": outcome.file_path,
                    "_hash": outcome.content_hash,
                    "warnings": outcome.warnings,
                    "type_guidance": outcome.type_guidance,
                }))?;
            } else {
                let warnings = if outcome.warnings.is_empty() {
                    String::new()
                } else {
                    // WarningHint's Display impl renders human-
                    // readable text per variant.
                    let rendered: Vec<String> =
                        outcome.warnings.iter().map(|w| w.to_string()).collect();
                    let warnings_block =
                        format!("\n\n> warnings:\n> - {}", rendered.join("\n> - "),);
                    let guidance_block = super::render_type_guidance_block(&outcome.type_guidance);
                    format!("{warnings_block}{guidance_block}")
                };
                print_markdown(&format!(
                    "# Created `{}`\n\n- Title: {}\n- Mem: {}\n- File: {}\n- Hash: `{}`{}",
                    outcome.id,
                    payload.title,
                    outcome.id.mem(),
                    outcome.file_path,
                    outcome.content_hash,
                    warnings,
                ));
            }
        }
    }
    Ok(())
}

fn parse_kv_list(items: &[String], flag: &str) -> anyhow::Result<IndexMap<String, String>> {
    let mut out = IndexMap::with_capacity(items.len());
    for raw in items {
        let (k, v) = raw.split_once('=').ok_or_else(|| {
            CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!("{flag}: expected KEY=VALUE, got `{raw}`"),
            )
        })?;
        out.insert(k.to_string(), v.to_string());
    }
    Ok(out)
}

fn parse_relation_list(items: &[String]) -> anyhow::Result<Vec<RelationPayload>> {
    let mut out = Vec::with_capacity(items.len());
    for raw in items {
        let (rel_type, to) = raw.split_once(':').ok_or_else(|| {
            CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!("--relation: expected TYPE:target-id, got `{raw}`"),
            )
        })?;
        out.push(RelationPayload {
            rel_type: rel_type.to_string(),
            to: to.to_string(),
            description: None,
        });
    }
    Ok(out)
}

#[cfg(feature = "mem-repo")]
fn first_writable_mem(engine: &memstead_base::Engine) -> anyhow::Result<String> {
    // Resolve through the shared stable-default contract so the CLI and
    // MCP omitted-`mem` paths always agree: the first writable mount in
    // declaration order — the seed mem — not an alphabetically-first or
    // set-order pick that shifts when an unrelated mem is added.
    match engine.default_writable_mem() {
        Some(name) => Ok(name.to_string()),
        None => Err(CliError::new(
            ExitKind::Generic,
            "NO_WRITABLE_MEM",
            "no writable mem loaded — pass --mem <name>",
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `--from` payload accepts a top-level `note`, matching the MCP
    /// `memstead_create` shape the help text claims parity with. A payload
    /// without `note` still deserialises (the field is optional).
    #[test]
    fn create_payload_accepts_optional_note() {
        let with_note: CreatePayload =
            serde_json::from_str(r#"{"title":"X","entity_type":"spec","note":"why this landed"}"#)
                .expect("payload with note must parse");
        assert_eq!(with_note.note.as_deref(), Some("why this landed"));

        let without: CreatePayload = serde_json::from_str(r#"{"title":"X","entity_type":"spec"}"#)
            .expect("note-less payload must still parse");
        assert!(without.note.is_none());
    }

    /// `--note` (CLI flag) takes precedence over a `note` in the file;
    /// the file's note is used only when the flag is absent.
    #[test]
    fn cli_note_takes_precedence_over_file_note() {
        let cli = Some("from-flag".to_string());
        let file = Some("from-file".to_string());
        assert_eq!(
            cli.clone().or_else(|| file.clone()).as_deref(),
            Some("from-flag")
        );
        assert_eq!(None.or_else(|| file.clone()).as_deref(), Some("from-file"));
    }
}
