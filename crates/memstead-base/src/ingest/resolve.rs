//! Ingest runtime resolution — turn a stored [`Ingest`] config plus the
//! rest of the four-primitive [`PipelineConfigs`] into a [`ResolvedIngest`]:
//! the ingest joined to its projection, and the projection's source facets
//! joined to the mediums they engage, in the shape the selection, backoff,
//! change-detection, and brief-assembly stages all read.
//!
//! This is the engine-side port of the Claude-Code plugin's
//! `workspace-loader.mjs` `assembleIngest` + `loadFourPrimitiveStore`
//! resolution (the structural join). It is a pure transformation over
//! already-loaded configs — no I/O, so it is unit-testable without a
//! workspace. Resolving a source's *change-detection strategy* (which reads
//! the medium's declared strategy and probes the filesystem for a git work
//! tree) is a separate, filesystem-touching concern that lands with the
//! slice drivers that consume it.
//!
//! Where the plugin tolerated dangling references (a projection naming a
//! facet that does not exist resolved to an empty facet view), this port
//! **errors** with a located [`ResolveError`] instead — a dangling
//! reference is a config-integrity bug, and surfacing it beats carrying a
//! half-resolved source into brief assembly. The observable ingest
//! behaviour (which facets/mediums a well-formed config resolves to) is
//! preserved.

use std::path::{Path, PathBuf};

use crate::pipeline::{Facet, Ingest, IngestMode, IngestTrigger, Medium, MediumType, PatternEntry};
use crate::pipeline_store::PipelineConfigs;

/// A projection source resolved to what the run needs: a **primary** facet
/// joined to its medium (the territory to read and write back), or a
/// read-only **reference** mem supplying cross-mem context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedSource {
    /// A source facet joined to the medium it engages.
    Primary(ResolvedPrimarySource),
    /// A read-only reference mem (cross-mem context, never written).
    Reference {
        /// The reference mem's id.
        mem: String,
    },
}

/// A primary source: a facet's selection over a medium, plus the medium's
/// type and pointer (what kind of territory it is and where it lives).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPrimarySource {
    /// The facet's name (as referenced by the projection).
    pub facet_ref: String,
    /// The medium's name (the facet's `medium` field).
    pub medium: String,
    /// What kind of surface the medium references.
    pub medium_type: MediumType,
    /// Where the medium's body lives — path / URL / mem id, opaque here.
    pub medium_pointer: String,
    /// The medium's declared change-detection strategy, if any (`none` /
    /// `git` / `mtime` / `auto`). Unset means `auto` — see
    /// [`resolve_change_strategy`].
    pub declared_change_detection: Option<String>,
    /// The facet's allow/deny selection over the medium. Empty = whole medium.
    pub scope: Vec<PatternEntry>,
    /// A declared deterministic preparation step (e.g. `pdf-to-markdown`).
    /// Unset for every text medium today; a set value is reported
    /// unsupported at run time rather than run against raw content.
    pub preparation: Option<String>,
}

/// An [`Ingest`] joined to its projection and the projection's resolved
/// sources — the runtime shape the orchestration stages consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIngest {
    /// The ingest's name (its file stem in `.memstead/ingests/`).
    pub name: String,
    /// Discovery / refinement / one-shot.
    pub mode: IngestMode,
    /// Loop / manual / on-event.
    pub trigger: IngestTrigger,
    /// How many artifacts a single run processes.
    pub batch_size: u32,
    /// Paths excluded for this ingest's runs, on top of facet scope.
    pub deny_paths: Vec<String>,
    /// The projection reference verbatim (`"<mem>/<name>"`).
    pub projection_ref: String,
    /// The projection's owning mem (the part before the `/`).
    pub projection_mem: String,
    /// The projection's name (the part after the `/`).
    pub projection_name: String,
    /// The projection's intent — prose for the agent (the brief's "about
    /// the source" block).
    pub intent: Option<String>,
    /// The resolved sources, in projection order: primary facets first
    /// (in `source_facets` order), then reference mems.
    pub sources: Vec<ResolvedSource>,
    /// The single mem this ingest's projection writes into.
    pub destination_mem: String,
    /// Free-form projection rules (e.g. one-shot lens `routing`).
    pub rules: Option<serde_json::Value>,
    /// Free-form ingest post-run actions (e.g. one-shot `archive_source`).
    pub post_actions: Option<serde_json::Value>,
}

/// Why [`resolve_ingest`] could not produce a [`ResolvedIngest`]. Every
/// variant names the offending reference and, where useful, what *was*
/// available — so a config typo is diagnosable without re-reading the store.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    /// No ingest with the given name in the store.
    #[error("ingest '{name}' not found; available: {}", fmt_list(available))]
    IngestNotFound {
        /// The requested ingest name.
        name: String,
        /// The ingest names that do exist.
        available: Vec<String>,
    },
    /// The ingest's `projection` field is not the required `"<mem>/<name>"`.
    #[error(
        "ingest '{ingest}' has a malformed projection ref '{projection}'; expected \"<mem>/<name>\""
    )]
    MalformedProjectionRef {
        /// The ingest whose projection ref is malformed.
        ingest: String,
        /// The malformed value.
        projection: String,
    },
    /// The projection the ingest references does not exist.
    #[error(
        "ingest '{ingest}' references projection '{projection_ref}' not found in mem '{mem}'; available: {}",
        fmt_list(available)
    )]
    ProjectionNotFound {
        /// The referencing ingest.
        ingest: String,
        /// The full `"<mem>/<name>"` ref.
        projection_ref: String,
        /// The mem the projection was looked up in.
        mem: String,
        /// The projection names that do exist in that mem.
        available: Vec<String>,
    },
    /// A projection source facet does not exist in the projection's mem.
    #[error(
        "projection '{projection_ref}' references facet '{facet}' not found in mem '{mem}'; available: {}",
        fmt_list(available)
    )]
    FacetNotFound {
        /// The projection that references the missing facet.
        projection_ref: String,
        /// The missing facet name.
        facet: String,
        /// The mem the facet was looked up in.
        mem: String,
        /// The facet names that do exist in that mem.
        available: Vec<String>,
    },
    /// A facet's medium does not exist in the projection's mem.
    #[error(
        "facet '{facet}' references medium '{medium}' not found in mem '{mem}'; available: {}",
        fmt_list(available)
    )]
    MediumNotFound {
        /// The facet whose medium is missing.
        facet: String,
        /// The missing medium name.
        medium: String,
        /// The mem the medium was looked up in.
        mem: String,
        /// The medium names that do exist in that mem.
        available: Vec<String>,
    },
}

/// Render a name list for an error message: `a, b, c` or `(none)`.
fn fmt_list(names: &[String]) -> String {
    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    }
}

/// Resolve one ingest (by name) against the loaded [`PipelineConfigs`].
///
/// Mirrors the plugin's `assembleIngest`: parse the `"<mem>/<name>"`
/// projection ref, find the projection in that mem, and join each of its
/// `source_facets` to the facet (and the facet's medium) in the same mem,
/// then append each `reference_mem` as a read-only source. Every lookup
/// failure is a located [`ResolveError`].
pub fn resolve_ingest(
    configs: &PipelineConfigs,
    ingest_name: &str,
) -> Result<ResolvedIngest, ResolveError> {
    let ingest: &Ingest = configs
        .ingests
        .iter()
        .find(|r| r.name == ingest_name)
        .map(|r| &r.config)
        .ok_or_else(|| ResolveError::IngestNotFound {
            name: ingest_name.to_string(),
            available: configs.ingests.iter().map(|r| r.name.clone()).collect(),
        })?;

    // Projection ref is "<mem>/<name>" — split on the first '/'.
    let projection_ref = ingest.projection.clone();
    let (projection_mem, projection_name) = projection_ref
        .split_once('/')
        .filter(|(mem, name)| !mem.is_empty() && !name.is_empty())
        .ok_or_else(|| ResolveError::MalformedProjectionRef {
            ingest: ingest_name.to_string(),
            projection: projection_ref.clone(),
        })?;
    let projection_mem = projection_mem.to_string();
    let projection_name = projection_name.to_string();

    let projection = configs
        .projections
        .iter()
        .find(|r| r.mem == projection_mem && r.name == projection_name)
        .map(|r| &r.config)
        .ok_or_else(|| ResolveError::ProjectionNotFound {
            ingest: ingest_name.to_string(),
            projection_ref: projection_ref.clone(),
            mem: projection_mem.clone(),
            available: configs
                .projections
                .iter()
                .filter(|r| r.mem == projection_mem)
                .map(|r| r.name.clone())
                .collect(),
        })?;

    // Primary sources: each source facet joined to the medium it engages,
    // both looked up in the projection's owning mem.
    let mut sources =
        Vec::with_capacity(projection.source_facets.len() + projection.reference_mems.len());
    for facet_name in &projection.source_facets {
        let facet: &Facet = configs
            .facets
            .iter()
            .find(|r| r.mem == projection_mem && r.name == *facet_name)
            .map(|r| &r.config)
            .ok_or_else(|| ResolveError::FacetNotFound {
                projection_ref: projection_ref.clone(),
                facet: facet_name.clone(),
                mem: projection_mem.clone(),
                available: configs
                    .facets
                    .iter()
                    .filter(|r| r.mem == projection_mem)
                    .map(|r| r.name.clone())
                    .collect(),
            })?;

        let medium: &Medium = configs
            .mediums
            .iter()
            .find(|r| r.mem == projection_mem && r.name == facet.medium)
            .map(|r| &r.config)
            .ok_or_else(|| ResolveError::MediumNotFound {
                facet: facet_name.clone(),
                medium: facet.medium.clone(),
                mem: projection_mem.clone(),
                available: configs
                    .mediums
                    .iter()
                    .filter(|r| r.mem == projection_mem)
                    .map(|r| r.name.clone())
                    .collect(),
            })?;

        sources.push(ResolvedSource::Primary(ResolvedPrimarySource {
            facet_ref: facet_name.clone(),
            medium: facet.medium.clone(),
            medium_type: medium.medium_type,
            medium_pointer: medium.pointer.clone(),
            declared_change_detection: medium.change_detection.clone(),
            scope: facet.scope.clone(),
            preparation: facet.preparation.clone(),
        }));
    }

    // Reference sources: read-only mems, in projection order, after the
    // primary facets.
    for reference_mem in &projection.reference_mems {
        sources.push(ResolvedSource::Reference {
            mem: reference_mem.clone(),
        });
    }

    Ok(ResolvedIngest {
        name: ingest_name.to_string(),
        mode: ingest.mode,
        trigger: ingest.trigger,
        batch_size: ingest.batch_size,
        deny_paths: ingest.deny_paths.clone(),
        projection_ref,
        projection_mem,
        projection_name,
        intent: projection.intent.clone(),
        sources,
        destination_mem: projection.destination_mem.clone(),
        rules: projection.rules.clone(),
        post_actions: ingest.post_actions.clone(),
    })
}

/// A primary source's resolved change-detection strategy — how "what
/// changed since the last synced pass" is computed for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeStrategy {
    /// No change detection — the source is re-roamed whole (inert signal).
    None,
    /// Git-commit diff between the baseline commit and current `HEAD`.
    Git,
    /// Filesystem `(mtime, size)` stat-map digest + diff.
    Mtime,
    /// Graph snapshot diff (the source mem's snapshot token).
    Graph,
}

/// Resolve a primary source's [`ChangeStrategy`], mirroring the plugin's
/// `resolveChangeDetection`. A graph-typed medium always uses [`Graph`]
/// (its signal is the source mem's snapshot token, which the engine
/// provides). Otherwise the medium's declared strategy wins
/// (`none`/`git`/`mtime`); `auto` — the default when unset, and any
/// unrecognized value — probes for a git work tree over the medium pointer
/// (resolved against `workspace_root`): present → [`Git`], absent →
/// [`Mtime`].
///
/// [`Graph`]: ChangeStrategy::Graph
/// [`Git`]: ChangeStrategy::Git
/// [`Mtime`]: ChangeStrategy::Mtime
pub fn resolve_change_strategy(
    source: &ResolvedPrimarySource,
    workspace_root: &Path,
) -> ChangeStrategy {
    if source.medium_type == MediumType::Graph {
        return ChangeStrategy::Graph;
    }
    match source.declared_change_detection.as_deref() {
        Some("none") => ChangeStrategy::None,
        Some("git") => ChangeStrategy::Git,
        Some("mtime") => ChangeStrategy::Mtime,
        // `auto`, unset, or any unrecognized value: probe the filesystem.
        _ => {
            let base = if source.medium_pointer.is_empty() {
                workspace_root.to_path_buf()
            } else {
                // `Path::join` yields the pointer verbatim when it is
                // absolute, matching the plugin's `resolve(root, pointer)`.
                workspace_root.join(&source.medium_pointer)
            };
            if find_git_root(&base).is_some() {
                ChangeStrategy::Git
            } else {
                ChangeStrategy::Mtime
            }
        }
    }
}

/// Walk up from `start` looking for a `.git` entry (a directory *or* a file
/// — a submodule/worktree gitlink is a file), returning the directory that
/// contains it (the git work-tree root), or `None`. Bounded to 64 ancestors.
/// Pure filesystem, no subprocess — deterministic for tests.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    for _ in 0..64 {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{PatternMode, Projection};
    use crate::pipeline_store::{MemPipelineRecord, PipelineRecord};

    fn medium(mem: &str, name: &str, ty: MediumType, pointer: &str) -> MemPipelineRecord<Medium> {
        MemPipelineRecord {
            mem: mem.to_string(),
            name: name.to_string(),
            config: Medium {
                name: name.to_string(),
                medium_type: ty,
                pointer: pointer.to_string(),
                change_detection: None,
            },
        }
    }

    fn primary(
        medium_type: MediumType,
        pointer: &str,
        declared: Option<&str>,
    ) -> ResolvedPrimarySource {
        ResolvedPrimarySource {
            facet_ref: "f".to_string(),
            medium: "m".to_string(),
            medium_type,
            medium_pointer: pointer.to_string(),
            declared_change_detection: declared.map(str::to_string),
            scope: vec![],
            preparation: None,
        }
    }

    fn facet(
        mem: &str,
        name: &str,
        medium: &str,
        scope: Vec<PatternEntry>,
    ) -> MemPipelineRecord<Facet> {
        MemPipelineRecord {
            mem: mem.to_string(),
            name: name.to_string(),
            config: Facet {
                name: name.to_string(),
                medium: medium.to_string(),
                scope,
                engagement: None,
                preparation: None,
            },
        }
    }

    fn projection(
        mem: &str,
        name: &str,
        source_facets: &[&str],
        reference_mems: &[&str],
        destination_mem: &str,
    ) -> MemPipelineRecord<Projection> {
        MemPipelineRecord {
            mem: mem.to_string(),
            name: name.to_string(),
            config: Projection {
                intent: Some(format!("intent of {name}")),
                source_facets: source_facets.iter().map(|s| s.to_string()).collect(),
                reference_mems: reference_mems.iter().map(|s| s.to_string()).collect(),
                destination_mem: destination_mem.to_string(),
                rules: None,
            },
        }
    }

    fn ingest(name: &str, projection: &str) -> PipelineRecord<Ingest> {
        PipelineRecord {
            name: name.to_string(),
            config: Ingest {
                projection: projection.to_string(),
                mode: IngestMode::Discovery,
                trigger: IngestTrigger::Loop,
                batch_size: 20,
                deny_paths: vec!["VISION.md".to_string()],
                post_actions: None,
            },
        }
    }

    fn allow(path: &str) -> PatternEntry {
        PatternEntry {
            path: path.to_string(),
            mode: PatternMode::Allow,
        }
    }

    /// A well-formed ingest resolves to its projection, joins each source
    /// facet to its medium (type/pointer/scope/preparation), and appends
    /// reference mems as read-only sources after the primaries.
    #[test]
    fn resolves_a_well_formed_ingest() {
        let configs = PipelineConfigs {
            mediums: vec![medium(
                "macos",
                "source-tree",
                MediumType::Codebase,
                "../macos",
            )],
            facets: vec![facet(
                "macos",
                "source-files",
                "source-tree",
                vec![allow("../macos/**/*.swift")],
            )],
            projections: vec![projection(
                "macos",
                "macos-graph",
                &["source-files"],
                &["engine"],
                "macos",
            )],
            ingests: vec![ingest("macos", "macos/macos-graph")],
        };

        let r = resolve_ingest(&configs, "macos").unwrap();
        assert_eq!(r.mode, IngestMode::Discovery);
        assert_eq!(r.batch_size, 20);
        assert_eq!(r.deny_paths, ["VISION.md"]);
        assert_eq!(r.projection_mem, "macos");
        assert_eq!(r.projection_name, "macos-graph");
        assert_eq!(r.intent.as_deref(), Some("intent of macos-graph"));
        assert_eq!(r.destination_mem, "macos");
        assert_eq!(r.sources.len(), 2);
        match &r.sources[0] {
            ResolvedSource::Primary(p) => {
                assert_eq!(p.facet_ref, "source-files");
                assert_eq!(p.medium, "source-tree");
                assert_eq!(p.medium_type, MediumType::Codebase);
                assert_eq!(p.medium_pointer, "../macos");
                assert_eq!(p.declared_change_detection, None);
                assert_eq!(p.scope, vec![allow("../macos/**/*.swift")]);
                assert_eq!(p.preparation, None);
            }
            other => panic!("expected primary source first, got {other:?}"),
        }
        assert_eq!(
            r.sources[1],
            ResolvedSource::Reference {
                mem: "engine".to_string()
            },
            "reference mem follows the primary facets"
        );
    }

    /// An unknown ingest name errors with the available list.
    #[test]
    fn unknown_ingest_errors_with_available() {
        let configs = PipelineConfigs {
            ingests: vec![ingest("a", "m/p"), ingest("b", "m/p")],
            ..Default::default()
        };
        let err = resolve_ingest(&configs, "c").unwrap_err();
        assert_eq!(
            err,
            ResolveError::IngestNotFound {
                name: "c".to_string(),
                available: vec!["a".to_string(), "b".to_string()],
            }
        );
    }

    /// A projection ref without a `/` (or with an empty half) is malformed.
    #[test]
    fn malformed_projection_ref_errors() {
        for bad in ["noslash", "/name", "mem/"] {
            let configs = PipelineConfigs {
                ingests: vec![ingest("i", bad)],
                ..Default::default()
            };
            let err = resolve_ingest(&configs, "i").unwrap_err();
            assert!(
                matches!(err, ResolveError::MalformedProjectionRef { .. }),
                "'{bad}' should be malformed, got {err:?}"
            );
        }
    }

    /// A projection ref pointing at a missing projection errors with the
    /// available projections in that mem.
    #[test]
    fn missing_projection_errors_with_available() {
        let configs = PipelineConfigs {
            projections: vec![projection("macos", "other", &[], &[], "macos")],
            ingests: vec![ingest("i", "macos/macos-graph")],
            ..Default::default()
        };
        let err = resolve_ingest(&configs, "i").unwrap_err();
        assert_eq!(
            err,
            ResolveError::ProjectionNotFound {
                ingest: "i".to_string(),
                projection_ref: "macos/macos-graph".to_string(),
                mem: "macos".to_string(),
                available: vec!["other".to_string()],
            }
        );
    }

    /// A projection whose source facet does not exist errors, located.
    #[test]
    fn dangling_facet_errors() {
        let configs = PipelineConfigs {
            projections: vec![projection("macos", "p", &["missing-facet"], &[], "macos")],
            ingests: vec![ingest("i", "macos/p")],
            ..Default::default()
        };
        let err = resolve_ingest(&configs, "i").unwrap_err();
        assert!(matches!(
            err,
            ResolveError::FacetNotFound { ref facet, .. } if facet == "missing-facet"
        ));
    }

    /// A facet whose medium does not exist errors, located.
    #[test]
    fn dangling_medium_errors() {
        let configs = PipelineConfigs {
            facets: vec![facet("macos", "f", "missing-medium", vec![])],
            projections: vec![projection("macos", "p", &["f"], &[], "macos")],
            ingests: vec![ingest("i", "macos/p")],
            ..Default::default()
        };
        let err = resolve_ingest(&configs, "i").unwrap_err();
        assert!(matches!(
            err,
            ResolveError::MediumNotFound { ref medium, .. } if medium == "missing-medium"
        ));
    }

    /// A graph-typed medium always resolves to the graph strategy,
    /// regardless of any declared value.
    #[test]
    fn graph_medium_always_uses_graph_strategy() {
        let root = Path::new("/nonexistent");
        assert_eq!(
            resolve_change_strategy(&primary(MediumType::Graph, "", None), root),
            ChangeStrategy::Graph
        );
        // Even a declared override does not change a graph medium.
        assert_eq!(
            resolve_change_strategy(&primary(MediumType::Graph, "", Some("mtime")), root),
            ChangeStrategy::Graph
        );
    }

    /// A declared `none`/`git`/`mtime` wins for a non-graph medium.
    #[test]
    fn declared_strategy_wins_for_non_graph() {
        let root = Path::new("/nonexistent");
        for (declared, expected) in [
            ("none", ChangeStrategy::None),
            ("git", ChangeStrategy::Git),
            ("mtime", ChangeStrategy::Mtime),
        ] {
            assert_eq!(
                resolve_change_strategy(&primary(MediumType::Codebase, "x", Some(declared)), root),
                expected,
                "declared '{declared}'"
            );
        }
    }

    /// `auto` (unset, or an unrecognized value) probes the filesystem: a
    /// pointer under a git work tree → git, otherwise → mtime.
    #[test]
    fn auto_probes_for_a_git_work_tree() {
        let git = tempfile::tempdir().unwrap();
        std::fs::create_dir(git.path().join(".git")).unwrap();
        std::fs::create_dir(git.path().join("sub")).unwrap();
        let plain = tempfile::tempdir().unwrap();

        // Unset → auto → probe. Pointer resolves under the git root → Git.
        assert_eq!(
            resolve_change_strategy(&primary(MediumType::Codebase, "sub", None), git.path()),
            ChangeStrategy::Git
        );
        // An unrecognized declared value also falls through to the probe.
        assert_eq!(
            resolve_change_strategy(
                &primary(MediumType::Codebase, "sub", Some("weird")),
                git.path()
            ),
            ChangeStrategy::Git
        );
        // No git work tree over the pointer → Mtime.
        assert_eq!(
            resolve_change_strategy(&primary(MediumType::Filesystem, ".", None), plain.path()),
            ChangeStrategy::Mtime
        );
    }

    /// `find_git_root` returns the containing directory of a `.git` entry,
    /// walking up from a nested start, and `None` when there is none.
    #[test]
    fn find_git_root_walks_up() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        let nested = root.path().join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();

        assert_eq!(
            find_git_root(&nested).as_deref(),
            Some(root.path()),
            "walks up to the work-tree root"
        );

        let plain = tempfile::tempdir().unwrap();
        assert_eq!(find_git_root(plain.path()), None, "no .git anywhere above");
    }
}
