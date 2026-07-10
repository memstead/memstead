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

use crate::binding::{BindingV1, BuildMode, ResolvedBinding, medium_capabilities};
use crate::pipeline::{Facet, IngestTrigger, Medium, MediumType, PatternEntry};
use crate::pipeline_store::{BindingConfigs, PipelineConfigs};

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
    /// The facet's allow/deny selection over the medium. A facet with **no
    /// allow patterns is *unscoped*** — a typed refusal at run time (see
    /// [`super::cursor`]'s empty-scope semantics), not "whole medium". A facet
    /// that truly wants everything writes `**/*`.
    pub scope: Vec<PatternEntry>,
    /// A declared deterministic preparation step (e.g. `pdf-to-markdown`).
    /// Unset for every text medium today; a set value is reported
    /// unsupported at run time rather than run against raw content.
    pub preparation: Option<String>,
}

/// A v1 binding joined to its resolved sources — the runtime shape the
/// orchestration stages (cursor, brief, selection) consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIngest {
    /// The run's identity — the canonical binding id `<mem>/<stem>` (D3), the
    /// string every downstream key (sync_state, selection cache, brief header)
    /// derives from.
    pub name: String,
    /// The build mode — discovery / one-shot (`refinement` is deleted).
    /// Defaults to discovery when the binding declares no `build` block.
    pub mode: BuildMode,
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
    /// No binding with the given id in the store.
    #[error("binding '{name}' not found; available: {}", fmt_list(available))]
    BindingNotFound {
        /// The requested binding id.
        name: String,
        /// The binding ids that do exist.
        available: Vec<String>,
    },
    /// The binding id is not the required `"<mem>/<name>"`.
    #[error("malformed binding id '{projection}'; expected \"<mem>/<name>\"")]
    MalformedProjectionRef {
        /// The binding whose id is malformed.
        ingest: String,
        /// The malformed value.
        projection: String,
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

/// Resolve a v1 [`BindingV1`]'s **primary** sources (facet + medium) against
/// the loaded [`PipelineConfigs`], producing a [`ResolvedBinding`] ready for
/// [`crate::binding::hash_binding`] / [`crate::binding::validate_binding`].
///
/// The binding replaces the gen-2 projection, so — unlike [`resolve_ingest`] —
/// there is no projection record to look up: the binding *is* the declaration.
/// `binding_id` is the canonical `<mem>/<stem>` (D3); its `<mem>` half is the
/// tier the facets/mediums live under (the same mem `resolve_ingest` joins in).
/// Reference mems are not resolved here (only primary facets carry capability
/// constraints), matching [`crate::binding_migrate::resolve_migrated_binding`].
///
/// Every lookup failure is a located [`ResolveError`] (reusing the ingest
/// resolver's variants); a malformed `binding_id` is
/// [`ResolveError::MalformedProjectionRef`]. Pure — no I/O.
pub fn resolve_binding(
    configs: &PipelineConfigs,
    binding_id: &str,
    binding: &BindingV1,
) -> Result<ResolvedBinding, ResolveError> {
    let (mem, _name) = binding_id
        .split_once('/')
        .filter(|(m, n)| !m.is_empty() && !n.is_empty())
        .ok_or_else(|| ResolveError::MalformedProjectionRef {
            ingest: binding_id.to_string(),
            projection: binding_id.to_string(),
        })?;

    let mut primary_sources = Vec::with_capacity(binding.source_facets.len());
    for facet_name in &binding.source_facets {
        let facet: &Facet = configs
            .facets
            .iter()
            .find(|r| r.mem == mem && r.name == *facet_name)
            .map(|r| &r.config)
            .ok_or_else(|| ResolveError::FacetNotFound {
                projection_ref: binding_id.to_string(),
                facet: facet_name.clone(),
                mem: mem.to_string(),
                available: configs
                    .facets
                    .iter()
                    .filter(|r| r.mem == mem)
                    .map(|r| r.name.clone())
                    .collect(),
            })?;

        let medium: &Medium = configs
            .mediums
            .iter()
            .find(|r| r.mem == mem && r.name == facet.medium)
            .map(|r| &r.config)
            .ok_or_else(|| ResolveError::MediumNotFound {
                facet: facet_name.clone(),
                medium: facet.medium.clone(),
                mem: mem.to_string(),
                available: configs
                    .mediums
                    .iter()
                    .filter(|r| r.mem == mem)
                    .map(|r| r.name.clone())
                    .collect(),
            })?;

        primary_sources.push(ResolvedPrimarySource {
            facet_ref: facet_name.clone(),
            medium: facet.medium.clone(),
            medium_type: medium.medium_type,
            medium_pointer: medium.pointer.clone(),
            declared_change_detection: medium.change_detection.clone(),
            scope: facet.scope.clone(),
            preparation: facet.preparation.clone(),
        });
    }

    Ok(ResolvedBinding {
        binding: binding.clone(),
        primary_sources,
    })
}

/// Resolve a **v1 binding** (by canonical id) into the runtime
/// [`ResolvedIngest`] shape the orchestration stages (cursor, brief,
/// selection) consume — the binding-era counterpart of [`resolve_ingest`].
///
/// The binding *is* the declaration (no projection record to look up, no flat
/// ingest to join): its `operations.build` supplies the run mode / trigger /
/// batch / post-actions, and `deny_paths` / `intent` / `rules` come straight
/// off the binding. `binding_id` is the canonical `<mem>/<stem>` (D3), which
/// becomes the resolved ingest's `name` and `projection_ref` — every downstream
/// key (sync_state, selection cache, brief header) is derived from it.
///
/// The binding's `build.mode` is [`BuildMode::Discovery`] or
/// [`BuildMode::OneShot`] (`refinement` is deleted from the vocabulary, D1) and
/// becomes the resolved run's `mode` directly; an absent build block defaults
/// to discovery. Facets and mediums are joined in the binding-id's `<mem>` tier.
/// Every lookup failure is a located [`ResolveError`]; a malformed `binding_id`
/// is [`ResolveError::MalformedProjectionRef`]. Pure — no I/O.
pub fn resolve_binding_run(
    configs: &BindingConfigs,
    binding_id: &str,
    binding: &BindingV1,
) -> Result<ResolvedIngest, ResolveError> {
    let (mem, name) = binding_id
        .split_once('/')
        .filter(|(m, n)| !m.is_empty() && !n.is_empty())
        .ok_or_else(|| ResolveError::MalformedProjectionRef {
            ingest: binding_id.to_string(),
            projection: binding_id.to_string(),
        })?;
    let mem = mem.to_string();
    let name = name.to_string();

    let mut sources =
        Vec::with_capacity(binding.source_facets.len() + binding.reference_mems.len());
    for facet_name in &binding.source_facets {
        let facet: &Facet = configs
            .facets
            .iter()
            .find(|r| r.mem == mem && r.name == *facet_name)
            .map(|r| &r.config)
            .ok_or_else(|| ResolveError::FacetNotFound {
                projection_ref: binding_id.to_string(),
                facet: facet_name.clone(),
                mem: mem.clone(),
                available: configs
                    .facets
                    .iter()
                    .filter(|r| r.mem == mem)
                    .map(|r| r.name.clone())
                    .collect(),
            })?;

        let medium: &Medium = configs
            .mediums
            .iter()
            .find(|r| r.mem == mem && r.name == facet.medium)
            .map(|r| &r.config)
            .ok_or_else(|| ResolveError::MediumNotFound {
                facet: facet_name.clone(),
                medium: facet.medium.clone(),
                mem: mem.clone(),
                available: configs
                    .mediums
                    .iter()
                    .filter(|r| r.mem == mem)
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
    for reference_mem in &binding.reference_mems {
        sources.push(ResolvedSource::Reference {
            mem: reference_mem.clone(),
        });
    }

    // The build op supplies mode / trigger / batch / post-actions. An absent
    // build block (a not-yet-built obligation) resolves to sane defaults — the
    // build-path refusal (D6/AC4) is enforced at the brief entry point, not
    // here, so read-only callers (status) keep working.
    let build = binding.operations.build.as_ref();
    let mode = build.map_or(BuildMode::Discovery, |b| b.mode);
    let trigger = build.map_or(IngestTrigger::Loop, |b| b.trigger);
    let batch_size = build.map_or(20, |b| b.batch_size);
    let post_actions = build.and_then(|b| b.post_actions.clone());

    Ok(ResolvedIngest {
        name: binding_id.to_string(),
        mode,
        trigger,
        batch_size,
        deny_paths: binding.deny_paths.clone(),
        projection_ref: binding_id.to_string(),
        projection_mem: mem,
        projection_name: name,
        intent: binding.intent.clone(),
        sources,
        destination_mem: binding.destination_mem.clone(),
        rules: binding.rules.clone(),
        post_actions,
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
    // A detection-less medium (per the D6 capability matrix — `web` this cycle)
    // has no change signal: it resolves to the visible NoSignal (`none`), never
    // a fabricated `mtime`/`git` token (E1 / AC16). This mirrors the graph
    // special case above — the medium type overrides any declared value.
    if !medium_capabilities(source.medium_type).change_signal {
        return ChangeStrategy::None;
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
    use crate::pipeline::PatternMode;
    use crate::pipeline_store::MemPipelineRecord;

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

    fn allow(path: &str) -> PatternEntry {
        PatternEntry {
            path: path.to_string(),
            mode: PatternMode::Allow,
        }
    }

    fn v1_binding(dest: &str, facets: &[&str]) -> BindingV1 {
        use crate::binding::{
            BINDING_VERSION, BuildMode, BuildOperation, CoverageSemantics, Operations,
        };
        BindingV1 {
            version: BINDING_VERSION,
            intent: Some("prose".to_string()),
            source_facets: facets.iter().map(|s| s.to_string()).collect(),
            reference_mems: vec![],
            destination_mem: dest.to_string(),
            deny_paths: vec![],
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
                sync: None,
                verify: None,
            },
        }
    }

    /// `resolve_binding` joins each of the binding's source facets to the
    /// medium it engages, in the binding-id's `<mem>` tier — no projection
    /// record needed (the binding is the declaration).
    #[test]
    fn resolves_a_v1_binding() {
        let configs = PipelineConfigs {
            mediums: vec![medium("engine", "src", MediumType::Codebase, "../public")],
            facets: vec![facet(
                "engine",
                "source-tree",
                "src",
                vec![allow("../public/**/*.rs")],
            )],
            ..Default::default()
        };
        let binding = v1_binding("engine", &["source-tree"]);
        let resolved = resolve_binding(&configs, "engine/graph", &binding).unwrap();
        assert_eq!(resolved.primary_sources.len(), 1);
        let p = &resolved.primary_sources[0];
        assert_eq!(p.facet_ref, "source-tree");
        assert_eq!(p.medium, "src");
        assert_eq!(p.medium_type, MediumType::Codebase);
        assert_eq!(p.medium_pointer, "../public");
        assert_eq!(p.scope, vec![allow("../public/**/*.rs")]);
    }

    /// A binding whose source facet does not exist in its mem errors, located.
    #[test]
    fn resolve_binding_dangling_facet_errors() {
        let configs = PipelineConfigs::default();
        let binding = v1_binding("engine", &["missing-facet"]);
        let err = resolve_binding(&configs, "engine/graph", &binding).unwrap_err();
        assert!(matches!(
            err,
            ResolveError::FacetNotFound { ref facet, .. } if facet == "missing-facet"
        ));
    }

    /// A malformed binding id (no `/`) is a located error.
    #[test]
    fn resolve_binding_malformed_id_errors() {
        let configs = PipelineConfigs::default();
        let binding = v1_binding("engine", &[]);
        let err = resolve_binding(&configs, "noslash", &binding).unwrap_err();
        assert!(matches!(err, ResolveError::MalformedProjectionRef { .. }));
    }

    /// `resolve_binding_run` produces the runtime shape from a binding: the id
    /// becomes `name`/`projection_ref`, `build.mode` maps to the run mode,
    /// deny_paths / trigger / batch / post_actions come off the build op, and
    /// each facet joins its medium; reference mems follow the primaries.
    #[test]
    fn resolve_binding_run_produces_runtime_shape() {
        use crate::binding::{
            BINDING_VERSION, BuildMode, BuildOperation, CoverageSemantics, Operations,
        };
        let configs = BindingConfigs {
            mediums: vec![MemPipelineRecord {
                mem: "app".to_string(),
                name: "src".to_string(),
                config: Medium {
                    name: "src".to_string(),
                    medium_type: MediumType::Codebase,
                    pointer: "../app".to_string(),
                    change_detection: None,
                },
            }],
            facets: vec![MemPipelineRecord {
                mem: "app".to_string(),
                name: "source-tree".to_string(),
                config: Facet {
                    name: "source-tree".to_string(),
                    medium: "src".to_string(),
                    scope: vec![allow("../app/**/*.swift")],
                    engagement: None,
                    preparation: None,
                },
            }],
            bindings: vec![],
        };
        let binding = BindingV1 {
            version: BINDING_VERSION,
            intent: Some("swift".to_string()),
            source_facets: vec!["source-tree".to_string()],
            reference_mems: vec!["engine".to_string()],
            destination_mem: "app".to_string(),
            deny_paths: vec!["**/VISION.md".to_string()],
            coverage_semantics: CoverageSemantics::Exhaustive,
            rules: None,
            prune: None,
            operations: Operations {
                build: Some(BuildOperation {
                    mode: BuildMode::Discovery,
                    trigger: IngestTrigger::Loop,
                    batch_size: 20,
                    post_actions: Some(serde_json::json!({ "archive_source": true })),
                }),
                sync: None,
                verify: None,
            },
        };

        let r = resolve_binding_run(&configs, "app/graph", &binding).unwrap();
        assert_eq!(r.name, "app/graph");
        assert_eq!(r.projection_ref, "app/graph");
        assert_eq!(r.projection_mem, "app");
        assert_eq!(r.projection_name, "graph");
        assert_eq!(r.mode, BuildMode::Discovery);
        assert_eq!(r.batch_size, 20);
        assert_eq!(r.deny_paths, ["**/VISION.md"]);
        assert_eq!(r.destination_mem, "app");
        assert_eq!(r.intent.as_deref(), Some("swift"));
        assert_eq!(
            r.post_actions,
            Some(serde_json::json!({ "archive_source": true }))
        );
        assert_eq!(r.sources.len(), 2);
        match &r.sources[0] {
            ResolvedSource::Primary(p) => {
                assert_eq!(p.facet_ref, "source-tree");
                assert_eq!(p.medium_type, MediumType::Codebase);
                assert_eq!(p.medium_pointer, "../app");
            }
            other => panic!("expected primary first, got {other:?}"),
        }
        assert_eq!(
            r.sources[1],
            ResolvedSource::Reference {
                mem: "engine".to_string()
            }
        );
    }

    /// A one-shot binding maps to the one-shot run mode.
    #[test]
    fn resolve_binding_run_maps_one_shot() {
        use crate::binding::{
            BINDING_VERSION, BuildMode, BuildOperation, CoverageSemantics, Operations,
        };
        let configs = BindingConfigs::default();
        let binding = BindingV1 {
            version: BINDING_VERSION,
            intent: None,
            source_facets: vec![],
            reference_mems: vec![],
            destination_mem: "m".to_string(),
            deny_paths: vec![],
            coverage_semantics: CoverageSemantics::Exhaustive,
            rules: None,
            prune: None,
            operations: Operations {
                build: Some(BuildOperation {
                    mode: BuildMode::OneShot,
                    trigger: IngestTrigger::Manual,
                    batch_size: 5,
                    post_actions: None,
                }),
                sync: None,
                verify: None,
            },
        };
        let r = resolve_binding_run(&configs, "m/lens", &binding).unwrap();
        assert_eq!(r.mode, BuildMode::OneShot);
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

    /// A `web` medium (detection-less per the D6 matrix) resolves to the
    /// visible NoSignal `None` regardless of any declared value — `status`
    /// renders `signal: none`, never a fabricated `mtime`/`git` token (AC16).
    #[test]
    fn web_medium_resolves_to_none_signal() {
        let root = Path::new("/nonexistent");
        assert_eq!(
            resolve_change_strategy(&primary(MediumType::Web, "https://example.com", None), root),
            ChangeStrategy::None
        );
        // Even a declared override does not fabricate a signal for web.
        assert_eq!(
            resolve_change_strategy(
                &primary(MediumType::Web, "https://example.com", Some("mtime")),
                root
            ),
            ChangeStrategy::None
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
