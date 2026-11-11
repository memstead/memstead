//! The refusals a command meets when it creates a workspace, a mem's
//! folder or a schema package where one cannot go: inside another
//! workspace, over a workspace already there, over a `.memstead/` that is
//! not one, over a schema package already there, or into a folder with
//! content. One type, so `init`, `quickstart` and `schema new` report each
//! state with one code and one payload.

use std::path::{Path, PathBuf};

/// What a fresh workspace or mem folder refuses to be created over.
#[derive(Debug, Clone, thiserror::Error)]
pub enum WorkspaceCreateError {
    #[error(
        "{} is already inside the memstead workspace at {}; workspaces do not nest",
        target.display(),
        found_at.display()
    )]
    AlreadyExistsAbove { target: PathBuf, found_at: PathBuf },
    #[error("{} is already a Memstead workspace", path.display())]
    AlreadyInitialised { path: PathBuf },
    #[error("{} already exists; a mem-repo is never created over one", path.display())]
    MemRepoExists { path: PathBuf },
    #[error(
        "{} is a `.memstead/` directory that is not a workspace (no workspace.toml)",
        store.display()
    )]
    ForeignMemsteadDir { store: PathBuf },
    #[error("{} already contains a schema package, which is never overwritten", path.display())]
    SchemaPackageExists { path: PathBuf },
    #[error("{} is not empty (found {})", path.display(), found.join(", "))]
    TargetNotEmpty {
        path: PathBuf,
        found: Vec<String>,
        /// The command that retries with a free name, when the caller has one.
        retry: Option<String>,
    },
}

/// The recovery sentence a nested workspace carries on every surface.
/// It names no command: which one works in the existing workspace
/// depends on how that workspace was created (a quickstart workspace admits
/// no `mem init`), so each command adds its own.
pub const NESTED_WORKSPACE_HINT: &str =
    "Work in the existing workspace, or create the new one in a folder outside it.";

impl WorkspaceCreateError {
    pub fn code(&self) -> &'static str {
        match self {
            WorkspaceCreateError::AlreadyExistsAbove { .. } => "WORKSPACE_ALREADY_EXISTS_ABOVE",
            WorkspaceCreateError::AlreadyInitialised { .. } => "WORKSPACE_ALREADY_INITIALISED",
            WorkspaceCreateError::ForeignMemsteadDir { .. } => "FOREIGN_MEMSTEAD_DIR",
            WorkspaceCreateError::SchemaPackageExists { .. } => "SCHEMA_PACKAGE_EXISTS",
            WorkspaceCreateError::MemRepoExists { .. } => "MEM_DB_ALREADY_EXISTS",
            WorkspaceCreateError::TargetNotEmpty { .. } => "TARGET_NOT_EMPTY",
        }
    }

    pub fn details(&self) -> serde_json::Value {
        match self {
            WorkspaceCreateError::AlreadyExistsAbove { found_at, .. } => serde_json::json!({
                "found_at": found_at.display().to_string(),
                "hint": NESTED_WORKSPACE_HINT,
            }),
            WorkspaceCreateError::AlreadyInitialised { path }
            | WorkspaceCreateError::SchemaPackageExists { path }
            | WorkspaceCreateError::MemRepoExists { path } => {
                serde_json::json!({ "path": path.display().to_string() })
            }
            WorkspaceCreateError::ForeignMemsteadDir { store } => {
                serde_json::json!({ "path": store.display().to_string() })
            }
            WorkspaceCreateError::TargetNotEmpty { path, found, retry } => {
                let mut d = serde_json::json!({
                    "path": path.display().to_string(),
                    "found": found,
                });
                if let Some(retry) = retry {
                    d["retry"] = serde_json::json!(retry);
                }
                d
            }
        }
    }
}

/// The `workspace.toml` of the nearest workspace above `target` (not
/// `target` itself), if any.
pub fn ancestor_workspace(target: &Path) -> std::io::Result<Option<PathBuf>> {
    let abs = std::fs::canonicalize(target)?;
    for ancestor in abs.ancestors().skip(1) {
        if crate::is_workspace_root(ancestor) {
            return Ok(Some(
                ancestor
                    .join(crate::WORKSPACE_STORE_DIR)
                    .join("workspace.toml"),
            ));
        }
    }
    Ok(None)
}
