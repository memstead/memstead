use std::collections::HashMap;

use memstead_base::Store;
use memstead_base::ingest::status::{ProjectionStatus, projection_status};
use serde::Serialize;
use serde_json::json;

use crate::output::{print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

#[derive(Serialize)]
struct EdgeTypeCount<'a> {
    #[serde(rename = "type")]
    rel_type: &'a str,
    count: usize,
}

#[derive(Serialize)]
struct TypeCount<'a> {
    #[serde(rename = "type")]
    entity_type: &'a str,
    count: usize,
}

/// The `memstead status` JSON payload (D11). The graph-count fields are
/// byte-compatible with the former `stats` command's payload; `projections` is
/// the additive per-binding array. The rollup verdict is a later additive E3b
/// field — nothing reserved here.
#[derive(Serialize)]
struct StatusPayload<'a> {
    total_nodes: usize,
    real_nodes: usize,
    stub_nodes: usize,
    total_edges: usize,
    edge_types: Vec<EdgeTypeCount<'a>>,
    type_distribution: Vec<TypeCount<'a>>,
    projections: Vec<ProjectionStatus>,
}

pub fn run(ctx: &CliContext) -> anyhow::Result<()> {
    // The workspace root (for the projection store / advance store reads). The
    // engine build below fails before this matters when we are outside a
    // workspace, so a `None` here only ever means "in a workspace that declares
    // no projections" once we get past `cli_engine()?`.
    let root = ctx.workspace_shape().map(|(_, r)| r);

    let (status, total, real, schema_counts, projections) = match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(engine) => {
            let status = engine.status();
            let store: &Store = engine.store();
            let projections = root
                .as_deref()
                .map(|r| projection_status(&engine, r))
                .unwrap_or_default();
            (
                status,
                store.len(),
                store.all_entities().filter(|e| !e.stub).count(),
                count_by_type(store),
                projections,
            )
        }
        CliEngine::Filesystem(engine) => {
            let status = engine.status();
            let store: &Store = engine.store();
            let projections = root
                .as_deref()
                .map(|r| projection_status(&engine, r))
                .unwrap_or_default();
            (
                status,
                store.len(),
                store.all_entities().filter(|e| !e.stub).count(),
                count_by_type(store),
                projections,
            )
        }
    };
    let stubs = total - real;

    let mut edge_pairs: Vec<_> = status.edge_types.iter().collect();
    edge_pairs.sort_by(|a, b| b.1.cmp(a.1));

    let mut schema_pairs: Vec<(String, usize)> = schema_counts.into_iter().collect();
    schema_pairs.sort_by_key(|p| std::cmp::Reverse(p.1));

    if ctx.json {
        let payload = StatusPayload {
            total_nodes: total,
            real_nodes: real,
            stub_nodes: stubs,
            total_edges: status.edge_count,
            edge_types: edge_pairs
                .iter()
                .map(|(t, c)| EdgeTypeCount {
                    rel_type: t,
                    count: **c,
                })
                .collect(),
            type_distribution: schema_pairs
                .iter()
                .map(|(s, c)| TypeCount {
                    entity_type: s,
                    count: *c,
                })
                .collect(),
            projections,
        };
        return print_json(&json!(payload));
    }

    let mut lines = Vec::new();
    lines.push("# Graph status".to_string());
    lines.push(String::new());
    lines.push(format!("- Nodes: {total} ({real} real, {stubs} stubs)"));
    lines.push(format!("- Edges: {}", status.edge_count));
    if !edge_pairs.is_empty() {
        let edges: Vec<String> = edge_pairs
            .iter()
            .map(|(t, c)| format!("{t} ({c})"))
            .collect();
        lines.push(format!("- Edge types: {}", edges.join(", ")));
    }
    if !schema_pairs.is_empty() {
        let schemas: Vec<String> = schema_pairs
            .iter()
            .map(|(s, c)| format!("{s} ({c})"))
            .collect();
        lines.push(format!("- Types: {}", schemas.join(", ")));
    }
    if !projections.is_empty() {
        lines.push(String::new());
        lines.push("## Projections".to_string());
        lines.push(String::new());
        for p in &projections {
            lines.push(format!(
                "- `{}` → `{}` — operations: {}; advance: {} pending, {} disposed",
                p.binding,
                p.destination_mem,
                p.operations.join(", "),
                p.advance.pending,
                p.advance.disposed,
            ));
            for (facet, state) in &p.state {
                lines.push(format!(
                    "  - {facet}: signal {}, synced {}, verified {}",
                    state.signal,
                    state.synced.as_deref().unwrap_or("none"),
                    state.verified.as_deref().unwrap_or("none"),
                ));
            }
        }
    }
    print_markdown(&lines.join("\n"));
    Ok(())
}

/// Count real (non-stub) entities by `entity_type`. Both engine
/// flavours expose a `&Store`, so this helper is engine-agnostic.
fn count_by_type(store: &Store) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for e in store.all_entities().filter(|e| !e.stub) {
        *counts.entry(e.entity_type.clone()).or_default() += 1;
    }
    counts
}
