//! `memstead link <scope/name>` — fetch a published mem from the
//! registry, cache it locally, and record the dependency in the filesystem
//! workspace config.
//!
//! Re-fetch is the refresh: invoking `memstead link <same-ref>` again
//! re-downloads the archive and overwrites the cached file. The
//! workspace-config dep entry is idempotent — `WorkspaceConfig::add_dep`
//! deduplicates on `==`, so repeated invocations do not accumulate
//! duplicate entries.
//!
//! Cache layout: `<workspace_root>/.memstead/memstead-io/<scope>/<name>.mem`.
//! The Tier 3 wiki-link resolver consumes the cached archive at this
//! exact path (criterion 7 of the plan, gated on filesystem engine
//! context).

use std::path::{Path, PathBuf};

use clap::Args;
use memstead_base::filesystem::config::{DepRef, read_workspace_config, write_workspace_config};
use serde_json::json;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::registry::{self, DownloadError};
use crate::setup::CliContext;

/// `memstead link` arguments.
#[derive(Args, Debug)]
pub struct LinkArgs {
    /// Cross-mem dependency in `scope/name` form (no `@` prefix —
    /// that is the `memstead install` shape). Tier 3 wiki-links use the
    /// same form, so the input here matches what users will type
    /// inside `[[scope/name:slug]]`.
    #[arg(value_name = "SCOPE/NAME")]
    pub dep: String,

    /// Override the registry URL. Falls back to `MEMSTEAD_REGISTRY` then
    /// the default `https://memstead.io`.
    #[arg(long, value_name = "URL")]
    pub registry: Option<String>,

    /// Override the workspace root. When omitted, the command walks up
    /// from the current working directory to find it.
    #[arg(long, value_name = "PATH")]
    pub workspace: Option<PathBuf>,
}

pub fn run(ctx: &CliContext, args: LinkArgs) -> anyhow::Result<()> {
    let dep: DepRef = args.dep.parse().map_err(|e: String| CliError {
        code: "INVALID_INPUT",
        message: format!(
            "invalid dependency reference {value:?}: {e} (expected 'scope/name')",
            value = args.dep
        ),
        kind: ExitKind::Validation,
        details: None,
    })?;

    let workspace_root = match args.workspace.clone() {
        Some(p) => {
            let canon = p.canonicalize().unwrap_or(p);
            if !memstead_base::is_workspace_root(&canon) {
                return Err(CliError {
                    code: "WORKSPACE_NOT_INITIALISED",
                    message: format!(
                        "no workspace at {} (missing .memstead/workspace.toml)",
                        canon.display()
                    ),
                    kind: ExitKind::NotFound,
                    details: None,
                }
                .into());
            }
            canon
        }
        None => find_filesystem_workspace_root()?,
    };

    let mut config = read_workspace_config(&workspace_root).map_err(|e| CliError {
        code: "WORKSPACE_CONFIG_READ_FAILED",
        message: format!("read workspace config: {e}"),
        kind: ExitKind::Generic,
        details: None,
    })?;

    let cache_dir = workspace_root
        .join(memstead_base::WORKSPACE_STORE_DIR)
        .join("memstead-io")
        .join(&dep.scope);
    std::fs::create_dir_all(&cache_dir).map_err(|e| CliError {
        code: crate::INTERNAL_CODE,
        message: format!("create cache dir {}: {e}", cache_dir.display()),
        kind: ExitKind::Generic,
        details: None,
    })?;
    let cache_path = cache_dir.join(format!(
        "{}.{}",
        dep.name,
        memstead_schema::ARCHIVE_EXTENSION
    ));

    let base = registry::registry_base(args.registry.as_deref());
    let client = registry::build_http()?;
    let bytes = registry::download_mem(&client, &base, &dep.scope, &dep.name, &cache_path)
        .map_err(|e| {
            let (msg, kind, code): (String, ExitKind, &'static str) = match e {
                DownloadError::NotFound => (
                    format!(
                        "registry has no mem {}/{} — check the spelling or `memstead publish` it first",
                        dep.scope, dep.name
                    ),
                    ExitKind::NotFound,
                    "REGISTRY_NOT_FOUND",
                ),
                DownloadError::Gone => (
                    format!(
                        "mem {}/{} has been unpublished from the registry",
                        dep.scope, dep.name
                    ),
                    ExitKind::NotFound,
                    "GONE",
                ),
                other => (
                    format!("download from registry: {other}"),
                    ExitKind::Generic,
                    "REGISTRY_ERROR",
                ),
            };
            CliError {
                code,
                message: msg,
                kind,
                details: None,
            }
        })?;

    let added = config.add_dep(dep.clone());
    write_workspace_config(&workspace_root, &config).map_err(|e| CliError {
        code: crate::INTERNAL_CODE,
        message: format!("update workspace config: {e}"),
        kind: ExitKind::Generic,
        details: None,
    })?;

    if ctx.json {
        let payload = json!({
            "scope": dep.scope,
            "name": dep.name,
            "cached_at": cache_path.display().to_string(),
            "bytes": bytes,
            "registry": base,
            "newly_recorded": added,
            "deps_total": config.deps.len(),
        });
        return print_json(&payload);
    }

    let action = if added { "Linked" } else { "Re-fetched" };
    let lines = [
        format!("# {} `{}`", action, dep.as_display()),
        String::new(),
        format!("- Cached:   `{}`", cache_path.display()),
        format!("- Bytes:    {bytes}"),
        format!("- Registry: {base}"),
        format!("- Total deps in this workspace: {}", config.deps.len()),
    ];
    print_markdown(&lines.join("\n"));
    Ok(())
}

/// Walk upward from `cwd` looking for the first ancestor that
/// carries `.memstead/workspace.toml` — the post-rebuild workspace
/// marker. Mirrors the resolver in `memstead-cli/src/setup.rs` and the
/// MCP binary's walker; keep them in sync.
fn find_filesystem_workspace_root() -> Result<PathBuf, CliError> {
    let cwd = std::env::current_dir().map_err(|e| CliError {
        code: crate::INTERNAL_CODE,
        message: format!("read cwd: {e}"),
        kind: ExitKind::Generic,
        details: None,
    })?;

    let mut current: &Path = &cwd;
    loop {
        if memstead_base::is_workspace_root(current) {
            return Ok(current.to_path_buf());
        }
        match current.parent() {
            Some(p) => current = p,
            None => {
                return Err(CliError {
                    code: "WORKSPACE_NOT_INITIALISED",
                    message: format!(
                        "no workspace found from {} or any ancestor (missing \
                         .memstead/workspace.toml) — run `memstead init` first or \
                         pass --workspace <path>",
                        cwd.display()
                    ),
                    kind: ExitKind::NotFound,
                    details: None,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memstead_base::filesystem::config::{FILESYSTEM_WORKSPACE_FORMAT, WorkspaceConfig};
    use memstead_schema::SchemaRef;
    use tempfile::TempDir;

    fn write_minimal_workspace(tmp: &TempDir) {
        // Lay down the post-rebuild marker `.memstead/workspace.toml` so
        // the link command's walk-up resolves. The legacy
        // `.memstead/config.json` still holds the `deps` list and lands
        // alongside via `write_workspace_config`.
        let memstead_dir = tmp.path().join(".memstead");
        std::fs::create_dir_all(&memstead_dir).unwrap();
        std::fs::write(
            memstead_dir.join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        let pin: SchemaRef = "default@1.0.0".parse().unwrap();
        let cfg = WorkspaceConfig::new("demo", pin);
        write_workspace_config(tmp.path(), &cfg).unwrap();
    }

    /// Spin up a tiny axum server that serves a single fixture archive
    /// at `/api/mem/<scope>/<name>.mem`. Returns the bound base
    /// URL (e.g. `http://127.0.0.1:54321`) and a `JoinHandle` the
    /// caller drops to shut the server down.
    async fn spawn_fixture_registry(
        scope: &'static str,
        name: &'static str,
        body: Vec<u8>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use axum::{Router, extract::Path as AxumPath, http::StatusCode, routing::get};
        use std::sync::Arc;

        let body = Arc::new(body);
        let app: Router = Router::new().route(
            "/api/mem/{scope_at}/{name_memstead}",
            get({
                let body = body.clone();
                move |AxumPath((scope_at, name_memstead)): AxumPath<(String, String)>| {
                    let body = body.clone();
                    async move {
                        let want_scope = scope.to_string();
                        let want_name = format!("{name}.mem");
                        if scope_at == want_scope && name_memstead == want_name {
                            (StatusCode::OK, (*body).clone())
                        } else {
                            (StatusCode::NOT_FOUND, vec![])
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn link_downloads_archive_and_records_dep() {
        let tmp = TempDir::new().unwrap();
        write_minimal_workspace(&tmp);

        let archive_bytes = b"fake-memstead-archive-bytes".to_vec();
        let (base, handle) =
            spawn_fixture_registry("anthropic", "core", archive_bytes.clone()).await;

        // Run on a blocking thread because `registry::download_mem`
        // is `reqwest::blocking`.
        let workspace = tmp.path().to_path_buf();
        let base_clone = base.clone();
        let result = tokio::task::spawn_blocking(move || {
            let ctx = CliContext {
                json: false,
                quiet: false,
            };
            run(
                &ctx,
                LinkArgs {
                    dep: "anthropic/core".to_string(),
                    registry: Some(base_clone),
                    workspace: Some(workspace),
                },
            )
        })
        .await
        .unwrap();
        handle.abort();
        result.unwrap();

        // Cached archive lands at the expected path with the expected bytes.
        let cache = tmp
            .path()
            .join(".memstead")
            .join("memstead-io")
            .join("anthropic")
            .join("core.mem");
        assert!(
            cache.is_file(),
            "cached archive must exist at {}",
            cache.display()
        );
        assert_eq!(std::fs::read(&cache).unwrap(), archive_bytes);

        // Workspace config records the dep.
        let cfg = read_workspace_config(tmp.path()).unwrap();
        assert_eq!(cfg.format, FILESYSTEM_WORKSPACE_FORMAT);
        assert_eq!(cfg.deps.len(), 1);
        assert_eq!(cfg.deps[0].as_display(), "anthropic/core");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn link_is_idempotent_on_repeat() {
        let tmp = TempDir::new().unwrap();
        write_minimal_workspace(&tmp);

        let archive_bytes = b"v1".to_vec();
        let (base, handle) =
            spawn_fixture_registry("anthropic", "core", archive_bytes.clone()).await;

        let workspace = tmp.path().to_path_buf();
        let base_clone = base.clone();
        for _ in 0..2 {
            let workspace = workspace.clone();
            let base_clone = base_clone.clone();
            tokio::task::spawn_blocking(move || {
                let ctx = CliContext {
                    json: false,
                    quiet: false,
                };
                run(
                    &ctx,
                    LinkArgs {
                        dep: "anthropic/core".to_string(),
                        registry: Some(base_clone),
                        workspace: Some(workspace),
                    },
                )
                .unwrap();
            })
            .await
            .unwrap();
        }
        handle.abort();

        let cfg = read_workspace_config(tmp.path()).unwrap();
        assert_eq!(
            cfg.deps.len(),
            1,
            "repeated link must not duplicate the dep entry"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn link_404_is_typed_and_actionable() {
        // Server only knows about `anthropic/core`; we ask for a
        // different name and expect a `NotFound` exit code.
        let tmp = TempDir::new().unwrap();
        write_minimal_workspace(&tmp);

        let (base, handle) = spawn_fixture_registry("anthropic", "core", b"".to_vec()).await;
        let workspace = tmp.path().to_path_buf();
        let base_clone = base.clone();
        let err = tokio::task::spawn_blocking(move || {
            let ctx = CliContext {
                json: false,
                quiet: false,
            };
            run(
                &ctx,
                LinkArgs {
                    dep: "anthropic/missing".to_string(),
                    registry: Some(base_clone),
                    workspace: Some(workspace),
                },
            )
            .unwrap_err()
        })
        .await
        .unwrap();
        handle.abort();
        let msg = err.to_string();
        assert!(
            msg.contains("registry has no mem"),
            "expected actionable 404 message, got: {msg}"
        );
    }

    #[test]
    fn link_rejects_invalid_dep_ref() {
        let tmp = TempDir::new().unwrap();
        write_minimal_workspace(&tmp);
        let ctx = CliContext {
            json: false,
            quiet: false,
        };
        let err = run(
            &ctx,
            LinkArgs {
                dep: "not-a-scope-name".to_string(),
                registry: None,
                workspace: Some(tmp.path().to_path_buf()),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid dependency reference"));
    }

    #[test]
    fn link_rejects_missing_workspace() {
        let tmp = TempDir::new().unwrap();
        // No .memstead/workspace.toml under tmp.
        let ctx = CliContext {
            json: false,
            quiet: false,
        };
        let err = run(
            &ctx,
            LinkArgs {
                dep: "anthropic/core".to_string(),
                registry: None,
                workspace: Some(tmp.path().to_path_buf()),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("no workspace"));
    }
}
