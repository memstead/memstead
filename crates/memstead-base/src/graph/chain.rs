//! A chain scope: the subgraph reachable from one root along a named
//! rel-type set in one direction — the reduced set the export formats
//! and the topology projection render when a caller asks for a chain
//! instead of a whole mem.
//!
//! One resolver, one walker. The scope is resolved here once, through
//! [`reachable_via`](crate::graph::query::reachable_via) — the same
//! primitive `memstead_search`'s `expand_via` uses, so direction means
//! the same thing on both surfaces: applied at EVERY hop, a pure
//! transitive closure. The renderers then filter their existing
//! per-entity passes by the resolved set; nothing is rendered twice and
//! no second walker exists.

use std::collections::HashSet;

use serde::Serialize;

use crate::entity::EntityId;
use crate::graph::query::{ReachedVia, TraversalDirection, reachable_via};
use crate::runtime_validator::validate_rel_type;

/// What a caller asks for: a root entity, the rel-types to follow, the
/// direction to follow them in, and how many hops.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChainScope {
    /// The entity the walk starts from; it is always part of the set.
    pub root: EntityId,
    /// Rel-types followed at every hop. Validated against the mem's
    /// schema vocabulary before the walk.
    pub via: Vec<String>,
    /// Direction applied at every hop.
    pub direction: TraversalDirection,
    /// Maximum hops; `usize::MAX` for an unbounded walk.
    pub depth: usize,
}

/// A resolved chain: the root plus everything reached, in any mem. The
/// renderers filter to their mem; cross-mem members stay in the set so
/// an edge into one renders as a reached (marked) target, not as an
/// unresolved link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSet {
    pub scope: ChainScope,
    /// Root plus reached ids.
    pub ids: HashSet<EntityId>,
    /// Every reached entity with the edge it was first reached by.
    pub reached: Vec<ReachedVia>,
}

impl ChainSet {
    pub fn contains(&self, id: &EntityId) -> bool {
        self.ids.contains(id)
    }

    /// One line naming the scope, for the headers of scoped renderings.
    pub fn describe(&self) -> String {
        let depth = if self.scope.depth == usize::MAX {
            "unbounded".to_string()
        } else {
            self.scope.depth.to_string()
        };
        format!(
            "root {} via {} direction {} depth {}",
            self.scope.root,
            self.scope.via.join(","),
            self.scope.direction.as_wire(),
            depth
        )
    }
}

impl TraversalDirection {
    /// Stable wire form.
    pub fn as_wire(self) -> &'static str {
        match self {
            TraversalDirection::Out => "out",
            TraversalDirection::In => "in",
            TraversalDirection::Both => "both",
        }
    }

    /// Inverse of [`Self::as_wire`]; `None` for an unknown string.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "out" => Some(TraversalDirection::Out),
            "in" => Some(TraversalDirection::In),
            "both" => Some(TraversalDirection::Both),
            _ => None,
        }
    }

    /// Every wire string.
    pub const WIRE_VALUES: &'static [&'static str] = &["out", "in", "both"];
}

impl crate::Engine {
    /// Resolve `scope` against `mem`: the mem must be mounted, the root
    /// must be a real (non-stub) entity of that mem, `via` must be
    /// non-empty and every rel-type known to the mem's schema
    /// (`INVALID_REL_TYPE` naming the vocabulary otherwise), and the walk
    /// follows `reachable_via` with the scope's direction at every hop.
    pub fn chain_set(&self, mem: &str, scope: &ChainScope) -> Result<ChainSet, crate::EngineError> {
        if self.mount(mem).is_none() {
            return Err(self.unknown_mem_error(mem));
        }
        let root = self
            .store()
            .get(&scope.root)
            .filter(|e| !e.stub)
            .ok_or_else(|| crate::EngineError::NotFound {
                id: scope.root.to_string(),
            })?;
        if root.mem != mem {
            return Err(crate::EngineError::InvalidInput(format!(
                "root {} lives in mem '{}', not in the exported mem '{mem}'",
                scope.root, root.mem
            )));
        }
        if scope.via.is_empty() {
            return Err(crate::EngineError::InvalidInput(
                "a chain needs at least one rel-type in `via`".to_string(),
            ));
        }
        if let Some(schema) = self.schema_for(mem) {
            for rel in &scope.via {
                validate_rel_type(rel, &schema).map_err(crate::EngineError::Validation)?;
            }
        }
        let reached = reachable_via(
            self.store(),
            &scope.root,
            &scope.via,
            scope.depth,
            scope.direction,
        );
        let mut ids: HashSet<EntityId> = reached.iter().map(|r| r.id.clone()).collect();
        ids.insert(scope.root.clone());
        Ok(ChainSet {
            scope: scope.clone(),
            ids,
            reached,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::graph::query::TraversalDirection;
    use crate::storage::MemWriter;

    use super::ChainScope;

    /// root --A--> a1 --B--> a2 ; root --C--> c1 ; back --A--> root
    /// (reachable only against `out`); a1 also cites x in another mem.
    fn engine() -> (crate::Engine, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let seed = |dir: &std::path::Path, files: &[(&str, &str)]| {
            std::fs::create_dir_all(dir).unwrap();
            let writer = crate::storage::FilesystemMemWriter::new(dir.to_path_buf());
            for (name, body) in files {
                writer
                    .write_entity(std::path::Path::new(name), body.as_bytes())
                    .unwrap();
            }
            writer
                .commit("seed", &crate::vcs::CommitContext::internal())
                .unwrap();
        };
        let spec = |title: &str, rels: &str| {
            format!(
                "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# {title}\n\n## Identity\n\n{title}. See [[a2]] and [[c1]].\n{rels}"
            )
        };
        let m = tmp.path().join("m");
        let other = tmp.path().join("other");
        seed(
            &m,
            &[
                (
                    "root.md",
                    &spec(
                        "Root",
                        "\n## Relationships\n\n- **USES**: [[a1]]\n- **PART_OF**: [[c1]]\n",
                    ),
                ),
                (
                    "a1.md",
                    &spec(
                        "A1",
                        "\n## Relationships\n\n- **DEPENDS_ON**: [[a2]]\n- **USES**: [[other--x]]\n",
                    ),
                ),
                ("a2.md", &spec("A2", "")),
                ("c1.md", &spec("C1", "")),
                (
                    "back.md",
                    &spec("Back", "\n## Relationships\n\n- **USES**: [[root]]\n"),
                ),
            ],
        );
        seed(&other, &[("x.md", &spec("X", ""))]);
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
        let engine =
            crate::Engine::from_mounts(vec![mount("m", m), mount("other", other)]).unwrap();
        (engine, tmp)
    }

    fn scope(via: &[&str], direction: TraversalDirection, depth: usize) -> ChainScope {
        ChainScope {
            root: crate::EntityId::canonical("m--root"),
            via: via.iter().map(|s| s.to_string()).collect(),
            direction,
            depth,
        }
    }

    fn ids(engine: &crate::Engine, s: &ChainScope) -> Vec<String> {
        let mut v: Vec<String> = engine
            .chain_set("m", s)
            .unwrap()
            .ids
            .iter()
            .map(|i| i.to_string())
            .collect();
        v.sort();
        v
    }

    /// The set is root plus exactly what the rel-types reach in the
    /// direction, at every hop; the other rel-type and the
    /// against-direction referrer stay out; depth 1 drops the second hop;
    /// `in` walks the other way; cross-mem reached nodes stay in the set.
    #[test]
    fn chain_set_follows_via_and_direction_at_every_hop() {
        let (engine, _tmp) = engine();
        assert_eq!(
            ids(
                &engine,
                &scope(&["USES", "DEPENDS_ON"], TraversalDirection::Out, usize::MAX)
            ),
            vec!["m--a1", "m--a2", "m--root", "other--x"]
        );
        assert_eq!(
            ids(
                &engine,
                &scope(&["USES", "DEPENDS_ON"], TraversalDirection::Out, 1)
            ),
            vec!["m--a1", "m--root"]
        );
        assert_eq!(
            ids(
                &engine,
                &scope(&["USES"], TraversalDirection::In, usize::MAX)
            ),
            vec!["m--back", "m--root"]
        );
        assert_eq!(
            ids(
                &engine,
                &scope(&["PART_OF"], TraversalDirection::Out, usize::MAX)
            ),
            vec!["m--c1", "m--root"]
        );
        let set = engine
            .chain_set(
                "m",
                &scope(&["USES", "DEPENDS_ON"], TraversalDirection::Out, usize::MAX),
            )
            .unwrap();
        assert_eq!(set.reached.len(), 3);
        assert!(
            set.describe()
                .contains("root m--root via USES,DEPENDS_ON direction out depth unbounded")
        );
    }

    /// Refusals: an unknown rel-type names the vocabulary, a missing root
    /// is ENTITY_NOT_FOUND, an empty via and a root outside the mem are
    /// INVALID_INPUT, an unknown mem is UNKNOWN_MEM.
    #[test]
    fn chain_set_refuses_typed() {
        let (engine, _tmp) = engine();
        let err = engine
            .chain_set("m", &scope(&["NOPE"], TraversalDirection::Out, 3))
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_REL_TYPE", "{err}");
        assert!(
            err.details().to_string().contains("USES"),
            "the recovery payload names the vocabulary: {}",
            err.details()
        );
        let missing = ChainScope {
            root: crate::EntityId::canonical("m--missing"),
            ..scope(&["USES"], TraversalDirection::Out, 3)
        };
        assert_eq!(
            engine.chain_set("m", &missing).unwrap_err().code(),
            "ENTITY_NOT_FOUND"
        );
        assert_eq!(
            engine
                .chain_set("m", &scope(&[], TraversalDirection::Out, 3))
                .unwrap_err()
                .code(),
            "INVALID_INPUT"
        );
        assert_eq!(
            engine
                .chain_set("other", &scope(&["USES"], TraversalDirection::Out, 3))
                .unwrap_err()
                .code(),
            "INVALID_INPUT",
            "root in another mem"
        );
        assert_eq!(
            engine
                .chain_set("ghost", &scope(&["USES"], TraversalDirection::Out, 3))
                .unwrap_err()
                .code(),
            "UNKNOWN_MEM"
        );
    }

    /// The scoped renderers carry exactly the chain's in-mem entities,
    /// mark links to excluded entities unresolved, and the unscoped call
    /// stays byte-identical to the pre-scope output.
    #[test]
    fn scoped_renderers_reduce_and_unscoped_stays_identical() {
        let (engine, _tmp) = engine();
        let chain = engine
            .chain_set(
                "m",
                &scope(&["USES", "DEPENDS_ON"], TraversalDirection::Out, usize::MAX),
            )
            .unwrap();

        // HTML
        let full = engine.render_html_export("m", "2026-09-02").unwrap();
        let same = engine
            .render_html_export_scoped("m", "2026-09-02", None)
            .unwrap();
        assert_eq!(full, same, "None is byte-identical to the unscoped export");
        let reduced = engine
            .render_html_export_scoped("m", "2026-09-02", Some(&chain))
            .unwrap();
        for id in ["m--root", "m--a1", "m--a2"] {
            assert!(reduced.contains(&format!("id=\"{id}\"")), "{id} rendered");
        }
        for id in ["m--c1", "m--back"] {
            assert!(!reduced.contains(&format!("id=\"{id}\"")), "{id} excluded");
        }
        assert!(reduced.contains("Chain:"), "header names the chain");
        assert!(
            reduced.contains("unresolved"),
            "links to excluded entities are marked"
        );
        assert!(reduced.contains("3 entities"));

        // llms-txt
        let ctx = crate::engine::export_llms_txt::LlmsTxtContext {
            authority: None,
            href_prefix: String::new(),
            wider_project: Vec::new(),
        };
        let full = engine.render_llms_txt("m", &ctx).unwrap();
        assert_eq!(
            full,
            engine.render_llms_txt_scoped("m", &ctx, None).unwrap()
        );
        let reduced = engine
            .render_llms_txt_scoped("m", &ctx, Some(&chain))
            .unwrap();
        assert!(
            reduced
                .contains("Chain: root m--root via USES,DEPENDS_ON direction out depth unbounded")
        );
        assert!(reduced.contains("Entities: 3"));
        assert!(reduced.contains("# A2"));
        assert!(!reduced.contains("# C1"));
        assert!(!reduced.contains("# Back"));
        // The link to the included a2 resolves; the link to the excluded
        // c1 is left unresolved (the linkifier's plain-text form), never
        // a link to a page this document does not contain.
        assert!(reduced.contains("[A2](entity/m--a2)"), "{reduced}");
        assert!(reduced.contains(" and c1."), "{reduced}");
        assert!(!reduced.contains("entity/m--c1"), "{reduced}");

        // Topology
        let full = engine.mem_topology("m").unwrap();
        assert_eq!(full, engine.mem_topology_scoped("m", None).unwrap());
        let reduced = engine.mem_topology_scoped("m", Some(&chain)).unwrap();
        let node_ids: Vec<&str> = reduced.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(node_ids, vec!["m--a1", "m--a2", "m--root"]);
        let edges: Vec<(String, String, bool)> = reduced
            .edges
            .iter()
            .map(|e| (e.source.clone(), e.target.clone(), e.target_in_mem))
            .collect();
        assert_eq!(
            edges,
            vec![
                ("m--a1".to_string(), "m--a2".to_string(), true),
                ("m--a1".to_string(), "other--x".to_string(), false),
                ("m--root".to_string(), "m--a1".to_string(), true),
            ],
            "edges with both ends in the chain, cross-mem target marked"
        );
    }
}
