//! Engine setup from global CLI flags. Produces an `Engine`
//! synchronously (no tokio) for the CLI to call into directly.
//!
//! Post-rebuild there is one workspace marker: `.memstead/workspace.toml`
//! at the workspace root. The `vault-repo` Cargo feature decides
//! which engine factory consumes it — pro routes through
//! [`memstead_git_branch::workspace_store::engine_from_workspace_root`]
//! (git-branch backends plus folder + archive), basis routes through
//! [`memstead_base::Engine::from_workspace_root`] (folder + archive
//! only).
//!
//! [`CliEngine`] wraps either flavour; subcommands match-dispatch on
//! it. The `WorkspaceShape` variant is retained so the basis build
//! can still surface an actionable "this is the basis binary, your
//! workspace has git-branch mounts" error when the operator points a
//! basis binary at a pro workspace — the shape tag is derived from
//! `vault-repo/.git` co-existing with the marker rather than the
//! marker itself.

use std::path::{Path, PathBuf};

use anyhow::Context;

use memstead_base::Engine as BaseEngine;
use memstead_base::vcs::ClientId;
#[cfg(feature = "vault-repo")]
use memstead_base::vcs::{Actor, CommitContext};
#[cfg(feature = "vault-repo")]
use memstead_git_branch::workspace_store::engine_from_workspace_root;

use crate::CliError;
use crate::output::ExitKind;

/// Structured-code constant for the missing-workspace exit envelope.
/// Surfaced on both `--json` output (under the `code` key in
/// `details`) and as the `Display` body of the underlying `CliError`.
/// Scripts and agents branch on this stable token; the human prose
/// (which mentions the recovery command) is the message and can be
/// adjusted without breaking the contract.
pub const WORKSPACE_NOT_INITIALISED_CODE: &str = "WORKSPACE_NOT_INITIALISED";

/// Recovery command suggested when no `.memstead/workspace.toml` is
/// reachable from cwd. `memstead vault-repo init` in the pro build (this
/// binary speaks vault-repo); `memstead init` in the basis build. The
/// structured `hint.recovery_command` field carries this token
/// verbatim so an agent can re-exec it.
#[cfg(feature = "vault-repo")]
pub const WORKSPACE_RECOVERY_COMMAND: &str = "memstead vault-repo init";
#[cfg(not(feature = "vault-repo"))]
pub const WORKSPACE_RECOVERY_COMMAND: &str = "memstead init";

/// Build the typed `WORKSPACE_NOT_INITIALISED` exit envelope. Goes
/// through `CliError` so the top-level `main` downcast lifts the
/// `code` + `hint` fields into the JSON output.
pub fn workspace_not_initialised_error(message: &str) -> CliError {
    CliError {
        kind: ExitKind::Generic,
        code: WORKSPACE_NOT_INITIALISED_CODE,
        message: message.to_string(),
        details: Some(serde_json::json!({
            "hint": { "recovery_command": WORKSPACE_RECOVERY_COMMAND },
        })),
    }
}

/// Global CLI state: shared flags + a lazily-initialized `Engine`.
pub struct CliContext {
    pub json: bool,
    /// User asked for quiet stderr (`--quiet`). The CLI runs the
    /// engine in-process and never installs a `tracing_subscriber`,
    /// so the flag is informational.
    pub quiet: bool,
}

/// Workspace flavour resolved from cwd. Subcommands dispatch on this
/// to pick the right engine accessor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceShape {
    /// Vault-repo workspace — multi-vault, git-backed.
    /// The `.memstead/workspace.toml` root also carries `vault-repo/.git/`.
    VaultRepo,
    /// Filesystem-vault workspace — single-vault, history-free.
    /// The `.memstead/workspace.toml` root has no `vault-repo/.git/`.
    Filesystem,
}

/// Engine instance + the workspace flavour it serves. Subcommands
/// match on the variant to call the right engine API; the read-side
/// store accessor (`engine.store()`) lives on both flavours so simple
/// read commands can share most of their bodies.
///
/// The `VaultRepo` variant is only present under the `vault-repo`
/// feature. In the basis build (`--no-default-features`) the enum
/// collapses to a single `Filesystem` arm — every subcommand's
/// dispatch elides the missing arm via `cfg`.
pub enum CliEngine {
    #[cfg(feature = "vault-repo")]
    VaultRepo(BaseEngine),
    /// Filesystem-vault flavour, served by the unified [`memstead_base::Engine`].
    Filesystem(BaseEngine),
}

impl CliContext {
    /// Resolve the workspace flavour by walking up from cwd. Returns
    /// `None` when no `.memstead/workspace.toml` is found in any ancestor.
    ///
    /// Post-rebuild the marker is shape-neutral — the same
    /// `.memstead/workspace.toml` carries both folder-only workspaces and
    /// vault-repo workspaces. The flavour tag comes from whether the
    /// workspace root also carries `vault-repo/.git/` (vault-repo
    /// flavour) or not (folder-only flavour). The basis CLI uses this
    /// distinction to surface "this is the basis binary" when the
    /// operator points it at a workspace with git-branch mounts.
    pub fn workspace_shape(&self) -> Option<(WorkspaceShape, PathBuf)> {
        let cwd = std::env::current_dir().ok()?;
        let root = find_workspace_root(&cwd)?;
        let shape = if root.join("vault-repo").join(".git").is_dir() {
            WorkspaceShape::VaultRepo
        } else {
            WorkspaceShape::Filesystem
        };
        Some((shape, root))
    }

    /// Build a [`CliEngine`] from the current cwd. The workspace
    /// marker `.memstead/workspace.toml` resolves either flavour; the
    /// presence of `vault-repo/.git/` switches the engine factory.
    ///
    /// On the basis build (`--no-default-features`) the vault-repo
    /// branch surfaces a clear "not built into this binary" error so
    /// a user pointing the basis build at a vault-repo workspace
    /// gets an actionable signal rather than a confusing "no
    /// workspace" bail.
    pub fn cli_engine(&self) -> anyhow::Result<CliEngine> {
        match self.workspace_shape() {
            Some((_, root)) => self.cli_engine_at(&root),
            None => Err(workspace_not_initialised_error(
                "No workspace found. Run from a directory containing `.memstead/workspace.toml` (run `memstead init` for a folder-mount workspace, or `memstead vault-repo init` for a vault-repo workspace).",
            )
            .into()),
        }
    }

    /// Build a [`CliEngine`] rooted at an explicit workspace directory,
    /// skipping the cwd walk-up. The flavour is still derived from
    /// whether `<root>/vault-repo/.git/` is present, so callers that
    /// already know the root (e.g. `memstead publish --workspace`) get
    /// the same factory selection as [`Self::cli_engine`]. The split
    /// also gives subcommands a chdir-free, unit-testable engine seam.
    pub fn cli_engine_at(&self, root: &Path) -> anyhow::Result<CliEngine> {
        if root.join("vault-repo").join(".git").is_dir() {
            #[cfg(feature = "vault-repo")]
            {
                let engine = engine_from_workspace_root(root)
                    .map_err(|e| anyhow::anyhow!("init engine at {}: {e:#}", root.display()))?;
                return Ok(CliEngine::VaultRepo(engine));
            }
            #[cfg(not(feature = "vault-repo"))]
            {
                return Err(CliError {
                    kind: ExitKind::Generic,
                    code: "UNSUPPORTED_WORKSPACE_SHAPE",
                    message:
                        "this is the basis build of memstead (folder-mount only); the workspace is vault-repo-shaped (`vault-repo/.git/` present). Install the pro build (`cargo build --features vault-repo`) or run from a workspace whose mounts are all folder-backed."
                            .to_string(),
                    details: None,
                }
                .into());
            }
        }
        let engine = BaseEngine::from_workspace_root(root)
            .with_context(|| format!("init filesystem-vault engine at {}", root.display()))?;
        Ok(CliEngine::Filesystem(engine))
    }

    /// Build the unified [`memstead_base::Engine`] for a vault-repo-shaped
    /// workspace. Delegates to `engine_from_workspace_root` which
    /// handles layout detection, mount enumeration, schema resolution,
    /// and readVaults hydration in one pass.
    ///
    /// Only compiled into the pro build — the basis build never sees a
    /// vault-repo workspace because `cli_engine()` rejects it before
    /// reaching here.
    #[cfg(feature = "vault-repo")]
    pub fn engine(&self) -> anyhow::Result<BaseEngine> {
        let cwd = std::env::current_dir().context("Could not determine current directory")?;

        let Some(root) = find_workspace_root(&cwd) else {
            return Err(workspace_not_initialised_error(
                "No workspace found. Run from a directory containing `.memstead/workspace.toml` (run `memstead vault-repo init` to bootstrap).",
            )
            .into());
        };

        // Subcommands routed through `engine()` (rather than
        // `cli_engine()`) require vault-repo shape — they read /
        // write commit-shaped artefacts (`workspace dump` snapshots,
        // `batch-update` commit envelopes) that have no analogue on a
        // folder-mount-only workspace. Surface the vault-repo-only
        // tag here so callers print an actionable message instead of
        // booting into a foldery engine and erroring later.
        if !root.join("vault-repo").join(".git").is_dir() {
            return Err(CliError {
                kind: ExitKind::Generic,
                code: "UNSUPPORTED_WORKSPACE_SHAPE",
                message:
                    "this subcommand is vault-repo-only and not yet supported on filesystem-vault workspaces — run from a vault-repo workspace, or use `memstead stats` / `memstead list` / `memstead search` / `memstead entity` / `memstead health` / `memstead create|update|delete|relate|rename` instead."
                        .to_string(),
                details: None,
            }
            .into());
        }

        engine_from_workspace_root(&root)
            .map_err(|e| anyhow::anyhow!("init engine at {}: {e:#}", root.display()))
    }
}

/// Walk upward from `start` looking for the first ancestor that
/// contains `.memstead/workspace.toml` (the post-rebuild workspace
/// marker). Returns the first ancestor directory carrying the marker,
/// or `None` if the walk reaches filesystem root without finding one.
///
/// Both files and directories are accepted as `start`. A plain file's
/// parent is used as the first candidate; for a directory, the
/// directory itself is the first candidate.
///
/// Deeper-marker semantics: because the walk is upward and stops at
/// the first match, an inner workspace nested inside an outer one
/// resolves to the inner.
///
/// Mirrors `memstead-mcp/src/main.rs::find_workspace_root` and the
/// per-command walkers in `memstead-cli/src/commands/link.rs` /
/// `memstead-cli/src/commands/publish.rs`. Keep the resolution rules in
/// sync if any of these change.
pub fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut cursor: PathBuf = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    loop {
        if memstead_base::is_workspace_root(&cursor) {
            return Some(cursor);
        }
        let Some(parent) = cursor.parent() else {
            return None;
        };
        if parent == cursor {
            return None;
        }
        cursor = parent.to_path_buf();
    }
}

/// Compatibility alias for `find_workspace_root` — kept so existing
/// CLI subcommands (export, changes, …) that historically routed
/// through the basis-flavour walker continue to compile. Both walkers
/// now find the same marker; the alias is intentional for
/// call-site clarity (`find_workspace_root` reads as the canonical
/// surface; `find_filesystem_workspace_root` documents the
/// folder-mount-only intent of its caller).
pub fn find_filesystem_workspace_root(start: &Path) -> Option<PathBuf> {
    find_workspace_root(start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn touch_marker(ws: &std::path::Path) {
        std::fs::create_dir_all(ws.join(".memstead")).unwrap();
        std::fs::write(ws.join(".memstead").join("workspace.toml"), "").unwrap();
    }

    #[test]
    fn find_workspace_root_walks_up_to_marker() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        let nested = ws.join("a").join("b").join("specs");
        std::fs::create_dir_all(&nested).unwrap();
        touch_marker(&ws);
        let found =
            find_workspace_root(&nested).expect("walk should find .memstead/workspace.toml");
        assert_eq!(found.canonicalize().unwrap(), ws.canonicalize().unwrap());
    }

    #[test]
    fn find_workspace_root_returns_none_when_absent() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(find_workspace_root(&nested).is_none());
    }

    #[test]
    fn find_workspace_root_stops_at_containing_dir() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        touch_marker(&ws);
        let found =
            find_workspace_root(&ws).expect("ws itself carries .memstead/workspace.toml");
        assert_eq!(found, ws);
    }

    #[test]
    fn find_workspace_root_accepts_file_start() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        touch_marker(&ws);
        let file = ws.join("some-file.md");
        std::fs::write(&file, "").unwrap();
        let found = find_workspace_root(&file).expect("file start should resolve to its dir");
        assert_eq!(found, ws);
    }

    #[test]
    fn find_workspace_root_deeper_marker_wins() {
        // Outer and inner each carry `.memstead/workspace.toml`. The walk
        // starts deep inside the inner dir and must resolve to the
        // inner — deeper marker wins because the upward walk stops at
        // the first match.
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        let inner = outer.join("inner");
        let deep = inner.join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        touch_marker(&outer);
        touch_marker(&inner);
        let found = find_workspace_root(&deep).expect("walk should find the inner marker");
        assert_eq!(found.canonicalize().unwrap(), inner.canonicalize().unwrap());
    }
}

/// Provenance bundle for every CLI-initiated mutation. `Actor::Cli` +
/// `memstead-cli@<CARGO_PKG_VERSION>`. The `Tool:` trailer stays `None`: CLI
/// subcommands aren't MCP tools and the commit subject (`memstead: create …`)
/// already carries the action verb — a second taxonomy would drift.
///
/// Only used by vault-repo write paths today; filesystem-vault write
/// paths assemble their own provenance directly. The function therefore
/// only compiles when `vault-repo` is enabled.
#[cfg(feature = "vault-repo")]
pub fn cli_ctx() -> CommitContext<'static> {
    cli_ctx_with_note(None)
}

/// The `memstead-cli@<version>` client identity stamped into the commit
/// body's `Client:` provenance trailer. Shared by every CLI mutation
/// path so the trailer is uniform across `create` / `update` / `relate`
/// / `rename`. Un-gated (unlike [`cli_ctx_with_note`]) because the
/// `relate` path passes the client to `relate_entity` directly rather
/// than through a `CommitContext`, and that path compiles on both
/// flavours.
pub fn cli_client_id() -> ClientId {
    ClientId {
        name: "memstead-cli".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Provenance bundle carrying an optional agent-authored `--note`.
/// The note rides into the same payload slot the MCP `note` parameter
/// uses; the engine's `require_notes` policy gate fires `NOTE_MISSING`
/// symmetrically across both surfaces.
#[cfg(feature = "vault-repo")]
pub fn cli_ctx_with_note(note: Option<String>) -> CommitContext<'static> {
    CommitContext {
        actor: Actor::Cli,
        client: Some(cli_client_id()),
        tool: None,
        note,
        logical_operation_id: None,
        entity_ids: None,
    }
}

/// Build the unified [`memstead_base::Engine`] for a vault-repo-shaped
/// workspace. Delegates to `engine_from_workspace_root` which
/// handles layout detection, mount enumeration, schema resolution,
/// and readVaults hydration in one pass.
///
/// Subcommands routed through this helper require vault-repo shape —
/// they read / write commit-shaped artefacts (`workspace dump`
/// snapshots, `batch-update` commit envelopes) that have no analogue
/// on a folder-mount-only workspace.
#[cfg(feature = "vault-repo")]
pub fn pro_engine(_ctx: &CliContext) -> anyhow::Result<BaseEngine> {
    let cwd = std::env::current_dir().map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            crate::INTERNAL_CODE,
            format!("could not determine current directory: {e}"),
        )
    })?;

    let Some(root) = find_workspace_root(&cwd) else {
        return Err(workspace_not_initialised_error(
            "No workspace found. Run from a directory containing `.memstead/workspace.toml` (run `memstead vault-repo init` to bootstrap).",
        )
        .into());
    };

    if !root.join("vault-repo").join(".git").is_dir() {
        return Err(CliError {
            code: "UNSUPPORTED_WORKSPACE_SHAPE",
            kind: ExitKind::Generic,
            message:
                "this subcommand is vault-repo-only and not yet supported on filesystem-vault workspaces — run from a vault-repo workspace, or use `memstead stats` / `memstead list` / `memstead search` / `memstead entity` / `memstead health` / `memstead create|update|delete|relate|rename` instead."
                    .to_string(),
            details: None,
        }
        .into());
    }

    engine_from_workspace_root(&root)
        .map_err(|e| anyhow::anyhow!("init engine at {}: {e:#}", root.display()))
}
