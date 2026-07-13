//! `memstead install` — two accepted input shapes:
//!
//! * `memstead install <path/to/file.mem>` — local-file install.
//! * `memstead install <scope>/<name>` — registry install.
//!   Downloads the archive from `<registry>/api/mem/<scope>/<name>.mem`
//!   into a tempfile, then funnels through the same
//!   `mem_cache::install_read_mem` helper the local path uses.
//!   No authentication required — registry downloads are public.
//!
//! Both shapes:
//!
//! 1. Copy (or re-validate) the archive into the global mem cache
//!    (`<data_dir>/memstead/mems/<name>-<key>.mem`) via the engine helper.
//! 2. Add a `readMems` entry to the target mem's `config.json` if
//!    the name isn't already declared. Local installs write
//!    `source: { type: "local" }`; registry installs write
//!    `source: { type: "url", url: "<registry>/api/mem/..." }`.

use std::path::{Path, PathBuf};

use clap::Parser;
use serde_json::json;

use memstead_git_branch::mem_cache::{self, TargetMem};
use memstead_git_branch::mem_repo_config;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::registry::{self, DownloadError};
use crate::setup::CliContext;
use crate::setup::cli_ctx;

/// Install a sealed mem archive into the global mem cache and register it
/// in the current project's `readMems`. Archives with non-slug-form
/// body wiki-links refuse with `INVALID_WIKI_LINK_TARGET` — convert via
/// search-and-replace before installing.
#[derive(Parser, Debug)]
pub struct Args {
    /// Either a path to a `.mem` file, or
    /// `<scope>/<name>` for registry installs (no `@` prefix).
    #[arg(value_name = "PATH or SCOPE/NAME")]
    pub source: String,

    /// Which writable mem to register this read-mem into (by
    /// name). Defaults to the first writable mem when omitted.
    ///
    /// This flag selects the *host* mem — the writable workspace
    /// mem that will list the archive in its read-mems set. It does
    /// NOT rename the archive's internal mem; the archive's internal
    /// name is the canonical identity used by all cross-mem
    /// references and shadow checks.
    #[arg(long = "mem", value_name = "NAME")]
    pub mem_name: Option<String>,

    /// Registry URL for `<scope>/<name>` installs. Ignored for local paths.
    /// Overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io.
    #[arg(long, value_name = "URL")]
    pub registry: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let engine = crate::setup::full_engine(ctx)?;

    let mem_name = resolve_mem_name(&engine, args.mem_name.clone())?;
    // Resolve target shape: mem-repo-backed mems have `dir: None`
    // under the dir-less create flow; the registration lands in
    // `mem-repo-git:__MEMSTEAD:mems/<mem_name>/config.json`. Disk
    // mems still get the `<mem_dir>/.memstead/config.json` rewrite.
    let mem_disk_dir = engine
        .mem_router()
        .dir_for_mem(&mem_name)
        .map(|p| p.to_path_buf());
    let workspace_root = engine
        .workspace_root()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    // Snapshot the workspace's writable-mount roster
    // so `install_read_mem` can refuse a shadowing archive name
    // before the cache copy + config registration lands.
    let writable: Vec<String> = engine
        .mem_router()
        .writable_mems()
        .iter()
        .map(|n| n.to_string())
        .collect();

    // The legacy `@scope/name` syntax is rejected, not silently treated as a
    // local path.
    if args.source.starts_with('@') {
        anyhow::bail!(
            "the `@scope/name` syntax is no longer supported — use \
             `github:<handle>/<name>`, `<domain>/<name>`, or a bare `<handle>/<name>`"
        );
    }
    // Registry install path: "<scope>/<name>".
    if let Some((scope, name)) = registry::parse_ref(&args.source) {
        return install_from_registry(
            ctx,
            &mem_name,
            mem_disk_dir.as_deref(),
            &workspace_root,
            &scope,
            &name,
            args.registry.as_deref(),
            &writable,
        );
    }

    // Local path install.
    install_from_local(
        ctx,
        &mem_name,
        mem_disk_dir.as_deref(),
        &workspace_root,
        &PathBuf::from(&args.source),
        &writable,
    )
}

fn install_from_local(
    ctx: &CliContext,
    mem_name: &str,
    mem_disk_dir: Option<&Path>,
    workspace_root: &Path,
    archive: &Path,
    writable: &[String],
) -> anyhow::Result<()> {
    let writable_refs: Vec<&str> = writable.iter().map(String::as_str).collect();
    let target = build_target(mem_name, mem_disk_dir, workspace_root);
    let commit_ctx = cli_ctx();
    let message = format!("memstead: install (read-mem registration into {mem_name})");
    let outcome =
        mem_cache::install_read_mem(archive, target, &commit_ctx, &message, &writable_refs)
            .map_err(install_err_to_cli)?;
    emit_outcome(ctx, mem_name, outcome, None)
}

// Flat forwarding of the install subcommand's CLI flags to the one
// registry download path; a params struct would just re-declare them.
#[allow(clippy::too_many_arguments)]
fn install_from_registry(
    ctx: &CliContext,
    mem_name: &str,
    mem_disk_dir: Option<&Path>,
    workspace_root: &Path,
    scope: &str,
    name: &str,
    registry_arg: Option<&str>,
    writable: &[String],
) -> anyhow::Result<()> {
    let base = registry::registry_base(registry_arg);
    let client = registry::build_http()?;

    // Stream the archive into a tempfile; `install_read_mem` reads
    // from a path, so a tempfile is the cheapest bridge.
    let tmp = tempfile::NamedTempFile::new().map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            crate::INTERNAL_CODE,
            format!("tempfile: {e}"),
        )
    })?;
    registry::download_mem(&client, &base, scope, name, tmp.path()).map_err(|e| {
        let msg = match &e {
            DownloadError::NotFound => {
                format!("{scope}/{name} not found on {base}")
            }
            DownloadError::Gone => {
                format!("{scope}/{name} has been taken down")
            }
            _ => format!("download failed: {e}"),
        };
        let code: &'static str = match &e {
            DownloadError::NotFound => "REGISTRY_NOT_FOUND",
            DownloadError::Gone => "GONE",
            _ => "REGISTRY_ERROR",
        };
        CliError::new(
            match e {
                DownloadError::NotFound => ExitKind::NotFound,
                _ => ExitKind::Generic,
            },
            code,
            msg,
        )
    })?;

    // The archive now lives at tmp.path(); hand it to the same helper
    // the local path uses. `install_read_mem` re-validates — the
    // consumer side is symmetric with the registry's server-side
    // validator by construction.
    let writable_refs: Vec<&str> = writable.iter().map(String::as_str).collect();
    let target = build_target(mem_name, mem_disk_dir, workspace_root);
    let commit_ctx = cli_ctx();
    let message = format!("memstead: install (read-mem registration into {mem_name})");
    let outcome =
        mem_cache::install_read_mem(tmp.path(), target, &commit_ctx, &message, &writable_refs)
            .map_err(install_err_to_cli)?;

    let source_url = format!(
        "{base}/api/mem/{scope}/{name}.mem",
        base = base,
        scope = scope,
        name = name
    );
    update_source_to_url(
        mem_name,
        mem_disk_dir,
        workspace_root,
        &outcome.mem_name,
        &source_url,
    )?;

    emit_outcome(ctx, mem_name, outcome, Some(source_url))
}

/// Resolve the install target: prefer the disk dir when present
/// (legacy disk-shaped mem), otherwise fall back to the mem-repo
/// shape rooted at the workspace.
fn build_target<'a>(
    mem_name: &'a str,
    mem_disk_dir: Option<&'a Path>,
    workspace_root: &'a Path,
) -> TargetMem<'a> {
    match mem_disk_dir {
        Some(p) => TargetMem::Disk(p),
        None => TargetMem::MemRepo {
            workspace_root,
            mem_name,
        },
    }
}

/// Rewrite the fresh `readMems` entry so its `source` becomes
/// `type: "url"` pointing back at the registry — `install_read_mem`
/// always writes `type: "local"`, which is right for files dropped by
/// hand but wrong for registry installs where the CLI can re-fetch.
///
/// Idempotent and safe: if the entry already has a `url` source, we
/// leave it alone (user edits win). Branches on disk vs. mem-repo
/// shape — disk mems rewrite `<mem_dir>/.memstead/config.json`,
/// mem-repo mems commit the updated blob to
/// `mem-repo-git:__MEMSTEAD:mems/<host_mem>/config.json`.
fn update_source_to_url(
    host_mem_name: &str,
    mem_disk_dir: Option<&Path>,
    workspace_root: &Path,
    read_mem_name: &str,
    source_url: &str,
) -> anyhow::Result<()> {
    use serde_json::{Map, Value, json};

    // Build the URL-source entry, preserving the content-addressed
    // `cacheKey` that `install_read_mem` wrote — the loader resolves the
    // cache file by `<name>-<cacheKey>.mem`, so dropping it here would
    // strand the just-installed archive.
    let url_entry = |existing: Option<&Value>| -> Value {
        let mut obj = Map::new();
        obj.insert("source".into(), json!({ "type": "url", "url": source_url }));
        if let Some(key) = existing
            .and_then(|e| e.get("cacheKey"))
            .and_then(|k| k.as_str())
        {
            obj.insert("cacheKey".into(), json!(key));
        }
        Value::Object(obj)
    };

    match mem_disk_dir {
        Some(mem_dir) => {
            let (mut config, config_path) =
                memstead_schema::config::load_config(mem_dir).map_err(|e| {
                    CliError::new(
                        ExitKind::Generic,
                        "WORKSPACE_CONFIG_READ_FAILED",
                        format!("reading config: {e}"),
                    )
                })?;

            let root = config.as_object_mut().ok_or_else(|| {
                CliError::new(
                    ExitKind::Generic,
                    "WORKSPACE_CONFIG_INVALID",
                    "config root must be a JSON object",
                )
            })?;
            let read_mems = root
                .entry("readMems")
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .ok_or_else(|| {
                    CliError::new(
                        ExitKind::Generic,
                        "WORKSPACE_CONFIG_INVALID",
                        "readMems must be a JSON object",
                    )
                })?;

            let existing_is_url_same = read_mems
                .get(read_mem_name)
                .and_then(|v| v.get("source"))
                .and_then(|s| s.get("type"))
                .and_then(|t| t.as_str())
                .is_some_and(|t| t == "url");
            if existing_is_url_same {
                return Ok(());
            }

            let entry = url_entry(read_mems.get(read_mem_name));
            read_mems.insert(read_mem_name.to_string(), entry);

            let body = serde_json::to_string_pretty(&config).map_err(|e| {
                CliError::new(
                    ExitKind::Generic,
                    crate::INTERNAL_CODE,
                    format!("serializing config: {e}"),
                )
            })?;
            std::fs::write(&config_path, body + "\n").map_err(|e| {
                CliError::new(
                    ExitKind::Generic,
                    crate::INTERNAL_CODE,
                    format!("writing config: {e}"),
                )
            })?;
            Ok(())
        }
        None => {
            // Mem-repo shape: read configs/<host_mem>.json, mutate, commit on main.
            let config =
                mem_repo_config::read_config(workspace_root, host_mem_name).map_err(|e| {
                    CliError::new(
                        ExitKind::Generic,
                        "WORKSPACE_CONFIG_READ_FAILED",
                        format!("reading configs/{host_mem_name}.json from mem-repo-git:main: {e}"),
                    )
                })?;
            let mut value = serde_json::to_value(&config).map_err(|e| {
                CliError::new(
                    ExitKind::Generic,
                    crate::INTERNAL_CODE,
                    format!("re-serialize MemConfig: {e}"),
                )
            })?;
            let root = value.as_object_mut().ok_or_else(|| {
                CliError::new(
                    ExitKind::Generic,
                    "WORKSPACE_CONFIG_INVALID",
                    "config root must be a JSON object",
                )
            })?;
            let read_mems = root
                .entry("readMems")
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .ok_or_else(|| {
                    CliError::new(
                        ExitKind::Generic,
                        "WORKSPACE_CONFIG_INVALID",
                        "readMems must be a JSON object",
                    )
                })?;

            let existing_is_url_same = read_mems
                .get(read_mem_name)
                .and_then(|v| v.get("source"))
                .and_then(|s| s.get("type"))
                .and_then(|t| t.as_str())
                .is_some_and(|t| t == "url");
            if existing_is_url_same {
                return Ok(());
            }

            let entry = url_entry(read_mems.get(read_mem_name));
            read_mems.insert(read_mem_name.to_string(), entry);

            let updated_bytes = serde_json::to_vec_pretty(&value).map_err(|e| {
                CliError::new(
                    ExitKind::Generic,
                    crate::INTERNAL_CODE,
                    format!("serializing updated config: {e}"),
                )
            })?;
            let commit_ctx = cli_ctx();
            let message = format!(
                "memstead: install (rewrite source URL for {read_mem_name} in {host_mem_name})"
            );
            mem_repo_config::commit_config(
                workspace_root,
                host_mem_name,
                &updated_bytes,
                &commit_ctx,
                &message,
            )
            .map_err(|e| {
                CliError::new(
                    ExitKind::Generic,
                    "WORKSPACE_CONFIG_WRITE_FAILED",
                    format!("commit configs/{host_mem_name}.json: {e}"),
                )
            })?;
            Ok(())
        }
    }
}

fn emit_outcome(
    ctx: &CliContext,
    target_mem: &str,
    outcome: mem_cache::InstallOutcome,
    source_url: Option<String>,
) -> anyhow::Result<()> {
    if ctx.json {
        print_json(&json!({
            "mem_name": outcome.mem_name,
            "copied_to_cache": outcome.copied_to_cache,
            "registered_in_config": outcome.registered_in_config,
            "target_mem": target_mem,
            "source_url": source_url,
            // `{ code, message, details }` envelopes — same shape every
            // warning-carrying surface uses.
            "warnings": outcome.warnings,
        }))?;
    } else {
        let cache_status = if outcome.copied_to_cache {
            "copied into cache"
        } else {
            "already in cache (unchanged)"
        };
        // Drop the on-disk `.memstead/config.json` path
        // from the success message. The path string does not exist for
        // mem-repo workspaces (configs live in `__MEMSTEAD` blobs in the
        // workspace registry ref); the message read as if the operator
        // could grep that path, which they cannot. Name the workspace
        // role instead.
        let config_status = if outcome.registered_in_config {
            format!("registered as a read-mem on `{target_mem}`'s workspace config")
        } else {
            format!("already registered as a read-mem on `{target_mem}`'s workspace config")
        };
        let mut body = format!(
            "# Installed `{}`\n\n- Archive: {}\n- Config: {}",
            outcome.mem_name, cache_status, config_status,
        );
        if let Some(url) = source_url {
            body.push_str(&format!("\n- Source: {url}"));
        }
        if !outcome.warnings.is_empty() {
            body.push_str("\n\n## Warnings\n");
            for w in &outcome.warnings {
                body.push_str(&format!("\n- **{}**: {}", w.code(), w.message()));
            }
        }
        print_markdown(&body);
    }
    Ok(())
}

/// Map `InstallError` into the CLI error envelope. The
/// `ShadowsWritable` variant gets a typed
/// `READ_MEM_SHADOWS_WRITABLE` wire code with structured
/// `details.archive_name` + `details.shadows_writable` so callers
/// branch on the code rather than parsing the message. Other
/// variants stay on the generic exit code with the underlying error
/// message — they already carry the right shape for the CLI.
fn install_err_to_cli(e: memstead_git_branch::mem_cache::InstallError) -> anyhow::Error {
    use memstead_git_branch::mem_cache::InstallError;
    if let InstallError::ShadowsWritable {
        archive_name,
        shadows_writable,
    } = &e
    {
        return CliError::new(
            ExitKind::Validation,
            "READ_MEM_SHADOWS_WRITABLE",
            e.to_string(),
        )
        .with_details(json!({
            "archive_name": archive_name,
            "shadows_writable": shadows_writable,
        }))
        .into();
    }
    // There is no `CACHE_NAME_COLLISION` mapping: the cache is
    // content-addressed (`<name>-<content_key>.mem`), so distinct bytes
    // under the same mem name don't collide and the engine cannot produce
    // `InstallError::CacheNameCollision`.
    // Install-archive validation failures route through the typed
    // ARCHIVE_VALIDATION_FAILED code (F10 of the 2026-05-18 CLI probe).
    // Other InstallError variants (write failures, etc.) flow through the
    // same envelope but the wire-shape captures the refusal source via the
    // message text.
    CliError::new(
        ExitKind::Generic,
        crate::ARCHIVE_VALIDATION_FAILED_CODE,
        e.to_string(),
    )
    .into()
}

fn resolve_mem_name(
    engine: &memstead_base::Engine,
    explicit: Option<String>,
) -> anyhow::Result<String> {
    let writable: Vec<String> = engine
        .mem_configs_named()
        .filter(|(name, _)| engine.mem_router().is_writable(name))
        .map(|(name, _)| name.to_string())
        .collect();

    if let Some(name) = explicit {
        // Precondition check at the entry point. Otherwise an
        // unknown mem name flows through to archive validation,
        // which surfaces a misleading `ARCHIVE_VALIDATION_FAILED`
        // envelope carrying a `__MEMSTEAD:mems/...` internal path
        // (the path is engine-private; the failure root cause is
        // the missing host mem). The typed refusal here pins the
        // actual precondition the caller violated and short-
        // circuits the leak path.
        if !writable.iter().any(|v| v == &name) {
            return Err(CliError::new(
                ExitKind::Validation,
                "HOST_MEM_NOT_REGISTERED",
                format!(
                    "host mem `{name}` is not a registered writable mem — \
                     run `memstead mem init {name}` first OR pass `--mem <existing>`",
                ),
            )
            .with_details(json!({
                "requested": name,
                "known_mems": writable,
            }))
            .into());
        }
        return Ok(name);
    }

    match writable.len() {
        0 => Err(CliError::new(
            ExitKind::Generic,
            "NO_WRITABLE_MEM",
            "no writable mem loaded — nothing to install into",
        )
        .into()),
        1 => Ok(writable.into_iter().next().unwrap()),
        _ => Err(CliError::new(
            ExitKind::Validation,
            "AMBIGUOUS_MEM",
            format!(
                "multiple writable mems loaded ({}); pass --mem <name> \
                 to pick the install target",
                writable.join(", ")
            ),
        )
        .with_details(json!({ "mems": writable }))
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use crate::registry::parse_ref;

    #[test]
    fn parse_ref_accepts_three_scope_forms() {
        assert_eq!(
            parse_ref("memstead/knowledge"),
            Some(("memstead".into(), "knowledge".into()))
        );
        assert_eq!(
            parse_ref("github:alice/foo"),
            Some(("github:alice".into(), "foo".into()))
        );
        assert_eq!(
            parse_ref("acme.com:payments/foo"),
            Some(("acme.com:payments".into(), "foo".into()))
        );
    }

    #[test]
    fn parse_ref_rejects_local_paths() {
        assert!(parse_ref("/tmp/foo.mem").is_none());
        assert!(parse_ref("./foo.mem").is_none());
        assert!(parse_ref("foo.mem").is_none());
    }

    #[test]
    fn parse_ref_rejects_legacy_at_and_malformed() {
        // The legacy `@scope/name` syntax is not a valid registry ref.
        assert!(parse_ref("@memstead/knowledge").is_none());
        assert!(parse_ref("memstead").is_none()); // no name
        assert!(parse_ref("/knowledge").is_none()); // empty scope
        assert!(parse_ref("memstead/").is_none()); // empty name
        assert!(parse_ref("memstead/knowledge.mem").is_none()); // extension
        assert!(parse_ref("memstead/subdir/knowledge").is_none()); // path-shaped name
    }
}
