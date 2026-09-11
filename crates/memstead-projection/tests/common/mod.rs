//! Shared fixture for the crate-boundary suite: a real temporary
//! workspace with one folder mem (`engine`, pinned to `default@1.0.0`)
//! mounted through the workspace store, a git work tree at the root as
//! the codebase medium, and one binding `engine/graph` written through
//! the public binding store. Every test boots a real `Engine` over it
//! and drives the crate only through `memstead_projection::*`.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use memstead_base::anchor::{
    ANCHOR_SIDECAR_PATH, Anchor, AnchorGrain, AnchorHashStability, AnchorProvenanceClass,
    AnchorSidecar,
};
use memstead_base::binding::{
    BINDING_VERSION, Binding, BuildMode, BuildOperation, DEFAULT_ADJUDICATION_CAP,
    DEFAULT_FULL_RESYNC_EVERY, Operations, VerifyOperation,
};
use memstead_base::binding_run::{ResolvedIngest, resolve_binding_run};
use memstead_base::pipeline::{IngestTrigger, MediumType, PatternEntry, PatternMode, Source};
use memstead_base::pipeline_store::{load_pipeline_configs, write_binding};
use memstead_base::workspace::{
    Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
};
use memstead_base::workspace_store::WorkspaceStoreAdapter;

/// The destination mem every fixture binding writes into.
pub const MEM: &str = "engine";
/// The one binding's canonical id (`<mem>/<stem>`).
pub const BINDING: &str = "engine/graph";
/// The binding's single source facet.
pub const FACET: &str = "graph";

/// A temporary workspace; dropped with its directory.
pub struct Fixture {
    pub tmp: tempfile::TempDir,
}

impl Fixture {
    pub fn root(&self) -> &Path {
        self.tmp.path()
    }

    pub fn mem_dir(&self) -> PathBuf {
        self.root().join("mem")
    }
}

/// A workspace with the `engine` folder mem mounted and a git work tree
/// initialised at the root (no commits yet).
pub fn workspace() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mem_dir = root.join("mem");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".memstead")).unwrap();
    std::fs::write(
        root.join(".memstead").join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    let mount = Mount {
        mem: MEM.to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: MountStorage::Folder {
            path: mem_dir.clone(),
        },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: false,
        migration_target: None,
    };
    memstead_base::FileWorkspaceStore::new()
        .save_state(
            root,
            &Workspace {
                mounts: vec![mount],
                settings: WorkspaceSettings::default(),
            },
        )
        .unwrap();
    git(root, &["init", "-q"]);
    Fixture { tmp }
}

/// Run one git command in `repo`, with a pinned identity so commits work
/// on a machine without a global git config.
pub fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Stage everything and commit.
pub fn commit_all(repo: &Path, message: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-qm", message]);
    head_sha(repo)
}

/// The work tree's HEAD commit.
pub fn head_sha(repo: &Path) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Write a source file under the workspace root, creating parents.
pub fn write_source(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// Write one `decision` entity into the mem directory (a fixture write:
/// the engine's write path is the mutation surface, which this suite
/// does not exercise).
pub fn write_entity(fixture: &Fixture, slug: &str, title: &str, body: &str) {
    std::fs::write(
        fixture.mem_dir().join(format!("{slug}.md")),
        format!("---\ntype: decision\n---\n\n# {title}\n\n## Decision\n\n{body}\n"),
    )
    .unwrap();
}

/// A file-grain anchor of the given class; hash-bearing classes carry
/// `hash` (a deliberately wrong recorded hash drifts against the live
/// file), the others never carry one.
pub fn anchor(artifact: &str, class: AnchorProvenanceClass, hash: Option<&str>) -> Anchor {
    Anchor {
        artifact: artifact.to_string(),
        grain: AnchorGrain::File,
        class,
        at_version: None,
        hash: if class.is_hash_bearing() {
            hash.map(str::to_string)
        } else {
            None
        },
        hash_stability: AnchorHashStability::Stable,
        derived_from: Vec::new(),
        binding: None,
        source: None,
        span_unvalidated: false,
        hash_source: None,
        last_observed: None,
    }
}

/// Seed the mem's engine-owned anchors sidecar for one entity.
pub fn write_anchors(fixture: &Fixture, entity_id: &str, anchors: Vec<Anchor>) {
    let mut sidecar = AnchorSidecar::default();
    sidecar.set(entity_id, anchors);
    std::fs::write(
        fixture.mem_dir().join(ANCHOR_SIDECAR_PATH),
        sidecar.to_bytes(),
    )
    .unwrap();
}

/// The `engine/graph` binding: one codebase facet named `graph`, rooted
/// at the workspace, git change detection, the given allow scope.
pub fn graph_binding(scope: &[&str]) -> Binding {
    Binding {
        version: BINDING_VERSION,
        intent: None,
        sources: vec![Source {
            name: FACET.to_string(),
            medium_type: MediumType::Codebase,
            pointer: String::new(),
            change_detection: Some("git".to_string()),
            scope: scope
                .iter()
                .map(|p| PatternEntry {
                    path: (*p).to_string(),
                    mode: PatternMode::Allow,
                })
                .collect(),
            engagement: None,
            preparation: None,
        }],
        reference_mems: Vec::new(),
        destination_mem: MEM.to_string(),
        deny_paths: Vec::new(),
        coverage_semantics: None,
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
            verify: Some(VerifyOperation {
                trigger: IngestTrigger::Manual,
                batch_size: 20,
                adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
            }),
        },
    }
}

/// Write the `engine/graph` binding through the public binding store and
/// load it back the way a product does, resolved for a run.
pub fn write_graph_binding(fixture: &Fixture, scope: &[&str]) -> (Binding, ResolvedIngest) {
    write_binding(fixture.root(), MEM, FACET, &graph_binding(scope)).unwrap();
    load_graph_binding(fixture)
}

/// Load the stored `engine/graph` binding and resolve it for a run.
pub fn load_graph_binding(fixture: &Fixture) -> (Binding, ResolvedIngest) {
    let configs = load_pipeline_configs(fixture.root()).unwrap();
    let binding = configs
        .bindings
        .iter()
        .find(|b| b.config.destination_mem == MEM)
        .expect("the fixture binding is stored")
        .config
        .clone();
    let resolved = resolve_binding_run(BINDING, &binding).unwrap();
    (binding, resolved)
}

/// The sync-state key of the facet's `#synced` baseline.
pub fn synced_key() -> String {
    format!("{BINDING}/{FACET}#synced")
}

/// The sync-state key of the facet's `#verified` baseline.
pub fn verified_key() -> String {
    format!("{BINDING}/{FACET}#verified")
}
