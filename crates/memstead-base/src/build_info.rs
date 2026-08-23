//! Build identity of the running binary.
//!
//! Between releases every dev build reports the same crate semver, so
//! version-keyed signals — the plan-05 "engine version changed →
//! re-read the tool roster" hint and the plan-02 mutation-stamp /
//! `ENGINE_VERSION_SKEW` comparison — could never fire in dogfood or
//! field use. `build.rs` captures the git commit at build time
//! (`MEMSTEAD_BUILD_SHA`, empty outside a git checkout); this module
//! renders the full build version every version-carrying surface
//! serves: CLI `--version`, both MCP flavours' `serverInfo.version`,
//! the overview's `_engine_version`, and the per-mem mutation stamp.

/// The short git sha of the commit this binary was built from, with a
/// `-dirty` suffix when tracked build inputs (`crates/`, `Cargo.toml`,
/// `Cargo.lock`) were modified at build time.
/// Empty for builds outside a git checkout (crates.io, vendored
/// trees) — emptiness, not absence, is the sha-less signal.
pub const BUILD_SHA: &str = env!("MEMSTEAD_BUILD_SHA");

/// The full build version of the running binary: the bare crate
/// semver ([`crate::ENGINE_VERSION`]) when no build sha exists, else
/// `<semver>+g<sha>[-dirty]` (semver build-metadata syntax, so the
/// value still parses as a `semver::Version`). Two dev builds of the
/// same crate version compare unequal whenever their commits differ —
/// exactly the property the skew stamp and the roster-refresh hint
/// need.
pub fn full_version() -> &'static str {
    static FULL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FULL.get_or_init(|| {
        if BUILD_SHA.is_empty() {
            crate::ENGINE_VERSION.to_string()
        } else {
            format!("{}+g{}", crate::ENGINE_VERSION, BUILD_SHA)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full build version is the bare semver exactly when no sha
    /// was captured, else semver plus `+g<sha>` build metadata — and
    /// either way it parses as a real `semver::Version` whose
    /// version core equals the crate version.
    #[test]
    fn full_version_is_semver_with_optional_build_sha() {
        let full = full_version();
        if BUILD_SHA.is_empty() {
            assert_eq!(full, crate::ENGINE_VERSION);
        } else {
            assert_eq!(
                full,
                format!("{}+g{}", crate::ENGINE_VERSION, BUILD_SHA).as_str()
            );
        }
        let parsed: semver::Version = full.parse().expect("full build version parses as semver");
        let bare: semver::Version = crate::ENGINE_VERSION.parse().unwrap();
        assert_eq!(
            (parsed.major, parsed.minor, parsed.patch),
            (bare.major, bare.minor, bare.patch)
        );
    }
}
