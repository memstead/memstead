//! `memstead publish [<file.mem>]` — upload a mem to the registry.
//!
//! Three input shapes, resolved in priority order:
//!
//! - **`memstead publish <file.mem>`** — archive-already-built. Publish
//!   pre-existing bytes (e.g. produced by `memstead export --format mem`).
//! - **`memstead publish --mem <name>`** — export-and-publish in one
//!   step. Opens the current workspace's engine (any backend, including
//!   git-branch mem-repo), assembles the named mem's `.mem` archive
//!   in-process via [`memstead_base::Engine::export_mem_to_bytes`],
//!   stages it through a tempfile, and posts. This is the one-step path
//!   for mem-repo workspaces, where there is no folder to wrap up.
//! - **`memstead publish`** (no archive arg, no `--mem`) —
//!   filesystem-mem assembly. Walks up from cwd to the workspace
//!   marker, builds the archive in-memory via
//!   [`memstead_base::filesystem::publish::assemble_archive`], and posts.
//!   Equivalent to "wrap up what's in the current folder and ship it".
//!
//! Token resolution (first hit wins): `--token` → `MEMSTEAD_TOKEN` env →
//! `~/.config/memstead/credentials` → GitHub Device Flow on first use
//! (only if stdin is a TTY; CI sees "missing MEMSTEAD_TOKEN" instead).
//!
//! On success prints `<scope>/<name> vX.Y.Z` + the full mem URL so the
//! user has a clickable link.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::Parser;
use memstead_base::filesystem::publish::assemble_archive;
use serde_json::json;
use tempfile::NamedTempFile;

use crate::CliError;
use crate::auth::{credentials, device_flow, resolve_token};
use crate::output::{ExitKind, print_json, print_markdown};
use crate::registry::{self, ApiErrorBody, PublishError};
use crate::setup::CliContext;

#[derive(Parser, Debug)]
pub struct Args {
    /// Path to a `.mem` archive on disk. Omit to assemble the
    /// archive from the surrounding filesystem-mem workspace
    /// (walks up from cwd to find the workspace root).
    #[arg(value_name = "PATH")]
    pub archive: Option<PathBuf>,

    /// Export-and-publish a named mem from the current workspace in
    /// one step — the path for mem-repo (multi-mem, git-branch)
    /// workspaces, which have no folder to wrap up. Ignored when an
    /// archive PATH is provided. A single-mem folder workspace can
    /// omit this and just run `memstead publish`.
    #[arg(long, value_name = "NAME")]
    pub mem: Option<String>,

    /// Override the auto-derived scope — admin-only, reserved scopes
    /// only (currently just `memstead`). Without this flag the registry
    /// stores the mem under your GitHub username.
    #[arg(long, value_name = "NAME")]
    pub scope: Option<String>,

    /// Explicit token override. Takes precedence over `MEMSTEAD_TOKEN`
    /// and stored credentials.
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,

    /// Registry URL (overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io).
    #[arg(long, value_name = "URL")]
    pub registry: Option<String>,

    /// Set the mem's version to this semver and publish in one step,
    /// persisting the bump to the mem config (like `npm version` +
    /// `npm publish`). Requires `--mem <name>`; not valid with a
    /// pre-built archive PATH, whose version is already baked in. Omit
    /// to publish whatever version the mem config currently carries.
    #[arg(long, value_name = "SEMVER")]
    pub version: Option<String>,

    /// Blank every artifact reference in the packaged anchors sidecar —
    /// the `artifact` field and each `derived_from` entry — so the
    /// published mem discloses no source identity while the trust
    /// metadata (provenance class, at_version, grain, hash, source name)
    /// stays readable. Redact, not strip: consumers still see how
    /// strongly each entity claims fidelity to a source, without
    /// learning which source. The workspace's own sidecar is never
    /// touched. Residual disclosure remains by design — grain reveals
    /// the medium shape, at_version may carry a commit SHA, source is
    /// your chosen name, and hash permits confirming guessed content.
    /// Not valid with a pre-built archive PATH, whose anchors are
    /// already baked in — use --mem or the bare folder shape instead.
    #[arg(long)]
    pub redact_anchors: bool,

    /// Assemble and resolve everything, print exactly what would be
    /// published (mem, version, scope, archive size), but POST
    /// nothing and mutate nothing — including no version bump. The safe
    /// way to confirm a publish before it goes out.
    #[arg(long)]
    pub dry_run: bool,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    run_with_root(ctx, args, None)
}

/// Inner seam: `root_override` replaces the cwd walk for the
/// assembling shapes — used by tests (the CLI-level workspace override
/// is the ROOT command's global `--workspace` / `MEMSTEAD_WORKSPACE`,
/// applied before dispatch; no subcommand-level flag exists).
fn run_with_root(
    ctx: &CliContext,
    args: Args,
    root_override: Option<PathBuf>,
) -> anyhow::Result<()> {
    let base = registry::registry_base(args.registry.as_deref());
    let host = registry::registry_host(&base);
    let client = registry::build_http()?;

    // 0. Validate `--version` up front: it persists a bump through the
    //    workspace engine, so it needs `--mem <name>` and is
    //    meaningless against pre-built archive bytes whose version is
    //    already sealed.
    let target_version = match args.version.as_deref() {
        Some(v) => {
            if args.archive.is_some() {
                return Err(CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    "--version cannot be combined with a pre-built archive PATH (its version is already baked in) — drop the PATH and use --mem, or re-export at the new version",
                )
                .into());
            }
            if args.mem.is_none() {
                return Err(CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    "--version requires --mem <name> so the bump knows which mem to re-version",
                )
                .into());
            }
            Some(semver::Version::parse(v).map_err(|e| {
                CliError::new(
                    ExitKind::Validation,
                    "INVALID_VERSION",
                    format!("--version {v:?} is not a valid semver: {e}"),
                )
            })?)
        }
        None => None,
    };

    // 0b. `--redact-anchors` transforms the sidecar where the archive is
    //     assembled — pre-built bytes are refused up front (before any
    //     auth or network step), same precedent as `--version` above.
    if args.redact_anchors && args.archive.is_some() {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            "--redact-anchors cannot be combined with a pre-built archive PATH (its \
             anchors are already baked in) — drop the PATH and use --mem <name>, or run \
             the bare `memstead publish` from the mem's folder, both of which assemble \
             the archive and can redact it",
        )
        .into());
    }

    // 1. Resolve archive bytes by input shape (priority order):
    //    archive PATH > `--mem NAME` (engine export-to-bytes, any
    //    backend) > bare (folder assembly). The two assembling shapes
    //    stage their bytes through a tempfile so the existing
    //    `registry::publish` POST path stays file-based; the tempfile
    //    guard is held until the end of `run` so the path stays valid
    //    for the POST call. `resolved_version` is the version that will
    //    actually publish — surfaced in the dry-run preview.
    let mut resolved_version: Option<String> = None;
    let (archive_path, _tempfile_guard): (PathBuf, Option<NamedTempFile>) =
        if let Some(p) = args.archive {
            (p, None)
        } else if let Some(mem_name) = args.mem.as_deref() {
            let workspace_root = resolve_workspace_root(root_override.as_deref())?;
            let mut engine = ctx.cli_engine_at(&workspace_root)?.into_base();
            // Persist the version bump before exporting — but never
            // under --dry-run, which must leave the workspace untouched.
            if let Some(ver) = target_version.clone()
                && !args.dry_run
            {
                let bumped = engine
                    .set_mem_version(mem_name, ver, Some("version bump for registry publish"))
                    .map_err(CliError::from_engine_op)?;
                // Publish is about to ship this mem outward, so a config that
                // moved under us is exactly what the operator needs to see
                // before the archive leaves (04/03, criterion 3).
                for w in &bumped.warnings {
                    crate::output::print_markdown(&format!("> {w}"));
                }
            }
            resolved_version = target_version.as_ref().map(|v| v.to_string()).or_else(|| {
                engine
                    .mem_config_for(mem_name)
                    .and_then(|c| c.version.clone())
                    .map(|v| v.to_string())
            });
            let bytes = engine
                .export_mem_to_bytes(mem_name)
                .map_err(CliError::from_engine_op)?;
            stage_bytes_to_tempfile(&redact_if_requested(&args, bytes)?)?
        } else {
            let workspace_root = resolve_workspace_root(root_override.as_deref())?;
            // Engine-first: the engine exports whatever layout it
            // booted — the mount roster locates the mem folder and its
            // config, so the bare shape works from the workspace root
            // of a current-layout (`workspace.toml` + mounts) workspace
            // exactly like `--mem` does (sealed-gate finding F6's seam;
            // the legacy assembly resolved the config against the
            // workspace root, which only coincides with the mem folder
            // in the legacy single-mem layout). The legacy folder
            // assembly stays as the fallback for exactly that layout,
            // where no engine boots.
            // Exactly one writable mem routes through the engine;
            // multiple demand `--mem`; zero (an engine that boots but
            // mounts nothing writable — some legacy-shaped test trees)
            // falls through to the folder assembly like a failed boot.
            let engine_mem: Option<(memstead_base::Engine, String)> =
                match ctx.cli_engine_at(&workspace_root) {
                    Ok(cli_engine) => {
                        let engine = cli_engine.into_base();
                        let writable: Vec<String> = engine
                            // Every mount (04/05, criterion 8). Counting only
                            // config-readable mems could report exactly one
                            // writable mem while a second, broken one exists,
                            // and silently publish the wrong one.
                            .mounts_with_optional_config()
                            .filter(|(name, _)| engine.mem_router().is_writable(name))
                            .map(|(name, _)| name.to_string())
                            .collect();
                        match writable.len() {
                            1 => Some((engine, writable.into_iter().next().unwrap())),
                            0 => None,
                            _ => {
                                return Err(CliError::new(
                                    ExitKind::Validation,
                                    "AMBIGUOUS_MEM",
                                    format!(
                                        "multiple writable mems loaded ({}); pass --mem <name>",
                                        writable.join(", ")
                                    ),
                                )
                                .into());
                            }
                        }
                    }
                    Err(_) => None,
                };
            let bytes = match engine_mem {
                Some((engine, mem_name)) => {
                    resolved_version = engine
                        .mem_config_for(&mem_name)
                        .and_then(|c| c.version.clone())
                        .map(|v| v.to_string());
                    engine
                        .export_mem_to_bytes(&mem_name)
                        .map_err(CliError::from_engine_op)?
                }
                None => assemble_archive(&workspace_root).map_err(|e| {
                    CliError::new(
                        ExitKind::Validation,
                        "ARCHIVE_ASSEMBLY_FAILED",
                        format!("assemble archive: {e}"),
                    )
                })?,
            };
            stage_bytes_to_tempfile(&redact_if_requested(&args, bytes)?)?
        };

    // 2. Dry run: report the resolved publish and stop — no auth, no
    //    POST, no mutation (any --version bump was skipped above).
    if args.dry_run {
        return emit_dry_run(
            ctx,
            &base,
            &archive_path,
            args.mem.as_deref(),
            resolved_version.as_deref(),
            args.scope.as_deref(),
        );
    }

    // 3. Authorise + POST. A `<domain>:<handle>` scope is a domain-authority
    //    publish: it signs the upload with the domain's locally-stored key and
    //    needs no GitHub account. Any other scope uses the GitHub token path
    //    (with interactive device-flow fallback on a TTY).
    if let Some(domain) = domain_scope(args.scope.as_deref()) {
        let scope = args.scope.as_deref().expect("domain_scope implies a scope");
        let sig = build_domain_signature(&archive_path, scope, &domain)?;
        return match registry::publish(&client, &base, &archive_path, None, Some(scope), Some(&sig))
        {
            Ok(resp) => emit_success(ctx, &base, &resp),
            Err(e) => Err(map_publish_error(e).into()),
        };
    }

    let token = match resolve_token(&host, args.token.as_deref())? {
        Some(r) => r.token,
        None => {
            if !std::io::stdin().is_terminal() {
                return Err(CliError::new(
                    ExitKind::Generic,
                    "NOT_AUTHENTICATED",
                    "not logged in and stdin is not a TTY — set MEMSTEAD_TOKEN \
                     or run `memstead login` first",
                )
                .into());
            }
            login_inline(&client, &host)?
        }
    };

    match registry::publish(
        &client,
        &base,
        &archive_path,
        Some(&token),
        args.scope.as_deref(),
        None,
    ) {
        Ok(resp) => emit_success(ctx, &base, &resp),
        Err(e) => Err(map_publish_error(e).into()),
    }
}

/// Apply `--redact-anchors` to assembled archive bytes — publish-time
/// only, on the staged copy; the workspace's own sidecar is untouched.
/// An archive with no anchors member passes through byte-identical.
fn redact_if_requested(args: &Args, bytes: Vec<u8>) -> Result<Vec<u8>, CliError> {
    if !args.redact_anchors {
        return Ok(bytes);
    }
    memstead_base::filesystem::publish::redact_archive_anchors(&bytes).map_err(|e| {
        CliError::new(
            ExitKind::Validation,
            "ARCHIVE_ASSEMBLY_FAILED",
            format!("redact anchors: {e}"),
        )
    })
}

/// A `<domain>:<handle>` scope override → the domain. A domain scope's prefix
/// contains a `.` (e.g. `acme.com:payments`); `github:<h>` and bare handles do
/// not, so they fall through to the GitHub path.
fn domain_scope(scope: Option<&str>) -> Option<String> {
    let (prefix, handle) = scope?.split_once(':')?;
    if prefix.contains('.') && !handle.is_empty() {
        Some(prefix.to_ascii_lowercase())
    } else {
        None
    }
}

/// Build the per-publish domain signature: canonicalize the archive (the
/// signature covers the canonical content hash the registry will also compute),
/// then sign `(hash, scope, name, version, now)` with the domain's stored key.
#[cfg(feature = "mem-repo")]
fn build_domain_signature(
    archive_path: &Path,
    scope: &str,
    domain: &str,
) -> anyhow::Result<registry::DomainSignature> {
    use memstead_base::domain_authority_wire::signing_payload;
    use memstead_git_branch::validator::validate_and_normalize_archive;
    use sha2::{Digest, Sha256};

    use crate::auth::domain_key;

    let bytes = std::fs::read(archive_path).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "ARCHIVE_READ_FAILED",
            format!("read archive: {e}"),
        )
    })?;
    let validated = validate_and_normalize_archive(&bytes).map_err(|e| {
        CliError::new(
            ExitKind::Validation,
            "ARCHIVE_INVALID",
            format!("archive failed local validation before signing: {e}"),
        )
    })?;
    let content_sha256 = {
        let mut h = Sha256::new();
        h.update(&validated.canonical_bytes);
        h.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let name = validated.config.name.clone();
    let version = validated.config.version.to_string();

    let signing = domain_key::load(domain)
        .map_err(|e| CliError::new(ExitKind::NotFound, "DOMAIN_KEY_NOT_FOUND", e.to_string()))?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let payload = signing_payload(&content_sha256, scope, &name, &version, timestamp);
    Ok(registry::DomainSignature {
        key: domain_key::public_key_string(&signing),
        signature: domain_key::sign(&signing, &payload),
        timestamp,
    })
}

/// Lean build: canonicalizing an archive needs the git-branch validator, which
/// is only compiled into the full `memstead` binary. Domain publishing is
/// therefore unavailable here.
#[cfg(not(feature = "mem-repo"))]
fn build_domain_signature(
    _archive_path: &Path,
    _scope: &str,
    _domain: &str,
) -> anyhow::Result<registry::DomainSignature> {
    Err(CliError::new(
        ExitKind::Generic,
        "DOMAIN_PUBLISH_UNAVAILABLE",
        "domain publishing requires the full `memstead` build (the lean build cannot \
         canonicalize archives for signing)",
    )
    .into())
}

/// Render the `--dry-run` preview: what the real publish would send,
/// with nothing posted and nothing mutated. `scope` is the admin
/// override when present; otherwise the registry derives it from the
/// caller's GitHub login, which the client cannot know offline.
fn emit_dry_run(
    ctx: &CliContext,
    base: &str,
    archive_path: &Path,
    mem: Option<&str>,
    version: Option<&str>,
    scope: Option<&str>,
) -> anyhow::Result<()> {
    let size = std::fs::metadata(archive_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let mem_label = mem.unwrap_or("(workspace mem)");
    let version_label = version.unwrap_or("(from mem config / archive)");
    if ctx.json {
        print_json(&json!({
            "dry_run": true,
            "mem": mem,
            "version": version,
            "scope": scope,
            "archive_bytes": size,
            "registry": base,
            "published": false,
        }))?;
    } else {
        let scope_label = match scope {
            Some(s) => format!("`{s}` (override)"),
            None => "derived from your GitHub login".to_string(),
        };
        print_markdown(&format!(
            "# Dry run — would publish\n\n\
             - Mem: `{mem_label}`\n\
             - Version: `{version_label}`\n\
             - Scope: {scope_label}\n\
             - Archive: {size} bytes\n\
             - Registry: {base}\n\n\
             Nothing was published and nothing was changed.",
        ));
    }
    Ok(())
}

/// Walk upward from cwd looking for the first ancestor that carries
/// `.memstead/workspace.toml` — the post-rebuild workspace marker.
/// Mirrors the MCP binary's walker; keep the two in sync.
fn find_filesystem_workspace_root() -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir().map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            crate::INTERNAL_CODE,
            format!("read cwd: {e}"),
        )
    })?;
    let mut current: &Path = &cwd;
    loop {
        if memstead_base::is_workspace_root(current) {
            return Ok(current.to_path_buf());
        }
        match current.parent() {
            Some(p) => current = p,
            None => {
                return Err(CliError::new(
                    ExitKind::NotFound,
                    "WORKSPACE_NOT_INITIALISED",
                    format!(
                        "no workspace found from {} or any ancestor (missing \
                         .memstead/workspace.toml) — run `memstead init` first, pass \
                         --workspace <path>, or supply an archive path",
                        cwd.display()
                    ),
                )
                .into());
            }
        }
    }
}

/// Resolve the workspace root for the assembling shapes by walking up
/// from cwd. Shared by the `--mem` and bare-folder paths.
fn resolve_workspace_root(root_override: Option<&Path>) -> anyhow::Result<PathBuf> {
    // Workspace targeting is the root command's job (global
    // `--workspace` / `MEMSTEAD_WORKSPACE`, validated + applied before
    // dispatch); `root_override` is the in-process test seam.
    match root_override {
        Some(p) => Ok(p.to_path_buf()),
        None => find_filesystem_workspace_root(),
    }
}

/// Write assembled archive bytes to a tempfile so the file-based POST
/// path can read them back. Returns the path plus the `NamedTempFile`
/// guard the caller must hold until the POST completes.
fn stage_bytes_to_tempfile(bytes: &[u8]) -> anyhow::Result<(PathBuf, Option<NamedTempFile>)> {
    let tempfile = NamedTempFile::new().map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            crate::INTERNAL_CODE,
            format!("tempfile: {e}"),
        )
    })?;
    std::fs::write(tempfile.path(), bytes).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            crate::INTERNAL_CODE,
            format!("write tempfile {}: {e}", tempfile.path().display()),
        )
    })?;
    let path = tempfile.path().to_path_buf();
    Ok((path, Some(tempfile)))
}

fn login_inline(client: &reqwest::blocking::Client, host: &str) -> anyhow::Result<String> {
    println!("Not logged in — starting GitHub Device Flow…");
    let outcome = device_flow::run(
        client,
        device_flow::MEMSTEAD_GITHUB_CLIENT_ID,
        device_flow::MEMSTEAD_GITHUB_SCOPE,
        |url| {
            let _ = device_flow::open_browser(url);
        },
    )
    .map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "LOGIN_FAILED",
            format!("login failed: {e}"),
        )
    })?;

    // Best-effort username lookup for the credentials entry.
    let user_login = fetch_login(client, &outcome.access_token).unwrap_or_default();

    let entry = credentials::Entry::new(
        outcome.access_token.clone(),
        user_login,
        outcome.scopes.clone(),
    );
    credentials::save_for(host, entry)?;

    Ok(outcome.access_token)
}

fn fetch_login(client: &reqwest::blocking::Client, token: &str) -> anyhow::Result<String> {
    let base = std::env::var("MEMSTEAD_GITHUB_API_BASE")
        .unwrap_or_else(|_| "https://api.github.com".to_string());
    let url = format!("{}/user", base.trim_end_matches('/'));
    let resp = client
        .get(url)
        .bearer_auth(token)
        .header("accept", "application/vnd.github+json")
        .send()?;
    if !resp.status().is_success() {
        anyhow::bail!("GitHub /user returned {}", resp.status());
    }
    #[derive(serde::Deserialize)]
    struct User {
        login: String,
    }
    let user: User = resp.json()?;
    Ok(user.login)
}

fn emit_success(
    ctx: &CliContext,
    base: &str,
    resp: &registry::PublishResponse,
) -> anyhow::Result<()> {
    let full_url = format!("{}{}", base, resp.url);
    // Honest signal: the registry promotes the highest published version
    // to `current`, so publishing an older version succeeds but does not
    // become what users get by default. Surface that rather than letting
    // the bare "Published vX" imply X is now live.
    let demoted = resp.current.as_deref().filter(|cur| *cur != resp.version);
    if ctx.json {
        print_json(&json!({
            "ok": true,
            "scope": resp.scope,
            "name": resp.name,
            "version": resp.version,
            "current": resp.current,
            "url": full_url,
        }))?;
    } else {
        let mut block = format!(
            "# Published {}/{} v{}\n\n- URL: {}",
            resp.scope, resp.name, resp.version, full_url,
        );
        if let Some(cur) = demoted {
            block.push_str(&format!(
                "\n\n> Note: `current` stays at v{cur} — you published an older version, \
                 so it is retained and resolvable but is not the default users get.",
            ));
        }
        print_markdown(&block);
    }
    Ok(())
}

fn map_publish_error(err: PublishError) -> CliError {
    match err {
        PublishError::Io(e) => CliError::new(
            ExitKind::Generic,
            "ARCHIVE_READ_FAILED",
            format!("cannot read archive: {e}"),
        ),
        PublishError::Network(e) => CliError::new(
            ExitKind::Generic,
            "NETWORK_ERROR",
            format!("network error: {e}"),
        ),
        PublishError::Malformed(e) => CliError::new(
            ExitKind::Generic,
            "REGISTRY_MALFORMED_RESPONSE",
            format!("registry sent an unparseable success response: {e}"),
        ),
        PublishError::Raw { status, text } => CliError::new(
            ExitKind::Generic,
            "REGISTRY_ERROR",
            format!("registry returned {status}: {text}"),
        ),
        PublishError::Api { status, envelope } => map_api_error(status, envelope),
    }
}

fn map_api_error(status: reqwest::StatusCode, envelope: ApiErrorBody) -> CliError {
    let kind = match status.as_u16() {
        400 => ExitKind::Validation,
        401 | 403 => ExitKind::Generic,
        404 => ExitKind::NotFound,
        410 => ExitKind::Generic,
        413 | 429 => ExitKind::Generic,
        _ => ExitKind::Generic,
    };
    let code: &'static str = match status.as_u16() {
        400 => "REGISTRY_VALIDATION_FAILED",
        401 => "NOT_AUTHENTICATED",
        403 => "FORBIDDEN",
        404 => "REGISTRY_NOT_FOUND",
        410 => "GONE",
        413 => "ARCHIVE_TOO_LARGE",
        429 => "RATE_LIMITED",
        _ => "REGISTRY_ERROR",
    };

    let mut msg = match status.as_u16() {
        400 => {
            let variant = envelope
                .variant
                .clone()
                .unwrap_or_else(|| "ValidationFailed".to_string());
            let detail = envelope
                .detail
                .clone()
                .unwrap_or_else(|| "validation failed (no detail)".to_string());
            if let Some(path) = envelope.path.as_deref() {
                format!("{variant} at {path}: {detail}")
            } else {
                format!("{variant}: {detail}")
            }
        }
        401 => {
            "unauthorized — set MEMSTEAD_TOKEN, run `memstead login`, or pass --token".to_string()
        }
        403 => envelope
            .detail
            .clone()
            .map(|d| format!("forbidden: {d}"))
            .unwrap_or_else(|| "forbidden".to_string()),
        404 => "registry returned 404 — is the URL correct?".to_string(),
        410 => envelope
            .detail
            .clone()
            .map(|d| format!("gone: {d}"))
            .unwrap_or_else(|| "content is gone (taken down or deny-listed)".to_string()),
        413 => "archive exceeds the 2 MB publisher cap".to_string(),
        429 => {
            let retry = envelope.retry_after_seconds.unwrap_or(0);
            if retry > 0 {
                format!("rate-limited — retry after {retry}s")
            } else {
                "rate-limited".to_string()
            }
        }
        _ => envelope
            .detail
            .clone()
            .unwrap_or_else(|| format!("registry returned {status}")),
    };

    // Preserve the error discriminator so programmatic callers can
    // still see the wire `error` string.
    if !envelope.error.is_empty() && !msg.to_ascii_lowercase().contains(&envelope.error) {
        msg = format!("{msg} [{}]", envelope.error);
    }

    CliError::new(kind, code, msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use memstead_base::filesystem::config::{WorkspaceConfig, write_workspace_config};
    use memstead_schema::SchemaRef;
    use tempfile::TempDir;

    fn write_publishable_workspace(tmp: &TempDir, name: &str) {
        // Lay down the post-rebuild marker so the publish command's
        // walk-up resolves.
        let memstead_dir = tmp.path().join(".memstead");
        std::fs::create_dir_all(&memstead_dir).unwrap();
        std::fs::write(
            memstead_dir.join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        // Round-trip via serde so the test does not need a direct
        // `semver` dependency. The workspace-config writer accepts
        // an optional `version` field; we slot it in by serialising
        // a JSON value that matches the on-disk schema.
        let pin: SchemaRef = "default@1.0.0".parse().unwrap();
        let cfg = WorkspaceConfig::new(name, pin);
        write_workspace_config(tmp.path(), &cfg).unwrap();
        // Patch in the version field by re-reading + re-writing the
        // raw JSON. Avoids the test having to depend on `semver`
        // directly; the `to_published()` path needs `version` set.
        let cfg_path = tmp.path().join(".memstead").join("config.json");
        let raw = std::fs::read_to_string(&cfg_path).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        value["version"] = serde_json::json!("0.1.0");
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }

    /// Spin up an axum fixture that accepts `POST /api/publish` and
    /// echoes a success body. The body is captured so the test can
    /// assert it is a non-empty zip-shaped buffer (zip magic
    /// `PK\x03\x04`).
    async fn spawn_fixture_publish_registry() -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
        tokio::task::JoinHandle<()>,
    ) {
        use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
        use std::sync::{Arc, Mutex};

        let captured: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = captured.clone();
        let app: Router = Router::new()
            .route(
                "/api/publish",
                post(
                    move |State(buf): State<Arc<Mutex<Vec<u8>>>>, body: axum::body::Bytes| async move {
                        *buf.lock().unwrap() = body.to_vec();
                        (
                            StatusCode::OK,
                            Json(serde_json::json!({
                                "ok": true,
                                "scope": "fixture",
                                "name": "demo",
                                "version": "0.1.0",
                                "url": "/v/fixture/demo",
                            })),
                        )
                    },
                ),
            )
            .with_state(captured_clone);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), captured, handle)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_assembles_from_workspace_when_no_archive_arg() {
        let tmp = TempDir::new().unwrap();
        write_publishable_workspace(&tmp, "demo");

        let (base, captured, handle) = spawn_fixture_publish_registry().await;

        let workspace = tmp.path().to_path_buf();
        let base_clone = base.clone();
        let captured_clone = captured.clone();
        let result = tokio::task::spawn_blocking(move || {
            let ctx = CliContext {
                json: false,
                quiet: false,
                role: Default::default(),
            };
            run_with_root(
                &ctx,
                Args {
                    archive: None,
                    mem: None,
                    scope: None,
                    version: None,
                    dry_run: false,
                    redact_anchors: false,
                    token: Some("fixture-token".to_string()),
                    registry: Some(base_clone),
                },
                Some(workspace),
            )?;
            let body = captured_clone.lock().unwrap().clone();
            Ok::<Vec<u8>, anyhow::Error>(body)
        })
        .await
        .unwrap();
        handle.abort();
        let body = result.unwrap();

        // Body must be a non-empty zip buffer.
        assert!(body.len() > 4);
        assert_eq!(&body[0..4], b"PK\x03\x04");
    }

    /// The marker validation for workspace overrides lives on the ROOT
    /// command now (global `--workspace` / `MEMSTEAD_WORKSPACE`,
    /// refused before dispatch naming the tried path — covered by
    /// `read_commands::workspace_override_flag_env_precedence_and_refusal`).
    /// Through the in-process test seam a marker-less root still fails
    /// loudly downstream rather than publishing garbage.
    #[test]
    fn publish_errors_when_root_lacks_workspace_shape() {
        let tmp = TempDir::new().unwrap();
        // No `.memstead/workspace.toml` under tmp.
        let ctx = CliContext {
            json: false,
            quiet: false,
            role: Default::default(),
        };
        let err = run_with_root(
            &ctx,
            Args {
                archive: None,
                mem: None,
                scope: None,
                version: None,
                dry_run: false,
                redact_anchors: false,
                token: Some("fixture-token".to_string()),
                registry: Some("http://127.0.0.1:1".to_string()),
            },
            Some(tmp.path().to_path_buf()),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(!msg.is_empty(), "marker-less root must error, got: {msg}");
    }

    #[test]
    fn publish_mem_flag_routes_through_engine_and_maps_unknown_mem() {
        // `--mem NAME` must take the engine export-to-bytes branch
        // (not the bare folder assembly): it opens the workspace engine
        // and asks it to export the named mem. A name the workspace
        // does not carry surfaces the engine's typed `UNKNOWN_MEM`
        // through `from_engine_op` rather than a folder-assembly error —
        // proof the new dispatch reaches the engine path. The happy
        // path (a real mem → zip bytes) reuses the same
        // `export_mem_to_bytes` primitive that `memstead export
        // --format mem` exercises under test.
        let tmp = TempDir::new().unwrap();
        write_publishable_workspace(&tmp, "demo");
        let ctx = CliContext {
            json: false,
            quiet: false,
            role: Default::default(),
        };
        let err = run_with_root(
            &ctx,
            Args {
                archive: None,
                mem: Some("nonexistent".to_string()),
                scope: None,
                version: None,
                dry_run: false,
                redact_anchors: false,
                token: Some("fixture-token".to_string()),
                registry: Some("http://127.0.0.1:1".to_string()),
            },
            Some(tmp.path().to_path_buf()),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unknown mem") || msg.contains("nonexistent"),
            "expected an engine UNKNOWN_MEM error from the --mem path, got: {msg}"
        );
    }

    #[test]
    fn publish_version_without_mem_is_rejected_before_any_io() {
        // `--version` persists a bump through the workspace engine, so
        // it is meaningless without `--mem` — and must refuse up front
        // (no workspace touched, no network) with an actionable message.
        let ctx = CliContext {
            json: false,
            quiet: false,
            role: Default::default(),
        };
        let err = run(
            &ctx,
            Args {
                archive: None,
                mem: None,
                scope: None,
                version: Some("0.2.0".to_string()),
                dry_run: false,
                redact_anchors: false,
                token: None,
                registry: Some("http://127.0.0.1:1".to_string()),
            },
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("--version requires --mem"),
            "expected a --version-requires-mem refusal, got: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dry_run_posts_nothing() {
        // `--dry-run` resolves the archive but must not hit the
        // registry: the fixture captures the request body, and it stays
        // empty because no POST is made.
        let tmp = TempDir::new().unwrap();
        write_publishable_workspace(&tmp, "demo");

        let (base, captured, handle) = spawn_fixture_publish_registry().await;

        let workspace = tmp.path().to_path_buf();
        let base_clone = base.clone();
        let result = tokio::task::spawn_blocking(move || {
            let ctx = CliContext {
                json: false,
                quiet: false,
                role: Default::default(),
            };
            run_with_root(
                &ctx,
                Args {
                    archive: None,
                    mem: None,
                    scope: None,
                    version: None,
                    dry_run: true,
                    redact_anchors: false,
                    token: None,
                    registry: Some(base_clone),
                },
                Some(workspace),
            )?;
            Ok::<(), anyhow::Error>(())
        })
        .await
        .unwrap();
        handle.abort();
        result.unwrap();

        assert!(
            captured.lock().unwrap().is_empty(),
            "dry-run must not POST anything to the registry"
        );
    }

    /// `--redact-anchors` with a pre-built archive PATH refuses typed,
    /// BEFORE any auth or network step — the registry URL points at a
    /// closed port, so reaching the network would surface a different
    /// error than the refusal asserted here. The unflagged pre-built
    /// shape is untouched by the new gate (it proceeds far enough to
    /// hit the file read instead).
    #[test]
    fn redact_anchors_refuses_prebuilt_archive_path() {
        let ctx = CliContext {
            json: false,
            quiet: false,
            role: Default::default(),
        };
        let err = run_with_root(
            &ctx,
            Args {
                archive: Some(PathBuf::from("/nonexistent/some.mem")),
                mem: None,
                scope: None,
                version: None,
                dry_run: false,
                redact_anchors: true,
                token: Some("fixture-token".to_string()),
                registry: Some("http://127.0.0.1:1".to_string()),
            },
            None,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("--redact-anchors") && msg.contains("baked in"),
            "typed refusal names the flag and the alternative: {msg}"
        );
    }

    /// A redacted bare-shape publish POSTs a package whose anchors
    /// sidecar carries the sentinel and not the source path — and the
    /// workspace's own sidecar is untouched afterwards.
    #[tokio::test(flavor = "multi_thread")]
    async fn redacted_publish_posts_sentinel_sidecar() {
        let tmp = TempDir::new().unwrap();
        write_publishable_workspace(&tmp, "demo");
        std::fs::write(
            tmp.path().join("first.md"),
            "---\ntype: spec\n---\n# First\n",
        )
        .unwrap();
        let sidecar_path = tmp.path().join(".memstead").join("anchors.json");
        let local = br#"{"version":1,"entities":{"demo--first":[{"artifact":"src/private.rs","grain":"file","class":"anchored","hash_stability":"stable","hash":"h1"}]}}"#;
        std::fs::write(&sidecar_path, local).unwrap();

        let (base, captured, handle) = spawn_fixture_publish_registry().await;
        let workspace = tmp.path().to_path_buf();
        let base_clone = base.clone();
        let captured_clone = captured.clone();
        let body = tokio::task::spawn_blocking(move || {
            let ctx = CliContext {
                json: false,
                quiet: false,
                role: Default::default(),
            };
            run_with_root(
                &ctx,
                Args {
                    archive: None,
                    mem: None,
                    scope: None,
                    version: None,
                    dry_run: false,
                    redact_anchors: true,
                    token: Some("fixture-token".to_string()),
                    registry: Some(base_clone),
                },
                Some(workspace),
            )?;
            Ok::<Vec<u8>, anyhow::Error>(captured_clone.lock().unwrap().clone())
        })
        .await
        .unwrap()
        .unwrap();
        handle.abort();

        let posted = String::from_utf8_lossy(&body).into_owned();
        assert!(
            posted.contains("[redacted]"),
            "posted package carries the sentinel"
        );
        assert!(
            !posted.contains("src/private.rs"),
            "posted package must not carry the artifact path"
        );
        // Publish-time only: the workspace sidecar still names the source.
        let after = std::fs::read(&sidecar_path).unwrap();
        assert_eq!(after, local, "local sidecar bytes are byte-identical");
    }
}
