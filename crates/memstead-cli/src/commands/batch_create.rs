//! `memstead batch-create --from <file.json>` — create many entities in
//! one call: one workspace load, one commit per touched mem,
//! all-or-nothing with report-all refusals.
//!
//! Each entry is the same shape the single-entity `create --from`
//! accepts (minus `dry_run` — the batch family has no dry-run, matching
//! `batch-update`), and carries its own provenance `note`. There is
//! deliberately no batch-level note flag.
//!
//! Intra-batch references resolve as REAL targets: an entry's
//! `relations` (and body wiki-links) may point at entities created by
//! sibling entries in the same batch — cycles included where the
//! schema permits them — with full target-type shape validation and no
//! transient stubs.
//!
//! ```json
//! { "creates": [
//!     { "title": "Alpha", "entity_type": "spec",
//!       "sections": { "identity": "..." },
//!       "relations": [ { "to": "specs--beta", "type": "USES" } ],
//!       "note": "why alpha exists" },
//!     { "title": "Beta", "entity_type": "spec",
//!       "sections": { "identity": "..." } }
//! ] }
//! ```

use std::path::PathBuf;

use clap::Parser;
use indexmap::IndexMap;

use serde::Deserialize;

use memstead_base::EntityId;
use memstead_base::ops::RelateArg;
use memstead_base::{CreateEntityArgs, vcs::Actor};

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::CliContext;

#[derive(Parser, Debug)]
pub struct Args {
    /// JSON file with a top-level `creates: [...]` array.
    #[arg(long = "from", value_name = "FILE")]
    pub from: PathBuf,
}

/// Per-entry payload — the single `create --from` shape, per entry.
/// `id` is tolerated for template symmetry and only *checked* against
/// the title-derived slug (same contract as single create);
/// `expected_hash` is tolerated and ignored (nothing exists yet to
/// compare against). `dry_run` is deliberately NOT accepted: the batch
/// family has no dry-run (`batch-update` set the contract), so the key
/// refuses as unknown rather than silently not previewing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryPayload {
    title: String,
    entity_type: String,
    #[serde(default)]
    mem: Option<String>,
    #[serde(default)]
    sections: IndexMap<String, String>,
    #[serde(default)]
    metadata: IndexMap<String, String>,
    #[serde(default)]
    relations: Vec<RelationPayload>,
    #[serde(default)]
    anchors: Vec<memstead_base::anchor::AnchorInput>,
    /// Agent-authored provenance note for THIS entry's commit record —
    /// mirrors `batch-update`'s per-entry note handling exactly.
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    expected_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelationPayload {
    to: String,
    #[serde(rename = "type")]
    rel_type: String,
    #[serde(default)]
    description: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let entry_values = super::batch::parse_batch_envelope(&args.from, "creates")?;

    let mut entries: Vec<EntryPayload> = Vec::with_capacity(entry_values.len());
    for (idx, entry_value) in entry_values.into_iter().enumerate() {
        match serde_json::from_value::<EntryPayload>(entry_value) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                return Err(CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    format!("entry {idx}: invalid shape — {e}"),
                )
                .with_details(serde_json::json!({
                    "entry_index": idx,
                    "parser_error": e.to_string(),
                }))
                .into());
            }
        }
    }

    let mut engine = crate::setup::full_engine(ctx)?;

    let creates: Vec<(CreateEntityArgs, Option<String>)> = entries
        .into_iter()
        .enumerate()
        .map(|(idx, entry)| build_create_args(&engine, idx, entry))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let result = engine
        .batch_create(creates, Actor::Cli, None)
        .map_err(CliError::from_engine_op)?;
    // Reload-before-op runs inside `batch_create` for every mem the
    // batch touches; drain any `mem_changed` notice it stashed.
    let mem_changed = engine.take_mem_changed_notices();

    if result.applied {
        if ctx.json {
            let mut body = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
            crate::commands::merge_mem_changed_json(&mut body, &mem_changed);
            print_json(&body)?;
        } else {
            let mut md = super::batch::render_batch_markdown("create", &result);
            md.push_str(&crate::commands::render_mem_changed_block(&mem_changed));
            print_markdown(&md);
        }
        return Ok(());
    }

    // Refused batch: standard error envelope with the full result on
    // `details` — same contract as `batch-update` (CLI F12).
    if !ctx.json {
        print_markdown(&super::batch::render_batch_markdown("create", &result));
    }
    Err(super::batch::batch_refused_error("create", &result).into())
}

/// Map a single JSON entry to the engine's [`CreateEntityArgs`],
/// resolving an omitted `mem` to the workspace's stable default
/// (first writable mount) and checking a template-symmetry `id`
/// against the title-derived slug — same contract as single create,
/// with the entry index in the refusal.
fn build_create_args(
    engine: &memstead_base::Engine,
    idx: usize,
    entry: EntryPayload,
) -> anyhow::Result<(CreateEntityArgs, Option<String>)> {
    let mem = match entry.mem {
        Some(v) => v,
        None => match engine.default_writable_mem() {
            Some(name) => name.to_string(),
            None => {
                return Err(CliError::new(
                    ExitKind::Generic,
                    "NO_WRITABLE_MEM",
                    format!("entry {idx}: no writable mem loaded — set `mem` in the entry"),
                )
                .into());
            }
        },
    };

    if let Some(template_id) = entry.id.as_deref() {
        let derived_slug = memstead_base::entity::id::validate_and_derive_slug(&entry.title)
            .map_err(|e| {
                CliError::new(
                    ExitKind::Validation,
                    "INVALID_TITLE",
                    format!("entry {idx}: {e}"),
                )
            })?;
        let slug_part = template_id
            .rsplit_once("--")
            .map(|(_, s)| s)
            .unwrap_or(template_id);
        if slug_part != derived_slug {
            return Err(CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!(
                    "entry {idx}: template `id` {template_id:?} does not match the id derived \
                     from the title (slug {derived_slug:?}) — create derives identity from the \
                     title; fix the template's id or title"
                ),
            )
            .into());
        }
    }

    let note = entry.note;
    Ok((
        CreateEntityArgs {
            anchors: entry.anchors,
            mem,
            title: entry.title,
            entity_type: entry.entity_type,
            sections: entry.sections,
            metadata: entry.metadata,
            relations: entry
                .relations
                .into_iter()
                .map(|r| RelateArg {
                    to: EntityId::canonical(&r.to),
                    rel_type: r.rel_type,
                    description: r.description,
                })
                .collect(),
            dry_run: false,
        },
        note,
    ))
}
