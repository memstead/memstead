//! A config write must not revert what a sibling process wrote, on the
//! git-branch backend (consistency-sweep 04/03, criteria 1 and 8).
//!
//! The folder half of this lives in `memstead-base`'s lifecycle tests, where a
//! sibling write is a plain file write. Here it is a second engine over the
//! same mem-repo, which is the shape the reported damage actually had: a
//! long-lived MCP server holding the config it read at boot while a CLI
//! invocation writes the same mem's config underneath it.

use memstead_git_branch::test_support::init_real_mem_repo;
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use tempfile::TempDir;

/// The long-lived engine reads config at boot, a sibling engine changes a
/// different field, and the long-lived engine's next config write keeps it.
///
/// Before the fix each setter serialized its cached struct, so the sibling's
/// description was gone. On this backend the loss was recoverable from branch
/// history, which is exactly why it went unnoticed long enough to be reported
/// twice.
#[test]
fn a_long_lived_engine_does_not_revert_a_siblings_config_write() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);

    // The long-lived engine. It reads config once, here, and holds it.
    let mut long_lived = engine_from_workspace_root(tmp.path()).expect("engine boots");

    // A sibling process sets the description and exits.
    {
        let mut sibling = engine_from_workspace_root(tmp.path()).expect("sibling boots");
        sibling
            .set_mem_description("specs", Some("written by the sibling".to_string()), None)
            .expect("sibling sets description");
    }

    // The long-lived engine sets an unrelated field. Its cached config still
    // has no description; the write must not carry that staleness to disk.
    let outcome = long_lived
        .set_mem_version("specs", semver::Version::new(2, 0, 0), None)
        .expect("version bump succeeds");

    // Criterion 3: the intervention is reported on this operation's response.
    let hint = outcome
        .warnings
        .iter()
        .find(|w| w.code() == "CONFIG_WRITE_INTERVENED")
        .unwrap_or_else(|| {
            panic!(
                "the intervening write must be reported, got: {:?}",
                outcome.warnings
            )
        });
    assert!(
        format!("{hint}").contains("description"),
        "the report names what the sibling changed: {hint}"
    );

    // Criterion 1: and both fields are on disk, read through a fresh engine.
    let fresh = engine_from_workspace_root(tmp.path()).expect("re-boot");
    let config = fresh
        .mem_config_for("specs")
        .expect("specs has a config")
        .clone();
    assert_eq!(
        config.description.as_deref(),
        Some("written by the sibling"),
        "the sibling's description survived the long-lived engine's write"
    );
    assert_eq!(
        config.version,
        Some(semver::Version::new(2, 0, 0)),
        "and the long-lived engine's own change landed"
    );
}
