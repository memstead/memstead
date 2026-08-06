//! `memstead install` — two accepted input shapes:
//!
//! * `memstead install <path/to/file.mem>` — local-file install.
//! * `memstead install <scope>/<name>` — registry install.
//!   Downloads the archive from `<registry>/api/mem/<scope>/<name>.mem`
//!   into a tempfile, then funnels through the same cache helper the
//!   local path uses. No authentication required — registry downloads
//!   are public.
//!
//! Both shapes:
//!
//! 1. Validate and copy (or re-validate) the archive into the global
//!    mem cache (`<data_dir>/memstead/mems/<name>-<key>.mem`).
//! 2. Register the archive as a **workspace-level read-only mount**
//!    in the engine-managed mount state (`.memstead/state/mounts.json`),
//!    carrying `capability: read_only` and the content-addressed cache
//!    path as its `Archive` storage reference. No writable mem's
//!    config is touched — a read-mem attaches to the workspace, not to
//!    a host mem. `memstead uninstall <name>` is the symmetric removal.

use std::path::{Path, PathBuf};

use clap::Parser;
use serde_json::json;

use memstead_git_branch::mem_cache::{self, CacheInstallOutcome, MountRegistration};

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::registry::{self, DownloadError};
use crate::setup::CliContext;

/// Install a sealed mem archive: validate + copy into the global mem
/// cache, then register it as a workspace-level read-only mount. The
/// archive's internal name is its sole identity — cross-mem references
/// and shadow checks use it. Archives with non-slug-form body
/// wiki-links refuse with `INVALID_WIKI_LINK_TARGET` — convert via
/// search-and-replace before installing.
#[derive(Parser, Debug)]
pub struct Args {
    /// Either a path to a `.mem` file, or
    /// `<scope>/<name>` for registry installs (no `@` prefix).
    #[arg(value_name = "PATH or SCOPE/NAME")]
    pub source: String,

    /// Registry URL for `<scope>/<name>` installs. Ignored for local paths.
    /// Overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io.
    #[arg(long, value_name = "URL")]
    pub registry: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let mut engine = crate::setup::full_engine(ctx)?;

    // The legacy `@scope/name` syntax is rejected, not silently treated as a
    // local path. Typed refusal — a user-triggerable input shape must
    // never surface as INTERNAL.
    if args.source.starts_with('@') {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            "the `@scope/name` syntax is no longer supported — use \
             `github:<handle>/<name>`, `<domain>/<name>`, or a bare `<handle>/<name>`",
        )
        .into());
    }

    // Registry install path: "<scope>/<name>".
    if let Some((scope, name)) = registry::parse_ref(&args.source) {
        let base = registry::registry_base(args.registry.as_deref());
        let client = registry::build_http()?;

        // Stream the archive into a tempfile; the cache helper reads
        // from a path, so a tempfile is the cheapest bridge.
        let tmp = tempfile::NamedTempFile::new().map_err(|e| {
            CliError::new(
                ExitKind::Generic,
                crate::INTERNAL_CODE,
                format!("tempfile: {e}"),
            )
        })?;
        registry::download_mem(&client, &base, &scope, &name, tmp.path()).map_err(|e| {
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

        let source_url = format!(
            "{base}/api/mem/{scope}/{name}.mem",
            base = base,
            scope = scope,
            name = name
        );
        return install_archive(ctx, &mut engine, tmp.path(), Some(source_url));
    }

    // Local path install.
    let path = PathBuf::from(&args.source);
    install_archive(ctx, &mut engine, &path, None)
}

/// The shared back half of both install shapes: cache the archive,
/// then register (or refresh) the workspace-level read-only mount.
fn install_archive(
    ctx: &CliContext,
    engine: &mut memstead_base::Engine,
    archive: &Path,
    source_url: Option<String>,
) -> anyhow::Result<()> {
    // The shadow gate runs against the writable roster — an archive
    // whose internal name collides with a writable mem refuses before
    // any side effect.
    let writable: Vec<String> = engine
        .mem_router()
        .writable_mems()
        .iter()
        .map(|n| n.to_string())
        .collect();
    let writable_refs: Vec<&str> = writable.iter().map(String::as_str).collect();

    let outcome =
        mem_cache::install_to_cache(archive, &writable_refs).map_err(install_err_to_cli)?;

    let mount_state =
        mem_cache::register_cached_archive(engine, &outcome, "memstead install")
            .map_err(engine_err_to_cli)?;
    if mount_state != mem_cache::MountRegistration::AlreadyRegistered {
        engine.persist_state().map_err(engine_err_to_cli)?;
    }

    emit_outcome(ctx, outcome, mount_state, source_url)
}

fn emit_outcome(
    ctx: &CliContext,
    outcome: CacheInstallOutcome,
    mount_state: MountRegistration,
    source_url: Option<String>,
) -> anyhow::Result<()> {
    let mount_status_wire = match mount_state {
        MountRegistration::Registered => "registered",
        MountRegistration::AlreadyRegistered => "already_registered",
        MountRegistration::Refreshed => "refreshed",
    };
    if ctx.json {
        print_json(&json!({
            "mem_name": outcome.mem_name,
            "copied_to_cache": outcome.copied_to_cache,
            "mount": mount_status_wire,
            "cache_path": outcome.cache_path.to_string_lossy(),
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
        let mount_status = match mount_state {
            MountRegistration::Registered => {
                "registered as a workspace-level read-only mount".to_string()
            }
            MountRegistration::AlreadyRegistered => {
                "already registered as a read-mem mount (unchanged)".to_string()
            }
            MountRegistration::Refreshed => {
                "read-mem mount refreshed to the new archive content".to_string()
            }
        };
        let mut body = format!(
            "# Installed `{}`\n\n- Archive: {}\n- Mount: {}",
            outcome.mem_name, cache_status, mount_status,
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

/// Map engine-side registration errors into the typed CLI envelope.
fn engine_err_to_cli(e: memstead_base::EngineError) -> anyhow::Error {
    CliError::from_engine_op(e).into()
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
