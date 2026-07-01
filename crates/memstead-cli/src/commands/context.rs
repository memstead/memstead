use clap::Parser;
use serde_json::json;

use memstead_base::EntityId;
use memstead_base::chunking::apply_chunking;
use memstead_base::ops::{ContextResult, Query, SearchScope};
use memstead_base::render;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

const DEFAULT_TOKEN_BUDGET: usize = 25_000;

/// Read an entity's community cluster.
#[derive(Parser, Debug)]
pub struct Args {
    /// Entity ID (exact match preferred) or search text (fallback).
    pub id_or_query: String,

    /// 1-based chunk index for large contexts.
    #[arg(long)]
    pub chunk: Option<usize>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let id = EntityId::canonical(&args.id_or_query);
    let outcome: ContextOutcome = match ctx.cli_engine()? {
        #[cfg(feature = "vault-repo")]
        CliEngine::VaultRepo(engine) => resolve_context_vault_repo(&engine, &id, ctx, &args)?,
        CliEngine::Filesystem(engine) => resolve_context_filesystem(&engine, &id, ctx, &args)?,
    };

    match outcome {
        ContextOutcome::Resolved(Some(result)) => {
            let cluster_id = result.community.as_deref().unwrap_or("unknown").to_string();
            let md = render::render_context_markdown(&result, &cluster_id);
            let chunked = apply_chunking(
                &md,
                DEFAULT_TOKEN_BUDGET,
                args.chunk,
                &[("_cluster_id", cluster_id.as_str())],
            )
            .map_err(|e| CliError::new(ExitKind::Generic, "CHUNK_OUT_OF_RANGE", e))?;
            if ctx.json {
                print_json(&json!({ "markdown": chunked, "cluster_id": cluster_id }))?;
            } else {
                print_markdown(&chunked);
            }
            Ok(())
        }
        ContextOutcome::Resolved(None) => Err(CliError::new(
            ExitKind::Generic,
            "CONTEXT_NOT_COMPUTABLE",
            "Could not compute context",
        )
        .into()),
        ContextOutcome::NotFound { query } => Err(CliError::new(
            ExitKind::NotFound,
            "ENTITY_NOT_FOUND",
            format!("no entity found for: {query}"),
        )
        .with_details(json!({ "id": query }))
        .into()),
        ContextOutcome::Ambiguous { query, candidates } => Err(CliError::new(
            ExitKind::Validation,
            "AMBIGUOUS_QUERY",
            format!(
                "ambiguous query: {} candidates match `{query}` — pass an exact entity id",
                candidates.len()
            ),
        )
        .with_details(json!({ "query": query, "candidates": candidates }))
        .into()),
    }
}

/// Outcome of an `id_or_query` lookup. Resolved carries the engine's
/// `context()` reply (which may itself be `None` if a found entity has
/// no computable cluster); NotFound and Ambiguous carry the typed
/// recovery payload after their user-facing body has already printed.
enum ContextOutcome {
    Resolved(Option<ContextResult>),
    NotFound {
        query: String,
    },
    Ambiguous {
        query: String,
        candidates: Vec<serde_json::Value>,
    },
}

/// Resolve context for an id-or-query against the vault-repo engine.
/// On a successful unique resolution returns `Ok(Some(_))`. On
/// not-found / ambiguous results, prints the user-facing notice and
/// returns `Ok(None)` so the caller exits cleanly without invoking
/// the markdown render path.
#[cfg(feature = "vault-repo")]
fn resolve_context_vault_repo(
    engine: &memstead_base::Engine,
    id: &EntityId,
    ctx: &CliContext,
    args: &Args,
) -> anyhow::Result<ContextOutcome> {
    if engine.get_entity(id).is_some() {
        return Ok(ContextOutcome::Resolved(engine.context(id)));
    }
    let search = engine.search(&fuzzy_scope(&args.id_or_query))?;
    if let Some(miss) = handle_id_or_query_miss(ctx, &args.id_or_query, &search.hits)? {
        return Ok(miss);
    }
    Ok(ContextOutcome::Resolved(engine.context(&search.hits[0].id)))
}

/// Mirror of [`resolve_context_vault_repo`] for the filesystem engine.
fn resolve_context_filesystem(
    engine: &memstead_base::Engine,
    id: &EntityId,
    ctx: &CliContext,
    args: &Args,
) -> anyhow::Result<ContextOutcome> {
    if engine.get_entity(id).is_some() {
        return Ok(ContextOutcome::Resolved(engine.context(id)));
    }
    let search = engine.search(&fuzzy_scope(&args.id_or_query))?;
    if let Some(miss) = handle_id_or_query_miss(ctx, &args.id_or_query, &search.hits)? {
        return Ok(miss);
    }
    Ok(ContextOutcome::Resolved(engine.context(&search.hits[0].id)))
}

/// Build the fallback search scope used when the user-supplied
/// `id_or_query` doesn't resolve to an exact entity id.
fn fuzzy_scope(query_text: &str) -> SearchScope {
    SearchScope {
        query: Some(Query {
            any: vec![query_text.to_string()],
            ..Default::default()
        }),
        limit: Some(5),
        ..Default::default()
    }
}

/// Print the not-found / ambiguous body, then return the typed miss
/// disposition so the caller can build the recovery envelope. Returns
/// `None` when the search produced exactly one hit and the caller
/// should proceed with that hit.
fn handle_id_or_query_miss(
    ctx: &CliContext,
    query_text: &str,
    hits: &[memstead_base::SearchHit],
) -> anyhow::Result<Option<ContextOutcome>> {
    if hits.is_empty() {
        let msg = format!("No entity found for: {query_text}");
        if ctx.json {
            print_json(&json!({ "found": false, "message": msg }))?;
        } else {
            print_markdown(&format!("_{msg}_"));
        }
        return Ok(Some(ContextOutcome::NotFound {
            query: query_text.to_string(),
        }));
    }
    if hits.len() > 1 {
        let candidates: Vec<_> = hits
            .iter()
            .map(|h| json!({ "id": h.id.to_string(), "title": &h.title }))
            .collect();
        if ctx.json {
            print_json(&json!({ "ambiguous": true, "candidates": &candidates }))?;
        } else {
            let mut lines = vec!["# Ambiguous match".to_string(), String::new()];
            for h in hits {
                lines.push(format!("- {} — {}", h.id, h.title));
            }
            print_markdown(&lines.join("\n"));
        }
        return Ok(Some(ContextOutcome::Ambiguous {
            query: query_text.to_string(),
            candidates,
        }));
    }
    Ok(None)
}
