//! HTTP client for a Memstead registry.
//!
//! Thin wrapper around `reqwest::blocking` that knows the routes the CLI
//! consumes (`POST /api/publish`, `GET /api/mem/...`, `GET
//! /api/auth/config`) plus the typed error envelope (`ApiError`) a
//! registry emits. The CLI knows no registry by name: every registry
//! command needs `--registry <URL>` or `MEMSTEAD_REGISTRY`.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// The decoded wire-level error shape the registry returns on any
/// non-2xx. `variant` is present only for `validation_failed`
/// responses; other error kinds leave it `None`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiErrorBody {
    pub error: String,
    #[serde(default)]
    pub variant: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub retry_after_seconds: Option<i64>,
}

/// Outcome of a successful `/api/publish` POST.
#[derive(Debug, Clone, Deserialize)]
pub struct PublishResponse {
    // Wire field the server sends; deserialised for shape fidelity, not read.
    #[allow(dead_code)]
    pub ok: bool,
    pub scope: String,
    pub name: String,
    pub version: String,
    /// The version that is `current` for the handle after this publish
    /// (highest published, semver). Differs from `version` when an older
    /// version was published while a higher one exists. Absent on older
    /// servers that predate the field.
    #[serde(default)]
    pub current: Option<String>,
    /// Path-only — typically `/v/<scope>/<name>`. Caller composes the
    /// full URL against the registry base.
    pub url: String,
}

/// The registry base URL the caller configured, if any: CLI flag, then
/// `MEMSTEAD_REGISTRY` env. Blank values count as unset. Trailing slashes
/// are stripped so callers can unconditionally append route segments.
pub fn configured_registry(explicit: Option<&str>) -> Option<String> {
    explicit
        .map(str::to_string)
        .or_else(|| std::env::var("MEMSTEAD_REGISTRY").ok())
        .map(|raw| raw.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

/// Resolve the registry base URL: CLI flag, then `MEMSTEAD_REGISTRY` env.
/// There is no built-in default registry; with neither set this refuses
/// with the typed `REGISTRY_NOT_CONFIGURED` code.
pub fn registry_base(explicit: Option<&str>) -> Result<String, crate::CliError> {
    configured_registry(explicit).ok_or_else(|| {
        crate::CliError::new(
            crate::output::ExitKind::Generic,
            "REGISTRY_NOT_CONFIGURED",
            "no registry configured: pass `--registry <URL>` or set `MEMSTEAD_REGISTRY` \
             to the base URL of the Memstead registry you want to use",
        )
    })
}

/// GitHub OAuth settings a registry publishes at `GET /api/auth/config`:
/// the client id of the registry's own GitHub OAuth App (device flow is a
/// public-client protocol, so the id is not a secret) and the scope to
/// request. The CLI carries no client id of its own.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AuthConfig {
    pub github_client_id: String,
    #[serde(default = "default_github_scope")]
    pub github_scope: String,
}

fn default_github_scope() -> String {
    "read:user".to_string()
}

/// Fetch the registry's OAuth settings from `<base>/api/auth/config`.
/// Any failure (unreachable, non-2xx, malformed JSON, empty client id)
/// refuses with the typed `REGISTRY_AUTH_CONFIG_UNAVAILABLE` code.
pub fn fetch_auth_config(
    client: &reqwest::blocking::Client,
    base: &str,
) -> Result<AuthConfig, crate::CliError> {
    let url = format!("{base}/api/auth/config");
    let refuse = |why: String| {
        crate::CliError::new(
            crate::output::ExitKind::Generic,
            "REGISTRY_AUTH_CONFIG_UNAVAILABLE",
            format!(
                "could not read the login configuration from {url}: {why}. Check that \
                 `--registry` / `MEMSTEAD_REGISTRY` points at a Memstead registry that \
                 supports GitHub login"
            ),
        )
    };
    let resp = client
        .get(&url)
        .header("accept", "application/json")
        .send()
        .map_err(|e| refuse(format!("request failed ({e})")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(refuse(format!("the registry answered {status}")));
    }
    let bytes = resp
        .bytes()
        .map_err(|e| refuse(format!("reading the response failed ({e})")))?;
    let cfg: AuthConfig = serde_json::from_slice(&bytes)
        .map_err(|e| refuse(format!("the response is not the expected JSON ({e})")))?;
    if cfg.github_client_id.trim().is_empty() {
        return Err(refuse(
            "the response carries an empty `github_client_id`".into(),
        ));
    }
    let github_scope = if cfg.github_scope.trim().is_empty() {
        default_github_scope()
    } else {
        cfg.github_scope
    };
    Ok(AuthConfig {
        github_client_id: cfg.github_client_id,
        github_scope,
    })
}

/// Extract the hostname for credentials keying. Falls back to the
/// full URL on parse failure so nothing silently collides.
pub fn registry_host(base: &str) -> String {
    base.split_once("://")
        .map_or(base, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or(base)
        .to_ascii_lowercase()
}

/// Shared HTTP client. 30 s timeout is comfortable for the 2 MB cap
/// times a slow upstream — GitHub API is also cheap to tolerate.
///
/// Typed, not `INTERNAL`: the only realistic failure is a TLS backend
/// the host cannot initialise, which is the same "the registry is
/// unreachable from here" class every other transport failure on this
/// path reports.
pub fn build_http() -> Result<reqwest::blocking::Client> {
    // The TLS crypto provider: ring, installed once per process. reqwest
    // is built with `rustls-no-provider` (the alternative, aws-lc, is a C
    // build behind cmake the musl release targets cannot rely on), so
    // the process has to name its provider before the first client is
    // built; a second install is a no-op.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("memstead/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| {
            crate::CliError::new(
                crate::output::ExitKind::Generic,
                "REGISTRY_ERROR",
                format!("could not build the HTTP client used to reach the registry: {e}"),
            )
            .into()
        })
}

/// The publisher-terms version the CLI accepts on publish. Running
/// `memstead publish` is a deliberate act, so the CLI accepts the current
/// terms on the publisher's behalf by sending this version. Must track the
/// registry's `CURRENT_TERMS_VERSION`; a mismatch surfaces as a
/// `terms_not_accepted` refusal naming the version, telling the user to update.
pub const ACCEPTED_TERMS_VERSION: &str = "1.0";

/// A per-publish domain-authority signature, presented in request headers for a
/// `<domain>:<handle>` publish. The CLI builds this from the domain's stored key
/// (see `crate::auth::domain_key`); the registry verifies it against the hosted
/// proof manifest.
#[derive(Debug, Clone)]
pub struct DomainSignature {
    /// `ed25519:<base64>` public key the signature was made with.
    pub key: String,
    /// `ed25519:<base64>` signature over the canonical publish payload.
    pub signature: String,
    /// Publish timestamp, unix seconds.
    pub timestamp: i64,
}

/// POST a sealed `.mem` archive to `/api/publish`. Returns the parsed
/// success body or a typed `ApiErrorBody` on any non-2xx.
///
/// Authorisation is one of two channels: a GitHub `token` (the default path),
/// or a `domain_sig` for a `<domain>:<handle>` publish, which needs no GitHub
/// account. Exactly one should be supplied for a given publish.
pub fn publish(
    client: &reqwest::blocking::Client,
    base: &str,
    archive: &Path,
    token: Option<&str>,
    scope_override: Option<&str>,
    domain_sig: Option<&DomainSignature>,
) -> Result<PublishResponse, PublishError> {
    use memstead_base::domain_authority_wire::{HEADER_KEY, HEADER_SIGNATURE, HEADER_TIMESTAMP};

    let url = format!("{base}/api/publish");
    let mut file = std::fs::File::open(archive).map_err(PublishError::Io)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(PublishError::Io)?;

    let mut req = client
        .post(&url)
        .header("content-type", "application/octet-stream")
        .header("x-memstead-accept-terms", ACCEPTED_TERMS_VERSION)
        .body(bytes);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    if let Some(s) = scope_override {
        req = req.header("x-memstead-scope", s);
    }
    if let Some(ds) = domain_sig {
        req = req
            .header(HEADER_KEY, &ds.key)
            .header(HEADER_SIGNATURE, &ds.signature)
            .header(HEADER_TIMESTAMP, ds.timestamp.to_string());
    }

    let resp = req.send().map_err(PublishError::Network)?;
    let status = resp.status();
    let body_bytes = resp.bytes().map_err(PublishError::Network)?;

    if status.is_success() {
        return serde_json::from_slice::<PublishResponse>(&body_bytes)
            .map_err(|e| PublishError::Malformed(e.to_string()));
    }

    // Non-2xx: try the typed envelope, fall back to raw text.
    match serde_json::from_slice::<ApiErrorBody>(&body_bytes) {
        Ok(envelope) => Err(PublishError::Api { status, envelope }),
        Err(_) => {
            let text = String::from_utf8_lossy(&body_bytes).into_owned();
            Err(PublishError::Raw { status, text })
        }
    }
}

/// Outcome of a successful `DELETE /api/mem/<scope>/<name>`.
#[derive(Debug, Clone, Deserialize)]
pub struct UnpublishResponse {
    // Wire field the server sends; deserialised for shape fidelity, not read.
    #[allow(dead_code)]
    pub ok: bool,
    pub scope: String,
    pub name: String,
}

/// DELETE a mem from the registry. Same auth + error envelope as
/// publish, so reuse `PublishError` for the failure shape.
pub fn unpublish(
    client: &reqwest::blocking::Client,
    base: &str,
    scope: &str,
    name: &str,
    token: &str,
) -> Result<UnpublishResponse, PublishError> {
    let url = format!(
        "{base}/api/mem/{scope}/{name}",
        scope = url_segment(scope),
        name = url_segment(name),
    );
    let resp = client
        .delete(&url)
        .bearer_auth(token)
        .send()
        .map_err(PublishError::Network)?;
    let status = resp.status();
    let body_bytes = resp.bytes().map_err(PublishError::Network)?;

    if status.is_success() {
        return serde_json::from_slice::<UnpublishResponse>(&body_bytes)
            .map_err(|e| PublishError::Malformed(e.to_string()));
    }

    match serde_json::from_slice::<ApiErrorBody>(&body_bytes) {
        Ok(envelope) => Err(PublishError::Api { status, envelope }),
        Err(_) => {
            let text = String::from_utf8_lossy(&body_bytes).into_owned();
            Err(PublishError::Raw { status, text })
        }
    }
}

/// Admin-only takedown of a published mem: same `DELETE` route as
/// `unpublish`, but with the `x-memstead-takedown` header carrying the
/// statement-of-reasons notice reference. The server selects the
/// takedown path (deny-list the bytes, tombstone, burn the name)
/// instead of an ordinary hard-delete, and refuses non-admins with 403.
#[cfg(feature = "registry-ops")]
pub fn admin_takedown(
    client: &reqwest::blocking::Client,
    base: &str,
    scope: &str,
    name: &str,
    notice: &str,
    token: &str,
) -> Result<UnpublishResponse, PublishError> {
    let url = format!(
        "{base}/api/mem/{scope}/{name}",
        scope = url_segment(scope),
        name = url_segment(name),
    );
    let resp = client
        .delete(&url)
        .bearer_auth(token)
        .header("x-memstead-takedown", notice)
        .send()
        .map_err(PublishError::Network)?;
    let status = resp.status();
    let body_bytes = resp.bytes().map_err(PublishError::Network)?;

    if status.is_success() {
        return serde_json::from_slice::<UnpublishResponse>(&body_bytes)
            .map_err(|e| PublishError::Malformed(e.to_string()));
    }
    match serde_json::from_slice::<ApiErrorBody>(&body_bytes) {
        Ok(envelope) => Err(PublishError::Api { status, envelope }),
        Err(_) => {
            let text = String::from_utf8_lossy(&body_bytes).into_owned();
            Err(PublishError::Raw { status, text })
        }
    }
}

/// Outcome of a successful `POST /api/admin/denylist`.
#[cfg(feature = "registry-ops")]
#[derive(Debug, Clone, Deserialize)]
pub struct DenylistResponse {
    // Wire field the server sends; deserialised for shape fidelity, not read.
    #[allow(dead_code)]
    pub ok: bool,
    pub content_sha256: String,
}

/// Admin-only: add a canonical-bytes SHA-256 to the content deny-list so
/// those exact bytes can never be published. Refuses non-admins with 403.
#[cfg(feature = "registry-ops")]
pub fn admin_denylist(
    client: &reqwest::blocking::Client,
    base: &str,
    content_sha256: &str,
    reason: Option<&str>,
    token: &str,
) -> Result<DenylistResponse, PublishError> {
    let url = format!("{base}/api/admin/denylist");
    let resp = client
        .post(&url)
        .bearer_auth(token)
        .json(&serde_json::json!({ "content_sha256": content_sha256, "reason": reason }))
        .send()
        .map_err(PublishError::Network)?;
    let status = resp.status();
    let body_bytes = resp.bytes().map_err(PublishError::Network)?;

    if status.is_success() {
        return serde_json::from_slice::<DenylistResponse>(&body_bytes)
            .map_err(|e| PublishError::Malformed(e.to_string()));
    }
    match serde_json::from_slice::<ApiErrorBody>(&body_bytes) {
        Ok(envelope) => Err(PublishError::Api { status, envelope }),
        Err(_) => {
            let text = String::from_utf8_lossy(&body_bytes).into_owned();
            Err(PublishError::Raw { status, text })
        }
    }
}

/// GET a sealed `.mem` archive from the registry, streaming into
/// `dest_path`. Returns the number of bytes written.
pub fn download_mem(
    client: &reqwest::blocking::Client,
    base: &str,
    scope: &str,
    name: &str,
    dest_path: &Path,
) -> Result<u64, DownloadError> {
    let url = format!(
        "{base}/api/mem/{scope}/{name}.mem",
        scope = url_segment(scope),
        name = url_segment(name),
    );
    let resp = client.get(&url).send().map_err(DownloadError::Network)?;
    let status = resp.status();
    if !status.is_success() {
        return match status.as_u16() {
            404 => Err(DownloadError::NotFound),
            410 => Err(DownloadError::Gone),
            _ => {
                let text = resp.text().unwrap_or_default();
                Err(DownloadError::Http {
                    status,
                    text: text.chars().take(500).collect(),
                })
            }
        };
    }
    let bytes = resp.bytes().map_err(DownloadError::Network)?;
    std::fs::write(dest_path, &bytes).map_err(DownloadError::Io)?;
    Ok(bytes.len() as u64)
}

/// Minimal percent-encoding for a single path segment. Our scope +
/// name are slug-safe by server validation (`^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$`),
/// so a character set check is sufficient — the server would 400 any
/// non-slug anyway. Kept as a fn for explicitness.
fn url_segment(raw: &str) -> String {
    // Preserve the scope-form characters `:` (scheme/domain separator) and
    // `.` (domain labels) — both valid in a URL path segment — alongside the
    // slug characters; drop anything else as a path-shape defence.
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '-' | '_' | ':' | '.'))
        .collect()
}

/// Parse a registry ref `<scope>/<name>` in one of the three scope forms
/// (`github:<handle>/<name>`, `<domain>:<handle>/<name>`, or a bare
/// `<handle>/<name>`). The legacy `@scope/name` syntax is rejected by the
/// caller before this is reached. Returns `None` for anything that is not a
/// valid registry ref (e.g. a local file path), so `install` can fall back to
/// a local install.
pub fn parse_ref(raw: &str) -> Option<(String, String)> {
    let (scope, name) = raw.split_once('/')?;
    // The name is a bare slug — no extension, no further path segments.
    if name.is_empty() || name.contains('.') || name.contains('/') || name.contains('\\') {
        return None;
    }
    if !is_valid_scope_form(scope) {
        return None;
    }
    Some((scope.to_string(), name.to_string()))
}

fn is_valid_handle(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 39
        && !h.starts_with('-')
        && !h.ends_with('-')
        && h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// `github:<handle>`, `<domain>:<handle>` (domain has a `.`), or bare `<handle>`.
fn is_valid_scope_form(scope: &str) -> bool {
    match scope.split_once(':') {
        Some((prefix, handle)) => {
            is_valid_handle(handle)
                && (prefix == "github"
                    || (prefix.contains('.')
                        && prefix.split('.').all(|label| {
                            !label.is_empty()
                                && label
                                    .bytes()
                                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                        })))
        }
        None => is_valid_handle(scope),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("network: {0}")]
    Network(reqwest::Error),
    #[error("registry returned {status}: {envelope:?}")]
    Api {
        status: reqwest::StatusCode,
        envelope: ApiErrorBody,
    },
    #[error("registry returned {status}: {text}")]
    Raw {
        status: reqwest::StatusCode,
        text: String,
    },
    #[error("malformed success response: {0}")]
    Malformed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("network: {0}")]
    Network(reqwest::Error),
    #[error("not found")]
    NotFound,
    #[error("content taken down")]
    Gone,
    #[error("registry returned {status}: {text}")]
    Http {
        status: reqwest::StatusCode,
        text: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_registry_is_trimmed_of_trailing_slashes() {
        assert_eq!(
            configured_registry(Some("https://registry.example.com/")).as_deref(),
            Some("https://registry.example.com")
        );
    }

    /// A blank explicit value counts as unset and, with no fallback to a
    /// built-in host, refuses typed. The explicit flag shadows the env
    /// variable, so the assertion holds whatever the environment carries.
    #[test]
    fn blank_registry_refuses_with_typed_code() {
        let err = registry_base(Some("  ")).unwrap_err();
        assert_eq!(err.code, "REGISTRY_NOT_CONFIGURED");
        assert!(
            err.message.contains("--registry") && err.message.contains("MEMSTEAD_REGISTRY"),
            "the refusal names both ways to configure a registry: {}",
            err.message
        );
    }

    /// Serve `body` with `status` at `GET /api/auth/config`; returns the
    /// base URL and the server task.
    async fn spawn_auth_config(
        status: u16,
        body: &'static str,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use axum::{Router, http::StatusCode, routing::get};
        let app: Router = Router::new().route(
            "/api/auth/config",
            get(move || async move {
                (
                    StatusCode::from_u16(status).unwrap(),
                    [("content-type", "application/json")],
                    body,
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), handle)
    }

    async fn fetch(base: String) -> Result<AuthConfig, crate::CliError> {
        tokio::task::spawn_blocking(move || {
            let client = build_http().unwrap();
            fetch_auth_config(&client, &base)
        })
        .await
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn auth_config_reads_client_id_and_scope() {
        let (base, handle) = spawn_auth_config(
            200,
            r#"{"github_client_id":"fixture-client","github_scope":"read:user user:email"}"#,
        )
        .await;
        let cfg = fetch(base).await.unwrap();
        handle.abort();
        assert_eq!(cfg.github_client_id, "fixture-client");
        assert_eq!(cfg.github_scope, "read:user user:email");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn auth_config_scope_defaults_to_read_user() {
        let (base, handle) =
            spawn_auth_config(200, r#"{"github_client_id":"fixture-client"}"#).await;
        let cfg = fetch(base).await.unwrap();
        handle.abort();
        assert_eq!(cfg.github_scope, "read:user");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn auth_config_refuses_malformed_missing_and_empty() {
        for (status, body) in [
            (200, "not json"),
            (200, r#"{"github_scope":"read:user"}"#),
            (200, r#"{"github_client_id":"  "}"#),
            (404, r#"{"error":"not_found"}"#),
        ] {
            let (base, handle) = spawn_auth_config(status, body).await;
            let err = fetch(base).await.unwrap_err();
            handle.abort();
            assert_eq!(
                err.code, "REGISTRY_AUTH_CONFIG_UNAVAILABLE",
                "{status} {body}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn auth_config_refuses_unreachable_registry() {
        let err = fetch("http://127.0.0.1:1".to_string()).await.unwrap_err();
        assert_eq!(err.code, "REGISTRY_AUTH_CONFIG_UNAVAILABLE");
    }
}
