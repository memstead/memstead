//! `memstead status` projection view (bundle plan `03-projection-promotion`,
//! decision D11).
//!
//! The `projections` array the status payload carries alongside the graph
//! counts: one entry per v1 binding, reporting its declared operations, each
//! source's baseline tokens + resolved change-detection signal, and the
//! pending/disposed advance counts. Purely read-only — it loads the v1 binding
//! store, reads the destination mem's `sync_state`, resolves each source's
//! [`ChangeStrategy`], and reads the durable advance store. No mutation, no
//! scheduling.
//!
//! The `signal` is the *resolved* change-detection strategy or the literal
//! `"none"` (E1's visible-NoSignal) — never a fabricated token: a
//! detection-less source renders `"none"`, not a fake green.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::Engine;
use crate::ingest::advance::read_advance_store;
use crate::ingest::resolve::{
    ChangeStrategy, ResolvedSource, resolve_binding_run, resolve_change_strategy,
};
use crate::pipeline_store::load_pipeline_configs;

/// One source facet's (or reference mem's) baseline + signal state (D11). Keyed
/// in [`ProjectionStatus::state`] by the facet-or-refmem name — the same key
/// space the `sync_state` map uses (`<binding>/<facet>#synced`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FacetState {
    /// The `#synced` baseline token, or `None` (rendered `null`) when the
    /// source has never been synced.
    pub synced: Option<String>,
    /// The `#verified` baseline token, or `None` when never verified.
    pub verified: Option<String>,
    /// The resolved change-detection strategy — `git` / `mtime` / `graph` — or
    /// `none` (E1's visible-NoSignal). Never a fabricated token.
    pub signal: String,
}

/// The advance counts (D11): how many artifacts a frozen advance slice still
/// has pending versus how many have been disposed. Both zero when no advance
/// is in flight for the binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdvanceCounts {
    /// Undisposed artifacts remaining in the frozen slice.
    pub pending: usize,
    /// Artifacts disposed so far.
    pub disposed: usize,
}

/// One binding's status entry (D11). The `rollup` verdict is a later additive
/// E3b field — deliberately absent here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectionStatus {
    /// The canonical binding id `<mem>/<stem>` (D3).
    pub binding: String,
    /// The mem this binding writes into.
    pub destination_mem: String,
    /// The operations the binding declares — `build` always, plus `sync` /
    /// `verify` when their blocks are present.
    pub operations: Vec<String>,
    /// Per source facet-or-refmem state, keyed by the facet/mem name.
    pub state: BTreeMap<String, FacetState>,
    /// Pending / disposed advance counts for the binding.
    pub advance: AdvanceCounts,
}

/// Map a resolved [`ChangeStrategy`] to its `signal` string (D11). `None`
/// renders the literal `"none"` — E1's visible-NoSignal, never a fake token.
fn signal_of(strategy: ChangeStrategy) -> &'static str {
    match strategy {
        ChangeStrategy::None => "none",
        ChangeStrategy::Git => "git",
        ChangeStrategy::Mtime => "mtime",
        ChangeStrategy::Graph => "graph",
    }
}

/// Build the `projections` array for `memstead status` (D11) from the v1
/// binding store rooted at `workspace_root`, reading baselines off `engine`'s
/// destination-mem `sync_state` and the durable advance store.
///
/// Read-only and best-effort: a workspace with no v1 binding store (or one
/// whose store fails to load — e.g. a not-yet-migrated legacy layout) yields an
/// empty array rather than failing the whole status call. A binding whose
/// sources cannot be resolved (dangling facet/medium) contributes its
/// operations + advance counts with an empty `state` map.
pub fn projection_status(engine: &Engine, workspace_root: &Path) -> Vec<ProjectionStatus> {
    let Ok(configs) = load_pipeline_configs(workspace_root) else {
        return Vec::new();
    };

    let mut out = Vec::with_capacity(configs.bindings.len());
    for record in &configs.bindings {
        let binding_id = format!("{}/{}", record.mem, record.name);
        let binding = &record.config;

        let mut operations = Vec::new();
        if binding.operations.build.is_some() {
            operations.push("build".to_string());
        }
        if binding.operations.sync.is_some() {
            operations.push("sync".to_string());
        }
        if binding.operations.verify.is_some() {
            operations.push("verify".to_string());
        }

        // Baselines live on the destination mem's config `sync_state` (D4).
        let sync_state = engine
            .mem_config_for(&binding.destination_mem)
            .map(|c| c.sync_state.clone())
            .unwrap_or_default();

        // Resolve the binding's sources so each facet's change-detection
        // strategy (its `signal`) is the same one the cursor/brief path uses.
        let mut state = BTreeMap::new();
        if let Ok(resolved) = resolve_binding_run(&configs, &binding_id, binding) {
            for source in &resolved.sources {
                let (facet, signal) = match source {
                    ResolvedSource::Primary(p) => (
                        p.facet_ref.clone(),
                        signal_of(resolve_change_strategy(p, workspace_root)).to_string(),
                    ),
                    // Reference mems are graph-detected by definition (the
                    // source mem's snapshot token).
                    ResolvedSource::Reference { mem } => (mem.clone(), "graph".to_string()),
                };
                let synced = sync_state
                    .get(&format!("{binding_id}/{facet}#synced"))
                    .cloned();
                let verified = sync_state
                    .get(&format!("{binding_id}/{facet}#verified"))
                    .cloned();
                state.insert(
                    facet,
                    FacetState {
                        synced,
                        verified,
                        signal,
                    },
                );
            }
        }

        // Durable advance store (D7) — absent = nothing in flight (0/0).
        let advance = match read_advance_store(workspace_root, &record.mem, &record.name) {
            Ok(Some(s)) => AdvanceCounts {
                pending: s.pending(),
                disposed: s.disposed(),
            },
            _ => AdvanceCounts {
                pending: 0,
                disposed: 0,
            },
        };

        out.push(ProjectionStatus {
            binding: binding_id,
            destination_mem: binding.destination_mem.clone(),
            operations,
            state,
            advance,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{
        BINDING_VERSION, BindingV1, BuildMode, BuildOperation, CoverageSemantics, Operations,
        SyncOperation,
    };
    use crate::pipeline::{Facet, IngestTrigger, Medium, MediumType, PatternEntry, PatternMode};
    use crate::pipeline_store::{write_binding, write_facet, write_medium};
    use crate::storage::FilesystemMemWriter;
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};
    use tempfile::TempDir;

    /// A workspace with one folder mem `engine` (also a git source tree), a v1
    /// binding `engine/graph` over a `source-tree` facet (codebase / git), and
    /// a seeded `#synced` baseline — the projection status reports the binding's
    /// operations, the `git` signal, the synced token, and 0/0 advance.
    #[test]
    fn projection_status_reports_operations_signal_and_baseline() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // Mem config so `sync_state` can be read/written.
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("config.json"),
            br#"{"format":1,"schema":"default@1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "[workspace]\n",
        )
        .unwrap();
        // A git work tree so the codebase medium resolves the `git` strategy.
        let out = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success());

        // The v1 binding + its facet/medium.
        write_medium(
            root,
            "engine",
            "graph",
            &Medium {
                name: "graph".to_string(),
                medium_type: MediumType::Codebase,
                pointer: String::new(),
                change_detection: Some("git".to_string()),
            },
        )
        .unwrap();
        write_facet(
            root,
            "engine",
            "graph",
            &Facet {
                name: "graph".to_string(),
                medium: "graph".to_string(),
                scope: vec![PatternEntry {
                    path: "**/*.rs".to_string(),
                    mode: PatternMode::Allow,
                }],
                engagement: None,
                preparation: None,
            },
        )
        .unwrap();
        write_binding(
            root,
            "engine",
            "graph",
            &BindingV1 {
                version: BINDING_VERSION,
                intent: None,
                source_facets: vec!["graph".to_string()],
                reference_mems: Vec::new(),
                destination_mem: "engine".to_string(),
                deny_paths: Vec::new(),
                coverage_semantics: CoverageSemantics::Exhaustive,
                rules: None,
                prune: None,
                operations: Operations {
                    build: Some(BuildOperation {
                        mode: BuildMode::Discovery,
                        trigger: IngestTrigger::Loop,
                        batch_size: 20,
                        post_actions: None,
                    }),
                    sync: Some(SyncOperation {
                        trigger: IngestTrigger::Manual,
                        batch_size: 20,
                    }),
                    verify: None,
                },
            },
        )
        .unwrap();

        let mount = Mount {
            mem: "engine".to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: MountStorage::Folder {
                path: root.to_path_buf(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        };
        let mut engine = Engine::from_mounts(vec![(
            mount,
            Box::new(FilesystemMemWriter::new(root.to_path_buf()))
                as Box<dyn crate::backend::MemBackend>,
        )])
        .unwrap();
        engine
            .set_mem_sync_state("engine", "engine/graph/graph#synced", "deadbeef", None)
            .unwrap();

        let ps = projection_status(&engine, root);
        assert_eq!(ps.len(), 1);
        let p = &ps[0];
        assert_eq!(p.binding, "engine/graph");
        assert_eq!(p.destination_mem, "engine");
        assert_eq!(p.operations, vec!["build".to_string(), "sync".to_string()]);
        let facet = p.state.get("graph").expect("the source facet's state");
        assert_eq!(facet.signal, "git");
        assert_eq!(facet.synced.as_deref(), Some("deadbeef"));
        assert_eq!(facet.verified, None);
        assert_eq!(
            p.advance,
            AdvanceCounts {
                pending: 0,
                disposed: 0
            }
        );
    }

    /// A workspace with no v1 binding store yields an empty array — status
    /// never fails because a workspace declares no projections.
    #[test]
    fn projection_status_empty_without_bindings() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("config.json"),
            br#"{"format":1,"schema":"default@1.0.0"}"#,
        )
        .unwrap();
        let mount = Mount {
            mem: "engine".to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: MountStorage::Folder {
                path: root.to_path_buf(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        };
        let engine = Engine::from_mounts(vec![(
            mount,
            Box::new(FilesystemMemWriter::new(root.to_path_buf()))
                as Box<dyn crate::backend::MemBackend>,
        )])
        .unwrap();
        assert!(projection_status(&engine, root).is_empty());
    }
}
