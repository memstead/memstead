//! Auth plumbing for the registry commands.
//!
//! Token resolution (first hit wins) — the policy is centralised in
//! [`resolve_token`] so `publish`, `install` (future auth), and
//! `login` all read from the same ladder.

pub mod credentials;
pub mod device_flow;
pub mod domain_key;

/// Outcome of resolving a token for a given registry host.
pub struct ResolvedToken {
    pub token: String,
}

/// Resolve a token without prompting. Returns `Ok(None)` when no token
/// is available — the caller decides whether to trigger device flow,
/// fail, or proceed without auth (install doesn't need one).
pub fn resolve_token(
    registry_host: &str,
    flag_token: Option<&str>,
) -> anyhow::Result<Option<ResolvedToken>> {
    if let Some(token) = flag_token {
        return Ok(Some(ResolvedToken {
            token: token.to_string(),
        }));
    }
    if let Ok(token) = std::env::var("MEMSTEAD_TOKEN")
        && !token.is_empty()
    {
        return Ok(Some(ResolvedToken { token }));
    }
    if let Some(entry) = credentials::load_for(registry_host)? {
        return Ok(Some(ResolvedToken { token: entry.token }));
    }
    Ok(None)
}
