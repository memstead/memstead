//! `memstead login` — run the GitHub Device Flow and persist the resulting
//! token at `~/.config/memstead/credentials` keyed on the registry host.
//!
//! This command is optional — `memstead publish` auto-triggers the same
//! flow on first use. Useful for CI preflight, or users who want to
//! authenticate before their first publish.

use clap::Parser;
use serde_json::json;

use crate::CliError;
use crate::auth::{credentials, device_flow};
use crate::output::{ExitKind, print_json, print_markdown};
use crate::registry;
use crate::setup::CliContext;

#[derive(Parser, Debug)]
pub struct Args {
    /// Registry URL (overrides `MEMSTEAD_REGISTRY`; defaults to https://memstead.io).
    #[arg(long, value_name = "URL")]
    pub registry: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let base = registry::registry_base(args.registry.as_deref());
    let host = registry::registry_host(&base);
    let client = registry::build_http()?;

    let outcome = device_flow::run(
        &client,
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

    // Resolve the GitHub username right after the flow so the stored
    // entry has a display name and we surface "logged in as <login>"
    // on stdout. Failure here is non-fatal — the token still works.
    let user_login = fetch_login(&client, &outcome.access_token).unwrap_or_default();

    let entry = credentials::Entry::new(
        outcome.access_token.clone(),
        user_login.clone(),
        outcome.scopes.clone(),
    );
    credentials::save_for(&host, entry)?;

    if ctx.json {
        print_json(&json!({
            "ok": true,
            "registry": host,
            "user_login": user_login,
            "scopes": outcome.scopes,
        }))?;
    } else {
        let who = if user_login.is_empty() {
            "authorized".to_string()
        } else {
            format!("logged in as {user_login}")
        };
        print_markdown(&format!("# {who}\n\n- Registry: {host}"));
    }

    Ok(())
}

/// Resolve the GitHub username for a token. Uses the env-overridable
/// `MEMSTEAD_GITHUB_API_BASE` so the integration test can point this at a
/// local mock.
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
