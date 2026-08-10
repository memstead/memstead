use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use serde_json::json;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

/// Export the write mem as markdown (in place), as a portable `.mem`
/// archive, or as a structured JSON document on stdout.
///
/// `--format markdown` is supported only on folder-backed mems; use
/// `--format mem` for archive export on git-branch backends. Targeting
/// a mem on an incompatible backend returns
/// `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND`; workspace-wide markdown export
/// in a mixed-backend workspace completes the folder mounts and lists
/// the declined mounts under `skipped_mounts`.
///
/// `--format json` is the bulk read: one engine boot emits the complete
/// entity set — per entity the same structured envelope `memstead entity
/// --json` produces — grouped per mem, backend-uniform, observably
/// read-only. External projections and check scripts consume this
/// instead of per-entity CLI calls (which pay the engine boot per
/// entity) or raw git against the mem-repo.
#[derive(Parser, Debug)]
pub struct Args {
    /// Output format. `markdown` regenerates the mem directory in place
    /// (folder-backed mems only); `mem` writes a portable `.mem` zip
    /// suitable for sharing (every backend); `json` prints every
    /// non-stub entity of the selected mem(s) as one structured JSON
    /// document on stdout (every backend, read-only).
    #[arg(long, value_enum, default_value_t = Format::Markdown)]
    pub format: Format,

    /// Output path for `--format mem` (default `./<name>-<version>.mem`)
    /// and `--format html` (default `./<mem>.html`). Ignored for
    /// `--format markdown`; refused for `--format json` (that document
    /// goes to stdout).
    #[arg(long, short = 'o', value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Which mem to export (by name). For `--format markdown`, omitting
    /// this argument runs a workspace-wide export and reports any
    /// declined mounts under `skipped_mounts`. For `--format mem`,
    /// required when more than one write mem is loaded; defaults to
    /// the first writable mem otherwise. For `--format json`, omitting
    /// it exports every writable mem; naming a read-only mount exports
    /// that mount (read-mems are excluded from the workspace-wide
    /// default — they are someone else's published content).
    #[arg(long = "mem", value_name = "NAME")]
    pub mem_name: Option<String>,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum Format {
    /// Regenerate markdown files in place.
    Markdown,
    /// Write a `.mem` zip archive to `--output`.
    Mem,
    /// Print the full entity set as one JSON document on stdout.
    Json,
    /// Write one self-contained HTML file — the read surface for
    /// non-operators: no server, no scripts, zero network requests.
    Html,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    if matches!(args.format, Format::Json) {
        return run_json(ctx, args);
    }
    if matches!(args.format, Format::Html) {
        return run_html(ctx, args);
    }
    match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(engine) => match args.format {
            Format::Markdown => run_markdown(ctx, &engine, args.mem_name.as_deref()),
            Format::Mem => run_mem(ctx, &engine, args),
            Format::Json => unreachable!("dispatched to run_json above"),
            Format::Html => unreachable!("dispatched to run_html above"),
        },
        CliEngine::Filesystem(engine) => match args.format {
            // `--format markdown` regenerates files in place. The
            // filesystem engine's writer would do the same, but
            // there's no `export_markdown` accessor today; surface
            // the gap as a clear validation error rather than a
            // silent no-op.
            Format::Markdown => Err(CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                "--format markdown is not yet supported on filesystem-mem `memstead export` — entities are already on disk in their canonical form",
            )
            .into()),
            Format::Mem => run_mem_filesystem(ctx, &engine, args),
            Format::Json => unreachable!("dispatched to run_json above"),
            Format::Html => unreachable!("dispatched to run_html above"),
        },
    }
}

/// Version marker on the `--format json` document, following the
/// `workspace-dump/v0` convention: consumers assert the marker before
/// parsing so a future shape change fails loudly instead of silently.
const JSON_EXPORT_FORMAT: &str = "memstead-export/v1";

/// `--format json` — the bulk read. Backend-uniform (both engine
/// flavours serve it via [`CliEngine::base`]) and observably read-only:
/// pure store iteration, no engine mutation path is touched. Each
/// entity rides as the same structured envelope `memstead entity --json`
/// emits (plus mem-level grouping), so a consumer parses one entity
/// shape across both surfaces. Entities are sorted by id within each
/// mem for deterministic output; stubs are excluded (they are
/// unresolved references, not content).
fn run_json(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    // `-o` only means something for archive export. Refusing beats
    // silently ignoring: an operator who passed `-o dump.json` would
    // otherwise wait on a file that never appears.
    if args.output.is_some() {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            "--output applies only to --format mem — the JSON document goes to stdout; redirect it instead",
        )
        .into());
    }

    let cli_engine = ctx.cli_engine()?;
    let engine = cli_engine.base();

    let all_names: Vec<String> = engine.mem_names().into_iter().map(String::from).collect();
    // Named mem: any loaded mount qualifies, read-only included — an
    // explicit name is the opt-in. Workspace-wide default: writable
    // mems only; read-only mounts are someone else's published content.
    let selected: Vec<String> = match &args.mem_name {
        Some(name) => {
            if !all_names.iter().any(|n| n == name) {
                return Err(CliError::new(
                    ExitKind::NotFound,
                    "UNKNOWN_MEM",
                    format!(
                        "unknown mem '{name}' — loaded mems: {}",
                        all_names.join(", ")
                    ),
                )
                .with_details(json!({ "mem": name, "loaded": all_names }))
                .into());
            }
            vec![name.clone()]
        }
        None => all_names
            .iter()
            .filter(|n| engine.mem_router().is_writable(n))
            .cloned()
            .collect(),
    };

    let mut mems = serde_json::Map::new();
    for mem_name in &selected {
        // The authoritative schema pin lives in the mem's own config;
        // carried once at the group level rather than per entity.
        let schema_pin = engine
            .mem_configs_named()
            .find(|(name, _)| name == mem_name)
            .and_then(|(_, c)| c.schema.as_ref())
            .map(|s| s.to_string());

        let mut entities: Vec<&memstead_base::Entity> = engine
            .store()
            .all_entities()
            .filter(|e| !e.stub && e.mem == *mem_name)
            .collect();
        entities.sort_by(|a, b| a.id.as_ref().cmp(b.id.as_ref()));

        let envelopes: Vec<serde_json::Value> = entities
            .iter()
            .map(|entity| {
                let body = memstead_base::render::render_entity_markdown(entity, None);
                let tokens = memstead_base::chunking::estimate_tokens(&body);
                let outgoing = engine.store().outgoing(&entity.id);
                memstead_base::render::build_entity_envelope(
                    entity, tokens, None, None, None, outgoing,
                )
            })
            .collect();

        let mut group = serde_json::Map::new();
        if let Some(s) = schema_pin {
            group.insert("schema".to_string(), json!(s));
        }
        group.insert(
            "read_only".to_string(),
            json!(!engine.mem_router().is_writable(mem_name)),
        );
        group.insert("entity_count".to_string(), json!(envelopes.len()));
        group.insert("entities".to_string(), serde_json::Value::Array(envelopes));
        mems.insert(mem_name.clone(), serde_json::Value::Object(group));
    }

    print_json(&json!({
        "format": JSON_EXPORT_FORMAT,
        "mems": mems,
    }))
}

#[cfg(feature = "mem-repo")]
fn run_markdown(
    ctx: &CliContext,
    engine: &memstead_base::Engine,
    mem_filter: Option<&str>,
) -> anyhow::Result<()> {
    // The engine returns a
    // typed `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` when `--mem`
    // targets a mem whose backend doesn't support markdown
    // regeneration. The workspace-wide path returns counts plus a
    // structured `skipped_mounts` list.
    let result = engine
        .export_markdown(mem_filter, None)
        .map_err(CliError::from_engine_op)?;

    if ctx.json {
        let mut body = json!({
            "written": result.written,
            "unchanged": result.unchanged,
        });
        if !result.skipped_mounts.is_empty() {
            body["skipped_mounts"] = serde_json::to_value(&result.skipped_mounts)
                .unwrap_or_else(|_| serde_json::Value::Array(Vec::new()));
        }
        print_json(&body)?;
    } else {
        let mut block = format!(
            "# Export — markdown\n\n- Written: {}\n- Unchanged: {}",
            result.written, result.unchanged,
        );
        if !result.skipped_mounts.is_empty() {
            block.push_str("\n\n## Skipped mounts\n");
            for m in &result.skipped_mounts {
                block.push_str(&format!(
                    "\n- `{}` — backend `{}` ({}); use `--format mem` for archive export",
                    m.mem, m.active_backend, m.reason,
                ));
            }
        }
        print_markdown(&block);
    }
    Ok(())
}

#[cfg(feature = "mem-repo")]
fn run_mem(ctx: &CliContext, engine: &memstead_base::Engine, args: Args) -> anyhow::Result<()> {
    let mem_name = resolve_mem_name(engine, args.mem_name)?;
    let config = engine
        .mem_configs_named()
        .find(|(name, _)| *name == mem_name)
        .map(|(_, c)| c)
        .ok_or_else(|| {
            CliError::new(
                ExitKind::NotFound,
                "UNKNOWN_MEM",
                format!("mem config not found for '{mem_name}'"),
            )
        })?;

    let output = match args.output {
        Some(p) => p,
        None => default_output_path(&mem_name, config)?,
    };

    let result = engine
        .export_mem(&mem_name, &output)
        .map_err(CliError::from_engine_op)?;

    // Surface each cross-mem edge
    // whose target won't travel inside the single-mem archive — these
    // are exactly what `install` will refuse, so showing them at export
    // time lets the operator act before sharing.
    let dangling = &result.dangling_cross_mem_edges;

    if ctx.json {
        let warnings: Vec<_> = dangling
            .iter()
            .map(|e| {
                json!({
                    "code": "DANGLING_CROSS_MEM_EDGE_IN_EXPORT",
                    "entity": e.entity_path,
                    "target_id": e.target_id,
                    "target_mem": e.target_mem,
                })
            })
            .collect();
        print_json(&json!({
            "archive_path": result.archive_path,
            "name": result.name,
            "version": result.version,
            "entity_count": result.entity_count,
            "size_bytes": result.size_bytes,
            "warnings": warnings,
        }))?;
    } else {
        let mut block = format!(
            "# Exported `{}` v{}\n\n- Archive: `{}`\n- Entities: {}\n- Size: {} bytes",
            result.name,
            result.version,
            result.archive_path,
            result.entity_count,
            result.size_bytes,
        );
        if !dangling.is_empty() {
            block.push_str("\n\n## Warnings\n");
            for e in dangling {
                block.push_str(&format!(
                    "\n- **DANGLING_CROSS_MEM_EDGE_IN_EXPORT**: `{}` → `{}` (mem `{}`) — \
                     target lives outside this archive; `memstead install` will reject it unless \
                     mem `{}` is also present.",
                    e.entity_path, e.target_id, e.target_mem, e.target_mem,
                ));
            }
        }
        print_markdown(&block);
    }
    Ok(())
}

#[cfg(feature = "mem-repo")]
fn resolve_mem_name(
    engine: &memstead_base::Engine,
    explicit: Option<String>,
) -> anyhow::Result<String> {
    if let Some(name) = explicit {
        return Ok(name);
    }
    let writable: Vec<String> = engine
        .mem_configs_named()
        .filter(|(name, _)| engine.mem_router().is_writable(name))
        .map(|(name, _)| name.to_string())
        .collect();

    match writable.len() {
        0 => Err(CliError::new(
            ExitKind::Generic,
            "NO_WRITABLE_MEM",
            "no writable mem loaded — nothing to export",
        )
        .into()),
        1 => Ok(writable.into_iter().next().unwrap()),
        _ => Err(CliError::new(
            ExitKind::Validation,
            "AMBIGUOUS_MEM",
            format!(
                "multiple writable mems loaded ({}); pass --mem <name>",
                writable.join(", ")
            ),
        )
        .with_details(json!({ "mems": writable }))
        .into()),
    }
}

/// Filesystem-mem `memstead export --format mem` builds the `.mem`
/// archive bytes via [`memstead_base::filesystem::publish::assemble_archive`]
/// (the same path `memstead publish` uses on a filesystem-mem workspace)
/// and writes them to `--output` (defaulting to `<name>-<version>.mem`
/// in cwd). `--mem` is accepted for shape parity but only the
/// workspace's pinned mem matches.
fn run_mem_filesystem(
    ctx: &CliContext,
    engine: &memstead_base::Engine,
    args: Args,
) -> anyhow::Result<()> {
    let workspace_mem = engine
        .mem_names()
        .into_iter()
        .next()
        .map(String::from)
        .unwrap_or_default();
    if let Some(name) = args.mem_name.as_deref()
        && name != workspace_mem
    {
        return Err(CliError::new(
                ExitKind::NotFound,
                "UNKNOWN_MEM",
                format!(
                    "filesystem-mem is single-mem: workspace mem is `{workspace_mem}`, --mem `{name}` does not match"
                ),
            )
            .into());
    }

    // assemble_archive is engine-agnostic now — pass the discovered
    // workspace root directly.
    let workspace_root =
        crate::setup::find_filesystem_workspace_root(&std::env::current_dir().map_err(|e| {
            CliError::new(
                ExitKind::Generic,
                crate::INTERNAL_CODE,
                format!("current_dir: {e}"),
            )
        })?)
        .ok_or_else(|| {
            CliError::new(
                ExitKind::NotFound,
                "WORKSPACE_NOT_INITIALISED",
                "no filesystem-mem workspace found from cwd",
            )
        })?;
    let bytes =
        memstead_base::filesystem::publish::assemble_archive(&workspace_root).map_err(|e| {
            // F1: backend-symmetric typed envelope for the missing-
            // version case — the mem-repo path surfaces the same
            // MEM_CONFIG_INCOMPLETE via Engine::export_mem.
            if matches!(
                &e,
                memstead_base::filesystem::publish::AssembleError::Config(
                    memstead_schema::PublishConversionError::MissingVersion
                )
            ) {
                CliError::from_engine_op(memstead_base::EngineError::MemConfigIncomplete {
                    mem: workspace_mem.clone(),
                    missing_fields: vec!["version".to_string()],
                })
            } else {
                CliError::new(ExitKind::Generic, "ARCHIVE_ASSEMBLY_FAILED", e.to_string())
            }
        })?;

    let output = match args.output {
        Some(p) => p,
        None => {
            // Filesystem-mem config doesn't carry `version` today —
            // archive identity is `<mem_name>.mem` until the
            // assemble path threads a version through. Operator can
            // override with `-o`.
            PathBuf::from(format!(
                "{workspace_mem}.{}",
                memstead_schema::ARCHIVE_EXTENSION
            ))
        }
    };

    let size_bytes = bytes.len();
    std::fs::write(&output, &bytes).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            crate::INTERNAL_CODE,
            format!("write {}: {e}", output.display()),
        )
    })?;
    // Count only the exported mem's entities — the store also holds
    // mounted sibling mems (the multi-mount setup), which do not travel
    // in this archive.
    let entity_count = engine
        .store()
        .all_entities()
        .filter(|e| !e.stub && e.id.mem() == workspace_mem)
        .count();

    if ctx.json {
        print_json(&json!({
            "archive_path": output.to_string_lossy(),
            "name": workspace_mem,
            "entity_count": entity_count,
            "size_bytes": size_bytes,
        }))?;
    } else {
        print_markdown(&format!(
            "# Exported `{workspace_mem}`\n\n- Archive: `{}`\n- Entities: {}\n- Size: {} bytes",
            output.display(),
            entity_count,
            size_bytes,
        ));
    }
    Ok(())
}

#[cfg(feature = "mem-repo")]
fn default_output_path(
    mem_name: &str,
    config: &memstead_schema::MemConfig,
) -> anyhow::Result<PathBuf> {
    let version = config.version.as_ref().ok_or_else(|| {
        // F1: typed envelope replaces the pre-fix INTERNAL-collapse
        // path (config lives at
        // `__MEMSTEAD:mems/<name>/config.json` for the mem-repo
        // backend). The recovery hint
        // names the engine-owned setter that mutates the right
        // surface for whichever backend serves the mem.
        CliError::from_engine_op(memstead_base::EngineError::MemConfigIncomplete {
            mem: mem_name.to_string(),
            missing_fields: vec!["version".to_string()],
        })
    })?;
    // The mem name is supplied by the caller (engine mem state)
    // rather than pulled from the now-optional in-config `name` field.
    let filename = format!(
        "{mem_name}-{version}.{}",
        memstead_schema::ARCHIVE_EXTENSION
    );
    Ok(PathBuf::from(filename))
}

/// `--format html` — one self-contained HTML file per mem (the read
/// surface for non-operators). Backend-uniform via [`CliEngine::base`]
/// and observably read-only. The export date is stamped once (UTC);
/// `--today` on `memstead due` has no analogue here because the date
/// only labels the export, it never filters.
fn run_html(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let engine_holder = ctx.cli_engine()?;
    let engine = engine_holder.base();
    // Resolve the target mem like `--format mem`: explicit name wins
    // (read-only mounts allowed); otherwise the sole writable mem.
    let mem = match &args.mem_name {
        Some(m) => m.clone(),
        None => {
            let writables: Vec<String> = engine
                .writable_mem_names()
                .iter()
                .map(|s| s.to_string())
                .collect();
            match writables.as_slice() {
                [one] => one.clone(),
                [] => {
                    return Err(CliError::new(
                        ExitKind::Validation,
                        "INVALID_INPUT",
                        "no writable mem loaded — pass --mem <name>",
                    )
                    .into());
                }
                _ => {
                    return Err(CliError::new(
                        ExitKind::Validation,
                        "INVALID_INPUT",
                        format!(
                            "multiple writable mems loaded ({}) — pass --mem <name>",
                            writables.join(", ")
                        ),
                    )
                    .into());
                }
            }
        }
    };
    let now = time::OffsetDateTime::now_utc();
    let export_date = format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    );
    let html = engine
        .render_html_export(&mem, &export_date)
        .map_err(CliError::from_engine_op)?;
    let out_path = args
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("{mem}.html")));
    std::fs::write(&out_path, &html).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "IO_ERROR",
            format!("write {}: {e}", out_path.display()),
        )
    })?;
    if ctx.json {
        print_json(&serde_json::json!({
            "format": "html",
            "mem": mem,
            "path": out_path,
            "bytes": html.len(),
            "exported": export_date,
        }))?;
    } else {
        print_markdown(&format!(
            "# HTML export\n\n- Mem: `{mem}`\n- File: `{}`\n- Size: {} bytes\n- Exported: {export_date}\n\nSelf-contained — open it from anywhere, no server needed.\n",
            out_path.display(),
            html.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Mem selection is `--mem`, converged onto the convention every
    /// other subcommand uses; the former `--mem-name` outlier is gone.
    #[test]
    fn export_mem_selection_flag_is_mem_not_mem_name() {
        let parsed = Args::try_parse_from(["export", "--mem", "specs", "--format", "mem"]).unwrap();
        assert_eq!(parsed.mem_name.as_deref(), Some("specs"));
        assert!(
            Args::try_parse_from(["export", "--mem-name", "specs"]).is_err(),
            "the retired --mem-name flag must not parse"
        );
    }
}
