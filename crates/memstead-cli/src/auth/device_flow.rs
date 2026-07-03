//! GitHub OAuth Device Flow client.
//!
//! Reference: <https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow>
//!
//! Three steps:
//!
//! 1. `POST https://github.com/login/device/code` → returns a user
//!    code + device code + verification URI + polling interval.
//! 2. Print the user code, open the verification URI in the browser,
//!    wait for the user to approve.
//! 3. Poll `POST https://github.com/login/oauth/access_token` at the
//!    advertised interval until the server either returns an access
//!    token, a terminal error (`access_denied`, `expired_token`), or
//!    we exceed the code expiry.
//!
//! Device Flow is a public-client protocol — no client secret exists.
//! The client ID is the one registered for the registry's GitHub OAuth
//! App (operator prerequisites: `dev/registry-operations.md`).

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// GitHub OAuth App client ID for memstead.io. Public; safe to commit.
pub const MEMSTEAD_GITHUB_CLIENT_ID: &str = "Ov23linvCi8kvFipqMHh";

/// Scope requested at authorization time. `read:user` is the minimum
/// the registry needs to resolve the username; asking for more would
/// scare users off in the GitHub approval screen.
pub const MEMSTEAD_GITHUB_SCOPE: &str = "read:user";

/// Where `GET /user` lives in `crate::auth::device_flow` — used by
/// tests that stand up a GitHub mock so the device flow doesn't hit
/// real github.com.
const GITHUB_HOST_DEFAULT: &str = "https://github.com";

fn github_host() -> String {
    std::env::var("MEMSTEAD_GITHUB_HOST").unwrap_or_else(|_| GITHUB_HOST_DEFAULT.to_string())
}

#[derive(Debug, Clone, Serialize)]
struct DeviceCodeRequest<'a> {
    client_id: &'a str,
    scope: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds until `device_code` expires.
    pub expires_in: u64,
    /// Minimum seconds between polling attempts.
    pub interval: u64,
}

#[derive(Debug, Clone, Serialize)]
struct TokenRequest<'a> {
    client_id: &'a str,
    device_code: &'a str,
    grant_type: &'static str,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum TokenResponse {
    Success {
        access_token: String,
        #[serde(default)]
        scope: String,
        #[serde(default)]
        #[allow(dead_code)]
        token_type: String,
    },
    Error {
        error: String,
        #[serde(default)]
        #[allow(dead_code)]
        error_description: String,
    },
}

/// Result of a successful flow.
pub struct DeviceFlowOutcome {
    pub access_token: String,
    pub scopes: Vec<String>,
}

/// Run the device flow end-to-end against the host in
/// `MEMSTEAD_GITHUB_HOST` (default `https://github.com`). Stdout receives
/// the user-facing prompt; stderr is left alone so CI logs stay clean.
///
/// The `on_open` closure is called once with the verification URI so
/// the caller can decide whether to try opening a browser (interactive
/// publish) or stay text-only (CI preflight). It receives the URI by
/// reference and returns nothing — errors are ignored (the printed
/// URL + user code remain actionable).
pub fn run(
    client: &reqwest::blocking::Client,
    client_id: &str,
    scope: &str,
    on_open: impl FnOnce(&str),
) -> Result<DeviceFlowOutcome> {
    let base = github_host();
    let code = request_device_code(client, &base, client_id, scope)?;

    println!();
    println!("To authorize memstead, open");
    println!("  {}", code.verification_uri);
    println!("and enter the code");
    println!();
    println!("    {}", code.user_code);
    println!();
    println!("Waiting for authorization (Ctrl-C to abort)…");
    std::io::stdout().flush().ok();

    on_open(&code.verification_uri);

    poll_for_token(client, &base, client_id, &code)
}

fn request_device_code(
    client: &reqwest::blocking::Client,
    base: &str,
    client_id: &str,
    scope: &str,
) -> Result<DeviceCodeResponse> {
    let url = format!("{}/login/device/code", base.trim_end_matches('/'));
    let resp = client
        .post(url)
        .header("accept", "application/json")
        .form(&DeviceCodeRequest { client_id, scope })
        .send()
        .context("requesting device code from GitHub")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        anyhow::bail!(
            "GitHub rejected the device-code request ({status}): {}",
            body.chars().take(200).collect::<String>()
        );
    }
    resp.json::<DeviceCodeResponse>()
        .context("parsing device-code response")
}

fn poll_for_token(
    client: &reqwest::blocking::Client,
    base: &str,
    client_id: &str,
    code: &DeviceCodeResponse,
) -> Result<DeviceFlowOutcome> {
    let url = format!("{}/login/oauth/access_token", base.trim_end_matches('/'));
    let deadline = Instant::now() + Duration::from_secs(code.expires_in);
    let mut interval = Duration::from_secs(code.interval.max(1));

    loop {
        if Instant::now() >= deadline {
            anyhow::bail!(
                "device code expired before approval — rerun `memstead login` or `memstead publish`"
            );
        }
        std::thread::sleep(interval);

        let body = TokenRequest {
            client_id,
            device_code: &code.device_code,
            grant_type: "urn:ietf:params:oauth:grant-type:device_code",
        };

        let resp = client
            .post(&url)
            .header("accept", "application/json")
            .form(&body)
            .send()
            .context("polling GitHub for access token")?;

        if !resp.status().is_success() {
            // GitHub normally returns 200 with an error body; a 4xx/5xx
            // here is a real protocol failure, not a pending approval.
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            anyhow::bail!(
                "GitHub returned {status} while polling for token: {}",
                text.chars().take(200).collect::<String>()
            );
        }

        let parsed: TokenResponse = resp.json().context("parsing token response")?;
        match parsed {
            TokenResponse::Success {
                access_token,
                scope,
                ..
            } => {
                let scopes: Vec<String> = scope
                    .split([',', ' '])
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                return Ok(DeviceFlowOutcome {
                    access_token,
                    scopes,
                });
            }
            TokenResponse::Error { error, .. } => match error.as_str() {
                // Still waiting — expected.
                "authorization_pending" => {}
                // Polled too fast; honour the slowdown hint.
                "slow_down" => {
                    interval += Duration::from_secs(5);
                }
                "expired_token" => {
                    anyhow::bail!("device code expired before approval — rerun `memstead login`")
                }
                "access_denied" => {
                    anyhow::bail!("authorization was denied on GitHub")
                }
                "unsupported_grant_type" => anyhow::bail!(
                    "GitHub rejected the device-flow grant — the OAuth App may \
                     not have Device Flow enabled"
                ),
                other => anyhow::bail!("unexpected device-flow error from GitHub: {other}"),
            },
        }
    }
}

/// Best-effort browser open. Returns whether the open-command launched
/// without immediate failure. Never panics; the printed URL + code
/// remain actionable if this fails.
pub fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let launcher = ("open", vec![url]);
    #[cfg(target_os = "linux")]
    let launcher = ("xdg-open", vec![url]);
    #[cfg(target_os = "windows")]
    let launcher = ("cmd", vec!["/C", "start", "", url]);
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let launcher: (&str, Vec<&str>) = ("", vec![]);

    if launcher.0.is_empty() {
        return false;
    }

    std::process::Command::new(launcher.0)
        .args(&launcher.1)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}
