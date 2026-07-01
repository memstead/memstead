//! `memstead unpublish <scope>/<name>` — remove a vault from the registry.
//!
//! Permitted to the original uploader and to admins. Hard delete:
//! the same `<scope>/<name>` becomes immediately re-publishable.
//! Auth via the same token ladder as `publish`, except first-use
//! Device Flow is intentionally NOT triggered — unpublish is
//! destructive, and silently opening a browser when the user hasn't
//! logged in feels wrong. Tell them to `memstead login` first.

use clap::Parser;
use serde_json::json;

use crate::CliError;
use crate::auth::resolve_token;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::registry::{self, ApiErrorBody, PublishError, UnpublishResponse};
use crate::setup::CliContext;

#[derive(Parser, Debug)]
pub struct Args {
    /// `<scope>/<name>` of the vault to unpublish.
    #[arg(value_name = "SCOPE/NAME")]
    pub target: String,

    /// Explicit token override. Takes precedence over `MEMSTEAD_TOKEN`
    /// and stored credentials.
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,

    /// Registry URL (overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io).
    #[arg(long, value_name = "URL")]
    pub registry: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let (scope, name) = registry::parse_ref(&args.target).ok_or_else(|| {
        CliError::new(
            ExitKind::Generic,
            "INVALID_INPUT",
            format!("expected `<scope>/<name>`, got `{}`", args.target),
        )
    })?;

    let base = registry::registry_base(args.registry.as_deref());
    let host = registry::registry_host(&base);
    let client = registry::build_http()?;

    let token = match resolve_token(&host, args.token.as_deref())? {
        Some(r) => r.token,
        None => {
            return Err(CliError::new(
                ExitKind::Generic,
                "NOT_AUTHENTICATED",
                "not logged in — run `memstead login` or set MEMSTEAD_TOKEN \
                 (unpublish does not auto-trigger Device Flow)",
            )
            .into());
        }
    };

    match registry::unpublish(&client, &base, &scope, &name, &token) {
        Ok(resp) => emit_success(ctx, &resp),
        Err(e) => Err(map_unpublish_error(e).into()),
    }
}

fn emit_success(ctx: &CliContext, resp: &UnpublishResponse) -> anyhow::Result<()> {
    if ctx.json {
        print_json(&json!({
            "ok": true,
            "scope": resp.scope,
            "name": resp.name,
        }))?;
    } else {
        print_markdown(&format!(
            "# Unpublished {}/{}\n\n- The same {}/{} can be re-published immediately.",
            resp.scope, resp.name, resp.scope, resp.name,
        ));
    }
    Ok(())
}

fn map_unpublish_error(err: PublishError) -> CliError {
    match err {
        PublishError::Io(e) => CliError::new(ExitKind::Generic, crate::INTERNAL_CODE, format!("io: {e}")),
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
        401 | 403 => ExitKind::Generic,
        404 => ExitKind::NotFound,
        _ => ExitKind::Generic,
    };
    let code: &'static str = match status.as_u16() {
        401 => "NOT_AUTHENTICATED",
        403 => "FORBIDDEN",
        404 => "REGISTRY_NOT_FOUND",
        _ => "REGISTRY_ERROR",
    };

    let mut msg = match status.as_u16() {
        401 => "unauthorized — set MEMSTEAD_TOKEN, run `memstead login`, or pass --token".to_string(),
        403 => envelope
            .detail
            .clone()
            .map(|d| format!("forbidden: {d}"))
            .unwrap_or_else(|| {
                "forbidden — only the uploader or an admin can unpublish a vault".to_string()
            }),
        404 => "no such vault on the registry".to_string(),
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
