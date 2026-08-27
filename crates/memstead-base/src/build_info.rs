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

/// Which way a mem's stamped engine version differs from the running binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkewDirection {
    /// The mem was last written by a NEWER binary than this one. The
    /// interesting direction: this binary may not understand what that one
    /// wrote.
    StampedNewer,
    /// The mem was last written by an OLDER binary than this one.
    StampedOlder,
}

/// The direction of engine-version skew between a mem's stamp and the running
/// binary, or `None` when there is none to report.
///
/// Compared as semver, which ignores build metadata, so two builds of the same
/// release differ in their `+g<sha>` suffix and are NOT skew: the stamp writer
/// still restamps (the sha is provenance worth keeping current) but nobody is
/// told their engine disagrees when it does not (consistency-sweep 04/04,
/// criterion 8). The previous rule was raw string inequality, which called
/// every rebuild between releases a skew.
///
/// `None` also when either side fails to parse. A stamp this binary cannot
/// read is not evidence of a direction, and guessing one would be worse than
/// the silence.
pub fn skew_direction(stamped: &str, running: &str) -> Option<SkewDirection> {
    let (a, b) = (
        stamped.parse::<semver::Version>().ok()?,
        running.parse::<semver::Version>().ok()?,
    );
    match a.cmp_precedence(&b) {
        std::cmp::Ordering::Greater => Some(SkewDirection::StampedNewer),
        std::cmp::Ordering::Less => Some(SkewDirection::StampedOlder),
        std::cmp::Ordering::Equal => None,
    }
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

    /// 04/04, criterion 8. The build-metadata case is the one the old raw
    /// string comparison got wrong: every rebuild between releases read as
    /// skew, which is why the warning was noise on a dogfood workspace.
    #[test]
    fn skew_is_semver_difference_and_never_a_build_hash() {
        use super::{SkewDirection, skew_direction};
        assert_eq!(
            skew_direction("0.11.0", "0.12.0"),
            Some(SkewDirection::StampedOlder)
        );
        assert_eq!(
            skew_direction("0.13.0", "0.12.0"),
            Some(SkewDirection::StampedNewer)
        );
        // Same release, different commit: not skew in either direction.
        assert_eq!(skew_direction("0.12.0+gabc123", "0.12.0+gdef456"), None);
        assert_eq!(skew_direction("0.12.0", "0.12.0+gabc123"), None);
        assert_eq!(skew_direction("0.12.0+gabc123-dirty", "0.12.0"), None);
        // A pre-release IS a semver difference, and the direction is real.
        assert_eq!(
            skew_direction("0.12.0-rc.1", "0.12.0"),
            Some(SkewDirection::StampedOlder)
        );
        // Unparseable: no direction rather than a guessed one.
        assert_eq!(skew_direction("not-a-version", "0.12.0"), None);
        assert_eq!(skew_direction("0.12.0", ""), None);
    }
}
