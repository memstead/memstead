//! Bulk per-mem topology projection — `{nodes, edges, communities}`
//! for one mem, in one call, coordinate-free.
//!
//! "Projection" here is a render view of the live store, not the
//! pipeline's projection-binding; the two nouns are unrelated. The
//! shape exists for UI consumers (HTTP surfaces, UniFFI, CLI) that
//! would otherwise assemble topology from paged list reads plus
//! per-entity relation calls — the measured N+1 path — or re-derive it
//! per surface (the `serve/` precedent this module hoists).
//!
//! Contract decisions, deliberate and shared by every consumer:
//!
//! - **Coordinate-free.** Layout is the consumer's job; no x/y/z ever.
//! - **Unpaged, untruncated.** The design target is 1,000–5,000
//!   entities per mem, served whole. Any future scale cut must be
//!   declared on-surface, never silent.
//! - **Cross-mem edges ride, source-in-mem only.** An edge whose
//!   source entity lives in the projected mem is included even when
//!   its target lives elsewhere (`target_in_mem: false`) — the
//!   engine's established asymmetric convention (community bridges,
//!   health). Composing every mem's projection therefore yields the
//!   complete workspace graph with each cross-mem edge exactly once.
//! - **Global community ids.** Assignments come from the
//!   workspace-global Louvain partition (never re-run per mem), keyed
//!   by the partition's own cluster ids — the same cluster carries the
//!   same id across per-mem projections from one snapshot, so
//!   multi-mem consumers can compose without renumbering.

use serde::Serialize;

/// One entity as a topology node. Coordinate-free by design.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TopologyNode {
    pub id: String,
    pub title: String,
    pub entity_type: String,
    /// Global Louvain cluster id from the workspace partition; `None`
    /// when the partition carries no assignment for this entity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub community: Option<String>,
    /// True for stub entities (unresolved references) — consumers
    /// choose their own rendering; nothing is dropped here.
    pub stub: bool,
}

/// One directed relationship edge whose source lives in the projected
/// mem.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TopologyEdge {
    pub source: String,
    pub target: String,
    pub rel_type: String,
    /// `false` marks a cross-mem edge: the target entity lives outside
    /// the projected mem (reported here, at the source mem, only).
    pub target_in_mem: bool,
}

/// One global cluster's presence in the projected mem.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TopologyCommunity {
    /// Global cluster id (workspace partition).
    pub id: String,
    /// How many of the cluster's members live in the projected mem.
    pub size_in_mem: usize,
}

/// The full per-mem projection.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MemTopology {
    pub mem: String,
    pub nodes: Vec<TopologyNode>,
    pub edges: Vec<TopologyEdge>,
    /// Clusters with at least one member in the mem, sorted by id.
    pub communities: Vec<TopologyCommunity>,
}

impl crate::Engine {
    /// Project `mem`'s current topology from the live store — every
    /// entity in the mem, every relationship edge sourced in the mem
    /// (cross-mem targets marked), and the mem's community roster from
    /// the global partition. Recomputed on every call, never
    /// incremental: deleted or renamed entities are simply absent from
    /// the next projection. Unknown mems refuse with
    /// [`crate::EngineError::UnknownMem`].
    pub fn mem_topology(&self, mem: &str) -> Result<MemTopology, crate::EngineError> {
        if self.mount(mem).is_none() {
            return Err(crate::EngineError::UnknownMem(mem.to_string()));
        }
        let store = self.store();
        let louvain = self.communities();

        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut community_sizes: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();

        for entity in store.all_entities() {
            if entity.mem != mem {
                continue;
            }
            let id = entity.id.to_string();
            let community = louvain.entity_cluster_map.get(&id).cloned();
            if let Some(cluster) = &community {
                *community_sizes.entry(cluster.clone()).or_insert(0) += 1;
            }
            nodes.push(TopologyNode {
                id: id.clone(),
                title: entity.title.clone(),
                entity_type: entity.entity_type.clone(),
                community,
                stub: entity.stub,
            });
            for edge in store.outgoing(&entity.id) {
                let target_in_mem = store
                    .get(&edge.target)
                    .map(|t| t.mem == mem)
                    .unwrap_or(false);
                edges.push(TopologyEdge {
                    source: id.clone(),
                    target: edge.target.to_string(),
                    rel_type: edge.rel_type.clone(),
                    target_in_mem,
                });
            }
        }

        // Deterministic order: stable frames, simple assertions.
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        edges.sort_by(|a, b| {
            (&a.source, &a.target, &a.rel_type).cmp(&(&b.source, &b.target, &b.rel_type))
        });
        let communities = community_sizes
            .into_iter()
            .map(|(id, size_in_mem)| TopologyCommunity { id, size_in_mem })
            .collect();

        Ok(MemTopology {
            mem: mem.to_string(),
            nodes,
            edges,
            communities,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::MemWriter;

    /// Two folder mems, one cross-mem edge (seeded in the markdown so
    /// no mutation-time policy is involved). The projection must keep
    /// the cross-mem edge at its source mem only, use global cluster
    /// ids, carry no layout fields, and refuse unknown mems typed.
    fn two_mem_engine() -> (crate::Engine, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let seed = |dir: &std::path::Path, files: &[(&str, &str)]| {
            std::fs::create_dir_all(dir).unwrap();
            let writer = crate::storage::FilesystemMemWriter::new(dir.to_path_buf());
            for (name, body) in files {
                writer
                    .write_entity(std::path::Path::new(name), body.as_bytes())
                    .unwrap();
            }
            // write_entity buffers; the commit flushes to disk.
            writer
                .commit("seed", &crate::vcs::CommitContext::internal())
                .unwrap();
        };
        let specs_dir = tmp.path().join("specs");
        let vendor_dir = tmp.path().join("vendor");
        seed(
            &specs_dir,
            &[
                (
                    "alpha.md",
                    "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nA.\n\n## Relationships\n\n- **USES**: [[beta]]\n- **DEPENDS_ON**: [[vendor--gamma]]\n",
                ),
                (
                    "beta.md",
                    "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Beta\n\n## Identity\n\nB.\n",
                ),
            ],
        );
        seed(
            &vendor_dir,
            &[(
                "gamma.md",
                "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Gamma\n\n## Identity\n\nC.\n",
            )],
        );
        let mount = |mem: &str, path: std::path::PathBuf| {
            (
                crate::Mount {
                    mem: mem.to_string(),
                    schema: Some(memstead_schema::SchemaRef::new(
                        "default",
                        semver::Version::new(1, 0, 0),
                    )),
                    storage: crate::MountStorage::Folder { path: path.clone() },
                    capability: crate::MountCapability::Write,
                    lifecycle: crate::MountLifecycle::Eager,
                    cross_linkable: true,
                    migration_target: None,
                },
                Box::new(crate::storage::FilesystemMemWriter::new(path))
                    as Box<dyn crate::MemBackend>,
            )
        };
        let engine = crate::Engine::from_mounts(vec![
            mount("specs", specs_dir),
            mount("vendor", vendor_dir),
        ])
        .unwrap();
        (engine, tmp)
    }

    #[test]
    fn projection_is_faithful_composable_and_coordinate_free() {
        let (engine, _tmp) = two_mem_engine();

        let specs = engine.mem_topology("specs").unwrap();
        let vendor = engine.mem_topology("vendor").unwrap();

        // Node fidelity: exactly the mem's entities.
        let specs_ids: Vec<&str> = specs.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(specs_ids, vec!["specs--alpha", "specs--beta"]);
        assert_eq!(vendor.nodes.len(), 1);
        assert_eq!(vendor.nodes[0].id, "vendor--gamma");

        // Cross-mem composition: the DEPENDS_ON edge rides at its
        // source mem with the target marked outside; the target mem's
        // projection does not repeat it — union carries it once.
        let cross: Vec<_> = specs
            .edges
            .iter()
            .filter(|e| e.rel_type == "DEPENDS_ON")
            .collect();
        assert_eq!(cross.len(), 1);
        assert_eq!(cross[0].target, "vendor--gamma");
        assert!(!cross[0].target_in_mem);
        assert!(
            vendor.edges.iter().all(|e| e.rel_type != "DEPENDS_ON"),
            "cross-mem edge must not repeat at the target mem: {:?}",
            vendor.edges
        );
        // In-mem edge is marked in-mem.
        let uses: Vec<_> = specs
            .edges
            .iter()
            .filter(|e| e.rel_type == "USES")
            .collect();
        assert_eq!(uses.len(), 1);
        assert!(uses[0].target_in_mem);

        // Community stability: assignments equal the global partition's
        // and share ids across the two projections' rosters.
        let louvain = engine.communities();
        for node in specs.nodes.iter().chain(vendor.nodes.iter()) {
            assert_eq!(
                node.community.as_ref(),
                louvain.entity_cluster_map.get(&node.id),
                "node {} must carry the global assignment",
                node.id
            );
        }

        // Coordinate-free: no layout key anywhere in the payload.
        let json = serde_json::to_string(&specs).unwrap();
        for forbidden in ["\"x\":", "\"y\":", "\"z\":", "position", "layout"] {
            assert!(
                !json.contains(forbidden),
                "layout field leaked: {forbidden}"
            );
        }

        // Unknown mem refuses typed.
        let err = engine.mem_topology("ghost").unwrap_err();
        assert!(matches!(err, crate::EngineError::UnknownMem(m) if m == "ghost"));
    }
}
