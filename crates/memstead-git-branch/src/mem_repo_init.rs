//! Laying a mem-repo: the library form of `memstead mem-repo init`.
//!
//! A program that links the engine crates lays a mem-repo by calling
//! [`init_mem_repo`], with no `memstead` binary involved; the CLI's
//! `mem-repo init` is this function plus what is the CLI's own
//! (arguments, rendering, the enclosing repository's ignore file).

use std::fs;
use std::path::{Path, PathBuf};

use gix::object::tree::EntryKind;
use memstead_base::workspace_create::WorkspaceCreateError;

const README_TEMPLATE: &str = include_str!("../templates/mem-repo-readme.md");

/// Engine-required minimum for `.memstead/workspace.toml`. The two-layer
/// file adapter loader treats `format` + `[persistence_adapter]` as
/// the only mandatory keys; everything else (mem_management,
/// cross_mem_links, mutations, plugin.*) is operator-opt-in and
/// defaults to deny/empty. Matches the same baseline `memstead init`
/// writes for filesystem-mem, keeping the two workspace shapes symmetric.
const DEFAULT_WORKSPACE_TOML: &str = "\
format = \"memstead-git-branch-2\"\n\
\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n";

/// Why a mem-repo could not be laid.
#[derive(Debug, thiserror::Error)]
pub enum MemRepoInitError {
    /// The workspace already holds a `mem-repo/`; nothing was changed.
    /// Carries [`WorkspaceCreateError::MemRepoExists`].
    #[error(transparent)]
    Exists(WorkspaceCreateError),
    /// A filesystem or git step failed; the message names the step.
    #[error("{0}")]
    Failed(String),
}

/// What [`init_mem_repo`] laid.
#[derive(Debug)]
pub struct MemRepoInit {
    /// The canonical workspace root.
    pub workspace_root: PathBuf,
    /// `<workspace>/mem-repo`.
    pub mem_repo_dir: PathBuf,
    /// `<workspace>/.memstead/workspace.toml`.
    pub workspace_toml: PathBuf,
}

/// Lay a mem-repo under `workspace_path`: create
/// `<workspace>/mem-repo/.git/` with a working tree on `main`, make the
/// initial commit carrying the README, seed the unified `__MEMSTEAD`
/// registry ref with an empty tree, and write the minimum
/// `.memstead/workspace.toml` when none exists. The workspace directory
/// is created when missing.
///
/// Refuses with [`MemRepoInitError::Exists`] when the workspace already
/// holds a `mem-repo/`, and changes nothing in that case.
pub fn init_mem_repo(workspace_path: &Path) -> Result<MemRepoInit, MemRepoInitError> {
    let failed =
        |step: &str, e: &dyn std::fmt::Display| MemRepoInitError::Failed(format!("{step}: {e}"));

    fs::create_dir_all(workspace_path).map_err(|e| failed("create workspace directory", &e))?;
    let workspace =
        fs::canonicalize(workspace_path).map_err(|e| failed("canonicalize workspace path", &e))?;

    let mem_repo_root = workspace.join("mem-repo");
    if mem_repo_root.exists() {
        return Err(MemRepoInitError::Exists(
            WorkspaceCreateError::MemRepoExists {
                path: mem_repo_root,
            },
        ));
    }

    fs::create_dir_all(&mem_repo_root).map_err(|e| failed("create mem-repo directory", &e))?;
    gix::init(&mem_repo_root).map_err(|e| failed("init mem-repo gitdir", &e))?;
    let repo =
        gix::open(mem_repo_root.join(".git")).map_err(|e| failed("open mem-repo gitdir", &e))?;

    // `main` carries operator-facing docs only (README.md). Schemas and
    // per-mem configs live on the unified `__MEMSTEAD` registry ref.
    let mut editor = repo
        .empty_tree()
        .edit()
        .map_err(|e| failed("init main tree editor", &e))?;
    let readme_blob = repo
        .write_blob(README_TEMPLATE.as_bytes())
        .map_err(|e| failed("write README blob", &e))?
        .detach();
    editor
        .upsert("README.md", EntryKind::Blob, readme_blob)
        .map_err(|e| failed("upsert README.md", &e))?;
    let main_tree = editor
        .write()
        .map_err(|e| failed("write main tree", &e))?
        .detach();

    let actor = init_signature();
    let mut buf = gix::date::parse::TimeBuf::default();
    let actor_ref = actor.to_ref(&mut buf);
    repo.commit_as(
        actor_ref,
        actor_ref,
        "refs/heads/main",
        "mem-repo init: initial main commit",
        main_tree,
        Vec::<gix::ObjectId>::new(),
    )
    .map_err(|e| failed("commit main", &e))?;

    // Seed the unified `__MEMSTEAD` registry ref with an empty tree. Schemas
    // (`__MEMSTEAD:schemas/<name>/...`) and per-mem configs
    // (`__MEMSTEAD:mems/<mem>/config.json`) are upserted by subsequent
    // engine writes; the empty seed lets the engine's reader resolve
    // the ref without surfacing a bootstrap error.
    let empty_tree = repo.empty_tree().id().detach();
    let mut buf = gix::date::parse::TimeBuf::default();
    let actor_ref = actor.to_ref(&mut buf);
    repo.commit_as(
        actor_ref,
        actor_ref,
        "refs/heads/__MEMSTEAD",
        "mem-repo init: seed __MEMSTEAD",
        empty_tree,
        Vec::<gix::ObjectId>::new(),
    )
    .map_err(|e| failed("commit __MEMSTEAD", &e))?;

    // Write `README.md` to the working tree. Mirrors the just-committed
    // `main` tree so a human inspecting `<workspace>/mem-repo/` sees the
    // file immediately.
    fs::write(mem_repo_root.join("README.md"), README_TEMPLATE)
        .map_err(|e| failed("write working-tree README.md", &e))?;

    // The minimum-viable `.memstead/workspace.toml`, required by every
    // later command. A pre-existing file is left untouched so
    // operator-authored content survives.
    let memstead_dir = workspace.join(memstead_base::WORKSPACE_STORE_DIR);
    fs::create_dir_all(&memstead_dir).map_err(|e| failed("create .memstead directory", &e))?;
    let workspace_toml = memstead_dir.join("workspace.toml");
    if !workspace_toml.exists() {
        fs::write(&workspace_toml, DEFAULT_WORKSPACE_TOML)
            .map_err(|e| failed("write .memstead/workspace.toml", &e))?;
    }

    Ok(MemRepoInit {
        workspace_root: workspace,
        mem_repo_dir: mem_repo_root,
        workspace_toml,
    })
}

fn init_signature() -> gix::actor::Signature {
    gix::actor::Signature {
        name: "memstead-cli mem-repo init".into(),
        email: "mem-repo-init@memstead".into(),
        time: gix::date::Time {
            seconds: 0,
            offset: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Every file under `root` with its bytes.
    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.insert(path.clone(), fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, &mut out);
        out
    }

    /// A program that links the engine lays a mem-repo, creates a mem in
    /// it through the engine and writes an entity a second engine reads,
    /// with no CLI code and no binary involved.
    #[test]
    fn a_mem_repo_is_laid_and_used_without_the_cli() {
        let tmp = tempfile::TempDir::new().unwrap();
        let laid = init_mem_repo(&tmp.path().join("ws")).expect("a fresh directory is laid");
        assert!(laid.mem_repo_dir.join(".git").is_dir());
        assert!(laid.mem_repo_dir.join("README.md").is_file());
        assert!(laid.workspace_toml.is_file());

        let root = laid.workspace_root.clone();
        let mut engine = crate::workspace_store::engine_from_workspace_root(&root)
            .expect("the laid workspace boots");
        memstead_base::mem_management::create_mem(
            &mut engine,
            memstead_base::mem_management::MemCreateParams {
                name: "notes".to_string(),
                location: PathBuf::from("notes"),
                schema_ref: "default@1.0.0".parse().unwrap(),
                vcs: None,
                note: None,
                operator_mode: true,
                recovery: None,
                write_guidance: Default::default(),
                storage: Some(memstead_base::mem_management::StorageKind::GitBranch),
                actor: memstead_base::vcs::Actor::Cli,
                client: None,
            },
        )
        .expect("a mem is created in the laid mem-repo");
        engine
            .create_entity_with_ctx(
                memstead_base::engine::CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "notes".to_string(),
                    title: "First".to_string(),
                    entity_type: "spec".to_string(),
                    sections: indexmap::IndexMap::from_iter([
                        ("identity".to_string(), "i".to_string()),
                        ("purpose".to_string(), "p".to_string()),
                    ]),
                    metadata: indexmap::IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                &memstead_base::vcs::CommitContext::internal(),
            )
            .expect("an entity is written");

        let second = crate::workspace_store::engine_from_workspace_root(&root)
            .expect("a second engine boots over the same workspace");
        assert_eq!(second.mem_names(), ["notes"]);
        assert!(
            second
                .get_entity(&memstead_base::EntityId("notes--first".into()))
                .is_some(),
            "the second engine reads what the first wrote"
        );
    }

    /// A workspace that already holds a mem-repo refuses with the typed
    /// exists error, and nothing on disk changes: neither a mem-repo
    /// this function laid nor a `mem-repo` directory of other origin.
    #[test]
    fn an_existing_mem_repo_refuses_and_changes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        let laid = init_mem_repo(&ws).unwrap();
        let before = snapshot(&laid.workspace_root);
        let err = init_mem_repo(&ws).expect_err("a second laying refuses");
        match &err {
            MemRepoInitError::Exists(WorkspaceCreateError::MemRepoExists { path }) => {
                assert_eq!(path, &laid.mem_repo_dir)
            }
            other => panic!("expected the typed exists error, got {other:?}"),
        }
        assert_eq!(snapshot(&laid.workspace_root), before, "nothing changed");

        let other = tmp.path().join("other");
        fs::create_dir_all(other.join("mem-repo")).unwrap();
        fs::write(other.join("mem-repo").join("kept.txt"), b"kept").unwrap();
        let before = snapshot(&other);
        assert!(matches!(
            init_mem_repo(&other),
            Err(MemRepoInitError::Exists(
                WorkspaceCreateError::MemRepoExists { .. }
            ))
        ));
        assert_eq!(
            snapshot(&other),
            before,
            "a foreign mem-repo directory is untouched"
        );
        assert!(
            !other.join(".memstead").exists(),
            "and no workspace file is written beside it"
        );
    }
}
