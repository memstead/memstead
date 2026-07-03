use std::collections::HashMap;

use clap::Parser;

use memstead_base::ops::SearchScope;
use memstead_base::render;

use crate::CliError;
use crate::output::ExitKind;
use crate::output::{print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

/// Filter entities by metadata (no text match — use `search` for that).
#[derive(Parser, Debug)]
#[command(after_long_help = super::FILTER_HELP)]
pub struct Args {
    #[arg(long)]
    pub mem: Option<String>,

    #[arg(long = "type")]
    pub entity_type: Option<String>,

    #[arg(long)]
    pub level: Option<String>,

    #[arg(long)]
    pub status: Option<String>,

    #[arg(long)]
    pub edge_type: Option<String>,

    /// Equality filter on any schema-declared filterable field:
    /// repeatable `--filter KEY=VALUE`. The four named-flag
    /// shortcuts (`--type` / `--level` / `--status` / `--edge-type`)
    /// handle their common cases; every other `filterable: equality`
    /// field (e.g. `tags`, `scope`) is reachable via this generic
    /// flag. Unknown keys are dropped and surface as engine
    /// warnings. There is no `--confidence` shortcut: a field reached
    /// only when a schema declares it goes through
    /// `--filter <field>=<value>` rather than a dedicated flag.
    #[arg(long = "filter", value_name = "KEY=VALUE")]
    pub filter: Vec<String>,

    #[arg(long)]
    pub limit: Option<usize>,

    #[arg(long)]
    pub offset: Option<usize>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let mut filters = HashMap::new();
    if let Some(level) = args.level {
        filters.insert("level".to_string(), level);
    }
    if let Some(status) = args.status {
        filters.insert("status".to_string(), status);
    }
    for raw in &args.filter {
        let (key, value) = super::parse_filter_arg(raw)?;
        filters.insert(key, value);
    }

    let scope = SearchScope {
        mem: args.mem,
        entity_type: args.entity_type,
        limit: args.limit,
        offset: args.offset,
        edge_type: args.edge_type,
        filters,
        ..Default::default()
    };

    let result = match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(engine) => {
            // Validate `--mem` upfront so unknown names error
            // typed instead of silently returning `_total: 0` — matches
            // `memstead create --mem X`'s symmetry.
            if let Some(name) = scope.mem.as_deref()
                && engine.mount(name).is_none()
            {
                return Err(unknown_mem_error(name, &engine).into());
            }
            engine.list(&scope)
        }
        CliEngine::Filesystem(engine) => {
            if let Some(name) = scope.mem.as_deref()
                && engine.mount(name).is_none()
            {
                return Err(unknown_mem_error(name, &engine).into());
            }
            engine.list(&scope)
        }
    };

    if ctx.json {
        let envelope = render::build_list_envelope(&result);
        print_json(&envelope)?;
    } else {
        print_markdown(&render::render_list_markdown(&result));
    }
    Ok(())
}

/// Typed `UNKNOWN_MEM` envelope for `memstead list --mem X` / `memstead search --mem X`
/// when `X` resolves to no mount. Mirrors `memstead create --mem X`'s pre-fix
/// behaviour: read-side commands now reject unknown names instead of silently
/// returning empty results.
pub(super) fn unknown_mem_error(name: &str, engine: &memstead_base::Engine) -> CliError {
    let known: Vec<&str> = engine.mem_names();
    let detail = if known.is_empty() {
        "no mems loaded — run `memstead workspace dump` for the workspace state".to_string()
    } else {
        format!(
            "known mems: [{}]. Run `memstead workspace dump` for the full snapshot.",
            known.join(", ")
        )
    };
    CliError {
        code: "UNKNOWN_MEM",
        kind: ExitKind::NotFound,
        message: format!("unknown mem: {name} — {detail}"),
        details: Some(serde_json::json!({ "mem": name })),
    }
}
