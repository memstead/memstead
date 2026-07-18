//! Ingest runtime resolution — turn a stored v2 [`Binding`] into a
//! [`ResolvedIngest`]: the shape the selection, backoff, change-detection,
//! and brief-assembly stages all read.
//!
//! Since the 2026-07 single-record consolidation there is **no join**: a v2
//! binding carries its sources inline, so resolution is a pure unpacking —
//! the binding id supplies the identity, the `operations.build` block the
//! schedule, and each inline [`Source`] *is* the resolved primary source.
//! The cross-record reference errors of the three-file era (dangling facet /
//! medium refs) are gone with the references; in-record source validation
//! lives in [`crate::binding::validate_binding`].
//!
//! Resolving a source's *change-detection strategy* (which reads the source's
//! declared strategy and probes the filesystem for a git work tree) is the
//! separate, filesystem-touching concern at the bottom of this module.

use std::path::{Path, PathBuf};

use crate::binding::{Binding, BuildMode, medium_capabilities};
use crate::pipeline::{IngestTrigger, MediumType};
pub use crate::pipeline::Source;

/// A binding source resolved to what the run needs: a **primary** inline
/// source (the territory to read and write back), or a read-only
/// **reference** mem supplying cross-mem context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedSource {
    /// An inline primary source (both halves: where it lives / which part).
    Primary(Source),
    /// A read-only reference mem (cross-mem context, never written).
    Reference {
        /// The reference mem's id.
        mem: String,
    },
}

/// A v2 binding unpacked into the runtime shape the orchestration stages
/// (cursor, brief, selection) consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIngest {
    /// The run's identity — the canonical binding id `<mem>/<stem>`, the
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
    /// Paths excluded for this binding's runs, on top of source scope.
    pub deny_paths: Vec<String>,
    /// The binding id verbatim (`"<mem>/<name>"`).
    pub projection_ref: String,
    /// The binding's owning mem (the part before the `/`).
    pub projection_mem: String,
    /// The binding's name (the part after the `/`).
    pub projection_name: String,
    /// The binding's intent — prose for the agent (the brief's "about
    /// the source" block).
    pub intent: Option<String>,
    /// The resolved sources, in declaration order: inline primaries first,
    /// then reference mems.
    pub sources: Vec<ResolvedSource>,
    /// The single mem this binding writes into.
    pub destination_mem: String,
    /// Free-form binding rules (e.g. one-shot lens `routing`).
    pub rules: Option<serde_json::Value>,
    /// Free-form post-run actions (e.g. one-shot `archive_source`).
    pub post_actions: Option<serde_json::Value>,
}

/// Why a binding could not be resolved. With inline sources the only
/// structural failures left are identity-level: an unknown binding id, or a
/// malformed one.
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
}

/// Render a name list for an error message: `a, b, c` or `(none)`.
fn fmt_list(names: &[String]) -> String {
    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    }
}

/// Unpack a **v2 binding** (by canonical id) into the runtime
/// [`ResolvedIngest`] shape the orchestration stages (cursor, brief,
/// selection) consume.
///
/// The binding *is* the whole declaration: its inline sources become the
/// primary [`ResolvedSource`]s verbatim, its `operations.build` supplies the
/// run mode / trigger / batch / post-actions, and `deny_paths` / `intent` /
/// `rules` come straight off the record. `binding_id` is the canonical
/// `<mem>/<stem>`, which becomes the resolved ingest's `name` and
/// `projection_ref` — every downstream key (sync_state, selection cache,
/// brief header) is derived from it. A malformed `binding_id` is
/// [`ResolveError::MalformedProjectionRef`]. Pure — no I/O.
pub fn resolve_binding_run(
    binding_id: &str,
    binding: &Binding,
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

    let mut sources = Vec::with_capacity(binding.sources.len() + binding.reference_mems.len());
    for source in &binding.sources {
        sources.push(ResolvedSource::Primary(source.clone()));
    }
    for reference_mem in &binding.reference_mems {
        sources.push(ResolvedSource::Reference {
            mem: reference_mem.clone(),
        });
    }

    // The build op supplies mode / trigger / batch / post-actions. An absent
    // build block (a not-yet-built obligation) resolves to sane defaults — the
    // build-path refusal is enforced at the brief entry point, not here, so
    // read-only callers (status) keep working.
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

/// Resolve a primary source's [`ChangeStrategy`]. A graph-typed source
/// always uses [`Graph`] (its signal is the source mem's snapshot token,
/// which the engine provides). Otherwise the source's declared strategy wins
/// (`none`/`git`/`mtime`); `auto` — the default when unset, and any
/// unrecognized value — probes for a git work tree over the source pointer
/// (resolved against `workspace_root`): present → [`Git`], absent →
/// [`Mtime`].
///
/// [`Graph`]: ChangeStrategy::Graph
/// [`Git`]: ChangeStrategy::Git
/// [`Mtime`]: ChangeStrategy::Mtime
pub fn resolve_change_strategy(source: &Source, workspace_root: &Path) -> ChangeStrategy {
    if source.medium_type == MediumType::Graph {
        return ChangeStrategy::Graph;
    }
    // A detection-less medium (per the capability matrix — `web` this cycle)
    // has no change signal: it resolves to the visible NoSignal (`none`), never
    // a fabricated `mtime`/`git` token. This mirrors the graph special case
    // above — the medium type overrides any declared value.
    if !medium_capabilities(source.medium_type).change_signal {
        return ChangeStrategy::None;
    }
    match source.change_detection.as_deref() {
        Some("none") => ChangeStrategy::None,
        Some("git") => ChangeStrategy::Git,
        Some("mtime") => ChangeStrategy::Mtime,
        // `auto`, unset, or any unrecognized value: probe the filesystem.
        _ => {
            if find_git_root(&source_base_path(source, workspace_root)).is_some() {
                ChangeStrategy::Git
            } else {
                ChangeStrategy::Mtime
            }
        }
    }
}

/// The on-disk base directory a path-based primary source resolves to: the
/// source pointer joined onto the workspace root (`Path::join` yields the
/// pointer verbatim when it is absolute), or the workspace root itself for
/// an empty pointer. Only meaningful for path-namespaced mediums
/// (codebase / filesystem / git) — a graph pointer is a mem id and a web
/// pointer a URL.
pub fn source_base_path(source: &Source, workspace_root: &Path) -> PathBuf {
    if source.pointer.is_empty() {
        workspace_root.to_path_buf()
    } else {
        workspace_root.join(&source.pointer)
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
    use crate::binding::{
        BINDING_VERSION, BuildOperation, CoverageSemantics, Operations,
    };
    use crate::pipeline::{PatternEntry, PatternMode};

    fn source(name: &str, medium_type: MediumType, pointer: &str, declared: Option<&str>) -> Source {
        Source {
            name: name.to_string(),
            medium_type,
            pointer: pointer.to_string(),
            change_detection: declared.map(str::to_string),
            scope: vec![],
            engagement: None,
            preparation: None,
        }
    }

    fn allow(path: &str) -> PatternEntry {
        PatternEntry {
            path: path.to_string(),
            mode: PatternMode::Allow,
        }
    }

    fn v2_binding(dest: &str, sources: Vec<Source>) -> Binding {
        Binding {
            version: BINDING_VERSION,
            intent: Some("prose".to_string()),
            sources,
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

    /// `resolve_binding_run` produces the runtime shape from a v2 binding
    /// without any join: the id becomes `name`/`projection_ref`, `build.mode`
    /// maps to the run mode, deny_paths / trigger / batch / post_actions come
    /// off the build op, and each inline source carries over verbatim;
    /// reference mems follow the primaries.
    #[test]
    fn resolve_binding_run_produces_runtime_shape() {
        let mut swift_source = source("source-tree", MediumType::Codebase, "../app", None);
        swift_source.scope = vec![allow("../app/**/*.swift")];
        let mut binding = v2_binding("app", vec![swift_source.clone()]);
        binding.intent = Some("swift".to_string());
        binding.reference_mems = vec!["engine".to_string()];
        binding.deny_paths = vec!["**/VISION.md".to_string()];
        binding.operations.build.as_mut().unwrap().post_actions =
            Some(serde_json::json!({ "archive_source": true }));

        let r = resolve_binding_run("app/graph", &binding).unwrap();
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
        assert_eq!(r.sources[0], ResolvedSource::Primary(swift_source));
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
        let mut binding = v2_binding("m", vec![]);
        let build = binding.operations.build.as_mut().unwrap();
        build.mode = BuildMode::OneShot;
        build.trigger = IngestTrigger::Manual;
        build.batch_size = 5;
        let r = resolve_binding_run("m/lens", &binding).unwrap();
        assert_eq!(r.mode, BuildMode::OneShot);
        assert_eq!(r.batch_size, 5);
    }

    /// A malformed binding id (no `/`) is a located error.
    #[test]
    fn resolve_binding_run_malformed_id_errors() {
        let binding = v2_binding("m", vec![]);
        let err = resolve_binding_run("noslash", &binding).unwrap_err();
        assert!(matches!(err, ResolveError::MalformedProjectionRef { .. }));
    }

    /// An absent build block resolves to sane defaults (read-only callers
    /// keep working; the mutating-op refusal is enforced at the brief entry).
    #[test]
    fn absent_build_resolves_to_defaults() {
        let mut binding = v2_binding("m", vec![]);
        binding.operations.build = None;
        let r = resolve_binding_run("m/p", &binding).unwrap();
        assert_eq!(r.mode, BuildMode::Discovery);
        assert_eq!(r.trigger, IngestTrigger::Loop);
        assert_eq!(r.batch_size, 20);
        assert_eq!(r.post_actions, None);
    }

    /// A graph-typed source always resolves to the graph strategy,
    /// regardless of any declared value.
    #[test]
    fn graph_source_always_uses_graph_strategy() {
        let root = Path::new("/nonexistent");
        assert_eq!(
            resolve_change_strategy(&source("f", MediumType::Graph, "", None), root),
            ChangeStrategy::Graph
        );
        // Even a declared override does not change a graph source.
        assert_eq!(
            resolve_change_strategy(&source("f", MediumType::Graph, "", Some("mtime")), root),
            ChangeStrategy::Graph
        );
    }

    /// A `web` source (detection-less per the capability matrix) resolves to
    /// the visible NoSignal `None` regardless of any declared value —
    /// `status` renders `signal: none`, never a fabricated `mtime`/`git`
    /// token.
    #[test]
    fn web_source_resolves_to_none_signal() {
        let root = Path::new("/nonexistent");
        assert_eq!(
            resolve_change_strategy(
                &source("w", MediumType::Web, "https://example.com", None),
                root
            ),
            ChangeStrategy::None
        );
        // Even a declared override does not fabricate a signal for web.
        assert_eq!(
            resolve_change_strategy(
                &source("w", MediumType::Web, "https://example.com", Some("mtime")),
                root
            ),
            ChangeStrategy::None
        );
    }

    /// A declared `none`/`git`/`mtime` wins for a non-graph source.
    #[test]
    fn declared_strategy_wins_for_non_graph() {
        let root = Path::new("/nonexistent");
        for (declared, expected) in [
            ("none", ChangeStrategy::None),
            ("git", ChangeStrategy::Git),
            ("mtime", ChangeStrategy::Mtime),
        ] {
            assert_eq!(
                resolve_change_strategy(
                    &source("f", MediumType::Codebase, "x", Some(declared)),
                    root
                ),
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
            resolve_change_strategy(&source("f", MediumType::Codebase, "sub", None), git.path()),
            ChangeStrategy::Git
        );
        // An unrecognized declared value also falls through to the probe.
        assert_eq!(
            resolve_change_strategy(
                &source("f", MediumType::Codebase, "sub", Some("weird")),
                git.path()
            ),
            ChangeStrategy::Git
        );
        // No git work tree over the pointer → Mtime.
        assert_eq!(
            resolve_change_strategy(
                &source("f", MediumType::Filesystem, ".", None),
                plain.path()
            ),
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
