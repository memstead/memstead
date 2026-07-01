//! `memstead admin` — admin-only moderation over the registry's admin
//! endpoints. There is deliberately no web admin panel: moderation runs
//! from the terminal, gated server-side by the `MEMSTEAD_ADMINS`
//! allowlist (the CLI just sends the caller's token; a non-admin gets a
//! 403). Every action is recorded in the registry's append-only audit
//! log. Like `unpublish`, these destructive calls never auto-trigger the
//! Device Flow — authenticate first with `memstead login`.

use clap::{Parser, Subcommand};
use serde_json::json;

use crate::CliError;
use crate::auth::resolve_token;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::registry::{self, ApiErrorBody, PublishError};
use crate::setup::CliContext;

#[derive(Subcommand, Debug)]
pub enum AdminAction {
    /// Take down a published mem (admin-only): deny-list its bytes,
    /// tombstone every version, and burn the `<scope>/<name>` so neither
    /// the bytes nor the name can be re-published. The notice reference
    /// is recorded as the DSA statement-of-reasons in the audit log.
    Takedown(TakedownArgs),

    /// Add a canonical-bytes SHA-256 to the content deny-list (admin-only)
    /// so a publish of exactly those bytes is refused — even before they
    /// are ever uploaded.
    Denylist(DenylistArgs),
}

#[derive(Parser, Debug)]
pub struct TakedownArgs {
    /// `<scope>/<name>` of the mem to take down (e.g. `github:alice/my-mem`).
    #[arg(value_name = "SCOPE/NAME")]
    pub target: String,

    /// Statement-of-reasons / notice reference recorded with the action
    /// (e.g. an abuse-ticket id or legal-notice ref). Required so the
    /// audit log can justify the takedown.
    #[arg(long, value_name = "REF")]
    pub notice: String,

    /// Explicit token override. Takes precedence over `MEMSTEAD_TOKEN`
    /// and stored credentials.
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,

    /// Registry URL (overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io).
    #[arg(long, value_name = "URL")]
    pub registry: Option<String>,
}

#[derive(Parser, Debug)]
pub struct DenylistArgs {
    /// Canonical-bytes SHA-256 (64 hex chars) to block.
    #[arg(value_name = "SHA256")]
    pub sha256: String,

    /// Free-text reason recorded on the deny-list row.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,

    /// Explicit token override. Takes precedence over `MEMSTEAD_TOKEN`
    /// and stored credentials.
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,

    /// Registry URL (overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io).
    #[arg(long, value_name = "URL")]
    pub registry: Option<String>,
}

pub fn run_takedown(ctx: &CliContext, args: TakedownArgs) -> anyhow::Result<()> {
    let (scope, name) = registry::parse_ref(&args.target).ok_or_else(|| {
        CliError::new(
            ExitKind::Generic,
            "INVALID_INPUT",
            format!("expected `<scope>/<name>`, got `{}`", args.target),
        )
    })?;
    if args.notice.trim().is_empty() {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            "--notice must be a non-empty statement-of-reasons reference",
        )
        .into());
    }

    let base = registry::registry_base(args.registry.as_deref());
    let host = registry::registry_host(&base);
    let client = registry::build_http()?;
    let token = resolve_admin_token(&host, args.token.as_deref())?;

    match registry::admin_takedown(&client, &base, &scope, &name, &args.notice, &token) {
        Ok(resp) => {
            if ctx.json {
                print_json(&json!({
                    "ok": true,
                    "action": "takedown",
                    "scope": resp.scope,
                    "name": resp.name,
                    "notice": args.notice,
                }))?;
            } else {
                print_markdown(&format!(
                    "# Took down {}/{}\n\n- Bytes deny-listed, every version tombstoned, name burned.\n- The `{}/{}` name can no longer be re-published.\n- Notice: {}",
                    resp.scope, resp.name, resp.scope, resp.name, args.notice,
                ));
            }
            Ok(())
        }
        Err(e) => Err(map_admin_error(e).into()),
    }
}

pub fn run_denylist(ctx: &CliContext, args: DenylistArgs) -> anyhow::Result<()> {
    let sha = args.sha256.trim().to_ascii_lowercase();
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!("expected a 64-hex-char SHA-256, got `{}`", args.sha256),
        )
        .into());
    }

    let base = registry::registry_base(args.registry.as_deref());
    let host = registry::registry_host(&base);
    let client = registry::build_http()?;
    let token = resolve_admin_token(&host, args.token.as_deref())?;

    match registry::admin_denylist(&client, &base, &sha, args.reason.as_deref(), &token) {
        Ok(resp) => {
            if ctx.json {
                print_json(&json!({
                    "ok": true,
                    "action": "denylist",
                    "content_sha256": resp.content_sha256,
                }))?;
            } else {
                print_markdown(&format!(
                    "# Deny-listed {}\n\n- A publish of these exact bytes is now refused.",
                    resp.content_sha256,
                ));
            }
            Ok(())
        }
        Err(e) => Err(map_admin_error(e).into()),
    }
}

/// Admin actions are destructive — resolve the token without the
/// Device-Flow fallback (same posture as `unpublish`).
fn resolve_admin_token(host: &str, flag: Option<&str>) -> anyhow::Result<String> {
    match resolve_token(host, flag)? {
        Some(r) => Ok(r.token),
        None => Err(CliError::new(
            ExitKind::Generic,
            "NOT_AUTHENTICATED",
            "not logged in — run `memstead login` or set MEMSTEAD_TOKEN \
             (admin actions do not auto-trigger Device Flow)",
        )
        .into()),
    }
}

fn map_admin_error(err: PublishError) -> CliError {
    match err {
        PublishError::Io(e) => {
            CliError::new(ExitKind::Generic, crate::INTERNAL_CODE, format!("io: {e}"))
        }
        PublishError::Network(e) => {
            CliError::new(ExitKind::Generic, "NETWORK_ERROR", format!("network error: {e}"))
        }
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
        PublishError::Api { status, envelope } => map_admin_api_error(status, envelope),
    }
}

fn map_admin_api_error(status: reqwest::StatusCode, envelope: ApiErrorBody) -> CliError {
    let (kind, code): (ExitKind, &'static str) = match status.as_u16() {
        401 => (ExitKind::Generic, "NOT_AUTHENTICATED"),
        403 => (ExitKind::Generic, "FORBIDDEN"),
        404 => (ExitKind::NotFound, "REGISTRY_NOT_FOUND"),
        _ => (ExitKind::Generic, "REGISTRY_ERROR"),
    };
    let mut msg = match status.as_u16() {
        401 => "unauthorized — set MEMSTEAD_TOKEN, run `memstead login`, or pass --token".to_string(),
        403 => "forbidden — this is an admin-only action; your GitHub login is not in MEMSTEAD_ADMINS".to_string(),
        404 => "no such mem on the registry".to_string(),
        _ => envelope
            .detail
            .clone()
            .unwrap_or_else(|| format!("registry returned {status}")),
    };
    if !envelope.error.is_empty() && !msg.to_ascii_lowercase().contains(&envelope.error) {
        msg = format!("{msg} [{}]", envelope.error);
    }
    CliError::new(kind, code, msg)
}
