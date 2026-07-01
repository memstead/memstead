//! `memstead update` — strict-by-default entity update.
//!
//! Hash handling offers three opt-ins:
//!
//! * **Default (strict).** `--expected-hash <h>` must be supplied. Matches
//!   MCP's `memstead_update` contract. Safe for scripts, CI, pre-commit hooks.
//! * **`--auto-hash`.** Refetch the current hash immediately before writing.
//!   Ergonomic for one-off interactive edits; the user accepts the race window.
//! * **`--force`.** Skip the hash check entirely. Explicit opt-out.
//!
//! Only one of the three may be used per invocation.

use std::path::PathBuf;

use clap::Parser;
use indexmap::IndexMap;
use serde::Deserialize;

use memstead_base::vcs::Actor;
use memstead_base::{EntityId, UpdateEntityArgs};
#[cfg(feature = "mem-repo")]
use memstead_base::ops::PatchArg;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

#[derive(Parser, Debug)]
pub struct Args {
    /// Full entity ID (e.g. `specs--my-entity`). Required unless `--from` is given.
    pub id: Option<String>,

    /// Hash from `memstead entity <id>` (the `_hash` field). Required unless
    /// `--auto-hash` or `--force` is given.
    #[arg(long = "expected-hash", value_name = "HASH")]
    pub expected_hash: Option<String>,

    /// Refetch the current hash immediately before writing.
    /// Convenient for interactive use; accepts the race window between
    /// the refetch and the write.
    #[arg(long, conflicts_with_all = ["expected_hash", "force"])]
    pub auto_hash: bool,

    /// Skip the hash check entirely (explicit overwrite).
    #[arg(long, conflicts_with_all = ["expected_hash", "auto_hash"])]
    pub force: bool,

    /// Replace section content: repeatable `--section key=value`. Body
    /// wiki-links must take slug-form (`[[idempotency]]`, not the
    /// title-case `[[Idempotency]]`) — a non-slug target refuses with
    /// `INVALID_WIKI_LINK_TARGET` carrying a `proposed_slug` to retry with.
    #[arg(long = "section", value_name = "KEY=VALUE")]
    pub sections: Vec<String>,

    /// Append to section content: repeatable `--append key=value`.
    #[arg(long = "append", value_name = "KEY=VALUE")]
    pub append: Vec<String>,

    /// Find-and-replace inside a section: repeatable `--patch key=OLD=>NEW`.
    /// Use `=>` (two chars) as the separator between old and new. Exact match
    /// of the first occurrence; use `--patch-all` to replace every occurrence.
    #[arg(long = "patch", value_name = "KEY=OLD=>NEW")]
    pub patch: Vec<String>,

    /// Replace every occurrence of OLD in the section — sibling of `--patch`.
    /// Repeatable `--patch-all key=OLD=>NEW`.
    #[arg(long = "patch-all", value_name = "KEY=OLD=>NEW")]
    pub patch_all: Vec<String>,

    /// Metadata field: repeatable `--metadata key=value`.
    #[arg(long = "metadata", value_name = "KEY=VALUE")]
    pub metadata: Vec<String>,

    /// Remove a metadata field: repeatable `--metadata-unset KEY`. Silent
    /// no-op if the key is absent; errors on read-only fields (mem/id/type
    /// plus the engine-stamped created_date/last_modified) or
    /// schema-required fields.
    #[arg(long = "metadata-unset", value_name = "KEY")]
    pub metadata_unset: Vec<String>,

    /// Atomic batched relation declaration: repeatable
    /// `--declare-relations REL_TYPE:TARGET_ID`. Each entry is
    /// validated like an individual `memstead relate` call (schema-shape,
    /// cross-mem policy, target-id grammar) and appended to the
    /// entity's relations BEFORE the strict wiki-link/relation
    /// validator runs. Lets the agent add `[[target]]` body
    /// wiki-links AND declare the backing relation in one
    /// `memstead update` call without an interleaved `memstead relate`.
    /// Absent Write-mem targets are auto-stubbed identically to
    /// `memstead relate`'s add path. Each successful declaration is
    /// echoed in the response's `relations_declared` (with
    /// `target_was_stubbed` flagging the auto-stub case).
    #[arg(long = "declare-relations", value_name = "REL_TYPE:TARGET_ID")]
    pub declare_relations: Vec<String>,

    /// Preview what would change without writing.
    #[arg(long)]
    pub dry_run: bool,

    /// JSON file matching MCP `memstead_update` args shape. When set, flags
    /// above except the hash-mode flags are ignored.
    #[arg(long = "from", value_name = "FILE")]
    pub from: Option<PathBuf>,

    /// Agent-authored provenance note (≤280 chars). When
    /// `[mutations].require_notes = true` a missing note adds a
    /// `NOTE_MISSING` warning.
    #[arg(long)]
    pub note: Option<String>,
}

/// On-disk JSON payload shape — mirrors MCP `UpdateParams` + hash flags.
/// `expected_hash` inside the file takes effect only in strict mode.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdatePayload {
    id: String,
    expected_hash: Option<String>,
    #[serde(default)]
    sections: IndexMap<String, String>,
    #[serde(default)]
    append_sections: IndexMap<String, String>,
    #[serde(default)]
    patch_sections: IndexMap<String, PatchPayload>,
    #[serde(default)]
    metadata: IndexMap<String, String>,
    #[serde(default)]
    metadata_unset: Vec<String>,
    #[serde(default)]
    declare_relations: Vec<DeclareRelationPayload>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(feature = "mem-repo"), allow(dead_code))]
struct DeclareRelationPayload {
    /// Target entity id (`mem--slug` or cross-mem form).
    to: String,
    /// Relationship type — case-insensitive on input; engine
    /// canonicalises to UPPER_SNAKE_CASE.
    rel_type: String,
    /// Optional per-edge description. Validated against the rel-type's
    /// `per_edge_description` posture in the engine.
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(feature = "mem-repo"), allow(dead_code))]
struct PatchPayload {
    old: String,
    new: String,
    #[serde(default)]
    all: bool,
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
        let parsed: UpdatePayload = serde_json::from_slice(&bytes).map_err(|e| {
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
        let id = args.id.clone().ok_or_else(|| {
            CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                "missing entity ID (or pass --from <file.json>)",
            )
        })?;
        UpdatePayload {
            id,
            expected_hash: args.expected_hash.clone(),
            sections: parse_kv_list(&args.sections, "--section")?,
            append_sections: parse_kv_list(&args.append, "--append")?,
            patch_sections: parse_patch_list_combined(&args.patch, &args.patch_all)?,
            metadata: parse_kv_list(&args.metadata, "--metadata")?,
            metadata_unset: args.metadata_unset.clone(),
            declare_relations: parse_declare_relations(&args.declare_relations)?,
            dry_run: args.dry_run,
        }
    };

    let entity_id = EntityId::canonical(&payload.id);

    match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(mut engine) => {
            let expected_hash = resolve_hash_mem_repo(
                &engine,
                &entity_id,
                payload.expected_hash,
                args.auto_hash,
                args.force,
            )?;

            let patch_sections = payload
                .patch_sections
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        PatchArg {
                            old: v.old,
                            new: v.new,
                            all: v.all,
                        },
                    )
                })
                .collect();

            let declare_relations: Vec<memstead_base::ops::RelateArg> = payload
                .declare_relations
                .iter()
                .map(|r| memstead_base::ops::RelateArg {
                    to: EntityId::canonical(&r.to),
                    rel_type: r.rel_type.clone(),
                    description: r.description.clone(),
                })
                .collect();
            let update_args = UpdateEntityArgs {
                id: entity_id.clone(),
                expected_hash: Some(expected_hash),
                sections: payload.sections,
                append_sections: payload.append_sections,
                patch_sections,
                metadata: payload.metadata,
                metadata_unset: payload.metadata_unset,
                dry_run: payload.dry_run,
                declare_relations,
            relations_unset: Vec::new(),
        };

            let result = engine
                .update_entity_with_ctx(
                    update_args,
                    &crate::setup::cli_ctx_with_note(args.note.clone()),
                )
                .map_err(CliError::from_engine_op)?;
            let mem_changed = engine.take_mem_changed_notices();

            if ctx.json {
                let mut body =
                    serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
                super::merge_mem_changed_json(&mut body, &mem_changed);
                print_json(&body)?;
            } else {
                let header = if payload.dry_run {
                    format!("# Dry-run `{}`", result.id)
                } else {
                    format!("# Updated `{}`", result.id)
                };
                let sections_line = render_section_mutations(&result.modified_sections);
                let metadata_line = render_metadata_mutations(&result.modified_metadata);
                let mut body = format!("{header}\n\n- Title: {}", result.title);
                if let Some(line) = sections_line {
                    body.push_str(&format!("\n- Sections: {line}"));
                }
                if let Some(line) = metadata_line {
                    body.push_str(&format!("\n- Metadata: {line}"));
                }
                if !result.relations_declared.is_empty() {
                    let parts: Vec<String> = result
                        .relations_declared
                        .iter()
                        .map(|r| {
                            let stubbed_tag = if r.target_was_stubbed { " (stubbed)" } else { "" };
                            format!("{} → {}{}", r.rel_type, r.target, stubbed_tag)
                        })
                        .collect();
                    body.push_str(&format!("\n- Relations declared: {}", parts.join(", ")));
                }
                if !result.orphan_stubs_removed.is_empty() {
                    let ids: Vec<String> = result
                        .orphan_stubs_removed
                        .iter()
                        .map(|i| i.to_string())
                        .collect();
                    body.push_str(&format!("\n- Orphan stubs GC'd: {}", ids.join(", ")));
                }
                if !result.warnings.is_empty() {
                    let parts: Vec<String> =
                        result.warnings.iter().map(|w| w.to_string()).collect();
                    body.push_str(&format!("\n- Warnings: {}", parts.join("; ")));
                }
                body.push_str(&format!("\n- Hash: `{}`", result.content_hash));
                body.push_str(&super::render_mem_changed_block(&mem_changed));
                print_markdown(&body);
            }
        }
        CliEngine::Filesystem(mut engine) => {
            // The filesystem-mem `memstead_update` surface is intentionally
            // smaller than mem-repo's: whole-section replacement,
            // metadata set, and metadata unset are honoured;
            // append_sections / patch_sections / dry_run are not yet
            // wired on the filesystem engine. Surface that as a clear
            // validation error rather than silently dropping the flags.
            if !payload.append_sections.is_empty() {
                return Err(CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    "--append is not yet supported on filesystem-mem `memstead update`",
                )
                .into());
            }
            if !payload.patch_sections.is_empty() {
                return Err(CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    "--patch / --patch-all are not yet supported on filesystem-mem `memstead update`",
                )
                .into());
            }
            if payload.dry_run {
                return Err(CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    "--dry-run is not yet supported on filesystem-mem `memstead update`",
                )
                .into());
            }

            let expected_hash = resolve_hash_filesystem(
                &engine,
                &entity_id,
                payload.expected_hash,
                args.auto_hash,
                args.force,
            )?;

            let declare_relations: Vec<memstead_base::ops::RelateArg> = payload
                .declare_relations
                .iter()
                .map(|r| memstead_base::ops::RelateArg {
                    to: EntityId::canonical(&r.to),
                    rel_type: r.rel_type.clone(),
                    description: r.description.clone(),
                })
                .collect();
            let update_args = UpdateEntityArgs {
                id: entity_id.clone(),
                expected_hash: Some(expected_hash),
                sections: payload.sections,
                // CLI's update surface doesn't accept
                // append_sections / patch_sections on its wire
                // today; pass empty.
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                metadata: payload.metadata,
                metadata_unset: payload.metadata_unset,
                declare_relations,
                dry_run: false,
            relations_unset: Vec::new(),
        };
            let outcome = engine
                .update_entity(update_args, Actor::Cli, None, args.note.as_deref())
                .map_err(CliError::from_engine_op)?;

            if ctx.json {
                let relations_declared: Vec<serde_json::Value> = outcome
                    .relations_declared
                    .iter()
                    .map(|r| serde_json::json!({
                        "rel_type": r.rel_type,
                        "target": r.target.to_string(),
                        "target_was_stubbed": r.target_was_stubbed,
                    }))
                    .collect();
                print_json(&serde_json::json!({
                    "id": outcome.id.as_ref(),
                    "file_path": outcome.file_path,
                    "_hash": outcome.content_hash,
                    "modified_sections": outcome.modified_sections.replaced,
                    "modified_metadata_set": outcome.modified_metadata.set,
                    "modified_metadata_unset": outcome.modified_metadata.unset,
                    "relations_declared": relations_declared,
                    // Engine-emitted warnings (e.g. `NOTE_MISSING` under
                    // `[mutations].require_notes`) ride the response.
                    "warnings": outcome.warnings,
                    "orphan_stubs_removed": outcome
                        .orphan_stubs_removed
                        .iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>(),
                }))?;
            } else {
                let mut body = format!("# Updated `{}`", outcome.id);
                if !outcome.modified_sections.replaced.is_empty() {
                    let parts: Vec<String> = outcome
                        .modified_sections
                        .replaced
                        .iter()
                        .map(|k| format!("{k} (replaced)"))
                        .collect();
                    body.push_str(&format!("\n- Sections: {}", parts.join(", ")));
                }
                if !outcome.modified_metadata.set.is_empty()
                    || !outcome.modified_metadata.unset.is_empty()
                {
                    let mut parts = Vec::new();
                    for k in &outcome.modified_metadata.set {
                        parts.push(format!("{k} (set)"));
                    }
                    for k in &outcome.modified_metadata.unset {
                        parts.push(format!("{k} (unset)"));
                    }
                    body.push_str(&format!("\n- Metadata: {}", parts.join(", ")));
                }
                if !outcome.relations_declared.is_empty() {
                    let parts: Vec<String> = outcome
                        .relations_declared
                        .iter()
                        .map(|r| {
                            let stubbed_tag = if r.target_was_stubbed { " (stubbed)" } else { "" };
                            format!("{} → {}{}", r.rel_type, r.target, stubbed_tag)
                        })
                        .collect();
                    body.push_str(&format!("\n- Relations declared: {}", parts.join(", ")));
                }
                if !outcome.orphan_stubs_removed.is_empty() {
                    let ids: Vec<String> = outcome
                        .orphan_stubs_removed
                        .iter()
                        .map(|i| i.to_string())
                        .collect();
                    body.push_str(&format!("\n- Orphan stubs GC'd: {}", ids.join(", ")));
                }
                if !outcome.warnings.is_empty() {
                    let parts: Vec<String> =
                        outcome.warnings.iter().map(|w| w.to_string()).collect();
                    body.push_str(&format!("\n- Warnings: {}", parts.join("; ")));
                }
                body.push_str(&format!("\n- Hash: `{}`", outcome.content_hash));
                print_markdown(&body);
            }
        }
    }
    Ok(())
}

/// Render `modified_sections` as `identity (replaced), constraints (appended)`.
/// Returns `None` when nothing was modified, letting the caller omit the line.
#[cfg(feature = "mem-repo")]
fn render_section_mutations(m: &memstead_git_branch::ModifiedSections) -> Option<String> {
    let mut parts = Vec::new();
    for k in &m.replaced {
        parts.push(format!("{k} (replaced)"));
    }
    for k in &m.appended {
        parts.push(format!("{k} (appended)"));
    }
    for k in &m.patched {
        parts.push(format!("{k} (patched)"));
    }
    if parts.is_empty() { None } else { Some(parts.join(", ")) }
}

/// Render `modified_metadata` as `level (set), tags (unset)`. `None` when empty.
#[cfg(feature = "mem-repo")]
fn render_metadata_mutations(m: &memstead_git_branch::ModifiedMetadata) -> Option<String> {
    let mut parts = Vec::new();
    for k in &m.set {
        parts.push(format!("{k} (set)"));
    }
    for k in &m.unset {
        parts.push(format!("{k} (unset)"));
    }
    if parts.is_empty() { None } else { Some(parts.join(", ")) }
}

/// Resolve the hash the update will be issued with.
///
/// * `--force` and `--auto-hash` both refetch from the engine's in-memory
///   store. Because the CLI initializes a fresh engine per invocation, the
///   loaded hash matches the on-disk content as long as no concurrent writer
///   changed the file between load and update (race window is microseconds).
///   The two flags exist to encode user intent — `--auto-hash` for "I didn't
///   bother reading the entity first," `--force` for "I intend to overwrite
///   regardless of what's there."
/// * Strict (default) → use the explicit `--expected-hash` / JSON field, else error.
#[cfg(feature = "mem-repo")]
fn resolve_hash_mem_repo(
    engine: &memstead_base::Engine,
    id: &EntityId,
    explicit: Option<String>,
    auto_hash: bool,
    force: bool,
) -> anyhow::Result<String> {
    if auto_hash || force {
        let entity = engine.get_entity(id).ok_or_else(|| {
            CliError::new(
                ExitKind::NotFound,
                "ENTITY_NOT_FOUND",
                format!("entity not found: {id}"),
            )
            .with_details(serde_json::json!({ "id": id.to_string() }))
        })?;
        return Ok(entity.content_hash.clone());
    }
    require_explicit_hash(explicit)
}

/// Filesystem-mem counterpart of [`resolve_hash_mem_repo`]. Same
/// semantics; differs only in the engine accessor type.
fn resolve_hash_filesystem(
    engine: &memstead_base::Engine,
    id: &EntityId,
    explicit: Option<String>,
    auto_hash: bool,
    force: bool,
) -> anyhow::Result<String> {
    if auto_hash || force {
        let entity = engine.get_entity(id).ok_or_else(|| {
            CliError::new(
                ExitKind::NotFound,
                "ENTITY_NOT_FOUND",
                format!("entity not found: {id}"),
            )
            .with_details(serde_json::json!({ "id": id.to_string() }))
        })?;
        return Ok(entity.content_hash.clone());
    }
    require_explicit_hash(explicit)
}

fn require_explicit_hash(explicit: Option<String>) -> anyhow::Result<String> {
    match explicit {
        Some(h) if !h.is_empty() => Ok(h),
        _ => Err(CliError::new(
            ExitKind::Validation,
            crate::HASH_FLAG_REQUIRED_CODE,
            "missing --expected-hash. Read the entity first (memstead entity <id>) and pass its `_hash`, \
             or use --auto-hash for one-off interactive updates, or --force to overwrite.",
        )
        .into()),
    }
}

/// Parse repeatable `--declare-relations REL_TYPE:TARGET_ID` into
/// the structured payload used downstream. Splits on the FIRST `:`
/// so the target id can itself contain colons (cross-mem
/// `[[mem:slug]]` form). The rel-type half must match the
/// `[A-Za-z][A-Za-z_]*` grammar already used by `memstead relate`;
/// validation against the workspace's schema vocabulary happens at
/// the engine layer.
fn parse_declare_relations(items: &[String]) -> anyhow::Result<Vec<DeclareRelationPayload>> {
    let mut out = Vec::with_capacity(items.len());
    for raw in items {
        let (rel_type, target) = raw.split_once(':').ok_or_else(|| {
            CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!(
                    "--declare-relations: expected REL_TYPE:TARGET_ID, got `{raw}`"
                ),
            )
        })?;
        if rel_type.is_empty() || target.is_empty() {
            return Err(CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!(
                    "--declare-relations: REL_TYPE and TARGET_ID must both be non-empty, got `{raw}`"
                ),
            )
            .into());
        }
        out.push(DeclareRelationPayload {
            to: target.to_string(),
            rel_type: rel_type.to_string(),
            description: None,
        });
    }
    Ok(out)
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

fn parse_patch_list_combined(
    first_only: &[String],
    all: &[String],
) -> anyhow::Result<IndexMap<String, PatchPayload>> {
    let mut out = IndexMap::with_capacity(first_only.len() + all.len());
    for (items, flag, replace_all) in [
        (first_only, "--patch", false),
        (all, "--patch-all", true),
    ] {
        for raw in items {
            let (key, rest) = raw.split_once('=').ok_or_else(|| {
                CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    format!("{flag}: expected KEY=OLD=>NEW, got `{raw}`"),
                )
            })?;
            let (old, new) = rest.split_once("=>").ok_or_else(|| {
                CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    format!("{flag}: expected KEY=OLD=>NEW (missing `=>`), got `{raw}`"),
                )
            })?;
            if out.contains_key(key) {
                return Err(CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    format!(
                        "duplicate patch for section `{key}` -- only one of --patch / --patch-all per section"
                    ),
                )
                .into());
            }
            out.insert(
                key.to_string(),
                PatchPayload {
                    old: old.to_string(),
                    new: new.to_string(),
                    all: replace_all,
                },
            );
        }
    }
    Ok(out)
}
