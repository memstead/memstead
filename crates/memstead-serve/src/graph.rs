//! Coordinate-free graph projection for the live session stream.
//!
//! Derives `{nodes, edges, communities}` directly from a
//! [`memstead_base::Engine`]'s read surface — the store plus its Louvain
//! communities — with **no** git-branch / `gix` dependency. (The
//! registry's projection reaches the same engine types through
//! `memstead-git-branch` re-exports, so lifting it wholesale would drag
//! `gix` into this server; re-deriving against `memstead-base` directly is
//! the gix-free path the plan calls for.)
//!
//! The payload carries topology + community ids only — no x/y/z. Layout is
//! the viewer's job. Each call recomputes from the live store, so the
//! stream is a sequence of current-truth snapshots, never an append-only
//! log: a deleted or renamed entity simply isn't in the next frame.

use std::collections::HashMap;

use memstead_base::Engine;
use serde::Serialize;

/// A graph node — one entity. Coordinate-free.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphNode {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    /// Compact community index (Louvain cluster), stable within a snapshot.
    pub community: usize,
}

/// A directed edge between two nodes in the same mem. Coordinate-free.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub rel_type: String,
}

/// A Louvain community and its size.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphCommunity {
    pub id: usize,
    pub size: usize,
}

/// The full projection: nodes, edges, communities. No layout coordinates.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphSnapshot {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub communities: Vec<GraphCommunity>,
}

/// Compute `mem`'s current graph projection from `engine`. Recomputed
/// from the live store on every call — never incremental — so deleted and
/// renamed entities never linger across frames.
pub fn graph_projection(engine: &Engine, mem: &str) -> GraphSnapshot {
    let store = engine.store();
    let louvain = engine.communities();

    // Map each Louvain cluster-id string to a compact, deterministic usize.
    let mut cluster_ids: Vec<&String> = louvain.clusters.keys().collect();
    cluster_ids.sort();
    let index_of: HashMap<&str, usize> = cluster_ids
        .iter()
        .enumerate()
        .map(|(i, c)| (c.as_str(), i))
        .collect();

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for e in store.all_entities() {
        if e.mem != mem {
            continue;
        }
        let id_str = e.id.to_string();
        let community = louvain
            .entity_cluster_map
            .get(&id_str)
            .and_then(|c| index_of.get(c.as_str()))
            .copied()
            .unwrap_or(0);
        nodes.push(GraphNode {
            id: id_str.clone(),
            name: e.title.clone(),
            entity_type: e.entity_type.clone(),
            community,
        });
        for edge in store.outgoing(&e.id) {
            // Keep edges between nodes that travel in this snapshot — guards
            // against a dangling target (e.g. a hypothetical cross-mem edge).
            if store.get(&edge.target).map(|t| t.mem == mem).unwrap_or(false) {
                edges.push(GraphEdge {
                    source: id_str.clone(),
                    target: edge.target.to_string(),
                    rel_type: edge.rel_type.clone(),
                });
            }
        }
    }
    // Deterministic order so frames are stable and assertions are simple.
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    edges.sort_by(|a, b| {
        (&a.source, &a.target, &a.rel_type).cmp(&(&b.source, &b.target, &b.rel_type))
    });

    let communities = cluster_ids
        .iter()
        .enumerate()
        .map(|(i, c)| GraphCommunity {
            id: i,
            size: louvain.clusters[*c].entities.len(),
        })
        .collect();

    GraphSnapshot {
        nodes,
        edges,
        communities,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{CONTENT_MEM_NAME, mount_session_engine};
    use indexmap::IndexMap;
    use memstead_base::vcs::{Actor, ClientId};
    use memstead_base::{CreateEntityArgs, DeleteEntityArgs, MountStorage, RelateEntityArgs};
    use memstead_schema::SchemaRef;

    fn schema() -> SchemaRef {
        "default@1.0.0".parse().unwrap()
    }

    fn cli() -> (Actor, ClientId) {
        (Actor::Cli, ClientId { name: "test".to_string(), version: "0".to_string() })
    }

    fn spec_args(title: &str, identity: &str, purpose: &str) -> CreateEntityArgs {
        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), identity.to_string());
        sections.insert("purpose".to_string(), purpose.to_string());
        CreateEntityArgs {
            mem: "sketch".to_string(),
            title: title.to_string(),
            entity_type: "spec".to_string(),
            sections,
            metadata: IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        }
    }

    /// The projection tracks the live store: create adds a node, a body
    /// wiki-link / relate adds an edge, a delete removes the node and its
    /// incident edges — and every frame is recomputed, so the deleted
    /// entity never lingers (not an append-only log).
    #[test]
    fn projection_tracks_create_relate_delete_and_recomputes() {
        let mut engine =
            mount_session_engine(MountStorage::InMemory, schema(), CONTENT_MEM_NAME.to_string(), schema()).unwrap();
        let (actor, client) = cli();

        // Empty mem → empty projection.
        let snap = graph_projection(&engine, "sketch");
        assert!(snap.nodes.is_empty() && snap.edges.is_empty());

        // create Target → one node.
        let target = engine
            .create_entity(spec_args("Target Spec", "ti", "tp"), actor, Some(&client), None)
            .unwrap();
        let snap = graph_projection(&engine, "sketch");
        assert_eq!(snap.nodes.len(), 1, "create adds a node");
        assert_eq!(snap.nodes[0].id, "sketch--target-spec");

        // create Referrer with a body wiki-link → REFERENCES edge + 2 nodes.
        let referrer = engine
            .create_entity(
                spec_args("Referrer Spec", "ri", "relies on [[target-spec]]"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let snap = graph_projection(&engine, "sketch");
        assert_eq!(snap.nodes.len(), 2);
        assert!(
            snap.edges.iter().any(|e| e.source == "sketch--referrer-spec"
                && e.target == "sketch--target-spec"
                && e.rel_type == "REFERENCES"),
            "body wiki-link adds a REFERENCES edge: {:?}",
            snap.edges
        );

        // relate Referrer USES Target → an explicit edge.
        engine
            .relate_entity(
                RelateEntityArgs {
                    source: referrer.id.clone(),
                    expected_hash: Some(referrer.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let snap = graph_projection(&engine, "sketch");
        assert!(snap.edges.iter().any(|e| e.rel_type == "USES"), "relate adds an edge: {:?}", snap.edges);

        // delete Referrer → its node AND incident edges vanish from the next
        // recomputed frame; Target stays.
        let cur = engine.get_entity(&referrer.id).unwrap().content_hash.clone();
        engine
            .delete_entity(
                DeleteEntityArgs { id: referrer.id.clone(), expected_hash: Some(cur) },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let snap = graph_projection(&engine, "sketch");
        assert!(
            !snap.nodes.iter().any(|n| n.id == "sketch--referrer-spec"),
            "delete removes the node: {:?}",
            snap.nodes
        );
        assert!(
            !snap.edges.iter().any(|e| e.source == "sketch--referrer-spec"
                || e.target == "sketch--referrer-spec"),
            "delete removes incident edges: {:?}",
            snap.edges
        );
        assert!(
            snap.nodes.iter().any(|n| n.id == "sketch--target-spec"),
            "target remains after the referrer is deleted"
        );
    }

    /// The payload carries no layout coordinates.
    #[test]
    fn projection_carries_no_layout_coordinates() {
        let mut engine =
            mount_session_engine(MountStorage::InMemory, schema(), CONTENT_MEM_NAME.to_string(), schema()).unwrap();
        let (actor, client) = cli();
        engine
            .create_entity(spec_args("A Node", "i", "p"), actor, Some(&client), None)
            .unwrap();
        let json = serde_json::to_string(&graph_projection(&engine, "sketch")).unwrap();
        assert!(
            !json.contains("\"x\"") && !json.contains("\"y\"") && !json.contains("\"z\""),
            "projection must carry no x/y/z coordinates: {json}"
        );
    }
}
