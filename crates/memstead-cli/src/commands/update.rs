//! `memstead update` — strict-by-default entity update.
//!
//! Hash handling offers three opt-ins:
//!
//! * **Default (strict).** `--expected-hash <h>` must be supplied for any
//!   update that CHANGES CONTENT. Matches MCP's `memstead_update` contract.
//!   Safe for scripts, CI, pre-commit hooks. An anchors-only update
//!   (`--anchor` / `--anchor-unset` and nothing else) needs none: anchors live
//!   outside the content hash, so the token would compare a value the write
//!   cannot move.
//! * **`--auto-hash`.** Refetch the current hash immediately before writing.
//!   Ergonomic for one-off interactive edits; the user accepts the race window.
//! * **`--force`.** Skip the hash check entirely. Explicit opt-out.
//!
//! Only one of the three may be used per invocation.

use std::path::PathBuf;

use clap::Parser;
use indexmap::IndexMap;
use serde::Deserialize;

#[cfg(feature = "mem-repo")]
use memstead_base::ops::PatchArg;
use memstead_base::vcs::Actor;
use memstead_base::{EntityId, UpdateEntityArgs};

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

#[derive(Parser, Debug)]
pub struct Args {
    /// Full entity ID (e.g. `specs--my-entity`). Required unless `--from` is given.
    pub id: Option<String>,

    /// Hash from `memstead entity <id>` (the `_hash` field). Required for any
    /// update that changes content, unless `--auto-hash` or `--force` is
    /// given. Not required for an anchors-only update (`--anchor` /
    /// `--anchor-unset` and nothing else), because anchors live outside the
    /// content hash and the token would compare a value the write cannot
    /// move. With `--from`, this flag overrides the file's `expected_hash`
    /// field and enforces CAS exactly as on the inline path.
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
    #[arg(long = "section", value_name = "KEY=VALUE", conflicts_with = "from")]
    pub sections: Vec<String>,

    /// Append to section content: repeatable `--append key=value`.
    #[arg(long = "append", value_name = "KEY=VALUE", conflicts_with = "from")]
    pub append: Vec<String>,

    /// Remove a section outright (heading and body): repeatable
    /// `--section-unset KEY`. The close gesture for a declared-but-empty
    /// heading with nothing to receive; silent no-op on an absent key.
    /// Refuses for a schema-required section (fill it instead), for
    /// `relationships`, and for a key also written in the same call.
    #[arg(long = "section-unset", value_name = "KEY", conflicts_with = "from")]
    pub section_unset: Vec<String>,

    /// Find-and-replace inside a section: repeatable `--patch key=OLD=>NEW`.
    /// Use `=>` (two chars) as the separator between old and new. Exact match
    /// of the first occurrence; use `--patch-all` to replace every occurrence.
    #[arg(long = "patch", value_name = "KEY=OLD=>NEW", conflicts_with = "from")]
    pub patch: Vec<String>,

    /// Replace every occurrence of OLD in the section — sibling of `--patch`.
    /// Repeatable `--patch-all key=OLD=>NEW`.
    #[arg(
        long = "patch-all",
        value_name = "KEY=OLD=>NEW",
        conflicts_with = "from"
    )]
    pub patch_all: Vec<String>,

    /// Metadata field: repeatable `--metadata key=value`.
    #[arg(long = "metadata", value_name = "KEY=VALUE", conflicts_with = "from")]
    pub metadata: Vec<String>,

    /// Remove a metadata field: repeatable `--metadata-unset KEY`. Silent
    /// no-op if the key is absent; errors on read-only fields (mem/id/type
    /// plus the engine-stamped created_date/last_modified) or
    /// schema-required fields.
    #[arg(long = "metadata-unset", value_name = "KEY", conflicts_with = "from")]
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
    #[arg(
        long = "declare-relations",
        value_name = "REL_TYPE:TARGET_ID",
        conflicts_with = "from"
    )]
    pub declare_relations: Vec<String>,

    /// Provenance anchor: repeatable `--anchor '<json>'`, each a JSON
    /// object of the anchor shape. Written into the mem-branch anchors
    /// sidecar in the same commit as the update; a malformed anchor
    /// refuses `INVALID_ANCHOR`. An update carrying only `--anchor` (no
    /// section/metadata change) still commits the sidecar. Conflicts with
    /// `--from` (the file's `anchors[]` is authoritative there).
    #[arg(long = "anchor", value_name = "JSON", conflicts_with = "from")]
    pub anchors: Vec<String>,

    /// Explicit anchor removal: repeatable `--anchor-unset '<json>'`, each
    /// a JSON object `{ "artifact": "…" }` optionally narrowed by
    /// `"grain"` and/or `"class"` — a bare artifact removes every anchor
    /// on it. Applied BEFORE the `--anchor` merge in the same commit
    /// (anchors merge; writing never removes an anchor not named here).
    /// Unsetting a nonexistent target is a no-op. A malformed selector
    /// refuses `INVALID_ANCHOR`. Conflicts with `--from` (the file's
    /// `anchors_unset[]` is authoritative there).
    #[arg(long = "anchor-unset", value_name = "JSON", conflicts_with = "from")]
    pub anchors_unset: Vec<String>,

    /// Preview what would change without writing. Applies on both the
    /// inline and `--from` paths; with `--from` it forces a dry run even
    /// when the file's `dry_run` field is absent or `false`.
    #[arg(long)]
    pub dry_run: bool,

    /// JSON file matching MCP `memstead_update` args shape. The file is the
    /// single source of the mutation content — the content flags
    /// (`--section` / `--append` / `--patch` / `--patch-all` / `--metadata` /
    /// `--metadata-unset` / `--declare-relations` / `--anchor` /
    /// `--anchor-unset`) conflict with `--from` rather than being silently
    /// ignored. The flags that DO apply
    /// alongside `--from`: the hash-mode flags (`--expected-hash`, which
    /// overrides the file's `expected_hash` field; `--auto-hash`; `--force`),
    /// `--dry-run` (forces a dry run even when the file says otherwise), and
    /// `--note`. Deliberately: `auto_hash` is NOT a payload field here
    /// (unlike `batch-update` entries) — a stored payload must not be able
    /// to disable optimistic locking; pass the `--auto-hash` FLAG beside
    /// `--from` for that.
    #[arg(long = "from", value_name = "FILE")]
    pub from: Option<PathBuf>,

    /// Agent-authored provenance note (≤280 chars). When
    /// `[mutations].require_notes = true` a missing note adds a
    /// `NOTE_MISSING` warning.
    #[arg(long)]
    pub note: Option<String>,
}

/// Parse repeatable `--anchor-unset '<json>'` values into the engine's
/// permissive `AnchorUnsetInput` shape — sibling of
/// [`super::create::parse_anchor_list`]. Only JSON-shape errors refuse
/// here; selector validation (missing artifact, unknown grain/class) is
/// the engine's typed `INVALID_ANCHOR`.
fn parse_anchor_unset_list(
    items: &[String],
) -> anyhow::Result<Vec<memstead_base::anchor::AnchorUnsetInput>> {
    let mut out = Vec::with_capacity(items.len());
    for raw in items {
        let unset: memstead_base::anchor::AnchorUnsetInput =
            serde_json::from_str(raw).map_err(|e| {
                CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    format!("--anchor-unset: expected a JSON selector object, got `{raw}`: {e}"),
                )
            })?;
        out.push(unset);
    }
    Ok(out)
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
    patch_sections: IndexMap<String, PatchesPayload>,
    #[serde(default)]
    sections_unset: Vec<String>,
    #[serde(default)]
    metadata: IndexMap<String, String>,
    #[serde(default)]
    metadata_unset: Vec<String>,
    #[serde(default)]
    declare_relations: Vec<DeclareRelationPayload>,
    /// Repair-shaped relation removals — matches the MCP `memstead_update`
    /// `relations_unset[]` shape (`[{ rel_type, target }]`). Accepted only
    /// when the entity currently fails conformance (the engine refuses
    /// `REPAIR_NOT_NEEDED` on a conformant entity); everyday edge
    /// detachment goes through `memstead relate --remove`. Until 2026-08-28
    /// this key was refused outright here while MCP honoured it — the
    /// response-shape asymmetry `agent-surfaces.md` forbids.
    #[serde(default)]
    relations_unset: Vec<RelationUnsetPayload>,
    /// Provenance anchors — matches the MCP `memstead_update` `anchors[]`
    /// shape; validated engine-side into a typed `INVALID_ANCHOR` refusal
    /// on malformed input. Merged into the entity's existing set (same
    /// `(artifact, grain, class)` triple replaces, otherwise appends).
    #[serde(default)]
    anchors: Vec<memstead_base::anchor::AnchorInput>,
    /// Explicit anchor removals — matches the MCP `memstead_update`
    /// `anchors_unset[]` shape; applied before the `anchors` merge.
    #[serde(default)]
    anchors_unset: Vec<memstead_base::anchor::AnchorUnsetInput>,
    #[serde(default)]
    dry_run: bool,
    /// Agent-authored provenance note — same semantics as
    /// `create --from`: the command-line `--note` wins when both are
    /// supplied. One JSON template can therefore feed both
    /// `create --from` and `update --from`. The optimistic-locking
    /// selectors (`auto_hash`, `force`) are deliberately flag-only: a
    /// stored payload must never be able to disable locking on a
    /// future run.
    #[serde(default)]
    note: Option<String>,
    /// Tolerated for template symmetry with `create --from` (one JSON
    /// document feeds both commands). Update cannot rename an entity,
    /// so a supplied `title` is only *checked*: a value differing from
    /// the entity's current title refuses with `INVALID_INPUT`
    /// pointing at `memstead rename` — never silently dropped.
    #[serde(default)]
    title: Option<String>,
    /// Tolerated for template symmetry with `create --from`; must
    /// match the entity's current type (update cannot retype —
    /// delete + create instead). A differing value refuses.
    #[serde(default)]
    entity_type: Option<String>,
    /// Tolerated for template symmetry with `create --from`; must
    /// match the mem encoded in the entity id (update cannot move an
    /// entity between mems). A differing value refuses.
    #[serde(default)]
    mem: Option<String>,
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

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct RelationUnsetPayload {
    /// Relationship type of the edge to remove (case-insensitive input;
    /// engine canonicalises).
    rel_type: String,
    /// Full target entity id of the edge to remove.
    target: String,
}

impl RelationUnsetPayload {
    fn into_arg(self) -> memstead_base::ops::RelationUnsetArg {
        memstead_base::ops::RelationUnsetArg {
            rel_type: self.rel_type,
            target: EntityId::canonical(&self.target),
        }
    }
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

/// One patch or a list of patches per section — the payload accepts both
/// (`{...}` and `[{...}, ...]`), mirroring the MCP wire; a list applies
/// in order against the section's evolving body.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
#[cfg_attr(not(feature = "mem-repo"), allow(dead_code))]
enum PatchesPayload {
    One(PatchPayload),
    Many(Vec<PatchPayload>),
}

impl PatchesPayload {
    #[cfg(feature = "mem-repo")]
    fn into_vec(self) -> Vec<PatchPayload> {
        match self {
            PatchesPayload::One(p) => vec![p],
            PatchesPayload::Many(v) => v,
        }
    }
}

/// Template-symmetry check against the live entity: a shared
/// create/update template may carry `title` / `entity_type`; update
/// can change neither, so a present-but-differing value refuses
/// instead of being silently dropped. Absent entity → skip (the
/// engine's own `ENTITY_NOT_FOUND` is the better error).
fn check_template_identity(
    entity: Option<&memstead_base::Entity>,
    payload_title: Option<&str>,
    payload_type: Option<&str>,
) -> Result<(), CliError> {
    let Some(entity) = entity else {
        return Ok(());
    };
    if let Some(t) = payload_title
        && t != entity.title
    {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "template `title` {t:?} differs from the entity's current title {:?} — \
                 update cannot rename; use `memstead rename`",
                entity.title
            ),
        ));
    }
    if let Some(ty) = payload_type
        && ty != entity.entity_type
    {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "template `entity_type` {ty:?} differs from the entity's current type {:?} — \
                 update cannot retype; delete + create instead",
                entity.entity_type
            ),
        ));
    }
    Ok(())
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let mut payload = if let Some(ref file) = args.from {
        let bytes = std::fs::read(file).map_err(|e| {
            CliError::new(
                ExitKind::Generic,
                "INVALID_INPUT",
                format!("failed to read {}: {e}", file.display()),
            )
        })?;
        let mut parsed: UpdatePayload = serde_json::from_slice(&bytes).map_err(|e| {
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
        // The non-content flags apply on the `--from` path exactly as on the
        // inline path (the content flags conflict at parse time): `--dry-run`
        // forces a dry run, and an explicit `--expected-hash` overrides the
        // file's `expected_hash` field. Neither is ever silently dropped.
        parsed.dry_run |= args.dry_run;
        if args.expected_hash.is_some() {
            parsed.expected_hash = args.expected_hash.clone();
        }
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
            sections_unset: args.section_unset.clone(),
            metadata: parse_kv_list(&args.metadata, "--metadata")?,
            metadata_unset: args.metadata_unset.clone(),
            declare_relations: parse_declare_relations(&args.declare_relations)?,
            // The repair-shaped removal is `--from`-only, like MCP's own
            // JSON-args shape — the inline flag surface stays everyday-sized.
            relations_unset: Vec::new(),
            anchors: super::create::parse_anchor_list(&args.anchors)?,
            anchors_unset: parse_anchor_unset_list(&args.anchors_unset)?,
            dry_run: args.dry_run,
            note: None,
            title: None,
            entity_type: None,
            mem: None,
        }
    };

    // `--note` (CLI flag) wins over a `note` carried in the `--from`
    // payload when both are present — same precedence as `create --from`.
    let note = args.note.clone().or_else(|| payload.note.clone());

    let entity_id = EntityId::canonical(&payload.id);

    // Template-symmetry consistency checks: a shared create/update
    // template may carry `title` / `entity_type` / `mem`. Update can
    // change none of them, so each present value must match the
    // entity id's mem (checkable here) — the title/type compare runs
    // against the live entity below, per engine flavour.
    if let Some(m) = payload.mem.as_deref()
        && m != entity_id.mem()
    {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "template `mem` {m:?} does not match the mem in id `{entity_id}` — update                  cannot move an entity between mems (delete + create instead)"
            ),
        )
        .into());
    }

    match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(mut engine) => {
            check_template_identity(
                engine.get_entity(&entity_id),
                payload.title.as_deref(),
                payload.entity_type.as_deref(),
            )?;
            // Resolved AFTER the args are assembled, because whether the
            // compare-and-swap token is required depends on the payload's own
            // shape and the engine owns that predicate
            // (consistency-sweep 03/04).
            let explicit_hash = payload.expected_hash.take();

            let patch_sections = payload
                .patch_sections
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        v.into_vec()
                            .into_iter()
                            .map(|v| PatchArg {
                                old: v.old,
                                new: v.new,
                                all: v.all,
                            })
                            .collect(),
                    )
                })
                .collect();

            let declare_relations: Vec<memstead_base::ops::RelateArg> = payload
                .declare_relations
                .iter()
                .map(|r| memstead_base::ops::RelateArg {
                    target: EntityId::canonical(&r.to),
                    rel_type: r.rel_type.clone(),
                    description: r.description.clone(),
                })
                .collect();
            let update_args = UpdateEntityArgs {
                anchors: payload.anchors,
                id: entity_id.clone(),
                expected_hash: None,
                sections: payload.sections,
                append_sections: payload.append_sections,
                patch_sections,
                sections_unset: payload.sections_unset.clone(),
                metadata: payload.metadata,
                metadata_unset: payload.metadata_unset,
                dry_run: payload.dry_run,
                declare_relations,
                relations_unset: payload
                    .relations_unset
                    .into_iter()
                    .map(RelationUnsetPayload::into_arg)
                    .collect(),
                anchors_unset: payload.anchors_unset,
            };
            let mut update_args = update_args;
            update_args.expected_hash = resolve_hash_mem_repo(
                &engine,
                &entity_id,
                explicit_hash,
                args.auto_hash,
                args.force,
                // `dry_run` joins the exemption because MCP's contract already
                // says dry-run bypasses ONLY the hash check, and it is the
                // documented stale-hash recovery path. Demanding a token here
                // while MCP does not is a surface divergence
                // (consistency-sweep 03/04).
                !update_args.changes_content() || update_args.dry_run,
            )?;

            let result = engine
                .update_entity_with_ctx(update_args, &crate::setup::cli_ctx_with_note(note.clone()))
                .map_err(CliError::from_engine_op)?;
            let mem_changed = engine.take_mem_changed_notices();

            if ctx.json {
                let mut body = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
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
                            let stubbed_tag = if r.target_was_stubbed {
                                " (stubbed)"
                            } else {
                                ""
                            };
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
            check_template_identity(
                engine.get_entity(&entity_id),
                payload.title.as_deref(),
                payload.entity_type.as_deref(),
            )?;
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

            // Resolved after the args, as on the mem-repo path above.
            let explicit_hash = payload.expected_hash.take();

            let declare_relations: Vec<memstead_base::ops::RelateArg> = payload
                .declare_relations
                .iter()
                .map(|r| memstead_base::ops::RelateArg {
                    target: EntityId::canonical(&r.to),
                    rel_type: r.rel_type.clone(),
                    description: r.description.clone(),
                })
                .collect();
            let update_args = UpdateEntityArgs {
                anchors: payload.anchors,
                id: entity_id.clone(),
                expected_hash: None,
                sections: payload.sections,
                // CLI's update surface doesn't accept
                // append_sections / patch_sections on its wire
                // today; pass empty.
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: payload.sections_unset,
                metadata: payload.metadata,
                metadata_unset: payload.metadata_unset,
                declare_relations,
                dry_run: false,
                relations_unset: payload
                    .relations_unset
                    .into_iter()
                    .map(RelationUnsetPayload::into_arg)
                    .collect(),
                anchors_unset: payload.anchors_unset,
            };
            let mut update_args = update_args;
            update_args.expected_hash = resolve_hash_filesystem(
                &engine,
                &entity_id,
                explicit_hash,
                args.auto_hash,
                args.force,
                !update_args.changes_content(),
            )?;
            let outcome = engine
                .update_entity(
                    update_args,
                    Actor::Cli,
                    Some(&crate::setup::cli_client_id()),
                    note.as_deref(),
                )
                .map_err(CliError::from_engine_op)?;

            if ctx.json {
                let relations_declared: Vec<serde_json::Value> = outcome
                    .relations_declared
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "rel_type": r.rel_type,
                            "target": r.target.to_string(),
                            "target_was_stubbed": r.target_was_stubbed,
                        })
                    })
                    .collect();
                print_json(&serde_json::json!({
                    "id": outcome.id.as_ref(),
                    "file_path": outcome.file_path,
                    "_hash": outcome.content_hash,
                    // Backend write identity — response-shape parity with
                    // the MCP filesystem flavour and the CLI's own
                    // relate/conflicts commands.
                    "write_id": outcome.write_id,
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
                            let stubbed_tag = if r.target_was_stubbed {
                                " (stubbed)"
                            } else {
                                ""
                            };
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
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
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
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
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
    exempt: bool,
) -> anyhow::Result<Option<String>> {
    if auto_hash || force {
        let entity = engine.get_entity(id).ok_or_else(|| {
            CliError::new(
                ExitKind::NotFound,
                "ENTITY_NOT_FOUND",
                format!("entity not found: {id}"),
            )
            .with_details(serde_json::json!({ "id": id.to_string() }))
        })?;
        return Ok(Some(entity.content_hash.clone()));
    }
    require_explicit_hash(explicit, exempt)
}

/// Filesystem-mem counterpart of [`resolve_hash_mem_repo`]. Same
/// semantics; differs only in the engine accessor type.
fn resolve_hash_filesystem(
    engine: &memstead_base::Engine,
    id: &EntityId,
    explicit: Option<String>,
    auto_hash: bool,
    force: bool,
    exempt: bool,
) -> anyhow::Result<Option<String>> {
    if auto_hash || force {
        let entity = engine.get_entity(id).ok_or_else(|| {
            CliError::new(
                ExitKind::NotFound,
                "ENTITY_NOT_FOUND",
                format!("entity not found: {id}"),
            )
            .with_details(serde_json::json!({ "id": id.to_string() }))
        })?;
        return Ok(Some(entity.content_hash.clone()));
    }
    require_explicit_hash(explicit, exempt)
}

/// `exempt` waives the requirement (consistency-sweep 03/04). The
/// compare-and-swap token asserts that the entity's CONTENT is unchanged, and
/// on an anchors-only write the content is unchanged by construction: the
/// anchors sidecar is outside `_hash` by deliberate design, so the token
/// compares a value the guarded write cannot move. Demanding it therefore
/// bought no protection and cost a read or dry-run roundtrip per entity,
/// falling on exactly the backfill flows the anchor dialect exists to make
/// attractive.
///
/// Callers derive `exempt` from the engine's own `changes_content()`, so this
/// surface and MCP cannot come to disagree about whether a write is safe. The
/// mem-repo path additionally waives it for `--dry-run`, matching the shipped
/// MCP contract that a dry run bypasses only this check and is the designated
/// stale-hash recovery path; a dry run writes nothing, so there is nothing to
/// guard.
///
/// An EMPTY token counts as no token, here and on every other surface: it can
/// never match a real hash, so treating it as a supplied one turned an
/// anchors-only write into a spurious mismatch on whichever surface forgot.
fn require_explicit_hash(explicit: Option<String>, exempt: bool) -> anyhow::Result<Option<String>> {
    match explicit {
        Some(h) if !h.is_empty() => Ok(Some(h)),
        _ if exempt => Ok(None),
        _ => Err(CliError::new(
            ExitKind::Validation,
            crate::HASH_FLAG_REQUIRED_CODE,
            "missing --expected-hash. Read the entity first (memstead entity <id>) and pass its `_hash`, \
             or use --auto-hash for one-off interactive updates, or --force to overwrite. \
             An anchors-only update (--anchor / --anchor-unset and nothing else) needs none: \
             anchors are outside the content hash.",
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
                format!("--declare-relations: expected REL_TYPE:TARGET_ID, got `{raw}`"),
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
) -> anyhow::Result<IndexMap<String, PatchesPayload>> {
    let mut out: IndexMap<String, Vec<PatchPayload>> =
        IndexMap::with_capacity(first_only.len() + all.len());
    for (items, flag, replace_all) in [(first_only, "--patch", false), (all, "--patch-all", true)] {
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
            // The inline separator cannot express an OLD or NEW that itself
            // contains `=>`: the split is ambiguous, and a first-occurrence
            // split silently corrupted the section (backlog, live melt).
            // Refuse toward the payload form, which carries arbitrary text.
            if new.contains("=>") {
                return Err(CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    format!(
                        "{flag}: `{raw}` carries more than one `=>` — the inline form cannot                          say which one separates OLD from NEW. Use `--from <file.json>` with                          `patch_sections`, which carries arbitrary text unambiguously."
                    ),
                )
                .into());
            }
            // Repeats for one section apply in order against the evolving
            // body — batched edits land in one call (`--patch` and
            // `--patch-all` may mix per section).
            out.entry(key.to_string()).or_default().push(PatchPayload {
                old: old.to_string(),
                new: new.to_string(),
                all: replace_all,
            });
        }
    }
    Ok(out
        .into_iter()
        .map(|(k, v)| (k, PatchesPayload::Many(v)))
        .collect())
}
