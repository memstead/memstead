use std::collections::HashMap;

use memstead_base::Store;
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

#[derive(Serialize)]
struct StatsPayload<'a> {
    total_nodes: usize,
    real_nodes: usize,
    stub_nodes: usize,
    total_edges: usize,
    edge_types: Vec<EdgeTypeCount<'a>>,
    type_distribution: Vec<TypeCount<'a>>,
}

pub fn run(ctx: &CliContext) -> anyhow::Result<()> {
    let (stats, total, real, schema_counts) = match ctx.cli_engine()? {
        #[cfg(feature = "vault-repo")]
        CliEngine::VaultRepo(engine) => {
            let stats = engine.stats();
            let store: &Store = engine.store();
            (
                stats,
                store.len(),
                store.all_entities().filter(|e| !e.stub).count(),
                count_by_type(store),
            )
        }
        CliEngine::Filesystem(engine) => {
            let stats = engine.stats();
            let store: &Store = engine.store();
            (
                stats,
                store.len(),
                store.all_entities().filter(|e| !e.stub).count(),
                count_by_type(store),
            )
        }
    };
    let stubs = total - real;

    let mut edge_pairs: Vec<_> = stats.edge_types.iter().collect();
    edge_pairs.sort_by(|a, b| b.1.cmp(a.1));

    let mut schema_pairs: Vec<(String, usize)> = schema_counts.into_iter().collect();
    schema_pairs.sort_by(|a, b| b.1.cmp(&a.1));

    if ctx.json {
        let payload = StatsPayload {
            total_nodes: total,
            real_nodes: real,
            stub_nodes: stubs,
            total_edges: stats.edge_count,
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
        };
        return print_json(&json!(payload));
    }

    let mut lines = Vec::new();
    lines.push("# Graph stats".to_string());
    lines.push(String::new());
    lines.push(format!("- Nodes: {total} ({real} real, {stubs} stubs)"));
    lines.push(format!("- Edges: {}", stats.edge_count));
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
