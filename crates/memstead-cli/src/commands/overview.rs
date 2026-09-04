use clap::Parser;
use serde_json::json;

use memstead_base::chunking::apply_chunking;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::CliContext;

// Routes through the shared engine composer so the CLI renders the
// same rich content the MCP `memstead_overview` tool emits.
use memstead_base::overview::{
    ComposeOverviewError, DEFAULT_OVERVIEW_BUDGET, OverviewArgs, Surface, compose_overview,
};
const DEFAULT_CHUNK_BUDGET: usize = 25_000;

/// All clusters with summaries and member lists.
///
/// Calls the shared composer in `memstead-base` (`Surface::Cli`) and
/// renders the same rich content the MCP tool emits, differing only in
/// inline command-name hints (`memstead type <ref>` vs
/// `memstead_schema(name=<ref>)`).
#[derive(Parser, Debug)]
pub struct Args {
    /// Re-run Louvain community detection before rendering.
    #[arg(long)]
    pub rebuild: bool,

    /// 1-based chunk index for large overviews.
    #[arg(long)]
    pub chunk: Option<usize>,

    /// Scope schemas + mem inventory to any single visible mem
    /// (read-only mounts included).
    #[arg(long)]
    pub mem: Option<String>,

    /// Opt heavy content into the response: `community_members`,
    /// `community_bridges`, `mem_distribution`, `dangling_links`.
    /// Keys listed here are always included even past `token_budget`;
    /// keys omitted may surface in the `Hints` section instead.
    /// Repeatable (`--include K --include K`) AND comma-string
    /// (`--include K1,K2`) forms both parse — uniform with
    /// `memstead health --include`. Unknown keys emit
    /// `UNKNOWN_INCLUDE_KEY` warnings.
    #[arg(long = "include", value_name = "KEY", value_delimiter = ',')]
    pub include: Vec<String>,

    /// Token budget for heavy content only (`community_members`,
    /// `community_bridges`, `mem_distribution`, `dangling_links`).
    /// Hard-required content (mem roster, schema refs, community
    /// titles, workspace policy) always ships in addition — total
    /// response size will exceed this budget. Default 8000 (matches
    /// the MCP tool). Budgets below ~10 tokens are safe but
    /// unproductive — the response still arrives as a structured
    /// envelope (`_overview_mode: overbudget`), but no useful
    /// chunking happens and the full body ships as one chunk.
    #[arg(long = "token-budget", value_name = "N")]
    pub token_budget: Option<usize>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let mut engine = ctx.cli_engine()?.into_base();

    let composer_args = OverviewArgs {
        include: &args.include,
        mem: args.mem.as_deref(),
        rebuild: args.rebuild && args.chunk.unwrap_or(1) <= 1,
        token_budget: args.token_budget.unwrap_or(DEFAULT_OVERVIEW_BUDGET),
        // CLI surface never sees `--operator-mode` — the flag is an
        // MCP-server boot toggle. CLI callers always see the
        // agent-mode rendering.
        operator_mode: false,
        // The full CLI carries the mem-lifecycle commands, so the section is
        // truthful here.
        suppress_lifecycle: false,
    };

    let out = match compose_overview(&mut engine, composer_args, Surface::Cli) {
        Ok(o) => o,
        Err(ComposeOverviewError::InvalidIncludeKeySchemaTypes) => {
            return Err(CliError {
                code: "INVALID_INPUT",
                kind: ExitKind::Validation,
                message:
                    "include key 'schema_types' was removed; run `memstead type <name>` for full schema bodies."
                        .to_string(),
                details: None,
            }
            .into());
        }
        Err(ComposeOverviewError::MemQuarantined(name)) => {
            return Err(crate::CliError::from_engine_op(engine.unknown_mem_error(&name)).into());
        }
        Err(ComposeOverviewError::UnknownMem {
            name,
            writable_mems,
        }) => {
            return Err(CliError {
                code: "UNKNOWN_MEM",
                kind: ExitKind::NotFound,
                message: format!(
                    "unknown mem: \"{name}\". Writable mems: [{}]",
                    writable_mems.join(", ")
                ),
                details: Some(json!({
                    "name": name,
                    "writable_mems": writable_mems,
                })),
            }
            .into());
        }
    };

    // Apply chunking at the CLI transport budget. The composer's
    // `extra_frontmatter` rolls into every chunk's head so an agent
    // streaming chunks always sees the same anchors.
    let extra_fm: Vec<(&str, &str)> = out
        .extra_frontmatter
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let chunked = apply_chunking(
        &out.markdown,
        // Floor the chunk size: `--token-budget` is a content budget
        // (it shrinks what the composer includes); reusing a tiny value
        // as the transport chunk size would fragment the always-shipped
        // hard-required body. The floor keeps small overviews to one chunk.
        memstead_base::chunking::floor_chunk_budget(
            args.token_budget.unwrap_or(DEFAULT_CHUNK_BUDGET),
        ),
        args.chunk,
        &extra_fm,
    )
    .map_err(|e| CliError::new(ExitKind::Generic, "CHUNK_OUT_OF_RANGE", e))?;

    if ctx.json {
        let warnings_json: Vec<_> = out
            .warnings
            .iter()
            .map(|w| {
                json!({
                    "code": w.code(),
                    "message": w.message(),
                })
            })
            .collect();
        // Promote `overview_mode`, `total_chunks`, and `hints` to structured
        // envelope siblings so a programmatic consumer branches on the
        // mode and fetches the next chunk without parsing them out of the
        // `markdown` string. Additive — `markdown` is unchanged and still
        // carries the same frontmatter for the human-rendered view.
        // `total_chunks` reads the value `apply_chunking` injects into the
        // chunk frontmatter (the CLI parses it once so the consumer
        // doesn't have to).
        let total_chunks = parse_total_chunks(&chunked);
        let body = json!({
            "markdown": chunked,
            "cluster_count": out.cluster_count,
            "overview_mode": out.overview_mode,
            "total_chunks": total_chunks,
            "hints": out.hints,
            "warnings": warnings_json,
        });
        print_json(&body)?;
    } else {
        print_markdown(&chunked);
    }
    Ok(())
}

/// Read the `_total_chunks: N` value `apply_chunking` always injects
/// into the chunk's frontmatter. Defaults to 1 — `apply_chunking`
/// guarantees the marker, but a malformed head degrades to the
/// single-chunk reading rather than failing the command.
fn parse_total_chunks(chunked: &str) -> usize {
    chunked
        .lines()
        .find_map(|l| l.strip_prefix("_total_chunks: "))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    /// `--include` accepts both repeatable and comma-string shapes.
    /// Verifies clap parsing produces the same `Vec<String>` regardless
    /// of which form the caller used.
    #[test]
    fn include_accepts_repeated_and_comma_split_forms() {
        let repeated = Args::try_parse_from([
            "overview",
            "--include",
            "community_members",
            "--include",
            "mem_distribution",
        ])
        .expect("repeated form parses");
        assert_eq!(
            repeated.include,
            vec!["community_members", "mem_distribution"],
        );

        let comma = Args::try_parse_from([
            "overview",
            "--include",
            "community_members,mem_distribution",
        ])
        .expect("comma form parses");
        assert_eq!(comma.include, vec!["community_members", "mem_distribution"],);
    }

    /// The `--include` help text names every known overview include
    /// key — mirrors the `health` surface's `help_lists_every_include_key`
    /// test. The full build locks against the engine composer's
    /// allowlist; the lean build against `memstead-base`'s constant.
    #[test]
    fn help_lists_every_overview_include_key() {
        let keys: &[&str] = memstead_base::overview::ALLOWED_OVERVIEW_INCLUDE_KEYS;

        let cmd = Args::command();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id() == "include")
            .expect("--include arg must exist");
        let help = arg
            .get_help()
            .expect("--include must have help text")
            .to_string();
        for key in keys {
            assert!(
                help.contains(key),
                "`memstead overview --help` must name include key `{key}` (got: {help})"
            );
        }
    }
}
