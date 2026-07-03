use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use serde_json::json;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

/// Export the write mem as markdown (in place) or as a portable `.mem` archive.
///
/// `--format markdown` is supported only on folder-backed mems; use
/// `--format mem` for archive export on git-branch backends. Targeting
/// a mem on an incompatible backend returns
/// `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND`; workspace-wide markdown export
/// in a mixed-backend workspace completes the folder mounts and lists
/// the declined mounts under `skipped_mounts`.
#[derive(Parser, Debug)]
pub struct Args {
    /// Output format. `markdown` regenerates the mem directory in place
    /// (folder-backed mems only); `mem` writes a portable `.mem` zip
    /// suitable for sharing (every backend).
    #[arg(long, value_enum, default_value_t = Format::Markdown)]
    pub format: Format,

    /// Output path for `--format mem`. Defaults to `./<name>-<version>.mem`
    /// in the current directory, matching the "external vs cache filename"
    /// convention for portable mem archives. Ignored for `--format markdown`.
    #[arg(long, short = 'o', value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Which mem to export (by name). For `--format markdown`, omitting
    /// this argument runs a workspace-wide export and reports any
    /// declined mounts under `skipped_mounts`. For `--format mem`,
    /// required when more than one write mem is loaded; defaults to
    /// the first writable mem otherwise.
    #[arg(long = "mem", value_name = "NAME")]
    pub mem_name: Option<String>,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum Format {
    /// Regenerate markdown files in place.
    Markdown,
    /// Write a `.mem` zip archive to `--output`.
    Mem,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(engine) => match args.format {
            Format::Markdown => run_markdown(ctx, &engine, args.mem_name.as_deref()),
            Format::Mem => run_mem(ctx, &engine, args),
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
        },
    }
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
    let entity_count = engine.store().all_entities().filter(|e| !e.stub).count();

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
